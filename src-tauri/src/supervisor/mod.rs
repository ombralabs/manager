//! Жизнь профиля: запуск через прослойку, слежение, остановка, уборка (спецификация этапа 1, «Супервизор»).
pub mod gateway;
pub mod pidfile;
mod status;
#[cfg(test)]
pub(crate) mod test_engine;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod live_tests;

pub use status::{DebugEvent, DebugKind, Event, EventSink, ProfileStatus, RunningInfo, StatusFilter};

use crate::cookies::{Cookie, set_hash};
use crate::engine::args::ArgsError;
use crate::engine::cdp::{CdpClient, CdpError};
use crate::engine::process::{self, Probe};
use crate::engine::{Engine, EngineError, EngineProcess, ExitInfo, HumanizeLaunch, LaunchHooks, LaunchRequest};
use crate::fingerprint::{self, Fingerprint, GeoFacts};
use crate::lock::{DirLock, LockError};
use crate::paths::Paths;
use crate::relay::geo::{GeoClient, GeoError, GeoInfo};
use crate::relay::{self, Health, RelayHandle};
use crate::store::cookies::MergeCount;
use crate::store::profiles::ProfileRecord;
use crate::store::{Store, StoreError};
use crate::sys::unix_now;
use pidfile::PidRecord;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::{OwnedMutexGuard, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

/// Сколько выход из приложения ждёт `stop_all` (решение заказчика 01.10.2026: не дольше 15 с).
pub const STOP_ALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Сколько ждать последний снимок cookies при остановке. Остановка на выходе укладывается в `STOP_ALL_TIMEOUT`:
/// до 2 с на снимок, до 10 с на `stop_grace` и до 3 с на уборку помощников Chromium (`settle`).
const FINAL_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(2);

/// Как часто `start_or_attach` смотрит на исход чужого запуска или остановки.
const ATTACH_POLL: Duration = Duration::from_millis(50);

/// Параметры одного запуска (Local API): профиль они не меняют.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartOptions {
    /// Браузер без окна.
    pub headless: bool,
    /// Заменяет дополнительные аргументы профиля на этот запуск.
    pub extra_args: Option<Vec<String>>,
    /// Восстанавливать вкладки прошлой сессии; `None` — как в профиле.
    pub restore_tabs: Option<bool>,
    /// Стартовые адреса на этот запуск; пустой список подавляет адреса профиля.
    pub start_urls: Option<Vec<String>>,
    /// После этого сеанса (остановка, падение, закрытие пользователем) удалить кэш профиля.
    pub clear_cache_on_exit: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// Сколько ждать браузер после SIGTERM, прежде чем SIGKILL.
    pub stop_grace: Duration,
    /// То же для «сирот» прошлого запуска приложения.
    pub orphan_grace: Duration,
    /// Сколько ждать помощников Chromium после падения главного процесса.
    pub settle: Duration,
    /// Сколько запуски и удаление ждут уборку прошлого запуска приложения.
    pub ready_wait: Duration,
    /// Период снимка cookies работающего браузера.
    pub cookie_interval: Duration,
    /// Период опроса `/json/version` работающего браузера: не потерял ли он поток лицензии.
    pub license_poll: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            stop_grace: Duration::from_secs(10),
            orphan_grace: Duration::from_secs(10),
            settle: Duration::from_secs(3),
            ready_wait: Duration::from_secs(30),
            cookie_interval: Duration::from_secs(5),
            license_poll: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("профиль не найден")]
    NotFound,
    #[error("профиль уже запущен")]
    AlreadyRunning,
    #[error("профиль не запущен")]
    NotRunning,
    #[error("запуск отменён остановкой")]
    Cancelled,
    #[error("профиль всё ещё запускается")]
    StillStarting,
    #[error("прежний браузер ждёт лицензию; обновите браузер для работы без ключа")]
    LicenseWaiting,
    #[error("профиль запущен — сначала остановите его")]
    ProfileRunning,
    #[error("папка профиля занята другим процессом")]
    ProfileLocked,
    #[error("движок не установлен")]
    EngineMissing,
    #[error("нет свободных включённых прокси")]
    NoFreeProxy,
    #[error("прокси используется запущенным профилем")]
    ProxyInUse,
    #[error("прокси недоступен")]
    ProxyUnavailable,
    #[error("сервисы геоданных недоступны")]
    GeoUnavailable,
    #[error("отпечаток профиля повреждён")]
    BadFingerprint,
    #[error(transparent)]
    Args(ArgsError),
    #[error("браузер не запустился: {0}")]
    Launch(EngineError),
    #[error("остановка браузера: {0}")]
    Stop(EngineError),
    #[error("cookies: {0}")]
    Cookies(String),
    #[error(transparent)]
    Store(StoreError),
    #[error("файл: {0}")]
    Io(#[from] std::io::Error),
}

impl SupervisorError {
    /// Стабильная строка для перевода в интерфейсе и для Local API на этапе 2.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "NotFound",
            Self::AlreadyRunning => "AlreadyRunning",
            Self::NotRunning => "NotRunning",
            // Для интерфейса и Local API отменённый запуск — «профиль не запущен».
            Self::Cancelled => "NotRunning",
            // Local API дождался не всего запуска: для скрипта это «профиль уже запускается».
            Self::StillStarting => "AlreadyRunning",
            Self::LicenseWaiting => "LicenseWaiting",
            Self::ProfileRunning => "ProfileRunning",
            Self::ProfileLocked => "ProfileLocked",
            Self::EngineMissing => "EngineMissing",
            Self::NoFreeProxy => "NoFreeProxy",
            Self::ProxyInUse => "ProxyInUse",
            Self::ProxyUnavailable => "ProxyUnavailable",
            Self::GeoUnavailable => "GeoUnavailable",
            Self::BadFingerprint => "BadFingerprint",
            Self::Args(_) => "BadArgs",
            Self::Launch(EngineError::AccessRequired) => "BrowserAccessRequired",
            Self::Launch(EngineError::AccessInvalid) => "BrowserAccessInvalid",
            Self::Launch(EngineError::FreeDeviceBound { .. }) => "FreeDeviceBound",
            Self::Launch(EngineError::DeviceIdentityUnavailable) => "DeviceIdentityUnavailable",
            Self::Launch(_) => "LaunchFailed",
            Self::Stop(_) => "StopFailed",
            Self::Cookies(_) => "CookiesFailed",
            Self::Store(StoreError::Crypto) => "SecretsUnreadable",
            Self::Store(_) => "Storage",
            Self::Io(_) => "Io",
        }
    }
}

impl From<StoreError> for SupervisorError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(_) => Self::NotFound,
            other => Self::Store(other),
        }
    }
}

impl From<EngineError> for SupervisorError {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::NotInstalled => Self::EngineMissing,
            EngineError::Args(args) => Self::Args(args),
            other => Self::Launch(other),
        }
    }
}

/// Держит очередь операций профиля (для удаления) и блокировку его папки: профиль не запущен ни в этом процессе,
/// ни в другом процессе Ombra (`ombra flow` рядом с окном) и не запустится, пока охрана жива.
pub struct IdleGuard {
    dir: Option<DirLock>,
    _op: OwnedMutexGuard<()>,
}

pub struct Supervisor<E: Engine> {
    inner: Arc<Inner<E>>,
}

impl<E: Engine> Clone for Supervisor<E> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

/// Порядок блокировок: очередь профиля (`ops`, tokio) → мьютекс состояния (`state`) → мьютекс базы (`Store`).
/// Несколько очередей сразу берёт только `lock_idle_many` — по возрастанию id.
/// Методы `Store` не обращаются к супервизору, обратного вложения нет. `state` никогда не держится через `.await`.
struct Inner<E: Engine> {
    paths: Paths,
    store: Arc<Store>,
    engine: Arc<E>,
    geo: Arc<GeoClient>,
    sink: EventSink,
    timings: Timings,
    state: Mutex<State>,
    ops: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// `false`, пока не убраны браузеры из PID-файлов прошлого запуска: до этого запуски ждут.
    ready: watch::Sender<bool>,
}

#[derive(Default)]
struct State {
    statuses: HashMap<String, ProfileStatus>,
    active: HashMap<String, Active>,
    /// Прокси → профиль, который его занимает, и блокировка прокси в папке данных: от выбора шлюза при запуске до
    /// остановки или выхода браузера. Блокировку видят другие процессы Ombra (окно рядом с `ombra flow`).
    reserved: HashMap<i64, (String, DirLock)>,
    /// «Стоп» пришёл, пока запуск ждал уборку или очередь: запуск не выполняется.
    stop_requested: HashSet<String>,
    /// Запуск идёт: очередь профиля взята, пометка остановки проверена. «Стоп» в это время отменяет его этим токеном
    /// (до ответа браузера `200`) и ждёт в очереди. Убирается под тем же мьютексом, где статус уходит из «запускается»
    /// или «ждёт поток».
    launching: HashMap<String, CancellationToken>,
    /// Идёт выход из приложения (`stop_all`): новые запуски не выполняются. Обратно не снимается.
    closing: bool,
}

/// Всё, что держит запущенный профиль; отдаёт либо остановка, либо уборка после падения — кто первый заберёт.
struct Active {
    browser: EngineProcess,
    relay: RelayHandle,
    lock: DirLock,
    /// Останавливает слежение за браузером и снимки cookies.
    watcher: CancellationToken,
    /// Снимки cookies раз в `cookie_interval`.
    sync: JoinHandle<()>,
    /// Сеанс запущен с `clear_cache_on_exit`: после закрытия браузера кэш профиля удаляется.
    clear_cache_on_exit: bool,
    /// Браузер с окнами (не `headless`): без страниц он не отдаёт cookies.
    windowed: bool,
}

/// Браузер профиля глазами `start_or_attach`.
enum BrowserState {
    /// «Работает» или «пауза», браузер у супервизора.
    Open(RunningInfo),
    /// Статус ещё «работает» или «пауза», но браузер уже забрала остановка или уборка после выхода.
    Closing,
    /// Запускается или ждёт поток лицензии (при запуске или потеряв его): исход ждёт `wait_started`.
    Starting,
    Closed,
}

/// Задача запуска оборвалась паникой — уборка неудачного запуска всё равно выполняется (`launch_failed`).
/// Иначе профиль навсегда остался бы «запускается», когда вызвавший уже не ждёт (Local API: клиент оборвал соединение).
struct LaunchGuard<E: Engine> {
    inner: Arc<Inner<E>>,
    id: String,
    armed: bool,
}

impl<E: Engine> LaunchGuard<E> {
    fn new(inner: Arc<Inner<E>>, id: String) -> Self {
        Self { inner, id, armed: true }
    }

    /// Запуск закончился штатно — успехом или ошибкой, которую уже убрали: уборка не нужна.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl<E: Engine> Drop for LaunchGuard<E> {
    fn drop(&mut self) {
        if self.armed {
            self.inner.launch_task_died(&self.id);
        }
    }
}

/// Выбранный и проверенный шлюз: прокси зарезервирован, прослойка слушает.
struct Gateway {
    proxy_id: i64,
    relay: RelayHandle,
    geo: Option<GeoInfo>,
    /// У профиля был закреплённый прокси, а запускается другой.
    replaced: bool,
}

/// Сбой снимка cookies. В тексте нет значений cookies: только причина от CDP или базы.
#[derive(Debug, thiserror::Error)]
enum SnapshotError {
    #[error(transparent)]
    Cdp(#[from] CdpError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

fn cookies_error(e: CdpError) -> SupervisorError {
    SupervisorError::Cookies(e.to_string())
}

impl<E: Engine> Supervisor<E> {
    pub fn new(
        paths: Paths,
        store: Arc<Store>,
        engine: Arc<E>,
        geo: Arc<GeoClient>,
        sink: EventSink,
        timings: Timings,
    ) -> Self {
        let leftovers = pidfile::list(&paths.run_dir()).map(|found| !found.is_empty()).unwrap_or(true);
        let (ready, _) = watch::channel(!leftovers);
        Self {
            inner: Arc::new(Inner {
                paths,
                store,
                engine,
                geo,
                sink,
                timings,
                state: Mutex::default(),
                ops: Mutex::default(),
                ready,
            }),
        }
    }

    /// Живая статистика прослойки работающего профиля и момент её запуска (мс Unix).
    pub fn relay_stats(&self, id: &str) -> Option<(relay::StatsSnapshot, u128)> {
        self.inner.lock_state().active.get(id).map(|a| (a.relay.stats(), a.relay.started_ms()))
    }

    pub fn status(&self, id: &str) -> ProfileStatus {
        self.inner.status(id)
    }

    /// Сколько профилей активны: «запускается», «работает» или «пауза».
    pub fn active_count(&self) -> usize {
        self.inner.lock_state().statuses.values().filter(|s| s.is_active()).count()
    }

    /// Приёмник событий: для задач, которые живут дольше вызова.
    pub fn sink(&self) -> EventSink {
        self.inner.sink.clone()
    }

    /// Событие для интерфейса не о статусе профиля — например, `ProfilesChanged` из Local API.
    pub fn emit(&self, event: Event) {
        (self.inner.sink)(event);
    }

    /// Прокси → профиль этого процесса, который его занимает (запускается, работает, на паузе).
    pub fn proxy_users(&self) -> HashMap<i64, String> {
        self.inner.lock_state().reserved.iter().map(|(proxy, (owner, _))| (*proxy, owner.clone())).collect()
    }

    /// Выполнить `f` (удаление прокси), пока ни один из прокси никем не занят — ни в этом процессе, ни в другом
    /// процессе Ombra (блокировка прокси). Занятость проверяется под тем же мьютексом, под которым запуск выбирает
    /// шлюз, а блокировки держатся до конца `f`: между проверкой и `f` прокси никто не займёт, пачка удаляется вся
    /// или никак. `f` выполняется под мьютексом состояния: в ней можно звать `Store`, но не супервизор.
    pub fn while_proxies_idle<T>(&self, proxy_ids: &[i64], f: impl FnOnce() -> T) -> Result<T, SupervisorError> {
        let state = self.inner.lock_state();
        if proxy_ids.iter().any(|id| state.reserved.contains_key(id)) {
            return Err(SupervisorError::ProxyInUse);
        }
        let mut locks = Vec::with_capacity(proxy_ids.len());
        for id in proxy_ids {
            match DirLock::acquire(&self.inner.paths.proxy_lock_dir(*id)) {
                Ok(lock) => locks.push(lock),
                Err(LockError::Busy) => return Err(SupervisorError::ProxyInUse),
                Err(LockError::Io(e)) => return Err(SupervisorError::Io(e)),
            }
        }
        let result = f();
        drop(locks);
        drop(state);
        Ok(result)
    }

    /// Старт приложения: браузеры из `run/*.pid` завершаются (SIGTERM, затем SIGKILL), файлы удаляются.
    pub async fn cleanup_orphans(&self) -> std::io::Result<Vec<String>> {
        let result = self.inner.cleanup_orphans().await;
        self.inner.ready.send_replace(true);
        result
    }

    pub async fn start(&self, id: &str) -> Result<RunningInfo, SupervisorError> {
        self.start_with(id, StartOptions::default()).await
    }

    /// Запуск с параметрами одного сеанса (Local API): профиль они не меняют.
    pub async fn start_with(&self, id: &str, options: StartOptions) -> Result<RunningInfo, SupervisorError> {
        // Профиль и настройка читаются до мьютекса состояния: база не берётся под ним без нужды.
        // Профиля нет — «запускается» не появляется: интерфейсу и Local API незачем видеть запуск удалённого профиля.
        if self.inner.store.get_profile(id)?.is_none() {
            return Err(SupervisorError::NotFound);
        }
        {
            let mut state = self.inner.lock_state();
            // Выход из приложения уже собрал список на остановку: запущенный сейчас браузер пережил бы Ombra.
            if state.closing {
                return Err(SupervisorError::Cancelled);
            }
            if state.statuses.get(id).is_some_and(ProfileStatus::is_active) {
                return Err(SupervisorError::AlreadyRunning);
            }
            // Профиль не активен — значит, ни одного запуска в пути нет и старая пометка ничего не значит.
            state.stop_requested.remove(id);
            state.statuses.insert(id.to_string(), ProfileStatus::Starting);
        }
        self.inner.emit_status(id, ProfileStatus::Starting);
        // Дальше всё идёт в отдельной задаче: если вызывающий бросил ожидание, запуск всё равно доходит до конца.
        let inner = self.inner.clone();
        let id = id.to_string();
        let task = tokio::spawn(async move {
            // Паника внутри задачи не оставит профиль «запускается»: уборку сделает защитный объект.
            let guard = LaunchGuard::new(inner.clone(), id.clone());
            // Уборка прошлого запуска — до очереди: «стоп» в это время отменяет запуск.
            inner.wait_ready().await;
            let _op = inner.op_lock(&id).lock_owned().await;
            let result = inner.clone().launch(&id, &options).await;
            if result.is_err() {
                inner.launch_failed(&id);
            }
            guard.disarm();
            result
        });
        // Задача оборвалась паникой: уборку уже сделал её защитный объект.
        task.await
            .unwrap_or_else(|e| Err(SupervisorError::Io(std::io::Error::other(format!("задача запуска прервана: {e}")))))
    }

    /// Local API: профиль работает или на паузе — данные его браузера, нового запуска нет; запускается — ждём исхода
    /// этого запуска; браузер закрывается — ждём конца остановки и запускаем заново; иначе — запуск с `options`.
    /// Всё ожидание — не дольше `wait`; свой запуск, который к сроку ждёт поток лицензии, отменяется — `LicenseWaiting`.
    ///
    /// Закрывающийся браузер не отдаётся: скрипт, который закрыл браузер (`browser.close()`) и сразу запускает профиль
    /// снова, получил бы адрес, по которому уже никто не слушает. Закрывается — значит, статус ещё «работает», но
    /// браузер уже забрала остановка или уборка после выхода (его нет в `active`), либо порт отладки уже не принимает
    /// соединения (`Browser.close` получен, процесс ещё не вышел).
    pub async fn start_or_attach(
        &self,
        id: &str,
        options: StartOptions,
        wait: Duration,
    ) -> Result<RunningInfo, SupervisorError> {
        self.start_or_attach_owned(id, options, wait).await.map(|(info, _)| info)
    }

    /// То же, что `start_or_attach`, плюс признак «запущен этим вызовом» (`true` — `start_with` вернул `Ok`): только
    /// такой браузер вызывающий вправе потом закрыть. Открытый, чужой запуск и дождавшийся его — `false`.
    pub async fn start_or_attach_owned(
        &self,
        id: &str,
        options: StartOptions,
        wait: Duration,
    ) -> Result<(RunningInfo, bool), SupervisorError> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match self.inner.browser_state(id) {
                BrowserState::Open(info) => {
                    if self.inner.engine.debug_port_answers(info.debug_port).await {
                        return Ok((info, false));
                    }
                }
                BrowserState::Closing => {}
                BrowserState::Starting | BrowserState::Closed => break,
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(SupervisorError::StillStarting);
            }
            tokio::time::sleep(ATTACH_POLL).await;
        }
        let started = tokio::select! {
            biased;
            started = self.start_with(id, options) => started,
            () = async {
                tokio::time::sleep_until(deadline).await;
                self.until_waiting(id).await;
            } => {
                // Срок вышел, а браузер ждёт поток: скрипт получит ошибку — браузер, который возьмёт поток позже
                // (и займёт его у других), ему не нужен (решение заказчика).
                if let Err(e) = self.stop(id).await {
                    log::warn!("профиль {id}: запуск, не дождавшийся потока, не остановлен: {e}");
                }
                return Err(SupervisorError::LicenseWaiting);
            }
        };
        match started {
            Ok(info) => Ok((info, true)),
            Err(SupervisorError::AlreadyRunning) => self.wait_started(id, deadline).await.map(|info| (info, false)),
            Err(e) => Err(e),
        }
    }

    /// Ждёт, пока профиль «ждёт поток» лицензии: запуск дальше идёт сам, в своей задаче (`start_with`).
    // ponytail: опрос раз в `ATTACH_POLL` — «стоп» быстрее него массовый запуск сочтёт сбоем; точнее — сигнал из `on_license`.
    pub async fn until_waiting(&self, id: &str) {
        while !matches!(self.status(id), ProfileStatus::Waiting { .. }) {
            tokio::time::sleep(ATTACH_POLL).await;
        }
    }

    /// Данные открытого браузера профиля: «работает» или «пауза», и браузер ещё у супервизора. Закрывающийся браузер
    /// (остановка или уборка уже забрали его, статус ещё не сменился) — `None`: Local API, как и `start_or_attach`,
    /// не отдаёт его старый адрес.
    pub fn running_info(&self, id: &str) -> Option<RunningInfo> {
        match self.inner.browser_state(id) {
            BrowserState::Open(info) => Some(info),
            BrowserState::Closing | BrowserState::Starting | BrowserState::Closed => None,
        }
    }

    /// Все открытые браузеры («работает» и «пауза», без закрывающихся), по id.
    pub fn running(&self) -> Vec<(String, RunningInfo)> {
        let state = self.inner.lock_state();
        let mut list: Vec<(String, RunningInfo)> = state
            .statuses
            .iter()
            .filter(|(id, _)| state.active.contains_key(*id))
            .filter_map(|(id, status)| match status {
                ProfileStatus::Running(info) | ProfileStatus::Paused(info) => Some((id.clone(), info.clone())),
                _ => None,
            })
            .collect();
        drop(state);
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }

    /// Исход чужого запуска: «работает» или «пауза» — данные; запуск не удался — `NotRunning`;
    /// не дождались к `deadline` — `StillStarting`, а если браузер ждёт поток лицензии — `LicenseWaiting`.
    async fn wait_started(&self, id: &str, deadline: tokio::time::Instant) -> Result<RunningInfo, SupervisorError> {
        loop {
            let waiting = match self.status(id) {
                ProfileStatus::Running(info) | ProfileStatus::Paused(info) => return Ok(info),
                ProfileStatus::Starting => false,
                ProfileStatus::Waiting { .. } => true,
                _ => return Err(SupervisorError::NotRunning),
            };
            if tokio::time::Instant::now() >= deadline {
                return Err(if waiting { SupervisorError::LicenseWaiting } else { SupervisorError::StillStarting });
            }
            tokio::time::sleep(ATTACH_POLL).await;
        }
    }

    pub async fn stop(&self, id: &str) -> Result<(), SupervisorError> {
        let cancelled = {
            let mut state = self.inner.lock_state();
            // Запуск ещё ждёт уборку или очередь: он отменяется пометкой, «стоп» в очередь не встаёт.
            if matches!(state.statuses.get(id), Some(ProfileStatus::Starting)) && !state.launching.contains_key(id) {
                state.stop_requested.insert(id.to_string());
                return Ok(());
            }
            // Запуск идёт: его браузер может ждать поток лицензии сколько угодно долго — запуск отменяется, «стоп»
            // ждёт в очереди только его уборку. Браузер, уже ответивший `200`, токен не читает: его «стоп» останавливает.
            if let Some(cancel) = state.launching.get(id) {
                cancel.cancel();
                true
            } else {
                false
            }
        };
        // Остановка идёт в отдельной задаче: брошенный вызов не оставляет профиль «работающим» наполовину.
        // Начатый запуск держит очередь: «стоп» дожидается его и останавливает уже запущенный браузер.
        let inner = self.inner.clone();
        let id = id.to_string();
        let task = tokio::spawn(async move {
            let _op = inner.op_lock(&id).lock_owned().await;
            let Some(active) = inner.lock_state().active.remove(&id) else {
                // Отменённый запуск уже убран и сказал «остановлен».
                return if cancelled { Ok(()) } else { Err(SupervisorError::NotRunning) };
            };
            inner.stop_active(&id, active).await
        });
        task.await
            .unwrap_or_else(|e| Err(SupervisorError::Io(std::io::Error::other(format!("задача остановки прервана: {e}")))))
    }

    /// Выход из приложения: останавливает все запущенные и запускающиеся профили.
    /// Супервизор закрывается для запусков навсегда: `start` дальше отвечает `Cancelled` — иначе массовый запуск
    /// или Local API запустили бы браузер после того, как список на остановку собран. Звать только на выходе.
    pub async fn stop_all(&self) {
        let ids: Vec<String> = {
            let mut state = self.inner.lock_state();
            // Пометка и список — под одним мьютексом: запуск либо попал в список, либо увидит пометку.
            state.closing = true;
            state.statuses.iter().filter(|(_, status)| status.is_active()).map(|(id, _)| id.clone()).collect()
        };
        let mut tasks = tokio::task::JoinSet::new();
        for id in ids {
            let supervisor = self.clone();
            tasks.spawn(async move {
                if let Err(e) = supervisor.stop(&id).await {
                    log::warn!("профиль {id}: {e}");
                }
            });
        }
        while tasks.join_next().await.is_some() {}
    }

    /// Импорт под очередью профиля (не пересекается с запуском и остановкой).
    /// Работающему профилю — сначала в браузер: если тот не принял, база не меняется.
    /// Иначе следующий снимок стёр бы импорт. Второе значение — ушло ли в браузер.
    pub async fn import_cookies(&self, id: &str, cookies: Vec<Cookie>) -> Result<(MergeCount, bool), SupervisorError> {
        let inner = self.inner.clone();
        let id = id.to_string();
        let task = tokio::spawn(async move {
            let _op = inner.op_lock(&id).lock_owned().await;
            let ws = inner.lock_state().active.get(&id).map(|a| a.browser.ws_endpoint.clone());
            let sent = match ws {
                Some(ws) => {
                    let mut cdp = CdpClient::connect(&ws).await.map_err(cookies_error)?;
                    let report = cdp.set_cookies(&cookies).await.map_err(cookies_error)?;
                    if report.rejected > 0 {
                        log::warn!("профиль {id}: браузер не принял {} импортированных cookies", report.rejected);
                    }
                    true
                }
                None => false,
            };
            // Браузера нет у этого процесса: если профиль работает в другом, его снимок затёр бы импорт —
            // держим замок папки на время слияния (занят — отказ, ничего не меняется).
            // Уборка сирот прошлого запуска идёт первой: занятую нами папку она сочла бы живым владельцем.
            inner.wait_ready().await;
            let dir = inner.paths.profile_dir(&id);
            let _dir_lock = if !sent && dir.exists() { Some(acquire_dir_lock(dir).await?) } else { None };
            let count = inner.store.merge_cookies(&id, &cookies)?;
            // Тип ошибки задан явно: иначе `?` внутри async-блока не выводит его.
            Ok::<_, SupervisorError>((count, sent))
        });
        task.await
            .unwrap_or_else(|e| Err(SupervisorError::Io(std::io::Error::other(format!("задача импорта прервана: {e}")))))
    }

    pub async fn lock_idle(&self, id: &str) -> Result<IdleGuard, SupervisorError> {
        // Удаление ждёт уборку: иначе папку удалили бы под ещё живым браузером прошлого запуска.
        self.inner.wait_ready().await;
        let op = self.inner.op_lock(id);
        let guard = op.lock_owned().await;
        if self.status(id).is_active() {
            return Err(SupervisorError::ProfileRunning);
        }
        Ok(IdleGuard { dir: self.idle_dir_lock(id).await?, _op: guard })
    }

    /// Замки простоя пачки (удаление через Local API): очереди всех профилей по возрастанию id, повторы склеены —
    /// две пачки не ждут друг друга вечно. Активный профиль — `ProfileRunning`, папку держит другой процесс Ombra —
    /// `ProfileLocked`; в обоих случаях ни одного замка не держится.
    /// Проверка — до очередей (не ждать идущий запуск) и после них (запуск, вставший в очередь раньше, уже закончился;
    /// новый ждёт за замком и после удаления получает `NotFound`).
    pub async fn lock_idle_many(&self, ids: &[String]) -> Result<Vec<IdleGuard>, SupervisorError> {
        // Удаление ждёт уборку: иначе папку удалили бы под ещё живым браузером прошлого запуска.
        self.inner.wait_ready().await;
        let mut sorted: Vec<&str> = ids.iter().map(String::as_str).collect();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.iter().any(|id| self.status(id).is_active()) {
            return Err(SupervisorError::ProfileRunning);
        }
        let mut guards = Vec::with_capacity(sorted.len());
        for id in &sorted {
            guards.push(IdleGuard { dir: None, _op: self.inner.op_lock(id).lock_owned().await });
        }
        if sorted.iter().any(|id| self.status(id).is_active()) {
            return Err(SupervisorError::ProfileRunning);
        }
        for (guard, id) in guards.iter_mut().zip(&sorted) {
            guard.dir = self.idle_dir_lock(id).await?;
        }
        Ok(guards)
    }

    /// Блокировка папки профиля для охраны простоя: занята — профиль работает в другом процессе Ombra, `ProfileLocked`.
    /// Папки нет — держать нечего: `DirLock::acquire` создал бы её, в том числе для чужого id из Local API.
    async fn idle_dir_lock(&self, id: &str) -> Result<Option<DirLock>, SupervisorError> {
        let dir = self.inner.paths.profile_dir(id);
        if !dir.exists() {
            return Ok(None);
        }
        acquire_dir_lock(dir).await.map(Some)
    }

    /// Забыть удалённый профиль: статус, пометку остановки и его очередь операций.
    pub fn forget(&self, id: &str) {
        {
            let mut state = self.inner.lock_state();
            state.statuses.remove(id);
            state.stop_requested.remove(id);
        }
        self.inner.ops.lock().expect("мьютекс очередей отравлен").remove(id);
    }
}

impl<E: Engine> Inner<E> {
    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("мьютекс статусов отравлен")
    }

    fn status(&self, id: &str) -> ProfileStatus {
        self.lock_state().statuses.get(id).cloned().unwrap_or(ProfileStatus::Stopped)
    }

    /// Статус и запись браузера — под одним мьютексом: остановка и уборка забирают браузер из `active` раньше,
    /// чем меняют статус.
    /// Браузер, потерявший поток лицензии, скриптам не отдаётся: он ждёт поток, как запуск.
    fn browser_state(&self, id: &str) -> BrowserState {
        let state = self.lock_state();
        let held = state.active.contains_key(id);
        match state.statuses.get(id) {
            Some(ProfileStatus::Running(info) | ProfileStatus::Paused(info)) if held => BrowserState::Open(info.clone()),
            Some(ProfileStatus::Running(_) | ProfileStatus::Paused(_) | ProfileStatus::Waiting { info: Some(_), .. })
                if !held =>
            {
                BrowserState::Closing
            }
            Some(ProfileStatus::Starting | ProfileStatus::Waiting { .. }) => BrowserState::Starting,
            _ => BrowserState::Closed,
        }
    }

    fn op_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.ops.lock().expect("мьютекс очередей отравлен").entry(id.to_string()).or_default().clone()
    }

    fn emit_status(&self, id: &str, status: ProfileStatus) {
        (self.sink)(Event::Status { profile_id: id.to_string(), status });
    }

    fn set_status(&self, id: &str, status: ProfileStatus) {
        self.lock_state().statuses.insert(id.to_string(), status.clone());
        self.emit_status(id, status);
    }

    /// Браузер при запуске ждёт поток лицензии (или его состояние сменилось): «запускается» и «ждёт поток» → «ждёт
    /// поток» с этим состоянием.
    fn on_license(&self, id: &str, license: &str) {
        let next = ProfileStatus::Waiting { license: license.to_string(), info: None, owner_hint: None };
        {
            let mut state = self.lock_state();
            let Some(status) = state.statuses.get_mut(id) else { return };
            if !matches!(status, ProfileStatus::Starting | ProfileStatus::Waiting { info: None, .. }) {
                return;
            }
            *status = next.clone();
        }
        self.emit_status(id, next);
    }

    /// Запуск не удался, отменён или его задача оборвалась: конец запуска, снятие резерва прокси и «остановлен» —
    /// под одним мьютексом. Задача могла упасть с паникой под этим мьютексом — уборка всё равно проходит.
    /// Профиль удалили, пока запуск ждал очередь (удаление держит её, `forget` уже убрал статус), — запись статуса
    /// не возвращается.
    fn launch_failed(&self, id: &str) {
        let deleted = matches!(self.store.get_profile(id), Ok(None));
        self.end_failed_launch(id, false, deleted);
    }

    /// Задача запуска оборвалась паникой. Если браузер уже записан в `active`, запуск дошёл до «работает»
    /// (паника — позже): браузер работает под слежением, его забирают «стоп», выход или уборка после его выхода.
    fn launch_task_died(&self, id: &str) {
        if self.end_failed_launch(id, true, false) {
            log::error!("профиль {id}: задача запуска оборвалась, запуск считается неудачным");
        } else {
            log::error!("профиль {id}: задача запуска оборвалась после запуска браузера, браузер работает");
        }
    }

    /// Уборка неудачного запуска; `keep_running` — не трогать профиль, чей браузер уже в `active` (проверка — под тем же
    /// мьютексом); `deleted` — профиля больше нет: статус не записывается, интерфейс всё равно получает «остановлен».
    /// `false` — уборки не было.
    fn end_failed_launch(&self, id: &str, keep_running: bool, deleted: bool) -> bool {
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if keep_running && state.active.contains_key(id) {
                return false;
            }
            state.launching.remove(id);
            state.reserved.retain(|_, (owner, _)| owner != id);
            if deleted {
                state.statuses.remove(id);
            } else {
                state.statuses.insert(id.to_string(), ProfileStatus::Stopped);
            }
        }
        self.emit_status(id, ProfileStatus::Stopped);
        true
    }

    /// Ждёт уборку прошлого запуска, но не дольше `ready_wait`: зависшая уборка не должна блокировать запуски навсегда.
    async fn wait_ready(&self) {
        let mut ready = self.ready.subscribe();
        if tokio::time::timeout(self.timings.ready_wait, ready.wait_for(|done| *done)).await.is_err() {
            log::warn!(
                "уборка после прошлого запуска не закончилась за {:?}, операция идёт без неё",
                self.timings.ready_wait
            );
        }
    }

    /// Остановка по команде: последний снимок cookies, браузер, прослойка, блокировка, PID-файл, резерв прокси.
    /// При ошибке остановки PID-файл остаётся: уцелевших помощников Chromium найдёт уборка при следующем старте;
    /// кэш тогда тоже не трогается — браузер может быть жив.
    async fn stop_active(&self, id: &str, active: Active) -> Result<(), SupervisorError> {
        let Active { mut browser, relay, lock, watcher, mut sync, clear_cache_on_exit, windowed } = active;
        watcher.cancel();
        // Снимок по расписанию мог идти прямо сейчас: он не должен записать старый набор поверх последнего.
        // Запись в базу синхронная, прерывание срабатывает только на `.await` — набор не запишется наполовину.
        sync.abort();
        let _ = (&mut sync).await;
        self.final_snapshot(id, &browser.ws_endpoint, windowed).await;
        let result = self.engine.stop(&mut browser, self.timings.stop_grace).await;
        relay.shutdown().await;
        if result.is_ok() {
            // Профиль мог уйти в корзину, пока шёл сеанс. Запись — под блокировкой каталога.
            if let Ok(Some(profile)) = self.store.get_profile_any(id) {
                self.sync_login_data(id, &profile);
            }
        }
        drop(lock);
        if result.is_ok() {
            let _ = pidfile::remove(&self.paths.run_dir(), id);
            if clear_cache_on_exit {
                self.clear_cache_after_session(id).await;
            }
        }
        self.release_proxy(id);
        self.set_status(id, ProfileStatus::Stopped);
        match result {
            Ok(report) => {
                if !report.leftover_pids.is_empty() {
                    log::warn!("профиль {id}: после остановки добиты процессы {:?}", report.leftover_pids);
                }
                Ok(())
            }
            Err(e) => Err(SupervisorError::Stop(e)),
        }
    }

    /// Папки текущих версий расширений набора профиля. Папки нет или в пути запятая (разделитель списка флага) —
    /// расширение пропускается; в журнал — без имени и пути. Ошибка чтения набора не роняет запуск.
    fn extension_dirs(&self, id: &str, profile: &ProfileRecord) -> Vec<PathBuf> {
        let list = match self.store.category_extensions(profile.ext_category_id) {
            Ok(list) => list,
            Err(e) => {
                log::warn!("профиль {id}: набор расширений не прочитан: {e}");
                return Vec::new();
            }
        };
        list.into_iter()
            .filter_map(|(ext, version)| {
                let dir = self.paths.extension_dir(&ext, &version);
                let usable = dir.join("manifest.json").is_file() && !dir.to_string_lossy().contains(',');
                if !usable {
                    log::warn!("профиль {id}: расширение {ext} набора пропущено — нет папки или запятая в пути");
                }
                usable.then_some(dir)
            })
            .collect()
    }

    /// Логин и пароль профиля — в менеджер паролей Chromium. Файл небольшой, запись синхронная; в журнал — только
    /// номер профиля и вид ошибки.
    fn sync_login_data(&self, id: &str, profile: &ProfileRecord) {
        let password = match self.store.profile_secrets_each(id) {
            Ok((Ok(password), _)) => password,
            _ => {
                log::warn!("профиль {id}: пароль для автозаполнения не прочитан");
                return;
            }
        };
        if let Err(e) = crate::engine::login_data::sync(
            &self.paths.profile_dir(id),
            &profile.platform_domain,
            profile.username.as_deref(),
            password.as_ref(),
            unix_now(),
        ) {
            log::warn!("профиль {id}: автозаполнение не записано: {:?}", e.sqlite_error_code());
        }
    }

    async fn launch(self: Arc<Self>, id: &str, options: &StartOptions) -> Result<RunningInfo, SupervisorError> {
        let cancel = CancellationToken::new();
        let cancelled = {
            let mut state = self.lock_state();
            // «Стоп» пришёл, пока запуск ждал уборку или очередь.
            let cancelled = state.stop_requested.remove(id);
            if !cancelled {
                // Дальше «стоп» отменяет запуск токеном и ждёт его в очереди.
                state.launching.insert(id.to_string(), cancel.clone());
            }
            cancelled
        };
        if cancelled {
            log::info!("профиль {id}: запуск отменён остановкой");
            return Err(SupervisorError::Cancelled);
        }
        self.store.get_profile(id)?.ok_or(SupervisorError::NotFound)?;
        let lock = acquire_dir_lock(self.paths.profile_dir(id)).await?;
        // Другой процесс мог убрать профиль в корзину между чтением и замком папки: читаем заново, под замком.
        let profile = self.store.get_profile(id)?.ok_or(SupervisorError::NotFound)?;
        if self.engine.installed().is_none() {
            return Err(SupervisorError::EngineMissing);
        }
        let fingerprint =
            Fingerprint::from_json(&profile.fingerprint_json).map_err(|_| SupervisorError::BadFingerprint)?;
        // Закладки не должны ронять запуск: при ошибке — предупреждение без содержимого.
        match self.store.profile_bookmarks(id) {
            Ok(list) => {
                if let Err(e) = crate::engine::bookmarks::write_ombra_folder(&self.paths.profile_dir(id), &list) {
                    log::warn!("профиль {id}: закладки не записаны: {e}");
                }
            }
            Err(e) => log::warn!("профиль {id}: закладки не прочитаны: {e}"),
        }
        self.sync_login_data(id, &profile);
        let gateway = self.choose_gateway(id, &profile, &fingerprint).await?;
        let (browser, info) = match self.launch_through(id, &profile, &fingerprint, &gateway, options, cancel).await {
            Ok(parts) => parts,
            Err(e) => {
                gateway.relay.shutdown().await;
                self.release_proxy(id);
                return Err(e);
            }
        };
        let Gateway { proxy_id, relay, replaced, .. } = gateway;
        if proxy_id > 0 && let Err(e) = self.store.record_gateway(id, proxy_id, replaced.then_some(info.started_at)) {
            log::warn!("профиль {id}: шлюз не записан: {e}");
        }
        if let Err(e) = self.store.touch_last_opened(id, info.started_at) {
            log::warn!("профиль {id}: время запуска не записано: {e}");
        }
        let watcher = CancellationToken::new();
        // `--headless` бывает и в аргументах запуска — профиля или `launch_args` Local API.
        let args = options.extra_args.as_ref().unwrap_or(&profile.extra_args);
        let windowed = !options.headless && !args.iter().any(|arg| arg.to_ascii_lowercase().starts_with("--headless"));
        let sync =
            tokio::spawn(self.clone().cookie_sync(id.to_string(), browser.ws_endpoint.clone(), windowed, watcher.clone()));
        let health = relay.health();
        let watched = browser.clone();
        {
            let mut state = self.lock_state();
            let active = Active {
                browser,
                relay,
                lock,
                watcher: watcher.clone(),
                sync,
                clear_cache_on_exit: options.clear_cache_on_exit,
                windowed,
            };
            state.active.insert(id.to_string(), active);
            // Конец запуска и «работает» — под одним мьютексом: «стоп» не застанет «запускается» с живым браузером.
            state.launching.remove(id);
            state.statuses.insert(id.to_string(), ProfileStatus::Running(info.clone()));
        }
        // Слежение — до события «работает»: паника между ними (например, у подписчика событий) не оставит браузер
        // без присмотра, его выход уйдёт в обычную уборку. Выход слежение разбирает после `announced`: интерфейс
        // не получит «работает» позже «упал».
        let (announced, announcement) = oneshot::channel();
        tokio::spawn(self.clone().watch(id.to_string(), watched, health, watcher, announcement));
        self.emit_status(id, ProfileStatus::Running(info.clone()));
        if replaced {
            log::warn!("профиль {id}: закреплённый прокси не подошёл, запущен через прокси {proxy_id}");
            (self.sink)(Event::GatewayReplaced { profile_id: id.to_string(), name: profile.name.clone() });
        }
        let _ = announced.send(());
        log::info!("профиль {id}: запущен, pid {}", info.pid);
        Ok(info)
    }

    /// Выбор шлюза (спецификация, «Шлюз»): до трёх кандидатов, каждый проверяется запросом геоданных через прослойку.
    /// Возвращает шлюз с зарезервированным прокси; при ошибке резерв уже снят, прослойка закрыта.
    async fn choose_gateway(
        &self,
        id: &str,
        profile: &ProfileRecord,
        fingerprint: &Fingerprint,
    ) -> Result<Gateway, SupervisorError> {
        if profile.network.direct {
            let relay = relay::start(crate::relay::UpstreamConfig::direct(), Some(self.paths.relay_log(id))).await?;
            match self.geo.lookup_with(&relay, profile.network.ip_checker).await {
                Ok(info) => return Ok(Gateway { proxy_id: 0, relay, geo: Some(info), replaced: false }),
                Err(_) if !fingerprint::needs_geo(fingerprint) => return Ok(Gateway { proxy_id: 0, relay, geo: None, replaced: false }),
                Err(_) => { relay.shutdown().await; return Err(SupervisorError::GeoUnavailable); }
            }
        }
        let mut tried = HashSet::new();
        let mut last_error = SupervisorError::NoFreeProxy;
        for attempt in 0..gateway::MAX_GATEWAY_ATTEMPTS {
            let Some(pick) = self.reserve_proxy(id, profile.proxy_id, &tried)? else {
                return Err(if attempt == 0 { SupervisorError::NoFreeProxy } else { last_error });
            };
            tried.insert(pick.proxy_id);
            let replaced = profile.proxy_id.is_some_and(|pinned| pinned != pick.proxy_id);
            let credentials = match self.store.proxy_credentials(pick.proxy_id) {
                Ok(credentials) => credentials,
                Err(e) => {
                    self.release_proxy(id);
                    return Err(e.into());
                }
            };
            let relay = match relay::start(gateway::upstream_config(&credentials), Some(self.paths.relay_log(id))).await {
                Ok(relay) => relay,
                Err(e) => {
                    self.release_proxy(id);
                    return Err(e.into());
                }
            };
            // Геоданные — до браузера: пока через прослойку нет чужих соединений, класс ошибки точный.
            match self.geo.lookup_with(&relay, profile.network.ip_checker).await {
                Ok(info) => {
                    self.record_check(pick.proxy_id, Some(&info));
                    return Ok(Gateway { proxy_id: pick.proxy_id, relay, geo: Some(info), replaced });
                }
                Err(GeoError::ProxyUnavailable) => {
                    self.record_check(pick.proxy_id, None);
                    relay.shutdown().await;
                    self.release_proxy(id);
                    log::warn!("профиль {id}: прокси {} недоступен (попытка {})", pick.proxy_id, attempt + 1);
                    last_error = SupervisorError::ProxyUnavailable;
                }
                Err(_) if fingerprint::needs_geo(fingerprint) => {
                    relay.shutdown().await;
                    self.release_proxy(id);
                    return Err(SupervisorError::GeoUnavailable);
                }
                // Таймзона и язык заданы вручную: геоданные не нужны, прокси отвечает.
                Err(_) => return Ok(Gateway { proxy_id: pick.proxy_id, relay, geo: None, replaced }),
            }
        }
        Err(last_error)
    }

    /// Кандидат выбирается и занимается под мьютексом состояния: два запуска не возьмут один прокси.
    /// Список прокси читается здесь же — мьютекс базы вложен в мьютекс состояния, обратного вложения нет.
    /// Прокси, чью блокировку держит другой процесс Ombra, занят так же, как резерв этого процесса: закреплённый
    /// заменяется свободным, а свободных нет — `NoFreeProxy` (правило ТЗ «не выдавать прокси, занятый другим
    /// запущенным профилем» — и между окном и `ombra flow`).
    fn reserve_proxy(
        &self,
        id: &str,
        pinned: Option<i64>,
        tried: &HashSet<i64>,
    ) -> Result<Option<gateway::Pick>, SupervisorError> {
        let random = getrandom::u64().unwrap_or(0);
        let mut state = self.lock_state();
        let proxies = self.store.list_proxies()?;
        let mut busy: HashSet<i64> = state.reserved.keys().copied().collect();
        loop {
            let Some(pick) = gateway::pick_proxy(pinned, &proxies, &busy, tried, random) else { return Ok(None) };
            // ponytail: чужой занятый прокси стоит до 50 мс повторов `DirLock` под мьютексом состояния; если таких
            // много, пробовать блокировки вне мьютекса с двухфазным резервом.
            match DirLock::acquire(&self.paths.proxy_lock_dir(pick.proxy_id)) {
                Ok(lock) => {
                    state.reserved.insert(pick.proxy_id, (id.to_string(), lock));
                    return Ok(Some(pick));
                }
                Err(LockError::Busy) => {
                    busy.insert(pick.proxy_id);
                }
                Err(LockError::Io(e)) => return Err(SupervisorError::Io(e)),
            }
        }
    }

    /// Снять резерв профиля и блокировку прокси: остановка, выход браузера, ошибка запуска.
    fn release_proxy(&self, id: &str) {
        self.lock_state().reserved.retain(|_, (owner, _)| owner != id);
    }

    /// Кэш после сеанса с `clear_cache_on_exit`: браузер уже закрыт, блокировка папки снята. Сбой — только в журнал.
    /// Удаление — в блокирующем потоке: большой кэш не занимает поток tokio; вызывающий дожидается его до смены статуса.
    async fn clear_cache_after_session(&self, id: &str) {
        let paths = self.paths.cache_paths(id);
        let owner = id.to_string();
        let cleared = tokio::task::spawn_blocking(move || {
            for path in paths {
                if let Err(e) = crate::paths::remove_path(&path) {
                    log::warn!("профиль {owner}: кэш после сеанса не удалён ({}): {e}", path.display());
                }
            }
        })
        .await;
        if let Err(e) = cleared {
            log::warn!("профиль {id}: удаление кэша после сеанса прервано: {e}");
        }
    }

    /// Результат проверки при запуске — в запись прокси (колонки «Страна и внешний IP» и «Проверка», колонка «Шлюз»).
    fn record_check(&self, proxy_id: i64, geo: Option<&GeoInfo>) {
        let result = self.store.record_proxy_check(proxy_id, unix_now(), geo.map(|g| (g.ip.as_str(), g.country.as_str())));
        if let Err(e) = result {
            log::warn!("прокси {proxy_id}: результат проверки не записан: {e}");
        }
    }

    /// Шаги запуска после выбора шлюза: при ошибке прослойку закрывает и резерв снимает вызывающий.
    /// Пока браузер ждёт поток лицензии, профиль «ждёт поток»; `cancel` («стоп») обрывает это ожидание.
    async fn launch_through(
        self: &Arc<Self>,
        id: &str,
        profile: &ProfileRecord,
        fingerprint: &Fingerprint,
        gateway: &Gateway,
        options: &StartOptions,
        cancel: CancellationToken,
    ) -> Result<(EngineProcess, RunningInfo), SupervisorError> {
        let geo = gateway.geo.as_ref();
        let facts = geo.map(|g| GeoFacts { ip: &g.ip, country: &g.country, timezone: &g.timezone });
        let resolved = fingerprint::resolve(fingerprint, facts).map_err(|_| SupervisorError::GeoUnavailable)?;
        let profile_dir = self.paths.profile_dir(id);
        let request = LaunchRequest {
            profile_name: profile.name.clone(),
            profile_dir: profile_dir.clone(),
            log_path: self.paths.chromium_log(id),
            resolved: resolved.clone(),
            relay_addr: gateway.relay.addr(),
            headless: options.headless,
            restore_tabs: options.restore_tabs.unwrap_or(profile.restore_tabs),
            // Аргументы запуска проверяет `build_command`, как и аргументы профиля: запрещённые — `BadArgs`.
            extra_args: options.extra_args.clone().unwrap_or_else(|| profile.extra_args.clone()),
            extensions: self.extension_dirs(id, profile),
            humanize: profile.humanize.enabled.then(|| HumanizeLaunch {
                seed: profile.humanize_seed,
                settings: profile.humanize.clone(),
                // Стенд движка собирает журналы прогона в свой каталог (сквозной сценарий Э5).
                debug_log: std::env::var_os("OMBRA_HUMANIZE_DEBUG").map(|dir| PathBuf::from(dir).join(format!("{id}.jsonl"))),
            }),
        };
        let command = self.engine.build_command(&request)?;
        let binary = command.program.clone();
        let inner = self.clone();
        let owner = id.to_string();
        let hooks = LaunchHooks { on_license: Arc::new(move |license| inner.on_license(&owner, license)), cancel };
        let browser = match self.engine.launch(command, hooks).await {
            Ok(browser) => browser,
            Err(e) => {
                // `launch` сам добивает помощников, но при ошибке `ps` там уборка не доходит до конца.
                let _ = process::reap_profile_processes(&profile_dir, Duration::ZERO).await;
                if matches!(e, EngineError::Cancelled) {
                    log::info!("профиль {id}: запуск отменён — «Стоп» или закрыто окно ожидания потока");
                    return Err(SupervisorError::Cancelled);
                }
                log::warn!("профиль {id}: браузер не запустился: {e}");
                return Err(SupervisorError::Launch(e));
            }
        };
        let started_at = unix_now();
        let record = PidRecord { pid: browser.pid, binary, profile_dir, started_at };
        if let Err(e) = pidfile::write(&self.paths.run_dir(), id, &record) {
            let mut browser = browser;
            let _ = self.engine.stop(&mut browser, self.timings.stop_grace).await;
            return Err(e.into());
        }
        if let Err(e) = self.prepare_browser(id, &browser, profile, options).await {
            log::warn!("профиль {id}: {e}");
            let mut browser = browser;
            // PID-файл убирается, только если браузер точно остановлен: иначе его найдёт уборка при следующем старте.
            if self.engine.stop(&mut browser, self.timings.stop_grace).await.is_ok() {
                let _ = pidfile::remove(&self.paths.run_dir(), id);
            }
            return Err(e);
        }
        let info = RunningInfo {
            pid: browser.pid,
            cdp_ws: browser.ws_endpoint.clone(),
            debug_port: browser.debug_port,
            relay_port: gateway.relay.addr().port(),
            proxy_id: gateway.proxy_id,
            exit_ip: geo.map(|g| g.ip.clone()),
            country: geo.map(|g| g.country.clone()),
            timezone: resolved.timezone,
            started_at,
        };
        Ok((browser, info))
    }

    /// Cookies из базы — до стартовых вкладок (спецификация, «Cookies»): база — основной экземпляр,
    /// она перекрывает совпадающие cookies из папки браузера. Без своих cookies профиль не запускается.
    async fn prepare_browser(&self, id: &str, browser: &EngineProcess, profile: &ProfileRecord, options: &StartOptions) -> Result<(), SupervisorError> {
        let cookies = self.store.load_cookies(id)?;
        let mut cdp = CdpClient::connect(&browser.ws_endpoint).await.map_err(cookies_error)?;
        let report = cdp.set_cookies(&cookies).await.map_err(cookies_error)?;
        if report.rejected > 0 {
            log::warn!("профиль {id}: браузер не принял {} cookies из {}", report.rejected, cookies.len());
        }
        let start_urls = options.start_urls.as_ref().unwrap_or(&profile.start_urls);
        if !start_urls.is_empty()
            && let Err(e) = cdp.open_start_tabs(start_urls).await
        {
            log::warn!("профиль {id}: стартовые вкладки не открылись: {e}");
        }
        Ok(())
    }

    /// Снимки cookies работающего браузера раз в `cookie_interval`; останавливается токеном `watcher`.
    /// Закрытое последнее окно Ombra не останавливает: браузер выходит сам (на macOS — с патчем движка 245), а окно без
    /// вкладок может ждать ответа пользователя (вопрос об отмене загрузки).
    async fn cookie_sync(self: Arc<Self>, id: String, ws: String, windowed: bool, cancel: CancellationToken) {
        let mut cdp: Option<CdpClient> = None;
        let mut ticks = tokio::time::interval(self.timings.cookie_interval);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // Первый тик срабатывает сразу, а набор только что поставлен из базы.
        ticks.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                _ = ticks.tick() => {
                    // Браузер без потока лицензии закрыл CDP: снимок только засорил бы журнал.
                    if matches!(self.status(&id), ProfileStatus::Waiting { .. }) {
                        cdp = None;
                        continue;
                    }
                    if let Err(e) = self.snapshot(&id, &ws, windowed, &mut cdp).await {
                        // Соединение откроется заново на следующем тике.
                        cdp = None;
                        log::warn!("профиль {id}: снимок cookies не удался: {e}");
                    }
                }
            }
        }
    }

    /// Один снимок: полный набор браузера; в базу — только если хэш отличается от записанного.
    /// Снимок по расписанию идёт вне очереди профиля: иначе «стоп» ждал бы за зависшим `getCookies`.
    /// Поэтому записанное значение читается до запроса к браузеру, а пишется снимок, только если оно не сменилось:
    /// импорт, пришедший, пока браузер отвечал, не стирается набором без него.
    /// Браузер с окнами без страниц cookies не отдаёт: снимка нет.
    async fn snapshot(&self, id: &str, ws: &str, windowed: bool, cdp: &mut Option<CdpClient>) -> Result<(), SnapshotError> {
        if cdp.is_none() {
            *cdp = Some(CdpClient::connect(ws).await?);
        }
        let client = cdp.as_mut().expect("соединение CDP открыто выше");
        if windowed && !client.has_pages().await? {
            return Ok(());
        }
        let before = self.store.cookies_hash(id)?;
        let cookies = client.get_cookies().await?;
        let hash = set_hash(&cookies);
        if before.as_deref() == Some(hash.as_str()) {
            return Ok(());
        }
        if !self.store.replace_cookies(id, &cookies, &hash, before.as_deref())? {
            log::info!("профиль {id}: снимок cookies пропущен — набор в базе сменился, пока шёл снимок");
        }
        Ok(())
    }

    /// Последний снимок перед закрытием — не дольше `FINAL_SNAPSHOT_TIMEOUT`; при сбое или без окон в базе остаётся прошлый.
    async fn final_snapshot(&self, id: &str, ws: &str, windowed: bool) {
        let mut cdp = None;
        match tokio::time::timeout(FINAL_SNAPSHOT_TIMEOUT, self.snapshot(id, ws, windowed, &mut cdp)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("профиль {id}: последний снимок cookies не удался: {e}"),
            Err(_) => log::warn!("профиль {id}: последний снимок cookies не успел за {FINAL_SNAPSHOT_TIMEOUT:?}"),
        }
    }

    /// Ждёт выхода браузера (если не по команде «стоп» — это падение), следит за здоровьем шлюза и раз в
    /// `license_poll` спрашивает `/json/version`: не потерял ли браузер поток лицензии.
    /// Разбор начинается после `announced` — события «работает» от запуска.
    async fn watch(
        self: Arc<Self>,
        id: String,
        mut browser: EngineProcess,
        mut health: watch::Receiver<Health>,
        cancel: CancellationToken,
        announced: oneshot::Receiver<()>,
    ) {
        // `Err` — запуск оборвался до сигнала: «работает» уже не придёт, ждать нечего.
        let _ = announced.await;
        // Шлюз мог перестать отвечать ещё до начала слежения: такое изменение `changed()` уже не покажет.
        let initial = *health.borrow_and_update();
        if initial == Health::Unreachable {
            self.on_health(&id, initial);
        }
        let mut health_open = true;
        let mut probes = tokio::time::interval(self.timings.license_poll);
        probes.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // Первый тик срабатывает сразу, а браузер только что ответил `200`.
        probes.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                exit = browser.wait_exit() => {
                    self.on_exit(&id, exit).await;
                    return;
                }
                changed = health.changed(), if health_open => {
                    if changed.is_err() {
                        health_open = false;
                        continue;
                    }
                    let current = *health.borrow_and_update();
                    self.on_health(&id, current);
                }
                _ = probes.tick() => {
                    // Остановка не ждёт ответа: устаревший ответ не должен менять статус после неё.
                    let probe = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return,
                        probe = self.engine.probe(browser.debug_port) => probe,
                    };
                    self.on_probe(&id, probe, *health.borrow());
                }
            }
        }
    }

    /// Пять ошибок подключения подряд — «Пауза»; первое удачное соединение — снова «Работает».
    /// Браузер, ждущий поток, остаётся «ждёт поток»: шлюз учтёт `on_probe`, когда поток вернётся.
    fn on_health(&self, id: &str, health: Health) {
        let next = {
            let mut state = self.lock_state();
            let Some(status) = state.statuses.get_mut(id) else { return };
            let next = match (&*status, health) {
                (ProfileStatus::Running(info), Health::Unreachable) => ProfileStatus::Paused(info.clone()),
                (ProfileStatus::Paused(info), Health::Ok) => ProfileStatus::Running(info.clone()),
                _ => return,
            };
            *status = next.clone();
            next
        };
        let paused = matches!(next, ProfileStatus::Paused(_));
        self.emit_status(id, next);
        if paused {
            log::warn!("профиль {id}: шлюз не отвечает, пауза");
            (self.sink)(Event::GatewayUnreachable { profile_id: id.to_string() });
        } else {
            log::info!("профиль {id}: шлюз снова отвечает");
        }
    }

    /// Ответ `/json/version` работающего браузера: потерял поток лицензии — «ждёт поток» с данными браузера; получил
    /// снова — «работает» или «пауза», смотря по шлюзу сейчас. Нет ответа — статус не меняется.
    fn on_probe(&self, id: &str, probe: Probe, health: Health) {
        let next = {
            let mut state = self.lock_state();
            let Some(status) = state.statuses.get_mut(id) else { return };
            let next = match (&*status, probe) {
                (ProfileStatus::Waiting { license: old, owner_hint: old_hint, .. }, Probe::License { state: license, owner_hint })
                    if *old == license && *old_hint == owner_hint => return,
                (
                    ProfileStatus::Running(info) | ProfileStatus::Paused(info) | ProfileStatus::Waiting { info: Some(info), .. },
                    Probe::License { state: license, owner_hint },
                ) if license != "ok" => ProfileStatus::Waiting { license, info: Some(info.clone()), owner_hint },
                (ProfileStatus::Waiting { info: Some(info), .. }, Probe::Ready(_)) => match health {
                    Health::Unreachable => ProfileStatus::Paused(info.clone()),
                    Health::Ok => ProfileStatus::Running(info.clone()),
                },
                _ => return,
            };
            *status = next.clone();
            next
        };
        if let ProfileStatus::Waiting { license, .. } = &next {
            log::warn!("профиль {id}: браузер ждёт поток лицензии ({license})");
        } else {
            log::info!("профиль {id}: браузер снова получил поток лицензии");
        }
        self.emit_status(id, next);
    }

    /// Браузер завершился сам: прослойка закрывается, блокировка, PID-файл и резерв прокси снимаются.
    /// Штатный выход с кодом 0 (пользователь закрыл браузер) — «остановлен», остальное — «упал» (решение заказчика 01.10.2026).
    async fn on_exit(&self, id: &str, exit: ExitInfo) {
        let Some(active) = self.lock_state().active.remove(id) else { return };
        let closed_by_user = exit.code == Some(0) && exit.signal.is_none();
        if closed_by_user {
            log::info!("профиль {id}: браузер закрыт пользователем");
        } else {
            log::warn!("профиль {id}: браузер завершился без команды: код {:?}, сигнал {:?}", exit.code, exit.signal);
        }
        // Браузера уже нет — снимать cookies не у кого: в базе остаётся последний снимок.
        let Active { browser, relay, lock, watcher, clear_cache_on_exit, .. } = active;
        watcher.cancel();
        let _ = process::reap_profile_processes(&browser.profile_dir, self.timings.settle).await;
        relay.shutdown().await;
        drop(lock);
        let _ = pidfile::remove(&self.paths.run_dir(), id);
        if clear_cache_on_exit {
            self.clear_cache_after_session(id).await;
        }
        self.release_proxy(id);
        // Статус меняется последним: до этого повторный запуск получает «уже запущен», а не «папка занята».
        let status = if closed_by_user {
            ProfileStatus::Stopped
        } else {
            ProfileStatus::Crashed { code: exit.code, signal: exit.signal, at: unix_now() }
        };
        self.set_status(id, status);
    }

    /// Все PID-файлы убираются параллельно; ошибка одного профиля не мешает остальным.
    async fn cleanup_orphans(self: &Arc<Self>) -> std::io::Result<Vec<String>> {
        let run_dir = self.paths.run_dir();
        let mut tasks = tokio::task::JoinSet::new();
        for (id, _) in pidfile::list(&run_dir)? {
            let inner = self.clone();
            let run_dir = run_dir.clone();
            tasks.spawn(async move {
                let result = inner.cleanup_one(&run_dir, &id).await;
                (id, result)
            });
        }
        let mut cleaned = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((id, Ok(true))) => cleaned.push(id),
                // Профиль держит живой процесс Ombra: его браузер не сирота.
                Ok((_, Ok(false))) => {}
                Ok((id, Err(e))) => log::warn!("профиль {id}: уборка после прошлого запуска не удалась: {e}"),
                Err(e) => log::warn!("уборка после прошлого запуска прервана: {e}"),
            }
        }
        cleaned.sort();
        Ok(cleaned)
    }

    /// Папка берётся по идентификатору, а не из файла: так испорченный файл не уведёт к чужим процессам.
    /// Файл удаляется только после того, как процессы завершены.
    /// `Ok(false)` — папку профиля держит живой процесс Ombra (окно рядом с `ombra flow` или другая команда,
    /// спецификация А2): браузер не сирота, pid-файл остаётся владельцу.
    async fn cleanup_one(&self, run_dir: &Path, id: &str) -> std::io::Result<bool> {
        let dir = self.paths.profile_dir(id);
        // Блокировка держится до конца уборки: профиль не запустят посреди неё. Папки нет — профиль удалён.
        let _lock = if dir.exists() {
            match acquire_dir_lock(dir.clone()).await {
                Ok(lock) => Some(lock),
                Err(SupervisorError::ProfileLocked) => return Ok(false),
                Err(e) => return Err(std::io::Error::other(e.to_string())),
            }
        } else {
            None
        };
        let killed = process::terminate_profile_processes(&dir, self.timings.orphan_grace).await?;
        if !killed.is_empty() {
            log::warn!("профиль {id}: после прошлого запуска добиты процессы {killed:?}");
        }
        pidfile::remove(run_dir, id)?;
        Ok(true)
    }
}

/// `flock` с короткими повторами — в блокирующем потоке: повторы спят и не должны занимать поток tokio.
async fn acquire_dir_lock(dir: PathBuf) -> Result<DirLock, SupervisorError> {
    match tokio::task::spawn_blocking(move || DirLock::acquire(&dir)).await {
        Ok(Ok(lock)) => Ok(lock),
        Ok(Err(LockError::Busy)) => Err(SupervisorError::ProfileLocked),
        Ok(Err(LockError::Io(e))) => Err(SupervisorError::Io(e)),
        Err(e) => Err(SupervisorError::Io(std::io::Error::other(format!("блокировка папки прервана: {e}")))),
    }
}
