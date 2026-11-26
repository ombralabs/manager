//! Журнал запуска: строка JSON на событие (спецификация автоматизации, «Журнал»). Сообщения строятся из полей как
//! они записаны в процессе — подставленных значений и паролей в журнале нет.
use crate::flow::model::Node;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Start,
    End,
    Ok,
    Skip,
    Error,
    Branch,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub ts_ms: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub op: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub desc: String,
    pub status: Status,
    pub ms: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub msg: String,
    /// Глубина вызова (`control.call`): у узлов подпроцесса свои id, с узлами вызывающего их не сопоставить.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub depth: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Журнал запуска из файла; нет файла — пусто, нечитаемая строка пропускается.
pub fn read(path: &Path) -> std::io::Result<Vec<Entry>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Копия каждой строки журнала — ход задачи в CLI (спецификация А2).
pub type Sink = std::sync::Arc<dyn Fn(&Entry) + Send + Sync>;

/// Куда пишется журнал: файл запуска и (в тестах) память. `Journal::default()` никуда не пишет.
#[derive(Default)]
pub struct Journal {
    file: Option<File>,
    memory: Option<Vec<Entry>>,
    sink: Option<Sink>,
    /// Глубина вызова для строк узлов; задаёт `Ctx::call`.
    pub(crate) depth: u32,
}

impl Journal {
    pub fn memory() -> Self {
        Self { file: None, memory: Some(Vec::new()), sink: None, depth: 0 }
    }

    /// Дописывает в файл; папка создаётся.
    pub fn file(path: &Path) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(Self { file: Some(OpenOptions::new().create(true).append(true).open(path)?), memory: None, sink: None, depth: 0 })
    }

    pub fn with_sink(mut self, sink: Option<Sink>) -> Self {
        self.sink = sink;
        self
    }

    pub fn node(&mut self, node: &Node, status: Status, ms: u64, msg: &str) {
        let entry = Entry {
            ts_ms: now_ms(),
            node: node.id.clone(),
            op: node.op.clone(),
            desc: node.description.clone(),
            status,
            ms,
            msg: msg.to_string(),
            depth: self.depth,
        };
        self.push(entry);
    }

    /// Начало и конец запуска.
    pub fn event(&mut self, status: Status, msg: &str) {
        let entry = Entry { ts_ms: now_ms(), node: String::new(), op: String::new(), desc: String::new(), status, ms: 0, msg: msg.to_string(), depth: 0 };
        self.push(entry);
    }

    #[cfg(test)]
    pub fn entries(&self) -> &[Entry] {
        self.memory.as_deref().unwrap_or_default()
    }

    fn push(&mut self, entry: Entry) {
        if let Some(file) = &mut self.file {
            let mut line = serde_json::to_string(&entry).expect("строка журнала — JSON");
            line.push('\n');
            if let Err(e) = file.write_all(line.as_bytes()) {
                log::warn!("журнал процесса не записан: {e}");
            }
        }
        if let Some(sink) = &self.sink {
            sink(&entry);
        }
        if let Some(memory) = &mut self.memory {
            memory.push(entry);
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_skips_broken_lines_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l/1.jsonl");
        let mut journal = Journal::file(&path).unwrap();
        journal.event(Status::Start, "начало");
        journal.event(Status::End, "конец");
        drop(journal);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("мусор\n");
        std::fs::write(&path, text).unwrap();
        let entries = read(&path).unwrap();
        assert_eq!(entries.iter().map(|e| (e.status, e.msg.as_str())).collect::<Vec<_>>(), [(Status::Start, "начало"), (Status::End, "конец")]);
        assert!(read(&dir.path().join("нет.jsonl")).unwrap().is_empty());
    }
}
