//! Подставной браузер для тестов: CDP по WebSocket на 127.0.0.1, «банка» cookies в форме CDP и список вкладок.
//! Отвечает на `Storage.getCookies`, `Storage.setCookies`, `Target.getTargets`, `Target.createTarget`, `Target.closeTarget`
//! так же, как настоящий: `{"id":N,"result":{…}}` или `{"id":N,"error":{"code":…,"message":…}}`.
use super::cdp::{Bounds, from_cdp, to_cdp};
use crate::cookies::Cookie;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

/// Дескриптор подставного браузера; клоны смотрят в одно состояние.
#[derive(Clone, Default)]
pub(crate) struct FakeCdp {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    /// Cookies в том виде, в каком их отдаёт `Storage.getCookies`; переживают перезапуск, как папка профиля.
    cookies: Vec<Value>,
    calls: Vec<String>,
    opened: Vec<String>,
    /// Открытые страницы: (targetId, url).
    pages: Vec<(String, String)>,
    /// Пользователь закрыл все окна (macOS): браузер жив, но `Storage.getCookies` отвечает ошибкой.
    windowless: bool,
    rejected: HashSet<String>,
    /// Задержка ответов на `Storage.getCookies`: каждый ждёт разрешения; закрытый семафор — задержка снята.
    cookies_gate: Option<Arc<Semaphore>>,
    next_target: u64,
    /// Последние границы окна от `Browser.setWindowBounds`.
    bounds: Option<Bounds>,
    /// Состояние окна: `normal`, `maximized`, `minimized`, `fullscreen`.
    window_state: Option<String>,
    addr: Option<SocketAddr>,
    stop: Option<CancellationToken>,
}

impl FakeCdp {
    /// Браузер ещё не запущен: тест может настроить его до запуска профиля.
    pub fn new() -> FakeCdp {
        FakeCdp::default()
    }

    pub async fn spawn() -> FakeCdp {
        let fake = FakeCdp::new();
        fake.start().await;
        fake
    }

    /// Запуск браузера: слушатель на новом порту. Уже запущенный не трогается.
    pub async fn start(&self) {
        if self.lock().stop.is_some() {
            return;
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stop = CancellationToken::new();
        {
            let mut state = self.lock();
            state.addr = Some(listener.local_addr().unwrap());
            state.stop = Some(stop.clone());
        }
        tokio::spawn(accept_loop(listener, self.state.clone(), stop));
    }

    pub fn ws_url(&self) -> String {
        let addr = self.lock().addr.expect("подставной CDP ни разу не запускался");
        format!("ws://{addr}/devtools/browser/fake")
    }

    pub fn browser_cookies(&self) -> Vec<Cookie> {
        self.lock().cookies.iter().filter_map(from_cdp).collect()
    }

    pub fn set_browser_cookies(&self, cookies: Vec<Cookie>) {
        self.lock().cookies = cookies.iter().filter_map(|cookie| stored_form(&to_cdp(cookie))).collect();
    }

    /// Cookie в сыром виде CDP — например, партиционированная (с `partitionKey`).
    pub fn add_raw_cookie(&self, cookie: Value) {
        self.lock().cookies.push(cookie);
    }

    /// Методы CDP по порядку вызова, включая отвергнутые.
    pub fn calls(&self) -> Vec<String> {
        self.lock().calls.clone()
    }

    /// Адреса вкладок, открытых `Target.createTarget`.
    pub fn opened(&self) -> Vec<String> {
        self.lock().opened.clone()
    }

    /// Страница, которую браузер открыл сам (новая вкладка, восстановленная сессия).
    pub fn add_page(&self, url: &str) {
        let mut state = self.lock();
        let id = state.new_target_id();
        state.pages.push((id, url.to_string()));
        state.windowless = false;
    }

    /// Окно браузера осталось без вкладок (ждёт ответа об отмене загрузки, или движок без патча 245 на macOS): страниц нет, процесс жив.
    pub fn close_windows(&self) {
        let mut state = self.lock();
        state.pages.clear();
        state.windowless = true;
    }

    pub fn pages(&self) -> Vec<String> {
        self.lock().pages.iter().map(|(_, url)| url.clone()).collect()
    }

    pub fn set_window_state(&self, state: &str) {
        self.lock().window_state = Some(state.to_string());
    }

    pub fn window_state(&self) -> String {
        self.lock().window_state.clone().unwrap_or_else(|| "normal".into())
    }

    pub fn bounds(&self) -> Option<Bounds> {
        self.lock().bounds
    }

    /// `Storage.setCookies` с cookie этого имени отвергается целиком — и пакетом, и по одной.
    pub fn reject_cookie(&self, name: &str) {
        self.lock().rejected.insert(name.to_string());
    }

    /// Ответы на `Storage.getCookies` задерживаются: набор берётся в момент запроса (как у браузера, ответившего
    /// раньше чужого `Storage.setCookies`), а уходит по `release_get_cookies`.
    pub fn hold_get_cookies(&self) {
        self.lock().cookies_gate = Some(Arc::new(Semaphore::new(0)));
    }

    /// Отпустить один задержанный ответ на `Storage.getCookies` (или следующий, если запроса ещё нет).
    pub fn release_get_cookies(&self) {
        if let Some(gate) = &self.lock().cookies_gate {
            gate.add_permits(1);
        }
    }

    /// Снять задержку: задержанные и следующие ответы уходят сразу.
    pub fn stop_holding_get_cookies(&self) {
        if let Some(gate) = self.lock().cookies_gate.take() {
            gate.close();
        }
    }

    /// Браузер закрылся: слушатель и соединения закрыты, страниц нет, cookies остались.
    pub fn go_down(&self) {
        let mut state = self.lock();
        if let Some(stop) = state.stop.take() {
            stop.cancel();
        }
        state.pages.clear();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }
}

impl State {
    fn new_target_id(&mut self) -> String {
        self.next_target += 1;
        format!("TARGET{}", self.next_target)
    }

    fn answer(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "Storage.getCookies" if self.windowless => Err((-32000, "Browser context management is not supported.".to_string())),
            "Storage.getCookies" => Ok(json!({ "cookies": self.cookies })),
            "Storage.setCookies" => self.set_cookies(params),
            "Target.getTargets" => {
                let infos: Vec<Value> = self
                    .pages
                    .iter()
                    .map(|(id, url)| {
                        json!({ "targetId": id, "type": "page", "title": "", "url": url, "attached": false, "canAccessOpener": false })
                    })
                    .collect();
                Ok(json!({ "targetInfos": infos }))
            }
            "Target.createTarget" => {
                let url = params["url"].as_str().ok_or((-32602, "Invalid parameters: url".to_string()))?.to_string();
                let id = self.new_target_id();
                self.opened.push(url.clone());
                self.pages.push((id.clone(), url));
                Ok(json!({ "targetId": id }))
            }
            "Browser.getWindowForTarget" => match params["targetId"].as_str() {
                Some(id) if self.pages.iter().any(|(t, _)| t == id) => Ok(json!({ "windowId": 1 })),
                _ => Err((-32602, "No target with given id found".to_string())),
            },
            "Browser.getWindowBounds" => {
                let state = self.window_state.clone().unwrap_or_else(|| "normal".into());
                let mut bounds = json!({ "windowState": state });
                if let Some(b) = self.bounds {
                    (bounds["left"], bounds["top"], bounds["width"], bounds["height"]) = (b.left.into(), b.top.into(), b.width.into(), b.height.into());
                }
                Ok(json!({ "bounds": bounds }))
            }
            // Как Chromium: `windowState` меняет только состояние, размеры применяются лишь к окну в `normal`.
            "Browser.setWindowBounds" => {
                let b = &params["bounds"];
                let invalid = || (-32602, "Invalid parameters: bounds".to_string());
                if params["windowId"] != 1 {
                    return Err(invalid());
                }
                if let Some(state) = b["windowState"].as_str() {
                    self.window_state = Some(state.to_string());
                } else if self.window_state.as_deref().unwrap_or("normal") == "normal" {
                    self.bounds = Some(Bounds {
                        left: b["left"].as_i64().ok_or_else(invalid)? as i32,
                        top: b["top"].as_i64().ok_or_else(invalid)? as i32,
                        width: b["width"].as_u64().ok_or_else(invalid)? as u32,
                        height: b["height"].as_u64().ok_or_else(invalid)? as u32,
                    });
                }
                Ok(json!({}))
            }
            "Target.closeTarget" => {
                let id = params["targetId"].as_str().unwrap_or_default();
                let before = self.pages.len();
                self.pages.retain(|(target, _)| target.as_str() != id);
                if self.pages.len() == before {
                    return Err((-32602, "No target with given id found".to_string()));
                }
                Ok(json!({ "success": true }))
            }
            other => Err((-32601, format!("'{other}' wasn't found"))),
        }
    }

    /// Как у браузера: одна неверная cookie отвергает весь вызов, ничего не поставив.
    fn set_cookies(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let invalid = || (-32602, "Invalid cookie fields".to_string());
        let list = params["cookies"].as_array().ok_or_else(invalid)?;
        let mut stored = Vec::with_capacity(list.len());
        for param in list {
            if param["name"].as_str().is_some_and(|name| self.rejected.contains(name)) {
                return Err(invalid());
            }
            stored.push(stored_form(param).ok_or_else(invalid)?);
        }
        for cookie in stored {
            self.cookies.retain(|c| !same_key(c, &cookie));
            self.cookies.push(cookie);
        }
        Ok(json!({}))
    }
}

fn same_key(a: &Value, b: &Value) -> bool {
    a["domain"] == b["domain"] && a["name"] == b["name"] && a["path"] == b["path"]
}

/// Как браузер хранит `CookieParam`: host-only — домен из `url` без точки; доменная — с точкой; без срока — сессионная.
fn stored_form(param: &Value) -> Option<Value> {
    let domain = match param["domain"].as_str() {
        Some(domain) if domain.starts_with('.') => domain.to_string(),
        Some(domain) => format!(".{domain}"),
        None => {
            let rest = param["url"].as_str()?.split_once("://")?.1;
            rest.split(['/', '?']).next()?.split(':').next()?.to_string()
        }
    };
    let expires = param["expires"].as_f64();
    let mut cookie = json!({
        "name": param["name"].as_str()?,
        "value": param["value"].as_str().unwrap_or_default(),
        "domain": domain,
        "path": param["path"].as_str().unwrap_or("/"),
        "expires": expires.unwrap_or(-1.0),
        "size": 0,
        "httpOnly": param["httpOnly"].as_bool().unwrap_or(false),
        "secure": param["secure"].as_bool().unwrap_or(false),
        "session": expires.is_none(),
        "priority": "Medium",
        "sameParty": false,
        "sourceScheme": "Secure",
        "sourcePort": 443,
    });
    if let Some(same_site) = param["sameSite"].as_str() {
        cookie["sameSite"] = json!(same_site);
    }
    Some(cookie)
}

async fn accept_loop(listener: TcpListener, state: Arc<Mutex<State>>, stop: CancellationToken) {
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => return,
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted {
                    tokio::spawn(serve(stream, state.clone(), stop.clone()));
                }
            }
        }
    }
}

async fn serve(stream: TcpStream, state: Arc<Mutex<State>>, stop: CancellationToken) {
    let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else { return };
    loop {
        let message = tokio::select! {
            biased;
            _ = stop.cancelled() => return,
            message = socket.next() => message,
        };
        let text = match message {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(_)) => continue,
            _ => return,
        };
        let Ok(request) = serde_json::from_str::<Value>(text.as_str()) else { return };
        let method = request["method"].as_str().unwrap_or_default().to_string();
        let (result, gate) = {
            let mut state = state.lock().unwrap();
            state.calls.push(method.clone());
            let gate = if method == "Storage.getCookies" { state.cookies_gate.clone() } else { None };
            (state.answer(&method, &request["params"]), gate)
        };
        if let Some(gate) = gate {
            tokio::select! {
                biased;
                _ = stop.cancelled() => return,
                // Ошибка — семафор закрыт: задержка снята.
                permit = gate.acquire() => {
                    if let Ok(permit) = permit {
                        permit.forget();
                    }
                }
            }
        }
        let reply = match result {
            Ok(result) => json!({ "id": request["id"], "result": result }),
            Err((code, message)) => json!({ "id": request["id"], "error": { "code": code, "message": message } }),
        };
        if socket.send(Message::text(reply.to_string())).await.is_err() {
            return;
        }
    }
}
