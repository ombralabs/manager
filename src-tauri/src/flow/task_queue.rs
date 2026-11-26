//! Общая очередь задачи: один элемент — один запуск, профиль возвращается в пул после завершения.
//! Успех хранится по индексу элемента в базе и никогда не выдаётся повторно в этой задаче.
use crate::app::Core;
use crate::engine::Engine;
use crate::flow::journal::{self, Entry};
use crate::flow::params;
use crate::flow::run::{RunRequest, RunStatus, TaskInfo, run_once};
use crate::flow::tasks::{self, BatchReport, Progress, ProgressSink, RunReport, Which};
use crate::store::tasks::{RunState, TaskRecord};
use crate::sys::unix_now;
use futures_util::StreamExt;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub(super) async fn run<E: Engine>(
    core: &Core<E>, task: &TaskRecord, which: Which, cancel: CancellationToken, drain: CancellationToken, progress: ProgressSink,
) -> Result<BatchReport, String> {
    let mut flow = match (task.flow_id, &task.flow) {
        (Some(id), _) => core.store.get_flow(id).map_err(text)?.ok_or_else(|| format!("Сценарий задачи {id} удалён"))?.flow,
        (None, Some(flow)) => flow.clone(),
        _ => return Err("Для задачи не выбран сценарий".into()),
    };
    if let Some(headless) = task.headless { flow.settings.headless = headless; }
    let (wide, table) = tasks::prepare(core, &flow, &task.params, task.data.as_deref(), task.distribution)?;
    let mut profiles = VecDeque::new();
    for id in &task.profiles {
        if let Some(record) = core.store.get_profile(id).map_err(text)? { profiles.push_back((id.clone(), record.serial)); }
    }
    if profiles.is_empty() { return Err("в очереди нет доступных профилей: выбранные профили удалены или в корзине".into()); }
    let mut items: Vec<usize> = (0..table.as_ref().map_or(1, |t| t.rows().len())).collect();
    if task.order == params::Order::Random { tasks::shuffle(&mut items); }
    let Some((batch_id, runs)) = core.store.create_queue_batch_checked((task.id, Some(task.revision)), &flow, &items, which == Which::Failed, unix_now()).map_err(text)? else {
        return Ok(BatchReport { task_id: task.id, batch_id: None, files_dir: None, runs: Vec::new() });
    };
    let files = core.paths.task_files(task.id, batch_id);
    let info = TaskInfo { id: task.id, name: task.name.clone(), files: files.clone() };
    let threads = (task.threads as usize).min(profiles.len()).min(runs.len()).max(1);
    let pool = Mutex::new(profiles);
    let report = Mutex::new(runs.iter().map(|(item, _)| RunReport {
        item_index: Some(*item),
        acc_id: String::new(), serial_number: 0, status: RunState::Queued, error: None, exported: Default::default(),
    }).collect::<Vec<_>>());
    let persistence_error = Mutex::new(None::<String>);
    let set = |position: usize, run_id: i64, state: RunState, error: Option<String>, exported| {
        if let Err(e) = core.store.set_run_status(run_id, state, error.as_deref(), unix_now()) {
            *persistence_error.lock().unwrap() = Some(text(e));
            cancel.cancel();
            return;
        }
        let serial = {
            let mut report = report.lock().unwrap();
            let run = &mut report[position];
            (run.status, run.error, run.exported) = (state, error.clone(), exported);
            run.serial_number
        };
        progress(Progress::Run { serial, status: state, error });
    };
    futures_util::stream::iter(runs.iter().copied().enumerate()).for_each_concurrent(threads, |(position, (item, run_id))| {
        let (pool, report, set, wide, table, info, progress, cancel, drain) =
            (&pool, &report, &set, &wide, &table, info.clone(), progress.clone(), cancel.clone(), drain.clone());
        async move {
            if cancel.is_cancelled() || drain.is_cancelled() {
                set(position, run_id, RunState::Cancelled, None, Default::default());
                return;
            }
            let take_free = || {
                let mut pool = pool.lock().unwrap();
                // Если процесс оставляет браузер открытым, переиспользуем его вместо накопления новых
                // браузеров до лимита. Занятый другой автоматизацией профиль не задерживает свободные.
                for reuse in [true, false] {
                    for _ in 0..pool.len() {
                        let (id, serial) = pool.pop_front().unwrap();
                        let running = matches!(core.supervisor.status(&id), crate::supervisor::ProfileStatus::Running(_));
                        if running == reuse && let Some(busy) = core.flows.claim(&id) { return Some((id, serial, busy)); }
                        pool.push_back((id, serial));
                    }
                }
                None
            };
            let lease = match take_free() {
                Some(lease) => Some(lease),
                None => {
                    set(position, run_id, RunState::Waiting, None, Default::default());
                    tokio::select! {
                        lease = core.flows.claim_with(&cancel, take_free) => lease,
                        _ = drain.cancelled() => None,
                    }
                }
            };
            let Some((id, serial, busy)) = lease else {
                set(position, run_id, RunState::Cancelled, None, Default::default());
                return;
            };
            let result = async {
                if cancel.is_cancelled() || drain.is_cancelled() {
                    set(position, run_id, RunState::Cancelled, None, Default::default());
                    return Ok::<(), String>(());
                }
                core.store.assign_queue_run(run_id, &id, serial).map_err(text)?;
                {
                    let mut report = report.lock().unwrap();
                    report[position].acc_id = id.clone();
                    report[position].serial_number = serial;
                }
                if cancel.is_cancelled() || drain.is_cancelled() {
                    set(position, run_id, RunState::Cancelled, None, Default::default());
                    return Ok(());
                }
                set(position, run_id, RunState::Running, None, Default::default());
                if cancel.is_cancelled() {
                    set(position, run_id, RunState::Cancelled, None, Default::default());
                    return Ok(());
                }
                let values = params::layer(&task.params, table.as_ref().map(|t| &t.rows()[item]));
                let on_entry: journal::Sink = Arc::new(move |entry: &Entry| progress(Progress::Entry { serial, entry: entry.clone() }));
                let outcome = run_once(core, RunRequest {
                    flow: wide, profile_id: &id, params: values, run_id: run_id.to_string(), cancel,
                    position: item, task: Some(info), busy: Some(busy), on_entry: Some(on_entry), from: None, on_node: None,
                }).await;
                let state = match outcome.status { RunStatus::Done => RunState::Done, RunStatus::Error => RunState::Error, RunStatus::Cancelled => RunState::Cancelled };
                set(position, run_id, state, outcome.error, outcome.exported);
                Ok(())
            }.await;
            if let Err(error) = result { set(position, run_id, RunState::Error, Some(error), Default::default()); }
            pool.lock().unwrap().push_back((id, serial));
        }
    }).await;
    if let Some(error) = persistence_error.into_inner().unwrap() {
        let _ = core.store.abort_open_batches(task.id, std::process::id(), &error, unix_now());
        return Err(error);
    }
    core.store.finish_batch(batch_id, unix_now()).map_err(text)?;
    Ok(BatchReport { task_id: task.id, batch_id: Some(batch_id), files_dir: Some(files.display().to_string()), runs: report.into_inner().unwrap() })
}

fn text(error: impl std::fmt::Display) -> String { error.to_string() }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{create_with_proxy, env};
    use crate::flow::params::Distribution;
    use crate::flow::tasks::{FlowSource, ProfilePick, create_task};
    use crate::flow::tasks::tests::{exporting, flow, row, spec};
    use serde_json::json;
    use std::collections::HashSet;

    fn sink() -> ProgressSink { Arc::new(|_| {}) }

    #[tokio::test]
    async fn queue_uses_a_free_profile_instead_of_waiting_for_the_first_busy_one() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        let _occupied = e.core.flows.claim(&a.id).unwrap();
        let mut s = spec(exporting(json!([]), json!([])), ProfilePick::Ids(vec![a.id.clone(), b.id.clone()]));
        s.distribution = Distribution::Queue;
        let id = create_task(&e.core, &s, 1).unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2),
            tasks::run_batch(&e.core, id, Which::All, CancellationToken::new(), sink())).await.expect("свободный профиль должен получить задание сразу").unwrap();
        assert_eq!(result.runs[0].acc_id, b.id);
        assert_eq!(result.runs[0].status, RunState::Done);
    }

    #[tokio::test]
    async fn queue_consumes_all_rows_once_and_resume_does_not_repeat_successes() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        let mut s = spec(exporting(json!(["url"]), json!([])), ProfilePick::Ids(vec![a.id.clone(), b.id.clone()]));
        s.distribution = Distribution::Queue;
        s.threads = 2;
        let mut rows: Vec<_> = (0..7).map(|i| row(json!({ "url": format!("https://example.test/{i}") }))).collect();
        rows.push(row(json!({ "url": "https://example.test/0", "extra": "не выдавать тот же URL повторно" })));
        s.data = Some(rows);
        let id = create_task(&e.core, &s, 1).unwrap();
        assert_eq!(e.core.store.get_task(id).unwrap().unwrap().data.unwrap().len(), 7);
        let result = tasks::run_batch(&e.core, id, Which::All, CancellationToken::new(), sink()).await.unwrap();
        assert_eq!(result.runs.len(), 7, "строк больше, чем профилей, но очередь исчерпывается");
        assert!(result.runs.iter().all(|r| r.status == RunState::Done));
        assert_eq!(result.runs.iter().map(|r| r.exported["url"].as_str().unwrap()).collect::<HashSet<_>>().len(), 7);
        assert_eq!(result.runs.iter().map(|r| r.acc_id.as_str()).collect::<HashSet<_>>().len(), 2);
        let progress = e.core.store.task_rows().unwrap().remove(0).queue.unwrap();
        assert_eq!((progress.done, progress.total), (7, 7));
        for which in [Which::All, Which::Failed] {
            let resumed = tasks::run_batch(&e.core, id, which, CancellationToken::new(), sink()).await.unwrap();
            assert!(resumed.batch_id.is_none() && resumed.runs.is_empty());
        }
        assert_eq!(e.core.store.batch_summaries(id).unwrap().len(), 1);
        let runs = e.core.store.run_rows(result.batch_id.unwrap()).unwrap();
        assert_eq!(runs.iter().map(|r| r.item_index.unwrap()).collect::<HashSet<_>>().len(), 7);
    }

    #[tokio::test]
    async fn one_parameter_set_without_a_table_is_one_job() {
        let e = env().await;
        let ids: Vec<_> = (1..=3).map(|i| create_with_proxy(&e.core, "P", e.proxy_port, i).id).collect();
        let mut s = spec(exporting(json!(["url"]), json!([{ "name": "url", "type": "string", "default": "https://example.test/one" }])), ProfilePick::Ids(ids));
        s.distribution = Distribution::Queue;
        s.threads = 3;
        let id = create_task(&e.core, &s, 1).unwrap();
        let result = tasks::run_batch(&e.core, id, Which::All, CancellationToken::new(), sink()).await.unwrap();
        assert_eq!(result.runs.len(), 1);
        assert_eq!(result.runs[0].status, RunState::Done);
        assert_eq!(result.runs[0].exported["url"], json!("https://example.test/one"));
    }

    #[tokio::test]
    async fn unrestricted_queue_keeps_browsers_open_with_the_chosen_parallelism() {
        let e = env().await;
        let ids: Vec<_> = (1..=3).map(|i| create_with_proxy(&e.core, "P", e.proxy_port, i).id).collect();
        e.core.store.set_setting("profiles.max_running", "1").unwrap();
        let mut f = exporting(json!(["url"]), json!([]));
        f.settings.after_browser = crate::flow::model::AfterBrowser::Keep;
        let mut s = spec(f, ProfilePick::Ids(ids));
        s.distribution = Distribution::Queue;
        s.threads = 3;
        s.data = Some((0..4).map(|i| row(json!({ "url": format!("item-{i}") }))).collect());
        let id = create_task(&e.core, &s, 1).unwrap();
        let result = tasks::run_batch(&e.core, id, Which::All, CancellationToken::new(), sink()).await.unwrap();
        e.core.supervisor.stop_all().await;
        assert!(result.runs.iter().all(|r| r.status == RunState::Done), "{:?}", result.runs);
        assert_eq!(result.runs.iter().map(|r| &r.acc_id).collect::<HashSet<_>>().len(), 3);
    }

    #[tokio::test]
    async fn retry_follows_failed_items_instead_of_repeating_their_profiles_successes() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let broken = flow(json!({
            "variables": [{ "name": "d", "type": "object", "default": {} }],
            "nodes": [{ "id": "s", "op": "start" }, { "id": "f", "op": "proc.field", "config": { "data": "${d}", "key": "name", "save_to": "out" } }],
            "edges": [{ "from": "s", "to": "f" }]
        }));
        let db = e.core.store.create_flow(&broken, 1).unwrap().id;
        let mut s = spec(broken, ProfilePick::Ids(vec![a.id.clone()]));
        s.flow = FlowSource::Db(db);
        s.distribution = Distribution::Queue;
        s.data = Some(vec![row(json!({ "url": "a", "d": { "name": "ok" } })), row(json!({ "url": "b", "d": {} })), row(json!({ "url": "c", "d": { "name": "ok" } }))]);
        let id = create_task(&e.core, &s, 1).unwrap();
        let first = tasks::run_batch(&e.core, id, Which::All, CancellationToken::new(), sink()).await.unwrap();
        assert_eq!(first.runs.iter().map(|r| r.status).collect::<Vec<_>>(), [RunState::Done, RunState::Error, RunState::Done]);
        let fixed = exporting(json!(["url"]), json!([{ "name": "d", "type": "object", "default": {} }]));
        e.core.store.update_flow(db, &fixed, 2).unwrap();
        let retry = tasks::run_batch(&e.core, id, Which::Failed, CancellationToken::new(), sink()).await.unwrap();
        assert_eq!(retry.runs.len(), 1);
        assert_eq!((retry.runs[0].item_index, retry.runs[0].status), (Some(1), RunState::Done));
        assert_eq!(retry.runs[0].exported["url"], json!("b"));
        assert!(tasks::run_batch(&e.core, id, Which::All, CancellationToken::new(), sink()).await.unwrap().runs.is_empty());
        assert_eq!(e.core.store.task_rows().unwrap()[0].queue.as_ref().unwrap().done, 3);
    }

    #[tokio::test]
    async fn draining_keeps_running_items_and_resume_takes_only_the_rest() {
        let e = env().await;
        let ids: Vec<_> = (1..=2).map(|i| create_with_proxy(&e.core, "P", e.proxy_port, i).id).collect();
        let f = flow(json!({ "nodes": [{ "id": "s", "op": "start" }, { "id": "w", "op": "wait.time", "config": { "ms": 30 } }], "edges": [{ "from": "s", "to": "w" }] }));
        let mut s = spec(f, ProfilePick::Ids(ids));
        s.distribution = Distribution::Queue;
        s.threads = 2;
        s.data = Some((0..5).map(|i| row(json!({ "url": format!("item-{i}") }))).collect());
        let id = create_task(&e.core, &s, 1).unwrap();
        let drain = CancellationToken::new();
        let stop = drain.clone();
        let progress: ProgressSink = Arc::new(move |event| {
            if matches!(event, Progress::Run { status: RunState::Done, .. }) { stop.cancel(); }
        });
        let first = tasks::run_batch_with(&e.core, id, Which::All, CancellationToken::new(), drain, progress).await.unwrap();
        let done: HashSet<_> = first.runs.iter().filter(|r| r.status == RunState::Done).map(|r| r.item_index.unwrap()).collect();
        assert!(!done.is_empty() && done.len() < 5);
        assert!(first.runs.iter().filter(|r| r.status != RunState::Done).all(|r| r.status == RunState::Cancelled));
        let resumed = tasks::run_batch(&e.core, id, Which::All, CancellationToken::new(), sink()).await.unwrap();
        assert_eq!(resumed.runs.len(), 5 - done.len());
        assert!(resumed.runs.iter().all(|r| r.status == RunState::Done && !done.contains(&r.item_index.unwrap())));
        assert_eq!(e.core.store.task_rows().unwrap()[0].queue.as_ref().unwrap().done, 5);
    }

    #[tokio::test]
    async fn unrestricted_queue_runs_exclusive_profile_leases_without_the_old_limit() {
        let e = env().await;
        let ids: Vec<_> = (1..=3).map(|i| create_with_proxy(&e.core, "P", e.proxy_port, i).id).collect();
        e.core.store.set_setting("profiles.max_running", "2").unwrap();
        let f = flow(json!({ "nodes": [{ "id": "s", "op": "start" }, { "id": "w", "op": "wait.time", "config": { "ms": 25 } }], "edges": [{ "from": "s", "to": "w" }] }));
        let mut s = spec(f, ProfilePick::Ids(ids));
        s.distribution = Distribution::Queue;
        s.threads = 101;
        s.data = Some((0..9).map(|i| row(json!({ "url": format!("item-{i}") }))).collect());
        let id = create_task(&e.core, &s, 1).unwrap();
        let active = Arc::new(Mutex::new((HashSet::new(), 0usize)));
        let seen = active.clone();
        let progress: ProgressSink = Arc::new(move |event| {
            if let Progress::Run { serial, status, .. } = event {
                let mut active = seen.lock().unwrap();
                if status == RunState::Running {
                    assert!(active.0.insert(serial), "профиль уже обрабатывает задание");
                    active.1 = active.1.max(active.0.len());
                } else if matches!(status, RunState::Done | RunState::Error | RunState::Cancelled) {
                    active.0.remove(&serial);
                }
            }
        });
        let result = tasks::run_batch(&e.core, id, Which::All, CancellationToken::new(), progress).await.unwrap();
        assert!(result.runs.iter().all(|r| r.status == RunState::Done));
        assert_eq!(active.lock().unwrap().1, 3);
        assert_eq!(result.runs.iter().map(|r| r.acc_id.as_str()).collect::<HashSet<_>>().len(), 3, "профили возвращаются в общий пул");
    }
}
