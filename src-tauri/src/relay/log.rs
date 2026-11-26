use serde::{Deserialize, Serialize};
use std::fs::File;
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_BYTES: u64 = 10 * 1024 * 1024;

/// В журнале список посещённых хостов — только владельцу.
fn open_private(path: &Path) -> io::Result<File> {
    let file = crate::sys::private_options().create(true).append(true).open(path)?;
    // Файл мог остаться от прошлой версии с правами 0644.
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[derive(Debug, Serialize)]
pub struct Record<'a> {
    pub ts_ms: u128,
    pub proxy_id: i64,
    pub target: &'a str,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub ms: u64,
    pub result: &'a str,
}

/// Запись журнала, прочитанная ядром (писать — `Record`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnRecord {
    pub ts_ms: u128,
    pub proxy_id: i64,
    pub target: String,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub ms: u64,
    pub result: String,
}

/// Записи файла по порядку; нет файла — пусто, испорченные строки пропускаются.
fn read_records(path: &Path) -> io::Result<impl Iterator<Item = ConnRecord>> {
    let lines = match File::open(path) {
        Ok(file) => Some(BufReader::new(file).lines()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    Ok(lines.into_iter().flatten().filter_map(|line| serde_json::from_str(&line.ok()?).ok()))
}

/// Последние `limit` записей файла в порядке файла. Файл ≤ 10 МБ читается целиком, в памяти держится только хвост.
pub fn tail(path: &Path, limit: usize) -> io::Result<Vec<ConnRecord>> {
    let mut last = VecDeque::with_capacity(limit);
    for record in read_records(path)? {
        if last.len() == limit {
            last.pop_front();
        }
        last.push_back(record);
    }
    Ok(last.into())
}

/// `(bytes_up, bytes_down, connections, самый ранний ts_ms)` по записям файла; с `before_ms` — только завершившиеся
/// раньше этого момента.
pub fn totals(path: &Path, before_ms: Option<u128>) -> io::Result<(u64, u64, u64, Option<u128>)> {
    Ok(read_records(path)?.filter(|r| before_ms.is_none_or(|t| r.ts_ms < t)).fold((0, 0, 0, None), |(up, down, n, oldest), r| {
        (up + r.bytes_up, down + r.bytes_down, n + 1, Some(oldest.map_or(r.ts_ms, |o: u128| o.min(r.ts_ms))))
    }))
}

/// Журнал соединений профиля в JSONL: 10 МБ и одна ротация (`.jsonl.1`).
pub struct ConnLog {
    path: PathBuf,
    limit: u64,
    file: Mutex<File>,
}

impl ConnLog {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        Self::with_limit(path, MAX_BYTES)
    }

    pub fn with_limit(path: PathBuf, limit: u64) -> io::Result<Self> {
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
            // Папка могла остаться от прошлой версии с правами 0755.
            #[cfg(unix)]
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let file = open_private(&path)?;
        // Прошлая ротация могла остаться от версии, которая создавала файлы с правами 0644.
        let rotated = path.with_extension("jsonl.1");
        if rotated.exists() {
            #[cfg(unix)]
            std::fs::set_permissions(&rotated, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(Self { path, limit, file: Mutex::new(file) })
    }

    /// Ошибки записи журнала не должны ронять соединение браузера — они игнорируются.
    pub fn write(&self, record: &Record<'_>) {
        let Ok(mut line) = serde_json::to_vec(record) else { return };
        line.push(b'\n');
        let mut file = self.file.lock().expect("мьютекс журнала отравлен");
        if file.metadata().map(|m| m.len()).unwrap_or(0) + line.len() as u64 > self.limit {
            let rotated = self.path.with_extension("jsonl.1");
            if std::fs::rename(&self.path, rotated).is_ok()
                && let Ok(fresh) = open_private(&self.path)
            {
                *file = fresh;
            }
        }
        let _ = file.write_all(&line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn new_log_dir_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/relay/k7f2q9.jsonl");
        ConnLog::with_limit(path.clone(), 200).unwrap();
        assert_eq!(mode(path.parent().unwrap()), 0o700, "список посещённых хостов — только владельцу");
    }

    #[test]
    fn old_log_dir_and_rotation_become_private() {
        let dir = tempfile::tempdir().unwrap();
        let relay_dir = dir.path().join("relay");
        std::fs::create_dir_all(&relay_dir).unwrap();
        std::fs::set_permissions(&relay_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = relay_dir.join("k7f2q9.jsonl");
        let rotated = path.with_extension("jsonl.1");
        std::fs::write(&rotated, "{}\n").unwrap();
        std::fs::set_permissions(&rotated, std::fs::Permissions::from_mode(0o644)).unwrap();
        ConnLog::with_limit(path, 200).unwrap();
        assert_eq!(mode(&relay_dir), 0o700, "папка от прошлой версии становится 0700");
        assert_eq!(mode(&rotated), 0o600, "ротация от прошлой версии становится 0600");
    }

    #[test]
    fn tail_and_totals_skip_corrupt_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay/k7f2q9.jsonl");
        assert_eq!(tail(&path, 5).unwrap(), vec![]);
        assert_eq!(totals(&path, None).unwrap(), (0, 0, 0, None));
        let log = ConnLog::open(path.clone()).unwrap();
        for (ts_ms, target) in [(1, "a.test:443"), (2, "b.test:443"), (3, "c.test:443")] {
            log.write(&Record { ts_ms, proxy_id: 7, target, bytes_up: 10, bytes_down: 20, ms: 5, result: "ok" });
            if ts_ms == 2 {
                std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{oops\n").unwrap();
            }
        }
        let last = tail(&path, 2).unwrap();
        assert_eq!(last.iter().map(|r| r.target.as_str()).collect::<Vec<_>>(), ["b.test:443", "c.test:443"]);
        assert_eq!(totals(&path, None).unwrap(), (30, 60, 3, Some(1)));
        assert_eq!(totals(&path, Some(3)).unwrap(), (20, 40, 2, Some(1)));
    }

    #[test]
    fn rotates_when_full() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay/k7f2q9.jsonl");
        let log = ConnLog::with_limit(path.clone(), 200).unwrap();
        let record = Record { ts_ms: 1, proxy_id: 42, target: "example.com:443", bytes_up: 10, bytes_down: 20, ms: 5, result: "ok" };
        for _ in 0..5 {
            log.write(&record);
        }
        let current = std::fs::read_to_string(&path).unwrap();
        let rotated = std::fs::read_to_string(path.with_extension("jsonl.1")).unwrap();
        assert!(current.lines().count() >= 1 && rotated.lines().count() >= 1);
        assert!(current.contains(r#""target":"example.com:443""#));
    }

    #[test]
    fn log_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay/k7f2q9.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let log = ConnLog::with_limit(path.clone(), 200).unwrap();
        assert_eq!(mode(&path), 0o600, "файл от прошлой версии становится 0600");
        let record = Record { ts_ms: 1, proxy_id: 42, target: "example.com:443", bytes_up: 10, bytes_down: 20, ms: 5, result: "ok" };
        for _ in 0..5 {
            log.write(&record);
        }
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&path.with_extension("jsonl.1")), 0o600);
    }
}
