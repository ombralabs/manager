//! Один запуск процесса на профиле (спецификация автоматизации, «Запуск»): проверка, переменные, браузер профиля,
//! обход графа, действия «после задачи», журнал. В А1-1 его вызывают тесты; задачи, CLI и экраны — А2–А3.
use crate::app::Core;
use crate::engine::Engine;
use crate::engine::cdp::CdpError;
use crate::engine::page::Browser;
use crate::flow::interp::{Ctx, Exit, Fail, Lookup};
use crate::flow::journal::{self, Journal, Status};
use crate::flow::model::{AfterBrowser, AfterTabs, Flow};
use crate::flow::ops::profile::TAGS_LIMIT;
use crate::flow::ops::web::Tabs;
use crate::flow::validate::validate;
use crate::flow::vars::{self, Scope};
use crate::flow::{Busy, FlowState};
use crate::flow::host::{Host, Opened, ProfileDraft};
use crate::supervisor::{RunningInfo, Supervisor, SupervisorError};
use std::path::PathBuf;
use futures_util::future::BoxFuture;
use crate::store::Store;
use crate::store::profiles::ProfileEdit;
use crate::supervisor::Event;
use crate::supervisor::StartOptions;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Сколько ждать запуск профиля — как Local API.
const START_WAIT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Done,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunOutcome {
    pub status: RunStatus,
    pub error: Option<String>,
    /// Переменные, которые корневой процесс отдал Export Variable.
    pub exported: Map<String, Value>,
}

/// Профиль держит другой процесс Ombra: окно рядом с `ombra flow` или другая команда (спецификация А2).
const LOCKED: &str = "профиль открыт в другом процессе Ombra";

/// Запуск в задаче (А2): системные переменные `task_id`, `task_name` и папка файлов пакета.
#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub id: i64,
    pub name: String,
    pub files: PathBuf,
}

pub struct RunRequest<'a> {
    pub flow: &'a Flow,
    pub profile_id: &'a str,
    /// Значения переменных Start поверх умолчаний.
    pub params: Map<String, Value>,
    /// Имя файла журнала `logs/flows/<run_id>.jsonl`.
    pub run_id: String,
    pub cancel: CancellationToken,
    /// Номер профиля в задаче (с 0); вне задачи — 0.
    pub position: usize,
    /// Задача запуска; `None` — запуск вне задачи (отладка, тесты): папка файлов `flows/files/debug/<run_id>`.
    pub task: Option<TaskInfo>,
    /// Занятость профиля, которую задача уже взяла, дождавшись освобождения; `None` — запуск берёт её сам.
    pub busy: Option<Busy>,
    /// Копия каждой строки журнала — ход задачи в CLI.
    pub on_entry: Option<journal::Sink>,
    /// Начать обход с узла (отладка): id групп по вложенности, последним id узла; `None` — с Start.
    pub from: Option<Vec<String>>,
    /// Вызывается с id узла перед его выполнением — подсветка хода отладки.
    pub on_node: Option<crate::flow::interp::OnNode>,
}

pub async fn run_once<E: Engine>(core: &Core<E>, mut req: RunRequest<'_>) -> RunOutcome {
    let path = core.paths.flow_log(&req.run_id);
    let journal = Journal::file(&path)
        .unwrap_or_else(|e| {
            log::warn!("журнал процесса {} не открыт: {e}", path.display());
            Journal::default()
        })
        .with_sink(req.on_entry.clone());
    let busy = req.busy.take();
    let (outcome, mut journal) = execute(core, &req, journal, busy).await;
    journal.event(Status::End, &summary(&outcome));
    outcome
}

async fn execute<E: Engine>(core: &Core<E>, req: &RunRequest<'_>, mut journal: Journal, busy: Option<Busy>) -> (RunOutcome, Journal) {
    journal.event(Status::Start, &format!("сценарий «{}», профиль {}", req.flow.name, req.profile_id));
    let lookup = lookup(core, req.flow);
    let report = validate(req.flow, &*lookup);
    if !report.is_ok() {
        return (error(format!("В сценарии есть ошибки: {}", report.summary())), journal);
    }
    let (scope, humanize) = match scope(core, req) {
        Ok(pair) => pair,
        Err(e) => return (error(e), journal),
    };
    let Some(_busy) = busy.or_else(|| core.flows.claim(req.profile_id)) else {
        return (error("профиль занят другим процессом".into()), journal);
    };
    let settings = &req.flow.settings;
    if settings.delay_min > 0 {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(settings.delay_min * 60)) => {}
            _ = req.cancel.cancelled() => return (cancelled(), journal),
        }
    }
    // Запуск браузера не прерывается (А2), но отменённый заранее процесс его не начинает.
    if req.cancel.is_cancelled() {
        return (cancelled(), journal);
    }
    let options = StartOptions {
        headless: settings.headless,
        extra_args: (!settings.launch_args.is_empty()).then(|| settings.launch_args.clone()),
        restore_tabs: None,
        start_urls: None,
        clear_cache_on_exit: settings.clear_cache_on_quit,
    };
    // «Свой» браузер или нет, решает супервизор: предварительная проверка не видит профиль, который сейчас
    // запускает кто-то другой. Чужой, как и открытый до процесса, остаётся как был.
    let (info, launched, browser, current) = match connect_profile(&core.supervisor, req.profile_id, options.clone()).await {
        Ok(parts) => parts,
        Err(ConnectError::Start(SupervisorError::ProfileLocked)) => return (error(LOCKED.into()), journal),
        Err(ConnectError::Start(e)) => return (error(format!("профиль не запустился: {e}")), journal),
        Err(ConnectError::Link(e)) => return (error(format!("нет связи с браузером: {e}")), journal),
    };
    let attached = !launched;
    let mut ctx = Ctx::new(scope, settings.on_error, lookup, journal, req.cancel.clone());
    ctx.tabs = Some(Tabs::new(browser.clone(), Some(current)).with_relay(Some(info.relay_port)));
    ctx.position = req.position;
    ctx.humanize = humanize;
    ctx.on_node = req.on_node.clone();
    ctx.files = req.task.as_ref().map(|task| task.files.clone()).unwrap_or_else(|| core.paths.flow_files(&req.run_id));
    let host = Arc::new(CoreHost {
        store: core.store.clone(),
        supervisor: core.supervisor.clone(),
        flows: core.flows.clone(),
        profile_id: req.profile_id.to_string(),
        options,
        launched: Default::default(),
        opening: Default::default(),
    });
    ctx.host = Some(host.clone());
    ctx.store = Some(core.store.clone());
    // Отмена во время запуска: графа не начинаем, но «после задачи» выполняется как при любом конце.
    let run = async {
        if req.cancel.is_cancelled() {
            return Err(Fail::Cancelled);
        }
        // Оборванное соединение прерывает и ожидания, и циклы без обращений к браузеру.
        // `biased`: «Стоп» и обрыв одновременно — итог от графа («Отменён»), а не от обрыва.
        tokio::select! {
            biased;
            result = async {
                match &req.from {
                    Some(path) => ctx.run_from(req.flow, path).await,
                    None => ctx.run(req.flow).await,
                }
            } => result,
            _ = browser.closed() => Err(Fail::BrowserClosed),
        }
    };
    let result = if settings.timeout_s > 0 {
        tokio::time::timeout(Duration::from_secs(settings.timeout_s), run).await.unwrap_or(Err(Fail::Timeout))
    } else {
        run.await
    };
    // Слушатели и сеть, включённая для Request to finish, «после задачи» не нужны.
    ctx.listeners.stop();
    ctx.armed = None;
    if !attached {
        // Соединение могло оборваться при живом браузере: вкладки тогда не чистим, а запущенный нами браузер гасим.
        // Quit Browser верхнего процесса гасит его независимо от «после задачи».
        let (dropped, quit) = (result == Err(Fail::BrowserClosed), result == Ok(Exit::Quit));
        after(core, req, &browser, dropped, quit).await;
    }
    // Браузеры New Browser с after quit, до которых узел не дошёл (тайм-аут, закрытие основного браузера).
    host.stop_leftovers().await;
    save_profile(core, req.profile_id, &ctx.profile, &mut ctx.journal);
    (outcome(result), ctx.journal)
}

/// Приложение для запуска: данные основного профиля из базы.
struct CoreHost<E: Engine> {
    store: Arc<Store>,
    supervisor: Supervisor<E>,
    flows: FlowState,
    profile_id: String,
    /// Настройки запуска процесса: New Browser запускает профиль так же.
    options: StartOptions,
    /// Профили, запущенные New Browser с `quit`, и их занятость: до остановки браузера профиль никому не отдаётся.
    launched: Arc<Mutex<Vec<(String, Busy)>>>,
    /// Читатели — идущие `open`; `stop_leftovers` берёт запись и ждёт, пока они допишут свои профили в список.
    opening: Arc<tokio::sync::RwLock<()>>,
}

impl<E: Engine> CoreHost<E> {
    /// Гасит браузеры, которые New Browser запустил с `quit` и не успел погасить, и только после остановки
    /// освобождает профили. Сначала ждёт `open`, которые ещё запускают браузер.
    async fn stop_leftovers(&self) {
        let _writer = self.opening.write().await;
        let leftovers = std::mem::take(&mut *self.launched.lock().unwrap());
        for (id, busy) in leftovers {
            if let Err(e) = self.supervisor.stop(&id).await {
                log::warn!("профиль {id} после процесса не остановлен: {e}");
            }
            drop(busy);
        }
    }
}

impl<E: Engine> Host for CoreHost<E> {
    fn totp_secret(&self) -> Result<Option<String>, String> {
        let (_, totp) = self.store.profile_secrets_each(&self.profile_id).map_err(|e| e.to_string())?;
        let totp = totp.map_err(|_| "Не удалось прочитать ключ 2FA профиля".to_string())?;
        Ok(totp.map(|secret| secret.expose().to_string()))
    }

    fn profile_tags(&self) -> Result<Vec<String>, String> {
        self.store.profile_tags(&self.profile_id).map_err(|e| e.to_string())
    }

    fn tags(&self) -> Result<Vec<String>, String> {
        Ok(self.store.list_tags().map_err(|e| e.to_string())?.into_iter().map(|tag| tag.name).collect())
    }
    fn open(&self, serial: i64, quit: bool) -> BoxFuture<'_, Result<Opened, String>> {
        Box::pin(async move {
            let profile = self.store.profile_by_serial(serial).map_err(|e| e.to_string())?.ok_or("нет такого профиля")?;
            let busy = self.flows.claim(&profile.id).ok_or("профиль занят другим процессом")?;
            // Запуск идёт в своей задаче: оборванный `open` не теряет браузер, она допишет его в список.
            let reader = self.opening.clone().read_owned().await;
            let (supervisor, options, leftovers, id) =
                (self.supervisor.clone(), self.options.clone(), self.launched.clone(), profile.id.clone());
            let task = tokio::spawn(async move {
                let _reader = reader;
                let (info, launched, browser, tab) = match connect_profile(&supervisor, &id, options).await {
                    Ok(parts) => parts,
                    Err(ConnectError::Start(SupervisorError::ProfileLocked)) => return Err(LOCKED.to_string()),
                    Err(ConnectError::Start(e)) => return Err(format!("профиль не запустился: {e}")),
                    Err(ConnectError::Link(e)) => return Err(format!("нет связи с браузером профиля: {e}")),
                };
                let busy = if launched && quit {
                    leftovers.lock().unwrap().push((id, busy));
                    None
                } else {
                    Some(busy)
                };
                Ok((info, launched, browser, tab, busy))
            });
            let (info, launched, browser, tab, busy) = task.await.map_err(|e| format!("профиль не запустился: {e}"))??;
            let humanize = profile.humanize.enabled;
            Ok(Opened { profile_id: profile.id, browser, tab, relay_port: info.relay_port, launched, busy, humanize })
        })
    }

    fn close(&self, opened: Opened, quit: bool) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if quit && opened.launched {
                if let Err(e) = self.supervisor.stop(&opened.profile_id).await {
                    log::warn!("профиль {} после New Browser не остановлен: {e}", opened.profile_id);
                }
                self.launched.lock().unwrap().retain(|(id, _)| *id != opened.profile_id);
            }
        })
    }
}

/// Update Profile Remark / Tag: изменения запуска — в основной профиль при любом исходе, по порядку и к заметке и
/// тегам, какие у профиля сейчас (правки из интерфейса или Local API за время запуска не теряются); интерфейс узнаёт
/// о них.
fn save_profile<E: Engine>(core: &Core<E>, id: &str, draft: &ProfileDraft, journal: &mut Journal) {
    if draft.ops.is_empty() {
        return;
    }
    let mut failed = Vec::new();
    let note = draft.note(|| {
        let profile = core.store.get_profile(id).map_err(|e| e.to_string())?;
        profile.map(|profile| profile.note).ok_or_else(|| "профиль удалён".to_string())
    });
    match note {
        Ok(None) => {}
        Ok(Some(note)) => {
            let edit = ProfileEdit { note: Some(note), ..Default::default() };
            if let Err(e) = core.store.edit_profiles(&[id.to_string()], &edit) {
                failed.push(format!("заметка: {e}"));
            }
        }
        Err(e) => failed.push(format!("заметка: {e}")),
    }
    match draft.tags(|| core.store.profile_tags(id)) {
        Ok(None) => {}
        // Узел проверял предел по тегам своего времени; правки за время запуска могли его перейти.
        Ok(Some(tags)) if tags.len() > TAGS_LIMIT => {
            failed.push(format!("теги: у профиля было бы больше {TAGS_LIMIT} тегов"));
        }
        Ok(Some(tags)) => {
            if let Err(e) = core.store.set_profile_tags(id, &tags) {
                failed.push(format!("теги: {e}"));
            }
        }
        Err(e) => failed.push(format!("теги: {e}")),
    }
    core.supervisor.emit(Event::ProfilesChanged);
    if !failed.is_empty() {
        let message = format!("профиль не обновлён — {}", failed.join("; "));
        log::warn!("{message}");
        journal.event(Status::Error, &message);
    }
}

/// Подпроцессы: `subflows` файла, затем база.
pub(crate) fn lookup<E: Engine>(core: &Core<E>, flow: &Flow) -> Lookup {
    let subflows = flow.subflows.clone();
    let store = core.store.clone();
    Box::new(move |id| subflows.get(&id.to_string()).cloned().or_else(|| read_flow(&store, id)))
}

/// Процесс из базы; нечитаемый — в журнал приложения и `None`, как «не найден».
fn read_flow(store: &Store, id: i64) -> Option<Flow> {
    match store.get_flow(id) {
        Ok(record) => record.map(|record| record.flow),
        Err(e) => {
            log::warn!("процесс {id} не читается: {e}");
            None
        }
    }
}

/// Системные переменные из профиля, затем переменные Start с параметрами запуска. Второе — очеловечивание профиля.
fn scope<E: Engine>(core: &Core<E>, req: &RunRequest<'_>) -> Result<(Scope, bool), String> {
    let profile = core
        .store
        .get_profile(req.profile_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("профиль {} не найден", req.profile_id))?;
    let humanize = profile.humanize.enabled;
    let (password, _) = core.store.profile_secrets_each(req.profile_id).map_err(|e| e.to_string())?;
    // Не расшифровался — переменная не задана (`${password}` даст ошибку), а не подменена пустой строкой.
    let password = match password {
        Ok(secret) => Some(secret.map(|secret| secret.expose().to_string()).unwrap_or_default()),
        Err(e) => {
            log::warn!("пароль профиля не прочитан: {e}");
            None
        }
    };
    let (task_id, task_name) = req.task.as_ref().map(|task| (task.id.to_string(), task.name.clone())).unwrap_or_default();
    let mut system = HashMap::from([
        ("task_id", Value::from(task_id)),
        ("task_name", Value::from(task_name)),
        ("serial_number", Value::from(profile.serial.to_string())),
        ("browser_name", Value::from(profile.name)),
        ("acc_id", Value::from(profile.id)),
        ("comment", Value::from(profile.note)),
        ("user_name", Value::from(profile.username.unwrap_or_default())),
        ("last_error", Value::from("")),
    ]);
    if let Some(password) = password {
        system.insert("password", Value::from(password));
    }
    let mut scope = Scope::new(system);
    vars::declare(&mut scope, &req.flow.variables, req.params.clone())?;
    Ok((scope, humanize))
}

/// Где сорвалось подключение к профилю.
enum ConnectError {
    Start(SupervisorError),
    Link(String),
}

/// Запуск профиля или подключение к открытому, соединение и первая вкладка. Запущенный нами браузер при обрыве
/// связи гасится.
async fn connect_profile<E: Engine>(
    supervisor: &Supervisor<E>,
    id: &str,
    options: StartOptions,
) -> Result<(RunningInfo, bool, Browser, String), ConnectError> {
    let (info, launched) =
        supervisor.start_or_attach_owned(id, options, START_WAIT).await.map_err(ConnectError::Start)?;
    let connected = match Browser::connect(&info.cdp_ws).await {
        Ok(browser) => first_tab(&browser).await.map(|tab| (browser, tab)),
        Err(e) => Err(e),
    };
    match connected {
        Ok((browser, tab)) => Ok((info, launched, browser, tab)),
        Err(e) => {
            if launched
                && let Err(stop) = supervisor.stop(id).await
            {
                log::warn!("профиль {id} после обрыва связи не остановлен: {stop}");
            }
            Err(ConnectError::Link(e.to_string()))
        }
    }
}

/// Текущая вкладка запуска — первая страница; страниц нет — новая пустая.
async fn first_tab(browser: &Browser) -> Result<String, CdpError> {
    match browser.tabs().await?.into_iter().next() {
        Some(tab) => Ok(tab.target_id),
        None => browser.new_tab("about:blank").await,
    }
}

/// Действия «после задачи» у браузера, который запустил этот запуск.
async fn after<E: Engine>(core: &Core<E>, req: &RunRequest<'_>, browser: &Browser, dropped: bool, quit: bool) {
    let settings = &req.flow.settings;
    if settings.after_tabs == AfterTabs::Clear
        && !dropped
        && let Err(e) = clear_tabs(browser).await
    {
        log::warn!("вкладки после процесса не закрыты: {e}");
    }
    if quit || settings.after_browser == AfterBrowser::Quit {
        stop(core, req.profile_id).await;
    }
}

/// Все вкладки, кроме одной новой пустой: при следующем запуске восстанавливать нечего.
async fn clear_tabs(browser: &Browser) -> Result<(), CdpError> {
    let keep = browser.new_tab("about:blank").await?;
    for tab in browser.tabs().await?.into_iter().filter(|tab| tab.target_id != keep) {
        browser.close_tab(&tab.target_id).await?;
    }
    Ok(())
}

async fn stop<E: Engine>(core: &Core<E>, id: &str) {
    if let Err(e) = core.supervisor.stop(id).await {
        log::warn!("профиль {id} после процесса не остановлен: {e}");
    }
}

fn outcome(result: Result<Exit, Fail>) -> RunOutcome {
    match result {
        Ok(Exit::Export(exported)) => {
            let exported = exported.into_iter().map(|(key, value)| (key, vars::public(value))).collect();
            RunOutcome { status: RunStatus::Done, error: None, exported }
        }
        Ok(_) => RunOutcome { status: RunStatus::Done, error: None, exported: Map::new() },
        Err(Fail::Cancelled) => cancelled(),
        Err(Fail::Timeout) => error("Истекло время выполнения сценария".into()),
        Err(Fail::BrowserClosed) => error("браузер закрыт".into()),
        Err(Fail::Node { node, message } | Fail::Catch { node, message, .. }) => error(format!("узел {node}: {message}")),
    }
}

fn error(message: String) -> RunOutcome {
    RunOutcome { status: RunStatus::Error, error: Some(message), exported: Map::new() }
}

fn cancelled() -> RunOutcome {
    RunOutcome { status: RunStatus::Cancelled, error: None, exported: Map::new() }
}

fn summary(outcome: &RunOutcome) -> String {
    match outcome.status {
        RunStatus::Done => "готово".into(),
        RunStatus::Cancelled => "отменён".into(),
        RunStatus::Error => format!("ошибка: {}", outcome.error.as_deref().unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::forms::SecretInput;
    use crate::app::tests::{create_with_proxy, env, form, test_proxy};
    use serde_json::json;
    use std::time::Instant;

    fn flow(value: Value) -> Flow {
        let mut value = value;
        value["format"] = json!("ombra-flow/1");
        value["name"] = json!("Тест");
        serde_json::from_value(value).unwrap()
    }

    fn request<'a>(flow: &'a Flow, profile: &'a str, params: Value) -> RunRequest<'a> {
        RunRequest { flow, profile_id: profile, params: params.as_object().cloned().unwrap_or_default(), run_id: "t1".into(), cancel: CancellationToken::new(), position: 0, task: None, busy: None, on_entry: None, from: None, on_node: None }
    }

    fn exporting(names: Value) -> Flow {
        flow(json!({
            "variables": [{ "name": "n", "type": "number", "default": 1 }],
            "nodes": [{ "id": "s", "op": "start" }, { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": names } }],
            "edges": [{ "from": "s", "to": "x" }]
        }))
    }

    fn waiting(ms: i64, settings: Value) -> Flow {
        flow(json!({
            "settings": settings,
            "nodes": [{ "id": "s", "op": "start" }, { "id": "w", "op": "wait.time", "config": { "ms": ms } }],
            "edges": [{ "from": "s", "to": "w" }]
        }))
    }

    #[tokio::test]
    async fn profile_remark_and_tags_are_saved_at_the_end_even_after_an_error() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "Заметки", e.proxy_port, 1);
        let other = create_with_proxy(&e.core, "С тегами", e.proxy_port, 2);
        let edit = crate::store::profiles::ProfileEdit { note: Some("было".into()), ..Default::default() };
        e.core.store.edit_profiles(std::slice::from_ref(&profile.id), &edit).unwrap();
        e.core.store.add_tags(std::slice::from_ref(&other.id), &["VIP".to_string(), "Старый".to_string()]).unwrap();
        e.core.store.add_tags(std::slice::from_ref(&profile.id), &["Старый".to_string()]).unwrap();
        let f = flow(json!({
            "variables": [{ "name": "n", "type": "number", "default": 7 }],
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "r", "op": "profile.remark", "config": { "text": "${n}" } },
                { "id": "t", "op": "profile.tags", "config": { "tags": ["vip"] } },
                { "id": "bad", "op": "proc.field", "config": { "data": "${n}", "key": "x", "save_to": "y" } }
            ],
            "edges": [{ "from": "s", "to": "r" }, { "from": "r", "to": "t" }, { "from": "t", "to": "bad" }]
        }));
        let outcome = run_once(&e.core, request(&f, &profile.id, json!({}))).await;
        assert_eq!(outcome.status, RunStatus::Error, "{outcome:?}");
        let saved = e.core.store.get_profile(&profile.id).unwrap().unwrap();
        assert_eq!(saved.note, "было\n7");
        let mut tags = e.core.store.profile_tags(&profile.id).unwrap();
        tags.sort();
        assert_eq!(tags, ["VIP", "Старый"]);
    }

    #[tokio::test]
    async fn profile_edits_made_elsewhere_during_the_run_are_kept() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "Заметки", e.proxy_port, 1);
        let other = create_with_proxy(&e.core, "С тегами", e.proxy_port, 2);
        let many: Vec<String> = (0..29).map(|i| format!("т{i}")).collect();
        e.core.store.add_tags(std::slice::from_ref(&other.id), &["VIP".to_string(), "Извне".to_string(), "Лишний".to_string()]).unwrap();
        let f = flow(json!({
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "r", "op": "profile.remark", "config": { "text": "из процесса" } },
                { "id": "t", "op": "profile.tags", "config": { "tags": ["VIP"] } },
                { "id": "w", "op": "wait.time", "config": { "ms": 400 } }
            ],
            "edges": [{ "from": "s", "to": "r" }, { "from": "r", "to": "t" }, { "from": "t", "to": "w" }]
        }));
        // Пока процесс ждёт, заметку и теги меняют в интерфейсе.
        let (store, id) = (&e.core.store, &profile.id);
        let edit_meanwhile = move |tags: Vec<String>| async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let edit = crate::store::profiles::ProfileEdit { note: Some("извне".into()), ..Default::default() };
            store.edit_profiles(std::slice::from_ref(id), &edit).unwrap();
            store.add_tags(std::slice::from_ref(id), &tags).unwrap();
        };
        let (outcome, ()) = tokio::join!(run_once(&e.core, request(&f, &profile.id, json!({}))), edit_meanwhile(vec!["Извне".into()]));
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert_eq!(e.core.store.get_profile(&profile.id).unwrap().unwrap().note, "извне\nиз процесса");
        let mut tags = e.core.store.profile_tags(&profile.id).unwrap();
        tags.sort();
        assert_eq!(tags, ["VIP", "Извне"]);
        // Узел видел 29 тегов и 30-й допускал; к концу запуска их стало 30 — 31-й не пишется, в журнале ошибка.
        e.core.store.set_profile_tags(&profile.id, &many).unwrap();
        let (outcome, ()) = tokio::join!(run_once(&e.core, request(&f, &profile.id, json!({}))), edit_meanwhile(vec!["Лишний".into()]));
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        let tags = e.core.store.profile_tags(&profile.id).unwrap();
        assert!(tags.len() == 30 && !tags.contains(&"VIP".to_string()), "{tags:?}");
        let journal = std::fs::read_to_string(e.core.paths.flow_log("t1")).unwrap();
        let refused = journal.lines().any(|line| line.contains("\"status\":\"error\"") && line.contains("больше 30 тегов"));
        assert!(refused, "{journal}");
        assert_eq!(e.core.store.get_profile(&profile.id).unwrap().unwrap().note, "извне\nиз процесса", "заметка записана и без тегов");
    }

    #[tokio::test]
    async fn run_exports_params_and_profile_data_and_quits() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "Магазин", e.proxy_port, 1);
        let f = exporting(json!(["n", "acc_id", "browser_name", "serial_number"]));
        let outcome = run_once(&e.core, request(&f, &profile.id, json!({ "n": "7" }))).await;
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert_eq!(outcome.exported["n"], json!(7), "параметр перекрыл умолчание и приведён к number");
        assert_eq!(outcome.exported["acc_id"], json!(profile.id));
        assert_eq!(outcome.exported["browser_name"], json!("Магазин"));
        assert_eq!(outcome.exported["serial_number"], json!(profile.serial.to_string()));
        assert!(e.core.supervisor.running_info(&profile.id).is_none(), "after_browser quit — профиль остановлен");
        let journal = std::fs::read_to_string(e.core.paths.flow_log("t1")).unwrap();
        assert!(journal.lines().next().unwrap().contains("\"status\":\"start\""), "{journal}");
        assert!(journal.lines().last().unwrap().contains("\"status\":\"end\""), "{journal}");
    }

    #[tokio::test]
    async fn top_level_quit_browser_stops_a_browser_the_run_launched_even_with_keep() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let f = flow(json!({
            "settings": { "after_browser": "keep" },
            "nodes": [{ "id": "s", "op": "start" }, { "id": "q", "op": "control.quit_browser" }],
            "edges": [{ "from": "s", "to": "q" }]
        }));
        let outcome = run_once(&e.core, request(&f, &profile.id, json!({}))).await;
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert!(e.core.supervisor.running_info(&profile.id).is_none(), "Quit Browser остановил свой браузер");
    }

    #[tokio::test]
    async fn bad_params_and_invalid_flows_never_start_the_browser() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let outcome = run_once(&e.core, request(&exporting(json!(["n"])), &profile.id, json!({ "zzz": 1 }))).await;
        assert_eq!((outcome.status, outcome.error.as_deref()), (RunStatus::Error, Some("Переменная zzz не объявлена в сценарии")));
        let broken = flow(json!({ "nodes": [{ "id": "s", "op": "start" }, { "id": "t", "op": "web.teleport" }], "edges": [{ "from": "s", "to": "t" }] }));
        let outcome = run_once(&e.core, request(&broken, &profile.id, json!({}))).await;
        assert_eq!(outcome.status, RunStatus::Error);
        assert!(outcome.error.unwrap().starts_with("В сценарии есть ошибки: узел t, поле op"));
        assert_eq!(e.core.engine.launches(), 0);
    }

    #[tokio::test]
    async fn attached_browser_is_left_as_it_was() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        e.core.supervisor.start(&profile.id).await.unwrap();
        let fake = e.core.engine.cdp(&e.core.paths.profile_dir(&profile.id));
        fake.add_page("https://user.test/");
        let before = fake.pages();
        let outcome = run_once(&e.core, request(&exporting(json!(["n"])), &profile.id, json!({}))).await;
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert!(e.core.supervisor.running_info(&profile.id).is_some(), "браузер, запущенный до процесса, не закрывается");
        assert_eq!(fake.pages(), before, "его вкладки не трогаются");
        e.core.supervisor.stop(&profile.id).await.unwrap();
    }

    #[tokio::test]
    async fn browser_launched_by_someone_else_mid_run_is_left_alone() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let hold = std::sync::Arc::new(tokio::sync::Notify::new());
        *e.core.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
        let starting = || matches!(e.core.supervisor.status(&profile.id), crate::supervisor::ProfileStatus::Starting);
        let f = exporting(json!(["n"]));
        let (started, outcome, ()) = tokio::join!(
            e.core.supervisor.start(&profile.id),
            async {
                crate::supervisor::test_engine::wait_until("профиль запускается", starting).await;
                run_once(&e.core, request(&f, &profile.id, json!({}))).await
            },
            async {
                crate::supervisor::test_engine::wait_until("профиль запускается", starting).await;
                tokio::time::sleep(Duration::from_millis(300)).await;
                hold.notify_one();
            }
        );
        started.unwrap();
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert!(e.core.supervisor.running_info(&profile.id).is_some(), "чужой запуск не остановлен");
        assert_eq!(e.core.engine.cdp(&e.core.paths.profile_dir(&profile.id)).pages(), ["chrome://newtab/"]);
        e.core.supervisor.stop(&profile.id).await.unwrap();
    }

    #[tokio::test]
    async fn keep_browser_and_clear_tabs() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let f = flow(json!({
            "settings": { "after_tabs": "clear", "after_browser": "keep" },
            "nodes": [{ "id": "s", "op": "start" }, { "id": "a", "op": "web.new_tab" }, { "id": "b", "op": "web.new_tab" }],
            "edges": [{ "from": "s", "to": "a" }, { "from": "a", "to": "b" }]
        }));
        let outcome = run_once(&e.core, request(&f, &profile.id, json!({}))).await;
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert!(e.core.supervisor.running_info(&profile.id).is_some());
        assert_eq!(e.core.engine.cdp(&e.core.paths.profile_dir(&profile.id)).pages(), ["about:blank"], "осталась одна пустая вкладка");
        e.core.supervisor.stop(&profile.id).await.unwrap();
    }

    #[tokio::test]
    async fn busy_profile_is_rejected() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let _busy = e.core.flows.claim(&profile.id).unwrap();
        let outcome = run_once(&e.core, request(&exporting(json!(["n"])), &profile.id, json!({}))).await;
        assert_eq!(outcome.error.as_deref(), Some("профиль занят другим процессом"));
    }

    #[tokio::test]
    async fn timeout_and_stop_end_the_run() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let started = Instant::now();
        let outcome = run_once(&e.core, request(&waiting(10_000, json!({ "timeout_s": 1 })), &profile.id, json!({}))).await;
        assert_eq!((outcome.status, outcome.error.as_deref()), (RunStatus::Error, Some("Истекло время выполнения сценария")));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(e.core.supervisor.running_info(&profile.id).is_none(), "действия «после задачи» выполнены и после тайм-аута");
        let f = waiting(10_000, json!({}));
        let req = request(&f, &profile.id, json!({}));
        let cancel = req.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            cancel.cancel();
        });
        assert_eq!(run_once(&e.core, req).await.status, RunStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancelled_before_start_never_launches_a_browser() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let f = waiting(10_000, json!({}));
        let req = request(&f, &profile.id, json!({}));
        req.cancel.cancel();
        assert_eq!(run_once(&e.core, req).await.status, RunStatus::Cancelled);
        assert_eq!(e.core.engine.launches(), 0);
    }

    #[tokio::test]
    async fn cancelled_during_startup_ends_cancelled_and_cleans_up() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        *e.core.engine.launch_delay.lock().unwrap() = Duration::from_millis(400);
        let f = flow(json!({ "nodes": [{ "id": "s", "op": "start" }], "edges": [] }));
        let req = request(&f, &profile.id, json!({}));
        let cancel = req.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });
        assert_eq!(run_once(&e.core, req).await.status, RunStatus::Cancelled);
        assert!(e.core.supervisor.running_info(&profile.id).is_none(), "свой браузер убран после отмены");
    }

    #[tokio::test]
    async fn browser_killed_during_a_wait_ends_the_run_at_once() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let dir = e.core.paths.profile_dir(&profile.id);
        let engine = e.core.engine.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            engine.cdp(&dir).go_down();
        });
        let started = Instant::now();
        let outcome = run_once(&e.core, request(&waiting(10_000, json!({})), &profile.id, json!({}))).await;
        assert_eq!((outcome.status, outcome.error.as_deref()), (RunStatus::Error, Some("браузер закрыт")), "{outcome:?}");
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[tokio::test]
    async fn profile_password_never_reaches_the_journal() {
        let e = env().await;
        let mut input = form("С паролем", Some(test_proxy(&e.core, e.proxy_port).id));
        input.password = SecretInput::Set { value: "S3cr3t-пароль".into() };
        let profile = e.core.create_profile(&input, 1).unwrap();
        let f = flow(json!({
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "v", "op": "proc.vars", "config": { "vars": [{ "name": "p", "type": "string", "value": "${password}" }] } },
                { "id": "m", "op": "proc.regex", "config": { "data": "${p}", "pattern": "S3", "first_only": true, "save_to": "hit" } },
                { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["hit"] } }
            ],
            "edges": [{ "from": "s", "to": "v" }, { "from": "v", "to": "m" }, { "from": "m", "to": "x" }]
        }));
        let outcome = run_once(&e.core, request(&f, &profile.id, json!({}))).await;
        assert_eq!(outcome.exported["hit"], json!("S3"), "пароль профиля дошёл до процесса: {outcome:?}");
        let journal = std::fs::read_to_string(e.core.paths.flow_log("t1")).unwrap();
        assert!(!journal.contains("S3cr3t-пароль"), "{journal}");
    }

    #[tokio::test]
    async fn exported_elements_become_a_placeholder() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let f = flow(json!({
            "variables": [{ "name": "el", "type": "object", "default": { "$ombra_element": 0 } }],
            "nodes": [{ "id": "s", "op": "start" }, { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["el"] } }],
            "edges": [{ "from": "s", "to": "x" }]
        }));
        let outcome = run_once(&e.core, request(&f, &profile.id, json!({}))).await;
        assert_eq!(outcome.exported["el"], json!("<element>"), "{outcome:?}");
    }

    #[tokio::test]
    async fn humanize_flag_comes_from_the_profile() {
        let e = env().await;
        let on = create_with_proxy(&e.core, "С очеловечиванием", e.proxy_port, 1);
        let mut input = form("Без очеловечивания", Some(test_proxy(&e.core, e.proxy_port).id));
        input.humanize = Some(crate::humanize::Humanize { enabled: false, ..Default::default() });
        let off = e.core.create_profile(&input, 1).unwrap();
        let f = exporting(json!(["n"]));
        assert!(scope(&e.core, &request(&f, &on.id, json!({}))).unwrap().1, "у новых профилей включено по умолчанию");
        assert!(!scope(&e.core, &request(&f, &off.id, json!({}))).unwrap().1);
        assert!(e.core.paths.flow_files("t1").ends_with("flows/files/debug/t1"));
    }

    fn new_browser_flow(serial: i64, after: &str) -> Flow {
        flow(json!({
            "variables": [{ "name": "serial", "type": "number", "default": serial }],
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "b", "op": "control.new_browser", "config": { "serial": "${serial}", "after": after } },
                { "id": "in", "op": "proc.vars", "config": { "mode": "add", "vars": [{ "name": "inside", "type": "string", "value": "${acc_id}" }] } },
                { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["inside"] } }
            ],
            "edges": [{ "from": "s", "to": "b" }, { "from": "b", "port": "body", "to": "in" }, { "from": "b", "to": "x" }]
        }))
    }

    #[tokio::test]
    async fn new_browser_runs_its_body_in_another_profile_and_quits_it() {
        let e = env().await;
        let main = create_with_proxy(&e.core, "Основной", e.proxy_port, 1);
        let other = create_with_proxy(&e.core, "Второй", e.proxy_port, 2);
        let mut f = new_browser_flow(other.serial, "quit");
        f.settings.headless = true;
        let outcome = run_once(&e.core, request(&f, &main.id, json!({}))).await;
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert_eq!(outcome.exported["inside"], json!(main.id), "системные переменные внутри — основного профиля");
        assert!(e.core.engine.last_request().headless, "New Browser наследует режим всего запуска");
        assert!(e.core.supervisor.running_info(&other.id).is_none(), "after quit — второй профиль остановлен");
        assert!(e.core.flows.claim(&other.id).is_some(), "второй профиль освобождён");
    }

    #[tokio::test]
    async fn new_browser_with_keep_leaves_its_own_browser_running() {
        let e = env().await;
        let main = create_with_proxy(&e.core, "Основной", e.proxy_port, 1);
        let other = create_with_proxy(&e.core, "Второй", e.proxy_port, 2);
        let outcome = run_once(&e.core, request(&new_browser_flow(other.serial, "keep"), &main.id, json!({}))).await;
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert!(e.core.supervisor.running_info(&other.id).is_some(), "after keep — запущенный узлом браузер работает после запуска");
        assert!(e.core.flows.claim(&other.id).is_some(), "второй профиль освобождён");
        e.core.supervisor.stop(&other.id).await.unwrap();
    }

    #[tokio::test]
    async fn new_browser_keeps_a_browser_it_did_not_start() {
        let e = env().await;
        let main = create_with_proxy(&e.core, "Основной", e.proxy_port, 1);
        let other = create_with_proxy(&e.core, "Открыт заранее", e.proxy_port, 2);
        e.core.supervisor.start_with(&other.id, StartOptions::default()).await.unwrap();
        let outcome = run_once(&e.core, request(&new_browser_flow(other.serial, "quit"), &main.id, json!({}))).await;
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert!(e.core.supervisor.running_info(&other.id).is_some(), "что не открывали — не трогаем");
        e.core.supervisor.stop(&other.id).await.unwrap();
    }

    #[tokio::test]
    async fn new_browser_errors_leave_the_main_browser_current() {
        let e = env().await;
        let main = create_with_proxy(&e.core, "Основной", e.proxy_port, 1);
        let busy = run_once(&e.core, request(&new_browser_flow(main.serial, "quit"), &main.id, json!({}))).await;
        assert!(busy.error.as_deref().is_some_and(|m| m.ends_with("New Browser: профиль занят другим процессом")), "{busy:?}");
        let missing = run_once(&e.core, request(&new_browser_flow(99_999, "quit"), &main.id, json!({}))).await;
        assert!(missing.error.as_deref().is_some_and(|m| m.ends_with("New Browser: нет такого профиля")), "{missing:?}");
    }

    #[tokio::test]
    async fn break_in_new_browser_body_ends_only_the_body() {
        let e = env().await;
        let main = create_with_proxy(&e.core, "Основной", e.proxy_port, 1);
        let other = create_with_proxy(&e.core, "Второй", e.proxy_port, 2);
        let f = flow(json!({
            "variables": [{ "name": "n", "type": "string", "default": "" }],
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "l", "op": "control.for_times", "config": { "times": 2 } },
                { "id": "b", "op": "control.new_browser", "config": { "serial": other.serial, "after": "quit" } },
                { "id": "br", "op": "control.break" },
                { "id": "inc", "op": "proc.vars", "config": { "mode": "add", "vars": [{ "name": "n", "type": "string", "value": "${n}x" }] } },
                { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["n"] } }
            ],
            "edges": [
                { "from": "s", "to": "l" }, { "from": "l", "port": "body", "to": "b" },
                { "from": "b", "port": "body", "to": "br" }, { "from": "b", "to": "inc" },
                { "from": "inc", "to": "l", "to_port": "loop" }, { "from": "l", "to": "x" }
            ]
        }));
        let outcome = run_once(&e.core, request(&f, &main.id, json!({}))).await;
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert_eq!(outcome.exported["n"], json!("xx"), "{outcome:?}");
    }

    #[tokio::test]
    async fn new_browser_launched_browser_does_not_outlive_a_timed_out_run() {
        let e = env().await;
        let main = create_with_proxy(&e.core, "Основной", e.proxy_port, 1);
        let other = create_with_proxy(&e.core, "Второй", e.proxy_port, 2);
        let f = flow(json!({
            "settings": { "timeout_s": 1 },
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "b", "op": "control.new_browser", "config": { "serial": other.serial, "after": "quit" } },
                { "id": "w", "op": "wait.time", "config": { "ms": 10000 } }
            ],
            "edges": [{ "from": "s", "to": "b" }, { "from": "b", "port": "body", "to": "w" }]
        }));
        let outcome = run_once(&e.core, request(&f, &main.id, json!({}))).await;
        assert_eq!(outcome.error.as_deref(), Some("Истекло время выполнения сценария"), "{outcome:?}");
        // Не Starting и не Running: запуск второго профиля либо дописан и погашен, либо не шёл.
        assert!(matches!(e.core.supervisor.status(&other.id), crate::supervisor::ProfileStatus::Stopped));
        assert!(e.core.flows.claim(&other.id).is_some());
    }

    #[tokio::test]
    async fn closing_the_second_browser_interrupts_the_new_browser_body() {
        let e = env().await;
        let main = create_with_proxy(&e.core, "Основной", e.proxy_port, 1);
        let other = create_with_proxy(&e.core, "Второй", e.proxy_port, 2);
        let f = flow(json!({
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "b", "op": "control.new_browser", "config": { "serial": other.serial, "after": "quit" } },
                { "id": "w", "op": "wait.time", "config": { "ms": 10000 } }
            ],
            "edges": [{ "from": "s", "to": "b" }, { "from": "b", "port": "body", "to": "w" }]
        }));
        let close_soon = || {
            let (supervisor, engine, id, dir) = (e.core.supervisor.clone(), e.core.engine.clone(), other.id.clone(), e.core.paths.profile_dir(&other.id));
            tokio::spawn(async move {
                crate::supervisor::test_engine::wait_until("второй профиль запущен", || supervisor.running_info(&id).is_some()).await;
                tokio::time::sleep(Duration::from_millis(300)).await;
                engine.cdp(&dir).go_down();
            });
        };
        close_soon();
        let started = Instant::now();
        let outcome = run_once(&e.core, request(&f, &main.id, json!({}))).await;
        assert_eq!(outcome.error.as_deref(), Some("узел b: New Browser: браузер профиля закрыт"), "{outcome:?}");
        assert!(started.elapsed() < Duration::from_secs(5), "тело не дожидалось своей паузы: {:?}", started.elapsed());
        assert!(e.core.supervisor.running_info(&other.id).is_none(), "after quit — второй профиль остановлен");
        assert!(e.core.flows.claim(&other.id).is_some(), "второй профиль освобождён");
        // Прерванный посреди подпроцесса: переменные основного процесса на месте, запуск идёт дальше (skip).
        let sub = json!({
            "format": "ombra-flow/1", "name": "Пауза",
            "nodes": [{ "id": "s", "op": "start" }, { "id": "w", "op": "wait.time", "config": { "ms": 10000 } }],
            "edges": [{ "from": "s", "to": "w" }]
        });
        let f = flow(json!({
            "subflows": { "7": sub },
            "variables": [{ "name": "v", "type": "string", "default": "основной" }],
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "b", "op": "control.new_browser", "on_error": "skip", "config": { "serial": other.serial, "after": "quit" } },
                { "id": "c", "op": "control.call", "config": { "process": 7 } },
                { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["v"] } }
            ],
            "edges": [{ "from": "s", "to": "b" }, { "from": "b", "port": "body", "to": "c" }, { "from": "b", "to": "x" }]
        }));
        close_soon();
        let outcome = run_once(&e.core, request(&f, &main.id, json!({}))).await;
        assert_eq!((outcome.status, outcome.exported.get("v")), (RunStatus::Done, Some(&json!("основной"))), "{outcome:?}");
    }

    #[tokio::test]
    async fn task_runs_see_the_task_and_write_to_the_batch_folder() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "Задача", e.proxy_port, 1);
        let f = flow(json!({
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "t", "op": "data.save_txt", "config": { "file_name": "out", "template": "строка" } },
                { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["task_id", "task_name"] } }
            ],
            "edges": [{ "from": "s", "to": "t" }, { "from": "t", "to": "x" }]
        }));
        let files = e.core.paths.task_files(7, 3);
        let mut req = request(&f, &profile.id, json!({}));
        req.task = Some(TaskInfo { id: 7, name: "Ночная".into(), files: files.clone() });
        let outcome = run_once(&e.core, req).await;
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert_eq!((outcome.exported["task_id"].clone(), outcome.exported["task_name"].clone()), (json!("7"), json!("Ночная")));
        assert!(files.join("out-7.txt").is_file(), "файл — в папке пакета");
    }

    #[tokio::test]
    async fn a_run_handed_the_busy_profile_uses_it_and_frees_it() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let f = exporting(json!(["n"]));
        let mut req = request(&f, &profile.id, json!({}));
        req.busy = e.core.flows.claim(&profile.id);
        let outcome = run_once(&e.core, req).await;
        assert_eq!(outcome.status, RunStatus::Done, "занятость передана запуску — не «занят»: {outcome:?}");
        assert!(e.core.flows.claim(&profile.id).is_some(), "после запуска профиль свободен");
    }

    #[tokio::test]
    async fn profile_held_by_another_ombra_process_is_named() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        // Другой процесс Ombra (окно рядом с командой) держит папку профиля.
        let _held = crate::lock::DirLock::acquire(&e.core.paths.profile_dir(&profile.id)).unwrap();
        let outcome = run_once(&e.core, request(&exporting(json!(["n"])), &profile.id, json!({}))).await;
        assert_eq!(outcome.error.as_deref(), Some("профиль открыт в другом процессе Ombra"));
    }

    #[tokio::test]
    async fn journal_lines_reach_the_sink() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let seen: Arc<Mutex<Vec<crate::flow::journal::Entry>>> = Arc::default();
        let f = exporting(json!(["n"]));
        let mut req = request(&f, &profile.id, json!({}));
        let sink = seen.clone();
        req.on_entry = Some(Arc::new(move |entry: &crate::flow::journal::Entry| sink.lock().unwrap().push(entry.clone())));
        run_once(&e.core, req).await;
        let statuses: Vec<Status> = seen.lock().unwrap().iter().map(|entry| entry.status).collect();
        assert_eq!((statuses.first(), statuses.last()), (Some(&Status::Start), Some(&Status::End)));
        assert!(statuses.contains(&Status::Ok), "{statuses:?}");
        let file = std::fs::read_to_string(e.core.paths.flow_log("t1")).unwrap();
        assert_eq!(file.lines().count(), statuses.len(), "в файл — те же строки");
    }

    #[tokio::test]
    async fn run_once_from_a_node_skips_earlier_ones() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let f = flow(json!({
            "variables": [{ "name": "n", "type": "number", "default": 1 }],
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "a", "op": "wait.time", "config": { "ms": 0 } },
                { "id": "b", "op": "wait.time", "config": { "ms": 0 } }
            ],
            "edges": [{ "from": "s", "to": "a" }, { "from": "a", "to": "b" }]
        }));
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let mut req = request(&f, &profile.id, json!({}));
        req.from = Some(vec!["b".into()]);
        let sink = seen.clone();
        req.on_node = Some(Arc::new(move |id: &str| sink.lock().unwrap().push(id.to_string())));
        let outcome = run_once(&e.core, req).await;
        assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
        assert_eq!(*seen.lock().unwrap(), ["b"]);
        let log = std::fs::read_to_string(e.core.paths.flow_log("t1")).unwrap();
        assert!(log.contains("\"b\"") && !log.contains("\"a\""), "{log}");
    }

    #[tokio::test]
    async fn stop_during_the_start_delay_ends_the_run_at_once() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "A", e.proxy_port, 1);
        let f = waiting(10, json!({ "delay_min": 1 }));
        let req = request(&f, &profile.id, json!({}));
        let cancel = req.cancel.clone();
        let started = Instant::now();
        let stopper = async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        };
        let (outcome, ()) = tokio::join!(run_once(&e.core, req), stopper);
        assert_eq!(outcome.status, RunStatus::Cancelled, "{outcome:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(e.core.supervisor.running_info(&profile.id).is_none(), "браузер не запускался");
    }
}
