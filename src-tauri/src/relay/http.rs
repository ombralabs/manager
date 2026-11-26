use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

pub const MAX_HEAD: usize = 16 * 1024;
/// Заголовок ответа сайта не ограничен запросным лимитом: Chrome принимает до 256 КиБ.
pub const MAX_RESPONSE_HEAD: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    pub target: String,
    pub version: String,
    pub headers: Vec<(String, String)>,
    pub len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Authority { host: String, port: u16 },
    Absolute { host: String, port: u16, path: String },
}

pub fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Итог `read_head`.
pub enum Head {
    /// Заголовок прочитан (в буфере может оказаться и начало тела).
    Done(Vec<u8>),
    /// Поток закрылся раньше конца заголовка.
    Eof,
    /// Заголовок длиннее `max`.
    TooLarge,
}

/// Читает из `r` до `\r\n\r\n`; ошибки чтения отдаёт как есть.
pub async fn read_head<R: AsyncRead + Unpin>(r: &mut R, max: usize) -> io::Result<Head> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    while find_head_end(&buf).is_none() {
        if buf.len() > max {
            return Ok(Head::TooLarge);
        }
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            return Ok(Head::Eof);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(Head::Done(buf))
}

/// `Ok(None)` — заголовок ещё не дочитан.
pub fn parse_head(buf: &[u8]) -> Result<Option<RequestHead>, &'static str> {
    let Some(end) = find_head_end(buf) else {
        return if buf.len() > MAX_HEAD { Err("заголовок слишком длинный") } else { Ok(None) };
    };
    if end > MAX_HEAD {
        return Err("заголовок слишком длинный");
    }
    let text = std::str::from_utf8(&buf[..end - 4]).map_err(|_| "заголовок не UTF-8")?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (Some(method), Some(target), Some(version), None) = (first.next(), first.next(), first.next(), first.next()) else {
        return Err("неверная строка запроса");
    };
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or("неверный заголовок")?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    Ok(Some(RequestHead { method: method.into(), target: target.into(), version: version.into(), headers, len: end }))
}

/// `CONNECT host:port` или абсолютный `http://…` — единственное, что шлёт браузер HTTP-прокси.
pub fn parse_target(method: &str, target: &str) -> Result<Target, &'static str> {
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = parse_authority(target, None)?;
        return Ok(Target::Authority { host, port });
    }
    let rest = target.strip_prefix("http://").ok_or("ожидался абсолютный адрес http://")?;
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(split);
    let path = if tail.is_empty() {
        "/".to_string()
    } else if tail.starts_with('?') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    let (host, port) = parse_authority(authority, Some(80))?;
    Ok(Target::Absolute { host, port, path })
}

fn parse_authority(text: &str, default_port: Option<u16>) -> Result<(String, u16), &'static str> {
    if text.contains('@') {
        return Err("учётные данные в адресе не принимаются");
    }
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or("неверный адрес IPv6")?;
        let port = match after.strip_prefix(':') {
            Some(port) => Some(port),
            None if after.is_empty() => None,
            None => return Err("неверный адрес"),
        };
        (host.to_string(), port)
    } else {
        match text.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), Some(port)),
            None => (text.to_string(), None),
        }
    };
    let port = match port {
        Some(port) => port.parse::<u16>().map_err(|_| "неверный порт")?,
        None => default_port.ok_or("нет порта")?,
    };
    if host.is_empty() || port == 0 || host.contains(char::is_whitespace) {
        return Err("неверный адрес");
    }
    Ok((host, port))
}

pub enum Form<'a> {
    /// Через HTTP- или HTTPS-прокси: строка запроса остаётся абсолютной.
    Absolute,
    /// Через SOCKS5 прямо к сайту: короткая форма `GET /path`.
    Origin(&'a str),
}

const DROPPED: &[&str] = &["proxy-authorization", "proxy-connection", "connection", "keep-alive"];

pub fn forward_head(head: &RequestHead, form: Form<'_>, proxy_auth: Option<&str>) -> Vec<u8> {
    let target = match form {
        Form::Absolute => head.target.as_str(),
        Form::Origin(path) => path,
    };
    let mut out = format!("{} {} {}\r\n", head.method, target, head.version);
    for (name, value) in &head.headers {
        if !DROPPED.iter().any(|dropped| name.eq_ignore_ascii_case(dropped)) {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    out.push_str("Connection: close\r\n");
    if let Some(auth) = proxy_auth {
        out.push_str(&format!("Proxy-Authorization: {auth}\r\n"));
    }
    out.push_str("\r\n");
    out.into_bytes()
}

/// `host:port`, IPv6-адрес берётся в скобки.
pub fn join_host_port(host: &str, port: u16) -> String {
    if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
}

pub fn status_response(code: u16, reason: &str) -> Vec<u8> {
    format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}

/// Ответ браузеру на CONNECT: туннель открыт, но соединение с прослойкой не переиспользуется.
pub const TUNNEL_ESTABLISHED: &[u8] =
    b"HTTP/1.1 200 Connection established\r\nConnection: close\r\nProxy-Connection: close\r\n\r\n";

/// Заголовки ответа, которые прослойка заменяет своими: одно соединение — один запрос.
const RESPONSE_DROPPED: &[&str] = &["connection", "proxy-connection", "keep-alive"];

/// Заголовок ответа браузеру: убирает `Connection`, `Proxy-Connection` и `Keep-Alive`,
/// дописывает `Connection: close` и `Proxy-Connection: close`. `head` — заголовок целиком, с завершающей пустой строкой.
/// Иначе браузер пошлёт по тому же соединению следующий запрос, и до него может дойти 407, а до сайта — признаки прокси.
pub fn close_response_head(head: &[u8]) -> Vec<u8> {
    let text = head.strip_suffix(b"\r\n\r\n").unwrap_or(head);
    // Байты заголовка копируются как есть: значения вправе быть не UTF-8 (например, Latin-1 в `Content-Disposition`).
    let mut lines = text.split(|&b| b == b'\n').map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let mut out = Vec::with_capacity(head.len() + 48);
    out.extend_from_slice(lines.next().unwrap_or_default());
    out.extend_from_slice(b"\r\n");
    for line in lines {
        let name = line.split(|&b| b == b':').next().unwrap_or_default().trim_ascii();
        if RESPONSE_DROPPED.iter().any(|dropped| name.eq_ignore_ascii_case(dropped.as_bytes())) {
            continue;
        }
        out.extend_from_slice(line);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"Connection: close\r\nProxy-Connection: close\r\n\r\n");
    out
}

/// Код и длина заголовка ответа прокси на `CONNECT`.
pub fn parse_status(buf: &[u8]) -> Option<(u16, usize)> {
    let end = find_head_end(buf)?;
    // Разбирается только строка статуса: в значениях заголовков бывают не-UTF-8 байты.
    let line_end = buf[..end].windows(2).position(|w| w == b"\r\n")?;
    let line = std::str::from_utf8(&buf[..line_end]).ok()?;
    Some((line.split(' ').nth(1)?.parse().ok()?, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_response_head_replaces_connection_headers() {
        let head = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\
                     proxy-connection: keep-alive\r\nContent-Length: 2\r\n\r\n";
        let out = String::from_utf8(close_response_head(head)).unwrap();
        assert_eq!(
            out,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\nProxy-Connection: close\r\n\r\n"
        );
    }

    #[test]
    fn close_response_head_keeps_latin1_value_byte_exact() {
        let head = b"HTTP/1.1 200 OK\r\nContent-Disposition: attachment; filename=\"caf\xe9\"\r\nConnection: keep-alive\r\n\r\n";
        let out = close_response_head(head);
        let expected: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Disposition: attachment; filename=\"caf\xe9\"\r\nConnection: close\r\nProxy-Connection: close\r\n\r\n";
        assert_eq!(out, expected);
        assert_eq!(parse_status(head).map(|(code, _)| code), Some(200), "не-UTF-8 вне строки статуса не мешает");
    }

    #[test]
    fn close_response_head_keeps_bare_status_line() {
        let out = String::from_utf8(close_response_head(b"HTTP/1.1 304 Not Modified\r\n\r\n")).unwrap();
        assert_eq!(out, "HTTP/1.1 304 Not Modified\r\nConnection: close\r\nProxy-Connection: close\r\n\r\n");
        let tunnel = std::str::from_utf8(TUNNEL_ESTABLISHED).unwrap();
        assert!(tunnel.starts_with("HTTP/1.1 200 ") && tunnel.ends_with("\r\nConnection: close\r\nProxy-Connection: close\r\n\r\n"));
    }

    #[test]
    fn head_incomplete_complete_and_oversize() {
        assert_eq!(parse_head(b"CONNECT a:443 HTTP/1.1\r\nHost: a").unwrap(), None);
        let head = parse_head(b"CONNECT a:443 HTTP/1.1\r\nHost: a:443\r\n\r\nXX").unwrap().unwrap();
        assert_eq!((head.method.as_str(), head.target.as_str(), head.len), ("CONNECT", "a:443", 39));
        assert_eq!(head.headers, vec![("Host".to_string(), "a:443".to_string())]);
        assert!(parse_head(&[b'a'; MAX_HEAD + 1]).is_err());
        assert!(parse_head(b"BROKEN\r\n\r\n").is_err());
    }

    #[test]
    fn targets() {
        assert_eq!(parse_target("CONNECT", "example.com:443"), Ok(Target::Authority { host: "example.com".into(), port: 443 }));
        assert_eq!(parse_target("CONNECT", "[2001:db8::1]:443"), Ok(Target::Authority { host: "2001:db8::1".into(), port: 443 }));
        assert_eq!(
            parse_target("GET", "http://example.com:8080/a/b?q=1"),
            Ok(Target::Absolute { host: "example.com".into(), port: 8080, path: "/a/b?q=1".into() })
        );
        assert_eq!(parse_target("GET", "http://example.com"), Ok(Target::Absolute { host: "example.com".into(), port: 80, path: "/".into() }));
        assert_eq!(parse_target("GET", "http://example.com?x"), Ok(Target::Absolute { host: "example.com".into(), port: 80, path: "/?x".into() }));
        assert!(parse_target("CONNECT", "example.com").is_err());
        assert!(parse_target("GET", "/index.html").is_err());
        assert!(parse_target("GET", "https://example.com/").is_err());
        assert!(parse_target("GET", "http://user:pass@example.com/").is_err());
    }

    #[test]
    fn forward_head_rewrites_proxy_headers() {
        let head = parse_head(
            b"GET http://example.com/p?q HTTP/1.1\r\nHost: example.com\r\nProxy-Connection: keep-alive\r\n\
              Proxy-Authorization: Basic browser\r\nConnection: keep-alive\r\nAccept: */*\r\n\r\n",
        )
        .unwrap()
        .unwrap();
        let absolute = String::from_utf8(forward_head(&head, Form::Absolute, Some("Basic dTpw"))).unwrap();
        assert!(absolute.starts_with("GET http://example.com/p?q HTTP/1.1\r\n"), "{absolute}");
        assert!(absolute.contains("Proxy-Authorization: Basic dTpw\r\n"));
        assert!(!absolute.contains("browser"));
        assert!(!absolute.contains("Proxy-Connection"));
        assert!(absolute.contains("Connection: close\r\n"));
        assert!(absolute.contains("Accept: */*\r\n"));
        let origin = String::from_utf8(forward_head(&head, Form::Origin("/p?q"), None)).unwrap();
        assert!(origin.starts_with("GET /p?q HTTP/1.1\r\n"), "{origin}");
        assert!(!origin.contains("Proxy-"));
    }
}
