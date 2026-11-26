//! Список прокси в формате AdsPower: Add, Update, Delete, Query Proxy (спецификация 2Б, «Прокси»).

use super::envelope::{ApiFailure, ApiResult, Data, error_message_named, s};
use super::params::{Params, RawBody};
use super::server::both;
use crate::app::forms::{SecretInput, validate_proxy_form};
use crate::app::proxies::ProxyEditForm;
use crate::app::{CommandError, Core};
use crate::engine::Engine;
use crate::store::profiles::ListQuery;
use crate::store::proxies::ProxyKind;
use crate::store::StoreError;
use axum::Router;
use axum::extract::State;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Query Proxy: больше записей на странице AdsPower не отдаёт.
pub(super) const PROXY_PAGE_MAX: i64 = 200;
/// Query Proxy: записей на странице по умолчанию.
const PROXY_PAGE_DEFAULT: i64 = 50;
/// Add Proxy: столько записей за один запрос.
pub(super) const PROXY_CREATE_MAX: usize = 500;
/// Delete Proxy и фильтр Query Proxy: столько `proxy_id` за один запрос.
const PROXY_IDS_MAX: usize = 100;

/// Маршруты списка прокси; `server::api_routes` подключает их до слоя проверок.
/// Add Proxy берёт тело как есть (`RawBody`): массив в теле `Params` отверг бы («request body must be a JSON object»).
pub(super) fn routes<E: Engine>() -> Router<Arc<Core<E>>> {
    Router::new()
        .route("/api/v2/proxy-list/create", both(create::<E>))
        .route("/api/v2/proxy-list/update", both(update::<E>))
        .route("/api/v2/proxy-list/delete", both(delete::<E>))
        .route("/api/v2/proxy-list/list", both(list::<E>))
}

fn proxy_not_found(id: i64) -> ApiFailure {
    ApiFailure(format!("Proxy not found: {id}"))
}

/// `type`: http, https, socks5 без учёта регистра; ssh и прочее не поддерживаются.
fn proxy_kind(text: &str) -> Result<ProxyKind, ApiFailure> {
    let kind = text.trim().to_ascii_lowercase();
    if kind == "ssh" {
        return Err(ApiFailure("proxy type ssh is not supported".into()));
    }
    ProxyKind::parse(&kind).ok_or_else(|| ApiFailure(format!("unsupported proxy type: {kind}")))
}

fn proxy_port(text: &str) -> Result<u16, ApiFailure> {
    text.trim()
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| ApiFailure("port must be between 1 and 65535".into()))
}

/// Поле ядра в `msg` Add Proxy и Update Proxy — именем запроса AdsPower.
fn proxy_field_name(field: &str) -> String {
    match field {
        "proxy_host" | "host" => "host",
        "proxy_port" | "port" => "port",
        "proxy_username" | "username" => "user",
        "proxy_password" => "password",
        "kind" => "type",
        other => other,
    }
    .to_string()
}

/// Ошибка ядра для `msg`: поля — именами Add Proxy и Update Proxy.
fn proxy_failure(e: &CommandError) -> ApiFailure {
    ApiFailure(error_message_named(e, proxy_field_name))
}

/// Те же проверки, что делает ядро при записи: хост, порт, логин и пароль вместе.
fn check_form(form: &ProxyEditForm, saved_password: bool) -> Result<(), ApiFailure> {
    let errors = validate_proxy_form(form, saved_password);
    if errors.is_empty() { Ok(()) } else { Err(proxy_failure(&CommandError::validation(errors))) }
}

/// Тело Add Proxy: один объект или массив объектов.
fn proxy_items(body: &[u8]) -> Result<Vec<Params>, ApiFailure> {
    let shape = || ApiFailure("request body must be a proxy object or an array of proxies".into());
    let items = match serde_json::from_slice::<Value>(body).map_err(|_| shape())? {
        Value::Object(map) => vec![Value::Object(map)],
        Value::Array(items) => items,
        _ => return Err(shape()),
    };
    if items.is_empty() {
        return Err(ApiFailure("no proxies to add".into()));
    }
    if items.len() > PROXY_CREATE_MAX {
        return Err(ApiFailure(format!("at most {PROXY_CREATE_MAX} proxies per request")));
    }
    items
        .into_iter()
        .enumerate()
        .map(|(index, item)| match item {
            Value::Object(map) => Ok(Params::from_map(map)),
            _ => Err(ApiFailure(format!("proxy #{}: must be an object", index + 1))),
        })
        .collect()
}

/// Запись из одного объекта Add Proxy; `proxy_url` и `ipchecker` не хранятся.
fn new_proxy_form(item: &Params) -> Result<ProxyEditForm, ApiFailure> {
    let form = ProxyEditForm {
        kind: proxy_kind(&item.require_text("type")?)?,
        host: item.require_text("host")?,
        port: proxy_port(&item.require_text("port")?)?,
        username: item.text("user").unwrap_or_default(),
        password: match item.text("password") {
            Some(value) => SecretInput::Set { value },
            None => SecretInput::Clear,
        },
        note: item.text("remark").unwrap_or_default(),
        enabled: true,
    };
    check_form(&form, false)?;
    Ok(form)
}

/// Все записи проверяются до первой записи в базу; ошибка в любой — ничего не добавлено.
/// `POST /api/v2/proxy-list/create` — объект или массив объектов; тело разбирается здесь, а не `Params`.
pub(super) async fn create<E: Engine>(State(core): State<Arc<Core<E>>>, RawBody(body): RawBody) -> ApiResult {
    let forms = proxy_items(&body)?
        .iter()
        .enumerate()
        .map(|(index, item)| new_proxy_form(item).map_err(|e| ApiFailure(format!("proxy #{}: {}", index + 1, e.0))))
        .collect::<Result<Vec<_>, _>>()?;
    let mut added = Vec::with_capacity(forms.len());
    for (index, form) in forms.iter().enumerate() {
        match core.create_proxy(form) {
            Ok(record) => added.push(record.id),
            Err(error) => {
                // Проверки выше совпадают с проверками ядра, поэтому сюда приводит только сбой базы: добавленное убирается.
                for id in &added {
                    let _ = core.delete_proxy(*id);
                }
                return Err(ApiFailure(format!("proxy #{}: {}", index + 1, proxy_failure(&error).0)));
            }
        }
    }
    Ok(Data(json!({ "proxy_id": added.iter().map(s).collect::<Vec<_>>() })))
}

/// Меняются только переданные поля; логин, убранный без нового пароля, снимает и пароль.
pub(super) async fn update<E: Engine>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let id = params.int("proxy_id")?.ok_or_else(|| ApiFailure("proxy_id is required".into()))?;
    let saved = core.store.get_proxy(id)?.ok_or_else(|| proxy_not_found(id))?;
    let username = match params.get("user") {
        Some(_) => params.text("user").unwrap_or_default(),
        None => saved.username.clone().unwrap_or_default(),
    };
    let password = match (params.get("password"), params.text("password")) {
        (Some(_), Some(value)) => SecretInput::Set { value },
        (Some(_), None) => SecretInput::Clear,
        (None, _) if username.is_empty() => SecretInput::Clear,
        (None, _) => SecretInput::Keep,
    };
    let form = ProxyEditForm {
        kind: match params.text("type") {
            Some(kind) => proxy_kind(&kind)?,
            None => saved.kind,
        },
        host: params.text("host").unwrap_or(saved.host),
        port: match params.text("port") {
            Some(port) => proxy_port(&port)?,
            None => saved.port,
        },
        username,
        password,
        note: match params.get("remark") {
            Some(_) => params.text("remark").unwrap_or_default(),
            None => saved.note,
        },
        enabled: saved.enabled,
    };
    core.update_proxy(id, &form).map_err(|e| proxy_failure(&e))?;
    Ok(Data(json!({})))
}

/// `proxy_id` списком (или одним значением) → номера без повторов, в порядке запроса.
fn proxy_ids(params: &Params) -> Result<Option<Vec<i64>>, ApiFailure> {
    let Some(raw) = params.list("proxy_id")? else { return Ok(None) };
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for text in raw {
        let id: i64 = text.trim().parse().map_err(|_| ApiFailure("proxy_id must contain numbers".into()))?;
        if seen.insert(id) {
            ids.push(id);
        }
    }
    Ok(Some(ids))
}

/// Неизвестный или занятый активным профилем прокси пачки — текстом для `msg`; `None` — пачку можно удалять.
fn delete_problem<E: Engine>(core: &Core<E>, ids: &[i64]) -> Result<Option<ApiFailure>, ApiFailure> {
    for id in ids {
        if core.store.get_proxy(*id)?.is_none() {
            return Ok(Some(proxy_not_found(*id)));
        }
    }
    let users = core.supervisor.proxy_users();
    let busy: Vec<String> = ids.iter().filter(|id| users.contains_key(*id)).map(|id| id.to_string()).collect();
    if busy.is_empty() {
        return Ok(None);
    }
    let in_use = ApiFailure::from(CommandError::new("ProxyInUse", "прокси используется запущенным профилем")).0;
    Ok(Some(ApiFailure(format!("{in_use}: {}", busy.join(", ")))))
}

/// Сначала проверяются все: неизвестный или занятый активным профилем — ошибка, и ничего не удаляется.
/// Само удаление — `Core::delete_proxies`: под мьютексом выбора шлюза и одной транзакцией.
pub(super) async fn delete<E: Engine>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let ids = proxy_ids(&params)?
        .filter(|ids| !ids.is_empty())
        .ok_or_else(|| ApiFailure("proxy_id is required".into()))?;
    if ids.len() > PROXY_IDS_MAX {
        return Err(ApiFailure(format!("at most {PROXY_IDS_MAX} proxies per request")));
    }
    if let Some(problem) = delete_problem(&core, &ids)? {
        return Err(problem);
    }
    if let Err(error) = core.delete_proxies(&ids) {
        // Запуск занял прокси между проверкой и удалением: ответ — тот же, что дала бы проверка.
        return Err(delete_problem(&core, &ids)?.unwrap_or_else(|| error.into()));
    }
    Ok(Data(json!({})))
}

/// Записи по порядку добавления; `profile_count` и `related_profile_no` — закреплённые профили, пароль — открытым текстом.
pub(super) async fn list<E: Engine>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let paging = params.paging("limit", PROXY_PAGE_DEFAULT, PROXY_PAGE_MAX)?;
    let wanted = proxy_ids(&params)?;
    if wanted.as_ref().is_some_and(|ids| ids.len() > PROXY_IDS_MAX) {
        return Err(ApiFailure(format!("at most {PROXY_IDS_MAX} proxy ids per request")));
    }
    let wanted: Option<HashSet<i64>> = wanted.map(|ids| ids.into_iter().collect());
    let mut pinned: HashMap<i64, Vec<i64>> = HashMap::new();
    for profile in core.store.list_profiles(&ListQuery::default())? {
        if let Some(proxy_id) = profile.proxy_id {
            pinned.entry(proxy_id).or_default().push(profile.serial);
        }
    }
    let proxies: Vec<_> = core
        .store
        .list_proxies()?
        .into_iter()
        .filter(|proxy| wanted.as_ref().is_none_or(|ids| ids.contains(&proxy.id)))
        .collect();
    let total = proxies.len();
    let mut list = Vec::new();
    for proxy in proxies.into_iter().skip(paging.offset()).take(paging.limit()) {
        let password = list_password(&core, proxy.id)?;
        let mut serials = pinned.remove(&proxy.id).unwrap_or_default();
        serials.sort_unstable();
        list.push(json!({
            "proxy_id": s(proxy.id),
            "type": proxy.kind.as_str(),
            "host": proxy.host,
            "port": s(proxy.port),
            "user": proxy.username.unwrap_or_default(),
            "password": password,
            "proxy_url": "",
            "remark": proxy.note,
            "ipchecker": "",
            "proxy_partner": "",
            "profile_count": s(serials.len()),
            "related_profile_no": serials.iter().map(s).collect::<Vec<_>>(),
            "proxy_tags": [],
        }));
    }
    Ok(Data(json!({ "list": list, "total": total, "page": paging.page, "page_size": paging.size })))
}

/// Пароль записи для Query Proxy — открытым текстом (решение заказчика). Не расшифровывается (база с другого Mac) —
/// пустая строка и предупреждение в журнал без значений: одна испорченная запись не ломает весь список.
fn list_password<E: Engine>(core: &Core<E>, proxy_id: i64) -> Result<String, ApiFailure> {
    match core.store.proxy_credentials(proxy_id) {
        Ok(credentials) => Ok(credentials.password.map(|p| p.expose().to_string()).unwrap_or_default()),
        Err(StoreError::Crypto) => {
            log::warn!("прокси {proxy_id}: пароль не расшифровывается, в ответе Local API — пустая строка");
            Ok(String::new())
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::{api_env, keys, names};
    use crate::app::tests::{create_with_proxy, form};

    fn failure(code: &str) -> String {
        ApiFailure::from(CommandError::new(code, "")).0
    }

    fn proxy(host: &str, port: &str) -> Value {
        json!({"type": "http", "host": host, "port": port, "user": "u", "password": "p", "remark": "r",
               "proxy_url": "https://example.com/refresh", "ipchecker": "ipapi"})
    }

    fn ids(answer: &Value) -> Vec<i64> {
        answer["data"]["proxy_id"]
            .as_array()
            .expect("массив proxy_id")
            .iter()
            .map(|id| id.as_str().expect("proxy_id строкой").parse().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn proxies_are_added_one_or_many_without_dedup() {
        let api = api_env().await;
        let one = api.post("/api/v2/proxy-list/create", proxy("10.0.0.1", "8080")).await;
        assert_eq!(one["code"], 0, "{one}");
        assert_eq!(keys(&one["data"]), names(&["proxy_id"]));
        let first = ids(&one)[0];
        let record = api.core.store.get_proxy(first).unwrap().unwrap();
        assert_eq!(
            (record.kind, record.host.as_str(), record.port, record.username.as_deref(), record.note.as_str(), record.enabled),
            (ProxyKind::Http, "10.0.0.1", 8080, Some("u"), "r", true)
        );
        assert_eq!(api.core.store.proxy_credentials(first).unwrap().password.unwrap().expose(), "p");

        let many = api
            .post(
                "/api/v2/proxy-list/create",
                json!([
                    {"type": "socks5", "host": "10.0.0.2", "port": 1080},
                    {"Type": "HTTPS", "Host": "10.0.0.3", "Port": "443"},
                    proxy("10.0.0.1", "8080"),
                ]),
            )
            .await;
        assert_eq!(many["code"], 0, "{many}");
        let added = ids(&many);
        assert_eq!(added.len(), 3);
        assert!(!added.contains(&first), "дубликаты не проверяются, как у AdsPower");
        let kinds: Vec<ProxyKind> = added.iter().map(|id| api.core.store.get_proxy(*id).unwrap().unwrap().kind).collect();
        assert_eq!(kinds, [ProxyKind::Socks5, ProxyKind::Https, ProxyKind::Http]);
        assert_eq!(api.core.store.list_proxies().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn one_bad_proxy_adds_nothing() {
        let api = api_env().await;
        let cases = [
            (json!([proxy("10.0.0.1", "8080"), {"type": "ssh", "host": "10.0.0.2", "port": "22"}]),
             "proxy #2: proxy type ssh is not supported"),
            (json!([proxy("10.0.0.1", "8080"), {"type": "ftp", "host": "10.0.0.2", "port": "21"}]),
             "proxy #2: unsupported proxy type: ftp"),
            (json!([proxy("10.0.0.1", "8080"), {"type": "http", "host": "10.0.0.2", "port": "0"}]),
             "proxy #2: port must be between 1 and 65535"),
            (json!([proxy("10.0.0.1", "8080"), {"type": "http", "port": "80"}]), "proxy #2: host is required"),
            (json!([proxy("10.0.0.1", "8080"), 7]), "proxy #2: must be an object"),
            (json!([]), "no proxies to add"),
            (json!("10.0.0.1:8080"), "request body must be a proxy object or an array of proxies"),
        ];
        for (body, msg) in cases {
            let answer = api.post("/api/v2/proxy-list/create", body).await;
            assert_eq!((answer["code"].as_i64(), answer["msg"].as_str()), (Some(-1), Some(msg)), "{answer}");
        }
        let login_only = api
            .post(
                "/api/v2/proxy-list/create",
                json!([proxy("10.0.0.1", "8080"), {"type": "http", "host": "10.0.0.2", "port": "80", "user": "u"}]),
            )
            .await;
        let msg = login_only["msg"].as_str().unwrap();
        assert_eq!(msg, "proxy #2: Check the form fields (password: required)");
        assert!(!msg.contains("\"u\""), "логин в msg не попадает: {msg}");
        let too_many: Vec<Value> = (0..=PROXY_CREATE_MAX).map(|i| proxy("10.0.0.1", &(1000 + i).to_string())).collect();
        let answer = api.post("/api/v2/proxy-list/create", Value::Array(too_many)).await;
        assert_eq!(answer["msg"], "at most 500 proxies per request");
        assert!(api.core.store.list_proxies().unwrap().is_empty(), "ни одна запись не добавлена");
    }

    /// Пароль записи не расшифровывается (база перенесена с другого Mac): в списке — пустая строка, остальные поля
    /// и соседние записи целы, ответ успешный.
    #[tokio::test]
    async fn undecryptable_password_does_not_break_the_list() {
        let api = api_env().await;
        let created = api.post("/api/v2/proxy-list/create", json!([proxy("10.0.0.1", "8080"), proxy("10.0.0.2", "8081")])).await;
        let added = ids(&created);
        api.core.store.raw().execute("UPDATE proxies SET password_enc = x'00' WHERE id = ?1", [added[0]]).unwrap();
        let listed = api.post("/api/v2/proxy-list/list", json!({})).await;
        assert_eq!(listed["code"], 0, "{listed}");
        let list = listed["data"]["list"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["proxy_id"], added[0].to_string());
        assert_eq!(list[0]["password"], "", "{}", list[0]);
        assert_eq!((list[0]["host"].as_str(), list[0]["user"].as_str()), (Some("10.0.0.1"), Some("u")), "остальные поля целы");
        assert_eq!(list[1]["password"], "p", "соседняя запись не пострадала");
    }

    /// Ошибки проверки называют поля именами Add Proxy и Update Proxy: `host`, `port`, `user`, `password`.
    #[tokio::test]
    async fn proxy_errors_name_fields_as_the_request() {
        let api = api_env().await;
        let bad_host = api.post("/api/v2/proxy-list/create", json!({"type": "http", "host": "bad host", "port": "8080"})).await;
        assert_eq!(bad_host["msg"], "proxy #1: Check the form fields (host: invalid)", "{bad_host}");
        let id = ids(&api.post("/api/v2/proxy-list/create", proxy("10.0.0.1", "8080")).await)[0];
        let moved = api.post("/api/v2/proxy-list/update", json!({"proxy_id": id, "host": "bad host"})).await;
        assert_eq!(moved["msg"], "Check the form fields (host: invalid)", "{moved}");
        let no_password = api.post("/api/v2/proxy-list/update", json!({"proxy_id": id, "password": ""})).await;
        assert_eq!(no_password["msg"], "Check the form fields (password: required)", "{no_password}");
        assert_eq!(api.core.store.get_proxy(id).unwrap().unwrap().host, "10.0.0.1", "запись не изменилась");
    }

    #[tokio::test]
    async fn proxy_update_changes_only_given_fields() {
        let api = api_env().await;
        let id = ids(&api.post("/api/v2/proxy-list/create", proxy("10.0.0.1", "8080")).await)[0];

        let moved = api.post("/api/v2/proxy-list/update", json!({"Proxy_id": id.to_string(), "port": 9090})).await;
        assert_eq!((moved["code"].as_i64(), &moved["data"]), (Some(0), &json!({})), "{moved}");
        let record = api.core.store.get_proxy(id).unwrap().unwrap();
        assert_eq!(
            (record.host.as_str(), record.port, record.username.as_deref(), record.note.as_str()),
            ("10.0.0.1", 9090, Some("u"), "r")
        );
        assert_eq!(api.core.store.proxy_credentials(id).unwrap().password.unwrap().expose(), "p");

        assert_eq!(api.post("/api/v2/proxy-list/update", json!({"proxy_id": id, "password": "new"})).await["code"], 0);
        assert_eq!(api.core.store.proxy_credentials(id).unwrap().password.unwrap().expose(), "new");
        assert_eq!(api.post("/api/v2/proxy-list/update", json!({"proxy_id": id, "remark": ""})).await["code"], 0);
        assert_eq!(api.core.store.get_proxy(id).unwrap().unwrap().note, "");

        // Логин убран без нового пароля — пароль снимается вместе с ним.
        assert_eq!(api.post("/api/v2/proxy-list/update", json!({"proxy_id": id, "user": ""})).await["code"], 0);
        let record = api.core.store.get_proxy(id).unwrap().unwrap();
        assert_eq!((record.username, record.has_password), (None, false));

        let ssh = api.post("/api/v2/proxy-list/update", json!({"proxy_id": id, "type": "ssh"})).await;
        assert_eq!(ssh["msg"], "proxy type ssh is not supported");
        assert_eq!(api.core.store.get_proxy(id).unwrap().unwrap().kind, ProxyKind::Http);
        assert_eq!(api.post("/api/v2/proxy-list/update", json!({"proxy_id": 999})).await["msg"], "Proxy not found: 999");
        assert_eq!(api.post("/api/v2/proxy-list/update", json!({"port": 1})).await["msg"], "proxy_id is required");
    }

    #[tokio::test]
    async fn busy_proxy_is_not_deleted_and_idle_ones_unpin_profiles() {
        let api = api_env().await;
        let running = create_with_proxy(&api.core, "Работает", api.proxy_port, 1);
        let idle = create_with_proxy(&api.core, "Остановлен", api.proxy_port, 2);
        let (busy_proxy, idle_proxy) = (running.proxy_id.unwrap(), idle.proxy_id.unwrap());
        api.core.supervisor.start(&running.id).await.unwrap();

        let refused = api
            .post("/api/v2/proxy-list/delete", json!({"Proxy_id": [busy_proxy.to_string(), idle_proxy.to_string()]}))
            .await;
        assert_eq!(refused["msg"], format!("{}: {busy_proxy}", failure("ProxyInUse")), "{refused}");
        assert!(api.core.store.get_proxy(busy_proxy).unwrap().is_some(), "занятый не удалён");
        assert!(api.core.store.get_proxy(idle_proxy).unwrap().is_some(), "свободный тоже не удалён: ничего не удаляется");

        let unknown = api.post("/api/v2/proxy-list/delete", json!({"proxy_id": [idle_proxy, 999]})).await;
        assert_eq!(unknown["msg"], "Proxy not found: 999");
        assert!(api.core.store.get_proxy(idle_proxy).unwrap().is_some());

        let deleted = api.post("/api/v2/proxy-list/delete", json!({"proxy_id": [idle_proxy]})).await;
        assert_eq!((deleted["code"].as_i64(), &deleted["data"]), (Some(0), &json!({})), "{deleted}");
        assert!(api.core.store.get_proxy(idle_proxy).unwrap().is_none());
        assert_eq!(api.core.store.get_profile(&idle.id).unwrap().unwrap().proxy_id, None, "закрепление снято");

        let over: Vec<i64> = (1..=101).collect();
        let answer = api.post("/api/v2/proxy-list/delete", json!({"proxy_id": over})).await;
        assert_eq!(answer["msg"], "at most 100 proxies per request");
        assert_eq!(api.post("/api/v2/proxy-list/delete", json!({})).await["msg"], "proxy_id is required");
        api.core.supervisor.stop(&running.id).await.unwrap();
    }

    #[tokio::test]
    async fn proxy_list_follows_adspower_shape() {
        let api = api_env().await;
        let added = ids(
            &api.post(
                "/api/v2/proxy-list/create",
                json!([proxy("10.0.0.1", "8080"), proxy("10.0.0.2", "8081"), {"type": "socks5", "host": "10.0.0.3", "port": "1080"}]),
            )
            .await,
        );
        let a = api.core.create_profile(&form("A", None), 1).unwrap();
        let b = api.core.create_profile(&form("B", None), 2).unwrap();
        api.core.store.set_profiles_proxy(&[b.id.clone(), a.id.clone()], Some(added[0])).unwrap();

        let all = api.post("/api/v2/proxy-list/list", json!({})).await;
        assert_eq!(all["code"], 0, "{all}");
        let data = &all["data"];
        assert_eq!(keys(data), names(&["list", "total", "page", "page_size"]));
        assert_eq!((data["total"].as_i64(), data["page"].as_i64(), data["page_size"].as_i64()), (Some(3), Some(1), Some(50)));
        let first = &data["list"][0];
        assert_eq!(
            keys(first),
            names(&[
                "proxy_id", "type", "host", "port", "user", "password", "proxy_url", "remark", "ipchecker", "proxy_partner",
                "profile_count", "related_profile_no", "proxy_tags",
            ])
        );
        assert_eq!(first["proxy_id"], s(added[0]));
        assert_eq!((first["type"].as_str(), first["host"].as_str(), first["port"].as_str()), (Some("http"), Some("10.0.0.1"), Some("8080")));
        assert_eq!(
            (first["user"].as_str(), first["password"].as_str(), first["remark"].as_str()),
            (Some("u"), Some("p"), Some("r")),
            "пароль — открытым текстом (решение заказчика)"
        );
        assert_eq!(
            (first["proxy_url"].as_str(), first["ipchecker"].as_str(), first["proxy_partner"].as_str()),
            (Some(""), Some(""), Some(""))
        );
        assert_eq!(first["profile_count"], "2");
        assert_eq!(first["related_profile_no"], json!([a.serial.to_string(), b.serial.to_string()]));
        assert_eq!(first["proxy_tags"], json!([]));
        let third = &data["list"][2];
        assert_eq!(
            (third["user"].as_str(), third["password"].as_str(), third["profile_count"].as_str()),
            (Some(""), Some(""), Some("0"))
        );
        assert_eq!(third["related_profile_no"], json!([]));

        let page = api.post("/api/v2/proxy-list/list", json!({"Proxy_id": [added[1], added[2]], "limit": 1, "page": 2})).await;
        assert_eq!((page["data"]["total"].as_i64(), page["data"]["page_size"].as_i64()), (Some(2), Some(1)));
        assert_eq!(page["data"]["list"].as_array().unwrap().len(), 1);
        assert_eq!(page["data"]["list"][0]["proxy_id"], s(added[2]));

        assert_eq!(api.post("/api/v2/proxy-list/list", json!({"limit": 201})).await["msg"], "limit must be between 1 and 200");
        let over: Vec<i64> = (1..=101).collect();
        let answer = api.post("/api/v2/proxy-list/list", json!({"proxy_id": over})).await;
        assert_eq!(answer["msg"], "at most 100 proxy ids per request");
        let words = api.post("/api/v2/proxy-list/list", json!({"proxy_id": ["abc"]})).await;
        assert_eq!(words["msg"], "proxy_id must contain numbers");
    }
}
