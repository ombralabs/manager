//! Наборы расширений в формате AdsPower: Extension Category List v1 и v2 (спецификация 3Б, «Local API»).
//! Набор 0 «Общий» в списки не входит: в AdsPower «0» — признак «как у команды», а не категория.
use super::envelope::{ApiResult, Data, s};
use super::params::Params;
use super::server::both;
use crate::app::Core;
use crate::engine::Engine;
use crate::store::extensions::{DEFAULT_CATEGORY, ExtCategory};
use axum::Router;
use axum::extract::State;
use serde_json::json;
use std::sync::Arc;

/// Category List v2: `limit` 1–100; для v1 справочник предела не называет — тот же.
const CATEGORY_PAGE_MAX: i64 = 100;

pub(super) fn routes<E: Engine>() -> Router<Arc<Core<E>>> {
    Router::new()
        .route("/api/v1/application/list", both(list_v1::<E>))
        .route("/api/v2/category/list", both(list_v2::<E>))
}

/// Созданные наборы по порядку создания.
fn categories<E: Engine>(core: &Core<E>) -> Result<Vec<ExtCategory>, crate::app::CommandError> {
    let mut list: Vec<ExtCategory> = core.list_ext_categories()?.into_iter().filter(|c| c.id != DEFAULT_CATEGORY).collect();
    list.sort_by_key(|c| c.id);
    Ok(list)
}

/// `GET /api/v1/application/list` — `page`, `page_size` (по умолчанию 1).
async fn list_v1<E: Engine>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let paging = params.paging("page_size", 1, CATEGORY_PAGE_MAX)?;
    let list: Vec<_> = categories(&core)?
        .into_iter()
        .skip(paging.offset())
        .take(paging.limit())
        .map(|c| json!({ "id": s(c.id), "name": c.name, "remark": c.remark }))
        .collect();
    Ok(Data(json!({ "list": list, "page": paging.page, "page_size": paging.size })))
}

/// `GET /api/v2/category/list` — `category_id`, `page`, `limit` (по умолчанию 1); с `category_id` — одна запись или пусто.
async fn list_v2<E: Engine>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let paging = params.paging("limit", 1, CATEGORY_PAGE_MAX)?;
    let only = params.int("category_id")?;
    let list: Vec<_> = categories(&core)?
        .into_iter()
        .filter(|c| only.is_none_or(|id| id == c.id))
        .skip(paging.offset())
        .take(paging.limit())
        .map(|c| json!({ "category_id": s(c.id), "category_name": c.name, "remark": c.remark }))
        .collect();
    Ok(Data(json!({ "list": list, "page": paging.page, "page_size": paging.size })))
}

#[cfg(test)]
mod tests {
    use crate::api::test_support::api_env;
    use serde_json::json;

    #[tokio::test]
    async fn category_lists_follow_adspower() {
        let api = api_env().await;
        assert_eq!(api.get("/api/v1/application/list").await, json!({"code": 0, "data": {"list": [], "page": 1, "page_size": 1}, "msg": "success"}));
        let a = api.core.create_ext_category("Магазин", "для магазина", 1).unwrap();
        let b = api.core.create_ext_category("Аккаунты", "", 2).unwrap();
        // По порядку создания; набор 0 «Общий» в список не входит.
        let v1 = api.get("/api/v1/application/list?page_size=10").await;
        assert_eq!(v1["data"]["list"], json!([
            {"id": a.id.to_string(), "name": "Магазин", "remark": "для магазина"},
            {"id": b.id.to_string(), "name": "Аккаунты", "remark": ""}
        ]));
        assert_eq!(api.get("/api/v1/application/list?page=2&page_size=1").await["data"]["list"][0]["name"], "Аккаунты");
        assert_eq!(api.get("/api/v1/application/list?page_size=101").await["msg"], "page_size must be between 1 and 100");
        let v2 = api.get("/api/v2/category/list?limit=100").await;
        assert_eq!(v2["data"]["list"][0], json!({"category_id": a.id.to_string(), "category_name": "Магазин", "remark": "для магазина"}));
        assert_eq!((v2["data"]["page"].clone(), v2["data"]["page_size"].clone()), (json!(1), json!(100)));
        assert_eq!(api.get("/api/v2/category/list").await["data"]["list"].as_array().unwrap().len(), 1, "limit по умолчанию 1");
        let one = api.get(&format!("/api/v2/category/list?category_id={}&limit=10", b.id)).await;
        assert_eq!(one["data"]["list"], json!([{"category_id": b.id.to_string(), "category_name": "Аккаунты", "remark": ""}]));
        assert_eq!(api.get("/api/v2/category/list?category_id=0&limit=10").await["data"]["list"], json!([]));
        assert_eq!(api.get("/api/v2/category/list?category_id=x").await["msg"], "category_id must be a number");
        assert_eq!(api.get("/api/v2/category/list?limit=0").await["msg"], "limit must be between 1 and 100");
    }
}
