//! Распаковка пакета расширения (zip или папка) во временную папку и разбор манифеста: имя, версия, поле `key`,
//! риск для шлюза.
use super::id::extension_id;
use super::{FormatError, PackageError};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde_json::{Map, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct Limits {
    pub bytes: u64,
    pub entries: usize,
}

/// Спецификация 3Б: распаковка не больше 100 МБ и 10 000 записей.
pub const LIMITS: Limits = Limits { bytes: 100 * 1024 * 1024, entries: 10_000 };

/// Длиннее имя обрезается: оно хранится в базе и уходит в интерфейс при каждом показе библиотеки.
const NAME_MAX_CHARS: usize = 200;
/// Длиннее описание обрезается: оно уходит в интерфейс с каждой карточкой.
const DESCRIPTION_MAX_CHARS: usize = 500;

/// Загружает ли движок расширения Manifest V2 — по пробе задачи 1 части 3Б.
pub const MV2_SUPPORTED: bool = true;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpacked {
    pub id: String,
    pub name: String,
    pub version: String,
    pub manifest_version: i64,
    pub gateway_risk: bool,
    pub description: String,
}

/// Столкновения путей в архиве (`x` и `x/y`) и неподходящие имена — плохой архив, не сбой диска.
fn fs_error(e: std::io::Error) -> PackageError {
    use std::io::ErrorKind::{AlreadyExists, InvalidFilename, IsADirectory, NotADirectory};
    if matches!(e.kind(), AlreadyExists | NotADirectory | IsADirectory | InvalidFilename) { FormatError::Zip.into() } else { e.into() }
}

/// Zip в папку `dir`. Записи с путями наружу, ссылки и всё, кроме файлов и папок, пропускаются.
pub fn unzip(bytes: &[u8], dir: &Path, limits: &Limits) -> Result<(), PackageError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| FormatError::Zip)?;
    if archive.len() > limits.entries {
        return Err(FormatError::TooLarge.into());
    }
    let mut total = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|_| FormatError::Zip)?;
        let Some(relative) = entry.enclosed_name() else { continue };
        if entry.is_symlink() {
            continue;
        }
        let target = dir.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(fs_error)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(fs_error)?;
        }
        let mut file = std::fs::File::create(&target).map_err(fs_error)?;
        // Размер в заголовке zip может лгать: считаются прочитанные байты. Ошибка чтения — плохой архив, записи — диск.
        loop {
            let n = entry.read(&mut buf).map_err(|_| FormatError::Zip)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > limits.bytes {
                return Err(FormatError::TooLarge.into());
            }
            file.write_all(&buf[..n])?;
        }
    }
    Ok(())
}

/// Папка расширения пользователя — в `dir`: только файлы и папки (ссылки пропускаются), с теми же лимитами.
pub fn copy_tree(src: &Path, dir: &Path, limits: &Limits) -> Result<(), PackageError> {
    let (mut bytes, mut entries) = (0u64, 0usize);
    copy_into(src, dir, limits, &mut bytes, &mut entries)
}

fn copy_into(src: &Path, dst: &Path, limits: &Limits, bytes: &mut u64, entries: &mut usize) -> Result<(), PackageError> {
    std::fs::create_dir_all(dst)?;
    for item in std::fs::read_dir(src)? {
        let item = item?;
        *entries += 1;
        if *entries > limits.entries {
            return Err(FormatError::TooLarge.into());
        }
        // `file_type` не идёт по ссылке: ссылка не файл и не папка.
        let kind = item.file_type()?;
        let target = dst.join(item.file_name());
        if kind.is_dir() {
            copy_into(&item.path(), &target, limits, bytes, entries)?;
        } else if kind.is_file() {
            *bytes += item.metadata()?.len();
            if *bytes > limits.bytes {
                return Err(FormatError::TooLarge.into());
            }
            std::fs::copy(item.path(), &target)?;
        }
    }
    Ok(())
}

/// Корень расширения: `dir` с `manifest.json` или единственная вложенная папка с ним (`__MACOSX` из Finder не в счёт).
pub fn root(dir: &Path) -> Result<PathBuf, FormatError> {
    if dir.join("manifest.json").is_file() {
        return Ok(dir.to_path_buf());
    }
    let mut folders = std::fs::read_dir(dir)
        .map_err(|_| FormatError::Manifest)?
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != "__MACOSX" && e.file_type().is_ok_and(|t| t.is_dir()));
    match (folders.next(), folders.next()) {
        (Some(only), None) if only.path().join("manifest.json").is_file() => Ok(only.path()),
        _ => Err(FormatError::Manifest),
    }
}

/// Манифест в `root`: проверка, имя, поле `key`. `key` — ключ из CRX (пишется в манифест); `None` — ключ манифеста
/// или свой случайный (32 байта): идентификатор не зависит от пути и не меняется при обновлениях.
pub fn finish(root: &Path, key: Option<&[u8]>) -> Result<Unpacked, PackageError> {
    let path = root.join("manifest.json");
    let text = std::fs::read_to_string(&path).map_err(|_| FormatError::Manifest)?;
    // ponytail: манифест с комментариями (Chromium их терпит) не читается — `Manifest`; JSON5-разбор, если попросят.
    let mut manifest: Map<String, Value> =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|_| FormatError::Manifest)?;
    let manifest_version =
        manifest.get("manifest_version").and_then(Value::as_i64).filter(|v| matches!(v, 2 | 3)).ok_or(FormatError::Manifest)?;
    if manifest_version == 2 && !MV2_SUPPORTED {
        return Err(FormatError::Mv2.into());
    }
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .filter(|v| version_key(v).is_some())
        .ok_or(FormatError::Manifest)?
        .to_string();
    let raw_name =
        manifest.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty()).ok_or(FormatError::Manifest)?.to_string();
    let name: String = localized(root, &manifest, &raw_name).unwrap_or(raw_name).chars().take(NAME_MAX_CHARS).collect();
    let description = manifest_description(root, &manifest);
    let key_bytes = match key {
        Some(key) => key.to_vec(),
        None => match manifest.get("key").and_then(Value::as_str) {
            // Chromium принимает и PEM: строки `-----…` и пробелы отбрасываются.
            Some(text) => {
                let body: String = text.lines().filter(|l| !l.starts_with("-----")).flat_map(str::split_whitespace).collect();
                // Пустой ключ Chromium отвергает: расширение лежало бы в библиотеке и не загружалось.
                B64.decode(body).ok().filter(|k| !k.is_empty()).ok_or(FormatError::Manifest)?
            }
            None => {
                let mut fresh = [0u8; 32];
                getrandom::fill(&mut fresh).map_err(|e| std::io::Error::other(e.to_string()))?;
                fresh.to_vec()
            }
        },
    };
    manifest.insert("key".into(), Value::String(B64.encode(&key_bytes)));
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).expect("JSON из Map"))?;
    let gateway_risk = ["permissions", "optional_permissions"]
        .iter()
        .filter_map(|field| manifest.get(*field)?.as_array())
        .flatten()
        .any(|p| matches!(p.as_str(), Some("proxy" | "privacy")));
    Ok(Unpacked { id: extension_id(&key_bytes), name, version, manifest_version, gateway_risk, description })
}

/// Поле `description` манифеста: `__MSG_…__` — из `_locales`, как имя; ключ без перевода — пусто; пробелы по краям
/// срезаны; не длиннее 500 символов.
fn manifest_description(root: &Path, manifest: &Map<String, Value>) -> String {
    let Some(raw) = manifest.get("description").and_then(Value::as_str) else { return String::new() };
    let text = match localized(root, manifest, raw) {
        Some(text) => text,
        None if raw.starts_with("__MSG_") => String::new(),
        None => raw.to_string(),
    };
    text.trim().chars().take(DESCRIPTION_MAX_CHARS).collect()
}

/// Описание из распакованной папки — для записей до миграции 007; нет манифеста или он не JSON — `None`.
pub fn description(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("manifest.json")).ok()?;
    let manifest: Map<String, Value> = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    Some(manifest_description(root, &manifest))
}

/// `a` новее `b` по правилам версий Chromium (недостающие части — нули).
pub fn newer(a: &str, b: &str) -> bool {
    version_key(a) > version_key(b)
}

/// Версия манифеста: 1–4 числа через точку, каждое до 65535, без ведущих нулей.
fn version_key(text: &str) -> Option<[u32; 4]> {
    let parts: Vec<u32> = text
        .split('.')
        .map(|p| if !p.bytes().all(|b| b.is_ascii_digit()) || p.is_empty() || (p.len() > 1 && p.starts_with('0')) { None } else { p.parse().ok().filter(|n| *n <= 65535) })
        .collect::<Option<_>>()?;
    if !(1..=4).contains(&parts.len()) {
        return None;
    }
    let mut key = [0; 4];
    key[..parts.len()].copy_from_slice(&parts);
    Some(key)
}

/// Имя `__MSG_x__` из `_locales/<default_locale>/messages.json` (ключ без учёта регистра).
fn localized(root: &Path, manifest: &Map<String, Value>, raw: &str) -> Option<String> {
    let key = raw.strip_prefix("__MSG_")?.strip_suffix("__")?.to_lowercase();
    let locale = manifest.get("default_locale")?.as_str()?;
    if locale.is_empty() || locale.contains(['/', '\\']) || locale.contains("..") {
        return None;
    }
    let text = std::fs::read_to_string(root.join("_locales").join(locale).join("messages.json")).ok()?;
    let messages: Map<String, Value> = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let (_, entry) = messages.iter().find(|(k, _)| k.to_lowercase() == key)?;
    entry.get("message")?.as_str().filter(|m| !m.trim().is_empty()).map(str::to_string)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    pub(crate) fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in files {
            writer.start_file(*name, SimpleFileOptions::default()).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    pub(crate) fn manifest_v3(name: &str, version: &str) -> Vec<u8> {
        serde_json::json!({"manifest_version": 3, "name": name, "version": version}).to_string().into_bytes()
    }

    const SMALL: Limits = Limits { bytes: 64, entries: 3 };

    #[test]
    fn unzip_skips_escapes_and_links() {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        writer.start_file("manifest.json", options).unwrap();
        writer.write_all(&manifest_v3("A", "1.0")).unwrap();
        writer.start_file("../evil.txt", options).unwrap();
        writer.write_all(b"x").unwrap();
        writer.add_symlink("link", "/etc/passwd", options).unwrap();
        writer.add_directory("icons/", options).unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("x");
        std::fs::create_dir(&dir).unwrap();
        unzip(&bytes, &dir, &LIMITS).unwrap();
        assert!(dir.join("manifest.json").is_file() && dir.join("icons").is_dir());
        assert!(!base.path().join("evil.txt").exists() && !dir.join("link").exists());
    }

    #[test]
    fn unzip_and_copy_respect_limits() {
        let dir = tempfile::tempdir().unwrap();
        let many = zip_of(&[("a", b"1"), ("b", b"2"), ("c", b"3"), ("d", b"4")]);
        assert!(matches!(unzip(&many, dir.path(), &SMALL), Err(PackageError::Format(FormatError::TooLarge))));
        let big = zip_of(&[("a", &[0u8; 65])]);
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(unzip(&big, dir.path(), &SMALL), Err(PackageError::Format(FormatError::TooLarge))));
        assert!(matches!(unzip(b"not a zip", dir.path(), &LIMITS), Err(PackageError::Format(FormatError::Zip))));
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("big"), [0u8; 65]).unwrap();
        let out = tempfile::tempdir().unwrap();
        assert!(matches!(copy_tree(src.path(), out.path(), &SMALL), Err(PackageError::Format(FormatError::TooLarge))));
    }

    #[test]
    fn copy_tree_skips_links() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("manifest.json"), manifest_v3("A", "1.0")).unwrap();
        std::fs::create_dir(src.path().join("js")).unwrap();
        std::fs::write(src.path().join("js/a.js"), b"1").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.path().join("link")).unwrap();
        let out = tempfile::tempdir().unwrap();
        copy_tree(src.path(), out.path(), &LIMITS).unwrap();
        assert!(out.path().join("js/a.js").is_file() && !out.path().join("link").exists());
    }

    #[test]
    fn root_is_the_folder_or_its_only_subfolder() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(root(dir.path()).err(), Some(FormatError::Manifest));
        std::fs::create_dir_all(dir.path().join("ext")).unwrap();
        std::fs::create_dir_all(dir.path().join("__MACOSX/ext")).unwrap();
        std::fs::write(dir.path().join("ext/manifest.json"), manifest_v3("A", "1.0")).unwrap();
        assert_eq!(root(dir.path()).unwrap(), dir.path().join("ext"));
        std::fs::write(dir.path().join("manifest.json"), manifest_v3("A", "1.0")).unwrap();
        assert_eq!(root(dir.path()).unwrap(), dir.path());
    }

    fn written(dir: &Path, manifest: serde_json::Value) -> PathBuf {
        let mut text = vec![0xEF, 0xBB, 0xBF];
        text.extend(manifest.to_string().into_bytes());
        std::fs::write(dir.join("manifest.json"), text).unwrap();
        dir.to_path_buf()
    }

    fn key_of(dir: &Path) -> String {
        let text = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["key"].as_str().unwrap().to_string()
    }

    #[test]
    fn finish_writes_the_key_and_reads_the_manifest() {
        let a = tempfile::tempdir().unwrap();
        written(a.path(), serde_json::json!({"manifest_version": 3, "name": "A", "version": "1.2.3",
            "optional_permissions": ["proxy"]}));
        let got = finish(a.path(), Some(b"crx key")).unwrap();
        assert_eq!(got, Unpacked { id: extension_id(b"crx key"), name: "A".into(), version: "1.2.3".into(),
            manifest_version: 3, gateway_risk: true, description: String::new() });
        assert_eq!(key_of(a.path()), B64.encode(b"crx key"));
        // Ключ манифеста остаётся и задаёт идентификатор.
        let b = tempfile::tempdir().unwrap();
        written(b.path(), serde_json::json!({"manifest_version": 3, "name": "B", "version": "1",
            "key": B64.encode(b"own key"), "permissions": ["storage"]}));
        let got = finish(b.path(), None).unwrap();
        assert_eq!((got.id, got.gateway_risk), (extension_id(b"own key"), false));
        // Без ключа — свой случайный: два импорта — два идентификатора.
        let (c, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for dir in [c.path(), d.path()] {
            written(dir, serde_json::json!({"manifest_version": 3, "name": "C", "version": "1"}));
        }
        let (c_id, d_id) = (finish(c.path(), None).unwrap().id, finish(d.path(), None).unwrap().id);
        assert_ne!(c_id, d_id);
        assert_eq!(extension_id(&B64.decode(key_of(c.path())).unwrap()), c_id);
    }

    #[test]
    fn finish_resolves_localized_names_and_rejects_bad_manifests() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("_locales/en")).unwrap();
        std::fs::write(dir.path().join("_locales/en/messages.json"), r#"{"AppName": {"message": "Wallet"}}"#).unwrap();
        written(dir.path(), serde_json::json!({"manifest_version": 3, "name": "__MSG_appName__", "version": "2",
            "default_locale": "en"}));
        assert_eq!(finish(dir.path(), None).unwrap().name, "Wallet");
        for bad in [
            serde_json::json!({"manifest_version": 4, "name": "X", "version": "1"}),
            serde_json::json!({"manifest_version": 3, "name": "X"}),
            serde_json::json!({"manifest_version": 3, "name": " ", "version": "1"}),
            serde_json::json!({"manifest_version": 3, "name": "X", "version": "1.02"}),
            serde_json::json!({"manifest_version": 3, "name": "X", "version": "1.2.3.4.5"}),
            serde_json::json!({"manifest_version": 3, "name": "X", "version": "+1.0"}),
            serde_json::json!({"manifest_version": 3, "name": "X", "version": "1", "key": ""}),
            serde_json::json!(["not an object"]),
        ] {
            let dir = tempfile::tempdir().unwrap();
            written(dir.path(), bad.clone());
            assert!(matches!(finish(dir.path(), None), Err(PackageError::Format(FormatError::Manifest))), "{bad}");
        }
        let mv2 = tempfile::tempdir().unwrap();
        written(mv2.path(), serde_json::json!({"manifest_version": 2, "name": "Old", "version": "1"}));
        let result = finish(mv2.path(), None);
        if MV2_SUPPORTED {
            assert_eq!(result.unwrap().manifest_version, 2);
        } else {
            assert!(matches!(result, Err(PackageError::Format(FormatError::Mv2))));
        }
    }

    #[test]
    fn finish_caps_the_name() {
        let dir = tempfile::tempdir().unwrap();
        written(dir.path(), serde_json::json!({"manifest_version": 3, "name": "я".repeat(300), "version": "1"}));
        assert_eq!(finish(dir.path(), None).unwrap().name.chars().count(), NAME_MAX_CHARS);
    }

    #[test]
    fn finish_reads_the_description() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("_locales/en")).unwrap();
        std::fs::write(dir.path().join("_locales/en/messages.json"), r#"{"appDesc": {"message": "  Кошелёк  "}}"#).unwrap();
        written(dir.path(), serde_json::json!({"manifest_version": 3, "name": "A", "version": "1",
            "description": "__MSG_appDesc__", "default_locale": "en"}));
        assert_eq!(finish(dir.path(), None).unwrap().description, "Кошелёк");
        assert_eq!(description(dir.path()).as_deref(), Some("Кошелёк"));
        let long = tempfile::tempdir().unwrap();
        written(long.path(), serde_json::json!({"manifest_version": 3, "name": "A", "version": "1",
            "description": "я".repeat(600)}));
        assert_eq!(finish(long.path(), None).unwrap().description.chars().count(), DESCRIPTION_MAX_CHARS);
        // Нет описания, не строка или `__MSG_` без перевода — пусто.
        for manifest in [
            serde_json::json!({"manifest_version": 3, "name": "A", "version": "1"}),
            serde_json::json!({"manifest_version": 3, "name": "A", "version": "1", "description": 5}),
            serde_json::json!({"manifest_version": 3, "name": "A", "version": "1", "description": "__MSG_missing__"}),
        ] {
            let dir = tempfile::tempdir().unwrap();
            written(dir.path(), manifest.clone());
            assert_eq!(finish(dir.path(), None).unwrap().description, "", "{manifest}");
        }
        assert_eq!(description(tempfile::tempdir().unwrap().path()), None);
    }

    #[test]
    fn unzip_path_collisions_are_zip_errors() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = zip_of(&[("x", b"1"), ("x/y", b"2")]);
        assert!(matches!(unzip(&bytes, dir.path(), &LIMITS), Err(PackageError::Format(FormatError::Zip))));
    }

    #[test]
    fn versions_compare_like_chromium() {
        assert!(newer("1.10", "1.9") && newer("2", "1.99.99.99"));
        assert!(!newer("1.2", "1.2.0") && !newer("1.2.0", "1.2") && !newer("1.0", "1.1"));
    }
}
