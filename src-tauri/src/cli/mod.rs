//! `ombra flow …` — командная строка автоматизации (спецификация А2, «CLI»): своё ядро без окна, рядом с окном Ombra.
pub mod args;

use crate::app::{self, Core};
use crate::engine::Engine;
use crate::flow::bundle;
use crate::flow::journal::Status;
use crate::flow::model::{self, Flow};
use crate::flow::params::{self, Row};
use crate::flow::run::lookup;
use crate::flow::tasks::{self, BatchReport, FlowSource, Progress, ProgressSink, TaskSpec, Which};
use crate::flow::validate::validate;
use crate::paths::Paths;
use crate::store::Store;
use crate::store::tasks::RunState;
use crate::supervisor::Supervisor;
use crate::sys::unix_now;
use args::{Command, RunArgs};
use serde::Serialize;
use serde_json::Value;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_UNAVAILABLE: i32 = 3;
pub const EXIT_SIGNAL: i32 = 130;

/// Куда идёт ход задачи и ошибки — stderr; общий для запусков пакета.
pub type Output = Arc<Mutex<dyn Write + Send>>;

/// Точка входа из `main.rs`: `args` — всё после `flow`. Ядро — своё, без окна и Local API, рядом с окном Ombra.
pub fn main(args: Vec<String>) -> i32 {
    let command = match args::parse(&args) {
        Ok(command) => command,
        Err(e) => {
            eprintln!("{e}\nСправка: ombra flow --help");
            return EXIT_USAGE;
        }
    };
    if let Command::Help(topic) = &command {
        println!("{}", args::help(topic.as_deref()));
        return EXIT_OK;
    }
    let _ = crate::sys::raise_fd_limit(crate::FD_LIMIT);
    let paths = Paths::new(Paths::default_root());
    // Защищённый CLI подтверждает свой сеанс; обычный режим работает и рядом с окном.
    let (paths, _instance) = match crate::security::unlock_terminal(paths) {
        Ok(paths) => paths,
        Err(error) => { eprintln!("Ombra недоступна: {}", error.message); return EXIT_UNAVAILABLE; }
    };
    log_to_file(&paths.logs().join("ombra-cli.log"));
    let core = match app::open_default(paths, Arc::new(|_| {})) {
        Ok(core) => core,
        Err(e) => {
            eprintln!("Ombra недоступна: {}", e.message);
            return EXIT_UNAVAILABLE;
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("Ombra недоступна: {e}");
            return EXIT_UNAVAILABLE;
        }
    };
    runtime.block_on(async {
        // Уборка с проверкой блокировок не трогает браузеры окна Ombra и других команд; без неё запуски ждали бы её.
        if let Err(e) = core.supervisor.cleanup_orphans().await {
            log::warn!("уборка после прошлого запуска: {e}");
        }
        let cancel = CancellationToken::new();
        let watcher = tokio::spawn(signals(core.supervisor.clone(), cancel.clone()));
        let err: Output = Arc::new(Mutex::new(std::io::stderr()));
        let code = run(&core, command, &mut std::io::stdout(), err, cancel).await;
        watcher.abort();
        // Свои браузеры не переживают команду: `after_browser: keep` в CLI — до конца команды.
        core.supervisor.stop_all().await;
        code
    })
}

/// Команда на открытом ядре: вывод — в `out`, ход и ошибки — в `err`; `cancel` — «Стоп».
pub async fn run<E: Engine>(core: &Core<E>, command: Command, out: &mut dyn Write, err: Output, cancel: CancellationToken) -> i32 {
    match command {
        Command::Help(topic) => {
            let _ = writeln!(out, "{}", args::help(topic.as_deref()));
            EXIT_OK
        }
        Command::List { json } => list(core, out, &err, json),
        Command::Validate { file, json } => validate_file(core, &file, out, &err, json),
        Command::Import { file } => import(core, &file, out, &err).await,
        Command::Export { id, file } => export(core, id, file.as_deref(), out, &err),
        Command::Run(run_args) => run_task(core, run_args, out, err, cancel).await,
        Command::Retry { task_id, failed, json } => {
            let which = if failed { Which::Failed } else { Which::All };
            batch(core, task_id, which, json, out, err, cancel).await
        }
    }
}

async fn run_task<E: Engine>(core: &Core<E>, a: RunArgs, out: &mut dyn Write, err: Output, cancel: CancellationToken) -> i32 {
    if core.engine.installed().is_none() {
        return fail(&err, EXIT_UNAVAILABLE, "Браузер не установлен. Откройте Ombra → Настройки → Браузер.");
    }
    let flow = match resolve_flow(&core.store, &a.flow) {
        Ok(flow) => flow,
        Err(e) => return fail(&err, EXIT_USAGE, &e),
    };
    let data = match &a.data {
        Some(path) => match params::read_table(path) {
            Ok(rows) => Some(rows),
            Err(e) => return fail(&err, EXIT_USAGE, &e),
        },
        None => None,
    };
    let params: Row = a.set.into_iter().map(|(name, value)| (name, Value::String(value))).collect();
    let spec = TaskSpec {
        headless: None,
        distribution: a.distribution,
        name: a.name,
        flow,
        profiles: a.profiles,
        params,
        data,
        threads: a.threads,
        order: a.order,
        missing_row: a.missing_row,
    };
    let task_id = match tasks::create_task(core, &spec, unix_now()) {
        Ok(id) => id,
        Err(e) => return fail(&err, EXIT_USAGE, &e),
    };
    batch(core, task_id, Which::All, a.json, out, err, cancel).await
}

/// Пакет задачи: ход — в `err`, итог — строкой в `err` и (`json`) объектом в `out`.
async fn batch<E: Engine>(
    core: &Core<E>,
    task_id: i64,
    which: Which,
    json: bool,
    out: &mut dyn Write,
    err: Output,
    cancel: CancellationToken,
) -> i32 {
    if core.engine.installed().is_none() {
        return fail(&err, EXIT_UNAVAILABLE, "Браузер не установлен. Откройте Ombra → Настройки → Браузер.");
    }
    let report = match tasks::run_batch(core, task_id, which, cancel.clone(), printer(err.clone())).await {
        Ok(report) => report,
        Err(e) => return fail(&err, EXIT_USAGE, &e),
    };
    let line = if report.batch_id.is_none() { "Итог: нечего повторять".to_string() } else { summary(&report) };
    let _ = writeln!(lock(&err), "{line}");
    if json {
        print_json(out, &report);
    }
    if cancel.is_cancelled() {
        EXIT_SIGNAL
    } else if report.runs.iter().all(|run| run.status == RunState::Done) {
        EXIT_OK
    } else {
        EXIT_FAILED
    }
}

/// Процесс в `run`: существующий путь — файл; иначе число — id в базе; иначе имя в базе.
fn resolve_flow(store: &Store, text: &str) -> Result<FlowSource, String> {
    let path = Path::new(text);
    if path.exists() {
        return read_flow(path).map(|flow| FlowSource::File(Box::new(flow)));
    }
    if let Ok(id) = text.trim().parse::<i64>() {
        return match store.get_flow(id).map_err(|e| e.to_string())? {
            Some(_) => Ok(FlowSource::Db(id)),
            None => Err(format!("Сценарий {id} не найден")),
        };
    }
    match store.flows_named(text).map_err(|e| e.to_string())?.as_slice() {
        [] => Err(format!("Файл или сценарий «{text}» не найден")),
        [id] => Ok(FlowSource::Db(*id)),
        ids => Err(format!(
            "Найдено несколько сценариев с названием «{text}»: {}. Укажите идентификатор.",
            ids.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")
        )),
    }
}

fn read_flow(path: &Path) -> Result<Flow, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("файл {} не прочитан: {e}", path.display()))?;
    model::parse(&text)
}

#[derive(Serialize)]
struct FlowRow {
    id: i64,
    name: String,
    group: String,
    created_at: i64,
    updated_at: i64,
    has_errors: bool,
}

fn list<E: Engine>(core: &Core<E>, out: &mut dyn Write, err: &Output, json: bool) -> i32 {
    let records = match core.store.list_flows() {
        Ok(records) => records,
        Err(e) => return fail(err, EXIT_USAGE, &format!("Не удалось прочитать список сценариев: {e}")),
    };
    let rows: Vec<FlowRow> = records
        .into_iter()
        .map(|record| FlowRow {
            has_errors: !validate(&record.flow, &*lookup(core, &record.flow)).is_ok(),
            id: record.id,
            name: record.flow.name,
            group: record.flow.group,
            created_at: record.created_at,
            updated_at: record.updated_at,
        })
        .collect();
    if json {
        print_json(out, &rows);
    } else {
        for row in &rows {
            let mark = if row.has_errors { "\tесть ошибки" } else { "" };
            let _ = writeln!(out, "{}\t{}\t{}{mark}", row.id, row.name, row.group);
        }
    }
    EXIT_OK
}

fn validate_file<E: Engine>(core: &Core<E>, file: &Path, out: &mut dyn Write, err: &Output, json: bool) -> i32 {
    let flow = match read_flow(file) {
        Ok(flow) => flow,
        Err(e) => return fail(err, EXIT_USAGE, &e),
    };
    let report = validate(&flow, &*lookup(core, &flow));
    if json {
        print_json(out, &report);
    } else {
        for issue in &report.errors {
            let _ = writeln!(out, "ошибка: {issue}");
        }
        for issue in &report.warnings {
            let _ = writeln!(out, "предупреждение: {issue}");
        }
        if report.errors.is_empty() && report.warnings.is_empty() {
            let _ = writeln!(out, "ошибок нет");
        }
    }
    if report.is_ok() { EXIT_OK } else { EXIT_FAILED }
}

async fn import<E: Engine>(core: &Core<E>, file: &Path, out: &mut dyn Write, err: &Output) -> i32 {
    match core.import_flow_file(file, unix_now()).await {
        Ok(row) => {
            let _ = writeln!(out, "{}", row.id);
            EXIT_OK
        }
        Err(e) => fail(err, EXIT_USAGE, &format!("Не удалось импортировать сценарий: {}", e.message)),
    }
}

fn export<E: Engine>(core: &Core<E>, id: i64, file: Option<&Path>, out: &mut dyn Write, err: &Output) -> i32 {
    let flow = match bundle::export(&core.store, id) {
        Ok(flow) => flow,
        Err(e) => return fail(err, EXIT_USAGE, &format!("Не удалось экспортировать сценарий {id}: {e}")),
    };
    let text = serde_json::to_string_pretty(&flow).expect("процесс — JSON");
    let Some(path) = file else {
        let _ = writeln!(out, "{text}");
        return EXIT_OK;
    };
    let written = std::fs::OpenOptions::new().write(true).create_new(true).open(path).and_then(|mut f| f.write_all(text.as_bytes()));
    match written {
        Ok(()) => EXIT_OK,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => fail(err, EXIT_USAGE, &format!("файл {} уже есть", path.display())),
        Err(e) => fail(err, EXIT_USAGE, &format!("файл {} не записан: {e}", path.display())),
    }
}

/// Ход пакета строками в stderr: статусы запусков, строки узлов (как в журнале — правило секретов то же), заметки.
fn printer(err: Output) -> ProgressSink {
    Arc::new(move |event: Progress| {
        let line = match event {
            Progress::Run { serial, status, error: Some(error) } => format!("#{serial} {}: {error}", status.label()),
            Progress::Run { serial, status, error: None } => format!("#{serial} {}", status.label()),
            // Начало и конец запуска уже видны по его статусам.
            Progress::Entry { entry, .. } if matches!(entry.status, Status::Start | Status::End) => return,
            Progress::Entry { serial, entry } => {
                let mut line = format!("#{serial} {} {} {} {} мс", entry.node, entry.op, status_word(entry.status), entry.ms);
                if !entry.msg.is_empty() {
                    line.push_str("  ");
                    line.push_str(&entry.msg);
                }
                line
            }
            Progress::Note(note) => note,
        };
        let _ = writeln!(lock(&err), "{line}");
    })
}

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Start => "start",
        Status::End => "end",
        Status::Ok => "ok",
        Status::Skip => "skip",
        Status::Error => "error",
        Status::Branch => "branch",
        Status::Disabled => "disabled",
    }
}

/// `Итог: готово 18, ошибка 1, пропущен 1 · файлы: <папка>`.
fn summary(report: &BatchReport) -> String {
    let count = |state: RunState| report.runs.iter().filter(|run| run.status == state).count();
    let parts: Vec<String> = [RunState::Done, RunState::Error, RunState::Cancelled, RunState::Skipped]
        .into_iter()
        .filter(|state| count(*state) > 0)
        .map(|state| format!("{} {}", state.label(), count(state)))
        .collect();
    let files = report.files_dir.as_deref().map(|dir| format!(" · файлы: {dir}")).unwrap_or_default();
    format!("Итог: {}{files}", parts.join(", "))
}

#[cfg(windows)]
async fn signals<E: Engine>(supervisor: Supervisor<E>, cancel: CancellationToken) {
    use tokio::signal::windows::{ctrl_c, ctrl_break, ctrl_close, ctrl_shutdown};
    let (Ok(mut interrupt), Ok(mut brk), Ok(mut close), Ok(mut shutdown)) =
        (ctrl_c(), ctrl_break(), ctrl_close(), ctrl_shutdown()) else { return };
    for second in [false, true] {
        tokio::select! {
            _ = interrupt.recv() => {},
            _ = brk.recv() => {},
            _ = close.recv() => {},
            _ = shutdown.recv() => {},
        }
        if second { supervisor.stop_all().await; } else { cancel.cancel(); }
    }
}

fn print_json(out: &mut dyn Write, value: &impl Serialize) {
    let _ = serde_json::to_writer_pretty(&mut *out, value);
    let _ = writeln!(out);
}

fn fail(err: &Output, code: i32, message: &str) -> i32 {
    let _ = writeln!(lock(err), "{message}");
    code
}

fn lock(err: &Output) -> std::sync::MutexGuard<'_, dyn Write + Send + 'static> {
    err.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Первый SIGINT, SIGTERM или SIGHUP — «Стоп» пакета; второй — свои браузеры гасятся сразу.
/// Под `nohup` (SIGHUP игнорируется с запуска) SIGHUP не перехватывается: закрытие терминала пакет не останавливает.
#[cfg(unix)]
async fn signals<E: Engine>(supervisor: Supervisor<E>, cancel: CancellationToken) {
    use tokio::signal::unix::{SignalKind, signal};
    // Проверка — до `signal()`: обработчик tokio заменил бы `SIG_IGN`, поставленный `nohup`.
    let hangup = (!crate::sys::signal_ignored(libc::SIGHUP)).then(|| signal(SignalKind::hangup()));
    let (Ok(mut interrupt), Ok(mut terminate), Ok(mut hangup)) =
        (signal(SignalKind::interrupt()), signal(SignalKind::terminate()), hangup.transpose())
    else {
        log::warn!("сигналы не перехватываются");
        return;
    };
    for second in [false, true] {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
            _ = async {
                match hangup.as_mut() {
                    Some(hangup) => hangup.recv().await,
                    None => std::future::pending().await,
                }
            } => {}
        }
        if second {
            // Сначала дело, потом вывод: после SIGHUP терминала нет, а запись в stderr не должна ничего ронять.
            supervisor.stop_all().await;
            let _ = writeln!(std::io::stderr(), "Браузеры останавливаются без действий «после задачи».");
        } else {
            cancel.cancel();
            let _ = writeln!(std::io::stderr(), "Остановка: текущие запуски выполнят действия «После задачи». Повторите команду остановки, чтобы немедленно закрыть браузеры.");
        }
    }
}

/// Журнал ядра CLI — в файл, как у окна Ombra; stderr остаётся для хода задачи.
struct FileLog(Mutex<std::fs::File>);

impl log::Log for FileLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        // На подробных уровнях tungstenite пишет сообщения WebSocket целиком, а в ответах CDP — значения cookies.
        metadata.level() <= log::Level::Info && !metadata.target().contains("tungstenite")
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let mut file = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = writeln!(file, "[{}][{}][{}] {}", unix_now(), record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

fn log_to_file(path: &Path) {
    let opened = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(path));
    let Ok(file) = opened else { return };
    if log::set_logger(Box::leak(Box::new(FileLog(Mutex::new(file))))).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{create_with_proxy, env};
    use crate::supervisor::test_engine::FakeEngine;
    use serde_json::{Value, json};
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    async fn call(core: &Core<FakeEngine>, args: &[&str], cancel: CancellationToken) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let command = args::parse(&args).unwrap();
        let mut out = Vec::new();
        let err: Arc<Mutex<Vec<u8>>> = Arc::default();
        let code = run(core, command, &mut out, err.clone(), cancel).await;
        let err = String::from_utf8(err.lock().unwrap().clone()).unwrap();
        (code, String::from_utf8(out).unwrap(), err)
    }

    fn file(dir: &Path, name: &str, text: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path.display().to_string()
    }

    fn flow_text(variables: Value, nodes: Value) -> String {
        let mut edges = Vec::new();
        let ids: Vec<String> = nodes.as_array().unwrap().iter().map(|node| node["id"].as_str().unwrap().to_string()).collect();
        for pair in ids.windows(2) {
            edges.push(json!({ "from": pair[0], "to": pair[1] }));
        }
        json!({ "format": "ombra-flow/1", "name": "Вход", "variables": variables, "nodes": nodes, "edges": edges }).to_string()
    }

    fn export_login() -> String {
        flow_text(json!([]), json!([{ "id": "s", "op": "start" }, { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["login"] } }]))
    }

    fn picky() -> String {
        flow_text(
            json!([{ "name": "d", "type": "object", "default": {} }]),
            json!([{ "id": "s", "op": "start" }, { "id": "f", "op": "proc.field", "config": { "data": "${d}", "key": "name", "save_to": "out" } }]),
        )
    }

    #[tokio::test]
    async fn run_prints_progress_and_json_and_exits_0() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        let dir = tempfile::tempdir().unwrap();
        let flow = file(dir.path(), "f.json", &export_login());
        let data = file(dir.path(), "rows.csv", "login\nu1\nu2\n");
        let serials = format!("{},{}", a.serial, b.serial);
        let (code, out, err) = call(&e.core, &["run", &flow, "--serial", &serials, "--data", &data, "--json"], CancellationToken::new()).await;
        assert_eq!(code, EXIT_OK, "{err}");
        let report: Value = serde_json::from_str(&out).unwrap();
        let runs: Vec<(Value, Value)> =
            report["runs"].as_array().unwrap().iter().map(|run| (run["status"].clone(), run["exported"]["login"].clone())).collect();
        assert_eq!(runs, [(json!("done"), json!("u1")), (json!("done"), json!("u2"))]);
        assert!(report["task_id"].is_i64() && report["batch_id"].is_i64() && report["files_dir"].is_string());
        assert!(err.contains(&format!("#{} выполняется", a.serial)) && err.contains(&format!("#{} готово", b.serial)), "{err}");
        assert!(err.contains(" proc.vars ok "), "строки узлов: {err}");
        assert!(err.contains("Итог: готово 2"), "{err}");
    }

    #[tokio::test]
    async fn failed_runs_exit_1() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        let dir = tempfile::tempdir().unwrap();
        let flow = file(dir.path(), "f.json", &picky());
        let data = file(dir.path(), "rows.json", r#"[{"d":{"name":"x"}},{"d":{}}]"#);
        let serials = format!("{},{}", a.serial, b.serial);
        let (code, out, err) = call(&e.core, &["run", &flow, "--serial", &serials, "--data", &data], CancellationToken::new()).await;
        assert_eq!(code, EXIT_FAILED, "{err}");
        assert!(out.is_empty());
        assert!(err.contains(&format!("#{} ошибка: узел f", b.serial)), "{err}");
        assert!(err.contains("Итог: готово 1, ошибка 1"), "{err}");
    }

    #[tokio::test]
    async fn bad_input_exits_2_with_empty_stdout() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let dir = tempfile::tempdir().unwrap();
        let flow = file(dir.path(), "f.json", &picky());
        let serial = a.serial.to_string();
        let twin = crate::flow::model::parse(&picky()).unwrap();
        let mut twin = twin;
        twin.name = "Двойник".into();
        let first = e.core.store.create_flow(&twin, 1).unwrap().id;
        let second = e.core.store.create_flow(&twin, 1).unwrap().id;
        let bad_table = file(dir.path(), "rows.txt", "d\n{}\n");
        let cases: Vec<(Vec<&str>, String)> = vec![
            (vec!["run", flow.as_str(), "--serial", "999", "--json"], "999".into()),
            (vec!["run", flow.as_str(), "--serial", serial.as_str(), "--set", "nope=1", "--json"], "nope".into()),
            (vec!["run", "Нет такого", "--serial", serial.as_str(), "--json"], "Файл или сценарий «Нет такого» не найден".into()),
            (vec!["run", "Двойник", "--serial", serial.as_str(), "--json"], format!("{first}, {second}")),
            (vec!["run", flow.as_str(), "--serial", serial.as_str(), "--data", bad_table.as_str(), "--json"], ".csv, .xlsx или .json".into()),
            (vec!["retry", "999", "--json"], "нет задачи 999".into()),
        ];
        for (args, expected) in cases {
            let (code, out, err) = call(&e.core, &args, CancellationToken::new()).await;
            assert_eq!(code, EXIT_USAGE, "{args:?}: {err}");
            assert!(out.is_empty(), "{args:?}: stdout {out}");
            assert!(err.contains(&expected), "{args:?}: {err}");
        }
    }

    #[tokio::test]
    async fn missing_engine_exits_3() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let dir = tempfile::tempdir().unwrap();
        let flow = file(dir.path(), "f.json", &picky());
        e.core.engine.installed.store(false, Ordering::SeqCst);
        let (code, _, err) = call(&e.core, &["run", &flow, "--serial", &a.serial.to_string()], CancellationToken::new()).await;
        assert_eq!(code, EXIT_UNAVAILABLE);
        assert!(err.contains("Браузер не установлен"), "{err}");
    }

    #[tokio::test]
    async fn signal_stop_exits_130() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let dir = tempfile::tempdir().unwrap();
        let slow = flow_text(json!([]), json!([{ "id": "s", "op": "start" }, { "id": "w", "op": "wait.time", "config": { "ms": 5000 } }]));
        let flow = file(dir.path(), "f.json", &slow);
        let cancel = CancellationToken::new();
        let stopper = {
            let cancel = cancel.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                cancel.cancel();
            }
        };
        let serial = a.serial.to_string();
        let args = ["run", flow.as_str(), "--serial", serial.as_str()];
        let ((code, _, err), ()) = tokio::join!(call(&e.core, &args, cancel), stopper);
        assert_eq!(code, EXIT_SIGNAL, "{err}");
        assert!(err.contains("отменён"), "{err}");
    }

    #[tokio::test]
    async fn retry_failed_and_nothing_to_retry() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let b = create_with_proxy(&e.core, "B", e.proxy_port, 2);
        let dir = tempfile::tempdir().unwrap();
        let flow = file(dir.path(), "f.json", &picky());
        let data = file(dir.path(), "rows.json", r#"[{"d":{"name":"x"}},{"d":{}}]"#);
        let serials = format!("{},{}", a.serial, b.serial);
        let (_, out, _) = call(&e.core, &["run", &flow, "--serial", &serials, "--data", &data, "--json"], CancellationToken::new()).await;
        let task = serde_json::from_str::<Value>(&out).unwrap()["task_id"].to_string();
        let (code, out, err) = call(&e.core, &["retry", &task, "--failed", "--json"], CancellationToken::new()).await;
        assert_eq!(code, EXIT_FAILED, "{err}");
        let report: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(report["runs"].as_array().unwrap().len(), 1);
        assert_eq!(report["runs"][0]["acc_id"], json!(b.id));
        let good = file(dir.path(), "good.json", r#"[{"d":{"name":"x"}}]"#);
        let (_, out, _) = call(&e.core, &["run", &flow, "--serial", &a.serial.to_string(), "--data", &good, "--json"], CancellationToken::new()).await;
        let task = serde_json::from_str::<Value>(&out).unwrap()["task_id"].to_string();
        let (code, out, err) = call(&e.core, &["retry", &task, "--failed", "--json"], CancellationToken::new()).await;
        assert_eq!(code, EXIT_OK, "{err}");
        assert!(err.contains("нечего повторять"), "{err}");
        assert_eq!(serde_json::from_str::<Value>(&out).unwrap()["batch_id"], Value::Null);
    }

    #[tokio::test]
    async fn list_validate_import_export() {
        let e = env().await;
        let dir = tempfile::tempdir().unwrap();
        let good = file(dir.path(), "good.json", &picky());
        let broken = file(
            dir.path(),
            "broken.json",
            &flow_text(json!([]), json!([{ "id": "s", "op": "start" }, { "id": "x", "op": "no.such" }])),
        );
        let (code, out, _) = call(&e.core, &["import", &good], CancellationToken::new()).await;
        assert_eq!(code, EXIT_OK);
        let id: i64 = out.trim().parse().unwrap();
        let (code, out, _) = call(&e.core, &["import", &broken], CancellationToken::new()).await;
        assert_eq!(code, EXIT_OK, "черновик с ошибками импортируется");
        let draft: i64 = out.trim().parse().unwrap();
        let (_, out, _) = call(&e.core, &["list", "--json"], CancellationToken::new()).await;
        let listed: Value = serde_json::from_str(&out).unwrap();
        let flags: Vec<(i64, bool)> =
            listed.as_array().unwrap().iter().map(|row| (row["id"].as_i64().unwrap(), row["has_errors"].as_bool().unwrap())).collect();
        assert_eq!(flags, [(id, false), (draft, true)]);
        let (code, out, _) = call(&e.core, &["validate", &broken, "--json"], CancellationToken::new()).await;
        assert_eq!(code, EXIT_FAILED);
        assert!(!serde_json::from_str::<Value>(&out).unwrap()["errors"].as_array().unwrap().is_empty());
        let (code, out, _) = call(&e.core, &["validate", &good], CancellationToken::new()).await;
        assert_eq!((code, out.trim()), (EXIT_OK, "ошибок нет"));
        let not_json = file(dir.path(), "bad.json", "{");
        assert_eq!(call(&e.core, &["validate", &not_json], CancellationToken::new()).await.0, EXIT_USAGE);
        let (code, out, _) = call(&e.core, &["export", &id.to_string()], CancellationToken::new()).await;
        assert_eq!(code, EXIT_OK);
        assert_eq!(serde_json::from_str::<Value>(&out).unwrap()["name"], json!("Вход"));
        let target = dir.path().join("out.json").display().to_string();
        assert_eq!(call(&e.core, &["export", &id.to_string(), &target], CancellationToken::new()).await.0, EXIT_OK);
        assert!(Path::new(&target).is_file());
        let (code, _, err) = call(&e.core, &["export", &id.to_string(), &target], CancellationToken::new()).await;
        assert_eq!(code, EXIT_USAGE);
        assert!(err.contains("уже есть"), "{err}");
        assert_eq!(call(&e.core, &["export", "999"], CancellationToken::new()).await.0, EXIT_USAGE);
    }

    #[tokio::test]
    async fn secrets_stay_out_of_stderr_and_journals() {
        let e = env().await;
        let a = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let dir = tempfile::tempdir().unwrap();
        let text = flow_text(
            json!([{ "name": "pass", "type": "string", "default": "" }]),
            json!([
                { "id": "s", "op": "start" },
                { "id": "v", "op": "proc.vars", "config": { "mode": "add", "vars": [
                    { "name": "copy", "type": "string", "value": "${pass}" },
                    { "name": "row", "type": "string", "value": "${token}" }
                ] } }
            ]),
        );
        let flow = file(dir.path(), "f.json", &text);
        let data = file(dir.path(), "rows.json", r#"[{"token":"row-secret-42"}]"#);
        let (code, _, err) =
            call(&e.core, &["run", &flow, "--serial", &a.serial.to_string(), "--set", "pass=s3cr3t-cli", "--data", &data], CancellationToken::new()).await;
        assert_eq!(code, EXIT_OK, "{err}");
        for secret in ["s3cr3t-cli", "row-secret-42"] {
            assert!(!err.contains(secret), "stderr: {err}");
            for entry in std::fs::read_dir(e.core.paths.logs().join("flows")).unwrap() {
                let journal = std::fs::read_to_string(entry.unwrap().path()).unwrap();
                assert!(!journal.contains(secret), "{journal}");
            }
        }
    }
}
