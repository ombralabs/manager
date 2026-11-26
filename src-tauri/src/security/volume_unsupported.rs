//! APFS profile vaults are not portable. Refuse protected data instead of opening it without encryption.
use crate::{app::CommandError, paths::Paths};
use std::{fs, path::Path};

pub(super) const LEGACY: &[&str] = &["ombra.db", "ombra.db-wal", "ombra.db-shm", "dev-secrets.key", "secrets.key", "profiles", "extensions", "flows", "run", "logs", "logs/chromium", "logs/relay", "logs/flows"];
fn unsupported() -> CommandError { super::volume_unsupported() }

pub(super) fn private_dir(path: &Path) -> Result<(), CommandError> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_dir() || meta.file_type().is_symlink() { return Err(unsupported()); }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.uid() != unsafe { libc::geteuid() } { return Err(unsupported()); }
        }
    } else { fs::create_dir_all(path)?; }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(super) fn atomic_json(path: &Path, value: &impl serde::Serialize) -> Result<(), CommandError> {
    let parent = path.parent().ok_or_else(unsupported)?;
    private_dir(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut file, value).map_err(|_| unsupported())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub(super) fn operation_lock(paths: &Paths) -> Result<fs::File, CommandError> {
    let file = crate::sys::private_options().create(true).truncate(false).write(true).open(paths.root().join(".vault-operation.lock"))?;
    file.lock()?;
    Ok(file)
}

pub(super) fn seal_stale(paths: &Paths) -> Result<(), CommandError> {
    if paths.vault_image().exists() || paths.security_config().exists() { Err(unsupported()) } else { Ok(()) }
}
pub(super) fn mounted(_: &Paths) -> bool { false }
pub(super) fn create(_: &Paths, _: &str) -> Result<(), CommandError> { Err(unsupported()) }
pub(super) fn attach(_: &Paths, _: &str) -> Result<(), CommandError> { Err(unsupported()) }
pub(super) fn detach(paths: &Paths) -> Result<(), CommandError> { seal_stale(paths) }
pub(super) fn legacy_items(_: &Paths) -> Vec<String> { vec![] }
pub(super) fn remaining_logs(_: &Paths) -> Result<(), CommandError> { Err(unsupported()) }
pub(super) struct LegacyLocks;
pub(super) fn lock_legacy(_: &Paths) -> Result<LegacyLocks, CommandError> { Err(unsupported()) }
pub(super) fn item_matches(_: &Paths, _: &str) -> Result<bool, CommandError> { Err(unsupported()) }
pub(super) fn migrate_item(_: &Paths, _: &str) -> Result<(), CommandError> { Err(unsupported()) }
pub(super) fn remove_legacy(_: &Paths, _: &str) -> Result<(), CommandError> { Err(unsupported()) }
pub(super) fn spawn_guard(_: &Paths) -> Result<(), CommandError> { Err(unsupported()) }
pub(super) fn stop_browsers(_: &Paths) -> Result<(), CommandError> { Err(unsupported()) }
pub fn guard(_: u32, _: Paths) -> Result<(), CommandError> { Err(unsupported()) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_existing_vault_and_preserves_data() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        assert!(seal_stale(&paths).is_ok());
        fs::write(paths.security_config(), "protected").unwrap();
        assert!(seal_stale(&paths).is_err());
        assert!(attach(&paths, "password").is_err());
        assert!(create(&paths, "password").is_err());
        assert_eq!(fs::read_to_string(paths.security_config()).unwrap(), "protected");
    }
}
