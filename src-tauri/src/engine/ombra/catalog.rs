//! Неизменяемые папки версий и атомарный выбор сборки для следующих запусков.
use super::{install, version::{Version, platform}};
use crate::engine::{EngineBuild, EngineError};
use crate::lock::DirLock;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct InstalledVersion {
    pub id: String,
    pub version: String,
    pub chromium: String,
    pub binary: PathBuf,
    pub active: bool,
}

impl InstalledVersion {
    pub fn build(&self) -> EngineBuild {
        EngineBuild { version: self.chromium.clone(), binary: self.binary.clone() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub versions: Vec<InstalledVersion>,
    pub automatic: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Selection {
    pub active: Option<String>,
    #[serde(default = "enabled")]
    pub automatic: bool,
    #[serde(default)]
    pub stable_serial: u64,
}

fn enabled() -> bool { true }

pub(crate) fn valid_id(id: &str) -> bool {
    if id.parse::<Version>().is_ok() { return true; }
    id.split_once("-0+dev.").is_some_and(|(base, commit)| {
        base.parse::<Version>().is_ok() && (7..=64).contains(&commit.len()) && commit.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

pub(crate) fn metadata_path(app: &Path) -> PathBuf {
    app.join(if cfg!(target_os = "macos") { "Contents/Resources/ombra-build.json" } else { "ombra-build.json" })
}

/// Старый импорт без метаданных тоже виден; сетевой установщик отдельно требует точную версию выпуска.
pub(crate) fn inspect(app: &Path, id: &str) -> Option<InstalledVersion> {
    let chromium = install::read_version(app)?;
    chromium.parse::<Version>().ok()?;
    let binary = install::binary_path(app);
    if !binary.is_file() || !binary.canonicalize().ok()?.starts_with(app.canonicalize().ok()?) { return None; }
    let version = match std::fs::read(metadata_path(app)) {
        Ok(bytes) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            if value.get("platform")?.as_str()? != platform() || value.get("chromium")?.as_str()? != chromium { return None; }
            let version = value.get("version").and_then(|v| v.as_str()).unwrap_or(&chromium);
            if !valid_id(version) || version.split('-').next()? != chromium { return None; }
            version.to_string()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => chromium.clone(),
        Err(_) => return None,
    };
    if id != "legacy" && id != version { return None; }
    Some(InstalledVersion { id: id.into(), version, chromium, binary, active: false })
}

pub(crate) fn read_selection(engines: &Path) -> Result<Selection, EngineError> {
    match std::fs::read(engines.join("selection.json")) {
        Ok(bytes) => {
            let selection: Selection = serde_json::from_slice(&bytes).map_err(|_| EngineError::Install("Не удалось прочитать выбранную версию браузера. Выберите версию заново.".into()))?;
            if selection.active.as_ref().is_some_and(|id| id != "legacy" && !valid_id(id)) {
                return Err(EngineError::Install("недопустимая версия браузера".into()));
            }
            Ok(selection)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let legacy = install::app_dir(engines);
            let development = legacy.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink())
                || inspect(&legacy, "legacy").is_some_and(|v| v.version.contains("+dev."));
            Ok(Selection { active: None, automatic: !development, stable_serial: 0 })
        }
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn write_selection(engines: &Path, selection: &Selection) -> Result<(), EngineError> {
    std::fs::create_dir_all(engines)?;
    let mut file = tempfile::NamedTempFile::new_in(engines)?;
    serde_json::to_writer(&mut file, selection).map_err(|e| EngineError::Install(e.to_string()))?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(engines.join("selection.json")).map_err(|e| e.error)?;
    Ok(())
}

pub(crate) fn mutation_lock(engines: &Path) -> Result<DirLock, EngineError> {
    DirLock::acquire(engines).map_err(|_| EngineError::Install("другая операция уже меняет список браузеров".into()))
}

pub fn list(engines: &Path) -> Result<Catalog, EngineError> {
    let selection = read_selection(engines)?;
    let mut versions: Vec<_> = inspect(&install::app_dir(engines), "legacy").into_iter().collect();
    match std::fs::read_dir(engines.join("versions")) {
        Ok(entries) => for entry in entries {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if !entry.file_type()?.is_dir() || !valid_id(&id) { continue; }
            let app = install::version_app_dir(&entry.path());
            if let Some(version) = inspect(&app, &id) { versions.push(version); }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let active = selection.active.as_deref().unwrap_or("legacy");
    for version in &mut versions { version.active = version.id == active; }
    versions.sort_by(|a, b| b.version.parse::<Version>().ok().cmp(&a.version.parse::<Version>().ok()).then_with(|| b.version.cmp(&a.version)));
    Ok(Catalog { versions, automatic: selection.automatic })
}

pub fn active(engines: &Path) -> Option<EngineBuild> {
    list(engines).ok()?.versions.into_iter().find(|v| v.active).map(|v| v.build())
}

pub fn select(engines: &Path, id: &str, automatic: bool) -> Result<EngineBuild, EngineError> {
    let _lock = mutation_lock(engines)?;
    activate(engines, id, automatic)
}

/// Вызывается под mutation_lock, в том числе после успешной установки новой папки.
pub(crate) fn activate(engines: &Path, id: &str, automatic: bool) -> Result<EngineBuild, EngineError> {
    let version = list(engines)?.versions.into_iter().find(|v| v.id == id)
        .ok_or_else(|| EngineError::Install("установленная версия браузера не найдена".into()))?;
    let mut selection = read_selection(engines)?;
    selection.active = Some(id.into());
    selection.automatic = automatic;
    write_selection(engines, &selection)?;
    Ok(version.build())
}

pub fn set_automatic(engines: &Path, automatic: bool) -> Result<(), EngineError> {
    let _lock = mutation_lock(engines)?;
    let mut selection = read_selection(engines)?;
    selection.automatic = automatic;
    write_selection(engines, &selection)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use super::super::install::tests::{archive_of, fake_app};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn keeps_every_version_and_persists_selection() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        for version in ["150.0.1.1", "151.0.1.1", "158.0.8127.37"] {
            let source = tempfile::tempdir().unwrap();
            let app = fake_app(source.path(), Some(version));
            let archive = archive_of(source.path(), &app, install::APP_NAME);
            install::import_archive(&engines, &archive).unwrap();
        }
        assert_eq!(list(&engines).unwrap().versions.len(), 3);
        select(&engines, "150.0.1.1", false).unwrap();
        assert_eq!(active(&engines).unwrap().version, "150.0.1.1");
        assert!(!list(&engines).unwrap().automatic);
        for version in list(&engines).unwrap().versions { assert!(version.binary.is_file()); }
        set_automatic(&engines, true).unwrap();
        assert!(list(&engines).unwrap().automatic);
    }

    #[test]
    fn a_repeat_reuses_the_folder_but_different_bytes_cannot_replace_it() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        let source = tempfile::tempdir().unwrap();
        let app = fake_app(source.path(), Some("158.0.8127.37"));
        let archive = archive_of(source.path(), &app, install::APP_NAME);
        let first = install::import_archive(&engines, &archive).unwrap();
        assert_eq!(install::import_archive(&engines, &archive).unwrap(), first);
        std::fs::write(install::binary_path(&app), "changed").unwrap();
        let archive = archive_of(source.path(), &app, install::APP_NAME);
        assert!(install::import_archive(&engines, &archive).is_err());
        assert_eq!(std::fs::read_to_string(first.binary).unwrap(), "#!/bin/sh\nexit 0\n");
        assert_eq!(list(&engines).unwrap().versions.len(), 1);
    }

    #[test]
    fn invalid_selection_and_archive_version_do_not_change_active_browser() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        fake_app(&engines, Some("150.0.1.1"));
        let source = tempfile::tempdir().unwrap();
        let app = fake_app(source.path(), Some("158.0.8127.37"));
        let archive = archive_of(source.path(), &app, install::APP_NAME);
        assert!(install::install_archive(&engines, &archive, Some("158.0.8127.37-2"), None, true, &CancellationToken::new()).is_err());
        for id in ["../outside", "158.0.8127.37", "/tmp/browser"] { assert!(select(&engines, id, false).is_err()); }
        assert_eq!(active(&engines).unwrap().version, "150.0.1.1");
    }
}
