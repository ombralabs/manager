//! Задачи автоматизации (спецификация А2, «Задачи, пакеты, запуски»): задача — процесс, профили, параметры и таблица;
//! пакет — одно её выполнение. Сейчас их вызывает CLI, в А3 — интерфейс.
use crate::app::Core;
use crate::engine::Engine;
use crate::flow::model::Flow;
use crate::flow::params::{self, Distribution, MissingRow, Order, Row, Table};
use crate::flow::run::lookup;
use crate::flow::validate::validate;
use crate::store::{Store, StoreError};
use crate::store::profiles::{GroupFilter, ListQuery, SortKey};
use crate::flow::journal::{self, Entry};
use crate::flow::run::{RunRequest, RunStatus, TaskInfo, run_once};
use crate::flow::vars;
use crate::store::tasks::RunState;
use crate::sys::unix_now;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use crate::store::tasks::NewTask;

pub const NAME_MAX: usize = 64;

/// Откуда процесс задачи.
#[derive(Debug, Clone)]
pub enum FlowSource {
    Db(i64),
    File(Box<Flow>),
}

/// Какие профили взять в задачу.
#[derive(Debug, Clone, PartialEq)]
pub enum ProfilePick {
    Ids(Vec<String>),
    Serials(Vec<i64>),
    Group(String),
}

/// Новая задача — то, что дают CLI и (в А3) интерфейс.
#[derive(Debug, Clone)]
pub struct TaskSpec {
    /// None — использовать режим из настроек процесса.
    pub headless: Option<bool>,
    pub distribution: Distribution,
    pub name: Option<String>,
    pub flow: FlowSource,
    pub profiles: ProfilePick,
    /// Значения задачи (`--set`): строки приводятся к типам переменных.
    pub params: Row,
    pub data: Option<Vec<Row>>,
    pub threads: u32,
    pub order: Order,
    pub missing_row: MissingRow,
}

/// Профили по выбору: в указанном порядке, группа — по возрастанию номера; повторы убираются. Профиль из корзины —
/// как несуществующий.
pub fn resolve_profiles(store: &Store, pick: &ProfilePick) -> Result<Vec<String>, String> {
    let ids: Vec<String> = match pick {
        ProfilePick::Ids(ids) => ids.clone(),
        ProfilePick::Serials(serials) => {
            let (mut ids, mut missing) = (Vec::new(), Vec::new());
            for serial in serials {
                match store.profile_by_serial(*serial).map_err(text)? {
                    Some(profile) => ids.push(profile.id),
                    None => missing.push(serial.to_string()),
                }
            }
            if !missing.is_empty() {
                return Err(format!("нет профилей с номерами (или они в корзине): {}", missing.join(", ")));
            }
            ids
        }
        ProfilePick::Group(name) => {
            let folded = crate::store::fold(name.trim());
            let group = store
                .list_groups()
                .map_err(text)?
                .into_iter()
                .find(|group| crate::store::fold(&group.name) == folded)
                .ok_or_else(|| format!("нет группы «{name}»"))?;
            let query = ListQuery { search: None, sort: SortKey::Serial, descending: false, group: Some(GroupFilter::Id(group.id)), tag: None };
            let ids: Vec<String> = store.list_profiles(&query).map_err(text)?.into_iter().map(|profile| profile.id).collect();
            if ids.is_empty() {
                return Err(format!("в группе «{name}» нет профилей"));
            }
            ids
        }
    };
    let mut seen = HashSet::new();
    let ids: Vec<String> = ids.into_iter().filter(|id| seen.insert(id.clone())).collect();
    if ids.is_empty() {
        return Err("не выбрано ни одного профиля".into());
    }
    if let ProfilePick::Ids(_) = pick {
        let mut missing = Vec::new();
        for id in &ids {
            if store.get_profile(id).map_err(text)?.is_none() {
                missing.push(id.as_str());
            }
        }
        if !missing.is_empty() {
            return Err(format!("нет таких профилей (или они в корзине): {}", missing.join(", ")));
        }
    }
    Ok(ids)
}

pub struct PreparedTask {
    pub name: String,
    pub profiles: Vec<String>,
    pub data: Option<Vec<Row>>,
}

/// Одинаковая проверка и нормализация при создании и редактировании задачи.
pub fn prepare_task<E: Engine>(core: &Core<E>, spec: &TaskSpec, now: i64) -> Result<PreparedTask, String> {
    let name = spec.name.clone().unwrap_or_else(|| default_name(now));
    if name.trim().is_empty() {
        return Err("имя задачи пустое".into());
    }
    if name.chars().count() > NAME_MAX {
        return Err(format!("имя задачи длиннее {NAME_MAX} символов"));
    }
    if spec.threads == 0 {
        return Err("Количество одновременно работающих профилей должно быть положительным".into());
    }
    let flow = match &spec.flow {
        FlowSource::Db(id) => core.store.get_flow(*id).map_err(text)?.ok_or_else(|| format!("Сценарий {id} не найден"))?.flow,
        FlowSource::File(flow) => (**flow).clone(),
    };
    let (_, table) = prepare(core, &flow, &spec.params, spec.data.as_deref(), spec.distribution)?;
    let profiles = resolve_profiles(&core.store, &spec.profiles)?;
    let data = if spec.distribution == Distribution::Queue { table.as_ref().map(|table| table.rows().to_vec()) } else { spec.data.clone() };
    Ok(PreparedTask { name, profiles, data })
}

/// Задача проверяется целиком до сохранения: имя, потоки, процесс, параметры с таблицей, профили.
pub fn create_task<E: Engine>(core: &Core<E>, spec: &TaskSpec, now: i64) -> Result<i64, String> {
    let prepared = prepare_task(core, spec, now)?;
    let (flow_id, file) = match &spec.flow {
        FlowSource::Db(id) => (Some(*id), None),
        FlowSource::File(flow) => (None, Some(&**flow)),
    };
    let task = NewTask {
        headless: spec.headless,
        distribution: spec.distribution,
        name: &prepared.name,
        flow_id,
        flow: file,
        profiles: &prepared.profiles,
        order: spec.order,
        threads: spec.threads,
        missing_row: spec.missing_row,
        params: &spec.params,
        data: prepared.data.as_deref(),
    };
    core.store.create_task(&task, now).map_err(text)
}

/// Ход пакета для CLI (в А3 — для экрана): смена статуса запуска, строки его журнала, заметки пакета.
#[derive(Debug, Clone)]
pub enum Progress {
    Run { serial: i64, status: RunState, error: Option<String> },
    Entry { serial: i64, entry: Entry },
    Note(String),
}

pub type ProgressSink = Arc<dyn Fn(Progress) + Send + Sync>;

/// Какие профили взять в новый пакет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    All,
    /// Только профили, чей запуск в последнем пакете — «Ошибка» или «Отменён».
    Failed,
}

/// Итог пакета — вывод `--json`. Повторять нечего — `batch_id` и `files_dir` пусты, запусков нет.
#[derive(Debug, Clone, Serialize)]
pub struct BatchReport {
    pub task_id: i64,
    pub batch_id: Option<i64>,
    pub files_dir: Option<String>,
    pub runs: Vec<RunReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub item_index: Option<usize>,
    pub acc_id: String,
    pub serial_number: i64,
    pub status: RunState,
    pub error: Option<String>,
    pub exported: Map<String, Value>,
}

/// Новый пакет задачи и его выполнение до конца или «Стопа» (`cancel`). `Err` — только до создания пакета.
pub async fn run_batch<E: Engine>(
    core: &Core<E>,
    task_id: i64,
    which: Which,
    cancel: CancellationToken,
    progress: ProgressSink,
) -> Result<BatchReport, String> {
    run_batch_with(core, task_id, which, cancel, CancellationToken::new(), progress).await
}

/// То же с `drain` («Отменить ожидающие»): запуски, ещё не начавшие работу (в очереди и ждущие профиль), становятся
/// «Отменён»; идущие доделывают.
pub async fn run_batch_with<E: Engine>(
    core: &Core<E>,
    task_id: i64,
    which: Which,
    cancel: CancellationToken,
    drain: CancellationToken,
    progress: ProgressSink,
) -> Result<BatchReport, String> {
    let store = &core.store;
    let task = store.get_task(task_id).map_err(text)?.ok_or_else(|| format!("нет задачи {task_id}"))?;
    let last = store.last_batch(task_id).map_err(text)?;
    if let Some(last) = &last
        && last.finished_at.is_none()
        && crate::sys::pid_alive(last.pid)
    {
        return Err(format!("задача {task_id} ещё выполняется (процесс {})", last.pid));
    }
    if task.distribution == Distribution::Queue {
        return crate::flow::task_queue::run(core, &task, which, cancel, drain, progress).await;
    }
    let failed: Option<HashSet<&str>> = (which == Which::Failed).then(|| {
        last.iter()
            .flat_map(|batch| &batch.runs)
            .filter(|run| matches!(run.status, RunState::Error | RunState::Cancelled))
            .map(|run| run.profile_id.as_str())
            .collect()
    });
    let keep = |id: &str| failed.as_ref().is_none_or(|failed| failed.contains(id));
    let picked = batch_profiles(&task.profiles, keep, task.order == Order::Random);
    if picked.is_empty() {
        return Ok(BatchReport { task_id, batch_id: None, files_dir: None, runs: Vec::new() });
    }
    // Процесс из базы — текущая версия: повтор после исправления процесса берёт исправленный.
    let mut flow = match (task.flow_id, &task.flow) {
        (Some(id), _) => store.get_flow(id).map_err(text)?.ok_or_else(|| format!("Сценарий задачи {id} удалён"))?.flow,
        (None, Some(flow)) => flow.clone(),
        (None, None) => return Err(format!("Для задачи {task_id} не выбран сценарий")),
    };
    if let Some(headless) = task.headless { flow.settings.headless = headless; }
    let (wide, table) = prepare(core, &flow, &task.params, task.data.as_deref(), task.distribution)?;
    let mut profiles = Vec::with_capacity(picked.len());
    for (slot, id) in picked {
        let record = store.get_profile(&id).map_err(text)?;
        profiles.push((slot, id, record));
    }
    let rows: Vec<(String, i64)> = profiles.iter().map(|(_, id, record)| (id.clone(), record.as_ref().map_or(0, |r| r.serial))).collect();
    // Проверка выше — быстрый ответ до подготовки; гонку двух «Повторить» закрывает проверка в транзакции пакета.
    let created = store.create_batch_checked((task_id, Some(task.revision)), &flow, std::process::id(), &rows, unix_now(), &crate::sys::pid_alive);
    let (batch_id, run_ids) = match created {
        Ok(created) => created,
        Err(StoreError::TaskRunning(pid)) => return Err(format!("задача {task_id} ещё выполняется (процесс {pid})")),
        Err(e) => return Err(text(e)),
    };
    let files = core.paths.task_files(task_id, batch_id);
    let report: Mutex<Vec<RunReport>> = Mutex::new(
        rows.iter()
            .map(|(id, serial)| RunReport { item_index: None, acc_id: id.clone(), serial_number: *serial, status: RunState::Queued, error: None, exported: Map::new() })
            .collect(),
    );
    let set = |position: usize, status: RunState, error: Option<String>, exported: Map<String, Value>| {
        if let Err(e) = store.set_run_status(run_ids[position], status, error.as_deref(), unix_now()) {
            log::warn!("статус запуска {} не записан: {e}", run_ids[position]);
        }
        {
            let mut report = report.lock().expect("мьютекс итога пакета отравлен");
            let run = &mut report[position];
            (run.status, run.error, run.exported) = (status, error.clone(), exported);
        }
        progress(Progress::Run { serial: rows[position].1, status, error });
    };
    let mut jobs = Vec::new();
    for (position, (slot, id, record)) in profiles.iter().enumerate() {
        let Some(record) = record else {
            set(position, RunState::Skipped, Some("профиль удалён или в корзине".into()), Map::new());
            continue;
        };
        let row = match &table {
            Some(table) => match table.row(*slot, record.serial, id, task.missing_row) {
                Some(row) => Some(row),
                None => {
                    set(position, RunState::Skipped, Some("нет строки таблицы для профиля".into()), Map::new());
                    continue;
                }
            },
            None => None,
        };
        jobs.push((position, *slot, id.as_str(), record.serial, params::layer(&task.params, row)));
    }
    let threads = (task.threads as usize).min(jobs.len()).max(1);
    let info = TaskInfo { id: task_id, name: task.name.clone(), files: files.clone() };
    let (set, wide, run_ids) = (&set, &wide, &run_ids);
    futures_util::stream::iter(jobs)
        .for_each_concurrent(threads, |(position, slot, id, serial, values)| {
            let (cancel, drain, info, progress) = (cancel.clone(), drain.clone(), info.clone(), progress.clone());
            async move {
                if cancel.is_cancelled() || drain.is_cancelled() {
                    set(position, RunState::Cancelled, None, Map::new());
                    return;
                }
                // Профиль занят другим запуском этого процесса (New Browser) — ждём его, держа место в потоках.
                let busy = match core.flows.claim(id) {
                    Some(busy) => busy,
                    None => {
                        set(position, RunState::Waiting, None, Map::new());
                        let claimed = tokio::select! {
                            busy = core.flows.claim_wait(id, &cancel) => busy,
                            _ = drain.cancelled() => None,
                        };
                        // select! при двух готовых ветках может выбрать захват: отмену перепроверяем, профиль отпускаем.
                        match claimed.filter(|_| !drain.is_cancelled()) {
                            Some(busy) => busy,
                            None => {
                                set(position, RunState::Cancelled, None, Map::new());
                                return;
                            }
                        }
                    }
                };
                set(position, RunState::Running, None, Map::new());
                let on_entry: journal::Sink = Arc::new(move |entry: &Entry| progress(Progress::Entry { serial, entry: entry.clone() }));
                let req = RunRequest {
                    flow: wide,
                    profile_id: id,
                    params: values,
                    run_id: run_ids[position].to_string(),
                    cancel,
                    // Input «по порядку» — строка по месту профиля в выборе задачи, как и строка таблицы.
                    position: slot,
                    task: Some(info),
                    busy: Some(busy),
                    on_entry: Some(on_entry),
                    from: None,
                    on_node: None,
                };
                let outcome = run_once(core, req).await;
                let status = match outcome.status {
                    RunStatus::Done => RunState::Done,
                    RunStatus::Error => RunState::Error,
                    RunStatus::Cancelled => RunState::Cancelled,
                };
                set(position, status, outcome.error, outcome.exported);
            }
        })
        .await;
    if let Err(e) = store.finish_batch(batch_id, unix_now()) {
        log::warn!("пакет {batch_id} не закрыт: {e}");
    }
    let runs = report.into_inner().expect("мьютекс итога пакета отравлен");
    Ok(BatchReport { task_id, batch_id: Some(batch_id), files_dir: Some(files.display().to_string()), runs })
}

/// Профили пакета (`keep`) с местом каждого в выборе задачи (с 0); `random` — перемешаны. Строка таблицы «по порядку»
/// и строка Input `in_order` берутся по этому месту: от `random` и «Повторить» не зависят (иначе повтор упавших
/// получил бы чужие строки).
fn batch_profiles(selection: &[String], keep: impl Fn(&str) -> bool, random: bool) -> Vec<(usize, String)> {
    let mut picked: Vec<(usize, String)> =
        selection.iter().enumerate().filter(|(_, id)| keep(id)).map(|(slot, id)| (slot, id.clone())).collect();
    if random {
        shuffle(&mut picked);
    }
    picked
}

/// Перестановка Фишера — Йетса: `random` перемешивает профили один раз на пакет.
pub(super) fn shuffle<T>(items: &mut [T]) {
    for i in (1..items.len()).rev() {
        items.swap(i, vars::random_index(i + 1));
    }
}

/// Процесс, готовый к запуску: колонки таблицы объявлены переменными, процесс проверен, параметры и каждая строка
/// приводятся к типам — ошибки до первого браузера.
pub(super) fn prepare<E: Engine>(core: &Core<E>, flow: &Flow, values: &Row, data: Option<&[Row]>, distribution: Distribution) -> Result<(Flow, Option<Table>), String> {
    let table = data.map(|rows| match distribution {
        Distribution::PerProfile => Table::new(rows.to_vec()),
        Distribution::Queue => Table::queue(rows.to_vec()),
    }).transpose()?;
    let mut wide = flow.clone();
    if let Some(table) = &table {
        params::widen(&mut wide, table);
    }
    let report = validate(&wide, &*lookup(core, &wide));
    if !report.is_ok() {
        return Err(format!("В сценарии есть ошибки: {}", report.summary()));
    }
    params::check(&wide, values, table.as_ref())?;
    Ok((wide, table))
}

/// `RPA_TASK_<ГГГГММДД_ЧЧММСС>` по местному времени, как в AdsPower.
fn default_name(now: i64) -> String {
    let time = now as libc::time_t;
    // SAFETY: обе структуры на стеке; `localtime_r` потокобезопасна.
    let tm = crate::sys::local_time(time);
    format!(
        "RPA_TASK_{:04}{:02}{:02}_{:02}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// Ошибка хранилища — строкой для ответа задачи.
fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::tests::{create_with_proxy, env};
    use crate::store::groups::GroupInput;
    use serde_json::{Value, json};

    pub(crate) fn flow(value: Value) -> Flow {
        let mut value = value;
        value["format"] = json!("ombra-flow/1");
        value["name"] = json!("Тест");
        serde_json::from_value(value).unwrap()
    }

    /// Процесс отдаёт `names` узлом Export Variable.
    pub(crate) fn exporting(names: Value, variables: Value) -> Flow {
        flow(json!({
            "variables": variables,
            "nodes": [{ "id": "s", "op": "start" }, { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": names } }],
            "edges": [{ "from": "s", "to": "x" }]
        }))
    }

    pub(crate) fn row(value: Value) -> Row {
        value.as_object().unwrap().clone()
    }

    pub(crate) fn spec(flow: Flow, profiles: ProfilePick) -> TaskSpec {
        TaskSpec {
            headless: None,
            distribution: Distribution::PerProfile,
            name: None,
            flow: FlowSource::File(Box::new(flow)),
            profiles,
            params: Row::new(),
            data: None,
            threads: 1,
            order: Order::Ordered,
            missing_row: MissingRow::Skip,
        }
    }

    fn group(store: &Store, name: &str) -> crate::store::groups::GroupRecord {
        let input = GroupInput { name: name.into(), color: "#0b6e6e".into(), note: String::new(), quick_access: false };
        store.create_group(&input, 1).unwrap()
    }

    #[tokio::test]
    async fn profiles_come_from_ids_serials_or_a_group() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        let c = create_with_proxy(&e.core, "C", e.proxy_port, 3);
        let store = &e.core.store;
        let ids = resolve_profiles(store, &ProfilePick::Ids(vec![c.id.clone(), a.id.clone(), c.id.clone()])).unwrap();
        assert_eq!(ids, [c.id.clone(), a.id.clone()], "порядок как задан, повторы убраны");
        assert_eq!(resolve_profiles(store, &ProfilePick::Serials(vec![b.serial, a.serial])).unwrap(), [b.id.clone(), a.id.clone()]);
        let night = group(store, "Ночные");
        store.move_profiles(&[c.id.clone(), a.id.clone()], Some(night.id)).unwrap();
        let grouped = resolve_profiles(store, &ProfilePick::Group("ночные".into())).unwrap();
        assert_eq!(grouped, [a.id.clone(), c.id.clone()], "группа — по возрастанию номера, имя без учёта регистра");
    }

    #[tokio::test]
    async fn bad_profile_selections_fail() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let gone = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        let store = &e.core.store;
        store.set_deleted(std::slice::from_ref(&gone.id), Some(5)).unwrap();
        let err = resolve_profiles(store, &ProfilePick::Ids(vec![a.id.clone(), gone.id.clone(), "nope01".into()])).unwrap_err();
        assert!(err.contains(&gone.id) && err.contains("nope01") && !err.contains(&a.id), "{err}");
        let err = resolve_profiles(store, &ProfilePick::Serials(vec![gone.serial, 99])).unwrap_err();
        assert!(err.contains(&gone.serial.to_string()) && err.contains("99"), "{err}");
        assert!(resolve_profiles(store, &ProfilePick::Group("Нет такой".into())).unwrap_err().contains("нет группы"));
        let empty = group(store, "Пустая");
        assert!(resolve_profiles(store, &ProfilePick::Group(empty.name)).unwrap_err().contains("нет профилей"));
        let many: Vec<String> = (0..=5000).map(|i| format!("p{i}")).collect();
        assert!(resolve_profiles(store, &ProfilePick::Ids(many)).unwrap_err().starts_with("нет таких профилей"));
    }

    #[tokio::test]
    async fn task_is_checked_before_it_is_stored() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let ok = exporting(json!(["n"]), json!([{ "name": "n", "type": "number", "default": 1 }]));
        let pick = ProfilePick::Ids(vec![a.id.clone()]);
        let broken = flow(json!({ "nodes": [{ "id": "s", "op": "start" }, { "id": "x", "op": "no.such" }], "edges": [{ "from": "s", "to": "x" }] }));
        let cases: Vec<(TaskSpec, &str)> = vec![
            (spec(broken, pick.clone()), "В сценарии есть ошибки"),
            (TaskSpec { params: row(json!({ "n": "abc" })), ..spec(ok.clone(), pick.clone()) }, "переменная n"),
            (TaskSpec { params: row(json!({ "nope": "1" })), ..spec(ok.clone(), pick.clone()) }, "nope"),
            (TaskSpec { data: Some(vec![row(json!({ "n": "2" })), row(json!({ "n": "x" }))]), ..spec(ok.clone(), pick.clone()) }, "строка 2"),
            (TaskSpec { threads: 0, ..spec(ok.clone(), pick.clone()) }, "должно быть положительным"),
            (TaskSpec { name: Some("я".repeat(65)), ..spec(ok.clone(), pick.clone()) }, "64"),
            (TaskSpec { flow: FlowSource::Db(999), ..spec(ok.clone(), pick.clone()) }, "Сценарий 999 не найден"),
            (spec(ok.clone(), ProfilePick::Ids(vec!["nope01".into()])), "nope01"),
        ];
        for (bad, expected) in cases {
            let err = create_task(&e.core, &bad, 10).unwrap_err();
            assert!(err.contains(expected), "{expected}: {err}");
        }
        assert!(e.core.store.get_task(1).unwrap().is_none(), "ничего не сохранено");
    }

    #[tokio::test]
    async fn file_task_keeps_its_json_and_db_task_its_id() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let f = exporting(json!(["n"]), json!([{ "name": "n", "type": "number", "default": 1 }]));
        let pick = ProfilePick::Ids(vec![a.id.clone()]);
        let id = create_task(&e.core, &TaskSpec { name: Some("Из файла".into()), ..spec(f.clone(), pick.clone()) }, 10).unwrap();
        let task = e.core.store.get_task(id).unwrap().unwrap();
        assert_eq!((task.name.as_str(), task.flow_id, task.flow.as_ref()), ("Из файла", None, Some(&f)));
        assert_eq!(task.profiles, std::slice::from_ref(&a.id));
        let db = e.core.store.create_flow(&f, 1).unwrap().id;
        let id = create_task(&e.core, &TaskSpec { flow: FlowSource::Db(db), ..spec(f.clone(), pick) }, 1_760_950_000).unwrap();
        let task = e.core.store.get_task(id).unwrap().unwrap();
        assert_eq!((task.flow_id, task.flow), (Some(db), None));
        let digits = task.name.strip_prefix("RPA_TASK_").unwrap();
        assert!(digits.len() == 15 && digits.chars().enumerate().all(|(i, c)| if i == 8 { c == '_' } else { c.is_ascii_digit() }), "{}", task.name);
    }

    use crate::app::tests::Env;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn recorder() -> (ProgressSink, Arc<Mutex<Vec<Progress>>>) {
        let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
        let sink = seen.clone();
        (Arc::new(move |event| sink.lock().unwrap().push(event)), seen)
    }

    fn statuses(seen: &Mutex<Vec<Progress>>, serial: i64) -> Vec<RunState> {
        let seen = seen.lock().unwrap();
        seen.iter()
            .filter_map(|event| match event {
                Progress::Run { serial: s, status, .. } if *s == serial => Some(*status),
                _ => None,
            })
            .collect()
    }

    fn task(e: &Env, flow: Flow, ids: &[&str], change: impl FnOnce(&mut TaskSpec)) -> i64 {
        let mut spec = spec(flow, ProfilePick::Ids(ids.iter().map(|id| id.to_string()).collect()));
        change(&mut spec);
        create_task(&e.core, &spec, 10).unwrap()
    }

    #[tokio::test]
    async fn browser_mode_is_inherited_or_overridden_in_both_distributions_and_repeats() {
        for distribution in [Distribution::PerProfile, Distribution::Queue] {
            for mode in [None, Some(true), Some(false)] {
                let e = env().await;
                let a = create_with_proxy(&e.core, "Режим браузера", e.proxy_port, 1);
                let mut f = exporting(json!([]), json!([]));
                f.settings.headless = true;
                let db = e.core.store.create_flow(&f, 1).unwrap().id;
                let mut s = spec(f.clone(), ProfilePick::Ids(vec![a.id]));
                s.flow = FlowSource::Db(db);
                s.distribution = distribution;
                s.headless = mode;
                let id = create_task(&e.core, &s, 1).unwrap();
                for inherited in [true, false] {
                    f.settings.headless = inherited;
                    e.core.store.update_flow(db, &f, 2).unwrap();
                    // Очередь с завершённым заданием больше не запустится — новая проверка на той же настройке.
                    let run_id = if inherited || distribution == Distribution::PerProfile { id } else { create_task(&e.core, &s, 3).unwrap() };
                    let result = run_batch(&e.core, run_id, Which::All, CancellationToken::new(), Arc::new(|_| {})).await.unwrap();
                    assert!(result.runs.iter().all(|run| run.status == RunState::Done), "{result:?}");
                    assert_eq!(e.core.engine.last_request().headless, mode.unwrap_or(inherited));
                    assert_eq!(e.core.store.get_flow(db).unwrap().unwrap().flow.settings.headless, inherited, "задача не меняет процесс");
                }
            }
        }
    }

    fn sleeping(ms: i64) -> Flow {
        flow(json!({
            "nodes": [{ "id": "s", "op": "start" }, { "id": "w", "op": "wait.time", "config": { "ms": ms } }],
            "edges": [{ "from": "s", "to": "w" }]
        }))
    }

    /// Extract Field по ключу `name` из переменной-объекта `d`: строка таблицы без ключа роняет запуск.
    fn picky() -> Flow {
        flow(json!({
            "variables": [{ "name": "d", "type": "object", "default": {} }],
            "nodes": [{ "id": "s", "op": "start" }, { "id": "f", "op": "proc.field", "config": { "data": "${d}", "key": "name", "save_to": "out" } }],
            "edges": [{ "from": "s", "to": "f" }]
        }))
    }

    #[tokio::test]
    async fn batch_runs_each_profile_with_its_row() {
        let e = env().await;
        let (a, b, c) = (
            create_with_proxy(&e.core, "A", e.proxy_port, 1),
            create_with_proxy(&e.core, "B", e.proxy_port, 2),
            create_with_proxy(&e.core, "C", e.proxy_port, 3),
        );
        let rows = vec![row(json!({ "login": "u1" })), row(json!({ "login": "u2" })), row(json!({ "login": "u3" }))];
        let id = task(&e, exporting(json!(["login", "task_id"]), json!([])), &[&a.id, &b.id, &c.id], |s| s.data = Some(rows));
        let (sink, seen) = recorder();
        let report = run_batch(&e.core, id, Which::All, CancellationToken::new(), sink).await.unwrap();
        let batch = report.batch_id.unwrap();
        let got: Vec<(&str, RunState, Value)> =
            report.runs.iter().map(|run| (run.acc_id.as_str(), run.status, run.exported["login"].clone())).collect();
        assert_eq!(got, [(a.id.as_str(), RunState::Done, json!("u1")), (b.id.as_str(), RunState::Done, json!("u2")), (c.id.as_str(), RunState::Done, json!("u3"))]);
        assert_eq!(report.runs[0].exported["task_id"], json!(id.to_string()));
        assert_eq!(report.files_dir, Some(e.core.paths.task_files(id, batch).display().to_string()));
        assert_eq!(statuses(&seen, a.serial), [RunState::Running, RunState::Done]);
        assert!(seen.lock().unwrap().iter().any(|event| matches!(event, Progress::Entry { serial, .. } if *serial == b.serial)));
        let last = e.core.store.last_batch(id).unwrap().unwrap();
        assert!(last.finished_at.is_some() && last.runs.iter().all(|run| run.status == RunState::Done));
        assert!(e.core.paths.flow_log(&last.runs[0].id.to_string()).is_file(), "журнал — по id запуска");
    }

    #[tokio::test]
    async fn unrestricted_threads_control_concurrency_and_ignore_the_legacy_limit() {
        let e = env().await;
        let profiles: Vec<_> = (1..=4).map(|i| create_with_proxy(&e.core, &format!("P{i}"), e.proxy_port, i)).collect();
        let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        let peak = |seen: &Mutex<Vec<Progress>>| {
            let (mut now, mut peak) = (0i32, 0i32);
            for event in seen.lock().unwrap().iter() {
                match event {
                    Progress::Run { status: RunState::Running, .. } => now += 1,
                    Progress::Run { status: RunState::Done, .. } => now -= 1,
                    _ => {}
                }
                peak = peak.max(now);
            }
            peak
        };
        let id = task(&e, sleeping(300), &ids, |s| s.threads = 2);
        let (sink, seen) = recorder();
        let started = Instant::now();
        let report = run_batch(&e.core, id, Which::All, CancellationToken::new(), sink).await.unwrap();
        assert!(report.runs.iter().all(|run| run.status == RunState::Done));
        assert_eq!(peak(&seen), 2);
        assert!(started.elapsed() >= Duration::from_millis(600));
        // An old persisted cap must not lower the chosen concurrency.
        e.core.store.set_setting("profiles.max_running", "1").unwrap();
        let id = task(&e, sleeping(50), &ids, |s| s.threads = 3);
        let (sink, seen) = recorder();
        run_batch(&e.core, id, Which::All, CancellationToken::new(), sink).await.unwrap();
        assert_eq!(peak(&seen), 3);
        assert!(!seen.lock().unwrap().iter().any(|event| matches!(event, Progress::Note(note) if note.contains("Лимит одновременных профилей"))));
    }

    #[tokio::test]
    async fn random_order_runs_every_profile_once() {
        let e = env().await;
        let profiles: Vec<_> = (1..=5).map(|i| create_with_proxy(&e.core, &format!("P{i}"), e.proxy_port, i)).collect();
        let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        let id = task(&e, exporting(json!(["n"]), json!([{ "name": "n", "type": "number", "default": 1 }])), &ids, |s| s.order = Order::Random);
        let (sink, _) = recorder();
        let report = run_batch(&e.core, id, Which::All, CancellationToken::new(), sink).await.unwrap();
        let mut ran: Vec<&str> = report.runs.iter().map(|run| run.acc_id.as_str()).collect();
        ran.sort();
        let mut all = ids.clone();
        all.sort();
        assert_eq!(ran, all);
        let mut shuffled: Vec<String> = (0..50).map(|i| i.to_string()).collect();
        let before = shuffled.clone();
        shuffle(&mut shuffled);
        let mut sorted = shuffled.clone();
        sorted.sort_by_key(|text| text.parse::<i32>().unwrap());
        assert_eq!(sorted, before, "перестановка");
        assert_ne!(shuffled, before, "50 элементов почти наверняка перемешаны");
    }

    #[test]
    fn batch_profiles_keep_their_selection_index() {
        let selection: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        // «Повторить --failed»: упали b и d — места 1 и 3, а не 0 и 1.
        assert_eq!(batch_profiles(&selection, |id| id == "b" || id == "d", false), [(1, "b".to_string()), (3, "d".to_string())]);
        // `random`: порядок другой, место каждого профиля — прежнее.
        let mut shuffled = batch_profiles(&selection, |_| true, true);
        shuffled.sort();
        assert_eq!(shuffled, selection.iter().cloned().enumerate().collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn missing_rows_are_skipped_or_take_a_random_row() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        let rows = vec![row(json!({ "serial_number": a.serial.to_string(), "login": "u1" }))];
        let f = exporting(json!(["login"]), json!([]));
        let id = task(&e, f.clone(), &[&a.id, &b.id], |s| s.data = Some(rows.clone()));
        let (sink, _) = recorder();
        let report = run_batch(&e.core, id, Which::All, CancellationToken::new(), sink).await.unwrap();
        assert_eq!(report.runs[0].status, RunState::Done);
        assert_eq!((report.runs[1].status, report.runs[1].error.as_deref()), (RunState::Skipped, Some("нет строки таблицы для профиля")));
        let id = task(&e, f, &[&a.id, &b.id], |s| {
            s.data = Some(rows);
            s.missing_row = MissingRow::Random;
        });
        let (sink, _) = recorder();
        let report = run_batch(&e.core, id, Which::All, CancellationToken::new(), sink).await.unwrap();
        assert_eq!((report.runs[1].status, report.runs[1].exported["login"].clone()), (RunState::Done, json!("u1")));
    }

    #[tokio::test]
    async fn stop_cancels_running_and_queued_runs() {
        let e = env().await;
        let profiles: Vec<_> = (1..=3).map(|i| create_with_proxy(&e.core, &format!("P{i}"), e.proxy_port, i)).collect();
        let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        let id = task(&e, sleeping(5000), &ids, |_| {});
        let (sink, seen) = recorder();
        let cancel = CancellationToken::new();
        let stopper = {
            let cancel = cancel.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                cancel.cancel();
            }
        };
        let started = Instant::now();
        let (report, ()) = tokio::join!(run_batch(&e.core, id, Which::All, cancel, sink), stopper);
        let report = report.unwrap();
        assert!(report.runs.iter().all(|run| run.status == RunState::Cancelled), "{:?}", report.runs);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(statuses(&seen, profiles[0].serial), [RunState::Running, RunState::Cancelled]);
        assert_eq!(statuses(&seen, profiles[2].serial), [RunState::Cancelled], "в очереди — не стартовал");
        for profile in &profiles {
            assert!(e.core.supervisor.running_info(&profile.id).is_none());
        }
    }

    #[tokio::test]
    async fn waiting_run_takes_the_profile_when_it_is_freed() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        // Профиль B занят New Browser другого запуска этого процесса.
        let held = e.core.flows.claim(&b.id).unwrap();
        let id = task(&e, exporting(json!(["n"]), json!([{ "name": "n", "type": "number", "default": 1 }])), &[&a.id, &b.id], |s| s.threads = 2);
        let (sink, seen) = recorder();
        let release = async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            drop(held);
        };
        let (report, ()) = tokio::join!(run_batch(&e.core, id, Which::All, CancellationToken::new(), sink), release);
        assert!(report.unwrap().runs.iter().all(|run| run.status == RunState::Done));
        assert_eq!(statuses(&seen, b.serial), [RunState::Waiting, RunState::Running, RunState::Done]);
    }

    #[tokio::test]
    async fn retry_failed_reruns_only_failed_profiles_with_their_rows() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        // Строки по порядку выбора: у B строка без ключа name — его запуск падает.
        let rows = vec![row(json!({ "d": { "name": "x" } })), row(json!({ "d": {} }))];
        let id = task(&e, picky(), &[&a.id, &b.id], |s| s.data = Some(rows));
        let (sink, _) = recorder();
        let first = run_batch(&e.core, id, Which::All, CancellationToken::new(), sink.clone()).await.unwrap();
        assert_eq!(first.runs.iter().map(|run| run.status).collect::<Vec<_>>(), [RunState::Done, RunState::Error]);
        let again = run_batch(&e.core, id, Which::Failed, CancellationToken::new(), sink.clone()).await.unwrap();
        assert_ne!(again.batch_id, first.batch_id);
        let got: Vec<(&str, RunState)> = again.runs.iter().map(|run| (run.acc_id.as_str(), run.status)).collect();
        assert_eq!(got, [(b.id.as_str(), RunState::Error)], "B снова со своей строкой, а не с первой");
        // Всё прошло — повторять нечего, пакет не создаётся.
        let fine = task(&e, picky(), &[&a.id], |s| s.data = Some(vec![row(json!({ "d": { "name": "x" } }))]));
        run_batch(&e.core, fine, Which::All, CancellationToken::new(), sink.clone()).await.unwrap();
        let nothing = run_batch(&e.core, fine, Which::Failed, CancellationToken::new(), sink).await.unwrap();
        assert_eq!((nothing.batch_id, nothing.files_dir, nothing.runs.len()), (None, None, 0));
    }

    #[tokio::test]
    async fn retry_refuses_a_running_task_and_a_deleted_flow_and_skips_trashed_profiles() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        let f = exporting(json!(["n"]), json!([{ "name": "n", "type": "number", "default": 1 }]));
        let db = e.core.store.create_flow(&f, 1).unwrap().id;
        let id = task(&e, f.clone(), &[&a.id, &b.id], |s| s.flow = FlowSource::Db(db));
        // Незакрытый пакет живого процесса (этого же) — задача ещё идёт.
        let (open, _) = e.core.store.create_batch(id, &f, std::process::id(), &[(a.id.clone(), a.serial)], 5, &|_| true).unwrap();
        let (sink, _) = recorder();
        let err = run_batch(&e.core, id, Which::All, CancellationToken::new(), sink.clone()).await.unwrap_err();
        assert!(err.contains("ещё выполняется"), "{err}");
        e.core.store.finish_batch(open, 6).unwrap();
        e.core.store.set_deleted(std::slice::from_ref(&b.id), Some(7)).unwrap();
        let report = run_batch(&e.core, id, Which::All, CancellationToken::new(), sink.clone()).await.unwrap();
        assert_eq!(report.runs[0].status, RunState::Done);
        assert_eq!((report.runs[1].status, report.runs[1].error.as_deref()), (RunState::Skipped, Some("профиль удалён или в корзине")));
        e.core.store.delete_flow(db).unwrap();
        let err = run_batch(&e.core, id, Which::All, CancellationToken::new(), sink).await.unwrap_err();
        assert!(err.contains("удалён"), "{err}");
    }

    #[tokio::test]
    async fn drain_cancels_only_runs_not_yet_started() {
        let e = env().await;
        let profiles: Vec<_> = (1..=3).map(|i| create_with_proxy(&e.core, &format!("P{i}"), e.proxy_port, i)).collect();
        let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        let id = task(&e, sleeping(600), &ids, |_| {});
        let (sink, seen) = recorder();
        let drain = CancellationToken::new();
        let drainer = {
            let (drain, seen) = (drain.clone(), seen.clone());
            async move {
                // Ждём, пока первый запуск реально пошёл (до 5 с), и только тогда отменяем ожидающих.
                for _ in 0..250 {
                    if !statuses(&seen, profiles[0].serial).is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                drain.cancel();
            }
        };
        let (report, ()) = tokio::join!(run_batch_with(&e.core, id, Which::All, CancellationToken::new(), drain, sink), drainer);
        let got: Vec<RunState> = report.unwrap().runs.iter().map(|run| run.status).collect();
        assert_eq!(got, [RunState::Done, RunState::Cancelled, RunState::Cancelled]);
    }

    #[tokio::test]
    async fn drain_at_the_moment_the_profile_is_freed_does_not_start_the_run() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        // select! выбирает готовую ветку случайно — несколько заходов.
        for _ in 0..12 {
            let held = e.core.flows.claim(&a.id).unwrap();
            let id = task(&e, exporting(json!(["n"]), json!([{ "name": "n", "type": "number", "default": 1 }])), &[&a.id], |_| {});
            let (sink, seen) = recorder();
            let drain = CancellationToken::new();
            let drainer = {
                let (drain, seen) = (drain.clone(), seen.clone());
                async move {
                    while statuses(&seen, a.serial).is_empty() {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    // Без await между освобождением и отменой: к следующему опросу готовы обе ветки.
                    drop(held);
                    drain.cancel();
                }
            };
            let (report, ()) = tokio::join!(run_batch_with(&e.core, id, Which::All, CancellationToken::new(), drain, sink), drainer);
            assert_eq!(report.unwrap().runs[0].status, RunState::Cancelled);
            assert_eq!(statuses(&seen, a.serial), [RunState::Waiting, RunState::Cancelled]);
            assert!(e.core.flows.claim(&a.id).is_some(), "профиль освобождён");
        }
    }
}
