//! `cookies.txt` в формате Netscape: 7 полей через табуляцию; `#HttpOnly_` перед доменом — признак httpOnly.
use super::{Cookie, FormatError, Parsed, SkipCode, Skipped, dedupe_last_wins};

const HEADER: &str = "# Netscape HTTP Cookie File\n";
const HTTP_ONLY_PREFIX: &str = "#HttpOnly_";

pub fn parse_netscape(text: &str) -> Result<Parsed, FormatError> {
    let mut cookies = Vec::new();
    let mut skipped = Vec::new();
    for (index, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let (line, http_only) = match line.strip_prefix(HTTP_ONLY_PREFIX) {
            Some(rest) => (rest, true),
            None => (line, false),
        };
        // Пустые строки и комментарии пропускаются молча.
        if line.trim().is_empty() || (!http_only && line.trim_start().starts_with('#')) {
            continue;
        }
        match from_line(line, http_only) {
            Ok(cookie) => cookies.push(cookie),
            Err(code) => skipped.push(Skipped::new(index + 1, code)),
        }
    }
    Ok(Parsed { cookies: dedupe_last_wins(cookies), skipped })
}

fn from_line(line: &str, http_only: bool) -> Result<Cookie, SkipCode> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != 7 && fields.len() != 6 {
        return Err(SkipCode::Unparsed);
    }
    let include_subdomains = flag(fields[1]).ok_or(SkipCode::Unparsed)?;
    let secure = flag(fields[3]).ok_or(SkipCode::Unparsed)?;
    let expiry = parse_expiry(fields[4]).ok_or(SkipCode::BadExpires)?;
    Cookie {
        domain: fields[0].to_string(),
        name: fields[5].to_string(),
        value: fields.get(6).copied().unwrap_or("").to_string(),
        path: fields[2].to_string(),
        expires: (expiry > 0).then_some(expiry),
        http_only,
        secure,
        same_site: None,
        host_only: !include_subdomains,
    }
    .validated()
}

fn flag(text: &str) -> Option<bool> {
    match text.trim().to_ascii_uppercase().as_str() {
        "TRUE" => Some(true),
        "FALSE" => Some(false),
        _ => None,
    }
}

/// Целое число секунд; дробную часть некоторые выгрузки пишут — она отбрасывается.
fn parse_expiry(text: &str) -> Option<i64> {
    let text = text.trim();
    text.parse::<i64>().ok().or_else(|| text.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v.trunc() as i64))
}

fn upper(flag: bool) -> &'static str {
    if flag { "TRUE" } else { "FALSE" }
}

pub fn to_netscape(cookies: &[Cookie]) -> String {
    let mut out = String::from(HEADER);
    for c in cookies {
        let prefix = if c.http_only { HTTP_ONLY_PREFIX } else { "" };
        out.push_str(&format!(
            "{prefix}{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            c.domain,
            upper(!c.host_only),
            c.path,
            upper(c.secure),
            c.expires.unwrap_or(0),
            c.name,
            c.value
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cookies::tests::cookie;

    #[test]
    fn parses_lines_with_http_only_and_session() {
        // Строки: 1 заголовок, 2 комментарий, 3 пустая, 4–6 cookies, 7–8 ошибочные.
        let text = "# Netscape HTTP Cookie File\n# комментарий\n\n\
                    .example.com\tTRUE\t/\tTRUE\t1767225600\tsid\tabc\n\
                    #HttpOnly_shop.example.com\tFALSE\t/cart\tFALSE\t0\tcart\t1\n\
                    example.org\tFALSE\t/\tFALSE\t0\tempty\n\
                    сломанная строка\n\
                    example.net\tMAYBE\t/\tFALSE\t0\tx\t1\r\n";
        let parsed = parse_netscape(text).unwrap();
        let mut sid = cookie(".example.com", "sid", "abc");
        sid.secure = true;
        sid.expires = Some(1_767_225_600);
        let mut cart = cookie("shop.example.com", "cart", "1");
        cart.http_only = true;
        cart.path = "/cart".into();
        let empty = cookie("example.org", "empty", "");
        assert_eq!(parsed.cookies, [sid, cart, empty]);
        assert_eq!(parsed.skipped.iter().map(|s| s.line).collect::<Vec<_>>(), [7, 8]);
    }

    #[test]
    fn netscape_round_trip() {
        let mut sid = cookie(".example.com", "sid", "abc");
        sid.secure = true;
        sid.http_only = true;
        sid.expires = Some(1_767_225_600);
        let all = vec![sid, cookie("example.org", "a", "")];
        let text = to_netscape(&all);
        assert!(text.starts_with("# Netscape HTTP Cookie File\n") && text.ends_with('\n'), "{text}");
        assert!(text.contains("#HttpOnly_.example.com\tTRUE\t/\tTRUE\t1767225600\tsid\tabc\n"), "{text}");
        assert_eq!(parse_netscape(&text).unwrap().cookies, all);
    }

    #[test]
    fn fractional_expiry_is_truncated() {
        let parsed = parse_netscape("example.com\tFALSE\t/\tFALSE\t1767225600.9\ta\t1\n").unwrap();
        assert_eq!(parsed.cookies[0].expires, Some(1_767_225_600));
    }
}
