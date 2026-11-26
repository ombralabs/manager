//! Живые тесты (`#[ignore]`): настоящий движок Ombra, прослойка и локальный прокси с авторизацией.
//! Запуск: `cargo test --lib live -- --ignored --nocapture --test-threads=1`. Тест примера Local API (`live_api`)
//! ещё требует Node и `npm ci` в корне репозитория. Сеть нужна только для сайтов проверки.
//! `live_twenty_profiles` держит 20 браузеров не меньше 60 с: около 3 минут и несколько ГБ памяти.
use super::EventSink;
use super::{Event, ProfileStatus, Supervisor, SupervisorError, Timings};
use super::pidfile;
use crate::api::{ApiSettings, serve};
use crate::app::Core;
use crate::engine::Engine;
use crate::engine::ombra::OmbraEngine;
use crate::engine::process::{find_profile_processes, matches_profile, parse_ps};
use crate::fingerprint::{Arch, Catalog, OsRandom, generate};
use crate::paths::Paths;
use crate::relay::geo::{GeoClient, Provider};
use crate::relay::test_support::spawn_routed_proxy_with;
use crate::relay::test_support::spawn_switchable_routed_proxy;
use crate::relay::test_support::{Log, read_head, spawn_routed_proxy};
use crate::relay::test_support::port_closes;
use crate::store::Store;
use crate::store::profiles::ProfileEdit;
use crate::store::profiles::ProfileInput;
use crate::store::proxies::{ProxyInput, ProxyKind};
use crate::store::secrets::{Secret, SecretField};
use crate::store::tests::memory_store;
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde_json::json;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// Ответ «ipwho.is» для IP прокси: Япония, чтобы таймзона и язык заметно отличались от машины разработчика.
const GEO_JSON: &str = r#"{"ip":"198.51.100.77","success":true,"country_code":"JP","timezone":{"id":"Asia/Tokyo"}}"#;

/// Страница сообщает, что видит сайт: таймзону, язык и cookies.
const REPORT_PAGE: &str = "<!doctype html><title>ombra</title><script>\
    fetch('/report?tz=' + encodeURIComponent(Intl.DateTimeFormat().resolvedOptions().timeZone) + \
    '&lang=' + encodeURIComponent(navigator.language) + '&cookie=' + encodeURIComponent(document.cookie))\
    </script>";

/// Страница раз в полсекунды запрашивает `/ping`: браузер сам всё время ходит через прокси, ошибки не останавливают опрос.
const POLL_PAGE: &str = "<!doctype html><title>poll</title><script>\
    setInterval(() => fetch('/ping?' + Date.now()).catch(() => {}), 500)\
    </script>";

/// Адрес Local API в примере AdsPower: копия примера заменяет его, и только его, на адрес тестового сервера.
const ADSPOWER_API: &str = "http://local.adspower.net:50325";
/// Сайт, который открывает пример AdsPower.
const ADSPOWER_HOST: &str = "www.adspower.io";
const ADSPOWER_PAGE: &str = "<!doctype html><title>AdsPower</title><h1>AdsPower</h1>";

/// Сайт проверки: `/geo` — подставной геосервис, `/set` ставит cookie, `/show` только сообщает; запросы пишутся в журнал.
async fn spawn_check_site() -> (SocketAddr, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Log = Arc::default();
    let seen = log.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { continue };
            let seen = seen.clone();
            tokio::spawn(async move {
                let head = String::from_utf8_lossy(&read_head(&mut stream).await).into_owned();
                let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                // Через HTTP-прокси путь приходит в абсолютной форме.
                let path = target
                    .strip_prefix("http://")
                    .and_then(|rest| rest.find('/').map(|i| rest[i..].to_string()))
                    .unwrap_or(target);
                seen.lock().unwrap().push(path.clone());
                let (content_type, extra, body) = if path.starts_with("/geo") {
                    ("application/json", "", GEO_JSON)
                } else if path.starts_with("/set") {
                    ("text/html", "Set-Cookie: ombra_live=1; Max-Age=86400; Path=/\r\n", REPORT_PAGE)
                } else if path.starts_with("/show") {
                    ("text/html", "", REPORT_PAGE)
                } else if path.starts_with("/poll") {
                    ("text/html", "", POLL_PAGE)
                } else {
                    ("text/plain", "", "")
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (addr, log)
}

async fn wait_for_report(log: &Log, needle: &str) -> String {
    for _ in 0..300 {
        let found = log.lock().unwrap().iter().find(|p| p.starts_with("/report?") && p.contains(needle)).cloned();
        if let Some(report) = found {
            return report;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("страница не прислала отчёт с {needle}: {:?}", log.lock().unwrap());
}

/// Сайт `https://www.adspower.io` для примера AdsPower без выхода в сеть: самоподписанный сертификат на это имя
/// (профиль примера запускается с `--ignore-certificate-errors`), страница с заголовком, журнал «строка запроса | Host».
async fn spawn_adspower_site() -> (SocketAddr, Log) {
    let rcgen::CertifiedKey { cert, key_pair } = rcgen::generate_simple_self_signed(vec![ADSPOWER_HOST.to_string()]).unwrap();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(key_pair.serialize_der().into());
    let config = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(vec![cert.der().clone()], key).unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Log = Arc::default();
    let seen = log.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { continue };
            let (acceptor, seen) = (acceptor.clone(), seen.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else { return };
                let head = String::from_utf8_lossy(&read_head(&mut tls).await).into_owned();
                let line = head.lines().next().unwrap_or_default().to_string();
                let host = head
                    .lines()
                    .find_map(|l| l.strip_prefix("Host: ").or_else(|| l.strip_prefix("host: ")))
                    .unwrap_or_default()
                    .to_string();
                seen.lock().unwrap().push(format!("{line} | {host}"));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ADSPOWER_PAGE}",
                    ADSPOWER_PAGE.len()
                );
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, log)
}

/// При панике теста добивает всё, что осталось от браузера профиля: иначе Chromium переживёт временную папку.
pub(crate) struct KillProfileOnPanic(pub(crate) PathBuf);

impl Drop for KillProfileOnPanic {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let Ok(out) = std::process::Command::new("ps").args(["-axww", "-o", "pid=,command="]).output() else { return };
        for (pid, command) in parse_ps(&String::from_utf8_lossy(&out.stdout)) {
            if pid != std::process::id() && matches_profile(&command, &self.0) {
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
        }
    }
}

/// Импорт настоящего архива сборки: `OMBRA_ENGINE_ARCHIVE=/путь/ombra-engine-….tar.gz`; затем бинарник отвечает на `--version`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "распаковывает настоящий архив движка; OMBRA_ENGINE_ARCHIVE=… cargo test --lib live_import_real_engine_archive -- --ignored --nocapture"]
async fn live_import_real_engine_archive() {
    let Some(archive) = std::env::var_os("OMBRA_ENGINE_ARCHIVE") else {
        println!("пропуск: OMBRA_ENGINE_ARCHIVE не задан");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let engine = OmbraEngine::new(dir.path().join("engines"));
    let build = engine.import_archive(Path::new(&archive)).await.unwrap();
    println!("импортировано: {} ({})", build.version, build.binary.display());
    assert_eq!(build.version, crate::engine::ombra::install::EXPECTED_VERSION);
    let out = std::process::Command::new(&build.binary).arg("--version").output().unwrap();
    let line = String::from_utf8_lossy(&out.stdout);
    println!("--version: {}", line.trim());
    assert!(line.contains(crate::engine::ombra::install::EXPECTED_VERSION), "--version: {line:?}, stderr: {}", String::from_utf8_lossy(&out.stderr));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib live -- --ignored"]
async fn live_profile_through_authenticated_proxy() {
    let engine = Arc::new(OmbraEngine::new(Paths::new(Paths::default_root()).engines()));
    engine.installed().expect("движок не импортирован: Ombra → Настройки → Движок (или положите Chromium.app в engines/)");

    let root = tempfile::tempdir().unwrap();
    let paths = Paths::new(root.path());
    let (site, site_log) = spawn_check_site().await;
    let (proxy, proxy_log) = spawn_routed_proxy("dXNlcjpwYXNz", site).await;
    let store = Arc::new(memory_store());
    let proxy_id = store
        .create_proxy(&ProxyInput {
            kind: ProxyKind::Http,
            host: "127.0.0.1".into(),
            port: proxy.port(),
            username: Some("user".into()),
            password: SecretField::Set(Secret::new("pass")),
            note: String::new(),
            enabled: true,
        })
        .unwrap()
        .id;
    let fingerprint = generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap();
    let mut input = ProfileInput::new("Живой тест", fingerprint.to_json());
    input.proxy_id = Some(proxy_id);
    input.start_urls = vec!["http://check.ombra.test/set".into()];
    let id = store.create_profile(&input, 1).unwrap().id;
    let _cleanup = KillProfileOnPanic(paths.profile_dir(&id));
    let geo = Arc::new(GeoClient::with_endpoints(vec![(Provider::IpWhoIs, "http://geo.ombra.test/geo".into())]));
    let supervisor =
        Supervisor::new(paths.clone(), store.clone(), engine, geo, Arc::new(|_: Event| {}), Timings::default());

    // 1–2. Запуск через прокси с логином и паролем; IP и таймзона — от геосервиса, браузер их видит.
    let info = supervisor.start(&id).await.expect("профиль не запустился");
    assert_eq!(info.exit_ip.as_deref(), Some("198.51.100.77"));
    assert_eq!(info.timezone, "Asia/Tokyo");
    let version: serde_json::Value = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://127.0.0.1:{}/json/version", info.debug_port))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(version["webSocketDebuggerUrl"], info.cdp_ws.as_str());
    let report = wait_for_report(&site_log, "tz=Asia%2FTokyo").await;
    assert!(report.contains("lang=ja-JP"), "{report}");
    assert!(
        proxy_log
            .lock()
            .unwrap()
            .iter()
            .any(|h| h.contains("check.ombra.test") && h.contains("Proxy-Authorization: Basic dXNlcjpwYXNz")),
        "сайт открыт не через прокси с авторизацией"
    );

    // 4. Повторный запуск отклоняется.
    assert!(matches!(supervisor.start(&id).await, Err(SupervisorError::AlreadyRunning)));

    // 3. После остановки нет процессов.
    supervisor.stop(&id).await.unwrap();
    assert_eq!(supervisor.status(&id), ProfileStatus::Stopped);
    assert!(find_profile_processes(&paths.profile_dir(&id)).await.unwrap().is_empty());

    // 5. Cookie переживает перезапуск.
    site_log.lock().unwrap().clear();
    input.start_urls = vec!["http://check.ombra.test/show".into()];
    // Вкладка `/set` прошлой сессии не должна вернуться: cookie обязана прийти из прошлого запуска, а не от сайта.
    input.restore_tabs = false;
    store.update_profile(&id, &input).unwrap();
    supervisor.start(&id).await.expect("повторный запуск после остановки");
    wait_for_report(&site_log, "cookie=ombra_live%3D1").await;
    assert!(
        !site_log.lock().unwrap().iter().any(|p| p.starts_with("/set")),
        "cookie должна прийти из папки профиля, а не быть поставлена заново"
    );
    supervisor.stop(&id).await.unwrap();
    assert!(find_profile_processes(&paths.profile_dir(&id)).await.unwrap().is_empty());
}

/// Окружение живых тестов 2А: установленный движок, сайт проверки и супервизор с журналом событий.
struct Live {
    supervisor: Supervisor<OmbraEngine>,
    store: Arc<Store>,
    paths: Paths,
    site: SocketAddr,
    site_log: Log,
    events: Arc<Mutex<Vec<Event>>>,
    _root: tempfile::TempDir,
}

async fn live(timings: Timings) -> Live {
    // OMBRA_PROBE_ENGINES — запасной каталог слота (например, со ссылкой на свежую сборку).
    let slot = std::env::var_os("OMBRA_PROBE_ENGINES").map(PathBuf::from).unwrap_or_else(|| Paths::new(Paths::default_root()).engines());
    let engine = Arc::new(OmbraEngine::new(slot));
    engine.installed().expect("движок не импортирован: Ombra → Настройки → Движок (или положите Chromium.app в engines/)");
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::new(root.path());
    let (site, site_log) = spawn_check_site().await;
    let store = Arc::new(memory_store());
    let geo = Arc::new(GeoClient::with_endpoints(vec![(Provider::IpWhoIs, "http://geo.ombra.test/geo".into())]));
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let seen = events.clone();
    let sink: EventSink = Arc::new(move |event: Event| seen.lock().unwrap().push(event));
    let supervisor = Supervisor::new(paths.clone(), store.clone(), engine, geo, sink, timings);
    Live { supervisor, store, paths, site, site_log, events, _root: root }
}

/// HTTP-прокси `user:pass` на 127.0.0.1.
fn live_proxy(store: &Store, port: u16) -> i64 {
    store
        .create_proxy(&ProxyInput {
            kind: ProxyKind::Http,
            host: "127.0.0.1".into(),
            port,
            username: Some("user".into()),
            password: SecretField::Set(Secret::new("pass")),
            note: String::new(),
            enabled: true,
        })
        .unwrap()
        .id
}

fn live_profile(store: &Store, proxy_id: Option<i64>, start_url: &str) -> String {
    let fingerprint = generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap();
    let mut input = ProfileInput::new("Живой тест 2А", fingerprint.to_json());
    input.proxy_id = proxy_id;
    input.start_urls = vec![start_url.to_string()];
    store.create_profile(&input, 1).unwrap().id
}

async fn wait_status(
    supervisor: &Supervisor<OmbraEngine>,
    id: &str,
    done: impl Fn(&ProfileStatus) -> bool,
) -> ProfileStatus {
    for _ in 0..300 {
        let status = supervisor.status(id);
        if done(&status) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("статус не дождались: {:?}", supervisor.status(id));
}

/// Cookie живёт в базе Ombra: папка браузера потеряна, а сайт после запуска всё равно её видит.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib live -- --ignored --test-threads=1"]
async fn live_cookies_survive_lost_profile_folder() {
    let live = live(Timings { cookie_interval: Duration::from_secs(1), ..Timings::default() }).await;
    let (proxy, _) = spawn_routed_proxy("dXNlcjpwYXNz", live.site).await;
    let id = live_profile(&live.store, Some(live_proxy(&live.store, proxy.port())), "http://check.ombra.test/set");
    let _cleanup = KillProfileOnPanic(live.paths.profile_dir(&id));

    live.supervisor.start(&id).await.expect("профиль не запустился");
    wait_for_report(&live.site_log, "cookie=ombra_live%3D1").await;
    // Снимок раз в секунду переносит cookie сайта в базу.
    for _ in 0..100 {
        if live.store.load_cookies(&id).unwrap().iter().any(|c| c.name == "ombra_live" && c.value == "1") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let stored = live.store.load_cookies(&id).unwrap();
    let cookie = stored.iter().find(|c| c.name == "ombra_live").expect("cookie сайта не попала в базу");
    assert_eq!((cookie.domain.as_str(), cookie.host_only, cookie.value.as_str()), ("check.ombra.test", true, "1"));
    live.supervisor.stop(&id).await.unwrap();

    std::fs::remove_dir_all(live.paths.profile_dir(&id)).unwrap();
    live.site_log.lock().unwrap().clear();
    let show = ProfileEdit { start_urls: Some(vec!["http://check.ombra.test/show".into()]), ..Default::default() };
    live.store.edit_profiles(std::slice::from_ref(&id), &show).unwrap();
    live.supervisor.start(&id).await.expect("запуск после потери папки");
    wait_for_report(&live.site_log, "cookie=ombra_live%3D1").await;
    assert!(
        !live.site_log.lock().unwrap().iter().any(|p| p.starts_with("/set")),
        "cookie должна прийти из базы, а не быть поставлена сайтом заново"
    );
    live.supervisor.stop(&id).await.unwrap();
    assert!(find_profile_processes(&live.paths.profile_dir(&id)).await.unwrap().is_empty());
}

/// Пользователь закрыл последнее окно: движок выходит сам (на macOS — патч движка 245), профиль «остановлен»,
/// cookie сайта остаётся в базе из снимка, браузер закрыт.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib live_closing_last_window -- --ignored --nocapture"]
async fn live_closing_last_window_stops_profile() {
    let live = live(Timings { cookie_interval: Duration::from_secs(1), ..Timings::default() }).await;
    let (proxy, _) = spawn_routed_proxy("dXNlcjpwYXNz", live.site).await;
    let id = live_profile(&live.store, Some(live_proxy(&live.store, proxy.port())), "http://check.ombra.test/set");
    let _cleanup = KillProfileOnPanic(live.paths.profile_dir(&id));

    let info = live.supervisor.start(&id).await.expect("профиль не запустился");
    wait_for_report(&live.site_log, "cookie=ombra_live%3D1").await;
    // Без окон браузер cookies не отдаёт: в базе останется то, что успел снять снимок по расписанию.
    for _ in 0..100 {
        if live.store.load_cookies(&id).unwrap().iter().any(|c| c.name == "ombra_live" && c.value == "1") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let mut cdp = crate::engine::cdp::CdpClient::connect(&info.cdp_ws).await.unwrap();
    let targets = cdp.send("Target.getTargets", json!({})).await.unwrap();
    for page in targets["targetInfos"].as_array().unwrap().iter().filter(|t| t["type"] == "page") {
        // С последней страницей браузер выходит: ответа на её закрытие может не быть.
        let _ = cdp.send("Target.closeTarget", json!({ "targetId": page["targetId"] })).await;
    }

    wait_status(&live.supervisor, &id, |s| *s == ProfileStatus::Stopped).await;
    assert!(find_profile_processes(&live.paths.profile_dir(&id)).await.unwrap().is_empty(), "браузер закрыт");
    let stored = live.store.load_cookies(&id).unwrap();
    assert!(stored.iter().any(|c| c.name == "ombra_live" && c.value == "1"), "cookie сайта — в базе");
}

/// Закреплённый прокси выключен: профиль запускается через другой и получает отметку о замене.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib live -- --ignored --test-threads=1"]
async fn live_pinned_proxy_is_replaced_when_disabled() {
    let live = live(Timings::default()).await;
    let (first, _) = spawn_routed_proxy("dXNlcjpwYXNz", live.site).await;
    let (second, second_log) = spawn_routed_proxy("dXNlcjpwYXNz", live.site).await;
    let pinned = live_proxy(&live.store, first.port());
    let spare = live_proxy(&live.store, second.port());
    let id = live_profile(&live.store, Some(pinned), "http://check.ombra.test/show");
    let _cleanup = KillProfileOnPanic(live.paths.profile_dir(&id));
    live.store.set_proxy_enabled(pinned, false).unwrap();

    let info = live.supervisor.start(&id).await.expect("профиль не запустился");
    assert_eq!(info.proxy_id, spare);
    let record = live.store.get_profile(&id).unwrap().unwrap();
    assert_eq!(record.proxy_id, Some(spare), "запасной прокси закреплён");
    assert!(record.gateway_replaced_at.is_some(), "отметка «шлюз заменён при запуске»");
    assert!(
        live.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::GatewayReplaced { profile_id, .. } if *profile_id == id)),
        "событие о замене шлюза"
    );
    wait_for_report(&live.site_log, "tz=Asia%2FTokyo").await;
    assert!(
        second_log.lock().unwrap().iter().any(|h| h.contains("check.ombra.test")),
        "сайт открыт через запасной прокси"
    );
    live.supervisor.stop(&id).await.unwrap();
    assert!(find_profile_processes(&live.paths.profile_dir(&id)).await.unwrap().is_empty());
}

/// Прокси «упал» — профиль на «Паузе», браузер открыт; прокси вернулся — профиль снова «Работает».
#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib live -- --ignored --test-threads=1"]
async fn live_gateway_outage_pauses_and_resumes() {
    let live = live(Timings::default()).await;
    let (proxy, _, down) = spawn_switchable_routed_proxy("dXNlcjpwYXNz", live.site).await;
    let id = live_profile(&live.store, Some(live_proxy(&live.store, proxy.port())), "http://check.ombra.test/poll");
    let _cleanup = KillProfileOnPanic(live.paths.profile_dir(&id));

    let info = live.supervisor.start(&id).await.expect("профиль не запустился");
    for _ in 0..150 {
        if live.site_log.lock().unwrap().iter().any(|p| p.starts_with("/ping")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(live.site_log.lock().unwrap().iter().any(|p| p.starts_with("/ping")), "страница не опрашивает сайт");

    down.store(true, Ordering::SeqCst);
    wait_status(&live.supervisor, &id, |s| matches!(s, ProfileStatus::Paused(_))).await;
    assert!(live.events.lock().unwrap().contains(&Event::GatewayUnreachable { profile_id: id.clone() }));
    assert!(
        !find_profile_processes(&live.paths.profile_dir(&id)).await.unwrap().is_empty(),
        "на паузе браузер остаётся открытым"
    );

    down.store(false, Ordering::SeqCst);
    let status = wait_status(&live.supervisor, &id, |s| matches!(s, ProfileStatus::Running(_))).await;
    let ProfileStatus::Running(resumed) = status else { unreachable!() };
    assert_eq!(resumed.pid, info.pid, "тот же браузер, без перезапуска");
    live.supervisor.stop(&id).await.unwrap();
    assert!(find_profile_processes(&live.paths.profile_dir(&id)).await.unwrap().is_empty());
}

/// Критерий этапа 2: пример Puppeteer из документации AdsPower (`tools/api-examples/adspower-puppeteer.js`, дословно)
/// работает с Ombra, если заменить только адрес Local API и id профиля.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер и Node; npm ci, затем cargo test --lib live_api -- --ignored --test-threads=1"]
async fn live_api_puppeteer_example() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let node_modules = repo.join("node_modules");
    assert!(
        node_modules.join("puppeteer-core").is_dir() && node_modules.join("axios").is_dir(),
        "сначала npm ci в корне репозитория"
    );
    let sample = std::fs::read_to_string(repo.join("tools/api-examples/adspower-puppeteer.js")).unwrap();
    assert_eq!(sample.matches(ADSPOWER_API).count(), 1, "адрес Local API в примере ровно один");
    assert_eq!(sample.matches("XXX").count(), 1, "id профиля в примере ровно один");

    let engine = Arc::new(OmbraEngine::new(Paths::new(Paths::default_root()).engines()));
    engine.installed().expect("движок не импортирован: Ombra → Настройки → Движок (или положите Chromium.app в engines/)");
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::new(root.path());
    let (site, _) = spawn_check_site().await;
    let (adspower, adspower_log) = spawn_adspower_site().await;
    let (proxy, proxy_log) = spawn_routed_proxy_with("dXNlcjpwYXNz", site, vec![(ADSPOWER_HOST, adspower)]).await;
    let store = Arc::new(memory_store());
    let geo = Arc::new(GeoClient::with_endpoints(vec![(Provider::IpWhoIs, "http://geo.ombra.test/geo".into())]));
    let core = Arc::new(Core::new(paths.clone(), store.clone(), engine, geo, Arc::new(|_: Event| {}), Timings::default()));
    let server = serve(core.clone(), ApiSettings { port: 0, key: None }).await.expect("Local API не поднялся");
    let base = format!("http://{}", server.addr());

    // Профиль создаётся через Local API, как это сделал бы скрипт AdsPower.
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let created: serde_json::Value = client
        .post(format!("{base}/api/v1/user/create"))
        .json(&json!({
            "name": "Пример AdsPower",
            "group_id": "0",
            "user_proxy_config": {
                "proxy_soft": "other", "proxy_type": "http", "proxy_host": "127.0.0.1",
                "proxy_port": proxy.port().to_string(), "proxy_user": "user", "proxy_password": "pass"
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["code"], 0, "{created}");
    let id = created["data"]["id"].as_str().expect("id профиля строкой").to_string();
    let _cleanup = KillProfileOnPanic(paths.profile_dir(&id));
    // Настройка профиля, не часть примера: браузер должен принять самоподписанный сертификат тестового сайта.
    let trust = ProfileEdit { extra_args: Some(vec!["--ignore-certificate-errors".into()]), ..Default::default() };
    store.edit_profiles(std::slice::from_ref(&id), &trust).unwrap();

    // Копия примера: заменены только адрес Local API и id профиля.
    let script = sample.replace(ADSPOWER_API, &base).replace("XXX", &id);
    let work = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("adspower-puppeteer.js"), script).unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(180),
        tokio::process::Command::new("node")
            .arg("adspower-puppeteer.js")
            .current_dir(work.path())
            .env("NODE_PATH", &node_modules)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("пример не завершился за 3 минуты")
    .expect("node не запустился");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Вывод примера (`console.log(res.data)`) виден при `--nocapture`.
    println!("{stdout}");
    assert!(output.status.success(), "node: {:?}\n{stdout}\n{stderr}", output.status);
    assert!(stdout.contains("code: 0") && stdout.contains("puppeteer:"), "ответ запуска: {stdout}");

    // Пример ловит все ошибки сам, поэтому доказательство — скриншот: он делается только после открытой страницы.
    let png = std::fs::read(work.path().join("adspower.png")).unwrap_or_else(|_| panic!("скриншота нет: {stdout}"));
    assert!(png.starts_with(b"\x89PNG") && png.len() > 1000, "это не скриншот страницы");
    assert!(
        adspower_log.lock().unwrap().iter().any(|l| l.starts_with("GET / ") && l.contains(ADSPOWER_HOST)),
        "страница не открывалась: {:?}",
        adspower_log.lock().unwrap()
    );
    assert!(
        proxy_log
            .lock()
            .unwrap()
            .iter()
            .any(|h| h.starts_with("CONNECT www.adspower.io:443") && h.contains("Proxy-Authorization: Basic dXNlcjpwYXNz")),
        "сайт открыт не через прокси профиля"
    );

    // `browser.close()` из примера закрывает браузер штатно: профиль «Остановлен», процессов нет.
    wait_status(&core.supervisor, &id, |s| *s == ProfileStatus::Stopped).await;
    assert!(find_profile_processes(&paths.profile_dir(&id)).await.unwrap().is_empty());
    server.shutdown();
}


/// Профилей в тесте «20 одновременно»: критерий этапа 2 и лимит запусков по умолчанию.
const TWENTY: usize = 20;
/// Сколько все профили должны прожить после запуска.
const ALIVE_FOR: Duration = Duration::from_secs(60);
/// Предел одного запроса к Local API: 20 браузеров разом на ноутбуке запускаются небыстро.
const API_CALL_LIMIT: Duration = Duration::from_secs(180);

/// GET к Local API; ответ — конверт AdsPower. `None` — сеть, тайм-аут или не JSON.
async fn api_get(client: reqwest::Client, url: String) -> Option<serde_json::Value> {
    let call = async { client.get(&url).send().await.ok()?.json::<serde_json::Value>().await.ok() };
    tokio::time::timeout(API_CALL_LIMIT, call).await.ok().flatten()
}

/// Все запросы разом, как у скрипта, который открывает профили параллельно; ответы — в порядке адресов.
async fn api_get_all(client: &reqwest::Client, urls: Vec<String>) -> Vec<Option<serde_json::Value>> {
    let mut calls = tokio::task::JoinSet::new();
    for (index, url) in urls.into_iter().enumerate() {
        let client = client.clone();
        calls.spawn(async move { (index, api_get(client, url).await) });
    }
    let mut answers = vec![None; calls.len()];
    while let Some(joined) = calls.join_next().await {
        let (index, answer) = joined.expect("запрос к Local API упал");
        answers[index] = answer;
    }
    answers
}

/// Ждёт, пока `done` не станет истинным, не дольше `limit`; итог — последняя проверка.
async fn eventually(limit: Duration, done: impl Fn() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    while !done() {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    true
}

/// Память (RSS, МБ) всех процессов браузеров этих профилей: главный процесс и помощники Chromium — у каждого в командной
/// строке `--user-data-dir` своего профиля. Общие страницы библиотек входят в каждый процесс: это оценка сверху.
async fn browsers_rss_mb(profile_dirs: &[PathBuf]) -> u64 {
    let out = tokio::process::Command::new("ps").args(["-axww", "-o", "pid=,rss=,command="]).output().await.unwrap();
    let me = std::process::id();
    let kib: u64 = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, rest) = line.trim_start().split_once(' ')?;
            let (rss, command) = rest.trim_start().split_once(' ')?;
            let (pid, rss) = (pid.parse::<u32>().ok()?, rss.parse::<u64>().ok()?);
            let ours = pid != me && profile_dirs.iter().any(|dir| matches_profile(command.trim_start(), dir));
            ours.then_some(rss)
        })
        .sum();
    kib / 1024
}

/// Критерий этапа 2 «20 профилей работают одновременно»: 20 профилей, каждый со своим прокси с авторизацией, запускаются
/// разом через Local API, живут 60 с и останавливаются без остатка.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает 20 настоящих браузеров, ~3 мин; cargo test --lib live_twenty -- --ignored --nocapture --test-threads=1"]
async fn live_twenty_profiles() {
    let slot = std::env::var_os("OMBRA_PROBE_ENGINES").map(PathBuf::from).unwrap_or_else(|| Paths::new(Paths::default_root()).engines());
    let engine = Arc::new(OmbraEngine::new(slot));
    engine.installed().expect("движок не импортирован: Ombra → Настройки → Движок (или положите Chromium.app в engines/)");
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::new(root.path());
    let store = Arc::new(memory_store());

    // 20 прокси с авторизацией, у каждого свой логин; каждый профиль закреплён за своим.
    let mut auths: Vec<&'static str> = Vec::new();
    let mut proxy_logs: Vec<Log> = Vec::new();
    let mut site_logs: Vec<Log> = Vec::new();
    let mut proxy_ids = Vec::new();
    let mut ids = Vec::new();
    for n in 0..TWENTY {
        let (login, password) = (format!("user{n:02}"), format!("pass{n:02}"));
        let auth: &'static str = Box::leak(B64.encode(format!("{login}:{password}")).into_boxed_str());
        // Свой сайт проверки у каждого прокси: страница профиля N дойдёт до сайта N только через прокси N.
        let (site, site_log) = spawn_check_site().await;
        let (proxy, log) = spawn_routed_proxy(auth, site).await;
        let proxy_id = store
            .create_proxy(&ProxyInput {
                kind: ProxyKind::Http,
                host: "127.0.0.1".into(),
                port: proxy.port(),
                username: Some(login),
                password: SecretField::Set(Secret::new(password)),
                note: String::new(),
                enabled: true,
            })
            .unwrap()
            .id;
        let fingerprint = generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap();
        let mut input = ProfileInput::new(format!("Двадцать {n:02}"), fingerprint.to_json());
        input.proxy_id = Some(proxy_id);
        input.start_urls = vec![format!("http://check.ombra.test/show?n={n}")];
        ids.push(store.create_profile(&input, 1).unwrap().id);
        auths.push(auth);
        proxy_logs.push(log);
        site_logs.push(site_log);
        proxy_ids.push(proxy_id);
    }
    let dirs: Vec<PathBuf> = ids.iter().map(|id| paths.profile_dir(id)).collect();
    let _cleanup: Vec<KillProfileOnPanic> = dirs.iter().cloned().map(KillProfileOnPanic).collect();

    let geo = Arc::new(GeoClient::with_endpoints(vec![(Provider::IpWhoIs, "http://geo.ombra.test/geo".into())]));
    let core = Arc::new(Core::new(paths.clone(), store.clone(), engine, geo, Arc::new(|_: Event| {}), Timings::default()));
    let server = serve(core.clone(), ApiSettings { port: 0, key: None }).await.expect("Local API не поднялся");
    let base = format!("http://{}", server.addr());
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    // 1. Все 20 — разом, через Local API.
    let began = std::time::Instant::now();
    let starts = api_get_all(&client, ids.iter().map(|id| format!("{base}/api/v1/browser/start?user_id={id}")).collect()).await;
    println!("запуск {TWENTY} профилей через Local API: {:?}", began.elapsed());
    let started = starts
        .iter()
        .filter(|answer| {
            answer.as_ref().is_some_and(|a| {
                a["code"] == 0 && a["data"]["ws"]["puppeteer"].as_str().is_some_and(|ws| ws.starts_with("ws://"))
            })
        })
        .count();
    let infos: Vec<_> = ids
        .iter()
        .map(|id| match core.supervisor.status(id) {
            ProfileStatus::Running(info) => Some(info),
            _ => None,
        })
        .collect();
    let running = infos.iter().flatten().count();
    let own_gateway = infos
        .iter()
        .zip(&proxy_ids)
        .filter(|(info, proxy_id)| {
            info.as_ref().is_some_and(|info| info.proxy_id == **proxy_id && info.timezone == "Asia/Tokyo")
        })
        .count();
    let listed = api_get(client.clone(), format!("{base}/api/v1/browser/local-active"))
        .await
        .and_then(|a| a["data"]["list"].as_array().map(Vec::len))
        .unwrap_or(0);

    // 2. Стартовая страница каждого пришла на сайт проверки, и через его прокси: в журнале прокси — сайт и его логин.
    let through_own_proxy = |n: usize| {
        let page = format!("/show?n={n}");
        let header = format!("Proxy-Authorization: Basic {}\r\n", auths[n]);
        site_logs[n].lock().unwrap().contains(&page)
            && (0..TWENTY).filter(|&m| m != n).all(|m| !site_logs[m].lock().unwrap().contains(&page))
            && proxy_logs[n].lock().unwrap().iter().any(|head| head.contains("check.ombra.test") && head.contains(&header))
    };
    eventually(Duration::from_secs(60), || (0..TWENTY).all(through_own_proxy)).await;
    let pages = (0..TWENTY).filter(|&n| through_own_proxy(n)).count();

    // 3. 60 с жизни: тот же процесс браузера, статус «Работает».
    let mut alive = 0;
    if running == TWENTY {
        tokio::time::sleep(ALIVE_FOR).await;
        for ((id, dir), info) in ids.iter().zip(&dirs).zip(&infos) {
            let Some(info) = info else { continue };
            let same = matches!(core.supervisor.status(id), ProfileStatus::Running(now) if now.pid == info.pid);
            if same && find_profile_processes(dir).await.unwrap().contains(&info.pid) {
                alive += 1;
            }
        }
    }
    let rss_mb = browsers_rss_mb(&dirs).await;

    // 4. Остановка всех через Local API: ни процессов браузера, ни слушающих прослоек, ни PID-файлов.
    let stops = api_get_all(&client, ids.iter().map(|id| format!("{base}/api/v1/browser/stop?user_id={id}")).collect()).await;
    let stop_answers = stops.iter().filter(|answer| answer.as_ref().is_some_and(|a| a["code"] == 0)).count();
    let stopped = ids.iter().filter(|id| core.supervisor.status(id) == ProfileStatus::Stopped).count();
    let mut leftover = 0;
    for dir in &dirs {
        leftover += find_profile_processes(dir).await.unwrap().len();
    }
    for info in infos.iter().flatten() {
        if !port_closes(SocketAddr::from(([127, 0, 0, 1], info.relay_port))).await {
            leftover += 1;
        }
    }
    let pid_files = pidfile::list(&paths.run_dir()).unwrap();
    server.shutdown();

    println!("ACCEPTANCE twenty_profiles running={running} alive_after_60s={alive} rss_mb={rss_mb} leftover_processes={leftover}");
    assert_eq!(started, TWENTY, "Local API запустил не все: {starts:?}");
    assert_eq!(running, TWENTY, "в «Работает» не все");
    assert_eq!(own_gateway, TWENTY, "не каждый профиль идёт через свой прокси с таймзоной шлюза");
    assert_eq!(listed, TWENTY, "local-active видит не все");
    assert_eq!(pages, TWENTY, "стартовая страница пришла через свой прокси не у всех");
    assert_eq!(alive, TWENTY, "через {ALIVE_FOR:?} живы не все");
    assert_eq!(stop_answers, TWENTY, "остановка через Local API ответила ошибкой: {stops:?}");
    assert_eq!(stopped, TWENTY, "после остановки не все «Остановлен»");
    assert_eq!(leftover, 0, "после остановки остались процессы браузера или слушающие прослойки");
    assert!(pid_files.is_empty(), "PID-файлы не убраны: {pid_files:?}");
}

/// Проба автозаполнения (спецификация 3А): после первого запуска в `Login Data` профиля кладётся строка,
/// при втором запуске страница с формой входа получает логин и пароль от менеджера паролей Chromium.
#[tokio::test]
#[ignore = "запускает настоящий браузер; cargo test --lib live_login_data_autofill -- --ignored --nocapture"]
async fn live_login_data_autofill() {
    use crate::engine::cdp::CdpClient;
    use crate::engine::login_data;
    // OMBRA_PROBE_ENGINES — запасной каталог слота, если основной сейчас пересобирается.
    let slot = std::env::var_os("OMBRA_PROBE_ENGINES").map(PathBuf::from).unwrap_or_else(|| Paths::new(Paths::default_root()).engines());
    let engine = OmbraEngine::new(slot);
    engine.installed().expect("движок не импортирован");
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("profile");
    let page = crate::relay::test_support::spawn_html(
        "<form action=\"/login\" method=\"post\"><input name=\"user\" type=\"text\" autocomplete=\"username\">\
         <input name=\"pass\" type=\"password\" autocomplete=\"current-password\"><button>Войти</button></form>",
    )
    .await;
    let realm = format!("http://127.0.0.1:{}/", page.port());
    let headless = std::env::var_os("OMBRA_PROBE_HEADFUL").is_none();
    let launch = || {
        let mut req = crate::engine::ombra::flags::tests::request();
        req.profile_dir = profile.clone();
        req.log_path = dir.path().join("chromium.log");
        req.headless = headless;
        req.extra_args = vec![];
        engine.build_command(&req).unwrap()
    };
    // Первый запуск: Chromium создаёт Login Data.
    let mut first = match engine.launch(launch(), crate::engine::LaunchHooks::none()).await {
        Ok(p) => p,
        Err(e) => panic!("{e:?}\n{}\n{:?}", std::fs::read_to_string(dir.path().join("chromium.log")).unwrap_or_default(), launch().args),
    };
    tokio::time::sleep(Duration::from_secs(2)).await;
    engine.stop(&mut first, Duration::from_secs(10)).await.unwrap();
    let login_data = profile.join("Default/Login Data");
    assert!(login_data.is_file(), "Chromium не создал Login Data");
    {
        let conn = rusqlite::Connection::open(&login_data).unwrap();
        let mut st = conn.prepare("SELECT name, type FROM pragma_table_info('logins')").unwrap();
        let cols: Vec<String> =
            st.query_map([], |r| Ok(format!("{} {}", r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap().map(Result::unwrap).collect();
        println!("logins: {}", cols.join(", "));
    }
    login_data::upsert(&login_data, &login_data::Credential { realm: &realm, username: "ann", password: "p@ss", now: 1_700_000_000 })
        .unwrap();
    // Закладка Ombra без `checksum` (как её пишет `write_ombra_folder`) — Chromium должен принять файл.
    let marks = [crate::store::bookmarks::Bookmark { title: "Проба".into(), url: "https://bookmark.ombra.test/".into() }];
    crate::engine::bookmarks::write_ombra_folder(&profile, &marks).unwrap();
    // Второй запуск: открыть форму и прочитать поля после автозаполнения.
    let mut second = engine.launch(launch(), crate::engine::LaunchHooks::none()).await.unwrap();
    let mut cdp = CdpClient::connect(&second.ws_endpoint).await.unwrap();
    cdp.open_start_tabs(std::slice::from_ref(&realm)).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let values = cdp
        .evaluate(
            "JSON.stringify([document.forms[0].user.value, document.forms[0].pass.value, \
             document.forms[0].pass.matches(':autofill'), document.forms[0].user.matches(':autofill')])",
        )
        .await
        .unwrap();
    engine.stop(&mut second, Duration::from_secs(10)).await.unwrap();
    // Chromium при загрузке принял файл без контрольной суммы и при выходе пересохранил: папка и закладка на месте,
    // а `checksum` в файле появилась (сам Chromium её посчитал).
    let saved = std::fs::read_to_string(profile.join("Default/Bookmarks")).unwrap();
    println!("BOOKMARKS после запуска: {saved}");
    assert!(saved.contains("Ombra") && saved.contains("https://bookmark.ombra.test/"), "Chromium потерял папку Ombra: {saved}");
    assert!(saved.contains("\"checksum\""), "Chromium не пересохранил Bookmarks: {saved}");
    println!("АВТОЗАПОЛНЕНИЕ [user, pass, pass:autofill, user:autofill]: {values}");
    // Chromium прячет значения подставленных паролей от скриптов страницы до жеста пользователя: поля пусты, но
    // `:autofill` истинно — менеджер паролей ключ расшифровал и поля заполнил. Значения проверяются, если открыты.
    let (user, pass, pass_filled, user_filled): (String, String, bool, bool) =
        serde_json::from_str(values.as_str().unwrap()).unwrap();
    assert!(pass_filled && user_filled, "менеджер паролей не подставил значения: {values}");
    assert!((user.is_empty() || user == "ann") && (pass.is_empty() || pass == "p@ss"), "подставлено не то: {values}");
}

/// Проба 3Б: распакованные расширения грузятся флагом `--load-extension` (три папки через запятую), идентификатор
/// берётся из поля `key` манифеста — в том числе из синтетического ключа (32 байта, не RSA). MV2 — печатается,
/// загрузилось ли: от этого зависит правило импорта.
#[tokio::test]
#[ignore = "запускает настоящий браузер; cargo test --lib live_extensions_probe -- --ignored --nocapture"]
async fn live_extensions_probe() {
    use crate::engine::cdp::CdpClient;
    use crate::extensions::id::extension_id;
    let slot = std::env::var_os("OMBRA_PROBE_ENGINES").map(PathBuf::from).unwrap_or_else(|| Paths::new(Paths::default_root()).engines());
    let engine = OmbraEngine::new(slot);
    engine.installed().expect("движок не импортирован");
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, manifest: serde_json::Value, file: (&str, &str)| {
        let path = dir.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("manifest.json"), manifest.to_string()).unwrap();
        std::fs::write(path.join(file.0), file.1).unwrap();
        path
    };
    let (key_a, key_b, key_mv2) = ([7u8; 32], [9u8; 32], [5u8; 32]);
    let worker = ("sw.js", "chrome.runtime.onInstalled.addListener(() => {});");
    let a = write("a", serde_json::json!({"manifest_version": 3, "name": "Ombra probe A", "version": "1.0",
        "key": B64.encode(key_a), "background": {"service_worker": "sw.js"}}), worker);
    let b = write("b", serde_json::json!({"manifest_version": 3, "name": "Ombra probe B", "version": "1.0",
        "key": B64.encode(key_b), "background": {"service_worker": "sw.js"}}), worker);
    let mv2 = write("c", serde_json::json!({"manifest_version": 2, "name": "Ombra probe MV2", "version": "1.0",
        "key": B64.encode(key_mv2), "background": {"scripts": ["bg.js"], "persistent": true}}), ("bg.js", ""));
    let mut req = crate::engine::ombra::flags::tests::request();
    req.profile_dir = dir.path().join("profile");
    req.log_path = dir.path().join("chromium.log");
    req.headless = std::env::var_os("OMBRA_PROBE_HEADFUL").is_none();
    req.extra_args = vec![format!("--load-extension={},{},{}", a.display(), b.display(), mv2.display())];
    let mut browser = engine.launch(engine.build_command(&req).unwrap(), crate::engine::LaunchHooks::none()).await.unwrap();
    let mut cdp = CdpClient::connect(&browser.ws_endpoint).await.unwrap();
    let (id_a, id_b, id_mv2) = (extension_id(&key_a), extension_id(&key_b), extension_id(&key_mv2));
    let has = |urls: &[String], id: &str| urls.iter().any(|u| u.starts_with(&format!("chrome-extension://{id}/")));
    // Service worker стартует не сразу после запуска: до 10 с.
    let mut urls = Vec::new();
    for _ in 0..20 {
        urls = cdp.target_urls().await.unwrap();
        if has(&urls, &id_a) && has(&urls, &id_b) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    engine.stop(&mut browser, Duration::from_secs(10)).await.unwrap();
    println!("ЦЕЛИ: {urls:#?}");
    println!("MV2 ЗАГРУЖЕНО: {}", has(&urls, &id_mv2));
    let prefs = std::fs::read_to_string(req.profile_dir.join("Default/Preferences")).unwrap_or_default();
    println!("В Preferences: A {}, B {}, MV2 {}", prefs.contains(&id_a), prefs.contains(&id_b), prefs.contains(&id_mv2));
    assert!(has(&urls, &id_a) && has(&urls, &id_b), "расширения с ключом не загрузились: {urls:?}");
}

/// Папки из `LaunchRequest.extensions` грузятся (флаг собирает `build_args`); следующий запуск без них — расширения нет.
#[tokio::test]
#[ignore = "запускает настоящий браузер; cargo test --lib live_launch_loads_set_extensions -- --ignored --nocapture"]
async fn live_launch_loads_set_extensions() {
    use crate::engine::cdp::CdpClient;
    use crate::extensions::id::extension_id;
    let slot = std::env::var_os("OMBRA_PROBE_ENGINES").map(PathBuf::from).unwrap_or_else(|| Paths::new(Paths::default_root()).engines());
    let engine = OmbraEngine::new(slot);
    engine.installed().expect("движок не импортирован");
    let dir = tempfile::tempdir().unwrap();
    let ext = dir.path().join("ext");
    std::fs::create_dir_all(&ext).unwrap();
    let key = [3u8; 32];
    std::fs::write(ext.join("manifest.json"), serde_json::json!({"manifest_version": 3, "name": "Set", "version": "1.0",
        "key": B64.encode(key), "background": {"service_worker": "sw.js"}}).to_string()).unwrap();
    std::fs::write(ext.join("sw.js"), "chrome.runtime.onInstalled.addListener(() => {});").unwrap();
    let id = extension_id(&key);
    let mut req = crate::engine::ombra::flags::tests::request();
    req.profile_dir = dir.path().join("profile");
    req.log_path = dir.path().join("chromium.log");
    req.headless = std::env::var_os("OMBRA_PROBE_HEADFUL").is_none();
    req.extra_args = vec![];
    let loaded = |urls: &[String]| urls.iter().any(|u| u.starts_with(&format!("chrome-extension://{id}/")));
    let mut seen = Vec::new();
    for (extensions, expected) in [(vec![ext.clone()], true), (vec![], false)] {
        req.extensions = extensions;
        let mut browser = engine.launch(engine.build_command(&req).unwrap(), crate::engine::LaunchHooks::none()).await.unwrap();
        let mut cdp = CdpClient::connect(&browser.ws_endpoint).await.unwrap();
        let mut urls = Vec::new();
        for _ in 0..10 {
            urls = cdp.target_urls().await.unwrap();
            if loaded(&urls) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        engine.stop(&mut browser, Duration::from_secs(10)).await.unwrap();
        seen.push((expected, loaded(&urls)));
    }
    assert_eq!(seen, vec![(true, true), (false, false)], "первый запуск с набором, второй без");
}

/// Ручные настройки отпечатка 3А доходят до страницы: масштаб, точки касания, квота хранилища, геолокация.
#[tokio::test]
#[ignore = "запускает настоящий браузер; cargo test --lib live_fingerprint_settings_reach_the_page -- --ignored --nocapture"]
async fn live_fingerprint_settings_reach_the_page() {
    use crate::engine::cdp::CdpClient;
    use crate::fingerprint::Location;
    let slot = std::env::var_os("OMBRA_PROBE_ENGINES").map(PathBuf::from).unwrap_or_else(|| Paths::new(Paths::default_root()).engines());
    let engine = OmbraEngine::new(slot);
    engine.installed().expect("движок не импортирован");
    let dir = tempfile::tempdir().unwrap();
    // Геолокация доступна только защищённой странице: адрес 127.0.0.1 ею считается.
    let page = crate::relay::test_support::spawn_html("<!doctype html><title>fp</title>").await;
    let origin = format!("http://127.0.0.1:{}", page.port());
    let mut req = crate::engine::ombra::flags::tests::request();
    req.profile_dir = dir.path().join("profile");
    req.log_path = dir.path().join("chromium.log");
    req.headless = std::env::var_os("OMBRA_PROBE_HEADFUL").is_none();
    req.extra_args = vec![];
    let fp = &mut req.resolved.fingerprint;
    fp.device_pixel_ratio = 3;
    fp.max_touch_points = 5;
    fp.storage_quota_mb = Some(5000);
    fp.location = Location::Manual { lat: 52.52, lon: 13.405 };
    let _guard = KillProfileOnPanic(req.profile_dir.clone());
    let mut browser = engine.launch(engine.build_command(&req).unwrap(), crate::engine::LaunchHooks::none()).await.unwrap();
    let mut cdp = CdpClient::connect(&browser.ws_endpoint).await.unwrap();
    cdp.open_start_tabs(&[format!("{origin}/")]).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    cdp.send("Browser.grantPermissions", json!({ "permissions": ["geolocation"], "origin": origin })).await.unwrap();
    let seen = cdp
        .evaluate(
            "(async () => JSON.stringify({ dpr: devicePixelRatio, touch: navigator.maxTouchPoints, \
             quota: (await navigator.storage.estimate()).quota, \
             geo: await new Promise((done) => navigator.geolocation.getCurrentPosition( \
               (p) => done([p.coords.latitude, p.coords.longitude]), \
               (e) => done(['error', e.code, e.message]), { timeout: 5000 })) }))()",
        )
        .await
        .unwrap();
    engine.stop(&mut browser, Duration::from_secs(10)).await.unwrap();
    println!("ОТПЕЧАТОК НА СТРАНИЦЕ: {seen}");
    let seen: serde_json::Value = serde_json::from_str(seen.as_str().unwrap()).unwrap();
    assert_eq!(seen["dpr"], json!(3), "масштаб: {seen}");
    assert_eq!(seen["touch"], json!(5), "точки касания: {seen}");
    assert_eq!(seen["quota"], json!(5_242_880_000u64), "квота 5000 МиБ в байтах: {seen}");
    assert_eq!(seen["geo"], json!([52.52, 13.405]), "геолокация: {seen}");
}

/// Живая лицензия (стенд `ombra-lk`, браузер с патчем 980). Ключ — только в окружении: `OMBRA_LICENSE_KEY="$(cat
/// …/.test-license.key)" OMBRA_PROBE_ENGINES=<каталог со ссылкой Chromium.app> cargo test --lib live_license --
/// --ignored --nocapture --test-threads=1`. Лицензия на один поток.
fn license_timings() -> Timings {
    Timings { license_poll: Duration::from_secs(1), ..Timings::default() }
}

/// Профилю нужен свой прокси (один на двоих не выдаётся): записи разные, порт общий.
async fn license_profile(live: &Live) -> String {
    let (proxy, _) = spawn_routed_proxy("dXNlcjpwYXNz", live.site).await;
    live_profile(&live.store, Some(live_proxy(&live.store, proxy.port())), "about:blank")
}

fn is_waiting(status: &ProfileStatus, state: &str) -> bool {
    matches!(status, ProfileStatus::Waiting { license, info: None, .. } if license == state)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "стенд ombra-lk + браузер 980; OMBRA_LICENSE_KEY=… cargo test --lib live_license -- --ignored --nocapture --test-threads=1"]
async fn live_license_waits_for_a_thread() {
    assert!(std::env::var_os("OMBRA_LICENSE_KEY").is_some(), "нужен OMBRA_LICENSE_KEY (лицензия на 1 поток)");
    let live = live(license_timings()).await;
    let (first, second) = (license_profile(&live).await, license_profile(&live).await);
    let _cleanup = [&first, &second].map(|id| KillProfileOnPanic(live.paths.profile_dir(id)));

    live.supervisor.start(&first).await.expect("первый профиль не запустился");
    assert!(matches!(live.supervisor.status(&first), ProfileStatus::Running(_)));
    // До первого ответа сервера браузер говорит `offline`: обычный запуск не мелькает «ждёт поток».
    let first_events: Vec<ProfileStatus> = live
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::Status { profile_id, status } if *profile_id == first => Some(status.clone()),
            _ => None,
        })
        .collect();
    println!("первый профиль: {first_events:?}");
    assert!(matches!(first_events.as_slice(), [ProfileStatus::Starting, ProfileStatus::Running(_)]), "{first_events:?}");

    // Второй занимает вызов запуска, пока ждёт поток.
    let waiting = tokio::spawn({
        let (supervisor, id) = (live.supervisor.clone(), second.clone());
        async move { supervisor.start(&id).await }
    });
    let status = wait_status(&live.supervisor, &second, |s| is_waiting(s, "no_slot")).await;
    println!("второй профиль: {status:?}");
    assert!(!waiting.is_finished(), "запуск ждёт поток, а не кончился");

    let began = std::time::Instant::now();
    live.supervisor.stop(&first).await.unwrap();
    let running = tokio::time::timeout(Duration::from_secs(30), waiting).await.expect("второй не запустился за 30 с");
    running.unwrap().expect("второй профиль не запустился");
    println!("второй стал running через {:?} после остановки первого", began.elapsed());
    assert!(matches!(live.supervisor.status(&second), ProfileStatus::Running(_)));

    live.supervisor.stop(&second).await.unwrap();
    for id in [&first, &second] {
        assert!(find_profile_processes(&live.paths.profile_dir(id)).await.unwrap().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "стенд ombra-lk + браузер 980; OMBRA_LICENSE_KEY=… cargo test --lib live_license -- --ignored --nocapture --test-threads=1"]
async fn live_license_stop_while_waiting() {
    assert!(std::env::var_os("OMBRA_LICENSE_KEY").is_some(), "нужен OMBRA_LICENSE_KEY (лицензия на 1 поток)");
    let live = live(license_timings()).await;
    let (first, second) = (license_profile(&live).await, license_profile(&live).await);
    let _cleanup = [&first, &second].map(|id| KillProfileOnPanic(live.paths.profile_dir(id)));

    live.supervisor.start(&first).await.expect("первый профиль не запустился");
    let waiting = tokio::spawn({
        let (supervisor, id) = (live.supervisor.clone(), second.clone());
        async move { supervisor.start(&id).await }
    });
    wait_status(&live.supervisor, &second, |s| is_waiting(s, "no_slot")).await;

    let began = std::time::Instant::now();
    live.supervisor.stop(&second).await.unwrap();
    let took = began.elapsed();
    println!("«Стоп» на ждущем профиле: {took:?}");
    assert!(took < Duration::from_secs(2), "«Стоп» занял {took:?}");
    assert_eq!(live.supervisor.status(&second), ProfileStatus::Stopped);
    assert!(waiting.await.unwrap().is_err(), "отменённый запуск не должен кончиться успехом");
    assert!(find_profile_processes(&live.paths.profile_dir(&second)).await.unwrap().is_empty(), "процессы ждущего профиля остались");

    live.supervisor.stop(&first).await.unwrap();
    assert!(find_profile_processes(&live.paths.profile_dir(&first)).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "стенд ombra-lk + браузер 980; cargo test --lib live_license -- --ignored --nocapture --test-threads=1"]
async fn live_license_no_key() {
    // Браузер наследует окружение теста: ключа в нём нет, а HOME пуст (файла ~/.ombra/license.key тоже нет).
    // Тест один меняет окружение (--test-threads=1); после него всё возвращается.
    let home = tempfile::tempdir().unwrap();
    let saved: Vec<_> = ["OMBRA_LICENSE_KEY", "HOME", "CFFIXED_USER_HOME"].map(|k| (k, std::env::var_os(k))).into();
    unsafe {
        std::env::remove_var("OMBRA_LICENSE_KEY");
        std::env::set_var("HOME", home.path());
        std::env::set_var("CFFIXED_USER_HOME", home.path());
    }
    struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                unsafe {
                    match v {
                        Some(v) => std::env::set_var(k, v),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }
    let _restore = Restore(saved);

    let live = live(license_timings()).await;
    let id = license_profile(&live).await;
    let _cleanup = KillProfileOnPanic(live.paths.profile_dir(&id));
    let waiting = tokio::spawn({
        let (supervisor, id) = (live.supervisor.clone(), id.clone());
        async move { supervisor.start(&id).await }
    });
    let status = wait_status(&live.supervisor, &id, |s| is_waiting(s, "no_key")).await;
    println!("профиль без ключа: {status:?}");
    live.supervisor.stop(&id).await.unwrap();
    assert!(waiting.await.unwrap().is_err());
    assert!(find_profile_processes(&live.paths.profile_dir(&id)).await.unwrap().is_empty());
}
