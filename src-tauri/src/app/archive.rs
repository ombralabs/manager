//! Архив профиля (спецификация 3А): `tar.gz` с `profile.json` и папкой браузера без кэшей; секреты — под паролем.
use super::forms::{bookmark_errors, clean_bookmarks, clean_lines, launch_line_errors, name_error, tag_errors};
use super::import_support::{GROUP_NAME_MAX, group_by_name, parse_proxy_line, proxy_line, resolve_proxy};
use super::{CommandError, Core};
use crate::engine::Engine;
use crate::fingerprint::{self, Arch, FieldError, Fingerprint};
use crate::paths::CACHE_DIRS;
use crate::store::bookmarks::Bookmark;
use crate::store::profiles::{ProfileInput, ProfileRecord};
use crate::store::secrets::{Secret, SecretField};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use ring::pbkdf2;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::path::Path;

const FORMAT: u32 = 1;
const KDF_ITERATIONS: u32 = 100_000;
/// Кэши и сокеты-симлинки Chromium: пропускаются на любой глубине.
const SKIP: &[&str] = &["Cache", "Code Cache", "GPUCache", "DevToolsActivePort", "SingletonLock", "SingletonSocket", "SingletonCookie"];

/// Пределы распаковки папки браузера. Кэши в архив не попадают, так что настоящий профиль много меньше.
#[derive(Clone, Copy)]
struct UnpackLimits {
    bytes: u64,
    entries: u64,
}

const UNPACK_LIMITS: UnpackLimits = UnpackLimits { bytes: 20 * 1024 * 1024 * 1024, entries: 500_000 };
/// `profile.json` больше этого — не наш архив.
const MANIFEST_MAX: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    name: String,
    note: String,
    platform_domain: String,
    start_urls: Vec<String>,
    extra_args: Vec<String>,
    restore_tabs: bool,
    fingerprint: serde_json::Value,
    tags: Vec<String>,
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
    group: Option<String>,
    /// Имя набора расширений; `None` — «Общий». Сами расширения в архив не кладутся.
    #[serde(default)]
    ext_category: Option<String>,
    /// Очеловечивание ввода и seed персоны (строкой: u64): импорт переносит почерк профиля (P-01). Архив без них —
    /// значения по умолчанию и новый seed, как у нового профиля. Нечитаемые настройки (архив более новой сборки с новым
    /// пресетом) — значения по умолчанию с предупреждением в журнале, как у столбца профиля (`row_to_record`).
    #[serde(default, deserialize_with = "lenient")]
    humanize: Option<crate::humanize::Humanize>,
    #[serde(default)]
    humanize_seed: Option<String>,
    #[serde(default)]
    network: Option<crate::network::ProfileNetwork>,
    /// `kind://login@host:port` — логин открытым текстом, пароль прокси только в `secrets`.
    proxy: Option<String>,
    /// `None` — архив без пароля.
    secrets: Option<Sealed>,
}

/// Настройки профиля из архива: нечитаемые — значения по умолчанию с предупреждением в журнале.
fn lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    Ok(Option::<serde_json::Value>::deserialize(d)?.map(|value| {
        // Массив serde прочёл бы как структуру: `[true]` стало бы `enabled: true`.
        let parsed = if value.is_object() { serde_json::from_value(value).map_err(|e| e.to_string()) } else { Err("ожидался объект".to_string()) };
        parsed.unwrap_or_else(|e| {
            log::warn!("настройки {} архива нечитаемы, взяты значения по умолчанию: {e}", std::any::type_name::<T>());
            T::default()
        })
    }))
}

#[derive(Serialize, Deserialize)]
struct Sealed {
    salt: String,
    nonce: String,
    data: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Secrets {
    username: Option<String>,
    password: Option<String>,
    totp: Option<String>,
    proxy_password: Option<String>,
}

fn key_from(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(KDF_ITERATIONS).unwrap(), salt, password.as_bytes(), &mut key);
    key
}

fn bad_password() -> CommandError {
    CommandError::new("ArchivePassword", "неверный пароль архива")
}

fn too_large() -> CommandError {
    CommandError::new("ArchiveTooLarge", "архив больше 20 ГБ или в нём больше 500 000 файлов")
}

fn format_error() -> CommandError {
    CommandError::new("ArchiveFormat", "это не архив профиля Ombra")
}

fn random<const N: usize>() -> Result<[u8; N], CommandError> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| CommandError::new("Internal", {
        log::warn!("Диагностическая причина: {e}");
        "Не удалось создать ключ для защиты архива".to_string()
    }))?;
    Ok(bytes)
}

fn seal(secrets: &Secrets, password: &str) -> Result<Sealed, CommandError> {
    let (salt, nonce) = (random::<16>()?, random::<12>()?);
    let json = serde_json::to_vec(secrets).map_err(|e| CommandError::new("Internal", e.to_string()))?;
    let data = ChaCha20Poly1305::new(&key_from(password, &salt).into())
        .encrypt(&nonce.into(), json.as_slice())
        .map_err(|_| CommandError::new("Internal", "не удалось зашифровать секреты архива"))?;
    Ok(Sealed { salt: B64.encode(salt), nonce: B64.encode(nonce), data: B64.encode(data) })
}

fn open(sealed: &Sealed, password: &str) -> Result<Secrets, CommandError> {
    let decode = |s: &str| B64.decode(s).map_err(|_| format_error());
    let (salt, nonce, data) = (decode(&sealed.salt)?, decode(&sealed.nonce)?, decode(&sealed.data)?);
    let nonce: [u8; 12] = nonce.try_into().map_err(|_| format_error())?;
    let json = ChaCha20Poly1305::new(&key_from(password, &salt).into())
        .decrypt(&nonce.into(), data.as_slice())
        .map_err(|_| bad_password())?;
    serde_json::from_slice(&json).map_err(|_| format_error())
}

/// Папка браузера под `browser/…` без `SKIP`; симлинки не архивируются и не разворачиваются.
fn append_browser<W: std::io::Write>(tar: &mut tar::Builder<W>, dir: &Path, name: &Path) -> std::io::Result<()> {
    tar.append_dir(name, dir)?;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let (path, inner) = (entry.path(), name.join(&file_name));
        if SKIP.iter().any(|s| file_name == *s) || CACHE_DIRS.iter().any(|c| path.ends_with(c)) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            append_browser(tar, &path, &inner)?;
        } else if kind.is_file() {
            let inner_str = inner.to_string_lossy();
            if inner_str == "browser/Default/Login Data" {
                // Строка Ombra (имя и пароль под публичным ключом) в архив не едет: пакуется очищенная копия.
                let tmp = tempfile::tempdir()?;
                let copy = tmp.path().join("Login Data");
                crate::engine::login_data::scrubbed_copy(&path, &copy)?;
                tar.append_path_with_name(&copy, &inner)?;
            } else if matches!(
                inner_str.as_ref(),
                "browser/Default/Login Data-journal" | "browser/Default/Login Data-wal" | "browser/Default/Login Data-shm"
            ) {
                // Могут хранить старые страницы с неочищенной строкой.
            } else {
                tar.append_path_with_name(&path, &inner)?;
            }
        }
    }
    Ok(())
}

fn write_archive(dir: &Path, out: &Path, manifest: &[u8]) -> std::io::Result<()> {
    let gz = flate2::write::GzEncoder::new(std::fs::File::create(out)?, flate2::Compression::fast());
    let mut tar = tar::Builder::new(gz);
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest.len() as u64);
    header.set_mode(0o600);
    tar.append_data(&mut header, "profile.json", manifest)?;
    if dir.is_dir() {
        append_browser(&mut tar, dir, Path::new("browser"))?;
    } else {
        // Профиль, который ни разу не запускали: папки ещё нет.
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_mode(0o755);
        header.set_size(0);
        tar.append_data(&mut header, "browser/", std::io::empty())?;
    }
    tar.into_inner()?.finish()?;
    Ok(())
}

/// Ошибки разбора архива — `ArchiveFormat`; ошибки диска и прав — `Io`.
fn unpack_error(e: std::io::Error) -> CommandError {
    use std::io::ErrorKind::{InvalidData, InvalidInput, Other, UnexpectedEof};
    if matches!(e.kind(), InvalidData | InvalidInput | UnexpectedEof | Other) { format_error() } else { e.into() }
}

/// Манифест и секреты — из первой записи `profile.json`: пароль проверяется до распаковки папки браузера.
/// Остальные записи распаковываются в `target` потоком: только файлы и папки (ссылка вывела бы запись за пределы
/// папки профиля), не выходя за `target`, в пределах `limits`.
fn read_archive(file: std::fs::File, target: &Path, password: Option<&str>, limits: UnpackLimits) -> Result<(Manifest, Secrets), CommandError> {
    use std::io::Read;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut entries = tar.entries().map_err(unpack_error)?;
    let mut first = entries.next().ok_or_else(format_error)?.map_err(unpack_error)?;
    if first.path().map_err(unpack_error)?.as_ref() != Path::new("profile.json") {
        return Err(format_error());
    }
    let mut bytes = Vec::new();
    (&mut first).take(MANIFEST_MAX + 1).read_to_end(&mut bytes).map_err(unpack_error)?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .ok()
        .filter(|m: &Manifest| m.format == FORMAT && bytes.len() as u64 <= MANIFEST_MAX)
        .ok_or_else(format_error)?;
    let secrets = match &manifest.secrets {
        Some(sealed) => open(sealed, password.ok_or_else(bad_password)?)?,
        None => Secrets::default(),
    };
    drop(first);
    let (mut total, mut count) = (0u64, 0u64);
    for entry in entries {
        let mut entry = entry.map_err(unpack_error)?;
        count += 1;
        // `entry.size()` — реально распаковываемый размер (PAX-запись `size` перекрывает заголовок); насыщение — от переполнения.
        total = total.saturating_add(entry.size());
        if count > limits.entries || total > limits.bytes {
            return Err(too_large());
        }
        let kind = entry.header().entry_type();
        // Папки создаёт `unpack_in` для файлов: режим папки из архива (например, 0o555) не применяется.
        if kind == tar::EntryType::Directory {
            continue;
        }
        if kind != tar::EntryType::Regular || !entry.unpack_in(target).map_err(unpack_error)? {
            return Err(format_error());
        }
    }
    Ok((manifest, secrets))
}

impl<E: Engine> Core<E> {
    /// Экспорт остановленного профиля. Пароль архива пустой или `None` — секретов в файле нет совсем.
    pub async fn export_profile(&self, id: &str, path: &Path, password: Option<&str>) -> Result<(), CommandError> {
        let _idle = self.supervisor.lock_idle(id).await?;
        let rec = self.store.get_profile(id)?.ok_or_else(super::profiles::not_found)?;
        let proxy = rec.proxy_id.map(|pid| self.store.get_proxy(pid)).transpose()?.flatten();
        // Без пароля архива ничего не расшифровывается: секреты читаются только для запечатывания.
        let secrets = match password.filter(|p| !p.is_empty()) {
            Some(pw) => {
                let (profile_password, totp) = self.store.profile_secrets_each(id)?;
                let proxy_password = match &proxy {
                    Some(p) => self.store.proxy_credentials(p.id)?.password,
                    None => None,
                };
                let all = Secrets {
                    username: rec.username.clone(),
                    password: profile_password?.map(|s| s.expose().to_string()),
                    totp: totp?.map(|s| s.expose().to_string()),
                    proxy_password: proxy_password.map(|s| s.expose().to_string()),
                };
                Some((all, pw.to_string()))
            }
            None => None,
        };
        let group = rec.group_id.map(|g| self.store.get_group(g)).transpose()?.flatten().map(|g| g.name);
        let ext_category = match rec.ext_category_id {
            crate::store::extensions::DEFAULT_CATEGORY => None,
            id => self.store.get_ext_category(id)?.map(|c| c.name),
        };
        let mut manifest = Manifest {
            format: FORMAT,
            name: rec.name.clone(),
            note: rec.note.clone(),
            platform_domain: rec.platform_domain.clone(),
            start_urls: rec.start_urls.clone(),
            extra_args: rec.extra_args.clone(),
            restore_tabs: rec.restore_tabs,
            fingerprint: serde_json::from_str(&rec.fingerprint_json).map_err(|e| CommandError::new("Internal", e.to_string()))?,
            tags: self.store.profile_tags(id)?,
            bookmarks: self.store.profile_bookmarks(id)?,
            group,
            ext_category,
            humanize: Some(rec.humanize.clone()),
            humanize_seed: Some(rec.humanize_seed.to_string()),
            network: Some(rec.network.clone()),
            proxy: proxy.as_ref().map(|p| proxy_line(p.kind, p.username.as_deref(), None, &p.host, p.port)),
            secrets: None,
        };
        let (dir, out) = (self.paths.profile_dir(id), path.to_path_buf());
        tokio::task::spawn_blocking(move || -> Result<(), CommandError> {
            if let Some((all, pw)) = &secrets {
                manifest.secrets = Some(seal(all, pw)?);
            }
            let json = serde_json::to_vec_pretty(&manifest).map_err(|e| CommandError::new("Internal", e.to_string()))?;
            write_archive(&dir, &out, &json).inspect_err(|_| {
                let _ = std::fs::remove_file(&out);
            })?;
            Ok(())
        })
        .await
        .map_err(|e| CommandError::new("Internal", format!("фоновая задача прервана: {e}")))??;
        Ok(())
    }

    /// Импорт — всегда новый профиль (новые id и номер); существующие не трогаются.
    pub async fn import_profile(&self, path: &Path, password: Option<&str>) -> Result<ProfileRecord, CommandError> {
        let file = std::fs::File::open(path)?;
        std::fs::create_dir_all(self.paths.profiles())?;
        let staging = tempfile::tempdir_in(self.paths.profiles())?;
        let result = self.import_staged(file, &staging, password).await;
        // Распакованная папка браузера после сбоя может быть большой: стирается не на воркере tokio.
        let _ = tokio::task::spawn_blocking(move || drop(staging)).await;
        result
    }

    async fn import_staged(&self, file: std::fs::File, staging: &tempfile::TempDir, password: Option<&str>) -> Result<ProfileRecord, CommandError> {
        let target = staging.path().to_path_buf();
        let password = password.filter(|p| !p.is_empty()).map(str::to_string);
        let (manifest, secrets) = tokio::task::spawn_blocking(move || read_archive(file, &target, password.as_deref(), UNPACK_LIMITS))
        .await
        .map_err(|e| CommandError::new("Internal", format!("фоновая задача прервана: {e}")))??;
        let mut errors = Vec::new();
        errors.extend(name_error(manifest.name.trim()));
        errors.extend(tag_errors(&clean_lines(&manifest.tags)));
        errors.extend(bookmark_errors(&clean_bookmarks(&manifest.bookmarks)));
        errors.extend(launch_line_errors(&clean_lines(&manifest.start_urls), &clean_lines(&manifest.extra_args)));
        if secrets.totp.as_deref().is_some_and(|t| super::totp::decode_base32(t).is_none()) {
            errors.push(FieldError { field: "totp", code: "invalid" });
        }
        if manifest.group.as_deref().is_some_and(|g| g.trim().chars().count() > GROUP_NAME_MAX) {
            errors.push(FieldError { field: "group", code: "invalid" });
        }
        errors.extend(manifest.humanize.as_ref().and_then(|h| crate::humanize::paste_key_error(&h.paste_key)));
        let humanize_seed = match manifest.humanize_seed.as_deref().map(str::parse::<u64>) {
            Some(Ok(seed)) => Some(seed),
            Some(Err(_)) => {
                errors.push(FieldError { field: "humanize_seed", code: "invalid" });
                None
            }
            None => None,
        };
        match Fingerprint::from_json(&manifest.fingerprint.to_string()) {
            Ok(fp) => {
                let fp_errors = fingerprint::validate(&fp, Arch::host());
                // Архив с Mac другой архитектуры — корректный архив: ошибка проверки с полем, а не «не архив».
                if fp_errors.iter().any(|e| (e.field, e.code) == ("gpu", "arch_mismatch")) {
                    return Err(CommandError::validation(fp_errors));
                }
                errors.extend(fp_errors);
            }
            Err(_) => return Err(format_error()),
        }
        if let Some(first) = errors.first() {
            return Err(CommandError::new("ArchiveFormat", format!("это не архив профиля Ombra: поле {}", first.field)));
        }

        let proxy_id = match manifest.proxy.as_deref().map(|l| parse_proxy_line(l).ok_or_else(format_error)).transpose()? {
            // Без пароля архива пароля прокси нет: запись выключена, чтобы не попасть в случайный выбор шлюза.
            Some(mut parsed) => {
                parsed.password = secrets.proxy_password.clone().map(Secret::new);
                Some(resolve_proxy(&self.store, parsed, manifest.secrets.is_some())?)
            }
            None => None,
        };
        let group_id = match manifest.group.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
            Some(name) => Some(group_by_name(&self.store, name)?),
            None => None,
        };

        let mut input = ProfileInput::new(manifest.name.trim(), manifest.fingerprint.to_string());
        input.note = manifest.note;
        input.group_id = group_id;
        input.platform_domain = manifest.platform_domain;
        input.start_urls = clean_lines(&manifest.start_urls);
        input.restore_tabs = manifest.restore_tabs;
        input.extra_args = clean_lines(&manifest.extra_args);
        input.username = secrets.username.filter(|u| !u.is_empty());
        input.password = secrets.password.map_or(SecretField::Keep, |p| SecretField::Set(Secret::new(p)));
        input.totp_secret = secrets.totp.map_or(SecretField::Keep, |t| SecretField::Set(Secret::new(t)));
        input.proxy_id = proxy_id;
        input.bookmarks = Some(clean_bookmarks(&manifest.bookmarks));
        input.humanize = manifest.humanize;
        input.humanize_seed = humanize_seed;
        input.network = manifest.network;
        // Набор подбирается по имени; нет такого — «Общий».
        input.ext_category_id = match manifest.ext_category.as_deref() {
            Some(name) => self.store.ext_category_by_name(name)?.unwrap_or(crate::store::extensions::DEFAULT_CATEGORY),
            None => crate::store::extensions::DEFAULT_CATEGORY,
        };
        let tags = clean_lines(&manifest.tags);
        let rec = self.store.create_profiles(std::slice::from_ref(&input), &tags, crate::sys::unix_now())?.remove(0);

        // Запись создана: любой сбой дальше стирает и её, и папку.
        let dir = self.paths.profile_dir(&rec.id);
        let browser = staging.path().join("browser");
        let moved = if browser.is_dir() { std::fs::rename(&browser, &dir) } else { std::fs::create_dir_all(&dir) };
        if let Err(e) = moved {
            let _ = crate::paths::remove_path(&dir);
            let _ = self.store.delete_profile(&rec.id);
            return Err(e.into());
        }
        Ok(rec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{env, form};

    /// Архив `tar.gz` из записей `(имя, содержимое)` в заданном порядке.
    fn raw_archive(path: &Path, entries: &[(&str, &str)]) {
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(path).unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for (name, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o600);
            tar.append_data(&mut h, name, data.as_bytes()).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
    }

    #[tokio::test]
    async fn password_is_checked_before_the_browser_folder_is_unpacked() {
        let e = env().await;
        let rec = e.core.create_profile(&form("sealed", None), 1).unwrap();
        let default = e.core.paths.profile_dir(&rec.id).join("Default");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::write(default.join("Preferences"), "{}").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.tar.gz");
        e.core.export_profile(&rec.id, &path, Some("pw")).await.unwrap();
        for password in [Some("wrong"), None] {
            let target = tempfile::tempdir().unwrap();
            let err = read_archive(std::fs::File::open(&path).unwrap(), target.path(), password, UNPACK_LIMITS).err().unwrap();
            assert_eq!(err.code, "ArchivePassword", "{password:?}");
            assert!(!target.path().join("browser").exists(), "{password:?}: папка браузера не распакована");
        }
        let target = tempfile::tempdir().unwrap();
        read_archive(std::fs::File::open(&path).unwrap(), target.path(), Some("pw"), UNPACK_LIMITS).unwrap();
        assert!(target.path().join("browser/Default/Preferences").is_file());
    }

    #[test]
    fn archive_without_profile_json_first_is_not_ours() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.tar.gz");
        raw_archive(&path, &[("browser/Default/Preferences", "{}"), ("profile.json", "{}")]);
        let target = tempfile::tempdir().unwrap();
        let err = read_archive(std::fs::File::open(&path).unwrap(), target.path(), None, UNPACK_LIMITS).err().unwrap();
        assert_eq!(err.code, "ArchiveFormat");
        assert!(!target.path().join("browser").exists());
    }

    #[tokio::test]
    async fn unpack_stops_at_the_limits() {
        let e = env().await;
        let rec = e.core.create_profile(&form("big", None), 1).unwrap();
        let default = e.core.paths.profile_dir(&rec.id).join("Default");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::write(default.join("a"), vec![0u8; 64]).unwrap();
        std::fs::write(default.join("b"), vec![0u8; 64]).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.tar.gz");
        e.core.export_profile(&rec.id, &path, None).await.unwrap();
        // Объём: 128 байт файлов при пределе 100; записи: четыре (две папки, два файла) при пределе 2.
        for limits in [UnpackLimits { bytes: 100, entries: 1000 }, UnpackLimits { bytes: 1 << 20, entries: 2 }] {
            let target = tempfile::tempdir().unwrap();
            let err = read_archive(std::fs::File::open(&path).unwrap(), target.path(), None, limits).err().unwrap();
            assert_eq!(err.code, "ArchiveTooLarge");
        }
        let target = tempfile::tempdir().unwrap();
        read_archive(std::fs::File::open(&path).unwrap(), target.path(), None, UNPACK_LIMITS).unwrap();
        assert!(target.path().join("browser/Default/a").is_file());
    }

    /// PAX-запись `size` перекрывает размер в заголовке ustar: учитывается тот, что реально распаковывается.
    #[tokio::test]
    async fn pax_size_override_does_not_bypass_the_byte_limit() {
        use std::io::Read;
        let e = env().await;
        let rec = e.core.create_profile(&form("pax", None), 1).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.tar.gz");
        e.core.export_profile(&rec.id, &real, None).await.unwrap();
        let mut manifest = Vec::new();
        let mut real_tar = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(&real).unwrap()));
        real_tar.entries().unwrap().next().unwrap().unwrap().read_to_end(&mut manifest).unwrap();

        let path = tmp.path().join("pax.tar.gz");
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(&path).unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.set_size(manifest.len() as u64);
        h.set_mode(0o600);
        tar.append_data(&mut h, "profile.json", manifest.as_slice()).unwrap();
        tar.append_pax_extensions([("size", b"5000".as_slice())]).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_path("browser/Default/x").unwrap();
        h.set_size(0);
        h.set_mode(0o600);
        h.set_cksum();
        tar.append(&h, vec![1u8; 5000].as_slice()).unwrap();
        tar.into_inner().unwrap().finish().unwrap();

        let target = tempfile::tempdir().unwrap();
        let err = read_archive(std::fs::File::open(&path).unwrap(), target.path(), None, UnpackLimits { bytes: 1000, entries: 1000 }).err().unwrap();
        assert_eq!(err.code, "ArchiveTooLarge");
        assert!(!target.path().join("browser/Default/x").exists());
    }
}
