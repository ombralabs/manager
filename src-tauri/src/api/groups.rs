//! Группы в формате AdsPower: New Group, Edit Group, Query Group (спецификация 2Б, «Группы»).

use super::envelope::{ApiFailure, ApiResult, Data, s};
use super::params::Params;
use super::server::both;
use crate::app::groups::GroupForm;
use crate::app::Core;
use crate::sys::unix_now;
use crate::engine::Engine;
use crate::store::fold;
use axum::Router;
use axum::extract::State;
use serde_json::{Value, json};
use std::sync::Arc;

/// Цвет группы, созданной через Local API: акцент палитры холста. Сменить его можно в интерфейсе.
pub(super) const GROUP_COLOR: &str = "#2447c9";
/// Query Group: больше групп на странице AdsPower не отдаёт.
pub(super) const GROUP_PAGE_MAX: i64 = 2000;

/// Маршруты групп; `server::api_routes` подключает их до слоя проверок.
pub(super) fn routes<E: Engine>() -> Router<Arc<Core<E>>> {
    Router::new()
        .route("/api/v1/group/create", both(create::<E>))
        .route("/api/v1/group/update", both(update::<E>))
        .route("/api/v1/group/list", both(list::<E>))
}

fn group_json(id: i64, name: String, note: String) -> Value {
    json!({ "group_id": s(id), "group_name": name, "remark": note })
}

/// `POST /api/v1/group/create` — `group_name`, `remark`.
pub(super) async fn create<E: Engine>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let form = GroupForm {
        name: params.require_text("group_name")?,
        color: GROUP_COLOR.to_string(),
        note: params.text("remark").unwrap_or_default(),
        quick_access: false,
    };
    let group = core.create_group(&form, unix_now())?;
    Ok(Data(group_json(group.id, group.name, group.note)))
}

/// `POST /api/v1/group/update` — `group_id`, `group_name`, `remark`. Меняет имя и, если передана, заметку; цвет и «быстрый доступ» остаются.
pub(super) async fn update<E: Engine>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let id = params.int("group_id")?.ok_or_else(|| ApiFailure("group_id is required".into()))?;
    let name = params.require_text("group_name")?;
    let group = core
        .store
        .get_group(id)?
        .ok_or_else(|| ApiFailure(format!("Group not found: {id}")))?;
    // `remark: ""` очищает заметку, отсутствие поля — оставляет прежнюю.
    let note = match params.get("remark") {
        Some(_) => params.text("remark").unwrap_or_default(),
        None => group.note,
    };
    core.update_group(id, &GroupForm { name, color: group.color, note, quick_access: group.quick_access })?;
    Ok(Data(json!({})))
}

/// `GET /api/v1/group/list` — `group_name`, `page`, `page_size`. Созданные группы по порядку создания; «Без группы» (`"0"`) в список не входит.
pub(super) async fn list<E: Engine>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let paging = params.paging("page_size", 1, GROUP_PAGE_MAX)?;
    let needle = params.text("group_name").map(|name| fold(name.trim()));
    let mut groups = core.list_groups()?.groups;
    groups.sort_by_key(|group| group.id);
    let list: Vec<Value> = groups
        .into_iter()
        .filter(|group| needle.as_deref().is_none_or(|needle| fold(&group.name).contains(needle)))
        .skip(paging.offset())
        .take(paging.limit())
        .map(|group| group_json(group.id, group.name, group.note))
        .collect();
    Ok(Data(json!({ "list": list, "page": paging.page, "page_size": paging.size })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::{api_env, keys, names};
    use crate::app::CommandError;

    /// Текст ошибки ядра в `msg`, как его строит `From<CommandError> for ApiFailure`.
    fn failure(code: &str) -> String {
        ApiFailure::from(CommandError::new(code, "")).0
    }

    #[tokio::test]
    async fn group_is_created_in_adspower_shape() {
        let api = api_env().await;
        let created = api.post("/api/v1/group/create", json!({"group_name": " Магазины ", "remark": "витрины"})).await;
        assert_eq!((created["code"].as_i64(), created["msg"].as_str()), (Some(0), Some("success")), "{created}");
        let data = &created["data"];
        assert_eq!(keys(data), names(&["group_id", "group_name", "remark"]));
        assert_eq!((data["group_name"].as_str(), data["remark"].as_str()), (Some("Магазины"), Some("витрины")));
        let id: i64 = data["group_id"].as_str().expect("group_id строкой").parse().unwrap();
        let group = api.core.store.get_group(id).unwrap().unwrap();
        assert_eq!((group.color.as_str(), group.quick_access), (GROUP_COLOR, false));
    }

    #[tokio::test]
    async fn group_name_is_required_and_unique_ignoring_case() {
        let api = api_env().await;
        assert_eq!(api.post("/api/v1/group/create", json!({"group_name": "Магазины"})).await["code"], 0);
        let again = api.post("/api/v1/group/create", json!({"Group_Name": "МАГАЗИНЫ"})).await;
        assert_eq!((again["code"].as_i64(), again["msg"].as_str()), (Some(-1), Some(failure("GroupExists").as_str())), "{again}");
        assert_eq!(again["data"], json!({}));
        let missing = api.post("/api/v1/group/create", json!({"remark": "без имени"})).await;
        assert_eq!(missing["msg"], "group_name is required");
        assert_eq!(api.core.list_groups().unwrap().groups.len(), 1);
    }

    #[tokio::test]
    async fn group_update_keeps_color_and_untouched_remark() {
        let api = api_env().await;
        let created = api.post("/api/v1/group/create", json!({"group_name": "Магазины", "remark": "витрины"})).await;
        let id = created["data"]["group_id"].as_str().unwrap().to_string();
        let numeric: i64 = id.parse().unwrap();
        let painted = GroupForm { name: "Магазины".into(), color: "#0b6e6e".into(), note: "витрины".into(), quick_access: true };
        api.core.update_group(numeric, &painted).unwrap();

        let renamed = api.post("/api/v1/group/update", json!({"group_id": numeric, "group_name": "Склады"})).await;
        assert_eq!((renamed["code"].as_i64(), &renamed["data"]), (Some(0), &json!({})), "{renamed}");
        let group = api.core.store.get_group(numeric).unwrap().unwrap();
        assert_eq!(
            (group.name.as_str(), group.note.as_str(), group.color.as_str(), group.quick_access),
            ("Склады", "витрины", "#0b6e6e", true),
            "без remark заметка, цвет и «быстрый доступ» остаются"
        );

        let cleared = api.post("/api/v1/group/update", json!({"group_id": id, "group_name": "Склады", "remark": ""})).await;
        assert_eq!(cleared["code"], 0, "{cleared}");
        assert_eq!(api.core.store.get_group(numeric).unwrap().unwrap().note, "");

        let unknown = api.post("/api/v1/group/update", json!({"group_id": "999", "group_name": "Нет"})).await;
        assert_eq!(unknown["msg"], "Group not found: 999");
        let no_name = api.post("/api/v1/group/update", json!({"group_id": id})).await;
        assert_eq!(no_name["msg"], "group_name is required");
        let no_id = api.post("/api/v1/group/update", json!({"group_name": "Склады"})).await;
        assert_eq!(no_id["msg"], "group_id is required");
    }

    #[tokio::test]
    async fn group_list_searches_by_part_of_name_and_pages() {
        let api = api_env().await;
        for name in ["Shops 1", "shops 2", "Ads"] {
            assert_eq!(api.post("/api/v1/group/create", json!({"group_name": name})).await["code"], 0);
        }

        let first = api.get("/api/v1/group/list").await;
        assert_eq!(first["code"], 0, "{first}");
        assert_eq!(keys(&first["data"]), names(&["list", "page", "page_size"]));
        assert_eq!((first["data"]["page"].as_i64(), first["data"]["page_size"].as_i64()), (Some(1), Some(1)));
        let list = first["data"]["list"].as_array().unwrap();
        assert_eq!(list.len(), 1, "по умолчанию одна группа на странице, как у AdsPower");
        assert_eq!(keys(&list[0]), names(&["group_id", "group_name", "remark"]));
        assert_eq!(list[0]["group_name"], "Shops 1", "порядок — по созданию");
        assert!(list[0]["group_id"].is_string());

        let shops = api.get("/api/v1/group/list?group_name=SHOP&page_size=10").await;
        let names: Vec<&str> =
            shops["data"]["list"].as_array().unwrap().iter().map(|g| g["group_name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Shops 1", "shops 2"]);
        let second = api.get("/api/v1/group/list?page=2&page_size=2").await;
        assert_eq!(second["data"]["list"].as_array().unwrap().len(), 1);
        assert_eq!(second["data"]["list"][0]["group_name"], "Ads");

        assert_eq!(api.get("/api/v1/group/list?page_size=2001").await["msg"], "page_size must be between 1 and 2000");
        assert_eq!(api.get("/api/v1/group/list?page=0").await["msg"], "page must be at least 1");
    }
}
