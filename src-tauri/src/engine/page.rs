//! Сессия CDP для автоматизации (спецификация автоматизации, «engine::page»): одно соединение с браузером, фоновое
//! чтение ответов и событий, вкладки, переходы и JS процесса. Методы CDP знает только `engine`; `Runtime.enable`
//! не вызывается никогда — движок его глушит.
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use super::cdp::{CALL_TIMEOUT, CdpError, is_local};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{broadcast, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tokio_util::sync::CancellationToken;

/// Сколько ждать скрипт процесса (`web.js`); дольше — ошибка узла.
pub const JS_TIMEOUT: Duration = Duration::from_secs(120);
/// Событий в очереди подписчика; отставший теряет старые — ожидания ограничены своими тайм-аутами.
const EVENT_QUEUE: usize = 1024;

/// Клавиши Keys по именам (их коды — в `NAMED_KEYS`, задача ввода).
pub const KEY_NAMES: &[&str] = &[
    "Backspace", "Tab", "Enter", "Space", "Escape", "Delete", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End",
    "PageUp", "PageDown", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

/// Сколько ждать снимок большой страницы.
const SHOT_TIMEOUT: Duration = Duration::from_secs(60);

/// Клавиши Keys: имя, `key`, `code`, код Windows, текст нажатия.
const NAMED_KEYS: &[(&str, &str, &str, i64, &str)] = &[
    ("Backspace", "Backspace", "Backspace", 8, ""),
    ("Tab", "Tab", "Tab", 9, ""),
    ("Enter", "Enter", "Enter", 13, "\r"),
    ("Space", " ", "Space", 32, " "),
    ("Escape", "Escape", "Escape", 27, ""),
    ("Delete", "Delete", "Delete", 46, ""),
    ("ArrowUp", "ArrowUp", "ArrowUp", 38, ""),
    ("ArrowDown", "ArrowDown", "ArrowDown", 40, ""),
    ("ArrowLeft", "ArrowLeft", "ArrowLeft", 37, ""),
    ("ArrowRight", "ArrowRight", "ArrowRight", 39, ""),
    ("Home", "Home", "Home", 36, ""),
    ("End", "End", "End", 35, ""),
    ("PageUp", "PageUp", "PageUp", 33, ""),
    ("PageDown", "PageDown", "PageDown", 34, ""),
    ("F1", "F1", "F1", 112, ""),
    ("F2", "F2", "F2", 113, ""),
    ("F3", "F3", "F3", 114, ""),
    ("F4", "F4", "F4", 115, ""),
    ("F5", "F5", "F5", 116, ""),
    ("F6", "F6", "F6", 117, ""),
    ("F7", "F7", "F7", 118, ""),
    ("F8", "F8", "F8", 119, ""),
    ("F9", "F9", "F9", 120, ""),
    ("F10", "F10", "F10", 121, ""),
    ("F11", "F11", "F11", 122, ""),
    ("F12", "F12", "F12", 123, ""),
];

/// `noopener` может скрыть живого opener, но CDP сохраняет исходный frame инициатора.
pub(crate) fn page_opened_tab(info: &Value) -> bool {
    info["openerId"].is_string() || info["openerFrameId"].is_string()
}

/// Выбор варианта `<select>` по `value` скриптом с событиями `input` и `change`, как `select()` в Puppeteer.
const SELECT: &str = "function(value) { if (this.tagName !== 'SELECT') return 'not-select'; \
if (!Array.from(this.options).some((option) => option.value === value)) return 'no-option'; this.value = value; \
this.dispatchEvent(new Event('input', { bubbles: true })); this.dispatchEvent(new Event('change', { bubbles: true })); return 'ok'; }";
/// Прокрутка страницы: позиция и предел.
const PAGE_SCROLL: &str = "function() { const e = document.scrollingElement || document.documentElement; return [e.scrollTop, e.scrollHeight - e.clientHeight]; }";
const PAGE_SCROLL_TO: &str = "function(top) { (document.scrollingElement || document.documentElement).scrollTop = top; }";
const ELEMENT_SCROLL: &str = "function() { return [this.scrollTop, this.scrollHeight - this.clientHeight]; }";
const ELEMENT_SCROLL_TO: &str = "function(top) { this.scrollTop = top; }";

/// Кнопка мыши.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    pub(super) fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Middle => "middle",
            Button::Right => "right",
        }
    }

    /// Бит кнопки в маске `buttons`.
    pub(crate) fn mask(self) -> i64 {
        match self {
            Button::Left => 1,
            Button::Right => 2,
            Button::Middle => 4,
        }
    }
}

struct KeyDef {
    key: String,
    code: String,
    vk: i64,
    text: String,
}

/// Клавиша по имени из списка Keys или одна латинская буква либо цифра.
fn key_def(name: &str, shift: bool) -> Option<KeyDef> {
    if let Some(&(_, key, code, vk, text)) = NAMED_KEYS.iter().find(|known| known.0 == name) {
        return Some(KeyDef { key: key.into(), code: code.into(), vk, text: text.into() });
    }
    let mut chars = name.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else { return None };
    if ch.is_ascii_alphabetic() {
        let upper = ch.to_ascii_uppercase();
        let key = if shift { upper } else { ch.to_ascii_lowercase() };
        return Some(KeyDef { key: key.to_string(), code: format!("Key{upper}"), vk: upper as i64, text: key.to_string() });
    }
    if ch.is_ascii_digit() {
        return Some(KeyDef { key: ch.to_string(), code: format!("Digit{ch}"), vk: ch as i64, text: ch.to_string() });
    }
    None
}

/// Клавиша есть в списке Keys или это одна латинская буква либо цифра.
pub fn is_key(name: &str) -> bool {
    key_def(name, false).is_some()
}

/// Команды редактирования macOS для ⌘+буква и ⌘⇧Z (по полному набору модификаторов `mask`): без поля `commands`
/// Chromium по CDP их не исполняет.
pub(super) fn mac_command(name: &str, mask: i64) -> Option<&'static str> {
    match (mask, name.to_ascii_lowercase().as_str()) {
        (4, "a") => Some("selectAll"),
        (4, "c") => Some("copy"),
        (4, "v") => Some("paste"),
        (4, "x") => Some("cut"),
        (4, "z") => Some("undo"),
        (12, "z") => Some("redo"),
        _ => None,
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
/// Ждущие ответа по `id`; `None` — соединение закрыто.
type Waiters = Arc<Mutex<Option<HashMap<u64, oneshot::Sender<Result<Value, CdpError>>>>>>;

/// Событие CDP: метод, параметры и сессия вкладки (`None` — браузер).
#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub session: Option<String>,
    pub method: String,
    pub params: Value,
}

/// Вкладка браузера (цель `page`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub target_id: String,
    pub url: String,
    pub title: String,
    /// Вкладку открыла страница (`openerId`).
    pub from_page: bool,
}

/// Найденный элемент: сессия (вкладки или межсайтового фрейма), фрейм, узел DOM (`backendNodeId`) и поколение
/// документа фрейма, где он найден (`Documents`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementRef {
    session: String,
    frame_id: String,
    node: i64,
    generation: u64,
}

#[cfg(test)]
impl ElementRef {
    /// Элемент для тестов процесса: узел `node` главного фрейма `T1` в сессии `S1`, первое поколение документа.
    pub(crate) fn test(node: i64) -> Self {
        ElementRef { session: "S1".into(), frame_id: "T1".into(), node, generation: 0 }
    }
}

/// Документ фрейма, где ищутся элементы: сессия и id фрейма.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Doc {
    session: String,
    frame_id: String,
}

/// Способ поиска элемента.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    Css,
    XPath,
    Text,
}

impl By {
    fn name(self) -> &'static str {
        match self {
            By::Css => "css",
            By::XPath => "xpath",
            By::Text => "text",
        }
    }
}

/// Что прочитать у элемента.
#[derive(Debug, Clone, Copy)]
pub enum Read<'a> {
    Text,
    Html,
    Attribute(&'a str),
    Tag,
    Visible,
}

/// Поиск элементов: функция `(by, selector) => Element[]` (её текст — в `page_helpers.js`).
const FIND: &str = include_str!("page_helpers.js");
/// Имя изолированного мира процесса; странице не видно.
const WORLD: &str = "ombra";
/// Видимость: рамка ненулевого размера, нет `display: none` и `visibility: hidden` у элемента и предков.
const VISIBLE: &str = "function() { const box = this.getBoundingClientRect(); if (!box.width || !box.height) return false; \
for (let el = this; el; el = el.parentElement) { const style = getComputedStyle(el); \
if (style.display === 'none' || style.visibility === 'hidden') return false; } return true; }";

/// Сессия межсайтового фрейма и сессия его родителя (там — его `<iframe>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FrameSession {
    pub(super) session: String,
    pub(super) parent: String,
}

/// Сессии межсайтовых фреймов вкладки по id фрейма.
pub(super) type Frames = Arc<Mutex<HashMap<String, FrameSession>>>;
/// Узел пропал: переход, перерисовка, закрытая сессия фрейма или вкладки.
const GONE: &str = "Элемент больше не доступен на странице";

/// Документы вкладки: миры процесса и поколения документов фреймов. Общие у страницы и наблюдателя `watch_frames`.
/// `executionContextId` и `backendNodeId` — счётчики процесса рендерера: после смены процесса (межсайтовый переход)
/// прежний номер может достаться чужому контексту или узлу, поэтому переход фрейма убирает его мир и меняет поколение.
#[derive(Debug, Default)]
struct Documents {
    /// Мир процесса по документу: `executionContextId`.
    worlds: HashMap<Doc, i64>,
    /// Поколение документа главного фрейма; у фреймов, не переходивших после него, — то же.
    main: u64,
    /// Поколения фреймов, переходивших после главного.
    frames: HashMap<String, u64>,
    /// Последний `loaderId` фрейма: переход видят и наблюдатель, и ожидание загрузки — считается он один раз.
    loaders: HashMap<String, String>,
    next: u64,
}

impl Documents {
    fn generation(&self, frame: &str) -> u64 {
        self.frames.get(frame).copied().unwrap_or(self.main)
    }

    /// Фрейм `frame` сменил документ: главный — вместе со всеми фреймами вкладки. Пустой `loader` — переход, загрузчик
    /// которого не запоминается (`goto` без ожидания): событие о нём отметит смену ещё раз.
    fn navigated(&mut self, frame: &str, main: bool, loader: &str) {
        if self.loaders.get(frame).is_some_and(|known| known == loader) {
            return;
        }
        self.next += 1;
        if main {
            self.worlds.clear();
            self.frames.clear();
            self.loaders.clear();
            self.main = self.next;
        } else {
            self.worlds.retain(|doc, _| doc.frame_id != frame);
            self.frames.insert(frame.to_string(), self.next);
        }
        if !loader.is_empty() {
            self.loaders.insert(frame.to_string(), loader.to_string());
        }
    }
}

/// Событие фрейма сессии вкладки (`from_tab`) или межсайтового фрейма: переход меняет документ; ушедший фрейм
/// (удалён или перешёл в свой процесс) уносит свой мир. Главный фрейм вкладки — только фрейм без родителя из сессии
/// вкладки; корень сессии фрейма без `parentId` (например, fenced frame) — подфрейм по своему id.
fn frame_event(docs: &Mutex<Documents>, event: &CdpEvent, from_tab: bool) {
    let mut docs = docs.lock().expect("мьютекс документов отравлен");
    match event.method.as_str() {
        "Page.frameNavigated" => {
            let frame = &event.params["frame"];
            if let (Some(id), Some(loader)) = (frame["id"].as_str(), frame["loaderId"].as_str()) {
                docs.navigated(id, from_tab && frame["parentId"].is_null(), loader);
            }
        }
        "Page.frameDetached" => {
            if let Some(id) = event.params["frameId"].as_str() {
                docs.worlds.retain(|doc, _| doc.frame_id != id);
            }
        }
        _ => {}
    }
}

/// Сколько ждать сессию межсайтового фрейма, который подключится чуть позже.
const FRAME_WAIT: Duration = Duration::from_secs(2);
const FRAME_POLL: Duration = Duration::from_millis(100);
/// Автоподключение дочерних целей сессии: межсайтовые фреймы приходят своими сессиями; воркеры и прочие цели —
/// не нужны процессу и не подключаются.
fn auto_attach() -> Value {
    json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true, "filter": [{ "type": "iframe" }, { "exclude": true }] })
}

/// Соединение с браузером; клоны делят одно соединение.
#[derive(Clone)]
pub struct Browser {
    conn: Arc<Conn>,
}

struct Conn {
    sink: tokio::sync::Mutex<SplitSink<Socket, Message>>,
    waiters: Waiters,
    next_id: AtomicU64,
    /// Образец подписки (`resubscribe`). Отправитель — только у задачи чтения: когда она кончилась, подписчики
    /// получают `Closed`.
    events: Mutex<broadcast::Receiver<CdpEvent>>,
    stop: CancellationToken,
    /// Отменяется задачей чтения, когда соединение кончилось (`Browser::closed`).
    closed: CancellationToken,
}

impl Drop for Conn {
    /// Последний клон отпущен — задача чтения кончается, соединение закрывается.
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

struct Pending<'a> {
    browser: &'a Browser,
    id: u64,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.browser.forget(self.id);
    }
}

impl Browser {
    /// Только `ws://127.0.0.1:<порт>/…`: отладочный порт браузера не должен уводить за пределы машины.
    pub async fn connect(ws_url: &str) -> Result<Self, CdpError> {
        if !is_local(ws_url) {
            return Err(CdpError::NotLocal);
        }
        let (socket, _) = tokio::time::timeout(CALL_TIMEOUT, connect_async(ws_url))
            .await
            .map_err(|_| CdpError::Timeout)?
            .map_err(|e| CdpError::Connect(e.to_string()))?;
        let (sink, stream) = socket.split();
        let waiters: Waiters = Arc::new(Mutex::new(Some(HashMap::new())));
        let (sender, receiver) = broadcast::channel(EVENT_QUEUE);
        let stop = CancellationToken::new();
        let closed = CancellationToken::new();
        tokio::spawn(read(stream, waiters.clone(), sender, stop.clone(), closed.clone()));
        let conn = Conn { sink: tokio::sync::Mutex::new(sink), waiters, next_id: AtomicU64::new(0), events: Mutex::new(receiver), stop, closed };
        Ok(Self { conn: Arc::new(conn) })
    }

    /// Ждёт обрыва соединения; уже оборванное — возвращается сразу. Соединение не удерживает.
    pub async fn closed(&self) {
        self.conn.closed.cancelled().await
    }

    #[cfg(test)]
    pub(crate) async fn close(&self) {
        let _ = self.call(None, "Browser.close", json!({})).await;
    }

    /// Вкладки в порядке `Target.getTargets`.
    pub async fn tabs(&self) -> Result<Vec<Tab>, CdpError> {
        let reply = self.call(None, "Target.getTargets", json!({})).await?;
        Ok(reply["targetInfos"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter(|target| target["type"] == "page")
                    .filter_map(|target| {
                        Some(Tab {
                            target_id: target["targetId"].as_str()?.to_string(),
                            url: target["url"].as_str().unwrap_or_default().to_string(),
                            title: target["title"].as_str().unwrap_or_default().to_string(),
                            from_page: page_opened_tab(target),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Новая вкладка; ответ — её id.
    pub async fn new_tab(&self, url: &str) -> Result<String, CdpError> {
        let reply = self.call(None, "Target.createTarget", json!({ "url": url })).await?;
        reply["targetId"].as_str().map(str::to_string).ok_or_else(|| CdpError::Protocol({
            log::warn!("нет targetId");
            "Не удалось получить данные вкладки"
        }.into()))
    }

    pub async fn close_tab(&self, target_id: &str) -> Result<(), CdpError> {
        self.call(None, "Target.closeTarget", json!({ "targetId": target_id })).await.map(|_| ())
    }

    /// Сессия вкладки (`flatten`) с доменом `Page`: его события нужны ожиданию загрузки.
    pub async fn attach(&self, target_id: &str) -> Result<Page, CdpError> {
        let reply = self.call(None, "Target.attachToTarget", json!({ "targetId": target_id, "flatten": true })).await?;
        let session = reply["sessionId"].as_str().ok_or_else(|| CdpError::Protocol({
            log::warn!("нет sessionId");
            "Не удалось получить данные сеанса управления"
        }.into()))?.to_string();
        let frames: Frames = Arc::default();
        let files_intercepted: Arc<AtomicBool> = Arc::default();
        let docs: Arc<Mutex<Documents>> = Arc::default();
        let stop = CancellationToken::new();
        // Подписка — до автоподключения: уже открытые межсайтовые фреймы приходят событиями сразу.
        let events = self.subscribe();
        let watched = Watched { frames: frames.clone(), files_intercepted: files_intercepted.clone(), docs: docs.clone() };
        tokio::spawn(watch_frames(self.clone(), session.clone(), events, watched, stop.clone()));
        let page = Page {
            browser: self.clone(),
            session,
            target_id: target_id.to_string(),
            docs,
            frames,
            files_intercepted,
            owners: Mutex::default(),
            net: Arc::default(),
            stop,
        };
        page.call("Page.enable", json!({})).await?;
        page.call("Target.setAutoAttach", auto_attach()).await?;
        Ok(page)
    }

    pub(super) fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.conn.events.lock().expect("мьютекс событий отравлен").resubscribe()
    }

    pub(super) async fn call(&self, session: Option<&str>, method: &str, params: Value) -> Result<Value, CdpError> {
        self.call_for(session, method, params, CALL_TIMEOUT).await
    }

    pub(super) async fn call_for(&self, session: Option<&str>, method: &str, params: Value, wait: Duration) -> Result<Value, CdpError> {
        let id = self.conn.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        match self.conn.waiters.lock().expect("мьютекс ответов отравлен").as_mut() {
            Some(waiters) => {
                waiters.insert(id, tx);
            }
            None => return Err(CdpError::Closed),
        }
        let mut request = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        // Ждущий убирается и тогда, когда вызов брошен (future отменён «Стопом»).
        let _pending = Pending { browser: self, id };
        let mut sink = self.conn.sink.lock().await;
        if sink.send(Message::text(request.to_string())).await.is_err() {
            return Err(CdpError::Closed);
        }
        drop(sink);
        let reply = tokio::time::timeout(wait, rx).await;
        match reply {
            Ok(Ok(result)) => result,
            // Отправитель сброшен: задача чтения кончилась вместе с соединением.
            Ok(Err(_)) => Err(CdpError::Closed),
            Err(_) => Err(CdpError::Timeout),
        }
    }

    fn forget(&self, id: u64) {
        if let Some(waiters) = self.conn.waiters.lock().expect("мьютекс ответов отравлен").as_mut() {
            waiters.remove(&id);
        }
    }
}

/// Задача чтения: ответы — ждущим по `id`, события — подписчикам.
async fn read(mut stream: SplitStream<Socket>, waiters: Waiters, events: broadcast::Sender<CdpEvent>, stop: CancellationToken, closed: CancellationToken) {
    loop {
        let message = tokio::select! {
            _ = stop.cancelled() => break,
            message = stream.next() => message,
        };
        let Some(Ok(message)) = message else { break };
        let Message::Text(text) = message else { continue };
        let Ok(reply) = serde_json::from_str::<Value>(text.as_str()) else { continue };
        if let Some(id) = reply["id"].as_u64() {
            let waiter = waiters.lock().expect("мьютекс ответов отравлен").as_mut().and_then(|map| map.remove(&id));
            if let Some(waiter) = waiter {
                let result = match reply.get("error") {
                    Some(error) => Err(CdpError::Remote(error["message"].as_str().unwrap_or("без описания").to_string())),
                    None => Ok(reply.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiter.send(result);
            }
        } else if let Some(method) = reply["method"].as_str() {
            let session = reply["sessionId"].as_str().map(str::to_string);
            let event = CdpEvent { session, method: method.to_string(), params: reply["params"].clone() };
            let _ = events.send(event);
        }
    }
    // Ждущие получают `Closed`: их отправители сбрасываются вместе с картой; новые вызовы видят `None`.
    waiters.lock().expect("мьютекс ответов отравлен").take();
    closed.cancel();
}

/// Сессия вкладки.
pub struct Page {
    pub(super) browser: Browser,
    pub(super) session: String,
    /// Id главного фрейма совпадает с id вкладки.
    pub(super) target_id: String,
    /// Миры процесса и поколения документов (общие с `watch_frames`).
    docs: Arc<Mutex<Documents>>,
    /// Сессии межсайтовых фреймов (наполняет `watch_frames`).
    pub(super) frames: Frames,
    /// Окно выбора файла не открывается и во фреймах: `watch_frames` перехватывает его в их новых сессиях. Читается и
    /// пишется под мьютексом `frames` — фрейм, вносимый в карту, не разминётся с включением.
    pub(super) files_intercepted: Arc<AtomicBool>,
    /// Владельцы межсайтовых фреймов: сессия фрейма → элемент `<iframe>` в родителе (для сдвига координат).
    owners: Mutex<HashMap<String, ElementRef>>,
    /// Сколько частей процесса держат сеть вкладки включённой (общий счётчик всех `Network` этой страницы).
    net: Arc<tokio::sync::Mutex<usize>>,
    /// Останавливает `watch_frames`, когда страница отпущена.
    stop: CancellationToken,
}

impl Drop for Page {
    /// Наблюдатель фреймов держит клон соединения — отпускаем его вместе со страницей.
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl Page {
    /// Страница без настоящей сессии — для тестов, где важно лишь, какие вызовы уходят браузеру.
    #[cfg(test)]
    pub(crate) fn detached(browser: &Browser) -> Self {
        Self {
            browser: browser.clone(),
            session: "S0".into(), target_id: "T0".into(),
            docs: Arc::default(),
            frames: Arc::default(),
            files_intercepted: Arc::default(),
            owners: Mutex::default(),
            net: Arc::default(),
            stop: CancellationToken::new(),
        }
    }

    /// Сеть вкладки (события запросов, тела); включается охранником `Network::acquire`.
    pub fn network(&self) -> crate::engine::network::Network {
        crate::engine::network::Network::new(self.browser.clone(), self.session.clone(), self.net.clone())
    }

    /// Cookies адреса `url` (как их отправил бы браузер); партиционированные не берутся.
    pub async fn cookies(&self, url: &str) -> Result<Vec<crate::cookies::Cookie>, CdpError> {
        let reply = self.call("Network.getCookies", json!({ "urls": [url] })).await?;
        Ok(reply["cookies"].as_array().into_iter().flatten().filter_map(super::cdp::from_cdp).collect())
    }

    /// Удалить cookies: по имени, домену и пути каждой.
    pub async fn delete_cookies(&self, cookies: &[crate::cookies::Cookie]) -> Result<(), CdpError> {
        for cookie in cookies {
            self.call("Network.deleteCookies", json!({ "name": cookie.name, "domain": cookie.domain, "path": cookie.path })).await?;
        }
        Ok(())
    }

    /// Отцепить сессию вкладки: иначе они копятся на соединении, пока оно живо.
    pub async fn detach(&self) -> Result<(), CdpError> {
        self.browser.call(None, "Target.detachFromTarget", json!({ "sessionId": self.session })).await.map(|_| ())
    }

    pub(super) async fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
        self.browser.call(Some(&self.session), method, params).await
    }

    pub async fn mouse_move(&self, x: f64, y: f64) -> Result<(), CdpError> {
        self.call("Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": x, "y": y })).await.map(|_| ())
    }

    /// Клик: движение, нажатие и отпускание через `hold`; двойной — две пары с `clickCount` 1 и 2.
    pub async fn click(&self, x: f64, y: f64, button: Button, count: u32, hold: Duration) -> Result<(), CdpError> {
        self.mouse_move(x, y).await?;
        for n in 1..=count {
            let press = json!({ "type": "mousePressed", "x": x, "y": y, "button": button.name(), "buttons": button.mask(), "clickCount": n });
            self.call("Input.dispatchMouseEvent", press).await?;
            if !hold.is_zero() {
                tokio::time::sleep(hold).await;
            }
            let release = json!({ "type": "mouseReleased", "x": x, "y": y, "button": button.name(), "buttons": 0, "clickCount": n });
            self.call("Input.dispatchMouseEvent", release).await?;
        }
        Ok(())
    }

    /// Колесо мыши над точкой: `dy` > 0 — вниз.
    pub async fn wheel(&self, x: f64, y: f64, dy: f64) -> Result<(), CdpError> {
        self.call("Input.dispatchMouseEvent", json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": 0, "deltaY": dy })).await.map(|_| ())
    }

    pub async fn focus(&self, element: &ElementRef) -> Result<(), CdpError> {
        self.same_document(element)?;
        self.browser.call(Some(&element.session), "DOM.focus", json!({ "backendNodeId": element.node })).await.map_err(gone).map(|_| ())
    }

    /// Файлы в `<input type=file>` (`DOM.setFileInputFiles`; поле получает `input` и `change`); другой элемент — ошибка.
    pub async fn set_files(&self, element: &ElementRef, files: &[String]) -> Result<(), CdpError> {
        let check = self.call_on(element, "function() { return this.tagName === 'INPUT' && this.type === 'file'; }", vec![], true).await?;
        if check["value"] != Value::Bool(true) {
            return Err(CdpError::Page("элемент — не поле выбора файла".into()));
        }
        self.same_document(element)?;
        let params = json!({ "files": files, "backendNodeId": element.node });
        self.browser.call(Some(&element.session), "DOM.setFileInputFiles", params).await.map_err(gone)?;
        Ok(())
    }

    /// Текст одной командой: при включённом очеловечивании движок разворачивает её в нажатия по персоне.
    pub async fn insert_text(&self, text: &str) -> Result<(), CdpError> {
        self.call("Input.insertText", json!({ "text": text })).await.map(|_| ())
    }

    /// Один символ нажатием и отпусканием; перевод строки — Enter. Латиница, цифры и пробел — с кодом клавиши, как
    /// с клавиатуры; прочие символы — только текстом.
    pub async fn type_char(&self, ch: char) -> Result<(), CdpError> {
        if ch == '\n' {
            return self.press("Enter", &[]).await;
        }
        let text = ch.to_string();
        let mut down = json!({ "type": "keyDown", "key": text, "text": text, "unmodifiedText": text });
        let mut up = json!({ "type": "keyUp", "key": text });
        if let Some(def) = key_def(if ch == ' ' { "Space" } else { &text }, ch.is_ascii_uppercase()) {
            for event in [&mut down, &mut up] {
                event["code"] = json!(def.code);
                event["windowsVirtualKeyCode"] = json!(def.vk);
            }
        }
        self.call("Input.dispatchKeyEvent", down).await?;
        self.call("Input.dispatchKeyEvent", up).await.map(|_| ())
    }

    /// Клавиша с модификаторами (`cmd`, `ctrl`, `alt`, `shift`): нажатие и отпускание.
    pub async fn press(&self, name: &str, modifiers: &[&str]) -> Result<(), CdpError> {
        let mask: i64 = modifiers.iter().map(|m| match *m { "alt" => 1, "ctrl" => 2, "cmd" => 4, "shift" => 8, _ => 0 }).sum();
        let def = key_def(name, mask & 8 != 0).ok_or_else(|| CdpError::Protocol(format!("неизвестная клавиша {name}")))?;
        // С ⌘, Ctrl или Alt нажатие текста не печатает.
        let text = if mask & 7 == 0 { def.text.clone() } else { String::new() };
        let kind = if text.is_empty() { "rawKeyDown" } else { "keyDown" };
        let mut down = json!({ "type": kind, "modifiers": mask, "key": def.key, "code": def.code, "windowsVirtualKeyCode": def.vk });
        if !text.is_empty() {
            down["text"] = json!(text);
            down["unmodifiedText"] = json!(text);
        }
        if let Some(command) = mac_command(name, mask) {
            down["commands"] = json!([command]);
        }
        self.call("Input.dispatchKeyEvent", down).await?;
        let up = json!({ "type": "keyUp", "modifiers": mask, "key": def.key, "code": def.code, "windowsVirtualKeyCode": def.vk });
        self.call("Input.dispatchKeyEvent", up).await.map(|_| ())
    }

    /// Центр видимой области — точка колеса при прокрутке страницы.
    pub async fn viewport_center(&self) -> Result<(f64, f64), CdpError> {
        let metrics = self.call("Page.getLayoutMetrics", json!({})).await?;
        let viewport = &metrics["cssLayoutViewport"];
        Ok((viewport["clientWidth"].as_f64().unwrap_or(0.0) / 2.0, viewport["clientHeight"].as_f64().unwrap_or(0.0) / 2.0))
    }

    /// Снимок вкладки: `format` — `png` или `jpeg` (с `quality`); `full_page` — вся страница, не только видимая часть.
    pub async fn screenshot(&self, format: &str, quality: i64, full_page: bool) -> Result<Vec<u8>, CdpError> {
        let mut params = json!({ "format": format });
        if format == "jpeg" {
            params["quality"] = json!(quality);
        }
        if full_page {
            let metrics = self.call("Page.getLayoutMetrics", json!({})).await?;
            let size = &metrics["cssContentSize"];
            params["captureBeyondViewport"] = json!(true);
            params["clip"] = json!({ "x": 0, "y": 0, "width": size["width"], "height": size["height"], "scale": 1 });
        }
        self.capture(params).await
    }

    async fn capture(&self, params: Value) -> Result<Vec<u8>, CdpError> {
        let reply = self.browser.call_for(Some(&self.session), "Page.captureScreenshot", params, SHOT_TIMEOUT).await?;
        let data = reply["data"].as_str().ok_or_else(|| CdpError::Protocol({
            log::warn!("В ответе браузера отсутствует data.");
            "Не удалось получить снимок страницы"
        }.into()))?;
        B64.decode(data).map_err(|_| CdpError::Protocol("Не удалось прочитать снимок страницы".into()))
    }

    /// Вариант `<select>` по `value`; `false` — такого варианта нет.
    pub async fn select(&self, element: &ElementRef, value: &str) -> Result<bool, CdpError> {
        let status = self.call_on(element, SELECT, vec![json!(value)], true).await?;
        match status["value"].as_str() {
            Some("ok") => Ok(true),
            Some("no-option") => Ok(false),
            Some("not-select") => Err(CdpError::Page("элемент — не выпадающий список".into())),
            _ => Err(CdpError::Protocol("неожиданный ответ выбора варианта".into())),
        }
    }

    /// Прокрутка страницы (`None`) или элемента-контейнера: позиция и предел.
    pub async fn scroll_state(&self, element: Option<&ElementRef>) -> Result<(f64, f64), CdpError> {
        let reply = match element {
            Some(element) => self.call_on(element, ELEMENT_SCROLL, vec![], true).await?,
            None => self.call_in(&self.main_doc(), PAGE_SCROLL, vec![], true).await?,
        };
        Ok((reply["value"][0].as_f64().unwrap_or(0.0), reply["value"][1].as_f64().unwrap_or(0.0)))
    }

    /// Сразу к позиции (Scroll «Auto»).
    pub async fn scroll_to(&self, element: Option<&ElementRef>, top: f64) -> Result<(), CdpError> {
        match element {
            Some(element) => self.call_on(element, ELEMENT_SCROLL_TO, vec![json!(top)], true).await.map(|_| ()),
            None => self.call_in(&self.main_doc(), PAGE_SCROLL_TO, vec![json!(top)], true).await.map(|_| ()),
        }
    }

    /// Адрес вкладки.
    pub async fn url(&self) -> Result<String, CdpError> {
        let reply = self.browser.call(None, "Target.getTargetInfo", json!({ "targetId": self.target_id })).await?;
        reply["targetInfo"]["url"].as_str().map(str::to_string).ok_or_else(|| CdpError::Protocol("нет url".into()))
    }

    /// Переход с ожиданием `load` не дольше `wait`. Переход внутри документа (якорь) загрузки не ждёт.
    pub async fn goto(&self, url: &str, wait: Duration) -> Result<(), CdpError> {
        // Тайм-аут 0 — переход без ожидания загрузки: команда уходит, операция идёт дальше. Документ отмечается
        // сменённым сразу (элемент, найденный до перехода, уже не на странице), а событие перехода отметит смену
        // ещё раз: найденное между ответом и событием может быть ещё из прежнего документа.
        if wait.is_zero() {
            let reply = self.call("Page.navigate", json!({ "url": url })).await?;
            if let Some(error) = reply["errorText"].as_str().filter(|error| !error.is_empty()) {
                return Err(CdpError::Remote(error.to_string()));
            }
            if reply["loaderId"].as_str().is_some_and(|loader| !loader.is_empty()) {
                self.docs().navigated(&self.target_id, true, "");
            }
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + wait;
        let mut events = self.browser.subscribe();
        // Один срок на команду и на ожидание загрузки.
        let reply = self.browser.call_for(Some(&self.session), "Page.navigate", json!({ "url": url }), wait).await?;
        if let Some(error) = reply["errorText"].as_str().filter(|error| !error.is_empty()) {
            return Err(CdpError::Remote(error.to_string()));
        }
        if reply["loaderId"].as_str().is_none_or(str::is_empty) {
            return Ok(());
        }
        let loader = reply["loaderId"].as_str().map(str::to_string);
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        self.wait_load(&mut events, left, LoadWait::new(&self.target_id, loader, false)).await
    }

    pub async fn reload(&self, wait: Duration) -> Result<(), CdpError> {
        let mut events = self.browser.subscribe();
        self.call("Page.reload", json!({})).await?;
        self.wait_load(&mut events, wait, LoadWait::new(&self.target_id, None, false)).await
    }

    /// Назад по истории вкладки. Истории нет — `false`, ничего не происходит.
    pub async fn back(&self, wait: Duration) -> Result<bool, CdpError> {
        let history = self.call("Page.getNavigationHistory", json!({})).await?;
        let index = history["currentIndex"].as_u64().unwrap_or(0) as usize;
        let Some(entry) = index.checked_sub(1).map(|previous| history["entries"][previous]["id"].clone()) else {
            return Ok(false);
        };
        let mut events = self.browser.subscribe();
        self.call("Page.navigateToHistoryEntry", json!({ "entryId": entry })).await?;
        self.wait_load(&mut events, wait, LoadWait::new(&self.target_id, None, true)).await?;
        Ok(true)
    }

    /// Скрипт процесса (`web.js`) в главном мире страницы: ему нужны объекты сайта. Переменные `inject` объявлены
    /// константами в блоке перед кодом; ответ — значение последнего выражения, промис ожидается.
    pub async fn eval_main(&self, code: &str, inject: &[(String, Value)]) -> Result<Value, CdpError> {
        let params = json!({ "expression": block(code, inject), "awaitPromise": true, "returnByValue": true, "timeout": JS_TIMEOUT.as_millis() as u64 });
        let result = result_of(self.browser.call_for(Some(&self.session), "Runtime.evaluate", params, JS_TIMEOUT).await?)?;
        Ok(result["value"].clone())
    }

    /// Главный документ вкладки (id главного фрейма — id вкладки).
    pub fn main_doc(&self) -> Doc {
        Doc { session: self.session.clone(), frame_id: self.target_id.clone() }
    }

    /// Сколько элементов находит `selector` в документе.
    pub async fn count(&self, doc: &Doc, by: By, selector: &str) -> Result<usize, CdpError> {
        let declaration = format!("function(by, selector) {{ return ({FIND})(by, selector).length; }}");
        let result = self.call_in(doc, &declaration, vec![json!(by.name()), json!(selector)], true).await?;
        Ok(result["value"].as_u64().unwrap_or(0) as usize)
    }

    /// Найденный элемент с номером `index` (с 0); за концом списка — `None`.
    pub async fn nth(&self, doc: &Doc, by: By, selector: &str, index: usize) -> Result<Option<ElementRef>, CdpError> {
        // Поколение — до поиска: документ, сменившийся во время него, не выдаст элемент за свой.
        let generation = self.generation(&doc.frame_id);
        let declaration = format!("function(by, selector, index) {{ return ({FIND})(by, selector)[index] || null; }}");
        let object = self.call_in(doc, &declaration, vec![json!(by.name()), json!(selector), json!(index)], false).await?;
        self.element_of(&doc.session, &doc.frame_id, generation, &object).await
    }

    /// Все найденные элементы по порядку документа — одним поиском: массив, затем его элементы.
    pub async fn all(&self, doc: &Doc, by: By, selector: &str) -> Result<Vec<ElementRef>, CdpError> {
        let generation = self.generation(&doc.frame_id);
        let declaration = format!("function(by, selector) {{ return ({FIND})(by, selector); }}");
        let list = self.call_in(doc, &declaration, vec![json!(by.name()), json!(selector)], false).await?;
        let Some(list_id) = list["objectId"].as_str() else { return Ok(Vec::new()) };
        let reply = self.browser.call(Some(&doc.session), "Runtime.getProperties", json!({ "objectId": list_id, "ownProperties": true })).await;
        let mut items: Vec<(usize, Value)> = match &reply {
            Ok(reply) => reply["result"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|property| Some((property["name"].as_str()?.parse().ok()?, property["value"].clone())))
                .collect(),
            Err(_) => Vec::new(),
        };
        items.sort_by_key(|(index, _)| *index);
        // Каждый объект отпускается, даже если описать соседний не удалось. После вызова без ответа описание
        // прекращается: каждый следующий ждал бы столько же; неописанные объекты уйдут вместе с документом.
        let (mut found, mut failed) = (Vec::with_capacity(items.len()), None);
        for (_, object) in items {
            match self.element_of(&doc.session, &doc.frame_id, generation, &object).await {
                Ok(element) => found.extend(element),
                Err(e) if unanswered(&e) => {
                    failed = Some(e);
                    break;
                }
                Err(e) => failed = failed.or(Some(e)),
            }
        }
        // Список отпускается и после тайм-аута элемента, но не после обрыва и не после списка без ответа.
        if !reply.as_ref().is_err_and(unanswered) && !matches!(failed, Some(CdpError::Closed)) {
            self.release(&doc.session, list_id).await;
        }
        reply?;
        failed.map_or(Ok(found), Err)
    }

    pub async fn read(&self, element: &ElementRef, what: Read<'_>) -> Result<Value, CdpError> {
        let (declaration, args) = match what {
            Read::Text => ("function() { return this.innerText; }", vec![]),
            Read::Html => ("function() { return this.innerHTML; }", vec![]),
            Read::Attribute(name) => ("function(name) { return this.getAttribute(name) ?? ''; }", vec![json!(name)]),
            Read::Tag => ("function() { return this.tagName; }", vec![]),
            Read::Visible => (VISIBLE, vec![]),
        };
        Ok(self.call_on(element, declaration, args, true).await?["value"].clone())
    }

    /// Элемент ещё на странице: документ тот же и узел в DOM (`isConnected`).
    pub async fn connected(&self, element: &ElementRef) -> Result<bool, CdpError> {
        match self.call_on(element, "function() { return this.isConnected; }", vec![], true).await {
            Ok(reply) => Ok(reply["value"] == true),
            Err(CdpError::Page(message)) if message == GONE => Ok(false),
            Err(other) => Err(other),
        }
    }

    /// Первый потомок по CSS-селектору; нет — `None`.
    pub async fn child(&self, element: &ElementRef, selector: &str) -> Result<Option<ElementRef>, CdpError> {
        let object = self.call_on(element, "function(selector) { return this.querySelector(selector); }", vec![json!(selector)], false).await?;
        self.element_of(&element.session, &element.frame_id, element.generation, &object).await
    }

    /// Элемент с фокусом в главном документе.
    pub async fn focused(&self) -> Result<Option<ElementRef>, CdpError> {
        let doc = self.main_doc();
        let generation = self.generation(&doc.frame_id);
        let object = self.call_in(&doc, "function() { return document.activeElement; }", vec![], false).await?;
        self.element_of(&doc.session, &doc.frame_id, generation, &object).await
    }

    /// Скрипт процесса в изолированном мире документа фрейма: DOM доступен, объекты сайта — нет.
    pub async fn eval_in(&self, doc: &Doc, code: &str, inject: &[(String, Value)]) -> Result<Value, CdpError> {
        let script = block(code, inject);
        let reply = self.in_world(doc, |context| self.evaluate(doc, context, &script)).await?;
        Ok(reply["value"].clone())
    }

    async fn evaluate(&self, doc: &Doc, context: i64, script: &str) -> Result<Value, CdpError> {
        let params = json!({ "expression": script, "contextId": context, "awaitPromise": true, "returnByValue": true, "timeout": JS_TIMEOUT.as_millis() as u64 });
        result_of(self.browser.call_for(Some(&doc.session), "Runtime.evaluate", params, JS_TIMEOUT).await?)
    }

    /// Мир процесса в документе; `fresh` — создать заново (прежний устарел после перехода).
    async fn world(&self, doc: &Doc, fresh: bool) -> Result<i64, CdpError> {
        if !fresh
            && let Some(context) = self.docs().worlds.get(doc)
        {
            return Ok(*context);
        }
        let generation = self.generation(&doc.frame_id);
        let reply = self.browser.call(Some(&doc.session), "Page.createIsolatedWorld", json!({ "frameId": doc.frame_id, "worldName": WORLD })).await?;
        let context = reply["executionContextId"].as_i64().ok_or_else(|| CdpError::Protocol({
            log::warn!("нет executionContextId");
            "Не удалось получить данные контекста страницы"
        }.into()))?;
        // Документ сменился, пока создавался мир: в кэш его номер не кладём — он прежнего процесса.
        let mut docs = self.docs();
        if docs.generation(&doc.frame_id) == generation {
            docs.worlds.insert(doc.clone(), context);
        }
        Ok(context)
    }

    fn docs(&self) -> std::sync::MutexGuard<'_, Documents> {
        self.docs.lock().expect("мьютекс документов отравлен")
    }

    /// Поколение документа фрейма (`Documents`).
    pub(crate) fn generation(&self, frame: &str) -> u64 {
        self.docs().generation(frame)
    }

    /// Документ элемента не сменился; иначе — «элемент больше не на странице» до любого вызова `DOM.*`: его номер
    /// узла мог достаться чужому узлу нового процесса.
    fn same_document(&self, element: &ElementRef) -> Result<(), CdpError> {
        if self.generation(&element.frame_id) == element.generation { Ok(()) } else { Err(CdpError::Page(GONE.into())) }
    }

    /// Вызов в мире документа; мир устарел (страница сменила документ) — ещё раз с новым.
    async fn in_world<'s, F, Fut>(&'s self, doc: &Doc, call: F) -> Result<Value, CdpError>
    where
        F: Fn(i64) -> Fut,
        Fut: std::future::Future<Output = Result<Value, CdpError>> + 's,
    {
        let context = self.world(doc, false).await?;
        match call(context).await {
            Err(error) if stale_world(&error) => call(self.world(doc, true).await?).await,
            other => other,
        }
    }

    /// Функция `declaration` в мире документа; ответ — объект результата `Runtime`.
    async fn call_in(&self, doc: &Doc, declaration: &str, args: Vec<Value>, by_value: bool) -> Result<Value, CdpError> {
        let arguments: Vec<Value> = args.into_iter().map(|value| json!({ "value": value })).collect();
        self.in_world(doc, |context| {
            let params = json!({ "functionDeclaration": declaration, "executionContextId": context, "arguments": arguments, "returnByValue": by_value, "awaitPromise": true });
            async move { result_of(self.browser.call(Some(&doc.session), "Runtime.callFunctionOn", params).await?) }
        })
        .await
    }

    /// Функция `declaration` с элементом в `this`.
    async fn call_on(&self, element: &ElementRef, declaration: &str, args: Vec<Value>, by_value: bool) -> Result<Value, CdpError> {
        self.same_document(element)?;
        let doc = Doc { session: element.session.clone(), frame_id: element.frame_id.clone() };
        let arguments: Vec<Value> = args.into_iter().map(|value| json!({ "value": value })).collect();
        self.in_world(&doc, |context| {
            let arguments = arguments.clone();
            async move {
                let object = self.resolve(element, context).await?;
                let params = json!({ "functionDeclaration": declaration, "objectId": object, "arguments": arguments, "returnByValue": by_value, "awaitPromise": true });
                let reply = self.browser.call(Some(&element.session), "Runtime.callFunctionOn", params).await;
                if !reply.as_ref().is_err_and(unanswered) {
                    self.release(&element.session, &object).await;
                }
                result_of(reply?)
            }
        })
        .await
        .map_err(gone)
    }

    /// Отпустить временный объект `Runtime` (элемент или список): иначе страница держит его до перехода. Ошибка не
    /// важна — объекта уже нет вместе с документом или сессией. После вызова без ответа не зовётся (`unanswered`).
    async fn release(&self, session: &str, object: &str) {
        let _ = self.browser.call(Some(session), "Runtime.releaseObject", json!({ "objectId": object })).await;
    }

    /// Объект элемента в мире `context`.
    async fn resolve(&self, element: &ElementRef, context: i64) -> Result<String, CdpError> {
        let params = json!({ "backendNodeId": element.node, "executionContextId": context });
        let reply = self.browser.call(Some(&element.session), "DOM.resolveNode", params).await.map_err(gone)?;
        reply["object"]["objectId"].as_str().map(str::to_string).ok_or_else(|| CdpError::Protocol({
            log::warn!("нет objectId");
            "Не удалось получить данные элемента страницы"
        }.into()))
    }

    /// Ссылка на элемент документа поколения `generation` из объекта результата (объект затем отпускается);
    /// `null` — `None`.
    async fn element_of(&self, session: &str, frame_id: &str, generation: u64, object: &Value) -> Result<Option<ElementRef>, CdpError> {
        let Some(object_id) = object["objectId"].as_str() else { return Ok(None) };
        let reply = self.browser.call(Some(session), "DOM.describeNode", json!({ "objectId": object_id })).await;
        if !reply.as_ref().is_err_and(unanswered) {
            self.release(session, object_id).await;
        }
        let node = reply?["node"]["backendNodeId"].as_i64().ok_or_else(|| CdpError::Protocol({
            log::warn!("нет backendNodeId");
            "Не удалось получить данные элемента страницы"
        }.into()))?;
        Ok(Some(ElementRef { session: session.to_string(), frame_id: frame_id.to_string(), node, generation }))
    }

    /// Документ фрейма, которым владеет элемент `<iframe>`: в той же сессии или в сессии межсайтового фрейма.
    /// Сессию межсайтового фрейма ждёт до `FRAME_WAIT`, и из Element appears тоже: попытка может пережить его срок.
    pub async fn frame_doc(&self, owner: &ElementRef) -> Result<Doc, CdpError> {
        self.same_document(owner)?;
        let reply = self.browser.call(Some(&owner.session), "DOM.describeNode", json!({ "backendNodeId": owner.node })).await.map_err(gone)?;
        let frame_id = reply["node"]["frameId"].as_str().filter(|id| !id.is_empty()).ok_or_else(|| CdpError::Page("элемент — не фрейм".into()))?.to_string();
        if let Some(doc) = self.cross_site(owner, &frame_id) {
            return Ok(doc);
        }
        // Фрейм того же процесса принимает мир в сессии владельца (готовый — из кэша; после перехода фрейма его там
        // уже нет); межсайтовый — нет, и его сессия может прийти позже.
        let same = Doc { session: owner.session.clone(), frame_id: frame_id.clone() };
        if self.world(&same, false).await.is_ok() {
            return Ok(same);
        }
        let deadline = tokio::time::Instant::now() + FRAME_WAIT;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(FRAME_POLL).await;
            if let Some(doc) = self.cross_site(owner, &frame_id) {
                return Ok(doc);
            }
        }
        Err(CdpError::Page("фрейм недоступен".into()))
    }

    fn cross_site(&self, owner: &ElementRef, frame_id: &str) -> Option<Doc> {
        let session = self.frames.lock().expect("мьютекс фреймов отравлен").get(frame_id).map(|frame| frame.session.clone())?;
        self.owners.lock().expect("мьютекс владельцев отравлен").insert(session.clone(), owner.clone());
        Some(Doc { session, frame_id: frame_id.to_string() })
    }

    /// Точка для мыши в окне вкладки: элемент прокручен в видимую область; `at` — сдвиг от левого верхнего угла,
    /// иначе центр. Координаты элементов межсайтового фрейма — от его окна, к ним прибавляются рамки содержимого
    /// `<iframe>` по цепочке вверх; фреймы того же процесса уже отдают координаты окна вкладки.
    pub async fn point(&self, element: &ElementRef, at: Option<(f64, f64)>) -> Result<(f64, f64), CdpError> {
        let quad = self.quad(element).await?;
        Ok(match at {
            Some((dx, dy)) => (quad[0] + dx, quad[1] + dy),
            None => ((quad[0] + quad[2] + quad[4] + quad[6]) / 4.0, (quad[1] + quad[3] + quad[5] + quad[7]) / 4.0),
        })
    }

    /// Снимок элемента PNG по его рамке (`clip`): элемент прокручен в видимую область, рамка окна вкладки сдвинута на
    /// прокрутку страницы — `clip` в координатах документа.
    pub async fn screenshot_element(&self, element: &ElementRef) -> Result<Vec<u8>, CdpError> {
        let quad = self.quad(element).await?;
        let (xs, ys) = ([quad[0], quad[2], quad[4], quad[6]], [quad[1], quad[3], quad[5], quad[7]]);
        let (left, top) = (xs.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MAX, f64::min));
        let (right, bottom) = (xs.iter().copied().fold(f64::MIN, f64::max), ys.iter().copied().fold(f64::MIN, f64::max));
        let metrics = self.call("Page.getLayoutMetrics", json!({})).await?;
        let scroll = &metrics["cssVisualViewport"];
        let (x, y) = (left + scroll["pageX"].as_f64().unwrap_or(0.0), top + scroll["pageY"].as_f64().unwrap_or(0.0));
        let clip = json!({ "x": x, "y": y, "width": right - left, "height": bottom - top, "scale": 1 });
        self.capture(json!({ "format": "png", "clip": clip })).await
    }

    /// Рамка элемента в окне вкладки (4 точки по 2 координаты), элемент прокручен в видимую область.
    async fn quad(&self, element: &ElementRef) -> Result<Vec<f64>, CdpError> {
        let chain = self.owner_chain(element)?;
        for each in &chain {
            self.same_document(each)?;
        }
        for each in chain.iter().rev() {
            self.browser.call(Some(&each.session), "DOM.scrollIntoViewIfNeeded", json!({ "backendNodeId": each.node })).await.map_err(gone)?;
        }
        let reply = self.browser.call(Some(&element.session), "DOM.getContentQuads", json!({ "backendNodeId": element.node })).await.map_err(gone)?;
        let mut quad: Vec<f64> = reply["quads"][0].as_array().map(|q| q.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        if quad.len() < 8 {
            return Err(CdpError::Page("у элемента нет рамки — он невидим".into()));
        }
        for owner in &chain[1..] {
            let model = self.browser.call(Some(&owner.session), "DOM.getBoxModel", json!({ "backendNodeId": owner.node })).await.map_err(gone)?;
            let (dx, dy) = (model["model"]["content"][0].as_f64().unwrap_or(0.0), model["model"]["content"][1].as_f64().unwrap_or(0.0));
            for (n, value) in quad.iter_mut().enumerate() {
                *value += if n % 2 == 0 { dx } else { dy };
            }
        }
        Ok(quad)
    }

    /// Элемент и владельцы его межсайтовых фреймов до вкладки: `[элемент, <iframe> фрейма, <iframe> его родителя…]`.
    fn owner_chain(&self, element: &ElementRef) -> Result<Vec<ElementRef>, CdpError> {
        let owners = self.owners.lock().expect("мьютекс владельцев отравлен");
        let mut chain = vec![element.clone()];
        while chain.last().is_some_and(|last| last.session != self.session) {
            let session = &chain.last().expect("цепочка не пуста").session;
            let owner = owners.get(session).cloned().ok_or_else(|| CdpError::Protocol("неизвестный межсайтовый фрейм".into()))?;
            chain.push(owner);
        }
        Ok(chain)
    }

    pub(super) async fn wait_load(&self, events: &mut broadcast::Receiver<CdpEvent>, wait: Duration, mut state: LoadWait) -> Result<(), CdpError> {
        let done = async {
            loop {
                match events.recv().await {
                    Ok(event) if event.session.as_deref() == Some(self.session.as_str()) => {
                        // Смену документа отмечает и само ожидание (наблюдатель фреймов мог ещё не дойти до события):
                        // после перехода, перезагрузки и шага назад прежние миры и элементы уже не годятся.
                        frame_event(&self.docs, &event, true);
                        if state.step(&event) {
                            return Ok(());
                        }
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return Err(CdpError::Closed),
                }
            }
        };
        tokio::time::timeout(wait, done).await.map_err(|_| CdpError::Timeout)?
    }
}

/// Что `watch_frames` делит со страницей.
struct Watched {
    frames: Frames,
    files_intercepted: Arc<AtomicBool>,
    docs: Arc<Mutex<Documents>>,
}

/// Сессии межсайтовых фреймов вкладки `own`: подключившийся — в карту, отключившийся — из неё; его вложенные фреймы
/// тоже автоподключаются. Переходы фреймов меняют документы вкладки (`frame_event`) — и из сессии вкладки, и из сессий
/// фреймов: межсайтовый переход фрейма B→C сессию не меняет (фрейм остаётся в своём процессе), а событие о нём
/// приходит только в сессию фрейма. Повтор события в двух сессиях ничего не меняет (`loaderId`). События других
/// вкладок того же соединения пропускаются.
async fn watch_frames(browser: Browser, own: String, mut events: broadcast::Receiver<CdpEvent>, watched: Watched, stop: CancellationToken) {
    let Watched { frames, files_intercepted, docs } = watched;
    loop {
        let event = tokio::select! {
            _ = stop.cancelled() => return,
            event = events.recv() => event,
        };
        let event = match event {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let parent = event.session.as_deref().unwrap_or_default();
        let ours = parent == own || frames.lock().expect("мьютекс фреймов отравлен").values().any(|frame| frame.session == parent);
        match event.method.as_str() {
            "Target.attachedToTarget" if ours && event.params["targetInfo"]["type"] == "iframe" => {
                let (Some(frame), Some(session)) = (event.params["targetInfo"]["targetId"].as_str(), event.params["sessionId"].as_str()) else {
                    continue;
                };
                // Сначала автоподключение его вложенных фреймов, потом — в карту: найденный фрейм уже настроен.
                let _ = browser.call(Some(session), "Target.setAutoAttach", auto_attach()).await;
                let intercept = {
                    let mut frames = frames.lock().expect("мьютекс фреймов отравлен");
                    frames.insert(frame.to_string(), FrameSession { session: session.to_string(), parent: parent.to_string() });
                    files_intercepted.load(Ordering::SeqCst)
                };
                // События его переходов (и выбор файла) — без ожидания ответа: занятый рендерер фрейма не держит
                // наблюдатель до тайм-аута вызова. Команды одной сессии браузер исполняет по порядку.
                let (clone, id) = (browser.clone(), session.to_string());
                tokio::spawn(async move {
                    let _ = clone.call(Some(&id), "Page.enable", json!({})).await;
                    if intercept {
                        let _ = clone.call(Some(&id), "Page.setInterceptFileChooserDialog", json!({ "enabled": true })).await;
                    }
                });
            }
            "Target.detachedFromTarget" => {
                if let Some(session) = event.params["sessionId"].as_str() {
                    frames.lock().expect("мьютекс фреймов отравлен").retain(|_, frame| frame.session != session);
                    docs.lock().expect("мьютекс документов отравлен").worlds.retain(|doc, _| doc.session != session);
                }
            }
            "Page.frameNavigated" | "Page.frameDetached" if ours => frame_event(&docs, &event, parent == own),
            _ => {}
        }
    }
}

/// Скрипт процесса: `inject` — константы в блоке перед кодом; значение — последнего выражения.
fn block(code: &str, inject: &[(String, Value)]) -> String {
    let mut script = String::from("{\n");
    for (name, value) in inject {
        script.push_str(&format!("const {name} = {value};\n"));
    }
    script.push_str(code);
    script.push_str("\n}");
    script
}

/// Ответ `Runtime.*`: исключение скрипта — ошибкой с его текстом, иначе объект результата.
pub(super) fn result_of(reply: Value) -> Result<Value, CdpError> {
    if let Some(details) = reply.get("exceptionDetails") {
        let text = details["exception"]["description"].as_str().or(details["text"].as_str()).unwrap_or("ошибка скрипта");
        return Err(CdpError::Script(text.to_string()));
    }
    Ok(reply["result"].clone())
}

/// Мир устарел: браузер не нашёл контекст до выполнения, повтор безопасен. «Execution context was destroyed» и
/// прочие ошибки не повторяются: код мог уже частично выполниться; исключения скрипта — `Script`, не `Remote`.
fn stale_world(error: &CdpError) -> bool {
    matches!(error, CdpError::Remote(message) if message.contains("Cannot find context") || message.contains("Cannot find execution context"))
}

/// Вызов без ответа: тайм-аут или обрыв соединения. Отпускать его объект — ещё столько же ждать впустую.
fn unanswered(error: &CdpError) -> bool {
    matches!(error, CdpError::Timeout | CdpError::Closed)
}

/// Тексты браузера об узле, которого нет в документе, об ушедшем фрейме и о закрытой сессии элемента (фрейм или
/// вкладка ушли).
const GONE_TEXTS: &[&str] =
    &["No node", "does not belong to the document", "No frame for given id", "Session with given id not found", "No session with given id"];

/// Узла больше нет (переход, перерисовка, закрытая сессия) — своим текстом.
fn gone(error: CdpError) -> CdpError {
    match error {
        CdpError::Remote(message) if GONE_TEXTS.iter().any(|known| message.contains(known)) => CdpError::Page(GONE.into()),
        other => other,
    }
}

/// Ожидание загрузки своего перехода: сначала коммит главного фрейма, потом `load` (`load` до коммита — от прежнего
/// документа и не считается).
pub(super) struct LoadWait {
    target_id: String,
    /// Коммит должен нести этот `loaderId` (`goto`); `None` — подойдёт любой коммит главного фрейма.
    loader_id: Option<String>,
    /// Шаг по истории: переход внутри документа главного фрейма тоже конец ожидания.
    within_document: bool,
    committed: bool,
}

impl LoadWait {
    pub(super) fn new(target_id: &str, loader_id: Option<String>, within_document: bool) -> Self {
        Self { target_id: target_id.to_string(), loader_id, within_document, committed: false }
    }

    /// Событие своей сессии; `true` — ожидание закончено.
    fn step(&mut self, event: &CdpEvent) -> bool {
        match event.method.as_str() {
            "Page.frameNavigated" => {
                let frame = &event.params["frame"];
                if !frame["parentId"].is_null() {
                    return false;
                }
                // Восстановление из кэша «назад-вперёд»: `load` не будет.
                if event.params["type"] == "BackForwardCacheRestore" {
                    return true;
                }
                if self.loader_id.as_deref().is_none_or(|id| frame["loaderId"].as_str() == Some(id)) {
                    self.committed = true;
                }
                false
            }
            "Page.navigatedWithinDocument" => self.within_document && event.params["frameId"].as_str() == Some(self.target_id.as_str()),
            "Page.loadEventFired" => self.committed,
            _ => false,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::engine::fake_cdp::FakeCdp;
    use std::sync::atomic::AtomicU64 as Counter;
    use tokio::net::TcpListener;

    /// Сервер-сценарий: принимает два вызова, шлёт событие, отвечает в обратном порядке и закрывает соединение.
    async fn scripted() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/devtools/browser/x", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut ids = Vec::new();
            while ids.len() < 2 {
                if let Some(Ok(Message::Text(text))) = socket.next().await {
                    ids.push(serde_json::from_str::<Value>(text.as_str()).unwrap()["id"].clone());
                }
            }
            for reply in [
                json!({ "method": "Page.loadEventFired", "sessionId": "S1", "params": { "timestamp": 1 } }),
                json!({ "id": ids[1], "result": { "n": 2 } }),
                json!({ "id": ids[0], "error": { "code": -32000, "message": "нет" } }),
            ] {
                socket.send(Message::text(reply.to_string())).await.unwrap();
            }
            let _ = socket.close(None).await;
        });
        url
    }

    #[tokio::test]
    async fn replies_are_matched_by_id_and_events_are_broadcast() {
        let browser = Browser::connect(&scripted().await).await.unwrap();
        let mut events = browser.subscribe();
        let (first, second) = tokio::join!(browser.call(None, "A.one", json!({})), browser.call(None, "B.two", json!({})));
        assert!(matches!(first, Err(CdpError::Remote(m)) if m == "нет"));
        assert_eq!(second.unwrap(), json!({ "n": 2 }));
        let event = events.recv().await.unwrap();
        assert_eq!((event.session.as_deref(), event.method.as_str()), (Some("S1"), "Page.loadEventFired"));
        assert!(matches!(browser.call(None, "C.three", json!({})).await, Err(CdpError::Closed)), "сервер закрыл соединение");
    }

    #[tokio::test]
    async fn tabs_are_listed_opened_and_closed() {
        let fake = FakeCdp::spawn().await;
        fake.add_page("https://a.test/");
        let browser = Browser::connect(&fake.ws_url()).await.unwrap();
        let id = browser.new_tab("about:blank").await.unwrap();
        let tabs = browser.tabs().await.unwrap();
        assert_eq!(tabs.iter().map(|t| t.url.as_str()).collect::<Vec<_>>(), ["https://a.test/", "about:blank"]);
        assert_eq!(tabs[1].target_id, id);
        browser.close_tab(&tabs[0].target_id).await.unwrap();
        assert_eq!(fake.pages(), ["about:blank"]);
        assert!(browser.attach(&id).await.is_err(), "подставной браузер сессий вкладок не даёт");
    }

    #[tokio::test]
    async fn only_local_addresses() {
        assert!(matches!(Browser::connect("ws://10.0.0.1:9222/devtools/browser/x").await, Err(CdpError::NotLocal)));
    }

    /// Сервер-сценарий вкладки: отвечает на attach/enable/navigate, затем шлёт события и держит соединение.
    async fn tab_server(events: Vec<Value>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/devtools/browser/x", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let request: Value = serde_json::from_str(text.as_str()).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "Target.attachToTarget" => json!({ "sessionId": "S1" }),
                    "Page.navigate" => json!({ "frameId": "T1", "loaderId": "L2" }),
                    _ => json!({}),
                };
                socket.send(Message::text(json!({ "id": request["id"], "result": result }).to_string())).await.unwrap();
                if request["method"] == "Page.navigate" {
                    for event in &events {
                        socket.send(Message::text(event.to_string())).await.unwrap();
                    }
                }
            }
        });
        url
    }

    fn event(method: &str, params: Value) -> Value {
        json!({ "method": method, "sessionId": "S1", "params": params })
    }

    fn commit(loader: &str) -> Value {
        event("Page.frameNavigated", json!({ "frame": { "id": "T1", "loaderId": loader } }))
    }

    async fn goto_with(events: Vec<Value>) -> Result<(), CdpError> {
        let browser = Browser::connect(&tab_server(events).await).await.unwrap();
        browser.attach("T1").await.unwrap().goto("https://a.test/", Duration::from_millis(300)).await
    }

    #[tokio::test]
    async fn stale_load_does_not_finish_goto() {
        let stale = event("Page.loadEventFired", json!({}));
        assert!(matches!(goto_with(vec![stale]).await, Err(CdpError::Timeout)));
    }

    #[tokio::test]
    async fn goto_finishes_on_load_after_own_commit() {
        let stale = event("Page.loadEventFired", json!({}));
        let load = event("Page.loadEventFired", json!({}));
        assert!(goto_with(vec![stale, commit("L2"), load]).await.is_ok());
    }

    #[tokio::test]
    async fn foreign_commit_does_not_finish_goto() {
        let load = event("Page.loadEventFired", json!({}));
        assert!(matches!(goto_with(vec![commit("L9"), load]).await, Err(CdpError::Timeout)));
    }

    #[tokio::test]
    async fn goto_with_zero_timeout_does_not_wait_for_load() {
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.navigate" => json!({ "frameId": "T1", "loaderId": "L1" }),
                _ => json!({}),
            })
        })
        .await;
        let page = tab(&mock).await;
        page.docs().worlds.insert(page.main_doc(), 5);
        let before = page.generation("T1");
        let started = std::time::Instant::now();
        page.goto("https://a.test/", Duration::ZERO).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(1), "загрузку не ждём");
        assert_eq!(mock.sent("Page.navigate").len(), 1);
        assert!(page.generation("T1") != before && page.docs().worlds.is_empty(), "прежний документ отмечен сразу");
        let after = page.generation("T1");
        page.docs().navigated("T1", true, "L1");
        assert_ne!(page.generation("T1"), after, "событие перехода отмечает смену ещё раз");
    }

    #[tokio::test]
    async fn closed_resolves_after_the_server_drops_the_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/devtools/browser/x", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = socket.close(None).await;
        });
        let browser = Browser::connect(&url).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), browser.closed()).await.expect("closed() не дождался обрыва");
        tokio::time::timeout(Duration::from_millis(50), browser.closed()).await.expect("уже закрытое — сразу");
    }

    #[tokio::test]
    async fn navigate_reply_after_the_deadline_is_a_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/devtools/browser/x", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let request: Value = serde_json::from_str(text.as_str()).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "Target.attachToTarget" => json!({ "sessionId": "S1" }),
                    "Page.navigate" => {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        json!({ "frameId": "T1", "loaderId": "L2" })
                    }
                    _ => json!({}),
                };
                let _ = socket.send(Message::text(json!({ "id": request["id"], "result": result }).to_string())).await;
            }
        });
        let browser = Browser::connect(&url).await.unwrap();
        let page = browser.attach("T1").await.unwrap();
        let started = std::time::Instant::now();
        let result = page.goto("https://a.test/", Duration::from_millis(200)).await;
        assert!(matches!(result, Err(CdpError::Timeout)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(2), "срок общий и на команду");
    }

    #[test]
    fn back_within_document_counts_only_for_main_frame() {
        let mut wait = LoadWait::new("T1", None, true);
        assert!(!wait.step(&CdpEvent { session: None, method: "Page.navigatedWithinDocument".into(), params: json!({ "frameId": "F2" }) }));
        assert!(wait.step(&CdpEvent { session: None, method: "Page.navigatedWithinDocument".into(), params: json!({ "frameId": "T1" }) }));
    }

    /// Подставной браузер для сессии вкладки: отвечает по таблице `answer(метод, параметры, сессия)`, копит
    /// запросы, шлёт события по команде. Ответ `{"error": "…"}` уходит ошибкой CDP, `{"silent": true}` — без ответа.
    pub(crate) struct Mock {
        pub(crate) url: String,
        pub(crate) requests: Arc<Mutex<Vec<Value>>>,
        pub(crate) events: tokio::sync::mpsc::UnboundedSender<Value>,
    }

    impl Mock {
        pub(crate) async fn spawn(answer: impl Fn(&str, &Value, &str) -> Value + Send + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}/devtools/browser/mock", listener.local_addr().unwrap());
            let requests: Arc<Mutex<Vec<Value>>> = Arc::default();
            let (events, mut queue) = tokio::sync::mpsc::unbounded_channel::<Value>();
            let seen = requests.clone();
            tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                loop {
                    tokio::select! {
                        message = socket.next() => {
                            let Some(Ok(Message::Text(text))) = message else { return };
                            let request: Value = serde_json::from_str(text.as_str()).unwrap();
                            seen.lock().unwrap().push(request.clone());
                            let method = request["method"].as_str().unwrap_or_default();
                            let session = request["sessionId"].as_str().unwrap_or_default();
                            let result = answer(method, &request["params"], session);
                            if result["silent"] == true {
                                continue;
                            }
                            let reply = match result.get("error") {
                                Some(message) => json!({ "id": request["id"], "error": { "code": -32000, "message": message } }),
                                None => json!({ "id": request["id"], "result": result }),
                            };
                            if socket.send(Message::text(reply.to_string())).await.is_err() {
                                return;
                            }
                        }
                        event = queue.recv() => {
                            let Some(event) = event else { return };
                            if socket.send(Message::text(event.to_string())).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            });
            Self { url, requests, events }
        }

        /// Параметры запросов метода по порядку.
        pub(crate) fn sent(&self, method: &str) -> Vec<Value> {
            self.requests.lock().unwrap().iter().filter(|r| r["method"] == method).map(|r| r["params"].clone()).collect()
        }

        /// Сессии запросов метода по порядку.
        pub(crate) fn sessions(&self, method: &str) -> Vec<String> {
            let requests = self.requests.lock().unwrap();
            requests.iter().filter(|r| r["method"] == method).map(|r| r["sessionId"].as_str().unwrap_or_default().to_string()).collect()
        }
    }

    /// Ответы, нужные любой вкладке: подключение, домены, автоподключение фреймов.
    pub(crate) fn base(method: &str) -> Option<Value> {
        match method {
            "Target.attachToTarget" => Some(json!({ "sessionId": "S1" })),
            "Page.enable" | "Target.setAutoAttach" | "Target.detachFromTarget" => Some(json!({})),
            _ => None,
        }
    }

    async fn tab(mock: &Mock) -> Page {
        Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap()
    }

    #[tokio::test]
    async fn abandoned_call_leaves_no_waiter() {
        let mock = Mock::spawn(|_, _, _| json!({ "silent": true })).await;
        let browser = Browser::connect(&mock.url).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(50), browser.call(None, "Slow.never", json!({}))).await.is_err());
        assert!(browser.conn.waiters.lock().unwrap().as_ref().unwrap().is_empty(), "брошенный вызов убрал своего ждущего");
    }

    #[tokio::test]
    async fn worlds_are_cached_and_recreated_after_navigation() {
        let created = Arc::new(Counter::new(0));
        let counter = created.clone();
        let mock = Mock::spawn(move |method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": counter.fetch_add(1, Ordering::SeqCst) + 1 }),
                // Первый мир «устарел»: страница сменила документ.
                "Runtime.callFunctionOn" if params["executionContextId"] == 1 => json!({ "error": "Cannot find context with specified id" }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "number", "value": 3 } }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let doc = page.main_doc();
        assert_eq!(page.count(&doc, By::Css, "li").await.unwrap(), 3);
        assert_eq!(page.count(&doc, By::Css, "li").await.unwrap(), 3);
        assert_eq!(created.load(Ordering::SeqCst), 2, "устаревший мир пересоздан один раз, новый закэширован");
        assert_eq!(mock.sent("Page.createIsolatedWorld")[0]["frameId"], "T1", "главный фрейм — id вкладки");
        assert_eq!(mock.sent("Runtime.callFunctionOn")[0]["arguments"], json!([{ "value": "css" }, { "value": "li" }]));
    }

    #[tokio::test]
    async fn found_elements_are_read_and_gone_nodes_are_named() {
        let mock = Mock::spawn(|method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 7 }),
                "Runtime.callFunctionOn" if params["objectId"] == "O2" => json!({ "result": { "type": "string", "value": "Привет" } }),
                "Runtime.callFunctionOn" if params["arguments"][2]["value"] == 5 => json!({ "result": { "type": "object", "subtype": "null", "value": null } }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "object", "objectId": "O1" } }),
                "DOM.describeNode" => json!({ "node": { "backendNodeId": 42 } }),
                "DOM.resolveNode" if params["backendNodeId"] == 42 => json!({ "object": { "objectId": "O2" } }),
                "DOM.resolveNode" => json!({ "error": "No node with given id found" }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let doc = page.main_doc();
        let element = page.nth(&doc, By::Text, "Привет", 0).await.unwrap().unwrap();
        assert_eq!(element, ElementRef { session: "S1".into(), frame_id: "T1".into(), node: 42, generation: 0 });
        assert!(page.nth(&doc, By::Css, "li", 5).await.unwrap().is_none(), "за концом списка — None");
        assert_eq!(page.read(&element, Read::Text).await.unwrap(), json!("Привет"));
        let gone = ElementRef { node: 43, ..element };
        let error = page.read(&gone, Read::Text).await.unwrap_err();
        assert!(matches!(error, CdpError::Page(ref m) if m == "Элемент больше не доступен на странице"), "{error:?}");
        assert_eq!(error.to_string(), "Элемент больше не доступен на странице", "свой текст — без приставки CDP");
        let released: Vec<Value> = mock.sent("Runtime.releaseObject").iter().map(|p| p["objectId"].clone()).collect();
        assert_eq!(released, [json!("O1"), json!("O2")], "найденный и разрешённый объекты отпущены");
    }

    #[tokio::test]
    async fn non_stale_errors_mentioning_context_are_not_retried() {
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "Runtime.callFunctionOn" => json!({ "error": "ReferenceError: context is not defined" }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let result = page.count(&page.main_doc(), By::Css, "li").await;
        assert!(matches!(result, Err(CdpError::Remote(m)) if m == "ReferenceError: context is not defined"));
        assert_eq!(mock.sent("Page.createIsolatedWorld").len(), 1);
        assert_eq!(mock.sent("Runtime.callFunctionOn").len(), 1);
    }

    #[tokio::test]
    async fn second_stale_error_is_returned_not_looped() {
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "Runtime.callFunctionOn" => json!({ "error": "Cannot find context with specified id" }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        assert!(page.count(&page.main_doc(), By::Css, "li").await.is_err());
        assert_eq!(mock.sent("Page.createIsolatedWorld").len(), 2);
    }

    #[tokio::test]
    async fn same_site_frame_stays_in_the_tab_session() {
        let mock = Mock::spawn(|method, _, session| {
            base(method).unwrap_or_else(|| match (method, session) {
                ("DOM.describeNode", "S1") => json!({ "node": { "backendNodeId": 10, "frameId": "F2" } }),
                ("Page.createIsolatedWorld", "S1") => json!({ "executionContextId": 3 }),
                ("DOM.scrollIntoViewIfNeeded", _) => json!({}),
                ("DOM.getContentQuads", "S1") => json!({ "quads": [[10, 10, 30, 10, 30, 30, 10, 30]] }),
                _ => json!({ "error": format!("unexpected {method} in {session}") }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let owner = ElementRef { session: "S1".into(), frame_id: "T1".into(), node: 10, generation: 0 };
        assert_eq!(page.frame_doc(&owner).await.unwrap(), Doc { session: "S1".into(), frame_id: "F2".into() });
        assert_eq!(page.frame_doc(&owner).await.unwrap(), Doc { session: "S1".into(), frame_id: "F2".into() });
        assert_eq!(mock.sent("Page.createIsolatedWorld").len(), 1, "мир фрейма того же процесса — из кэша");
        let inner = ElementRef { session: "S1".into(), frame_id: "F2".into(), node: 5, generation: 0 };
        assert_eq!(page.point(&inner, None).await.unwrap(), (20.0, 20.0), "фрейм того же процесса — координаты уже в окне вкладки");
        assert_eq!(page.point(&inner, Some((2.0, 3.0))).await.unwrap(), (12.0, 13.0), "смещение — от левого верхнего угла");
    }

    #[tokio::test]
    async fn cross_site_frame_has_its_own_session_and_offset() {
        let mock = Mock::spawn(|method, _, session| {
            base(method).unwrap_or_else(|| match (method, session) {
                ("DOM.describeNode", "S1") => json!({ "node": { "backendNodeId": 10, "frameId": "F3" } }),
                ("Page.createIsolatedWorld", "S1") => json!({ "error": "No frame for given id found" }),
                ("DOM.getBoxModel", "S1") => json!({ "model": { "content": [100, 50, 300, 50, 300, 250, 100, 250] } }),
                ("DOM.getContentQuads", "S9") => json!({ "quads": [[10, 10, 30, 10, 30, 30, 10, 30]] }),
                ("DOM.scrollIntoViewIfNeeded", _) => json!({}),
                _ => json!({ "error": format!("unexpected {method} in {session}") }),
            })
        })
        .await;
        let page = tab(&mock).await;
        // Межсайтовый фрейм подключается чуть позже обращения к нему.
        let event = json!({ "method": "Target.attachedToTarget", "sessionId": "S1", "params": { "sessionId": "S9", "targetInfo": { "targetId": "F3", "type": "iframe" } } });
        let sender = mock.events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            sender.send(event).unwrap();
        });
        let owner = ElementRef { session: "S1".into(), frame_id: "T1".into(), node: 10, generation: 0 };
        assert_eq!(page.frame_doc(&owner).await.unwrap(), Doc { session: "S9".into(), frame_id: "F3".into() });
        let inner = ElementRef { session: "S9".into(), frame_id: "F3".into(), node: 5, generation: 0 };
        assert_eq!(page.point(&inner, None).await.unwrap(), (120.0, 70.0), "центр в окне фрейма плюс рамка содержимого <iframe>");
        assert_eq!(mock.sessions("DOM.scrollIntoViewIfNeeded"), ["S1", "S9"], "сначала владелец фрейма, потом элемент");
        assert!(mock.sessions("Target.setAutoAttach").contains(&"S9".to_string()), "вложенные фреймы тоже автоподключаются");
    }

    #[tokio::test]
    async fn element_without_box_is_named() {
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "DOM.scrollIntoViewIfNeeded" => json!({}),
                "DOM.getContentQuads" => json!({ "quads": [] }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let element = ElementRef { session: "S1".into(), frame_id: "T1".into(), node: 5, generation: 0 };
        assert!(matches!(page.point(&element, None).await, Err(CdpError::Page(m)) if m == "у элемента нет рамки — он невидим"));
    }

    fn input_mock() -> impl Fn(&str, &Value, &str) -> Value + Send + 'static {
        |method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Input.dispatchMouseEvent" | "Input.dispatchKeyEvent" | "Input.insertText" | "DOM.focus" => json!({}),
                "Page.getLayoutMetrics" => json!({ "cssLayoutViewport": { "clientWidth": 800, "clientHeight": 600 }, "cssContentSize": { "width": 800, "height": 3000 } }),
                "Page.captureScreenshot" => json!({ "data": "aGVsbG8=" }),
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "DOM.resolveNode" => json!({ "object": { "objectId": "O1" } }),
                "Runtime.callFunctionOn" if params["arguments"][0]["value"] == "z" => json!({ "result": { "value": "no-option" } }),
                "Runtime.callFunctionOn" if params["arguments"][0]["value"] == "u" => json!({ "result": { "value": 5 } }),
                "Runtime.callFunctionOn" if params["functionDeclaration"].as_str().is_some_and(|f| f.contains("scrollHeight")) => json!({ "result": { "value": [30, 900] } }),
                "Runtime.callFunctionOn" if params["arguments"][0]["value"] == "q" => json!({ "result": { "value": "not-select" } }),
                "Runtime.callFunctionOn" => json!({ "result": { "value": "ok" } }),
                _ => json!({ "error": "unexpected" }),
            })
        }
    }

    #[tokio::test]
    async fn clicks_are_move_press_release() {
        let mock = Mock::spawn(input_mock()).await;
        let page = tab(&mock).await;
        page.click(5.0, 6.0, Button::Left, 2, Duration::ZERO).await.unwrap();
        let events: Vec<(String, i64)> = mock
            .sent("Input.dispatchMouseEvent")
            .iter()
            .map(|e| (e["type"].as_str().unwrap().to_string(), e["clickCount"].as_i64().unwrap_or(0)))
            .collect();
        assert_eq!(
            events,
            [("mouseMoved".into(), 0), ("mousePressed".into(), 1), ("mouseReleased".into(), 1), ("mousePressed".into(), 2), ("mouseReleased".into(), 2)]
        );
        assert_eq!(mock.sent("Input.dispatchMouseEvent")[1]["button"], "left");
    }

    #[tokio::test]
    async fn keys_carry_codes_text_and_mac_commands() {
        let mock = Mock::spawn(input_mock()).await;
        let page = tab(&mock).await;
        page.press("a", &["cmd"]).await.unwrap();
        page.press("Enter", &[]).await.unwrap();
        page.type_char('ж').await.unwrap();
        let keys = mock.sent("Input.dispatchKeyEvent");
        assert_eq!(keys[0]["type"], "rawKeyDown");
        assert_eq!((keys[0]["modifiers"].as_i64(), keys[0]["commands"].clone()), (Some(4), json!(["selectAll"])));
        assert!(keys[0].get("text").is_none(), "с ⌘ текст не печатается");
        assert_eq!((keys[2]["type"].as_str(), keys[2]["text"].as_str(), keys[2]["windowsVirtualKeyCode"].as_i64()), (Some("keyDown"), Some("\r"), Some(13)));
        assert_eq!((keys[4]["type"].as_str(), keys[4]["text"].as_str()), (Some("keyDown"), Some("ж")));
        assert_eq!(keys[5]["type"], "keyUp");
        page.press("z", &["cmd", "shift"]).await.unwrap();
        page.press("z", &["cmd"]).await.unwrap();
        page.press("a", &["cmd", "alt"]).await.unwrap();
        let keys = mock.sent("Input.dispatchKeyEvent");
        assert_eq!((keys[6]["commands"].clone(), keys[8]["commands"].clone()), (json!(["redo"]), json!(["undo"])), "⌘⇧Z — повтор, ⌘Z — отмена");
        assert!(keys[10].get("commands").is_none(), "⌘⌥A — не команда редактирования");
        assert!(is_key("F12") && is_key("7") && is_key("q"));
        assert!(!is_key("F13") && !is_key("ж") && !is_key("ab"));
        assert_eq!(KEY_NAMES, NAMED_KEYS.iter().map(|k| k.0).collect::<Vec<_>>().as_slice(), "имена Keys — из таблицы клавиш");
    }

    #[tokio::test]
    async fn screenshot_of_the_whole_page_uses_content_size() {
        let mock = Mock::spawn(input_mock()).await;
        let page = tab(&mock).await;
        assert_eq!(page.screenshot("png", 50, true).await.unwrap(), b"hello");
        let shot = &mock.sent("Page.captureScreenshot")[0];
        assert_eq!((shot["captureBeyondViewport"].clone(), shot["clip"]["height"].clone()), (json!(true), json!(3000)));
        assert!(shot.get("quality").is_none(), "качество — только у jpeg");
        assert_eq!(page.viewport_center().await.unwrap(), (400.0, 300.0));
    }

    #[tokio::test]
    async fn element_shot_is_a_png_clipped_to_the_element_box_on_the_page() {
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "DOM.scrollIntoViewIfNeeded" => json!({}),
                "DOM.getContentQuads" => json!({ "quads": [[10, 20, 110, 20, 110, 60, 10, 60]] }),
                "Page.getLayoutMetrics" => json!({ "cssVisualViewport": { "pageX": 5, "pageY": 300 } }),
                "Page.captureScreenshot" => json!({ "data": "aGVsbG8=" }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let element = ElementRef { session: "S1".into(), frame_id: "T1".into(), node: 5, generation: 0 };
        assert_eq!(page.screenshot_element(&element).await.unwrap(), b"hello");
        let shot = &mock.sent("Page.captureScreenshot")[0];
        assert_eq!(shot["format"], "png");
        assert_eq!(shot["clip"], json!({ "x": 15.0, "y": 320.0, "width": 100.0, "height": 40.0, "scale": 1 }), "рамка окна плюс прокрутка страницы");
        assert_eq!(mock.sent("DOM.scrollIntoViewIfNeeded").len(), 1, "элемент прокручен в видимую область");
    }

    #[tokio::test]
    async fn select_reports_missing_options_and_wrong_elements() {
        let mock = Mock::spawn(input_mock()).await;
        let page = tab(&mock).await;
        let element = ElementRef { session: "S1".into(), frame_id: "T1".into(), node: 5, generation: 0 };
        assert!(page.select(&element, "b").await.unwrap());
        assert!(!page.select(&element, "z").await.unwrap());
        assert!(matches!(page.select(&element, "q").await, Err(CdpError::Page(m)) if m == "элемент — не выпадающий список"));
        assert!(matches!(page.select(&element, "u").await, Err(CdpError::Protocol(m)) if m == "неожиданный ответ выбора варианта"));
    }

    #[tokio::test]
    async fn scroll_state_is_parsed_and_scroll_to_sends_the_position() {
        let mock = Mock::spawn(input_mock()).await;
        let page = tab(&mock).await;
        let element = ElementRef { session: "S1".into(), frame_id: "T1".into(), node: 5, generation: 0 };
        assert_eq!(page.scroll_state(None).await.unwrap(), (30.0, 900.0));
        assert_eq!(page.scroll_state(Some(&element)).await.unwrap(), (30.0, 900.0));
        page.scroll_to(None, 120.0).await.unwrap();
        let calls = mock.sent("Runtime.callFunctionOn");
        assert_eq!(calls.last().unwrap()["arguments"][0]["value"], 120.0);
    }

    #[tokio::test]
    async fn jpeg_shot_has_quality_and_shift_makes_capitals() {
        let mock = Mock::spawn(input_mock()).await;
        let page = tab(&mock).await;
        page.screenshot("jpeg", 70, false).await.unwrap();
        let shot = &mock.sent("Page.captureScreenshot")[0];
        assert_eq!((shot["quality"].clone(), shot.get("clip").is_none()), (json!(70), true));
        page.press("a", &["shift"]).await.unwrap();
        let down = &mock.sent("Input.dispatchKeyEvent")[0];
        assert_eq!((down["type"].as_str(), down["key"].as_str(), down["text"].as_str(), down["modifiers"].as_i64()), (Some("keyDown"), Some("A"), Some("A"), Some(8)));
    }

    #[tokio::test]
    async fn typed_latin_digits_and_space_carry_key_codes() {
        let mock = Mock::spawn(input_mock()).await;
        let page = tab(&mock).await;
        for ch in ['a', 'A', '7', ' ', 'ж'] {
            page.type_char(ch).await.unwrap();
        }
        let downs: Vec<(Value, Value, Value)> = mock
            .sent("Input.dispatchKeyEvent")
            .iter()
            .filter(|e| e["type"] == "keyDown")
            .map(|e| (e["key"].clone(), e["code"].clone(), e["windowsVirtualKeyCode"].clone()))
            .collect();
        assert_eq!(
            downs,
            [
                (json!("a"), json!("KeyA"), json!(65)),
                (json!("A"), json!("KeyA"), json!(65)),
                (json!("7"), json!("Digit7"), json!(55)),
                (json!(" "), json!("Space"), json!(32)),
                (json!("ж"), Value::Null, Value::Null),
            ]
        );
        assert_eq!(mock.sent("Input.dispatchKeyEvent")[1]["code"], "KeyA", "отпускание — с тем же кодом");
    }

    /// Событие `Page.frameNavigated` сессии вкладки `S1`.
    fn navigated(frame: &str, parent: Option<&str>, loader: &str) -> Value {
        json!({ "method": "Page.frameNavigated", "sessionId": "S1", "params": { "frame": { "id": frame, "parentId": parent, "loaderId": loader } } })
    }

    /// Ждёт, пока наблюдатель фреймов не обработает событие.
    async fn until(check: impl Fn() -> bool) {
        let wait = async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(2), wait).await.expect("событие не обработано");
    }

    #[tokio::test]
    async fn frame_navigation_drops_worlds_and_ages_saved_elements() {
        let created = Arc::new(Counter::new(0));
        let counter = created.clone();
        let mock = Mock::spawn(move |method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": counter.fetch_add(1, Ordering::SeqCst) + 1 }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "object", "objectId": "O1" } }),
                "DOM.describeNode" => json!({ "node": { "backendNodeId": 42 } }),
                "DOM.focus" | "Runtime.releaseObject" => json!({}),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let main = page.nth(&page.main_doc(), By::Css, "li", 0).await.unwrap().unwrap();
        let inner_doc = Doc { session: "S1".into(), frame_id: "F2".into() };
        let inner = page.nth(&inner_doc, By::Css, "li", 0).await.unwrap().unwrap();
        // Переход фрейма F2: его мир и элементы больше не годятся, главный документ на месте.
        mock.events.send(navigated("F2", Some("T1"), "L2")).unwrap();
        until(|| page.generation("F2") != 0).await;
        page.focus(&main).await.unwrap();
        let focused = mock.sent("DOM.focus").len();
        assert!(matches!(page.focus(&inner).await, Err(CdpError::Page(m)) if m == GONE));
        assert!(matches!(page.read(&inner, Read::Text).await, Err(CdpError::Page(m)) if m == GONE));
        assert!(matches!(page.point(&inner, None).await, Err(CdpError::Page(m)) if m == GONE));
        assert_eq!(mock.sent("DOM.focus").len(), focused, "до DOM.* дело не дошло");
        assert!(mock.sent("DOM.resolveNode").is_empty() && mock.sent("DOM.scrollIntoViewIfNeeded").is_empty());
        page.count(&inner_doc, By::Css, "li").await.unwrap();
        // Переход главного фрейма: прочь всё.
        mock.events.send(navigated("T1", None, "L3")).unwrap();
        until(|| page.generation("T1") != 0).await;
        assert!(matches!(page.focus(&main).await, Err(CdpError::Page(m)) if m == GONE));
        page.count(&page.main_doc(), By::Css, "li").await.unwrap();
        let worlds: Vec<Value> = mock.sent("Page.createIsolatedWorld").iter().map(|p| p["frameId"].clone()).collect();
        assert_eq!(worlds, [json!("T1"), json!("F2"), json!("F2"), json!("T1")], "миры перешедших фреймов созданы заново");
        assert_eq!(created.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn cross_site_frame_navigation_in_its_own_session_ages_its_elements() {
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "object", "objectId": "O1" } }),
                "DOM.describeNode" => json!({ "node": { "backendNodeId": 42 } }),
                "DOM.focus" | "Runtime.releaseObject" => json!({}),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let attached = json!({ "method": "Target.attachedToTarget", "sessionId": "S1", "params": { "sessionId": "S9", "targetInfo": { "targetId": "F3", "type": "iframe" } } });
        mock.events.send(attached).unwrap();
        until(|| page.frames.lock().unwrap().contains_key("F3") && mock.sessions("Page.enable").len() == 2).await;
        assert_eq!(mock.sessions("Page.enable"), ["S1", "S9"], "переходы межсайтового фрейма — из его сессии");
        let main = page.nth(&page.main_doc(), By::Css, "li", 0).await.unwrap().unwrap();
        let doc = Doc { session: "S9".into(), frame_id: "F3".into() };
        let inner = page.nth(&doc, By::Css, "li", 0).await.unwrap().unwrap();
        // Межсайтовый переход фрейма B→C: сессия та же, событие — только в ней.
        let commit = |session: &str, frame: &str, loader: &str| {
            json!({ "method": "Page.frameNavigated", "sessionId": session, "params": { "frame": { "id": frame, "parentId": "T1", "loaderId": loader } } })
        };
        mock.events.send(commit("S9", "F3", "L2")).unwrap();
        until(|| page.generation("F3") != 0).await;
        let generation = page.generation("F3");
        assert!(matches!(page.focus(&inner).await, Err(CdpError::Page(m)) if m == GONE));
        assert!(matches!(page.read(&inner, Read::Text).await, Err(CdpError::Page(m)) if m == GONE));
        page.focus(&main).await.unwrap();
        assert_eq!(mock.sessions("DOM.focus"), ["S1"], "элемент прежнего документа фрейма до DOM.* не дошёл");
        assert!(mock.sent("DOM.resolveNode").is_empty());
        page.count(&doc, By::Css, "li").await.unwrap();
        assert_eq!(mock.sessions("Page.createIsolatedWorld"), ["S1", "S9", "S9"], "мир фрейма создан заново");
        // Тот же переход из сессии вкладки и повтор — не новое поколение; чужая сессия не в счёт. Метка — переход
        // другого фрейма: события обрабатываются по порядку.
        for event in [commit("S1", "F3", "L2"), commit("S9", "F3", "L2"), commit("S7", "F3", "L5"), commit("S1", "F4", "L6")] {
            mock.events.send(event).unwrap();
        }
        until(|| page.generation("F4") != 0).await;
        assert_eq!(page.generation("F3"), generation);
        assert_eq!(page.generation("T1"), 0, "главный документ на месте");
    }

    #[tokio::test]
    async fn a_frame_session_root_without_parent_is_not_the_main_frame() {
        let mock = Mock::spawn(|method, _, session| {
            // Занятый рендерер фрейма не отвечает на `Page.enable` — наблюдатель его не ждёт.
            if (method, session) == ("Page.enable", "S9") {
                return json!({ "silent": true });
            }
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "number", "value": 1 } }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let attached = json!({ "method": "Target.attachedToTarget", "sessionId": "S1", "params": { "sessionId": "S9", "targetInfo": { "targetId": "F3", "type": "iframe" } } });
        mock.events.send(attached).unwrap();
        until(|| page.frames.lock().unwrap().contains_key("F3")).await;
        page.count(&page.main_doc(), By::Css, "li").await.unwrap();
        // Корень сессии фрейма без `parentId` (fenced frame и т.п.): подфрейм, вкладка его переход не видит.
        let commit = json!({ "method": "Page.frameNavigated", "sessionId": "S9", "params": { "frame": { "id": "F3", "loaderId": "L2" } } });
        mock.events.send(commit).unwrap();
        until(|| page.generation("F3") != 0).await;
        assert_eq!(page.generation("T1"), 0, "главный документ вкладки на месте");
        page.count(&page.main_doc(), By::Css, "li").await.unwrap();
        assert_eq!(mock.sent("Page.createIsolatedWorld").len(), 1, "мир главного документа — из кэша");
    }

    #[test]
    fn a_navigation_seen_twice_counts_once() {
        let mut docs = Documents::default();
        docs.worlds.insert(Doc { session: "S1".into(), frame_id: "F2".into() }, 3);
        docs.navigated("F2", false, "L1");
        let inner = docs.generation("F2");
        assert!(inner != 0 && docs.worlds.is_empty());
        docs.navigated("F2", false, "L1");
        assert_eq!(docs.generation("F2"), inner, "то же событие у наблюдателя и у ожидания загрузки");
        assert_eq!(docs.generation("T1"), 0, "переход фрейма не трогает главный документ");
        docs.navigated("T1", true, "L2");
        assert!(docs.generation("T1") > inner && docs.generation("F2") == docs.generation("T1"), "главный переход — новое поколение всех фреймов");
    }

    #[tokio::test]
    async fn goto_itself_retires_the_old_document() {
        let mock = Mock::spawn(|method, _, _| match method {
            "Page.navigate" => json!({ "frameId": "T0", "loaderId": "L2" }),
            _ => json!({}),
        })
        .await;
        let browser = Browser::connect(&mock.url).await.unwrap();
        // Без наблюдателя фреймов: смену документа отмечает само ожидание загрузки.
        let page = Page::detached(&browser);
        page.docs().worlds.insert(page.main_doc(), 5);
        let sender = mock.events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let commit = json!({ "method": "Page.frameNavigated", "sessionId": "S0", "params": { "frame": { "id": "T0", "loaderId": "L2" } } });
            for event in [commit, json!({ "method": "Page.loadEventFired", "sessionId": "S0", "params": {} })] {
                sender.send(event).unwrap();
            }
        });
        page.goto("https://b.test/", Duration::from_secs(2)).await.unwrap();
        assert!(page.docs().worlds.is_empty(), "мир прежнего документа убран до возврата из перехода");
        assert_ne!(page.generation("T0"), 0);
    }

    #[tokio::test]
    async fn all_is_one_search_and_releases_its_objects() {
        let mock = Mock::spawn(|method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "object", "subtype": "array", "objectId": "L" } }),
                "Runtime.getProperties" => json!({ "result": [
                    { "name": "1", "value": { "type": "object", "objectId": "B" } },
                    { "name": "0", "value": { "type": "object", "objectId": "A" } },
                    { "name": "length", "value": { "type": "number", "value": 2 } }
                ] }),
                "DOM.describeNode" => json!({ "node": { "backendNodeId": if params["objectId"] == "A" { 1 } else { 2 } } }),
                "Runtime.releaseObject" => json!({}),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let found = page.all(&page.main_doc(), By::Css, "li").await.unwrap();
        assert_eq!(found.iter().map(|e| e.node).collect::<Vec<_>>(), [1, 2], "по номерам массива");
        assert_eq!(mock.sent("Runtime.callFunctionOn").len(), 1, "один поиск на весь список");
        let released: Vec<Value> = mock.sent("Runtime.releaseObject").iter().map(|p| p["objectId"].clone()).collect();
        assert_eq!(released, [json!("A"), json!("B"), json!("L")]);
    }

    /// Ждёт `CALL_TIMEOUT`: чтение элемента и список — одновременно.
    #[tokio::test]
    async fn timed_out_calls_release_nothing_and_stop_the_list() {
        let mock = Mock::spawn(|method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "DOM.resolveNode" => json!({ "object": { "objectId": "O1" } }),
                "Runtime.callFunctionOn" if params["objectId"] == "O1" => json!({ "silent": true }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "object", "subtype": "array", "objectId": "L" } }),
                "Runtime.getProperties" => json!({ "result": [
                    { "name": "0", "value": { "type": "object", "objectId": "A" } },
                    { "name": "1", "value": { "type": "object", "objectId": "B" } }
                ] }),
                "DOM.describeNode" if params["objectId"] == "A" => json!({ "silent": true }),
                "DOM.describeNode" => json!({ "node": { "backendNodeId": 2 } }),
                "Runtime.releaseObject" => json!({}),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let element = ElementRef { session: "S1".into(), frame_id: "T1".into(), node: 5, generation: 0 };
        let doc = page.main_doc();
        let (read, all) = tokio::join!(page.read(&element, Read::Text), page.all(&doc, By::Css, "li"));
        assert!(matches!(read, Err(CdpError::Timeout)), "{read:?}");
        assert!(matches!(all, Err(CdpError::Timeout)), "{all:?}");
        assert_eq!(mock.sent("DOM.describeNode").len(), 1, "после тайм-аута элементы списка не описываются");
        let released: Vec<Value> = mock.sent("Runtime.releaseObject").iter().map(|p| p["objectId"].clone()).collect();
        assert_eq!(released, [json!("L")], "объекты вызовов без ответа не отпускаются, список — отпускается");
    }

    #[tokio::test]
    async fn connected_is_false_when_the_node_document_or_session_is_gone() {
        let mock = Mock::spawn(|method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "DOM.resolveNode" if params["backendNodeId"] == 7 => json!({ "error": "Session with given id not found." }),
                "DOM.resolveNode" => json!({ "object": { "objectId": format!("O{}", params["backendNodeId"]) } }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "boolean", "value": params["objectId"] == "O5" } }),
                "DOM.focus" => json!({ "error": "No session with given id" }),
                "Runtime.releaseObject" => json!({}),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let at = |node| ElementRef { session: "S1".into(), frame_id: "T1".into(), node, generation: 0 };
        assert!(page.connected(&at(5)).await.unwrap());
        assert!(!page.connected(&at(6)).await.unwrap(), "узел вне DOM");
        assert!(!page.connected(&at(7)).await.unwrap(), "сессии элемента нет");
        assert!(!page.connected(&ElementRef { generation: 9, ..at(5) }).await.unwrap(), "документ сменился");
        assert!(matches!(page.focus(&at(5)).await, Err(CdpError::Page(m)) if m == GONE), "закрытая сессия — элемент пропал");
    }

    #[tokio::test]
    async fn script_exceptions_are_not_retried_even_when_they_mention_a_context() {
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "object" }, "exceptionDetails": { "text": "Uncaught", "exception": { "description": "Error: Cannot find context x" } } }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        let result = page.count(&page.main_doc(), By::Css, "li").await;
        assert!(matches!(result, Err(CdpError::Script(ref m)) if m == "Error: Cannot find context x"), "{result:?}");
        assert_eq!(mock.sent("Page.createIsolatedWorld").len(), 1);
    }

    #[tokio::test]
    async fn tab_address_comes_from_target_info() {
        let mock = Mock::spawn(|method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Target.getTargetInfo" if params["targetId"] == "T1" => json!({ "targetInfo": { "url": "https://a.test/p?k=7" } }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = tab(&mock).await;
        assert_eq!(page.url().await.unwrap(), "https://a.test/p?k=7");
    }

    #[tokio::test]
    async fn files_go_only_into_file_inputs() {
        let mock = Mock::spawn(|method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "DOM.resolveNode" => json!({ "object": { "objectId": if params["backendNodeId"] == 5 { "O5" } else { "O6" } } }),
                "Runtime.callFunctionOn" => json!({ "result": { "value": params["objectId"] == "O5" } }),
                _ => json!({}),
            })
        })
        .await;
        let page = tab(&mock).await;
        // ElementRef::test — узел главного фрейма T1 в сессии S1, первое поколение документа.
        let input = ElementRef::test(5);
        page.set_files(&input, &["/tmp/a.txt".to_string()]).await.unwrap();
        assert_eq!(mock.sent("DOM.setFileInputFiles")[0], json!({ "files": ["/tmp/a.txt"], "backendNodeId": 5 }));
        let other = ElementRef::test(6);
        assert!(matches!(page.set_files(&other, &["/tmp/a.txt".to_string()]).await, Err(CdpError::Page(m)) if m == "элемент — не поле выбора файла"));
        assert_eq!(mock.sent("DOM.setFileInputFiles").len(), 1);
    }
}
