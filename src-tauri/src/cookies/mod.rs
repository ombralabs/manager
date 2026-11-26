//! Cookies профиля без базы и браузера: модель, нормализация, хэш набора, импорт и экспорт
//! (спецификация 2А, «Cookies»). Значение cookie — секрет: в `Debug` и журналы не попадает.
pub mod json;
pub mod netscape;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub struct Cookie {
    pub domain: String,
    pub name: String,
    pub value: String,
    pub path: String,
    /// Секунды UNIX; `None` — сессионная.
    pub expires: Option<i64>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: Option<SameSite>,
    /// Только для этого хоста, без поддоменов; у такой cookie домен без точки в начале.
    pub host_only: bool,
}

impl fmt::Debug for Cookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cookie")
            .field("domain", &self.domain)
            .field("name", &self.name)
            .field("value", &"***")
            .field("path", &self.path)
            .field("expires", &self.expires)
            .field("http_only", &self.http_only)
            .field("secure", &self.secure)
            .field("same_site", &self.same_site)
            .field("host_only", &self.host_only)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

impl SameSite {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Lax => "lax",
            Self::None => "none",
        }
    }

    /// Без учёта регистра: strict, lax, none и no_restriction (так пишет Chrome-расширение) — значение;
    /// unspecified и всё прочее — «не задано».
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "strict" => Some(Self::Strict),
            "lax" => Some(Self::Lax),
            "none" | "no_restriction" => Some(Self::None),
            _ => None,
        }
    }
}

impl Cookie {
    /// Домен без пробелов и в нижнем регистре; точка в начале — ровно у не-host-only; пустой путь — `/`.
    /// Запись не годится, если в домене, имени, пути или значении управляющий символ (табуляция и перевод строки
    /// ломают `cookies.txt` при экспорте), домен пуст или с пробелом либо `/`, путь не с `/`. Ошибка — код причины.
    pub fn validated(mut self) -> Result<Cookie, SkipCode> {
        let fields = [&self.domain, &self.name, &self.path, &self.value];
        if fields.into_iter().any(|field| field.contains(char::is_control)) {
            return Err(SkipCode::Control);
        }
        let domain = self.domain.trim().trim_start_matches('.').to_lowercase();
        if domain.is_empty() {
            return Err(SkipCode::NoDomain);
        }
        if domain.contains(|c: char| c.is_whitespace() || c == '/') {
            return Err(SkipCode::BadDomain);
        }
        self.domain = if self.host_only { domain } else { format!(".{domain}") };
        if self.path.is_empty() {
            self.path = "/".to_string();
        }
        if !self.path.starts_with('/') {
            return Err(SkipCode::BadPath);
        }
        Ok(self)
    }

    /// `validated` без причины — для cookies из браузера (`engine::cdp`).
    pub fn normalized(self) -> Option<Cookie> {
        self.validated().ok()
    }
}

/// Ключ cookie в браузере и в базе.
pub(crate) fn key(cookie: &Cookie) -> (&str, &str, &str) {
    (&cookie.domain, &cookie.name, &cookie.path)
}

/// SHA-256 в hex по набору, отсортированному по (домен, имя, путь): порядок входа не важен.
pub fn set_hash(cookies: &[Cookie]) -> String {
    let mut sorted: Vec<&Cookie> = cookies.iter().collect();
    sorted.sort_by_key(|&cookie| key(cookie));
    let mut hasher = Sha256::new();
    for cookie in sorted {
        let expires = cookie.expires.map(|e| e.to_string()).unwrap_or_default();
        let flags = [cookie.http_only, cookie.secure, cookie.host_only].map(|f| if f { "1" } else { "0" }).concat();
        let same_site = cookie.same_site.map(SameSite::as_str).unwrap_or("");
        let fields = [
            cookie.domain.as_str(),
            cookie.name.as_str(),
            cookie.value.as_str(),
            cookie.path.as_str(),
            expires.as_str(),
            flags.as_str(),
            same_site,
        ];
        for field in fields {
            // Длина перед каждым полем: разные наборы не склеиваются в одинаковый поток байтов.
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
    }
    format!("{:x}", hasher.finalize())
}

/// Повторы ключа: остаётся последняя запись, на месте первой.
pub(crate) fn dedupe_last_wins(cookies: Vec<Cookie>) -> Vec<Cookie> {
    let mut out: Vec<Cookie> = Vec::with_capacity(cookies.len());
    let mut index: HashMap<(String, String, String), usize> = HashMap::new();
    for cookie in cookies {
        match index.entry((cookie.domain.clone(), cookie.name.clone(), cookie.path.clone())) {
            Entry::Occupied(slot) => out[*slot.get()] = cookie,
            Entry::Vacant(slot) => {
                slot.insert(out.len());
                out.push(cookie);
            }
        }
    }
    out
}

/// Почему запись импорта пропущена — код для интерфейса (`editor.cookies.skip.<код>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipCode {
    /// Нет поля `domain` или домен пуст.
    NoDomain,
    /// В домене пробел или `/`.
    BadDomain,
    /// Нет поля `name` (JSON).
    NoName,
    /// Путь не начинается с `/`.
    BadPath,
    /// Управляющие символы в домене, имени, пути или значении.
    Control,
    /// Срок — не число секунд.
    BadExpires,
    /// Запись не разобрана: элемент JSON — не объект, в строке не 6–7 полей, флаг не TRUE или FALSE.
    Unparsed,
}

impl SkipCode {
    /// Русский текст причины — для журнала.
    pub fn reason(self) -> &'static str {
        match self {
            Self::NoDomain => "нет домена",
            Self::BadDomain => "неверный домен",
            Self::NoName => "нет поля name",
            Self::BadPath => "путь не начинается с /",
            Self::Control => "управляющие символы",
            Self::BadExpires => "срок — не число секунд",
            Self::Unparsed => "запись не разобрана",
        }
    }
}

/// Пропущенная запись импорта: строка Netscape или номер элемента JSON-массива (с 1), код и текст причины.
/// Интерфейс переводит `code`; `reason` — для журнала.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Skipped {
    pub line: usize,
    pub code: SkipCode,
    pub reason: String,
}

impl Skipped {
    pub fn new(line: usize, code: SkipCode) -> Self {
        Self { line, code, reason: code.reason().to_string() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub cookies: Vec<Cookie>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CookieFormat {
    Json,
    Netscape,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    #[error("файл пустой")]
    Empty,
    #[error("неверный JSON: {0}")]
    Json(String),
    #[error("Поддерживаются JSON-массив cookies и файл cookies.txt")]
    Unrecognized,
}

/// Текст, начинающийся с `[` (после пробелов), — JSON, иначе Netscape.
/// Ни одной cookie и хотя бы одна ошибочная запись — формат не распознан.
pub fn parse(text: &str) -> Result<Parsed, FormatError> {
    let text = text.trim_start_matches('\u{feff}');
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(FormatError::Empty);
    }
    let parsed = if trimmed.starts_with('[') { json::parse_json(trimmed)? } else { netscape::parse_netscape(text)? };
    if parsed.cookies.is_empty() && !parsed.skipped.is_empty() {
        return Err(FormatError::Unrecognized);
    }
    Ok(parsed)
}

pub fn export(cookies: &[Cookie], format: CookieFormat) -> String {
    match format {
        CookieFormat::Json => json::to_json(cookies),
        CookieFormat::Netscape => netscape::to_netscape(cookies),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Cookie для тестов: путь «/», сессионная, без флагов; host-only, если домен без точки.
    pub(crate) fn cookie(domain: &str, name: &str, value: &str) -> Cookie {
        Cookie {
            domain: domain.into(),
            name: name.into(),
            value: value.into(),
            path: "/".into(),
            expires: None,
            http_only: false,
            secure: false,
            same_site: None,
            host_only: !domain.starts_with('.'),
        }
    }

    #[test]
    fn normalization_fixes_domain_dot_case_and_path() {
        let mut c = cookie(" Example.COM ", "sid", "1");
        c.host_only = false;
        c.path = String::new();
        let n = c.normalized().unwrap();
        assert_eq!((n.domain.as_str(), n.path.as_str()), (".example.com", "/"));

        let mut c = cookie(".Shop.example.com", "sid", "1");
        c.host_only = true;
        assert_eq!(c.normalized().unwrap().domain, "shop.example.com");

        for bad in [".", "", "  ", "a b.com", "example.com/x"] {
            assert_eq!(cookie(bad, "sid", "1").normalized(), None, "{bad:?}");
        }
        let mut c = cookie("example.com", "sid", "1");
        c.path = "cart".into();
        assert_eq!(c.normalized(), None);
    }

    #[test]
    fn control_characters_reject_the_cookie() {
        let mut bad_path = cookie("example.com", "sid", "1");
        bad_path.path = "/a\u{0}b".into();
        let bad = [
            cookie("exa\u{7}mple.com", "sid", "1"),
            cookie("example.com", "s\tid", "1"),
            cookie("example.com", "sid", "a\u{7f}b"),
            bad_path,
        ];
        for c in bad {
            assert_eq!(c.clone().normalized(), None, "{c:?}");
        }
        assert_eq!(
            cookie("example.com", "sid", "a b;c").normalized(),
            Some(cookie("example.com", "sid", "a b;c")),
            "пробел и `;` — не управляющие символы"
        );

        // В JSON `\n` — перевод строки в значении: запись пропускается с причиной, остальные принимаются.
        let parsed = parse(
            r#"[{"domain":"example.com","name":"a","value":"a\nb"},{"domain":"example.com","name":"b","value":"1"}]"#,
        )
        .unwrap();
        assert_eq!(parsed.cookies, [cookie("example.com", "b", "1")]);
        assert_eq!(parsed.skipped.iter().map(|s| s.line).collect::<Vec<_>>(), [1]);
        assert_eq!(parsed.skipped[0].code, SkipCode::Control, "{:?}", parsed.skipped);
    }

    #[test]
    fn set_hash_ignores_order_and_tracks_values() {
        let a = cookie("example.com", "a", "1");
        let b = cookie(".example.com", "b", "2");
        let hash = set_hash(&[a.clone(), b.clone()]);
        assert_eq!(hash.len(), 64);
        assert!(hash.bytes().all(|x| x.is_ascii_hexdigit()));
        assert_eq!(set_hash(&[b.clone(), a.clone()]), hash);
        let mut changed = a.clone();
        changed.value = "2".into();
        assert_ne!(set_hash(&[changed, b.clone()]), hash);
        let mut later = a.clone();
        later.expires = Some(1_767_225_600);
        assert_ne!(set_hash(&[later, b.clone()]), hash);
        let mut secure = a.clone();
        secure.secure = true;
        assert_ne!(set_hash(&[secure, b]), hash);
        assert_ne!(set_hash(&[]), hash);
    }

    #[test]
    fn same_site_is_parsed_loosely() {
        assert_eq!(SameSite::parse("Strict"), Some(SameSite::Strict));
        assert_eq!(SameSite::parse("LAX"), Some(SameSite::Lax));
        assert_eq!(SameSite::parse("no_restriction"), Some(SameSite::None));
        assert_eq!(SameSite::parse("None"), Some(SameSite::None));
        assert_eq!(SameSite::parse("unspecified"), None);
        assert_eq!(SameSite::parse(""), None);
        assert_eq!(SameSite::None.as_str(), "none");
    }

    #[test]
    fn parse_detects_format_and_rejects_garbage() {
        assert_eq!(parse("  \n "), Err(FormatError::Empty));
        assert!(matches!(parse("[{\"domain\":"), Err(FormatError::Json(_))));
        assert_eq!(parse("просто текст"), Err(FormatError::Unrecognized));
        assert_eq!(parse("[\"строка\"]"), Err(FormatError::Unrecognized));
        assert_eq!(parse("[]").unwrap(), Parsed { cookies: vec![], skipped: vec![] });
        let json = parse("\u{feff} [{\"domain\":\"example.com\",\"name\":\"a\",\"value\":\"1\"}]").unwrap();
        assert_eq!(json.cookies, [cookie("example.com", "a", "1")]);
        let netscape = parse("# Netscape HTTP Cookie File\nexample.com\tFALSE\t/\tFALSE\t0\ta\t1\n").unwrap();
        assert_eq!(netscape.cookies, [cookie("example.com", "a", "1")]);
        assert_eq!(parse("# только комментарий\n").unwrap().cookies, vec![]);
    }

    #[test]
    fn export_uses_the_requested_format() {
        let cookies = vec![cookie("example.com", "a", "1")];
        assert!(export(&cookies, CookieFormat::Netscape).starts_with("# Netscape HTTP Cookie File\n"));
        assert_eq!(parse(&export(&cookies, CookieFormat::Json)).unwrap().cookies, cookies);
    }

    #[test]
    fn debug_hides_value() {
        let text = format!("{:?}", cookie("example.com", "sid", "s3cr3t-value"));
        assert!(!text.contains("s3cr3t-value"), "{text}");
        assert!(text.contains("example.com") && text.contains("sid"), "{text}");
    }

    #[test]
    fn every_skip_reason_has_its_code() {
        let codes = |parsed: &Parsed| parsed.skipped.iter().map(|s| (s.line, s.code)).collect::<Vec<_>>();
        let from_json = json::parse_json(
            r#"[
              "строка",
              {"name": "a", "value": "1"},
              {"domain": "example.com", "value": "1"},
              {"domain": "exa mple.com", "name": "a"},
              {"domain": "example.com", "name": "a", "path": "cart"},
              {"domain": "example.com", "name": "a", "value": "a\nb"},
              {"domain": " . ", "name": "a"},
              {"domain": "example.com", "name": "ok", "value": "1"}
            ]"#,
        )
        .unwrap();
        assert_eq!(
            codes(&from_json),
            [
                (1, SkipCode::Unparsed),
                (2, SkipCode::NoDomain),
                (3, SkipCode::NoName),
                (4, SkipCode::BadDomain),
                (5, SkipCode::BadPath),
                (6, SkipCode::Control),
                (7, SkipCode::NoDomain),
            ]
        );
        assert_eq!(from_json.cookies.len(), 1);

        let from_netscape = netscape::parse_netscape(
            "example.com\tFALSE\t/\n\
             example.com\tMAYBE\t/\tFALSE\t0\ta\t1\n\
             example.com\tFALSE\t/\tFALSE\tзавтра\ta\t1\n\
             example.com\tFALSE\tcart\tFALSE\t0\ta\t1\n\
             exa mple.com\tFALSE\t/\tFALSE\t0\ta\t1\n\
             \tFALSE\t/\tFALSE\t0\ta\t1\n\
             example.com\tFALSE\t/\tFALSE\t0\ta\tx\u{7}y\n\
             example.com\tFALSE\t/\tFALSE\t0\tok\t1\n",
        )
        .unwrap();
        assert_eq!(
            codes(&from_netscape),
            [
                (1, SkipCode::Unparsed),
                (2, SkipCode::Unparsed),
                (3, SkipCode::BadExpires),
                (4, SkipCode::BadPath),
                (5, SkipCode::BadDomain),
                (6, SkipCode::NoDomain),
                (7, SkipCode::Control),
            ]
        );
        assert_eq!(from_netscape.cookies.len(), 1);
        assert_eq!(
            serde_json::to_value(&from_netscape.skipped[2]).unwrap(),
            serde_json::json!({"line": 3, "code": "bad_expires", "reason": SkipCode::BadExpires.reason()})
        );
    }
}
