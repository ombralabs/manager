//! Формат ответа AdsPower: `{"code":0,"data":{…},"msg":"success"}` или `{"code":-1,"data":{},"msg":"…"}`, HTTP 200
//! (спецификация 2Б, «Формат ответа»). `msg` — по-английски.
use crate::app::CommandError;
use crate::store::StoreError;
use crate::supervisor::SupervisorError;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use std::sync::OnceLock;

pub const API_SUCCESS: &str = "success";

/// Результат обработчика Local API: данные для `data` или текст ошибки для `msg`.
pub type ApiResult = Result<Data, ApiFailure>;

/// Успешный ответ: содержимое `data`.
pub struct Data(pub Value);

impl IntoResponse for Data {
    fn into_response(self) -> Response {
        ok(self.0)
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        fail(self.0)
    }
}

/// Ошибка Local API — готовый английский текст для `msg`. Логины, пароли и значения cookies сюда не попадают.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiFailure(pub String);

impl From<CommandError> for ApiFailure {
    fn from(e: CommandError) -> Self {
        Self(error_message(&e))
    }
}

impl From<StoreError> for ApiFailure {
    fn from(e: StoreError) -> Self {
        CommandError::from(e).into()
    }
}

impl From<SupervisorError> for ApiFailure {
    fn from(e: SupervisorError) -> Self {
        CommandError::from(e).into()
    }
}

pub fn ok(data: Value) -> Response {
    Json(json!({"code": 0, "data": data, "msg": API_SUCCESS})).into_response()
}

pub fn fail(msg: impl Into<String>) -> Response {
    Json(json!({"code": -1, "data": {}, "msg": msg.into()})).into_response()
}

/// Неизвестный путь — HTTP 404 в том же формате.
pub fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"code": -1, "data": {}, "msg": "unknown endpoint"}))).into_response()
}

/// Число или идентификатор — строкой, как в примерах AdsPower.
pub fn s(value: impl ToString) -> Value {
    Value::String(value.to_string())
}

/// Английский текст кода ошибки; поля формы — в скобках, именами ядра: `Check the form fields (name: required)`.
pub fn error_message(e: &CommandError) -> String {
    error_message_named(e, str::to_string)
}

/// То же, но поле называется так, как его прислал скрипт: `name` переводит имя поля ядра в имя запроса AdsPower
/// (решение заказчика 2В-3). Ядро и интерфейс знают свои имена, перевод — только здесь, в `msg`.
pub fn error_message_named(e: &CommandError, name: impl Fn(&str) -> String) -> String {
    if e.code == "FreeDeviceBound"
        && let Some(hint) = crate::engine::masked_owner_hint(e.owner_hint.as_deref()) {
        return english("FreeDeviceBoundAccount").replace("{account}", &hint);
    }
    let text = english(&e.code);
    match &e.fields {
        Some(fields) if !fields.is_empty() => {
            let fields: Vec<String> = fields.iter().map(|f| format!("{}: {}", name(f.field), f.code)).collect();
            format!("{text} ({})", fields.join(", "))
        }
        _ => text,
    }
}

/// `errors.<код>` из английского словаря интерфейса; словарь читается один раз.
fn english(code: &str) -> String {
    static ERRORS: OnceLock<Map<String, Value>> = OnceLock::new();
    let errors = ERRORS.get_or_init(|| {
        let dictionary: Value =
            serde_json::from_str(include_str!("../../../src/i18n/en.json")).expect("en.json — корректный JSON");
        dictionary["errors"].as_object().cloned().unwrap_or_default()
    });
    errors
        .get(code)
        .or_else(|| errors.get("Unknown"))
        .and_then(Value::as_str)
        .unwrap_or("Unknown error")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::FieldError;
    use crate::supervisor::SupervisorError;
    use serde_json::json;

    async fn read(response: Response) -> (u16, Value) {
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn success_and_failure_envelopes() {
        assert_eq!(
            read(ok(json!({"id": "k7f2q9"}))).await,
            (200, json!({"code": 0, "data": {"id": "k7f2q9"}, "msg": "success"}))
        );
        assert_eq!(read(fail("failed")).await, (200, json!({"code": -1, "data": {}, "msg": "failed"})));
        assert_eq!(read(not_found()).await, (404, json!({"code": -1, "data": {}, "msg": "unknown endpoint"})));
        assert_eq!(read(Data(json!({})).into_response()).await, (200, json!({"code": 0, "data": {}, "msg": "success"})));
        assert_eq!(
            read(ApiFailure("user_id is required".into()).into_response()).await,
            (200, json!({"code": -1, "data": {}, "msg": "user_id is required"}))
        );
    }

    #[test]
    fn numbers_and_ids_become_strings() {
        assert_eq!(s(5), json!("5"));
        assert_eq!(s(50325_u16), json!("50325"));
        assert_eq!(s("k7f2q9"), json!("k7f2q9"));
    }

    #[test]
    fn device_denials_in_local_api_include_only_a_masked_owner() {
        let error = CommandError::from(crate::engine::EngineError::FreeDeviceBound { owner_hint: Some("iv***v@example.com".into()) });
        assert!(error_message(&error).contains("iv***v@example.com"));
        let unsafe_error = CommandError::from(crate::engine::EngineError::FreeDeviceBound { owner_hint: Some("ivan@example.com".into()) });
        assert!(!unsafe_error.message.contains("ivan@example.com"));
        assert!(!error_message(&unsafe_error).contains("ivan@example.com"));
    }

    #[test]
    fn command_errors_become_english_messages() {
        let limit = CommandError::new("AlreadyRunning", "профиль уже запущен");
        assert_eq!(ApiFailure::from(limit).0, "The profile is already running");
        let fields = CommandError::validation(vec![
            FieldError { field: "name", code: "required" },
            FieldError { field: "start_urls", code: "invalid" },
        ]);
        assert_eq!(error_message(&fields), "Check the form fields (name: required, start_urls: invalid)");
        assert_eq!(ApiFailure::from(CommandError::new("NoSuchCode", "что-то")).0, "Unknown error");
        assert_eq!(ApiFailure::from(SupervisorError::NoFreeProxy).0, "No free enabled proxies");
        let not_found = ApiFailure::from(CommandError::new("NotFound", "профиль не найден"));
        assert_eq!(not_found.0, "Profile not found");
        assert!(!not_found.0.contains("профиль"), "русский текст ядра в msg не попадает");
    }

    #[test]
    fn field_names_follow_the_caller() {
        let fields = CommandError::validation(vec![
            FieldError { field: "start_urls", code: "invalid" },
            FieldError { field: "proxy_host", code: "invalid" },
        ]);
        let named = error_message_named(&fields, |field| match field {
            "start_urls" => "open_urls".to_string(),
            other => format!("user_proxy_config.{other}"),
        });
        assert_eq!(named, "Check the form fields (open_urls: invalid, user_proxy_config.proxy_host: invalid)");
        assert_eq!(
            error_message(&fields),
            "Check the form fields (start_urls: invalid, proxy_host: invalid)",
            "без перевода — имена ядра"
        );
        let limit = CommandError::new("AlreadyRunning", "профиль уже запущен");
        assert_eq!(error_message_named(&limit, |_| unreachable!("у ошибки без полей имён нет")), error_message(&limit));
    }
}
