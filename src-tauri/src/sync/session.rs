//! Сессия синхронизатора (спецификация синхронизатора, «Повтор в ведомых окнах», «Отклонение и пауза», «Сессия и
//! края»): перехват на вкладках главного окна, рабочий с очередью шагов на каждое ведомое, статусы окон.
//! Главный никого не ждёт: шаг ведомому — `try_send` в его очередь, окна повторяют независимо друг от друга.
use super::model::{Action, Chunker, History, NavStep, Signature, TYPING_IDLE, classify_navigation};
use super::replay::{DIALOG_WAIT, NAV_WAIT, Pause, answer_dialog, delay, navigate, replay, wanted, wheel_series};
use super::{Mode, SyncSettings};
use crate::engine::cdp::{Bounds, CdpClient, CdpError};
use crate::engine::network::NetGuard;
use crate::engine::page::{Browser, CdpEvent, Page};
use crate::engine::sync::{Capture, CaptureFlags, Captured};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Шагов, ждущих ведомое окно; больше — окно не успевает за главным, пауза.
const QUEUE: usize = 1024;
const DROPPED: &str = "Окно отключено от синхронизации";
/// Смена вкладки у ведомых уводит к ним фокус ОС: через столько вкладка главного выходит снова, фокус — у главного окна.
const REFOCUS: Duration = Duration::from_millis(300);
/// Переход, который страница начала сама, — не позже этого после своего запроса или после ввода, вызвавшего его.
const PAGE_STEP: Duration = Duration::from_secs(1);

/// Окно сессии: профиль и адрес CDP его браузера.
#[derive(Clone)]
pub struct WindowSpec {
    pub profile_id: String,
    pub ws: String,
}

/// Состояние сессии для окна Ombra: главное окно — первым.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub active: bool,
    pub mode: Mode,
    pub main: Option<String>,
    pub windows: Vec<WindowState>,
}

impl SyncState {
    /// Сессии нет.
    pub fn idle() -> Self {
        Self { active: false, mode: Mode::FullCopy, main: None, windows: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub profile_id: String,
    pub main: bool,
    pub status: WindowStatus,
    /// У ведомого нет очеловечивания: повтор без траекторий, только с задержками (бывает лишь в `varied`).
    pub plain: bool,
}

/// Статус окна; `code` — ключ причины (`sync.reason.*`), `detail` подставляется в её текст.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WindowStatus {
    Synced,
    Paused { code: String, detail: String },
    Dropped { code: String },
}

/// Сообщает каждую смену состояния сессии.
pub type OnChange = Arc<dyn Fn(SyncState) + Send + Sync>;

/// Шаг ведомому; `tab` — номер вкладки по порядку появления вкладок главного.
#[derive(Debug, Clone)]
enum Step {
    /// `series` — колесо продолжает серию прокрутки: задержки «С вариациями» перед ним нет.
    Input { tab: usize, action: Arc<Action>, series: bool },
    /// Переход из интерфейса браузера главного (адрес, назад-вперёд, перезагрузка).
    Nav { tab: usize, step: NavStep },
    /// Вкладка из интерфейса браузера главного; открытая страницей появится у ведомого сама, из повтора клика.
    NewTab { url: String },
    /// Вкладка ссылки уже открывается из повтора клика: дождаться её у ведомого, не создавать вторую.
    LinkedTab { parent: usize, tab: usize, url: String },
    CloseTab(usize),
    Activate(usize),
    /// Ответ главного на диалог страницы (`alert`, `confirm`, `prompt`, `beforeunload`).
    Dialog { tab: usize, accept: bool, text: String },
}

impl Step {
    fn tab(&self) -> Option<usize> {
        match self {
            Self::Input { tab, .. } | Self::Nav { tab, .. } | Self::Dialog { tab, .. } | Self::Activate(tab) => Some(*tab),
            _ => None,
        }
    }
}

fn take_opening<T>(opened: &mut HashMap<String, T>, info: &serde_json::Value) -> Option<(String, T)> {
    let parent = info["openerId"].as_str().or_else(|| info["openerFrameId"].as_str())?;
    opened.remove(parent).map(|opening| (parent.to_string(), opening))
}

/// Статус ведомого окна и его очередь шагов. Поколение растёт на «Вернуть»: шаги, поставленные до паузы, рабочий
/// выбрасывает — пропущенное не догоняется.
struct FollowerSlot {
    state: watch::Sender<(WindowStatus, u64)>,
    sender: mpsc::Sender<(u64, Step)>,
}

impl FollowerSlot {
    fn new(sender: mpsc::Sender<(u64, Step)>) -> Self {
        Self { state: watch::channel((WindowStatus::Synced, 0)).0, sender }
    }

    fn status(&self) -> WindowStatus {
        self.state.borrow().0.clone()
    }

    fn synced_at(&self, epoch: u64) -> bool {
        *self.state.borrow() == (WindowStatus::Synced, epoch)
    }

    /// Синхронное окно встаёт на паузу; с `epoch` — только в том же поколении (рабочий доигрывал шаг прежнего).
    fn pause(&self, epoch: Option<u64>, (code, detail): Pause) {
        self.state.send_if_modified(|(status, current)| {
            let hit = *status == WindowStatus::Synced && epoch.is_none_or(|epoch| epoch == *current);
            if hit {
                *status = WindowStatus::Paused { code: code.to_string(), detail };
            }
            hit
        });
    }
}

/// Шаг в очередь окна без ожидания. Окно не синхронно — шаг выбрасывается; очередь полна — пауза `failed`.
fn offer(slot: &FollowerSlot, step: Step) -> bool {
    let epoch = {
        let state = slot.state.borrow();
        if state.0 != WindowStatus::Synced {
            return false;
        }
        state.1
    };
    match slot.sender.try_send((epoch, step)) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            slot.pause(Some(epoch), ("failed", "очередь переполнена".into()));
            false
        }
        // Рабочий кончился с закрытым окном: выпадение сообщит сторож по `Browser::closed`.
        Err(TrySendError::Closed(_)) => false,
    }
}

/// Окно сессии и то, что сессия в нём поменяла, — вернуть при остановке и выпадении.
struct Window {
    profile_id: String,
    browser: Browser,
    /// Очеловечивание выключила сессия.
    humanize_off: AtomicBool,
}

impl Window {
    /// Очеловечивание — если `quiet` и оно включено; политика капч остаётся у профиля.
    async fn take(profile_id: String, browser: Browser, quiet: bool) -> (Self, bool) {
        let humanize = browser.humanize_enabled().await == Some(true);
        let humanize_off = quiet && humanize && changed(&profile_id, "Не удалось отключить имитацию естественного ввода", browser.set_humanize(false).await);
        let window = Self { profile_id, browser, humanize_off: AtomicBool::new(humanize_off) };
        (window, humanize && !humanize_off)
    }

    /// Вернуть как было; повторный вызов ничего не делает.
    async fn restore(&self) {
        if self.humanize_off.swap(false, Ordering::SeqCst) {
            changed(&self.profile_id, "Не удалось восстановить настройки естественного ввода", self.browser.set_humanize(true).await);
        }
    }
}

/// Модуль `Ombra.*` у окна есть, а смена не прошла, — в журнал; окно закрыто — возвращать нечему. Ответ — сменилось ли.
fn changed(profile_id: &str, what: &str, result: Result<(), CdpError>) -> bool {
    match result {
        Ok(()) => true,
        Err(CdpError::Closed) => false,
        Err(error) => {
            log::warn!("синхронизатор, профиль {profile_id}: {what}: {error}");
            false
        }
    }
}

struct Follower {
    window: Window,
    plain: bool,
    slot: FollowerSlot,
    /// Сессии вкладок по порядку появления; на паузе отключены, но порядок target id сохранён.
    tabs: tokio::sync::Mutex<Vec<Arc<Page>>>,
    /// Вкладки пользовательских ссылок, ещё не сопоставленные с шагом главного: (родитель, новая цель).
    linked: watch::Sender<LinkedTabs>,
}

#[derive(Default)]
struct LinkedTabs {
    opened: VecDeque<(u64, String, String)>,
    /// A late native popup after a URL copy is ambiguous: pause before it can claim another tab's input.
    copied: HashSet<String>,
}

struct PendingTab {
    epoch: u64,
    tab: usize,
    parent_tab: usize,
    parent: Arc<Page>,
    url: String,
    deadline: Option<Instant>,
}

/// Вкладка главного в сессии.
struct MainTab {
    target: String,
    page: Arc<Page>,
    capture: Capture,
    /// Сеть вкладки включена, пока она в сессии: по ней виден статус документа.
    _net: NetGuard,
    /// Последний документ вкладки загрузился без ошибки — с ним сравнивается ошибка страницы ведомого.
    ok: bool,
    /// Открыт диалог страницы.
    dialog: bool,
    /// Когда диалог последний раз открылся или закрылся: только что закрытый ещё отвечается ведомым.
    dialog_at: Option<Instant>,
    /// Когда ведомым ушло последнее нажатие мыши или клавиши.
    input_at: Option<Instant>,
}

impl MainTab {
    /// У главного диалог есть или только что был — диалог ведомого не лишний.
    fn dialog_busy(&self) -> bool {
        self.dialog || self.dialog_at.is_some_and(|at| at.elapsed() < DIALOG_WAIT)
    }
}

/// Задача, которая снимается вместе с владельцем.
struct Owned<T>(JoinHandle<T>);

impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Сессия вкладки с включённой сетью и подпиской на события соединения, взятой до включения.
struct Tab {
    page: Arc<Page>,
    session: String,
    events: broadcast::Receiver<CdpEvent>,
    net: NetGuard,
}

/// Подключённая вкладка главного: сессия, перехват с его потоком и история до следующего перехода.
type MainParts = (Tab, Capture, mpsc::Receiver<Captured>, History);

struct Inner {
    settings: SyncSettings,
    main: Window,
    /// Вкладки главного по порядку появления.
    tabs: Mutex<Vec<MainTab>>,
    /// Вкладка главного, ставшая видимой последней.
    active: Mutex<Option<String>>,
    followers: Vec<Follower>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    on_change: OnChange,
    /// Смены сообщаются по одной: последнее сообщённое состояние — самое свежее.
    report: Mutex<()>,
    stopping: AtomicBool,
    master_closed: AtomicBool,
    ended: CancellationToken,
}

/// Сессия синхронизатора; клон — та же сессия.
#[derive(Clone)]
pub struct Session(Arc<Inner>);

impl Session {
    /// Подключиться ко всем окнам, выровнять их размер, поставить перехват на вкладки главного, подключить вкладки
    /// ведомых; политика капч остаётся у профилей. Сбой подключения ничего в окнах не меняет.
    pub async fn start(main: WindowSpec, followers: Vec<WindowSpec>, settings: SyncSettings, on_change: OnChange) -> Result<Session, String> {
        Self::open(main, followers, settings, on_change).await.map_err(|e| e.to_string())
    }

    async fn open(main: WindowSpec, followers: Vec<WindowSpec>, settings: SyncSettings, on_change: OnChange) -> Result<Session, CdpError> {
        let main_browser = Browser::connect(&main.ws).await?;
        let mut browsers = Vec::new();
        for spec in &followers {
            browsers.push(Browser::connect(&spec.ws).await?);
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), match_master_size(&main, &followers)).await;
        let resize_main = main.clone();
        let resize_followers = followers.clone();
        // Подписка на вкладки — до их списка: открытая между ними не пропадёт.
        let mut follower_tabs = Vec::new();
        for browser in &browsers {
            let discovered = browser.discover_tabs().await?;
            follower_tabs.push((attach_tabs(browser, &[]).await?, discovered));
        }
        let main_discovered = main_browser.discover_tabs().await?;
        let mut main_tabs = Vec::new();
        for tab in main_browser.tabs().await?.into_iter().filter(|tab| !tab.from_page) {
            main_tabs.push((tab.target_id.clone(), main_tab(&main_browser, &tab.target_id, &settings).await?));
        }

        let full_copy = settings.mode == Mode::FullCopy;
        let (main_window, _) = Window::take(main.profile_id, main_browser, false).await;
        let (mut list, mut queues) = (Vec::new(), Vec::new());
        for (spec, browser) in followers.into_iter().zip(browsers) {
            let (window, humanize) = Window::take(spec.profile_id, browser, full_copy).await;
            let (sender, receiver) = mpsc::channel(QUEUE);
            queues.push(receiver);
            list.push(Follower { window, plain: !full_copy && !humanize, slot: FollowerSlot::new(sender), tabs: tokio::sync::Mutex::default(), linked: watch::channel(LinkedTabs::default()).0 });
        }
        let session = Session(Arc::new(Inner {
            settings,
            main: main_window,
            tabs: Mutex::default(),
            active: Mutex::default(),
            followers: list,
            tasks: Mutex::default(),
            on_change,
            report: Mutex::default(),
            stopping: AtomicBool::new(false),
            master_closed: AtomicBool::new(false),
            ended: CancellationToken::new(),
        }));
        for (target, parts) in main_tabs {
            session.run_main(target, parts);
        }
        for (i, ((tabs, discovered), queue)) in follower_tabs.into_iter().zip(queues).enumerate() {
            *session.0.followers[i].tabs.lock().await = session.follow(i, tabs);
            session.spawn(session.clone().work(i, queue));
            session.spawn(session.clone().guard(i));
            session.spawn(session.clone().watch_follower_tabs(i, discovered));
        }
        session.spawn(session.clone().watch_main_tabs(main_discovered));
        session.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let _ = tokio::time::timeout(Duration::from_secs(2), match_master_size(&resize_main, &resize_followers)).await;
            }
        });
        let closed = session.clone();
        session.spawn(async move {
            closed.0.main.browser.closed().await;
            closed.0.master_closed.store(true, Ordering::SeqCst);
            closed.finish();
        });
        Ok(session)
    }

    pub fn same_session(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn master_closed(&self) -> bool {
        self.0.master_closed.load(Ordering::SeqCst)
    }

    pub fn state(&self) -> SyncState {
        let inner = &self.0;
        let main = WindowState { profile_id: inner.main.profile_id.clone(), main: true, status: WindowStatus::Synced, plain: false };
        let followers = inner.followers.iter().map(|f| WindowState { profile_id: f.window.profile_id.clone(), main: false, status: f.slot.status(), plain: f.plain });
        SyncState {
            active: !inner.stopping.load(Ordering::SeqCst),
            mode: inner.settings.mode,
            main: Some(inner.main.profile_id.clone()),
            windows: std::iter::once(main).chain(followers).collect(),
        }
    }

    /// «Отпустить»: пауза `released` — окно можно поправить руками.
    pub async fn release(&self, id: &str) -> Result<(), String> {
        let follower = &self.0.followers[self.follower(id)?];
        if matches!(follower.slot.status(), WindowStatus::Dropped { .. }) {
            return Err(DROPPED.into());
        }
        follower.linked.send_modify(|tabs| *tabs = LinkedTabs::default());
        follower.slot.state.send_if_modified(|(status, _)| {
            let released = WindowStatus::Paused { code: "released".into(), detail: String::new() };
            if matches!(status, WindowStatus::Dropped { .. }) || *status == released { return false; }
            *status = released;
            true
        });
        Ok(())
    }

    /// «Вернуть»: вкладки подключаются заново, окно повторяет с текущего действия главного.
    pub async fn resume(&self, id: &str) -> Result<(), String> {
        let i = self.follower(id)?;
        let follower = &self.0.followers[i];
        follower.linked.send_modify(|tabs| *tabs = LinkedTabs::default());
        let mut tabs = follower.tabs.lock().await;
        // Под мьютексом вкладок: остановка снимет и то, что подключено здесь.
        if self.0.stopping.load(Ordering::SeqCst) {
            return Err("сессия остановлена".into());
        }
        match follower.slot.status() {
            WindowStatus::Synced => return Ok(()),
            WindowStatus::Dropped { .. } => return Err(DROPPED.into()),
            WindowStatus::Paused { .. } => {}
        }
        let order: Vec<_> = tabs.iter().map(|p| p.target().to_string()).collect();
        for page in tabs.iter() { let _ = page.detach().await; }
        let mut attached = attach_tabs(&follower.window.browser, &order).await.map_err(|e| e.to_string())?;
        attached.sort_by_key(|t| order.iter().position(|id| id == t.page.target()).unwrap_or(usize::MAX));
        *tabs = self.follow(i, attached);
        let resumed = follower.slot.state.send_if_modified(|(status, epoch)| {
            let paused = matches!(status, WindowStatus::Paused { .. });
            if paused {
                *status = WindowStatus::Synced;
                *epoch += 1;
            }
            paused
        });
        if resumed { Ok(()) } else { Err(DROPPED.into()) }
    }

    /// Номер видимой вкладки главного: ставшая видимой последней, иначе видимая сейчас, иначе первая.
    pub async fn main_tab(&self) -> usize {
        let last = self.0.active.lock().expect("мьютекс вкладки отравлен").clone();
        if let Some(tab) = last.and_then(|target| self.with_tab(&target, |tab, _| tab)) {
            return tab;
        }
        let visible = self.0.main.browser.visible_tab().await.ok().flatten();
        visible.and_then(|target| self.with_tab(&target, |tab, _| tab)).unwrap_or(0)
    }

    /// «Открыть как в главном» использует сохранённое соответствие: CDP не гарантирует порядок `getTargets`.
    pub async fn open_like_main(&self, id: &str) -> Result<(), String> {
        let follower = &self.0.followers[self.follower(id)?];
        let browser = &follower.window.browser;
        let tab = self.main_tab().await;
        let main = self.0.tabs.lock().expect("мьютекс вкладок отравлен").get(tab).map(|t| t.page.clone());
        let url = main.ok_or("у главного окна нет вкладок")?.url().await.map_err(|e| e.to_string())?;
        let target = follower.tabs.lock().await.get(tab).map(|p| p.target().to_string()).ok_or("нет такой вкладки")?;
        let page = browser.attach(&target).await.map_err(|e| e.to_string())?;
        let went = page.goto(&url, NAV_WAIT).await;
        let _ = page.detach().await;
        went.map_err(|e| e.to_string())
    }

    /// Остановка: перехват снят, сессии вкладок отключены, очеловечивание — как до сессии.
    pub async fn stop(&self) {
        let inner = &self.0;
        if inner.stopping.swap(true, Ordering::SeqCst) {
            return self.ended().await;
        }
        let tasks = std::mem::take(&mut *inner.tasks.lock().expect("мьютекс задач отравлен"));
        for task in tasks {
            task.abort();
        }
        let tabs = std::mem::take(&mut *inner.tabs.lock().expect("мьютекс вкладок отравлен"));
        for tab in tabs {
            tab.capture.stop().await;
            let _ = tab.page.detach().await;
        }
        inner.main.restore().await;
        for follower in &inner.followers {
            detach(&mut *follower.tabs.lock().await).await;
            follower.window.restore().await;
        }
        self.changed();
        inner.ended.cancel();
    }

    /// Ждёт конца сессии: `stop` или закрытия главного окна.
    pub async fn ended(&self) {
        self.0.ended.cancelled().await
    }

    fn follower(&self, id: &str) -> Result<usize, String> {
        self.0.followers.iter().position(|f| f.window.profile_id == id).ok_or_else(|| "Окно не участвует в синхронизации".to_string())
    }

    /// Задача сессии; после начала остановки не ставится, поставленные снимает `stop`.
    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = self.0.tasks.lock().expect("мьютекс задач отравлен");
        tasks.retain(|task| !task.is_finished());
        if !self.0.stopping.load(Ordering::SeqCst) {
            tasks.push(tokio::spawn(task));
        }
    }

    fn offer_all(&self, step: Step) {
        for follower in &self.0.followers {
            offer(&follower.slot, step.clone());
        }
    }

    /// `f` над вкладкой главного `target` и её номером сейчас — под замком вкладок: шаг, отправленный в `f`, не
    /// разойдётся с номером из-за закрытия вкладки. Вкладки уже нет — ничего.
    fn with_tab<R>(&self, target: &str, f: impl FnOnce(usize, &mut MainTab) -> R) -> Option<R> {
        let mut tabs = self.0.tabs.lock().expect("мьютекс вкладок отравлен");
        let tab = tabs.iter().position(|t| t.target == target)?;
        Some(f(tab, &mut tabs[tab]))
    }

    /// Новая вкладка главного: создать у ведомых или подхватить вкладку ссылки — раньше её ввода; затем перехват.
    /// Не подключилась, а вкладка есть — ведомые встают на паузу: молча они разошлись бы с главным.
    async fn main_tab_opened(&self, target: String, url: String, parent: Option<String>) {
        if self.with_tab(&target, |_, _| ()).is_some() {
            return;
        }
        let step = match parent {
            Some(parent) => {
                let Some(parent) = self.with_tab(&parent, |tab, _| tab) else { return };
                let tab = self.0.tabs.lock().expect("мьютекс вкладок отравлен").len();
                Step::LinkedTab { parent, tab, url }
            }
            None => Step::NewTab { url },
        };
        match main_tab(&self.0.main.browser, &target, &self.0.settings).await {
            Ok(parts) => {
                self.offer_all(step);
                // Новая вкладка выходит вперёд без `visibilitychange`: какая видима, снова неизвестно.
                *self.0.active.lock().expect("мьютекс вкладки отравлен") = None;
                self.run_main(target, parts);
            }
            // Вкладки уже нет (окно на миг, загрузка в `_blank`) — расходиться ведомым не с чем.
            Err(error) => {
                let open = self.0.main.browser.tabs().await.is_ok_and(|tabs| tabs.iter().any(|tab| tab.target_id == target));
                if open {
                    for follower in &self.0.followers {
                        follower.slot.pause(None, ("failed", format!("Не удалось подключиться к вкладке главного окна: {error}")));
                    }
                }
            }
        }
    }

    /// Вкладка главного закрыта: ведомым `CloseTab` с её номером.
    fn main_tab_closed(&self, target: &str) {
        let mut tabs = self.0.tabs.lock().expect("мьютекс вкладок отравлен");
        if let Some(i) = tabs.iter().position(|t| t.target == target) {
            let _closed = tabs.remove(i);
            self.offer_all(Step::CloseTab(i));
        }
    }

    /// Вкладка главного в сессии: её перехват и сторож; после начала остановки — нет.
    fn run_main(&self, target: String, (tab, capture, captured, history): MainParts) {
        let Tab { page, session, events, net } = tab;
        {
            let mut tabs = self.0.tabs.lock().expect("мьютекс вкладок отравлен");
            if self.0.stopping.load(Ordering::SeqCst) {
                return;
            }
            tabs.push(MainTab { target: target.clone(), page: page.clone(), capture, _net: net, ok: true, dialog: false, dialog_at: None, input_at: None });
        }
        self.spawn(self.clone().listen(target.clone(), captured));
        self.spawn(self.clone().watch_main(target, page, session, events, history));
    }

    /// Вкладки ведомого `i` под сторожами; ответ — их сессии по порядку.
    fn follow(&self, i: usize, tabs: Vec<Tab>) -> Vec<Arc<Page>> {
        tabs.into_iter()
            .map(|tab| {
                let page = tab.page.clone();
                self.spawn(self.clone().watch_follower(i, tab));
                page
            })
            .collect()
    }

    /// Новая вкладка ведомого — в конец его вкладок, если её там ещё нет. На паузе вкладки отключены: «Вернуть»
    /// подключит все заново.
    async fn adopt(&self, i: usize, target: &str, epoch: u64) -> Result<(), CdpError> {
        let follower = &self.0.followers[i];
        let mut tabs = follower.tabs.lock().await;
        if self.0.stopping.load(Ordering::SeqCst) || !follower.slot.synced_at(epoch) || tabs.iter().any(|p| p.target() == target) {
            return Ok(());
        }
        let tab = follower_tab(&follower.window.browser, target).await?;
        if self.0.stopping.load(Ordering::SeqCst) || !follower.slot.synced_at(epoch) {
            let _ = tab.page.detach().await;
            return Ok(());
        }
        tabs.extend(self.follow(i, vec![tab]));
        Ok(())
    }

    fn changed(&self) {
        let _turn = self.0.report.lock().expect("мьютекс смен отравлен");
        (self.0.on_change)(self.state());
    }

    /// Остановка изнутри: в своей задаче — `stop` снимает задачи сессии, и ту, что её начала.
    fn finish(&self) {
        let session = self.clone();
        tokio::spawn(async move { session.stop().await });
    }

    /// Поток вкладки главного `target`: включённые в настройках виды действий — ведомым; в «С вариациями» — через
    /// куски набора.
    async fn listen(self, target: String, mut events: mpsc::Receiver<Captured>) {
        let settings = &self.0.settings;
        let mut chunker = (settings.mode == Mode::Varied).then(Chunker::default);
        let mut wheel_at = None;
        loop {
            let idle = tokio::time::sleep(TYPING_IDLE);
            tokio::pin!(idle);
            let captured = tokio::select! {
                captured = events.recv() => match captured { Some(captured) => Some(captured), None => break },
                () = &mut idle, if chunker.is_some() => None,
            };
            let actions = match captured {
                Some(captured) => match wanted(settings, &captured) {
                    Some(action) => match chunker.as_mut() {
                        Some(chunker) => chunker.push(action, Instant::now()),
                        None => vec![action],
                    },
                    None => Vec::new(),
                },
                None => chunker.as_mut().map(Chunker::flush).unwrap_or_default(),
            };
            for action in actions {
                if action == Action::Visible {
                    if self.activated(&target) {
                        self.with_tab(&target, |tab, _| self.offer_all(Step::Activate(tab)));
                    }
                    continue;
                }
                let series = wheel_series(&mut wheel_at, &action, Instant::now());
                self.with_tab(&target, |tab, main| {
                    if page_input(&action) { main.input_at = Some(Instant::now()); }
                    self.offer_all(Step::Input { tab, action: Arc::new(action), series });
                });
            }
        }
    }

    /// Вкладка главного стала видимой: у ведомых выходит та же, а через `REFOCUS` у главного — снова она, и фокус ОС
    /// возвращается главному окну. Видимой снова стала прежняя (окно развернули, его открыло ведомое) — ничего.
    fn activated(&self, target: &str) -> bool {
        let mut active = self.0.active.lock().expect("мьютекс вкладки отравлен");
        if active.as_deref() == Some(target) {
            return false;
        }
        *active = Some(target.to_string());
        let (browser, target) = (self.0.main.browser.clone(), target.to_string());
        self.spawn(async move {
            tokio::time::sleep(REFOCUS).await;
            let _ = browser.activate(&target).await;
        });
        true
    }

    /// Сторож вкладки главного: переходы из интерфейса браузера (по снимкам истории до и после перехода главного
    /// фрейма), статус документа, диалоги страницы. Переход, который начала сама страница, получится у ведомых из
    /// повтора ввода — командой он не повторяется (спецификация, «Переходы»).
    async fn watch_main(self, target: String, page: Arc<Page>, session: String, mut events: broadcast::Receiver<CdpEvent>, mut history: History) {
        // Запрос перехода от страницы (`frameRequestedNavigation`: `location.reload()`, скрипт, ссылка) и начало
        // перехода, который уже идёт: когда, начала ли его страница, внутри ли он документа.
        let (mut requested, mut started): (Option<Instant>, Option<(Instant, bool, bool)>) = (None, None);
        while let Some(event) = next_event(&mut events, &session).await {
            let Some(event) = event else {
                // Отстали: переход мог пропасть — снимок истории берётся заново, шаг не угадывается.
                if let Ok(now) = page.history().await {
                    history = now;
                }
                continue;
            };
            if let Some(outcome) = document_outcome(&event, &target) {
                self.with_tab(&target, |_, main| main.ok = outcome.is_ok());
            }
            let ours = event.params["frameId"] == target.as_str();
            // Переход внутри документа (`pushState`, якорь, назад-вперёд в приложении одной страницы) — тем же порядком;
            // запись и номер у `replaceState` прежние, это не перезагрузка.
            let within = event.method == "Page.navigatedWithinDocument";
            let navigated = if within { ours } else { event.method == "Page.frameNavigated" && event.params["frame"]["id"] == target.as_str() };
            match event.method.as_str() {
                // Новая вкладка или окно (`_blank`, ⌘-клик) — не переход этой вкладки.
                "Page.frameRequestedNavigation" if ours && event.params["disposition"] == "currentTab" => requested = Some(Instant::now()),
                "Page.frameStartedNavigating" if ours => {
                    let by_page = requested.take().is_some_and(|at| at.elapsed() <= PAGE_STEP);
                    let same = matches!(event.params["navigationType"].as_str(), Some("sameDocument" | "historySameDocument"));
                    started = Some((Instant::now(), by_page, same));
                }
                _ if navigated => {
                    let Ok(after) = page.history().await else { continue };
                    let step = classify_navigation(&history, &after);
                    history = after;
                    // Переход внутри документа берёт только своё начало: `pushState` прежнего документа, пока грузится
                    // новый, не уносит начала его загрузки.
                    let start = if within { started.take_if(|(_, _, same)| *same) } else { started.take() };
                    let (at, by_page, _) = start.unwrap_or((Instant::now(), false, false));
                    let Some(step) = step.filter(|step| !by_page && !(within && *step == NavStep::Reload)) else { continue };
                    self.with_tab(&target, |tab, main| {
                        // `history.back()` страницы запроса не шлёт: шаг по истории сразу за вводом — от страницы.
                        let after_input = main.input_at.is_some_and(|input| at.saturating_duration_since(input) <= PAGE_STEP);
                        if !(matches!(step, NavStep::History(_)) && after_input) {
                            self.offer_all(Step::Nav { tab, step });
                        }
                    });
                }
                "Page.javascriptDialogOpening" | "Page.javascriptDialogClosed" => {
                    let open = event.method == "Page.javascriptDialogOpening";
                    let answer = (!open).then(|| (event.params["result"].as_bool().unwrap_or(false), event.params["userInput"].as_str().unwrap_or_default().to_string()));
                    self.with_tab(&target, |tab, main| {
                        (main.dialog, main.dialog_at) = (open, Some(Instant::now()));
                        if let Some((accept, text)) = answer {
                            self.offer_all(Step::Dialog { tab, accept, text });
                        }
                    });
                }
                _ => {}
            }
        }
    }

    /// Вкладки главного: новая — в конец, закрытая — ведомым её номер.
    async fn watch_main_tabs(self, mut events: broadcast::Receiver<CdpEvent>) {
        let mut opened: HashMap<String, Option<String>> = HashMap::new();
        loop {
            let event = match events.recv().await {
                Ok(event) if event.session.is_none() || event.method == "Page.windowOpen" => event,
                Ok(_) => continue,
                // Отстали: вкладки сверяются со списком браузера.
                Err(RecvError::Lagged(_)) => {
                    opened.clear();
                    for follower in &self.0.followers {
                        follower.slot.pause(None, ("failed", "потеряны события открытия вкладок".into()));
                    }
                    let Ok(list) = self.0.main.browser.tabs().await else { continue };
                    let gone: Vec<String> = {
                        let tabs = self.0.tabs.lock().expect("мьютекс вкладок отравлен");
                        tabs.iter().filter(|t| !list.iter().any(|l| l.target_id == t.target)).map(|t| t.target.clone()).collect()
                    };
                    for target in gone {
                        self.main_tab_closed(&target);
                    }
                    for tab in list.into_iter().filter(|tab| !tab.from_page) {
                        self.main_tab_opened(tab.target_id, tab.url, None).await;
                    }
                    continue;
                }
                Err(RecvError::Closed) => return,
            };
            let info = &event.params["targetInfo"];
            match event.method.as_str() {
                "Page.windowOpen" => {
                    let parent = event.session.as_deref().and_then(|session| {
                        self.0.tabs.lock().expect("мьютекс вкладок отравлен").iter()
                            .find(|tab| tab.page.has_session(session)).map(|tab| tab.target.clone())
                    });
                    if let Some(parent) = parent {
                        let url = (event.params["userGesture"] == true)
                            .then(|| event.params["url"].as_str().unwrap_or("about:blank").to_string());
                        // windowOpen precedes popup/sandbox checks; a blocked attempt must not consume the next real target.
                        opened.insert(parent, url);
                    }
                }
                "Target.targetCreated" if info["type"] == "page" => {
                    let (parent, url) = if crate::engine::page::page_opened_tab(info) {
                        let Some((parent, Some(url))) = take_opening(&mut opened, info) else { continue };
                        (Some(parent), url)
                    } else { (None, info["url"].as_str().unwrap_or_default().to_string()) };
                    self.main_tab_opened(info["targetId"].as_str().unwrap_or_default().to_string(), url, parent).await;
                }
                "Target.targetDestroyed" => {
                    let target = event.params["targetId"].as_str().unwrap_or_default();
                    opened.remove(target);
                    self.main_tab_closed(target);
                }
                _ => {}
            }
        }
    }

    /// Вкладка ссылки ждёт своего шага: самопроизвольные popup не сдвигают соответствие вкладок.
    async fn watch_follower_tabs(self, i: usize, mut events: broadcast::Receiver<CdpEvent>) {
        let follower = &self.0.followers[i];
        let mut opened: HashMap<String, Option<u64>> = HashMap::new();
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(RecvError::Lagged(_)) => {
                    opened.clear();
                    follower.slot.pause(None, ("failed", "потеряны события открытия вкладок".into()));
                    continue;
                }
                Err(RecvError::Closed) => return,
            };
            if event.method == "Page.windowOpen" {
                let tabs = follower.tabs.lock().await;
                let state = follower.slot.state.borrow();
                if let Some(page) = event.session.as_deref().and_then(|session| tabs.iter().find(|page| page.has_session(session))) {
                    let epoch = (event.params["userGesture"] == true && state.0 == WindowStatus::Synced).then_some(state.1);
                    opened.insert(page.target().to_string(), epoch);
                }
                continue;
            }
            if event.session.is_some() { continue; }
            let info = &event.params["targetInfo"];
            match event.method.as_str() {
                "Target.targetCreated" if info["type"] == "page" => {
                    let target = info["targetId"].as_str().unwrap_or_default();
                    if crate::engine::page::page_opened_tab(info) {
                        if let Some((parent, Some(epoch))) = take_opening(&mut opened, info) && follower.slot.synced_at(epoch) {
                            let mut late = false;
                            follower.linked.send_modify(|tabs| {
                                late = tabs.copied.contains(&parent);
                                tabs.opened.retain(|(current, _, _)| *current == epoch);
                                if !late { tabs.opened.push_back((epoch, parent, target.to_string())); }
                            });
                            if late {
                                follower.slot.pause(Some(epoch), ("failed", "Запоздалое открытие вкладки: верните окно в синхронизацию".into()));
                            } else if follower.linked.borrow().opened.len() >= QUEUE {
                                follower.slot.pause(Some(epoch), ("failed", "очередь вкладок переполнена".into()));
                            }
                        }
                    } else {
                        let epoch = follower.slot.state.borrow().1;
                        let _ = self.adopt(i, target, epoch).await;
                    }
                }
                "Target.targetDestroyed" => {
                    opened.remove(event.params["targetId"].as_str().unwrap_or_default());
                    follower.linked.send_modify(|tabs| tabs.opened.retain(|(_, _, target)| event.params["targetId"] != *target));
                }
                _ => {}
            }
        }
    }

    /// Сторож вкладки ведомого: окно выбора файла — пауза `file`; документ с ошибкой, когда последний документ главного
    /// на этой вкладке загрузился без неё, — `page_error`; диалог страницы, которого у главного нет, — `dialog`.
    // ponytail: сравнение — с последним статусом главного; ответ ведомого раньше ответа главного, оба с ошибкой, — ложная
    // пауза. Ждать ответа главного на тот же переход, если это проявится.
    async fn watch_follower(self, i: usize, tab: Tab) {
        let Tab { page, session, mut events, net: _net } = tab;
        let follower = &self.0.followers[i];
        // Отставший сторож пропущенного не восстановит: выбор файла и ошибки загрузки — события, не состояние. Выбор
        // файла межсайтового фрейма приходит в сессию фрейма.
        let file_in_frame = |event: &CdpEvent| event.method == "Page.fileChooserOpened" && event.session.as_deref().is_some_and(|s| page.has_session(s));
        while let Some(event) = next_event_or(&mut events, &session, file_in_frame).await {
            let Some(event) = event else { continue };
            let pause = match event.method.as_str() {
                "Page.fileChooserOpened" => ("file", String::new()),
                "Page.javascriptDialogOpening" => match self.extra_dialog(i, &page, &session, &mut events).await {
                    Some(true) => ("dialog", String::new()),
                    Some(false) => continue,
                    // Сессия вкладки кончилась, пока ждали диалог.
                    None => return,
                },
                _ => match document_outcome(&event, page.target()) {
                    Some(Err(detail)) if self.main_tab_at(i, &page, |t| t.ok).await == Some(true) => ("page_error", detail),
                    _ => continue,
                },
            };
            follower.slot.pause(None, pause);
        }
    }

    /// Вкладка главного с тем же номером, что вкладка ведомого `page`, — `f` над ней.
    async fn main_tab_at<R>(&self, i: usize, page: &Arc<Page>, f: impl FnOnce(&MainTab) -> R) -> Option<R> {
        let tab = self.0.followers[i].tabs.lock().await.iter().position(|p| Arc::ptr_eq(p, page))?;
        self.0.tabs.lock().expect("мьютекс вкладок отравлен").get(tab).map(f)
    }

    /// Диалог ведомого открыт дольше `DIALOG_WAIT`, а у главного на этой вкладке диалога нет и не было только что:
    /// его не закроет ответ главного (`Some(true)`). Диалог главного ещё открыт (клавиша, загрузка) — ждём его ответа.
    /// Диалог закрылся — `Some(false)`, сессия вкладки кончилась — `None`.
    async fn extra_dialog(&self, i: usize, page: &Arc<Page>, session: &str, events: &mut broadcast::Receiver<CdpEvent>) -> Option<bool> {
        loop {
            let closed = async {
                while let Some(event) = next_event(events, session).await {
                    if event.is_some_and(|event| event.method == "Page.javascriptDialogClosed") {
                        return Some(false);
                    }
                }
                None
            };
            if let Ok(done) = tokio::time::timeout(DIALOG_WAIT, closed).await {
                return done;
            }
            if self.main_tab_at(i, page, MainTab::dialog_busy).await != Some(true) {
                return Some(true);
            }
        }
    }

    /// Рабочий ведомого: шаги по порядку, каждый — только в синхронном окне и в своём поколении.
    async fn work(self, i: usize, mut steps: mpsc::Receiver<(u64, Step)>) {
        let follower = &self.0.followers[i];
        let browser = &follower.window.browser;
        // Поколение и поле, где фокус уже сверен.
        let mut focus: (u64, Option<Signature>) = (0, None);
        // Ответы на диалоги — по очереди и без ожидания: диалог ведомого откроет, может быть, шаг, идущий следом.
        let mut answering: Option<Owned<()>> = None;
        let mut linking: Option<PendingTab> = None;
        let mut linked = follower.linked.subscribe();
        let mut pending: VecDeque<(u64, Step)> = VecDeque::new();
        let mut status = follower.slot.state.subscribe();
        loop {
            while let Ok((epoch, step)) = steps.try_recv() {
                if pending.len() >= QUEUE {
                    follower.slot.pause(Some(epoch), ("failed", "очередь переполнена".into()));
                } else { pending.push_back((epoch, step)); }
            }
            if linking.as_ref().is_some_and(|link| !follower.slot.synced_at(link.epoch)) { linking = None; }
            if let Some(link) = &mut linking {
                let waiting = pending.iter().take_while(|(_, step)| !matches!(step, Step::CloseTab(tab) if *tab < link.tab))
                    .any(|(epoch, step)| *epoch == link.epoch && step.tab() == Some(link.parent_tab));
                if waiting { link.deadline = None; }
                let expired = !waiting && *link.deadline.get_or_insert_with(|| Instant::now() + NAV_WAIT) <= Instant::now();
                let mut target = None;
                follower.linked.send_if_modified(|tabs| {
                    if let Some(at) = tabs.opened.iter().position(|(epoch, opener, _)| *epoch == link.epoch && opener == link.parent.target()) {
                        target = tabs.opened.remove(at).map(|(_, _, target)| target);
                        true
                    } else if expired && (!self.0.settings.clicks || !self.0.settings.typing) {
                        tabs.copied.insert(link.parent.target().to_string());
                        true
                    } else { false }
                });
                if target.is_some() || expired {
                    let link = linking.take().unwrap();
                    if !follower.slot.synced_at(link.epoch) { continue; }
                    let target = match target {
                        Some(target) => Ok(target),
                        None if !self.0.settings.clicks || !self.0.settings.typing => browser.new_tab(&link.url).await,
                        None => Err(CdpError::Timeout),
                    };
                    let result = match target {
                        Ok(target) => self.adopt(i, &target, link.epoch).await,
                        Err(error) => Err(error),
                    };
                    if let Err(error) = result {
                        follower.slot.pause(Some(link.epoch), ("failed", format!("Не удалось подключиться к открытой вкладке: {error}")));
                    }
                    continue;
                }
            }
            // Opening input can arrive after targetCreated; it must keep draining.
            // Structural steps wait so tab indices cannot shift underneath the pending attachment.
            let pending_ready = if let Some(link) = &linking {
                let mut closing = false;
                pending.iter().position(|(current, step)| {
                    if !follower.slot.synced_at(*current) { return true; }
                    closing |= matches!(step, Step::CloseTab(tab) if *tab < link.tab);
                    !closing && *current == link.epoch && step.tab().is_some_and(|tab| tab < link.tab)
                })
            } else { (!pending.is_empty()).then_some(0) };
            let from_pending = pending_ready.is_some();
            let (epoch, step) = if let Some(at) = pending_ready {
                pending.remove(at).unwrap()
            } else {
                let tab_wait = linking.as_ref().and_then(|link| link.deadline).map(|at| at.saturating_duration_since(Instant::now()));
                tokio::select! {
                    changed = linked.changed() => {
                        if changed.is_err() { return; }
                        continue;
                    }
                    () = tokio::time::sleep(tab_wait.unwrap_or(NAV_WAIT)), if tab_wait.is_some() => continue,
                    changed = status.changed() => {
                        if changed.is_err() { return; }
                        continue;
                    }
                    step = steps.recv() => match step { Some((epoch, step)) => (epoch, step), None => return },
                }
            };
            if !follower.slot.synced_at(epoch) {
                continue;
            }
            if let Some(link) = &linking
                && (link.epoch != epoch || !step.tab().is_some_and(|tab| tab < link.tab)
                    || (!from_pending && pending.iter().any(|(_, step)| matches!(step, Step::CloseTab(tab) if *tab < link.tab))))
            {
                if pending.len() + steps.len() >= QUEUE {
                    follower.slot.pause(Some(epoch), ("failed", "очередь переполнена".into()));
                } else { pending.push_back((epoch, step)); }
                continue;
            }
            if let Some(link) = &mut linking && step.tab() == Some(link.parent_tab) { link.deadline = None; }
            if let Step::Input { action, series: false, .. } = &step
                && let Some(wait) = delay(&self.0.settings, action)
            {
                tokio::time::sleep(wait).await;
                if !follower.slot.synced_at(epoch) {
                    continue;
                }
            }
            if focus.0 != epoch {
                focus = (epoch, None);
            }
            let result = match step {
                Step::NewTab { url } => match browser.new_tab(&url).await {
                    Ok(target) => self.adopt(i, &target, epoch).await.map(|()| None),
                    Err(error) => Err(error),
                },
                Step::LinkedTab { parent, tab, url } => {
                    let parent_tab = parent;
                    let Some(parent) = self.page(i, epoch, parent).await else { continue };
                    linking = Some(PendingTab { epoch, tab, parent_tab, parent, url, deadline: None });
                    continue;
                }
                Step::CloseTab(tab) => {
                    let page = {
                        let mut tabs = follower.tabs.lock().await;
                        if !follower.slot.synced_at(epoch) {
                            continue;
                        }
                        (tab < tabs.len()).then(|| tabs.remove(tab))
                    };
                    match page {
                        // Вкладку могла уже закрыть сама страница ведомого.
                        Some(page) => {
                            let _ = browser.close_tab(page.target()).await;
                            Ok(None)
                        }
                        None => Ok(Some(("no_tab", String::new()))),
                    }
                }
                Step::Activate(tab) => {
                    let Some(page) = self.page(i, epoch, tab).await else { continue };
                    browser.activate(page.target()).await.map(|()| None)
                }
                Step::Dialog { tab, accept, text } => {
                    let Some(page) = self.page(i, epoch, tab).await else { continue };
                    let (previous, session) = (answering.take(), self.clone());
                    answering = Some(Owned(tokio::spawn(async move {
                        if let Some(mut previous) = previous {
                            let _ = (&mut previous.0).await;
                        }
                        if !answer_dialog(&page, accept, &text).await {
                            session.0.followers[i].slot.pause(Some(epoch), ("no_dialog", String::new()));
                        }
                    })));
                    continue;
                }
                Step::Nav { tab, step } => {
                    let Some(page) = self.page(i, epoch, tab).await else { continue };
                    let call = {
                        let page = page.clone();
                        async move { navigate(&page, &step).await }
                    };
                    let Some(result) = self.unblocked(i, epoch, &page, call).await else { continue };
                    result
                }
                Step::Input { tab, action, .. } => {
                    let Some(page) = self.page(i, epoch, tab).await else { continue };
                    let call = {
                        let (page, mut checked) = (page.clone(), focus.1.take());
                        async move { replay(&page, &action, &mut checked).await.map(|pause| (pause, checked)) }
                    };
                    let Some(result) = self.unblocked(i, epoch, &page, call).await else { continue };
                    result.map(|(pause, checked)| {
                        focus.1 = checked;
                        pause
                    })
                }
            };
            match result {
                Ok(None) => {}
                Ok(Some(pause)) => follower.slot.pause(Some(epoch), pause),
                // Окно закрыто: выпадение сообщит сторож.
                Err(CdpError::Closed) => return,
                Err(error) => follower.slot.pause(Some(epoch), ("failed", error.to_string())),
            }
        }
    }

    /// Вкладка `tab` ведомого `i` для шага поколения `epoch`; нет такой — пауза `no_tab`. Окно, пока ждали вкладок,
    /// встало на паузу (и, может быть, вернулось) — шаг прежнего поколения, `None`.
    async fn page(&self, i: usize, epoch: u64, tab: usize) -> Option<Arc<Page>> {
        let follower = &self.0.followers[i];
        let page = follower.tabs.lock().await.get(tab).cloned();
        if !follower.slot.synced_at(epoch) {
            return None;
        }
        if page.is_none() {
            follower.slot.pause(Some(epoch), ("no_tab", String::new()));
        }
        page
    }

    /// Действие во вкладке ведомого. Открылся диалог страницы (действие ответит, только когда его закроют) или окно
    /// больше не синхронно — рабочий его не ждёт, а недоигранное снимается: команда, уже отправленная браузеру,
    /// исполнится, остальные — нет, и ничего из этого шага не уйдёт после следующего. Ответ на диалог идёт сразу.
    async fn unblocked<T: Send + 'static>(
        &self,
        i: usize,
        epoch: u64,
        page: &Page,
        call: impl Future<Output = Result<T, CdpError>> + Send + 'static,
    ) -> Option<Result<T, CdpError>> {
        // Подписки — до вызова: диалог, открытый им, не пропадёт.
        let (session, mut events) = page.events();
        let mut state = self.0.followers[i].slot.state.subscribe();
        let mut call = Owned(tokio::spawn(call));
        let opened = async {
            while let Some(event) = next_event(&mut events, &session).await {
                if event.is_some_and(|event| event.method == "Page.javascriptDialogOpening") {
                    return;
                }
            }
            std::future::pending().await
        };
        let left = async {
            let _ = state.wait_for(|now| *now != (WindowStatus::Synced, epoch)).await;
        };
        tokio::select! {
            done = &mut call.0 => Some(done.expect("шаг ведомого упал")),
            () = opened => None,
            () = left => None,
        }
    }

    /// Сторож ведомого: на паузе отключает сессии его вкладок (движок снимает блок ручного ввода), закрытое окно
    /// выпадает. Каждая смена статуса сообщается; ведомых не осталось — сессия кончается.
    async fn guard(self, i: usize) {
        let follower = &self.0.followers[i];
        let mut status = follower.slot.state.subscribe();
        loop {
            tokio::select! {
                _ = follower.window.browser.closed() => break,
                changed = status.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    // Под мьютексом вкладок: «Вернуть», успевшее подключить их заново, не теряет сессий.
                    let tabs = follower.tabs.lock().await;
                    if follower.slot.status() != WindowStatus::Synced {
                        for page in tabs.iter() {
                            let _ = page.detach().await;
                        }
                    }
                    drop(tabs);
                    self.changed();
                }
            }
        }
        follower.slot.state.send_modify(|(status, _)| *status = WindowStatus::Dropped { code: "closed".into() });
        follower.window.restore().await;
        self.changed();
        if self.0.followers.iter().all(|f| matches!(f.slot.status(), WindowStatus::Dropped { .. })) {
            self.finish();
        }
    }
}

/// Всем окнам — наименьшие ширина и высота среди них, положение прежнее: повтор идёт по координатам окна
/// просмотра, и ни одно окно не становится больше экрана своего отпечатка.
async fn match_master_size(main: &WindowSpec, followers: &[WindowSpec]) -> Result<(), CdpError> {
    let mut master = CdpClient::connect(&main.ws).await?;
    let window = master.window_for_first_page().await?;
    if master.window_state(window).await? == "minimized" {
        return Ok(());
    }
    let wanted = master.window_bounds(window).await?;
    futures_util::future::join_all(followers.iter().map(|spec| async {
        let resize = async {
            let mut cdp = CdpClient::connect(&spec.ws).await?;
            let window = cdp.window_for_first_page().await?;
            let bounds = cdp.window_bounds(window).await?;
            if (bounds.width, bounds.height) != (wanted.width, wanted.height) {
                cdp.set_window_bounds(window, &Bounds { width: wanted.width, height: wanted.height, ..bounds }).await?;
            }
            Ok::<_, CdpError>(())
        };
        // Недоступное/закрытое ведомое не мешает остальным окнам и повтору действий.
        let _ = resize.await;
    })).await;
    Ok(())
}

/// Вкладки ведомых — как у главного, чтобы сопоставление по порядку сошлось с первого шага: вкладка с тем же номером
/// открывает адрес главного (без ожидания загрузки), недостающие открываются, лишние закрываются. Вкладок несколько —
/// у ведомых видима та, что у главного, затем главное окно снова впереди.
pub async fn align_tabs(main: &WindowSpec, followers: &[WindowSpec]) -> Result<(), CdpError> {
    let main = Browser::connect(&main.ws).await?;
    let want = main.tabs().await?;
    if want.is_empty() {
        return Err(CdpError::Protocol("У главного окна нет вкладок".into()));
    }
    let visible = if want.len() > 1 { main.visible_tab().await? } else { None };
    let shown = visible.as_ref().and_then(|target| want.iter().position(|tab| &tab.target_id == target));
    for spec in followers {
        let browser = Browser::connect(&spec.ws).await?;
        let have = browser.tabs().await?;
        for extra in have.iter().skip(want.len()) {
            browser.close_tab(&extra.target_id).await?;
        }
        for (i, tab) in want.iter().enumerate() {
            match have.get(i) {
                Some(own) if own.url == tab.url => {}
                Some(own) => {
                    let page = browser.attach(&own.target_id).await?;
                    let went = page.goto(&tab.url, Duration::ZERO).await;
                    let _ = page.detach().await;
                    went?;
                }
                None => {
                    browser.new_tab(&tab.url).await?;
                }
            }
        }
        if let Some(tab) = shown
            && let Some(own) = browser.tabs().await?.get(tab)
        {
            browser.activate(&own.target_id).await?;
        }
    }
    if let Some(target) = visible {
        main.activate(&target).await?;
    }
    Ok(())
}

/// Вкладка главного в сессии: сеть, история и перехват.
async fn main_tab(browser: &Browser, target: &str, settings: &SyncSettings) -> Result<MainParts, CdpError> {
    let tab = attach_tab(browser, target).await?;
    let history = tab.page.history().await?;
    let (capture, captured) = Capture::start(&tab.page, CaptureFlags { mouse: settings.mouse }).await?;
    Ok((tab, capture, captured, history))
}

async fn attach_tab(browser: &Browser, target: &str) -> Result<Tab, CdpError> {
    let page = browser.attach(target).await?;
    let (session, events) = page.events();
    let net = page.network().acquire().await?;
    Ok(Tab { page: Arc::new(page), session, events, net })
}

/// Вкладка ведомого: окно выбора файла в ней не открывается (придёт `Page.fileChooserOpened`).
async fn follower_tab(browser: &Browser, target: &str) -> Result<Tab, CdpError> {
    let tab = attach_tab(browser, target).await?;
    tab.page.intercept_files().await?;
    Ok(tab)
}

/// Вкладки ведомого в порядке `Target.getTargets`.
async fn attach_tabs(browser: &Browser, known: &[String]) -> Result<Vec<Tab>, CdpError> {
    let mut tabs = Vec::new();
    for tab in browser.tabs().await?.into_iter().filter(|tab| !tab.from_page || known.contains(&tab.target_id)) {
        tabs.push(follower_tab(browser, &tab.target_id).await?);
    }
    Ok(tabs)
}

/// Ввод, после которого шаг по истории может сделать страница (`history.back()` по клику или клавише). Клавиши с
/// Alt, Ctrl или ⌘ и сами модификаторы — нет: ⌥←, ⌘[ — «Назад» браузера, а повтор нажатий его у ведомых не вызовет,
/// шаг должен прийти `Nav`.
fn page_input(action: &Action) -> bool {
    match action {
        Action::Move { .. } | Action::Wheel { .. } | Action::Visible => false,
        Action::Key(key) => key.mods & (1 | 2 | 4) == 0 && !matches!(key.key.as_str(), "Alt" | "AltGraph" | "Control" | "Meta" | "Shift"),
        _ => true,
    }
}

/// Следующее событие сессии вкладки; `Some(None)` — подписка отстала и события могли пропасть, `None` — сессия
/// отключена или соединение закрыто.
async fn next_event(events: &mut broadcast::Receiver<CdpEvent>, session: &str) -> Option<Option<CdpEvent>> {
    next_event_or(events, session, |_| false).await
}

/// То же, и событие другой сессии, которое берёт `also`.
async fn next_event_or(events: &mut broadcast::Receiver<CdpEvent>, session: &str, also: impl Fn(&CdpEvent) -> bool) -> Option<Option<CdpEvent>> {
    loop {
        match events.recv().await {
            Ok(event) if event.method == "Target.detachedFromTarget" && event.params["sessionId"] == session => return None,
            Ok(event) if event.session.as_deref() == Some(session) || also(&event) => return Some(Some(event)),
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => return Some(None),
            Err(RecvError::Closed) => return None,
        }
    }
}

/// Чем кончилась загрузка главного документа вкладки `target`, если событие об этом: ответ `Document` его фрейма или
/// страница ошибки сети (`unreachableUrl`). Ошибка — текст для причины паузы.
fn document_outcome(event: &CdpEvent, target: &str) -> Option<Result<(), String>> {
    let params = &event.params;
    match event.method.as_str() {
        "Network.responseReceived" if params["type"] == "Document" && params["frameId"] == target => {
            let status = params["response"]["status"].as_i64().unwrap_or_default();
            Some(if status >= 400 { Err(format!("HTTP {status}")) } else { Ok(()) })
        }
        "Page.frameNavigated" if params["frame"]["parentId"].is_null() && params["frame"]["unreachableUrl"].is_string() => Some(Err("ошибка сети".into())),
        _ => None,
    }
}

async fn detach(tabs: &mut Vec<Arc<Page>>) {
    for page in tabs.drain(..) {
        let _ = page.detach().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use crate::engine::page::Button;
    use crate::sync::model::KeyEvent;

    #[test]
    fn page_input_leaves_out_browser_shortcuts() {
        let key = |key: &str, mods: i64| Action::Key(KeyEvent { down: true, key: key.into(), code: String::new(), key_code: 0, mods, location: 0, sig: None });
        let down = Action::Down { x: 1.0, y: 1.0, button: Button::Left, buttons: 1, count: 1, mods: 0, sig: Signature::default(), select: false };
        assert!(page_input(&down));
        assert!(page_input(&key("Enter", 0)) && page_input(&key(" ", 0)) && page_input(&key("A", 8)), "клавиши страницы, Shift тоже");
        assert!(!page_input(&Action::Move { x: 1.0, y: 1.0, buttons: 0, mods: 0 }));
        // ⌥←, Ctrl+←, ⌘[ — «Назад» браузера, и отпускание самих модификаторов после них.
        for (k, mods) in [("ArrowLeft", 1), ("ArrowLeft", 2), ("[", 4), ("Alt", 0), ("Meta", 0), ("Control", 0)] {
            assert!(!page_input(&key(k, mods)), "{k} {mods}");
        }
    }

    #[test]
    fn document_outcome_reads_status_of_the_tab_document() {
        let event = |method: &str, params: Value| CdpEvent { session: Some("S1".into()), method: method.into(), params };
        let response = |kind: &str, frame: &str, status: i64| {
            event("Network.responseReceived", json!({ "type": kind, "frameId": frame, "response": { "status": status } }))
        };
        assert_eq!(document_outcome(&response("Document", "T1", 500), "T1"), Some(Err("HTTP 500".into())));
        assert_eq!(document_outcome(&response("Document", "T1", 200), "T1"), Some(Ok(())));
        assert_eq!(document_outcome(&response("Document", "F2", 404), "T1"), None, "документ фрейма — не вкладки");
        assert_eq!(document_outcome(&response("Script", "T1", 404), "T1"), None);
        let navigated = |frame: Value| event("Page.frameNavigated", json!({ "frame": frame }));
        assert_eq!(document_outcome(&navigated(json!({ "id": "T1", "unreachableUrl": "http://x/" })), "T1"), Some(Err("ошибка сети".into())));
        assert_eq!(document_outcome(&navigated(json!({ "id": "T1" })), "T1"), None);
        assert_eq!(document_outcome(&navigated(json!({ "id": "F2", "parentId": "T1", "unreachableUrl": "http://x/" })), "T1"), None);
    }

    async fn until(check: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !check() { tokio::task::yield_now().await; }
        }).await.expect("sync transition not observed");
    }

    async fn mock_browser() -> crate::engine::page::tests::Mock {
        crate::engine::page::tests::Mock::spawn(|method, params, session| match method {
            "Target.attachToTarget" => json!({"sessionId":format!("S{}", params["targetId"].as_str().unwrap().trim_start_matches('T'))}),
            "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":format!("T{}", session.trim_start_matches('S')),"loaderId":"L1"}}}),
            "Page.createIsolatedWorld" => json!({"executionContextId":1}),
            "Page.getNavigationHistory" => json!({"currentIndex":0,"entries":[{"id":1,"url":"https://test/"}]}),
            "Page.addScriptToEvaluateOnNewDocument" => json!({"identifier":"1"}),
            "Runtime.evaluate" if params["expression"] == "globalThis.__ombraSyncNext()" || params["expression"].as_str().is_some_and(|expression| expression.contains("return sig(document.activeElement);")) => json!({"silent":true}),
            "Runtime.evaluate" => json!({"result":{"type":"undefined"}}),
            "Ombra.getHumanizeState" => json!({"state":{"enabled":false}}),
            _ => json!({}),
        }).await
    }

    async fn fixture(settings: SyncSettings) -> (Session, crate::engine::page::tests::Mock, crate::engine::page::tests::Mock) {
        fixture_with_follower(settings, mock_browser().await).await
    }

    async fn fixture_with_follower(settings: SyncSettings, follower: crate::engine::page::tests::Mock) -> (Session, crate::engine::page::tests::Mock, crate::engine::page::tests::Mock) {
        let main = mock_browser().await;
        let main_browser = Browser::connect(&main.url).await.unwrap();
        let follower_browser = Browser::connect(&follower.url).await.unwrap();
        let (main_window, _) = Window::take("main".into(), main_browser.clone(), false).await;
        let (window, _) = Window::take("follower".into(), follower_browser.clone(), false).await;
        let (sender, receiver) = mpsc::channel(QUEUE);
        let session = Session(Arc::new(Inner {
            settings, main: main_window, tabs: Mutex::default(), active: Mutex::default(),
            followers: vec![Follower { window, plain:false, slot:FollowerSlot::new(sender), tabs:tokio::sync::Mutex::default(), linked:watch::channel(LinkedTabs::default()).0 }],
            tasks:Mutex::default(), on_change:Arc::new(|_| {}), report:Mutex::default(), stopping:AtomicBool::new(false), master_closed:AtomicBool::new(false), ended:CancellationToken::new(),
        }));
        for target in ["T1", "T2"] {
            session.run_main(target.into(), main_tab(&main_browser, target, &session.0.settings).await.unwrap());
            let tab = follower_tab(&follower_browser, target).await.unwrap();
            session.0.followers[0].tabs.lock().await.extend(session.follow(0, vec![tab]));
        }
        session.spawn(session.clone().work(0, receiver));
        (session, main, follower)
    }

    async fn reattaching_browser(stall_release: bool, stall: Option<&'static str>) -> (crate::engine::page::tests::Mock, Arc<Mutex<Vec<Value>>>) {
        let sessions = Mutex::new(HashMap::<String, String>::new());
        let serial = std::sync::atomic::AtomicUsize::new(0);
        let accepted = Arc::new(Mutex::new(Vec::new()));
        let inputs = accepted.clone();
        let mock = crate::engine::page::tests::Mock::spawn(move |method, params, session| {
            let mut sessions = sessions.lock().unwrap();
            if method == "Target.attachToTarget" {
                let id = format!("S{}", serial.fetch_add(1, Ordering::SeqCst) + 1);
                sessions.insert(id.clone(), params["targetId"].as_str().unwrap().into());
                if stall == Some(method) && id == "S3" { return json!({"silent":true}); }
                return json!({"sessionId":id});
            }
            if method == "Target.detachFromTarget" {
                sessions.remove(params["sessionId"].as_str().unwrap());
                if stall == Some(method) && params["sessionId"] == "S1" { return json!({"silent":true}); }
                return json!({});
            }
            if !session.is_empty() && !sessions.contains_key(session) {
                return json!({"error":"Session with given id not found"});
            }
            if method.starts_with("Input.") {
                inputs.lock().unwrap().push(json!({"sessionId":session,"method":method,"params":params}));
                if stall_release && session == "S1" && params["type"] == "mouseReleased" { return json!({"silent":true}); }
            }
            match method {
                "Target.getTargets" if stall == Some(method) => json!({"silent":true}),
                "Target.getTargets" => json!({"targetInfos":[{"type":"page","targetId":"T2"},{"type":"page","targetId":"T1"}]}),
                "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":sessions[session],"loaderId":"L1"}}}),
                "Page.createIsolatedWorld" => json!({"executionContextId":1}),
                "Runtime.evaluate" => json!({"result":{"type":"undefined"}}),
                "Ombra.getHumanizeState" => json!({"state":{"enabled":false}}),
                _ => json!({}),
            }
        }).await;
        (mock, accepted)
    }

    fn movement(tab: usize, x: f64) -> Step {
        Step::Input { tab, action: Arc::new(Action::Move { x, y: 0.0, buttons: 0, mods: 0 }), series: false }
    }

    #[tokio::test]
    async fn unrelated_popup_cannot_consume_the_expected_linked_tab() {
        let (session, main, follower) = fixture(SyncSettings::default()).await;
        session.spawn(session.clone().watch_main_tabs(session.0.main.browser.discover_tabs().await.unwrap()));
        session.spawn(session.clone().watch_follower_tabs(0, session.0.followers[0].window.browser.discover_tabs().await.unwrap()));
        for mock in [&main, &follower] {
            // Neither blocked attempt creates a target; the successful attempt must replace both.
            mock.events.send(json!({"method":"Page.windowOpen","sessionId":"S1","params":{"userGesture":false,"url":"https://test/blocked-script"}})).unwrap();
            mock.events.send(json!({"method":"Page.windowOpen","sessionId":"S1","params":{"userGesture":true,"url":"https://test/blocked-user"}})).unwrap();
            mock.events.send(json!({"method":"Page.windowOpen","sessionId":"S1","params":{"userGesture":true}})).unwrap();
            mock.events.send(json!({"method":"Page.windowOpen","sessionId":"S2","params":{"userGesture":false}})).unwrap();
            mock.events.send(json!({"method":"Target.targetCreated","params":{"targetInfo":{"type":"page","targetId":"T98","openerId":"T2"}}})).unwrap();
            for (target, parent) in [("T99", "T88"), ("T3", "T1")] {
                mock.events.send(json!({"method":"Target.targetCreated","params":{"targetInfo":{"type":"page","targetId":target,"openerId":parent}}})).unwrap();
            }
        }
        until(|| main.sent("Target.attachToTarget").len() >= 3 && follower.sent("Target.attachToTarget").len() >= 3).await;
        assert_eq!(main.sent("Target.attachToTarget")[2]["targetId"], "T3");
        assert_eq!(follower.sent("Target.attachToTarget")[2]["targetId"], "T3");
        session.stop().await;
    }

    #[tokio::test]
    async fn linked_tab_wait_preserves_navigation_before_input_in_an_existing_tab() {
        let (session, _main, follower) = fixture(SyncSettings::default()).await;
        let slot = &session.0.followers[0].slot;
        session.main_tab_opened("T3".into(), "https://test/child".into(), Some("T1".into())).await;
        offer(slot, Step::Nav { tab:1, step:NavStep::Goto("https://test/next".into()) });
        offer(slot, movement(1,20.0));
        until(|| !follower.sent("Input.dispatchMouseEvent").is_empty()).await;
        {
            let requests = follower.requests.lock().unwrap();
            let nav = requests.iter().position(|r| r["method"]=="Page.navigate" && r["sessionId"]=="S2").expect("navigation must run before input");
            let input = requests.iter().position(|r| r["method"]=="Input.dispatchMouseEvent").unwrap();
            assert!(nav < input, "pending linked tab must not reorder another tab's navigation and input");
        }
        session.stop().await;
    }

    #[tokio::test]
    async fn closing_an_old_tab_cannot_send_pending_child_input_to_its_old_index() {
        let (session, _main, follower) = fixture(SyncSettings::default()).await;
        session.spawn(session.clone().watch_follower_tabs(0, session.0.followers[0].window.browser.discover_tabs().await.unwrap()));
        let slot = &session.0.followers[0].slot;
        session.main_tab_opened("T3".into(), "https://test/child".into(), Some("T2".into())).await;
        session.main_tab_closed("T1");
        let mut input = movement(2, 30.0);
        if let Step::Input { tab, .. } = &mut input { *tab = 1; }
        offer(slot, input);
        until(|| slot.sender.capacity() == QUEUE).await;
        assert!(follower.sent("Input.dispatchMouseEvent").is_empty());
        follower.events.send(json!({"method":"Page.windowOpen","sessionId":"S2","params":{"userGesture":true}})).unwrap();
        follower.events.send(json!({"method":"Target.targetCreated","params":{"targetInfo":{"type":"page","targetId":"T3","openerId":"T2"}}})).unwrap();
        until(|| !follower.sent("Input.dispatchMouseEvent").is_empty()).await;
        assert_eq!(follower.sessions("Input.dispatchMouseEvent"), ["S3"]);
        assert_eq!(follower.sent("Target.closeTarget")[0]["targetId"], "T1");
        session.stop().await;
    }

    #[tokio::test]
    async fn closing_a_pending_child_still_allows_its_opening_input_to_drain() {
        let (session, _main, follower) = fixture(SyncSettings::default()).await;
        session.spawn(session.clone().watch_follower_tabs(0, session.0.followers[0].window.browser.discover_tabs().await.unwrap()));
        session.main_tab_opened("T3".into(), "https://test/child".into(), Some("T1".into())).await;
        session.main_tab_closed("T3");
        offer(&session.0.followers[0].slot, movement(0,10.0));
        until(|| !follower.sent("Input.dispatchMouseEvent").is_empty()).await;
        follower.events.send(json!({"method":"Page.windowOpen","sessionId":"S1","params":{"userGesture":true}})).unwrap();
        follower.events.send(json!({"method":"Target.targetCreated","params":{"targetInfo":{"type":"page","targetId":"T3","openerId":"T1"}}})).unwrap();
        until(|| !follower.sent("Target.closeTarget").is_empty()).await;
        assert_eq!(follower.sent("Target.closeTarget")[0]["targetId"], "T3");
        assert_eq!(session.0.followers[0].slot.status(), WindowStatus::Synced);
        session.stop().await;
    }

    #[tokio::test]
    async fn releasing_during_linked_tab_attachment_detaches_the_late_session() {
        let (mock, _) = reattaching_browser(false, Some("Target.attachToTarget")).await;
        let (session, _main, follower) = fixture_with_follower(SyncSettings::default(), mock).await;
        session.spawn(session.clone().watch_follower_tabs(0, session.0.followers[0].window.browser.discover_tabs().await.unwrap()));
        session.main_tab_opened("T3".into(), "https://test/child".into(), Some("T1".into())).await;
        follower.events.send(json!({"method":"Page.windowOpen","sessionId":"S1","params":{"userGesture":true}})).unwrap();
        follower.events.send(json!({"method":"Target.targetCreated","params":{"targetInfo":{"type":"page","targetId":"T3","openerId":"T1"}}})).unwrap();
        until(|| follower.sent("Target.attachToTarget").len() == 3).await;
        session.release("follower").await.unwrap();
        let request = follower.requests.lock().unwrap().iter().rev().find(|r| r["method"]=="Target.attachToTarget").cloned().unwrap();
        follower.events.send(json!({"id":request["id"],"result":{"sessionId":"S3"}})).unwrap();
        until(|| follower.sent("Target.detachFromTarget").iter().any(|p| p["sessionId"]=="S3")).await;
        assert_eq!(session.0.followers[0].tabs.lock().await.len(),2);
        session.stop().await;
    }

    #[tokio::test]
    async fn slow_follower_does_not_block_main() {
        // Получатель жив, но не читает: очередь заполняется.
        let (sender, _receiver) = mpsc::channel(QUEUE);
        let slot = FollowerSlot::new(sender);
        tokio::time::timeout(Duration::from_secs(1), async {
            for _ in 0..1100 {
                offer(&slot, Step::Input { tab: 0, action: Arc::new(Action::Visible), series: false });
            }
        })
        .await
        .expect("главный ждал медленное окно");
        assert_eq!(slot.status(), WindowStatus::Paused { code: "failed".into(), detail: "очередь переполнена".into() });
        assert!(!offer(&slot, Step::Input { tab: 0, action: Arc::new(Action::Visible), series: false }), "окну на паузе шаги не идут");
    }
}
