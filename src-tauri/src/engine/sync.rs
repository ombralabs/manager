//! Синхронизатор окон в движке (спецификация синхронизатора, «Как это устроено»): перехват ввода главного окна.
//! Скрипт перехвата живёт в изолированном мире `ombra-sync` — мир страницы его не видит; события ядро забирает
//! длинным опросом `Runtime.evaluate` с `awaitPromise`, без `Runtime.enable`.
use super::cdp::{CALL_TIMEOUT, CdpError};
use super::page::{Browser, Button, CdpEvent, Frames, LoadWait, Page, mac_command, result_of};
use crate::sync::model::{History, KeyEvent, Signature};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinHandle, JoinSet};

/// Имя изолированного мира синхронизатора; странице не видно.
pub const SYNC_WORLD: &str = "ombra-sync";
/// Скрипт перехвата: функция `(flags) => …` (её текст — в `sync_capture.js`).
const SCRIPT: &str = include_str!("sync_capture.js");
/// Подпись цели: общий кусок скрипта перехвата (на место `//@SIG`) и проверок ведомых.
const SIG: &str = include_str!("sync_sig.js");
/// Опрос: скрипт отдаёт пустой список через 20 с; нет ответа дольше — рендерер занят, опрос повторяется.
const POLL_WAIT: Duration = Duration::from_secs(30);
/// Событий, ждущих получателя.
const QUEUE: usize = 1024;
/// Как часто перехват сверяется с картой межсайтовых фреймов вкладки: `watch_frames` вносит фрейм после своего вызова,
/// и событие о нём перехват мог пропустить.
const FRAMES_RESCAN: Duration = Duration::from_secs(1);
/// Кусок набора: очеловеченный ввод отвечает после последней клавиши — до 60 с плана (D42 движка) и запас.
const CHUNK_WAIT: Duration = Duration::from_secs(70);

/// Событие скрипта перехвата (JSON, поле `t` — вид) и фрейм, где оно случилось (id главного — id вкладки). Точки
/// событий подфреймов — уже в окне просмотра вкладки.
#[derive(Debug, Clone)]
pub struct Captured {
    pub frame_id: String,
    pub event: Value,
}

/// Что перехватывать сверх кликов, клавиш, колеса и списков; скрипт получает это при установке.
#[derive(Debug, Clone, Copy)]
pub struct CaptureFlags {
    /// Наведение мыши (`move` без нажатой кнопки, не чаще раза в 50 мс); перетаскивание перехватывается всегда.
    pub mouse: bool,
}

/// Скрипты новых документов: сессия и id скрипта в ней.
type Scripts = Arc<Mutex<Vec<(String, String)>>>;

/// Перехват одной вкладки главного окна: скрипт в новых документах её сессии и сессий её межсайтовых фреймов, опрос
/// каждого фрейма.
pub struct Capture {
    browser: Browser,
    scripts: Scripts,
    task: JoinHandle<()>,
}

impl Drop for Capture {
    /// Задача опроса держит клон соединения — кончается вместе с перехватом.
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Capture {
    /// Скрипт — в открытые документы вкладки и её фреймов и во все новые; события — в поток. Поток кончается с
    /// соединением, вкладкой или `stop`.
    pub async fn start(page: &Page, flags: CaptureFlags) -> Result<(Capture, mpsc::Receiver<Captured>), CdpError> {
        let source = format!("({})({})", SCRIPT.replace("//@SIG", SIG), json!({ "mouse": flags.mouse }));
        let (sender, receiver) = mpsc::channel(QUEUE);
        let tab = Arc::new(Tab { browser: page.browser.clone(), session: page.session.clone(), target: page.target_id.clone(), frames: page.frames.clone(), source, sender });
        // Подписка — до миров: переход сразу после них и новый фрейм не пропадут.
        let events = page.browser.subscribe();
        let mut watch = Watch { tab, scripts: Scripts::default(), roots: HashMap::new(), polls: HashMap::new(), tasks: JoinSet::new() };
        watch.install(&page.session, &page.target_id).await?;
        // Межсайтовые фреймы из карты `watch_frames`; внесённые в неё позже подберёт обход карты (`FRAMES_RESCAN`).
        watch.scan().await;
        let scripts = watch.scripts.clone();
        let task = tokio::spawn(watch.run(events));
        Ok((Capture { browser: page.browser.clone(), scripts, task }, receiver))
    }

    /// Опросы кончаются, новые документы скрипт не получают. В открытых он остаётся до перехода — с ограниченной очередью.
    pub async fn stop(self) {
        self.task.abort();
        let scripts = std::mem::take(&mut *self.scripts.lock().expect("мьютекс скриптов отравлен"));
        for (session, identifier) in scripts {
            let _ = self.browser.call(Some(&session), "Page.removeScriptToEvaluateOnNewDocument", json!({ "identifier": identifier })).await;
        }
    }
}

/// Вкладка перехвата: соединение, сессия и id вкладки, сессии её межсайтовых фреймов, скрипт с флагами и поток событий.
struct Tab {
    browser: Browser,
    session: String,
    target: String,
    frames: Frames,
    source: String,
    sender: mpsc::Sender<Captured>,
}

/// Сессии вкладки со скриптом и опросы их фреймов. Кончается с вкладкой, соединением или получателем — с ним
/// снимаются и опросы.
struct Watch {
    tab: Arc<Tab>,
    scripts: Scripts,
    /// Сессии со скриптом и их корневые фреймы.
    roots: HashMap<String, String>,
    /// Опросы по сессии и фрейму.
    polls: HashMap<(String, String), AbortHandle>,
    tasks: JoinSet<()>,
}

impl Watch {
    /// Скрипт — в новые документы сессии с корнем `root`.
    async fn install(&mut self, session: &str, root: &str) -> Result<(), CdpError> {
        let params = json!({ "source": self.tab.source, "worldName": SYNC_WORLD });
        let reply = self.tab.browser.call(Some(session), "Page.addScriptToEvaluateOnNewDocument", params).await?;
        let script = reply["identifier"].as_str().ok_or_else(|| CdpError::Protocol("нет identifier".into()))?;
        self.scripts.lock().expect("мьютекс скриптов отравлен").push((session.to_string(), script.to_string()));
        self.roots.insert(session.to_string(), root.to_string());
        Ok(())
    }

    /// Сессии межсайтовых фреймов из карты вкладки — со скриптом; опросы — всем фреймам всех сессий. Главный фрейм —
    /// и без дерева фреймов.
    async fn scan(&mut self) {
        self.adopt().await;
        let (session, target) = (self.tab.session.clone(), self.tab.target.clone());
        self.poll(&session, &target).await;
        for session in self.roots.keys().cloned().collect::<Vec<_>>() {
            self.poll_tree(&session).await;
        }
    }

    /// Сессии из карты вкладки, где скрипта ещё нет (или он не встал), — скрипт и опросы их фреймов.
    async fn adopt(&mut self) {
        let found: Vec<(String, String)> = self.tab.frames.lock().expect("мьютекс фреймов отравлен").iter().map(|(frame, known)| (known.session.clone(), frame.clone())).collect();
        for (session, root) in found {
            if !self.roots.contains_key(&session) && self.install(&session, &root).await.is_ok() {
                self.poll_tree(&session).await;
            }
        }
    }

    /// Опросы фреймов сессии по её дереву.
    async fn poll_tree(&mut self, session: &str) {
        let Ok(tree) = self.tab.browser.call(Some(session), "Page.getFrameTree", json!({})).await else { return };
        let mut frames = Vec::new();
        frame_ids(&tree["frameTree"], &mut frames);
        for frame in frames {
            self.poll(session, &frame).await;
        }
    }

    /// Опрос фрейма, если его ещё нет: мир документа — до ответа. Мир подфрейма не создался (фрейм ушёл, он чужого
    /// процесса) — опроса нет, следующий переход фрейма начнёт его снова; главный фрейм ждёт перехода или диалога.
    // ponytail: миры при старте — по очереди, зависший рендерер фрейма задержит старт на время вызовов; мир — в задаче
    // опроса, если это проявится.
    async fn poll(&mut self, session: &str, frame: &str) {
        let key = (session.to_string(), frame.to_string());
        if self.polls.get(&key).is_some_and(|poll| !poll.is_finished()) {
            return;
        }
        while self.tasks.try_join_next().is_some() {}
        self.polls.retain(|_, poll| !poll.is_finished());
        let poll = Poll { tab: self.tab.clone(), session: key.0.clone(), frame: key.1.clone() };
        let events = self.tab.browser.subscribe();
        let world = poll.world().await.ok();
        if world.is_some() || *frame == self.tab.target {
            self.polls.insert(key, self.tasks.spawn(poll.run(world, events)));
        }
    }

    async fn run(mut self, mut events: broadcast::Receiver<CdpEvent>) {
        let mut rescan = tokio::time::interval_at(tokio::time::Instant::now() + FRAMES_RESCAN, FRAMES_RESCAN);
        rescan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let event = tokio::select! {
                () = self.tab.sender.closed() => return,
                _ = rescan.tick() => {
                    self.adopt().await;
                    continue;
                }
                event = events.recv() => event,
            };
            let event = match event {
                Ok(event) => event,
                // Отстали: сессии и фреймы — по карте и деревьям.
                Err(RecvError::Lagged(_)) => {
                    self.scan().await;
                    continue;
                }
                Err(RecvError::Closed) => return,
            };
            let from = event.session.clone().unwrap_or_default();
            match event.method.as_str() {
                // Межсайтовый фрейм вкладки или её фрейма; подключает его `watch_frames`.
                "Target.attachedToTarget" if self.roots.contains_key(&from) && event.params["targetInfo"]["type"] == "iframe" => {
                    let (Some(session), Some(root)) = (event.params["sessionId"].as_str(), event.params["targetInfo"]["targetId"].as_str()) else {
                        continue;
                    };
                    // Сессию мог уже взять обход карты: второй скрипт ей не нужен.
                    if !self.roots.contains_key(session) && self.install(session, root).await.is_ok() {
                        self.poll_tree(session).await;
                    }
                }
                "Target.detachedFromTarget" => {
                    let session = event.params["sessionId"].as_str().unwrap_or_default();
                    if session == self.tab.session {
                        return;
                    }
                    if self.roots.remove(session).is_some() {
                        self.scripts.lock().expect("мьютекс скриптов отравлен").retain(|(known, _)| known != session);
                    }
                }
                // Новый документ фрейма.
                "Page.frameNavigated" if self.roots.contains_key(&from) => {
                    let Some(frame) = event.params["frame"]["id"].as_str() else { continue };
                    if self.roots[&from] != frame {
                        self.poll(&from, frame).await;
                        continue;
                    }
                    // Документ корня сессии: подфреймы прежнего ушли с ним, у вернувшегося из кэша «назад-вперёд» — свои
                    // прежние, без переходов.
                    self.polls.retain(|(session, other), poll| {
                        let gone = *session == from && other != frame;
                        if gone {
                            poll.abort();
                        }
                        !gone
                    });
                    self.poll_tree(&from).await;
                }
                _ => {}
            }
        }
    }
}

/// Id всех фреймов дерева `Page.getFrameTree`.
fn frame_ids(tree: &Value, ids: &mut Vec<String>) {
    ids.extend(tree["frame"]["id"].as_str().map(str::to_string));
    for child in tree["childFrames"].as_array().into_iter().flatten() {
        frame_ids(child, ids);
    }
}

/// Фрейм `id` в дереве `Page.getFrameTree`.
fn find_frame<'a>(tree: &'a Value, id: &str) -> Option<&'a Value> {
    if tree["frame"]["id"] == id {
        return Some(&tree["frame"]);
    }
    tree["childFrames"].as_array()?.iter().find_map(|child| find_frame(child, id))
}

/// Опрос одного фрейма вкладки в его сессии.
struct Poll {
    tab: Arc<Tab>,
    session: String,
    frame: String,
}

impl Poll {
    /// Мир синхронизатора текущего документа со скриптом и `loaderId` документа. `createIsolatedWorld` отдаёт тот же
    /// мир, куда браузер ставит скрипт новых документов; документу, открытому до старта, ставим сами — от второй
    /// установки скрипт защищён.
    async fn world(&self) -> Result<(i64, String), CdpError> {
        let reply = self.call("Page.createIsolatedWorld", json!({ "frameId": self.frame, "worldName": SYNC_WORLD })).await?;
        let context = reply["executionContextId"].as_i64().ok_or_else(|| CdpError::Protocol("нет executionContextId".into()))?;
        // `loaderId` — после мира: переход между ними оставит мир прежнего документа, его опрос кончится ошибкой. Прочитанный
        // раньше, он записал бы мир нового документа на прежний, и событие о переходе бросило бы живой опрос.
        let loader = self.loader().await?;
        self.evaluate(context, &self.tab.source, CALL_TIMEOUT).await?;
        Ok((context, loader))
    }

    /// `loaderId` документа фрейма сейчас.
    async fn loader(&self) -> Result<String, CdpError> {
        let tree = self.call("Page.getFrameTree", json!({})).await?;
        Ok(find_frame(&tree["frameTree"], &self.frame).and_then(|frame| frame["loaderId"].as_str()).unwrap_or_default().to_string())
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
        self.tab.browser.call(Some(&self.session), method, params).await
    }

    async fn evaluate(&self, context: i64, expression: &str, wait: Duration) -> Result<Value, CdpError> {
        let params = json!({ "expression": expression, "contextId": context, "awaitPromise": true, "returnByValue": true });
        result_of(self.tab.browser.call_for(Some(&self.session), "Runtime.evaluate", params, wait).await?)
    }

    /// Опрос до обрыва соединения, закрытия сессии или фрейма или ухода получателя. Переход фрейма или ошибка мира —
    /// новый мир; не создался — ждём следующего перехода.
    async fn run(self, mut world: Option<(i64, String)>, mut events: broadcast::Receiver<CdpEvent>) {
        // Последний известный сдвиг окна подфрейма от окна вкладки.
        let mut offset = None;
        loop {
            let context = world.as_ref().map(|(context, _)| *context);
            // Один опрос живёт до ответа или смены документа: брошенный терял бы уже отданную пачку, а события других
            // вкладок и доменов соединения перезапускали бы его.
            let stale = {
                let poll = async {
                    match context {
                        Some(context) => self.evaluate(context, "globalThis.__ombraSyncNext()", POLL_WAIT).await,
                        None => std::future::pending().await,
                    }
                };
                tokio::pin!(poll);
                loop {
                    // Готовый ответ — раньше перехода: события прежнего документа не теряются.
                    tokio::select! {
                        biased;
                        reply = &mut poll => break match reply {
                            Ok(batch) => {
                                if !self.deliver(&batch["value"], &mut offset).await {
                                    return;
                                }
                                false
                            }
                            Err(CdpError::Closed) => return,
                            Err(CdpError::Timeout) => false,
                            Err(_) => true,
                        },
                        event = events.recv() => match event {
                            Ok(event) if event.method == "Target.detachedFromTarget" && event.params["sessionId"].as_str() == Some(&self.session) => return,
                            Ok(event) if event.session.as_deref() != Some(&self.session) => {}
                            Ok(event) if event.method == "Page.frameDetached" && event.params["frameId"] == self.frame.as_str() => return,
                            // Переход в документ, где мир уже стоит, — опрос не бросается: старый ответил ошибкой
                            // («Inspected target navigated») раньше этого события, и пачка нового пропала бы.
                            Ok(event) if event.method == "Page.frameNavigated"
                                && event.params["frame"]["id"] == self.frame.as_str()
                                && event.params["frame"]["loaderId"].as_str() != world.as_ref().map(|(_, loader)| loader.as_str()) => break true,
                            // Мир не создался, пока документ держал диалог (`alert` при загрузке дольше вызова): диалог
                            // закрыли — пробуем снова.
                            Ok(event) if context.is_none() && event.method == "Page.javascriptDialogClosed" => break true,
                            Ok(_) => {}
                            // Отставший мог пропустить переход. Документ тот же — опрос остаётся: брошенный унёс бы уже
                            // отданную пачку; мир ушедшего документа кончит опрос ошибкой сам.
                            Err(RecvError::Lagged(_)) => {
                                if self.loader().await.ok().as_deref() != world.as_ref().map(|(_, loader)| loader.as_str()) {
                                    break true;
                                }
                            }
                            Err(RecvError::Closed) => return,
                        },
                    }
                }
            };
            if stale {
                world = self.world().await.ok();
            }
        }
    }

    /// Пачка событий — получателю; `false` — его нет. Точки подфрейма переводятся в окно вкладки: сдвиг — один на
    /// пачку. Не узнать (клик убрал фрейм) — прежний: отпускание не теряется после нажатия; не было и его — события с
    /// точкой выбрасываются, ведомым её не найти.
    async fn deliver(&self, batch: &Value, offset: &mut Option<(f64, f64)>) -> bool {
        let events = batch.as_array().map(Vec::as_slice).unwrap_or_default();
        let sub = self.frame != self.tab.target;
        // ponytail: сдвиг читается, когда пачка пришла, а не в миг события: колесо во фрейме, прокрутившее родителя, уже
        // сдвинуло фрейм, и точки пачки переводятся со сдвигом после прокрутки. Сдвиг на миг события, если это проявится.
        if sub
            && events.iter().any(|event| event["x"].is_number() && event["t"] != "move")
            && let Ok(now) = self.offset().await
        {
            *offset = Some(now);
        }
        for event in events {
            let mut event = event.clone();
            // ponytail: наведение во фреймах ведомым не идёт (наведение внутри фрейма у них не повторится) — сдвиг на
            // каждое движение был бы лишним вызовом; кэш сдвига по документу фрейма, если понадобится. Перетаскивание
            // идёт со сдвигом, прочитанным для его нажатия.
            if sub && event["t"] == "move" && event["buttons"].as_i64().unwrap_or(0) == 0 {
                continue;
            }
            if sub && let (Some(x), Some(y)) = (event["x"].as_f64(), event["y"].as_f64()) {
                let Some((dx, dy)) = *offset else { continue };
                (event["x"], event["y"]) = (json!(x + dx), json!(y + dy));
            }
            let Ok(permit) = self.tab.sender.reserve().await else { return false; };
            permit.send(Captured { frame_id: self.frame.clone(), event });
        }
        true
    }

    /// Сдвиг окна фрейма от окна вкладки: левые верхние углы `content` (`DOM.getBoxModel`) владельцев (`DOM.getFrameOwner`)
    /// по цепочке. Рамка владельца — от окна корня его сессии (фреймы одного процесса сдвигов не копят), у межсайтового
    /// корня владелец — в сессии родителя.
    async fn offset(&self) -> Result<(f64, f64), CdpError> {
        let tab = &self.tab;
        let (mut session, mut frame, mut offset) = (self.session.clone(), self.frame.clone(), (0.0, 0.0));
        loop {
            let parent = tab.frames.lock().expect("мьютекс фреймов отравлен").get(&frame).filter(|known| known.session == session).map(|known| known.parent.clone());
            let at = parent.unwrap_or(session);
            let owner = tab.browser.call(Some(&at), "DOM.getFrameOwner", json!({ "frameId": frame })).await?;
            let model = tab.browser.call(Some(&at), "DOM.getBoxModel", json!({ "backendNodeId": owner["backendNodeId"] })).await?;
            let content = &model["model"]["content"];
            offset = (offset.0 + content[0].as_f64().unwrap_or(0.0), offset.1 + content[1].as_f64().unwrap_or(0.0));
            if at == tab.session {
                return Ok(offset);
            }
            // Дальше — владелец корня сессии `at`.
            let root = tab.frames.lock().expect("мьютекс фреймов отравлен").iter().find(|(_, known)| known.session == at).map(|(id, _)| id.clone());
            frame = root.ok_or_else(|| CdpError::Protocol("неизвестный межсайтовый фрейм".into()))?;
            session = at;
        }
    }
}

/// Элемент под точкой окна фрейма.
fn at_point((x, y): (f64, f64)) -> String {
    format!("document.elementFromPoint({x}, {y})")
}

/// Документ, где ищется подпись: сессия, фрейм и сдвиг окна корня сессии от окна вкладки.
struct Spot {
    session: String,
    frame: String,
    root: (f64, f64),
}

/// Операции ведомой вкладки: только доверенный ввод `Input.*`; страница не получает ни событий, ни значений из скрипта.
impl Page {
    /// Подпись элемента под точкой окна вкладки (мир `ombra-sync`, подпись строится как у главного); пусто — `None`.
    pub async fn signature_at(&self, x: f64, y: f64) -> Result<Option<Signature>, CdpError> {
        Ok(self.find(at_point, (x, y)).await?.0)
    }

    /// Подпись элемента в фокусе: фокус во фрейме — поле в его документе.
    pub async fn focused_signature(&self) -> Result<Option<Signature>, CdpError> {
        Ok(self.find(|_| "document.activeElement".into(), (0.0, 0.0)).await?.0)
    }

    /// Элемент `element(точка)` — выражения от точки в окне фрейма: подпись, его документ и выражение в нём. Это
    /// `<iframe>` или `<frame>` — спуск в документ фрейма (у межсайтового — своя сессия), точка переводится в его окно.
    async fn find(&self, element: impl Fn((f64, f64)) -> String, (x, y): (f64, f64)) -> Result<(Option<Signature>, Spot, String), CdpError> {
        let mut spot = self.top();
        let mut at = (x, y);
        loop {
            let expression = element(at);
            let value = self.evaluate_in(&spot, &format!("(() => {{\n{SIG}\nreturn sig({expression});\n}})()"), true).await?;
            let sig = (!value.is_null()).then(|| Signature::from_json(&value));
            if !sig.as_ref().is_some_and(|sig| matches!(sig.tag.as_str(), "IFRAME" | "FRAME")) {
                return Ok((sig, spot, expression));
            }
            let Some((dx, dy)) = self.enter(&mut spot, &expression).await? else { return Ok((sig, spot, expression)) };
            at = (x - dx, y - dy);
        }
    }

    /// Документ вкладки.
    fn top(&self) -> Spot {
        Spot { session: self.session.clone(), frame: self.target_id.clone(), root: (0.0, 0.0) }
    }

    /// `spot` — во фрейм элемента `element` (`<iframe>` его документа); ответ — сдвиг окна фрейма от окна вкладки,
    /// документа у элемента нет — `None`. Рамка `<iframe>` — от окна корня сессии; межсайтовый фрейм — сам корень.
    async fn enter(&self, spot: &mut Spot, element: &str) -> Result<Option<(f64, f64)>, CdpError> {
        let object = self.evaluate_in(spot, element, false).await?;
        let Some(object) = object["objectId"].as_str() else { return Ok(None) };
        let call = |method: &'static str| self.browser.call(Some(&spot.session), method, json!({ "objectId": object }));
        let (node, model) = (call("DOM.describeNode").await, call("DOM.getBoxModel").await);
        let _ = call("Runtime.releaseObject").await;
        let Some(frame) = node?["node"]["frameId"].as_str().filter(|id| !id.is_empty()).map(str::to_string) else { return Ok(None) };
        let content = &model?["model"]["content"];
        let offset = (spot.root.0 + content[0].as_f64().unwrap_or(0.0), spot.root.1 + content[1].as_f64().unwrap_or(0.0));
        if let Some(known) = self.frames.lock().expect("мьютекс фреймов отравлен").get(&frame).cloned() {
            (spot.session, spot.root) = (known.session, offset);
        }
        spot.frame = frame;
        Ok(Some(offset))
    }

    /// Выражение в мире `ombra-sync` документа `spot`; мир запрашивается на каждый вызов (документ мог смениться).
    // ponytail: лишний вызов на проверку; кэш мира по поколению документа, если проверки станут узким местом.
    async fn evaluate_in(&self, spot: &Spot, expression: &str, by_value: bool) -> Result<Value, CdpError> {
        let session = Some(spot.session.as_str());
        let params = json!({ "frameId": spot.frame, "worldName": SYNC_WORLD });
        let world = self.browser.call(session, "Page.createIsolatedWorld", params).await?;
        let context = world["executionContextId"].as_i64().ok_or_else(|| CdpError::Protocol("нет executionContextId".into()))?;
        let params = json!({ "expression": expression, "contextId": context, "returnByValue": by_value });
        let result = result_of(self.browser.call(session, "Runtime.evaluate", params).await?)?;
        Ok(if by_value { result["value"].clone() } else { result })
    }

    /// Событие мыши: `mouseMoved`, `mousePressed` или `mouseReleased`; `buttons` — маска нажатых кнопок.
    #[allow(clippy::too_many_arguments)]
    pub async fn mouse(&self, kind: &str, x: f64, y: f64, button: Option<Button>, buttons: i64, count: i64, mods: i64) -> Result<(), CdpError> {
        let name = button.map_or("none", Button::name);
        let params = json!({ "type": kind, "x": x, "y": y, "button": name, "buttons": buttons, "clickCount": count, "modifiers": mods });
        self.call("Input.dispatchMouseEvent", params).await.map(|_| ())
    }

    /// Кусок набора одной командой (`Input.insertText`); при очеловечивании движок разворачивает его в нажатия по
    /// персоне и отвечает после последнего — ждём дольше обычного вызова.
    pub async fn insert_chunk(&self, text: &str) -> Result<(), CdpError> {
        self.browser.call_for(Some(&self.session), "Input.insertText", json!({ "text": text }), CHUNK_WAIT).await.map(|_| ())
    }

    /// Колесо над точкой.
    pub async fn wheel_by(&self, x: f64, y: f64, dx: f64, dy: f64, mods: i64) -> Result<(), CdpError> {
        let params = json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy, "modifiers": mods });
        self.call("Input.dispatchMouseEvent", params).await.map(|_| ())
    }

    /// Клавиша главного: печатная — с текстом, с ⌘ или Ctrl — сырая, с командой редактирования macOS (иначе Chromium
    /// её не исполнит), отпускание — `keyUp`.
    pub async fn key(&self, event: &KeyEvent) -> Result<(), CdpError> {
        let mut params = json!({
            "type": "keyUp", "modifiers": event.mods, "key": event.key, "code": event.code,
            "windowsVirtualKeyCode": event.key_code, "location": event.location,
        });
        if event.down {
            // Enter печатает перевод строки (отправка формы, абзац), прочие непечатные — сырое нажатие.
            let text = if event.key == "Enter" && event.mods & 6 == 0 { Some("\r") } else { event.printable() };
            params["type"] = json!(if text.is_some() { "keyDown" } else { "rawKeyDown" });
            if let Some(text) = text {
                params["text"] = json!(text);
                params["unmodifiedText"] = json!(text);
            }
            if let Some(command) = mac_command(&event.key, event.mods) {
                params["commands"] = json!([command]);
            }
        }
        self.call("Input.dispatchKeyEvent", params).await.map(|_| ())
    }

    /// Вариант `<select>` под точкой: фокус на элемент и набор текста варианта по символам (поиск по первым буквам) —
    /// нативный список не открывается, значение скриптом не ставится. `true` — выбран вариант с этой подписью.
    pub async fn select_by_label(&self, x: f64, y: f64, label: &str) -> Result<bool, CdpError> {
        let (sig, spot, expression) = self.find(at_point, (x, y)).await?;
        // Не список (и фрейм, куда не спуститься): фокус и набор ушли бы в чужое поле.
        if sig.is_none_or(|sig| sig.tag != "SELECT") {
            return Ok(false);
        }
        let element = self.evaluate_in(&spot, &expression, false).await?;
        let Some(object) = element["objectId"].as_str() else { return Ok(false) };
        // Объект — документа `spot`, его вызовы — в его сессии; клавиши — во вкладку: браузер ведёт их во фрейм в фокусе.
        let call = |method: &'static str, params: Value| self.browser.call(Some(&spot.session), method, params);
        let focused = call("DOM.focus", json!({ "objectId": object })).await;
        let selected = match focused {
            Ok(_) => {
                let mut typed = Ok(());
                for ch in label.chars() {
                    typed = self.type_char(ch).await;
                    if typed.is_err() {
                        break;
                    }
                }
                match typed {
                    // Объект отпускается и при ошибке набора.
                    Err(error) => Err(error),
                    Ok(()) => {
                        let read = json!({ "functionDeclaration": "function() { return this.selectedOptions ? (this.selectedOptions[0]?.text ?? '') : ''; }", "objectId": object, "returnByValue": true });
                        call("Runtime.callFunctionOn", read).await
                    }
                }
            }
            Err(error) => Err(error),
        };
        let _ = call("Runtime.releaseObject", json!({ "objectId": object })).await;
        Ok(result_of(selected?)?["value"].as_str() == Some(label))
    }

    /// Ответ на диалог страницы (`alert`, `confirm`, `prompt`).
    pub async fn answer_dialog(&self, accept: bool, text: &str) -> Result<(), CdpError> {
        self.call("Page.handleJavaScriptDialog", json!({ "accept": accept, "promptText": text })).await.map(|_| ())
    }

    /// Окно выбора файла не открывается ни во вкладке, ни в её межсайтовых фреймах (и в тех, что появятся): приходит
    /// `Page.fileChooserOpened` в сессию вкладки или фрейма (`has_session`).
    pub async fn intercept_files(&self) -> Result<(), CdpError> {
        let enabled = json!({ "enabled": true });
        let sessions: Vec<String> = {
            let frames = self.frames.lock().expect("мьютекс фреймов отравлен");
            self.files_intercepted.store(true, std::sync::atomic::Ordering::SeqCst);
            frames.values().map(|frame| frame.session.clone()).collect()
        };
        self.call("Page.setInterceptFileChooserDialog", enabled.clone()).await?;
        // Фрейм мог уже уйти — его ошибка не важна.
        for session in sessions {
            let _ = self.browser.call(Some(&session), "Page.setInterceptFileChooserDialog", enabled.clone()).await;
        }
        Ok(())
    }

    /// Сессия — вкладки или её межсайтового фрейма.
    pub fn has_session(&self, session: &str) -> bool {
        session == self.session || self.frames.lock().expect("мьютекс фреймов отравлен").values().any(|frame| frame.session == session)
    }

    /// Снимок истории вкладки.
    pub async fn history(&self) -> Result<History, CdpError> {
        let reply = self.call("Page.getNavigationHistory", json!({})).await?;
        let entries = reply["entries"].as_array().map(Vec::as_slice).unwrap_or_default();
        Ok(History {
            index: reply["currentIndex"].as_u64().unwrap_or(0) as usize,
            ids: entries.iter().map(|e| e["id"].as_i64().unwrap_or(-1)).collect(),
            transitions: entries.iter().map(|e| e["transitionType"].as_str().unwrap_or_default().to_string()).collect(),
            urls: entries.iter().map(|e| e["url"].as_str().unwrap_or_default().to_string()).collect(),
            typed: entries.iter().map(|e| e["userTypedURL"].as_str().unwrap_or_default().to_string()).collect(),
        })
    }

    /// Шаг по истории на `delta` с ожиданием загрузки не дольше `wait`; записи нет — `false`, ничего не происходит.
    pub async fn go_history(&self, delta: i64, wait: Duration) -> Result<bool, CdpError> {
        let history = self.history().await?;
        let target = history.index as i64 + delta;
        let Some(id) = usize::try_from(target).ok().and_then(|i| history.ids.get(i)) else { return Ok(false) };
        let mut events = self.browser.subscribe();
        self.call("Page.navigateToHistoryEntry", json!({ "entryId": id })).await?;
        self.wait_load(&mut events, wait, LoadWait::new(&self.target_id, None, true)).await?;
        Ok(true)
    }

    /// Документ вкладки виден (`visibilityState`, читается в мире `ombra-sync`).
    pub async fn visible(&self) -> Result<bool, CdpError> {
        Ok(self.evaluate_in(&self.top(), "document.visibilityState === 'visible'", true).await? == json!(true))
    }

    /// Сессия вкладки и подписка на события соединения.
    pub fn events(&self) -> (String, broadcast::Receiver<CdpEvent>) {
        (self.session.clone(), self.browser.subscribe())
    }

    /// Id вкладки (он же id главного фрейма).
    pub fn target(&self) -> &str {
        &self.target_id
    }
}

/// Операции браузера окна: вкладки и модули движка `Ombra.*` (ошибка — модуль у профиля выключен: менять нечего).
impl Browser {
    /// Вкладка выходит на передний план.
    pub async fn activate(&self, target_id: &str) -> Result<(), CdpError> {
        self.call(None, "Target.activateTarget", json!({ "targetId": target_id })).await.map(|_| ())
    }

    /// Первая видимая вкладка по порядку `Target.getTargets`; не видна ни одна (окно свёрнуто или закрыто другими) —
    /// `None`. Ушедшая или не ответившая вкладка — не видна.
    pub async fn visible_tab(&self) -> Result<Option<String>, CdpError> {
        for tab in self.tabs().await? {
            let Ok(page) = self.attach(&tab.target_id).await else { continue };
            let visible = page.visible().await.unwrap_or(false);
            let _ = page.detach().await;
            if visible {
                return Ok(Some(tab.target_id));
            }
        }
        Ok(None)
    }

    /// Браузер сообщает о появлении, смене и закрытии вкладок (`Target.targetCreated` и др.; об уже открытых — сразу):
    /// ответ — подписка на события соединения, взятая до включения.
    pub async fn discover_tabs(&self) -> Result<broadcast::Receiver<CdpEvent>, CdpError> {
        let events = self.subscribe();
        self.call(None, "Target.setDiscoverTargets", json!({ "discover": true })).await?;
        Ok(events)
    }

    pub async fn humanize_enabled(&self) -> Option<bool> {
        self.call(None, "Ombra.getHumanizeState", json!({})).await.ok()?["state"]["enabled"].as_bool()
    }

    pub async fn set_humanize(&self, enabled: bool) -> Result<(), CdpError> {
        self.call(None, "Ombra.setHumanize", json!({ "enabled": enabled })).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::page::FrameSession;
    use crate::engine::page::tests::{Mock, base};

    const NEXT: &str = "globalThis.__ombraSyncNext()";

    /// Запросы опроса по порядку.
    fn polls(mock: &Mock) -> Vec<Value> {
        mock.requests.lock().unwrap().iter().filter(|r| r["params"]["expression"] == NEXT).cloned().collect()
    }

    #[tokio::test]
    async fn unrelated_events_keep_the_poll_and_its_batch() {
        let mock = Mock::spawn(|method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.getFrameTree" => json!({ "frameTree": { "frame": { "id": "T1", "loaderId": "L1" } } }),
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                // Ответ опроса тест шлёт сам.
                "Runtime.evaluate" if params["expression"] == NEXT => json!({ "silent": true }),
                "Runtime.evaluate" => json!({ "result": { "type": "undefined" } }),
                "Page.addScriptToEvaluateOnNewDocument" => json!({ "identifier": "1" }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
        let (_capture, mut rx) = Capture::start(&page, CaptureFlags { mouse: false }).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while polls(&mock).is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("опрос не начался");
        // События других вкладок и доменов того же соединения.
        for session in ["S2", "S1"] {
            mock.events.send(json!({ "method": "Network.requestWillBeSent", "sessionId": session, "params": {} })).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let sent = polls(&mock);
        assert_eq!(sent.len(), 1, "чужие события не перезапускают опрос");
        let reply = json!({ "id": sent[0]["id"], "result": { "result": { "type": "object", "value": [{ "t": "visible" }] } } });
        mock.events.send(reply).unwrap();
        let captured = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.expect("пачка не пришла").unwrap();
        assert_eq!((captured.frame_id.as_str(), captured.event), ("T1", json!({ "t": "visible" })));
    }

    #[tokio::test]
    async fn lagging_behind_keeps_the_poll_of_the_same_document() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let installs = Arc::new(AtomicUsize::new(0));
        let count = installs.clone();
        let mock = Mock::spawn(move |method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.getFrameTree" => json!({ "frameTree": { "frame": { "id": "T1", "loaderId": "L1" } } }),
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "Runtime.evaluate" if params["expression"] == NEXT => json!({ "silent": true }),
                // Первую установку скрипта тест отвечает сам — после потока событий, переполнившего подписку опроса.
                "Runtime.evaluate" if count.fetch_add(1, SeqCst) == 0 => json!({ "silent": true }),
                "Runtime.evaluate" => json!({ "result": { "type": "undefined" } }),
                "Page.addScriptToEvaluateOnNewDocument" => json!({ "identifier": "1" }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
        let install = async {
            let request = loop {
                let sent = mock.requests.lock().unwrap().iter().find(|r| r["method"] == "Runtime.evaluate").cloned();
                match sent {
                    Some(request) => break request,
                    None => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            };
            // Больше очереди событий соединения (1024).
            for _ in 0..1100 {
                mock.events.send(json!({ "method": "Network.dataReceived", "sessionId": "S2", "params": {} })).unwrap();
            }
            mock.events.send(json!({ "id": request["id"], "result": { "result": { "type": "undefined" } } })).unwrap();
        };
        let (started, ()) = tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(Capture::start(&page, CaptureFlags { mouse: false }), install) })
            .await
            .expect("перехват не начался");
        let (_capture, mut rx) = started.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while polls(&mock).is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("опрос не начался");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let sent = polls(&mock);
        assert_eq!(sent.len(), 1, "отставший опрос того же документа не бросается");
        let reply = json!({ "id": sent[0]["id"], "result": { "result": { "type": "object", "value": [{ "t": "visible" }] } } });
        mock.events.send(reply).unwrap();
        let captured = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.expect("пачка пропала").unwrap();
        assert_eq!(captured.event, json!({ "t": "visible" }));
    }

    #[tokio::test]
    async fn world_of_a_document_that_came_meanwhile_keeps_its_poll() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let created = Arc::new(AtomicBool::new(false));
        let world = created.clone();
        let mock = Mock::spawn(move |method, params, _| {
            base(method).unwrap_or_else(|| match method {
                // Документ сменился, пока создавался мир: мир — уже нового.
                "Page.getFrameTree" => json!({ "frameTree": { "frame": { "id": "T1", "loaderId": if world.load(SeqCst) { "L2" } else { "L1" } } } }),
                "Page.createIsolatedWorld" => {
                    world.store(true, SeqCst);
                    json!({ "executionContextId": 1 })
                }
                "Runtime.evaluate" if params["expression"] == NEXT => json!({ "silent": true }),
                "Runtime.evaluate" => json!({ "result": { "type": "undefined" } }),
                "Page.addScriptToEvaluateOnNewDocument" => json!({ "identifier": "1" }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
        let (_capture, _rx) = Capture::start(&page, CaptureFlags { mouse: false }).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while polls(&mock).is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("опрос не начался");
        mock.events.send(json!({ "method": "Page.frameNavigated", "sessionId": "S1", "params": { "frame": { "id": "T1", "loaderId": "L2" } } })).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(polls(&mock).len(), 1, "мир записан за документом, где он создан");
    }

    #[tokio::test]
    async fn world_is_retried_when_a_dialog_closes() {
        use std::sync::atomic::{AtomicI64, Ordering::SeqCst};
        async fn until(what: &str, check: impl Fn() -> bool) {
            let wait = async {
                while !check() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            };
            tokio::time::timeout(Duration::from_secs(2), wait).await.unwrap_or_else(|_| panic!("Истекло время ожидания: {what}"));
        }
        let worlds = std::sync::Arc::new(AtomicI64::new(0));
        let count = worlds.clone();
        let mock = Mock::spawn(move |method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.getFrameTree" => json!({ "frameTree": { "frame": { "id": "T1", "loaderId": "L1" } } }),
                // Второй мир не создаётся: документ занят диалогом.
                "Page.createIsolatedWorld" => match count.fetch_add(1, SeqCst) + 1 {
                    2 => json!({ "error": "busy" }),
                    n => json!({ "executionContextId": n }),
                },
                "Runtime.evaluate" if params["expression"] == NEXT => json!({ "silent": true }),
                "Runtime.evaluate" => json!({ "result": { "type": "undefined" } }),
                "Page.addScriptToEvaluateOnNewDocument" => json!({ "identifier": "1" }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
        let (_capture, _rx) = Capture::start(&page, CaptureFlags { mouse: false }).await.unwrap();
        until("опрос", || !polls(&mock).is_empty()).await;
        mock.events.send(json!({ "method": "Page.frameNavigated", "sessionId": "S1", "params": { "frame": { "id": "T1" } } })).unwrap();
        until("мир нового документа", || worlds.load(SeqCst) == 2).await;
        mock.events.send(json!({ "method": "Page.javascriptDialogClosed", "sessionId": "S1", "params": { "result": true, "userInput": "" } })).unwrap();
        until("опрос в мире, созданном после диалога", || polls(&mock).iter().any(|p| p["params"]["contextId"] == 3)).await;
    }

    #[tokio::test]
    async fn navigation_already_seen_keeps_the_poll() {
        use std::sync::atomic::{AtomicI64, Ordering::SeqCst};
        let polled = std::sync::Arc::new(AtomicI64::new(0));
        let count = polled.clone();
        let mock = Mock::spawn(move |method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.getFrameTree" => json!({ "frameTree": { "frame": { "id": "T1", "loaderId": "L2" } } }),
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                // Опрос прежнего документа отвечает ошибкой раньше события о переходе; следующий тест шлёт сам.
                "Runtime.evaluate" if params["expression"] == NEXT => match count.fetch_add(1, SeqCst) {
                    0 => json!({ "error": "Inspected target navigated or closed" }),
                    _ => json!({ "silent": true }),
                },
                "Runtime.evaluate" => json!({ "result": { "type": "undefined" } }),
                "Page.addScriptToEvaluateOnNewDocument" => json!({ "identifier": "1" }),
                _ => json!({ "error": "unexpected" }),
            })
        })
        .await;
        let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
        let (_capture, mut rx) = Capture::start(&page, CaptureFlags { mouse: false }).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while polls(&mock).len() < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("опрос нового документа не начался");
        // Событие о том же переходе — уже после нового мира.
        mock.events.send(json!({ "method": "Page.frameNavigated", "sessionId": "S1", "params": { "frame": { "id": "T1", "loaderId": "L2" } } })).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let sent = polls(&mock);
        assert_eq!(sent.len(), 2, "опрос документа, где уже стоит мир, не бросается");
        let reply = json!({ "id": sent[1]["id"], "result": { "result": { "type": "object", "value": [{ "t": "down" }] } } });
        mock.events.send(reply).unwrap();
        let captured = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.expect("пачка пропала").unwrap();
        assert_eq!(captured.event, json!({ "t": "down" }));
    }

    /// Межсайтовый фрейм `F3` вкладки `T1` подключился сессией `S9` и попал в карту фреймов страницы.
    async fn with_cross_frame(mock: &Mock, page: &Page) {
        let attached = json!({ "method": "Target.attachedToTarget", "sessionId": "S1", "params": { "sessionId": "S9", "targetInfo": { "targetId": "F3", "type": "iframe" } } });
        mock.events.send(attached).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !page.frames.lock().unwrap().contains_key("F3") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("фрейм не подключился");
    }

    #[tokio::test]
    async fn cross_site_frame_is_polled_in_its_session_with_points_in_the_tab_window() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let owners = Arc::new(AtomicUsize::new(0));
        let asked = owners.clone();
        let mock = Mock::spawn(move |method, params, session| {
            base(method).unwrap_or_else(|| match (method, session) {
                ("Page.getFrameTree", "S1") => json!({ "frameTree": { "frame": { "id": "T1", "loaderId": "L1" } } }),
                ("Page.getFrameTree", "S9") => json!({ "frameTree": { "frame": { "id": "F3", "loaderId": "L3" } } }),
                ("Page.createIsolatedWorld", _) => json!({ "executionContextId": if session == "S9" { 9 } else { 1 } }),
                ("Runtime.evaluate", _) if params["expression"] == NEXT => json!({ "silent": true }),
                ("Runtime.evaluate", _) => json!({ "result": { "type": "undefined" } }),
                ("Page.addScriptToEvaluateOnNewDocument", _) => json!({ "identifier": session }),
                // `<iframe>` фрейма — в документе вкладки; после первого клика страница его убрала.
                ("DOM.getFrameOwner", "S1") if params["frameId"] == "F3" => match asked.fetch_add(1, SeqCst) {
                    0 => json!({ "backendNodeId": 10 }),
                    _ => json!({ "error": "Frame with the given id was not found." }),
                },
                ("DOM.getBoxModel", "S1") if params["backendNodeId"] == 10 => json!({ "model": { "content": [314, 10, 614, 10, 614, 160, 314, 160] } }),
                _ => json!({ "error": format!("unexpected {method} in {session}") }),
            })
        })
        .await;
        let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
        with_cross_frame(&mock, &page).await;
        let (_capture, mut rx) = Capture::start(&page, CaptureFlags { mouse: true }).await.unwrap();
        assert_eq!(mock.sessions("Page.addScriptToEvaluateOnNewDocument"), ["S1", "S9"], "скрипт — и в сессию фрейма");
        // `n`-й опрос фрейма.
        let nth_poll = |n: usize| {
            let mock = &mock;
            tokio::time::timeout(Duration::from_secs(2), async move {
                loop {
                    match polls(mock).into_iter().filter(|p| p["sessionId"] == "S9").nth(n) {
                        Some(poll) => return poll,
                        None => tokio::time::sleep(Duration::from_millis(10)).await,
                    }
                }
            })
        };
        let reply = |poll: &Value, batch: Value| json!({ "id": poll["id"], "result": { "result": { "type": "object", "value": batch } } });
        let poll = nth_poll(0).await.expect("опрос фрейма в его сессии");
        let batch = json!([{ "t": "move", "x": 1, "y": 1 }, { "t": "down", "x": 5, "y": 6 }, { "t": "key", "kind": "down" }]);
        mock.events.send(reply(&poll, batch)).unwrap();
        let mut got = Vec::new();
        for _ in 0..2 {
            let captured = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.expect("пачка не пришла").unwrap();
            got.push((captured.frame_id, captured.event));
        }
        assert_eq!(got[0], ("F3".to_string(), json!({ "t": "down", "x": 319.0, "y": 16.0 })), "точка — в окне вкладки, движение фрейма не шлётся");
        assert_eq!(got[1].1["t"], "key");
        // Клик убрал фрейм: сдвиг не узнать, перетаскивание и отпускание — с прежним.
        let poll = nth_poll(1).await.expect("второй опрос фрейма");
        mock.events.send(reply(&poll, json!([{ "t": "move", "x": 7, "y": 6, "buttons": 1 }, { "t": "up", "x": 5, "y": 6 }]))).unwrap();
        let drag = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.expect("перетаскивание пропало").unwrap();
        assert_eq!(drag.event, json!({ "t": "move", "x": 321.0, "y": 16.0, "buttons": 1 }), "перетаскивание во фрейме идёт");
        let up = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.expect("отпускание пропало").unwrap();
        assert_eq!(up.event, json!({ "t": "up", "x": 319.0, "y": 16.0 }));
        assert_eq!(owners.load(SeqCst), 2);
    }

    #[tokio::test]
    async fn cross_site_frame_mapped_after_start_still_gets_the_script() {
        let mock = Mock::spawn(|method, params, session| {
            base(method).unwrap_or_else(|| match (method, session) {
                ("Page.getFrameTree", "S1") => json!({ "frameTree": { "frame": { "id": "T1", "loaderId": "L1" } } }),
                ("Page.getFrameTree", "S9") => json!({ "frameTree": { "frame": { "id": "F3", "loaderId": "L3" } } }),
                ("Page.createIsolatedWorld", _) => json!({ "executionContextId": 1 }),
                ("Runtime.evaluate", _) if params["expression"] == NEXT => json!({ "silent": true }),
                ("Runtime.evaluate", _) => json!({ "result": { "type": "undefined" } }),
                ("Page.addScriptToEvaluateOnNewDocument", _) => json!({ "identifier": session }),
                _ => json!({ "error": format!("unexpected {method} in {session}") }),
            })
        })
        .await;
        let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
        let (_capture, _rx) = Capture::start(&page, CaptureFlags { mouse: false }).await.unwrap();
        // Событие о фрейме ушло до подписки перехвата, а `watch_frames` внёс его в карту уже после старта.
        page.frames.lock().unwrap().insert("F3".into(), FrameSession { session: "S9".into(), parent: "S1".into() });
        tokio::time::timeout(Duration::from_secs(3), async {
            while !polls(&mock).iter().any(|p| p["sessionId"] == "S9") {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("фрейм из карты не опрашивается");
        assert_eq!(mock.sessions("Page.addScriptToEvaluateOnNewDocument"), ["S1", "S9"], "скрипт — один раз на сессию");
    }

    #[tokio::test]
    async fn signature_descends_into_a_cross_site_frame() {
        let (mock, page, _b) = follower(|method, params| match method {
            "Page.createIsolatedWorld" => json!({ "executionContextId": 9 }),
            // Под точкой во вкладке — `<iframe>`, в его документе — кнопка.
            "Runtime.evaluate" if params["expression"].as_str().unwrap().contains("elementFromPoint(20, 30)") => {
                json!({ "result": { "type": "object", "value": { "tag": "BUTTON", "id": "ib" } } })
            }
            "Runtime.evaluate" if params["returnByValue"] == true => json!({ "result": { "type": "object", "value": { "tag": "IFRAME", "id": "cross" } } }),
            "Runtime.evaluate" => json!({ "result": { "type": "object", "objectId": "O1" } }),
            "DOM.describeNode" => json!({ "node": { "backendNodeId": 10, "frameId": "F3" } }),
            "DOM.getBoxModel" => json!({ "model": { "content": [100, 50, 400, 50, 400, 200, 100, 200] } }),
            _ => json!({}),
        })
        .await;
        with_cross_frame(&mock, &page).await;
        let sig = page.signature_at(120.0, 80.0).await.unwrap().unwrap();
        assert_eq!((sig.tag.as_str(), sig.id.as_str()), ("BUTTON", "ib"));
        assert_eq!(mock.sessions("Runtime.evaluate"), ["S1", "S1", "S9"], "кнопка — в сессии фрейма");
        assert_eq!(mock.sent("Page.createIsolatedWorld")[2], json!({ "frameId": "F3", "worldName": "ombra-sync" }));
        assert_eq!(mock.sent("Runtime.releaseObject"), [json!({ "objectId": "O1" })]);
    }

    /// Вкладка на моке: ответы `answer` поверх базовых.
    async fn follower(answer: impl Fn(&str, &Value) -> Value + Send + 'static) -> (Mock, Page, Browser) {
        let mock = Mock::spawn(move |method, params, _| base(method).unwrap_or_else(|| answer(method, params))).await;
        let browser = Browser::connect(&mock.url).await.unwrap();
        let page = browser.attach("T1").await.unwrap();
        (mock, page, browser)
    }

    fn key(down: bool, key: &str, mods: i64) -> KeyEvent {
        KeyEvent { down, key: key.into(), code: "KeyA".into(), key_code: 65, mods, location: 0, sig: None }
    }

    #[tokio::test]
    async fn key_sends_text_or_commands() {
        let (mock, page, _b) = follower(|_, _| json!({})).await;
        page.key(&key(true, "ж", 0)).await.unwrap();
        page.key(&key(true, "a", 4)).await.unwrap();
        page.key(&key(false, "a", 4)).await.unwrap();
        let sent = mock.sent("Input.dispatchKeyEvent");
        assert_eq!((&sent[0]["type"], &sent[0]["text"]), (&json!("keyDown"), &json!("ж")));
        assert_eq!((&sent[1]["type"], &sent[1]["commands"], &sent[1]["modifiers"]), (&json!("rawKeyDown"), &json!(["selectAll"]), &json!(4)));
        assert!(sent[1].get("text").is_none(), "⌘A ничего не печатает");
        assert_eq!(sent[2]["type"], "keyUp");
    }

    #[tokio::test]
    async fn enter_prints_newline_unless_command_held() {
        let (mock, page, _b) = follower(|_, _| json!({})).await;
        page.key(&key(true, "Enter", 0)).await.unwrap();
        page.key(&key(true, "Enter", 4)).await.unwrap();
        let sent = mock.sent("Input.dispatchKeyEvent");
        assert_eq!((&sent[0]["type"], &sent[0]["text"]), (&json!("keyDown"), &json!("\r")));
        assert_eq!(sent[1]["type"], "rawKeyDown");
        assert!(sent[1].get("text").is_none(), "⌘Enter ничего не печатает");
    }

    #[tokio::test]
    async fn mouse_and_wheel_params() {
        let (mock, page, _b) = follower(|_, _| json!({})).await;
        page.mouse("mousePressed", 5.0, 6.0, Some(Button::Right), 2, 2, 8).await.unwrap();
        page.mouse("mouseMoved", 7.0, 8.0, None, 0, 0, 0).await.unwrap();
        page.wheel_by(1.0, 2.0, 3.0, 4.0, 2).await.unwrap();
        let sent = mock.sent("Input.dispatchMouseEvent");
        assert_eq!(
            (&sent[0]["button"], &sent[0]["buttons"], &sent[0]["clickCount"], &sent[0]["modifiers"]),
            (&json!("right"), &json!(2), &json!(2), &json!(8))
        );
        assert_eq!(sent[1]["button"], "none");
        assert_eq!((&sent[2]["deltaX"], &sent[2]["deltaY"], &sent[2]["modifiers"]), (&json!(3.0), &json!(4.0), &json!(2)));
    }

    #[tokio::test]
    async fn insert_chunk_sends_insert_text() {
        let (mock, page, _b) = follower(|_, _| json!({})).await;
        page.insert_chunk("привет").await.unwrap();
        assert_eq!(mock.sent("Input.insertText"), [json!({ "text": "привет" })]);
        assert_eq!(mock.sessions("Input.insertText"), ["S1"], "во вкладку");
    }

    #[tokio::test]
    async fn signature_is_read_in_sync_world() {
        let (mock, page, _b) = follower(|method, _| match method {
            "Page.createIsolatedWorld" => json!({ "executionContextId": 9 }),
            "Runtime.evaluate" => json!({ "result": { "type": "object", "value": { "tag": "A", "role": "", "type": "button", "id": "a", "name": "", "text": "Купить" } } }),
            _ => json!({}),
        })
        .await;
        let sig = page.signature_at(3.0, 4.0).await.unwrap().unwrap();
        assert_eq!((sig.tag.as_str(), sig.kind.as_str(), sig.id.as_str(), sig.text.as_str()), ("A", "button", "a", "Купить"));
        assert_eq!(mock.sent("Page.createIsolatedWorld")[0]["worldName"], "ombra-sync");
        let eval = &mock.sent("Runtime.evaluate")[0];
        assert_eq!(eval["contextId"], 9);
        assert!(eval["expression"].as_str().unwrap().contains("elementFromPoint(3, 4)"));
    }

    #[tokio::test]
    async fn humanize_toggle() {
        let (mock, _page, browser) = follower(|method, _| match method {
            "Ombra.getHumanizeState" => json!({ "state": { "enabled": true } }),
            _ => json!({}),
        })
        .await;
        assert_eq!(browser.humanize_enabled().await, Some(true));
        browser.set_humanize(false).await.unwrap();
        assert_eq!(mock.sent("Ombra.setHumanize"), [json!({ "enabled": false })]);
        assert_eq!(mock.sessions("Ombra.setHumanize"), [""], "Ombra.setHumanize — на браузер, без сессии");
        let (_mock, _page, off) = follower(|_, _| json!({ "error": "module off" })).await;
        assert_eq!(off.humanize_enabled().await, None);
    }

    #[tokio::test]
    async fn history_and_go() {
        let (mock, page, _b) = follower(|method, _| match method {
            "Page.getNavigationHistory" => json!({ "currentIndex": 1, "entries": [
                { "id": 7, "url": "http://a/", "transitionType": "typed" },
                { "id": 9, "url": "http://b/", "userTypedURL": "http://typed/", "transitionType": "link" },
            ] }),
            _ => json!({}),
        })
        .await;
        let history = page.history().await.unwrap();
        assert_eq!((history.index, history.ids, history.transitions, history.urls), (1, vec![7, 9], vec!["typed".to_string(), "link".into()], vec!["http://a/".to_string(), "http://b/".into()]));
        assert_eq!(history.typed, ["", "http://typed/"]);
        let wait = Duration::from_secs(2);
        assert!(!page.go_history(1, wait).await.unwrap(), "вперёд записи нет");
        assert!(mock.sent("Page.navigateToHistoryEntry").is_empty());
        // Шаг кончается переходом главного фрейма (здесь — внутри документа).
        let committed = async {
            while mock.sent("Page.navigateToHistoryEntry").is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            mock.events.send(json!({ "method": "Page.navigatedWithinDocument", "sessionId": "S1", "params": { "frameId": "T1" } })).unwrap();
        };
        let (went, _) = tokio::join!(page.go_history(-1, wait), committed);
        assert!(went.unwrap());
        assert_eq!(mock.sent("Page.navigateToHistoryEntry"), [json!({ "entryId": 7 })]);
        let (mock, page, _b) = follower(|_, _| json!({ "currentIndex": 0, "entries": [{ "id": 7 }] })).await;
        assert!(!page.go_history(-1, wait).await.unwrap(), "перед первой записью — нет");
        assert!(mock.sent("Page.navigateToHistoryEntry").is_empty());
    }

    #[tokio::test]
    async fn select_types_label_and_checks_selection() {
        let (mock, page, _b) = follower(|method, params| match method {
            "Page.createIsolatedWorld" => json!({ "executionContextId": 9 }),
            // Под точкой (1, 2) — список, под (5, 5) — поле.
            "Runtime.evaluate" if params["returnByValue"] == true => {
                let tag = if params["expression"].as_str().unwrap().contains("elementFromPoint(1, 2)") { "SELECT" } else { "INPUT" };
                json!({ "result": { "type": "object", "value": { "tag": tag } } })
            }
            "Runtime.evaluate" => json!({ "result": { "type": "object", "objectId": "O1" } }),
            "Runtime.callFunctionOn" => json!({ "result": { "type": "string", "value": "Бета" } }),
            _ => json!({}),
        })
        .await;
        assert!(page.select_by_label(1.0, 2.0, "Бета").await.unwrap());
        assert!(!page.select_by_label(1.0, 2.0, "Альфа").await.unwrap(), "выбран другой вариант");
        assert_eq!(mock.sent("DOM.focus")[0], json!({ "objectId": "O1" }));
        assert_eq!(mock.sent("Input.dispatchKeyEvent").len(), 4 * 2 + 5 * 2, "каждый символ — нажатие и отпускание");
        assert!(!page.select_by_label(5.0, 5.0, "Бета").await.unwrap(), "под точкой не список");
        assert_eq!((mock.sent("DOM.focus").len(), mock.sent("Input.dispatchKeyEvent").len()), (2, 18), "в чужое поле ничего не набрано");
        assert!(mock.sent("Runtime.evaluate").iter().all(|e| !e["expression"].as_str().unwrap().contains("dispatchEvent")));
    }

    #[tokio::test]
    async fn dialog_and_file_chooser_calls() {
        let (mock, page, _b) = follower(|_, _| json!({})).await;
        page.answer_dialog(true, "да").await.unwrap();
        page.intercept_files().await.unwrap();
        assert_eq!(mock.sent("Page.handleJavaScriptDialog"), [json!({ "accept": true, "promptText": "да" })]);
        assert_eq!(mock.sent("Page.setInterceptFileChooserDialog"), [json!({ "enabled": true })]);
        let (session, _events) = page.events();
        assert_eq!(session, "S1");
    }

    #[tokio::test]
    async fn file_chooser_is_intercepted_in_frame_sessions_too() {
        let (mock, page, _b) = follower(|_, _| json!({})).await;
        with_cross_frame(&mock, &page).await;
        page.intercept_files().await.unwrap();
        assert_eq!(mock.sessions("Page.setInterceptFileChooserDialog"), ["S1", "S9"], "вкладка и фрейм, уже бывший в карте");
        // Фрейм, подключившийся позже, — тоже.
        let attached = json!({ "method": "Target.attachedToTarget", "sessionId": "S9", "params": { "sessionId": "S10", "targetInfo": { "targetId": "F4", "type": "iframe" } } });
        mock.events.send(attached).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while mock.sessions("Page.setInterceptFileChooserDialog").len() < 3 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("новый фрейм не перехватывает выбор файла");
        assert_eq!(mock.sessions("Page.setInterceptFileChooserDialog")[2], "S10");
        assert!(page.has_session("S1") && page.has_session("S9") && page.has_session("S10") && !page.has_session("S7"));
    }

    #[tokio::test]
    async fn tab_calls() {
        let (mock, _page, browser) = follower(|_, _| json!({})).await;
        browser.activate("T9").await.unwrap();
        browser.discover_tabs().await.unwrap();
        assert_eq!(mock.sent("Target.activateTarget"), [json!({ "targetId": "T9" })]);
        assert_eq!(mock.sent("Target.setDiscoverTargets"), [json!({ "discover": true })]);
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use crate::engine::page::Button;
    use crate::flow::live_tests::live;
    use crate::supervisor::StartOptions;
    use crate::supervisor::live_tests::KillProfileOnPanic;
    use tokio::sync::mpsc::Receiver;

    const LOAD: Duration = Duration::from_secs(30);

    /// Первое событие перехвата, подходящее под `what`; все пришедшие до него — в `seen`.
    async fn next(rx: &mut Receiver<Captured>, seen: &mut Vec<Value>, what: impl Fn(&Value) -> bool) -> Captured {
        let wait = async {
            loop {
                let captured = rx.recv().await.expect("поток перехвата кончился");
                seen.push(captured.event.clone());
                if what(&captured.event) {
                    return captured;
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), wait).await.expect("событие перехвата не пришло")
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "запускает настоящий браузер; cargo test --lib engine::sync::live -- --ignored --nocapture --test-threads=1"]
    async fn live_capture_reports_trusted_input_and_page_world_is_clean() {
        let live = live().await;
        let _cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.quiet));
        let options = StartOptions { headless: std::env::var_os("OMBRA_PROBE_HEADFUL").is_none(), ..Default::default() };
        let info = live.core.supervisor.start_with(&live.quiet, options).await.unwrap();
        let browser = Browser::connect(&info.cdp_ws).await.unwrap();
        let tab = browser.tabs().await.unwrap().remove(0).target_id;
        let page = browser.attach(&tab).await.unwrap();
        page.goto("http://sync.ombra.test/sync", LOAD).await.unwrap();
        let (capture, mut rx) = Capture::start(&page, CaptureFlags { mouse: false }).await.unwrap();
        // «Пользователь» — своя сессия той же вкладки.
        let user = browser.attach(&tab).await.unwrap();
        let mut seen = Vec::new();

        user.click(80.0, 40.0, Button::Left, 1, Duration::ZERO).await.unwrap();
        for t in ["down", "up"] {
            let captured = next(&mut rx, &mut seen, |e| e["t"] == t).await;
            assert_eq!(captured.frame_id, tab, "главный фрейм — id вкладки");
            let e = captured.event;
            assert_eq!((&e["sig"]["id"], &e["sig"]["tag"], &e["detail"]), (&json!("a"), &json!("BUTTON"), &json!(1)), "{t}: {e}");
        }
        user.click(80.0, 95.0, Button::Left, 1, Duration::ZERO).await.unwrap();
        let mut typed = Vec::new();
        for ch in ['а', 'б'] {
            user.type_char(ch).await.unwrap();
            typed.push(next(&mut rx, &mut seen, |e| e["t"] == "key" && e["kind"] == "down" && e["key"] == ch.to_string()).await.event);
        }
        assert_eq!((&typed[1]["sig"]["id"], &typed[1]["sig"]["text"]), (&json!("q"), &json!("а")), "текст поля — его значение");
        // Пустое место страницы: у `<html>` и `<body>` текста нет — всю страницу на каждое нажатие не читаем.
        user.click(400.0, 400.0, Button::Left, 1, Duration::ZERO).await.unwrap();
        let empty = next(&mut rx, &mut seen, |e| e["t"] == "down").await.event;
        assert!(["HTML", "BODY"].contains(&empty["sig"]["tag"].as_str().unwrap_or_default()) && empty["sig"]["text"] == "", "{empty}");

        // Переход: новый документ получает скрипт сам, опрос продолжается в его мире.
        page.goto("http://sync.ombra.test/sync-price", LOAD).await.unwrap();
        user.click(80.0, 40.0, Button::Left, 1, Duration::ZERO).await.unwrap();
        let down = next(&mut rx, &mut seen, |e| e["t"] == "down").await.event;
        assert_eq!((&down["sig"]["id"], &down["sig"]["text"]), (&json!("a"), &json!("Купить за 90")), "{down}");
        assert!(!seen.iter().any(|e| e["t"] == "move"), "без mouse движения не шлются: {seen:?}");

        assert_eq!(page.eval_main("typeof __ombraSyncNext", &[]).await.unwrap(), json!("undefined"), "мир страницы чист");
        let log = page.eval_main("document.documentElement.dataset.log", &[]).await.unwrap();
        assert!(log.as_str().unwrap_or_default().contains("click:a:true"), "клик страницы доверенный: {log}");
        capture.stop().await;
        live.core.supervisor.stop(&live.quiet).await.unwrap();
    }
}
