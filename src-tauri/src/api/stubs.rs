//! Операции AdsPower, которых в Ombra нет: отвечают ошибкой с понятным `msg` (спецификация 2Б, «Нереализуемые операции»).
use super::envelope::fail;
use super::server::both;
use axum::Router;
use axum::response::Response;

pub const NEEDS_SERVER: &str = "not supported by Ombra: requires a server";

pub(super) fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/api/v2/browser-profile/share", both(needs_server))
        .route("/api/v1/browser/cloud-active", both(needs_server))
}

async fn needs_server() -> Response {
    fail(NEEDS_SERVER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::api_env;
    use serde_json::json;

    #[tokio::test]
    async fn unsupported_operations_answer_with_clear_errors() {
        let env = api_env().await;
        for path in ["/api/v2/browser-profile/share", "/api/v1/browser/cloud-active"] {
            assert_eq!(env.post(path, json!({})).await, json!({"code": -1, "data": {}, "msg": NEEDS_SERVER}), "{path}");
        }
        assert_eq!(NEEDS_SERVER, "not supported by Ombra: requires a server");
    }
}
