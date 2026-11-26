//! Полные данные, включая SQLite, хранилища сайтов и кэш Chromium, — на зашифрованном томе macOS.
use crate::{app::CommandError, paths::Paths};
use sha2::{Digest, Sha256};
use std::{fs, io::{Read, Write}, os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt}, path::Path, process::{Command, Stdio}, time::Duration};

pub(super) const LEGACY: &[&str] = &["ombra.db", "ombra.db-wal", "ombra.db-shm", "dev-secrets.key", "secrets.key", "profiles", "extensions", "flows", "run", "logs", "logs/chromium", "logs/relay", "logs/flows"];
fn failure() -> CommandError { CommandError::new("SecurityStorage", "не удалось открыть зашифрованное хранилище; исходные данные сохранены") }
pub(super) fn private_dir(path: &Path) -> Result<(), CommandError> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } { return Err(failure()); }
    } else { fs::create_dir_all(path)?; }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
pub(super) fn atomic_json(path: &Path, value: &impl serde::Serialize) -> Result<(), CommandError> {
    let suffix: String = super::crypto::random::<12>()?.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = path.with_extension(format!("{suffix}.pending"));
    let mut file = fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&tmp)?;
    serde_json::to_writer(&mut file, value).map_err(|_| failure())?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    if let Some(parent) = path.parent() { fs::File::open(parent)?.sync_all()?; }
    Ok(())
}
pub(super) fn operation_lock(paths: &Paths) -> Result<fs::File, CommandError> {
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).mode(0o600).open(paths.root().join(".vault-operation.lock"))?;
    file.lock()?;
    Ok(file)
}
fn hdiutil(args: &[&std::ffi::OsStr], password: Option<&str>) -> Result<(), CommandError> {
    let mut child = Command::new("/usr/bin/hdiutil").args(args).stdin(if password.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|_| failure())?;
    if let Some(password) = password {
        let mut input = child.stdin.take().ok_or_else(failure)?;
        input.write_all(password.as_bytes())?;
        input.write_all(&[0])?;
    }
    if !child.wait()?.success() { return Err(failure()); }
    Ok(())
}
pub(super) fn mounted(paths: &Paths) -> bool {
    let path = paths.vault_mount();
    let Some(parent) = path.parent() else { return false };
    fs::metadata(&path).ok().zip(fs::metadata(parent).ok()).is_some_and(|(a, b)| a.dev() != b.dev())
}
pub(super) fn create(paths: &Paths, password: &str) -> Result<(), CommandError> {
    let pending = paths.root().join("Profiles.pending.sparsebundle");
    if pending.exists() { fs::remove_dir_all(&pending)?; }
    hdiutil(&["create".as_ref(), "-size".as_ref(), "1t".as_ref(), "-type".as_ref(), "SPARSEBUNDLE".as_ref(), "-fs".as_ref(), "APFS".as_ref(), "-volname".as_ref(), "Ombra Profiles".as_ref(), "-encryption".as_ref(), "AES-256".as_ref(), "-stdinpass".as_ref(), "-nospotlight".as_ref(), pending.as_os_str()], Some(password))?;
    fs::rename(pending, paths.vault_image())?;
    Ok(())
}
pub(super) fn attach(paths: &Paths, password: &str) -> Result<(), CommandError> {
    if mounted(paths) { return Err(CommandError::new("SecurityBusy", "предыдущий сеанс ещё закрывается; повторите через несколько секунд")); }
    private_dir(&paths.vault_mount())?;
    hdiutil(&["attach".as_ref(), paths.vault_image().as_os_str(), "-stdinpass".as_ref(), "-nobrowse".as_ref(), "-noautoopen".as_ref(), "-mountpoint".as_ref(), paths.vault_mount().as_os_str()], Some(password))?;
    if !mounted(paths) { return Err(failure()); }
    private_dir(paths.in_vault().data_root())?;
    Ok(())
}
pub(super) fn detach(paths: &Paths) -> Result<(), CommandError> {
    if !mounted(paths) { return Ok(()); }
    let result = hdiutil(&["detach".as_ref(), paths.vault_mount().as_os_str()], None);
    if !mounted(paths) { Ok(()) } else { result }
}
pub(super) fn legacy_items(paths: &Paths) -> Vec<String> {
    LEGACY.iter().filter(|name| paths.root().join(name).exists() && !name.starts_with("logs/")).map(|name| (*name).to_string()).collect()
}
pub(super) fn remaining_logs(paths: &Paths) -> Result<(), CommandError> {
    let source = paths.root().join("logs");
    if !source.exists() { return Ok(()); }
    let suffix: String = super::crypto::random::<8>()?.iter().map(|b| format!("{b:02x}")).collect();
    let target = paths.in_vault().logs().join(format!("legacy-{suffix}"));
    private_dir(&paths.in_vault().logs())?;
    copy_checked(&source, &target)?;
    crate::paths::remove_path(&source)?;
    Ok(())
}

pub(super) struct LegacyLocks {
    _database: Option<rusqlite::Connection>,
    _profiles: Vec<crate::lock::DirLock>,
}
pub(super) fn lock_legacy(paths: &Paths) -> Result<LegacyLocks, CommandError> {
    let busy = || CommandError::new("SecurityBusy", "Данные используются другим приложением. Закройте его и повторите перенос.");
    let database = if paths.db().exists() {
        let conn = rusqlite::Connection::open_with_flags(paths.db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(|_| busy())?;
        conn.busy_timeout(Duration::from_secs(3)).map_err(|_| busy())?;
        // Отдельный старый CLI может ещё работать: не копируем SQLite во время его записи.
        conn.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;").map_err(|_| busy())?;
        Some(conn)
    } else { None };
    let mut profiles = vec![];
    if paths.profiles().exists() {
        for entry in fs::read_dir(paths.profiles())? {
            let entry = entry?;
            if entry.file_type()?.is_dir() { profiles.push(crate::lock::DirLock::acquire(&entry.path()).map_err(|_| busy())?); }
        }
    }
    Ok(LegacyLocks { _database: database, _profiles: profiles })
}

fn digest(path: &Path) -> Result<[u8; 32], CommandError> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 128 * 1024];
    loop { let n = file.read(&mut buffer)?; if n == 0 { break; } hash.update(&buffer[..n]); }
    Ok(hash.finalize().into())
}
fn temporary_lock(entry: &fs::DirEntry) -> Result<bool, CommandError> {
    let kind = entry.file_type()?;
    Ok((kind.is_symlink() || kind.is_socket()) && ["SingletonLock", "SingletonCookie", "SingletonSocket"].iter().any(|name| entry.file_name() == *name))
}
fn copy_checked(source: &Path, target: &Path) -> Result<(), CommandError> {
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() { return Err(CommandError::new("SecurityMigration", "в старых данных есть символическая ссылка; перенос остановлен, исходные данные сохранены")); }
    if meta.is_dir() {
        private_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            if temporary_lock(&entry)? { continue; }
            copy_checked(&entry.path(), &target.join(entry.file_name()))?;
        }
        fs::File::open(target)?.sync_all()?;
    } else if meta.is_file() {
        fs::copy(source, target)?;
        fs::set_permissions(target, fs::Permissions::from_mode(meta.permissions().mode() & 0o700))?;
        fs::File::open(target)?.sync_all()?;
        if digest(source)? != digest(target)? { return Err(failure()); }
    } else { return Err(failure()); }
    Ok(())
}
fn matches_copy(source: &Path, target: &Path) -> Result<bool, CommandError> {
    let source_meta = fs::symlink_metadata(source)?;
    let Ok(target_meta) = fs::symlink_metadata(target) else { return Ok(false) };
    if source_meta.file_type().is_symlink() || target_meta.file_type().is_symlink() { return Ok(false); }
    if source_meta.is_file() { return Ok(target_meta.is_file() && source_meta.len() == target_meta.len() && digest(source)? == digest(target)?); }
    if !source_meta.is_dir() || !target_meta.is_dir() { return Ok(false); }
    let names = |path: &Path| -> Result<std::collections::BTreeSet<std::ffi::OsString>, CommandError> {
        let mut names = std::collections::BTreeSet::new();
        for entry in fs::read_dir(path)? { let entry = entry?; if !temporary_lock(&entry)? { names.insert(entry.file_name()); } }
        Ok(names)
    };
    if names(source)? != names(target)? { return Ok(false); }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if temporary_lock(&entry)? { continue; }
        if !matches_copy(&entry.path(), &target.join(entry.file_name()))? { return Ok(false); }
    }
    Ok(true)
}
pub(super) fn item_matches(paths: &Paths, name: &str) -> Result<bool, CommandError> {
    if !LEGACY.contains(&name) { return Err(failure()); }
    let source = paths.root().join(name);
    if !source.exists() { return Ok(true); }
    matches_copy(&source, &paths.in_vault().data_root().join(name))
}
pub(super) fn migrate_item(paths: &Paths, name: &str) -> Result<(), CommandError> {
    if !LEGACY.contains(&name) { return Err(failure()); }
    let source = paths.root().join(name);
    if !source.exists() { return Ok(()); }
    let target = paths.in_vault().data_root().join(name);
    // Исходник не удаляется здесь. Манифест переноса сначала надёжно записывается вызывающим кодом.
    if target.exists() { crate::paths::remove_path(&target)?; }
    if let Some(parent) = target.parent() { private_dir(parent)?; }
    copy_checked(&source, &target)
}
pub(super) fn remove_legacy(paths: &Paths, name: &str) -> Result<(), CommandError> {
    if !LEGACY.contains(&name) { return Err(failure()); }
    if name == "profiles" && paths.profiles().exists() {
        for entry in fs::read_dir(paths.profiles())? {
            let entry = entry?;
            if let Some(id) = entry.file_name().to_str() && let Some(cache) = paths.chromium_cache_dir(id) { crate::paths::remove_path(&cache)?; }
        }
    }
    crate::paths::remove_path(&paths.root().join(name))?;
    Ok(())
}
pub(super) fn spawn_guard(paths: &Paths) -> Result<(), CommandError> {
    atomic_json(&paths.root().join(".vault-owner.json"), &serde_json::json!({ "pid": std::process::id() }))?;
    Command::new(std::env::current_exe()?).arg("--vault-guard").arg(std::process::id().to_string()).arg(paths.root()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    Ok(())
}
/// Отдельный процесс переживает падение окна: закрывает браузеры и снимает том после завершения владельца.
pub fn guard(parent: u32, paths: Paths) -> Result<(), CommandError> {
    while crate::sys::pid_alive(parent) { std::thread::sleep(Duration::from_millis(250)); }
    let _operation = operation_lock(&paths)?;
    let owner = fs::read(paths.root().join(".vault-owner.json")).ok().and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok()).and_then(|value| value.get("pid").and_then(serde_json::Value::as_u64));
    if owner.is_some_and(|pid| pid != u64::from(parent)) { return Ok(()); }
    // hdiutil мог пережить окно в момент монтирования. Дожидаемся его до снятия образа.
    loop {
        let output = Command::new("/bin/ps").args(["-axww", "-o", "command="]).output()?;
        if !output.status.success() { return Err(failure()); }
        let running = String::from_utf8_lossy(&output.stdout).lines().any(|line| line.trim_start().starts_with("/usr/bin/hdiutil ")
            && (line.contains(paths.root().to_string_lossy().as_ref()) || line.contains(paths.vault_mount().to_string_lossy().as_ref())));
        if !running { break; }
        std::thread::sleep(Duration::from_millis(100));
    }
    seal_stale(&paths)
}

fn image_device(paths: &Paths) -> Result<Option<String>, CommandError> {
    if !paths.vault_image().exists() { return Ok(None); }
    let info = Command::new("/usr/bin/hdiutil").args(["info", "-plist"]).output()?;
    if !info.status.success() { return Err(failure()); }
    let mut convert = Command::new("/usr/bin/plutil").args(["-convert", "json", "-o", "-", "--", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    convert.stdin.take().ok_or_else(failure)?.write_all(&info.stdout)?;
    let json = convert.wait_with_output()?;
    if !json.status.success() { return Err(failure()); }
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).map_err(|_| failure())?;
    let canonical = fs::canonicalize(paths.vault_image())?;
    for image in value.get("images").and_then(serde_json::Value::as_array).into_iter().flatten() {
        let matches = image.get("image-path").and_then(serde_json::Value::as_str).and_then(|path| fs::canonicalize(path).ok()).is_some_and(|path| path == canonical);
        if matches {
            return Ok(image.get("system-entities").and_then(serde_json::Value::as_array).into_iter().flatten()
                .filter_map(|entity| entity.get("dev-entry").and_then(serde_json::Value::as_str))
                .find(|device| device.strip_prefix("/dev/disk").is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))).map(String::from));
        }
    }
    Ok(None)
}
pub(super) fn seal_stale(paths: &Paths) -> Result<(), CommandError> {
    let Some(device) = image_device(paths)? else { return Ok(()) };
    if mounted(paths) { stop_browsers(&paths.in_vault())?; }
    // SQLite может ещё закрываться в завершившемся процессе.
    for _ in 0..20 {
        if hdiutil(&["detach".as_ref(), device.as_ref()], None).is_ok() || image_device(paths)?.is_none() { return Ok(()); }
        std::thread::sleep(Duration::from_millis(250));
    }
    hdiutil(&["detach".as_ref(), "-force".as_ref(), device.as_ref()], None)?;
    if image_device(paths)?.is_none() { Ok(()) } else { Err(failure()) }
}
pub(super) fn stop_browsers(paths: &Paths) -> Result<(), CommandError> {
    if !paths.profiles().exists() { return Ok(()); }
    let runtime = tokio::runtime::Runtime::new()?;
    for entry in fs::read_dir(paths.profiles())? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            runtime.block_on(crate::engine::process::terminate_profile_processes(&entry.path(), Duration::from_secs(5)))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_checks_content_and_keeps_source_until_commit() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("old");
        let target = dir.path().join("new");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Cookies"), "private-cookie").unwrap();
        copy_checked(&source, &target).unwrap();
        assert!(matches_copy(&source, &target).unwrap());
        assert_eq!(fs::read(target.join("Cookies")).unwrap(), b"private-cookie");
        assert!(source.join("Cookies").exists());
        fs::write(source.join("Cookies"), "changed-after-an-interrupted-migration").unwrap();
        assert!(!matches_copy(&source, &target).unwrap());
        copy_checked(&source, &target).unwrap();
        assert!(matches_copy(&source, &target).unwrap());
        fs::write(target.join("removed-from-source"), "old value").unwrap();
        assert!(!matches_copy(&source, &target).unwrap());
        std::os::unix::fs::symlink("/etc", source.join("outside")).unwrap();
        assert!(copy_checked(&source, &target).is_err());
    }

    #[test]
    #[ignore = "создаёт и монтирует временный зашифрованный том macOS"]
    fn real_vault_encrypts_all_profile_files_and_rejects_wrong_password() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let password = super::super::crypto::encode(&super::super::crypto::random::<32>().unwrap());
        create(&paths, &password).unwrap();
        attach(&paths, &password).unwrap();
        struct Seal(Paths);
        impl Drop for Seal { fn drop(&mut self) { let _ = detach(&self.0); } }
        let _seal = Seal(paths.clone());
        let file = paths.in_vault().profile_dir("private-profile").join("Default/Local Storage/secret");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let private = b"private-profile-cookie-and-local-storage-71b9f93c";
        fs::write(&file, private).unwrap();
        super::super::append_log(&paths.in_vault(), b"encrypted activity log\n").unwrap();
        assert!(paths.in_vault().logs().join("ombra.log").exists());
        assert!(!paths.logs().exists());
        assert_eq!(fs::read(&file).unwrap(), private);
        detach(&paths).unwrap();
        assert!(!file.exists(), "закрытый том не раскрывает папку браузера");
        assert!(attach(&paths, "incorrect password").is_err());
        assert!(!mounted(&paths));
        for entry in fs::read_dir(paths.vault_image().join("bands")).unwrap() {
            let bytes = fs::read(entry.unwrap().path()).unwrap();
            assert!(!bytes.windows(private.len()).any(|part| part == private), "в образе не должно быть открытого содержимого профиля");
        }
        attach(&paths, &password).unwrap();
        assert_eq!(fs::read(file).unwrap(), private);
        detach(&paths).unwrap();
    }

    #[test]
    #[ignore = "переносит тестовую базу и профиль в настоящий временный том macOS"]
    fn real_vault_migrates_legacy_data_and_guard_seals_after_owner_exits() {
        check_vault_migration(false);
    }

    #[test]
    #[ignore = "переносит локальный ключ и тестовые данные в настоящий временный том macOS"]
    fn real_vault_migrates_local_key_and_guard_seals_after_owner_exits() {
        check_vault_migration(true);
    }

    fn check_vault_migration(local_key: bool) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let key = [7; 32];
        {
            let store = crate::store::Store::open(&paths.db(), crate::store::secrets::SecretBox::from_key(&key)).unwrap();
            store.set_setting("migration-proof", "private-database-value").unwrap();
            store.set_api_key(Some("private-encrypted-api-token")).unwrap();
        }
        let source_key = if local_key { paths.local_key_file() } else { paths.dev_key_file() };
        fs::write(&source_key, super::super::crypto::encode(&key)).unwrap();
        fs::create_dir_all(paths.logs()).unwrap();
        fs::write(paths.logs().join("ombra.log"), "old private activity log").unwrap();
        let cookie = paths.profile_dir("profile1").join("Default/Network/Cookies");
        fs::create_dir_all(cookie.parent().unwrap()).unwrap();
        fs::write(&cookie, "private-cookie-value").unwrap();
        let now = crate::sys::unix_now();
        let user_password = "a long temporary test password";
        let mut security = super::super::Security::load(paths.clone()).unwrap();
        security.test_device = Some(key);
        let details = security.begin_setup(user_password, now).unwrap();
        let seed = crate::app::totp::decode_base32(&details.secret).unwrap();
        security.verify_setup(&crate::app::totp::code_at(&seed, now).0, now).unwrap();
        let password = security.pending.as_ref().unwrap().payload.disk_password.clone();
        assert!(security.finish_setup(true, now).unwrap().is_none());
        assert!(paths.db().exists() && cookie.exists(), "до перезапуска исходные данные на месте");
        drop(security);
        struct Seal(Paths);
        impl Drop for Seal { fn drop(&mut self) { let _ = detach(&self.0); } }
        let _seal = Seal(paths.clone());
        let mut restarted = super::super::Security::load(paths.clone()).unwrap();
        restarted.test_device = Some(key);
        // Сторож проверяется ниже напрямую: unit-test бинарник не запускает служебный режим main.
        restarted.guard_started = true;
        restarted.unlock(user_password, &crate::app::totp::code_at(&seed, now + 30).0, false, now + 30).unwrap();
        assert!(restarted.status(now + 30).unlocked && !restarted.status(now + 30).migrating);
        assert!(!paths.db().exists() && !cookie.exists());
        {
            let core = crate::app::open_default(paths.in_vault(), std::sync::Arc::new(|_| {})).unwrap();
            assert_eq!(core.store.get_setting("migration-proof").unwrap().as_deref(), Some("private-database-value"));
            assert_eq!(core.store.api_key().unwrap().as_deref(), Some("private-encrypted-api-token"));
            assert!(core.paths.vault_key_file().exists());
            assert_eq!(fs::read(core.paths.logs().join("ombra.log")).unwrap(), b"old private activity log");
            assert_eq!(fs::read(core.paths.profile_dir("profile1").join("Default/Network/Cookies")).unwrap(), b"private-cookie-value");
        }
        assert!(!source_key.exists());
        let copied_key = if local_key { paths.in_vault().local_key_file() } else { paths.in_vault().dev_key_file() };
        fs::remove_file(copied_key).unwrap();
        {
            let restored = crate::app::open_default(paths.in_vault(), std::sync::Arc::new(|_| {})).unwrap();
            assert_eq!(restored.store.api_key().unwrap().as_deref(), Some("private-encrypted-api-token"), "прежний Keychain или отладочный ключ больше не нужны для восстановления секретов");
        }
        let mut child = Command::new("/bin/sleep").arg("1").spawn().unwrap();
        let parent = child.id();
        let wait = std::thread::spawn(move || child.wait().unwrap());
        guard(parent, paths.clone()).unwrap();
        wait.join().unwrap();
        assert!(!mounted(&paths), "сторож закрывает том после завершения владельца");
        attach(&paths, &password).unwrap();
        assert_eq!(fs::read(paths.in_vault().profile_dir("profile1").join("Default/Network/Cookies")).unwrap(), b"private-cookie-value");
        atomic_json(&paths.root().join(".vault-owner.json"), &serde_json::json!({ "pid": std::process::id() })).unwrap();
        guard(parent, paths.clone()).unwrap();
        assert!(mounted(&paths), "сторож прошлого владельца не снимает том нового сеанса");
        detach(&paths).unwrap();
    }
}
