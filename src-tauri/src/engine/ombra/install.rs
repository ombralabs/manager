//! Импорт в отдельную папку версии. Старый слот и установленные браузеры никогда не удаляются.
use super::catalog;
use super::version::platform;
use crate::engine::{EngineBuild, EngineError};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use tokio_util::sync::CancellationToken;

pub const APP_NAME: &str = if cfg!(target_os = "macos") { "Ombra.app" } else { "Ombra" };
const LEGACY_APP_NAME: &str = if cfg!(target_os = "macos") { "Chromium.app" } else { "Chromium" };
/// Версия сборки, которую Local API называет, пока движок не установлен.
pub const EXPECTED_VERSION: &str = "158.0.8127.37";

const MAX_FILE: u64 = 2 * 1024 * 1024 * 1024;
const MAX_UNPACKED: u64 = 8 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

/// Папка локальной/разработческой сборки в корне `engines/`: приоритет у `Ombra.app`/`Ombra`,
/// с обратной совместимостью для прежнего `Chromium.app`/`Chromium`.
pub fn app_dir(engines_dir: &Path) -> PathBuf {
    let modern = engines_dir.join(APP_NAME);
    if modern.symlink_metadata().is_ok() {
        return modern;
    }
    let legacy = engines_dir.join(LEGACY_APP_NAME);
    if legacy.symlink_metadata().is_ok() {
        return legacy;
    }
    modern
}

/// Папка приложения внутри `engines/versions/<version>/`:
/// поддерживает как Chromium 158 (`Ombra.app`, прямой `ombra`/`ombra.exe`), так и прежние (`Chromium.app`, `Chromium/chrome`).
pub fn version_app_dir(version_dir: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let modern = version_dir.join("Ombra.app");
        if modern.is_dir() {
            return modern;
        }
        let legacy = version_dir.join("Chromium.app");
        if legacy.is_dir() {
            return legacy;
        }
        modern
    }
    #[cfg(not(target_os = "macos"))]
    {
        if binary_path(version_dir).is_file() {
            return version_dir.to_path_buf();
        }
        for name in [APP_NAME, LEGACY_APP_NAME] {
            let candidate = version_dir.join(name);
            if candidate.is_dir() {
                return candidate;
            }
        }
        version_dir.to_path_buf()
    }
}

pub fn binary_path(app: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let modern = app.join("Contents/MacOS/Ombra");
        if modern.is_file() {
            return modern;
        }
        let legacy = app.join("Contents/MacOS/Chromium");
        if legacy.is_file() {
            return legacy;
        }
        if app.file_name().is_some_and(|n| n == LEGACY_APP_NAME) {
            legacy
        } else {
            modern
        }
    }
    #[cfg(windows)]
    {
        let modern = app.join("ombra.exe");
        if modern.is_file() {
            return modern;
        }
        let legacy = app.join("chrome.exe");
        if legacy.is_file() {
            return legacy;
        }
        modern
    }
    #[cfg(all(not(target_os = "macos"), not(windows)))]
    {
        let modern = app.join("ombra");
        if modern.is_file() {
            return modern;
        }
        let legacy = app.join("chrome");
        if legacy.is_file() {
            return legacy;
        }
        modern
    }
}

fn valid_chromium(version: &str) -> bool {
    version.split('.').count() == 4 && version.split('.').all(|v| v.parse::<u32>().is_ok())
}

/// Читает версию Chromium из `ombra-build.json` (с проверкой платформы); на macOS при отсутствии
/// `ombra-build.json` берёт `CFBundleShortVersionString` из `Contents/Info.plist`.
pub fn read_version(app: &Path) -> Option<String> {
    let meta = catalog::metadata_path(app);
    match fs::read(&meta) {
        Ok(bytes) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            if value.get("platform")?.as_str()? != platform() {
                return None;
            }
            let version = value.get("chromium")?.as_str()?;
            return valid_chromium(version).then(|| version.to_string());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return None,
    }
    #[cfg(target_os = "macos")]
    {
        const KEY: &str = "<key>CFBundleShortVersionString</key>";
        let text = fs::read_to_string(app.join("Contents/Info.plist")).ok()?;
        let after_key = &text[text.find(KEY)? + KEY.len()..];
        let start = after_key.find("<string>")? + "<string>".len();
        let end = start + after_key[start..].find("</string>")?;
        let version = after_key[start..end].trim();
        (!version.is_empty()).then(|| version.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// Первое число версии: `158.0.8127.37` → 158; мусор → 0.
pub fn major(version: &str) -> u32 {
    version.split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0)
}

/// Сборка в слоте: есть бинарник и версия. Symlink на папку сборки разработчика подходит.
pub fn installed(engines_dir: &Path) -> Option<EngineBuild> {
    catalog::active(engines_dir)
}

/// Снимает атрибуты карантина: сборка не нотаризована, Gatekeeper убивает процесс с карантином при первом запуске.
pub fn clear_quarantine(app: &Path) -> Result<(), EngineError> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("xattr").arg("-cr").arg(app).status()?;
        if !status.success() {
            return Err(EngineError::Install({
                log::warn!("xattr -cr завершился с {status}");
                "Не удалось подготовить браузер к запуску".to_string()
            }));
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
    Ok(())
}

/// Ручной импорт закрепляет сборку; сетевое обновление сохраняет автоматический режим.
pub fn import_archive(engines_dir: &Path, archive: &Path) -> Result<EngineBuild, EngineError> {
    install_archive(engines_dir, archive, None, None, false, &CancellationToken::new())
}

pub fn install_archive(
    engines_dir: &Path,
    archive: &Path,
    expected: Option<&str>,
    expected_commit: Option<&str>,
    automatic: bool,
    cancel: &CancellationToken,
) -> Result<EngineBuild, EngineError> {
    if cancel.is_cancelled() {
        return Err(EngineError::Update("EngineUpdateCancelled"));
    }
    fs::create_dir_all(engines_dir)?;
    let staging = tempfile::tempdir_in(engines_dir)?;
    let unpacked = staging.path().join("unpacked");
    fs::create_dir(&unpacked)?;
    unpack_archive(archive, &unpacked, cancel)?;
    let app = find_unpacked_app(&unpacked);
    if !binary_path(&app).is_file() {
        return Err(EngineError::Install(format!("в архиве нет {APP_NAME} для этой системы")));
    }
    let build = catalog::inspect(&app, "legacy")
        .ok_or_else(|| EngineError::Install("не удалось проверить версию и платформу сборки".into()))?;
    if expected.is_some_and(|version| version != build.version) {
        return Err(EngineError::Install("версия архива не совпадает с каталогом".into()));
    }
    if let Some(commit) = expected_commit {
        let bytes = fs::read(catalog::metadata_path(&app))
            .map_err(|_| EngineError::Install("в архиве нет метаданных сборки".into()))?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| EngineError::Install("не удалось прочитать метаданные сборки".into()))?;
        if value.get("commit").and_then(|v| v.as_str()) != Some(commit) {
            return Err(EngineError::Install("коммит сборки не совпадает с подписанным выпуском".into()));
        }
    }
    let archive_digest = file_sha256(archive, cancel)?;
    let binary_digest = file_sha256(&build.binary, cancel)?;
    let _lock = catalog::mutation_lock(engines_dir)?;
    let versions = engines_dir.join("versions");
    fs::create_dir_all(&versions)?;
    let destination = versions.join(&build.version);
    if destination.symlink_metadata().is_ok() {
        // Номер версии неизменяем: повтор установки тех же байтов безопасен, подмена запрещена.
        let saved = fs::read_to_string(destination.join("archive.sha256")).ok();
        if destination.symlink_metadata()?.file_type().is_symlink()
            || saved.as_deref().map(str::trim) != Some(archive_digest.as_str())
        {
            return Err(EngineError::Install("эта версия уже установлена из другого архива".into()));
        }
        return catalog::activate(engines_dir, &build.version, automatic);
    }
    clear_quarantine(&app)?;
    let ready = staging.path().join("ready");
    #[cfg(target_os = "macos")]
    {
        fs::create_dir(&ready)?;
        let bundle_name = match app.file_name().and_then(|n| n.to_str()) {
            Some("Ombra.app") => "Ombra.app",
            Some("Chromium.app") => "Chromium.app",
            _ => APP_NAME,
        };
        fs::rename(&app, ready.join(bundle_name))?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        // На Linux и Windows бинарник (`ombra` / `ombra.exe`) лежит прямо в `versions/<version>/` — как в SDK.
        fs::rename(&app, &ready)?;
    }
    fs::write(ready.join("archive.sha256"), format!("{archive_digest}\n"))?;
    fs::write(ready.join("binary.sha256"), format!("{binary_digest}\n"))?;
    if cancel.is_cancelled() {
        return Err(EngineError::Update("EngineUpdateCancelled"));
    }
    fs::rename(ready, &destination)?;
    // При ошибке записи выбора старая активная сборка сохранится, новая останется доступной в списке.
    catalog::activate(engines_dir, &build.version, automatic)
}

fn find_unpacked_app(unpacked: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        for name in [APP_NAME, LEGACY_APP_NAME] {
            let candidate = unpacked.join(name);
            if candidate.is_dir() {
                return candidate;
            }
        }
        unpacked.to_path_buf()
    }
    #[cfg(not(target_os = "macos"))]
    {
        if binary_path(unpacked).is_file() {
            return unpacked.to_path_buf();
        }
        for name in [APP_NAME, LEGACY_APP_NAME] {
            let candidate = unpacked.join(name);
            if candidate.is_dir() {
                return candidate;
            }
        }
        unpacked.to_path_buf()
    }
}

fn file_sha256(path: &Path, cancel: &CancellationToken) -> Result<String, EngineError> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 128 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(EngineError::Update("EngineUpdateCancelled"));
        }
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn relative(path: &Path) -> Result<PathBuf, EngineError> {
    let text = path.to_str().ok_or_else(|| EngineError::Install("недопустимый путь в архиве".into()))?;
    if text.len() > 4096 || text.contains(['\\', ':', '\0']) {
        return Err(EngineError::Install("недопустимый путь в архиве".into()));
    }
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(value) => result.push(value),
            Component::CurDir => (),
            _ => return Err(EngineError::Install("путь в архиве выходит за пределы папки".into())),
        }
    }
    Ok(result)
}

fn link_target(parent: &Path, target: &Path) -> Result<PathBuf, EngineError> {
    let text = target.to_str().ok_or_else(|| EngineError::Install("недопустимая ссылка в архиве".into()))?;
    if text.is_empty() || text.len() > 4096 || text.contains(['\\', ':', '\0']) {
        return Err(EngineError::Install("недопустимая ссылка в архиве".into()));
    }
    let mut resolved = parent.to_path_buf();
    for part in target.components() {
        match part {
            Component::Normal(value) => resolved.push(value),
            Component::CurDir => (),
            Component::ParentDir if resolved.pop() => (),
            _ => return Err(EngineError::Install("ссылка в архиве выходит за пределы папки".into())),
        }
    }
    Ok(resolved)
}

fn ensure_dir(path: &Path) -> Result<(), EngineError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(EngineError::Install("файл в архиве перекрывает каталог".into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(fs::create_dir(path)?),
        Err(error) => Err(error.into()),
    }
}

fn ensure_parents(root: &Path, path: &Path) -> Result<(), EngineError> {
    let mut cursor = root.to_path_buf();
    if let Some(parent) = path.parent() {
        for part in parent.components() {
            cursor.push(part);
            ensure_dir(&cursor)?;
        }
    }
    Ok(())
}

fn copy_entry(reader: &mut impl Read, path: &Path, size: u64, mode: u32, cancel: &CancellationToken) -> Result<(), EngineError> {
    let mut output = fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut buffer = [0; 128 * 1024];
    let mut written = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(EngineError::Update("EngineUpdateCancelled"));
        }
        let n = reader.read(&mut buffer).map_err(|e| EngineError::Install(e.to_string()))?;
        if n == 0 {
            break;
        }
        written = written.checked_add(n as u64).ok_or_else(|| EngineError::Install("слишком большой файл в архиве".into()))?;
        if written > size {
            return Err(EngineError::Install("размер файла в архиве превышает заявленный".into()));
        }
        output.write_all(&buffer[..n])?;
    }
    if written != size {
        return Err(EngineError::Install("файл в архиве обрезан".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        output.set_permissions(fs::Permissions::from_mode(mode & 0o777))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(())
}

fn check_budget(size: u64, total: &mut u64, entries: usize) -> Result<(), EngineError> {
    *total = total.checked_add(size).ok_or_else(|| EngineError::Install("архив слишком большой".into()))?;
    if size > MAX_FILE || *total > MAX_UNPACKED || entries > MAX_ENTRIES {
        return Err(EngineError::Install("архив превышает допустимый размер".into()));
    }
    Ok(())
}

fn unpack_archive(archive: &Path, destination: &Path, cancel: &CancellationToken) -> Result<(), EngineError> {
    if cancel.is_cancelled() {
        return Err(EngineError::Update("EngineUpdateCancelled"));
    }
    let file = fs::File::open(archive)?;
    let mut seen = HashSet::new();
    let mut total = 0u64;
    if archive.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("zip")) {
        let mut zip = zip::ZipArchive::new(file).map_err(|error| EngineError::Install(error.to_string()))?;
        if zip.len() > MAX_ENTRIES {
            return Err(EngineError::Install("слишком много файлов в архиве".into()));
        }
        for i in 0..zip.len() {
            if cancel.is_cancelled() {
                return Err(EngineError::Update("EngineUpdateCancelled"));
            }
            let mut entry = zip.by_index(i).map_err(|error| EngineError::Install(error.to_string()))?;
            let path = relative(Path::new(entry.name()))?;
            if path.as_os_str().is_empty() || !seen.insert(path.clone()) {
                return Err(EngineError::Install("повторяющийся или пустой путь в архиве".into()));
            }
            let mode = entry.unix_mode().unwrap_or(0o644);
            if !matches!(mode & 0o170000, 0 | 0o100000 | 0o040000) {
                return Err(EngineError::Install("Архив браузера содержит неподдерживаемую ссылку на файл или папку".into()));
            }
            check_budget(entry.size(), &mut total, seen.len())?;
            ensure_parents(destination, &path)?;
            let output = destination.join(path);
            if entry.is_dir() {
                ensure_dir(&output)?;
            } else {
                let size = entry.size();
                copy_entry(&mut entry, &output, size, mode, cancel)?;
            }
        }
        return Ok(());
    }
    let reader: Box<dyn Read> = if archive.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("xz")) {
        Box::new(xz2::read::XzDecoder::new(file))
    } else {
        Box::new(flate2::read::GzDecoder::new(file))
    };
    let mut tar = tar::Archive::new(reader);
    let mut links = Vec::new();
    for entry in tar.entries().map_err(|e| EngineError::Install(e.to_string()))? {
        if cancel.is_cancelled() {
            return Err(EngineError::Update("EngineUpdateCancelled"));
        }
        let mut entry = entry.map_err(|e| EngineError::Install(e.to_string()))?;
        let path = relative(&entry.path().map_err(|e| EngineError::Install(e.to_string()))?)?;
        let kind = entry.header().entry_type();
        if path.as_os_str().is_empty() && kind.is_dir() {
            continue;
        }
        if path.as_os_str().is_empty() || !seen.insert(path.clone()) {
            return Err(EngineError::Install("повторяющийся или пустой путь в архиве".into()));
        }
        let size = entry.header().size().map_err(|e| EngineError::Install(e.to_string()))?;
        check_budget(size, &mut total, seen.len())?;
        ensure_parents(destination, &path)?;
        let output = destination.join(&path);
        if kind.is_dir() {
            ensure_dir(&output)?;
        } else if kind.is_file() {
            let mode = entry.header().mode().map_err(|e| EngineError::Install(e.to_string()))?;
            copy_entry(&mut entry, &output, size, mode, cancel)?;
        } else if kind.is_symlink() || kind.is_hard_link() {
            let target = entry
                .link_name()
                .map_err(|e| EngineError::Install(e.to_string()))?
                .ok_or_else(|| EngineError::Install("в архиве ссылка без цели".into()))?
                .into_owned();
            if kind.is_symlink() {
                link_target(path.parent().unwrap_or(Path::new("")), &target)?;
            } else {
                relative(&target)?;
            }
            links.push((path, target, kind.is_symlink()));
        } else {
            return Err(EngineError::Install("неподдерживаемый тип файла в архиве".into()));
        }
    }
    for (path, target, _) in links.iter().filter(|entry| !entry.2) {
        let source = destination.join(relative(target)?);
        if !fs::symlink_metadata(&source)?.is_file() {
            return Err(EngineError::Install("недопустимая жёсткая ссылка в архиве".into()));
        }
        ensure_parents(destination, path)?;
        fs::hard_link(source, destination.join(path))?;
    }
    for (path, target, _) in links.iter().filter(|entry| entry.2) {
        ensure_parents(destination, path)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, destination.join(path))?;
        #[cfg(not(unix))]
        {
            let _ = target;
            return Err(EngineError::Install("неподдерживаемая ссылка в архиве".into()));
        }
    }
    let root = destination.canonicalize()?;
    for (path, _, _) in links {
        if !destination
            .join(path)
            .canonicalize()
            .map_err(|e| EngineError::Install(e.to_string()))?
            .starts_with(&root)
        {
            return Err(EngineError::Install("ссылка в архиве выходит за пределы папки".into()));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Поддельная сборка: `Ombra.app` (macOS) или `Ombra` (Linux) с бинарником и метаданными; `version: None` — без версии.
    pub(crate) fn fake_app(parent: &Path, version: Option<&str>) -> PathBuf {
        fake_app_named(parent, APP_NAME, version)
    }

    pub(crate) fn fake_app_named(parent: &Path, name: &str, version: Option<&str>) -> PathBuf {
        let app = parent.join(name);
        if cfg!(target_os = "macos") {
            fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        } else {
            fs::create_dir_all(&app).unwrap();
        }
        let binary = if cfg!(target_os = "macos") {
            app.join(if name == LEGACY_APP_NAME { "Contents/MacOS/Chromium" } else { "Contents/MacOS/Ombra" })
        } else {
            app.join(if name == LEGACY_APP_NAME { "chrome" } else { "ombra" })
        };
        fs::write(&binary, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        if let Some(version) = version {
            if !cfg!(target_os = "macos") {
                fs::write(app.join("ombra-build.json"), serde_json::json!({"chromium": version, "platform": platform()}).to_string()).unwrap();
            } else {
                fs::write(
                    app.join("Contents/Info.plist"),
                    format!(
                        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n\
                         \t<key>CFBundleName</key>\n\t<string>{name}</string>\n\
                         \t<key>CFBundleShortVersionString</key>\n\t<string>{version}</string>\n\
                         \t<key>CFBundleVersion</key>\n\t<string>8127.37</string>\n</dict></plist>\n"
                    ),
                )
                .unwrap();
            }
        }
        app
    }

    /// `.tar.gz`, в котором папка `src` лежит под именем `name_in_archive`.
    pub(crate) fn archive_of(dir: &Path, src: &Path, name_in_archive: &str) -> PathBuf {
        let path = dir.join("engine.tar.gz");
        let gz = flate2::write::GzEncoder::new(fs::File::create(&path).unwrap(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(gz);
        builder.append_dir_all(name_in_archive, src).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        path
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn version_comes_from_metadata_or_plist() {
        let dir = tempfile::tempdir().unwrap();
        let app = fake_app(dir.path(), Some("158.0.8127.37"));
        assert_eq!(read_version(&app).as_deref(), Some("158.0.8127.37"));
        assert_eq!(major("158.0.8127.37"), 158);
        assert_eq!(major("garbage"), 0);
        if cfg!(target_os = "macos") {
            fs::write(app.join("Contents/Info.plist"), "<plist><dict></dict></plist>").unwrap();
        } else {
            fs::remove_file(app.join("ombra-build.json")).unwrap();
        }
        assert_eq!(read_version(&app), None);
    }

    #[test]
    fn supports_both_modern_ombra_and_legacy_chromium_layouts() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        fake_app_named(&engines, LEGACY_APP_NAME, Some("151.0.7922.108"));
        let legacy = installed(&engines).expect("legacy Chromium layout still works");
        assert_eq!(legacy.version, "151.0.7922.108");
        let src = dir.path().join("src");
        let modern = fake_app(&src, Some("158.0.8127.37"));
        let archive = archive_of(dir.path(), &modern, APP_NAME);
        let imported = import_archive(&engines, &archive).unwrap();
        assert_eq!(imported.version, "158.0.8127.37");
        assert_eq!(catalog::list(&engines).unwrap().versions.len(), 2);
        assert!(engines.join("versions/158.0.8127.37/binary.sha256").is_file());
        assert!(engines.join("versions/158.0.8127.37/archive.sha256").is_file());
    }

    #[test]
    fn installed_sees_a_folder_or_a_symlink_in_the_slot() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        assert_eq!(installed(&engines), None, "пустой слот");
        let build_dir = dir.path().join("out");
        fake_app(&build_dir, Some("158.0.8127.37"));
        fs::create_dir_all(&engines).unwrap();
        std::os::unix::fs::symlink(build_dir.join(APP_NAME), app_dir(&engines)).unwrap();
        let build = installed(&engines).expect("symlink на сборку разработчика");
        assert_eq!(build.version, "158.0.8127.37");
        assert_eq!(build.binary, binary_path(&app_dir(&engines)));
        fs::remove_file(app_dir(&engines)).unwrap();
        fake_app(&engines, None);
        assert_eq!(installed(&engines), None, "без версии сборка не считается установленной");
    }

    #[test]
    fn import_keeps_the_old_browser_and_can_switch_back() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        fake_app(&engines, Some("149.0.1.1"));
        let src = dir.path().join("src");
        fake_app(&src, Some("158.0.8127.37"));
        let archive = archive_of(dir.path(), &src.join(APP_NAME), APP_NAME);
        let build = import_archive(&engines, &archive).unwrap();
        assert_eq!(build.version, "158.0.8127.37");
        let expected_app = version_app_dir(&engines.join("versions/158.0.8127.37"));
        assert_eq!(build.binary, binary_path(&expected_app));
        assert_eq!(installed(&engines).unwrap().version, "158.0.8127.37");
        assert_eq!(read_version(&app_dir(&engines)).as_deref(), Some("149.0.1.1"));
        let versions = catalog::list(&engines).unwrap();
        assert_eq!(versions.versions.len(), 2);
        assert!(!versions.automatic, "ручной импорт закрепляет сборку");
        catalog::select(&engines, "legacy", false).unwrap();
        assert_eq!(installed(&engines).unwrap().version, "149.0.1.1");
        assert!(build.binary.is_file(), "новая сборка тоже сохранена");
        catalog::select(&engines, "158.0.8127.37", false).unwrap();
        let mode = fs::metadata(&build.binary).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "бинарник остался исполняемым");
    }

    #[test]
    fn import_preserves_a_development_symlink_and_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        let dev = dir.path().join("out");
        fake_app(&dev, Some("149.0.1.1"));
        fs::create_dir_all(&engines).unwrap();
        std::os::unix::fs::symlink(dev.join(APP_NAME), app_dir(&engines)).unwrap();
        let src = dir.path().join("src");
        fake_app(&src, Some("158.0.8127.37"));
        let archive = archive_of(dir.path(), &src.join(APP_NAME), APP_NAME);
        import_archive(&engines, &archive).unwrap();
        assert!(app_dir(&engines).symlink_metadata().unwrap().file_type().is_symlink(), "ссылка разработчика сохранена");
        catalog::select(&engines, "legacy", false).unwrap();
        assert_eq!(installed(&engines).unwrap().version, "149.0.1.1");
        assert_eq!(read_version(&dev.join(APP_NAME)).as_deref(), Some("149.0.1.1"), "сборка разработчика цела");
    }

    #[test]
    fn import_rejects_an_archive_without_the_app_at_its_root() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        fake_app(&engines, Some("149.0.1.1"));
        let src = dir.path().join("src");
        fake_app(&src, Some("158.0.8127.37"));
        let nested = archive_of(dir.path(), &src, "Nested");
        let err = import_archive(&engines, &nested).unwrap_err();
        assert!(err.to_string().contains(&format!("нет {APP_NAME}")), "{err}");
        assert_eq!(installed(&engines).unwrap().version, "149.0.1.1", "слот не тронут");
        assert_eq!(names(&engines), [APP_NAME], "временная папка удалена");
    }

    #[test]
    fn import_rejects_an_app_without_a_version() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        let src = dir.path().join("src");
        fake_app(&src, None);
        let archive = archive_of(dir.path(), &src.join(APP_NAME), APP_NAME);
        let err = import_archive(&engines, &archive).unwrap_err();
        assert!(err.to_string().contains("версию"), "{err}");
        assert_eq!(installed(&engines), None);
    }

    #[test]
    fn reads_xz_and_zip_archives_without_changing_their_contents() {
        let dir = tempfile::tempdir().unwrap();
        let xz_path = dir.path().join("engine.tar.xz");
        let xz = xz2::write::XzEncoder::new(fs::File::create(&xz_path).unwrap(), 1);
        let mut tar = tar::Builder::new(xz);
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "ombra", &b"hello"[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let zip_path = dir.path().join("engine.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
        zip.start_file("ombra.exe", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"hello").unwrap();
        zip.finish().unwrap();
        for (archive, name) in [(&xz_path, "ombra"), (&zip_path, "ombra.exe")] {
            let output = tempfile::tempdir().unwrap();
            unpack_archive(archive, output.path(), &CancellationToken::new()).unwrap();
            assert_eq!(fs::read(output.path().join(name)).unwrap(), b"hello");
        }
    }

    #[test]
    fn zip_and_tar_cannot_escape_the_staging_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("engine.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file("../escaped", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"bad").unwrap();
        zip.finish().unwrap();
        let output = dir.path().join("staging");
        fs::create_dir(&output).unwrap();
        assert!(unpack_archive(&path, &output, &CancellationToken::new()).is_err());
        assert!(!dir.path().join("escaped").exists());

        let tar_path = dir.path().join("escape.tar.gz");
        let gz = flate2::write::GzEncoder::new(fs::File::create(&tar_path).unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_cksum();
        tar.append_link(&mut header, "escape", "../../etc/passwd").unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let output2 = dir.path().join("staging2");
        fs::create_dir(&output2).unwrap();
        assert!(unpack_archive(&tar_path, &output2, &CancellationToken::new()).is_err());
    }

    #[test]
    fn refuses_a_manifest_for_another_platform() {
        let dir = tempfile::tempdir().unwrap();
        let app = fake_app(dir.path(), Some("158.0.8127.37"));
        let meta = catalog::metadata_path(&app);
        fs::create_dir_all(meta.parent().unwrap()).unwrap();
        fs::write(meta, r#"{"chromium":"158.0.8127.37","platform":"windows-x64"}"#).unwrap();
        assert!(read_version(&app).is_none());
    }
}
