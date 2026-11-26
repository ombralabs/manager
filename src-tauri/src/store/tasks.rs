//! Задачи автоматизации, их пакеты и запуски (спецификация А2, «Хранилище»). Параметры и таблица задачи хранятся
//! зашифрованными: в них бывают логины и пароли.
use super::secrets::Secret;
use super::{Store, StoreError};
use crate::flow::model::Flow;
use crate::flow::params::{Distribution, MissingRow, Order, Row};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

/// Ошибка запусков пакета, который вёл исчезнувший процесс Ombra.
const DEAD_PROCESS: &str = "процесс Ombra завершился";

/// Статус запуска в пакете.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Waiting,
    Running,
    Done,
    Error,
    Cancelled,
    Skipped,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Waiting => "waiting",
            Self::Running => "running",
            Self::Done => "done",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        Ok(match text {
            "queued" => Self::Queued,
            "waiting" => Self::Waiting,
            "running" => Self::Running,
            "done" => Self::Done,
            "error" => Self::Error,
            "cancelled" => Self::Cancelled,
            "skipped" => Self::Skipped,
            _ => return Err(StoreError::Invalid("flow_runs.status")),
        })
    }

    /// Подпись для хода задачи в CLI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "в очереди",
            Self::Waiting => "ожидает",
            Self::Running => "выполняется",
            Self::Done => "готово",
            Self::Error => "ошибка",
            Self::Cancelled => "отменён",
            Self::Skipped => "пропущен",
        }
    }
}

/// Новая задача.
pub struct NewTask<'a> {
    pub headless: Option<bool>,
    pub distribution: Distribution,
    pub name: &'a str,
    /// Процесс из базы; `None` — процесс из файла в `flow`.
    pub flow_id: Option<i64>,
    pub flow: Option<&'a Flow>,
    pub profiles: &'a [String],
    pub order: Order,
    pub threads: u32,
    pub missing_row: MissingRow,
    pub params: &'a Row,
    pub data: Option<&'a [Row]>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskRecord {
    pub headless: Option<bool>,
    pub revision: i64,
    pub distribution: Distribution,
    pub id: i64,
    pub name: String,
    pub flow_id: Option<i64>,
    pub flow: Option<Flow>,
    pub profiles: Vec<String>,
    pub order: Order,
    pub threads: u32,
    pub missing_row: MissingRow,
    pub params: Row,
    pub data: Option<Vec<Row>>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub id: i64,
    pub profile_id: String,
    pub serial: i64,
    pub position: usize,
    pub status: RunState,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatchRecord {
    pub id: i64,
    pub pid: u32,
    pub finished_at: Option<i64>,
    pub runs: Vec<RunRecord>,
}

/// Итог пакета для списка задач: числа запусков по статусам.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BatchSummary {
    pub id: i64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub total: usize,
    pub queued: usize,
    pub waiting: usize,
    pub running: usize,
    pub done: usize,
    pub error: usize,
    pub cancelled: usize,
    pub skipped: usize,
}

/// Строка списка задач; `running` заполняет `app`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskRow {
    pub distribution: Distribution,
    pub queue: Option<QueueProgress>,
    pub id: i64,
    pub name: String,
    pub flow_id: Option<i64>,
    pub flow_name: String,
    pub profiles: usize,
    pub threads: u32,
    pub order: Order,
    pub missing_row: MissingRow,
    pub created_at: i64,
    pub running: bool,
    pub last: Option<BatchSummary>,
}

/// Строка журнала пакета: запуск с именем профиля.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunRow {
    pub item_index: Option<usize>,
    pub id: i64,
    pub profile_id: String,
    pub serial: i64,
    pub name: String,
    pub position: usize,
    pub status: RunState,
    pub error: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueueProgress {
    pub total: usize,
    pub done: usize,
}

type TaskTuple = (String, Option<i64>, Option<String>, String, String, u32, String, Vec<u8>, Option<Vec<u8>>, i64, String, i64, Option<bool>);
type QueueBatch = (i64, Vec<(usize, i64)>);

impl Store {
    pub fn create_task(&self, task: &NewTask<'_>, now: i64) -> Result<i64, StoreError> {
        let flow = task.flow.map(serde_json::to_string).transpose().map_err(|_| StoreError::Invalid("flow"))?;
        let profiles = serde_json::to_string(task.profiles).map_err(|_| StoreError::Invalid("profiles"))?;
        let params = self.seal_json(&serde_json::Value::Object(task.params.clone()))?;
        let data = task.data.map(|rows| self.seal_json(&serde_json::Value::from(rows.to_vec()))).transpose()?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO flow_tasks (name, flow_id, flow_json, profiles, run_order, threads, missing_row, params_enc, data_enc, created_at, distribution, work_total, headless) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![task.name, task.flow_id, flow, profiles, task.order.as_str(), task.threads, task.missing_row.as_str(), params, data, now,
                task.distribution.as_str(), if task.distribution == Distribution::Queue { task.data.map_or(1, <[Row]>::len) as i64 } else { 0 }, task.headless],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_task(&self, id: i64) -> Result<Option<TaskRecord>, StoreError> {
        let row: Option<TaskTuple> = self
            .conn()
            .query_row(
                "SELECT name, flow_id, flow_json, profiles, run_order, threads, missing_row, params_enc, data_enc, created_at, distribution \
                 , revision, headless FROM flow_tasks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?, r.get(12)?)),
            )
            .optional()?;
        let Some((name, flow_id, flow, profiles, order, threads, missing_row, params, data, created_at, distribution, revision, headless)) = row else {
            return Ok(None);
        };
        Ok(Some(TaskRecord {
            headless,
            revision,
            distribution: Distribution::parse(&distribution).ok_or(StoreError::Invalid("flow_tasks.distribution"))?,
            id,
            name,
            flow_id,
            flow: flow.map(|text| serde_json::from_str(&text)).transpose().map_err(|_| StoreError::Invalid("flow_tasks.flow_json"))?,
            profiles: serde_json::from_str(&profiles).map_err(|_| StoreError::Invalid("flow_tasks.profiles"))?,
            order: Order::parse(&order).ok_or(StoreError::Invalid("flow_tasks.run_order"))?,
            threads,
            missing_row: MissingRow::parse(&missing_row).ok_or(StoreError::Invalid("flow_tasks.missing_row"))?,
            params: self.unseal_json(&params, "flow_tasks.params_enc")?,
            data: data.map(|blob| self.unseal_json(&blob, "flow_tasks.data_enc")).transpose()?,
            created_at,
        }))
    }

    /// Пакет и его запуски `queued` в порядке исполнения; ответ — id пакета и id запусков по порядку.
    /// Последний пакет задачи не закончен и его процесс жив (`alive`) — `TaskRunning`: проверка и вставка — одна
    /// транзакция с замком записи, два одновременных «Повторить» одной задачи не пройдут оба.
    pub fn create_batch(
        &self,
        task_id: i64,
        flow: &Flow,
        pid: u32,
        runs: &[(String, i64)],
        now: i64,
        alive: &dyn Fn(u32) -> bool,
    ) -> Result<(i64, Vec<i64>), StoreError> {
        self.create_batch_checked((task_id, None), flow, pid, runs, now, alive)
    }

    pub fn create_batch_checked(
        &self, (task_id, expected_revision): (i64, Option<i64>), flow: &Flow, pid: u32,
        runs: &[(String, i64)], now: i64, alive: &dyn Fn(u32) -> bool,
    ) -> Result<(i64, Vec<i64>), StoreError> {
        let flow = serde_json::to_string(flow).map_err(|_| StoreError::Invalid("flow"))?;
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        Self::refuse_if_running(&tx, task_id, alive)?;
        let revision = Self::check_task_revision(&tx, task_id, expected_revision)?;
        tx.execute(
            "INSERT INTO flow_batches (task_id, flow_json, pid, started_at, task_revision) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task_id, flow, pid, now, revision],
        )?;
        let batch = tx.last_insert_rowid();
        let mut ids = Vec::with_capacity(runs.len());
        for (position, (profile_id, serial)) in runs.iter().enumerate() {
            tx.execute(
                "INSERT INTO flow_runs (batch_id, profile_id, serial, position, status) VALUES (?1, ?2, ?3, ?4, 'queued')",
                params![batch, profile_id, serial, position as i64],
            )?;
            ids.push(tx.last_insert_rowid());
        }
        tx.commit()?;
        Ok((batch, ids))
    }

    /// Снимок незавершённых заданий и новый пакет — одна транзакция. Даже два параллельных запуска,
    /// один из которых уже успел закончиться, не выдадут успешно выполненное задание повторно.
    pub fn create_queue_batch(
        &self, task_id: i64, flow: &Flow, items: &[usize], failed_only: bool, now: i64,
    ) -> Result<Option<QueueBatch>, StoreError> {
        self.create_queue_batch_checked((task_id, None), flow, items, failed_only, now)
    }

    pub fn create_queue_batch_checked(
        &self, (task_id, expected_revision): (i64, Option<i64>), flow: &Flow, items: &[usize], failed_only: bool, now: i64,
    ) -> Result<Option<QueueBatch>, StoreError> {
        let flow = serde_json::to_string(flow).map_err(|_| StoreError::Invalid("flow"))?;
        let conn = self.conn();
        let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
        Self::refuse_if_running(&tx, task_id, &crate::sys::pid_alive)?;
        let revision = Self::check_task_revision(&tx, task_id, expected_revision)?;
        let mut done = std::collections::HashSet::new();
        let mut last = std::collections::HashMap::new();
        {
            let mut statement = tx.prepare(
                "SELECT r.item_index, r.status FROM flow_runs r JOIN flow_batches b ON b.id = r.batch_id \
                 WHERE b.task_id = ?1 AND b.task_revision = ?2 AND r.item_index IS NOT NULL ORDER BY r.id",
            )?;
            for row in statement.query_map(params![task_id, revision], |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, String>(1)?)))? {
                let (index, state) = row?;
                if state == "done" { done.insert(index); }
                last.insert(index, state);
            }
        }
        let pending: Vec<_> = items.iter().copied().filter(|index| {
            !done.contains(index) && (!failed_only || last.get(index).is_some_and(|state| state != "done" && state != "skipped"))
        }).collect();
        if pending.is_empty() { return Ok(None); }
        tx.execute(
            "INSERT INTO flow_batches (task_id, flow_json, pid, started_at, task_revision) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task_id, flow, std::process::id(), now, revision],
        )?;
        let batch = tx.last_insert_rowid();
        let mut runs = Vec::with_capacity(pending.len());
        for (position, index) in pending.into_iter().enumerate() {
            tx.execute(
                "INSERT INTO flow_runs (batch_id, profile_id, serial, position, status, item_index) VALUES (?1, '', 0, ?2, 'queued', ?3)",
                params![batch, position as i64, index as i64],
            )?;
            runs.push((index, tx.last_insert_rowid()));
        }
        tx.commit()?;
        Ok(Some((batch, runs)))
    }

    pub fn assign_queue_run(&self, run_id: i64, profile_id: &str, serial: i64) -> Result<(), StoreError> {
        let assigned = self.conn().execute(
            "UPDATE flow_runs SET profile_id = ?2, serial = ?3 WHERE id = ?1 AND status IN ('queued', 'waiting') AND profile_id = '' AND item_index IS NOT NULL",
            params![run_id, profile_id, serial],
        )?;
        if assigned != 1 { return Err(StoreError::Invalid("flow_runs.assignment")); }
        Ok(())
    }

    /// `running` отмечает начало запуска, итоговые статусы — конец.
    pub fn set_run_status(&self, run_id: i64, status: RunState, error: Option<&str>, now: i64) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE flow_runs SET status = ?2, error = ?3, \
             started_at = CASE WHEN ?2 = 'running' THEN ?4 ELSE started_at END, \
             finished_at = CASE WHEN ?2 IN ('done', 'error', 'cancelled', 'skipped') THEN ?4 ELSE finished_at END \
             WHERE id = ?1",
            params![run_id, status.as_str(), error, now],
        )?;
        Ok(())
    }

    pub fn finish_batch(&self, batch_id: i64, now: i64) -> Result<(), StoreError> {
        self.conn().execute("UPDATE flow_batches SET finished_at = ?2 WHERE id = ?1", params![batch_id, now])?;
        Ok(())
    }

    /// Последний пакет задачи с запусками по порядку исполнения.
    pub fn last_batch(&self, task_id: i64) -> Result<Option<BatchRecord>, StoreError> {
        let conn = self.conn();
        let batch: Option<(i64, u32, Option<i64>)> = conn
            .query_row(
                "SELECT id, pid, finished_at FROM flow_batches WHERE task_id = ?1 \
                 AND task_revision = (SELECT revision FROM flow_tasks WHERE id = ?1) ORDER BY id DESC LIMIT 1",
                [task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((id, pid, finished_at)) = batch else { return Ok(None) };
        let mut statement =
            conn.prepare("SELECT id, profile_id, serial, position, status, error FROM flow_runs WHERE batch_id = ?1 ORDER BY position")?;
        let rows = statement.query_map([id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, String>(4)?, r.get::<_, Option<String>>(5)?))
        })?;
        let mut runs = Vec::new();
        for row in rows {
            let (run, profile_id, serial, position, status, error) = row?;
            runs.push(RunRecord { id: run, profile_id, serial, position: position as usize, status: RunState::parse(&status)?, error });
        }
        Ok(Some(BatchRecord { id, pid, finished_at, runs }))
    }

    /// Задачи для списка, новые сверху; параметры и таблица не расшифровываются.
    pub fn task_rows(&self) -> Result<Vec<TaskRow>, StoreError> {
        let mut rows = {
            let conn = self.conn();
            let mut statement = conn.prepare(
                "SELECT t.id, t.name, t.flow_id, f.name, t.flow_json, t.profiles, t.threads, t.run_order, t.missing_row, t.created_at, \
                 t.distribution, t.work_total, (SELECT COUNT(DISTINCT r.item_index) FROM flow_runs r JOIN flow_batches b ON b.id = r.batch_id \
                 WHERE b.task_id = t.id AND b.task_revision = t.revision AND r.status = 'done') \
                 FROM flow_tasks t LEFT JOIN flows f ON f.id = t.flow_id ORDER BY t.id DESC",
            )?;
            let raw = statement.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?, r.get(6)?, r.get::<_, String>(7)?, r.get::<_, String>(8)?, r.get(9)?, r.get::<_, String>(10)?, r.get::<_, i64>(11)?, r.get::<_, i64>(12)?))
            })?;
            let mut rows = Vec::new();
            for row in raw {
                let (id, name, flow_id, db_name, file, profiles, threads, order, missing_row, created_at, distribution, total, done) = row?;
                let distribution = Distribution::parse(&distribution).ok_or(StoreError::Invalid("flow_tasks.distribution"))?;
                let file_name = file.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()).and_then(|v| v["name"].as_str().map(str::to_string));
                let profiles: Vec<serde_json::Value> = serde_json::from_str(&profiles).map_err(|_| StoreError::Invalid("flow_tasks.profiles"))?;
                rows.push(TaskRow {
                    distribution,
                    queue: (distribution == Distribution::Queue).then_some(QueueProgress { total: total as usize, done: done as usize }),
                    id,
                    name,
                    flow_id,
                    flow_name: db_name.or(file_name).unwrap_or_default(),
                    profiles: profiles.len(),
                    threads,
                    order: Order::parse(&order).ok_or(StoreError::Invalid("flow_tasks.run_order"))?,
                    missing_row: MissingRow::parse(&missing_row).ok_or(StoreError::Invalid("flow_tasks.missing_row"))?,
                    created_at,
                    running: false,
                    last: None,
                });
            }
            rows
        };
        // Последний пакет каждой задачи — одним запросом на все задачи.
        let mut last: std::collections::HashMap<i64, BatchSummary> =
            self.summaries("SELECT MAX(b.id) FROM flow_batches b JOIN flow_tasks t ON t.id = b.task_id WHERE b.task_revision = t.revision GROUP BY b.task_id", &[])?.into_iter().collect();
        for row in &mut rows {
            row.last = last.remove(&row.id);
        }
        Ok(rows)
    }

    /// Пакеты задачи, новые сверху, со счётчиками запусков по статусам.
    pub fn batch_summaries(&self, task_id: i64) -> Result<Vec<BatchSummary>, StoreError> {
        let all = self.summaries("SELECT id FROM flow_batches WHERE task_id = ?1", &[&task_id])?;
        Ok(all.into_iter().map(|(_, batch)| batch).collect())
    }

    /// Пакеты, чьи id даёт `ids_sql`, вместе с id задачи; новые сверху. Счётчики — одним запросом на все пакеты.
    fn summaries(&self, ids_sql: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<(i64, BatchSummary)>, StoreError> {
        let conn = self.conn();
        let mut statement = conn.prepare(&format!("SELECT id, task_id, started_at, finished_at FROM flow_batches WHERE id IN ({ids_sql}) ORDER BY id DESC"))?;
        let mut batches: Vec<(i64, BatchSummary)> = statement
            .query_map(args, |r| {
                Ok((r.get(1)?, BatchSummary { id: r.get(0)?, started_at: r.get(2)?, finished_at: r.get(3)?, total: 0, queued: 0, waiting: 0, running: 0, done: 0, error: 0, cancelled: 0, skipped: 0 }))
            })?
            .collect::<Result<_, _>>()?;
        let index: std::collections::HashMap<i64, usize> = batches.iter().enumerate().map(|(i, (_, b))| (b.id, i)).collect();
        let mut counts = conn.prepare(&format!("SELECT batch_id, status, COUNT(*) FROM flow_runs WHERE batch_id IN ({ids_sql}) GROUP BY batch_id, status"))?;
        for row in counts.query_map(args, |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? as usize)))? {
            let (batch, status, n) = row?;
            let Some(&at) = index.get(&batch) else { continue };
            let b = &mut batches[at].1;
            b.total += n;
            match RunState::parse(&status)? {
                RunState::Queued => b.queued = n,
                RunState::Waiting => b.waiting = n,
                RunState::Running => b.running = n,
                RunState::Done => b.done = n,
                RunState::Error => b.error = n,
                RunState::Cancelled => b.cancelled = n,
                RunState::Skipped => b.skipped = n,
            }
        }
        Ok(batches)
    }

    /// Запуски пакета по порядку исполнения; имя — из `profiles`, в том числе из корзины, удалённого профиля — пусто.
    pub fn run_rows(&self, batch_id: i64) -> Result<Vec<RunRow>, StoreError> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT r.id, r.profile_id, r.serial, p.name, r.position, r.status, r.error, r.started_at, r.finished_at, r.item_index \
             FROM flow_runs r LEFT JOIN profiles p ON p.id = r.profile_id WHERE r.batch_id = ?1 ORDER BY r.position",
        )?;
        let raw = statement.query_map([batch_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, i64>(4)?, r.get::<_, String>(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get::<_, Option<i64>>(9)?.map(|v| v as usize)))
        })?;
        let mut rows = Vec::new();
        for row in raw {
            let (id, profile_id, serial, name, position, status, error, started_at, finished_at, item_index) = row?;
            rows.push(RunRow { item_index, id, profile_id, serial, name: name.unwrap_or_default(), position: position as usize, status: RunState::parse(&status)?, error, started_at, finished_at });
        }
        Ok(rows)
    }

    pub fn batch_task(&self, batch_id: i64) -> Result<Option<i64>, StoreError> {
        Ok(self.conn().query_row("SELECT task_id FROM flow_batches WHERE id = ?1", [batch_id], |r| r.get(0)).optional()?)
    }

    /// Последний пакет задачи не закончен и его процесс жив — `TaskRunning`. Звать внутри транзакции.
    fn refuse_if_running(tx: &rusqlite::Connection, task_id: i64, alive: &dyn Fn(u32) -> bool) -> Result<(), StoreError> {
        let open: Option<u32> = tx
            .query_row(
                "SELECT pid FROM flow_batches WHERE task_id = ?1 AND finished_at IS NULL \
                 AND id = (SELECT MAX(id) FROM flow_batches WHERE task_id = ?1)",
                [task_id],
                |r| r.get(0),
            )
            .optional()?;
        match open.filter(|owner| alive(*owner)) {
            Some(owner) => Err(StoreError::TaskRunning(owner)),
            None => Ok(()),
        }
    }

    fn check_task_revision(conn: &rusqlite::Connection, id: i64, expected: Option<i64>) -> Result<i64, StoreError> {
        let revision: i64 = conn.query_row("SELECT revision FROM flow_tasks WHERE id = ?1", [id], |row| row.get(0))
            .optional()?.ok_or_else(|| StoreError::NotFound(format!("задача {id}")))?;
        if expected.is_some_and(|value| value != revision) { return Err(StoreError::TaskChanged); }
        Ok(revision)
    }

    /// Все поля задачи обновляются атомарно; старые пакеты остаются в журнале.
    /// Новые входные данные получают свою очередь, прежние успешные строки к ней не относятся.
    pub fn update_task(&self, id: i64, expected_revision: i64, task: &NewTask<'_>, alive: &dyn Fn(u32) -> bool) -> Result<bool, StoreError> {
        let file = task.flow.map(serde_json::to_string).transpose().map_err(|_| StoreError::Invalid("flow"))?;
        let profiles = serde_json::to_string(task.profiles).map_err(|_| StoreError::Invalid("profiles"))?;
        let values = self.seal_json(&serde_json::Value::Object(task.params.clone()))?;
        let data = task.data.map(|rows| self.seal_json(&serde_json::Value::from(rows.to_vec()))).transpose()?;
        let conn = self.conn();
        let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
        Self::refuse_if_running(&tx, id, alive)?;
        let revision = Self::check_task_revision(&tx, id, Some(expected_revision))?;
        type WorkloadTuple = (Option<i64>, Option<String>, Vec<u8>, Option<Vec<u8>>, String, String, String);
        let (old_id, old_file, old_values, old_data, old_distribution, old_profiles, old_missing): WorkloadTuple = tx.query_row(
            "SELECT flow_id, flow_json, params_enc, data_enc, distribution, profiles, missing_row FROM flow_tasks WHERE id = ?1", [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
        )?;
        let old_values: Row = self.unseal_json(&old_values, "flow_tasks.params_enc")?;
        let old_data: Option<Vec<Row>> = old_data.map(|bytes| self.unseal_json(&bytes, "flow_tasks.data_enc")).transpose()?;
        let changed = old_id != task.flow_id || old_file != file || &old_values != task.params
            || old_data.as_deref() != task.data || old_distribution != task.distribution.as_str()
            || (task.distribution == Distribution::PerProfile && (old_profiles != profiles
                || (task.data.is_some() && old_missing != task.missing_row.as_str())));
        tx.execute(
            "UPDATE flow_tasks SET name = ?2, flow_id = ?3, flow_json = ?4, profiles = ?5, run_order = ?6, threads = ?7, \
             missing_row = ?8, params_enc = ?9, data_enc = ?10, distribution = ?11, work_total = ?12, revision = ?13, headless = ?14 WHERE id = ?1",
            params![id, task.name, task.flow_id, file, profiles, task.order.as_str(), task.threads, task.missing_row.as_str(), values, data,
                task.distribution.as_str(), if task.distribution == Distribution::Queue { task.data.map_or(1, <[Row]>::len) as i64 } else { 0 },
                revision + i64::from(changed), task.headless],
        )?;
        tx.commit()?;
        Ok(changed)
    }

    /// Сохраняет настройки без изменения данных, очереди и истории запусков.
    /// Живой пакет проверяется под тем же замком записи, что и обновление.
    pub fn update_task_settings(&self, id: i64, name: &str, threads: u32, alive: &dyn Fn(u32) -> bool) -> Result<(), StoreError> {
        let conn = self.conn();
        let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
        Self::refuse_if_running(&tx, id, alive)?;
        if tx.execute("UPDATE flow_tasks SET name = ?2, threads = ?3 WHERE id = ?1", params![id, name, threads])? == 0 {
            return Err(StoreError::NotFound(format!("задача {id}")));
        }
        tx.commit()?;
        Ok(())
    }

    /// Удаляет задачу, если её пакет не идёт: проверка живого пакета и удаление — одна транзакция, `create_batch`
    /// другого процесса между ними не встанет. Пакеты и запуски уходят каскадом.
    pub fn delete_task(&self, id: i64, alive: &dyn Fn(u32) -> bool) -> Result<(), StoreError> {
        let conn = self.conn();
        let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
        Self::refuse_if_running(&tx, id, alive)?;
        tx.execute("DELETE FROM flow_tasks WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// Закрывает незаконченный пакет задачи, который вёл процесс `pid` (после паники): незаконченные запуски —
    /// «Ошибка» с текстом `error`. Ответ — сколько пакетов закрыто.
    pub fn abort_open_batches(&self, task_id: i64, pid: u32, error: &str, now: i64) -> Result<usize, StoreError> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let ids: Vec<i64> = {
            let mut statement = tx.prepare("SELECT id FROM flow_batches WHERE task_id = ?1 AND pid = ?2 AND finished_at IS NULL")?;
            statement.query_map(params![task_id, pid], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        for id in &ids {
            tx.execute(
                "UPDATE flow_runs SET status = 'error', error = ?2, finished_at = ?3 \
                 WHERE batch_id = ?1 AND status IN ('queued', 'waiting', 'running')",
                params![id, error, now],
            )?;
            tx.execute("UPDATE flow_batches SET finished_at = ?2 WHERE id = ?1", params![id, now])?;
        }
        tx.commit()?;
        Ok(ids.len())
    }

    /// Пакеты процессов Ombra, которых уже нет (`alive(pid)` ложно): незаконченные запуски — «Отменён», пакет закрыт.
    /// Ответ — сколько пакетов закрыто.
    pub fn close_dead_batches(&self, alive: &dyn Fn(u32) -> bool, now: i64) -> Result<usize, StoreError> {
        let conn = self.conn();
        let open: Vec<(i64, u32)> = {
            let mut statement = conn.prepare("SELECT id, pid FROM flow_batches WHERE finished_at IS NULL")?;
            statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
        };
        let dead: Vec<i64> = open.into_iter().filter(|(_, pid)| !alive(*pid)).map(|(id, _)| id).collect();
        let tx = conn.unchecked_transaction()?;
        for id in &dead {
            tx.execute(
                "UPDATE flow_runs SET status = 'cancelled', error = ?2, finished_at = ?3 \
                 WHERE batch_id = ?1 AND status IN ('queued', 'waiting', 'running')",
                params![id, DEAD_PROCESS, now],
            )?;
            tx.execute("UPDATE flow_batches SET finished_at = ?2 WHERE id = ?1", params![id, now])?;
        }
        tx.commit()?;
        Ok(dead.len())
    }

    fn seal_json(&self, value: &serde_json::Value) -> Result<Vec<u8>, StoreError> {
        self.secrets.encrypt(&Secret::new(value.to_string()))
    }

    fn unseal_json<T: serde::de::DeserializeOwned>(&self, blob: &[u8], what: &'static str) -> Result<T, StoreError> {
        let text = self.secrets.decrypt(blob)?;
        serde_json::from_str(text.expose()).map_err(|_| StoreError::Invalid(what))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::memory_store;
    use serde_json::json;

    fn flow() -> Flow {
        crate::flow::model::parse(
            r#"{"format":"ombra-flow/1","name":"Вход","nodes":[{"id":"s","op":"start"}],
                "subflows":{"3":{"format":"ombra-flow/1","name":"sub","nodes":[]}}}"#,
        )
        .unwrap()
    }

    fn row(value: serde_json::Value) -> Row {
        value.as_object().unwrap().clone()
    }

    fn new_task(store: &Store, params: &Row, data: Option<&[Row]>, file: Option<&Flow>) -> i64 {
        let profiles = vec!["a1".to_string(), "b2".to_string()];
        let task = NewTask {
            headless: None,
            distribution: Distribution::PerProfile,
            name: "Ночная",
            flow_id: file.is_none().then_some(4),
            flow: file,
            profiles: &profiles,
            order: Order::Random,
            threads: 3,
            missing_row: MissingRow::Random,
            params,
            data,
        };
        store.create_task(&task, 10).unwrap()
    }

    fn contains(bytes: &[u8], text: &str) -> bool {
        bytes.windows(text.len()).any(|window| window == text.as_bytes())
    }

    #[test]
    fn queue_claims_are_atomic_and_successes_survive_later_failed_batches() {
        let store = memory_store();
        let id = new_task(&store, &Row::new(), None, None);
        let (batch, runs) = store.create_queue_batch(id, &flow(), &[0, 1, 2], false, 1).unwrap().unwrap();
        assert!(matches!(store.create_queue_batch(id, &flow(), &[0, 1, 2], false, 2), Err(StoreError::TaskRunning(_))));
        store.assign_queue_run(runs[0].1, "a1", 1).unwrap();
        assert!(store.assign_queue_run(runs[0].1, "b2", 2).is_err(), "задание не получает второй профиль");
        store.set_run_status(runs[0].1, RunState::Done, None, 3).unwrap();
        store.set_run_status(runs[1].1, RunState::Error, Some("ошибка"), 3).unwrap();
        store.set_run_status(runs[2].1, RunState::Cancelled, None, 3).unwrap();
        store.finish_batch(batch, 4).unwrap();
        let (batch, retry) = store.create_queue_batch(id, &flow(), &[0, 1, 2], true, 5).unwrap().unwrap();
        assert_eq!(retry.iter().map(|(item, _)| *item).collect::<Vec<_>>(), [1, 2]);
        store.set_run_status(retry[0].1, RunState::Done, None, 6).unwrap();
        store.set_run_status(retry[1].1, RunState::Error, Some("ещё ошибка"), 6).unwrap();
        store.finish_batch(batch, 7).unwrap();
        let (_, retry) = store.create_queue_batch(id, &flow(), &[0, 1, 2], false, 8).unwrap().unwrap();
        assert_eq!(retry.iter().map(|(item, _)| *item).collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn queue_resumes_after_reopening_the_database_without_repeating_a_done_item() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.db");
        let id = {
            let store = Store::open(&path, crate::store::tests::test_box()).unwrap();
            let rows = vec![row(json!({ "url": "a" })), row(json!({ "url": "b" }))];
            let id = store.create_task(&NewTask {
                headless: Some(true),
                distribution: Distribution::Queue, name: "Очередь", flow_id: None, flow: Some(&flow()),
                profiles: &["a1".into()], order: Order::Ordered, threads: 1, missing_row: MissingRow::Skip,
                params: &Row::new(), data: Some(&rows),
            }, 1).unwrap();
            let (batch, runs) = store.create_queue_batch(id, &flow(), &[0, 1], false, 2).unwrap().unwrap();
            store.set_run_status(runs[0].1, RunState::Done, None, 3).unwrap();
            store.set_run_status(runs[1].1, RunState::Cancelled, None, 3).unwrap();
            store.finish_batch(batch, 4).unwrap();
            id
        };
        let store = Store::open(&path, crate::store::tests::test_box()).unwrap();
        assert_eq!(store.get_task(id).unwrap().unwrap().headless, Some(true));
        assert_eq!(store.get_task(id).unwrap().unwrap().data.unwrap().len(), 2);
        assert_eq!(store.task_rows().unwrap()[0].queue, Some(QueueProgress { total: 2, done: 1 }));
        let (_, runs) = store.create_queue_batch(id, &flow(), &[0, 1], false, 5).unwrap().unwrap();
        assert_eq!(runs.iter().map(|(index, _)| *index).collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn task_round_trip_keeps_params_and_rows_sealed() {
        let store = memory_store();
        let params = row(json!({ "pass": "s3cr3t-пароль" }));
        let rows = vec![row(json!({ "login": "user1", "pass": "rowsecret" }))];
        let f = flow();
        let id = new_task(&store, &params, Some(&rows), Some(&f));
        let task = store.get_task(id).unwrap().unwrap();
        assert_eq!((task.name.as_str(), task.flow_id, task.flow.as_ref()), ("Ночная", None, Some(&f)), "JSON файла с subflows");
        assert_eq!(task.profiles, ["a1", "b2"]);
        assert_eq!((task.order, task.threads, task.missing_row, task.created_at), (Order::Random, 3, MissingRow::Random, 10));
        assert_eq!((task.params, task.data), (params, Some(rows)));
        let (sealed_params, sealed_rows): (Vec<u8>, Vec<u8>) = store
            .raw()
            .query_row("SELECT params_enc, data_enc FROM flow_tasks WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        for secret in ["s3cr3t", "rowsecret", "user1"] {
            assert!(!contains(&sealed_params, secret) && !contains(&sealed_rows, secret), "{secret} открытым текстом");
        }
        let from_db = new_task(&store, &Row::new(), None, None);
        let task = store.get_task(from_db).unwrap().unwrap();
        assert_eq!((task.flow_id, task.flow, task.data), (Some(4), None, None));
        assert!(store.get_task(999).unwrap().is_none());
    }

    #[test]
    fn batch_runs_move_through_statuses() {
        let store = memory_store();
        let task = new_task(&store, &Row::new(), None, None);
        assert!(store.last_batch(task).unwrap().is_none());
        let (batch, runs) = store.create_batch(task, &flow(), 4242, &[("a1".into(), 1), ("b2".into(), 2)], 20, &|_| true).unwrap();
        store.set_run_status(runs[0], RunState::Running, None, 21).unwrap();
        store.set_run_status(runs[0], RunState::Done, None, 22).unwrap();
        store.set_run_status(runs[1], RunState::Error, Some("узел x: сломан"), 23).unwrap();
        store.finish_batch(batch, 24).unwrap();
        let last = store.last_batch(task).unwrap().unwrap();
        assert_eq!((last.id, last.pid, last.finished_at), (batch, 4242, Some(24)));
        let runs_seen: Vec<(&str, i64, usize, RunState)> =
            last.runs.iter().map(|run| (run.profile_id.as_str(), run.serial, run.position, run.status)).collect();
        assert_eq!(runs_seen, [("a1", 1, 0, RunState::Done), ("b2", 2, 1, RunState::Error)]);
        assert_eq!(last.runs[1].error.as_deref(), Some("узел x: сломан"));
        let times: (Option<i64>, Option<i64>) = store
            .raw()
            .query_row("SELECT started_at, finished_at FROM flow_runs WHERE id = ?1", [runs[0]], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(times, (Some(21), Some(22)));
        let (second, _) = store.create_batch(task, &flow(), 4242, &[("a1".into(), 1)], 30, &|_| true).unwrap();
        assert_eq!(store.last_batch(task).unwrap().unwrap().id, second, "последний — новый пакет");
    }

    #[test]
    fn batches_of_dead_processes_are_closed() {
        let store = memory_store();
        let task = new_task(&store, &Row::new(), None, None);
        let (_, runs) = store.create_batch(task, &flow(), 111, &[("a1".into(), 1), ("b2".into(), 2)], 20, &|_| true).unwrap();
        store.set_run_status(runs[0], RunState::Done, None, 21).unwrap();
        store.set_run_status(runs[1], RunState::Running, None, 21).unwrap();
        let other = new_task(&store, &Row::new(), None, None);
        store.create_batch(other, &flow(), 222, &[("a1".into(), 1)], 20, &|_| true).unwrap();
        assert_eq!(store.close_dead_batches(&|pid| pid == 222, 50).unwrap(), 1);
        let closed = store.last_batch(task).unwrap().unwrap();
        assert_eq!(closed.finished_at, Some(50));
        assert_eq!(closed.runs[0].status, RunState::Done, "законченный запуск не трогается");
        assert_eq!((closed.runs[1].status, closed.runs[1].error.as_deref()), (RunState::Cancelled, Some("процесс Ombra завершился")));
        assert_eq!(store.last_batch(other).unwrap().unwrap().finished_at, None, "пакет живого процесса идёт");
        assert_eq!(store.close_dead_batches(&|pid| pid == 222, 60).unwrap(), 0, "закрытый не закрывается дважды");
    }

    #[test]
    fn a_second_batch_is_refused_while_the_last_one_runs() {
        let store = memory_store();
        let task = new_task(&store, &Row::new(), None, None);
        let (first, _) = store.create_batch(task, &flow(), 4242, &[("a1".into(), 1)], 20, &|_| true).unwrap();
        let refused = store.create_batch(task, &flow(), 5151, &[("a1".into(), 1)], 21, &|pid| pid == 4242);
        assert!(matches!(refused, Err(StoreError::TaskRunning(4242))), "{refused:?}");
        assert_eq!(store.last_batch(task).unwrap().unwrap().id, first, "второй пакет не записан");
        // Процесс незаконченного пакета умер — новый пакет создаётся; закрытый пакет живого процесса не мешает.
        let (second, _) = store.create_batch(task, &flow(), 5151, &[("a1".into(), 1)], 22, &|pid| pid == 5151).unwrap();
        store.finish_batch(second, 23).unwrap();
        store.create_batch(task, &flow(), 6161, &[("a1".into(), 1)], 24, &|_| true).unwrap();
    }

    #[test]
    fn task_rows_summarize_the_last_batch() {
        let store = memory_store();
        let f = flow();
        let file = new_task(&store, &Row::new(), None, Some(&f));
        let db = new_task(&store, &Row::new(), None, None);
        store.raw().execute("INSERT INTO flows (id, name, grp, body, created_at, updated_at) VALUES (4, 'Из базы', '', '{}', 1, 1)", []).unwrap();
        let (old, _) = store.create_batch(db, &f, 1, &[("a1".into(), 1)], 20, &|_| true).unwrap();
        store.finish_batch(old, 25).unwrap();
        let (second, runs) = store.create_batch(db, &f, 1, &[("a1".into(), 1), ("b2".into(), 2), ("c3".into(), 3)], 30, &|_| true).unwrap();
        store.set_run_status(runs[0], RunState::Done, None, 31).unwrap();
        store.set_run_status(runs[1], RunState::Error, Some("x"), 32).unwrap();
        let rows = store.task_rows().unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [db, file], "новые сверху");
        assert_eq!((rows[0].flow_name.as_str(), rows[0].profiles, rows[0].threads, rows[0].running), ("Из базы", 2, 3, false));
        let last = rows[0].last.as_ref().unwrap();
        assert_eq!((last.id, last.total, last.done, last.error, last.queued, last.finished_at), (second, 3, 1, 1, 1, None));
        assert_eq!((rows[1].flow_name.as_str(), rows[1].last.clone()), ("Вход", None));
    }

    #[test]
    fn batch_summaries_and_run_rows() {
        let store = memory_store();
        let task = new_task(&store, &Row::new(), None, None);
        let (first, _) = store.create_batch(task, &flow(), 1, &[("a1".into(), 1)], 20, &|_| true).unwrap();
        store.finish_batch(first, 25).unwrap();
        let (second, runs) = store.create_batch(task, &flow(), 1, &[("zz".into(), 7), ("b2".into(), 2)], 30, &|_| true).unwrap();
        store.set_run_status(runs[1], RunState::Running, None, 31).unwrap();
        let ids: Vec<i64> = store.batch_summaries(task).unwrap().iter().map(|b| b.id).collect();
        assert_eq!(ids, [second, first]);
        assert_eq!(store.batch_task(second).unwrap(), Some(task));
        assert_eq!(store.batch_task(999).unwrap(), None);
        let rows = store.run_rows(second).unwrap();
        assert_eq!(rows.iter().map(|r| (r.profile_id.as_str(), r.position, r.name.as_str())).collect::<Vec<_>>(), [("zz", 0, ""), ("b2", 1, "")]);
        assert_eq!((rows[1].status, rows[1].started_at, rows[1].finished_at), (RunState::Running, Some(31), None));
    }

    #[test]
    fn full_task_edit_versions_work_and_preserves_history() {
        let store = memory_store();
        let flow = flow();
        let profiles = vec!["a1".into(), "b2".into()];
        let values = row(json!({ "secret": "private-value" }));
        let data = vec![row(json!({ "url": "https://one.example" })), row(json!({ "url": "https://two.example" }))];
        let original = NewTask { headless: None, distribution: Distribution::Queue, name: "Очередь", flow_id: None, flow: Some(&flow),
            profiles: &profiles, order: Order::Ordered, threads: 2, missing_row: MissingRow::Skip, params: &values, data: Some(&data) };
        let id = store.create_task(&original, 10).unwrap();
        let (batch, runs) = store.create_queue_batch_checked((id, Some(0)), &flow, &[0, 1], false, 11).unwrap().unwrap();
        store.set_run_status(runs[0].1, RunState::Done, None, 12).unwrap();
        store.set_run_status(runs[1].1, RunState::Error, Some("ошибка"), 12).unwrap();
        store.finish_batch(batch, 13).unwrap();
        let history = store.batch_summaries(id).unwrap();
        let old_runs = store.run_rows(batch).unwrap();
        let pool = vec!["b2".into()];
        let adjusted = NewTask { headless: Some(true), name: "Другое имя", threads: 7, profiles: &pool, order: Order::Random, ..original };
        assert!(!store.update_task(id, 0, &adjusted, &|_| true).unwrap());
        assert_eq!(store.get_task(id).unwrap().unwrap().revision, 0);
        assert_eq!(store.get_task(id).unwrap().unwrap().headless, Some(true));
        assert_eq!(store.task_rows().unwrap()[0].queue.as_ref().unwrap().done, 1);
        let new_data = vec![row(json!({ "url": "https://new.example" })), row(json!({ "url": "https://another.example" }))];
        let changed = NewTask { data: Some(&new_data), ..adjusted };
        assert!(store.update_task(id, 0, &changed, &|_| true).unwrap());
        let current = store.get_task(id).unwrap().unwrap();
        assert_eq!(current.revision, 1);
        assert_eq!(current.created_at, 10);
        assert_eq!(current.data.as_ref(), Some(&new_data));
        assert_eq!(store.batch_summaries(id).unwrap(), history);
        assert_eq!(store.run_rows(batch).unwrap(), old_runs);
        assert!(store.last_batch(id).unwrap().is_none());
        let list = store.task_rows().unwrap();
        assert_eq!(list[0].queue.as_ref().unwrap().done, 0);
        assert!(list[0].last.is_none());
        assert!(matches!(store.update_task(id, 0, &changed, &|_| true), Err(StoreError::TaskChanged)));
        assert!(matches!(store.create_queue_batch_checked((id, Some(0)), &flow, &[0, 1], false, 20), Err(StoreError::TaskChanged)));
        assert!(matches!(store.create_batch_checked((id, Some(0)), &flow, 1, &[], 20, &|_| true), Err(StoreError::TaskChanged)));
        let (_, fresh) = store.create_queue_batch_checked((id, Some(1)), &flow, &[0, 1], false, 20).unwrap().unwrap();
        assert_eq!(fresh.iter().map(|(index, _)| *index).collect::<Vec<_>>(), [0, 1]);
        assert!(matches!(store.update_task(id, 1, &changed, &|_| true), Err(StoreError::TaskRunning(_))));
        assert_eq!(store.get_task(id).unwrap().unwrap(), current);
        let encrypted: Vec<u8> = store.conn().query_row("SELECT params_enc FROM flow_tasks WHERE id = ?1", [id], |row| row.get(0)).unwrap();
        assert!(!contains(&encrypted, "private-value"));
    }

    #[test]
    fn editing_task_settings_keeps_payload_and_history() {
        let store = memory_store();
        let values = row(json!({ "login": "secret-login" }));
        let data = vec![row(json!({ "url": "https://example.test" }))];
        let task = new_task(&store, &values, Some(&data), Some(&flow()));
        let (batch, runs) = store.create_queue_batch(task, &flow(), &[0], false, 20).unwrap().unwrap();
        store.set_run_status(runs[0].1, RunState::Done, None, 21).unwrap();
        store.finish_batch(batch, 22).unwrap();
        let before_history = store.batch_summaries(task).unwrap();
        let before_runs = store.run_rows(batch).unwrap();
        let mut expected = store.get_task(task).unwrap().unwrap();
        expected.name = "Новое имя".into();
        expected.threads = 7;
        store.update_task_settings(task, "Новое имя", 7, &|_| true).unwrap();
        assert_eq!(store.get_task(task).unwrap().unwrap(), expected);
        assert_eq!(store.batch_summaries(task).unwrap(), before_history);
        assert_eq!(store.run_rows(batch).unwrap(), before_runs);
        assert!(store.create_queue_batch(task, &flow(), &[0], false, 23).unwrap().is_none(), "готовое задание не сброшено");
        assert!(matches!(store.update_task_settings(999, "Нет", 1, &|_| false), Err(StoreError::NotFound(_))));
    }

    #[test]
    fn editing_task_settings_refuses_a_live_foreign_batch() {
        let store = memory_store();
        let task = new_task(&store, &Row::new(), None, None);
        let before = store.get_task(task).unwrap().unwrap();
        store.create_batch(task, &flow(), 4242, &[("a1".into(), 1)], 20, &|_| true).unwrap();
        assert!(matches!(store.update_task_settings(task, "Новое", 5, &|pid| pid == 4242), Err(StoreError::TaskRunning(4242))));
        assert_eq!(store.get_task(task).unwrap().unwrap(), before);
    }

    #[test]
    fn deleting_a_task_removes_its_batches() {
        let store = memory_store();
        let task = new_task(&store, &Row::new(), None, None);
        let (batch, _) = store.create_batch(task, &flow(), 1, &[("a1".into(), 1)], 20, &|_| true).unwrap();
        store.finish_batch(batch, 21).unwrap();
        store.delete_task(task, &|_| true).unwrap();
        assert!(store.batch_summaries(task).unwrap().is_empty());
        assert!(store.run_rows(batch).unwrap().is_empty());
        assert!(store.task_rows().unwrap().is_empty());
    }

    #[test]
    fn delete_checks_the_live_batch_in_the_same_transaction() {
        let store = memory_store();
        let task = new_task(&store, &Row::new(), None, None);
        let (open, _) = store.create_batch(task, &flow(), 4242, &[("a1".into(), 1)], 20, &|_| true).unwrap();
        let refused = store.delete_task(task, &|pid| pid == 4242);
        assert!(matches!(refused, Err(StoreError::TaskRunning(4242))), "{refused:?}");
        assert!(store.get_task(task).unwrap().is_some(), "задача на месте");
        store.delete_task(task, &|_| false).unwrap();
        assert!(store.get_task(task).unwrap().is_none(), "пакет умершего процесса не мешает");
        assert!(store.batch_summaries(task).unwrap().iter().all(|b| b.id != open));
    }

    #[test]
    fn abort_closes_own_open_batch_with_an_error() {
        let store = memory_store();
        let task = new_task(&store, &Row::new(), None, None);
        let (_, runs) = store.create_batch(task, &flow(), 111, &[("a1".into(), 1), ("b2".into(), 2)], 20, &|_| true).unwrap();
        store.set_run_status(runs[0], RunState::Done, None, 21).unwrap();
        store.set_run_status(runs[1], RunState::Running, None, 21).unwrap();
        assert_eq!(store.abort_open_batches(task, 222, "сбой", 30).unwrap(), 0, "чужой pid не трогается");
        assert_eq!(store.abort_open_batches(task, 111, "сбой", 30).unwrap(), 1);
        let closed = store.last_batch(task).unwrap().unwrap();
        assert_eq!(closed.finished_at, Some(30));
        assert_eq!((closed.runs[0].status, closed.runs[1].status), (RunState::Done, RunState::Error));
        assert_eq!(closed.runs[1].error.as_deref(), Some("сбой"));
    }

    #[test]
    fn run_states_have_labels() {
        assert_eq!(RunState::Waiting.as_str(), "waiting");
        assert_eq!(RunState::Cancelled.label(), "отменён");
        assert_eq!(serde_json::to_value(RunState::Skipped).unwrap(), json!("skipped"));
    }
}
