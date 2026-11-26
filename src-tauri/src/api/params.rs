//! Параметры запроса Local API: query и JSON-тело, имена без учёта регистра, тело важнее query,
//! значения строкой или числом (спецификация 2Б, «Параметры»).
use super::envelope::{ApiFailure, fail};
use axum::body::Bytes;
use axum::extract::{FromRequest, Query, Request};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Map, Value};
use std::fmt;

pub struct Params {
    /// Имена — в нижнем регистре: в документации AdsPower встречаются `Proxy_id` и `Profile_id`.
    values: Map<String, Value>,
}

// В значениях бывают пароли и ключ 2FA: `Debug` показывает только имена.
impl fmt::Debug for Params {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Params").field("names", &self.values.keys().collect::<Vec<_>>()).finish()
    }
}

impl<S: Send + Sync> FromRequest<S> for Params {
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let mut values = Map::new();
        let query: Query<Vec<(String, String)>> =
            Query::try_from_uri(request.uri()).map_err(|_| fail("invalid query string"))?;
        for (name, value) in query.0 {
            values.insert(name.to_ascii_lowercase(), Value::String(value));
        }
        let RawBody(body) = RawBody::from_request(request, state).await?;
        if !body.iter().all(u8::is_ascii_whitespace) {
            match serde_json::from_slice::<Value>(&body) {
                Ok(Value::Object(map)) => {
                    for (name, value) in map {
                        values.insert(name.to_ascii_lowercase(), value);
                    }
                }
                _ => return Err(fail("request body must be a JSON object")),
            }
        }
        Ok(Self { values })
    }
}

/// Тело запроса как есть (Add Proxy: массив в теле). Тело больше предела axum (2 МБ) или не прочитанное — ответ
/// в формате AdsPower, а не текст axum.
pub struct RawBody(pub Bytes);

impl<S: Send + Sync> FromRequest<S> for RawBody {
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        Bytes::from_request(request, state).await.map(RawBody).map_err(|rejection| {
            if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                fail("request body is too large")
            } else {
                fail("could not read request body")
            }
        })
    }
}

impl Params {
    /// Для тестов и вложенных объектов (`user_proxy_config`, элементы массива при добавлении прокси).
    pub fn from_map(map: Map<String, Value>) -> Self {
        Self { values: map.into_iter().map(|(name, value)| (name.to_ascii_lowercase(), value)).collect() }
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(&name.to_ascii_lowercase())
    }

    /// Строка или число — строкой; `true`/`false` — `"1"`/`"0"`; пустая строка, `null`, массив, объект — `None`.
    /// Строка не обрезается: так приходят пароли.
    pub fn text(&self, name: &str) -> Option<String> {
        scalar_text(self.get(name)?)
    }

    pub fn require_text(&self, name: &str) -> Result<String, ApiFailure> {
        self.text(name).ok_or_else(|| ApiFailure(format!("{name} is required")))
    }

    pub fn int(&self, name: &str) -> Result<Option<i64>, ApiFailure> {
        let invalid = || ApiFailure(format!("{name} must be a number"));
        match self.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(n)) => n
                .as_i64()
                .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
                .map(Some)
                .ok_or_else(invalid),
            Some(Value::String(text)) if text.trim().is_empty() => Ok(None),
            Some(Value::String(text)) => text.trim().parse().map(Some).map_err(|_| invalid()),
            Some(_) => Err(invalid()),
        }
    }

    /// `"1"`, `1`, `true` — да; `"0"`, `0`, `false` — нет.
    pub fn flag(&self, name: &str) -> Result<Option<bool>, ApiFailure> {
        let invalid = || ApiFailure(format!("{name} must be 0 or 1"));
        match self.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(*value)),
            Some(value @ (Value::String(_) | Value::Number(_))) => match scalar_text(value).as_deref().map(str::trim) {
                None | Some("") => Ok(None),
                Some("1" | "true") => Ok(Some(true)),
                Some("0" | "false") => Ok(Some(false)),
                Some(_) => Err(invalid()),
            },
            Some(_) => Err(invalid()),
        }
    }

    /// Массив строк и чисел, строка с JSON-массивом (так `launch_args` приходит в query) или одно значение.
    pub fn list(&self, name: &str) -> Result<Option<Vec<String>>, ApiFailure> {
        let invalid = || ApiFailure(format!("{name} must be a list"));
        let items = match self.get(name) {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::String(text)) if text.trim().is_empty() => return Ok(None),
            Some(Value::String(text)) if text.trim_start().starts_with('[') => {
                serde_json::from_str::<Vec<Value>>(text).map_err(|_| invalid())?
            }
            Some(Value::Array(items)) => items.clone(),
            Some(single @ (Value::String(_) | Value::Number(_))) => vec![single.clone()],
            Some(_) => return Err(invalid()),
        };
        items
            .iter()
            .map(|item| match item {
                Value::String(text) => Ok(text.clone()),
                Value::Number(n) => Ok(n.to_string()),
                _ => Err(invalid()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    /// Объект или строка с JSON-объектом (так `user_sort` приходит в query).
    pub fn object(&self, name: &str) -> Result<Option<Map<String, Value>>, ApiFailure> {
        let invalid = || ApiFailure(format!("{name} must be an object"));
        match self.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Object(map)) => Ok(Some(map.clone())),
            Some(Value::String(text)) if text.trim().is_empty() => Ok(None),
            Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
                Ok(Value::Object(map)) => Ok(Some(map)),
                _ => Err(invalid()),
            },
            Some(_) => Err(invalid()),
        }
    }
}

/// Страница списка: номер с 1 и размер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paging {
    pub page: i64,
    pub size: i64,
}

impl Paging {
    /// Сколько записей пропустить до этой страницы.
    pub fn offset(&self) -> usize {
        usize::try_from((self.page - 1).saturating_mul(self.size)).unwrap_or(usize::MAX)
    }

    /// Сколько записей взять.
    pub fn limit(&self) -> usize {
        usize::try_from(self.size).unwrap_or(0)
    }
}

impl Params {
    /// `page` (по умолчанию 1, не меньше 1) и размер страницы под именем `size_name`
    /// (по умолчанию `default_size`, от 1 до `max_size`), как в списках AdsPower.
    pub fn paging(&self, size_name: &str, default_size: i64, max_size: i64) -> Result<Paging, ApiFailure> {
        let page = self.int("page")?.unwrap_or(1);
        if page < 1 {
            return Err(ApiFailure("page must be at least 1".into()));
        }
        let size = self.int(size_name)?.unwrap_or(default_size);
        if !(1..=max_size).contains(&size) {
            return Err(ApiFailure(format!("{size_name} must be between 1 and {max_size}")));
        }
        Ok(Paging { page, size })
    }
}

pub(super) fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(value) => Some(if *value { "1" } else { "0" }.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use serde_json::json;

    fn params(value: Value) -> Params {
        Params::from_map(value.as_object().unwrap().clone())
    }

    async fn extract(method: &str, uri: &str, body: &str) -> Result<Params, (u16, Value)> {
        let request = axum::http::Request::builder().method(method).uri(uri).body(Body::from(body.to_string())).unwrap();
        match Params::from_request(request, &()).await {
            Ok(params) => Ok(params),
            Err(response) => {
                let status = response.status().as_u16();
                let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
                Err((status, serde_json::from_slice(&bytes).unwrap()))
            }
        }
    }

    #[tokio::test]
    async fn names_ignore_case_and_body_beats_query() {
        let p = extract("POST", "/x?USER_ID=q&serial_number=5", r#"{"User_Id": 7, "Profile_id": ["a"]}"#).await.unwrap();
        assert_eq!(p.text("user_id").as_deref(), Some("7"), "тело важнее query");
        assert_eq!(p.text("Serial_Number").as_deref(), Some("5"));
        assert_eq!(p.list("profile_id").unwrap(), Some(vec!["a".to_string()]));
    }

    #[tokio::test]
    async fn query_only_and_empty_body() {
        let p = extract("GET", "/x?user_id=h1yynkm&launch_args=%5B%22--a%22%5D", "").await.unwrap();
        assert_eq!(p.text("user_id").as_deref(), Some("h1yynkm"));
        assert_eq!(p.list("launch_args").unwrap(), Some(vec!["--a".to_string()]));
        assert!(extract("POST", "/x", "  \n").await.unwrap().get("user_id").is_none());
    }

    #[tokio::test]
    async fn body_must_be_a_json_object() {
        for body in ["[1]", "{", "\"text\""] {
            assert_eq!(
                extract("POST", "/x", body).await.err(),
                Some((200, json!({"code": -1, "data": {}, "msg": "request body must be a JSON object"}))),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn oversized_body_is_reported_in_the_envelope() {
        let big = format!("{{\"remark\": \"{}\"}}", "x".repeat(3 * 1024 * 1024));
        let too_large = (200, json!({"code": -1, "data": {}, "msg": "request body is too large"}));
        assert_eq!(extract("POST", "/x", &big).await.err(), Some(too_large.clone()));
        let request = axum::http::Request::builder().method("POST").uri("/x").body(Body::from(big)).unwrap();
        let Err(rejected) = RawBody::from_request(request, &()).await else { panic!("тело больше предела принято") };
        let status = rejected.status().as_u16();
        let bytes = axum::body::to_bytes(rejected.into_body(), usize::MAX).await.unwrap();
        assert_eq!((status, serde_json::from_slice::<Value>(&bytes).unwrap()), too_large, "Add Proxy");
    }

    #[test]
    fn text_reads_strings_numbers_and_switches() {
        let p = params(json!({"a": "x", "b": 12, "c": true, "d": "", "e": null, "f": [1], "g": " pass "}));
        assert_eq!(p.text("a").as_deref(), Some("x"));
        assert_eq!(p.text("b").as_deref(), Some("12"));
        assert_eq!(p.text("c").as_deref(), Some("1"));
        assert_eq!(p.text("d"), None);
        assert_eq!(p.text("e"), None);
        assert_eq!(p.text("f"), None);
        assert_eq!(p.text("g").as_deref(), Some(" pass "), "пароли не обрезаются");
        assert_eq!(p.require_text("d"), Err(ApiFailure("d is required".into())));
        assert_eq!(p.require_text("missing"), Err(ApiFailure("missing is required".into())));
    }

    #[test]
    fn numbers_from_strings_and_numbers() {
        let p = params(json!({"a": "42", "b": 42, "c": " ", "d": "x", "e": 4.0, "f": 4.5, "g": [1]}));
        assert_eq!(p.int("a"), Ok(Some(42)));
        assert_eq!(p.int("b"), Ok(Some(42)));
        assert_eq!(p.int("c"), Ok(None));
        assert_eq!(p.int("d"), Err(ApiFailure("d must be a number".into())));
        assert_eq!(p.int("e"), Ok(Some(4)));
        assert_eq!(p.int("f"), Err(ApiFailure("f must be a number".into())));
        assert_eq!(p.int("g"), Err(ApiFailure("g must be a number".into())));
        assert_eq!(p.int("missing"), Ok(None));
    }

    #[test]
    fn flags_accept_adspower_switches() {
        let p = params(json!({"a": "1", "b": 1, "c": true, "d": "0", "e": 0, "f": false, "g": "yes", "h": [1], "i": ""}));
        for name in ["a", "b", "c"] {
            assert_eq!(p.flag(name), Ok(Some(true)), "{name}");
        }
        for name in ["d", "e", "f"] {
            assert_eq!(p.flag(name), Ok(Some(false)), "{name}");
        }
        assert_eq!(p.flag("g"), Err(ApiFailure("g must be 0 or 1".into())));
        assert_eq!(p.flag("h"), Err(ApiFailure("h must be 0 or 1".into())));
        assert_eq!(p.flag("i"), Ok(None));
        assert_eq!(p.flag("missing"), Ok(None));
    }

    #[test]
    fn lists_from_arrays_json_strings_and_single_values() {
        let p = params(json!({
            "a": ["x", 1], "b": "[\"--x\",\"--y\"]", "c": "single", "d": 5, "e": "", "f": [{}], "g": {}, "h": []
        }));
        assert_eq!(p.list("a"), Ok(Some(vec!["x".to_string(), "1".to_string()])));
        assert_eq!(p.list("b"), Ok(Some(vec!["--x".to_string(), "--y".to_string()])));
        assert_eq!(p.list("c"), Ok(Some(vec!["single".to_string()])));
        assert_eq!(p.list("d"), Ok(Some(vec!["5".to_string()])));
        assert_eq!(p.list("e"), Ok(None));
        assert_eq!(p.list("f"), Err(ApiFailure("f must be a list".into())));
        assert_eq!(p.list("g"), Err(ApiFailure("g must be a list".into())));
        assert_eq!(p.list("h"), Ok(Some(Vec::new())));
    }

    #[test]
    fn objects_from_objects_and_json_strings() {
        let p = params(json!({
            "a": {"proxy_soft": "no_proxy"}, "b": "{\"serial_number\":\"desc\"}", "c": "", "d": "[1]", "e": 5
        }));
        assert_eq!(p.object("a").unwrap().unwrap()["proxy_soft"], json!("no_proxy"));
        assert_eq!(p.object("b").unwrap().unwrap()["serial_number"], json!("desc"));
        assert_eq!(p.object("c"), Ok(None));
        assert_eq!(p.object("d"), Err(ApiFailure("d must be an object".into())));
        assert_eq!(p.object("e"), Err(ApiFailure("e must be an object".into())));
    }

    #[test]
    fn debug_shows_names_but_not_values() {
        let text = format!("{:?}", params(json!({"password": "hunter2", "fakey": "JBSWY3DP"})));
        assert!(text.contains("password") && text.contains("fakey"), "{text}");
        assert!(!text.contains("hunter2") && !text.contains("JBSWY3DP"), "{text}");
    }
}
