//! `run/<id>.pid`: по ним при старте приложения находятся браузеры, пережившие его падение.
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PidRecord {
    pub pid: u32,
    pub binary: PathBuf,
    pub profile_dir: PathBuf,
    pub started_at: i64,
}

pub fn path(run_dir: &Path, id: &str) -> PathBuf {
    run_dir.join(format!("{id}.pid"))
}

/// Запись через временный файл и переименование: при падении посреди записи не останется половины JSON.
pub fn write(run_dir: &Path, id: &str, record: &PidRecord) -> io::Result<()> {
    std::fs::create_dir_all(run_dir)?;
    let target = path(run_dir, id);
    let tmp = target.with_extension("pid.tmp");
    std::fs::write(&tmp, serde_json::to_vec(record).map_err(io::Error::other)?)?;
    std::fs::rename(tmp, target)
}

pub fn remove(run_dir: &Path, id: &str) -> io::Result<()> {
    match std::fs::remove_file(path(run_dir, id)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Все PID-файлы: идентификатор профиля и запись (`None`, если файл не читается).
pub fn list(run_dir: &Path) -> io::Result<Vec<(String, Option<PidRecord>)>> {
    let entries = match std::fs::read_dir(run_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pid") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
        let record = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        found.push((id, record));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_list_remove() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        assert!(list(&run).unwrap().is_empty());
        let record = PidRecord {
            pid: 4242,
            binary: "/X/Chromium".into(),
            profile_dir: "/data/profiles/k7f2q9".into(),
            started_at: 1_790_000_000,
        };
        write(&run, "k7f2q9", &record).unwrap();
        assert_eq!(path(&run, "k7f2q9"), run.join("k7f2q9.pid"));
        std::fs::write(run.join("broken.pid"), "{").unwrap();
        std::fs::write(run.join("notes.txt"), "x").unwrap();
        assert_eq!(list(&run).unwrap(), vec![("broken".to_string(), None), ("k7f2q9".to_string(), Some(record))]);
        remove(&run, "k7f2q9").unwrap();
        remove(&run, "k7f2q9").unwrap();
        assert_eq!(list(&run).unwrap().len(), 1);
    }
}
