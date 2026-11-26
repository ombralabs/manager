mod http;
pub(crate) mod log;
mod upstream;
mod countries;
pub mod geo;

#[cfg(test)]
pub(crate) mod test_support;

pub use upstream::{BoxedIo, Credentials, UpstreamConfig, UpstreamKind, client_tls, connect as connect_upstream};

use http::{
    Form, MAX_HEAD, MAX_RESPONSE_HEAD, TUNNEL_ESTABLISHED, Target, close_response_head, find_head_end, forward_head, join_host_port,
    parse_head, parse_status, parse_target, status_response,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// После стольких ошибок подключения подряд шлюз считается недоступным.
pub const UNREACHABLE_AFTER: u32 = 5;

/// Пауза после ошибки `accept` (например, кончились дескрипторы), чтобы цикл не крутился вхолостую.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Ok,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct StatsSnapshot {
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub connections: u64,
    pub upstream_failures: u64,
}

#[derive(Default)]
struct Stats {
    up: AtomicU64,
    down: AtomicU64,
    connections: AtomicU64,
    upstream_failures: AtomicU64,
    consecutive_failures: AtomicU32,
}

struct Shared {
    upstream: UpstreamConfig,
    stats: Arc<Stats>,
    health: watch::Sender<Health>,
    log: Option<log::ConnLog>,
}

impl Shared {
    fn upstream_ok(&self) {
        self.stats.consecutive_failures.store(0, Relaxed);
        self.health.send_if_modified(|h| std::mem::replace(h, Health::Ok) != Health::Ok);
    }

    fn upstream_failed(&self, error: &upstream::UpstreamError) {
        if !error.is_proxy_failure() {
            return;
        }
        self.stats.upstream_failures.fetch_add(1, Relaxed);
        let streak = self.stats.consecutive_failures.fetch_add(1, Relaxed) + 1;
        if streak >= UNREACHABLE_AFTER {
            self.health.send_if_modified(|h| std::mem::replace(h, Health::Unreachable) != Health::Unreachable);
        }
    }
}

/// Прослойка одного профиля. Слушает только `127.0.0.1`.
pub struct RelayHandle {
    addr: SocketAddr,
    cancel: CancellationToken,
    stats: Arc<Stats>,
    started_ms: u128,
    health: watch::Receiver<Health>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl RelayHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Момент запуска (мс Unix): записи журнала с `ts_ms` не раньше него уже учтены в `stats()`.
    pub fn started_ms(&self) -> u128 {
        self.started_ms
    }

    pub fn stats(&self) -> StatsSnapshot {
        StatsSnapshot {
            bytes_up: self.stats.up.load(Relaxed),
            bytes_down: self.stats.down.load(Relaxed),
            connections: self.stats.connections.load(Relaxed),
            upstream_failures: self.stats.upstream_failures.load(Relaxed),
        }
    }

    pub fn health(&self) -> watch::Receiver<Health> {
        self.health.clone()
    }

    /// Закрывает слушатель и все соединения профиля.
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

/// Брошенная прослойка не должна держать порт: без этого она жила бы до выхода из приложения.
impl Drop for RelayHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub async fn start(upstream: UpstreamConfig, log_path: Option<PathBuf>) -> std::io::Result<RelayHandle> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let stats = Arc::new(Stats::default());
    let (health_tx, health_rx) = watch::channel(Health::Ok);
    let log = log_path.map(log::ConnLog::open).transpose()?;
    let shared = Arc::new(Shared { upstream, stats: stats.clone(), health: health_tx, log });
    let cancel = CancellationToken::new();
    let task = tokio::spawn(accept_loop(listener, shared, cancel.clone()));
    let started_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    Ok(RelayHandle { addr, cancel, stats, started_ms, health: health_rx, task: Some(task) })
}

async fn accept_loop(listener: TcpListener, shared: Arc<Shared>, cancel: CancellationToken) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(pair) => pair,
                    Err(_) => {
                        // Пауза прерывается отменой: остановка профиля не ждёт её конца.
                        if !pause_unless_cancelled(&cancel, ACCEPT_BACKOFF).await {
                            break;
                        }
                        continue;
                    }
                };
                let shared = shared.clone();
                let token = cancel.child_token();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = token.cancelled() => {}
                        _ = handle(stream, &shared) => {}
                    }
                });
            }
        }
    }
}

/// Пауза `pause`, прерываемая отменой; `false` — отменено.
async fn pause_unless_cancelled(cancel: &CancellationToken, pause: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(pause) => true,
    }
}

struct Outcome {
    target: String,
}

/// Поток браузера со счётчиками: байты идут в общую и в пособытийную статистику по мере передачи.
struct Counted {
    inner: TcpStream,
    stats: Arc<Stats>,
    up: u64,
    down: u64,
}

impl AsyncRead for Counted {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &poll {
            let n = (buf.filled().len() - before) as u64;
            self.up += n;
            self.stats.up.fetch_add(n, Relaxed);
        }
        poll
    }
}

impl AsyncWrite for Counted {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, data);
        if let Poll::Ready(Ok(n)) = &poll {
            self.down += *n as u64;
            self.stats.down.fetch_add(*n as u64, Relaxed);
        }
        poll
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

struct Failure {
    target: String,
    reason: String,
}

async fn handle(stream: TcpStream, shared: &Shared) {
    let started = Instant::now();
    shared.stats.connections.fetch_add(1, Relaxed);
    let mut client = Counted { inner: stream, stats: shared.stats.clone(), up: 0, down: 0 };
    let (target, result) = match serve(&mut client, shared).await {
        Ok(o) => (o.target, "ok".to_string()),
        Err(f) => (f.target, f.reason),
    };
    if let Some(log) = &shared.log {
        let ts_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        log.write(&log::Record {
            ts_ms,
            proxy_id: shared.upstream.proxy_id,
            target: &target,
            bytes_up: client.up,
            bytes_down: client.down,
            ms: started.elapsed().as_millis() as u64,
            result: &result,
        });
    }
}

async fn read_client_head(client: &mut Counted) -> Result<Vec<u8>, (u16, &'static str)> {
    match http::read_head(client, MAX_HEAD).await {
        Ok(http::Head::Done(buf)) => Ok(buf),
        Ok(http::Head::TooLarge) => Err((431, "Request Header Fields Too Large")),
        Ok(http::Head::Eof) | Err(_) => Err((400, "Bad Request")),
    }
}

async fn reply(client: &mut Counted, code: u16, reason: &str) {
    let _ = client.write_all(&status_response(code, reason)).await;
}

async fn serve(client: &mut Counted, shared: &Shared) -> Result<Outcome, Failure> {
    let buf = match read_client_head(client).await {
        Ok(buf) => buf,
        Err((code, reason)) => {
            reply(client, code, reason).await;
            return Err(Failure { target: String::new(), reason: code.to_string() });
        }
    };
    let head = match parse_head(&buf) {
        Ok(Some(head)) => head,
        _ => {
            reply(client, 400, "Bad Request").await;
            return Err(Failure { target: String::new(), reason: "400: неверный заголовок".into() });
        }
    };
    let target = match parse_target(&head.method, &head.target) {
        Ok(target) => target,
        Err(reason) => {
            reply(client, 400, "Bad Request").await;
            return Err(Failure { target: head.target.clone(), reason: format!("400: {reason}") });
        }
    };
    let cfg = &shared.upstream;
    let (label, remote, request) = match target {
        Target::Authority { host, port } => (join_host_port(&host, port), upstream::connect(cfg, &host, port).await, None),
        Target::Absolute { host, port, path } => {
            let label = format!("http://{}", join_host_port(&host, port));
            match cfg.kind {
                UpstreamKind::Http | UpstreamKind::Https => {
                    let request = forward_head(&head, Form::Absolute, cfg.basic_auth().as_deref());
                    (label, upstream::open_proxy_stream(cfg).await, Some(request))
                }
                UpstreamKind::Socks5 | UpstreamKind::Direct => {
                    let request = forward_head(&head, Form::Origin(&path), None);
                    (label, upstream::connect(cfg, &host, port).await, Some(request))
                }
            }
        }
    };
    let remote = match remote {
        Ok(remote) => remote,
        Err(error) => {
            shared.upstream_failed(&error);
            reply(client, 502, "Bad Gateway").await;
            return Err(Failure { target: label, reason: format!("502: {error}") });
        }
    };
    let leftover = &buf[head.len..];
    let io_failure = |e: std::io::Error| Failure { target: label.clone(), reason: e.to_string() };
    let mut remote = match request {
        // Запрос в абсолютной форме: браузер получает ответ с `Connection: close` и следующий запрос по этому соединению не шлёт.
        Some(request) => {
            // Через HTTP(S)-прокси заголовок ответа отдаёт сам прокси; через SOCKS5 — уже сайт, прокси своё дело сделал.
            let proxy_answers = matches!(cfg.kind, UpstreamKind::Http | UpstreamKind::Https);
            if !proxy_answers {
                shared.upstream_ok();
            }
            let (mut rr, mut rw) = tokio::io::split(remote);
            let response = match exchange_head(client, &mut rr, &mut rw, &request, leftover, proxy_answers).await {
                Ok(response) => response,
                Err(error) => {
                    if proxy_answers {
                        shared.upstream_failed(&error);
                    }
                    reply(client, 502, "Bad Gateway").await;
                    return Err(Failure { target: label, reason: format!("502: {error}") });
                }
            };
            if proxy_answers {
                shared.upstream_ok();
            }
            let end = find_head_end(&response).unwrap_or(response.len());
            client.write_all(&close_response_head(&response[..end])).await.map_err(io_failure)?;
            client.write_all(&response[end..]).await.map_err(io_failure)?;
            rr.unsplit(rw)
        }
        None => {
            shared.upstream_ok();
            client.write_all(TUNNEL_ESTABLISHED).await.map_err(io_failure)?;
            let mut remote = remote;
            if !leftover.is_empty() {
                remote.write_all(leftover).await.map_err(io_failure)?;
            }
            remote
        }
    };
    tokio::io::copy_bidirectional(client, &mut remote).await.map_err(io_failure)?;
    Ok(Outcome { target: label })
}

/// Шлёт запрос и читает заголовок ответа; пока он не пришёл, тело от браузера продолжает пересылаться.
/// Через HTTP(S)-прокси (`proxy_answers`) ответ `407` — сбой самого прокси: браузер его не увидит.
/// Через SOCKS5 отвечает уже сайт, и его `407` — просто ответ сайта.
async fn exchange_head(
    client: &mut Counted,
    rr: &mut tokio::io::ReadHalf<upstream::BoxedIo>,
    rw: &mut tokio::io::WriteHalf<upstream::BoxedIo>,
    request: &[u8],
    leftover: &[u8],
    proxy_answers: bool,
) -> Result<Vec<u8>, upstream::UpstreamError> {
    use upstream::UpstreamError;
    rw.write_all(request).await?;
    rw.write_all(leftover).await?;
    let mut response = Vec::new();
    let mut from_proxy = [0u8; 4096];
    let mut from_client = [0u8; 4096];
    let mut client_open = true;
    while find_head_end(&response).is_none() {
        if response.len() > MAX_RESPONSE_HEAD {
            return Err(UpstreamError::Protocol);
        }
        // Без ограничения по времени: сайт вправе отвечать сколь угодно долго, это не сбой прокси.
        let (is_proxy, n) = tokio::select! {
            n = rr.read(&mut from_proxy) => (true, n?),
            n = client.read(&mut from_client), if client_open => (false, n?),
        };
        if is_proxy {
            if n == 0 {
                return Err(UpstreamError::Protocol);
            }
            response.extend_from_slice(&from_proxy[..n]);
        } else if n == 0 {
            client_open = false;
        } else {
            rw.write_all(&from_client[..n]).await?;
        }
    }
    match parse_status(&response) {
        Some((407, _)) if proxy_answers => Err(UpstreamError::Status(407)),
        Some(_) => Ok(response),
        None => Err(UpstreamError::Protocol),
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    fn upstream(kind: UpstreamKind, addr: SocketAddr, creds: Option<(&str, &str)>) -> UpstreamConfig {
        UpstreamConfig {
            proxy_id: 42,
            kind,
            host: addr.ip().to_string(),
            port: addr.port(),
            credentials: creds.map(|(u, p)| Credentials { username: u.into(), password: p.into() }),
            tls_roots: None,
        }
    }

    async fn open_tunnel(relay: SocketAddr, target: SocketAddr) -> (TcpStream, String) {
        let mut s = TcpStream::connect(relay).await.unwrap();
        s.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes()).await.unwrap();
        let head = String::from_utf8(read_head(&mut s).await).unwrap();
        (s, head)
    }

    async fn ping(mut s: TcpStream) {
        s.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    }

    async fn raw_request(relay: SocketAddr, request: &str) -> String {
        let mut s = TcpStream::connect(relay).await.unwrap();
        s.write_all(request.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Ответ браузеру просит закрыть соединение: ровно один `Connection: close` и `Proxy-Connection: close`.
    fn assert_closes_connection(response: &str) {
        let head = response.split("\r\n\r\n").next().unwrap_or_default();
        assert_eq!(head.matches("\r\nConnection: close").count(), 1, "{head}");
        assert!(head.contains("\r\nProxy-Connection: close"), "{head}");
    }

    #[tokio::test]
    async fn connect_through_http_upstream_with_auth() {
        let echo = spawn_echo().await;
        let (proxy, log) = spawn_http_proxy(Some("dTpw")).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, Some(("u", "p"))), None).await.unwrap();
        assert!(relay.addr().ip().is_loopback());
        let (s, head) = open_tunnel(relay.addr(), echo).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        ping(s).await;
        let seen = log.lock().unwrap()[0].clone();
        assert!(seen.starts_with(&format!("CONNECT {echo} HTTP/1.1")), "{seen}");
        assert!(seen.contains("Proxy-Authorization: Basic dTpw\r\n"));
    }

    #[tokio::test]
    async fn explicit_direct_connection_supports_http_and_connect_without_a_proxy() {
        let echo = spawn_echo().await;
        let relay = start(UpstreamConfig::direct(), None).await.unwrap();
        let (stream, head) = open_tunnel(relay.addr(), echo).await;
        assert!(head.starts_with("HTTP/1.1 200"));
        ping(stream).await;
        let (origin, hits) = spawn_origin(200, "direct-ok").await;
        let response = raw_request(relay.addr(), &format!("GET http://{origin}/direct?q=1 HTTP/1.1\r\nHost: {origin}\r\nProxy-Authorization: Basic browser-secret\r\n\r\n")).await;
        assert!(response.ends_with("direct-ok"));
        let request = hits.lock().unwrap()[0].clone();
        assert!(request.starts_with("GET /direct?q=1 HTTP/1.1"));
        assert!(!request.to_lowercase().contains("proxy-authorization"));
        assert_eq!(relay.stats().upstream_failures, 0);
        relay.shutdown().await;
    }

    #[tokio::test]
    async fn connect_through_socks5_with_auth() {
        let echo = spawn_echo().await;
        let (proxy, log) = spawn_socks5(Some(("u", "p"))).await;
        let relay = start(upstream(UpstreamKind::Socks5, proxy, Some(("u", "p"))), None).await.unwrap();
        let (s, head) = open_tunnel(relay.addr(), echo).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        ping(s).await;
        assert_eq!(log.lock().unwrap()[0], echo.to_string());
    }

    #[tokio::test]
    async fn connect_through_https_upstream() {
        let echo = spawn_echo().await;
        let (proxy, _log, roots) = spawn_https_proxy(Some("dTpw")).await;
        let mut cfg = upstream(UpstreamKind::Https, proxy, Some(("u", "p")));
        cfg.host = "localhost".into();
        cfg.tls_roots = Some(roots);
        let relay = start(cfg, None).await.unwrap();
        let (s, head) = open_tunnel(relay.addr(), echo).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        ping(s).await;
    }

    #[tokio::test]
    async fn plain_http_via_http_upstream_is_absolute_with_auth() {
        let (origin, _) = spawn_origin(200, "ok").await;
        let (proxy, log) = spawn_http_proxy(Some("dTpw")).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, Some(("u", "p"))), None).await.unwrap();
        let response = raw_request(
            relay.addr(),
            &format!(
                "GET http://{origin}/path?q HTTP/1.1\r\nHost: {origin}\r\nProxy-Connection: keep-alive\r\n\
                 Proxy-Authorization: Basic browser-secret\r\n\r\n"
            ),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200") && response.ends_with("ok"), "{response}");
        assert_closes_connection(&response);
        let seen = log.lock().unwrap()[0].clone();
        assert!(seen.starts_with(&format!("GET http://{origin}/path?q HTTP/1.1\r\n")), "{seen}");
        assert!(seen.contains("Proxy-Authorization: Basic dTpw\r\n"));
        assert!(!seen.contains("browser-secret") && !seen.contains("Proxy-Connection"));
        assert!(seen.contains("Connection: close\r\n"));
    }

    #[tokio::test]
    async fn plain_http_via_socks5_is_origin_form() {
        let (origin, seen) = spawn_origin(200, "ok").await;
        let (proxy, _) = spawn_socks5(None).await;
        let relay = start(upstream(UpstreamKind::Socks5, proxy, None), None).await.unwrap();
        let response = raw_request(
            relay.addr(),
            &format!("GET http://{origin}/path?q HTTP/1.1\r\nHost: {origin}\r\nProxy-Connection: keep-alive\r\n\r\n"),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200") && response.ends_with("ok"), "{response}");
        assert_closes_connection(&response);
        let head = seen.lock().unwrap()[0].clone();
        assert!(head.starts_with("GET /path?q HTTP/1.1\r\n"), "{head}");
        assert!(!head.contains("Proxy-"), "{head}");
    }

    #[tokio::test]
    async fn connect_reply_asks_browser_to_close() {
        let echo = spawn_echo().await;
        let (proxy, _) = spawn_http_proxy(None).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, None), None).await.unwrap();
        let (s, head) = open_tunnel(relay.addr(), echo).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_closes_connection(&head);
        ping(s).await;
    }

    #[tokio::test]
    async fn socks5_receives_target_by_name() {
        let echo = spawn_echo().await;
        let (proxy, log) = spawn_socks5(None).await;
        let relay = start(upstream(UpstreamKind::Socks5, proxy, None), None).await.unwrap();
        let target = format!("localhost:{}", echo.port());
        let mut s = TcpStream::connect(relay.addr()).await.unwrap();
        s.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes()).await.unwrap();
        let head = String::from_utf8(read_head(&mut s).await).unwrap();
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        ping(s).await;
        assert_eq!(log.lock().unwrap()[0], target, "имя цели резолвит прокси, а не прослойка");
    }

    #[tokio::test]
    async fn refused_upstream_gives_502_and_no_direct_connection() {
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        let (proxy, _) = spawn_http_proxy(Some("dTpw")).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, Some(("u", "wrong"))), None).await.unwrap();
        let (_, head) = open_tunnel(relay.addr(), target_addr).await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        assert!(tokio::time::timeout(Duration::from_millis(300), target.accept()).await.is_err(), "прямое соединение");
        assert_eq!(relay.stats().upstream_failures, 1);
    }

    #[tokio::test]
    async fn unreachable_upstream_turns_health_red_and_recovers() {
        let port = closed_port().await;
        let dead: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let echo = spawn_echo().await;
        let relay = start(upstream(UpstreamKind::Http, dead, None), None).await.unwrap();
        let health = relay.health();
        for _ in 0..UNREACHABLE_AFTER {
            let (_, head) = open_tunnel(relay.addr(), echo).await;
            assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        }
        assert_eq!(*health.borrow(), Health::Unreachable);
        let listener = TcpListener::bind(dead).await.unwrap();
        serve_http_proxy_on(listener, None);
        let (s, head) = open_tunnel(relay.addr(), echo).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        ping(s).await;
        assert_eq!(*health.borrow(), Health::Ok);
    }

    #[tokio::test]
    async fn log_has_target_but_never_password() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k7f2q9.jsonl");
        let echo = spawn_echo().await;
        let (proxy, _) = spawn_socks5(Some(("user", "s3cr3t-pass"))).await;
        let relay = start(upstream(UpstreamKind::Socks5, proxy, Some(("user", "s3cr3t-pass"))), Some(path.clone())).await.unwrap();
        let (s, _) = open_tunnel(relay.addr(), echo).await;
        ping(s).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&echo.to_string()) && text.contains(r#""proxy_id":42"#), "{text}");
        assert!(!text.contains("s3cr3t"), "{text}");
        let stats = relay.stats();
        assert!(stats.bytes_up >= 4 && stats.bytes_down >= 4, "{stats:?}");
        assert!(!format!("{:?}", upstream(UpstreamKind::Http, echo, Some(("u", "s3cr3t-pass")))).contains("s3cr3t"));
    }

    #[tokio::test]
    async fn bad_requests_are_answered_not_forwarded() {
        let (proxy, log) = spawn_http_proxy(None).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, None), None).await.unwrap();
        assert!(raw_request(relay.addr(), "GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.starts_with("HTTP/1.1 400"));
        let huge = format!("GET http://x/ HTTP/1.1\r\nX: {}\r\n", "a".repeat(20_000));
        assert!(raw_request(relay.addr(), &huge).await.starts_with("HTTP/1.1 431"));
        assert!(log.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn shutdown_closes_listener() {
        let (proxy, _) = spawn_http_proxy(None).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, None), None).await.unwrap();
        let addr = relay.addr();
        relay.shutdown().await;
        assert!(port_closes(addr).await);
    }

    #[tokio::test]
    async fn dropped_handle_closes_listener() {
        let (proxy, _) = spawn_http_proxy(None).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, None), None).await.unwrap();
        let addr = relay.addr();
        drop(relay);
        let mut closed = false;
        for _ in 0..20 {
            if TcpStream::connect(addr).await.is_err() {
                closed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(closed, "слушатель остался открытым после drop");
    }

    #[tokio::test]
    async fn accept_pause_ends_on_cancel() {
        let cancel = CancellationToken::new();
        assert!(pause_unless_cancelled(&cancel, Duration::from_millis(10)).await, "без отмены пауза доходит до конца");

        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            trigger.cancel();
        });
        let began = Instant::now();
        assert!(!pause_unless_cancelled(&cancel, Duration::from_secs(10)).await, "отмена прерывает паузу");
        assert!(began.elapsed() < Duration::from_secs(1), "{:?}", began.elapsed());

        let began = Instant::now();
        assert!(!pause_unless_cancelled(&cancel, ACCEPT_BACKOFF).await);
        assert!(began.elapsed() < ACCEPT_BACKOFF, "отменённая прослойка не ждёт паузу: {:?}", began.elapsed());
    }

    #[tokio::test]
    async fn stats_grow_while_tunnel_is_open() {
        let echo = spawn_echo().await;
        let (proxy, _) = spawn_http_proxy(None).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, None), None).await.unwrap();
        let (mut s, head) = open_tunnel(relay.addr(), echo).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        s.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).await.unwrap();
        let stats = relay.stats();
        assert!(stats.bytes_up >= 4 && stats.bytes_down >= 4, "{stats:?}");
        drop(s);
    }

    #[tokio::test]
    async fn aborted_tunnel_still_logs_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k7f2q9.jsonl");
        let echo = spawn_echo().await;
        let (proxy, _) = spawn_http_proxy(None).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, None), Some(path.clone())).await.unwrap();
        let (mut s, _) = open_tunnel(relay.addr(), echo).await;
        s.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).await.unwrap();
        #[allow(deprecated)]
        s.set_linger(Some(Duration::ZERO)).unwrap();
        drop(s);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let text = std::fs::read_to_string(&path).unwrap();
        let record: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert!(record["bytes_up"].as_u64().unwrap() >= 4 && record["bytes_down"].as_u64().unwrap() >= 4, "{text}");
    }

    #[tokio::test]
    async fn ipv6_connect_is_bracketed_toward_upstream() {
        let (proxy, log) = spawn_http_proxy(None).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, None), None).await.unwrap();
        let mut s = TcpStream::connect(relay.addr()).await.unwrap();
        s.write_all(b"CONNECT [::1]:9 HTTP/1.1\r\nHost: [::1]:9\r\n\r\n").await.unwrap();
        let _ = read_head(&mut s).await;
        let seen = log.lock().unwrap()[0].clone();
        assert!(seen.starts_with("CONNECT [::1]:9 HTTP/1.1\r\nHost: [::1]:9\r\n"), "{seen}");
    }

    #[tokio::test]
    async fn upstream_407_on_absolute_form_becomes_502() {
        let (origin, _) = spawn_origin(200, "ok").await;
        let (proxy, _) = spawn_http_proxy(Some("dTpw")).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, Some(("u", "wrong"))), None).await.unwrap();
        let response = raw_request(relay.addr(), &format!("GET http://{origin}/ HTTP/1.1\r\nHost: {origin}\r\n\r\n")).await;
        assert!(response.starts_with("HTTP/1.1 502"), "{response}");
        assert!(!response.contains("407") && !response.contains("Proxy-Authenticate"), "{response}");
        assert_eq!(relay.stats().upstream_failures, 1);
    }

    /// Сайт отвечает заранее заданными байтами `response` (заголовок может быть не UTF-8 и очень длинным).
    async fn spawn_raw_origin(response: Vec<u8>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let response = response.clone();
                tokio::spawn(async move {
                    let _ = read_head(&mut s).await;
                    let _ = s.write_all(&response).await;
                });
            }
        });
        origin
    }

    async fn raw_request_bytes(relay: SocketAddr, request: &str) -> Vec<u8> {
        let mut s = TcpStream::connect(relay).await.unwrap();
        s.write_all(request.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
        out
    }

    async fn relay_for(kind: UpstreamKind) -> RelayHandle {
        let proxy = match kind {
            UpstreamKind::Socks5 => spawn_socks5(None).await.0,
            _ => spawn_http_proxy(None).await.0,
        };
        start(upstream(kind, proxy, None), None).await.unwrap()
    }

    #[tokio::test]
    async fn non_utf8_response_header_reaches_browser_intact() {
        for kind in [UpstreamKind::Socks5, UpstreamKind::Http] {
            let origin = spawn_raw_origin(
                b"HTTP/1.1 200 OK\r\nContent-Disposition: attachment; filename=\"caf\xe9\"\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec(),
            )
            .await;
            let relay = relay_for(kind).await;
            let out = raw_request_bytes(relay.addr(), &format!("GET http://{origin}/ HTTP/1.1\r\nHost: {origin}\r\n\r\n")).await;
            assert!(out.starts_with(b"HTTP/1.1 200"), "{kind:?}: {}", String::from_utf8_lossy(&out));
            assert!(out.windows(10).any(|w| w == b"filename=\"" ) && out.windows(5).any(|w| w == b"caf\xe9\""), "{kind:?}: байт искажён");
            let text = String::from_utf8_lossy(&out).into_owned();
            assert_closes_connection(&text);
            assert!(out.ends_with(b"ok"));
            assert_eq!(relay.stats().upstream_failures, 0, "{kind:?}");
        }
    }

    #[tokio::test]
    async fn large_response_head_passes() {
        for kind in [UpstreamKind::Socks5, UpstreamKind::Http] {
            let mut response = b"HTTP/1.1 200 OK\r\nX-Big: ".to_vec();
            response.extend(std::iter::repeat_n(b'a', 20 * 1024));
            response.extend_from_slice(b"\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            let origin = spawn_raw_origin(response).await;
            let relay = relay_for(kind).await;
            let out = raw_request(relay.addr(), &format!("GET http://{origin}/ HTTP/1.1\r\nHost: {origin}\r\n\r\n")).await;
            assert!(out.starts_with("HTTP/1.1 200") && out.ends_with("ok"), "{kind:?}: {}", &out[..60.min(out.len())]);
            assert_eq!(relay.stats().upstream_failures, 0, "{kind:?}");
        }
    }

    #[tokio::test]
    async fn slow_origin_response_is_not_a_proxy_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let _ = read_head(&mut s).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            let _ = s.write_all(b"HTTP/1.1 200 X\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
        });
        let (proxy, _) = spawn_http_proxy(None).await;
        let relay = start(upstream(UpstreamKind::Http, proxy, None), None).await.unwrap();
        let response = raw_request(relay.addr(), &format!("GET http://{origin}/ HTTP/1.1\r\nHost: {origin}\r\n\r\n")).await;
        assert!(response.starts_with("HTTP/1.1 200") && response.ends_with("ok"), "{response}");
        assert_eq!(relay.stats().upstream_failures, 0);
        assert_eq!(*relay.health().borrow(), Health::Ok);
    }

    #[test]
    fn basic_auth_header() {
        let cfg = upstream(UpstreamKind::Socks5, "127.0.0.1:1080".parse().unwrap(), Some(("u", "p")));
        assert_eq!(cfg.basic_auth().as_deref(), Some("Basic dTpw"));
    }

    #[test]
    fn upstream_debug_hides_login_and_password() {
        let cfg = upstream(UpstreamKind::Http, "127.0.0.1:8080".parse().unwrap(), Some(("alice-login", "s3cr3t")));
        let text = format!("{cfg:?}");
        assert!(!text.contains("alice-login") && !text.contains("s3cr3t"), "{text}");
    }
}
