//! Tauri-команды — тонкий слой над `app::Core`. Имена аргументов в JS совпадают с именами здесь.
use crate::api::control::{ApiConfig, ApiConfigForm, ApiController, ApiStatus, StatusSink};
use crate::app::automation::{FlowRow, TaskDetails, TaskForm, TaskUpdateForm, TaskSettingsForm, TablePreview, TablePreviewForm, TableDataPreviewForm};
use crate::flow::tasks::Which;
use crate::app::bulk::{BulkCreateForm, BulkEditForm, BulkReport};
use crate::app::cookies::{CookieImportReport, CookieSource, ExportTarget};
use crate::app::forms::ProfileForm;
use crate::app::groups::{GroupForm, GroupList};
use crate::app::profiles::{FingerprintOptions, ListQueryDto, ProfileDetails, ProfileList, ProfileRow};
use crate::app::proxies::{ImportReport, ProxyEditForm, ProxyRow};
use crate::app::settings::{ProfileSettings, QuitPlan};
use crate::app::totp::TotpCode;
use crate::app::traffic::TrafficSummary;
use crate::app::windows::ScreenArea;
use crate::app::{AppInfo, CommandError, Core};
use crate::sys::unix_now;
use crate::cookies::CookieFormat;
use crate::engine::ombra::OmbraEngine;
use crate::engine::{Engine, EngineBuild};
use crate::fingerprint::{FieldError, Fingerprint};
use crate::lock::DirLock;
use crate::paths::Paths;
use crate::relay::geo::GeoInfo;
use crate::relay::log::ConnRecord;
use crate::store::QuitAction;
use crate::store::cookies::CookieStats;
use crate::store::extensions::{ExtCategory, ExtensionRecord};
use crate::store::groups::GroupRecord;
use crate::store::profiles::ProfileRecord;
use crate::store::proxies::{ProxyKind, ProxyRecord};
use crate::store::tags::TagRecord;
use crate::supervisor::{Event, RunningInfo, STOP_ALL_TIMEOUT};
use crate::sync::session::SyncState;
use crate::sync::SyncSettings;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, State};

pub const SUPERVISOR_EVENT: &str = "supervisor";
pub const QUIT_REQUESTED_EVENT: &str = "quit-requested";

/// Состояние Local API для интерфейса (экран — в 2В).
pub const API_STATUS_EVENT: &str = "api-status";

/// Local API изменил профили, группы или прокси: экраны «Профили» и «Группы» перечитывают списки.
pub const PROFILES_CHANGED_EVENT: &str = "profiles-changed";

/// Пакет задачи окна изменился: экран «Задачи» перечитывает список и журнал.
pub const TASKS_CHANGED_EVENT: &str = "tasks-changed";

/// Ход отладки процесса из редактора: подсветка узла, строки журнала, конец.
pub const FLOW_DEBUG_EVENT: &str = "flow-debug";

/// Сессия синхронизатора окон изменилась.
pub const SYNC_EVENT: &str = "sync";

/// Канал Tauri для события ядра: «данные изменились» — отдельный, остальное — `SUPERVISOR_EVENT`.
pub fn event_channel(event: &Event) -> &'static str {
    match event {
        Event::ProfilesChanged => PROFILES_CHANGED_EVENT,
        Event::TasksChanged { .. } => TASKS_CHANGED_EVENT,
        Event::FlowDebug(_) => FLOW_DEBUG_EVENT,
        Event::SyncChanged(_) => SYNC_EVENT,
        _ => SUPERVISOR_EVENT,
    }
}

pub struct AppState {
    paths: Paths,
    security: Mutex<Result<crate::security::Security, CommandError>>,
    sink: OnceLock<crate::supervisor::EventSink>,
    opening: Mutex<()>,
    activation: OnceLock<()>,
    /// Обычное ядро открывается при старте, защищённое — после подтверждения 2FA.
    core: OnceLock<Result<Arc<Core>, CommandError>>,
    quitting: AtomicBool,
    /// Общий срок выхода: первый путь выхода ставит его, следующие ждут только остаток.
    exit: ExitDeadline,
    /// Вопрос о выходе, отложенный до `ui_ready`.
    quit_gate: QuitGate,
    /// Контроллер Local API; создаётся в `setup`, если ядро открылось.
    api: OnceLock<ApiController<OmbraEngine>>,
    /// Блокировка папки данных: второй экземпляр не откроет ту же базу.
    instance: OnceLock<DirLock>,
}

impl AppState {
    pub fn new(paths: Paths) -> Self {
        Self {
            security: Mutex::new(crate::security::Security::load(paths.clone())),
            sink: OnceLock::new(),
            opening: Mutex::new(()),
            activation: OnceLock::new(),
            paths,
            core: OnceLock::new(),
            quitting: AtomicBool::new(false),
            exit: ExitDeadline::default(),
            quit_gate: QuitGate::default(),
            api: OnceLock::new(),
            instance: OnceLock::new(),
        }
    }

    /// `setup`: папка данных заблокирована этим экземпляром до выхода.
    pub fn hold_instance(&self, lock: DirLock) {
        let _ = self.instance.set(lock);
    }

    pub fn initialize_security(&self, sink: crate::supervisor::EventSink) {
        let _ = self.sink.set(sink);
        let result = self.with_security(|security| {
            if let Err(error) = security.prepare() { log::warn!("предыдущий том ещё закрывается: {}", error.code); }
            Ok(security.unprotected_paths())
        });
        match result {
            Ok(Some(paths)) => { let _ = self.open_unlocked_core(paths); }
            Ok(None) => {}
            Err(error) => self.set_core(Err(error)),
        }
    }

    fn with_security<T>(&self, action: impl FnOnce(&mut crate::security::Security) -> Result<T, CommandError>) -> Result<T, CommandError> {
        let mut security = self.security.lock().map_err(|_| CommandError::new("SecurityStorage", "защита недоступна"))?;
        match &mut *security { Ok(security) => action(security), Err(error) => Err(error.clone()) }
    }

    fn open_unlocked_core(&self, paths: Paths) -> Result<(), CommandError> {
        let _opening = self.opening.lock().map_err(|_| CommandError::new("SecurityStorage", "открытие хранилища прервано"))?;
        if self.core.get().is_some() { self.core()?; return Ok(()); }
        let sink = self.sink.get().ok_or_else(|| CommandError::new("Internal", "приложение ещё загружается"))?.clone();
        let core = crate::app::open_default(paths, sink).map(Arc::new);
        let result = core.as_ref().map(|_| ()).map_err(Clone::clone);
        self.set_core(core);
        result
    }

    /// `setup`: итог открытия ядра. Ставится один раз.
    pub fn set_core(&self, core: Result<Arc<Core>, CommandError>) {
        let _ = self.core.set(core);
    }

    pub fn core(&self) -> Result<&Arc<Core>, CommandError> {
        match self.core.get() {
            Some(Ok(core)) => Ok(core),
            Some(Err(e)) => Err(e.clone()),
            None => Err(crate::security::locked()),
        }
    }

    /// Контроллер Local API для команд «Настроек»; ядро не открылось — его ошибка.
    pub fn api(&self) -> Result<&ApiController<OmbraEngine>, CommandError> {
        self.core()?;
        self.api.get().ok_or_else(|| CommandError::new("Internal", "Local API ещё не запущен"))
    }

    /// `app_info`: данные ядра или причина, по которой оно не открылось (связка ключей, база, блокировка папки
    /// данных), — интерфейс показывает её экраном ошибки вместо экранов.
    pub fn info(&self) -> AppInfo {
        match self.core() {
            Ok(core) => core.app_info(),
            Err(e) => AppInfo {
                version: env!("CARGO_PKG_VERSION").into(),
                data_dir: self.paths.root().display().to_string(),
                language: crate::security::boot_language(&self.paths),
                engine: OmbraEngine::new(self.paths.engines()).installed(),
                init_error: (e.code != "SecurityLocked").then_some(e),
            },
        }
    }
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    state.info()
}

#[tauri::command]
pub fn security_status(state: State<'_, AppState>) -> Result<crate::security::SecurityStatus, CommandError> {
    state.with_security(|security| Ok(security.status(unix_now())))
}

#[tauri::command]
pub fn list_announcements(state: State<'_, AppState>) -> Vec<crate::app::announcements::Announcement> {
    crate::app::announcements::list(&state.paths, unix_now()).unwrap_or_else(|error| { log::warn!("каталог объявлений: {}", error.code); vec![] })
}

#[tauri::command]
pub fn open_announcement(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    crate::app::announcements::open(&state.paths, &id, unix_now())
}

#[tauri::command]
pub async fn security_begin_setup(app: AppHandle, password: String) -> Result<crate::security::SetupDetails, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let password = zeroize::Zeroizing::new(password);
        app.state::<AppState>().with_security(|security| security.begin_setup(&password, unix_now()))
    }).await.map_err(|_| CommandError::new("Internal", "настройка защиты прервана"))?
}

#[tauri::command]
pub fn security_verify_setup(state: State<'_, AppState>, code: String) -> Result<(), CommandError> {
    let code = zeroize::Zeroizing::new(code);
    state.with_security(|security| security.verify_setup(&code, unix_now()))
}

#[tauri::command]
pub fn security_cancel_setup(state: State<'_, AppState>) -> Result<(), CommandError> {
    state.with_security(|security| { security.cancel_setup(); Ok(()) })
}

async fn security_open(app: AppHandle, unlock: impl FnOnce(&mut crate::security::Security) -> Result<Paths, CommandError> + Send + 'static) -> Result<(), CommandError> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let paths = state.with_security(unlock)?;
        state.open_unlocked_core(paths)
    }).await.map_err(|_| CommandError::new("Internal", "разблокировка прервана"))??;
    activate_core(&app);
    Ok(())
}

#[tauri::command]
pub async fn security_finish_setup(app: AppHandle, recovery_saved: bool) -> Result<(), CommandError> {
    let handle = app.clone();
    let paths = tauri::async_runtime::spawn_blocking(move || {
        handle.state::<AppState>().with_security(|security| security.finish_setup(recovery_saved, unix_now()))
    }).await.map_err(|_| CommandError::new("Internal", "настройка защиты прервана"))??;
    if let Some(paths) = paths {
        let handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || handle.state::<AppState>().open_unlocked_core(paths))
            .await.map_err(|_| CommandError::new("Internal", "открытие хранилища прервано"))??;
        activate_core(&app);
    } else {
        let state = app.state::<AppState>();
        state.quitting.store(true, Ordering::SeqCst);
        let deadline = state.exit.start();
        stop_everything(state.api.get(), state.core().ok().map(Arc::as_ref), deadline).await;
        app.request_restart();
    }
    Ok(())
}

#[tauri::command]
pub async fn security_unlock(app: AppHandle, password: String, code: String, recovery: bool) -> Result<(), CommandError> {
    let password = zeroize::Zeroizing::new(password);
    let code = zeroize::Zeroizing::new(code);
    security_open(app, move |security| security.unlock(&password, &code, recovery, unix_now())).await
}

#[tauri::command]
pub async fn security_change_password(app: AppHandle, current: String, code: String, new_password: String) -> Result<Vec<String>, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let (current, code, new_password) = (zeroize::Zeroizing::new(current), zeroize::Zeroizing::new(code), zeroize::Zeroizing::new(new_password));
        app.state::<AppState>().with_security(|security| security.change_password(&current, &code, &new_password, unix_now()))
    }).await.map_err(|_| CommandError::new("Internal", "смена пароля прервана"))?
}

#[tauri::command]
pub async fn security_replace_authenticator(app: AppHandle, password: String, code: String) -> Result<crate::security::SetupDetails, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let (password, code) = (zeroize::Zeroizing::new(password), zeroize::Zeroizing::new(code));
        app.state::<AppState>().with_security(|security| security.replace_authenticator(&password, &code, unix_now()))
    }).await.map_err(|_| CommandError::new("Internal", "смена аутентификатора прервана"))?
}

/// Вызывается один раз после открытия базы: сразу в обычном режиме или после 2FA в защищённом.
pub fn activate_core(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.activation.set(()).is_err() { return; }
    if let Ok(core) = state.core() {
        let core = core.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = core.supervisor.cleanup_orphans().await { log::warn!("уборка предыдущего сеанса: {error}"); }
            if let Err(error) = core.sweep_extensions_when_alone().await { log::warn!("чистка расширений: {}", error.code); }
        });
    }
    start_api(app);
}

#[tauri::command]
pub fn list_profiles(state: State<'_, AppState>, query: ListQueryDto) -> Result<ProfileList, CommandError> {
    state.core()?.list_profiles(&query)
}

#[tauri::command]
pub fn list_profile_labels(state: State<'_, AppState>) -> Result<Vec<crate::store::profile_labels::ProfileLabel>, CommandError> {
    state.core()?.list_profile_labels()
}

#[tauri::command]
pub fn save_profile_label(state: State<'_, AppState>, id: Option<i64>, name: String, color: String) -> Result<crate::store::profile_labels::ProfileLabel, CommandError> {
    state.core()?.save_profile_label(id, &name, &color)
}

#[tauri::command]
pub fn delete_profile_label(state: State<'_, AppState>, id: i64) -> Result<(), CommandError> {
    state.core()?.delete_profile_label(id)
}

#[tauri::command]
pub fn set_profile_label(state: State<'_, AppState>, id: String, label_id: Option<i64>) -> Result<Option<crate::store::profile_labels::ProfileLabel>, CommandError> {
    state.core()?.set_profile_label(&id, label_id)
}

#[tauri::command]
pub fn get_profile(state: State<'_, AppState>, id: String) -> Result<ProfileDetails, CommandError> {
    state.core()?.profile_details(&id)
}

#[tauri::command]
pub fn reset_persona(state: State<'_, AppState>, id: String) -> Result<ProfileDetails, CommandError> {
    state.core()?.reset_persona(&id)
}

#[tauri::command]
pub fn totp_code(state: State<'_, AppState>, id: String) -> Result<TotpCode, CommandError> {
    state.core()?.totp_code(&id)
}

#[tauri::command]
pub fn create_profile(state: State<'_, AppState>, form: ProfileForm) -> Result<ProfileRecord, CommandError> {
    state.core()?.create_profile(&form, unix_now())
}

#[tauri::command]
pub fn update_profile(state: State<'_, AppState>, id: String, form: ProfileForm) -> Result<ProfileRecord, CommandError> {
    state.core()?.update_profile(&id, &form)
}

#[tauri::command]
pub async fn delete_profile(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    state.core()?.delete_profile(&id).await
}

#[tauri::command]
pub async fn start_profile(state: State<'_, AppState>, id: String) -> Result<RunningInfo, CommandError> {
    Ok(state.core()?.supervisor.start(&id).await?)
}

#[tauri::command]
pub async fn stop_profile(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    Ok(state.core()?.supervisor.stop(&id).await?)
}

#[tauri::command]
pub fn list_groups(state: State<'_, AppState>) -> Result<GroupList, CommandError> {
    state.core()?.list_groups()
}

#[tauri::command]
pub fn create_group(state: State<'_, AppState>, form: GroupForm) -> Result<GroupRecord, CommandError> {
    state.core()?.create_group(&form, unix_now())
}

#[tauri::command]
pub fn update_group(state: State<'_, AppState>, id: i64, form: GroupForm) -> Result<GroupRecord, CommandError> {
    state.core()?.update_group(id, &form)
}

#[tauri::command]
pub fn delete_group(state: State<'_, AppState>, id: i64) -> Result<(), CommandError> {
    state.core()?.delete_group(id)
}

/// «Запустить все» у группы; `group: null` — «Без группы».
#[tauri::command]
pub async fn start_group(state: State<'_, AppState>, group: Option<i64>) -> Result<BulkReport, CommandError> {
    state.core()?.start_group(group).await
}

#[tauri::command]
pub fn list_tags(state: State<'_, AppState>) -> Result<Vec<TagRecord>, CommandError> {
    state.core()?.list_tags()
}

#[tauri::command]
pub fn change_profile_tags(state: State<'_, AppState>, id: String, add: Vec<String>, remove: Vec<String>) -> Result<Vec<String>, CommandError> {
    state.core()?.change_profile_tags(&id, &add, &remove)
}

/// Работа ядра в блокирующем потоке (спецификация 2В-3, «Тяжёлые команды»): тысячи строк базы или запись файлов
/// не занимают рабочий поток tokio, окно не замирает. Ответ — как у вызова напрямую.
async fn blocking<T, F>(state: &AppState, work: F) -> Result<T, CommandError>
where
    T: Send + 'static,
    F: FnOnce(&Core) -> Result<T, CommandError> + Send + 'static,
{
    let core = state.core()?.clone();
    tauri::async_runtime::spawn_blocking(move || work(&core))
        .await
        .map_err(|e| CommandError::new("Internal", format!("фоновая задача прервана: {e}")))?
}

// Команды ниже пишут много строк или файлов: работа ядра — в `blocking`.

#[tauri::command]
pub async fn clone_profiles(state: State<'_, AppState>, ids: Vec<String>) -> Result<BulkReport, CommandError> {
    blocking(&state, move |core| Ok(core.clone_profiles(&ids, unix_now()))).await
}

#[tauri::command]
pub async fn create_profiles_bulk(
    state: State<'_, AppState>,
    form: BulkCreateForm,
) -> Result<Vec<ProfileRecord>, CommandError> {
    blocking(&state, move |core| core.create_profiles_bulk(&form, unix_now())).await
}

/// «Расставить окна»: рабочая область главного монитора (`Monitor::work_area`, Tauri 2.12 — без строки меню и дока)
/// в логических пикселях.
#[tauri::command]
pub async fn arrange_windows(app: AppHandle, state: State<'_, AppState>) -> Result<BulkReport, CommandError> {
    let core = state.core()?.clone();
    let monitor = app
        .primary_monitor()
        .map_err(|e| CommandError::new("Internal", e.to_string()))?
        .ok_or_else(|| CommandError::new("Internal", "нет главного монитора"))?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let size = area.size.to_logical::<u32>(scale);
    let pos = area.position.to_logical::<i32>(scale);
    Ok(core.arrange_windows(&ScreenArea { x: pos.x, y: pos.y, width: size.width, height: size.height }).await)
}

#[tauri::command]
pub fn sync_state(state: State<'_, AppState>) -> Result<SyncState, CommandError> {
    Ok(state.core()?.sync_state())
}

#[tauri::command]
pub async fn sync_start(state: State<'_, AppState>, main: String, followers: Vec<String>) -> Result<SyncState, CommandError> {
    state.core()?.sync_start(&main, &followers).await
}

#[tauri::command]
pub async fn sync_stop(state: State<'_, AppState>) -> Result<(), CommandError> {
    state.core()?.sync_stop().await;
    Ok(())
}

#[tauri::command]
pub async fn sync_release(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    state.core()?.sync_release(&id).await
}

#[tauri::command]
pub async fn sync_resume(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    state.core()?.sync_resume(&id).await
}

#[tauri::command]
pub async fn sync_open_like_main(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    state.core()?.sync_open_like_main(&id).await
}

#[tauri::command]
pub async fn show_window(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    state.core()?.show_window(&id).await
}

#[tauri::command]
pub fn get_sync_settings(state: State<'_, AppState>) -> Result<SyncSettings, CommandError> {
    Ok(state.core()?.sync_settings())
}

#[tauri::command]
pub fn set_sync_settings(state: State<'_, AppState>, settings: SyncSettings) -> Result<(), CommandError> {
    state.core()?.set_sync_settings(settings)
}

#[tauri::command]
pub async fn traffic_summary(state: State<'_, AppState>, id: String) -> Result<TrafficSummary, CommandError> {
    blocking(&state, move |core| core.traffic_summary(&id)).await
}

#[tauri::command]
pub async fn connection_log(state: State<'_, AppState>, id: String, limit: usize) -> Result<Vec<ConnRecord>, CommandError> {
    blocking(&state, move |core| core.connection_log(&id, limit)).await
}

// Расширения (спецификация 3Б). Скачивание и распаковка — в ядре вне главного потока.

#[tauri::command]
pub fn list_extensions(state: State<'_, AppState>) -> Result<Vec<ExtensionRecord>, CommandError> {
    state.core()?.list_extensions()
}

/// Идентификатор или ссылка Chrome Web Store.
#[tauri::command]
pub async fn add_store_extension(state: State<'_, AppState>, input: String) -> Result<ExtensionRecord, CommandError> {
    let core = state.core()?.clone();
    core.add_store_extension(&input, unix_now()).await
}

/// `.crx`, `.zip` или папка с `manifest.json`.
#[tauri::command]
pub async fn add_file_extension(state: State<'_, AppState>, path: String) -> Result<ExtensionRecord, CommandError> {
    let core = state.core()?.clone();
    core.add_file_extension(std::path::Path::new(&path), unix_now()).await
}

#[tauri::command]
pub async fn update_extension(state: State<'_, AppState>, id: String) -> Result<ExtensionRecord, CommandError> {
    let core = state.core()?.clone();
    core.update_extension(&id, unix_now()).await
}

#[tauri::command]
pub async fn remove_extension(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    let core = state.core()?.clone();
    core.remove_extension(&id).await
}

#[tauri::command]
pub fn list_ext_categories(state: State<'_, AppState>) -> Result<Vec<ExtCategory>, CommandError> {
    state.core()?.list_ext_categories()
}

#[tauri::command]
pub fn create_ext_category(state: State<'_, AppState>, name: String, remark: String) -> Result<ExtCategory, CommandError> {
    state.core()?.create_ext_category(&name, &remark, unix_now())
}

#[tauri::command]
pub fn update_ext_category(state: State<'_, AppState>, id: i64, name: String, remark: String) -> Result<ExtCategory, CommandError> {
    state.core()?.update_ext_category(id, &name, &remark)
}

#[tauri::command]
pub fn set_ext_category_items(state: State<'_, AppState>, id: i64, extension_ids: Vec<String>) -> Result<ExtCategory, CommandError> {
    state.core()?.set_ext_category_items(id, &extension_ids)
}

#[tauri::command]
pub fn delete_ext_category(state: State<'_, AppState>, id: i64) -> Result<(), CommandError> {
    state.core()?.delete_ext_category(id)
}

#[tauri::command]
pub async fn set_profiles_ext_category(state: State<'_, AppState>, ids: Vec<String>, category_id: i64) -> Result<BulkReport, CommandError> {
    blocking(&state, move |core| core.set_profiles_ext_category(&ids, category_id)).await
}

/// «Установить в группу»; `None` — «Без группы».
#[tauri::command]
pub async fn set_group_ext_category(state: State<'_, AppState>, group_id: Option<i64>, category_id: i64) -> Result<BulkReport, CommandError> {
    blocking(&state, move |core| core.set_group_ext_category(group_id, category_id)).await
}

#[tauri::command]
pub async fn start_profiles(state: State<'_, AppState>, ids: Vec<String>) -> Result<BulkReport, CommandError> {
    Ok(state.core()?.start_profiles(&ids).await)
}

#[tauri::command]
pub async fn stop_profiles(state: State<'_, AppState>, ids: Vec<String>) -> Result<BulkReport, CommandError> {
    Ok(state.core()?.stop_profiles(&ids).await)
}

#[tauri::command]
pub fn list_flows(state: State<'_, AppState>) -> Result<Vec<FlowRow>, CommandError> {
    state.core()?.list_flows()
}

#[tauri::command]
pub fn list_tasks(state: State<'_, AppState>) -> Result<Vec<crate::store::tasks::TaskRow>, CommandError> {
    state.core()?.list_tasks()
}

#[tauri::command]
pub fn list_batches(state: State<'_, AppState>, task_id: i64) -> Result<Vec<crate::store::tasks::BatchSummary>, CommandError> {
    state.core()?.list_batches(task_id)
}

#[tauri::command]
pub fn list_runs(state: State<'_, AppState>, batch_id: i64) -> Result<Vec<crate::store::tasks::RunRow>, CommandError> {
    state.core()?.list_runs(batch_id)
}

#[tauri::command]
pub fn run_journal(state: State<'_, AppState>, run_id: i64) -> Result<Vec<crate::flow::journal::Entry>, CommandError> {
    state.core()?.run_journal(run_id)
}

#[tauri::command]
pub fn get_task(state: State<'_, AppState>, task_id: i64) -> Result<TaskDetails, CommandError> {
    state.core()?.get_task(task_id)
}

#[tauri::command]
pub async fn update_task(state: State<'_, AppState>, task_id: i64, form: TaskUpdateForm) -> Result<(), CommandError> {
    state.core()?.update_task(task_id, form, unix_now())
}

#[tauri::command]
pub fn preview_table_data(state: State<'_, AppState>, form: TableDataPreviewForm) -> Result<TablePreview, CommandError> {
    state.core()?.preview_table_data(form)
}

#[tauri::command]
pub fn update_task_settings(state: State<'_, AppState>, task_id: i64, form: TaskSettingsForm) -> Result<(), CommandError> {
    state.core()?.update_task_settings(task_id, form)
}

#[tauri::command]
pub fn delete_task(state: State<'_, AppState>, task_id: i64) -> Result<(), CommandError> {
    state.core()?.delete_task(task_id)
}

#[tauri::command]
pub async fn create_task(state: State<'_, AppState>, form: TaskForm) -> Result<i64, CommandError> {
    state.core()?.create_and_start_task(form, unix_now())
}

#[tauri::command]
pub async fn start_task(state: State<'_, AppState>, task_id: i64, failed: bool) -> Result<(), CommandError> {
    state.core()?.start_task(task_id, if failed { Which::Failed } else { Which::All })
}

#[tauri::command]
pub fn stop_task(state: State<'_, AppState>, task_id: i64) -> Result<bool, CommandError> {
    Ok(state.core()?.stop_task(task_id))
}

#[tauri::command]
pub async fn test_node(state: State<'_, AppState>, flow: crate::flow::model::Flow, node_id: String) -> Result<String, CommandError> {
    state.core()?.test_node(flow, node_id).await
}

#[tauri::command]
pub async fn debug_flow(state: State<'_, AppState>, flow: crate::flow::model::Flow, serial: i64, from: Option<Vec<String>>) -> Result<String, CommandError> {
    state.core()?.debug_flow(flow, serial, from)
}

#[tauri::command]
pub fn stop_debug(state: State<'_, AppState>, run_id: String) -> Result<bool, CommandError> {
    Ok(state.core()?.stop_debug(&run_id))
}

#[tauri::command]
pub fn cancel_waiting(state: State<'_, AppState>, task_id: i64) -> Result<bool, CommandError> {
    Ok(state.core()?.cancel_waiting(task_id))
}

/// Чтение таблицы (большой XLSX — распаковка и разбор XML) — в `blocking`: окно не замирает.
#[tauri::command]
pub async fn preview_table(state: State<'_, AppState>, form: TablePreviewForm) -> Result<TablePreview, CommandError> {
    blocking(&state, move |core| core.preview_table(form)).await
}

#[tauri::command]
pub fn open_batch_files(state: State<'_, AppState>, batch_id: i64) -> Result<(), CommandError> {
    let dir = state.core()?.batch_files_dir(batch_id)?;
    open::that(&dir).map_err(CommandError::from)
}

#[tauri::command]
pub async fn import_flow_json(state: State<'_, AppState>, source: String) -> Result<FlowRow, CommandError> {
    state.core()?.import_flow_json(&source, unix_now()).await
}

#[tauri::command]
pub async fn import_flow_file(state: State<'_, AppState>, path: String) -> Result<FlowRow, CommandError> {
    state.core()?.import_flow_file(std::path::Path::new(&path), unix_now()).await
}

#[tauri::command]
pub fn export_flow_file(state: State<'_, AppState>, id: i64, path: String) -> Result<(), CommandError> {
    state.core()?.export_flow_file(id, std::path::Path::new(&path))
}

#[tauri::command]
pub async fn copy_flow(state: State<'_, AppState>, id: i64) -> Result<FlowRow, CommandError> {
    state.core()?.copy_flow(id, unix_now()).await
}

#[tauri::command]
pub fn ops_catalog(state: State<'_, AppState>) -> Result<crate::app::automation::OpsCatalog, CommandError> {
    Ok(state.core()?.ops_catalog())
}

#[tauri::command]
pub fn get_flow(state: State<'_, AppState>, id: i64) -> Result<crate::flow::model::Flow, CommandError> {
    state.core()?.get_flow(id)
}

#[tauri::command]
pub fn validate_flow(state: State<'_, AppState>, flow: crate::flow::model::Flow) -> Result<crate::flow::validate::Report, CommandError> {
    Ok(state.core()?.validate_flow(&flow))
}

#[tauri::command]
pub async fn save_flow(state: State<'_, AppState>, id: Option<i64>, flow: crate::flow::model::Flow) -> Result<FlowRow, CommandError> {
    state.core()?.save_flow(id, flow, unix_now()).await
}

#[tauri::command]
pub async fn delete_flow(state: State<'_, AppState>, id: i64) -> Result<(), CommandError> {
    state.core()?.delete_flow(id).await
}

#[tauri::command]
pub fn set_flow_defaults(state: State<'_, AppState>, id: i64, values: serde_json::Map<String, serde_json::Value>) -> Result<FlowRow, CommandError> {
    state.core()?.set_flow_defaults(id, values, unix_now())
}

#[tauri::command]
pub fn list_trash(state: State<'_, AppState>) -> Result<Vec<ProfileRow>, CommandError> {
    state.core()?.list_trash()
}

#[tauri::command]
pub async fn restore_profiles(state: State<'_, AppState>, ids: Vec<String>) -> Result<BulkReport, CommandError> {
    Ok(state.core()?.restore_profiles(&ids).await)
}

#[tauri::command]
pub async fn purge_profiles(state: State<'_, AppState>, ids: Vec<String>) -> Result<BulkReport, CommandError> {
    Ok(state.core()?.purge_profiles(&ids).await)
}

#[tauri::command]
pub async fn empty_trash(state: State<'_, AppState>) -> Result<BulkReport, CommandError> {
    state.core()?.empty_trash().await
}

#[tauri::command]
pub async fn delete_profiles(state: State<'_, AppState>, ids: Vec<String>) -> Result<BulkReport, CommandError> {
    Ok(state.core()?.delete_profiles(&ids).await)
}

/// Перенос в группу; `group: null` — в «Без группы».
#[tauri::command]
pub async fn move_profiles(
    state: State<'_, AppState>,
    ids: Vec<String>,
    group: Option<i64>,
) -> Result<BulkReport, CommandError> {
    blocking(&state, move |core| core.move_profiles(&ids, group)).await
}

#[tauri::command]
pub async fn edit_profiles(
    state: State<'_, AppState>,
    ids: Vec<String>,
    form: BulkEditForm,
) -> Result<BulkReport, CommandError> {
    blocking(&state, move |core| core.edit_profiles(&ids, &form)).await
}

/// Путь выбирает пользователь в окне `tauri-plugin-dialog`, файлы пишет ядро.
#[tauri::command]
pub async fn export_cookies_to(
    state: State<'_, AppState>,
    ids: Vec<String>,
    format: CookieFormat,
    target: ExportTarget,
) -> Result<BulkReport, CommandError> {
    blocking(&state, move |core| core.export_cookies_to(&ids, format, &target)).await
}

#[tauri::command]
pub fn list_proxies(state: State<'_, AppState>) -> Result<Vec<ProxyRow>, CommandError> {
    state.core()?.list_proxies()
}

#[tauri::command]
pub async fn clear_cache(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    state.core()?.clear_cache(&id).await
}

#[tauri::command]
pub fn create_proxy(state: State<'_, AppState>, form: ProxyEditForm) -> Result<ProxyRecord, CommandError> {
    state.core()?.create_proxy(&form)
}

#[tauri::command]
pub fn update_proxy(state: State<'_, AppState>, id: i64, form: ProxyEditForm) -> Result<ProxyRecord, CommandError> {
    state.core()?.update_proxy(id, &form)
}

/// Прокси, занятый запущенным профилем, не удаляется — `ProxyInUse`.
#[tauri::command]
pub fn delete_proxy(state: State<'_, AppState>, id: i64) -> Result<(), CommandError> {
    state.core()?.delete_proxy(id)
}

#[tauri::command]
pub fn set_proxy_enabled(state: State<'_, AppState>, id: i64, enabled: bool) -> Result<(), CommandError> {
    state.core()?.set_proxy_enabled(id, enabled)
}

/// Импорт строками пишет много записей: работа — в блокирующем потоке.
#[tauri::command]
pub async fn import_proxies(state: State<'_, AppState>, text: String, kind: ProxyKind) -> Result<ImportReport, CommandError> {
    blocking(&state, move |core| core.import_proxies(&text, kind)).await
}

/// «Проверить» на экране «Прокси» и во вкладке «Шлюз»: итог пишется в запись прокси.
#[tauri::command]
pub async fn check_proxy(state: State<'_, AppState>, id: i64) -> Result<GeoInfo, CommandError> {
    state.core()?.check_proxy(id).await
}

#[tauri::command]
pub async fn check_connection(state: State<'_, AppState>, target: crate::app::proxies::ConnectionCheck, checker: crate::network::IpChecker) -> Result<GeoInfo, CommandError> {
    let core = state.core()?.clone();
    core.check_connection(&target, checker).await
}

/// Архив `tar.gz` остановленного профиля; пароль архива запечатывает секреты, без него их в файле нет.
#[tauri::command]
pub async fn export_profile(
    state: State<'_, AppState>,
    id: String,
    path: String,
    password: Option<String>,
) -> Result<(), CommandError> {
    let core = state.core()?.clone();
    core.export_profile(&id, std::path::Path::new(&path), password.as_deref()).await
}

/// Новый профиль из архива `tar.gz`; существующие не затрагиваются.
#[tauri::command]
pub async fn import_profile(
    state: State<'_, AppState>,
    path: String,
    password: Option<String>,
) -> Result<ProfileRecord, CommandError> {
    let core = state.core()?.clone();
    core.import_profile(std::path::Path::new(&path), password.as_deref()).await
}

/// Профили по фильтрам списка — в файл CSV; без `with_secrets` логин, пароли и 2FA пусты. Ответ — число профилей.
#[tauri::command]
pub async fn export_csv_file(state: State<'_, AppState>, query: ListQueryDto, with_secrets: bool, path: String) -> Result<usize, CommandError> {
    blocking(&state, move |core| core.export_csv_file(&query, with_secrets, std::path::Path::new(&path))).await
}

/// Профили из файла CSV: хорошие строки создаются, плохие — в отчёте с номером строки.
#[tauri::command]
pub async fn import_csv_file(state: State<'_, AppState>, path: String) -> Result<crate::app::csv::CsvImportReport, CommandError> {
    blocking(&state, move |core| core.import_csv_file(std::path::Path::new(&path))).await
}

/// Расшифровывает все cookies профиля: работа — в блокирующем потоке.
#[tauri::command]
pub async fn cookie_stats(state: State<'_, AppState>, id: String) -> Result<CookieStats, CommandError> {
    blocking(&state, move |core| core.cookie_stats(&id)).await
}

/// Текст из поля или путь из окна выбора файла: файл читает ядро.
#[tauri::command]
pub async fn import_cookies(
    state: State<'_, AppState>,
    id: String,
    source: CookieSource,
) -> Result<CookieImportReport, CommandError> {
    state.core()?.import_cookies_from(&id, &source).await
}

#[tauri::command]
pub async fn clear_cookies(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    state.core()?.clear_cookies(&id).await
}

#[tauri::command]
pub fn profile_settings(state: State<'_, AppState>) -> Result<ProfileSettings, CommandError> {
    state.core()?.profile_settings()
}

#[tauri::command]
pub fn set_on_quit(state: State<'_, AppState>, action: QuitAction) -> Result<(), CommandError> {
    state.core()?.set_on_quit(action)
}

#[tauri::command]
pub fn get_services_proxy(state: State<'_, AppState>) -> Result<Option<i64>, CommandError> {
    state.core()?.services_proxy()
}

#[tauri::command]
pub fn set_services_proxy(state: State<'_, AppState>, id: Option<i64>) -> Result<(), CommandError> {
    state.core()?.set_services_proxy(id)
}

#[tauri::command]
pub fn set_google_key(state: State<'_, AppState>, path: String) -> Result<String, CommandError> {
    state.core()?.set_google_key(std::path::Path::new(&path))
}

#[tauri::command]
pub fn clear_google_key(state: State<'_, AppState>) -> Result<(), CommandError> {
    state.core()?.clear_google_key()
}

#[tauri::command]
pub fn google_key_email(state: State<'_, AppState>) -> Result<Option<String>, CommandError> {
    state.core()?.google_key_email()
}

#[tauri::command]
pub async fn api_config(state: State<'_, AppState>) -> Result<ApiConfig, CommandError> {
    state.api()?.config().await
}

/// Включение, порт и «Требовать ключ» — на ходу; занятый порт — ошибка поля `port`, сервер работает на прежнем.
#[tauri::command]
pub async fn set_api_config(state: State<'_, AppState>, form: ApiConfigForm) -> Result<ApiConfig, CommandError> {
    state.api()?.apply(&form).await
}

#[tauri::command]
pub async fn regenerate_api_key(state: State<'_, AppState>) -> Result<ApiConfig, CommandError> {
    state.api()?.regenerate_key().await
}

#[tauri::command]
pub fn new_fingerprint(state: State<'_, AppState>) -> Result<Fingerprint, CommandError> {
    state.core()?.new_fingerprint()
}

#[tauri::command]
pub fn regenerate_fingerprint(state: State<'_, AppState>, current: Fingerprint) -> Result<Fingerprint, CommandError> {
    state.core()?.regenerate_fingerprint(&current)
}

#[tauri::command]
pub fn validate_fingerprint(state: State<'_, AppState>, fingerprint: Fingerprint) -> Result<Vec<FieldError>, CommandError> {
    Ok(state.core()?.validate_fingerprint(&fingerprint))
}

#[tauri::command]
pub fn fingerprint_options(state: State<'_, AppState>) -> Result<FingerprintOptions, CommandError> {
    Ok(state.core()?.fingerprint_options())
}

/// «Выбрать архив…» в настройках: путь к `.tar.gz` из диалога файла.
pub const ENGINE_PROGRESS_EVENT: &str = "engine-progress";

#[tauri::command]
pub fn engine_catalog(state: State<'_, AppState>) -> Result<crate::engine::ombra::catalog::Catalog, CommandError> {
    Ok(state.core()?.engine.catalog()?)
}

#[tauri::command]
pub async fn select_engine(state: State<'_, AppState>, id: String) -> Result<EngineBuild, CommandError> {
    Ok(state.core()?.engine.select_version(id).await?)
}

#[tauri::command]
pub fn set_engine_automatic(state: State<'_, AppState>, automatic: bool) -> Result<(), CommandError> {
    Ok(state.core()?.engine.set_automatic(automatic)?)
}

#[tauri::command]
pub async fn update_engine(state: State<'_, AppState>, app: AppHandle, background: bool) -> Result<crate::engine::ombra::update::Outcome, CommandError> {
    Ok(state.core()?.engine.update_stable(background, move |progress| {
        let _ = app.emit(ENGINE_PROGRESS_EVENT, progress);
    }).await?)
}

#[tauri::command]
pub fn cancel_engine_update(state: State<'_, AppState>) -> Result<(), CommandError> {
    state.core()?.engine.cancel_update();
    Ok(())
}

#[tauri::command]
pub async fn import_engine(state: State<'_, AppState>, path: String) -> Result<EngineBuild, CommandError> {
    let core = state.core()?.clone();
    core.import_engine(std::path::Path::new(&path)).await
}

#[tauri::command]
pub fn set_language(app: AppHandle, state: State<'_, AppState>, language: String) -> Result<(), CommandError> {
    if !["ru", "en"].contains(&language.as_str()) { return Err(CommandError::new("Validation", "неизвестный язык интерфейса")); }
    if let Ok(core) = state.core() { core.set_language(&language)?; }
    else { crate::security::remember_language(&state.paths, &language); }
    crate::menu::install(&app, &language).map_err(|e| CommandError::new("Internal", e.to_string()))
}

#[tauri::command]
pub fn reveal_data_dir(state: State<'_, AppState>) -> Result<(), CommandError> {
    // `open` возвращается сразу; ждём, чтобы не копились зомби-процессы.
    open::that(state.paths.root())?;
    Ok(())
}

/// «Найти в Chrome Web Store»: поиск в системном браузере. Адрес собирает ядро — интерфейс не открывает произвольные ссылки.
#[tauri::command]
pub fn open_store_search(query: String) -> Result<(), CommandError> {
    open::that(crate::app::extensions::store_search_url(&query))?;
    Ok(())
}

/// Ссылка «Chrome Web Store» на карточке: страница расширения в системном браузере.
#[tauri::command]
pub fn open_store_page(id: String) -> Result<(), CommandError> {
    let url = crate::app::extensions::store_page_url(&id)
        .ok_or_else(|| CommandError::validation(vec![FieldError { field: "id", code: "invalid" }]))?;
    open::that(url)?;
    Ok(())
}

/// «Остановить и выйти» в диалоге выхода.
#[tauri::command]
pub async fn confirm_quit(app: AppHandle) -> Result<(), CommandError> {
    stop_and_exit(app).await;
    Ok(())
}

/// Интерфейс подписался на события (`App.vue`, после всех подписок). Cmd+Q, нажатый раньше, разбирается сейчас:
/// профили могли остановиться, пока интерфейс грузился, — тогда выход без вопроса.
#[tauri::command]
pub fn ui_ready(app: AppHandle, state: State<'_, AppState>) {
    if state.quit_gate.ready() && !handle_quit_request(&app) {
        app.exit(0);
    }
}

/// Общий срок выхода из приложения (решение заказчика: весь выход — не дольше `STOP_ALL_TIMEOUT`). Ставится первым
/// путём выхода — «Остановить все», «Остановить и выйти», Dock, системный выход; следующие ждут только остаток.
#[derive(Debug, Default)]
pub struct ExitDeadline(OnceLock<Instant>);

impl ExitDeadline {
    /// Ставит срок «сейчас + `STOP_ALL_TIMEOUT`», если его ещё нет, и возвращает действующий.
    pub fn start(&self) -> Instant {
        *self.0.get_or_init(|| Instant::now() + STOP_ALL_TIMEOUT)
    }

    /// Сколько осталось до срока; `None` — выход не начат.
    #[cfg(test)]
    pub fn remaining(&self) -> Option<std::time::Duration> {
        self.0.get().map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }
}

/// Вопрос о выходе ждёт готовности интерфейса: Cmd+Q, нажатый до подписки на `quit-requested`, не теряется.
/// Готовность и отложенный вопрос — под одним мьютексом: `ui_ready`, пришедший в момент Cmd+Q, не разминётся с ним.
#[derive(Debug, Default)]
pub struct QuitGate(Mutex<QuitGateState>);

#[derive(Debug, Default)]
struct QuitGateState {
    ui_ready: bool,
    pending: bool,
}

impl QuitGate {
    /// Вопрос нужен сейчас. `true` — интерфейс подписан, задать; `false` — отложен до `ready`.
    pub fn ask_or_defer(&self) -> bool {
        let mut gate = self.lock();
        gate.pending |= !gate.ui_ready;
        gate.ui_ready
    }

    /// Интерфейс подписался на события. `true` — вопрос ждал, его пора задать (один раз).
    pub fn ready(&self) -> bool {
        let mut gate = self.lock();
        gate.ui_ready = true;
        std::mem::take(&mut gate.pending)
    }

    fn lock(&self) -> MutexGuard<'_, QuitGateState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Остановка всех профилей на выходе — до общего срока `deadline`: зависший браузер не держит выход, а вторая
/// остановка ждёт только остаток. `stop_all` ставит `closing` при первом же опросе — и тогда, когда срок вышел:
/// запуск после начала выхода не стартует.
pub async fn stop_all_until<E: Engine>(core: &Core<E>, deadline: Instant) {
    // Сессия синхронизатора — раньше браузеров: она возвращает очеловечивание и капчу через их соединения.
    let left = deadline.saturating_duration_since(Instant::now());
    let _ = tokio::time::timeout(left, core.sync_stop()).await;
    let left = deadline.saturating_duration_since(Instant::now());
    if tokio::time::timeout(left, core.supervisor.stop_all()).await.is_err() {
        log::warn!("профили не остановились до срока выхода ({} с от его начала)", STOP_ALL_TIMEOUT.as_secs());
    }
}

/// Выход с остановкой, без Tauri: сначала закрывается Local API (скрипт не запустит профиль, пока остальные
/// останавливаются), затем профили — до общего срока. Общая часть «Остановить и выйти» и «Остановить все».
pub async fn stop_everything<E: Engine>(api: Option<&ApiController<E>>, core: Option<&Core<E>>, deadline: Instant) {
    if let Some(api) = api {
        api.stop().await;
    }
    if let Some(core) = core {
        stop_all_until(core, deadline).await;
    }
}

/// `stop_everything` до общего срока и выход. `quitting` — повторное закрытие окна или Cmd+Q больше не спрашивают
/// и выходят сразу: `RunEvent::Exit` ждёт только остаток срока.
async fn stop_and_exit(app: AppHandle) {
    let state = app.state::<AppState>();
    state.quitting.store(true, Ordering::SeqCst);
    let deadline = state.exit.start();
    stop_everything(state.api.get(), state.core().ok().map(Arc::as_ref), deadline).await;
    app.exit(0);
}

/// Контроллер Local API (спецификация 2В-2): состояние сервера уходит интерфейсу событием `API_STATUS_EVENT`.
/// Сервер поднимается по настройкам; порт занят или настройки не прочитаны — приложение работает дальше,
/// интерфейс видит `failed`.
pub fn start_api(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(core) = state.core() else { return };
    let emitter = app.clone();
    let sink: StatusSink = Arc::new(move |status: ApiStatus| {
        let _ = emitter.emit(API_STATUS_EVENT, status);
    });
    state.api.get_or_init(|| ApiController::new(core.clone(), sink));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if let Some(api) = state.api.get() {
            api.start().await;
        }
    });
}

/// `RunEvent::Exit`, вне рантайма: Local API перестаёт принимать запросы раньше, чем останавливаются профили.
/// Идущие запросы не ждутся: весь выход укладывается в `STOP_ALL_TIMEOUT`.
pub fn stop_api(state: &AppState) {
    if let Some(api) = state.api.get() {
        api.stop_blocking();
    }
}

/// `RunEvent::Exit` — Dock, системный выход, конец «Остановить все» или повторный Cmd+Q. Local API закрывается,
/// профили останавливаются до общего срока (`ExitDeadline`): если выход начался раньше, ждётся только остаток.
/// Срок ставится первым делом: `stop_api` может ждать мьютекс контроллера, и срок после него нарушил бы
/// «выход целиком — не дольше `STOP_ALL_TIMEOUT`». `stop_all` зовётся и без активных профилей: флаг `closing`
/// не даст стартовать запуску, который уже вошёл в обработчик Local API.
pub fn finish_exit(state: &AppState) {
    let deadline = state.exit.start();
    stop_api(state);
    if let Ok(core) = state.core() {
        let core = core.clone();
        tauri::async_runtime::block_on(async move { stop_all_until(&core, deadline).await });
    }
}

/// Закрытие окна, Cmd+Q и закрытие последнего окна — по `Core::quit_plan` (ядро не открылось или выход уже идёт — закрыть сразу):
/// - `Ask` — вопрос интерфейсу; пока интерфейс не подписан (`ui_ready`), вопрос откладывается;
/// - `StopAll` — окно скрывается, Local API и профили останавливаются в фоне, затем выход;
/// - `Exit` — закрыть сразу.
///
/// `true` — закрытие отменить: оно случится после ответа пользователя или после остановки.
pub fn handle_quit_request(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    if state.quitting.load(Ordering::SeqCst) {
        return false;
    }
    match state.core().ok().map(|core| core.quit_plan()) {
        Some(QuitPlan::Exit) | None => false,
        Some(QuitPlan::Ask) => {
            if state.quit_gate.ask_or_defer() {
                ask_quit(app);
            }
            true
        }
        Some(QuitPlan::StopAll) => {
            // Флаг — сразу: повторный Cmd+Q, пока профили останавливаются, выходит без второй остановки,
            // `RunEvent::Exit` ждёт только остаток общего срока.
            state.quitting.store(true, Ordering::SeqCst);
            // До 15 с окно не отвечало бы и выглядело зависшим: оно скрывается, остановка идёт в фоне.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.hide();
            }
            tauri::async_runtime::spawn(stop_and_exit(app.clone()));
            true
        }
    }
}

/// Показывает окно и просит интерфейс задать вопрос «Остановить все профили и выйти?».
pub fn ask_quit(app: &AppHandle) {
    let active = app.state::<AppState>().core().map(|core| core.supervisor.active_count()).unwrap_or(0);
    show_main_window(app);
    let _ = app.emit(QUIT_REQUESTED_EVENT, active);
}

/// Окно выходит на передний план: вопрос о выходе и повторный запуск Ombra (плагин single-instance).
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Повторный запуск (плагин single-instance) и reopen из Finder/Dock: окно выходит на передний план, пока выход
/// не идёт, — иначе скрытое окно уходящего экземпляра всплыло бы на время остановки.
pub fn reveal_main_window(app: &AppHandle) {
    if !app.state::<AppState>().quitting.load(Ordering::SeqCst) {
        show_main_window(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use crate::api::test_support::api_env;
    use crate::api::control::ApiFailureCode;
    use crate::app::tests::{create_with_proxy, env};
    use crate::relay::test_support::{closed_port, port_closes};
    use crate::relay::geo::GeoClient;
    use crate::store::tests::memory_store;
    use crate::supervisor::test_engine::wait_until;
    use crate::supervisor::{ProfileStatus, SupervisorError, Timings};
    use serde_json::{Value, json};
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::sync::Notify;

    /// Выход, когда запрос Local API ещё идёт (запуск ждёт браузер): API закрывается сразу, не дожидаясь запроса,
    /// а весь выход укладывается в `STOP_ALL_TIMEOUT` (решение заказчика: не дольше 15 с).
    #[tokio::test]
    async fn quit_with_a_start_in_flight_fits_the_exit_timeout() {
        let env = api_env().await;
        let profile = create_with_proxy(&env.core, "Выход", env.proxy_port, 1);
        let hold = Arc::new(Notify::new());
        *env.core.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
        let request =
            tokio::spawn(env.client.get(format!("{}/api/v1/browser/start?user_id={}", env.base, profile.id)).send());
        wait_until("запуск дошёл до движка", || env.core.engine.launches() == 1).await;
        assert_eq!(env.core.engine.launches(), 1, "запуск дошёл до движка и ждёт");

        let quit = Instant::now();
        let addr = env.server.addr();
        // Как `stop_api` на выходе.
        env.server.shutdown();
        let closed = quit.elapsed();
        assert!(closed < Duration::from_millis(500), "Local API закрывался {closed:?}: выход ждал идущий запрос");
        assert!(port_closes(addr).await, "новые соединения не принимаются");

        // Запуск доходит до конца сам, остановка выхода дожидается его и останавливает браузер.
        let core = env.core.clone();
        let deadline = quit + STOP_ALL_TIMEOUT;
        let stopping = tokio::spawn(async move { stop_all_until(&core, deadline).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        hold.notify_one();
        stopping.await.unwrap();
        let total = quit.elapsed();
        assert!(total < STOP_ALL_TIMEOUT, "выход занял {total:?}");
        assert_eq!(env.core.supervisor.status(&profile.id), ProfileStatus::Stopped);
        // Обработчик идущего запроса не оборван: скрипт получил ответ.
        let answer: Value = request.await.unwrap().expect("ответ на запуск пришёл").json().await.unwrap();
        assert_eq!(answer["code"], 0, "{answer}");
    }

    /// «Остановить все» и «Остановить и выйти» без Tauri: при запускающемся профиле вопроса нет, Local API
    /// закрывается, профили остановлены, интерфейс последним получил `off`.
    #[tokio::test]
    async fn stop_everything_closes_api_and_stops_profiles() {
        let env = api_env().await;
        let port = closed_port().await;
        env.core.store.set_api_port(port).unwrap();
        let statuses: Arc<Mutex<Vec<ApiStatus>>> = Arc::default();
        let seen = statuses.clone();
        let sink: StatusSink = Arc::new(move |status: ApiStatus| seen.lock().unwrap().push(status));
        let api = ApiController::new(env.core.clone(), sink);
        api.start().await;
        assert_eq!(api.status().await, ApiStatus::Listening { port });
        let profile = create_with_proxy(&env.core, "Выход", env.proxy_port, 1);
        env.core.supervisor.start(&profile.id).await.unwrap();
        assert_eq!(env.core.quit_plan(), QuitPlan::Ask, "по умолчанию — вопрос");

        // Второй профиль ещё запускается: браузер не ответил.
        let starting = create_with_proxy(&env.core, "Запуск", env.proxy_port, 2);
        let hold = Arc::new(Notify::new());
        *env.core.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
        let core = env.core.clone();
        let id = starting.id.clone();
        let launch = tokio::spawn(async move { core.supervisor.start(&id).await });
        wait_until("запуск дошёл до движка", || env.core.engine.launches() == 2).await;
        assert_eq!(env.core.engine.launches(), 2, "второй запуск дошёл до движка и ждёт");
        env.core.set_on_quit(QuitAction::StopAll).unwrap();
        assert_eq!(env.core.quit_plan(), QuitPlan::StopAll, "«Остановить все» при запускающемся профиле — без вопроса");
        hold.notify_one();
        launch.await.unwrap().unwrap();

        stop_everything(Some(&api), Some(env.core.as_ref()), Instant::now() + STOP_ALL_TIMEOUT).await;
        assert_eq!(api.status().await, ApiStatus::Off);
        assert!(port_closes(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await, "Local API не принимает соединения");
        assert_eq!(env.core.supervisor.status(&profile.id), ProfileStatus::Stopped);
        assert_eq!(env.core.supervisor.status(&starting.id), ProfileStatus::Stopped);
        assert_eq!(statuses.lock().unwrap().last(), Some(&ApiStatus::Off));
        assert_eq!(env.core.quit_plan(), QuitPlan::Exit, "активных нет — выход без вопроса");
    }

    #[test]
    fn exit_deadline_is_set_once() {
        let exit = ExitDeadline::default();
        assert_eq!(exit.remaining(), None, "выход не начат");
        let before = Instant::now();
        let deadline = exit.start();
        assert!(deadline >= before + STOP_ALL_TIMEOUT);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(exit.start(), deadline, "второй выход не продлевает срок");
        let left = exit.remaining().expect("выход начат");
        assert!(left <= STOP_ALL_TIMEOUT - Duration::from_millis(20), "{left:?}");
    }

    /// Cmd+Q до подписки интерфейса на `quit-requested`: вопрос откладывается и задаётся один раз после `ui_ready`.
    #[test]
    fn quit_question_waits_for_the_interface() {
        let gate = QuitGate::default();
        assert!(!gate.ask_or_defer(), "интерфейс не подписан — вопрос отложен");
        assert!(!gate.ask_or_defer(), "повторный Cmd+Q — тот же отложенный вопрос");
        assert!(gate.ready(), "после подписки вопрос задаётся");
        assert!(!gate.ready(), "один раз");
        assert!(gate.ask_or_defer(), "интерфейс готов — вопрос сразу");
        assert!(!gate.ready(), "заданный сразу вопрос не откладывается");
        assert!(!QuitGate::default().ready(), "без Cmd+Q вопроса нет");
    }

    #[tokio::test]
    async fn stop_all_until_stops_running_profiles() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "Работает", e.proxy_port, 1);
        e.core.supervisor.start(&profile.id).await.unwrap();
        stop_all_until(&e.core, Instant::now() + STOP_ALL_TIMEOUT).await;
        assert_eq!(e.core.supervisor.status(&profile.id), ProfileStatus::Stopped);
        assert_eq!(e.core.supervisor.active_count(), 0);
    }

    /// Dock и системный выход без запущенных профилей: `stop_all` всё равно закрывает супервизор — запуск,
    /// уже вошедший в обработчик Local API, не стартует в последние миллисекунды.
    #[tokio::test]
    async fn exit_closes_the_supervisor_even_without_profiles() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "Поздний", e.proxy_port, 1);
        assert_eq!(e.core.supervisor.active_count(), 0);
        stop_all_until(&e.core, Instant::now() + STOP_ALL_TIMEOUT).await;
        let late = e.core.supervisor.start(&profile.id).await;
        assert!(matches!(late, Err(SupervisorError::Cancelled)), "{late:?}");
        assert_eq!(e.core.engine.launches(), 0);
    }

    /// Общий срок: зависшая остановка держит выход только до срока, вторая (`RunEvent::Exit` после «Остановить все»,
    /// повторный Cmd+Q) при вышедшем сроке не ждёт вовсе. Брошенная остановка доходит до конца сама.
    #[tokio::test]
    async fn exit_waits_only_until_the_shared_deadline() {
        let e = env().await;
        let profile = create_with_proxy(&e.core, "Завис", e.proxy_port, 1);
        e.core.supervisor.start(&profile.id).await.unwrap();
        let hold = Arc::new(Notify::new());
        *e.core.engine.hold_next_stop.lock().unwrap() = Some(hold.clone());

        let began = Instant::now();
        let deadline = began + Duration::from_millis(300);
        stop_all_until(&e.core, deadline).await;
        let first = began.elapsed();
        assert!(first >= Duration::from_millis(300) && first < Duration::from_secs(2), "первая остановка — до срока: {first:?}");

        let again = Instant::now();
        stop_all_until(&e.core, deadline).await;
        assert!(again.elapsed() < Duration::from_millis(200), "срок вышел — вторая остановка не ждёт: {:?}", again.elapsed());

        hold.notify_one();
        wait_until("профиль остановлен", || e.core.supervisor.status(&profile.id) == ProfileStatus::Stopped).await;
        assert_eq!(e.core.supervisor.status(&profile.id), ProfileStatus::Stopped);
    }

    /// Ядро открывается в `setup`: до этого команды отвечают «ядро ещё не открылось», после — ядром; второй раз не ставится.
    #[test]
    fn commands_wait_for_the_core_opened_in_setup() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let state = AppState::new(paths.clone());
        let early = state.core().err().unwrap();
        assert_eq!(early.code, "SecurityLocked");
        assert_eq!(state.api().err().unwrap().code, "SecurityLocked");
        assert_eq!(state.info().init_error, None, "блокировка — обычный экран входа, не ошибка данных");

        let core = Arc::new(Core::new(
            paths.clone(),
            Arc::new(memory_store()),
            Arc::new(OmbraEngine::new(paths.engines())),
            Arc::new(GeoClient::new()),
            Arc::new(|_: Event| {}),
            Timings::default(),
        ));
        state.set_core(Ok(core.clone()));
        assert!(Arc::ptr_eq(state.core().unwrap(), &core));
        assert_eq!(state.info().init_error, None);
        state.set_core(Err(CommandError::new("Io", "поздно")));
        assert!(state.core().is_ok(), "ядро ставится один раз");
    }

    /// Предложение включить 2FA не запрещает открыть базу и использовать команды.
    #[test]
    fn starting_without_2fa_opens_the_native_core_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let state = AppState::new(paths.clone());
        state.initialize_security(Arc::new(|_| {}));
        assert!(state.core().is_ok());
        assert!(paths.db().exists());
        assert_eq!(state.info().init_error, None);
        assert!(!state.with_security(|security| Ok(security.status(unix_now()).configured)).unwrap());
    }

    /// Ошибка блокировки папки данных — на экран ошибки интерфейса через `app_info`.
    #[test]
    fn failed_lock_reaches_the_error_screen() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(Paths::new(dir.path()));
        state.set_core(Err(CommandError::new("Io", "не удалось заблокировать папку данных")));
        let info = state.info();
        assert_eq!(info.init_error.map(|e| e.code), Some("Io".to_string()));
        assert_eq!(info.data_dir, dir.path().display().to_string());
    }

    #[test]
    fn event_names_are_kebab_case() {
        for name in [SUPERVISOR_EVENT, QUIT_REQUESTED_EVENT, API_STATUS_EVENT, PROFILES_CHANGED_EVENT, TASKS_CHANGED_EVENT, FLOW_DEBUG_EVENT, SYNC_EVENT] {
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c == '-'), "{name}");
        }
        assert_eq!(API_STATUS_EVENT, "api-status");
        assert_eq!(PROFILES_CHANGED_EVENT, "profiles-changed");
    }

    #[test]
    fn profiles_changed_goes_to_its_own_channel() {
        assert_eq!(event_channel(&Event::ProfilesChanged), PROFILES_CHANGED_EVENT);
        let status = Event::Status { profile_id: "k7f2q9".into(), status: ProfileStatus::Stopped };
        assert_eq!(event_channel(&status), SUPERVISOR_EVENT);
        assert_eq!(event_channel(&Event::GatewayUnreachable { profile_id: "k7f2q9".into() }), SUPERVISOR_EVENT);
    }

    #[test]
    fn sync_changed_goes_to_its_own_channel() {
        use crate::sync::session::SyncState;
        assert_eq!(event_channel(&Event::SyncChanged(SyncState::idle())), SYNC_EVENT);
        assert_eq!(SYNC_EVENT, "sync");
    }

    #[test]
    fn tasks_changed_goes_to_its_own_channel() {
        assert_eq!(event_channel(&Event::TasksChanged { task_id: 1, error: None }), TASKS_CHANGED_EVENT);
        assert_eq!(TASKS_CHANGED_EVENT, "tasks-changed");
    }

    #[test]
    fn flow_debug_goes_to_its_own_channel() {
        use crate::supervisor::{DebugEvent, DebugKind};
        let event = Event::FlowDebug(DebugEvent { run_id: "d".into(), kind: DebugKind::End, node: None, entry: None, status: None, error: None });
        assert_eq!(event_channel(&event), FLOW_DEBUG_EVENT);
        assert_eq!(FLOW_DEBUG_EVENT, "flow-debug");
        assert_eq!(serde_json::to_value(&event).unwrap()["phase"], "end");
    }

    #[test]
    fn api_status_serializes_for_the_interface() {
        assert_eq!(serde_json::to_value(ApiStatus::Off).unwrap(), json!({"state": "off"}));
        assert_eq!(
            serde_json::to_value(ApiStatus::Listening { port: 50325 }).unwrap(),
            json!({"state": "listening", "port": 50325})
        );
        assert_eq!(
            serde_json::to_value(ApiStatus::Failed {
                port: 50325,
                code: ApiFailureCode::PortBusy,
                reason: "порт 50325 занят".into()
            })
            .unwrap(),
            json!({"state": "failed", "port": 50325, "code": "port_busy", "reason": "порт 50325 занят"})
        );
        assert_eq!(serde_json::to_value(ApiFailureCode::SettingsUnreadable).unwrap(), json!("settings_unreadable"));
        assert_eq!(serde_json::to_value(ApiFailureCode::Io).unwrap(), json!("io"));
    }
}
