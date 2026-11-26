//! Процессы автоматизации для окна (спецификация А3-1): список с признаком ошибок и переменными, файлы
//! импорта и экспорта, копия, удаление, умолчания переменных. Запуск и задачи — в `flow::tasks`.
use super::{CommandError, Core};
use crate::engine::Engine;
use crate::flow::model::{self, Flow, VarDecl};
use crate::flow::{bundle, journal, run, validate, vars};
use crate::store::tasks::{BatchSummary, RunRow, TaskRow};
use crate::store::flows::FlowRecord;
use crate::flow::params::{self, Distribution, MissingRow, Order, Table};
use crate::flow::tasks::{self, FlowSource, Progress, ProfilePick, ProgressSink, TaskSpec, Which};
use crate::supervisor::{DebugEvent, DebugKind, Event};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Пакеты задач, идущие в этом окне: `task_id → (стоп, отмена ожидающих)`.
#[derive(Default)]
pub struct Automation {
    running: Mutex<HashMap<i64, (CancellationToken, CancellationToken)>>,
    /// Отладка из редактора: одна на окно, `(id запуска, стоп)`.
    debug: Mutex<Option<(String, CancellationToken)>>,
    /// Тест: следующий пакет паникует сразу после старта.
    #[cfg(test)]
    panic_next: std::sync::atomic::AtomicBool,
}

/// Форма «Новая задача» из окна.
#[derive(Debug, Clone, Deserialize)]
pub struct TaskForm {
    #[serde(default)]
    pub headless: Option<bool>,
    #[serde(default)]
    pub distribution: Distribution,
    pub name: Option<String>,
    pub flow_id: i64,
    pub profile_ids: Vec<String>,
    pub params: Map<String, Value>,
    pub data_path: Option<String>,
    pub threads: u32,
    pub order: Order,
    pub missing_row: MissingRow,
}

/// Настройки сохранённой задачи, меняются без запуска нового пакета.
#[derive(Debug, Clone, Deserialize)]
pub struct TaskSettingsForm {
    pub name: String,
    pub threads: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskUpdateForm {
    #[serde(default)]
    pub headless: Option<bool>,
    pub revision: i64,
    pub distribution: Distribution,
    pub name: String,
    pub flow_id: Option<i64>,
    pub profile_ids: Vec<String>,
    pub params: Map<String, Value>,
    pub data_path: Option<String>,
    pub data: Option<Vec<Map<String, Value>>>,
    pub threads: u32,
    pub order: Order,
    pub missing_row: MissingRow,
}

#[derive(Debug, Serialize)]
pub struct TaskDetails {
    #[serde(flatten)]
    pub task: crate::store::tasks::TaskRecord,
    pub process: Option<FlowRow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TableDataPreviewForm {
    pub distribution: Distribution,
    pub profile_ids: Vec<String>,
    pub data: Vec<Map<String, Value>>,
    pub missing_row: MissingRow,
}

/// Предпросмотр таблицы на профили формы.
#[derive(Debug, Clone, Deserialize)]
pub struct TablePreviewForm {
    #[serde(default)]
    pub distribution: Distribution,
    pub profile_ids: Vec<String>,
    pub data_path: String,
    pub missing_row: MissingRow,
}

/// `key`: `serial_number`, `acc_id` или `order`; `skipped` — номера профилей без строки.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TablePreview {
    pub duplicates: usize,
    pub columns: Vec<String>,
    pub rows: usize,
    pub key: String,
    pub matched: usize,
    pub skipped: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowRow {
    pub headless: bool,
    pub id: i64,
    pub name: String,
    pub group: String,
    pub remark: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub has_errors: bool,
    pub variables: Vec<VarDecl>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldInfo {
    pub name: &'static str,
    pub kind: &'static str,
    pub options: Vec<&'static str>,
    pub required: bool,
    pub default: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpInfo {
    pub id: &'static str,
    pub adspower: &'static str,
    pub group: &'static str,
    pub inputs: Vec<&'static str>,
    pub outputs: Vec<&'static str>,
    pub fields: Vec<FieldInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpsCatalog {
    pub ops: Vec<OpInfo>,
    pub system: Vec<&'static str>,
}

fn bad(message: String) -> CommandError {
    CommandError::new("BadParams", message)
}

impl<E: Engine> Core<E> {
    fn flow_row(&self, record: FlowRecord) -> FlowRow {
        let flow = record.flow;
        let has_errors = !validate::validate(&flow, &*run::lookup(self, &flow)).is_ok();
        FlowRow {
            headless: flow.settings.headless,
            id: record.id,
            name: flow.name,
            group: flow.group,
            remark: flow.remark,
            created_at: record.created_at,
            updated_at: record.updated_at,
            has_errors,
            variables: flow.variables,
        }
    }

    fn flow_record(&self, id: i64) -> Result<FlowRecord, CommandError> {
        self.store.get_flow(id)?.ok_or_else(|| CommandError::new("NotFound", format!("сценарий {id}")))
    }

    /// Каталог операций для палитры и форм редактора.
    pub fn ops_catalog(&self) -> OpsCatalog {
        let ops = crate::flow::ops::OPS
            .iter()
            .map(|op| OpInfo {
                id: op.id,
                adspower: op.adspower,
                group: crate::flow::ops::group(op.id),
                inputs: op.shape.inputs().to_vec(),
                outputs: op.shape.outputs().to_vec(),
                fields: op
                    .fields
                    .iter()
                    .map(|f| FieldInfo {
                        name: f.name,
                        kind: f.kind.name(),
                        options: f.kind.options().to_vec(),
                        required: f.required,
                        // Реестр проверен тестом `registry_is_consistent`: умолчание — JSON, как читает и `Cfg::raw`.
                        default: f.default.map(|d| serde_json::from_str(d).expect("умолчание реестра — JSON")),
                    })
                    .collect(),
            })
            .collect();
        OpsCatalog { ops, system: vars::SYSTEM.to_vec() }
    }

    pub fn get_flow(&self, id: i64) -> Result<Flow, CommandError> {
        Ok(self.flow_record(id)?.flow)
    }

    /// Проверка несохранённого процесса; подпроцессы — как при запуске.
    pub fn validate_flow(&self, flow: &Flow) -> validate::Report {
        validate::validate(flow, &*run::lookup(self, flow))
    }

    /// Создать (`None`) или обновить; процесс с ошибками сохраняется.
    pub async fn save_flow(&self, id: Option<i64>, flow: Flow, now: i64) -> Result<FlowRow, CommandError> {
        let record = match id {
            None => {
                let mut flow = flow;
                flow.subflows.clear();
                self.store.create_flow(&flow, now)?
            },
            Some(id) => self.store.update_flow(id, &flow, now)?,
        };
        Ok(self.flow_row(record))
    }

    pub fn list_flows(&self) -> Result<Vec<FlowRow>, CommandError> {
        Ok(self.store.list_flows()?.into_iter().map(|r| self.flow_row(r)).collect())
    }

    pub async fn import_flow_file(&self, path: &Path, now: i64) -> Result<FlowRow, CommandError> {
        let text = std::fs::read_to_string(path).map_err(|e| CommandError::new("ImportFailed", format!("файл не прочитан: {e}")))?;
        self.import_flow_json(&text, now).await
    }

    pub async fn import_flow_json(&self, source: &str, now: i64) -> Result<FlowRow, CommandError> {
        let flow = model::parse(source).map_err(|e| CommandError::new("ImportFailed", e))?;
        let record = self.store.import_flow_bundle(&flow, now)?;
        Ok(self.flow_row(record))
    }

    pub fn export_flow_file(&self, id: i64, path: &Path) -> Result<(), CommandError> {
        let flow = bundle::export(&self.store, id)?;
        let text = serde_json::to_string_pretty(&flow).map_err(|e| CommandError::new("Internal", e.to_string()))?;
        std::fs::write(path, text)?;
        Ok(())
    }

    pub async fn copy_flow(&self, id: i64, now: i64) -> Result<FlowRow, CommandError> {
        let mut flow: Flow = self.flow_record(id)?.flow;
        flow.name = super::bulk::clone_name(&flow.name);
        Ok(self.flow_row(self.store.create_flow(&flow, now)?))
    }

    pub async fn delete_flow(&self, id: i64) -> Result<(), CommandError> {
        self.store.delete_flow(id)?;
        Ok(())
    }

    pub fn list_tasks(&self) -> Result<Vec<TaskRow>, CommandError> {
        self.close_dead_batches();
        let mut rows = self.store.task_rows()?;
        for row in &mut rows {
            row.running = self.is_task_running(row.id);
        }
        Ok(rows)
    }

    pub fn is_task_running(&self, task_id: i64) -> bool {
        self.automation.running.lock().expect("реестр пакетов отравлен").contains_key(&task_id)
    }

    pub fn get_task(&self, task_id: i64) -> Result<TaskDetails, CommandError> {
        let task = self.store.get_task(task_id)?.ok_or_else(|| CommandError::new("NotFound", format!("задача {task_id}")))?;
        let process = if let Some(id) = task.flow_id {
            self.store.get_flow(id)?.map(|record| self.flow_row(record))
        } else {
            task.flow.clone().map(|flow| self.flow_row(FlowRecord { id: -1, quota_id: String::new(), flow, created_at: task.created_at, updated_at: task.created_at }))
        };
        Ok(TaskDetails { task, process })
    }

    /// Сохраняет всю настройку без запуска; входные данные шифруются в хранилище.
    pub fn update_task(&self, task_id: i64, form: TaskUpdateForm, now: i64) -> Result<(), CommandError> {
        let old = self.store.get_task(task_id)?.ok_or_else(|| CommandError::new("NotFound", format!("задача {task_id}")))?;
        if old.revision != form.revision { return Err(crate::store::StoreError::TaskChanged.into()); }
        let flow = match form.flow_id {
            Some(id) => FlowSource::Db(id),
            None => FlowSource::File(Box::new(old.flow.ok_or_else(|| bad("Выберите сценарий".into()))?)),
        };
        let data = match (form.data_path, form.data) {
            (Some(_), Some(_)) => return Err(bad("выберите файл или сохранённую таблицу".into())),
            (Some(path), None) => Some(params::read_table(Path::new(&path)).map_err(bad)?),
            (None, data) => data,
        };
        let spec = TaskSpec { headless: form.headless, distribution: form.distribution, name: Some(form.name.trim().to_string()), flow,
            profiles: ProfilePick::Ids(form.profile_ids), params: form.params, data, threads: form.threads,
            order: form.order, missing_row: form.missing_row };
        let prepared = tasks::prepare_task(self, &spec, now).map_err(bad)?;
        let (flow_id, file) = match &spec.flow { FlowSource::Db(id) => (Some(*id), None), FlowSource::File(flow) => (None, Some(&**flow)) };
        let task = crate::store::tasks::NewTask { headless: spec.headless, distribution: spec.distribution, name: &prepared.name, flow_id, flow: file,
            profiles: &prepared.profiles, order: spec.order, threads: spec.threads, missing_row: spec.missing_row,
            params: &spec.params, data: prepared.data.as_deref() };
        let running = self.automation.running.lock().expect("реестр пакетов отравлен");
        if running.contains_key(&task_id) { return Err(CommandError::new("TaskRunning", "задача выполняется — сначала остановите её")); }
        self.store.update_task(task_id, form.revision, &task, &crate::sys::pid_alive)?;
        Ok(())
    }

    /// «Стоп»: отменяет и идущие, и ожидающие запуски; `false` — пакета нет.
    pub fn stop_task(&self, task_id: i64) -> bool {
        let running = self.automation.running.lock().expect("реестр пакетов отравлен");
        running.get(&task_id).map(|(stop, _)| stop.cancel()).is_some()
    }

    /// «Стоп» отладки; `false` — такого запуска нет.
    pub fn stop_debug(&self, run_id: &str) -> bool {
        let slot = self.automation.debug.lock().expect("слот отладки отравлен");
        slot.as_ref().filter(|(id, _)| id == run_id).map(|(_, stop)| stop.cancel()).is_some()
    }

    /// «Отменить ожидающие»: ещё не начавшие запуски отменяются, идущие доделывают.
    pub fn cancel_waiting(&self, task_id: i64) -> bool {
        let running = self.automation.running.lock().expect("реестр пакетов отравлен");
        running.get(&task_id).map(|(_, drain)| drain.cancel()).is_some()
    }

    /// Какие строки таблицы найдут профили формы; ничего не сохраняет.
    pub fn preview_table(&self, form: TablePreviewForm) -> Result<TablePreview, CommandError> {
        let rows = params::read_table(Path::new(&form.data_path)).map_err(bad)?;
        self.preview_rows(rows, form.distribution, form.profile_ids, form.missing_row)
    }

    pub fn preview_table_data(&self, form: TableDataPreviewForm) -> Result<TablePreview, CommandError> {
        self.preview_rows(form.data, form.distribution, form.profile_ids, form.missing_row)
    }

    fn preview_rows(&self, rows: Vec<Map<String, Value>>, distribution: Distribution, profile_ids: Vec<String>, missing_row: MissingRow) -> Result<TablePreview, CommandError> {
        // Колонки по первому появлению, внутри строки — в порядке файла (serde_json с preserve_order).
        let mut columns: Vec<String> = Vec::new();
        for name in rows.iter().flat_map(|row| row.keys()) {
            if !columns.contains(name) {
                columns.push(name.clone());
            }
        }
        let count = rows.len();
        let table = match distribution {
            Distribution::PerProfile => Table::new(rows),
            Distribution::Queue => Table::queue(rows),
        }.map_err(bad)?;
        if distribution == Distribution::Queue {
            return Ok(TablePreview { duplicates: count - table.rows().len(), columns, rows: count,
                key: table.key().unwrap_or("row").into(), matched: table.rows().len(), skipped: Vec::new() });
        }
        let (mut matched, mut skipped) = (0, Vec::new());
        // Повторы профилей убираются, как при создании задачи (`resolve_profiles`): иначе пропуски считаются иначе.
        let mut seen = std::collections::HashSet::new();
        let unique = profile_ids.iter().filter(|id| seen.insert(id.as_str()));
        for (index, id) in unique.enumerate() {
            let profile = self.store.get_profile(id)?.ok_or_else(|| bad(format!("нет профиля {id}")))?;
            if table.row(index, profile.serial, id, missing_row).is_some() {
                matched += 1;
            } else {
                skipped.push(profile.serial);
            }
        }
        Ok(TablePreview { duplicates: 0, columns, rows: count, key: table.key().unwrap_or("order").into(), matched, skipped })
    }

    pub fn list_batches(&self, task_id: i64) -> Result<Vec<BatchSummary>, CommandError> {
        Ok(self.store.batch_summaries(task_id)?)
    }

    pub fn list_runs(&self, batch_id: i64) -> Result<Vec<RunRow>, CommandError> {
        Ok(self.store.run_rows(batch_id)?)
    }

    /// Журнал запуска из файла `logs/flows/<id запуска>.jsonl`.
    pub fn run_journal(&self, run_id: i64) -> Result<Vec<journal::Entry>, CommandError> {
        Ok(journal::read(&self.paths.flow_log(&run_id.to_string()))?)
    }

    pub fn update_task_settings(&self, task_id: i64, form: TaskSettingsForm) -> Result<(), CommandError> {
        let name = form.name.trim();
        let mut errors = Vec::new();
        if name.is_empty() || name.chars().count() > tasks::NAME_MAX {
            errors.push(crate::fingerprint::FieldError { field: "name", code: "invalid" });
        }
        if form.threads == 0 {
            errors.push(crate::fingerprint::FieldError { field: "threads", code: "out_of_range" });
        }
        if !errors.is_empty() {
            return Err(CommandError::validation(errors));
        }
        // Замок не даёт локальному start_task пройти между проверкой и сохранением.
        let running = self.automation.running.lock().expect("реестр пакетов отравлен");
        if running.contains_key(&task_id) {
            return Err(CommandError::new("TaskRunning", "задача выполняется — сначала остановите её"));
        }
        self.store.update_task_settings(task_id, name, form.threads, &crate::sys::pid_alive)?;
        Ok(())
    }

    pub fn delete_task(&self, task_id: i64) -> Result<(), CommandError> {
        if self.is_task_running(task_id) {
            return Err(CommandError::new("TaskRunning", "задача выполняется — сначала остановите её"));
        }
        // Ids запусков собираем до каскадного удаления строк: по ним снимаются файлы журналов.
        let mut run_ids = Vec::new();
        for batch in self.store.batch_summaries(task_id)? {
            run_ids.extend(self.store.run_rows(batch.id)?.into_iter().map(|run| run.id));
        }
        // Пакет другого процесса Ombra (`ombra flow`) тоже держит задачу: проверка и удаление — одна транзакция.
        self.store.delete_task(task_id, &crate::sys::pid_alive)?;
        for id in run_ids {
            // Файла может не быть (запуск не дошёл до журнала) — это не ошибка.
            let _ = std::fs::remove_file(self.paths.flow_log(&id.to_string()));
        }
        Ok(())
    }

    /// Папка файлов пакета; создаётся, чтобы `open` не падал на пустом пакете.
    pub fn batch_files_dir(&self, batch_id: i64) -> Result<PathBuf, CommandError> {
        let task = self.store.batch_task(batch_id)?.ok_or_else(|| CommandError::new("NotFound", format!("запуск задачи {batch_id}")))?;
        let dir = self.paths.task_files(task, batch_id);
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Новые умолчания переменных; все значения приводятся к типам заранее, при ошибке запись не меняется.
    pub fn set_flow_defaults(&self, id: i64, values: Map<String, Value>, now: i64) -> Result<FlowRow, CommandError> {
        let mut flow = self.flow_record(id)?.flow;
        for (name, value) in values {
            let decl = flow.variables.iter_mut().find(|d| d.name == name).ok_or_else(|| bad(format!("переменная {name}: не объявлена")))?;
            decl.default = vars::coerce(decl.ty, value).map_err(|e| bad(format!("переменная {name}: {e}")))?;
        }
        Ok(self.flow_row(self.store.update_flow(id, &flow, now)?))
    }
}

impl<E: Engine + 'static> Core<E> {
    /// Создаёт задачу из формы и сразу запускает пакет из всех профилей.
    pub fn create_and_start_task(self: &Arc<Self>, form: TaskForm, now: i64) -> Result<i64, CommandError> {
        let data = form.data_path.as_deref().map(|path| params::read_table(Path::new(path))).transpose().map_err(bad)?;
        let spec = TaskSpec {
            headless: form.headless,
            distribution: form.distribution,
            name: form.name,
            flow: FlowSource::Db(form.flow_id),
            profiles: ProfilePick::Ids(form.profile_ids),
            params: form.params,
            data,
            threads: form.threads,
            order: form.order,
            missing_row: form.missing_row,
        };
        let id = tasks::create_task(self, &spec, now).map_err(bad)?;
        self.start_task(id, Which::All)?;
        Ok(id)
    }

    /// «Проверить» в форме узла: один узел `data.email` или `data.totp` без страницы, переменные — умолчания Start.
    /// Значение возвращается, в `save_to` ничего не пишется, письмо прочитанным не отмечается; ошибка — с текстом узла
    /// (секретов в нём нет).
    pub async fn test_node(&self, flow: Flow, node_id: String) -> Result<String, CommandError> {
        const SLOT: &str = "_probe";
        fn find<'a>(nodes: &'a [model::Node], id: &str) -> Option<&'a model::Node> {
            nodes.iter().find_map(|n| if n.id == id { Some(n) } else { find(&n.nodes, id) })
        }
        let node = find(&flow.nodes, &node_id).ok_or_else(|| CommandError::new("NotFound", format!("блок {node_id}")))?;
        let spec = crate::flow::ops::spec(&node.op).filter(|s| matches!(s.id, "data.email" | "data.totp" | "ext.google_sheet"));
        let Some(spec) = spec else { return Err(bad("проверка этой операции недоступна".into())) };
        let mut node = node.clone();
        node.config.insert("save_to".into(), Value::from(SLOT));
        // Только чтение: поле не задано, `null` или `read`; любое другое значение (число из импорта, опечатка) — отказ.
        let reading = match node.config.get("operation") {
            None | Some(Value::Null) => true,
            Some(operation) => *operation == "read",
        };
        if spec.id == "ext.google_sheet" && !reading {
            return Err(bad("Проверка доступна для чтения таблицы. Чтобы проверить запись или очистку, запустите сценарий.".into()));
        }
        if spec.id == "data.email" {
            node.config.insert("mark_read".into(), Value::Bool(false));
        }
        let mut ctx = crate::flow::interp::Ctx::new(
            crate::flow::vars::Scope::new(HashMap::new()),
            model::ErrorPolicy::Interrupt,
            Box::new(|_| None),
            journal::Journal::memory(),
            CancellationToken::new(),
        );
        vars::declare(&mut ctx.scope, &flow.variables, Map::new()).map_err(bad)?;
        ctx.store = Some(self.store.clone());
        crate::flow::ops::exec(&mut ctx, spec, &node).await.map_err(|e| match e {
            crate::flow::ops::OpError::Failed(message) => bad(message),
            _ => bad("проверка прервана".into()),
        })?;
        Ok(ctx.scope.get(SLOT).map(vars::to_text).unwrap_or_default())
    }

    /// Отладка процесса из редактора (черновик `flow`, без задачи) на профиле с номером `serial`, с Start или с узла
    /// `from`. Идёт в фоне; ход и конец — событиями `FlowDebug`. Возвращает id запуска.
    pub fn debug_flow(self: &Arc<Self>, flow: Flow, serial: i64, from: Option<Vec<String>>) -> Result<String, CommandError> {
        let profile = self.store.profile_by_serial(serial)?.ok_or_else(|| CommandError::new("NotFound", format!("профиль {serial}")))?;
        let run_id = format!("debug-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        let cancel = CancellationToken::new();
        {
            let mut slot = self.automation.debug.lock().expect("слот отладки отравлен");
            if let Some((_, stop)) = slot.as_ref() {
                // Остановленная держит слот, пока не закроет браузер (по настройкам «после задачи»).
                let text = if stop.is_cancelled() { "предыдущая отладка ещё останавливается — повторите через несколько секунд" } else { "отладка уже идёт" };
                return Err(bad(text.into()));
            }
            *slot = Some((run_id.clone(), cancel.clone()));
        }
        let core = self.clone();
        let id = run_id.clone();
        tokio::spawn(async move {
            let emit = |kind, node: Option<String>, entry, status, error| {
                core.supervisor.emit(Event::FlowDebug(DebugEvent { run_id: id.clone(), kind, node, entry, status, error }))
            };
            let (sink_core, sink_id) = (core.clone(), id.clone());
            let on_entry: journal::Sink = Arc::new(move |entry| {
                sink_core.supervisor.emit(Event::FlowDebug(DebugEvent { run_id: sink_id.clone(), kind: DebugKind::Entry, node: None, entry: Some(entry.clone()), status: None, error: None }))
            });
            let (node_core, node_id) = (core.clone(), id.clone());
            let on_node: crate::flow::interp::OnNode = Arc::new(move |node| {
                node_core.supervisor.emit(Event::FlowDebug(DebugEvent { run_id: node_id.clone(), kind: DebugKind::Node, node: Some(node.to_string()), entry: None, status: None, error: None }))
            });
            let req = run::RunRequest {
                flow: &flow,
                profile_id: &profile.id,
                params: Map::new(),
                run_id: id.clone(),
                cancel,
                position: 0,
                task: None,
                busy: None,
                on_entry: Some(on_entry),
                from,
                on_node: Some(on_node),
            };
            // Паника не должна оставить слот занятым: слот и событие конца — всегда.
            let (status, error) = match futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(run::run_once(&core, req))).await {
                Ok(outcome) => (outcome.status, outcome.error),
                Err(_) => (run::RunStatus::Error, Some("отладка прервалась: внутренняя ошибка".to_string())),
            };
            core.automation.debug.lock().expect("слот отладки отравлен").take();
            emit(DebugKind::End, None, None, Some(status), error);
        });
        Ok(run_id)
    }

    /// Пакеты умерших процессов (упавший `ombra flow`) закрываются; ошибка базы не мешает ответу.
    fn close_dead_batches(&self) {
        if let Err(e) = self.store.close_dead_batches(&crate::sys::pid_alive, crate::sys::unix_now()) {
            log::warn!("брошенные пакеты не закрыты: {e}");
        }
    }

    /// Запускает пакет в фоне. Ход и конец — событием `TasksChanged`; пакет, который не удалось начать, — с текстом ошибки.
    pub fn start_task(self: &Arc<Self>, task_id: i64, which: Which) -> Result<(), CommandError> {
        let (stop, drain) = (CancellationToken::new(), CancellationToken::new());
        {
            let mut running = self.automation.running.lock().expect("реестр пакетов отравлен");
            if running.contains_key(&task_id) {
                return Err(CommandError::new("TaskRunning", "задача уже выполняется"));
            }
            running.insert(task_id, (stop.clone(), drain.clone()));
        }
        self.close_dead_batches();
        let core = self.clone();
        tokio::spawn(async move {
            let changed = |error: Option<String>| core.supervisor.emit(Event::TasksChanged { task_id, error });
            let progress: ProgressSink = {
                let core = core.clone();
                Arc::new(move |event| {
                    if let Progress::Run { .. } = event {
                        #[cfg(test)]
                        if core.automation.panic_next.swap(false, std::sync::atomic::Ordering::SeqCst) {
                            panic!("тестовая паника");
                        }
                        core.supervisor.emit(Event::TasksChanged { task_id, error: None });
                    }
                })
            };
            // Паника не должна оставить задачу «идущей» в окне: реестр и событие — всегда, а открытый пакет в базе
            // (он хранит живой pid этого процесса) закрывается ошибкой — иначе запуск и удаление отказывали бы.
            let work = tasks::run_batch_with(&core, task_id, which, stop, drain, progress);
            let result = match futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(work)).await {
                Ok(result) => result,
                Err(_) => {
                    const TEXT: &str = "Запуск задачи прерван из-за внутренней ошибки";
                    if let Err(e) = core.store.abort_open_batches(task_id, std::process::id(), TEXT, crate::sys::unix_now()) {
                        log::warn!("пакет после паники не закрыт: {e}");
                    }
                    Err(TEXT.to_string())
                }
            };
            core.automation.running.lock().expect("реестр пакетов отравлен").remove(&task_id);
            changed(result.err());
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::app::tests::env;
    use crate::flow::model::{self, Flow};
    use serde_json::{json, Map};

    fn flow(value: serde_json::Value) -> Flow {
        serde_json::from_value(value).unwrap()
    }

    fn good() -> Flow {
        flow(json!({"format":"ombra-flow/1","name":"Хороший","variables":[{"name":"n","type":"number","default":1}],
            "nodes":[{"id":"s","op":"start"}]}))
    }

    /// Процесс с Start-умолчанием `key` и одним узлом `op`.
    fn probe(op: &str, config: serde_json::Value) -> Flow {
        flow(json!({"format":"ombra-flow/1","name":"П","variables":[{"name":"key","type":"string","default":"GEZDGNBV"},{"name":"pw","type":"string","default":"pa$$word"}],
            "nodes":[{"id":"s","op":"start"},{"id":"x","op":op,"config":config}],"edges":[{"from":"s","to":"x"}]}))
    }

    #[tokio::test]
    async fn unrestricted_flows_save_copy_and_import_beyond_the_old_quota() {
        let e = env().await;
        let first = e.core.save_flow(None, good(), 1).await.unwrap();
        let source = serde_json::to_string(&good()).unwrap();
        for i in 2..=6 {
            e.core.save_flow(None, good(), i).await.unwrap();
            e.core.copy_flow(first.id, i).await.unwrap();
            e.core.import_flow_json(&source, i).await.unwrap();
        }
        assert_eq!(e.core.list_flows().unwrap().len(), 16);
        e.core.delete_flow(first.id).await.unwrap();
        assert_eq!(e.core.list_flows().unwrap().len(), 15);
    }

    #[tokio::test]
    async fn unrestricted_upgrade_of_a_database_copy_preserves_profiles_secrets_and_old_quota_rows() {
        use crate::store::{Store, profiles::ProfileInput, secrets::{Secret, SecretField}};
        let dir = tempfile::tempdir().unwrap();
        let old_path = dir.path().join("previous.sqlite");
        let old = Store::open(&old_path, crate::store::tests::test_box()).unwrap();
        let mut input = ProfileInput::new("Existing profile", "{}");
        input.password = SecretField::Set(Secret::new("synthetic-site-password"));
        let profile = old.create_profile(&input, 1).unwrap();
        let first = old.create_flow(&good(), 1).unwrap();
        old.create_flow(&good(), 2).unwrap();
        old.set_setting("profiles.max_running", "1").unwrap();
        old.set_api_key(Some("synthetic-local-api-key")).unwrap();
        old.raw().execute("INSERT INTO rpa_licenses(resource, license, pending, pid) VALUES (?1, 'legacy-owner', 1, 0)", [&first.quota_id]).unwrap();
        let version = old.schema_version().unwrap();
        drop(old);
        let copied = dir.path().join("upgraded.sqlite");
        std::fs::copy(&old_path, &copied).unwrap();
        let store = std::sync::Arc::new(Store::open(&copied, crate::store::tests::test_box()).unwrap());
        let core = crate::app::Core::new(crate::paths::Paths::new(dir.path()), store.clone(),
            std::sync::Arc::new(crate::supervisor::test_engine::FakeEngine::new()),
            std::sync::Arc::new(crate::relay::geo::GeoClient::new()), std::sync::Arc::new(|_| {}),
            crate::supervisor::test_engine::fast_timings());
        core.save_flow(None, good(), 3).await.unwrap();
        core.copy_flow(first.id, 4).await.unwrap();
        assert_eq!(core.list_flows().unwrap().len(), 4);
        assert_eq!(store.get_profile(&profile.id).unwrap().unwrap(), profile);
        assert_eq!(store.profile_secrets(&profile.id).unwrap().0.unwrap().expose(), "synthetic-site-password");
        assert_eq!(store.api_key().unwrap().as_deref(), Some("synthetic-local-api-key"));
        assert_eq!(store.schema_version().unwrap(), version);
        assert_eq!(store.get_setting("profiles.max_running").unwrap().as_deref(), Some("1"));
        let pending: (String, i64) = store.raw().query_row("SELECT license, pending FROM rpa_licenses WHERE resource = ?1", [&first.quota_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(pending, ("legacy-owner".into(), 1));
        let original = Store::open(&old_path, crate::store::tests::test_box()).unwrap();
        assert_eq!(original.list_flows().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn unrestricted_task_parallelism_accepts_101_and_rejects_zero() {
        use crate::flow::tasks;
        let e = env().await;
        let saved = e.core.save_flow(None, good(), 1).await.unwrap();
        let profile = create_with_proxy(&e.core, "one", e.proxy_port, 1);
        let mut spec = tasks::TaskSpec { name: Some("parallel".into()), flow: tasks::FlowSource::Db(saved.id),
            profiles: tasks::ProfilePick::Ids(vec![profile.id]), params: Default::default(), data: None,
            threads: 101, order: crate::flow::params::Order::Ordered, missing_row: crate::flow::params::MissingRow::Skip,
            distribution: crate::flow::params::Distribution::PerProfile, headless: None };
        let id = tasks::create_task(&e.core, &spec, 1).unwrap();
        assert_eq!(e.core.get_task(id).unwrap().task.threads, 101);
        spec.threads = 0;
        assert!(tasks::create_task(&e.core, &spec, 1).is_err());
    }

    #[tokio::test]
    async fn test_node_gives_totp_from_the_start_default() {
        let e = env().await;
        let code = e.core.test_node(probe("data.totp", json!({"secret":"${key}","save_to":"c"})), "x".into()).await.unwrap();
        assert!(code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()), "{code}");
    }

    #[tokio::test]
    async fn test_node_reads_the_mailbox_and_never_shows_the_password() {
        use crate::flow::ops::email::tests::{config, fake, mailbox};
        let e = env().await;
        let server = fake(mailbox(&[1, 2, 3, 4])).await;
        let mut cfg = config(server.port, json!({"password":"${pw}","from":"instagram","extract":"\\d{6}"}));
        let ok = e.core.test_node(probe("data.email", json!(cfg)), "x".into()).await.unwrap();
        assert_eq!(ok, "654321");
        let wrong = fake(mailbox(&[1, 2, 3, 4])).await;
        cfg = config(wrong.port, json!({"password":"${key}"}));
        let err = e.core.test_node(probe("data.email", json!(cfg)), "x".into()).await.unwrap_err();
        assert!(!err.message.contains("GEZDGNBV") && !err.message.contains("pa$$word"), "{}", err.message);
        assert!(!err.message.is_empty());
    }

    #[tokio::test]
    async fn test_node_leaves_the_letter_unread() {
        use crate::flow::ops::email::tests::{config, fake, mailbox};
        let e = env().await;
        let server = fake(mailbox(&[1, 2, 3, 4])).await;
        e.core.test_node(probe("data.email", json!(config(server.port, json!({"mark_read": true})))), "x".into()).await.unwrap();
        assert!(!server.log().iter().any(|c| c.starts_with("UID STORE")), "{:?}", server.log());
    }

    #[tokio::test]
    async fn test_node_never_quotes_a_typed_password() {
        use crate::flow::ops::email::tests::config;
        let e = env().await;
        for typed in ["Ab${12", "x${a-b}"] {
            let err = e.core.test_node(probe("data.email", json!(config(9, json!({"password": typed})))), "x".into()).await.unwrap_err();
            assert_eq!(err.message, "поле «password»: неверная ссылка на переменную (буквальное «${» — «$${»)", "{typed}");
        }
    }

    #[tokio::test]
    async fn test_node_finds_a_node_inside_nested_groups() {
        let e = env().await;
        let f = flow(json!({"format":"ombra-flow/1","name":"Г","variables":[{"name":"key","type":"string","default":"GEZDGNBV"}],
            "nodes":[{"id":"s","op":"start"},{"id":"g","op":"control.group","nodes":[
                {"id":"t1","op":"data.totp","config":{"secret":"${key}","save_to":"c"}},
                {"id":"g2","op":"control.group","nodes":[{"id":"t2","op":"data.totp","config":{"secret":"${key}","save_to":"c"}}]}]}]}));
        for id in ["t1", "t2"] {
            let code = e.core.test_node(f.clone(), id.into()).await.unwrap();
            assert_eq!(code.len(), 6, "{id}: {code}");
        }
    }

    #[tokio::test]
    async fn test_node_reads_a_google_sheet_and_refuses_writing() {
        use crate::flow::ops::sheets::tests::fake;
        let e = env().await;
        fake(200, json!({ "values": [["a"], ["1"]] }), "a\n1\n").await;
        let cfg = json!({"spreadsheet_id":"SID","save_to":"c"});
        assert_eq!(e.core.test_node(probe("ext.google_sheet", cfg), "x".into()).await.unwrap(), r#"[{"a":"1"}]"#);
        let cfg = json!({"spreadsheet_id":"SID","operation":null,"save_to":"c"});
        assert_eq!(e.core.test_node(probe("ext.google_sheet", cfg), "x".into()).await.unwrap(), r#"[{"a":"1"}]"#);
        // Только чтение: запись, очистка и любое другое значение (число, флаг) — отказ до обращения к таблице.
        for operation in [json!("write"), json!("clear"), json!("erase"), json!(1), json!(true), json!({})] {
            let cfg = json!({"spreadsheet_id":"SID","operation":operation,"data":[[1]]});
            let err = e.core.test_node(probe("ext.google_sheet", cfg), "x".into()).await.unwrap_err();
            assert_eq!((err.code.as_str(), err.message.as_str()), ("BadParams", "Проверка доступна для чтения таблицы. Чтобы проверить запись или очистку, запустите сценарий."), "{operation}");
        }
    }

    #[tokio::test]
    async fn test_node_refuses_other_ops_and_missing_nodes() {
        let e = env().await;
        let err = e.core.test_node(probe("web.click", json!({})), "x".into()).await.unwrap_err();
        assert_eq!((err.code.as_str(), err.message.as_str()), ("BadParams", "проверка этой операции недоступна"));
        assert_eq!(e.core.test_node(probe("data.totp", json!({})), "нет".into()).await.unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn catalog_lists_every_registry_op_with_ports_and_fields() {
        let e = env().await;
        let c = e.core.ops_catalog();
        assert_eq!(c.ops.len(), crate::flow::ops::OPS.len());
        let op = |id: &str| c.ops.iter().find(|o| o.id == id).unwrap();
        assert_eq!(op("control.if").outputs, ["true", "false"]);
        assert_eq!(op("control.for_times").inputs, ["in", "loop"]);
        let button = op("web.click").fields.iter().find(|f| f.name == "button").unwrap();
        assert_eq!(button.kind, "choice");
        assert_eq!(button.options, ["left", "middle", "right"]);
        assert_eq!(button.default, Some(json!("left")));
        assert_eq!(op("start").group, "control");
        assert!(c.system.contains(&"password"));
    }

    #[tokio::test]
    async fn validate_flow_reports_unsaved_errors() {
        let e = env().await;
        let bad = flow(json!({"format":"ombra-flow/1","name":"П","nodes":[{"id":"s","op":"start"},{"id":"x","op":"no.such"}]}));
        let r = e.core.validate_flow(&bad);
        assert!(r.errors.iter().any(|i| i.node.as_deref() == Some("x")));
    }

    #[tokio::test]
    async fn save_flow_creates_then_updates() {
        let e = env().await;
        let row = e.core.save_flow(None, good(), 1).await.unwrap();
        let mut f = good();
        f.name = "Новое".into();
        let up = e.core.save_flow(Some(row.id), f, 5).await.unwrap();
        assert_eq!((up.id, up.name.as_str(), up.updated_at), (row.id, "Новое", 5));
        let bad = flow(json!({"format":"ombra-flow/1","name":"П","nodes":[{"id":"s","op":"start"},{"id":"x","op":"no.such"}]}));
        assert!(e.core.save_flow(Some(row.id), bad, 6).await.unwrap().has_errors);
    }

    #[tokio::test]
    async fn get_flow_returns_the_body() {
        let e = env().await;
        let id = e.core.save_flow(None, good(), 1).await.unwrap().id;
        assert_eq!(e.core.get_flow(id).unwrap().name, "Хороший");
        assert_eq!(e.core.get_flow(id + 99).unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn flows_are_listed_with_errors_and_variables() {
        let e = env().await;
        let a = e.core.store.create_flow(&good(), 1).unwrap().id;
        let bad = flow(json!({"format":"ombra-flow/1","name":"Плохой","nodes":[{"id":"s","op":"start"},{"id":"x","op":"no.such"}]}));
        let b = e.core.store.create_flow(&bad, 2).unwrap().id;
        let rows = e.core.list_flows().unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [a, b]);
        assert_eq!(rows.iter().map(|r| r.has_errors).collect::<Vec<_>>(), [false, true]);
        assert_eq!(rows[0].variables[0].name, "n");
    }

    #[tokio::test]
    async fn import_and_export_files_round_trip() {
        let e = env().await;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.json");
        let mut f = good();
        f.nodes.push(serde_json::from_value(json!({"id":"c","op":"control.call","config":{"process":9}})).unwrap());
        f.subflows.insert("9".into(), flow(json!({"format":"ombra-flow/1","name":"sub","nodes":[{"id":"s","op":"start"}]})));
        std::fs::write(&src, serde_json::to_string(&f).unwrap()).unwrap();
        let row = e.core.import_flow_file(&src, 5).await.unwrap();
        assert_eq!(row.name, "Хороший");
        let out = dir.path().join("out.json");
        e.core.export_flow_file(row.id, &out).unwrap();
        let parsed = model::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(parsed.name, "Хороший");
        assert!(!parsed.subflows.is_empty());
        assert_eq!(e.core.import_flow_file(&dir.path().join("нет.json"), 5).await.unwrap_err().code, "ImportFailed");
        let junk = dir.path().join("junk.json");
        std::fs::write(&junk, "не json").unwrap();
        assert_eq!(e.core.import_flow_file(&junk, 5).await.unwrap_err().code, "ImportFailed");
    }

    #[tokio::test]
    async fn import_json_keeps_subflows_and_does_not_modify_existing_flows() {
        let dir = tempfile::tempdir().unwrap();
        let core = crate::app::Core::new(
            crate::paths::Paths::new(dir.path()),
            std::sync::Arc::new(crate::store::tests::memory_store()),
            std::sync::Arc::new(crate::supervisor::test_engine::FakeEngine::new()),
            std::sync::Arc::new(crate::relay::geo::GeoClient::new()),
            std::sync::Arc::new(|_| {}), crate::supervisor::test_engine::fast_timings(),
        );
        let existing = core.save_flow(None, good(), 1).await.unwrap();
        let mut f = good();
        f.nodes.push(serde_json::from_value(json!({"id":"c","op":"control.call","config":{"process":9}})).unwrap());
        f.subflows.insert("9".into(), flow(json!({"format":"ombra-flow/1","name":"sub","nodes":[{"id":"s","op":"start"}]})));
        let source = serde_json::to_string(&f).unwrap();
        let row = core.import_flow_json(&source, 2).await.unwrap();
        assert_ne!(existing.id, row.id);
        assert_eq!(core.get_flow(existing.id).unwrap(), good());
        let exported = crate::flow::bundle::export(&core.store, row.id).unwrap();
        assert_eq!(exported.subflows.len(), 1);
        assert!(crate::flow::bundle::called(&exported).iter().all(|id| exported.subflows.contains_key(&id.to_string())));
        assert_eq!(core.import_flow_json("invalid", 3).await.unwrap_err().code, "ImportFailed");
        assert_eq!(core.list_flows().unwrap().len(), 3);
        assert!(core.import_flow_json(&source, 4).await.is_ok());
        assert_eq!(core.list_flows().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn copy_gets_a_new_id_and_suffix() {
        let e = env().await;
        let id = e.core.store.create_flow(&good(), 1).unwrap().id;
        let copy = e.core.copy_flow(id, 2).await.unwrap();
        assert_ne!(copy.id, id);
        assert_eq!(copy.name, "Хороший (копия)");
        let mut expected = good();
        expected.name = copy.name.clone();
        assert_eq!(e.core.store.get_flow(copy.id).unwrap().unwrap().flow, expected);
    }

    #[tokio::test]
    async fn copy_of_a_long_name_stays_valid() {
        let e = env().await;
        let mut long = good();
        long.name = "я".repeat(100);
        let id = e.core.store.create_flow(&long, 1).unwrap().id;
        let copy = e.core.copy_flow(id, 2).await.unwrap();
        assert!(!copy.has_errors);
        assert!(copy.name.ends_with(" (копия)"));
        assert_eq!(copy.name.chars().count(), 100);
    }

    #[tokio::test]
    async fn null_default_resets_to_the_empty_value() {
        let e = env().await;
        let id = e.core.store.create_flow(&good(), 1).unwrap().id;
        e.core.set_flow_defaults(id, serde_json::from_value(json!({"n": 5})).unwrap(), 2).unwrap();
        let row = e.core.set_flow_defaults(id, serde_json::from_value(json!({"n": null})).unwrap(), 3).unwrap();
        assert_eq!(row.variables.iter().find(|v| v.name == "n").unwrap().default, json!(0));
    }

    #[tokio::test]
    async fn defaults_are_coerced_and_unknown_names_refused() {
        let e = env().await;
        let id = e.core.store.create_flow(&good(), 1).unwrap().id;
        let map = |v: serde_json::Value| -> Map<String, serde_json::Value> { v.as_object().unwrap().clone() };
        assert_eq!(e.core.set_flow_defaults(id, map(json!({"n":"7"})), 2).unwrap().variables[0].default, json!(7));
        let before = e.core.store.get_flow(id).unwrap().unwrap();
        let err = e.core.set_flow_defaults(id, map(json!({"n":"abc"})), 3).unwrap_err();
        assert_eq!(err.code, "BadParams");
        assert_eq!(err.message, "переменная n: ожидается число");
        let err = e.core.set_flow_defaults(id, map(json!({"nope":"1"})), 3).unwrap_err();
        assert_eq!(err.code, "BadParams");
        assert!(err.message.contains("nope"));
        assert_eq!(e.core.store.get_flow(id).unwrap().unwrap(), before);
    }

    #[tokio::test]
    async fn full_task_edit_round_trips_all_fields_without_starting() {
        use crate::flow::params::{Distribution, MissingRow, Order};
        use crate::flow::tasks::{FlowSource, ProfilePick, TaskSpec};
        let e = env().await;
        let a = crate::app::tests::create_with_proxy(&e.core, "A", e.proxy_port, 1).id;
        let b = crate::app::tests::create_with_proxy(&e.core, "B", e.proxy_port, 2).id;
        let values = |value: serde_json::Value| value.as_object().unwrap().clone();
        let spec = TaskSpec { headless: None, distribution: Distribution::Queue, name: Some("Исходная".into()), flow: FlowSource::File(Box::new(good())),
            profiles: ProfilePick::Ids(vec![a, b.clone()]), params: values(json!({ "n": 3 })), data: Some(vec![values(json!({ "n": 5 }))]),
            threads: 2, order: Order::Ordered, missing_row: MissingRow::Skip };
        let id = crate::flow::tasks::create_task(&e.core, &spec, 10).unwrap();
        let loaded = e.core.get_task(id).unwrap();
        assert_eq!(loaded.process.as_ref().unwrap().id, -1);
        assert_eq!(loaded.task.params, spec.params);
        assert_eq!(loaded.task.data, spec.data);
        let mut other = good();
        other.name = "Другой".into();
        let flow_id = e.core.store.create_flow(&other, 11).unwrap().id;
        let form = super::TaskUpdateForm { headless: Some(true), revision: 0, distribution: Distribution::PerProfile, name: "Новая".into(), flow_id: Some(flow_id),
            profile_ids: vec![b.clone()], params: values(json!({ "n": 9 })), data: Some(vec![values(json!({ "n": 11 }))]), data_path: None,
            threads: 4, order: Order::Random, missing_row: MissingRow::Random };
        e.core.update_task(id, form.clone(), 12).unwrap();
        let saved = e.core.get_task(id).unwrap();
        assert_eq!((saved.task.name.as_str(), saved.task.threads, saved.task.flow_id), ("Новая", 4, Some(flow_id)));
        assert_eq!(saved.task.profiles, [b]);
        assert_eq!(saved.task.params, form.params);
        assert_eq!(saved.task.data, form.data);
        assert_eq!(saved.task.headless, Some(true));
        assert_eq!((saved.task.distribution, saved.task.order, saved.task.missing_row), (Distribution::PerProfile, Order::Random, MissingRow::Random));
        assert!(e.core.list_batches(id).unwrap().is_empty());
        assert!(!e.core.is_task_running(id));
        let mut invalid = super::TaskUpdateForm { revision: saved.task.revision, ..form };
        invalid.params = values(json!({ "n": "не число" }));
        assert_eq!(e.core.update_task(id, invalid.clone(), 13).unwrap_err().code, "BadParams");
        assert_eq!(e.core.store.get_task(id).unwrap().unwrap(), saved.task);
        invalid.params = values(json!({ "n": 12 }));
        invalid.profile_ids = vec!["missing".into()];
        assert_eq!(e.core.update_task(id, invalid, 13).unwrap_err().code, "BadParams");
        assert_eq!(e.core.store.get_task(id).unwrap().unwrap(), saved.task);
    }

    #[tokio::test]
    async fn editing_task_settings_validates_without_starting_a_batch() {
        let e = env().await;
        let (task, batch, _) = task_with_batch(&e);
        e.core.store.finish_batch(batch, 3).unwrap();
        let before = e.core.store.get_task(task).unwrap().unwrap();
        for (name, threads) in [(" ".to_string(), 1), ("я".repeat(65), 1), ("Имя".to_string(), 0)] {
            let error = e.core.update_task_settings(task, super::TaskSettingsForm { name, threads }).unwrap_err();
            assert_eq!(error.code, "Validation");
            assert_eq!(e.core.store.get_task(task).unwrap().unwrap(), before);
        }
        e.core.update_task_settings(task, super::TaskSettingsForm { name: "  Новое имя  ".into(), threads: 101 }).unwrap();
        let saved = e.core.store.get_task(task).unwrap().unwrap();
        assert_eq!((saved.name.as_str(), saved.threads), ("Новое имя", 101));
        assert_eq!(e.core.store.batch_summaries(task).unwrap().len(), 1);
        assert!(!e.core.is_task_running(task));
        let stop = tokio_util::sync::CancellationToken::new();
        e.core.automation.running.lock().unwrap().insert(task, (stop.clone(), stop));
        assert_eq!(e.core.update_task_settings(task, super::TaskSettingsForm { name: "Ещё".into(), threads: 5 }).unwrap_err().code, "TaskRunning");
        assert_eq!(e.core.store.get_task(task).unwrap().unwrap(), saved);
    }

    fn task_with_batch(e: &crate::app::tests::Env) -> (i64, i64, Vec<i64>) {
        use crate::flow::params::{MissingRow, Order};
        use crate::store::tasks::NewTask;
        let profiles = vec!["a1".to_string()];
        let task = e
            .core
            .store
            .create_task(
                &NewTask { headless: None, distribution: crate::flow::params::Distribution::PerProfile, name: "Т", flow_id: None, flow: Some(&good()), profiles: &profiles, order: Order::Ordered, threads: 1, missing_row: MissingRow::Skip, params: &Map::new(), data: None },
                1,
            )
            .unwrap();
        let (batch, runs) = e.core.store.create_batch(task, &good(), 1, &[("a1".into(), 1)], 2, &|_| true).unwrap();
        (task, batch, runs)
    }

    #[tokio::test]
    async fn run_journal_reads_the_run_file() {
        let e = env().await;
        let (_, batch, runs) = task_with_batch(&e);
        assert!(e.core.run_journal(runs[0]).unwrap().is_empty(), "файла нет");
        let mut j = crate::flow::journal::Journal::file(&e.core.paths.flow_log(&runs[0].to_string())).unwrap();
        j.event(crate::flow::journal::Status::Start, "go");
        drop(j);
        assert_eq!(e.core.run_journal(runs[0]).unwrap()[0].msg, "go");
        let dir = e.core.batch_files_dir(batch).unwrap();
        assert!(dir.is_dir());
        assert_eq!(e.core.batch_files_dir(999).unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn deleted_task_disappears_from_the_list() {
        let e = env().await;
        let (task, batch, runs) = task_with_batch(&e);
        assert_eq!(e.core.list_tasks().unwrap().len(), 1);
        assert_eq!(e.core.list_batches(task).unwrap().len(), 1);
        assert_eq!(e.core.list_runs(batch).unwrap().len(), 1);
        e.core.store.finish_batch(batch, 3).unwrap();
        let log = e.core.paths.flow_log(&runs[0].to_string());
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "{}").unwrap();
        e.core.delete_task(task).unwrap();
        assert!(!log.exists(), "журнал запуска удалён вместе с задачей");
        assert!(e.core.list_tasks().unwrap().is_empty());
        assert!(e.core.list_batches(task).unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_flow_is_not_found() {
        let e = env().await;
        assert_eq!(e.core.copy_flow(999, 1).await.unwrap_err().code, "NotFound");
        assert_eq!(e.core.delete_flow(999).await.unwrap_err().code, "NotFound");
        let out = tempfile::tempdir().unwrap().path().join("o.json");
        assert_eq!(e.core.export_flow_file(999, &out).unwrap_err().code, "NotFound");
    }

    use super::{TaskForm, TablePreview, TablePreviewForm};
    use crate::app::tests::{create_with_proxy, test_core_with_sink};
    use crate::app::Core;
    use crate::flow::params::{MissingRow, Order};
    use crate::flow::tasks::Which;
    use crate::store::tasks::RunState;
    use crate::supervisor::test_engine::{wait_until, FakeEngine};
    use crate::flow::run::RunStatus;
    use crate::supervisor::{DebugKind, Event};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct Rig {
        core: Arc<Core<FakeEngine>>,
        events: Arc<Mutex<Vec<Event>>>,
        port: u16,
        _dir: tempfile::TempDir,
    }

    async fn rig() -> Rig {
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let sink = events.clone();
        let (core, port, dir) = test_core_with_sink(Arc::new(move |e| sink.lock().unwrap().push(e))).await;
        Rig { core: Arc::new(core), events, port, _dir: dir }
    }

    fn sleeping(ms: i64) -> Flow {
        flow(json!({"format":"ombra-flow/1","name":"Сон","nodes":[{"id":"s","op":"start"},{"id":"w","op":"wait.time","config":{"ms":ms}}],
            "edges":[{"from":"s","to":"w"}]}))
    }

    /// Задача в базе: процесс «Сон» `ms` мс на `n` профилей, потоки `threads`.
    fn form(r: &Rig, ms: i64, n: usize, threads: u32) -> TaskForm {
        let flow_id = r.core.store.create_flow(&sleeping(ms), 1).unwrap().id;
        let profile_ids = (0..n).map(|i| create_with_proxy(&r.core, &format!("P{i}"), r.port, i as i64 + 1).id).collect();
        TaskForm { headless: None, distribution: crate::flow::params::Distribution::PerProfile, name: None, flow_id, profile_ids, params: Map::new(), data_path: None, threads, order: Order::Ordered, missing_row: MissingRow::Skip }
    }

    async fn finished(r: &Rig, task: i64) {
        for _ in 0..250 {
            if !r.core.is_task_running(task) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("пакет не закончился за 5 с");
    }

    fn statuses(r: &Rig, task: i64) -> Vec<RunState> {
        let batch = r.core.store.last_batch(task).unwrap().unwrap();
        batch.runs.iter().map(|run| run.status).collect()
    }

    #[tokio::test]
    async fn created_task_runs_in_the_background_and_reports_changes() {
        let r = rig().await;
        let f = form(&r, 200, 2, 2);
        let id = r.core.create_and_start_task(f, 5).unwrap();
        assert!(r.core.is_task_running(id));
        assert!(r.core.list_tasks().unwrap()[0].running);
        finished(&r, id).await;
        assert_eq!(statuses(&r, id), [RunState::Done, RunState::Done]);
        let events = r.events.lock().unwrap();
        let changes = events.iter().filter(|e| **e == Event::TasksChanged { task_id: id, error: None }).count();
        assert!(changes >= 2, "{changes}");
        assert!(!r.core.list_tasks().unwrap()[0].running);
    }

    #[tokio::test]
    async fn second_start_of_a_running_task_is_refused() {
        let r = rig().await;
        let f = form(&r, 2000, 1, 1);
        let id = r.core.create_and_start_task(f, 5).unwrap();
        assert_eq!(r.core.start_task(id, Which::All).unwrap_err().code, "TaskRunning");
        assert!(r.core.stop_task(id));
        finished(&r, id).await;
        assert!(!r.core.stop_task(id), "пакета уже нет");
    }

    #[tokio::test]
    async fn stop_cancels_and_cancel_waiting_lets_the_running_finish() {
        let r = rig().await;
        let f = form(&r, 600, 3, 1);
        let id = r.core.create_and_start_task(f, 5).unwrap();
        wait_until("первый пошёл", || r.core.store.last_batch(id).unwrap().is_some_and(|b| b.runs.iter().any(|run| run.status == RunState::Running))).await;
        assert!(r.core.cancel_waiting(id));
        finished(&r, id).await;
        assert_eq!(statuses(&r, id), [RunState::Done, RunState::Cancelled, RunState::Cancelled]);
        let f = form(&r, 2000, 3, 1);
        let id = r.core.create_and_start_task(f, 6).unwrap();
        wait_until("первый пошёл", || r.core.store.last_batch(id).unwrap().is_some_and(|b| b.runs.iter().any(|run| run.status == RunState::Running))).await;
        let started = std::time::Instant::now();
        assert!(r.core.stop_task(id));
        finished(&r, id).await;
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(statuses(&r, id), [RunState::Cancelled; 3]);
    }

    #[tokio::test]
    async fn batch_that_could_not_start_reports_its_error() {
        let r = rig().await;
        let f = form(&r, 100, 1, 1);
        let id = r.core.create_and_start_task(f, 5).unwrap();
        finished(&r, id).await;
        let (open, _) = r.core.store.create_batch(id, &sleeping(1), std::process::id(), &[(r.core.store.get_task(id).unwrap().unwrap().profiles[0].clone(), 1)], 6, &|_| true).unwrap();
        r.events.lock().unwrap().clear();
        r.core.start_task(id, Which::All).unwrap();
        wait_until("ошибка пакета", || {
            r.events.lock().unwrap().iter().any(|e| matches!(e, Event::TasksChanged { error: Some(text), .. } if text.contains("ещё выполняется")))
        })
        .await;
        finished(&r, id).await;
        assert!(!r.core.is_task_running(id));
        r.core.store.finish_batch(open, 7).unwrap();
    }

    #[tokio::test]
    async fn running_task_is_not_deleted() {
        let r = rig().await;
        let f = form(&r, 2000, 1, 1);
        let id = r.core.create_and_start_task(f, 5).unwrap();
        assert_eq!(r.core.delete_task(id).unwrap_err().code, "TaskRunning");
        assert_eq!(r.core.list_tasks().unwrap().len(), 1);
        assert!(r.core.list_tasks().unwrap()[0].running);
        r.core.stop_task(id);
        finished(&r, id).await;
        r.core.delete_task(id).unwrap();
    }

    #[tokio::test]
    async fn bad_form_is_bad_params() {
        let r = rig().await;
        let mut f = form(&r, 100, 1, 1);
        f.data_path = Some("/нет/такого.csv".into());
        assert_eq!(r.core.create_and_start_task(f.clone(), 5).unwrap_err().code, "BadParams");
        f.data_path = None;
        f.params.insert("nope".into(), json!("1"));
        assert_eq!(r.core.create_and_start_task(f, 5).unwrap_err().code, "BadParams");
        // Известное имя, но значение не того типа.
        let typed = flow(json!({"format":"ombra-flow/1","name":"Типы","variables":[{"name":"n","type":"number","default":1}],"nodes":[{"id":"s","op":"start"}]}));
        let mut f = form(&r, 100, 1, 1);
        f.flow_id = r.core.store.create_flow(&typed, 1).unwrap().id;
        f.params.insert("n".into(), json!("abc"));
        let err = r.core.create_and_start_task(f, 5).unwrap_err();
        assert!(err.code == "BadParams" && err.message.contains("переменная n"), "{err:?}");
        assert!(r.core.list_tasks().unwrap().is_empty());
    }

    #[tokio::test]
    async fn preview_counts_matched_and_skipped() {
        let r = rig().await;
        let f = form(&r, 100, 2, 1);
        let serials: Vec<i64> = f.profile_ids.iter().map(|id| r.core.store.get_profile(id).unwrap().unwrap().serial).collect();
        let dir = tempfile::tempdir().unwrap();
        let keyed = dir.path().join("k.csv");
        std::fs::write(&keyed, format!("serial_number,login\n{},u1\n", serials[0])).unwrap();
        let form = |path: &std::path::Path, missing| TablePreviewForm { distribution: crate::flow::params::Distribution::PerProfile, profile_ids: f.profile_ids.clone(), data_path: path.display().to_string(), missing_row: missing };
        let got = r.core.preview_table(form(&keyed, MissingRow::Skip)).unwrap();
        assert_eq!(got, TablePreview { duplicates: 0, columns: vec!["serial_number".into(), "login".into()], rows: 1, key: "serial_number".into(), matched: 1, skipped: vec![serials[1]] });
        assert!(r.core.preview_table(form(&keyed, MissingRow::Random)).unwrap().skipped.is_empty());
        // Повтор профиля в форме (как в исполнении) не удваивает счёт; колонки JSON — в порядке ключей файла.
        let repeated = TablePreviewForm { profile_ids: vec![f.profile_ids[0].clone(), f.profile_ids[0].clone(), f.profile_ids[1].clone()], ..form(&keyed, MissingRow::Skip) };
        let got = r.core.preview_table(repeated).unwrap();
        assert_eq!((got.matched, got.skipped), (1, vec![serials[1]]));
        let json = dir.path().join("j.json");
        std::fs::write(&json, r#"[{"zeta":"1","alpha":"2"},{"mid":"3","alpha":"4"}]"#).unwrap();
        assert_eq!(r.core.preview_table(form(&json, MissingRow::Skip)).unwrap().columns, ["zeta", "alpha", "mid"]);
        let plain = dir.path().join("p.csv");
        std::fs::write(&plain, "login\nu1\n").unwrap();
        let got = r.core.preview_table(form(&plain, MissingRow::Skip)).unwrap();
        assert_eq!((got.key.as_str(), got.matched, got.skipped), ("order", 1, vec![serials[1]]));
        let links = dir.path().join("links.csv");
        std::fs::write(&links, "url\nhttps://example.test/a\nhttps://example.test/b\nhttps://example.test/a\nhttps://example.test/c\n").unwrap();
        let mut shared = form(&links, MissingRow::Skip);
        shared.distribution = crate::flow::params::Distribution::Queue;
        let got = r.core.preview_table(shared).unwrap();
        assert_eq!((got.rows, got.matched, got.duplicates, got.skipped), (4, 3, 1, Vec::new()), "в очередь входят все уникальные строки, даже если профилей меньше");
    }

    #[tokio::test]
    async fn panicking_batch_is_cleaned_up_and_reported() {
        let r = rig().await;
        let id = r.core.create_and_start_task(form(&r, 100, 1, 1), 5).unwrap();
        finished(&r, id).await;
        r.core.automation.panic_next.store(true, std::sync::atomic::Ordering::SeqCst);
        r.events.lock().unwrap().clear();
        r.core.start_task(id, Which::All).unwrap();
        finished(&r, id).await;
        assert!(r.events.lock().unwrap().iter().any(|e| matches!(e, Event::TasksChanged { error: Some(t), .. } if t.contains("внутренней ошибки"))));
        // Паника — после создания пакета: он закрыт ошибкой, запуск и удаление работают.
        let last = r.core.store.last_batch(id).unwrap().unwrap();
        assert!(last.finished_at.is_some());
        assert_eq!(last.runs[0].status, RunState::Error);
        r.core.start_task(id, Which::All).unwrap();
        finished(&r, id).await;
        assert_eq!(statuses(&r, id), [RunState::Done]);
        r.core.delete_task(id).unwrap();
    }

    #[tokio::test]
    async fn list_tasks_closes_batches_of_dead_processes() {
        let e = env().await;
        let (task, batch, _) = task_with_batch(&e);
        e.core.store.finish_batch(batch, 2).unwrap();
        // Пакет упавшего `ombra flow`: pid не существует.
        e.core.store.create_batch(task, &good(), 2_000_000_000, &[], 3, &|_| false).unwrap();
        assert!(e.core.store.last_batch(task).unwrap().unwrap().finished_at.is_none());
        let rows = e.core.list_tasks().unwrap();
        assert!(rows[0].last.as_ref().unwrap().finished_at.is_some(), "сводка уже закрытого пакета");
        assert!(!rows[0].running);
        assert!(e.core.store.last_batch(task).unwrap().unwrap().finished_at.is_some());
    }

    #[tokio::test]
    async fn summary_shows_the_newest_batch_and_run_names_come_from_db_and_trash() {
        let r = rig().await;
        let live = create_with_proxy(&r.core, "Живой", r.port, 1);
        let trashed = create_with_proxy(&r.core, "Корзина", r.port, 2);
        r.core.store.set_deleted(std::slice::from_ref(&trashed.id), Some(5)).unwrap();
        let (task, first, _) = task_with_batch_for(&r, &[]);
        r.core.store.finish_batch(first, 3).unwrap();
        let rows = [(live.id.clone(), live.serial), (trashed.id.clone(), trashed.serial), ("gone00".into(), 9)];
        let (second, _) = r.core.store.create_batch(task, &good(), 1, &rows, 4, &|_| true).unwrap();
        let tasks = r.core.list_tasks().unwrap();
        assert_eq!(tasks[0].last.as_ref().unwrap().id, second, "в сводке — новейший пакет, не первый");
        let names: Vec<String> = r.core.list_runs(second).unwrap().into_iter().map(|run| run.name).collect();
        assert_eq!(names, ["Живой", "Корзина", ""]);
    }

    fn task_with_batch_for(r: &Rig, profiles: &[(String, i64)]) -> (i64, i64, Vec<i64>) {
        use crate::store::tasks::NewTask;
        let ids = vec!["a1".to_string()];
        let task = r
            .core
            .store
            .create_task(&NewTask { headless: None, distribution: crate::flow::params::Distribution::PerProfile, name: "Т", flow_id: None, flow: Some(&good()), profiles: &ids, order: Order::Ordered, threads: 1, missing_row: MissingRow::Skip, params: &Map::new(), data: None }, 1)
            .unwrap();
        let (batch, runs) = r.core.store.create_batch(task, &good(), 1, profiles, 2, &|_| true).unwrap();
        (task, batch, runs)
    }

    #[test]
    fn tasks_changed_json_shape_is_stable() {
        let ok = serde_json::to_value(Event::TasksChanged { task_id: 7, error: None }).unwrap();
        assert_eq!(ok, json!({"kind": "tasks_changed", "task_id": 7, "error": null}));
        let bad = serde_json::to_value(Event::TasksChanged { task_id: 7, error: Some("сбой".into()) }).unwrap();
        assert_eq!(bad, json!({"kind": "tasks_changed", "task_id": 7, "error": "сбой"}));
    }

    fn debug_events(r: &Rig) -> Vec<crate::supervisor::DebugEvent> {
        r.events.lock().unwrap().iter().filter_map(|e| if let Event::FlowDebug(d) = e { Some(d.clone()) } else { None }).collect()
    }

    async fn debug_ended(r: &Rig) {
        wait_until("конец отладки", || debug_events(r).iter().any(|d| d.kind == DebugKind::End)).await;
    }

    fn serial(r: &Rig, n: i64) -> i64 {
        create_with_proxy(&r.core, "D", r.port, n).serial
    }

    #[tokio::test]
    async fn debug_runs_and_reports_node_entry_end() {
        let r = rig().await;
        let serial = serial(&r, 1);
        let id = r.core.debug_flow(sleeping(0), serial, None).unwrap();
        debug_ended(&r).await;
        let got = debug_events(&r);
        assert!(got.iter().all(|d| d.run_id == id));
        let first_node = got.iter().position(|d| d.kind == DebugKind::Node).expect("начало узла");
        assert_eq!(got[first_node].node.as_deref(), Some("w"), "Start не подсвечивается: обход начинается после него");
        let first_entry = got.iter().position(|d| d.kind == DebugKind::Entry && d.entry.as_ref().is_some_and(|e| e.node == "w")).expect("строка журнала узла");
        assert!(first_entry > first_node, "начало узла раньше его строки");
        let end = got.last().unwrap();
        assert_eq!((end.kind, end.status), (DebugKind::End, Some(RunStatus::Done)));
        assert!(r.core.automation.debug.lock().unwrap().is_none(), "слот свободен");
    }

    #[tokio::test]
    async fn debug_from_node_starts_there() {
        let r = rig().await;
        let serial = serial(&r, 1);
        r.core.debug_flow(sleeping(0), serial, Some(vec!["w".into()])).unwrap();
        debug_ended(&r).await;
        let nodes: Vec<_> = debug_events(&r).into_iter().filter(|d| d.kind == DebugKind::Node).filter_map(|d| d.node).collect();
        assert_eq!(nodes, ["w"]);
    }

    #[tokio::test]
    async fn second_debug_is_refused_while_running() {
        let r = rig().await;
        let serial = serial(&r, 1);
        let id = r.core.debug_flow(sleeping(2000), serial, None).unwrap();
        let err = r.core.debug_flow(sleeping(0), serial, None).unwrap_err();
        assert_eq!((err.code.as_str(), err.message.as_str()), ("BadParams", "отладка уже идёт"));
        assert!(r.core.stop_debug(&id));
        // Остановленная отладка держит слот, пока закрывается её браузер: отказ говорит, чего ждать.
        let err = r.core.debug_flow(sleeping(0), serial, None).unwrap_err();
        assert_eq!(err.message, "предыдущая отладка ещё останавливается — повторите через несколько секунд");
        debug_ended(&r).await;
    }

    #[tokio::test]
    async fn stop_debug_cancels_and_ends_cancelled() {
        let r = rig().await;
        let serial = serial(&r, 1);
        let id = r.core.debug_flow(sleeping(5000), serial, None).unwrap();
        wait_until("узел сна пошёл", || debug_events(&r).iter().any(|d| d.node.as_deref() == Some("w"))).await;
        assert!(!r.core.stop_debug("чужой"));
        let started = std::time::Instant::now();
        assert!(r.core.stop_debug(&id));
        debug_ended(&r).await;
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(debug_events(&r).last().unwrap().status, Some(RunStatus::Cancelled));
        assert!(!r.core.stop_debug(&id), "запуска уже нет");
        r.core.debug_flow(sleeping(0), serial, None).unwrap();
    }

    #[tokio::test]
    async fn debug_unknown_serial_is_not_found() {
        let r = rig().await;
        assert_eq!(r.core.debug_flow(sleeping(0), 999, None).unwrap_err().code, "NotFound");
        assert!(r.core.automation.debug.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn debug_entry_hides_secret() {
        let r = rig().await;
        let serial = serial(&r, 1);
        let f = flow(json!({"format":"ombra-flow/1","name":"С","variables":[{"name":"p","type":"string","default":"S3cr3t-значение"}],
            "nodes":[{"id":"s","op":"start"},{"id":"i","op":"web.input","config":{"target":{"by":"css","selector":"#q"},"content":"${p}","interval_ms":0}}],
            "edges":[{"from":"s","to":"i"}]}));
        r.core.debug_flow(f, serial, None).unwrap();
        debug_ended(&r).await;
        let got = debug_events(&r);
        assert!(got.iter().any(|d| d.entry.as_ref().is_some_and(|e| e.node == "i")), "строка web.input есть");
        assert!(!format!("{got:?}").contains("S3cr3t"), "{got:?}");
    }

    #[tokio::test]
    async fn debug_with_validation_errors_ends_with_error() {
        let r = rig().await;
        let serial = serial(&r, 1);
        let bad = flow(json!({"format":"ombra-flow/1","name":"П","nodes":[{"id":"s","op":"start"},{"id":"x","op":"no.such"}],"edges":[{"from":"s","to":"x"}]}));
        r.core.debug_flow(bad, serial, None).unwrap();
        debug_ended(&r).await;
        let end = debug_events(&r).pop().unwrap();
        assert_eq!(end.status, Some(RunStatus::Error));
        assert!(end.error.is_some());
    }

    #[tokio::test]
    async fn task_running_in_another_process_is_not_deleted() {
        let r = rig().await;
        let id = r.core.create_and_start_task(form(&r, 50, 1, 1), 5).unwrap();
        finished(&r, id).await;
        let profile = r.core.store.get_task(id).unwrap().unwrap().profiles[0].clone();
        let (open, _) = r.core.store.create_batch(id, &sleeping(1), std::process::id(), &[(profile, 1)], 6, &|_| true).unwrap();
        assert_eq!(r.core.delete_task(id).unwrap_err().code, "TaskRunning");
        assert_eq!(r.core.list_tasks().unwrap().len(), 1);
        r.core.store.finish_batch(open, 7).unwrap();
        r.core.delete_task(id).unwrap();
        assert!(r.core.list_tasks().unwrap().is_empty());
    }
}
