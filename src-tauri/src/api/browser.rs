//! Служебное и браузер Local API: запуск, остановка, статус (спецификация 2Б, «Служебное и браузер»).
use super::envelope::{ApiFailure, ApiResult, Data, s};
use super::params::Params;
use super::server::both;
use crate::app::forms::clean_lines;
use crate::app::Core;
use crate::engine::Engine;
use crate::engine::args::{ArgsError, check_extra_args};
use crate::store::profiles::{ListQuery, ProfileRecord};
use crate::supervisor::{RunningInfo, StartOptions, SupervisorError};
use axum::Router;
use axum::extract::State;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// Сколько запуск через API ждёт уже идущий запуск того же профиля (решение плана 4).
pub const START_WAIT: Duration = Duration::from_secs(120);

/// Версия Local API AdsPower: у v1 и v2 разные имена параметров.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiVersion {
    V1,
    V2,
}

impl ApiVersion {
    /// Версия по const-параметру обработчика: `1` — v1, иначе v2.
    pub(crate) const fn of(v: u8) -> Self {
        if v == 1 { Self::V1 } else { Self::V2 }
    }
}

/// Имена id и номера профиля в параметрах версии.
pub(crate) fn id_names(version: ApiVersion) -> (&'static str, &'static str) {
    match version {
        ApiVersion::V1 => ("user_id", "serial_number"),
        ApiVersion::V2 => ("profile_id", "profile_no"),
    }
}

/// Маршруты «Служебное и браузер»; `server::api_routes` подключает их до слоя проверок.
pub(super) fn routes<E: Engine>() -> Router<Arc<Core<E>>> {
    Router::new()
        .route("/api/v1/browser/start", both(start::<E, 1>))
        .route("/api/v2/browser-profile/start", both(start::<E, 2>))
        .route("/api/v1/browser/stop", both(stop::<E, 1>))
        .route("/api/v2/browser-profile/stop", both(stop::<E, 2>))
        .route("/api/v1/browser/active", both(active::<E, 1>))
        .route("/api/v2/browser-profile/active", both(active::<E, 2>))
        .route("/api/v1/browser/local-active", both(local_active::<E>))
}

/// Профиль по id (`id_name`), а если id нет — по номеру (`no_name`); при обоих побеждает id, как у AdsPower.
pub(crate) fn find_profile<E: Engine>(
    core: &Core<E>,
    params: &Params,
    id_name: &str,
    no_name: &str,
) -> Result<ProfileRecord, ApiFailure> {
    if let Some(id) = params.text(id_name) {
        let id = id.trim().to_string();
        return core.store.get_profile(&id)?.ok_or_else(|| profile_not_found(&id));
    }
    let Some(number) = params.text(no_name) else {
        return Err(ApiFailure(format!("{id_name} or {no_name} is required")));
    };
    let serial: i64 = number.trim().parse().map_err(|_| ApiFailure(format!("{no_name} must be a number")))?;
    core.store
        .list_profiles(&ListQuery::default())?
        .into_iter()
        .find(|profile| profile.serial == serial)
        .ok_or_else(|| profile_not_found(number.trim()))
}

fn profile_not_found(key: &str) -> ApiFailure {
    ApiFailure(format!("Profile not found: {key}"))
}

/// Параметры этого запуска; аргументы проверяются до запуска, чтобы не тратить проверку шлюза.
/// `launch_args` — `Params::list` (задача 4): JSON-массив, одно значение или JSON-массив в строке (так он приходит в query у GET v1).
/// Пустой после очистки список (`[]`, `[""]`, `[" "]`) — как без параметра: аргументы профиля на этот запуск не стираются.
fn start_options(params: &Params, version: ApiVersion) -> Result<StartOptions, ApiFailure> {
    let extra_args = params.list("launch_args")?.map(|args| clean_lines(&args)).filter(|args| !args.is_empty());
    if let Some(args) = &extra_args {
        check_extra_args(args).map_err(|e| match e {
            ArgsError::Forbidden(arg) => ApiFailure(format!("Extra arguments are not allowed: {arg}")),
            ArgsError::NotAFlag(arg) => ApiFailure(format!("launch_args must start with --: {arg}")),
            ArgsError::BadStartUrl(arg) => ApiFailure(format!("launch_args: invalid value {arg}")),
        })?;
    }
    let (without_last_tabs, clear_cache) = match version {
        ApiVersion::V1 => (params.flag("open_tabs")? == Some(true), params.flag("clear_cache_after_closing")?),
        ApiVersion::V2 => (params.flag("last_opened_tabs")? == Some(false), params.flag("delete_cache")?),
    };
    Ok(StartOptions {
        headless: params.flag("headless")?.unwrap_or(false),
        extra_args,
        restore_tabs: without_last_tabs.then_some(false),
        start_urls: None,
        clear_cache_on_exit: clear_cache.unwrap_or(false),
    })
}

/// `data` ответа на запуск: адреса отладки браузера, как в примере Open Browser.
pub(crate) fn browser_data(info: &RunningInfo) -> Value {
    json!({
        "ws": {
            "selenium": format!("127.0.0.1:{}", info.debug_port),
            "puppeteer": info.cdp_ws,
        },
        "debug_port": s(info.debug_port),
        "webdriver": "",
    })
}

/// `data` статуса: «пауза» — тоже Active, браузер открыт.
pub(crate) fn active_data(info: Option<RunningInfo>) -> Value {
    match info {
        Some(info) => {
            let mut data = browser_data(&info);
            data["status"] = json!("Active");
            data
        }
        None => json!({ "status": "Inactive" }),
    }
}

pub(super) async fn start<E: Engine, const V: u8>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let version = ApiVersion::of(V);
    let (id_name, no_name) = id_names(version);
    let profile = find_profile(&core, &params, id_name, no_name)?;
    let options = start_options(&params, version)?;
    let info = core.supervisor.start_or_attach(&profile.id, options, START_WAIT).await?;
    Ok(Data(browser_data(&info)))
}

pub(super) async fn stop<E: Engine, const V: u8>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let version = ApiVersion::of(V);
    let (id_name, no_name) = id_names(version);
    let profile = find_profile(&core, &params, id_name, no_name)?;
    match core.supervisor.stop(&profile.id).await {
        // Скрипты зовут stop в `finally`: остановленный профиль — не ошибка.
        Ok(()) | Err(SupervisorError::NotRunning) => Ok(Data(json!({}))),
        Err(e) => Err(e.into()),
    }
}

pub(super) async fn active<E: Engine, const V: u8>(State(core): State<Arc<Core<E>>>, params: Params) -> ApiResult {
    let version = ApiVersion::of(V);
    let (id_name, no_name) = id_names(version);
    let profile = find_profile(&core, &params, id_name, no_name)?;
    Ok(Data(active_data(core.supervisor.running_info(&profile.id))))
}

/// Все открытые браузеры («работает» и «пауза», без закрывающихся), по id.
pub(super) async fn local_active<E: Engine>(State(core): State<Arc<Core<E>>>) -> Data {
    let list: Vec<Value> = core
        .supervisor
        .running()
        .into_iter()
        .map(|(id, info)| {
            let mut item = browser_data(&info);
            item["user_id"] = json!(id);
            item
        })
        .collect();
    Data(json!({ "list": list }))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::{ApiEnv, api_env, keys, names, with_query};
    use crate::app::tests::{create_with_proxy, form, test_proxy};
    use crate::relay::test_support::switchable_proxy;
    use crate::supervisor::test_engine::wait_until;
    use crate::supervisor::{ProfileStatus, RunningInfo};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use tokio::sync::Notify;

    fn params(value: Value) -> Params {
        Params::from_map(value.as_object().expect("объект").clone())
    }

    fn add_profile(api: &ApiEnv, name: &str) -> ProfileRecord {
        create_with_proxy(&api.core, name, api.proxy_port, 1)
    }

    async fn wait_status(api: &ApiEnv, id: &str, done: impl Fn(&ProfileStatus) -> bool) -> ProfileStatus {
        let mut last = api.core.supervisor.status(id);
        wait_until(&format!("статус профиля {id}"), || {
            last = api.core.supervisor.status(id);
            done(&last)
        })
        .await;
        last
    }

    fn running_info() -> RunningInfo {
        RunningInfo {
            pid: 1,
            cdp_ws: "ws://127.0.0.1:9222/devtools/browser/x".into(),
            debug_port: 9222,
            relay_port: 5,
            proxy_id: 3,
            exit_ip: None,
            country: None,
            timezone: "UTC".into(),
            started_at: 0,
        }
    }

    #[test]
    fn browser_data_matches_adspower_shape() {
        // Open Browser v1/v2 в справочнике: ws.selenium, ws.puppeteer, debug_port и webdriver — строки.
        assert_eq!(
            browser_data(&running_info()),
            json!({
                "ws": {"selenium": "127.0.0.1:9222", "puppeteer": "ws://127.0.0.1:9222/devtools/browser/x"},
                "debug_port": "9222",
                "webdriver": ""
            })
        );
        assert_eq!(active_data(None), json!({"status": "Inactive"}));
        let active = active_data(Some(running_info()));
        assert_eq!(keys(&active), names(&["debug_port", "status", "webdriver", "ws"]));
        assert_eq!(active["status"], "Active");
    }

    #[test]
    fn start_options_follow_api_version() {
        assert_eq!(start_options(&params(json!({})), ApiVersion::V1).unwrap(), StartOptions::default());
        assert_eq!(start_options(&params(json!({"open_tabs": "0"})), ApiVersion::V1).unwrap().restore_tabs, None);
        assert_eq!(start_options(&params(json!({"open_tabs": 1})), ApiVersion::V1).unwrap().restore_tabs, Some(false));
        assert_eq!(start_options(&params(json!({"last_opened_tabs": "1"})), ApiVersion::V2).unwrap().restore_tabs, None);
        assert_eq!(
            start_options(&params(json!({"last_opened_tabs": "0"})), ApiVersion::V2).unwrap().restore_tabs,
            Some(false)
        );
        let v2 = start_options(
            &params(json!({"delete_cache": "1", "headless": 1, "launch_args": "[\"--a\", \" \"]"})),
            ApiVersion::V2,
        )
        .unwrap();
        assert!(v2.clear_cache_on_exit && v2.headless);
        assert_eq!(v2.extra_args, Some(vec!["--a".to_string()]), "JSON-массив в строке, пустые отброшены");
        assert!(
            !start_options(&params(json!({"clear_cache_after_closing": "1"})), ApiVersion::V2).unwrap().clear_cache_on_exit,
            "имя v1 в v2 не действует"
        );
        assert!(start_options(&params(json!({"clear_cache_after_closing": "1"})), ApiVersion::V1).unwrap().clear_cache_on_exit);
        let forbidden = start_options(&params(json!({"launch_args": ["--proxy-server=direct://"]})), ApiVersion::V1);
        assert_eq!(forbidden.unwrap_err().0, "Extra arguments are not allowed: --proxy-server=direct://");
        let not_flag = start_options(&params(json!({"launch_args": ["disable-sync"]})), ApiVersion::V1);
        assert_eq!(not_flag.unwrap_err().0, "launch_args must start with --: disable-sync");
    }

    #[test]
    fn empty_launch_args_keep_the_profile_arguments() {
        for empty in [json!([]), json!([""]), json!([" "]), json!("[]"), json!("[\" \"]")] {
            for version in [ApiVersion::V1, ApiVersion::V2] {
                let options = start_options(&params(json!({"launch_args": empty})), version).unwrap();
                assert_eq!(options.extra_args, None, "{empty}: аргументы профиля не стираются");
            }
        }
    }

    #[tokio::test]
    async fn empty_launch_args_start_with_the_profile_arguments() {
        let api = api_env().await;
        let mut with_args = form("Аргументы", Some(test_proxy(&api.core, api.proxy_port).id));
        with_args.extra_args = vec!["--disable-sync".into()];
        let profile = api.core.create_profile(&with_args, 1).unwrap();
        let started = api
            .post("/api/v2/browser-profile/start", json!({"profile_id": &profile.id, "launch_args": [" "]}))
            .await;
        assert_eq!(started["code"], 0, "{started}");
        assert_eq!(api.core.engine.last_request().extra_args, ["--disable-sync"], "аргументы профиля");
        api.core.supervisor.stop(&profile.id).await.unwrap();
    }

    #[tokio::test]
    async fn start_v1_returns_debug_endpoints_and_repeat_attaches() {
        let api = api_env().await;
        let profile = add_profile(&api, "Магазин");
        let path = format!("/api/v1/browser/start?user_id={}", profile.id);
        let first = api.get(&path).await;
        assert_eq!(first["code"], 0, "{first}");
        assert_eq!(first["msg"], "success");
        let data = &first["data"];
        assert_eq!(keys(data), names(&["debug_port", "webdriver", "ws"]));
        assert_eq!(keys(&data["ws"]), names(&["puppeteer", "selenium"]));
        let port = data["debug_port"].as_str().expect("debug_port — строка");
        assert_eq!(data["ws"]["selenium"], format!("127.0.0.1:{port}"));
        assert!(data["ws"]["puppeteer"].as_str().unwrap().starts_with("ws://127.0.0.1:"), "{data}");
        assert_eq!(data["webdriver"], "");
        let again = api.get(&path).await;
        assert_eq!(again["data"], first["data"], "повторный запуск — тот же браузер");
        assert_eq!(api.core.engine.launches(), 1, "второго браузера нет");
        api.core.supervisor.stop(&profile.id).await.unwrap();
    }

    #[tokio::test]
    async fn start_v2_finds_by_number_and_prefers_id() {
        let api = api_env().await;
        let a = add_profile(&api, "A");
        let b = add_profile(&api, "B");
        let by_number = api.post("/api/v2/browser-profile/start", json!({"profile_no": b.serial})).await;
        assert_eq!(by_number["code"], 0, "{by_number}");
        assert!(api.core.supervisor.running_info(&b.id).is_some(), "номер — числом");
        let both = api
            .post("/api/v2/browser-profile/start", json!({"profile_id": &a.id, "profile_no": b.serial.to_string()}))
            .await;
        assert_eq!(both["code"], 0, "{both}");
        assert!(api.core.supervisor.running_info(&a.id).is_some(), "id важнее номера");
        assert_eq!(api.core.engine.launches(), 2);
        // Имя параметра с заглавной, как в документации (`Profile_id`).
        let stopped = api.post("/api/v2/browser-profile/stop", json!({"Profile_id": &a.id})).await;
        assert_eq!(stopped["code"], 0, "{stopped}");
        assert!(api.core.supervisor.running_info(&a.id).is_none());
        api.core.supervisor.stop(&b.id).await.unwrap();
    }

    #[tokio::test]
    async fn start_during_start_waits_for_the_same_browser() {
        let api = Arc::new(api_env().await);
        let profile = add_profile(&api, "Ждёт");
        let hold = Arc::new(Notify::new());
        *api.core.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
        let path = format!("/api/v1/browser/start?user_id={}", profile.id);
        let first = tokio::spawn({
            let api = api.clone();
            let path = path.clone();
            async move { api.get(&path).await }
        });
        wait_status(&api, &profile.id, |s| matches!(s, ProfileStatus::Starting)).await;
        let second = tokio::spawn({
            let api = api.clone();
            async move { api.get(&path).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!second.is_finished(), "второй запрос ждёт идущий запуск");
        hold.notify_one();
        let (first, second) = (first.await.unwrap(), second.await.unwrap());
        assert_eq!(first["code"], 0, "{first}");
        assert_eq!(second["code"], 0, "{second}");
        assert_eq!(second["data"], first["data"]);
        assert_eq!(api.core.engine.launches(), 1);
        api.core.supervisor.stop(&profile.id).await.unwrap();
    }

    #[tokio::test]
    async fn v1_launch_parameters_reach_the_browser_and_cache_is_cleared_after_session() {
        let api = api_env().await;
        let profile = add_profile(&api, "Параметры");
        let path = with_query(
            "/api/v1/browser/start",
            &[
                ("user_id", profile.id.as_str()),
                ("launch_args", r#"["--disable-sync"]"#),
                ("headless", "1"),
                ("open_tabs", "1"),
                ("clear_cache_after_closing", "1"),
                ("ip_tab", "0"),
                ("cdp_mask", "1"),
            ],
        );
        let started = api.get(&path).await;
        assert_eq!(started["code"], 0, "{started}");
        let request = api.core.engine.last_request();
        assert_eq!(request.extra_args, ["--disable-sync"]);
        assert!(request.headless);
        assert!(!request.restore_tabs, "open_tabs=1 — без вкладок прошлой сессии");
        let dir = api.core.paths.profile_dir(&profile.id);
        let cache = dir.join("Default/Cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("data_0"), b"x").unwrap();
        let storage = dir.join("Default/Local Storage");
        std::fs::create_dir_all(&storage).unwrap();
        let stopped = api.get(&format!("/api/v1/browser/stop?user_id={}", profile.id)).await;
        assert_eq!(stopped["code"], 0, "{stopped}");
        assert!(!cache.exists(), "кэш удалён после сеанса");
        assert!(storage.exists(), "хранилище сайтов не тронуто");
    }

    #[tokio::test]
    async fn v2_launch_parameters_and_defaults() {
        let api = api_env().await;
        let profile = add_profile(&api, "V2");
        let started = api
            .post(
                "/api/v2/browser-profile/start",
                json!({
                    "profile_id": &profile.id,
                    "launch_args": ["--disable-sync"],
                    "headless": "1",
                    "last_opened_tabs": "0",
                    "delete_cache": "1",
                    "proxy_detection": "0",
                    "password_filling": "0",
                    "device_scale": "1"
                }),
            )
            .await;
        assert_eq!(started["code"], 0, "{started}");
        let request = api.core.engine.last_request();
        assert_eq!(request.extra_args, ["--disable-sync"]);
        assert!(request.headless && !request.restore_tabs);
        let cache = api.core.paths.profile_dir(&profile.id).join("Default/Cache");
        std::fs::create_dir_all(&cache).unwrap();
        assert_eq!(api.post("/api/v2/browser-profile/stop", json!({"profile_id": &profile.id})).await["code"], 0);
        assert!(!cache.exists(), "delete_cache=1 — кэш удалён");

        // Без параметров — как в профиле: окно есть, вкладки восстанавливаются, кэш остаётся.
        let plain = api.post("/api/v2/browser-profile/start", json!({"profile_id": &profile.id})).await;
        assert_eq!(plain["code"], 0, "{plain}");
        let request = api.core.engine.last_request();
        assert!(!request.headless && request.restore_tabs);
        assert!(request.extra_args.is_empty(), "аргументы профиля");
        std::fs::create_dir_all(&cache).unwrap();
        assert_eq!(api.post("/api/v2/browser-profile/stop", json!({"profile_id": &profile.id})).await["code"], 0);
        assert!(cache.exists(), "без флага кэш не трогается");
    }

    #[tokio::test]
    async fn forbidden_launch_argument_is_rejected_before_start() {
        let api = api_env().await;
        let profile = add_profile(&api, "Аргумент");
        let rejected = api
            .post(
                "/api/v2/browser-profile/start",
                json!({"profile_id": &profile.id, "launch_args": ["--proxy-server=direct://"]}),
            )
            .await;
        assert_eq!(rejected["code"], -1, "{rejected}");
        assert_eq!(rejected["data"], json!({}));
        assert!(rejected["msg"].as_str().unwrap().contains("--proxy-server"), "{rejected}");
        assert_eq!(api.core.engine.launches(), 0);
        assert_eq!(api.core.supervisor.status(&profile.id), ProfileStatus::Stopped);
    }

    #[tokio::test]
    async fn unknown_profile_and_missing_identifier_are_errors() {
        let api = api_env().await;
        let unknown = api.get("/api/v1/browser/start?user_id=nope00").await;
        assert_eq!(unknown["code"], -1);
        assert_eq!(unknown["msg"], "Profile not found: nope00");
        assert_eq!(api.get("/api/v1/browser/start").await["msg"], "user_id or serial_number is required");
        assert_eq!(api.get("/api/v1/browser/active?serial_number=abc").await["msg"], "serial_number must be a number");
        let unknown_stop = api.post("/api/v2/browser-profile/stop", json!({"profile_id": "nope00"})).await;
        assert_eq!(unknown_stop["code"], -1);
        assert_eq!(
            api.get("/api/v2/browser-profile/active").await["msg"],
            "profile_id or profile_no is required"
        );
    }

    #[tokio::test]
    async fn stop_of_stopped_profile_succeeds() {
        let api = api_env().await;
        let profile = add_profile(&api, "Стоп");
        assert_eq!(api.get(&format!("/api/v1/browser/start?user_id={}", profile.id)).await["code"], 0);
        let stopped = api.get(&format!("/api/v1/browser/stop?user_id={}", profile.id)).await;
        assert_eq!(stopped["code"], 0, "{stopped}");
        assert_eq!(stopped["data"], json!({}));
        assert_eq!(api.core.supervisor.status(&profile.id), ProfileStatus::Stopped);
        assert_eq!(api.get(&format!("/api/v1/browser/stop?serial_number={}", profile.serial)).await["code"], 0);
        assert_eq!(api.post("/api/v2/browser-profile/stop", json!({"profile_id": &profile.id})).await["code"], 0);
    }

    #[tokio::test]
    async fn active_reports_status_in_both_versions() {
        let api = api_env().await;
        let profile = add_profile(&api, "Статус");
        let inactive = api.get(&format!("/api/v1/browser/active?user_id={}", profile.id)).await;
        assert_eq!(inactive["code"], 0, "{inactive}");
        assert_eq!(inactive["data"], json!({"status": "Inactive"}));
        let started = api.get(&format!("/api/v1/browser/start?user_id={}", profile.id)).await;
        let v1 = api.get(&format!("/api/v1/browser/active?user_id={}", profile.id)).await;
        assert_eq!(keys(&v1["data"]), names(&["debug_port", "status", "webdriver", "ws"]));
        assert_eq!(v1["data"]["status"], "Active");
        assert_eq!(v1["data"]["ws"], started["data"]["ws"]);
        let v2 = api.get(&format!("/api/v2/browser-profile/active?profile_no={}", profile.serial)).await;
        assert_eq!(v2["data"], v1["data"]);
        api.core.supervisor.stop(&profile.id).await.unwrap();
        let after = api.get(&format!("/api/v2/browser-profile/active?profile_id={}", profile.id)).await;
        assert_eq!(after["data"], json!({"status": "Inactive"}));
    }

    #[tokio::test]
    async fn paused_profile_counts_as_active() {
        use tokio::io::AsyncWriteExt;
        let api = api_env().await;
        let (proxy, down) = switchable_proxy().await;
        let profile = create_with_proxy(&api.core, "Пауза", proxy.port(), 1);
        let info = api.core.supervisor.start(&profile.id).await.unwrap();
        down.store(true, Ordering::SeqCst);
        for _ in 0..crate::relay::UNREACHABLE_AFTER {
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", info.relay_port)).await.unwrap();
            s.write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n").await.unwrap();
            let head = String::from_utf8(crate::relay::test_support::read_head(&mut s).await).unwrap();
            assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        }
        wait_status(&api, &profile.id, |s| matches!(s, ProfileStatus::Paused(_))).await;
        let active = api.get(&format!("/api/v2/browser-profile/active?profile_id={}", profile.id)).await;
        assert_eq!(active["data"]["status"], "Active", "{active}");
        let local = api.get("/api/v1/browser/local-active").await;
        assert_eq!(local["data"]["list"].as_array().unwrap().len(), 1, "{local}");
        api.core.supervisor.stop(&profile.id).await.unwrap();
    }

    /// Браузер уже забрала остановка (процесс ещё завершается), статус ещё «работает»: `/active` (v1, v2) и
    /// `local-active` не отдают его старый `ws` — как и запуск, считают его закрытым.
    #[tokio::test]
    async fn closing_browser_is_not_active() {
        let api = api_env().await;
        let profile = add_profile(&api, "Закрывается");
        assert_eq!(api.get(&format!("/api/v1/browser/start?user_id={}", profile.id)).await["code"], 0);
        let hold = Arc::new(Notify::new());
        *api.core.engine.hold_next_stop.lock().unwrap() = Some(hold.clone());
        let stopping = tokio::spawn({
            let core = api.core.clone();
            let id = profile.id.clone();
            async move { core.supervisor.stop(&id).await }
        });
        wait_until("остановка дошла до движка", || api.core.engine.hold_next_stop.lock().unwrap().is_none()).await;
        assert!(api.core.engine.hold_next_stop.lock().unwrap().is_none(), "остановка дошла до движка");
        assert!(matches!(api.core.supervisor.status(&profile.id), ProfileStatus::Running(_)), "статус ещё «работает»");

        for path in [
            format!("/api/v1/browser/active?user_id={}", profile.id),
            format!("/api/v2/browser-profile/active?profile_id={}", profile.id),
        ] {
            let answer = api.get(&path).await;
            assert_eq!(answer["data"], json!({"status": "Inactive"}), "{path}: {answer}");
        }
        assert_eq!(api.get("/api/v1/browser/local-active").await["data"], json!({"list": []}));

        hold.notify_one();
        stopping.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn local_active_lists_open_profiles() {
        let api = api_env().await;
        let a = add_profile(&api, "A");
        let b = add_profile(&api, "B");
        add_profile(&api, "Стоит");
        let empty = api.get("/api/v1/browser/local-active").await;
        assert_eq!(empty["data"], json!({"list": []}));
        api.get(&format!("/api/v1/browser/start?user_id={}", a.id)).await;
        api.get(&format!("/api/v1/browser/start?user_id={}", b.id)).await;
        let listed = api.get("/api/v1/browser/local-active").await;
        assert_eq!(listed["code"], 0, "{listed}");
        let list = listed["data"]["list"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        for item in list {
            assert_eq!(keys(item), names(&["debug_port", "user_id", "webdriver", "ws"]));
        }
        let mut ids: Vec<&str> = list.iter().map(|item| item["user_id"].as_str().unwrap()).collect();
        ids.sort_unstable();
        let mut expected = vec![a.id.as_str(), b.id.as_str()];
        expected.sort_unstable();
        assert_eq!(ids, expected);
        api.core.supervisor.stop_all().await;
    }

    #[tokio::test]
    async fn unrestricted_local_api_ignores_the_legacy_limit() {
        let api = api_env().await;
        api.core.store.set_setting("profiles.max_running", "1").unwrap();
        let a = add_profile(&api, "A");
        let b = add_profile(&api, "B");
        let first = api.get(&format!("/api/v1/browser/start?user_id={}", a.id)).await;
        let second = api.get(&format!("/api/v1/browser/start?user_id={}", b.id)).await;
        api.core.supervisor.stop_all().await;
        assert_eq!(first["code"], 0);
        assert_eq!(second["code"], 0, "{second}");
    }

    #[tokio::test]
    async fn dropped_start_request_does_not_leave_profile_starting() {
        let api = Arc::new(api_env().await);
        let profile = add_profile(&api, "Обрыв");
        let hold = Arc::new(Notify::new());
        *api.core.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
        let request = tokio::spawn({
            let api = api.clone();
            let id = profile.id.clone();
            async move { api.get(&format!("/api/v1/browser/start?user_id={id}")).await }
        });
        wait_status(&api, &profile.id, |s| matches!(s, ProfileStatus::Starting)).await;
        request.abort();
        let _ = request.await;
        hold.notify_one();
        let status = wait_status(&api, &profile.id, |s| !matches!(s, ProfileStatus::Starting)).await;
        assert!(matches!(status, ProfileStatus::Running(_)), "запуск доходит до конца без клиента: {status:?}");
        let active = api.get(&format!("/api/v1/browser/active?user_id={}", profile.id)).await;
        assert_eq!(active["data"]["status"], "Active");
        api.core.supervisor.stop(&profile.id).await.unwrap();
    }
}
