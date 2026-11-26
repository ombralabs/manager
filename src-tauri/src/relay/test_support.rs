//! Подставные серверы для тестов прослойки и геоданных.
use super::http::{Target, find_head_end, parse_head, parse_target};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub type Log = Arc<Mutex<Vec<String>>>;

pub async fn read_head<S: AsyncRead + Unpin>(s: &mut S) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while find_head_end(&buf).is_none() {
        let n = s.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    buf
}

pub async fn spawn_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

/// Сайт: пишет заголовок запроса в журнал и отвечает `status` с телом `body`.
pub async fn spawn_origin(status: u16, body: &'static str) -> (SocketAddr, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Log = Arc::default();
    let seen = log.clone();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            let seen = seen.clone();
            tokio::spawn(async move {
                let head = read_head(&mut s).await;
                seen.lock().unwrap().push(String::from_utf8_lossy(&head).into_owned());
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(response.as_bytes()).await;
            });
        }
    });
    (addr, log)
}

/// Сайт с HTML-страницей `body` на любой запрос (для проб с формами).
pub async fn spawn_html(body: &'static str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                read_head(&mut s).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(response.as_bytes()).await;
            });
        }
    });
    addr
}

/// HTTP-прокси: при заданном `auth` требует `Proxy-Authorization: Basic <auth>`; CONNECT и абсолютные запросы пересылает.
pub async fn spawn_http_proxy(auth: Option<&'static str>) -> (SocketAddr, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (addr, serve_http_proxy_on(listener, auth))
}

pub fn serve_http_proxy_on(listener: TcpListener, auth: Option<&'static str>) -> Log {
    let log: Log = Arc::default();
    let seen = log.clone();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            tokio::spawn(serve_http_proxy(s, auth, seen.clone()));
        }
    });
    log
}

pub async fn serve_http_proxy<S: AsyncRead + AsyncWrite + Unpin>(mut s: S, auth: Option<&'static str>, log: Log) {
    let buf = read_head(&mut s).await;
    let text = String::from_utf8_lossy(&buf).into_owned();
    log.lock().unwrap().push(text.clone());
    if let Some(expected) = auth
        && !text.contains(&format!("Proxy-Authorization: Basic {expected}\r\n"))
    {
        let _ = s.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n").await;
        return;
    }
    let Ok(Some(head)) = parse_head(&buf) else { return };
    let (host, port) = match parse_target(&head.method, &head.target) {
        Ok(Target::Authority { host, port }) | Ok(Target::Absolute { host, port, .. }) => (host, port),
        Err(_) => return,
    };
    let Ok(mut remote) = TcpStream::connect((host.as_str(), port)).await else {
        let _ = s.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n").await;
        return;
    };
    if head.method == "CONNECT" {
        let _ = s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await;
    } else {
        let _ = remote.write_all(&buf).await;
    }
    let _ = tokio::io::copy_bidirectional(&mut s, &mut remote).await;
}

/// SOCKS5 с авторизацией по логину и паролю (или без неё); в журнал пишет запрошенную цель.
pub async fn spawn_socks5(creds: Option<(&'static str, &'static str)>) -> (SocketAddr, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Log = Arc::default();
    let seen = log.clone();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            tokio::spawn(serve_socks5(s, creds, seen.clone()));
        }
    });
    (addr, log)
}

async fn serve_socks5(mut s: TcpStream, creds: Option<(&'static str, &'static str)>, log: Log) -> Option<()> {
    let mut greeting = [0u8; 2];
    s.read_exact(&mut greeting).await.ok()?;
    let mut methods = vec![0u8; greeting[1] as usize];
    s.read_exact(&mut methods).await.ok()?;
    let wanted = if creds.is_some() { 0x02 } else { 0x00 };
    if !methods.contains(&wanted) {
        s.write_all(&[5, 0xff]).await.ok()?;
        return None;
    }
    s.write_all(&[5, wanted]).await.ok()?;
    if let Some((user, pass)) = creds {
        let mut head = [0u8; 2];
        s.read_exact(&mut head).await.ok()?;
        let mut u = vec![0u8; head[1] as usize];
        s.read_exact(&mut u).await.ok()?;
        let mut plen = [0u8; 1];
        s.read_exact(&mut plen).await.ok()?;
        let mut p = vec![0u8; plen[0] as usize];
        s.read_exact(&mut p).await.ok()?;
        let ok = u == user.as_bytes() && p == pass.as_bytes();
        s.write_all(&[1, if ok { 0 } else { 1 }]).await.ok()?;
        if !ok {
            return None;
        }
    }
    let mut request = [0u8; 4];
    s.read_exact(&mut request).await.ok()?;
    let host = match request[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await.ok()?;
            Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await.ok()?;
            let mut name = vec![0u8; len[0] as usize];
            s.read_exact(&mut name).await.ok()?;
            String::from_utf8(name).ok()?
        }
        4 => {
            let mut a = [0u8; 16];
            s.read_exact(&mut a).await.ok()?;
            Ipv6Addr::from(a).to_string()
        }
        _ => return None,
    };
    let mut port = [0u8; 2];
    s.read_exact(&mut port).await.ok()?;
    let port = u16::from_be_bytes(port);
    log.lock().unwrap().push(format!("{host}:{port}"));
    let Ok(mut remote) = TcpStream::connect((host.as_str(), port)).await else {
        s.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok()?;
        return None;
    };
    s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok()?;
    let _ = tokio::io::copy_bidirectional(&mut s, &mut remote).await;
    Some(())
}

/// HTTPS-прокси с самоподписанным сертификатом на `localhost`; возвращает корни для клиента.
pub async fn spawn_https_proxy(auth: Option<&'static str>) -> (SocketAddr, Log, Arc<rustls::RootCertStore>) {
    let rcgen::CertifiedKey { cert, key_pair } = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_der = cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(key_pair.serialize_der().into());
    let config = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(vec![cert_der.clone()], key).unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert_der).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Log = Arc::default();
    let seen = log.clone();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    serve_http_proxy(tls, auth, seen).await;
                }
            });
        }
    });
    (addr, log, Arc::new(roots))
}

/// Ждёт, пока `addr` перестанет принимать соединения, — не дольше 2 с; `false`, если так и не перестал.
/// Разовая попытка соединиться сразу после закрытия слушателя ненадёжна: на macOS новый процесс, который в этот момент
/// запускается из соседнего потока (`posix_spawn`, например `ps` в других тестах), держит копии всех дескрипторов,
/// пока его `exec` не закроет помеченные `FD_CLOEXEC`. Всё это время ядро ещё принимает соединения на закрытый порт.
pub async fn port_closes(addr: SocketAddr) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        // Соединение ограничено по времени: проверка не зависает, даже если порт не отвечает.
        if let Ok(Err(_)) = tokio::time::timeout(Duration::from_millis(200), TcpStream::connect(addr)).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Свободный порт, на котором сейчас никто не слушает.
pub async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

/// HTTP-прокси с авторизацией для живого теста. Имена `*.ombra.test` ведут на `origin`,
/// остальное получает 502: Chromium не ходит через прокси на 127.0.0.1, а тест не должен выходить в сеть.
pub async fn spawn_routed_proxy(auth: &'static str, origin: SocketAddr) -> (SocketAddr, Log) {
    spawn_routed_proxy_with(auth, origin, Vec::new()).await
}

/// Как `spawn_routed_proxy`, плюс точные имена хостов, которые ведут на свои адреса
/// (например, `www.adspower.io` — на локальный TLS-сайт для примера AdsPower).
pub async fn spawn_routed_proxy_with(
    auth: &'static str,
    origin: SocketAddr,
    hosts: Vec<(&'static str, SocketAddr)>,
) -> (SocketAddr, Log) {
    let (addr, log, _) = spawn_routed(auth, origin, hosts).await;
    (addr, log)
}

/// Как `spawn_routed_proxy`, но с переключателем: пока он выставлен, принятые соединения сразу закрываются — прокси «упал».
pub async fn spawn_switchable_routed_proxy(auth: &'static str, origin: SocketAddr) -> (SocketAddr, Log, Arc<AtomicBool>) {
    spawn_routed(auth, origin, Vec::new()).await
}

async fn spawn_routed(
    auth: &'static str,
    origin: SocketAddr,
    hosts: Vec<(&'static str, SocketAddr)>,
) -> (SocketAddr, Log, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Log = Arc::default();
    let down = Arc::new(AtomicBool::new(false));
    let hosts = Arc::new(hosts);
    let (seen, flag) = (log.clone(), down.clone());
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            if flag.load(Ordering::SeqCst) {
                drop(stream);
                continue;
            }
            tokio::spawn(serve_routed(stream, auth, origin, hosts.clone(), seen.clone()));
        }
    });
    (addr, log, down)
}

async fn serve_routed(
    mut s: TcpStream,
    auth: &'static str,
    origin: SocketAddr,
    hosts: Arc<Vec<(&'static str, SocketAddr)>>,
    log: Log,
) {
    let buf = read_head(&mut s).await;
    let text = String::from_utf8_lossy(&buf).into_owned();
    log.lock().unwrap().push(text.clone());
    if !text.contains(&format!("Proxy-Authorization: Basic {auth}\r\n")) {
        let _ = s
            .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\nContent-Length: 0\r\n\r\n")
            .await;
        return;
    }
    let Ok(Some(head)) = parse_head(&buf) else { return };
    let host = match parse_target(&head.method, &head.target) {
        Ok(Target::Authority { host, .. }) | Ok(Target::Absolute { host, .. }) => host,
        Err(_) => return,
    };
    let target = if host.ends_with(".ombra.test") {
        Some(origin)
    } else {
        hosts.iter().find(|(name, _)| *name == host).map(|(_, addr)| *addr)
    };
    let Some(target) = target else {
        let _ = s.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n").await;
        return;
    };
    let Ok(mut remote) = TcpStream::connect(target).await else { return };
    if head.method == "CONNECT" {
        let _ = s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await;
    } else {
        let _ = remote.write_all(&buf).await;
    }
    let _ = tokio::io::copy_bidirectional(&mut s, &mut remote).await;
}

/// HTTP-прокси, который на любой запрос отвечает кодом `status` (например, 403 вместо 407).
pub async fn spawn_status_proxy(status: u16) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else { continue };
            tokio::spawn(async move {
                let _ = read_head(&mut s).await;
                let _ = s.write_all(format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\n\r\n").as_bytes()).await;
            });
        }
    });
    addr
}

/// SOCKS5 без авторизации, который на любой запрос отвечает кодом `reply` (1 — «general failure»).
pub async fn spawn_failing_socks5(reply: u8) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = listener.accept().await else { continue };
            tokio::spawn(answer_socks5(s, reply));
        }
    });
    addr
}

async fn answer_socks5(mut s: TcpStream, reply: u8) -> Option<()> {
    let mut greeting = [0u8; 2];
    s.read_exact(&mut greeting).await.ok()?;
    let mut methods = vec![0u8; greeting[1] as usize];
    s.read_exact(&mut methods).await.ok()?;
    s.write_all(&[5, 0]).await.ok()?;
    let mut request = [0u8; 4];
    s.read_exact(&mut request).await.ok()?;
    let address_len = match request[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await.ok()?;
            len[0] as usize
        }
        _ => return None,
    };
    let mut rest = vec![0u8; address_len + 2];
    s.read_exact(&mut rest).await.ok()?;
    s.write_all(&[5, reply, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok()?;
    Some(())
}

/// HTTP-прокси с авторизацией `user:pass`, который можно «уронить»: пока флаг выставлен, принятые соединения сразу закрываются.
pub async fn switchable_proxy() -> (SocketAddr, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let down = Arc::new(AtomicBool::new(false));
    let flag = down.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            if flag.load(Ordering::SeqCst) {
                drop(stream);
                continue;
            }
            tokio::spawn(serve_http_proxy(stream, Some("dXNlcjpwYXNz"), Arc::default()));
        }
    });
    (addr, down)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn routed_proxy_sends_listed_host_to_its_address() {
        let echo = spawn_echo().await;
        let (origin, _) = spawn_origin(200, "ok").await;
        let (proxy, log) = spawn_routed_proxy_with("dXNlcjpwYXNz", origin, vec![("www.adspower.io", echo)]).await;

        let mut tunnel = TcpStream::connect(proxy).await.unwrap();
        tunnel
            .write_all(b"CONNECT www.adspower.io:443 HTTP/1.1\r\nHost: www.adspower.io:443\r\nProxy-Authorization: Basic dXNlcjpwYXNz\r\n\r\n")
            .await
            .unwrap();
        let head = String::from_utf8(read_head(&mut tunnel).await).unwrap();
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        tunnel.write_all(b"ping").await.unwrap();
        let mut back = [0u8; 4];
        tunnel.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"ping", "туннель ведёт на адрес из списка");

        let mut other = TcpStream::connect(proxy).await.unwrap();
        other
            .write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Basic dXNlcjpwYXNz\r\n\r\n")
            .await
            .unwrap();
        let head = String::from_utf8(read_head(&mut other).await).unwrap();
        assert!(head.starts_with("HTTP/1.1 502"), "прочие имена по-прежнему не выпускаются: {head}");
        assert!(log.lock().unwrap().iter().any(|h| h.starts_with("CONNECT www.adspower.io:443")));
    }
}
