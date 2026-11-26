//! Сеть вкладки для процесса (спецификация автоматизации, «engine::page», «Сеть»): события запросов, конец запроса,
//! тела запроса и ответа. Домен `Network` включён, только пока он кому-то нужен: включённая сеть видна странице.
use super::cdp::CdpError;
use super::page::{Browser, CdpEvent};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Тело запроса в событии — до этого размера; больше — отдельным вызовом `Network.getRequestPostData`.
const MAX_POST: u64 = 65_536;

/// Запрос вкладки, как его видит процесс.
#[derive(Debug, Clone, PartialEq)]
pub enum NetEvent {
    /// Запрос начался. `post_data` — тело, если браузер отдал его в событии; `has_post_data` — тело есть.
    Sent { id: String, url: String, headers: Value, post_data: Option<String>, has_post_data: bool },
    /// Запрос кончился: ответ получен целиком (`failed: false`) или ошибка.
    Done { id: String, failed: bool },
}

/// Сеть сессии вкладки; клоны делят счётчик тех, кому она нужна.
#[derive(Clone)]
pub struct Network {
    browser: Browser,
    session: String,
    users: Arc<tokio::sync::Mutex<usize>>,
}

/// Пока жив, домен `Network` сессии включён; последний отпущенный выключает его.
pub struct NetGuard {
    net: Network,
}

/// События запросов одной сессии.
pub struct NetEvents {
    events: broadcast::Receiver<CdpEvent>,
    session: String,
}

impl Network {
    pub(super) fn new(browser: Browser, session: String, users: Arc<tokio::sync::Mutex<usize>>) -> Self {
        Self { browser, session, users }
    }

    /// Включить сеть на время жизни охранника. События — из `events()`, взятого до этого вызова.
    pub async fn acquire(&self) -> Result<NetGuard, CdpError> {
        // Один замок на счётчик и на вызов CDP: включение и выключение идут строго по очереди,
        // поэтому ни гонки enable/disable, ни охранника без включённой сети не бывает.
        let mut users = self.users.lock().await;
        if *users == 0 {
            self.browser.call(Some(&self.session), "Network.enable", json!({ "maxPostDataSize": MAX_POST })).await?;
        }
        *users += 1;
        Ok(NetGuard { net: self.clone() })
    }

    /// Сеть той же страницы (общий счётчик — у одного `Page`).
    pub fn same_page(&self, other: &Network) -> bool {
        Arc::ptr_eq(&self.users, &other.users)
    }

    pub fn events(&self) -> NetEvents {
        NetEvents { events: self.browser.subscribe(), session: self.session.clone() }
    }

    /// Тело запроса, которого не было в событии.
    pub async fn post_data(&self, id: &str) -> Result<String, CdpError> {
        let reply = self.browser.call(Some(&self.session), "Network.getRequestPostData", json!({ "requestId": id })).await?;
        Ok(reply["postData"].as_str().unwrap_or_default().to_string())
    }

    /// Тело ответа строкой; двоичное — как UTF-8 с заменой.
    pub async fn response_body(&self, id: &str) -> Result<String, CdpError> {
        let reply = self.browser.call(Some(&self.session), "Network.getResponseBody", json!({ "requestId": id })).await?;
        let body = reply["body"].as_str().unwrap_or_default();
        if reply["base64Encoded"].as_bool() == Some(true) {
            let bytes = B64.decode(body).map_err(|_| CdpError::Protocol("тело ответа не декодировано".into()))?;
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        } else {
            Ok(body.to_string())
        }
    }
}

impl Drop for NetGuard {
    fn drop(&mut self) {
        // Без среды выполнения выключить нечем: сеть остаётся включённой.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let net = self.net.clone();
        runtime.spawn(async move {
            let mut users = net.users.lock().await;
            *users -= 1;
            if *users == 0 {
                let _ = net.browser.call(Some(&net.session), "Network.disable", json!({})).await;
            }
        });
    }
}

impl NetEvents {
    /// Следующее событие запроса этой сессии; `None` — соединение закрыто.
    pub async fn next(&mut self) -> Option<NetEvent> {
        loop {
            let event = match self.events.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            };
            if event.session.as_deref() != Some(self.session.as_str()) {
                continue;
            }
            let id = event.params["requestId"].as_str().unwrap_or_default().to_string();
            match event.method.as_str() {
                "Network.requestWillBeSent" => {
                    let request = &event.params["request"];
                    let post_data = request["postData"].as_str().map(str::to_string);
                    return Some(NetEvent::Sent {
                        id,
                        url: request["url"].as_str().unwrap_or_default().to_string(),
                        headers: request["headers"].clone(),
                        has_post_data: post_data.is_some() || request["hasPostData"].as_bool() == Some(true),
                        post_data,
                    });
                }
                "Network.loadingFinished" => return Some(NetEvent::Done { id, failed: false }),
                "Network.loadingFailed" => return Some(NetEvent::Done { id, failed: true }),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::page::tests::{Mock, base};
    use std::time::Duration;

    async fn page(mock: &Mock) -> crate::engine::page::Page {
        Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap()
    }

    #[tokio::test]
    async fn network_is_enabled_while_needed_and_disabled_by_the_last_user() {
        let mock = Mock::spawn(|method, _, _| base(method).unwrap_or_else(|| json!({}))).await;
        let page = page(&mock).await;
        let first = page.network().acquire().await.unwrap();
        let second = page.network().acquire().await.unwrap();
        assert_eq!(mock.sent("Network.enable").len(), 1, "второй пользователь не включает заново");
        assert_eq!(mock.sessions("Network.enable"), ["S1"]);
        drop(first);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(mock.sent("Network.disable").is_empty(), "второй ещё слушает");
        drop(second);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(mock.sent("Network.disable").len(), 1);
    }

    #[tokio::test]
    async fn request_events_of_the_tab_session_and_bodies() {
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Network.getResponseBody" => json!({ "body": "eyJvayI6MX0=", "base64Encoded": true }),
                "Network.getRequestPostData" => json!({ "postData": "big=1" }),
                _ => json!({}),
            })
        })
        .await;
        let page = page(&mock).await;
        let net = page.network();
        let mut events = net.events();
        let _guard = net.acquire().await.unwrap();
        let sent = |session: &str, id: &str, url: &str| {
            json!({ "method": "Network.requestWillBeSent", "sessionId": session,
                    "params": { "requestId": id, "request": { "url": url, "headers": { "X-A": "1" }, "hasPostData": true } } })
        };
        mock.events.send(sent("S7", "R0", "https://other.test/")).unwrap();
        mock.events.send(sent("S1", "R1", "https://a.test/api?k=7")).unwrap();
        mock.events.send(json!({ "method": "Network.loadingFinished", "sessionId": "S1", "params": { "requestId": "R1" } })).unwrap();
        mock.events.send(json!({ "method": "Network.loadingFailed", "sessionId": "S1", "params": { "requestId": "R2" } })).unwrap();
        let first = NetEvent::Sent { id: "R1".into(), url: "https://a.test/api?k=7".into(), headers: json!({ "X-A": "1" }), post_data: None, has_post_data: true };
        assert_eq!(events.next().await, Some(first), "событие другой вкладки пропущено");
        assert_eq!(events.next().await, Some(NetEvent::Done { id: "R1".into(), failed: false }));
        assert_eq!(events.next().await, Some(NetEvent::Done { id: "R2".into(), failed: true }));
        assert_eq!(net.response_body("R1").await.unwrap(), r#"{"ok":1}"#);
        assert_eq!(net.post_data("R1").await.unwrap(), "big=1");
    }

    #[tokio::test]
    async fn cookies_of_the_tab_address_are_read_and_deleted() {
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Network.getCookies" => json!({ "cookies": [{ "name": "sid", "value": "1", "domain": "a.test", "path": "/", "expires": -1,
                                                             "httpOnly": true, "secure": false, "session": true }] }),
                _ => json!({}),
            })
        })
        .await;
        let page = page(&mock).await;
        let cookies = page.cookies("https://a.test/x").await.unwrap();
        assert_eq!((cookies.len(), cookies[0].name.as_str(), cookies[0].host_only), (1, "sid", true));
        assert_eq!(mock.sent("Network.getCookies")[0]["urls"], json!(["https://a.test/x"]));
        assert!(mock.sent("Network.enable").is_empty(), "cookies читаются без включения сети");
        page.delete_cookies(&cookies).await.unwrap();
        assert_eq!(mock.sent("Network.deleteCookies")[0], json!({ "name": "sid", "domain": "a.test", "path": "/" }));
    }

    fn order(mock: &Mock) -> Vec<String> {
        let requests = mock.requests.lock().unwrap();
        requests.iter().filter_map(|r| r["method"].as_str()).filter(|m| m.starts_with("Network.")).map(str::to_string).collect()
    }

    #[tokio::test]
    async fn failed_enable_rolls_back() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let mock = Mock::spawn(move |method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Network.enable" if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 => json!({ "error": "x" }),
                _ => json!({}),
            })
        })
        .await;
        let page = page(&mock).await;
        assert!(page.network().acquire().await.is_err());
        let _guard = page.network().acquire().await.unwrap();
        assert_eq!(order(&mock), ["Network.enable", "Network.enable"]);
    }

    #[tokio::test]
    async fn drop_then_reacquire_stays_on() {
        let mock = Mock::spawn(|method, _, _| base(method).unwrap_or_else(|| json!({}))).await;
        let page = page(&mock).await;
        let first = page.network().acquire().await.unwrap();
        drop(first);
        let _second = page.network().acquire().await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_ne!(order(&mock).last().map(String::as_str), Some("Network.disable"));
    }
}
