use super::*;
use crate::cookies::{Cookie, SameSite, set_hash};
use crate::engine::cdp::CdpClient;
use crate::engine::fake_cdp::FakeCdp;
use crate::engine::{EngineError, ExitInfo};
use crate::fingerprint::Setting;
use crate::relay::UNREACHABLE_AFTER;
use crate::relay::test_support::{closed_port, port_closes, read_head, spawn_echo, switchable_proxy};
use crate::store::cookies::MergeCount;
use crate::store::profiles::ProfileInput;
use crate::store::secrets::SecretBox;
use crate::store::tests::memory_store;
use super::pidfile::PidRecord;
use super::test_engine::{FakeEngine, add_profile, add_profile_with, add_proxy, fast_timings, geo_stack, wait_until};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

struct Harness {
    sup: Supervisor<FakeEngine>,
    engine: Arc<FakeEngine>,
    store: Arc<Store>,
    paths: Paths,
    proxy: std::net::SocketAddr,
    events: Arc<Mutex<Vec<Event>>>,
    _dir: tempfile::TempDir,
}

async fn harness(geo_status: u16) -> Harness {
    harness_with(geo_status, fast_timings()).await
}

#[tokio::test]
async fn direct_profiles_launch_without_any_proxy_and_do_not_reserve_a_fake_proxy() {
    let h = harness(200).await;
    let fp = crate::fingerprint::generate(crate::fingerprint::Catalog::builtin(), crate::fingerprint::Arch::host(), &mut crate::fingerprint::OsRandom).unwrap();
    let mut input = ProfileInput::new("direct", fp.to_json());
    input.network = Some(crate::network::ProfileNetwork { direct: true, ip_checker: crate::network::IpChecker::IpWhoIs });
    let first = h.store.create_profile(&input, 1).unwrap();
    let second = h.store.create_profile(&input, 2).unwrap();
    assert!(h.store.list_proxies().unwrap().is_empty());
    let a = h.sup.start(&first.id).await.unwrap();
    let b = h.sup.start(&second.id).await.unwrap();
    assert_eq!((a.proxy_id, b.proxy_id), (0, 0));
    assert_eq!(a.exit_ip.as_deref(), Some("203.0.113.24"));
    assert!(h.sup.proxy_users().is_empty());
    assert_eq!(h.store.get_profile(&first.id).unwrap().unwrap().proxy_id, None);
    h.sup.stop_all().await;
}

#[tokio::test]
async fn launch_honors_a_profile_specific_ip_checker_without_automatic_fallback() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let record = h.store.get_profile(&id).unwrap().unwrap();
    let mut input = ProfileInput::new(record.name, record.fingerprint_json);
    input.proxy_id = record.proxy_id;
    // Стенд предоставляет только ipwho.is. Выбранный ipapi.is нельзя заменить им молча.
    input.network = Some(crate::network::ProfileNetwork { direct: false, ip_checker: crate::network::IpChecker::IpapiIs });
    h.store.update_profile(&id, &input).unwrap();
    assert!(matches!(h.sup.start(&id).await, Err(SupervisorError::GeoUnavailable)));
    assert!(h.sup.proxy_users().is_empty());
}

async fn harness_with(geo_status: u16, timings: Timings) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let store = Arc::new(memory_store());
    let engine = Arc::new(FakeEngine::new());
    let stack = geo_stack(geo_status).await;
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let seen = events.clone();
    let sink: EventSink = Arc::new(move |event: Event| seen.lock().unwrap().push(event));
    let sup = Supervisor::new(paths.clone(), store.clone(), engine.clone(), Arc::new(stack.geo), sink, timings);
    Harness { sup, engine, store, paths, proxy: stack.proxy, events, _dir: dir }
}

async fn wait_for(sup: &Supervisor<FakeEngine>, id: &str, done: impl Fn(&ProfileStatus) -> bool) -> ProfileStatus {
    let mut last = sup.status(id);
    wait_until(&format!("статус профиля {id}"), || {
        last = sup.status(id);
        done(&last)
    })
    .await;
    last
}

/// CONNECT через прослойку профиля к `target`; первая строка ответа прослойки.
async fn tunnel_status(relay_port: u16, target: std::net::SocketAddr) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", relay_port)).await.unwrap();
    s.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes()).await.unwrap();
    let head = String::from_utf8(read_head(&mut s).await).unwrap();
    head.lines().next().unwrap_or_default().to_string()
}

fn status_events(events: &Mutex<Vec<Event>>, id: &str) -> Vec<ProfileStatus> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::Status { profile_id, status } if profile_id == id => Some(status.clone()),
            _ => None,
        })
        .collect()
}

/// Ждёт, пока подставной движок получит `count` запусков.
async fn wait_launches(engine: &FakeEngine, count: u32) {
    wait_until(&format!("движок получил {count} запусков"), || engine.launches() >= count).await;
}

/// Кэш и файл cookies браузера в папке профиля: (папка кэша, файл cookies).
fn fill_profile_dir(paths: &Paths, id: &str) -> (PathBuf, PathBuf) {
    let dir = paths.profile_dir(id);
    let cache = dir.join("Default/Cache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("data_0"), b"x").unwrap();
    let cookies = dir.join("Default/Cookies");
    std::fs::write(&cookies, b"c").unwrap();
    (cache, cookies)
}

fn cookie(name: &str, value: &str) -> Cookie {
    Cookie {
        domain: ".example.com".into(),
        name: name.into(),
        value: value.into(),
        path: "/".into(),
        expires: Some(4_102_444_800),
        http_only: false,
        secure: true,
        same_site: Some(SameSite::Lax),
        host_only: false,
    }
}

/// Cookies профиля в базе: (имя, значение), по имени.
fn stored(store: &Store, id: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> =
        store.load_cookies(id).unwrap().into_iter().map(|c| (c.name, c.value)).collect();
    pairs.sort();
    pairs
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
}

/// Сколько снимков браузер уже отдал.
fn snapshots(cdp: &FakeCdp) -> usize {
    cdp.calls().iter().filter(|call| call.as_str() == "Storage.getCookies").count()
}

/// Сколько раз браузер спросили о страницах (`Target.getTargets`).
fn page_checks(cdp: &FakeCdp) -> usize {
    cdp.calls().iter().filter(|call| call.as_str() == "Target.getTargets").count()
}

/// В базе — хэш текущего набора браузера: снимок его записал.
fn snapshot_written(store: &Store, id: &str, cdp: &FakeCdp) -> bool {
    store.cookies_hash(id).unwrap() == Some(set_hash(&cdp.browser_cookies()))
}

/// Зашифрованное значение единственной cookie профиля: при перезаписи оно меняется (новый nonce).
fn cookie_blob(store: &Store, id: &str) -> Vec<u8> {
    store.raw().query_row("SELECT value_enc FROM cookies WHERE profile_id = ?1", [id], |r| r.get(0)).unwrap()
}

#[tokio::test]
async fn start_resolves_geo_and_stop_cleans_up() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let info = h.sup.start(&id).await.unwrap();
    assert_eq!(
        (info.exit_ip.as_deref(), info.country.as_deref(), info.timezone.as_str()),
        (Some("203.0.113.24"), Some("DE"), "Europe/Berlin")
    );
    let request = h.engine.last_request();
    assert_eq!((request.resolved.timezone.as_str(), request.resolved.language.as_str()), ("Europe/Berlin", "de-DE"));
    assert!(request.relay_addr.ip().is_loopback());
    assert_eq!(request.relay_addr.port(), info.relay_port);
    assert_eq!(request.profile_dir, h.paths.profile_dir(&id));
    assert!(request.restore_tabs, "вкладки прошлой сессии по умолчанию восстанавливаются");
    assert_eq!(info.cdp_ws, h.engine.cdp(&h.paths.profile_dir(&id)).ws_url(), "у браузера профиля свой CDP");
    CdpClient::connect(&info.cdp_ws).await.expect("CDP браузера отвечает");
    assert!(matches!(h.sup.status(&id), ProfileStatus::Running(_)));
    let pids = pidfile::list(&h.paths.run_dir()).unwrap();
    assert_eq!(pids.len(), 1);
    assert_eq!(pids[0].1.as_ref().unwrap().pid, info.pid);
    assert!(h.store.get_profile(&id).unwrap().unwrap().last_opened_at.is_some());
    assert!(matches!(DirLock::acquire(&h.paths.profile_dir(&id)), Err(LockError::Busy)));
    assert_eq!(h.sup.active_count(), 1);
    let check = h.store.get_proxy(info.proxy_id).unwrap().unwrap().check.expect("проверка при запуске записана");
    assert_eq!(check.country.as_deref(), Some("DE"));

    h.sup.stop(&id).await.unwrap();
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    assert!(pidfile::list(&h.paths.run_dir()).unwrap().is_empty());
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
    assert_eq!(h.sup.active_count(), 0);
    assert!(port_closes((std::net::Ipv4Addr::LOCALHOST, info.relay_port).into()).await, "прослойка закрыта");
    // Слушатель подставного CDP закрывается в своей задаче — ждём с пределом.
    let mut cdp_closed = false;
    for _ in 0..50 {
        if CdpClient::connect(&info.cdp_ws).await.is_err() {
            cdp_closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(cdp_closed, "CDP закрылся вместе с браузером");
    assert!(matches!(h.sup.stop(&id).await, Err(SupervisorError::NotRunning)));
    assert!(matches!(
        status_events(&h.events, &id).as_slice(),
        [ProfileStatus::Starting, ProfileStatus::Running(_), ProfileStatus::Stopped]
    ));
}

#[tokio::test]
async fn start_writes_ombra_bookmarks_folder() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let mark = crate::store::bookmarks::Bookmark { title: "Shop".into(), url: "https://shop.test/".into() };
    h.store.set_profile_bookmarks(&id, &[mark]).unwrap();
    h.sup.start(&id).await.unwrap();
    let text = std::fs::read_to_string(h.paths.profile_dir(&id).join("Default/Bookmarks")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["roots"]["bookmark_bar"]["children"][0]["name"], "Ombra");
    assert_eq!(json["roots"]["bookmark_bar"]["children"][0]["children"][0]["url"], "https://shop.test/");
}

#[tokio::test]
async fn start_passes_the_set_extensions_that_exist_on_disk() {
    use crate::store::extensions::ExtensionRecord;
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    assert!(h.store.get_profile(&id).unwrap().unwrap().ext_category_id == 0);
    h.sup.start(&id).await.unwrap();
    assert!(h.engine.last_request().extensions.is_empty(), "пустой набор 0 — расширений нет");
    h.sup.stop(&id).await.unwrap();
    let record = |ext: &str| ExtensionRecord {
        id: ext.into(),
        name: "E".into(),
        version: "1.0".into(),
        source: "file".into(),
        manifest_version: 3,
        gateway_risk: false,
        added_at: 1,
        updated_at: 1,
        description: String::new(),
        file_name: None,
    };
    let (present, missing) = ("a".repeat(32), "b".repeat(32));
    h.store.upsert_extension(&record(&present)).unwrap();
    h.store.upsert_extension(&record(&missing)).unwrap();
    let dir = h.paths.extension_dir(&present, "1.0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), "{}").unwrap();
    let set = h.store.create_ext_category("Набор", "", 1).unwrap();
    h.store.set_ext_category_items(set.id, &[missing.clone(), present.clone()]).unwrap();
    h.store.set_profiles_ext_category(std::slice::from_ref(&id), set.id).unwrap();
    h.sup.start(&id).await.unwrap();
    // Папки `missing` на диске нет: расширение пропущено, запуск не упал.
    assert_eq!(h.engine.last_request().extensions, vec![dir]);
}

#[tokio::test]
async fn login_data_is_written_after_stop_and_before_start() {
    use crate::store::secrets::{Secret, SecretField};
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let record = h.store.get_profile(&id).unwrap().unwrap();
    let mut input = ProfileInput::new(record.name.clone(), record.fingerprint_json.clone());
    input.proxy_id = record.proxy_id;
    input.platform_domain = "shop.test".into();
    input.username = Some("ann".into());
    input.password = SecretField::Set(Secret::new("p@ss"));
    h.store.update_profile(&id, &input).unwrap();
    let login_data = h.paths.profile_dir(&id).join("Default/Login Data");
    h.sup.start(&id).await.unwrap();
    assert!(!login_data.exists(), "первый запуск: файла ещё нет, ядро его не создаёт");
    // Chromium «создал» файл во время сеанса.
    std::fs::create_dir_all(login_data.parent().unwrap()).unwrap();
    crate::engine::login_data::tests::fake_login_data(&login_data);
    h.sup.stop(&id).await.unwrap();
    let rows = |sql: &str| -> i64 {
        rusqlite::Connection::open(&login_data).unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    };
    assert_eq!(
        rows("SELECT COUNT(*) FROM logins WHERE signon_realm = 'https://shop.test/' AND username_value = 'ann'"),
        1
    );
    input.username = None;
    h.store.update_profile(&id, &input).unwrap();
    h.sup.start(&id).await.unwrap();
    assert_eq!(rows("SELECT COUNT(*) FROM logins"), 0, "без логина запись удалена перед запуском");
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn second_start_is_rejected_even_when_concurrent() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let (a, b) = tokio::join!(h.sup.start(&id), h.sup.start(&id));
    let started = [&a, &b].iter().filter(|r| r.is_ok()).count();
    let rejected = [&a, &b].iter().filter(|r| matches!(r, Err(SupervisorError::AlreadyRunning))).count();
    assert_eq!((started, rejected), (1, 1), "{a:?} {b:?}");
    assert!(matches!(h.sup.start(&id).await, Err(SupervisorError::AlreadyRunning)));
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn preconditions_keep_profile_stopped() {
    let h = harness(200).await;
    let no_proxy = add_profile(&h.store, None, "pass");
    assert!(matches!(h.sup.start(&no_proxy).await, Err(SupervisorError::NoFreeProxy)));
    assert_eq!(h.sup.status(&no_proxy), ProfileStatus::Stopped);
    assert!(matches!(h.sup.start("nope00").await, Err(SupervisorError::NotFound)));

    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.engine.installed.store(false, Ordering::SeqCst);
    assert!(matches!(h.sup.start(&id).await, Err(SupervisorError::EngineMissing)));
    h.engine.installed.store(true, Ordering::SeqCst);
    let held = DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
    assert!(matches!(h.sup.start(&id).await, Err(SupervisorError::ProfileLocked)));
    drop(held);
    h.sup.start(&id).await.unwrap();
    assert!(matches!(
        status_events(&h.events, &id).as_slice(),
        [
            ProfileStatus::Starting,
            ProfileStatus::Stopped,
            ProfileStatus::Starting,
            ProfileStatus::Stopped,
            ProfileStatus::Starting,
            ProfileStatus::Running(_)
        ]
    ));
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn dead_or_rejecting_proxy_is_proxy_unavailable() {
    let h = harness(200).await;
    let dead = add_profile(&h.store, Some(closed_port().await), "pass");
    assert!(matches!(h.sup.start(&dead).await, Err(SupervisorError::ProxyUnavailable)));
    let wrong = add_profile(&h.store, Some(h.proxy.port()), "nope");
    assert!(matches!(h.sup.start(&wrong).await, Err(SupervisorError::ProxyUnavailable)));
    assert_eq!(h.sup.status(&wrong), ProfileStatus::Stopped);
    DirLock::acquire(&h.paths.profile_dir(&wrong)).unwrap();
    assert!(pidfile::list(&h.paths.run_dir()).unwrap().is_empty());
}

#[tokio::test]
async fn geo_outage_blocks_only_auto_profiles() {
    let h = harness(500).await;
    let auto = add_profile(&h.store, Some(h.proxy.port()), "pass");
    assert!(matches!(h.sup.start(&auto).await, Err(SupervisorError::GeoUnavailable)));

    let manual = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let record = h.store.get_profile(&manual).unwrap().unwrap();
    let mut fingerprint = Fingerprint::from_json(&record.fingerprint_json).unwrap();
    fingerprint.timezone = Setting::Manual { value: "Asia/Tokyo".into() };
    fingerprint.language = Setting::Manual { value: "ja-JP".into() };
    let mut input = ProfileInput::new(record.name.clone(), fingerprint.to_json());
    input.proxy_id = record.proxy_id;
    h.store.update_profile(&manual, &input).unwrap();
    let info = h.sup.start(&manual).await.unwrap();
    assert_eq!((info.exit_ip, info.timezone.as_str()), (None, "Asia/Tokyo"));
    assert_eq!(h.engine.last_request().resolved.language, "ja-JP");
    h.sup.stop(&manual).await.unwrap();
}

#[tokio::test]
async fn crash_is_detected_and_cleaned_up() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let info = h.sup.start(&id).await.unwrap();
    h.engine.crash(&h.paths.profile_dir(&id), 3);
    let status = wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Crashed { .. })).await;
    assert!(matches!(status, ProfileStatus::Crashed { code: Some(3), signal: None, .. }), "{status:?}");
    assert!(pidfile::list(&h.paths.run_dir()).unwrap().is_empty());
    assert!(h.sup.proxy_users().is_empty(), "резерв прокси снят");
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
    assert!(port_closes((std::net::Ipv4Addr::LOCALHOST, info.relay_port).into()).await, "прослойка закрыта");
    assert!(matches!(h.sup.stop(&id).await, Err(SupervisorError::NotRunning)));
    h.sup.start(&id).await.expect("после падения профиль запускается снова");
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn browser_closed_by_user_is_stopped_not_crashed() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let info = h.sup.start(&id).await.unwrap();
    // Пользователь закрыл окно браузера сам: штатный выход с кодом 0.
    h.engine.crash(&h.paths.profile_dir(&id), 0);
    let status = wait_for(&h.sup, &id, |s| !s.is_active()).await;
    assert_eq!(status, ProfileStatus::Stopped);
    assert!(!status_events(&h.events, &id).iter().any(|s| matches!(s, ProfileStatus::Crashed { .. })));
    assert!(pidfile::list(&h.paths.run_dir()).unwrap().is_empty());
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
    assert!(port_closes((std::net::Ipv4Addr::LOCALHOST, info.relay_port).into()).await, "прослойка закрыта");
    h.sup.start(&id).await.expect("профиль запускается снова");
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn launch_failure_reports_exit_code_and_releases_everything() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    *h.engine.fail_next_launch.lock().unwrap() = Some(ExitInfo { code: Some(76), signal: None });
    let err = h.sup.start(&id).await.unwrap_err();
    assert!(matches!(err, SupervisorError::Launch(EngineError::ExitedEarly(ExitInfo { code: Some(76), .. }))), "{err}");
    assert_eq!(err.code(), "LaunchFailed");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
    assert!(pidfile::list(&h.paths.run_dir()).unwrap().is_empty());
    assert!(h.sup.proxy_users().is_empty(), "резерв прокси снят");

    let record = h.store.get_profile(&id).unwrap().unwrap();
    let mut input = ProfileInput::new(record.name.clone(), record.fingerprint_json.clone());
    input.proxy_id = record.proxy_id;
    input.extra_args = vec!["--proxy-server=direct://".into()];
    h.store.update_profile(&id, &input).unwrap();
    let err = h.sup.start(&id).await.unwrap_err();
    assert_eq!(err.code(), "BadArgs", "{err}");
}

#[tokio::test]
async fn stop_racing_crash_leaves_consistent_state() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&id).await.unwrap();
    h.engine.crash(&h.paths.profile_dir(&id), 9);
    let stopped = h.sup.stop(&id).await;
    assert!(matches!(stopped, Ok(()) | Err(SupervisorError::NotRunning)), "{stopped:?}");
    let status = wait_for(&h.sup, &id, |s| !s.is_active()).await;
    assert!(matches!(status, ProfileStatus::Stopped | ProfileStatus::Crashed { .. }), "{status:?}");
    assert!(pidfile::list(&h.paths.run_dir()).unwrap().is_empty());
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
    h.sup.start(&id).await.unwrap();
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn gateway_outage_pauses_and_recovery_resumes() {
    let h = harness(200).await;
    let (proxy, down) = switchable_proxy().await;
    let target = spawn_echo().await;
    let id = add_profile(&h.store, Some(proxy.port()), "pass");
    let info = h.sup.start(&id).await.unwrap();

    down.store(true, Ordering::SeqCst);
    for _ in 0..UNREACHABLE_AFTER {
        let status = tunnel_status(info.relay_port, target).await;
        assert!(status.starts_with("HTTP/1.1 502"), "{status}");
    }
    let paused = wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Paused(_))).await;
    assert!(matches!(&paused, ProfileStatus::Paused(i) if i.relay_port == info.relay_port), "{paused:?}");
    assert!(h.events.lock().unwrap().contains(&Event::GatewayUnreachable { profile_id: id.clone() }));

    // Прокси снова отвечает: первое удачное соединение возвращает «Работает», проверять отдельно не нужно.
    down.store(false, Ordering::SeqCst);
    let status = tunnel_status(info.relay_port, target).await;
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Running(_))).await;
    let events = status_events(&h.events, &id);
    assert!(
        matches!(
            events.as_slice(),
            [ProfileStatus::Starting, ProfileStatus::Running(_), ProfileStatus::Paused(_), ProfileStatus::Running(_)]
        ),
        "{events:?}"
    );
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn paused_profile_counts_as_active_and_stops() {
    let h = harness(200).await;
    let (proxy, down) = switchable_proxy().await;
    let target = spawn_echo().await;
    let id = add_profile(&h.store, Some(proxy.port()), "pass");
    let info = h.sup.start(&id).await.unwrap();
    down.store(true, Ordering::SeqCst);
    for _ in 0..UNREACHABLE_AFTER {
        tunnel_status(info.relay_port, target).await;
    }
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Paused(_))).await;

    assert_eq!(h.sup.active_count(), 1);
    assert!(matches!(h.sup.start(&id).await, Err(SupervisorError::AlreadyRunning)));
    assert!(matches!(h.sup.lock_idle(&id).await, Err(SupervisorError::ProfileRunning)));
    h.store.set_setting("profiles.max_running", "1").unwrap();
    let other = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&other).await.expect("old max_running does not limit another profile");
    h.sup.stop(&other).await.unwrap();

    h.sup.stop(&id).await.unwrap();
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    assert!(pidfile::list(&h.paths.run_dir()).unwrap().is_empty());
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
}

#[tokio::test]
async fn unrestricted_start_ignores_the_old_limit_and_runs_more_than_twenty_profiles() {
    let h = harness(200).await;
    h.store.set_setting("profiles.max_running", "1").unwrap();
    let ids: Vec<String> = (0..25).map(|_| add_profile(&h.store, Some(h.proxy.port()), "pass")).collect();
    let results = futures_util::future::join_all(ids.iter().map(|id| h.sup.start(id))).await;
    let active = h.sup.active_count();
    h.sup.stop_all().await;
    assert!(results.iter().all(Result::is_ok), "{results:?}");
    assert_eq!(active, 25);
    assert_eq!(h.sup.active_count(), 0);
    assert_eq!(h.store.get_setting("profiles.max_running").unwrap().as_deref(), Some("1"));
}

#[tokio::test]
async fn stop_before_queued_start_cancels_it() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    // PID-файл прошлого запуска: пока уборка не прошла, запуск ждёт её.
    std::fs::create_dir_all(paths.run_dir()).unwrap();
    std::fs::write(paths.run_dir().join("ghost01.pid"), "{").unwrap();
    let stack = geo_stack(200).await;
    let store = Arc::new(memory_store());
    let id = add_profile(&store, Some(stack.proxy.port()), "pass");
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let seen = events.clone();
    let sup = Supervisor::new(
        paths.clone(),
        store,
        Arc::new(FakeEngine::new()),
        Arc::new(stack.geo),
        Arc::new(move |event: Event| seen.lock().unwrap().push(event)),
        fast_timings(),
    );

    let starting = tokio::spawn({
        let sup = sup.clone();
        let id = id.clone();
        async move { sup.start(&id).await }
    });
    wait_for(&sup, &id, |s| *s == ProfileStatus::Starting).await;
    sup.stop(&id).await.expect("«стоп» для ждущего запуска выполняется");
    sup.cleanup_orphans().await.unwrap();

    let result = starting.await.unwrap();
    assert!(matches!(result, Err(SupervisorError::Cancelled)), "{result:?}");
    assert_eq!(result.unwrap_err().code(), "NotRunning");
    assert_eq!(sup.status(&id), ProfileStatus::Stopped);
    assert_eq!(status_events(&events, &id), [ProfileStatus::Starting, ProfileStatus::Stopped]);
    assert!(pidfile::list(&paths.run_dir()).unwrap().is_empty());
    DirLock::acquire(&paths.profile_dir(&id)).unwrap();
}

#[tokio::test]
async fn stop_cancels_start_waiting_in_operation_queue() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    // Очередь профиля занята (как при удалении): запуск встаёт за ней.
    let guard = h.sup.lock_idle(&id).await.unwrap();
    let starting = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.start(&id).await }
    });
    wait_for(&h.sup, &id, |s| *s == ProfileStatus::Starting).await;
    // Задача запуска встаёт в очередь раньше «стопа».
    tokio::time::sleep(Duration::from_millis(50)).await;
    tokio::time::timeout(Duration::from_secs(1), h.sup.stop(&id))
        .await
        .expect("«стоп» для запуска в очереди не ждёт очередь")
        .expect("«стоп» для ждущего запуска выполняется");
    drop(guard);

    let result = starting.await.unwrap();
    assert!(matches!(result, Err(SupervisorError::Cancelled)), "{result:?}");
    assert_eq!(h.engine.launches(), 0, "браузер не запускался");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    assert_eq!(status_events(&h.events, &id), [ProfileStatus::Starting, ProfileStatus::Stopped]);
    assert!(pidfile::list(&h.paths.run_dir()).unwrap().is_empty());
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
}

#[tokio::test]
async fn stop_during_launch_waits_for_it_and_stops_the_browser() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let hold = Arc::new(tokio::sync::Notify::new());
    *h.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
    let starting = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.start(&id).await }
    });
    wait_until("запуск дошёл до движка", || h.engine.launches() == 1).await;
    assert_eq!(h.engine.launches(), 1, "запуск дошёл до движка");

    // Запуск уже идёт: «стоп» не отменяет его, а ждёт конца запуска в очереди.
    let stopping = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.stop(&id).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!stopping.is_finished(), "«стоп» ждёт идущий запуск");
    assert_eq!(h.sup.status(&id), ProfileStatus::Starting);
    hold.notify_one();

    // Ожидания ограничены: регрессия (например, «стоп» и запуск ждут друг друга) роняет тест, а не вешает его.
    let info = tokio::time::timeout(Duration::from_secs(5), starting)
        .await
        .expect("запуск не завис")
        .unwrap()
        .expect("начатый запуск доходит до конца");
    tokio::time::timeout(Duration::from_secs(5), stopping)
        .await
        .expect("«стоп» не завис")
        .unwrap()
        .expect("«стоп» останавливает запущенный браузер");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    let events = status_events(&h.events, &id);
    assert!(
        matches!(events.as_slice(), [ProfileStatus::Starting, ProfileStatus::Running(_), ProfileStatus::Stopped]),
        "{events:?}"
    );
    assert!(port_closes((std::net::Ipv4Addr::LOCALHOST, info.relay_port).into()).await, "прослойка закрыта");
    let mut cdp_closed = false;
    for _ in 0..50 {
        if CdpClient::connect(&info.cdp_ws).await.is_err() {
            cdp_closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(cdp_closed, "браузер остановлен: CDP закрылся");
    assert!(pidfile::list(&h.paths.run_dir()).unwrap().is_empty());
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
}

#[tokio::test]
async fn orphan_cleanup_continues_after_failure() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let run = paths.run_dir();
    // «PID-файл»-папку нельзя удалить как файл: уборка этого профиля падает.
    std::fs::create_dir_all(run.join("bad01.pid")).unwrap();
    std::fs::write(run.join("good01.pid"), "{").unwrap();
    let store = Arc::new(memory_store());
    // Профиль без прокси: после уборки запуск отвечает `NoFreeProxy` без сети.
    let id = add_profile(&store, None, "pass");
    let sup = Supervisor::new(
        paths.clone(),
        store,
        Arc::new(FakeEngine::new()),
        Arc::new(GeoClient::new()),
        Arc::new(|_: Event| {}),
        fast_timings(),
    );
    let cleaned = sup.cleanup_orphans().await.unwrap();
    assert_eq!(cleaned, ["good01"]);
    assert!(!run.join("good01.pid").exists());
    assert!(run.join("bad01.pid").exists(), "сбойный остаётся до следующего старта");
    assert!(matches!(sup.start(&id).await, Err(SupervisorError::NoFreeProxy)), "после уборки запуски не ждут");
}

#[tokio::test]
async fn start_waits_for_cleanup_at_most_ready_wait() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.run_dir()).unwrap();
    std::fs::write(paths.run_dir().join("ghost01.pid"), "{").unwrap();
    let timings = Timings { ready_wait: Duration::from_millis(300), ..fast_timings() };
    let store = Arc::new(memory_store());
    // Профиль без прокси: дождавшись уборки, запуск отвечает `NoFreeProxy` без сети.
    let id = add_profile(&store, None, "pass");
    let sup = Supervisor::new(
        paths,
        store,
        Arc::new(FakeEngine::new()),
        Arc::new(GeoClient::new()),
        Arc::new(|_: Event| {}),
        timings,
    );
    // Уборку никто не запускает: запуск и удаление ждут её не дольше `ready_wait`.
    let began = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(3), sup.start(&id))
        .await
        .expect("запуск не ждёт уборку бесконечно");
    assert!(matches!(result, Err(SupervisorError::NoFreeProxy)), "{result:?}");
    assert!(began.elapsed() >= Duration::from_millis(300), "сначала запуск ждал уборку");

    let began = std::time::Instant::now();
    let guard = tokio::time::timeout(Duration::from_secs(3), sup.lock_idle(&id))
        .await
        .expect("удаление не ждёт уборку бесконечно");
    assert!(guard.is_ok());
    assert!(began.elapsed() >= Duration::from_millis(300), "удаление тоже ждало уборку");
}

#[tokio::test]
async fn failed_stop_keeps_pid_file() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&id).await.unwrap();
    h.engine.fail_next_stop.store(true, Ordering::SeqCst);
    let err = h.sup.stop(&id).await.unwrap_err();
    assert_eq!(err.code(), "StopFailed", "{err}");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    let left: Vec<String> = pidfile::list(&h.paths.run_dir()).unwrap().into_iter().map(|(id, _)| id).collect();
    assert_eq!(left, std::slice::from_ref(&id), "PID-файл остаётся: уцелевшие процессы найдёт уборка при следующем старте");
}

#[tokio::test]
async fn forget_drops_operation_queue() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&id).await.unwrap();
    h.sup.stop(&id).await.unwrap();
    assert!(h.sup.inner.ops.lock().unwrap().contains_key(&id));
    h.sup.forget(&id);
    assert!(!h.sup.inner.ops.lock().unwrap().contains_key(&id), "очередь удалённого профиля не копится");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
}

#[tokio::test]
async fn stop_all_stops_everything() {
    let h = harness(200).await;
    let a = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let b = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&a).await.unwrap();
    h.sup.start(&b).await.unwrap();
    assert_eq!(h.sup.active_count(), 2);
    h.sup.stop_all().await;
    assert_eq!(h.sup.active_count(), 0);
    assert_eq!((h.sup.status(&a), h.sup.status(&b)), (ProfileStatus::Stopped, ProfileStatus::Stopped));
}

#[test]
fn stop_fits_exit_timeout() {
    // Остановка на выходе: последний снимок, SIGTERM и до `stop_grace` ожидания, затем уборка помощников
    // Chromium — до `settle` (у движка те же 3 с, что в `Timings`). Профили останавливаются параллельно.
    let t = Timings::default();
    assert!(FINAL_SNAPSHOT_TIMEOUT + t.stop_grace + t.settle <= STOP_ALL_TIMEOUT, "остановка не укладывается в выход");
}

#[tokio::test]
async fn stop_all_refuses_new_starts() {
    let h = harness(200).await;
    let a = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let b = add_profile(&h.store, Some(h.proxy.port()), "pass");
    // Запуск A держится в движке: «Остановить все» ждёт его в очереди профиля.
    let hold = Arc::new(tokio::sync::Notify::new());
    *h.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
    let starting = tokio::spawn({
        let sup = h.sup.clone();
        let a = a.clone();
        async move { sup.start(&a).await }
    });
    wait_until("запуск A дошёл до движка", || h.engine.launches() == 1).await;
    let stopping = tokio::spawn({
        let sup = h.sup.clone();
        async move { sup.stop_all().await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!stopping.is_finished(), "«Остановить все» ждёт идущий запуск");

    // Запуск, пришедший после начала выхода (массовый запуск, Local API), не выполняется.
    let result = h.sup.start(&b).await;
    assert!(matches!(result, Err(SupervisorError::Cancelled)), "{result:?}");
    assert_eq!(h.engine.launches(), 1, "браузер B не запускался");
    assert!(status_events(&h.events, &b).is_empty(), "отклонённый запуск не мелькает «запускается»");

    hold.notify_one();
    starting.await.unwrap().expect("начатый запуск доходит до конца");
    stopping.await.unwrap();
    assert_eq!(h.sup.active_count(), 0, "после «Остановить все» активных профилей нет");
    assert_eq!((h.sup.status(&a), h.sup.status(&b)), (ProfileStatus::Stopped, ProfileStatus::Stopped));
    assert!(matches!(h.sup.start(&b).await, Err(SupervisorError::Cancelled)), "выход не отменяется");
}

#[tokio::test]
async fn idle_guard_refuses_active_profile_and_dropped_start_completes() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    drop(h.sup.lock_idle(&id).await.unwrap());
    h.sup.start(&id).await.unwrap();
    assert!(matches!(h.sup.lock_idle(&id).await, Err(SupervisorError::ProfileRunning)));
    h.sup.stop(&id).await.unwrap();
    let guard = h.sup.lock_idle(&id).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), h.sup.start(&id)).await.is_err(),
        "пока держится очередь операций, запуск ждёт"
    );
    drop(guard);
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Running(_))).await;
    h.sup.stop(&id).await.unwrap();
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
}

/// Папку профиля держит другой процесс Ombra (`ombra flow` рядом с окном): охрана простоя отказывает, а взятая охрана
/// держит папку от других процессов до конца операции.
#[tokio::test]
async fn idle_guard_refuses_a_profile_held_by_another_process() {
    let h = harness(200).await;
    let a = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let b = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let dir = h.paths.profile_dir(&b);
    let foreign = DirLock::acquire(&dir).unwrap();
    assert!(matches!(h.sup.lock_idle(&b).await, Err(SupervisorError::ProfileLocked)));
    let refused = h.sup.lock_idle_many(&[a.clone(), b.clone()]).await.map(|guards| guards.len());
    assert!(matches!(refused, Err(SupervisorError::ProfileLocked)), "{refused:?}");
    drop(foreign);
    let guard = h.sup.lock_idle(&b).await.unwrap();
    assert!(matches!(DirLock::acquire(&dir), Err(LockError::Busy)), "охрана держит папку от других процессов");
    drop(guard);
    drop(DirLock::acquire(&dir).unwrap());
    assert!(!h.paths.profile_dir(&a).exists(), "папки нет — охрана её не создаёт");
    drop(h.sup.lock_idle_many(&[a.clone(), b.clone()]).await.unwrap());
    assert!(!h.paths.profile_dir(&a).exists());
}

#[tokio::test]
async fn batch_idle_locks_are_all_or_nothing() {
    let h = harness(200).await;
    let a = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let b = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&b).await.unwrap();
    let refused = h.sup.lock_idle_many(&[a.clone(), b.clone()]).await.map(|guards| guards.len());
    assert!(matches!(refused, Err(SupervisorError::ProfileRunning)), "{refused:?}");
    // Отказ не держит ни одного замка: сосед по пачке запускается без ожидания.
    tokio::time::timeout(Duration::from_secs(2), h.sup.start(&a)).await.expect("замок A не держится").unwrap();
    h.sup.stop(&a).await.unwrap();
    h.sup.stop(&b).await.unwrap();

    let guards = h.sup.lock_idle_many(&[b.clone(), a.clone(), b.clone()]).await.unwrap();
    assert_eq!(guards.len(), 2, "повтор id — один замок");
    for id in [&a, &b] {
        assert!(
            tokio::time::timeout(Duration::from_millis(200), h.sup.start(id)).await.is_err(),
            "пока держатся замки пачки, запуск ждёт"
        );
    }
    drop(guards);
    for id in [&a, &b] {
        wait_for(&h.sup, id, |s| matches!(s, ProfileStatus::Running(_))).await;
        h.sup.stop(id).await.unwrap();
    }
}

#[tokio::test]
async fn batch_lock_rechecks_after_waiting_and_releases_everything() {
    let h = harness(200).await;
    let x = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let y = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let (low, high) = if x < y { (x, y) } else { (y, x) };
    // Очередь младшего id занята (как одиночным удалением): пачка проходит первую проверку и ждёт.
    let held = h.sup.lock_idle(&low).await.unwrap();
    let batch = tokio::spawn({
        let sup = h.sup.clone();
        let ids = [low.clone(), high.clone()];
        async move { sup.lock_idle_many(&ids).await.map(|guards| guards.len()) }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Тем временем старший запускается: его очередь свободна.
    h.sup.start(&high).await.unwrap();
    drop(held);
    let result = tokio::time::timeout(Duration::from_secs(2), batch).await.expect("пачка дождалась очередей").unwrap();
    assert!(matches!(result, Err(SupervisorError::ProfileRunning)), "{result:?}");
    let again = tokio::time::timeout(Duration::from_secs(1), h.sup.lock_idle(&low)).await;
    drop(again.expect("замок младшего, взятый пачкой, отпущен").unwrap());
    h.sup.stop(&high).await.unwrap();
}

#[tokio::test]
async fn batch_locks_in_any_order_do_not_deadlock() {
    let h = harness(200).await;
    let x = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let y = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let (low, high) = if x < y { (x, y) } else { (y, x) };
    let held = h.sup.lock_idle(&low).await.unwrap();
    let spawn_batch = |ids: [String; 2]| {
        let sup = h.sup.clone();
        tokio::spawn(async move { sup.lock_idle_many(&ids).await.map(drop) })
    };
    let forward = spawn_batch([low.clone(), high.clone()]);
    tokio::time::sleep(Duration::from_millis(20)).await;
    // Без порядка по возрастанию эта пачка заняла бы старший id и ждала младший, а первая — наоборот.
    let backward = spawn_batch([high.clone(), low.clone()]);
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(held);
    let (forward, backward) = tokio::time::timeout(Duration::from_secs(2), async { (forward.await, backward.await) })
        .await
        .expect("пачки не ждут друг друга вечно");
    assert!(forward.unwrap().is_ok() && backward.unwrap().is_ok());
}

#[tokio::test]
async fn dropped_stop_still_completes() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&id).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_millis(1), h.sup.stop(&id)).await;
    wait_for(&h.sup, &id, |s| *s == ProfileStatus::Stopped).await;
    assert!(pidfile::list(&h.paths.run_dir()).unwrap().is_empty());
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
    h.sup.start(&id).await.unwrap();
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn orphans_from_pid_files_are_cleaned_before_starts() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let profile = paths.profile_dir("orph01");
    std::fs::create_dir_all(&profile).unwrap();
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "while :; do sleep 1; done", "orphan", &format!("--user-data-dir={}", profile.display())])
        .spawn()
        .unwrap();
    let record = PidRecord { pid: child.id(), binary: "/X/Chromium".into(), profile_dir: profile.clone(), started_at: 1 };
    pidfile::write(&paths.run_dir(), "orph01", &record).unwrap();
    std::fs::write(paths.run_dir().join("ghost01.pid"), "{").unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let store = Arc::new(memory_store());
    // Профиль без прокси: после уборки запуск отвечает `NoFreeProxy` без сети.
    let id = add_profile(&store, None, "pass");
    let sup = Supervisor::new(
        paths.clone(),
        store,
        Arc::new(FakeEngine::new()),
        Arc::new(GeoClient::new()),
        Arc::new(|_: Event| {}),
        fast_timings(),
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), sup.start(&id)).await.is_err(),
        "до уборки запуск ждёт"
    );
    let cleaned = sup.cleanup_orphans().await.unwrap();
    let _ = child.wait();
    assert_eq!(cleaned, ["ghost01", "orph01"]);
    assert!(process::find_profile_processes(&profile).await.unwrap().is_empty());
    assert!(pidfile::list(&paths.run_dir()).unwrap().is_empty());
    wait_for(&sup, &id, |s| !s.is_active()).await;
    assert!(matches!(sup.start(&id).await, Err(SupervisorError::NoFreeProxy)));
}

#[tokio::test]
async fn orphan_cleanup_leaves_a_browser_whose_profile_another_process_holds() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let profile = paths.profile_dir("held01");
    std::fs::create_dir_all(&profile).unwrap();
    // Другой процесс Ombra (окно рядом с `ombra flow`) держит папку профиля, его браузер работает.
    let held = DirLock::acquire(&profile).unwrap();
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "while :; do sleep 1; done", "held", &format!("--user-data-dir={}", profile.display())])
        .spawn()
        .unwrap();
    let record = PidRecord { pid: child.id(), binary: "/X/Chromium".into(), profile_dir: profile.clone(), started_at: 1 };
    pidfile::write(&paths.run_dir(), "held01", &record).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let sup = Supervisor::new(
        paths.clone(),
        Arc::new(memory_store()),
        Arc::new(FakeEngine::new()),
        Arc::new(GeoClient::new()),
        Arc::new(|_: Event| {}),
        fast_timings(),
    );
    let cleaned = sup.cleanup_orphans().await.unwrap();
    assert!(cleaned.is_empty(), "{cleaned:?}");
    assert!(!process::find_profile_processes(&profile).await.unwrap().is_empty(), "чужой браузер жив");
    assert!(pidfile::path(&paths.run_dir(), "held01").exists(), "pid-файл остаётся владельцу");
    // Владелец пропал (упал) — браузер сирота.
    drop(held);
    let cleaned = sup.cleanup_orphans().await.unwrap();
    let _ = child.wait();
    assert_eq!(cleaned, ["held01"]);
    assert!(process::find_profile_processes(&profile).await.unwrap().is_empty());
}

#[tokio::test]
async fn foreign_key_gives_secrets_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let id = {
        let store = Store::open(&paths.db(), SecretBox::from_key(&[7; 32])).unwrap();
        add_profile(&store, Some(1080), "pass")
    };
    let store = Arc::new(Store::open(&paths.db(), SecretBox::from_key(&[8; 32])).unwrap());
    let sup = Supervisor::new(
        paths.clone(),
        store,
        Arc::new(FakeEngine::new()),
        Arc::new(GeoClient::new()),
        Arc::new(|_: Event| {}),
        fast_timings(),
    );
    let err = sup.start(&id).await.unwrap_err();
    assert_eq!(err.code(), "SecretsUnreadable", "{err}");
    assert_eq!(sup.status(&id), ProfileStatus::Stopped);
}

#[tokio::test]
async fn pinned_proxy_is_used_and_check_is_recorded() {
    let h = harness(200).await;
    let pinned = add_proxy(&h.store, h.proxy.port(), "pass");
    let spare = add_proxy(&h.store, h.proxy.port(), "pass");
    let id = add_profile_with(&h.store, Some(pinned));
    let info = h.sup.start(&id).await.unwrap();
    assert_eq!(info.proxy_id, pinned);
    let check = h.store.get_proxy(pinned).unwrap().unwrap().check.expect("проверка при запуске записана");
    assert!(check.ok);
    assert_eq!((check.exit_ip.as_deref(), check.country.as_deref()), (Some("203.0.113.24"), Some("DE")));
    assert!(h.store.get_proxy(spare).unwrap().unwrap().check.is_none(), "запасной не трогали");
    let profile = h.store.get_profile(&id).unwrap().unwrap();
    assert_eq!((profile.proxy_id, profile.gateway_replaced_at), (Some(pinned), None));
    assert_eq!(h.sup.proxy_users(), HashMap::from([(pinned, id.clone())]));
    assert!(!h.events.lock().unwrap().iter().any(|e| matches!(e, Event::GatewayReplaced { .. })));
    h.sup.stop(&id).await.unwrap();
    assert!(h.sup.proxy_users().is_empty(), "остановка освобождает прокси");
}

#[tokio::test]
async fn unpinned_profile_gets_random_proxy_without_mark() {
    let h = harness(200).await;
    let only = add_proxy(&h.store, h.proxy.port(), "pass");
    let id = add_profile_with(&h.store, None);
    let info = h.sup.start(&id).await.unwrap();
    assert_eq!(info.proxy_id, only);
    let profile = h.store.get_profile(&id).unwrap().unwrap();
    assert_eq!(profile.proxy_id, Some(only), "выбранный прокси закрепляется");
    assert_eq!(profile.gateway_replaced_at, None, "первый выбор — не замена");
    assert!(!h.events.lock().unwrap().iter().any(|e| matches!(e, Event::GatewayReplaced { .. })));
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn busy_pinned_proxy_is_replaced_and_marked() {
    let h = harness(200).await;
    let shared = add_proxy(&h.store, h.proxy.port(), "pass");
    let spare = add_proxy(&h.store, h.proxy.port(), "pass");
    let first = add_profile_with(&h.store, Some(shared));
    let second = add_profile_with(&h.store, Some(shared));
    h.sup.start(&first).await.unwrap();
    let info = h.sup.start(&second).await.unwrap();
    assert_eq!(info.proxy_id, spare, "закреплённый занят — берётся свободный");
    let profile = h.store.get_profile(&second).unwrap().unwrap();
    assert_eq!(profile.proxy_id, Some(spare), "новый прокси закреплён");
    assert!(profile.gateway_replaced_at.is_some(), "пометка «шлюз заменён при запуске»");
    assert!(
        h.events.lock().unwrap().contains(&Event::GatewayReplaced { profile_id: second.clone(), name: "Магазин".into() })
    );
    assert_eq!(h.sup.proxy_users(), HashMap::from([(shared, first.clone()), (spare, second.clone())]));

    // Следующий запуск с теперь закреплённым и свободным прокси — без замены, пометка снимается.
    h.sup.stop(&second).await.unwrap();
    h.sup.start(&second).await.unwrap();
    assert_eq!(h.store.get_profile(&second).unwrap().unwrap().gateway_replaced_at, None);
    h.sup.stop_all().await;
}

#[tokio::test]
async fn dead_pinned_proxy_is_replaced() {
    let h = harness(200).await;
    let dead = add_proxy(&h.store, closed_port().await, "pass");
    let alive = add_proxy(&h.store, h.proxy.port(), "pass");
    let id = add_profile_with(&h.store, Some(dead));
    let info = h.sup.start(&id).await.unwrap();
    assert_eq!(info.proxy_id, alive);
    let check = h.store.get_proxy(dead).unwrap().unwrap().check.expect("неудача записана");
    assert!(!check.ok);
    let profile = h.store.get_profile(&id).unwrap().unwrap();
    assert_eq!(profile.proxy_id, Some(alive));
    assert!(profile.gateway_replaced_at.is_some());
    assert_eq!(h.sup.proxy_users(), HashMap::from([(alive, id.clone())]), "резерв недоступного снят");
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn three_dead_proxies_give_proxy_unavailable() {
    let h = harness(200).await;
    let mut dead = Vec::new();
    for _ in 0..4 {
        dead.push(add_proxy(&h.store, closed_port().await, "pass"));
    }
    let id = add_profile_with(&h.store, Some(dead[0]));
    let err = h.sup.start(&id).await.unwrap_err();
    assert!(matches!(err, SupervisorError::ProxyUnavailable), "{err}");
    let checked: Vec<i64> =
        h.store.list_proxies().unwrap().into_iter().filter(|p| p.check.is_some()).map(|p| p.id).collect();
    assert_eq!(checked.len(), gateway::MAX_GATEWAY_ATTEMPTS, "проверено ровно три: {checked:?}");
    assert!(checked.contains(&dead[0]), "закреплённый пробуется первым");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    assert!(h.sup.proxy_users().is_empty());
    assert_eq!(
        h.store.get_profile(&id).unwrap().unwrap().proxy_id,
        Some(dead[0]),
        "без удачного запуска закрепление не меняется"
    );
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
}

#[tokio::test]
async fn no_enabled_proxy_gives_no_free_proxy() {
    let h = harness(200).await;
    let off = add_proxy(&h.store, h.proxy.port(), "pass");
    h.store.set_proxy_enabled(off, false).unwrap();
    for pinned in [None, Some(off)] {
        let id = add_profile_with(&h.store, pinned);
        let err = h.sup.start(&id).await.unwrap_err();
        assert!(matches!(err, SupervisorError::NoFreeProxy), "{err}");
        assert_eq!(err.code(), "NoFreeProxy");
        assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    }
    assert!(h.store.get_proxy(off).unwrap().unwrap().check.is_none(), "выключенный не проверяется");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_starts_never_share_a_proxy() {
    let h = harness(200).await;
    let shared = add_proxy(&h.store, h.proxy.port(), "pass");
    let spare = add_proxy(&h.store, h.proxy.port(), "pass");
    let a = add_profile_with(&h.store, Some(shared));
    let b = add_profile_with(&h.store, Some(shared));
    let (ia, ib) = tokio::join!(h.sup.start(&a), h.sup.start(&b));
    let (ia, ib) = (ia.unwrap(), ib.unwrap());
    let mut used = [ia.proxy_id, ib.proxy_id];
    used.sort();
    assert_eq!(used, [shared, spare], "два запуска не взяли один прокси");
    let replaced = [&a, &b]
        .iter()
        .filter(|id| h.store.get_profile(id).unwrap().unwrap().gateway_replaced_at.is_some())
        .count();
    assert_eq!(replaced, 1);
    h.sup.stop_all().await;
}

#[tokio::test]
async fn busy_proxy_cannot_be_deleted() {
    let h = harness(200).await;
    let proxy = add_proxy(&h.store, h.proxy.port(), "pass");
    let id = add_profile_with(&h.store, Some(proxy));
    h.sup.start(&id).await.unwrap();
    let refused = h.sup.while_proxies_idle(&[proxy], || h.store.delete_proxy(proxy));
    assert!(matches!(refused, Err(SupervisorError::ProxyInUse)), "{refused:?}");
    assert_eq!(SupervisorError::ProxyInUse.code(), "ProxyInUse");
    assert!(h.store.get_proxy(proxy).unwrap().is_some());

    h.sup.stop(&id).await.unwrap();
    h.sup.while_proxies_idle(&[proxy], || h.store.delete_proxy(proxy)).unwrap().unwrap();
    assert!(h.store.get_proxy(proxy).unwrap().is_none());
    assert_eq!(h.store.get_profile(&id).unwrap().unwrap().proxy_id, None, "закрепление снято");
}

/// Прокси, чью блокировку держит другой процесс Ombra (окно рядом с `ombra flow`), занят: закреплённый за профилем
/// не выдаётся и не удаляется, случайный выбор его пропускает; свой резерв другие процессы видят так же.
#[tokio::test]
async fn proxy_held_by_another_process_is_neither_given_nor_deleted() {
    let h = harness(200).await;
    let proxy = add_proxy(&h.store, h.proxy.port(), "pass");
    let id = add_profile_with(&h.store, Some(proxy));
    let foreign = DirLock::acquire(&h.paths.proxy_lock_dir(proxy)).unwrap();
    let err = h.sup.start(&id).await.unwrap_err();
    assert!(matches!(err, SupervisorError::NoFreeProxy), "{err}");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    let refused = h.sup.while_proxies_idle(&[proxy], || h.store.delete_proxy(proxy));
    assert!(matches!(refused, Err(SupervisorError::ProxyInUse)), "{refused:?}");
    assert!(h.store.get_proxy(proxy).unwrap().is_some());

    drop(foreign);
    assert_eq!(h.sup.start(&id).await.unwrap().proxy_id, proxy);
    assert!(matches!(DirLock::acquire(&h.paths.proxy_lock_dir(proxy)), Err(LockError::Busy)), "резерв виден другим процессам");
    h.sup.stop(&id).await.unwrap();
    drop(DirLock::acquire(&h.paths.proxy_lock_dir(proxy)).expect("остановка снимает блокировку прокси"));

    let spare = add_proxy(&h.store, h.proxy.port(), "pass");
    let free = add_profile_with(&h.store, None);
    let _foreign = DirLock::acquire(&h.paths.proxy_lock_dir(proxy)).unwrap();
    assert_eq!(h.sup.start(&free).await.unwrap().proxy_id, spare, "случайный выбор пропускает чужой прокси");
    h.sup.stop(&free).await.unwrap();
}

#[tokio::test]
async fn stored_cookies_are_set_before_start_tabs() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.store.merge_cookies(&id, &[cookie("sid", "1")]).unwrap();
    h.sup.start(&id).await.unwrap();
    let cdp = h.engine.cdp(&h.paths.profile_dir(&id));
    let calls = cdp.calls();
    let set = calls.iter().position(|c| c == "Storage.setCookies").expect("cookies поставлены");
    let tab = calls.iter().position(|c| c == "Target.createTarget").expect("стартовая вкладка открыта");
    assert!(set < tab, "cookies — до вкладок: {calls:?}");
    assert_eq!(cdp.opened(), ["https://example.com/"]);
    assert!(cdp.browser_cookies().iter().any(|c| c.name == "sid" && c.value == "1"));
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn clean_start_suppresses_profile_urls_without_changing_saved_settings() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.store.merge_cookies(&id, &[cookie("sid", "1")]).unwrap();
    let options = StartOptions { restore_tabs: Some(false), start_urls: Some(vec![]), ..Default::default() };
    h.sup.start_with(&id, options).await.unwrap();
    let cdp = h.engine.cdp(&h.paths.profile_dir(&id));
    assert!(cdp.opened().is_empty(), "чистый запуск не загружает адреса профиля");
    assert!(!h.engine.last_request().restore_tabs);
    assert!(cdp.browser_cookies().iter().any(|c| c.name == "sid" && c.value == "1"));
    h.sup.stop(&id).await.unwrap();
    let saved = h.store.get_profile(&id).unwrap().unwrap();
    assert!(saved.restore_tabs);
    assert_eq!(saved.start_urls, ["https://example.com/"]);
    h.sup.start(&id).await.unwrap();
    assert!(h.engine.last_request().restore_tabs);
    assert_eq!(h.engine.cdp(&h.paths.profile_dir(&id)).opened(), ["https://example.com/"]);
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn snapshot_writes_only_changes() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.store.merge_cookies(&id, &[cookie("sid", "1")]).unwrap();
    h.sup.start(&id).await.unwrap();
    let cdp = h.engine.cdp(&h.paths.profile_dir(&id));
    // Вместо хэша импорт оставил свою пометку, поэтому первый снимок записывает набор.
    wait_until("первый снимок", || snapshot_written(&h.store, &id, &cdp)).await;
    let blob = cookie_blob(&h.store, &id);
    let taken = snapshots(&cdp);
    wait_until("ещё три снимка", || snapshots(&cdp) >= taken + 3).await;
    assert_eq!(cookie_blob(&h.store, &id), blob, "набор не менялся — база не перезаписывается");
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn browser_changes_reach_store() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.store.merge_cookies(&id, &[cookie("keep", "1"), cookie("gone", "1")]).unwrap();
    h.sup.start(&id).await.unwrap();
    let cdp = h.engine.cdp(&h.paths.profile_dir(&id));
    wait_until("первый снимок", || snapshot_written(&h.store, &id, &cdp)).await;
    // Сайт поменял значение, поставил новую cookie и удалил одну (выход из аккаунта).
    cdp.set_browser_cookies(vec![cookie("keep", "2"), cookie("new", "1")]);
    wait_until("изменения в базе", || stored(&h.store, &id) == pairs(&[("keep", "2"), ("new", "1")])).await;
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn final_snapshot_on_stop() {
    // Снимков по расписанию нет: всё, что попало в базу, — из последнего снимка перед закрытием.
    let timings = Timings { cookie_interval: Duration::from_secs(3600), ..fast_timings() };
    let h = harness_with(200, timings).await;
    let a = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let b = add_profile(&h.store, Some(h.proxy.port()), "pass");

    h.sup.start(&a).await.unwrap();
    h.engine.cdp(&h.paths.profile_dir(&a)).set_browser_cookies(vec![cookie("late", "a")]);
    h.sup.stop(&a).await.unwrap();
    assert_eq!(stored(&h.store, &a), pairs(&[("late", "a")]));

    // То же при выходе из приложения: «Остановить все».
    h.sup.start(&b).await.unwrap();
    h.engine.cdp(&h.paths.profile_dir(&b)).set_browser_cookies(vec![cookie("exit", "b")]);
    h.sup.stop_all().await;
    assert_eq!(stored(&h.store, &b), pairs(&[("exit", "b")]));
}

#[tokio::test]
async fn windowed_browser_without_pages_keeps_running() {
    // Последняя вкладка закрыта, а окно ещё открыто (вопрос об отмене загрузки): профиль работает, пока браузер не
    // выйдет сам (патч движка 245). Без страниц cookies не запрашиваются — ошибок в журнале нет.
    let timings = Timings { cookie_interval: Duration::from_millis(300), ..fast_timings() };
    let h = harness_with(200, timings).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&id).await.unwrap();
    let cdp = h.engine.cdp(&h.paths.profile_dir(&id));
    cdp.set_browser_cookies(vec![cookie("late", "1")]);
    wait_until("снимок с окном", || stored(&h.store, &id) == pairs(&[("late", "1")])).await;

    cdp.close_windows();
    let checks = page_checks(&cdp);
    wait_until("проверка без окон", || page_checks(&cdp) > checks).await;
    let taken = snapshots(&cdp);
    wait_until("ещё три проверки", || page_checks(&cdp) >= checks + 4).await;
    assert!(matches!(h.sup.status(&id), ProfileStatus::Running(_)), "{:?}", h.sup.status(&id));
    assert_eq!(snapshots(&cdp), taken, "без страниц cookies не запрашиваются");
    assert_eq!(stored(&h.store, &id), pairs(&[("late", "1")]), "в базе — последний снимок с окном");
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn headless_profile_without_pages_keeps_running() {
    // Без окна закрывать нечего: скрипт закрыл все вкладки — профиль работает, cookies снимаются.
    // Без окна — и по `headless`, и по `--headless` в аргументах запуска.
    let by_args = StartOptions { extra_args: Some(vec!["--headless=new".into()]), ..StartOptions::default() };
    for options in [StartOptions { headless: true, ..StartOptions::default() }, by_args] {
        headless_without_pages(options).await;
    }
}

async fn headless_without_pages(options: StartOptions) {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start_with(&id, options).await.unwrap();
    let cdp = h.engine.cdp(&h.paths.profile_dir(&id));
    let mut script = CdpClient::connect(&cdp.ws_url()).await.unwrap();
    let targets = script.send("Target.getTargets", serde_json::json!({})).await.unwrap();
    for target in targets["targetInfos"].as_array().unwrap() {
        script.send("Target.closeTarget", serde_json::json!({ "targetId": target["targetId"] })).await.unwrap();
    }
    assert!(cdp.pages().is_empty());
    cdp.set_browser_cookies(vec![cookie("late", "1")]);
    wait_until("снимок без страниц", || stored(&h.store, &id) == pairs(&[("late", "1")])).await;
    let taken = snapshots(&cdp);
    wait_until("ещё три снимка", || snapshots(&cdp) >= taken + 3).await;
    assert!(matches!(h.sup.status(&id), ProfileStatus::Running(_)), "{:?}", h.sup.status(&id));
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn stop_after_windows_closed_skips_final_snapshot() {
    // Окна закрыты, «Стоп» пришёл раньше проверки: снимать нечего, в базе — прошлый снимок, ошибок в журнале нет.
    let timings = Timings { cookie_interval: Duration::from_secs(3600), ..fast_timings() };
    let h = harness_with(200, timings).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&id).await.unwrap();
    let cdp = h.engine.cdp(&h.paths.profile_dir(&id));
    cdp.close_windows();
    h.sup.stop(&id).await.unwrap();
    assert_eq!(snapshots(&cdp), 0);
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
}

#[tokio::test]
async fn cdp_failure_gives_cookies_failed_and_stops_browser() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.engine.cdp_down.store(true, Ordering::SeqCst);
    let err = h.sup.start(&id).await.unwrap_err();
    assert!(matches!(err, SupervisorError::Cookies(_)), "{err}");
    assert_eq!(err.code(), "CookiesFailed");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    assert!(!h.engine.is_running(&h.paths.profile_dir(&id)), "браузер без своих cookies остановлен");
    assert!(pidfile::list(&h.paths.run_dir()).unwrap().is_empty());
    assert!(h.sup.proxy_users().is_empty());
    assert!(!h.sup.inner.lock_state().launching.contains_key(&id), "запуск закончен — «стоп» не будет ждать его");
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();

    h.engine.cdp_down.store(false, Ordering::SeqCst);
    h.sup.start(&id).await.expect("с доступным CDP профиль запускается");
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn rejected_cookie_does_not_fail_launch() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.engine.reject_cookie("bad");
    h.store.merge_cookies(&id, &[cookie("good", "1"), cookie("bad", "1")]).unwrap();
    h.sup.start(&id).await.expect("одна отвергнутая cookie не мешает запуску");
    let names: Vec<String> =
        h.engine.cdp(&h.paths.profile_dir(&id)).browser_cookies().into_iter().map(|c| c.name).collect();
    assert_eq!(names, ["good"]);
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn crash_keeps_last_snapshot() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.store.merge_cookies(&id, &[cookie("a", "1")]).unwrap();
    h.sup.start(&id).await.unwrap();
    let cdp = h.engine.cdp(&h.paths.profile_dir(&id));
    cdp.set_browser_cookies(vec![cookie("a", "1"), cookie("b", "1")]);
    wait_until("снимок с b", || stored(&h.store, &id).len() == 2).await;
    h.engine.crash(&h.paths.profile_dir(&id), 9);
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Crashed { .. })).await;
    assert_eq!(stored(&h.store, &id), pairs(&[("a", "1"), ("b", "1")]), "в базе — последний снимок");
}

#[tokio::test]
async fn import_reaches_running_browser_and_survives_snapshot() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&id).await.unwrap();
    let cdp = h.engine.cdp(&h.paths.profile_dir(&id));
    let (count, sent) = h.sup.import_cookies(&id, vec![cookie("imp", "1")]).await.unwrap();
    assert_eq!((count, sent), (MergeCount { added: 1, replaced: 0 }, true));
    assert!(cdp.browser_cookies().iter().any(|c| c.name == "imp"), "импорт сразу в работающем браузере");
    let taken = snapshots(&cdp);
    wait_until("три снимка после импорта", || snapshots(&cdp) >= taken + 3).await;
    assert!(stored(&h.store, &id).contains(&("imp".to_string(), "1".to_string())), "снимок не стёр импорт");
    h.sup.stop(&id).await.unwrap();
    assert!(stored(&h.store, &id).contains(&("imp".to_string(), "1".to_string())));
}

#[tokio::test]
async fn snapshot_in_flight_does_not_overwrite_import() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let cdp = h.engine.cdp(&h.paths.profile_dir(&id));
    // В папке браузера — cookie сайта, которой нет в базе: снимок увидит набор, отличный от записанного.
    cdp.set_browser_cookies(vec![cookie("site", "1")]);
    h.store.merge_cookies(&id, &[cookie("old", "1")]).unwrap();
    cdp.hold_get_cookies();
    h.sup.start(&id).await.unwrap();
    // Снимок по расписанию получил набор браузера без импорта, но ответ ещё не дошёл.
    wait_until("снимок в пути", || snapshots(&cdp) >= 1).await;
    let (_, sent) = h.sup.import_cookies(&id, vec![cookie("imp", "1")]).await.unwrap();
    assert!(sent, "импорт ушёл в браузер");
    cdp.release_get_cookies();
    // Следующий запрос приходит, только когда снимок в пути закончил запись в базу.
    wait_until("снимок в пути закончен", || snapshots(&cdp) >= 2).await;
    assert_eq!(stored(&h.store, &id), pairs(&[("imp", "1"), ("old", "1")]), "снимок, начатый до импорта, не стёр его");

    cdp.stop_holding_get_cookies();
    let all = pairs(&[("imp", "1"), ("old", "1"), ("site", "1")]);
    wait_until("следующий снимок пишет набор браузера", || stored(&h.store, &id) == all).await;
    h.sup.stop(&id).await.unwrap();
    assert_eq!(stored(&h.store, &id), all);
}

#[tokio::test]
async fn import_into_stopped_profile_only_updates_store() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let (count, sent) = h.sup.import_cookies(&id, vec![cookie("imp", "1")]).await.unwrap();
    assert_eq!((count.added, sent), (1, false));
    assert_eq!(stored(&h.store, &id), pairs(&[("imp", "1")]));
    assert!(matches!(h.sup.import_cookies("nope00", vec![cookie("x", "1")]).await, Err(SupervisorError::NotFound)));
    // При запуске набор из базы уходит в браузер.
    h.sup.start(&id).await.unwrap();
    assert!(h.engine.cdp(&h.paths.profile_dir(&id)).browser_cookies().iter().any(|c| c.name == "imp"));
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn import_into_profile_held_by_another_process_is_refused() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.store.merge_cookies(&id, &[cookie("old", "1")]).unwrap();
    std::fs::create_dir_all(h.paths.profile_dir(&id)).unwrap();
    let held = DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
    let err = h.sup.import_cookies(&id, vec![cookie("imp", "1")]).await.unwrap_err();
    assert!(matches!(err, SupervisorError::ProfileLocked), "{err}");
    assert_eq!(stored(&h.store, &id), pairs(&[("old", "1")]));
    drop(held);
    h.sup.import_cookies(&id, vec![cookie("imp", "1")]).await.unwrap();
    assert_eq!(stored(&h.store, &id), pairs(&[("imp", "1"), ("old", "1")]));
}

#[tokio::test]
async fn import_into_unreachable_browser_changes_nothing() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.store.merge_cookies(&id, &[cookie("old", "1")]).unwrap();
    h.sup.start(&id).await.unwrap();
    h.engine.cdp(&h.paths.profile_dir(&id)).go_down();
    let err = h.sup.import_cookies(&id, vec![cookie("imp", "1")]).await.unwrap_err();
    assert_eq!(err.code(), "CookiesFailed", "{err}");
    assert_eq!(stored(&h.store, &id), pairs(&[("old", "1")]), "база не тронута");
    // Последний снимок тоже не удаётся — остаётся прежний набор, остановка проходит.
    h.sup.stop(&id).await.unwrap();
    assert_eq!(stored(&h.store, &id), pairs(&[("old", "1")]));
}

#[tokio::test]
async fn start_with_passes_options_to_the_engine() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&id).await.unwrap();
    let plain = h.engine.last_request();
    assert!(!plain.headless && plain.restore_tabs && plain.extra_args.is_empty(), "{plain:?}");
    h.sup.stop(&id).await.unwrap();

    let options = StartOptions {
        headless: true,
        extra_args: Some(vec!["--disable-sync".into()]),
        restore_tabs: Some(false),
        start_urls: None,
        clear_cache_on_exit: false,
    };
    h.sup.start_with(&id, options).await.unwrap();
    let request = h.engine.last_request();
    assert!(request.headless);
    assert_eq!(request.extra_args, ["--disable-sync"]);
    assert!(!request.restore_tabs);
    let profile = h.store.get_profile(&id).unwrap().unwrap();
    assert!(profile.restore_tabs && profile.extra_args.is_empty(), "параметры запуска профиль не меняют");
    h.sup.stop(&id).await.unwrap();
}

/// Очеловечивание профиля доходит до запуска: включённое — его настройки и seed (флаги собирает `engine/ombra/flags.rs`),
/// выключенное — ничего, штатный движок.
#[tokio::test]
async fn start_passes_humanize_only_when_it_is_on() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let profile = h.store.get_profile(&id).unwrap().unwrap();
    h.sup.start(&id).await.unwrap();
    let launch = h.engine.last_request().humanize.expect("очеловечивание включено по умолчанию");
    assert_eq!((launch.seed, &launch.settings), (profile.humanize_seed, &profile.humanize));
    h.sup.stop(&id).await.unwrap();

    let off = crate::humanize::Humanize { enabled: false, ..profile.humanize.clone() };
    h.store.raw().execute("UPDATE profiles SET humanize = ?2 WHERE id = ?1", rusqlite::params![id, off.to_json()]).unwrap();
    h.sup.start(&id).await.unwrap();
    assert!(h.engine.last_request().humanize.is_none(), "выключенное — без флагов");
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn forbidden_launch_args_fail_the_start() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let options = StartOptions { extra_args: Some(vec!["--proxy-server=direct://".into()]), ..Default::default() };
    let err = h.sup.start_with(&id, options).await.unwrap_err();
    assert!(matches!(err, SupervisorError::Args(_)), "{err:?}");
    assert_eq!(err.code(), "BadArgs");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    assert!(h.sup.proxy_users().is_empty(), "резерв прокси снят");
    h.sup.start(&id).await.expect("обычный запуск после отказа проходит");
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn start_or_attach_returns_the_running_browser() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let first = h.sup.start(&id).await.unwrap();
    let again = h.sup.start_or_attach(&id, StartOptions::default(), Duration::from_secs(1)).await.unwrap();
    assert_eq!(again, first);
    assert_eq!(h.engine.launches(), 1, "второго браузера нет");
    assert_eq!(h.sup.running_info(&id), Some(first.clone()));

    h.sup.stop(&id).await.unwrap();
    assert_eq!(h.sup.running_info(&id), None);
    let fresh = h.sup.start_or_attach(&id, StartOptions::default(), Duration::from_secs(1)).await.unwrap();
    assert_ne!(fresh.pid, first.pid, "остановленный профиль запускается заново");
    assert_eq!(h.engine.launches(), 2);
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn start_or_attach_owned_tells_launched_from_attached() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let (first, launched) = h.sup.start_or_attach_owned(&id, StartOptions::default(), Duration::from_secs(1)).await.unwrap();
    assert!(launched, "закрытый профиль запущен этим вызовом");
    let (again, launched) = h.sup.start_or_attach_owned(&id, StartOptions::default(), Duration::from_secs(1)).await.unwrap();
    assert!(!launched, "работающий профиль не свой");
    assert_eq!(again, first);
    assert_eq!(h.engine.launches(), 1);
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn start_or_attach_waits_for_a_starting_profile() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let hold = Arc::new(tokio::sync::Notify::new());
    *h.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
    let starting = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.start(&id).await }
    });
    wait_launches(&h.engine, 1).await;

    let attaching = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.start_or_attach(&id, StartOptions::default(), Duration::from_secs(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!attaching.is_finished(), "ждёт идущий запуск");
    hold.notify_one();

    let started = starting.await.unwrap().unwrap();
    let attached = attaching.await.unwrap().unwrap();
    assert_eq!(attached, started);
    assert_eq!(h.engine.launches(), 1, "второго запуска нет");
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn start_or_attach_gives_up_after_wait() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let hold = Arc::new(tokio::sync::Notify::new());
    *h.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
    let starting = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.start(&id).await }
    });
    wait_launches(&h.engine, 1).await;

    let err = h.sup.start_or_attach(&id, StartOptions::default(), Duration::from_millis(200)).await.unwrap_err();
    assert!(matches!(err, SupervisorError::StillStarting), "{err:?}");
    assert_eq!(err.code(), "AlreadyRunning");
    hold.notify_one();
    starting.await.unwrap().expect("исходный запуск доходит до конца");
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn start_or_attach_reports_a_failed_start() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let hold = Arc::new(tokio::sync::Notify::new());
    *h.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
    *h.engine.fail_next_launch.lock().unwrap() = Some(ExitInfo { code: Some(1), signal: None });
    let starting = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.start(&id).await }
    });
    wait_launches(&h.engine, 1).await;
    let attaching = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.start_or_attach(&id, StartOptions::default(), Duration::from_secs(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    hold.notify_one();

    assert!(starting.await.unwrap().is_err());
    let err = attaching.await.unwrap().unwrap_err();
    assert!(matches!(err, SupervisorError::NotRunning), "{err:?}");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
}

#[tokio::test]
async fn start_or_attach_during_stop_launches_a_fresh_browser() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let first = h.sup.start(&id).await.unwrap();
    let hold = Arc::new(tokio::sync::Notify::new());
    *h.engine.hold_next_stop.lock().unwrap() = Some(hold.clone());
    let stopping = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.stop(&id).await }
    });
    // «Стоп» уже забрал браузер и ждёт движок; статус ещё «работает», браузер ещё отвечает.
    wait_until("остановка дошла до движка", || h.engine.hold_next_stop.lock().unwrap().is_none()).await;
    assert!(matches!(h.sup.status(&id), ProfileStatus::Running(_)), "{:?}", h.sup.status(&id));

    let err = h.sup.start_or_attach(&id, StartOptions::default(), Duration::from_millis(200)).await.unwrap_err();
    assert!(matches!(err, SupervisorError::StillStarting), "остановка не кончилась за wait: {err:?}");
    let attaching = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.start_or_attach(&id, StartOptions::default(), Duration::from_secs(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!attaching.is_finished(), "закрывающийся браузер не отдаётся: ждём конца остановки");
    hold.notify_one();
    stopping.await.unwrap().expect("остановка проходит");

    let fresh = tokio::time::timeout(Duration::from_secs(5), attaching)
        .await
        .expect("запуск после остановки не завис")
        .unwrap()
        .expect("после остановки — новый запуск");
    assert_ne!(fresh.cdp_ws, first.cdp_ws, "не адрес остановленного браузера");
    assert_ne!(fresh.pid, first.pid);
    assert_eq!(h.engine.launches(), 2);
    assert_eq!(h.sup.running_info(&id), Some(fresh));
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn start_or_attach_right_after_the_browser_exits_launches_a_fresh_browser() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let first = h.sup.start(&id).await.unwrap();
    // Скрипт закрыл браузер (`browser.close()` в примере AdsPower) и сразу запускает профиль снова:
    // процесс вышел, а супервизор этого ещё не заметил.
    h.engine.crash(&h.paths.profile_dir(&id), 0);
    assert!(matches!(h.sup.status(&id), ProfileStatus::Running(_)), "выход ещё не замечен");

    let fresh = tokio::time::timeout(
        Duration::from_secs(5),
        h.sup.start_or_attach(&id, StartOptions::default(), Duration::from_secs(5)),
    )
    .await
    .expect("запуск не завис")
    .expect("новый запуск");
    assert_ne!(fresh.cdp_ws, first.cdp_ws, "не адрес вышедшего браузера");
    assert_ne!(fresh.pid, first.pid);
    assert_eq!(h.engine.launches(), 2);
    assert_eq!(h.sup.running_info(&id), Some(fresh));
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn start_or_attach_waits_until_a_closing_browser_exits() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let first = h.sup.start(&id).await.unwrap();
    // `Browser.close` получен: порт отладки закрыт, процесс ещё завершается.
    h.engine.close_debug_port(&h.paths.profile_dir(&id));
    assert!(!h.engine.debug_port_answers(first.debug_port).await);
    let attaching = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.start_or_attach(&id, StartOptions::default(), Duration::from_secs(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!attaching.is_finished(), "к браузеру с закрытым портом отладки не подключиться: ждём его выхода");
    assert_eq!(h.engine.launches(), 1);

    h.engine.crash(&h.paths.profile_dir(&id), 0);
    let fresh = tokio::time::timeout(Duration::from_secs(5), attaching)
        .await
        .expect("запуск после выхода не завис")
        .unwrap()
        .expect("новый запуск");
    assert_ne!(fresh.pid, first.pid);
    assert_eq!(h.engine.launches(), 2);
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn running_lists_running_and_paused_profiles() {
    let h = harness(200).await;
    let (proxy, down) = switchable_proxy().await;
    let target = spawn_echo().await;
    let paused_id = add_profile(&h.store, Some(proxy.port()), "pass");
    let running_id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let stopped_id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let paused_info = h.sup.start(&paused_id).await.unwrap();
    let running_info = h.sup.start(&running_id).await.unwrap();

    down.store(true, Ordering::SeqCst);
    for _ in 0..UNREACHABLE_AFTER {
        tunnel_status(paused_info.relay_port, target).await;
    }
    wait_for(&h.sup, &paused_id, |s| matches!(s, ProfileStatus::Paused(_))).await;
    assert_eq!(h.sup.running_info(&paused_id), Some(paused_info.clone()), "на паузе браузер открыт");
    assert_eq!(h.sup.running_info(&stopped_id), None);

    let mut expected = vec![(paused_id.clone(), paused_info), (running_id.clone(), running_info)];
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(h.sup.running(), expected);

    h.sup.stop(&paused_id).await.unwrap();
    h.sup.stop(&running_id).await.unwrap();
    assert!(h.sup.running().is_empty());
}

#[tokio::test]
async fn cache_is_cleared_after_session_when_asked() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let options = StartOptions { clear_cache_on_exit: true, ..Default::default() };

    h.sup.start_with(&id, options.clone()).await.unwrap();
    let (cache, cookies) = fill_profile_dir(&h.paths, &id);
    h.sup.stop(&id).await.unwrap();
    assert!(!cache.exists(), "кэш удалён после остановки");
    assert!(cookies.exists(), "файл cookies браузера не тронут");

    h.sup.start_with(&id, options).await.unwrap();
    let (cache, cookies) = fill_profile_dir(&h.paths, &id);
    h.engine.crash(&h.paths.profile_dir(&id), 3);
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Crashed { .. })).await;
    assert!(!cache.exists(), "кэш удалён и после падения");
    assert!(cookies.exists());
}

#[tokio::test]
async fn cache_is_kept_without_clear_cache_on_exit() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&id).await.unwrap();
    let (cache, _) = fill_profile_dir(&h.paths, &id);
    h.sup.stop(&id).await.unwrap();
    assert!(cache.exists(), "без параметра кэш остаётся");
}

#[tokio::test]
async fn panicked_start_task_does_not_leave_profile_starting() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let hold = Arc::new(tokio::sync::Notify::new());
    *h.engine.hold_next_launch.lock().unwrap() = Some(hold.clone());
    h.engine.panic_next_launch.store(true, Ordering::SeqCst);
    let caller = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.start(&id).await }
    });
    wait_launches(&h.engine, 1).await;

    // Вызвавший бросил ожидание (Local API: клиент оборвал соединение) — запуск идёт дальше сам и паникует.
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    hold.notify_one();

    let status = wait_for(&h.sup, &id, |s| !s.is_active()).await;
    assert_eq!(status, ProfileStatus::Stopped);
    assert!(h.sup.proxy_users().is_empty(), "резерв прокси снят");
    assert_eq!(h.sup.active_count(), 0);
    h.sup.start(&id).await.expect("после оборванного запуска профиль запускается снова");
    h.sup.stop(&id).await.unwrap();
}

/// Стенд для паники подписчика событий на первом «работает»: задача запуска падает, когда браузер уже записан
/// в `active`. Запуск уже прошёл и вернул ошибку `Io`.
struct PanickedRunning {
    paths: Paths,
    engine: Arc<FakeEngine>,
    sup: Supervisor<FakeEngine>,
    id: String,
    _dir: tempfile::TempDir,
}

async fn start_with_panicking_subscriber() -> PanickedRunning {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    let store = Arc::new(memory_store());
    let engine = Arc::new(FakeEngine::new());
    let stack = geo_stack(200).await;
    let armed = Arc::new(AtomicBool::new(true));
    let trap = armed.clone();
    let sink: EventSink = Arc::new(move |event: Event| {
        if matches!(&event, Event::Status { status: ProfileStatus::Running(_), .. }) && trap.swap(false, Ordering::SeqCst) {
            panic!("подписчик событий: паника по заказу теста");
        }
    });
    let sup = Supervisor::new(paths.clone(), store.clone(), engine.clone(), Arc::new(stack.geo), sink, fast_timings());
    let id = add_profile(&store, Some(stack.proxy.port()), "pass");

    let err = sup.start(&id).await.unwrap_err();
    assert!(matches!(err, SupervisorError::Io(_)), "{err:?}");
    assert!(!armed.load(Ordering::SeqCst), "паника была");
    PanickedRunning { paths, engine, sup, id, _dir: dir }
}

#[tokio::test]
async fn panic_after_the_browser_is_running_does_not_mark_it_stopped() {
    let p = start_with_panicking_subscriber().await;
    let (paths, engine, sup, id) = (&p.paths, &p.engine, &p.sup, &p.id);
    assert!(engine.is_running(&paths.profile_dir(id)), "браузер работает");
    let status = sup.status(id);
    assert!(matches!(status, ProfileStatus::Running(_)), "работающий браузер — не «остановлен»: {status:?}");
    assert_eq!(sup.active_count(), 1);
    sup.stop(id).await.expect("работающий браузер останавливается");
    assert_eq!(sup.status(id), ProfileStatus::Stopped);
    assert!(!engine.is_running(&paths.profile_dir(id)));
    assert!(sup.proxy_users().is_empty(), "резерв прокси снят");
}

/// Паника между «работает» и слежением (здесь — у подписчика событий): слежение уже запущено, выход браузера
/// уходит в обычную уборку — «упал», PID-файл, блокировка и резерв прокси сняты.
#[tokio::test]
async fn panic_after_running_still_watches_the_browser() {
    let p = start_with_panicking_subscriber().await;
    let (paths, engine, sup, id) = (&p.paths, &p.engine, &p.sup, &p.id);
    engine.crash(&paths.profile_dir(id), 3);
    let status = wait_for(sup, id, |s| matches!(s, ProfileStatus::Crashed { .. })).await;
    assert!(matches!(status, ProfileStatus::Crashed { code: Some(3), .. }), "{status:?}");
    assert_eq!(sup.active_count(), 0);
    assert!(sup.proxy_users().is_empty(), "резерв прокси снят");
    assert!(pidfile::list(&paths.run_dir()).unwrap().is_empty(), "PID-файл убран");
    DirLock::acquire(&paths.profile_dir(id)).unwrap();
}

#[tokio::test]
async fn panicked_start_task_is_an_error_for_a_waiting_caller() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.engine.panic_next_launch.store(true, Ordering::SeqCst);
    let err = h.sup.start(&id).await.unwrap_err();
    assert!(matches!(err, SupervisorError::Io(_)), "{err:?}");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    assert!(h.sup.proxy_users().is_empty());
    h.sup.start(&id).await.expect("следующий запуск проходит");
    h.sup.stop(&id).await.unwrap();
}

/// Профиля нет — «запускается» не появляется. Профиль удалили, пока запуск ждал его очередь (удаление держит её,
/// как `Core::delete_profile`), — `NotFound`, браузер не запускается, записи статуса не остаётся.
#[tokio::test]
async fn deleted_profile_never_shows_starting_and_leaves_no_status() {
    let h = harness(200).await;
    assert!(matches!(h.sup.start("nope00").await, Err(SupervisorError::NotFound)));
    assert!(status_events(&h.events, "nope00").is_empty(), "несуществующий профиль не мелькает «запускается»");
    assert!(!h.sup.inner.lock_state().statuses.contains_key("nope00"));

    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let guard = h.sup.lock_idle(&id).await.unwrap();
    let starting = tokio::spawn({
        let sup = h.sup.clone();
        let id = id.clone();
        async move { sup.start(&id).await }
    });
    wait_for(&h.sup, &id, |s| *s == ProfileStatus::Starting).await;
    // Задача запуска встаёт в очередь профиля раньше удаления.
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.store.delete_profile(&id).unwrap();
    h.sup.forget(&id);
    drop(guard);

    let result = starting.await.unwrap();
    assert!(matches!(result, Err(SupervisorError::NotFound)), "{result:?}");
    assert_eq!(h.engine.launches(), 0, "браузер не запускался");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    assert!(!h.sup.inner.lock_state().statuses.contains_key(&id), "после forget записи статуса нет");
    assert_eq!(
        status_events(&h.events, &id),
        [ProfileStatus::Starting, ProfileStatus::Stopped],
        "интерфейс снимает «запускается»"
    );
    assert!(h.sup.proxy_users().is_empty());
}

/// Браузер с патчем 980 без потока лицензии: `/json/version` отвечает `503` с состоянием.
fn no_slot() -> Probe {
    Probe::License { state: "no_slot".into(), owner_hint: None }
}

/// Запуск профиля в отдельной задаче: тест смотрит на статусы, пока он идёт.
fn spawn_start(h: &Harness, id: &str) -> JoinHandle<Result<RunningInfo, SupervisorError>> {
    let sup = h.sup.clone();
    let id = id.to_string();
    tokio::spawn(async move { sup.start(&id).await })
}

#[tokio::test]
async fn waiting_status_is_emitted_and_cleared() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.engine.answer(no_slot());
    let starting = spawn_start(&h, &id);
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Waiting { info: None, .. })).await;
    assert!(h.sup.status(&id).is_active(), "ждущий профиль активен");
    assert!(matches!(h.sup.start(&id).await, Err(SupervisorError::AlreadyRunning)));

    h.engine.answer(Probe::Ready(String::new()));
    let info = tokio::time::timeout(Duration::from_secs(5), starting).await.expect("запуск не завис").unwrap().unwrap();
    assert_eq!(
        status_events(&h.events, &id),
        [
            ProfileStatus::Starting,
            ProfileStatus::Waiting { license: "no_slot".into(), info: None, owner_hint: None },
            ProfileStatus::Running(info),
        ]
    );
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn stop_cancels_a_license_wait() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.engine.answer(no_slot());
    let starting = spawn_start(&h, &id);
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Waiting { info: None, .. })).await;

    tokio::time::timeout(Duration::from_secs(2), h.sup.stop(&id))
        .await
        .expect("«стоп» не ждёт конца запуска")
        .expect("«стоп» ждущего профиля — не ошибка");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    let result = starting.await.unwrap();
    assert!(matches!(result, Err(SupervisorError::Cancelled)), "{result:?}");
    assert!(!h.engine.is_running(&h.paths.profile_dir(&id)), "браузер убит");
    assert!(h.sup.proxy_users().is_empty(), "прокси свободен");
    assert_eq!(
        status_events(&h.events, &id),
        [
            ProfileStatus::Starting,
            ProfileStatus::Waiting { license: "no_slot".into(), info: None, owner_hint: None },
            ProfileStatus::Stopped,
        ]
    );
    DirLock::acquire(&h.paths.profile_dir(&id)).unwrap();
}

#[tokio::test]
async fn old_engine_goes_straight_to_running() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let info = h.sup.start(&id).await.unwrap();
    // Несколько опросов работающего браузера: `200` статус не меняет.
    tokio::time::sleep(fast_timings().license_poll * 5).await;
    assert_eq!(status_events(&h.events, &id), [ProfileStatus::Starting, ProfileStatus::Running(info)]);
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn running_profile_that_loses_slot_waits_and_returns() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    let info = h.sup.start(&id).await.unwrap();

    h.engine.answer(no_slot());
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Waiting { .. })).await;
    assert!(h.sup.running_info(&id).is_none(), "браузер без потока скриптам не отдаётся");
    assert!(h.sup.status(&id).is_active());
    h.engine.answer(Probe::Ready(String::new()));
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Running(_))).await;
    assert_eq!(
        status_events(&h.events, &id),
        [
            ProfileStatus::Starting,
            ProfileStatus::Running(info.clone()),
            ProfileStatus::Waiting { license: "no_slot".into(), info: Some(info.clone()), owner_hint: None },
            ProfileStatus::Running(info),
        ]
    );
    h.sup.stop(&id).await.unwrap();
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
}

#[tokio::test]
async fn a_waiting_profile_can_receive_its_owner_hint_without_changing_license_state() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&id).await.unwrap();
    h.engine.answer(Probe::License { state: "device_bound".into(), owner_hint: None });
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Waiting { .. })).await;
    h.engine.answer(Probe::License { state: "device_bound".into(), owner_hint: Some("iv***v@example.com".into()) });
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Waiting { owner_hint: Some(hint), .. } if hint == "iv***v@example.com")).await;
    h.engine.answer(Probe::Ready(String::new()));
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Running(_))).await;
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn restored_profile_respects_gateway_health() {
    let h = harness(200).await;
    let (proxy, down) = switchable_proxy().await;
    let target = spawn_echo().await;
    let id = add_profile(&h.store, Some(proxy.port()), "pass");
    let info = h.sup.start(&id).await.unwrap();
    h.engine.answer(no_slot());
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Waiting { .. })).await;

    // Шлюз перестал отвечать, пока браузер ждёт поток: статус остаётся «ждёт поток».
    down.store(true, Ordering::SeqCst);
    for _ in 0..UNREACHABLE_AFTER {
        tunnel_status(info.relay_port, target).await;
    }
    h.engine.answer(Probe::Ready(String::new()));
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Paused(_))).await;
    assert_eq!(
        status_events(&h.events, &id),
        [
            ProfileStatus::Starting,
            ProfileStatus::Running(info.clone()),
            ProfileStatus::Waiting { license: "no_slot".into(), info: Some(info.clone()), owner_hint: None },
            ProfileStatus::Paused(info),
        ]
    );
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn attach_to_waiting_profile_times_out_with_license_waiting() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.engine.answer(no_slot());
    let starting = spawn_start(&h, &id);
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Waiting { .. })).await;

    let err = h.sup.start_or_attach(&id, StartOptions::default(), Duration::from_millis(200)).await.unwrap_err();
    assert!(matches!(err, SupervisorError::LicenseWaiting), "{err:?}");
    assert_eq!(err.code(), "LicenseWaiting");
    assert_eq!(h.engine.launches(), 1, "второго запуска нет");
    h.sup.stop(&id).await.unwrap();
    assert!(matches!(starting.await.unwrap(), Err(SupervisorError::Cancelled)));
}

#[tokio::test]
async fn fresh_start_or_attach_gives_up_on_a_license_wait() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.engine.answer(no_slot());
    let err = tokio::time::timeout(Duration::from_secs(5), h.sup.start_or_attach(&id, StartOptions::default(), Duration::from_millis(200)))
        .await
        .expect("Local API отвечает к сроку")
        .unwrap_err();
    assert!(matches!(err, SupervisorError::LicenseWaiting), "{err:?}");
    // Скрипт получил ошибку: браузер, который возьмёт поток позже, ему не нужен.
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    assert!(!h.engine.is_running(&h.paths.profile_dir(&id)), "запуск этого вызова отменён");
    assert!(h.sup.proxy_users().is_empty(), "прокси свободен");
}

#[tokio::test]
async fn waiting_browser_skips_cookie_snapshots() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.sup.start(&id).await.unwrap();
    h.engine.answer(no_slot());
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Waiting { .. })).await;
    // Снимок, начатый до «ждёт поток», успевает закончиться.
    tokio::time::sleep(fast_timings().cookie_interval).await;
    let cdp = h.engine.cdp(&h.paths.profile_dir(&id));
    let before = cdp.calls().len();
    tokio::time::sleep(fast_timings().cookie_interval * 4).await;
    assert_eq!(cdp.calls().len(), before, "браузер без потока закрыл CDP: снимков нет");
    h.engine.answer(Probe::Ready(String::new()));
    wait_for(&h.sup, &id, |s| matches!(s, ProfileStatus::Running(_))).await;
    h.sup.stop(&id).await.unwrap();
}

#[tokio::test]
async fn stop_during_starting_cancels_a_later_license_wait() {
    let h = harness(200).await;
    let id = add_profile(&h.store, Some(h.proxy.port()), "pass");
    h.engine.answer(no_slot());
    *h.engine.launch_delay.lock().unwrap() = Duration::from_millis(300);
    let starting = spawn_start(&h, &id);
    // Запуск взял очередь и ещё «запускается»: браузер скажет про лицензию позже, когда «стоп» уже придёт.
    wait_until("запуск идёт, статус «запускается»", || {
        h.sup.inner.lock_state().launching.contains_key(&id) && h.sup.status(&id) == ProfileStatus::Starting
    })
    .await;

    tokio::time::timeout(Duration::from_secs(2), h.sup.stop(&id))
        .await
        .expect("«стоп» не ждёт потока лицензии")
        .expect("«стоп» запускающегося профиля — не ошибка");
    assert_eq!(h.sup.status(&id), ProfileStatus::Stopped);
    assert!(matches!(starting.await.unwrap(), Err(SupervisorError::Cancelled)));
    assert!(!h.engine.is_running(&h.paths.profile_dir(&id)), "браузер убит");
}
