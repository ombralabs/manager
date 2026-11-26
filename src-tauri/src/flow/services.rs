//! Сеть для сторонних сервисов автоматизации (почта, OpenAI, 2Captcha, Google Sheets): напрямую или через один прокси,
//! выбранный для всего менеджера в «Настройках» (`services.proxy_id`). Профили и `data.download` этот прокси не используют.
use crate::relay::{self as upstream, BoxedIo};
use crate::store::Store;
use crate::supervisor::gateway::upstream_config;
use std::time::Duration;
use tokio::net::TcpStream;

/// Ключ настройки: номер записи прокси; нет ключа или пусто — сеть компьютера.
pub const PROXY_KEY: &str = "services.proxy_id";
/// Предел на одно обращение к сервису.
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// Прокси для сервисов; `None` — сеть компьютера (ключа нет или он пуст). Испорченное значение — ошибка:
/// трафик не должен молча уйти напрямую.
pub fn proxy_id(store: &Store) -> Result<Option<i64>, String> {
    let raw = store.get_setting(PROXY_KEY).map_err(|e| e.to_string())?;
    match raw.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) => v.parse().map(Some).map_err(|_| "настройка прокси для сервисов повреждена".to_string()),
    }
}

/// Запоминает выбор; несуществующий прокси — `NotFound` (текст ошибки хранилища).
pub fn set_proxy_id(store: &Store, id: Option<i64>) -> Result<(), crate::store::StoreError> {
    match id {
        Some(id) => {
            store.proxy_credentials(id)?;
            store.set_setting(PROXY_KEY, &id.to_string())
        }
        None => store.set_setting(PROXY_KEY, ""),
    }
}

const NOT_FOUND: &str = "прокси для сервисов не найден";

/// Прокси из настройки с расшифрованным паролем; `None` — настройки нет.
pub(crate) fn chosen(store: &Store) -> Result<Option<upstream::UpstreamConfig>, String> {
    let Some(id) = proxy_id(store)? else { return Ok(None) };
    match store.proxy_credentials(id) {
        Ok(creds) => Ok(Some(upstream_config(&creds))),
        Err(crate::store::StoreError::NotFound(_)) => Err(NOT_FOUND.into()),
        Err(e) => Err(e.to_string()),
    }
}

/// TCP до `host:port` напрямую или через прокси для сервисов. Ошибка называет адрес прокси, без логина и пароля.
pub async fn connect(store: &Store, host: &str, port: u16) -> Result<BoxedIo, String> {
    let Some(cfg) = chosen(store)? else {
        let tcp = tokio::time::timeout(TIMEOUT, TcpStream::connect((host, port)))
            .await
            .map_err(|_| format!("таймаут подключения к {host}:{port}"))?
            .map_err(|e| format!("подключение к {host}:{port}: {e}"))?;
        return Ok(Box::new(tcp));
    };
    upstream::connect_upstream(&cfg, host, port)
        .await
        .map_err(|e| format!("прокси для сервисов {}:{}: {e}", cfg.host, cfg.port))
}

/// HTTP-клиент с тем же прокси (http, https, socks5 с разрешением имён на стороне прокси) и таймаутом 60 с.
pub fn http(store: &Store) -> Result<reqwest::Client, String> {
    // «Сеть компьютера» — напрямую: переменные окружения и системный прокси не учитываются, как в `connect`.
    let mut builder = reqwest::Client::builder().timeout(TIMEOUT).no_proxy();
    if let Some(cfg) = chosen(store)? {
        let url = proxy_url(&cfg)?;
        builder = builder.proxy(reqwest::Proxy::all(url).map_err(|_| bad(&cfg))?);
    }
    builder.build().map_err(|e| format!("HTTP-клиент: {e}"))
}

fn bad(cfg: &upstream::UpstreamConfig) -> String {
    format!("прокси для сервисов {}:{}: неверный адрес", cfg.host, cfg.port)
}

/// Адрес прокси для reqwest: схема по виду, логин и пароль в адресе.
fn proxy_url(cfg: &upstream::UpstreamConfig) -> Result<reqwest::Url, String> {
    let scheme = match cfg.kind {
        upstream::UpstreamKind::Http => "http",
        upstream::UpstreamKind::Https => "https",
        upstream::UpstreamKind::Socks5 => "socks5h",
        upstream::UpstreamKind::Direct => return Err("прямому соединению не нужен адрес прокси".into()),
    };
    let mut url = reqwest::Url::parse(&format!("{scheme}://placeholder")).map_err(|_| bad(cfg))?;
    // IPv6 из хранилища — без скобок: `set_host` обрезал бы его по первому «:».
    match cfg.host.parse::<std::net::IpAddr>() {
        Ok(ip) => url.set_ip_host(ip),
        Err(_) => url.set_host(Some(&cfg.host)).map_err(|_| ()),
    }
    .map_err(|_| bad(cfg))?;
    url.set_port(Some(cfg.port)).map_err(|_| bad(cfg))?;
    // Сеттеры кодируют спецсимволы, но не «%», а reqwest раскодирует логин и пароль: «%» — заранее «%25».
    if let Some(c) = &cfg.credentials {
        url.set_username(&c.username.replace('%', "%25")).map_err(|_| bad(cfg))?;
        url.set_password(Some(&c.password.replace('%', "%25"))).map_err(|_| bad(cfg))?;
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::proxies::{ProxyInput, ProxyKind};
    use crate::store::secrets::{Secret, SecretField};
    use crate::store::tests::memory_store;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn listener() -> (TcpListener, u16) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        (l, port)
    }

    fn add_proxy(store: &Store, kind: ProxyKind, port: u16, auth: bool) -> i64 {
        store
            .create_proxy(&ProxyInput {
                kind,
                host: "127.0.0.1".into(),
                port,
                username: auth.then(|| "user1".into()),
                password: if auth { SecretField::Set(Secret::new("s3cret")) } else { SecretField::Keep },
                note: String::new(),
                enabled: true,
            })
            .unwrap()
            .id
    }

    async fn read_head(s: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        while !buf.ends_with(b"\r\n\r\n") {
            buf.push(s.read_u8().await.unwrap());
        }
        String::from_utf8(buf).unwrap()
    }

    /// Подставной CONNECT-прокси: отдаёт голову запроса через канал, после «200» пишет `via-proxy`.
    async fn fake_http_proxy() -> (u16, tokio::sync::oneshot::Receiver<String>) {
        let (l, port) = listener().await;
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let head = read_head(&mut s).await;
            s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await.unwrap();
            // Туннельные данные идут отдельной порцией: прокси не шлёт их вместе с ответом.
            tokio::time::sleep(Duration::from_millis(100)).await;
            s.write_all(b"via-proxy").await.unwrap();
            let _ = tx.send(head);
        });
        (port, rx)
    }

    #[tokio::test]
    async fn without_setting_connects_directly() {
        let store = memory_store();
        let (l, port) = listener().await;
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            s.write_all(b"direct").await.unwrap();
        });
        let mut io = connect(&store, "127.0.0.1", port).await.unwrap();
        let mut got = String::new();
        io.read_to_string(&mut got).await.unwrap();
        assert_eq!(got, "direct");
    }

    #[tokio::test]
    async fn goes_through_http_proxy() {
        let store = memory_store();
        let (port, head) = fake_http_proxy().await;
        let id = add_proxy(&store, ProxyKind::Http, port, true);
        set_proxy_id(&store, Some(id)).unwrap();
        let mut io = connect(&store, "mail.example.test", 993).await.unwrap();
        let mut got = [0u8; 9];
        io.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"via-proxy");
        let head = head.await.unwrap();
        assert!(head.starts_with("CONNECT mail.example.test:993 HTTP/1.1"), "{head}");
        assert!(head.contains("Proxy-Authorization: Basic "), "{head}");
    }

    #[tokio::test]
    async fn goes_through_socks5_proxy() {
        let store = memory_store();
        let (l, port) = listener().await;
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut hello = [0u8; 2];
            s.read_exact(&mut hello).await.unwrap();
            let mut methods = vec![0u8; hello[1] as usize];
            s.read_exact(&mut methods).await.unwrap();
            s.write_all(&[5, 0]).await.unwrap();
            let mut head = [0u8; 5];
            s.read_exact(&mut head).await.unwrap();
            assert_eq!(head[3], 3, "имя цели уходит прокси");
            let mut rest = vec![0u8; head[4] as usize + 2];
            s.read_exact(&mut rest).await.unwrap();
            let name = String::from_utf8(rest[..head[4] as usize].to_vec()).unwrap();
            let port = u16::from_be_bytes([rest[rest.len() - 2], rest[rest.len() - 1]]);
            s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
            s.write_all(b"via-socks").await.unwrap();
            let _ = tx.send((name, port));
        });
        let id = add_proxy(&store, ProxyKind::Socks5, port, false);
        set_proxy_id(&store, Some(id)).unwrap();
        let mut io = connect(&store, "mail.example.test", 993).await.unwrap();
        let mut got = [0u8; 9];
        io.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"via-socks");
        assert_eq!(rx.await.unwrap(), ("mail.example.test".to_string(), 993));
    }

    #[tokio::test]
    async fn deleted_proxy_is_an_error() {
        let store = memory_store();
        let id = add_proxy(&store, ProxyKind::Http, 9, false);
        set_proxy_id(&store, Some(id)).unwrap();
        store.delete_proxy(id).unwrap();
        assert_eq!(connect(&store, "127.0.0.1", 1).await.err().unwrap(), "прокси для сервисов не найден");
        assert_eq!(http(&store).err().unwrap(), "прокси для сервисов не найден");
        assert!(set_proxy_id(&store, Some(id)).is_err(), "выбрать несуществующий нельзя");
    }

    #[tokio::test]
    async fn dead_proxy_error_names_address_not_password() {
        let store = memory_store();
        let (l, port) = listener().await;
        drop(l);
        let id = add_proxy(&store, ProxyKind::Http, port, true);
        set_proxy_id(&store, Some(id)).unwrap();
        let err = connect(&store, "mail.example.test", 993).await.err().unwrap();
        assert!(err.contains(&format!("127.0.0.1:{port}")), "{err}");
        assert!(!err.contains("s3cret") && !err.contains("user1"), "{err}");
    }

    #[tokio::test]
    async fn corrupted_setting_fails_closed() {
        let store = memory_store();
        store.set_setting(PROXY_KEY, "abc").unwrap();
        let (l, port) = listener().await;
        let want = "настройка прокси для сервисов повреждена";
        assert_eq!(proxy_id(&store).err().unwrap(), want);
        assert_eq!(connect(&store, "127.0.0.1", port).await.err().unwrap(), want);
        assert_eq!(http(&store).err().unwrap(), want);
        assert!(tokio::time::timeout(Duration::from_millis(100), l.accept()).await.is_err(), "трафика нет");
        set_proxy_id(&store, None).unwrap();
        assert_eq!(proxy_id(&store).unwrap(), None, "сброс на «Нет» перезаписывает повреждённое значение");
        store.set_setting(PROXY_KEY, "  ").unwrap();
        assert_eq!(proxy_id(&store).unwrap(), None);
    }

    #[tokio::test]
    async fn http_client_uses_proxy() {
        let store = memory_store();
        let (l, port) = listener().await;
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let head = read_head(&mut s).await;
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
            let _ = tx.send(head);
        });
        let id = add_proxy(&store, ProxyKind::Http, port, true);
        set_proxy_id(&store, Some(id)).unwrap();
        let body = http(&store).unwrap().get("http://api.example.test/v1").send().await.unwrap().text().await.unwrap();
        assert_eq!(body, "ok");
        let head = rx.await.unwrap();
        assert!(head.starts_with("GET http://api.example.test/v1 HTTP/1.1"), "{head}");
        assert!(head.to_lowercase().contains("proxy-authorization: basic "), "{head}");
    }

    #[tokio::test]
    async fn proxy_credentials_reach_the_proxy_as_typed() {
        use base64::Engine as _;
        let store = memory_store();
        let (l, port) = listener().await;
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let head = read_head(&mut s).await;
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
            let _ = tx.send(head);
        });
        let (user, password) = ("u%3Ar@x", "p%40ss:@/#?%");
        let id = store
            .create_proxy(&ProxyInput {
                kind: ProxyKind::Http,
                host: "127.0.0.1".into(),
                port,
                username: Some(user.into()),
                password: SecretField::Set(Secret::new(password)),
                note: String::new(),
                enabled: true,
            })
            .unwrap()
            .id;
        set_proxy_id(&store, Some(id)).unwrap();
        http(&store).unwrap().get("http://api.example.test/v1").send().await.unwrap();
        let head = rx.await.unwrap();
        let basic = head.lines().find_map(|line| line.split_once(": ").filter(|(name, _)| name.eq_ignore_ascii_case("proxy-authorization")).map(|(_, v)| v.to_string()));
        let sent = basic.as_deref().and_then(|v| v.strip_prefix("Basic ")).map(|b| base64::engine::general_purpose::STANDARD.decode(b).unwrap());
        assert_eq!(sent.map(|b| String::from_utf8(b).unwrap()), Some(format!("{user}:{password}")), "{head}");
    }

    #[test]
    fn proxy_url_brackets_ipv6_hosts() {
        let cfg = |host: &str| upstream::UpstreamConfig { proxy_id: 1, kind: upstream::UpstreamKind::Http, host: host.into(), port: 8080, credentials: None, tls_roots: None };
        assert_eq!(proxy_url(&cfg("::1")).unwrap().as_str(), "http://[::1]:8080/");
        assert_eq!(proxy_url(&cfg("2001:db8::1")).unwrap().as_str(), "http://[2001:db8::1]:8080/");
        assert_eq!(proxy_url(&cfg("[2001:db8::1]")).unwrap().as_str(), "http://[2001:db8::1]:8080/");
        assert_eq!(proxy_url(&cfg("127.0.0.1")).unwrap().as_str(), "http://127.0.0.1:8080/");
        assert_eq!(proxy_url(&cfg("proxy.test")).unwrap().as_str(), "http://proxy.test:8080/");
    }

    #[test]
    fn http_client_builds_for_every_kind_and_without_proxy() {
        let store = memory_store();
        assert!(http(&store).is_ok());
        for kind in [ProxyKind::Http, ProxyKind::Https, ProxyKind::Socks5] {
            let id = add_proxy(&store, kind, 1080, true);
            set_proxy_id(&store, Some(id)).unwrap();
            http(&store).unwrap();
        }
        set_proxy_id(&store, None).unwrap();
        assert_eq!(proxy_id(&store).unwrap(), None);
    }
}
