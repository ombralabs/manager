//! JSON-массив cookies в формате AdsPower (как у расширения EditThisCookie).
use super::{Cookie, FormatError, Parsed, SameSite, SkipCode, Skipped, dedupe_last_wins};
use serde_json::{Map, Value, json};

pub fn parse_json(text: &str) -> Result<Parsed, FormatError> {
    let value: Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| FormatError::Json(e.to_string()))?;
    let Value::Array(items) = value else { return Err(FormatError::Unrecognized) };
    let mut cookies = Vec::new();
    let mut skipped = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match from_item(item) {
            Ok(cookie) => cookies.push(cookie),
            Err(code) => skipped.push(Skipped::new(index + 1, code)),
        }
    }
    Ok(Parsed { cookies: dedupe_last_wins(cookies), skipped })
}

fn from_item(item: &Value) -> Result<Cookie, SkipCode> {
    let Value::Object(map) = item else { return Err(SkipCode::Unparsed) };
    let domain = text(map, "domain").ok_or(SkipCode::NoDomain)?;
    let name = text(map, "name").ok_or(SkipCode::NoName)?;
    let expiration = map.get("expirationDate").and_then(Value::as_f64).filter(|e| e.is_finite());
    let session = map.get("session").and_then(Value::as_bool).unwrap_or(expiration.is_none());
    let host_only = map.get("hostOnly").and_then(Value::as_bool).unwrap_or(!domain.trim().starts_with('.'));
    Cookie {
        value: text(map, "value").unwrap_or_default(),
        path: text(map, "path").unwrap_or_else(|| "/".to_string()),
        expires: if session { None } else { expiration.map(|e| e.trunc() as i64) },
        http_only: flag(map, "httpOnly"),
        secure: flag(map, "secure"),
        same_site: map.get("sameSite").and_then(Value::as_str).and_then(SameSite::parse),
        host_only,
        domain,
        name,
    }
    .validated()
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).map(str::to_string)
}

fn flag(map: &Map<String, Value>, key: &str) -> bool {
    map.get(key).and_then(Value::as_bool).unwrap_or(false)
}

pub fn to_json(cookies: &[Cookie]) -> String {
    serde_json::to_string_pretty(&to_values(cookies)).expect("массив строк и чисел сериализуется")
}

/// Записи экспорта JSON по одной на cookie, в порядке `cookies`.
pub fn to_values(cookies: &[Cookie]) -> Vec<Value> {
    cookies
        .iter()
        .map(|c| {
            let same_site = match c.same_site {
                Some(SameSite::Strict) => "strict",
                Some(SameSite::Lax) => "lax",
                Some(SameSite::None) => "no_restriction",
                None => "unspecified",
            };
            let mut item = json!({
                "domain": c.domain,
                "hostOnly": c.host_only,
                "httpOnly": c.http_only,
                "name": c.name,
                "path": c.path,
                "sameSite": same_site,
                "secure": c.secure,
                "session": c.expires.is_none(),
                "value": c.value,
            });
            if let Some(expires) = c.expires {
                item["expirationDate"] = json!(expires);
            }
            item
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cookies::SameSite;
    use crate::cookies::tests::cookie;

    /// Выгрузка AdsPower (формат расширения EditThisCookie): лишние поля `storeId` и `id` не мешают.
    const ADSPOWER: &str = r#"[
      {"domain": ".example.com", "expirationDate": 1767225600.5, "hostOnly": false, "httpOnly": true, "name": "sid",
       "path": "/", "sameSite": "no_restriction", "secure": true, "session": false, "storeId": "0", "value": "abc", "id": 1},
      {"domain": "shop.example.com", "hostOnly": true, "httpOnly": false, "name": "cart", "path": "/cart",
       "sameSite": "unspecified", "secure": false, "session": true, "value": "1", "id": 2},
      {"name": "broken"},
      "строка"
    ]"#;

    #[test]
    fn parses_adspower_export() {
        let parsed = parse_json(ADSPOWER).unwrap();
        let sid = Cookie {
            domain: ".example.com".into(),
            name: "sid".into(),
            value: "abc".into(),
            path: "/".into(),
            expires: Some(1_767_225_600),
            http_only: true,
            secure: true,
            same_site: Some(SameSite::None),
            host_only: false,
        };
        let mut cart = cookie("shop.example.com", "cart", "1");
        cart.path = "/cart".into();
        assert_eq!(parsed.cookies, [sid, cart]);
        assert_eq!(parsed.skipped.iter().map(|s| s.line).collect::<Vec<_>>(), [3, 4]);
    }

    #[test]
    fn json_round_trip_keeps_every_field() {
        let mut strict = cookie(".example.com", "a", "1");
        strict.same_site = Some(SameSite::Strict);
        strict.expires = Some(1_800_000_000);
        strict.secure = true;
        let mut lax = cookie("example.com", "b", "");
        lax.same_site = Some(SameSite::Lax);
        lax.http_only = true;
        lax.path = "/app".into();
        let mut none = cookie("example.org", "c", "x=y; z");
        none.same_site = Some(SameSite::None);
        let session = cookie("example.net", "d", "4");
        let all = vec![strict, lax, none, session];
        let parsed = parse_json(&to_json(&all)).unwrap();
        assert_eq!(parsed.cookies, all);
        assert!(parsed.skipped.is_empty());
    }

    #[test]
    fn later_duplicate_wins_and_non_array_is_rejected() {
        let text = r#"[{"domain":"example.com","name":"a","value":"old"},{"domain":"EXAMPLE.com","name":"a","value":"new"}]"#;
        assert_eq!(parse_json(text).unwrap().cookies, [cookie("example.com", "a", "new")]);
        assert_eq!(parse_json(r#"{"cookies":[]}"#), Err(FormatError::Unrecognized));
        assert!(matches!(parse_json("[1,"), Err(FormatError::Json(_))));
    }
}
