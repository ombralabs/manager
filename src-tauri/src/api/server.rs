//! HTTP-сервер Local API: только `127.0.0.1`, проверки защиты и ключа одним слоем перед всеми маршрутами
//! (спецификация 2Б, «Общие правила»).
use super::envelope::{fail, not_found, ok};
use super::browser;
use super::extensions;
use super::groups;
use super::profiles;
use super::proxies;
use super::stubs;
use crate::app::Core;
use crate::engine::Engine;
use crate::store::{DEFAULT_API_PORT, Store, StoreError};
use crate::supervisor::Event;
use axum::Router;
use axum::extract::{Request, State};
use axum::handler::Handler;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, MethodRouter, on};
use serde_json::json;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Имена, под которыми скрипты зовут Local API: свои и оба имени AdsPower. Остальное — DNS rebinding.
const ALLOWED_HOSTS: &[&str] = &["127.0.0.1", "localhost", "[::1]", "local.adspower.net", "local.adspower.com"];

/// Операции, после которых меняются профили, группы или прокси: интерфейс перечитывает списки (спецификация 2В-1).
const CHANGING_PATHS: &[&str] = &[
    "/api/v1/user/create",
    "/api/v2/browser-profile/create",
    "/api/v1/user/update",
    "/api/v2/browser-profile/update",
    "/api/v1/user/delete",
    "/api/v2/browser-profile/delete",
    "/api/v1/user/regroup",
    "/api/v1/group/create",
    "/api/v1/group/update",
    "/api/v2/proxy-list/create",
    "/api/v2/proxy-list/update",
    "/api/v2/proxy-list/delete",
];

#[derive(Clone, PartialEq, Eq)]
pub struct ApiSettings {
    pub port: u16,
    /// `None` — ключ не проверяется.
    pub key: Option<String>,
}

// Ключ API — секрет: `Debug` показывает только, задан ли он.
impl fmt::Debug for ApiSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiSettings").field("port", &self.port).field("key", &self.key.as_ref().map(|_| "***")).finish()
    }
}

impl ApiSettings {
    /// Порт и ключ из настроек. Порт не прочитан — порт по умолчанию; ключ не прочитан — ошибка:
    /// сервер без проверки ключа, который задал пользователь, был бы открыт любому скрипту.
    pub fn load(store: &Store) -> Result<Self, StoreError> {
        Ok(Self { port: store.api_port().unwrap_or(DEFAULT_API_PORT), key: store.api_key()? })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("порт {0} занят")]
    PortBusy(u16),
    #[error("Local API: {0}")]
    Io(#[from] std::io::Error),
}

/// Работающий сервер. Брошенный — сразу закрывает порт.
pub struct ApiServer {
    addr: SocketAddr,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl ApiServer {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Сразу перестаёт принимать соединения и закрывает порт: приём отменяется без ожидания.
    /// Идущие запросы не ждутся — каждое соединение axum ведёт в своей задаче, они доходят до конца сами.
    /// Так выход из приложения укладывается в `STOP_ALL_TIMEOUT`, а запуск, пришедший после `stop_all`,
    /// отклоняет супервизор (`closing`).
    pub fn shutdown(self) {
        // Отмена и прерывание задачи приёма — в `Drop`.
        drop(self);
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Слушает только `127.0.0.1:<port>`; порт 0 — любой свободный (тесты).
pub async fn serve<E: Engine>(core: Arc<Core<E>>, settings: ApiSettings) -> Result<ApiServer, ApiError> {
    // На macOS привязка к 127.0.0.1 проходит, даже если чужой сервер (AdsPower) слушает 0.0.0.0 на том же порту, —
    // и тогда мы молча перехватили бы его запросы. Поэтому сначала пробуем подключиться: кто-то ответил — порт занят.
    if settings.port != 0
        && tokio::time::timeout(Duration::from_millis(500), tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, settings.port)))
            .await
            .is_ok_and(|connected| connected.is_ok())
    {
        return Err(ApiError::PortBusy(settings.port));
    }
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, settings.port)).await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return Err(ApiError::PortBusy(settings.port)),
        Err(e) => return Err(e.into()),
    };
    let addr = listener.local_addr()?;
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let app = router(core, settings.key);
    let task = tokio::spawn(async move {
        let shutdown = async move { signal.cancelled().await };
        if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
            log::warn!("Local API остановился с ошибкой: {e}");
        }
    });
    Ok(ApiServer { addr, cancel, task: Some(task) })
}

/// Все маршруты спецификации; проверки защиты и ключа — слоем поверх маршрутов, 404 и 405.
pub fn router<E: Engine>(core: Arc<Core<E>>, key: Option<String>) -> Router {
    let guard = Guard { key: key.filter(|key| !key.is_empty()).map(Arc::from) };
    api_routes::<E>()
        .fallback(|| async { not_found() })
        .method_not_allowed_fallback(|| async {
            (StatusCode::METHOD_NOT_ALLOWED, fail("method not allowed")).into_response()
        })
        .layer(middleware::from_fn_with_state(core.clone(), notify_changes::<E>))
        .layer(middleware::from_fn_with_state(guard, protect))
        .with_state(core)
}

/// Все операции Local API. Новые маршруты добавляются сюда (`.merge(…::routes::<E>())`) — до слоя защиты в `router`.
fn api_routes<E: Engine>() -> Router<Arc<Core<E>>> {
    Router::new()
        .route("/status", both(status))
        .merge(stubs::routes())
        .merge(browser::routes::<E>())
        .merge(profiles::routes::<E>())
        .merge(groups::routes::<E>())
        .merge(extensions::routes::<E>())
        .merge(proxies::routes::<E>())
}

/// GET и POST на один обработчик: параметры читаются и из query, и из тела, а скрипты не всегда точны с методом.
pub(super) fn both<H, T, S>(handler: H) -> MethodRouter<S>
where
    H: Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    on(MethodFilter::GET.or(MethodFilter::POST), handler)
}

/// Connection Status.
async fn status() -> Response {
    ok(json!({}))
}

#[derive(Clone)]
struct Guard {
    key: Option<Arc<str>>,
}

/// Операция из `CHANGING_PATHS` → событие `ProfilesChanged` на любой ответ в формате AdsPower (HTTP 200), в том числе
/// с ошибкой: пачка, упавшая посередине, могла изменить часть записей. Запросы, отклонённые `protect`, сюда не доходят
/// (слой снаружи); 404 и 405 ничего не меняют. Шлётся до отправки ответа: скрипт, получивший ответ, не обгонит событие.
async fn notify_changes<E: Engine>(State(core): State<Arc<Core<E>>>, request: Request, next: Next) -> Response {
    let changing = CHANGING_PATHS.contains(&request.uri().path());
    let response = next.run(request).await;
    if changing && response.status() == StatusCode::OK {
        core.supervisor.emit(Event::ProfilesChanged);
    }
    response
}

/// Защита одним слоем перед всеми маршрутами, 404 и 405: запросы страниц (`from_web_page`), чужое имя в `Host`
/// (DNS rebinding), неверный ключ API, если он задан.
///
/// Браузерный `User-Agent` не проверяется (решение заказчика, приёмка 2Г): PowerShell и `APIRequestContext` Playwright
/// шлют `Mozilla/…` по умолчанию и должны работать. Принятый риск: на имена `local.adspower.net` и `local.adspower.com`
/// браузер не шлёт `Sec-Fetch-*` (адрес для него не «надёжный»), а простой GET со страницы (`<img>`, `<iframe>`,
/// переход по ссылке) идёт без `Origin` — такой запрос с любого сайта дойдёт до Local API. Риск закрывает
/// «Требовать ключ»: простой запрос не несёт `Authorization`, а `fetch` с этим заголовком идёт с `Origin`.
async fn protect(State(guard): State<Guard>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    if from_web_page(headers) {
        return fail("requests from web pages are not allowed");
    }
    if !host_allowed(headers) {
        return fail("host is not allowed");
    }
    if let Some(key) = &guard.key
        && !key_matches(headers, key)
    {
        return fail("invalid api key");
    }
    next.run(request).await
}

/// Браузер ставит `Origin` на `fetch` и формы страниц, а `Sec-Fetch-Site` — на свои запросы к «надёжным» адресам
/// (`127.0.0.1`, `localhost`); скрипты не шлют ни того, ни другого.
fn from_web_page(headers: &HeaderMap) -> bool {
    headers.contains_key(header::ORIGIN)
        || matches!(
            headers.get("sec-fetch-site").and_then(|value| value.to_str().ok()),
            Some("cross-site" | "same-site")
        )
}

fn host_allowed(headers: &HeaderMap) -> bool {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| ALLOWED_HOSTS.contains(&host_name(host).to_ascii_lowercase().as_str()))
}

/// Имя из заголовка `Host` без порта; IPv6 — в квадратных скобках.
fn host_name(host: &str) -> &str {
    if host.starts_with('[') {
        return host.find(']').map_or(host, |end| &host[..=end]);
    }
    match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    }
}

fn key_matches(headers: &HeaderMap, key: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token.trim() == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::{ApiEnv, api_env, api_env_with_key};
    use crate::relay::test_support::port_closes;
    use crate::store::tests::memory_store;
    use serde_json::{Value, json};

    async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap())
    }

    fn status_with(env: &ApiEnv, name: &str, value: &str) -> reqwest::RequestBuilder {
        env.client.get(format!("{}/status", env.base)).header(name, value)
    }

    fn rejected(msg: &str) -> (u16, Value) {
        (200, json!({"code": -1, "data": {}, "msg": msg}))
    }

    fn success() -> (u16, Value) {
        (200, json!({"code": 0, "data": {}, "msg": "success"}))
    }

    #[tokio::test]
    async fn status_answers_on_get_and_post() {
        let env = api_env().await;
        assert_eq!(env.get("/status").await, json!({"code": 0, "data": {}, "msg": "success"}));
        assert_eq!(env.post("/status", json!({})).await, json!({"code": 0, "data": {}, "msg": "success"}));
    }

    #[tokio::test]
    async fn changing_operations_notify_on_any_answer() {
        let env = api_env().await;
        let created = env.post("/api/v1/user/create", json!({"group_id": "0"})).await;
        assert_eq!(created["code"], 0, "{created}");
        assert_eq!(env.changes(), 1);

        assert_eq!(env.get("/api/v1/user/list").await["code"], 0);
        assert_eq!(env.changes(), 1, "чтение ничего не меняет");
        // Ответ с ошибкой — тоже событие: пачка, упавшая посередине, могла изменить часть записей.
        let rejected = env.post("/api/v1/user/create", json!({"group_id": "999"})).await;
        assert_eq!(rejected["code"], -1, "{rejected}");
        assert_eq!(env.changes(), 2, "ответ изменяющего пути с ошибкой — событие есть");
        let (status, _) = send(env.client.delete(format!("{}/api/v1/user/create", env.base))).await;
        assert_eq!(status, 405);
        assert_eq!(env.changes(), 2, "метод не поддержан — операции не было");

        assert_eq!(env.post("/api/v1/group/create", json!({"group_name": "Магазины"})).await["code"], 0);
        assert_eq!(env.changes(), 3);
        let proxy = json!({"type": "http", "host": "10.0.0.1", "port": "8080"});
        assert_eq!(env.post("/api/v2/proxy-list/create", proxy).await["code"], 0);
        assert_eq!(env.changes(), 4);
    }

    #[tokio::test]
    async fn rejected_by_protection_does_not_notify() {
        let env = api_env().await;
        let request = env
            .client
            .post(format!("{}/api/v1/user/create", env.base))
            .header("Origin", "https://evil.example")
            .json(&json!({"group_id": "0"}));
        assert_eq!(send(request).await, rejected("requests from web pages are not allowed"));
        assert_eq!(env.changes(), 0);
        assert_eq!(env.core.store.count_profiles().unwrap(), 0);
    }

    #[tokio::test]
    async fn unknown_paths_and_methods_keep_the_envelope() {
        let env = api_env().await;
        assert_eq!(
            send(env.client.get(format!("{}/api/v9/nothing", env.base))).await,
            (404, json!({"code": -1, "data": {}, "msg": "unknown endpoint"}))
        );
        assert_eq!(
            send(env.client.delete(format!("{}/status", env.base))).await,
            (405, json!({"code": -1, "data": {}, "msg": "method not allowed"}))
        );
    }

    #[tokio::test]
    async fn web_pages_are_rejected() {
        let env = api_env().await;
        let web = "requests from web pages are not allowed";
        assert_eq!(send(status_with(&env, "Origin", "https://example.com")).await, rejected(web));
        assert_eq!(send(status_with(&env, "Origin", "null")).await, rejected(web));
        assert_eq!(send(status_with(&env, "Sec-Fetch-Site", "cross-site")).await, rejected(web));
        assert_eq!(send(status_with(&env, "Sec-Fetch-Site", "same-site")).await, rejected(web));
        assert_eq!(send(status_with(&env, "Sec-Fetch-Site", "none")).await, success());
        assert_eq!(send(status_with(&env, "Sec-Fetch-Site", "same-origin")).await, success());
        // Проверка идёт раньше маршрутов: страница не узнаёт даже, какие пути есть.
        assert_eq!(
            send(env.client.get(format!("{}/api/v1/user/list", env.base)).header("Origin", "https://example.com")).await,
            rejected(web)
        );
    }

    /// Запрос без заголовков клиента: только `Host` и `Connection: close` — в том числе без `User-Agent`.
    async fn bare_get(addr: SocketAddr, path: &str) -> (u16, Value) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, serde_json::from_str(body).unwrap())
    }

    /// Решение заказчика (2Г): клиенты с браузерным `User-Agent` по умолчанию (PowerShell, `APIRequestContext`
    /// Playwright) работают. От страниц защищают `Origin` и `Sec-Fetch-Site`; простой GET со страницы на имя
    /// `local.adspower.*` идёт без них и доходит — принятый риск, его закрывает «Требовать ключ».
    #[tokio::test]
    async fn browser_user_agent_is_allowed_web_pages_are_not() {
        let env = api_env().await;
        let port = env.server.addr().port();
        let web = "requests from web pages are not allowed";
        let chrome = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) \
                      Chrome/146.0.0.0 Safari/537.36";
        let powershell = "Mozilla/5.0 (Macintosh; Darwin 25.6.0 Darwin Kernel Version 25.6.0; ru-RU) PowerShell/7.5.3";
        assert_eq!(send(status_with(&env, "User-Agent", chrome)).await, success(), "браузерный User-Agent без Origin");
        assert_eq!(send(status_with(&env, "User-Agent", powershell)).await, success(), "PowerShell");
        // Имя AdsPower, GET и POST, операция с данными — как у скрипта.
        let adspower = format!("local.adspower.net:{port}");
        let get = env.client.get(format!("{}/status", env.base)).header("Host", &adspower).header("User-Agent", chrome);
        assert_eq!(send(get).await, success(), "GET на имя AdsPower");
        let post = env
            .client
            .post(format!("{}/status", env.base))
            .header("Host", &adspower)
            .header("User-Agent", chrome)
            .body("{}");
        assert_eq!(send(post).await, success(), "POST на имя AdsPower");
        let list = env.client.get(format!("{}/api/v1/user/list", env.base)).header("User-Agent", chrome);
        let (status, body) = send(list).await;
        assert_eq!((status, body["code"].clone()), (200, json!(0)), "{body}");

        // Признаки страницы — отказ при любом User-Agent.
        let with_origin = status_with(&env, "User-Agent", chrome).header("Origin", "https://evil.example");
        assert_eq!(send(with_origin).await, rejected(web), "Origin");
        for site in ["cross-site", "same-site"] {
            let request = status_with(&env, "User-Agent", chrome).header("Sec-Fetch-Site", site);
            assert_eq!(send(request).await, rejected(web), "Sec-Fetch-Site: {site}");
        }
        // Скрипты со своим User-Agent и без него — как раньше.
        assert_eq!(send(status_with(&env, "User-Agent", "axios/1.20.0")).await, success());
        assert_eq!(bare_get(env.server.addr(), "/status").await, success(), "без User-Agent");

        // «Требовать ключ» закрывает принятый риск: простой запрос страницы не несёт `Authorization`.
        let locked = api_env_with_key("s3cret").await;
        let page = locked
            .client
            .get(format!("{}/api/v1/browser/start?user_id=x", locked.base))
            .header("Host", format!("local.adspower.net:{}", locked.server.addr().port()))
            .header("User-Agent", chrome);
        assert_eq!(send(page).await, rejected("invalid api key"));
    }

    #[tokio::test]
    async fn only_local_host_names_are_accepted() {
        let env = api_env().await;
        let port = env.server.addr().port();
        for host in ["localhost", "127.0.0.1", "[::1]", "local.adspower.net", "LOCAL.ADSPOWER.COM"] {
            assert_eq!(send(status_with(&env, "Host", &format!("{host}:{port}"))).await, success(), "{host}");
        }
        assert_eq!(send(status_with(&env, "Host", "local.adspower.net")).await, success(), "без порта");
        for host in ["evil.example", "127.0.0.1.evil.example", "adspower.net", "[::2]"] {
            assert_eq!(
                send(status_with(&env, "Host", &format!("{host}:{port}"))).await,
                rejected("host is not allowed"),
                "{host}"
            );
        }
    }

    #[tokio::test]
    async fn api_key_is_checked_only_when_set() {
        let env = api_env_with_key("s3cret").await;
        assert_eq!(send(env.client.get(format!("{}/status", env.base))).await, rejected("invalid api key"));
        assert_eq!(send(status_with(&env, "Authorization", "Bearer wrong")).await, rejected("invalid api key"));
        assert_eq!(send(status_with(&env, "Authorization", "s3cret")).await, rejected("invalid api key"));
        assert_eq!(send(status_with(&env, "Authorization", "Bearer s3cret")).await, success());
        // Пример AdsPower шлёт заголовок всегда — без ключа он не мешает.
        let open = api_env().await;
        assert_eq!(send(status_with(&open, "Authorization", "Bearer your_api_key_here")).await, success());
    }

    #[tokio::test]
    async fn oversized_bodies_keep_the_envelope() {
        let env = api_env().await;
        // Чуть больше предела axum (2 МБ): тело Add Proxy читается как есть, остальные — через `Params`.
        let body = format!("{{\"remark\": \"{}\"}}", "x".repeat(2 * 1024 * 1024 + 1024));
        for path in ["/api/v2/proxy-list/create", "/api/v1/user/create"] {
            let request = env.client.post(format!("{}{path}", env.base)).body(body.clone());
            assert_eq!(send(request).await, rejected("request body is too large"), "{path}");
        }
    }

    #[tokio::test]
    async fn busy_port_is_reported_and_shutdown_frees_it() {
        let env = api_env().await;
        let port = env.server.addr().port();
        let second = serve(env.core.clone(), ApiSettings { port, key: None }).await;
        assert!(matches!(second, Err(ApiError::PortBusy(p)) if p == port), "второй сервер на занятом порту");
        // Порт держит чужой сервер (как AdsPower на 50325): тестовый HTTP-прокси окружения слушает 127.0.0.1.
        let foreign = serve(env.core.clone(), ApiSettings { port: env.proxy_port, key: None }).await;
        assert!(matches!(foreign, Err(ApiError::PortBusy(p)) if p == env.proxy_port), "порт чужого сервера");
        // Чужой сервер слушает все адреса (0.0.0.0), как может AdsPower: на macOS bind на 127.0.0.1 прошёл бы.
        let wildcard = std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let wildcard_port = wildcard.local_addr().unwrap().port();
        let shadowing = serve(env.core.clone(), ApiSettings { port: wildcard_port, key: None }).await;
        assert!(matches!(shadowing, Err(ApiError::PortBusy(p)) if p == wildcard_port), "порт, занятый на 0.0.0.0");
        drop(wildcard);

        let extra = serve(env.core.clone(), ApiSettings { port: 0, key: None }).await.unwrap();
        let addr = extra.addr();
        assert!(addr.ip().is_loopback(), "{addr}");
        extra.shutdown();
        assert!(port_closes(addr).await, "после остановки порт закрыт");
    }

    #[test]
    fn settings_come_from_the_store() {
        let store = memory_store();
        assert_eq!(ApiSettings::load(&store).unwrap(), ApiSettings { port: DEFAULT_API_PORT, key: None });
        store.set_api_port(8081).unwrap();
        store.set_api_key(Some("k")).unwrap();
        assert_eq!(ApiSettings::load(&store).unwrap(), ApiSettings { port: 8081, key: Some("k".into()) });
        let text = format!("{:?}", ApiSettings { port: 8081, key: Some("s3cret".into()) });
        assert!(text.contains("8081") && !text.contains("s3cret"), "ключ в Debug не печатается: {text}");
        store.set_api_key(Some("")).unwrap();
        assert_eq!(ApiSettings::load(&store).unwrap().key, None);
    }

    #[test]
    fn unreadable_key_is_an_error_not_an_open_api() {
        let store = memory_store();
        store.set_api_key(Some("k")).unwrap();
        store.raw().execute_batch("DROP TABLE settings").unwrap();
        assert!(store.api_key().is_err(), "ключ не читается");
        assert!(ApiSettings::load(&store).is_err(), "без ключа сервер был бы открыт всем");
    }

    #[test]
    fn host_names_lose_their_port() {
        assert_eq!(host_name("localhost:50325"), "localhost");
        assert_eq!(host_name("[::1]:50325"), "[::1]");
        assert_eq!(host_name("[::1]"), "[::1]");
        assert_eq!(host_name("local.adspower.net"), "local.adspower.net");
    }
}
