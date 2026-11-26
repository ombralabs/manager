//! Локальная защита: пароль + TOTP / одноразовый код восстановления, один раз на процесс приложения.
mod crypto;
#[cfg_attr(not(target_os = "macos"), path = "volume_unsupported.rs")]
mod volume;
#[cfg(all(test, target_os = "macos"))]
mod tests;
pub use volume::guard;

use crate::{app::CommandError, paths::Paths, store::local_key};
use crypto::{Key, combined, decode, encode, error, password_key, random, seal, unseal};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Serialize)]
pub struct SecurityStatus {
    pub supported: bool,
    pub configured: bool,
    pub unlocked: bool,
    pub migrating: bool,
    pub recovery_remaining: usize,
    pub retry_after: i64,
    pub recovery_recent: bool,
}

// Нет Debug: секреты возвращаются только в текущую форму настройки, не в события или список настроек.
#[derive(Serialize)]
pub struct SetupDetails {
    pub secret: String,
    pub uri: String,
    pub recovery_codes: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    version: u8,
    salt: String,
    wrapped: String,
    recovery: BTreeMap<String, String>,
    failed_attempts: u32,
    blocked_until: i64,
    migrating: bool,
    items: Vec<String>,
    copied: Vec<String>,
}

#[derive(Serialize, Deserialize, Zeroize)]
#[zeroize(drop)]
#[serde(deny_unknown_fields)]
struct Payload {
    disk_password: String,
    seed: String,
    last_step: i64,
}

struct Pending {
    config: Config,
    payload: Payload,
    password_key: Key,
    device: Key,
    expires: i64,
    verified: Option<i64>,
    attempts: usize,
    replacement: bool,
}

pub struct Security {
    paths: Paths,
    config: Option<Config>,
    pending: Option<Pending>,
    unlocked: bool,
    guard_started: bool,
    recovery_verified_at: Option<i64>,
    #[cfg(test)]
    test_device: Option<[u8; 32]>,
}

impl Security {
    pub fn load(paths: Paths) -> Result<Self, CommandError> {
        let config = if paths.security_config().exists() {
            let file = fs::File::open(paths.security_config())?;
            let config: Config = serde_json::from_reader(file).map_err(|_| corrupt())?;
            if config.version != 1 || decode(&config.salt)?.len() != 32 || config.items.iter().any(|name| !volume::LEGACY.contains(&name.as_str())) { return Err(corrupt()); }
            Some(config)
        } else {
            if paths.vault_image().exists() { return Err(corrupt()); }
            None
        };
        Ok(Self { paths, config, pending: None, unlocked: false, guard_started: false, recovery_verified_at: None, #[cfg(test)] test_device: None })
    }

    /// Перед показом окна закрывает том предыдущего, уже завершившегося владельца.
    pub fn prepare(&self) -> Result<(), CommandError> {
        if !self.paths.root().exists() { return Ok(()); }
        let _operation = volume::operation_lock(&self.paths)?;
        volume::seal_stale(&self.paths)
    }

    pub fn status(&self, now: i64) -> SecurityStatus {
        SecurityStatus { supported: cfg!(target_os = "macos"), configured: self.config.is_some(), unlocked: self.unlocked,
            migrating: self.config.as_ref().is_some_and(|c| c.migrating),
            recovery_remaining: self.config.as_ref().map_or(0, |c| c.recovery.len()),
            retry_after: self.config.as_ref().map_or(0, |c| (c.blocked_until - now).max(0)),
            recovery_recent: self.unlocked && self.recovery_verified_at.is_some_and(|at| (0..=300).contains(&(now - at))) }
    }

    /// До добровольного включения защиты доступен обычный режим работы.
    pub(crate) fn unprotected_paths(&self) -> Option<Paths> {
        self.config.is_none().then(|| self.paths.clone())
    }

    pub fn begin_setup(&mut self, password: &str, now: i64) -> Result<SetupDetails, CommandError> {
        if !cfg!(target_os = "macos") { return Err(volume_unsupported()); }
        if self.config.is_some() { return Err(CommandError::new("SecurityConfigured", "защита уже настроена")); }
        validate_password(password)?;
        volume::private_dir(self.paths.root())?;
        let device = self.device(true)?;
        let salt = encode(&random::<32>()?);
        let config = Config { version: 1, salt: salt.clone(), wrapped: String::new(), recovery: BTreeMap::new(), failed_attempts: 0, blocked_until: 0,
            migrating: true, items: volume::legacy_items(&self.paths), copied: vec![] };
        let payload = Payload { disk_password: encode(&random::<32>()?), seed: crypto::base32(&random::<20>()?), last_step: -1 };
        self.pending = Some(Pending { config, payload, password_key: password_key(password, &salt)?, device,
            expires: now + 600, verified: None, attempts: 0, replacement: false });
        self.setup_details()
    }

    fn setup_details(&mut self) -> Result<SetupDetails, CommandError> {
        let pending = self.pending.as_mut().ok_or_else(expired)?;
        let recovery_codes = make_recovery(&pending.password_key, &pending.payload, &mut pending.config)?;
        Ok(SetupDetails { secret: pending.payload.seed.clone(),
            uri: format!("otpauth://totp/Ombra:Local%20profiles?secret={}&issuer=Ombra&algorithm=SHA1&digits=6&period=30", pending.payload.seed), recovery_codes })
    }

    pub fn verify_setup(&mut self, code: &str, now: i64) -> Result<(), CommandError> {
        let pending = self.pending.as_mut().ok_or_else(expired)?;
        if pending.expires < now || pending.attempts >= 5 { self.pending = None; return Err(expired()); }
        pending.attempts += 1;
        let step = crypto::code_step(&pending.payload.seed, code, now, -1).ok_or_else(error)?;
        pending.payload.last_step = step;
        pending.verified = Some(now);
        Ok(())
    }

    pub fn cancel_setup(&mut self) { self.pending = None; }

    /// Активация — после проверки кода и явного подтверждения сохранённых кодов восстановления.
    pub fn finish_setup(&mut self, recovery_saved: bool, now: i64) -> Result<Option<Paths>, CommandError> {
        if !recovery_saved { return Err(CommandError::new("SecurityRecoveryRequired", "сохраните коды восстановления")); }
        let pending = self.pending.as_ref().ok_or_else(expired)?;
        if pending.expires < now || pending.verified.is_none_or(|time| now - time > 300) { return Err(expired()); }
        let mut pending = self.pending.take().unwrap();
        if !pending.replacement { pending.config.items = volume::legacy_items(&self.paths); }
        pending.config.wrapped = wrap(&combined(&pending.password_key, &*pending.device), &pending.payload)?;
        self.save(&pending.config)?;
        self.config = Some(pending.config);
        if pending.replacement { self.recovery_verified_at = None; return Ok(Some(self.paths.in_vault())); }
        // База обычного сеанса ещё открыта. Перенос выполняется после перезапуска и разблокировки,
        // когда прежнее ядро уже завершило запись и отпустило все файлы.
        Ok(None)
    }

    pub fn unlock(&mut self, password: &str, code: &str, recovery: bool, now: i64) -> Result<Paths, CommandError> {
        if self.unlocked { return Ok(self.paths.in_vault()); }
        let (_, payload) = self.authenticate(password, code, recovery, now)?;
        if recovery { self.recovery_verified_at = Some(now); }
        self.open_volume(&payload)
    }

    fn open_volume(&mut self, payload: &Payload) -> Result<Paths, CommandError> {
        let _operation = volume::operation_lock(&self.paths)?;
        volume::seal_stale(&self.paths)?;
        let migrating = self.config.as_ref().ok_or_else(corrupt)?.migrating;
        if migrating && !self.paths.vault_image().exists() { volume::create(&self.paths, &payload.disk_password)?; }
        if !self.guard_started { volume::spawn_guard(&self.paths)?; self.guard_started = true; }
        volume::attach(&self.paths, &payload.disk_password)?;
        let result = (|| {
            if migrating {
                volume::stop_browsers(&self.paths)?;
                let _legacy_locks = volume::lock_legacy(&self.paths)?;
                if self.paths.db().exists() {
                    for aux in ["ombra.db-wal", "ombra.db-shm"] { crate::paths::remove_path(&self.paths.in_vault().data_root().join(aux))?; }
                }
                let mut config = self.config.clone().ok_or_else(corrupt)?;
                for name in volume::legacy_items(&self.paths) {
                    if !config.items.contains(&name) { config.items.push(name); }
                }
                for name in config.items.clone() {
                    if !config.copied.contains(&name) || !volume::item_matches(&self.paths, &name)? {
                        volume::migrate_item(&self.paths, &name)?;
                        if !config.copied.contains(&name) { config.copied.push(name.clone()); }
                        self.save(&config)?;
                        self.config = Some(config.clone());
                    }
                }
                // Не удаляем ни один исходник, пока все копии не проверены.
                profile_key(&self.paths.in_vault())?;
                for name in &config.items { volume::remove_legacy(&self.paths, name)?; }
                volume::remaining_logs(&self.paths)?;
                config.migrating = false;
                self.save(&config)?;
                self.config = Some(config);
            }
            volume::remaining_logs(&self.paths)?;
            Ok(self.paths.in_vault())
        })();
        match result {
            Ok(paths) => { self.unlocked = true; Ok(paths) }
            Err(error) => { let _ = volume::detach(&self.paths); Err(error) }
        }
    }

    fn authenticate(&mut self, password: &str, code: &str, recovery: bool, now: i64) -> Result<(Key, Payload), CommandError> {
        self.authenticate_with_recent_recovery(password, code, recovery, now, false)
    }

    fn authenticate_with_recent_recovery(&mut self, password: &str, code: &str, recovery: bool, now: i64, allow_recent: bool) -> Result<(Key, Payload), CommandError> {
        let recent = allow_recent && code.is_empty() && self.recovery_verified_at.is_some_and(|at| (0..=300).contains(&(now - at)));
        let mut config = self.config.clone().ok_or_else(|| CommandError::new("SecurityRequired", "сначала настройте защиту в настройках пользователя"))?;
        if config.blocked_until > now { return Err(CommandError::new("SecurityRateLimited", "слишком много попыток; повторите позже")); }
        if password.len() > 4096 || code.len() > 128 { return Err(error()); }
        let password = password_key(password, &config.salt)?;
        let result = (|| {
            if recovery {
                let code = Zeroizing::new(normalize_recovery(code));
                let id = recovery_id(&code);
                let wrapped = config.recovery.get(&id).ok_or_else(error)?;
                let mut payload = unwrap(&combined(&password, code.as_bytes()), wrapped)?;
                payload.last_step = payload.last_step.max(now / 30);
                let device = self.device(true)?;
                config.wrapped = wrap(&combined(&password, &*device), &payload)?;
                config.recovery.remove(&id);
                Ok(payload)
            } else {
                let device = self.device(false)?;
                let mut payload = unwrap(&combined(&password, &*device), &config.wrapped)?;
                if !recent { payload.last_step = crypto::code_step(&payload.seed, code, now, payload.last_step).ok_or_else(error)?; }
                config.wrapped = wrap(&combined(&password, &*device), &payload)?;
                Ok(payload)
            }
        })();
        match result {
            Ok(payload) => {
                config.failed_attempts = 0;
                config.blocked_until = 0;
                self.save(&config)?;
                self.config = Some(config);
                Ok((password, payload))
            }
            Err(error) => {
                config.failed_attempts = config.failed_attempts.saturating_add(1);
                if config.failed_attempts >= 5 { config.blocked_until = now + (30 * 2i64.pow((config.failed_attempts - 5).min(5))).min(900); }
                self.save(&config)?;
                self.config = Some(config);
                Err(error)
            }
        }
    }

    pub fn change_password(&mut self, current: &str, code: &str, new: &str, now: i64) -> Result<Vec<String>, CommandError> {
        if !self.unlocked { return Err(locked()); }
        validate_password(new)?;
        let (_, payload) = self.authenticate(current, code, code.len() > 6, now)?;
        let mut config = self.config.clone().ok_or_else(corrupt)?;
        config.salt = encode(&random::<32>()?);
        let password = password_key(new, &config.salt)?;
        let device = self.device(false)?;
        config.wrapped = wrap(&combined(&password, &*device), &payload)?;
        let codes = make_recovery(&password, &payload, &mut config)?;
        self.save(&config)?;
        self.config = Some(config);
        Ok(codes)
    }

    pub fn replace_authenticator(&mut self, password: &str, code: &str, now: i64) -> Result<SetupDetails, CommandError> {
        if !self.unlocked { return Err(locked()); }
        let (password_key, mut payload) = self.authenticate_with_recent_recovery(password, code, code.len() > 6, now, true)?;
        payload.seed = crypto::base32(&random::<20>()?);
        payload.last_step = -1;
        self.pending = Some(Pending { config: self.config.clone().ok_or_else(corrupt)?, payload, password_key,
            device: self.device(false)?, expires: now + 600, verified: None, attempts: 0, replacement: true });
        self.setup_details()
    }

    fn save(&self, config: &Config) -> Result<(), CommandError> { volume::atomic_json(&self.paths.security_config(), config) }

    fn device(&self, create: bool) -> Result<Key, CommandError> {
        #[cfg(test)]
        if let Some(key) = self.test_device { return Ok(Zeroizing::new(key)); }
        local_key::load_with(&self.paths.device_key_file(), &self.paths.root().join(".vault-device-key.lock"), || {
            // Setup/recovery may create a new device key. Ordinary unlock must import the old one.
            if create { return local_key::new_key(); }
            let account: String = Sha256::digest(self.paths.root().as_os_str().as_encoded_bytes()).iter().map(|b| format!("{b:02x}")).collect();
            local_key::legacy_key("Ombra.ProfileVault", &account)
        }).map_err(|_| CommandError::new("SecurityDevice", "ключ этого устройства недоступен; используйте код восстановления"))
    }
}

pub fn locked() -> CommandError { CommandError::new("SecurityLocked", "разблокируйте профили паролем и кодом 2FA в окне Ombra") }
/// Старый ключ отдельных секретов тоже переносится внутрь тома: восстановление не зависит от старого Mac.
pub(crate) fn profile_key(paths: &Paths) -> Result<Zeroizing<[u8; 32]>, CommandError> {
    if !paths.is_protected() {
        let security = Security::load(paths.clone())?;
        if security.unprotected_paths().is_none() { return Err(locked()); }
        volume::private_dir(paths.root())?;
    } else if !volume::mounted(paths) { return Err(locked()); }
    Ok(local_key::profile_key(paths)?)
}

pub fn append_log(paths: &Paths, bytes: &[u8]) -> Result<(), CommandError> {
    use std::io::Write;
    if paths.is_protected() {
        if !volume::mounted(paths) { return Ok(()); }
    } else if paths.security_config().exists() || paths.vault_image().exists() { return Ok(()); }
    volume::private_dir(&paths.logs())?;
    let path = paths.logs().join("ombra.log");
    if fs::metadata(&path).is_ok_and(|meta| meta.len() >= 5 * 1024 * 1024) { fs::rename(&path, paths.logs().join("ombra.log.1"))?; }
    let mut file = crate::sys::private_options().create(true).append(true).open(path)?;
    file.write_all(bytes)?;
    Ok(())
}
pub fn boot_language(paths: &Paths) -> String {
    use std::io::Read;
    let read = || -> Option<String> {
        let mut bytes = vec![];
        fs::File::open(paths.root().join("ui.json")).ok()?.take(4096).read_to_end(&mut bytes).ok()?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        value.get("language")?.as_str().filter(|language| ["ru", "en"].contains(language)).map(String::from)
    };
    read().unwrap_or_else(|| "ru".into())
}
pub fn remember_language(paths: &Paths, language: &str) {
    if ["ru", "en"].contains(&language) {
        let _ = volume::atomic_json(&paths.root().join("ui.json"), &serde_json::json!({ "language": language }));
    }
}
fn corrupt() -> CommandError { CommandError::new("SecurityStorage", "настройки защиты недоступны; данные не открыты") }
fn expired() -> CommandError { CommandError::new("SecurityExpired", "Время настройки истекло. Начните заново.") }
fn validate_password(password: &str) -> Result<(), CommandError> {
    if password.chars().count() < 12 || password.len() > 4096 { return Err(CommandError::new("SecurityPassword", "пароль должен содержать от 12 символов")); }
    Ok(())
}
fn wrap(key: &[u8; 32], payload: &Payload) -> Result<String, CommandError> {
    let bytes = Zeroizing::new(serde_json::to_vec(payload).map_err(|_| corrupt())?);
    seal(key, &bytes)
}
fn unwrap(key: &[u8; 32], value: &str) -> Result<Payload, CommandError> {
    serde_json::from_slice(&unseal(key, value)?).map_err(|_| error())
}
fn normalize_recovery(code: &str) -> String { code.chars().filter(|c| !c.is_whitespace() && *c != '-').flat_map(char::to_uppercase).collect() }
fn recovery_id(code: &str) -> String { encode(&Sha256::digest(code.as_bytes())) }
fn make_recovery(password: &[u8; 32], payload: &Payload, config: &mut Config) -> Result<Vec<String>, CommandError> {
    config.recovery.clear();
    let mut codes = vec![];
    for _ in 0..8 {
        let normalized = Zeroizing::new(crypto::base32(&random::<16>()?));
        config.recovery.insert(recovery_id(&normalized), wrap(&combined(password, normalized.as_bytes()), payload)?);
        codes.push(normalized.as_bytes().chunks(5).map(|part| std::str::from_utf8(part).unwrap()).collect::<Vec<_>>().join("-"));
    }
    Ok(codes)
}

/// Отдельный процесс CLI — отдельный сеанс; секреты читаются только из терминала, не из аргументов или окружения.
pub fn unlock_terminal(paths: Paths) -> Result<(Paths, Option<crate::lock::DirLock>), CommandError> {
    use std::io::{BufRead, IsTerminal, Read, Write};
    let mut security = Security::load(paths)?;
    if let Some(paths) = security.unprotected_paths() { return Ok((paths, None)); }
    let instance = crate::lock::DirLock::acquire(security.paths.root()).map_err(|_| CommandError::new("SecurityBusy", "сеанс Ombra уже открыт; для автоматизации в его сеансе используйте Local API"))?;
    if !std::io::stdin().is_terminal() { return Err(locked()); }
    security.prepare()?;
    fn prompt(label: &str, hidden: bool) -> Result<Zeroizing<String>, CommandError> {
        eprint!("{label}"); std::io::stderr().flush()?;
        if hidden {
            return rpassword::read_password().map(Zeroizing::new).map_err(CommandError::from);
        }
        let mut line = Zeroizing::new(String::new());
        std::io::stdin().lock().take(8192).read_line(&mut line)?;
        if hidden { eprintln!(); }
        while line.ends_with(['\n', '\r']) { line.pop(); }
        Ok(line)
    }
    let password = prompt("Пароль хранилища: ", true)?;
    let code = prompt("Код 2FA (или одноразовый код восстановления): ", true)?;
    let recovery = code.len() > 6;
    if recovery && security.status(crate::sys::unix_now()).recovery_remaining <= 1 {
        return Err(CommandError::new("SecurityRecoveryRequired", "используйте последний код восстановления в окне Ombra и сразу замените потерянный аутентификатор в настройках"));
    }
    Ok((security.unlock(&password, &code, recovery, crate::sys::unix_now())?, Some(instance)))
}

fn volume_unsupported() -> CommandError {
    CommandError::new("SecurityUnsupported", "зашифрованное хранилище профилей пока поддерживается только в macOS")
}
