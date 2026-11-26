//! Общее для импорта профилей из архива и из CSV: строка прокси, подбор или создание прокси и группы по имени.
use super::CommandError;
use super::proxy_import::{ParsedProxy, split_address};
use crate::store::Store;
use crate::store::fold;
use crate::store::groups::GroupInput;
use crate::store::proxies::{ProxyInput, ProxyKind};
use crate::store::secrets::{Secret, SecretField};

/// Предел имени группы — тот же, что в базе.
pub(crate) use crate::store::groups::NAME_MAX as GROUP_NAME_MAX;

/// Логин и пароль в строке прокси: `%`, `:` и `@` кодируются, чтобы строка читалась обратно однозначно.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '%' => out.push_str("%25"),
            ':' => out.push_str("%3A"),
            '@' => out.push_str("%40"),
            c => out.push(c),
        }
    }
    out
}

/// `%` без двух шестнадцатеричных цифр остаётся как есть (строка набрана руками).
fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| text.get(i + 1..i + 3)).flatten().filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit())).and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// `kind://login@host:port`, с паролем — `kind://login:pass@host:port`; логин и пароль закодированы.
pub fn proxy_line(kind: ProxyKind, user: Option<&str>, password: Option<&str>, host: &str, port: u16) -> String {
    let login = match (user, password) {
        (Some(u), Some(p)) => format!("{}:{}@", encode(u), encode(p)),
        (Some(u), None) => format!("{}@", encode(u)),
        _ => String::new(),
    };
    // IPv6 в списке прокси хранится без скобок, в строке он в скобках, как в `split_address`.
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    format!("{}://{login}{host}:{port}", kind.as_str())
}

/// Обратное к `proxy_line`; допускает логин без пароля (архив). Хост и порт — как у импорта списка прокси;
/// пароль без логина и неверный хост — отказ (их не примет и база).
pub fn parse_proxy_line(line: &str) -> Option<ParsedProxy> {
    let (scheme, rest) = line.trim().split_once("://")?;
    let kind = ProxyKind::parse(&scheme.to_ascii_lowercase())?;
    let (auth, address) = match rest.rfind('@') {
        Some(at) => (Some(&rest[..at]), &rest[at + 1..]),
        None => (None, rest),
    };
    let (host, port) = split_address(address.trim_end_matches('/')).ok()?;
    if host.is_empty() || host.len() > 253 || host.contains(char::is_whitespace) {
        return None;
    }
    let (user, password) = match auth.map(|a| a.split_once(':').unwrap_or((a, ""))) {
        Some((u, p)) => (decode(u)?, decode(p)?),
        None => (String::new(), String::new()),
    };
    if user.is_empty() && !password.is_empty() {
        return None;
    }
    Some(ParsedProxy {
        kind,
        host,
        port,
        username: (!user.is_empty()).then_some(user),
        password: (!password.is_empty()).then(|| Secret::new(password)),
    })
}

/// Прокси списка по виду, хосту, порту и логину; нет такого — новая запись (`enabled` — включена ли она).
pub fn resolve_proxy(store: &Store, proxy: ParsedProxy, enabled: bool) -> Result<i64, CommandError> {
    if let Some(found) = store.find_proxy(proxy.kind, &proxy.host, proxy.port, proxy.username.as_deref())? {
        return Ok(found);
    }
    let input = ProxyInput {
        kind: proxy.kind,
        host: proxy.host,
        port: proxy.port,
        username: proxy.username,
        password: proxy.password.map_or(SecretField::Clear, SecretField::Set),
        note: String::new(),
        enabled,
    };
    Ok(store.create_proxy(&input)?.id)
}

/// Группа по имени (сравнение как в базе — `fold`); нет такой — создаётся. Имя должно быть уже проверено.
pub fn group_by_name(store: &Store, name: &str) -> Result<i64, CommandError> {
    let folded = fold(name);
    if let Some(group) = store.list_groups()?.into_iter().find(|g| fold(&g.name) == folded) {
        return Ok(group.id);
    }
    let input = GroupInput { name: name.into(), color: "#0b6e6e".into(), note: String::new(), quick_access: false };
    Ok(store.create_group(&input, crate::sys::unix_now())?.id)
}
