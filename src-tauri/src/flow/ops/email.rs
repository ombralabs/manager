//! Email: самое новое подходящее письмо из ящика IMAP и текст из него (спецификация А4, «А4-1»). Вход — паролем
//! или OAuth2 (Outlook, Hotmail: токен по `refresh_token` у Microsoft, вход XOAUTH2). Сеть — `services::connect`:
//! напрямую или через «Прокси для сервисов».
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::{Cfg, OpError, proc};
use crate::flow::services;
use crate::flow::vars::Scope;
use crate::relay::BoxedIo;
use crate::store::Store;
use async_imap::types::{Fetch, Name, NameAttribute};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use futures_util::TryStreamExt;
use mail_parser::MessageParser;
use serde_json::Value;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Предел на всё обращение к почте: токен OAuth2, подключение, вход, поиск, письмо.
#[cfg(not(test))]
const LIMIT: Duration = services::TIMEOUT;
/// В тестах молчащий сервер минуту не держит.
#[cfg(test)]
const LIMIT: Duration = Duration::from_secs(3);

/// Предел на выход (`LOGOUT`) после прочитанного письма — вне общего: зависший выход результат не отменяет.
#[cfg(not(test))]
const LOGOUT_LIMIT: Duration = Duration::from_secs(5);
#[cfg(test)]
const LOGOUT_LIMIT: Duration = Duration::from_millis(500);

/// Заголовков за один `FETCH`: пачки идут от новых писем к старым до первой с подходящим.
const CHUNK: usize = 100;

#[cfg_attr(test, allow(dead_code))]
const TOKEN_URL: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
const SCOPE: &str = "https://outlook.office.com/IMAP.AccessAsUser.All offline_access";

pub(crate) async fn exec(ctx: &mut Ctx, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let store = ctx.store.clone().ok_or("у запуска нет настроек сервисов")?;
    let text = tokio::select! {
        text = fetch(cfg, &ctx.scope, &store) => text?,
        _ = ctx.cancel.cancelled() => return Err(OpError::Cancelled),
    };
    Ok((Step::Port("next"), ctx.save(cfg, Value::String(text))?))
}

/// Поля узла после подстановки; `from` и `subject` — в нижнем регистре.
struct Mail {
    email: String,
    password: String,
    server: String,
    port: u16,
    ssl: bool,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    /// Отбор `UID SEARCH`.
    search: String,
    /// Местная полночь первого дня отбора (секунды Unix): письма раньше неё не берутся.
    cutoff: i64,
    mark_read: bool,
    spam: bool,
    from: String,
    subject: String,
}

/// Текст самого нового подходящего письма или первое совпадение `extract` в нём. Узел и «Проверить» (вне запуска).
/// Ошибка — с текстом сервера, без пароля и токенов.
pub(crate) async fn fetch(cfg: &Cfg<'_>, scope: &Scope, store: &Store) -> Result<String, String> {
    fetch_at(cfg, scope, store, crate::sys::unix_now()).await
}

/// `fetch` в момент `now` (секунды Unix): от него считаются даты отбора.
async fn fetch_at(cfg: &Cfg<'_>, scope: &Scope, store: &Store, now: i64) -> Result<String, String> {
    let text = |name: &str| cfg.text(scope, name);
    let pattern = text("extract")?;
    let rule = match pattern.is_empty() {
        true => None,
        false => Some(regex::Regex::new(&pattern).map_err(|e| proc::rule_error(cfg, "extract", &e))?),
    };
    let port = cfg.int(scope, "port")?;
    let days = match cfg.string("date").as_str() {
        "yesterday" => 1,
        "7d" => 7,
        "30d" => 30,
        _ => 0,
    };
    // Сервер сравнивает дни в своём поясе (Outlook и Gmail — в UTC): поиск — с дня раньше, граница — местная полночь.
    let since = local_day(now, days + 1).1;
    let mail = Mail {
        email: text("email")?.trim().to_string(),
        password: text("password")?,
        server: text("server")?.trim().to_string(),
        port: u16::try_from(port).ok().filter(|port| *port > 0).ok_or("поле port: от 1 до 65535")?,
        ssl: cfg.flag("ssl"),
        client_id: text("client_id")?.trim().to_string(),
        client_secret: text("client_secret")?,
        refresh_token: text("refresh_token")?.trim().to_string(),
        search: match cfg.string("status").as_str() {
            "unseen" => format!("UNSEEN SINCE {since}"),
            "seen" => format!("SEEN SINCE {since}"),
            _ => format!("SINCE {since}"),
        },
        cutoff: local_day(now, days).0,
        mark_read: cfg.flag("mark_read"),
        spam: cfg.flag("spam"),
        from: text("from")?.to_lowercase(),
        subject: text("subject")?.to_lowercase(),
    };
    if mail.password.is_empty() && mail.refresh_token.is_empty() {
        return Err("нужен пароль или OAuth2 (refresh token)".into());
    }
    let hidden = |e: String| hide(e, &[&mail.password, &quoted(&mail.password), &mail.client_secret, &mail.refresh_token]);
    // Один предел на все шаги; ошибка называет шаг, который не уложился.
    let deadline = tokio::time::Instant::now() + LIMIT;
    let late = |what: String| format!("{what} не ответил за {} с", LIMIT.as_secs());
    let token = match mail.refresh_token.is_empty() {
        true => None,
        false => match tokio::time::timeout_at(deadline, access_token(store, &mail)).await {
            Ok(token) => Some(token.map_err(hidden)?),
            Err(_) => return Err(late("OAuth2: Microsoft".into())),
        },
    };
    let (body, mut session) = match tokio::time::timeout_at(deadline, read(store, &mail, token)).await {
        Ok(read) => read.map_err(hidden)?,
        Err(_) => return Err(late(format!("почтовый сервер {}:{}", mail.server, mail.port))),
    };
    // Письмо уже прочитано (и, может быть, отмечено прочитанным): зависший выход результат не отменяет.
    let _ = tokio::time::timeout(LOGOUT_LIMIT, session.logout()).await;
    match rule {
        None => Ok(body),
        Some(rule) => rule.find(&body).map(|found| found.as_str().to_string()).ok_or_else(|| "в письме нет совпадения".into()),
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Местный день `now` минус `days`: его полночь (секунды Unix) и дата IMAP (`02-Mar-2026`).
fn local_day(now: i64, days: i64) -> (i64, String) {
    let time = now as libc::time_t;
    // SAFETY: структура на стеке; `localtime_r` потокобезопасна, `mktime` нормализует поля.
    let mut tm = crate::sys::local_time(time);
    (tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_isdst) = (tm.tm_mday - days as libc::c_int, 0, 0, 0, -1);
    let midnight = crate::sys::make_local_time(&mut tm);
    (midnight as i64, format!("{:02}-{}-{}", tm.tm_mday, MONTHS[tm.tm_mon as usize], tm.tm_year + 1900))
}

type Session = async_imap::Session<Conn>;

/// Письмо от подключения до текста; сессия остаётся для выхода. `token` — вход XOAUTH2, `None` — паролем.
async fn read(store: &Store, mail: &Mail, token: Option<String>) -> Result<(String, Session), String> {
    let io = services::connect(store, &mail.server, mail.port).await?;
    let io = if mail.ssl { tls(io, &mail.server).await? } else { io };
    let mut client = async_imap::Client::new(Conn(io));
    if !matches!(client.read_response().await, Ok(Some(_))) {
        return Err(format!("{}:{} — не сервер IMAP", mail.server, mail.port));
    }
    let refused = |e| format!("вход в почту {} не выполнен: {}", mail.email, imap(e));
    let mut session = match token {
        None => client.login(&mail.email, &mail.password).await.map_err(|(e, _)| refused(e))?,
        Some(token) => {
            let sasl = format!("user={}\x01auth=Bearer {token}\x01\x01", mail.email);
            let sent = B64.encode(&sasl);
            client.authenticate("XOAUTH2", XOAuth2(Some(sasl))).await.map_err(|(e, _)| hide(refused(e), &[&token, &sent]))?
        }
    };
    let folder = if mail.spam { junk(&mut session).await? } else { "INBOX".to_string() };
    session.select(&folder).await.map_err(|e| format!("папка {folder}: {}", imap(e)))?;
    let mut uids: Vec<u32> = session.uid_search(&mail.search).await.map_err(fail)?.into_iter().collect();
    uids.sort_unstable_by(|a, b| b.cmp(a));
    let uid = newest(&mut session, &uids, mail).await?.ok_or("письма нет")?;
    let letter: Vec<Fetch> = session.uid_fetch(uid.to_string(), "BODY.PEEK[]").await.map_err(fail)?.try_collect().await.map_err(fail)?;
    let raw = letter.iter().find_map(Fetch::body).ok_or("Почтовый сервер не вернул письмо")?;
    if mail.mark_read {
        let _: Vec<Fetch> = session.uid_store(uid.to_string(), "+FLAGS (\\Seen)").await.map_err(fail)?.try_collect().await.map_err(fail)?;
    }
    let text = MessageParser::default().parse(raw).and_then(|message| message.body_text(0).map(|text| text.trim().to_string()));
    Ok((text.unwrap_or_default(), session))
}

/// UID самого нового из найденных (UID растут с приходом письма в папку): не раньше местной границы дат и подходящего
/// по отправителю и теме. Даты и заголовки читаются пачками от новых к старым.
async fn newest(session: &mut Session, uids: &[u32], mail: &Mail) -> Result<Option<u32>, String> {
    let filtered = !mail.from.is_empty() || !mail.subject.is_empty();
    let items = if filtered { "(UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT)])" } else { "(UID INTERNALDATE)" };
    for chunk in uids.chunks(CHUNK) {
        let set = chunk.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let heads: Vec<Fetch> = session.uid_fetch(set, items).await.map_err(fail)?.try_collect().await.map_err(fail)?;
        // FETCH без запроса (флаги поменял другой клиент) — не из этой пачки: такой UID поиск не находил.
        let found = heads
            .iter()
            .filter(|head| head.uid.is_some_and(|uid| chunk.contains(&uid)))
            .filter(|head| head.internal_date().is_none_or(|date| date.timestamp() >= mail.cutoff))
            .filter(|head| !filtered || head.header().is_some_and(|raw| fits(raw, mail)))
            .filter_map(|head| head.uid)
            .max();
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

/// Отправитель («Имя <адрес>») и тема содержат `from` и `subject` без учёта регистра.
fn fits(raw: &[u8], mail: &Mail) -> bool {
    let Some(head) = MessageParser::default().parse_headers(raw) else { return false };
    let from = head.from().map(|list| list.iter().map(|a| format!("{} <{}>", a.name().unwrap_or_default(), a.address().unwrap_or_default())).collect::<Vec<_>>().join(", "));
    from.unwrap_or_default().to_lowercase().contains(&mail.from) && head.subject().unwrap_or_default().to_lowercase().contains(&mail.subject)
}

/// Папка «Нежелательная почта»: по признаку `\Junk` (в Outlook её имя зависит от языка ящика), иначе `Junk`.
async fn junk(session: &mut Session) -> Result<String, String> {
    let names: Vec<Name> = session.list(Some(""), Some("*")).await.map_err(fail)?.try_collect().await.map_err(fail)?;
    let junk = names.iter().find(|name| name.attributes().iter().any(|a| matches!(a, NameAttribute::Junk)));
    Ok(junk.map_or("Junk", Name::name).to_string())
}

/// Токен доступа Microsoft по `refresh_token`.
async fn access_token(store: &Store, mail: &Mail) -> Result<String, String> {
    let mut form = vec![("client_id", mail.client_id.as_str()), ("grant_type", "refresh_token"), ("refresh_token", &mail.refresh_token), ("scope", SCOPE)];
    if !mail.client_secret.is_empty() {
        form.push(("client_secret", &mail.client_secret));
    }
    let sent = services::http(store)?.post(token_url()).form(&form).send().await;
    let response = match sent {
        Ok(response) => response,
        Err(e) => {
            // `Display` у reqwest — только «error sending request»: причина — в цепочке источников.
            let Some(proxy) = services::chosen(store)? else { return Err(format!("OAuth2: Microsoft не ответил: {}", causes(e))) };
            let text = format!("OAuth2: Microsoft не ответил через прокси для сервисов {}:{}: {}", proxy.host, proxy.port, causes(e));
            return Err(match &proxy.credentials {
                Some(c) => hide(text, &[&c.password]),
                None => text,
            });
        }
    };
    let body: Value = response.json().await.map_err(|e| format!("OAuth2: ответ Microsoft не прочитан: {}", causes(e)))?;
    if let Some(token) = body["access_token"].as_str() {
        return Ok(token.to_string());
    }
    let reason = body["error_description"].as_str().or(body["error"].as_str()).unwrap_or("нет access_token");
    Err(format!("OAuth2: {}", reason.lines().next().unwrap_or_default()))
}

#[cfg(not(test))]
fn token_url() -> String {
    TOKEN_URL.to_string()
}

#[cfg(test)]
fn token_url() -> String {
    tests::TOKEN_URL.with(|url| url.borrow().clone())
}

/// Корни доверия TLS: Mozilla (webpki-roots).
#[cfg(not(test))]
fn roots() -> Option<rustls::RootCertStore> {
    None
}

/// В тестах — корни подставного сервера.
#[cfg(test)]
fn roots() -> Option<rustls::RootCertStore> {
    tests::ROOTS.with(|roots| roots.borrow().clone())
}

/// Ответ XOAUTH2: строка входа, а на отказ (сервер шлёт подробность как новый вызов) — пустая строка.
struct XOAuth2(Option<String>);

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;

    fn process(&mut self, _: &[u8]) -> String {
        self.0.take().unwrap_or_default()
    }
}

async fn tls(io: BoxedIo, server: &str) -> Result<BoxedIo, String> {
    let name = rustls::pki_types::ServerName::try_from(server.to_string()).map_err(|_| format!("неверное имя сервера {server}"))?;
    let stream = tokio_rustls::TlsConnector::from(crate::relay::client_tls(roots().as_ref())).connect(name, io).await.map_err(|e| format!("TLS {server}: {e}"))?;
    Ok(Box::new(stream))
}

/// Текст ошибки IMAP: у отказа сервера — его собственный.
fn imap(error: async_imap::error::Error) -> String {
    use async_imap::error::Error;
    match error {
        Error::No(text) | Error::Bad(text) => said(&text),
        Error::ConnectionLost => "сервер закрыл соединение".into(),
        Error::Io(e) => format!("соединение: {e}"),
        other => other.to_string(),
    }
}

/// Текст отказа: async-imap отдаёт его как `code: …, info: Some("…")` (строка в записи `Debug`).
fn said(text: &str) -> String {
    let info = text.split_once("info: Some(\"").and_then(|(_, rest)| rest.strip_suffix("\")"));
    info.and_then(|info| serde_json::from_str(&format!("\"{info}\"")).ok()).unwrap_or_else(|| text.to_string())
}

/// Ошибка reqwest с причинами, без адреса.
fn causes(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut text = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        text = format!("{text}: {cause}");
        source = cause.source();
    }
    text
}

fn fail(error: async_imap::error::Error) -> String {
    format!("почта: {}", imap(error))
}

/// Секреты в тексте ошибки — звёздочками: сервер мог повторить присланное.
fn hide(mut text: String, secrets: &[&str]) -> String {
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        text = text.replace(secret, "***");
    }
    text
}

/// Строка в записи IMAP внутри кавычек — так `LOGIN` шлёт пароль.
fn quoted(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Поток к серверу для async-imap: ему нужен `Debug`.
struct Conn(BoxedIo);

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Conn")
    }
}

impl AsyncRead for Conn {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Conn {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::flow::interp::Fail;
    use crate::flow::model::Flow;
    use crate::flow::ops::spec;
    use crate::relay::test_support::{closed_port, port_closes};
    use crate::store::proxies::{ProxyInput, ProxyKind};
    use crate::store::secrets::{Secret, SecretField};
    use crate::store::tests::memory_store;
    use serde_json::json;
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    thread_local! {
        /// Выдача токенов Microsoft для тестов этого потока; по умолчанию — закрытый порт: наружу тесты не ходят.
        pub(super) static TOKEN_URL: RefCell<String> = RefCell::new("http://127.0.0.1:9/token".into());
        /// Корни TLS для тестов этого потока; `None` — корни Mozilla.
        pub(super) static ROOTS: RefCell<Option<rustls::RootCertStore>> = const { RefCell::new(None) };
    }

    /// «Нежелательная почта» в modified UTF-7 — так её имя отдаёт Outlook с русским языком ящика.
    const JUNK_RU: &str = "&BB0ENQQ2BDUEOwQwBEIENQQ7BEwEPQQwBE8- &BD8EPgRHBEIEMA-";

    struct Letter {
        uid: u32,
        /// `INTERNALDATE`, секунды Unix.
        internal: i64,
        raw: String,
    }

    /// Письмо, пришедшее сейчас.
    fn letter(uid: u32, from: &str, subject: &str, kind: &str, body: &str) -> Letter {
        let raw = format!("From: {from}\r\nSubject: {subject}\r\nContent-Type: text/{kind}; charset=utf-8\r\n\r\n{body}\r\n");
        Letter { uid, internal: crate::sys::unix_now(), raw }
    }

    /// Ящик: подходящих под instagram/code два (UID 2 и 3), новее них — чужое письмо в HTML (UID 4).
    fn inbox() -> Vec<Letter> {
        vec![
            letter(1, "Shop <news@shop.test>", "Sale", "plain", "Nothing"),
            letter(2, "Instagram <security@mail.instagram.com>", "Your code", "plain", "Your code is 111111"),
            letter(3, "Instagram <security@mail.instagram.com>", "Your login code", "plain", "Hello!\r\nYour code is 654321.\r\nBye"),
            letter(4, "Shop <news@shop.test>", "Your code for a discount", "html", "<html><head><style>p{color:red}</style></head><body><p>Promo <b>999999</b></p></body></html>"),
        ]
    }

    /// Что отдаёт подставной сервер.
    #[derive(Default)]
    pub(crate) struct Mailbox {
        letters: Vec<Letter>,
        /// Ответ на любой `UID SEARCH`.
        found: Vec<u32>,
        /// TLS с этим сертификатом; `None` — без TLS.
        tls: Option<tokio_rustls::TlsAcceptor>,
        /// На `LOGOUT` сервер молчит.
        silent_logout: bool,
        /// UID письма, чьи флаги в это время меняет другой клиент: сервер шлёт его FETCH без запроса.
        unsolicited: Option<u32>,
    }

    pub(crate) fn mailbox(found: &[u32]) -> Mailbox {
        Mailbox { letters: inbox(), found: found.to_vec(), ..Default::default() }
    }

    /// Местное время — секунды Unix.
    fn local(year: i32, month: i32, day: i32, hour: i32, min: i32) -> i64 {
        // SAFETY: структура на стеке; `mktime` только читает и нормализует её.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        (tm.tm_year, tm.tm_mon, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_isdst) = (year - 1900, month - 1, day, hour, min, -1);
        unsafe { libc::mktime(&mut tm) as i64 }
    }

    /// `INTERNALDATE` по UTC, как его хранит Outlook: `20-Oct-2026 21:10:00 +0000`.
    fn imap_time(time: i64) -> String {
        let time = time as libc::time_t;
        // SAFETY: структуры на стеке; `gmtime_r` потокобезопасна.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::gmtime_r(&time, &mut tm) };
        let month = MONTHS[tm.tm_mon as usize];
        format!("{:02}-{month}-{} {:02}:{:02}:{:02} +0000", tm.tm_mday, tm.tm_year + 1900, tm.tm_hour, tm.tm_min, tm.tm_sec)
    }

    pub(crate) struct Fake {
        pub(crate) port: u16,
        /// Команды клиента без тегов по порядку; имя из SNI — `SNI <имя>`, ответ на XOAUTH2 — `XOAUTH2 <расшифровка>`.
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Fake {
        pub(crate) fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    /// Подставной IMAP на одно соединение. Пароль — `pa$$word`, токен XOAUTH2 — `at-1`. Отказы во входе повторяют
    /// присланное — как неаккуратный сервер.
    pub(crate) async fn fake(mailbox: Mailbox) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let journal = log.clone();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            match mailbox.tls.clone() {
                Some(acceptor) => {
                    let Ok(stream) = acceptor.accept(tcp).await else { return };
                    journal.lock().unwrap().push(format!("SNI {}", stream.get_ref().1.server_name().unwrap_or_default()));
                    serve(stream, mailbox, journal).await
                }
                None => serve(tcp, mailbox, journal).await,
            }
        });
        Fake { port, log }
    }

    async fn serve(stream: impl AsyncRead + AsyncWrite + Unpin, mailbox: Mailbox, journal: Arc<Mutex<Vec<String>>>) {
        let (read, mut write) = tokio::io::split(stream);
        let mut read = BufReader::new(read);
        write.write_all(b"* OK fake IMAP ready\r\n").await.unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            if read.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            let (tag, command) = (tag.to_string(), command.to_string());
            journal.lock().unwrap().push(command.clone());
            let reply = if let Some(args) = command.strip_prefix("LOGIN ") {
                if args.ends_with(" \"pa$$word\"") {
                    format!("{tag} OK LOGIN completed\r\n")
                } else {
                    format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials: {args}\r\n")
                }
            } else if command == "AUTHENTICATE XOAUTH2" {
                write.write_all(b"+ \r\n").await.unwrap();
                line.clear();
                read.read_line(&mut line).await.unwrap();
                let sent = line.trim_end().to_string();
                let sasl = String::from_utf8(B64.decode(&sent).unwrap()).unwrap();
                journal.lock().unwrap().push(format!("XOAUTH2 {sasl}"));
                if sasl == "user=me@outlook.test\x01auth=Bearer at-1\x01\x01" {
                    format!("{tag} OK AUTHENTICATE completed\r\n")
                } else {
                    write.write_all(format!("+ {}\r\n", B64.encode(r#"{"status":"401"}"#)).as_bytes()).await.unwrap();
                    line.clear();
                    read.read_line(&mut line).await.unwrap();
                    format!("{tag} NO AUTHENTICATE failed for {sent}\r\n")
                }
            } else if command.starts_with("LIST ") {
                format!("* LIST (\\HasNoChildren) \"/\" INBOX\r\n* LIST (\\HasNoChildren \\Junk) \"/\" \"{JUNK_RU}\"\r\n{tag} OK LIST completed\r\n")
            } else if command.starts_with("SELECT ") {
                format!("* {} EXISTS\r\n* FLAGS (\\Seen)\r\n{tag} OK [READ-WRITE] SELECT completed\r\n", mailbox.letters.len())
            } else if command.starts_with("UID SEARCH ") {
                format!("* SEARCH{}\r\n{tag} OK SEARCH completed\r\n", mailbox.found.iter().map(|uid| format!(" {uid}")).collect::<String>())
            } else if let Some(rest) = command.strip_prefix("UID FETCH ") {
                let (set, items) = rest.split_once(' ').unwrap();
                let mut out = mailbox.unsolicited.map(|uid| format!("* 9 FETCH (UID {uid} FLAGS (\\Seen))\r\n")).unwrap_or_default();
                for uid in set.split(',').map(|uid| uid.parse::<u32>().unwrap()) {
                    let letter = mailbox.letters.iter().find(|letter| letter.uid == uid).unwrap();
                    let mut attrs = format!("UID {uid}");
                    if items.contains("INTERNALDATE") {
                        attrs += &format!(" INTERNALDATE \"{}\"", imap_time(letter.internal));
                    }
                    if items.contains("HEADER.FIELDS") {
                        let head: String = letter
                            .raw
                            .split("\r\n")
                            .take_while(|l| !l.is_empty())
                            .filter(|l| l.starts_with("From:") || l.starts_with("Subject:"))
                            .map(|l| format!("{l}\r\n"))
                            .collect();
                        attrs += &format!(" BODY[HEADER.FIELDS (FROM SUBJECT)] {{{}}}\r\n{head}\r\n", head.len() + 2);
                    } else if items.contains("BODY.PEEK[]") {
                        attrs += &format!(" BODY[] {{{}}}\r\n{}", letter.raw.len(), letter.raw);
                    }
                    out += &format!("* {uid} FETCH ({attrs})\r\n");
                }
                format!("{out}{tag} OK FETCH completed\r\n")
            } else if let Some(rest) = command.strip_prefix("UID STORE ") {
                let uid = rest.split(' ').next().unwrap();
                format!("* {uid} FETCH (UID {uid} FLAGS (\\Seen))\r\n{tag} OK STORE completed\r\n")
            } else if command == "LOGOUT" {
                if mailbox.silent_logout {
                    continue;
                }
                format!("* BYE logging out\r\n{tag} OK LOGOUT completed\r\n")
            } else {
                format!("{tag} BAD unknown command\r\n")
            };
            write.write_all(reply.as_bytes()).await.unwrap();
        }
    }

    /// Поля узла для подставного сервера и `extra` поверх.
    pub(crate) fn config(port: u16, extra: Value) -> serde_json::Map<String, Value> {
        let mut config = json!({ "email": "me@mail.test", "password": "pa$$word", "server": "127.0.0.1", "port": port, "ssl": false, "save_to": "out" });
        config.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        config.as_object().unwrap().clone()
    }

    async fn mail_with(store: &Store, now: i64, port: u16, extra: Value) -> Result<String, String> {
        let config = config(port, extra);
        fetch_at(&Cfg::new(spec("data.email").unwrap(), &config), &Scope::new(Default::default()), store, now).await
    }

    async fn mail(port: u16, extra: Value) -> Result<String, String> {
        mail_with(&memory_store(), crate::sys::unix_now(), port, extra).await
    }

    /// Дата IMAP местного дня `days` дней назад.
    fn since(days: i64) -> String {
        local_day(crate::sys::unix_now(), days).1
    }

    #[test]
    fn local_day_counts_local_days_back() {
        // Местный полдень 2 марта — в любом поясе, от −12 до +14.
        let noon = local(2026, 3, 2, 12, 0);
        assert_eq!(local_day(noon, 0), (local(2026, 3, 2, 0, 0), "02-Mar-2026".into()));
        assert_eq!(local_day(noon, 1).1, "01-Mar-2026");
        assert_eq!(local_day(noon, 7).1, "23-Feb-2026");
        assert_eq!(local_day(noon, 30).1, "31-Jan-2026");
    }

    #[tokio::test]
    async fn newest_matching_letter_gives_the_code() {
        let server = fake(mailbox(&[1, 2, 3, 4])).await;
        let code = mail(server.port, json!({ "from": "INSTAGRAM", "subject": "code", "extract": "\\b\\d{6}\\b" })).await;
        assert_eq!(code, Ok("654321".into()));
        // Отбор по дате — на сервере (с дня раньше); заголовки и даты — только найденных, от новых; тело — одного письма.
        assert_eq!(
            server.log(),
            [
                "LOGIN \"me@mail.test\" \"pa$$word\"".to_string(),
                "SELECT \"INBOX\"".into(),
                format!("UID SEARCH SINCE {}", since(1)),
                "UID FETCH 4,3,2,1 (UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (FROM SUBJECT)])".into(),
                "UID FETCH 3 BODY.PEEK[]".into(),
                "LOGOUT".into(),
            ]
        );
    }

    #[tokio::test]
    async fn unsolicited_fetch_of_another_letter_is_not_a_match() {
        let server = fake(Mailbox { unsolicited: Some(999), ..mailbox(&[2, 3]) }).await;
        assert_eq!(mail(server.port, json!({ "extract": "\\d{6}" })).await, Ok("654321".into()));
    }

    #[tokio::test]
    async fn whole_text_without_extract_and_html_without_tags() {
        let server = fake(mailbox(&[1, 2, 3, 4])).await;
        assert_eq!(mail(server.port, json!({ "from": "instagram" })).await, Ok("Hello!\r\nYour code is 654321.\r\nBye".into()));
        let server = fake(mailbox(&[1, 2, 3, 4])).await;
        assert_eq!(mail(server.port, json!({})).await, Ok("Promo 999999".into()), "самое новое — HTML без тегов и стилей");
        assert!(!server.log().iter().any(|c| c.contains("HEADER.FIELDS")), "без from и subject заголовки не нужны");
    }

    #[tokio::test]
    async fn status_and_date_go_to_the_server_search() {
        let server = fake(mailbox(&[2])).await;
        mail(server.port, json!({ "status": "unseen", "date": "7d" })).await.unwrap();
        assert!(server.log().contains(&format!("UID SEARCH UNSEEN SINCE {}", since(8))), "{:?}", server.log());
        let server = fake(mailbox(&[2])).await;
        mail(server.port, json!({ "status": "seen", "date": "yesterday" })).await.unwrap();
        assert!(server.log().contains(&format!("UID SEARCH SEEN SINCE {}", since(2))), "{:?}", server.log());
        let server = fake(mailbox(&[2])).await;
        mail(server.port, json!({ "date": "30d" })).await.unwrap();
        assert!(server.log().contains(&format!("UID SEARCH SINCE {}", since(31))), "{:?}", server.log());
    }

    #[tokio::test]
    async fn date_cut_off_is_local_midnight_while_the_server_searches_a_day_earlier() {
        // 00:30 местного 21 октября. Восточнее UTC (Москва) письмо 00:10 по UTC ещё 20 октября: сервер с UTC-датами
        // при `SINCE 21-Oct` его бы не нашёл.
        let (now, midnight) = (local(2026, 10, 21, 0, 30), local(2026, 10, 21, 0, 0));
        let fresh = || Letter { internal: midnight + 600, ..letter(6, "Instagram <a@b.test>", "code", "plain", "fresh 222333") };
        let late = || Letter { internal: midnight - 600, ..letter(5, "Instagram <a@b.test>", "code", "plain", "late 111222") };
        let server = fake(Mailbox { letters: vec![late(), fresh()], found: vec![5, 6], ..Default::default() }).await;
        assert_eq!(mail_with(&memory_store(), now, server.port, json!({})).await, Ok("fresh 222333".into()));
        assert!(server.log().contains(&"UID SEARCH SINCE 20-Oct-2026".to_string()), "{:?}", server.log());
        // Письмо до местной полуночи «Сегодня» не берёт, хоть сервер и нашёл его.
        let server = fake(Mailbox { letters: vec![late(), fresh()], found: vec![5], ..Default::default() }).await;
        assert_eq!(mail_with(&memory_store(), now, server.port, json!({})).await, Err("письма нет".into()));
        let server = fake(Mailbox { letters: vec![late(), fresh()], found: vec![5], ..Default::default() }).await;
        assert_eq!(mail_with(&memory_store(), now, server.port, json!({ "date": "yesterday" })).await, Ok("late 111222".into()));
        assert!(server.log().contains(&"UID SEARCH SINCE 19-Oct-2026".to_string()), "{:?}", server.log());
    }

    #[tokio::test]
    async fn mark_read_stores_the_seen_flag() {
        let server = fake(mailbox(&[1, 2, 3, 4])).await;
        mail(server.port, json!({ "from": "instagram", "mark_read": true })).await.unwrap();
        let log = server.log();
        assert_eq!(log[log.len() - 2..], ["UID STORE 3 +FLAGS (\\Seen)".to_string(), "LOGOUT".into()]);
        let server = fake(mailbox(&[1, 2, 3, 4])).await;
        mail(server.port, json!({ "from": "instagram" })).await.unwrap();
        assert!(!server.log().iter().any(|c| c.contains("STORE")), "без mark_read письмо не трогаем");
    }

    #[tokio::test]
    async fn hanging_logout_keeps_the_letter() {
        // Письмо прочитано и отмечено: зависший выход не превращает результат в ошибку.
        let server = fake(Mailbox { silent_logout: true, ..mailbox(&[1, 2, 3, 4]) }).await;
        let started = std::time::Instant::now();
        let text = mail(server.port, json!({ "from": "instagram", "extract": "\\d{6}", "mark_read": true })).await;
        assert_eq!(text, Ok("654321".into()));
        assert!(started.elapsed() < LIMIT, "{:?}", started.elapsed());
        assert_eq!(server.log()[server.log().len() - 2..], ["UID STORE 3 +FLAGS (\\Seen)".to_string(), "LOGOUT".into()]);
    }

    #[tokio::test]
    async fn spam_reads_the_junk_folder() {
        let server = fake(mailbox(&[3])).await;
        mail(server.port, json!({ "spam": true })).await.unwrap();
        let log = server.log();
        assert_eq!(log[1..3], ["LIST \"\" *".to_string(), format!("SELECT \"{JUNK_RU}\"")], "папка с признаком \\Junk");
    }

    #[tokio::test]
    async fn tls_checks_the_server_certificate_by_name() {
        let rcgen::CertifiedKey { cert, key_pair } = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(key_pair.serialize_der().into());
        let config = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(vec![cert.der().clone()], key).unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        // По умолчанию — корни Mozilla: самоподписанный сертификат не принят.
        let server = fake(Mailbox { tls: Some(acceptor.clone()), ..mailbox(&[3]) }).await;
        let error = mail(server.port, json!({ "server": "localhost", "ssl": true })).await.unwrap_err();
        assert!(error.starts_with("TLS localhost: invalid peer certificate"), "{error}");
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.der().clone()).unwrap();
        ROOTS.with(|r| *r.borrow_mut() = Some(roots));
        let server = fake(Mailbox { tls: Some(acceptor), ..mailbox(&[3]) }).await;
        assert_eq!(mail(server.port, json!({ "server": "localhost", "ssl": true, "extract": "\\d{6}" })).await, Ok("654321".into()));
        assert_eq!(server.log()[..2], ["SNI localhost".to_string(), "LOGIN \"me@mail.test\" \"pa$$word\"".into()]);
    }

    #[tokio::test]
    async fn wrong_password_is_an_error_without_it() {
        // Сервер повторил присланное: пароль скрыт — и как есть, и в кавычках IMAP.
        let hidden = r#"вход в почту me@mail.test не выполнен: [AUTHENTICATIONFAILED] Invalid credentials: "me@mail.test" "***""#;
        let server = fake(mailbox(&[3])).await;
        assert_eq!(mail(server.port, json!({ "password": "Wr0ng-Secret" })).await, Err(hidden.into()));
        let server = fake(mailbox(&[3])).await;
        assert_eq!(mail(server.port, json!({ "password": "pa\"ss\\word" })).await, Err(hidden.into()));
    }

    #[tokio::test]
    async fn no_password_and_no_oauth_is_an_error_before_connecting() {
        for password in [json!(null), json!("")] {
            assert_eq!(mail(1, json!({ "password": password })).await, Err("нужен пароль или OAuth2 (refresh token)".into()));
        }
    }

    #[tokio::test]
    async fn no_letter_or_no_match_is_an_error() {
        let server = fake(mailbox(&[])).await;
        assert_eq!(mail(server.port, json!({})).await, Err("письма нет".into()));
        let server = fake(mailbox(&[1, 2, 3, 4])).await;
        assert_eq!(mail(server.port, json!({ "from": "nobody" })).await, Err("письма нет".into()));
        let server = fake(mailbox(&[1, 2, 3, 4])).await;
        assert_eq!(mail(server.port, json!({ "from": "instagram", "extract": "\\b\\d{8}\\b" })).await, Err("в письме нет совпадения".into()));
        assert!(mail(1, json!({ "extract": "(" })).await.unwrap_err().starts_with("неверное правило"), "правило проверяется до сети");
    }

    /// Подставная выдача токенов Microsoft: `rt-1` меняет на `at-1`, другой — отказ, повторяющий запрос.
    async fn token_endpoint() -> Arc<Mutex<Vec<String>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let bodies = seen.clone();
        let app = axum::Router::new().route(
            "/token",
            axum::routing::post(move |body: String| {
                let bodies = bodies.clone();
                async move {
                    bodies.lock().unwrap().push(body.clone());
                    if body.contains("refresh_token=rt-1") {
                        (axum::http::StatusCode::OK, axum::Json(json!({ "token_type": "Bearer", "access_token": "at-1" })))
                    } else {
                        let description = format!("AADSTS70000: bad grant {body}\r\nTrace ID: 1");
                        (axum::http::StatusCode::BAD_REQUEST, axum::Json(json!({ "error": "invalid_grant", "error_description": description })))
                    }
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}/token", listener.local_addr().unwrap().port());
        TOKEN_URL.with(|token| *token.borrow_mut() = url);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        seen
    }

    fn form(body: &str) -> std::collections::HashMap<String, String> {
        reqwest::Url::parse(&format!("http://x/?{body}")).unwrap().query_pairs().into_owned().collect()
    }

    #[tokio::test]
    async fn oauth2_takes_a_token_by_refresh_token_and_logs_in_with_xoauth2() {
        let seen = token_endpoint().await;
        let server = fake(mailbox(&[3])).await;
        let oauth = json!({ "email": "me@outlook.test", "password": null, "client_id": "cid", "client_secret": "cs-1", "refresh_token": "rt-1" });
        assert_eq!(mail(server.port, oauth).await, Ok("Hello!\r\nYour code is 654321.\r\nBye".into()));
        let sent = form(&seen.lock().unwrap()[0]);
        assert_eq!(sent["grant_type"], "refresh_token");
        assert_eq!(sent["refresh_token"], "rt-1");
        assert_eq!(sent["client_id"], "cid");
        assert_eq!(sent["client_secret"], "cs-1");
        assert_eq!(sent["scope"], "https://outlook.office.com/IMAP.AccessAsUser.All offline_access");
        let log = server.log();
        assert_eq!(log[..2], ["AUTHENTICATE XOAUTH2".to_string(), "XOAUTH2 user=me@outlook.test\x01auth=Bearer at-1\x01\x01".into()]);
        assert!(!log.iter().any(|c| c.starts_with("LOGIN")), "{log:?}");
        // Сервер не принял токен и повторил присланное (base64 строки входа с токеном): на подробность отказа клиент
        // отвечает пустой строкой и получает NO, а не виснет; присланное в тексте ошибки скрыто.
        let server = fake(mailbox(&[3])).await;
        let other = json!({ "email": "other@outlook.test", "client_id": "cid", "refresh_token": "rt-1" });
        let error = mail(server.port, other).await.unwrap_err();
        assert_eq!(error, "вход в почту other@outlook.test не выполнен: AUTHENTICATE failed for ***");
    }

    #[tokio::test]
    async fn refused_refresh_token_is_an_error_without_secrets() {
        let _seen = token_endpoint().await;
        let oauth = json!({ "client_id": "cid", "client_secret": "cs-secret", "refresh_token": "rt-expired" });
        let error = mail(1, oauth).await.unwrap_err();
        assert!(error.starts_with("OAuth2: AADSTS70000: bad grant"), "{error}");
        assert!(!error.contains("rt-expired") && !error.contains("cs-secret") && !error.contains("Trace ID"), "{error}");
    }

    #[tokio::test]
    async fn token_errors_name_the_step_and_the_services_proxy() {
        let oauth = json!({ "client_id": "cid", "refresh_token": "rt-1" });
        // Microsoft принял соединение и молчит: в ошибке — шаг OAuth2, а не почтовый сервер.
        let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
        TOKEN_URL.with(|url| *url.borrow_mut() = format!("http://127.0.0.1:{}/token", silent.local_addr().unwrap().port()));
        let _keep = tokio::spawn(async move {
            let _connection = silent.accept().await;
            std::future::pending::<()>().await
        });
        assert_eq!(mail(1, oauth.clone()).await, Err(format!("OAuth2: Microsoft не ответил за {} с", LIMIT.as_secs())));
        // Прокси для сервисов мёртв: в ошибке — прокси и причина, без его пароля.
        let store = memory_store();
        let dead = closed_port().await;
        let proxy = ProxyInput {
            kind: ProxyKind::Http,
            host: "127.0.0.1".into(),
            port: dead,
            username: Some("user1".into()),
            password: SecretField::Set(Secret::new("pr0xy-pass")),
            note: String::new(),
            enabled: true,
        };
        services::set_proxy_id(&store, Some(store.create_proxy(&proxy).unwrap().id)).unwrap();
        let error = mail_with(&store, crate::sys::unix_now(), 1, oauth).await.unwrap_err();
        assert!(error.starts_with(&format!("OAuth2: Microsoft не ответил через прокси для сервисов 127.0.0.1:{dead}: ")), "{error}");
        assert!(error.contains("Connection refused"), "причина: {error}");
        assert!(!error.contains("pr0xy-pass"), "{error}");
    }

    #[tokio::test]
    async fn unreachable_or_silent_server_is_a_clear_error() {
        let port = closed_port().await;
        assert!(port_closes(([127, 0, 0, 1], port).into()).await);
        let error = mail(port, json!({})).await.unwrap_err();
        assert!(error.starts_with(&format!("подключение к 127.0.0.1:{port}: ")), "{error}");
        assert!(!error.contains("pa$$word"), "{error}");
        // Сервер принял соединение и молчит: ошибка по пределу обращения.
        let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = silent.local_addr().unwrap().port();
        let _keep = tokio::spawn(async move {
            let _connection = silent.accept().await;
            std::future::pending::<()>().await
        });
        let error = mail(port, json!({})).await.unwrap_err();
        assert_eq!(error, format!("почтовый сервер 127.0.0.1:{port} не ответил за {} с", LIMIT.as_secs()));
    }

    fn one(config: serde_json::Map<String, Value>) -> Flow {
        serde_json::from_value(json!({
            "format": "ombra-flow/1", "name": "t",
            "nodes": [{ "id": "s", "op": "start" }, { "id": "x", "op": "data.email", "config": config }],
            "edges": [{ "from": "s", "to": "x" }]
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn a_typed_secret_with_a_bad_reference_stays_out_of_the_error_and_the_journal() {
        for (field, typed, fragment) in [("password", "Ab${12", "Ab"), ("password", "x${a-b}", "a-b"), ("client_secret", "Ab${12", "Ab"), ("refresh_token", "x${a-b}", "a-b")] {
            let mut ctx = Ctx::bare();
            ctx.store = Some(Arc::new(memory_store()));
            let result = ctx.run(&one(config(closed_port().await, json!({ field: typed })))).await;
            let Err(Fail::Node { message, .. }) = result else { panic!("{result:?}") };
            assert_eq!(message, format!("Email: поле «{field}»: неверная ссылка на переменную (буквальное «${{» — «$${{»)"));
            let journal = format!("{:?}", ctx.journal.entries());
            assert!(!journal.contains(typed) && !journal.contains(fragment), "{journal}");
        }
    }

    #[tokio::test]
    async fn node_saves_the_text_and_stops_on_cancel() {
        let server = fake(mailbox(&[1, 2, 3, 4])).await;
        let mut ctx = Ctx::bare();
        ctx.store = Some(Arc::new(memory_store()));
        let flow = one(config(server.port, json!({ "from": "instagram", "extract": "\\d{6}", "save_to": "code" })));
        ctx.run(&flow).await.unwrap();
        assert_eq!(ctx.scope.get("code"), Some(&json!("654321")));
        let line = ctx.journal.entries().iter().map(|e| format!("{e:?}")).find(|e| e.contains("password")).unwrap();
        assert!(line.contains("password: задан") && !line.contains("pa$$word"), "{line}");
        let result = Ctx::bare().run(&flow).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "Email: у запуска нет настроек сервисов"), "{result:?}");
        // «Стоп» прерывает ожидание молчащего сервера.
        let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = silent.local_addr().unwrap().port();
        let _keep = tokio::spawn(async move {
            let _connection = silent.accept().await;
            std::future::pending::<()>().await
        });
        let mut ctx = Ctx::bare();
        ctx.store = Some(Arc::new(memory_store()));
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            cancel.cancel();
        });
        let started = std::time::Instant::now();
        let result = ctx.run(&one(config(port, json!({})))).await;
        assert_eq!(result, Err(Fail::Cancelled));
        assert!(started.elapsed() < LIMIT, "{:?}", started.elapsed());
    }
}
