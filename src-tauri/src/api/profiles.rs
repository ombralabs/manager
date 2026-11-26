//! Профили Local API: создание, правка, запрос, удаление, перенос, кэш, cookies (спецификация 2Б, «Профили»).

use super::browser::{ApiVersion, find_profile, id_names};
use super::envelope::{ApiFailure, ApiResult, Data, error_message_named, s};
use super::mapping::{
    apply_fingerprint_config, fingerprint_config, fingerprint_config_field, parse_user_proxy_config, user_proxy_config,
};
use super::params::{Paging, Params};
use super::server::both;
use crate::app::forms::clean_lines;
use crate::app::patch::{HumanizePatch, ProfilePatch, ProxyAssign};
use crate::app::{CommandError, Core};
use crate::sys::unix_now;
use crate::cookies::{self, Cookie, set_hash};
use crate::engine::Engine;
use crate::engine::ombra::install::EXPECTED_VERSION;
use crate::fingerprint::{Arch, Fingerprint};
use crate::humanize::Preset;
use crate::paths::{CHROMIUM_COOKIE_FILES, remove_path};
use crate::store::StoreError;
use crate::store::profiles::{GroupFilter, ListQuery, ProfileRecord, SecretRead, SortKey};
use crate::store::proxies::{ProxyCheck, ProxyCredentials};
use crate::store::secrets::Secret;
use crate::supervisor::SupervisorError;
use axum::Router;
use axum::extract::State;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Маршруты профилей; `server::api_routes` подключает их до слоя проверок.
pub(super) fn routes<E: Engine>() -> Router<Arc<Core<E>>> {
    Router::new()
        .route("/api/v1/user/create", both(create::<E, 1>))
        .route("/api/v2/browser-profile/create", both(create::<E, 2>))
        .route("/api/v1/user/update", both(update::<E, 1>))
        .route("/api/v2/browser-profile/update", both(update::<E, 2>))
        .route("/api/v1/user/list", both(list::<E, 1>))
        .route("/api/v2/browser-profile/list", both(list::<E, 2>))
        .route("/api/v1/user/delete", both(delete::<E, 1>))
        .route("/api/v2/browser-profile/delete", both(delete::<E, 2>))
        .route("/api/v1/user/regroup", both(regroup::<E>))
        .route("/api/v1/user/delete-cache", both(delete_cache_all::<E>))
        .route("/api/v2/browser-profile/delete-cache", both(delete_cache_typed::<E>))
        .route("/api/v2/browser-profile/cookies", both(profile_cookies::<E>))
}

/// Больше профилей на страницу не отдаётся, как у AdsPower.
pub(super) const PROFILE_PAGE_MAX: i64 = 100;
/// Профилей за один запрос удаления.
pub(super) const DELETE_MAX: usize = 100;
/// Текст AdsPower для очистки кэша при открытом браузере.
pub(super) const OPEN_BROWSERS: &str = "There are currently open browsers, please close them before deleting the cache";
/// Название «Без группы» в строках запроса.
pub(super) const UNGROUPED: &str = "Ungrouped";

/// Папки и файлы типа очистки v2 внутри папки профиля. `image_file` — пустой список: его папки — `paths.cache_paths`;
/// `cookie` — ещё и cookies в базе. Неизвестный тип — `None`.
pub(super) fn cache_type_dirs(kind: &str) -> Option<&'static [&'static str]> {
    match kind {
        "image_file" => Some(&[]),
        "local_storage" => Some(&["Default/Local Storage", "Default/Session Storage"]),
        "indexeddb" => Some(&["Default/IndexedDB"]),
        "history" => Some(&[
            "Default/History",
            "Default/History-journal",
            "Default/Visited Links",
            "Default/Top Sites",
            "Default/Top Sites-journal",
        ]),
        "cookie" => Some(CHROMIUM_COOKIE_FILES),
        "extension_cache" => Some(&[
            "Default/Extension State",
            "Default/Local Extension Settings",
            "Default/Extension Rules",
            "Default/Extension Scripts",
        ]),
        _ => None,
    }
}

/// Текстовое поле, которое можно очистить: пустая строка и `null` — пустое значение, отсутствие — не менять.
fn field_text(params: &Params, name: &str) -> Result<Option<String>, ApiFailure> {
    match params.get(name) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(String::new())),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(Value::Number(number)) => Ok(Some(number.to_string())),
        Some(_) => Err(ApiFailure(format!("{name} must be a string"))),
    }
}

/// `None` — не передан; `Some(None)` — `"0"`, «Без группы»; иначе существующая группа.
fn group_param<E: Engine>(core: &Core<E>, params: &Params) -> Result<Option<Option<i64>>, ApiFailure> {
    let Some(text) = params.text("group_id") else { return Ok(None) };
    let text = text.trim();
    if text == "0" {
        return Ok(Some(None));
    }
    let id: i64 = text.parse().map_err(|_| ApiFailure("group_id must be a number".into()))?;
    core.store
        .get_group(id)?
        .ok_or_else(|| ApiFailure(format!("Group not found: {id}")))?;
    Ok(Some(Some(id)))
}

/// Номер набора расширений: `sys_app_cate_id` (создание v1 и v2, правка v1) или `category_id` (правка v2).
/// `0` — «Общий»; неизвестный — ошибка, как у `group_id`.
fn category_param<E: Engine>(core: &Core<E>, params: &Params, name: &str) -> Result<Option<i64>, ApiFailure> {
    let Some(text) = params.text(name) else { return Ok(None) };
    let id: i64 = text.trim().parse().map_err(|_| ApiFailure(format!("{name} must be a number")))?;
    core.store.get_ext_category(id)?.ok_or_else(|| ApiFailure(format!("Category not found: {id}")))?;
    Ok(Some(id))
}

/// `cookie`: JSON-массив (объектом или строкой) или текст Netscape. Без `ignore_cookie_error=1` любая ошибочная
/// запись — ошибка всего запроса.
fn cookie_param(params: &Params) -> Result<Option<Vec<Cookie>>, ApiFailure> {
    let text = match params.get("cookie") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(text)) if text.trim().is_empty() => return Ok(None),
        Some(Value::String(text)) => text.clone(),
        Some(value @ Value::Array(_)) => value.to_string(),
        Some(_) => return Err(ApiFailure("cookie must be a JSON array or Netscape text".into())),
    };
    let parsed = cookies::parse(&text).map_err(CommandError::from)?;
    let ignore_errors = params.flag("ignore_cookie_error")?.unwrap_or(false);
    if !ignore_errors && !parsed.skipped.is_empty() {
        let lines: Vec<String> = parsed.skipped.iter().map(|skipped| skipped.line.to_string()).collect();
        return Err(ApiFailure(format!("cookie: invalid records {}", lines.join(", "))));
    }
    Ok(Some(parsed.cookies))
}

/// Шлюз: `user_proxy_config` (важнее) или `proxyid`; `no_proxy` и `"random"` снимают закрепление.
fn proxy_param<E: Engine>(core: &Core<E>, params: &Params) -> Result<Option<ProxyAssign>, ApiFailure> {
    if let Some(config) = params.object("user_proxy_config")? {
        return Ok(Some(match parse_user_proxy_config(&config)? {
            Some(proxy) => ProxyAssign::Config(proxy),
            None => ProxyAssign::Unpin,
        }));
    }
    let Some(text) = params.text("proxyid") else { return Ok(None) };
    let text = text.trim();
    if text.eq_ignore_ascii_case("random") {
        return Ok(Some(ProxyAssign::Unpin));
    }
    let id: i64 = text.parse().map_err(|_| ApiFailure("proxyid must be a proxy id or \"random\"".into()))?;
    core.store
        .get_proxy(id)?
        .ok_or_else(|| ApiFailure(format!("Proxy not found: {id}")))?;
    Ok(Some(ProxyAssign::Existing(id)))
}

/// `ombra.humanize` — пространство Ombra в запросах AdsPower (ТЗ 7.5: не ломать совместимость): `enabled` и
/// `block_input` — true/false, `preset` — `default`, `careful` или `fast`, `paste_key` — горячая клавиша
/// «Вставить как человек» (`Ctrl+Alt+V`: ⌃ и/или ⌥, ⌘ нельзя; пусто — нет); переданные поля меняют настройки, остальные —
/// как были.
fn humanize_param(params: &Params) -> Result<Option<HumanizePatch>, ApiFailure> {
    let Some(ombra) = params.object("ombra")? else { return Ok(None) };
    let Some(value) = ombra.get("humanize") else { return Ok(None) };
    let Value::Object(fields) = value else { return Err(ApiFailure("ombra.humanize must be an object".into())) };
    let mut patch = HumanizePatch::default();
    for (key, value) in fields {
        match (key.as_str(), value) {
            ("enabled", Value::Bool(on)) => patch.enabled = Some(*on),
            ("block_input", Value::Bool(on)) => patch.block_input = Some(*on),
            ("preset", Value::String(preset)) => {
                patch.preset = Some(
                    Preset::parse(preset)
                        .ok_or_else(|| ApiFailure("ombra.humanize.preset must be default, careful or fast".into()))?,
                );
            }
            ("paste_key", Value::String(key)) => patch.paste_key = Some(key.clone()),
            ("seed", _) => return Err(ApiFailure("ombra.humanize.seed is read-only".into())),
            (key, _) => return Err(ApiFailure(format!("ombra.humanize.{key}: unknown field or wrong type"))),
        }
    }
    Ok(Some(patch))
}

/// `ombra` строки профиля: настройки очеловечивания и seed персоны (строкой: u64).
fn ombra_fields(profile: &ProfileRecord) -> Value {
    let h = &profile.humanize;
    json!({
        "humanize": {
            "enabled": h.enabled,
            "preset": h.preset.as_str(),
            "block_input": h.block_input,
            "paste_key": h.paste_key,
            "seed": profile.humanize_seed.to_string(),
        },
    })
}

/// Сохранённые неприменяемые поля `fingerprint_config`; испорченная строка — пустой объект.
fn stored_extra<E: Engine>(core: &Core<E>, id: &str) -> Result<Map<String, Value>, ApiFailure> {
    let text = core.store.api_extra(id)?;
    Ok(match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    })
}

/// `fingerprint_config` поверх текущего отпечатка (правка) или свежего от генератора (создание).
fn fingerprint_param<E: Engine>(
    core: &Core<E>,
    params: &Params,
    existing: Option<&ProfileRecord>,
) -> Result<Option<(Fingerprint, String)>, ApiFailure> {
    let Some(config) = params.object("fingerprint_config")? else { return Ok(None) };
    let (base, extra) = match existing {
        Some(record) => {
            let base = record.fingerprint()?;
            (base, stored_extra(core, &record.id)?)
        }
        None => (core.new_fingerprint()?, Map::new()),
    };
    let mapped = apply_fingerprint_config(&base, &extra, &config, Arch::host())?;
    Ok(Some((mapped.fingerprint, Value::Object(mapped.extra).to_string())))
}

/// Параметры создания или правки → `ProfilePatch`. Все проверки здесь, до записи.
pub(super) fn profile_patch<E: Engine>(
    core: &Core<E>,
    params: &Params,
    version: ApiVersion,
    existing: Option<&ProfileRecord>,
) -> Result<ProfilePatch, ApiFailure> {
    let (platform, tabs) = match version {
        ApiVersion::V1 => ("domain_name", "open_urls"),
        ApiVersion::V2 => ("platform", "tabs"),
    };
    let fingerprint = fingerprint_param(core, params, existing)?;
    let (fingerprint, api_extra) = match fingerprint {
        Some((fingerprint, extra)) => (Some(fingerprint), Some(extra)),
        None => (None, None),
    };
    Ok(ProfilePatch {
        name: params.text("name"),
        note: field_text(params, "remark")?,
        platform_domain: field_text(params, platform)?,
        start_urls: params.list(tabs)?.map(|urls| clean_lines(&urls)),
        extra_args: match (version, existing) {
            // Только v2 Update Profile: в остальных операциях у AdsPower нет `launch_args`.
            (ApiVersion::V2, Some(_)) => params.list("launch_args")?.map(|args| clean_lines(&args)),
            _ => None,
        },
        username: field_text(params, "username")?,
        password: params.text("password").map(Secret::new),
        // Пустой `fakey` или `null` — снять ключ.
        totp: field_text(params, "fakey")?.map(Secret::new),
        group_id: group_param(core, params)?,
        ext_category_id: category_param(core, params, match (version, existing) {
            (ApiVersion::V2, Some(_)) => "category_id",
            _ => "sys_app_cate_id",
        })?,
        fingerprint,
        api_extra,
        proxy: proxy_param(core, params)?,
        cookies: cookie_param(params)?,
        humanize: humanize_param(params)?,
    })
}

/// Поле ядра в `msg` ошибки проверки — именем запроса AdsPower этой версии: ядро и интерфейс знают свои имена,
/// скрипт — имена справочника. Поля отпечатка — `fingerprint_config.<имя AdsPower>`.
fn api_field_name(field: &str, version: ApiVersion) -> String {
    if let Some(name) = fingerprint_config_field(field) {
        return name;
    }
    let name = match (field, version) {
        ("start_urls", ApiVersion::V1) => "open_urls",
        ("start_urls", ApiVersion::V2) => "tabs",
        ("platform_domain", ApiVersion::V1) => "domain_name",
        ("platform_domain", ApiVersion::V2) => "platform",
        ("note", _) => "remark",
        ("ext_category_id", ApiVersion::V1) => "sys_app_cate_id",
        ("ext_category_id", ApiVersion::V2) => "category_id",
        ("extra_args", _) => "launch_args",
        ("totp_secret", _) => "fakey",
        ("proxy_id", _) => "proxyid",
        ("proxy_host", _) => "user_proxy_config.proxy_host",
        ("proxy_port", _) => "user_proxy_config.proxy_port",
        ("proxy_username", _) => "user_proxy_config.proxy_user",
        ("proxy_password", _) => "user_proxy_config.proxy_password",
        ("api_extra" | "fingerprint", _) => "fingerprint_config",
        ("humanize.paste_key", _) => "ombra.humanize.paste_key",
        (other, _) => other,
    };
    name.to_string()
}

/// Ошибка ядра при создании и правке профиля: поля — именами запроса этой версии.
fn core_failure(e: &CommandError, version: ApiVersion) -> ApiFailure {
    ApiFailure(error_message_named(e, |field| api_field_name(field, version)))
}

pub(super) async fn create<E: Engine, const V: u8>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let version = ApiVersion::of(V);
    let patch = profile_patch(&core, &params, version, None)?;
    let record = core.create_profile_patch(patch, unix_now()).await.map_err(|e| core_failure(&e, version))?;
    Ok(Data(match version {
        ApiVersion::V1 => json!({ "id": record.id }),
        ApiVersion::V2 => json!({ "profile_id": record.id, "profile_no": s(record.serial) }),
    }))
}

pub(super) async fn update<E: Engine, const V: u8>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let version = ApiVersion::of(V);
    let (id_name, no_name) = id_names(version);
    let existing = find_profile(&core, &params, id_name, no_name)?;
    let patch = profile_patch(&core, &params, version, Some(&existing))?;
    core.patch_profile(&existing.id, patch).await.map_err(|e| core_failure(&e, version))?;
    Ok(Data(json!({})))
}

/// Id из списка параметра: без пустых и повторов, в порядке первого появления.
fn unique_ids(list: Option<Vec<String>>) -> Vec<String> {
    let mut seen = HashSet::new();
    list.unwrap_or_default()
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .collect()
}

/// Id, которых нет в базе.
fn missing_profiles<E: Engine>(core: &Core<E>, ids: &[String]) -> Result<Vec<String>, ApiFailure> {
    let mut missing = Vec::new();
    for id in ids {
        if core.store.get_profile(id)?.is_none() {
            missing.push(id.clone());
        }
    }
    Ok(missing)
}

struct ListRequest {
    group: Option<GroupFilter>,
    ids: Option<Vec<String>>,
    serials: Option<Vec<i64>>,
    sort: SortKey,
    descending: bool,
    paging: Paging,
    size_name: &'static str,
}

fn sort_key(field: &str, param: &str) -> Result<SortKey, ApiFailure> {
    match field.trim() {
        "serial_number" | "profile_no" => Ok(SortKey::Serial),
        "last_open_time" => Ok(SortKey::LastOpened),
        "created_time" => Ok(SortKey::Created),
        other => Err(ApiFailure(format!("{param}: unsupported sort field {other}"))),
    }
}

/// `true` — по убыванию.
fn sort_order(order: &str, param: &str) -> Result<bool, ApiFailure> {
    match order.trim().to_ascii_lowercase().as_str() {
        "desc" => Ok(true),
        "asc" => Ok(false),
        other => Err(ApiFailure(format!("{param}: order must be asc or desc, got {other}"))),
    }
}

/// v1: `user_sort` — `{"поле":"asc|desc"}` объектом или JSON-строкой; по умолчанию номер по убыванию.
fn v1_sort(params: &Params) -> Result<(SortKey, bool), ApiFailure> {
    let Some(map) = params.object("user_sort")? else { return Ok((SortKey::Serial, true)) };
    let Some((field, order)) = map.iter().next() else { return Ok((SortKey::Serial, true)) };
    let order = order.as_str().ok_or_else(|| ApiFailure("user_sort: order must be asc or desc".into()))?;
    Ok((sort_key(field, "user_sort")?, sort_order(order, "user_sort")?))
}

/// v2: `sort_type` и `sort_order`; по умолчанию номер по убыванию.
fn v2_sort(params: &Params) -> Result<(SortKey, bool), ApiFailure> {
    let sort = match params.text("sort_type") {
        Some(field) => sort_key(&field, "sort_type")?,
        None => SortKey::Serial,
    };
    let descending = match params.text("sort_order") {
        Some(order) => sort_order(&order, "sort_order")?,
        None => true,
    };
    Ok((sort, descending))
}

fn list_request(params: &Params, version: ApiVersion) -> Result<ListRequest, ApiFailure> {
    let (id_name, no_name) = id_names(version);
    let (ids, numbers, (sort, descending), size_name) = match version {
        ApiVersion::V1 => {
            (params.text(id_name).map(|id| vec![id]), params.text(no_name).map(|no| vec![no]), v1_sort(params)?, "page_size")
        }
        ApiVersion::V2 => (params.list(id_name)?, params.list(no_name)?, v2_sort(params)?, "limit"),
    };
    let ids = ids.map(|list| list.into_iter().map(|id| id.trim().to_string()).collect());
    let serials = match numbers {
        None => None,
        Some(list) => Some(
            list.iter()
                .map(|no| no.trim().parse::<i64>().map_err(|_| ApiFailure(format!("{no_name} must be a number"))))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    let group = match params.text("group_id") {
        None => None,
        Some(text) if text.trim() == "0" => Some(GroupFilter::Ungrouped),
        Some(text) => Some(GroupFilter::Id(
            text.trim().parse().map_err(|_| ApiFailure("group_id must be a number".into()))?,
        )),
    };
    // По умолчанию 1 на страницу, больше `PROFILE_PAGE_MAX` — ошибка.
    let paging = params.paging(size_name, 1, PROFILE_PAGE_MAX)?;
    Ok(ListRequest { group, ids, serials, sort, descending, paging, size_name })
}

/// Данные для строк списка одним чтением: названия групп, проверки прокси, версия сборки для `ua`.
struct RowLookups {
    groups: HashMap<i64, String>,
    checks: HashMap<i64, ProxyCheck>,
    engine_version: String,
}

impl RowLookups {
    fn load<E: Engine>(core: &Core<E>) -> Result<Self, ApiFailure> {
        let groups = core.store.list_groups()?.into_iter().map(|g| (g.id, g.name)).collect();
        let checks = core
            .store
            .list_proxies()?
            .into_iter()
            .filter_map(|proxy| proxy.check.map(|check| (proxy.id, check)))
            .collect();
        let engine_version = core.engine.installed().map(|build| build.version).unwrap_or_else(|| EXPECTED_VERSION.to_string());
        Ok(Self { groups, checks, engine_version })
    }
}

/// Секрет для строки запроса открытым текстом. Не расшифрован — пустая строка и предупреждение в журнал (без значений):
/// одна испорченная запись не ломает весь список.
fn secret_text(read: SecretRead, profile_id: &str, what: &str) -> Result<Value, ApiFailure> {
    match read {
        Ok(secret) => Ok(json!(secret.map(|secret| secret.expose().to_string()).unwrap_or_default())),
        Err(StoreError::Crypto) => {
            log::warn!("профиль {profile_id}: {what} не расшифровывается, в ответе Local API — пустая строка");
            Ok(json!(""))
        }
        Err(e) => Err(e.into()),
    }
}

/// Прокси профиля для `user_proxy_config`. Пароль не расшифровывается — он пустой, остальные поля как есть.
fn row_proxy<E: Engine>(core: &Core<E>, profile_id: &str, proxy_id: i64) -> Result<ProxyCredentials, ApiFailure> {
    match core.store.proxy_credentials(proxy_id) {
        Err(StoreError::Crypto) => {
            log::warn!(
                "профиль {profile_id}: пароль прокси {proxy_id} не расшифровывается, в ответе Local API — пустая строка"
            );
            let proxy = core
                .store
                .get_proxy(proxy_id)?
                .ok_or_else(|| ApiFailure(format!("Proxy not found: {proxy_id}")))?;
            Ok(ProxyCredentials {
                id: proxy.id,
                kind: proxy.kind,
                host: proxy.host,
                port: proxy.port,
                username: proxy.username,
                password: None,
            })
        }
        other => other.map_err(ApiFailure::from),
    }
}

/// Строка запроса профиля: поля примера своей версии, все значения строками, секреты открытым текстом.
fn profile_row<E: Engine>(
    core: &Core<E>,
    lookups: &RowLookups,
    profile: &ProfileRecord,
    version: ApiVersion,
) -> Result<Value, ApiFailure> {
    let (password, totp) = core.store.profile_secrets_each(&profile.id)?;
    let proxy = profile.proxy_id.map(|id| row_proxy(core, &profile.id, id)).transpose()?;
    let check = profile.proxy_id.and_then(|id| lookups.checks.get(&id));
    let fingerprint = profile.fingerprint()?;
    let extra = stored_extra(core, &profile.id)?;
    let (group_id, group_name) = match profile.group_id.and_then(|id| lookups.groups.get(&id).map(|name| (id, name))) {
        Some((id, name)) => (s(id), json!(name)),
        None => (json!("0"), json!(UNGROUPED)),
    };
    let mut row = json!({
        "name": profile.name,
        "group_id": group_id,
        "group_name": group_name,
        "remark": profile.note,
        "username": profile.username.clone().unwrap_or_default(),
        "password": secret_text(password, &profile.id, "пароль")?,
        "fakey": secret_text(totp, &profile.id, "ключ 2FA")?,
        "created_time": s(profile.created_at),
        "last_open_time": s(profile.last_opened_at.unwrap_or(0)),
        "ip": check.and_then(|c| c.exit_ip.clone()).unwrap_or_default(),
        "ip_country": check.and_then(|c| c.country.as_ref()).map(|country| country.to_ascii_lowercase()).unwrap_or_default(),
        "fbcc_proxy_acc_id": profile.proxy_id.map(s).unwrap_or_else(|| json!("")),
        "user_proxy_config": user_proxy_config(proxy.as_ref()),
        "fingerprint_config": fingerprint_config(&fingerprint, &extra, &lookups.engine_version),
        "ombra": ombra_fields(profile),
    });
    let version_keys = match version {
        ApiVersion::V1 => json!({
            "serial_number": s(profile.serial),
            "user_id": profile.id,
            "domain_name": profile.platform_domain,
            "sys_app_cate_id": s(profile.ext_category_id),
        }),
        ApiVersion::V2 => json!({
            "profile_no": s(profile.serial),
            "profile_id": profile.id,
            "platform": profile.platform_domain,
            "category_id": s(profile.ext_category_id),
            "ipchecker": "",
        }),
    };
    row.as_object_mut().expect("json! объект").extend(version_keys.as_object().expect("json! объект").clone());
    Ok(row)
}

pub(super) async fn list<E: Engine, const V: u8>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let version = ApiVersion::of(V);
    let request = list_request(&params, version)?;
    let records = core
        .store
        .list_profiles(&ListQuery {
            sort: request.sort,
            descending: request.descending,
            group: request.group,
            ..Default::default()
        })?;
    let selected: Vec<ProfileRecord> = records
        .into_iter()
        .filter(|profile| request.ids.as_ref().is_none_or(|ids| ids.contains(&profile.id)))
        .filter(|profile| request.serials.as_ref().is_none_or(|serials| serials.contains(&profile.serial)))
        .skip(request.paging.offset())
        .take(request.paging.limit())
        .collect();
    let lookups = RowLookups::load(&core)?;
    let rows =
        selected.iter().map(|profile| profile_row(&core, &lookups, profile, version)).collect::<Result<Vec<_>, _>>()?;
    let mut data = json!({ "list": rows, "page": request.paging.page });
    data[request.size_name] = json!(request.paging.size);
    Ok(Data(data))
}

/// Неизвестные и активные профили пачки — текстом для `msg`; `None` — пачку можно удалять.
fn delete_problems<E: Engine>(core: &Core<E>, ids: &[String]) -> Result<Option<ApiFailure>, ApiFailure> {
    let missing = missing_profiles(core, ids)?;
    let running: Vec<&str> =
        ids.iter().filter(|id| core.supervisor.status(id).is_active()).map(String::as_str).collect();
    let mut problems = Vec::new();
    if !missing.is_empty() {
        problems.push(format!("Profile not found: {}", missing.join(", ")));
    }
    if !running.is_empty() {
        problems.push(format!("Stop the profile first: {}", running.join(", ")));
    }
    Ok((!problems.is_empty()).then(|| ApiFailure(problems.join("; "))))
}

/// Удаление — в корзину (спецификация 3А): сначала проверяются все id (неизвестный или активный — ошибка с их списком). Само удаление — под замками
/// простоя всей пачки (`Core::delete_profiles_atomic`): запуск, успевший между проверкой и замками, отменяет всё.
pub(super) async fn delete<E: Engine, const V: u8>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let list_name = if V == 1 { "user_ids" } else { "profile_id" };
    let ids = unique_ids(params.list(list_name)?);
    if ids.is_empty() {
        return Err(ApiFailure(format!("{list_name} is required")));
    }
    if ids.len() > DELETE_MAX {
        return Err(ApiFailure(format!("{list_name}: at most {DELETE_MAX} profiles per request")));
    }
    if let Some(problem) = delete_problems(&core, &ids)? {
        return Err(problem);
    }
    if let Err(error) = core.delete_profiles_atomic(&ids).await {
        // Гонка с запуском или другим удалением: ответ — тот же, что дала бы проверка, если она что-то видит.
        return Err(delete_problems(&core, &ids)?.unwrap_or_else(|| error.into()));
    }
    Ok(Data(json!({})))
}

pub(super) async fn regroup<E: Engine>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let ids = unique_ids(params.list("user_ids")?);
    if ids.is_empty() {
        return Err(ApiFailure("user_ids is required".into()));
    }
    let group = group_param(&core, &params)?.ok_or_else(|| ApiFailure("group_id is required".into()))?;
    let missing = missing_profiles(&core, &ids)?;
    if !missing.is_empty() {
        return Err(ApiFailure(format!("Profile not found: {}", missing.join(", "))));
    }
    core.store.move_profiles(&ids, group)?;
    Ok(Data(json!({})))
}

/// v1: кэш всех профилей; при любом открытом браузере — отказ с текстом AdsPower.
pub(super) async fn delete_cache_all<E: Engine>(State(core): State<Arc<Core<E>>>) -> ApiResult {
    if core.supervisor.active_count() > 0 {
        return Err(ApiFailure(OPEN_BROWSERS.into()));
    }
    for profile in core.store.list_profiles(&ListQuery::default())? {
        core.clear_cache(&profile.id).await?;
    }
    Ok(Data(json!({})))
}

/// Cookies остановленного профиля в базе — пустой набор (снимков нет: профиль под очередью и не запущен).
fn clear_stored_cookies<E: Engine>(core: &Core<E>, id: &str) -> Result<(), ApiFailure> {
    let current = core.store.cookies_hash(id)?;
    core.store.replace_cookies(id, &[], &set_hash(&[]), current.as_deref())?;
    Ok(())
}

/// v2: выбранные типы данных выбранных профилей; все профили должны быть остановлены.
pub(super) async fn delete_cache_typed<E: Engine>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let ids = unique_ids(params.list("profile_id")?);
    if ids.is_empty() {
        return Err(ApiFailure("profile_id is required".into()));
    }
    let kinds = unique_ids(params.list("type")?);
    if kinds.is_empty() {
        return Err(ApiFailure("type is required".into()));
    }
    if let Some(unknown) = kinds.iter().find(|kind| cache_type_dirs(kind).is_none()) {
        return Err(ApiFailure(format!("type: unsupported cache type {unknown}")));
    }
    let missing = missing_profiles(&core, &ids)?;
    if !missing.is_empty() {
        return Err(ApiFailure(format!("Profile not found: {}", missing.join(", "))));
    }
    if ids.iter().any(|id| core.supervisor.status(id).is_active()) {
        return Err(ApiFailure(OPEN_BROWSERS.into()));
    }
    for id in &ids {
        let _idle = core.supervisor.lock_idle(id).await.map_err(|e| match e {
            SupervisorError::ProfileRunning => ApiFailure(OPEN_BROWSERS.into()),
            other => ApiFailure::from(other),
        })?;
        let dir = core.paths.profile_dir(id);
        for kind in &kinds {
            let mut targets: Vec<PathBuf> =
                cache_type_dirs(kind).unwrap_or(&[]).iter().map(|relative| dir.join(relative)).collect();
            if kind == "image_file" {
                targets.extend(core.paths.cache_paths(id));
            }
            for target in &targets {
                remove_path(target).map_err(CommandError::from)?;
            }
            if kind == "cookie" {
                clear_stored_cookies(&core, id)?;
            }
        }
    }
    Ok(Data(json!({})))
}

/// `data.cookies` — строка с JSON-массивом из базы Ombra: экспорт 2А и `expires` (секунды; сессионная — `-1`), как
/// в примере AdsPower. Скрипты передают эти cookies в `page.setCookie`, а Puppeteer знает срок только по `expires`.
pub(super) async fn profile_cookies<E: Engine>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let profile = find_profile(&core, &params, "profile_id", "profile_no")?;
    let stored = core.store.load_cookies(&profile.id)?;
    let items: Vec<Value> = cookies::json::to_values(&stored)
        .into_iter()
        .zip(&stored)
        .map(|(mut item, cookie)| {
            item["expires"] = json!(cookie.expires.unwrap_or(-1));
            item
        })
        .collect();
    Ok(Data(json!({ "cookies": Value::Array(items).to_string() })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::{ApiEnv, api_env, keys, names, with_query};
    use crate::app::groups::GroupForm;
    use crate::app::tests::create_with_proxy;
    use crate::fingerprint::{Field, Setting, validate};
    use serde_json::json;

    fn proxy_config(api: &ApiEnv, password: &str) -> Value {
        json!({
            "proxy_soft": "other",
            "proxy_type": "http",
            "proxy_host": "127.0.0.1",
            "proxy_port": api.proxy_port.to_string(),
            "proxy_user": "user",
            "proxy_password": password
        })
    }

    fn group(api: &ApiEnv, name: &str) -> i64 {
        api.core
            .create_group(
                &GroupForm { name: name.into(), color: "#2447c9".into(), note: String::new(), quick_access: false },
                1,
            )
            .unwrap()
            .id
    }

    async fn create_v2(api: &ApiEnv, body: Value) -> String {
        let created = api.post("/api/v2/browser-profile/create", body).await;
        assert_eq!(created["code"], 0, "{created}");
        created["data"]["profile_id"].as_str().unwrap().to_string()
    }

    fn record(api: &ApiEnv, id: &str) -> ProfileRecord {
        api.core.store.get_profile(id).unwrap().expect("профиль есть")
    }

    #[tokio::test]
    async fn empty_fakey_clears_the_totp_key() {
        let api = api_env().await;
        let id = create_v2(&api, json!({"name": "2FA", "fakey": "JBSWY3DPEHPK3PXP"})).await;
        assert!(api.core.store.profile_secrets(&id).unwrap().1.is_some());
        let updated = api.post("/api/v2/browser-profile/update", json!({"profile_id": &id, "fakey": ""})).await;
        assert_eq!(updated["code"], 0, "{updated}");
        assert!(api.core.store.profile_secrets(&id).unwrap().1.is_none(), "пустой fakey снимает ключ");
        let empty = create_v2(&api, json!({"name": "no 2FA", "fakey": ""})).await;
        assert!(api.core.store.profile_secrets(&empty).unwrap().1.is_none());
    }

    #[tokio::test]
    async fn minimal_v1_create_gets_defaults() {
        let api = api_env().await;
        let created = api.post("/api/v1/user/create", json!({"group_id": "0"})).await;
        assert_eq!(created["code"], 0, "{created}");
        // New Profile v1 в справочнике: data.id.
        assert_eq!(keys(&created["data"]), names(&["id"]));
        let profile = record(&api, created["data"]["id"].as_str().unwrap());
        assert_eq!(profile.name, format!("Профиль {}", profile.serial));
        assert_eq!((profile.group_id, profile.proxy_id), (None, None));
        let fingerprint = Fingerprint::from_json(&profile.fingerprint_json).unwrap();
        assert!(validate(&fingerprint, Arch::host()).is_empty(), "отпечаток от генератора согласован");
    }

    #[tokio::test]
    async fn v2_create_takes_every_field() {
        let api = api_env().await;
        let group_id = group(&api, "Магазины");
        let created = api
            .post(
                "/api/v2/browser-profile/create",
                json!({
                    "name": "Магазин",
                    "group_id": group_id.to_string(),
                    "remark": "заметка",
                    "platform": "shop.example",
                    "tabs": ["https://shop.example/", " "],
                    "username": "buyer",
                    "password": "p@ss",
                    "fakey": "JBSWY3DPEHPK3PXP",
                    "cookie": [{"domain": ".shop.example", "name": "sid", "value": "v1", "path": "/",
                                "expirationDate": 1924992000, "sameSite": "lax", "secure": true, "id": 1}],
                    "user_proxy_config": proxy_config(&api, "pass"),
                    "fingerprint_config": {"automatic_timezone": "0", "timezone": "Europe/Berlin", "fonts": ["all"]},
                    "repeat_config": [0],
                    "ipchecker": "ip2location",
                    "sys_app_cate_id": "0"
                }),
            )
            .await;
        assert_eq!(created["code"], 0, "{created}");
        // New Profile v2 в справочнике: data.profile_id и data.profile_no — строки.
        assert_eq!(keys(&created["data"]), names(&["profile_id", "profile_no"]));
        let id = created["data"]["profile_id"].as_str().unwrap().to_string();
        let profile = record(&api, &id);
        assert_eq!(created["data"]["profile_no"], profile.serial.to_string());
        assert_eq!(
            (profile.name.as_str(), profile.note.as_str(), profile.platform_domain.as_str()),
            ("Магазин", "заметка", "shop.example")
        );
        assert_eq!(profile.group_id, Some(group_id));
        assert_eq!(profile.start_urls, ["https://shop.example/"]);
        assert_eq!(profile.username.as_deref(), Some("buyer"));
        let (password, totp) = api.core.store.profile_secrets(&id).unwrap();
        assert_eq!(password.unwrap().expose(), "p@ss");
        assert_eq!(totp.unwrap().expose(), "JBSWY3DPEHPK3PXP");
        let proxy = api.core.store.proxy_credentials(profile.proxy_id.expect("прокси закреплён")).unwrap();
        assert_eq!(proxy.port, api.proxy_port);
        assert_eq!(proxy.username.as_deref(), Some("user"));
        assert_eq!(proxy.password.unwrap().expose(), "pass");
        let cookies = api.core.store.load_cookies(&id).unwrap();
        assert_eq!(cookies.iter().map(|c| (c.name.as_str(), c.value.as_str())).collect::<Vec<_>>(), [("sid", "v1")]);
        let fingerprint = Fingerprint::from_json(&profile.fingerprint_json).unwrap();
        assert_eq!(fingerprint.timezone, Setting::Manual { value: "Europe/Berlin".into() });
        let extra: Value = serde_json::from_str(&api.core.store.api_extra(&id).unwrap()).unwrap();
        assert_eq!(extra["fonts"], json!(["all"]));
    }

    #[tokio::test]
    async fn v1_uses_its_own_field_names() {
        let api = api_env().await;
        let created = api
            .post(
                "/api/v1/user/create",
                json!({"name": "V1", "group_id": 0, "domain_name": "v1.example", "open_urls": ["https://v1.example/"]}),
            )
            .await;
        assert_eq!(created["code"], 0, "{created}");
        let profile = record(&api, created["data"]["id"].as_str().unwrap());
        assert_eq!(profile.platform_domain, "v1.example");
        assert_eq!(profile.start_urls, ["https://v1.example/"]);
        assert_eq!(profile.group_id, None);
        let other = api
            .post("/api/v1/user/create", json!({"platform": "x.example", "tabs": ["https://x.example/"]}))
            .await;
        let profile = record(&api, other["data"]["id"].as_str().unwrap());
        assert_eq!(profile.platform_domain, "", "имена v2 в v1 не действуют");
        assert!(profile.start_urls.is_empty());
    }

    /// Ошибки проверки называют поля так, как их прислал скрипт: v1 — `open_urls`, v2 — `tabs`; поля шлюза —
    /// `user_proxy_config.…`. Ничего не создаётся.
    #[tokio::test]
    async fn validation_msg_names_fields_as_the_request() {
        let api = api_env().await;
        let bad_proxy = json!({"proxy_type": "http", "proxy_host": "bad host", "proxy_port": "8080"});
        let v1 = api
            .post(
                "/api/v1/user/create",
                json!({"group_id": "0", "open_urls": ["--headless"], "user_proxy_config": bad_proxy.clone()}),
            )
            .await;
        assert_eq!(v1["msg"], "Check the form fields (open_urls: invalid, user_proxy_config.proxy_host: invalid)", "{v1}");
        let v2 = api
            .post(
                "/api/v2/browser-profile/create",
                json!({"group_id": "0", "tabs": ["--headless"], "user_proxy_config": bad_proxy}),
            )
            .await;
        assert_eq!(v2["msg"], "Check the form fields (tabs: invalid, user_proxy_config.proxy_host: invalid)", "{v2}");
        assert_eq!(api.core.store.count_profiles().unwrap(), 0, "профиль не создан");
        assert!(api.core.store.list_proxies().unwrap().is_empty(), "прокси не создан");
    }

    #[test]
    fn field_names_follow_the_api_version() {
        assert_eq!(api_field_name("start_urls", ApiVersion::V1), "open_urls");
        assert_eq!(api_field_name("start_urls", ApiVersion::V2), "tabs");
        assert_eq!(api_field_name("platform_domain", ApiVersion::V1), "domain_name");
        assert_eq!(api_field_name("platform_domain", ApiVersion::V2), "platform");
        for version in [ApiVersion::V1, ApiVersion::V2] {
            for (ours, adspower) in [
                ("proxy_id", "proxyid"),
                ("proxy_host", "user_proxy_config.proxy_host"),
                ("proxy_port", "user_proxy_config.proxy_port"),
                ("proxy_username", "user_proxy_config.proxy_user"),
                ("proxy_password", "user_proxy_config.proxy_password"),
                ("note", "remark"),
                ("extra_args", "launch_args"),
                ("totp_secret", "fakey"),
                ("api_extra", "fingerprint_config"),
                ("fingerprint", "fingerprint_config"),
                ("screen", "fingerprint_config.screen_resolution"),
                ("gpu", "fingerprint_config.webgl_config"),
                ("timezone", "fingerprint_config.timezone"),
                ("name", "name"),
                ("group_id", "group_id"),
            ] {
                assert_eq!(api_field_name(ours, version), adspower, "{ours} ({version:?})");
            }
        }
    }

    #[tokio::test]
    async fn unknown_group_creates_nothing() {
        let api = api_env().await;
        let before = api.core.store.count_profiles().unwrap();
        let rejected = api.post("/api/v1/user/create", json!({"group_id": "999"})).await;
        assert_eq!(rejected["code"], -1, "{rejected}");
        assert_eq!(rejected["msg"], "Group not found: 999");
        assert_eq!(api.core.store.count_profiles().unwrap(), before);
    }

    #[tokio::test]
    async fn cookies_in_both_formats_and_ignore_cookie_error() {
        let api = api_env().await;
        let netscape = "# Netscape HTTP Cookie File\n.shop.example\tTRUE\t/\tTRUE\t1924992000\tsid\tv1\n";
        let from_text = create_v2(&api, json!({"cookie": netscape})).await;
        assert_eq!(api.core.store.count_cookies(&from_text).unwrap(), 1);

        let mixed = json!([{"domain": ".shop.example", "name": "ok", "value": "1"}, {"name": "no-domain"}]);
        let before = api.core.store.count_profiles().unwrap();
        let strict = api.post("/api/v2/browser-profile/create", json!({"cookie": mixed})).await;
        assert_eq!(strict["code"], -1, "{strict}");
        assert_eq!(strict["msg"], "cookie: invalid records 2");
        assert_eq!(api.core.store.count_profiles().unwrap(), before, "ничего не создано");

        let lenient = create_v2(&api, json!({"cookie": mixed, "ignore_cookie_error": "1"})).await;
        let names: Vec<String> = api.core.store.load_cookies(&lenient).unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["ok"]);
        let as_string = create_v2(&api, json!({"cookie": mixed.to_string(), "ignore_cookie_error": 1})).await;
        assert_eq!(api.core.store.count_cookies(&as_string).unwrap(), 1, "JSON строкой");

        let garbage = api.post("/api/v1/user/create", json!({"cookie": "not cookies at all"})).await;
        assert_eq!(garbage["code"], -1);
        assert_eq!(garbage["msg"], "Could not read the file for import");
    }

    #[tokio::test]
    async fn proxy_config_reuses_records_and_proxyid_pins() {
        let api = api_env().await;
        let a = create_v2(&api, json!({"user_proxy_config": proxy_config(&api, "pass")})).await;
        let b = create_v2(&api, json!({"user_proxy_config": proxy_config(&api, "pass")})).await;
        let proxy_id = record(&api, &a).proxy_id.unwrap();
        assert_eq!(record(&api, &b).proxy_id, Some(proxy_id), "та же запись списка");
        assert_eq!(api.core.store.list_proxies().unwrap().len(), 1);
        create_v2(&api, json!({"user_proxy_config": proxy_config(&api, "new")})).await;
        assert_eq!(api.core.store.proxy_credentials(proxy_id).unwrap().password.unwrap().expose(), "new");

        let no_proxy = create_v2(&api, json!({"user_proxy_config": {"proxy_soft": "no_proxy"}})).await;
        assert_eq!(record(&api, &no_proxy).proxy_id, None);
        let pinned = create_v2(&api, json!({"proxyid": proxy_id.to_string()})).await;
        assert_eq!(record(&api, &pinned).proxy_id, Some(proxy_id));
        let random = create_v2(&api, json!({"proxyid": "random"})).await;
        assert_eq!(record(&api, &random).proxy_id, None);
        let unknown = api.post("/api/v2/browser-profile/create", json!({"proxyid": "999"})).await;
        assert_eq!(unknown["msg"], "Proxy not found: 999");
        let mut ssh = proxy_config(&api, "pass");
        ssh["proxy_type"] = json!("ssh");
        assert_eq!(api.post("/api/v1/user/create", json!({"user_proxy_config": ssh})).await["code"], -1);
        let both = create_v2(&api, json!({"user_proxy_config": {"proxy_soft": "no_proxy"}, "proxyid": proxy_id})).await;
        assert_eq!(record(&api, &both).proxy_id, None, "user_proxy_config важнее proxyid");
    }

    #[tokio::test]
    async fn windows_ua_creates_nothing() {
        let api = api_env().await;
        let before = api.core.store.count_profiles().unwrap();
        let rejected = api
            .post(
                "/api/v2/browser-profile/create",
                json!({"fingerprint_config": {"ua": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                    (KHTML, like Gecko) Chrome/87.0.4280.141 Safari/537.36"}}),
            )
            .await;
        assert_eq!(rejected["code"], -1, "{rejected}");
        assert!(rejected["msg"].as_str().unwrap().contains("only macOS Chrome profiles are supported"), "{rejected}");
        assert_eq!(api.core.store.count_profiles().unwrap(), before);
    }

    /// `ombra.humanize` (этап Э5 движка): своё пространство в запросах и строках AdsPower — создание задаёт настройки
    /// поверх значений по умолчанию, правка меняет только переданные, запрос отдаёт их с seed строкой; неверное —
    /// ошибка с именем поля, профиль не меняется.
    #[tokio::test]
    async fn ombra_humanize_goes_in_create_update_and_list() {
        use crate::humanize::{Humanize, Preset};
        let api = api_env().await;
        let id = create_v2(&api, json!({"name": "Почерк", "ombra": {"humanize": {"preset": "careful", "paste_key": "Ctrl+Alt+V"}}})).await;
        let created = record(&api, &id);
        assert_eq!(created.humanize, Humanize { preset: Preset::Careful, paste_key: "Ctrl+Alt+V".into(), ..Humanize::default() });
        let listed = api.get("/api/v2/browser-profile/list?limit=10").await;
        assert_eq!(listed["data"]["list"][0]["ombra"]["humanize"], json!({"enabled": true, "preset": "careful", "block_input": true,
                                                        "paste_key": "Ctrl+Alt+V", "seed": created.humanize_seed.to_string()}));
        let off = api.post("/api/v1/user/update", json!({"user_id": &id, "ombra": {"humanize": {"enabled": false}}})).await;
        assert_eq!(off["code"], 0, "{off}");
        assert_eq!(record(&api, &id).humanize, Humanize { enabled: false, ..created.humanize.clone() }, "только переданное");
        assert_eq!(api.post("/api/v1/user/update", json!({"user_id": &id, "remark": "x"})).await["code"], 0);
        assert_eq!(record(&api, &id).humanize_seed, created.humanize_seed, "seed правкой не меняется");
        for (bad, msg) in [
            (json!({"preset": "slow"}), "ombra.humanize.preset"),
            (json!({"paste_key": "V"}), "ombra.humanize.paste_key"),
            (json!({"seed": "1"}), "ombra.humanize.seed"),
            (json!({"enabled": "yes"}), "ombra.humanize.enabled"),
        ] {
            let failed = api.post("/api/v2/browser-profile/update", json!({"profile_id": &id, "ombra": {"humanize": bad}})).await;
            assert_eq!(failed["code"], -1, "{failed}");
            assert!(failed["msg"].as_str().unwrap().contains(msg), "{failed}");
        }
        assert_eq!(record(&api, &id).humanize, Humanize { enabled: false, ..created.humanize.clone() }, "ошибка ничего не меняет");
        // Горячая клавиша — то же правило, что у формы и архива: ⌃ и/или ⌥, без ⌘.
        for key in crate::humanize::refused_keys() {
            let failed = api.post("/api/v2/browser-profile/update", json!({"profile_id": &id, "ombra": {"humanize": {"paste_key": &key}}})).await;
            assert_eq!(failed["code"], -1, "{key}: {failed}");
            assert!(failed["msg"].as_str().unwrap().contains("ombra.humanize.paste_key"), "{key}: {failed}");
        }
        for key in crate::humanize::ACCEPTED_KEYS {
            let ok = api.post("/api/v2/browser-profile/update", json!({"profile_id": &id, "ombra": {"humanize": {"paste_key": key}}})).await;
            assert_eq!(ok["code"], 0, "{key}: {ok}");
            assert_eq!(record(&api, &id).humanize.paste_key, key);
        }
    }

    #[tokio::test]
    async fn update_changes_only_given_fields() {
        let api = api_env().await;
        let id = create_v2(&api, json!({"name": "Было", "remark": "старая", "username": "u", "password": "p"})).await;
        let fingerprint_before = Fingerprint::from_json(&record(&api, &id).fingerprint_json).unwrap();

        let updated = api.post("/api/v1/user/update", json!({"user_id": &id, "remark": "новая"})).await;
        assert_eq!(updated["code"], 0, "{updated}");
        assert_eq!(updated["data"], json!({}), "Update Profile в справочнике: пустой data");
        let profile = record(&api, &id);
        assert_eq!((profile.name.as_str(), profile.note.as_str()), ("Было", "новая"));
        assert_eq!(profile.username.as_deref(), Some("u"));
        assert_eq!(api.core.store.profile_secrets(&id).unwrap().0.unwrap().expose(), "p");

        let group_id = group(&api, "Группа");
        assert_eq!(api.post("/api/v1/user/update", json!({"user_id": &id, "group_id": group_id})).await["code"], 0);
        assert_eq!(record(&api, &id).group_id, Some(group_id));
        let v2 = api
            .post(
                "/api/v2/browser-profile/update",
                json!({"profile_id": &id, "name": "Стало", "group_id": "0", "remark": "",
                       "fingerprint_config": {"automatic_timezone": "0", "timezone": "Asia/Tokyo"}}),
            )
            .await;
        assert_eq!(v2["code"], 0, "{v2}");
        let profile = record(&api, &id);
        assert_eq!((profile.name.as_str(), profile.note.as_str(), profile.group_id), ("Стало", "", None));
        let fingerprint = Fingerprint::from_json(&profile.fingerprint_json).unwrap();
        assert_eq!(fingerprint.timezone, Setting::Manual { value: "Asia/Tokyo".into() });
        assert!(fingerprint.manual.contains(&Field::Timezone), "заданное через API — «вручную»");
        assert_eq!(
            (fingerprint.seed, fingerprint.screen, &fingerprint.gpu),
            (fingerprint_before.seed, fingerprint_before.screen, &fingerprint_before.gpu),
            "остальной отпечаток не меняется"
        );
        let unknown = api.post("/api/v2/browser-profile/update", json!({"profile_id": "nope00", "name": "x"})).await;
        assert_eq!(unknown["msg"], "Profile not found: nope00");
    }

    #[tokio::test]
    async fn update_with_cookie_reaches_running_browser() {
        let api = api_env().await;
        let profile = create_with_proxy(&api.core, "Работает", api.proxy_port, 1);
        api.core.supervisor.start(&profile.id).await.unwrap();
        let updated = api
            .post(
                "/api/v1/user/update",
                json!({"user_id": &profile.id, "cookie": [{"domain": ".shop.example", "name": "live", "value": "1"}]}),
            )
            .await;
        assert_eq!(updated["code"], 0, "{updated}");
        let browser = api.core.engine.cdp(&api.core.paths.profile_dir(&profile.id));
        assert!(browser.browser_cookies().iter().any(|c| c.name == "live"), "cookie ушла в браузер");
        assert!(api.core.store.load_cookies(&profile.id).unwrap().iter().any(|c| c.name == "live"));
        api.core.supervisor.stop(&profile.id).await.unwrap();
    }
    fn touch(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    fn ids(listed: &Value) -> Vec<String> {
        listed["data"]["list"]
            .as_array()
            .unwrap_or_else(|| panic!("нет списка: {listed}"))
            .iter()
            .map(|row| row["user_id"].as_str().or(row["profile_id"].as_str()).unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn v1_list_rows_match_adspower_shape() {
        let api = api_env().await;
        let first = create_v2(&api, json!({"name": "Первый"})).await;
        let second = create_v2(
            &api,
            json!({"name": "Второй", "password": "p@ss", "fakey": "JBSWY3DPEHPK3PXP", "domain_name": "ignored.example",
                   "platform": "shop.example", "user_proxy_config": proxy_config(&api, "pass"),
                   "fingerprint_config": {"fonts": ["all"]}}),
        )
        .await;
        let proxy_id = record(&api, &second).proxy_id.unwrap();
        api.core.store.record_proxy_check(proxy_id, 5, Some(("203.0.113.24", "DE"))).unwrap();
        let listed = api.get("/api/v1/user/list?page_size=10").await;
        assert_eq!(listed["code"], 0, "{listed}");
        let data = &listed["data"];
        // Query Profile v1 в справочнике: list, page и page_size (числа).
        assert_eq!(keys(data), names(&["list", "page", "page_size"]));
        assert_eq!((data["page"].clone(), data["page_size"].clone()), (json!(1), json!(10)));
        assert_eq!(ids(&listed), [second.clone(), first.clone()], "по умолчанию — по номеру по убыванию");
        let row = &data["list"][0];
        assert_eq!(
            keys(row),
            names(&[
                "created_time", "domain_name", "fakey", "fbcc_proxy_acc_id", "fingerprint_config", "group_id",
                "group_name", "ip", "ip_country", "last_open_time", "name", "ombra", "password", "remark",
                "serial_number", "sys_app_cate_id", "user_id", "user_proxy_config", "username",
            ])
        );
        for field in [
            "serial_number", "user_id", "name", "group_id", "group_name", "domain_name", "username", "remark",
            "sys_app_cate_id", "created_time", "ip", "ip_country", "password", "last_open_time", "fakey",
            "fbcc_proxy_acc_id",
        ] {
            assert!(row[field].is_string(), "{field} — строка: {row}");
        }
        assert_eq!(row["domain_name"], "shop.example");
        assert_eq!((row["password"].clone(), row["fakey"].clone()), (json!("p@ss"), json!("JBSWY3DPEHPK3PXP")));
        assert_eq!((row["ip"].clone(), row["ip_country"].clone()), (json!("203.0.113.24"), json!("de")));
        assert_eq!(row["fbcc_proxy_acc_id"], proxy_id.to_string());
        assert_eq!(row["user_proxy_config"]["proxy_password"], "pass", "пароль прокси открытым текстом");
        assert_eq!((row["group_id"].clone(), row["group_name"].clone()), (json!("0"), json!("Ungrouped")));
        assert_eq!((row["last_open_time"].clone(), row["sys_app_cate_id"].clone()), (json!("0"), json!("0")));
        assert_eq!(row["fingerprint_config"]["fonts"], json!(["all"]));
        assert!(row["fingerprint_config"]["ua"].as_str().unwrap().contains("Macintosh"), "{row}");
        let plain = &data["list"][1];
        assert_eq!(plain["user_proxy_config"], json!({"proxy_soft": "no_proxy"}));
        assert_eq!((plain["password"].clone(), plain["fbcc_proxy_acc_id"].clone()), (json!(""), json!("")));
        assert_eq!((plain["ip"].clone(), plain["ip_country"].clone()), (json!(""), json!("")));
    }

    #[tokio::test]
    async fn v2_list_rows_match_adspower_shape() {
        let api = api_env().await;
        let group_id = group(&api, "Группа");
        let id = create_v2(&api, json!({"name": "V2", "group_id": group_id, "platform": "shop.example"})).await;
        let listed = api.post("/api/v2/browser-profile/list", json!({"limit": 10})).await;
        assert_eq!(listed["code"], 0, "{listed}");
        // Query Profile v2 в справочнике: list, page и limit.
        assert_eq!(keys(&listed["data"]), names(&["limit", "list", "page"]));
        assert_eq!(listed["data"]["limit"], 10);
        let row = &listed["data"]["list"][0];
        assert_eq!(
            keys(row),
            names(&[
                "category_id", "created_time", "fakey", "fbcc_proxy_acc_id", "fingerprint_config", "group_id",
                "group_name", "ip", "ip_country", "ipchecker", "last_open_time", "name", "ombra", "password",
                "platform", "profile_id", "profile_no", "remark", "user_proxy_config", "username",
            ])
        );
        assert_eq!(row["profile_id"], id.as_str());
        assert_eq!(row["profile_no"], record(&api, &id).serial.to_string());
        assert_eq!((row["group_id"].clone(), row["group_name"].clone()), (json!(group_id.to_string()), json!("Группа")));
        assert_eq!((row["platform"].clone(), row["category_id"].clone()), (json!("shop.example"), json!("0")));
    }

    #[tokio::test]
    async fn list_filters_by_group_ids_and_numbers() {
        let api = api_env().await;
        let group_id = group(&api, "Группа");
        let a = create_v2(&api, json!({"group_id": group_id})).await;
        let b = create_v2(&api, json!({})).await;
        let c = create_v2(&api, json!({})).await;
        let in_group = api.get(&format!("/api/v1/user/list?page_size=10&group_id={group_id}")).await;
        assert_eq!(ids(&in_group), std::slice::from_ref(&a));
        assert_eq!(ids(&api.get("/api/v1/user/list?page_size=10&group_id=0").await), [c.clone(), b.clone()]);
        assert_eq!(ids(&api.get(&format!("/api/v1/user/list?page_size=10&user_id={b}")).await), std::slice::from_ref(&b));
        let c_no = record(&api, &c).serial;
        assert_eq!(ids(&api.get(&format!("/api/v1/user/list?page_size=10&serial_number={c_no}")).await), std::slice::from_ref(&c));
        let by_ids = api.post("/api/v2/browser-profile/list", json!({"profile_id": [&a, &c], "limit": 10})).await;
        assert_eq!(ids(&by_ids), [c.clone(), a.clone()]);
        let a_no = record(&api, &a).serial;
        let by_numbers = api.post("/api/v2/browser-profile/list", json!({"profile_no": [a_no.to_string()], "limit": 10})).await;
        assert_eq!(ids(&by_numbers), std::slice::from_ref(&a));
        let bad = api.post("/api/v2/browser-profile/list", json!({"profile_no": ["x"]})).await;
        assert_eq!(bad["msg"], "profile_no must be a number");
    }

    #[tokio::test]
    async fn list_sorts_and_pages() {
        let api = api_env().await;
        let a = create_v2(&api, json!({})).await;
        let b = create_v2(&api, json!({})).await;
        let c = create_v2(&api, json!({})).await;
        assert_eq!(ids(&api.get("/api/v1/user/list").await), std::slice::from_ref(&c), "по умолчанию 1 на страницу");
        assert_eq!(ids(&api.get("/api/v1/user/list?page=2&page_size=1").await), std::slice::from_ref(&b));
        assert_eq!(ids(&api.get("/api/v1/user/list?page=4&page_size=1").await), Vec::<String>::new());
        let too_big = api.get("/api/v1/user/list?page_size=101").await;
        assert_eq!(too_big["msg"], "page_size must be between 1 and 100");
        assert_eq!(api.get("/api/v1/user/list?page=0").await["msg"], "page must be at least 1");
        let ascending = with_query("/api/v1/user/list", &[("page_size", "10"), ("user_sort", r#"{"serial_number":"asc"}"#)]);
        assert_eq!(ids(&api.get(&ascending).await), [a.clone(), b.clone(), c.clone()]);
        api.core.store.touch_last_opened(&b, 100).unwrap();
        api.core.store.touch_last_opened(&a, 50).unwrap();
        let by_opened = api
            .post("/api/v2/browser-profile/list", json!({"sort_type": "last_open_time", "sort_order": "desc", "limit": 10}))
            .await;
        assert_eq!(ids(&by_opened), [b.clone(), a.clone(), c.clone()], "не открывавшиеся — в конце");
        let created_asc = api
            .post("/api/v2/browser-profile/list", json!({"sort_type": "created_time", "sort_order": "asc", "limit": 10}))
            .await;
        assert_eq!(ids(&created_asc), [a.clone(), b.clone(), c.clone()]);
        let bad = api.post("/api/v2/browser-profile/list", json!({"sort_type": "name"})).await;
        assert_eq!(bad["msg"], "sort_type: unsupported sort field name");
        let limit = api.post("/api/v2/browser-profile/list", json!({"limit": 0})).await;
        assert_eq!(limit["msg"], "limit must be between 1 and 100");
    }

    #[tokio::test]
    async fn delete_checks_every_profile_first() {
        let api = api_env().await;
        let a = create_v2(&api, json!({"user_proxy_config": proxy_config(&api, "pass")})).await;
        let b = create_v2(&api, json!({})).await;
        let c = create_v2(&api, json!({"user_proxy_config": proxy_config(&api, "pass")})).await;
        let deleted = api.post("/api/v1/user/delete", json!({"user_ids": [&a, &b]})).await;
        assert_eq!(deleted["code"], 0, "{deleted}");
        assert_eq!(deleted["data"], json!({}));
        assert!(api.core.store.get_profile(&a).unwrap().is_none() && api.core.store.get_profile(&b).unwrap().is_none());
        for id in [&a, &b] {
            assert!(api.core.store.get_profile_any(id).unwrap().unwrap().deleted_at.is_some(), "в корзине, не стёрт");
        }
        assert_eq!(api.core.store.list_proxies().unwrap().len(), 1, "запись прокси остаётся");

        let with_unknown = api.post("/api/v1/user/delete", json!({"user_ids": [&c, "nope00"]})).await;
        assert_eq!(with_unknown["code"], -1);
        assert_eq!(with_unknown["msg"], "Profile not found: nope00");
        assert!(api.core.store.get_profile(&c).unwrap().is_some(), "ничего не удалено");

        api.core.supervisor.start(&c).await.unwrap();
        let running = api.post("/api/v2/browser-profile/delete", json!({"Profile_id": [&c]})).await;
        assert_eq!(running["msg"], format!("Stop the profile first: {c}"));
        assert!(api.core.store.get_profile(&c).unwrap().is_some());
        api.core.supervisor.stop(&c).await.unwrap();
        assert_eq!(api.post("/api/v2/browser-profile/delete", json!({"profile_id": [&c]})).await["code"], 0);
        assert!(api.core.store.get_profile(&c).unwrap().is_none());
        assert!(api.core.store.get_profile_any(&c).unwrap().unwrap().deleted_at.is_some());

        let too_many: Vec<String> = (0..101).map(|i| format!("p{i}")).collect();
        let rejected = api.post("/api/v1/user/delete", json!({"user_ids": too_many})).await;
        assert_eq!(rejected["msg"], "user_ids: at most 100 profiles per request");
        assert_eq!(api.post("/api/v1/user/delete", json!({})).await["msg"], "user_ids is required");
    }

    #[tokio::test]
    async fn regroup_moves_existing_profiles_only() {
        let api = api_env().await;
        let group_id = group(&api, "Группа");
        let a = create_v2(&api, json!({})).await;
        let b = create_v2(&api, json!({})).await;
        let moved = api.post("/api/v1/user/regroup", json!({"user_ids": [&a, &b], "group_id": group_id.to_string()})).await;
        assert_eq!(moved["code"], 0, "{moved}");
        assert_eq!((record(&api, &a).group_id, record(&api, &b).group_id), (Some(group_id), Some(group_id)));
        assert_eq!(api.post("/api/v1/user/regroup", json!({"user_ids": [&a], "group_id": "0"})).await["code"], 0);
        assert_eq!(record(&api, &a).group_id, None);
        let unknown_group = api.post("/api/v1/user/regroup", json!({"user_ids": [&a], "group_id": "999"})).await;
        assert_eq!(unknown_group["msg"], "Group not found: 999");
        let unknown_profile = api.post("/api/v1/user/regroup", json!({"user_ids": [&a, "nope00"], "group_id": group_id})).await;
        assert_eq!(unknown_profile["msg"], "Profile not found: nope00");
        assert_eq!(record(&api, &a).group_id, None, "ничего не перенесено");
        assert_eq!(api.post("/api/v1/user/regroup", json!({"user_ids": [&a]})).await["msg"], "group_id is required");
    }

    #[tokio::test]
    async fn delete_cache_v1_clears_every_stopped_profile() {
        let api = api_env().await;
        let a = create_v2(&api, json!({"user_proxy_config": proxy_config(&api, "pass")})).await;
        let b = create_v2(&api, json!({})).await;
        api.core.supervisor.start(&a).await.unwrap();
        let busy = api.post("/api/v1/user/delete-cache", json!({})).await;
        assert_eq!(busy["code"], -1);
        assert_eq!(busy["msg"], OPEN_BROWSERS);
        api.core.supervisor.stop(&a).await.unwrap();
        let caches = [
            api.core.paths.profile_dir(&a).join("Default/Cache/data_0"),
            api.core.paths.profile_dir(&b).join("Default/Code Cache/js"),
        ];
        let storage = api.core.paths.profile_dir(&a).join("Default/Local Storage/leveldb");
        for path in caches.iter().chain([&storage]) {
            touch(path);
        }
        let cleared = api.post("/api/v1/user/delete-cache", json!({})).await;
        assert_eq!(cleared["code"], 0, "{cleared}");
        assert!(caches.iter().all(|path| !path.exists()), "кэш всех профилей удалён");
        assert!(storage.exists(), "v1 трогает только кэш");
    }

    #[tokio::test]
    async fn delete_cache_v2_removes_each_type() {
        let api = api_env().await;
        let id = create_v2(
            &api,
            json!({"user_proxy_config": proxy_config(&api, "pass"),
                   "cookie": [{"domain": ".shop.example", "name": "sid", "value": "1"}]}),
        )
        .await;
        let dir = api.core.paths.profile_dir(&id);
        let kinds = ["local_storage", "indexeddb", "history", "cookie", "extension_cache"];
        for kind in kinds {
            for rel in cache_type_dirs(kind).unwrap() {
                touch(&dir.join(rel));
            }
        }
        let page_cache = dir.join("Default/Cache/data_0");
        touch(&page_cache);
        let present = |kind: &str| cache_type_dirs(kind).unwrap().iter().all(|rel| dir.join(rel).exists());

        let first = api
            .post("/api/v2/browser-profile/delete-cache", json!({"profile_id": [&id], "type": ["local_storage", "history"]}))
            .await;
        assert_eq!(first["code"], 0, "{first}");
        assert!(!present("local_storage") && cache_type_dirs("history").unwrap().iter().all(|rel| !dir.join(rel).exists()));
        assert!(present("indexeddb") && present("cookie") && present("extension_cache") && page_cache.exists());
        assert_eq!(api.core.store.count_cookies(&id).unwrap(), 1, "cookies в базе не тронуты");

        let cookie = api.post("/api/v2/browser-profile/delete-cache", json!({"profile_id": [&id], "type": ["cookie"]})).await;
        assert_eq!(cookie["code"], 0, "{cookie}");
        assert!(!dir.join("Default/Cookies").exists());
        assert_eq!(api.core.store.count_cookies(&id).unwrap(), 0, "cookies в базе удалены");

        let rest = api
            .post(
                "/api/v2/browser-profile/delete-cache",
                json!({"profile_id": [&id], "type": ["image_file", "indexeddb", "extension_cache"]}),
            )
            .await;
        assert_eq!(rest["code"], 0, "{rest}");
        assert!(!page_cache.exists() && !dir.join("Default/IndexedDB").exists());
        assert!(!dir.join("Default/Extension State").exists());

        let unknown_type = api.post("/api/v2/browser-profile/delete-cache", json!({"profile_id": [&id], "type": ["passwords"]})).await;
        assert_eq!(unknown_type["msg"], "type: unsupported cache type passwords");
        let unknown_profile =
            api.post("/api/v2/browser-profile/delete-cache", json!({"profile_id": ["nope00"], "type": ["history"]})).await;
        assert_eq!(unknown_profile["msg"], "Profile not found: nope00");
        api.core.supervisor.start(&id).await.unwrap();
        let busy = api.post("/api/v2/browser-profile/delete-cache", json!({"profile_id": [&id], "type": ["history"]})).await;
        assert_eq!(busy["msg"], OPEN_BROWSERS);
        api.core.supervisor.stop(&id).await.unwrap();
    }

    #[tokio::test]
    async fn cookies_come_as_json_string() {
        let api = api_env().await;
        let id = create_v2(&api, json!({"cookie": [{"domain": ".shop.example", "name": "sid", "value": "v1"}]})).await;
        let answer = api.get(&format!("/api/v2/browser-profile/cookies?profile_id={id}")).await;
        assert_eq!(answer["code"], 0, "{answer}");
        // Query profile cookies в справочнике: data.cookies — строка с JSON-массивом.
        assert_eq!(keys(&answer["data"]), names(&["cookies"]));
        let text = answer["data"]["cookies"].as_str().expect("cookies — строка");
        let cookies: Vec<Value> = serde_json::from_str(text).unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!((cookies[0]["name"].clone(), cookies[0]["value"].clone()), (json!("sid"), json!("v1")));
        let serial = record(&api, &id).serial;
        let by_number = api.get(&format!("/api/v2/browser-profile/cookies?profile_no={serial}")).await;
        assert_eq!(by_number["data"], answer["data"]);
        let empty = create_v2(&api, json!({})).await;
        let none = api.get(&format!("/api/v2/browser-profile/cookies?profile_id={empty}")).await;
        let parsed: Vec<Value> = serde_json::from_str(none["data"]["cookies"].as_str().unwrap()).unwrap();
        assert!(parsed.is_empty());
        assert_eq!(api.get("/api/v2/browser-profile/cookies?profile_id=nope00").await["code"], -1);
    }

    #[tokio::test]
    async fn cookies_carry_expires_for_puppeteer() {
        let api = api_env().await;
        let id = create_v2(
            &api,
            json!({"cookie": [
                {"domain": ".shop.example", "name": "kept", "value": "1", "expirationDate": 1924992000},
                {"domain": ".shop.example", "name": "session", "value": "2"}
            ]}),
        )
        .await;
        let answer = api.get(&format!("/api/v2/browser-profile/cookies?profile_id={id}")).await;
        assert_eq!(answer["code"], 0, "{answer}");
        let cookies: Vec<Value> = serde_json::from_str(answer["data"]["cookies"].as_str().unwrap()).unwrap();
        let named = |name: &str| cookies.iter().find(|c| c["name"] == name).cloned().unwrap();
        // Пример AdsPower отдаёт `expires` (секунды): так `page.setCookie` сохраняет срок.
        let kept = named("kept");
        assert_eq!(kept["expires"], json!(1924992000));
        assert_eq!((kept["expirationDate"].clone(), kept["session"].clone()), (json!(1924992000), json!(false)), "форма 2А");
        let session = named("session");
        assert_eq!(session["expires"], json!(-1), "сессионная — -1, как в CDP");
        assert_eq!(session["session"], json!(true));
        assert!(session.get("expirationDate").is_none());
    }

    #[tokio::test]
    async fn v2_update_sets_launch_args() {
        let api = api_env().await;
        let id = create_v2(&api, json!({"name": "Было"})).await;
        let update = |body: Value| api.post("/api/v2/browser-profile/update", body);
        let set = update(json!({"profile_id": &id, "launch_args": ["--disable-sync", " "]})).await;
        assert_eq!(set["code"], 0, "{set}");
        assert_eq!(record(&api, &id).extra_args, ["--disable-sync"]);
        assert_eq!(update(json!({"profile_id": &id, "remark": "x"})).await["code"], 0);
        assert_eq!(record(&api, &id).extra_args, ["--disable-sync"], "без launch_args аргументы не меняются");

        let forbidden = update(json!({"profile_id": &id, "name": "Стало", "launch_args": ["--proxy-server=direct://"]})).await;
        assert_eq!(forbidden["code"], -1, "{forbidden}");
        assert_eq!(forbidden["msg"], "Check the form fields (launch_args: forbidden)");
        let not_flag = update(json!({"profile_id": &id, "launch_args": "[\"disable-sync\"]"})).await;
        assert_eq!(not_flag["msg"], "Check the form fields (launch_args: not_a_flag)");
        let profile = record(&api, &id);
        assert_eq!((profile.name.as_str(), profile.extra_args.as_slice()), ("Было", ["--disable-sync".to_string()].as_slice()));

        assert_eq!(update(json!({"profile_id": &id, "launch_args": []})).await["code"], 0);
        assert!(record(&api, &id).extra_args.is_empty(), "пустой список снимает аргументы профиля");
        // В Update Profile v1 у AdsPower нет launch_args.
        let v1 = api.post("/api/v1/user/update", json!({"user_id": &id, "launch_args": ["--disable-sync"]})).await;
        assert_eq!(v1["code"], 0, "{v1}");
        assert!(record(&api, &id).extra_args.is_empty());
    }

    #[tokio::test]
    async fn undecryptable_secret_does_not_break_the_list() {
        let api = api_env().await;
        let broken = create_v2(
            &api,
            json!({"name": "Пароль", "password": "p@ss", "fakey": "JBSWY3DPEHPK3PXP",
                   "user_proxy_config": proxy_config(&api, "pass")}),
        )
        .await;
        let totp_broken = create_v2(&api, json!({"name": "2FA", "password": "ok", "fakey": "JBSWY3DPEHPK3PXP"})).await;
        // Не расшифровываются (например, база перенесена с другого Mac): пароль первого профиля, его прокси
        // и ключ 2FA второго.
        let proxy_id = record(&api, &broken).proxy_id.unwrap();
        {
            let db = api.core.store.raw();
            db.execute("UPDATE profiles SET password_enc = x'00' WHERE id = ?1", [&broken]).unwrap();
            db.execute("UPDATE profiles SET totp_enc = x'00' WHERE id = ?1", [&totp_broken]).unwrap();
            db.execute("UPDATE proxies SET password_enc = x'00' WHERE id = ?1", [proxy_id]).unwrap();
        }
        let listed = api.get("/api/v1/user/list?page_size=10").await;
        assert_eq!(listed["code"], 0, "{listed}");
        let rows = listed["data"]["list"].as_array().unwrap();
        let row = |id: &str| rows.iter().find(|row| row["user_id"] == id).cloned().unwrap();
        let first = row(&broken);
        assert_eq!((first["password"].clone(), first["fakey"].clone()), (json!(""), json!("JBSWY3DPEHPK3PXP")));
        let proxy = &first["user_proxy_config"];
        assert_eq!(proxy["proxy_password"], "", "{proxy}");
        assert_eq!((proxy["proxy_host"].clone(), proxy["proxy_user"].clone()), (json!("127.0.0.1"), json!("user")));
        assert_eq!(proxy["proxy_port"], api.proxy_port.to_string());
        let second = row(&totp_broken);
        assert_eq!((second["password"].clone(), second["fakey"].clone()), (json!("ok"), json!("")));
    }

    #[tokio::test]
    async fn path_like_ids_are_unknown_and_delete_nothing() {
        let api = api_env().await;
        let id = create_v2(&api, json!({})).await;
        let victim = api.core.paths.profile_dir(&id).parent().unwrap().join("victim");
        touch(&victim.join("Default/History"));
        let outside = victim.parent().unwrap().join("x");
        touch(&outside.join("Default/History"));
        for bad in ["../x", "../victim", victim.to_str().unwrap()] {
            let typed = api
                .post("/api/v2/browser-profile/delete-cache", json!({"profile_id": [bad], "type": ["history", "image_file"]}))
                .await;
            assert_eq!(typed["msg"], format!("Profile not found: {bad}"));
            let deleted = api.post("/api/v2/browser-profile/delete", json!({"profile_id": [bad]})).await;
            assert_eq!(deleted["msg"], format!("Profile not found: {bad}"));
        }
        assert!(victim.join("Default/History").exists() && outside.join("Default/History").exists());
        assert!(api.core.store.get_profile(&id).unwrap().is_some());
    }

    #[tokio::test]
    async fn delete_cache_cookie_removes_chromium_network_store() {
        let api = api_env().await;
        let id = create_v2(&api, json!({"cookie": [{"domain": ".shop.example", "name": "sid", "value": "1"}]})).await;
        let dir = api.core.paths.profile_dir(&id);
        let stores = ["Default/Network/Cookies", "Default/Network/Cookies-journal", "Default/Cookies", "Default/Cookies-journal"];
        for rel in stores {
            touch(&dir.join(rel));
        }
        let keep = [dir.join("Default/History"), dir.join("Default/Local Storage/x")];
        for path in &keep {
            touch(path);
        }
        let cleared = api.post("/api/v2/browser-profile/delete-cache", json!({"profile_id": [&id], "type": ["cookie"]})).await;
        assert_eq!(cleared["code"], 0, "{cleared}");
        assert!(stores.iter().all(|rel| !dir.join(rel).exists()));
        assert_eq!(api.core.store.count_cookies(&id).unwrap(), 0);
        assert!(keep.iter().all(|path| path.exists()));
    }

    #[tokio::test]
    async fn profile_set_fields_follow_adspower() {
        let api = api_env().await;
        let shop = api.core.create_ext_category("Магазин", "", 1).unwrap();
        // Значения AdsPower по умолчанию принимаются как раньше.
        let plain = api.post("/api/v1/user/create", json!({"group_id": "0", "sys_app_cate_id": "0"})).await;
        assert_eq!(plain["code"], 0, "{plain}");
        let v1 = api.post("/api/v1/user/create", json!({"group_id": "0", "sys_app_cate_id": shop.id.to_string()})).await;
        let v1_id = v1["data"]["id"].as_str().unwrap().to_string();
        assert_eq!(record(&api, &v1_id).ext_category_id, shop.id);
        let listed = api.get(&format!("/api/v1/user/list?user_id={v1_id}")).await;
        assert_eq!(listed["data"]["list"][0]["sys_app_cate_id"], shop.id.to_string());
        let v2_id = create_v2(&api, json!({"name": "v2", "sys_app_cate_id": shop.id})).await;
        assert_eq!(record(&api, &v2_id).ext_category_id, shop.id);
        let updated = api.post("/api/v2/browser-profile/update", json!({"profile_id": &v2_id, "category_id": "0"})).await;
        assert_eq!(updated["code"], 0, "{updated}");
        let listed = api.post("/api/v2/browser-profile/list", json!({"profile_id": [&v2_id], "limit": 10})).await;
        assert_eq!(listed["data"]["list"][0]["category_id"], "0");
        let updated = api.post("/api/v1/user/update", json!({"user_id": &v1_id, "sys_app_cate_id": "0"})).await;
        assert_eq!((updated["code"].clone(), record(&api, &v1_id).ext_category_id), (json!(0), 0));
        // Ошибки — именами запроса этой операции.
        assert_eq!(api.post("/api/v1/user/create", json!({"group_id": "0", "sys_app_cate_id": "77"})).await["msg"], "Category not found: 77");
        assert_eq!(api.post("/api/v1/user/create", json!({"group_id": "0", "sys_app_cate_id": "x"})).await["msg"], "sys_app_cate_id must be a number");
        let wrong = api.post("/api/v2/browser-profile/update", json!({"profile_id": &v2_id, "category_id": 77})).await;
        assert_eq!(wrong["msg"], "Category not found: 77");
    }
}
