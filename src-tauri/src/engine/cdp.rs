//! Клиент CDP: WebSocket к отладочному порту браузера на 127.0.0.1.
//! Названия методов и формат протокола знает только `engine` (спецификация 2А, «Cookies»).
use crate::cookies::{Cookie, SameSite};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

/// Сколько ждать подключения и ответа на один вызов.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);

const WINDOW_STATE_POLL: Duration = Duration::from_millis(100);
const WINDOW_STATE_WAIT: Duration = Duration::from_secs(2);

/// Страницы, которые браузер открывает сам, когда открывать нечего: их закрывают стартовые вкладки.
const BLANK_PAGES: &[&str] = &["", "about:blank", "chrome://newtab/", "chrome://new-tab-page/"];

#[derive(Debug, thiserror::Error)]
pub enum CdpError {
    #[error("Управление браузером доступно только на этом компьютере")]
    NotLocal,
    #[error("Не удалось подключиться к браузеру: {0}")]
    Connect(String),
    #[error("Истекло время ожидания ответа от браузера")]
    Timeout,
    #[error("Соединение с браузером потеряно")]
    Closed,
    #[error("Не удалось прочитать ответ браузера: {0}")]
    Protocol(String),
    #[error("Браузер сообщил об ошибке: {0}")]
    Remote(String),
    /// Свой текст о странице или элементе («элемент больше не на странице» и т.п.) — пользователю как есть.
    #[error("{0}")]
    Page(String),
    /// Исключение скрипта на странице — его текстом; мир процесса из-за него не пересоздаётся.
    #[error("{0}")]
    Script(String),
}

/// Сколько cookies браузер принял и сколько отверг при постановке по одной.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SetCookiesReport {
    pub set: usize,
    pub rejected: usize,
}

/// Положение и размер окна браузера в логических пикселях.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

pub struct CdpClient {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl CdpClient {
    /// Только `ws://127.0.0.1:<порт>/…`: отладочный порт браузера не должен уводить за пределы машины.
    pub async fn connect(ws_url: &str) -> Result<Self, CdpError> {
        if !is_local(ws_url) {
            log::warn!("Ожидался локальный адрес CDP на 127.0.0.1");
            return Err(CdpError::NotLocal);
        }
        let (socket, _) = tokio::time::timeout(CALL_TIMEOUT, connect_async(ws_url))
            .await
            .map_err(|_| CdpError::Timeout)?
            .map_err(|e| CdpError::Connect(e.to_string()))?;
        Ok(Self { socket, next_id: 0 })
    }

    /// `Storage.getCookies`: весь набор браузера, нормализованный; партиционированные (CHIPS) не берутся.
    pub async fn get_cookies(&mut self) -> Result<Vec<Cookie>, CdpError> {
        let result = self.call("Storage.getCookies", json!({})).await?;
        let list = result["cookies"].as_array().ok_or_else(|| CdpError::Protocol("нет списка cookies".into()))?;
        // Один ключ (домен, имя, путь) — одна cookie: браузер может держать копии для разных схем и портов.
        let mut unique = BTreeMap::new();
        for cookie in list.iter().filter_map(from_cdp) {
            unique.insert((cookie.domain.clone(), cookie.name.clone(), cookie.path.clone()), cookie);
        }
        Ok(unique.into_values().collect())
    }

    /// `Storage.setCookies` одним пакетом (и для пустого набора). Если браузер отверг пакет — по одной:
    /// одна испорченная cookie не лишает профиль остальных. `Err` — только сбой связи с браузером.
    pub async fn set_cookies(&mut self, cookies: &[Cookie]) -> Result<SetCookiesReport, CdpError> {
        let params: Vec<Value> = cookies.iter().map(to_cdp).collect();
        match self.call("Storage.setCookies", json!({ "cookies": params })).await {
            Ok(_) => return Ok(SetCookiesReport { set: cookies.len(), rejected: 0 }),
            Err(CdpError::Remote(_)) => {}
            Err(other) => return Err(other),
        }
        let mut report = SetCookiesReport::default();
        for param in params {
            match self.call("Storage.setCookies", json!({ "cookies": [param] })).await {
                Ok(_) => report.set += 1,
                Err(CdpError::Remote(_)) => report.rejected += 1,
                Err(other) => return Err(other),
            }
        }
        Ok(report)
    }

    /// Открывает стартовые вкладки и закрывает пустые страницы, которые браузер открыл сам.
    /// Вкладки прошлой сессии остаются. Пустой список ничего не делает.
    pub async fn open_start_tabs(&mut self, urls: &[String]) -> Result<(), CdpError> {
        if urls.is_empty() {
            return Ok(());
        }
        let targets = self.call("Target.getTargets", json!({})).await?;
        let blank: Vec<String> = targets["targetInfos"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter(|t| t["type"] == "page" && t["url"].as_str().is_some_and(is_blank))
                    .filter_map(|t| t["targetId"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for url in urls {
            self.call("Target.createTarget", json!({ "url": url })).await?;
        }
        for target_id in blank {
            self.call("Target.closeTarget", json!({ "targetId": target_id })).await?;
        }
        Ok(())
    }

    /// Есть ли у браузера страницы (вкладки). У браузера с окнами их нет, когда пользователь закрыл все окна.
    pub async fn has_pages(&mut self) -> Result<bool, CdpError> {
        let targets = self.call("Target.getTargets", json!({})).await?;
        Ok(targets["targetInfos"].as_array().is_some_and(|list| list.iter().any(|t| t["type"] == "page")))
    }

    /// Окно, в котором открыта первая страница: `Target.getTargets` → `Browser.getWindowForTarget`.
    pub async fn window_for_first_page(&mut self) -> Result<i64, CdpError> {
        let targets = self.call("Target.getTargets", json!({})).await?;
        let page = targets["targetInfos"]
            .as_array()
            .and_then(|list| list.iter().find(|t| t["type"] == "page"))
            .and_then(|t| t["targetId"].as_str())
            .ok_or_else(|| CdpError::Protocol("нет страницы".into()))?
            .to_string();
        let window = self.call("Browser.getWindowForTarget", json!({ "targetId": page })).await?;
        window["windowId"].as_i64().ok_or_else(|| CdpError::Protocol({
            log::warn!("нет windowId");
            "Не удалось получить данные окна"
        }.into()))
    }

    /// Приводит окно в обычное состояние и ставит размеры. Chromium применяет `bounds` только к окну в `normal`:
    /// у развёрнутого, свёрнутого и полноэкранного они молча игнорируются, поэтому состояние меняется отдельным вызовом
    /// и ждётся (выход из полноэкранного на macOS анимируется).
    pub async fn set_window_bounds(&mut self, window_id: i64, bounds: &Bounds) -> Result<(), CdpError> {
        if self.window_state(window_id).await? != "normal" {
            self.call("Browser.setWindowBounds", json!({ "windowId": window_id, "bounds": { "windowState": "normal" } }))
                .await?;
            let mut waited = Duration::ZERO;
            while self.window_state(window_id).await? != "normal" {
                if waited >= WINDOW_STATE_WAIT {
                    return Err(CdpError::Protocol("Не удалось восстановить обычный размер окна".into()));
                }
                tokio::time::sleep(WINDOW_STATE_POLL).await;
                waited += WINDOW_STATE_POLL;
            }
        }
        let b = serde_json::to_value(bounds).map_err(|e| CdpError::Protocol(e.to_string()))?;
        self.call("Browser.setWindowBounds", json!({ "windowId": window_id, "bounds": b })).await.map(|_| ())
    }

    /// Положение и размер окна.
    pub async fn window_bounds(&mut self, window_id: i64) -> Result<Bounds, CdpError> {
        let reply = self.call("Browser.getWindowBounds", json!({ "windowId": window_id })).await?;
        let b = &reply["bounds"];
        let side = |name: &str| b[name].as_i64().ok_or_else(|| CdpError::Protocol(format!("нет {name} окна")));
        Ok(Bounds { left: side("left")? as i32, top: side("top")? as i32, width: side("width")? as u32, height: side("height")? as u32 })
    }

    pub(crate) async fn window_state(&mut self, window_id: i64) -> Result<String, CdpError> {
        let reply = self.call("Browser.getWindowBounds", json!({ "windowId": window_id })).await?;
        Ok(reply["bounds"]["windowState"].as_str().unwrap_or("normal").to_string())
    }

    /// Адреса всех целей браузера (`Target.getTargets`): страницы и service worker'ы расширений.
    #[cfg(test)]
    pub(crate) async fn target_urls(&mut self) -> Result<Vec<String>, CdpError> {
        let result = self.call("Target.getTargets", serde_json::json!({})).await?;
        Ok(result["targetInfos"]
            .as_array()
            .map(|list| list.iter().filter_map(|t| t["url"].as_str().map(str::to_string)).collect())
            .unwrap_or_default())
    }

    /// `Runtime.evaluate` на первой странице; промис ожидается — для проб и живых тестов. Движок глушит только `Runtime.enable`.
    #[cfg(test)]
    pub(crate) async fn evaluate(&mut self, expression: &str) -> Result<Value, CdpError> {
        let targets = self.call("Target.getTargets", json!({})).await?;
        let page = targets["targetInfos"]
            .as_array()
            .and_then(|list| list.iter().find(|t| t["type"] == "page"))
            .and_then(|t| t["targetId"].as_str())
            .ok_or_else(|| CdpError::Protocol("нет страницы".into()))?
            .to_string();
        let attached = self.call("Target.attachToTarget", json!({ "targetId": page, "flatten": true })).await?;
        let session = attached["sessionId"].as_str().unwrap_or_default().to_string();
        self.call_in_session(Some(&session), "Runtime.evaluate", json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }))
            .await
            .map(|reply| reply["result"]["value"].clone())
    }

    /// Вызов произвольного метода браузера — для живых тестов (`Browser.grantPermissions`, `Browser.getWindowBounds`).
    #[cfg(test)]
    pub(crate) async fn send(&mut self, method: &str, params: Value) -> Result<Value, CdpError> {
        self.call(method, params).await
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, CdpError> {
        self.call_in_session(None, method, params).await
    }

    /// Вызов в сессии вложенной цели (`flatten`): `sessionId` в запросе, ответ сопоставляется по `id` как обычно.
    async fn call_in_session(&mut self, session: Option<&str>, method: &str, params: Value) -> Result<Value, CdpError> {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        let request = request.to_string();
        tokio::time::timeout(CALL_TIMEOUT, self.exchange(id, request)).await.map_err(|_| CdpError::Timeout)?
    }

    async fn exchange(&mut self, id: u64, request: String) -> Result<Value, CdpError> {
        self.socket.send(Message::text(request)).await.map_err(|_| CdpError::Closed)?;
        while let Some(message) = self.socket.next().await {
            let Message::Text(text) = message.map_err(|_| CdpError::Closed)? else { continue };
            let reply: Value = serde_json::from_str(text.as_str()).map_err(|e| CdpError::Protocol(e.to_string()))?;
            // События и ответы на вызовы, брошенные по таймауту, пропускаются.
            if reply["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                let message = error["message"].as_str().unwrap_or("без описания");
                return Err(CdpError::Remote(message.to_string()));
            }
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(CdpError::Closed)
    }
}

pub(crate) fn is_local(ws_url: &str) -> bool {
    let Some(rest) = ws_url.strip_prefix("ws://127.0.0.1:") else { return false };
    let port = rest.split('/').next().unwrap_or_default();
    port.parse::<u16>().is_ok_and(|port| port != 0)
}

fn is_blank(url: &str) -> bool {
    BLANK_PAGES.contains(&url)
}

/// Cookie из ответа `Storage.getCookies`. Партиционированные не берутся.
pub(crate) fn from_cdp(value: &Value) -> Option<Cookie> {
    if value.get("partitionKey").is_some_and(|key| !key.is_null()) || value["partitionKeyOpaque"].as_bool() == Some(true) {
        return None;
    }
    let domain = value["domain"].as_str()?.to_string();
    let expires = value["expires"].as_f64().unwrap_or(-1.0);
    let session = value["session"].as_bool().unwrap_or(false) || expires < 0.0;
    Cookie {
        host_only: !domain.starts_with('.'),
        domain,
        name: value["name"].as_str()?.to_string(),
        value: value["value"].as_str().unwrap_or_default().to_string(),
        path: value["path"].as_str().unwrap_or("/").to_string(),
        expires: (!session).then_some(expires as i64),
        http_only: value["httpOnly"].as_bool().unwrap_or(false),
        secure: value["secure"].as_bool().unwrap_or(false),
        same_site: value["sameSite"].as_str().and_then(SameSite::parse),
    }
    .normalized()
}

/// `CookieParam` для `Storage.setCookies`. Host-only ставится через `url` без `domain`: с `domain` браузер сделал бы её доменной.
pub(crate) fn to_cdp(cookie: &Cookie) -> Value {
    let mut param = Map::new();
    param.insert("name".into(), json!(cookie.name));
    param.insert("value".into(), json!(cookie.value));
    if cookie.host_only {
        let scheme = if cookie.secure { "https" } else { "http" };
        param.insert("url".into(), json!(format!("{scheme}://{}{}", cookie.domain, cookie.path)));
    } else {
        param.insert("domain".into(), json!(cookie.domain));
    }
    param.insert("path".into(), json!(cookie.path));
    param.insert("secure".into(), json!(cookie.secure));
    param.insert("httpOnly".into(), json!(cookie.http_only));
    if let Some(same_site) = cookie.same_site {
        let name = match same_site {
            SameSite::Strict => "Strict",
            SameSite::Lax => "Lax",
            SameSite::None => "None",
        };
        param.insert("sameSite".into(), json!(name));
    }
    if let Some(expires) = cookie.expires {
        param.insert("expires".into(), json!(expires));
    }
    Value::Object(param)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::fake_cdp::FakeCdp;

    fn cookie(domain: &str, name: &str, host_only: bool, expires: Option<i64>) -> Cookie {
        Cookie {
            domain: domain.into(),
            name: name.into(),
            value: format!("v-{name}"),
            path: "/".into(),
            expires,
            http_only: true,
            secure: true,
            same_site: Some(SameSite::Lax),
            host_only,
        }
    }

    #[test]
    fn cookie_params_follow_cdp_rules() {
        let host = to_cdp(&cookie("shop.example", "a", true, None));
        assert_eq!(host["url"], "https://shop.example/");
        assert!(host.get("domain").is_none(), "host-only ставится через url, иначе станет доменной");
        assert!(host.get("expires").is_none(), "сессионная — без срока");
        assert_eq!(host["sameSite"], "Lax");
        let mut plain = cookie(".example.com", "b", false, Some(1_900_000_000));
        plain.secure = false;
        plain.same_site = None;
        let domain = to_cdp(&plain);
        assert_eq!(domain["domain"], ".example.com");
        assert_eq!(domain["expires"], 1_900_000_000);
        assert!(domain.get("url").is_none() && domain.get("sameSite").is_none());
        let mut insecure = cookie("shop.example", "c", true, None);
        insecure.secure = false;
        insecure.path = "/cart".into();
        assert_eq!(to_cdp(&insecure)["url"], "http://shop.example/cart");
    }

    #[test]
    fn browser_cookies_are_read_by_session_and_partition_rules() {
        let session = json!({"name": "s", "value": "1", "domain": "shop.example", "path": "/", "expires": -1, "size": 2,
                             "httpOnly": false, "secure": false, "session": true, "sameSite": "Strict"});
        let parsed = from_cdp(&session).unwrap();
        assert_eq!((parsed.expires, parsed.host_only, parsed.same_site), (None, true, Some(SameSite::Strict)));
        let lasting = json!({"name": "l", "value": "2", "domain": ".Example.COM", "path": "/", "expires": 1900000000.75,
                             "httpOnly": true, "secure": true, "session": false});
        let parsed = from_cdp(&lasting).unwrap();
        assert_eq!(
            (parsed.domain.as_str(), parsed.expires, parsed.host_only, parsed.same_site),
            (".example.com", Some(1_900_000_000), false, None)
        );
        let partitioned = json!({"name": "p", "value": "3", "domain": "shop.example", "path": "/", "expires": -1,
                                 "session": true, "partitionKey": {"topLevelSite": "https://other.example", "hasCrossSiteAncestor": true}});
        assert_eq!(from_cdp(&partitioned), None, "партиционированные cookies не берутся");
    }

    #[tokio::test]
    async fn cookies_round_trip_through_the_browser() {
        let fake = FakeCdp::spawn().await;
        let mut client = CdpClient::connect(&fake.ws_url()).await.unwrap();
        let sent = vec![
            cookie("shop.example", "host", true, Some(1_900_000_000)),
            cookie(".example.com", "domain", false, Some(1_900_000_000)),
            cookie("shop.example", "session", true, None),
        ];
        assert_eq!(client.set_cookies(&sent).await.unwrap(), SetCookiesReport { set: 3, rejected: 0 });
        let mut got = client.get_cookies().await.unwrap();
        got.sort_by(|a, b| a.name.cmp(&b.name));
        let mut expected = sent.clone();
        expected.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(got, expected);
        assert_eq!(fake.calls(), ["Storage.setCookies", "Storage.getCookies"]);
    }

    #[tokio::test]
    async fn partitioned_cookies_are_not_taken() {
        let fake = FakeCdp::spawn().await;
        fake.set_browser_cookies(vec![cookie("shop.example", "plain", true, None)]);
        fake.add_raw_cookie(json!({"name": "chips", "value": "x", "domain": "shop.example", "path": "/", "expires": -1,
                                   "session": true, "partitionKey": {"topLevelSite": "https://other.example", "hasCrossSiteAncestor": true}}));
        let mut client = CdpClient::connect(&fake.ws_url()).await.unwrap();
        let names: Vec<String> = client.get_cookies().await.unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["plain"]);
    }

    #[tokio::test]
    async fn rejected_cookie_is_skipped_one_by_one() {
        let fake = FakeCdp::spawn().await;
        fake.reject_cookie("__Host-bad");
        let mut client = CdpClient::connect(&fake.ws_url()).await.unwrap();
        let sent = vec![
            cookie("a.example", "ok1", true, None),
            cookie("a.example", "__Host-bad", true, None),
            cookie("a.example", "ok2", true, None),
        ];
        assert_eq!(client.set_cookies(&sent).await.unwrap(), SetCookiesReport { set: 2, rejected: 1 });
        let mut names: Vec<String> = fake.browser_cookies().into_iter().map(|c| c.name).collect();
        names.sort();
        assert_eq!(names, ["ok1", "ok2"]);
        assert_eq!(fake.calls().len(), 4, "пакет и три попытки по одной");
    }

    #[tokio::test]
    async fn window_bounds_reads_bounds() {
        let fake = FakeCdp::spawn().await;
        fake.add_page("https://a.example/");
        let mut client = CdpClient::connect(&fake.ws_url()).await.unwrap();
        let window = client.window_for_first_page().await.unwrap();
        let bounds = Bounds { left: 10, top: 20, width: 800, height: 600 };
        client.set_window_bounds(window, &bounds).await.unwrap();
        assert_eq!(client.window_bounds(window).await.unwrap(), bounds);
    }

    #[tokio::test]
    async fn empty_set_is_one_call() {
        let fake = FakeCdp::spawn().await;
        let mut client = CdpClient::connect(&fake.ws_url()).await.unwrap();
        assert_eq!(client.set_cookies(&[]).await.unwrap(), SetCookiesReport::default());
        assert_eq!(fake.calls(), ["Storage.setCookies"]);
    }

    #[tokio::test]
    async fn start_tabs_open_and_blank_page_closes() {
        let fake = FakeCdp::spawn().await;
        fake.add_page("chrome://newtab/");
        fake.add_page("https://restored.example/");
        let mut client = CdpClient::connect(&fake.ws_url()).await.unwrap();
        let urls = vec!["https://a.example/".to_string(), "https://b.example/".to_string()];
        client.open_start_tabs(&urls).await.unwrap();
        assert_eq!(fake.opened(), urls);
        assert_eq!(fake.pages(), ["https://restored.example/", "https://a.example/", "https://b.example/"]);
        assert_eq!(fake.calls(), ["Target.getTargets", "Target.createTarget", "Target.createTarget", "Target.closeTarget"]);
    }

    #[tokio::test]
    async fn no_start_tabs_leave_pages_alone() {
        let fake = FakeCdp::spawn().await;
        fake.add_page("chrome://newtab/");
        let mut client = CdpClient::connect(&fake.ws_url()).await.unwrap();
        client.open_start_tabs(&[]).await.unwrap();
        assert_eq!(fake.pages(), ["chrome://newtab/"]);
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn only_loopback_endpoints_are_accepted() {
        for url in [
            "ws://10.0.0.5:9222/devtools/browser/x",
            "ws://localhost:9222/devtools/browser/x",
            "wss://127.0.0.1:9222/devtools/browser/x",
            "ws://127.0.0.1:9222@evil.example/x",
            "ws://127.0.0.1.evil.example:9222/x",
            "ws://127.0.0.1:/x",
        ] {
            assert!(matches!(CdpClient::connect(url).await, Err(CdpError::NotLocal)), "{url}");
        }
    }

    #[tokio::test]
    async fn dead_browser_gives_closed_then_connect() {
        let fake = FakeCdp::spawn().await;
        let url = fake.ws_url();
        let mut client = CdpClient::connect(&url).await.unwrap();
        fake.go_down();
        assert!(matches!(client.get_cookies().await, Err(CdpError::Closed)), "соединение оборвано");
        assert!(matches!(CdpClient::connect(&url).await, Err(CdpError::Connect(_))), "слушатель закрыт");
    }
}
