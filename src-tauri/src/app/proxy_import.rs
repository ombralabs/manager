//! Разбор строк импорта прокси: `тип://логин:пароль@хост:порт` и `хост:порт:логин:пароль` (спецификация 2А, «Список прокси»).

use crate::store::proxies::ProxyKind;
use crate::store::secrets::Secret;
use std::fmt;

/// Прокси из строки импорта. Логин и пароль в `Debug` не попадают.
#[derive(Clone)]
pub struct ParsedProxy {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<Secret>,
}

impl fmt::Debug for ParsedProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParsedProxy")
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username.as_ref().map(|_| "***"))
            .field("password", &self.password.as_ref().map(|_| "***"))
            .finish()
    }
}

/// `Ok(None)` — пустая строка или комментарий `#`. Коды ошибок: `format`, `kind`, `port`.
/// Форматы: `тип://логин:пароль@хост:порт`, `тип://хост:порт`, `хост:порт:логин:пароль`, `хост:порт`.
/// В URL-форме логин — до первого `:`, хост — после последнего `@`, поэтому пароль может содержать `:` и `@`;
/// хост IPv6 — в квадратных скобках. В форме через двоеточия пароль — всё после третьего `:`.
pub fn parse_proxy_line(line: &str, default_kind: ProxyKind) -> Result<Option<ParsedProxy>, &'static str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let (kind, host, port, username, password) = match line.split_once("://") {
        Some((scheme, rest)) => {
            let kind = ProxyKind::parse(&scheme.to_ascii_lowercase()).ok_or("kind")?;
            let (auth, address) = match rest.rfind('@') {
                Some(at) => (Some(&rest[..at]), &rest[at + 1..]),
                None => (None, rest),
            };
            let (host, port) = split_address(address.trim_end_matches('/'))?;
            let (username, password) = match auth {
                Some(auth) => auth.split_once(':').unwrap_or((auth, "")),
                None => ("", ""),
            };
            (kind, host, port, username, password)
        }
        None => {
            let parts: Vec<&str> = line.splitn(4, ':').collect();
            let (host, port, username, password) = match parts.as_slice() {
                [host, port] => (*host, *port, "", ""),
                [host, port, username, password] => (*host, *port, *username, *password),
                _ => return Err("format"),
            };
            (default_kind, host.to_string(), parse_port(port)?, username, password)
        }
    };
    if host.is_empty() || host.len() > 253 || host.contains(char::is_whitespace) {
        return Err("format");
    }
    // Логин и пароль задаются только вместе, как в редакторе: иначе авторизация уйдёт наполовину.
    if username.is_empty() != password.is_empty() {
        return Err("format");
    }
    Ok(Some(ParsedProxy {
        kind,
        host,
        port,
        username: (!username.is_empty()).then(|| username.to_string()),
        password: (!password.is_empty()).then(|| Secret::new(password)),
    }))
}

/// `хост:порт` или `[IPv6]:порт`.
pub(crate) fn split_address(address: &str) -> Result<(String, u16), &'static str> {
    if let Some(rest) = address.strip_prefix('[') {
        let (host, port) = rest.split_once("]:").ok_or("format")?;
        return Ok((host.to_string(), parse_port(port)?));
    }
    let (host, port) = address.rsplit_once(':').ok_or("format")?;
    if host.contains(':') {
        return Err("format");
    }
    Ok((host.to_string(), parse_port(port)?))
}

fn parse_port(text: &str) -> Result<u16, &'static str> {
    match text.trim().parse::<u16>() {
        Ok(0) | Err(_) => Err("port"),
        Ok(port) => Ok(port),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Option<ParsedProxy>, &'static str> {
        parse_proxy_line(line, ProxyKind::Http)
    }

    fn parts(proxy: &ParsedProxy) -> (ProxyKind, &str, u16, Option<&str>, Option<&str>) {
        (
            proxy.kind,
            proxy.host.as_str(),
            proxy.port,
            proxy.username.as_deref(),
            proxy.password.as_ref().map(|p| p.expose()),
        )
    }

    #[test]
    fn url_form_with_and_without_login() {
        let proxy = parse("socks5://user:pass@gw.example.net:1080").unwrap().unwrap();
        assert_eq!(parts(&proxy), (ProxyKind::Socks5, "gw.example.net", 1080, Some("user"), Some("pass")));
        let proxy = parse("  HTTPS://gw.example.net:443/  ").unwrap().unwrap();
        assert_eq!(parts(&proxy), (ProxyKind::Https, "gw.example.net", 443, None, None));
    }

    #[test]
    fn password_may_contain_colon_and_at() {
        let proxy = parse("http://user:p@ss:w0rd@gw.example.net:8080").unwrap().unwrap();
        assert_eq!(parts(&proxy), (ProxyKind::Http, "gw.example.net", 8080, Some("user"), Some("p@ss:w0rd")));
        let proxy = parse("gw.example.net:3128:bob:se:cret").unwrap().unwrap();
        assert_eq!(parts(&proxy), (ProxyKind::Http, "gw.example.net", 3128, Some("bob"), Some("se:cret")));
    }

    #[test]
    fn colon_form_takes_the_default_kind() {
        let proxy = parse_proxy_line("gw.example.net:1080", ProxyKind::Socks5).unwrap().unwrap();
        assert_eq!(parts(&proxy), (ProxyKind::Socks5, "gw.example.net", 1080, None, None));
        let proxy = parse_proxy_line("http://gw.example.net:8080", ProxyKind::Socks5).unwrap().unwrap();
        assert_eq!(proxy.kind, ProxyKind::Http, "тип из строки важнее типа по умолчанию");
    }

    #[test]
    fn ipv6_host_goes_in_brackets() {
        let proxy = parse("https://user:pass@[2001:db8::1]:8443").unwrap().unwrap();
        assert_eq!(parts(&proxy), (ProxyKind::Https, "2001:db8::1", 8443, Some("user"), Some("pass")));
    }

    #[test]
    fn blank_lines_and_comments_are_skipped() {
        for line in ["", "   ", "# основной пул", "  # отступ"] {
            assert!(parse(line).unwrap().is_none(), "{line:?}");
        }
    }

    #[test]
    fn bad_lines_have_codes() {
        let cases = [
            ("ftp://gw.example.net:21", "kind"),
            ("gw.example.net:0", "port"),
            ("gw.example.net:http", "port"),
            ("http://gw.example.net:99999", "port"),
            ("gw.example.net", "format"),
            ("gw.example.net:1080:user", "format"),
            ("http://user@gw.example.net:80", "format"),
            ("gw.example.net:1080::pass", "format"),
            ("bad host:80", "format"),
            ("http://[2001:db8::1:8443", "format"),
        ];
        for (line, code) in cases {
            assert_eq!(parse(line).map(|_| ()), Err(code), "{line}");
        }
    }

    #[test]
    fn debug_hides_login_and_password() {
        let proxy = parse("http://alice-login:hunter2@gw.example.net:8080").unwrap().unwrap();
        let text = format!("{proxy:?}");
        assert!(!text.contains("alice-login") && !text.contains("hunter2"), "{text}");
        assert!(text.contains("gw.example.net") && text.contains("8080"), "{text}");
    }
}
