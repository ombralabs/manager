//! Живые тесты процессов (`#[ignore]`): настоящий движок, прослойка и локальный сайт-фикстура через маршрутизирующий
//! прокси (`*.ombra.test`, `frames.other.test` и `frames.third.test` — на фикстуру).
//! Запуск: `cargo test --lib flow::live -- --ignored --nocapture --test-threads=1`.
use crate::app::Core;
use crate::engine::Engine;
use crate::engine::ombra::OmbraEngine;
use crate::engine::page::Browser;
use crate::engine::process::find_profile_processes;
use crate::fingerprint::{Arch, Catalog, OsRandom, generate};
use crate::flow::model::parse;
use crate::flow::run::{RunRequest, RunStatus, run_once};
use crate::paths::Paths;
use crate::relay::geo::{GeoClient, Provider};
use crate::relay::test_support::{read_head, spawn_routed_proxy_with};
use crate::store::profiles::ProfileInput;
use crate::store::proxies::{ProxyInput, ProxyKind};
use crate::store::secrets::{Secret, SecretField};
use crate::store::tests::memory_store;
use crate::supervisor::Timings;
use crate::supervisor::live_tests::KillProfileOnPanic;
use serde_json::{Map, json};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const GEO_JSON: &str = r#"{"ip":"198.51.100.77","success":true,"country_code":"JP","timezone":{"id":"Asia/Tokyo"}}"#;

/// Журнал событий страницы в атрибуте `data-log` корня: тип, id цели, isTrusted.
const LOGGER: &str = "<script>for (const type of ['click', 'dblclick', 'mouseover', 'keydown', 'input', 'change', 'focus']) \
document.addEventListener(type, (e) => { const root = document.documentElement; \
root.dataset.log = (root.dataset.log || '') + type + ':' + (e.target.id || e.target.tagName) + ':' + e.isTrusted + ' '; }, true);</script>";
const FORM_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Форма</title><input id=q><textarea id=t></textarea>\
<select id=s><option value=a>A</option><option value=b>B</option></select><button id=b>Нажми</button>\
<div id=h style=\"width:200px;height:50px\">Наведи</div>";
const LIST_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Список</title><ul id=u><li>один</li><li>два</li><li><span>три</span></li></ul>\
<p>Текст <b>Войти</b></p><p style=\"display:none\">Войти</p><button id=hb>Login<span style=\"display:none\">Login</span></button><button id=dc><span style=\"display:contents\"><b>Сквозной</b></span></button><script>setTimeout(() => { const d = document.createElement('div'); \
d.id = 'late'; d.textContent = 'Поздно'; document.body.append(d); }, 300)</script>";
const TALL_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Длинная</title><div style=\"height:6000px\">верх</div>\
<div id=box style=\"height:100px;overflow:auto\"><div style=\"height:1000px\">внутри</div></div>";
const FRAMES_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Фреймы</title>\
<iframe id=same src=\"/inner\" style=\"width:300px;height:150px\"></iframe>\
<iframe id=cross src=\"http://frames.other.test/inner\" style=\"width:300px;height:150px\"></iframe>";
const INNER_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Внутри</title><button id=ib>Внутри</button><input id=ii>";
const UPLOAD_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Файл</title><input id=f type=file>\
<script>document.querySelector('#f').addEventListener('change', (e) => { const f = e.target.files[0]; \
document.documentElement.dataset.file = f.name + ':' + f.size; });</script>";
const REQUESTS_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Запросы</title><button id=go>Данные</button>\
<script>setTimeout(() => fetch('/api/ping?k=7', { method: 'POST', body: 'hello' }), 800); \
document.querySelector('#go').addEventListener('click', () => fetch('/api/data'));</script>";
/// Страница входа примера `tools/flows/examples/login-csv.json`: форма уходит на `/welcome?user=…&pass=…`.
const LOGIN_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Вход</title><form action=/welcome>\
<input id=user name=user><input id=pass name=pass type=password><button id=go>Войти</button></form>";
const COPY_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Буфер</title><input id=src value=\"буфер\"><input id=dst>";

/// Кнопка в 20,20 размером 120×40 спрашивает `confirm` и пишет ответ в `data-answer` корня; поле в 20,80 размером
/// 120×30 спрашивает на нажатии клавиши, ответ — в `data-key`.
const SYNC_DIALOG: &str = "<!doctype html><meta charset=utf-8><title>Диалог</title>\
<button id=d style=\"position:absolute;left:20px;top:20px;width:120px;height:40px\">Спросить</button>\
<input id=k style=\"position:absolute;left:20px;top:80px;width:120px;height:30px\"><script>const root = document.documentElement; \
document.querySelector('#d').addEventListener('click', () => { root.dataset.answer = confirm('Да?'); }); \
document.querySelector('#k').addEventListener('keydown', () => { root.dataset.key = confirm('Ещё?'); });</script>";
/// `alert` при загрузке, после него — `data-loaded`.
const SYNC_ALERT: &str = "<!doctype html><meta charset=utf-8><title>Алерт</title>\
<script>alert('Привет'); document.documentElement.dataset.loaded = 'да';</script>";
/// Кнопки страницы: «Назад» (`history.back()`) в 20,20 и «Обновить» (`location.reload()`) в 20,80, обе 120×40;
/// загрузки вкладки считает `sessionStorage.loads`.
const SYNC_HISTORY: &str = "<!doctype html><meta charset=utf-8><title>История</title>\
<button id=back onclick=\"history.back()\" style=\"position:absolute;left:20px;top:20px;width:120px;height:40px\">Назад</button>\
<button id=reload onclick=\"location.reload()\" style=\"position:absolute;left:20px;top:80px;width:120px;height:40px\">Обновить</button>\
<script>sessionStorage.loads = +(sessionStorage.loads || 0) + 1;</script>";
/// Ползунок, как у noUiSlider и Radix: полоса в 20,20 размером 300×40, значение — точка от её левого края в `data-value`
/// корня; движение без нажатой кнопки кончает перетаскивание. `preventDefault` нажатия гасит mousedown и mousemove.
const SYNC_DRAG: &str = "<!doctype html><meta charset=utf-8><title>Ползунок</title>\
<div id=track style=\"position:absolute;left:20px;top:20px;width:300px;height:40px;background:#ddd\"></div>\
<script>const root = document.documentElement; let drag = false; \
document.querySelector('#track').addEventListener('pointerdown', (e) => { e.preventDefault(); drag = true; root.dataset.value = e.clientX - 20; }); \
document.addEventListener('pointermove', (e) => { if (!drag) return; if (e.buttons === 0) { drag = false; return; } \
root.dataset.value = e.clientX - 20; }); document.addEventListener('pointerup', () => { drag = false; });</script>";
/// Приложение одной страницы: ссылки «Один» в 20,20 и «Два» в 20,80 меняют адрес `pushState` (`/sync-spa/1`,
/// `/sync-spa/2`), «Назад» в 20,140 — `history.back()`; все 120×40.
const SYNC_SPA: &str = "<!doctype html><meta charset=utf-8><title>Приложение</title>\
<style>#one,#two,#back{position:absolute;left:20px;width:120px;height:40px}#one{top:20px}#two{top:80px}#back{top:140px}</style>\
<a id=one href=/sync-spa/1>Один</a><a id=two href=/sync-spa/2>Два</a><button id=back onclick=\"history.back()\">Назад</button>\
<script>for (const a of document.querySelectorAll('a')) a.addEventListener('click', (e) => { e.preventDefault(); \
history.pushState(null, '', a.href); });</script>";
/// «Обновить» в 20,20 размером 120×40: `location.reload()`, а прежний документ, пока идёт загрузка, ещё зовёт
/// `replaceState`; загрузки считает `sessionStorage.loads`. Без `fast` в адресе сервер отвечает через секунду.
const SYNC_SLOW: &str = "<!doctype html><meta charset=utf-8><title>Медленно</title>\
<button id=reload onclick=\"location.reload(); setTimeout(() => history.replaceState(null, '', location.href), 300)\" \
style=\"position:absolute;left:20px;top:20px;width:120px;height:40px\">Обновить</button>\
<script>sessionStorage.loads = +(sessionStorage.loads || 0) + 1;</script>";
/// Поле выбора файла в 20,20 размером 120×40.
const SYNC_FILE: &str = "<!doctype html><meta charset=utf-8><title>Файл</title>\
<input id=f type=file style=\"position:absolute;left:20px;top:20px;width:120px;height:40px\">";

/// Страница фрейма синхронизатора: кнопка `#d` спрашивает `confirm` (ответ — в `data-answer` корня), поле `#q`, список
/// `#s` (как у `/sync`) и поле выбора файла `#f`.
const SYNC_INNER: &str = "<!doctype html><meta charset=utf-8><title>Фрейм</title><button id=d>Спросить</button><input id=q>\
<select id=s><option value=a>Альфа</option><option value=b>Бета</option></select><input id=f type=file>\
<script>document.querySelector('#d').addEventListener('click', () => { document.documentElement.dataset.answer = confirm('Да?'); });</script>";
/// Страница с фреймом `#cross` чужого сайта на `/sync-inner`.
const SYNC_FRAMES: &str = "<!doctype html><meta charset=utf-8><title>Фрейм чужого сайта</title>\
<iframe id=cross src=\"http://frames.other.test/sync-inner\" style=\"width:500px;height:200px\"></iframe>";

/// Страница синхронизатора: `first` — элемент `#a` или `#x` в 20,20 размером 120×40; под ним поле, список, ссылка и
/// ссылка в новую вкладку.
fn sync_page(first: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>Синхронизатор</title><style>#a,#x,#q,#s,#go,#blank{{position:absolute;left:20px;\
width:120px;height:30px}}#a,#x{{top:20px;height:40px}}#q{{top:80px}}#s{{top:130px}}#go{{top:180px}}#blank{{top:230px}}</style>{first}\
<input id=q><select id=s><option value=a>Альфа</option><option value=b>Бета</option></select><a id=go href=/a>Дальше</a>\
<a id=blank href=/b target=_blank>Вкладка</a>{LOGGER}"
    )
}

/// Сайт-фикстура: `/geo` — подставной геосервис, `/a` и `/b` — страницы с заголовками, `/js` — страница со своим
/// объектом, `/form` — поля, список и кнопка с журналом событий, `/list` — список, текст и элемент, который появится
/// позже, `/tall` — длинная страница с прокручиваемым блоком, `/frames` — фреймы того же и чужого сайта, `/inner` —
/// страница фрейма с журналом событий, `/upload` — поле выбора файла, `/requests` — запросы страницы (`/api/ping` сам
/// через 800 мс, `/api/data` сразу по кнопке), `/copy` — поля для ⌘C и ⌘V, `/file.txt` — файл для Download File, `/login` и `/welcome` — вход примера,
/// `/sync`, `/sync-other` (на месте кнопки — ссылка) и `/sync-price` (другая цена) — страницы синхронизатора,
/// `/sync-dialog` — `confirm` на клике и на клавише, `/sync-alert` — `alert` при загрузке, `/sync-file` — поле выбора
/// файла, `/sync-drag` — ползунок, `/sync-spa…` — приложение одной страницы, `/sync-slow…` — медленное обновление,
/// `/sync-history` — назад и обновление самой страницей, `/sync-500…` — ответ 500, `/sync-frames` — фрейм чужого сайта с `/sync-inner` (диалог, поле, список и
/// выбор файла во фрейме).
pub(crate) async fn spawn_site() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { continue };
            tokio::spawn(async move {
                let head = String::from_utf8_lossy(&read_head(&mut stream).await).into_owned();
                let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                // Через HTTP-прокси путь приходит в абсолютной форме.
                let path = target.strip_prefix("http://").and_then(|rest| rest.find('/').map(|i| rest[i..].to_string())).unwrap_or(target);
                let html = "text/html; charset=utf-8";
                let (kind, body) = match path.as_str() {
                    p if p.starts_with("/geo") => ("application/json", GEO_JSON.to_string()),
                    "/a" => (html, "<!doctype html><title>Страница A</title><a href=/b>b</a>".to_string()),
                    "/b" => (html, "<!doctype html><title>Страница B</title>".to_string()),
                    "/js" => (html, "<!doctype html><title>JS</title><script>window.siteValue = 41</script>".to_string()),
                    "/form" => (html, format!("{FORM_PAGE}{LOGGER}")),
                    "/list" => (html, LIST_PAGE.to_string()),
                    "/tall" => (html, TALL_PAGE.to_string()),
                    "/frames" => (html, FRAMES_PAGE.to_string()),
                    "/inner" => (html, format!("{INNER_PAGE}{LOGGER}")),
                    "/upload" => (html, UPLOAD_PAGE.to_string()),
                    "/requests" => (html, REQUESTS_PAGE.to_string()),
                    "/copy" => (html, COPY_PAGE.to_string()),
                    "/sync" => (html, sync_page("<button id=a>Купить за 100</button>")),
                    "/sync-other" => (html, sync_page("<a id=x href=#>Подробнее</a>")),
                    "/sync-price" => (html, sync_page("<button id=a>Купить за 90</button>")),
                    "/sync-dialog" => (html, SYNC_DIALOG.to_string()),
                    "/sync-alert" => (html, SYNC_ALERT.to_string()),
                    "/sync-file" => (html, SYNC_FILE.to_string()),
                    "/sync-drag" => (html, SYNC_DRAG.to_string()),
                    p if p.starts_with("/sync-spa") => (html, SYNC_SPA.to_string()),
                    p if p.starts_with("/sync-slow") => {
                        if !p.contains("fast") {
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                        (html, SYNC_SLOW.to_string())
                    }
                    "/sync-history" => (html, SYNC_HISTORY.to_string()),
                    "/sync-frames" => (html, SYNC_FRAMES.to_string()),
                    "/sync-inner" => (html, format!("{SYNC_INNER}{LOGGER}")),
                    p if p.starts_with("/sync-500") => (html, "<!doctype html><meta charset=utf-8><title>Ошибка</title>Сбой сервера".to_string()),
                    "/file.txt" => ("text/plain; charset=utf-8", "содержимое файла".to_string()),
                    "/api/data" => ("application/json", r#"{"ok":1}"#.to_string()),
                    p if p.starts_with("/api/ping") => ("text/plain", "pong".to_string()),
                    "/login" => (html, LOGIN_PAGE.to_string()),
                    p if p.starts_with("/welcome") => {
                        let user = p.split_once("user=").map(|(_, rest)| rest.split('&').next().unwrap_or_default()).unwrap_or_default();
                        (html, format!("<!doctype html><meta charset=utf-8><title>Кабинет</title><p id=hello>Здравствуйте, {user}</p>"))
                    }
                    _ => ("text/plain", String::new()),
                };
                let status = if path.starts_with("/sync-500") { "500 Internal Server Error" } else { "200 OK" };
                let response =
                    format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    addr
}

pub(crate) struct Live {
    pub(crate) core: Core<OmbraEngine>,
    profile: String,
    /// Профиль с выключенным очеловечиванием: ввод — посимвольно с паузами процесса.
    pub(crate) quiet: String,
    /// Запросы, дошедшие до внешнего прокси профиля (заголовки по порядку).
    proxy_log: crate::relay::test_support::Log,
    /// Порт маршрутизирующего прокси — для своих записей прокси новых профилей.
    pub(crate) proxy_port: u16,
    _root: tempfile::TempDir,
}

pub(crate) async fn live() -> Live {
    live_with_sink(Arc::new(|_| {})).await
}

/// То же, но события ядра уходят в `sink`.
pub(crate) async fn live_with_sink(sink: crate::supervisor::EventSink) -> Live {
    let slot = std::env::var_os("OMBRA_PROBE_ENGINES").map(std::path::PathBuf::from)
        .unwrap_or_else(|| Paths::new(Paths::default_root()).engines());
    let engine = Arc::new(OmbraEngine::new(slot));
    engine.installed().expect("движок не импортирован: Ombra → Настройки → Движок (или положите Chromium.app в engines/)");
    let root = tempfile::tempdir().unwrap();
    // Межсайтовые фреймы — с хостов вне `*.ombra.test`: их прокси ведёт на ту же фикстуру отдельными записями.
    let site = spawn_site().await;
    let (proxy, proxy_log) = spawn_routed_proxy_with("dXNlcjpwYXNz", site, vec![("frames.other.test", site), ("frames.third.test", site)]).await;
    let store = Arc::new(memory_store());
    let proxy_input = ProxyInput {
        kind: ProxyKind::Http,
        host: "127.0.0.1".into(),
        port: proxy.port(),
        username: Some("user".into()),
        password: SecretField::Set(Secret::new("pass")),
        note: String::new(),
        enabled: true,
    };
    let proxy_id = store.create_proxy(&proxy_input).unwrap().id;
    let fingerprint = generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap();
    let mut input = ProfileInput::new("Живой тест процессов", fingerprint.to_json());
    input.proxy_id = Some(proxy_id);
    let profile = store.create_profile(&input, 1).unwrap().id;
    // Прокси занимает один работающий профиль: второму (New Browser) — своя запись того же прокси.
    let quiet_proxy_id = store.create_proxy(&proxy_input).unwrap().id;
    let fingerprint = generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap();
    let mut input = ProfileInput::new("Живой тест процессов без очеловечивания", fingerprint.to_json());
    input.proxy_id = Some(quiet_proxy_id);
    input.humanize = Some(crate::humanize::Humanize { enabled: false, ..Default::default() });
    let quiet = store.create_profile(&input, 1).unwrap().id;
    let geo = Arc::new(GeoClient::with_endpoints(vec![(Provider::IpWhoIs, "http://geo.ombra.test/geo".into())]));
    let core = Core::new(Paths::new(root.path()), store, engine, geo, sink, Timings::default());
    Live { core, profile, quiet, proxy_log, proxy_port: proxy.port(), _root: root }
}

/// Живые тесты идут без окон; `OMBRA_PROBE_HEADFUL=1` показывает браузер для отладки.
fn windowless(mut flow: crate::flow::model::Flow) -> crate::flow::model::Flow {
    flow.settings.headless = std::env::var_os("OMBRA_PROBE_HEADFUL").is_none();
    flow
}

fn request<'a>(flow: &'a crate::flow::model::Flow, profile: &'a str) -> RunRequest<'a> {
    RunRequest { flow, profile_id: profile, params: Map::new(), run_id: "live".into(), cancel: CancellationToken::new(), position: 0, task: None, busy: None, on_entry: None, from: None, on_node: None }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib flow::live -- --ignored --nocapture --test-threads=1"]
async fn live_tabs_navigation_and_js() {
    let live = live().await;
    let _cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.profile));
    let flow = windowless(parse(
        r#"{
      "format": "ombra-flow/1", "name": "Вкладки, переходы, JS",
      "settings": { "after_tabs": "keep", "after_browser": "keep" },
      "variables": [{ "name": "base", "type": "string", "default": "http://flow.ombra.test" }, { "name": "n", "type": "number", "default": 1 }],
      "nodes": [
        { "id": "s", "op": "start" },
        { "id": "a", "op": "web.goto", "config": { "url": "${base}/a" } },
        { "id": "t1", "op": "web.js", "config": { "code": "document.title", "save_to": "title_a" } },
        { "id": "b", "op": "web.goto", "config": { "url": "${base}/b" } },
        { "id": "back", "op": "web.back" },
        { "id": "p1", "op": "web.js", "config": { "code": "location.pathname", "save_to": "after_back" } },
        { "id": "r", "op": "web.reload" },
        { "id": "nt", "op": "web.new_tab" },
        { "id": "j", "op": "web.goto", "config": { "url": "${base}/js" } },
        { "id": "sum", "op": "web.js", "config": { "code": "(async () => window.siteValue + n)()", "inject": ["n"], "save_to": "sum" } },
        { "id": "sw", "op": "web.switch_tab", "config": { "by": "title", "match": "eq", "value": "Страница A" } },
        { "id": "p2", "op": "web.js", "config": { "code": "location.pathname", "save_to": "switched" } },
        { "id": "nt2", "op": "web.new_tab" },
        { "id": "ct", "op": "web.close_tab" },
        { "id": "p3", "op": "web.js", "config": { "code": "location.pathname", "save_to": "after_close" } },
        { "id": "co", "op": "web.close_other_tabs" },
        { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["title_a", "after_back", "sum", "switched", "after_close"] } }
      ],
      "edges": [
        { "from": "s", "to": "a" }, { "from": "a", "to": "t1" }, { "from": "t1", "to": "b" }, { "from": "b", "to": "back" },
        { "from": "back", "to": "p1" }, { "from": "p1", "to": "r" }, { "from": "r", "to": "nt" }, { "from": "nt", "to": "j" },
        { "from": "j", "to": "sum" }, { "from": "sum", "to": "sw" }, { "from": "sw", "to": "p2" }, { "from": "p2", "to": "nt2" },
        { "from": "nt2", "to": "ct" }, { "from": "ct", "to": "p3" }, { "from": "p3", "to": "co" }, { "from": "co", "to": "x" }
      ]
    }"#,
    )
    .unwrap());
    let outcome = run_once(&live.core, request(&flow, &live.profile)).await;
    assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
    let mut exported = outcome.exported.clone();
    // Какая из оставшихся вкладок станет текущей, решает порядок `Target.getTargets` движка; важно, что не закрытая.
    let after_close = exported.remove("after_close").unwrap();
    assert!(after_close == "/a" || after_close == "/js", "текущей стала закрытая вкладка: {after_close}");
    assert_eq!(json!(exported), json!({ "title_a": "Страница A", "after_back": "/a", "sum": 42, "switched": "/a" }));
    let info = live.core.supervisor.running_info(&live.profile).expect("after_browser keep — браузер работает");
    let tabs = Browser::connect(&info.cdp_ws).await.unwrap().tabs().await.unwrap();
    assert_eq!(
        tabs.iter().map(|t| t.url.as_str()).collect::<Vec<_>>(),
        [format!("http://flow.ombra.test{}", after_close.as_str().unwrap())],
        "Close Other Tabs оставил текущую"
    );
    live.core.supervisor.stop(&live.profile).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib flow::live -- --ignored --nocapture --test-threads=1"]
async fn live_script_error_goes_to_the_error_branch_and_browser_quits() {
    let live = live().await;
    let _cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.profile));
    let flow = windowless(parse(
        r#"{
      "format": "ombra-flow/1", "name": "Ошибка скрипта",
      "nodes": [
        { "id": "s", "op": "start" },
        { "id": "e", "op": "web.js", "on_error": "branch", "config": { "code": "throw new Error('boom')" } },
        { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["last_error"] } }
      ],
      "edges": [{ "from": "s", "to": "e" }, { "from": "e", "port": "error", "to": "x" }]
    }"#,
    )
    .unwrap());
    let outcome = run_once(&live.core, request(&flow, &live.profile)).await;
    assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
    let message = outcome.exported["last_error"].as_str().unwrap();
    assert!(message.starts_with("Execute JavaScript: Error: boom"), "{message}");
    assert!(live.core.supervisor.running_info(&live.profile).is_none(), "after_browser quit");
    let dir = live.core.paths.profile_dir(&live.profile);
    assert!(find_profile_processes(&dir).await.unwrap().is_empty(), "после остановки нет процессов браузера");
}

/// Процесс: start → узлы по порядку; после него браузер закрывается. Узел с `on_error: "branch"` ведёт дальше
/// выходом `error` — следующий узел может сохранить `last_error`.
fn chain(name: &str, nodes: serde_json::Value) -> crate::flow::model::Flow {
    let list = nodes.as_array().unwrap();
    let mut all = vec![json!({ "id": "s", "op": "start" })];
    let mut edges = Vec::new();
    let mut prev = ("s".to_string(), "next");
    for node in list {
        let id = node["id"].as_str().unwrap().to_string();
        edges.push(json!({ "from": prev.0, "port": prev.1, "to": id }));
        prev = (id, if node["on_error"] == "branch" { "error" } else { "next" });
        all.push(node.clone());
    }
    windowless(
        serde_json::from_value(json!({
            "format": "ombra-flow/1", "name": name, "settings": { "after_browser": "quit" },
            "nodes": all, "edges": edges
        }))
        .unwrap(),
    )
}

fn js(id: &str, code: &str, save_to: &str) -> serde_json::Value {
    json!({ "id": id, "op": "web.js", "config": { "code": code, "save_to": save_to } })
}

fn css(selector: &str) -> serde_json::Value {
    json!({ "by": "css", "selector": selector })
}

/// Узел, который должен упасть, и следующий за ним — текст ошибки в переменную `save_to`.
fn failing(id: &str, op: &str, config: serde_json::Value, save_to: &str) -> [serde_json::Value; 2] {
    [
        json!({ "id": id, "op": op, "on_error": "branch", "config": config }),
        json!({ "id": format!("{id}_err"), "op": "proc.vars", "config": { "vars": [{ "name": save_to, "type": "string", "value": "${last_error}" }] } }),
    ]
}

async fn form_flow(live: &Live, profile: &str) -> serde_json::Map<String, serde_json::Value> {
    let flow = chain(
        "Форма",
        json!([
            { "id": "g", "op": "web.goto", "config": { "url": "http://flow.ombra.test/form" } },
            { "id": "c", "op": "web.click", "config": { "target": css("#b") } },
            { "id": "d", "op": "web.click", "config": { "target": css("#b"), "kind": "double" } },
            { "id": "h", "op": "web.hover", "config": { "target": css("#h") } },
            { "id": "i", "op": "web.input", "config": { "target": css("#q"), "content": "кофе", "clear": true, "interval_ms": 20 } },
            js("v1", "document.querySelector('#q').value", "typed"),
            { "id": "k1", "op": "key.combo", "config": { "modifiers": ["cmd"], "key": "a" } },
            { "id": "k2", "op": "key.press", "config": { "key": "Delete" } },
            js("v2", "document.querySelector('#q').value", "cleared"),
            { "id": "sel", "op": "web.select", "config": { "target": css("#s"), "value": "b" } },
            js("v3", "document.querySelector('#s').value", "chosen"),
            { "id": "f", "op": "web.focus", "config": { "target": css("#t") } },
            js("lg", "document.documentElement.dataset.log", "log"),
            { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["typed", "cleared", "chosen", "log"] } }
        ]),
    );
    let outcome = run_once(&live.core, request(&flow, profile)).await;
    assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
    outcome.exported
}

fn assert_form(exported: &serde_json::Map<String, serde_json::Value>) {
    assert_eq!(exported["typed"], "кофе");
    assert_eq!(exported["cleared"], "", "⌘A и Delete очистили поле");
    assert_eq!(exported["chosen"], "b");
    let log = exported["log"].as_str().unwrap();
    for event in ["click:b:true", "dblclick:b:true", "mouseover:h:true", "keydown:q:true", "change:s", "focus:t"] {
        assert!(log.contains(event), "нет {event} в {log}");
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib flow::live -- --ignored --nocapture --test-threads=1"]
async fn live_form_input_with_and_without_humanize() {
    let live = live().await;
    let _cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.profile));
    let _quiet = KillProfileOnPanic(live.core.paths.profile_dir(&live.quiet));
    assert_form(&form_flow(&live, &live.profile).await);
    assert_form(&form_flow(&live, &live.quiet).await);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib flow::live -- --ignored --nocapture --test-threads=1"]
async fn live_elements_text_lists_and_waits() {
    let live = live().await;
    let _cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.profile));
    let flow = chain(
        "Элементы",
        json!([
            { "id": "g", "op": "web.goto", "config": { "url": "http://flow.ombra.test/list" } },
            { "id": "e1", "op": "data.element", "config": { "target": { "by": "text", "selector": "Войти" }, "save_to": "word" } },
            { "id": "e2", "op": "data.element", "config": { "target": { "by": "css", "selector": "li", "index": 2 }, "kind": "html", "save_to": "second" } },
            { "id": "e3", "op": "data.element", "config": { "target": css("#u"), "kind": "child", "child_selector": "span", "save_to": "span" } },
            { "id": "e4", "op": "data.element", "config": { "target": { "element": "${span}" }, "save_to": "three" } },
            { "id": "e5", "op": "data.element", "config": { "target": { "by": "xpath", "selector": "//li[1]" }, "save_to": "first" } },
            // Скрытый потомок с тем же текстом не отнимает совпадение у видимой кнопки.
            { "id": "e6", "op": "data.element", "config": { "target": { "by": "text", "selector": "Login" }, "kind": "attribute", "attribute": "id", "save_to": "login" } },
            // Обёртка без рамки (`display: contents`) не прячет подходящий потомок: находится один `<b>`, без кнопки.
            { "id": "dc", "op": "control.for_elements", "config": { "target": { "by": "text", "selector": "Сквозной" }, "kind": "html", "item_to": "dc_html", "index_to": "dc_last" } },
            { "id": "l", "op": "control.for_elements", "config": { "target": css("li"), "kind": "text", "item_to": "item", "index_to": "i" } },
            { "id": "w1", "op": "wait.element", "config": { "target": css("#late"), "visible": true, "timeout_ms": 3000, "save_to": "late" } },
            { "id": "w2", "op": "wait.element", "config": { "target": css("#never"), "timeout_ms": 300, "save_to": "never" } },
            { "id": "fo", "op": "data.focused", "config": { "save_to": "focused" } },
            // Переход посреди ожидания: прежний `#late` убран, `/list` загружается заново и через 300 мс создаёт новый —
            // дождаться можно только его.
            js("nav", "document.querySelector('#late').remove(); setTimeout(() => { location.href = '/list' }, 50); 1", "navigated"),
            { "id": "w3", "op": "wait.element", "config": { "target": css("#late"), "visible": true, "timeout_ms": 5000, "save_to": "late2" } },
            { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["word", "second", "three", "first", "login", "dc_html", "dc_last", "item", "i", "late", "never", "focused", "span", "navigated", "late2"] } }
        ]),
    );
    let outcome = run_once(&live.core, request(&flow, &live.profile)).await;
    assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
    assert_eq!(
        json!(outcome.exported),
        json!({ "word": "Войти", "second": "два", "three": "три", "first": "один", "login": "hb", "dc_html": "Сквозной", "dc_last": 0, "item": "три", "i": 2, "late": true, "never": false,
                "focused": "<element>", "span": "<element>", "navigated": 1, "late2": true })
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib flow::live -- --ignored --nocapture --test-threads=1"]
async fn live_frames_same_and_cross_site() {
    let live = live().await;
    let _cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.profile));
    let serde_json::Value::Array(mut nodes) = json!([
        { "id": "g", "op": "web.goto", "config": { "url": "http://flow.ombra.test/frames" } },
        // Межсайтовый фрейм чужого источника: его документ странице недоступен.
        js("co", "document.querySelector('#cross').contentDocument === null", "cross_origin"),
        { "id": "fs", "op": "data.element", "config": { "target": css("#same"), "kind": "iframe", "save_to": "fs" } },
        { "id": "fc", "op": "data.element", "config": { "target": css("#cross"), "kind": "iframe", "save_to": "fc" } },
        { "id": "c1", "op": "web.click", "config": { "target": { "by": "css", "selector": "#ib", "frame": "${fs}" } } },
        { "id": "c2", "op": "web.click", "config": { "target": { "by": "css", "selector": "#ib", "frame": "${fc}" } } },
        { "id": "in", "op": "web.input", "config": { "target": { "by": "css", "selector": "#ii", "frame": "${fc}" }, "content": "x", "interval_ms": 10 } },
        { "id": "v1", "op": "web.js", "config": { "code": "document.documentElement.dataset.log", "frame": "${fs}", "save_to": "same" } },
        { "id": "v2", "op": "web.js", "config": { "code": "document.documentElement.dataset.log", "frame": "${fc}", "save_to": "cross" } },
        { "id": "v3", "op": "web.js", "config": { "code": "document.querySelector('#ii').value", "frame": "${fc}", "save_to": "value" } },
        // Межсайтовый переход фрейма B→C: сессия фрейма остаётся той же (фрейм — в своём процессе), номера узлов
        // нового процесса — снова с 1. Сотня блоков во фрейме и список всех его элементов раздают номера дальше
        // номера сохранённой кнопки: без поколения документа клик ушёл бы в чужой узел.
        { "id": "fb", "op": "data.element", "config": { "target": { "by": "css", "selector": "#ib", "frame": "${fc}" }, "kind": "object", "save_to": "fb" } },
        js("bc", "document.querySelector('#cross').src = 'http://frames.third.test/list'; 1", "moved"),
        { "id": "wc", "op": "wait.element", "config": { "target": { "by": "css", "selector": "#u", "frame": "${fc}" }, "timeout_ms": 5000, "save_to": "moved_in" } },
        { "id": "many", "op": "web.js", "config": { "code": "for (let i = 0; i < 120; i++) document.body.append(document.createElement('div')); 1", "frame": "${fc}", "save_to": "made" } },
        { "id": "n", "op": "control.for_elements", "config": { "target": { "by": "css", "selector": "*", "frame": "${fc}" } } }
    ]) else {
        unreachable!()
    };
    nodes.extend(failing("fg", "web.click", json!({ "target": { "element": "${fb}" } }), "frame_gone"));
    nodes.extend([
        // Свежий поиск во фрейме — уже в новом документе.
        json!({ "id": "fl", "op": "data.element", "config": { "target": { "by": "css", "selector": "li", "index": 2, "frame": "${fc}" }, "save_to": "frame_li" } }),
        json!({ "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["cross_origin", "same", "cross", "value", "moved_in", "frame_gone", "frame_li"] } }),
    ]);
    let flow = chain("Фреймы", json!(nodes));
    let outcome = run_once(&live.core, request(&flow, &live.profile)).await;
    assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
    // Нет итога — узел с веткой ошибки не упал (клик ушёл в чужой узел) и процесс кончился раньше экспорта.
    assert!(outcome.exported.contains_key("frame_gone"), "{outcome:?}");
    assert_eq!(outcome.exported["cross_origin"], true, "фрейм #cross — чужого источника");
    assert!(outcome.exported["same"].as_str().unwrap().contains("click:ib:true"), "{outcome:?}");
    let cross = outcome.exported["cross"].as_str().unwrap();
    assert!(cross.contains("click:ib:true") && cross.contains("keydown:ii:true"), "клик и ввод попали в межсайтовый фрейм: {cross}");
    assert_eq!(outcome.exported["value"], "x");
    assert_eq!(outcome.exported["moved_in"], true, "фрейм перешёл на третий сайт");
    assert_eq!(outcome.exported["frame_gone"], "Click: Элемент больше не доступен на странице");
    assert_eq!(outcome.exported["frame_li"], "два");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib flow::live -- --ignored --nocapture --test-threads=1"]
async fn live_scroll_and_screenshot() {
    let live = live().await;
    let _cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.profile));
    let flow = chain(
        "Прокрутка",
        json!([
            { "id": "g", "op": "web.goto", "config": { "url": "http://flow.ombra.test/tall" } },
            { "id": "s1", "op": "web.scroll", "config": { "position": "bottom", "kind": "auto" } },
            js("y1", "window.scrollY", "bottom"),
            // Крупные шаги: до верха — несколько шагов колеса; после каждого позиция перечитывается до секунды, пока
            // не сдвинется (очеловеченное колесо отвечает раньше, чем страница доезжает).
            { "id": "s2", "op": "web.scroll", "config": { "position": "top", "step_px": { "min": 800, "max": 1000 }, "step_ms": { "min": 250, "max": 300 } } },
            js("y2", "window.scrollY", "top"),
            { "id": "s3", "op": "web.scroll", "config": { "area": "target", "target": css("#box"), "position": "bottom", "kind": "auto" } },
            js("b1", "document.querySelector('#box').scrollTop", "inner"),
            { "id": "shot", "op": "web.screenshot", "config": { "name": "tall", "full_page": true } },
            { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["bottom", "top", "inner"] } }
        ]),
    );
    let outcome = run_once(&live.core, request(&flow, &live.profile)).await;
    assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
    assert!(outcome.exported["bottom"].as_f64().unwrap() > 4000.0, "{outcome:?}");
    assert_eq!(outcome.exported["top"].as_f64(), Some(0.0), "колесо довело до верха");
    assert!(outcome.exported["inner"].as_f64().unwrap() > 0.0);
    let shot = live.core.paths.flow_files("live").join("tall.png");
    assert!(std::fs::metadata(&shot).map(|m| m.len() > 0).unwrap_or(false), "снимок записан: {}", shot.display());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib flow::live -- --ignored --nocapture --test-threads=1"]
async fn live_saved_elements_across_tabs_and_documents() {
    let live = live().await;
    let _cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.profile));
    // Номера узлов (`backendNodeId`) процесс рендерера раздаёт по запросу, с 1. Межсайтовый переход — новый процесс:
    // если новая страница раздала номера дальше номера сохранённого элемента, тот же номер назван чужим узлом. Списки
    // всех элементов (For Loop Elements) раздают номера каждому: на `/list` — до сохранённой кнопки, на новой
    // странице — сотне блоков. Без поколения документа клик ушёл бы в один из них.
    let mut nodes = vec![
        json!({ "id": "g1", "op": "web.goto", "config": { "url": "http://flow.ombra.test/list" } }),
        json!({ "id": "n1", "op": "control.for_elements", "config": { "target": css("*") } }),
        json!({ "id": "l", "op": "data.element", "config": { "target": css("#hb"), "kind": "object", "save_to": "el" } }),
        json!({ "id": "g2", "op": "web.goto", "config": { "url": "http://frames.other.test/frames" } }),
        js("many", "for (let i = 0; i < 120; i++) document.body.append(Object.assign(document.createElement('div'), { textContent: 'блок' })); 1", "made"),
        json!({ "id": "n2", "op": "control.for_elements", "config": { "target": css("div") } }),
        json!({ "id": "f", "op": "data.element", "config": { "target": css("iframe"), "kind": "attribute", "attribute": "id", "save_to": "frame_id" } }),
    ];
    nodes.extend(failing("c1", "web.click", json!({ "target": { "element": "${el}" } }), "gone"));
    nodes.extend([
        json!({ "id": "g3", "op": "web.goto", "config": { "url": "http://flow.ombra.test/form" } }),
        json!({ "id": "b", "op": "data.element", "config": { "target": css("#b"), "kind": "object", "save_to": "btn" } }),
        json!({ "id": "nt", "op": "web.new_tab" }),
    ]);
    // Элемент другой вкладки — ошибка; после возврата на его вкладку — снова работает.
    nodes.extend(failing("o", "web.click", json!({ "target": { "element": "${btn}" } }), "other_tab"));
    nodes.extend([
        json!({ "id": "sw", "op": "web.switch_tab", "config": { "by": "title", "match": "eq", "value": "Форма" } }),
        json!({ "id": "c2", "op": "web.click", "config": { "target": { "element": "${btn}" } } }),
        js("lg", "document.documentElement.dataset.log", "log"),
    ]);
    nodes.push(json!({ "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["other_tab", "log", "frame_id", "gone"] } }));
    let flow = chain("Сохранённые элементы", json!(nodes));
    let outcome = run_once(&live.core, request(&flow, &live.profile)).await;
    assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
    // Нет итога — узел с веткой ошибки не упал (клик ушёл в чужой узел) и процесс кончился раньше экспорта.
    let exported = |name: &str| outcome.exported.get(name).cloned().unwrap_or_else(|| panic!("нет {name}: {outcome:?}"));
    assert_eq!(exported("gone"), "Click: Элемент больше не доступен на странице");
    assert_eq!(exported("frame_id"), "same", "поиск на новой странице — в её документе");
    assert_eq!(exported("other_tab"), "Click: элемент на другой вкладке");
    assert!(exported("log").as_str().unwrap().contains("click:b:true"), "{outcome:?}");
}

/// Возвращает системный буфер обмена, каким он был до теста.
struct ClipboardGuard(Vec<u8>);

impl ClipboardGuard {
    fn save() -> Self {
        Self(std::process::Command::new("pbpaste").output().map(|out| out.stdout).unwrap_or_default())
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        set_clipboard(&self.0);
    }
}

fn set_clipboard(text: &[u8]) {
    use std::io::Write;
    if let Ok(mut child) = std::process::Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text);
        }
        let _ = child.wait();
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib flow::live -- --ignored --nocapture --test-threads=1"]
async fn live_upload_download_and_url() {
    let live = live().await;
    let _cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.profile));
    let upload = live.core.paths.flow_files("live").join("upload-src.txt");
    std::fs::create_dir_all(upload.parent().unwrap()).unwrap();
    std::fs::write(&upload, "данные").unwrap();
    let flow = chain(
        "Файлы",
        json!([
            { "id": "g", "op": "web.goto", "config": { "url": "http://flow.ombra.test/upload" } },
            { "id": "u", "op": "web.upload", "config": { "target": css("#f"), "path": upload.to_str().unwrap() } },
            js("uf", "document.documentElement.dataset.file", "uploaded"),
            { "id": "d", "op": "data.download", "config": { "url": "http://flow.ombra.test/file.txt" } },
            { "id": "g2", "op": "web.goto", "config": { "url": "http://flow.ombra.test/b?x=5" } },
            { "id": "u1", "op": "data.url", "config": { "save_to": "full" } },
            { "id": "u2", "op": "data.url", "config": { "kind": "domain", "save_to": "domain" } },
            { "id": "u3", "op": "data.url", "config": { "kind": "param", "param": "x", "save_to": "x" } },
            { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["uploaded", "full", "domain", "x"] } }
        ]),
    );
    let outcome = run_once(&live.core, request(&flow, &live.profile)).await;
    assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
    assert_eq!(outcome.exported["uploaded"], json!(format!("upload-src.txt:{}", "данные".len())));
    assert_eq!(
        (outcome.exported["full"].clone(), outcome.exported["domain"].clone(), outcome.exported["x"].clone()),
        (json!("http://flow.ombra.test/b?x=5"), json!("http://flow.ombra.test"), json!("5"))
    );
    let file = live.core.paths.flow_files("live").join("file.txt");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "содержимое файла");
    // Сам браузер `/file.txt` не запрашивает: этот запрос до внешнего прокси довела прослойка профиля.
    let log = live.proxy_log.lock().unwrap();
    let downloaded = log.iter().any(|line| line.starts_with("GET http://flow.ombra.test/file.txt "));
    assert!(downloaded, "скачивание ушло через прослойку и прокси профиля: {log:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib flow::live -- --ignored --nocapture --test-threads=1"]
async fn live_network_listeners_and_cookies() {
    let live = live().await;
    let _cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.profile));
    let flow = chain(
        "Сеть",
        json!([
            { "id": "g", "op": "web.goto", "config": { "url": "http://flow.ombra.test/requests" } },
            { "id": "l1", "op": "data.listen_request", "config": { "url_contains": "api/ping", "kind": "get_param", "param": "k", "save_to": "k" } },
            { "id": "l2", "op": "data.listen_request", "config": { "url_contains": "api/ping", "kind": "post_data", "save_to": "sent" } },
            { "id": "l3", "op": "data.listen_response", "config": { "url_contains": "api/data", "save_to": "resp" } },
            { "id": "w1", "op": "wait.request", "config": { "url_contains": "api/ping", "timeout_ms": 5000 } },
            { "id": "c", "op": "web.click", "config": { "target": css("#go") } },
            { "id": "w2", "op": "wait.request", "config": { "url_contains": "api/data", "timeout_ms": 5000 } },
            { "id": "p", "op": "wait.time", "config": { "ms": 300 } },
            { "id": "st", "op": "data.listen_stop" },
            js("ck", "document.cookie = 'flow=1; path=/'; 1", "set"),
            { "id": "cg", "op": "data.cookies_get", "config": { "save_to": "c1" } },
            { "id": "cc", "op": "data.cookies_clear" },
            { "id": "cg2", "op": "data.cookies_get", "config": { "save_to": "c2" } },
            { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["k", "sent", "resp", "c1", "c2"] } }
        ]),
    );
    let outcome = run_once(&live.core, request(&flow, &live.profile)).await;
    assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
    assert_eq!(outcome.exported["k"], json!("7"));
    assert_eq!(outcome.exported["sent"], json!("hello"));
    assert_eq!(outcome.exported["resp"], json!(r#"{"ok":1}"#));
    assert!(outcome.exported["c1"].as_array().unwrap().iter().any(|c| c["name"] == "flow"), "{outcome:?}");
    assert_eq!(outcome.exported["c2"], json!([]));
    // Request to finish по тайм-ауту идёт дальше без ошибки: дождался ли он, видно только в журнале. `/api/data`
    // начинается и кончается ещё во время клика: его ловит сеть, включённая перед кликом.
    let journal = std::fs::read_to_string(live.core.paths.flow_log("live")).unwrap();
    assert!(!journal.contains("не дождались"), "Request to finish дождался обоих запросов: {journal}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib flow::live -- --ignored --nocapture --test-threads=1"]
async fn live_new_browser_and_copy_paste() {
    let live = live().await;
    let _cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.profile));
    let _quiet = KillProfileOnPanic(live.core.paths.profile_dir(&live.quiet));
    let _clipboard = ClipboardGuard::save();
    // Метка в системном буфере: Clipboard Content прочтёт её, если ⌘C браузера без окна до системного буфера не дошёл.
    const MARK: &str = "метка живого теста";
    set_clipboard(MARK.as_bytes());
    let serial = live.core.store.get_profile(&live.quiet).unwrap().unwrap().serial;
    let f: crate::flow::model::Flow = serde_json::from_value(json!({
        "format": "ombra-flow/1", "name": "Два браузера", "settings": { "after_browser": "quit" },
        "nodes": [
            { "id": "s", "op": "start" },
            { "id": "g", "op": "web.goto", "config": { "url": "http://flow.ombra.test/a" } },
            { "id": "nb", "op": "control.new_browser", "config": { "serial": serial, "after": "quit" } },
            { "id": "ig", "op": "web.goto", "config": { "url": "http://flow.ombra.test/b" } },
            js("it", "document.title", "inner"),
            js("ot", "document.title", "outer"),
            { "id": "cg", "op": "web.goto", "config": { "url": "http://flow.ombra.test/copy" } },
            js("sel", "document.querySelector('#src').select(); 1", "selected"),
            { "id": "kc", "op": "key.combo", "config": { "modifiers": ["cmd"], "key": "c" } },
            { "id": "fd", "op": "web.focus", "config": { "target": css("#dst") } },
            { "id": "kv", "op": "key.combo", "config": { "modifiers": ["cmd"], "key": "v" } },
            js("pv", "document.querySelector('#dst').value", "pasted"),
            { "id": "cb", "op": "data.clipboard", "config": { "save_to": "clip" } },
            { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["inner", "outer", "pasted", "clip"] } }
        ],
        "edges": [
            { "from": "s", "to": "g" }, { "from": "g", "to": "nb" },
            { "from": "nb", "port": "body", "to": "ig" }, { "from": "ig", "to": "it" },
            { "from": "nb", "to": "ot" }, { "from": "ot", "to": "cg" }, { "from": "cg", "to": "sel" }, { "from": "sel", "to": "kc" },
            { "from": "kc", "to": "fd" }, { "from": "fd", "to": "kv" }, { "from": "kv", "to": "pv" }, { "from": "pv", "to": "cb" }, { "from": "cb", "to": "x" }
        ]
    }))
    .unwrap();
    let f = windowless(f);
    let outcome = run_once(&live.core, request(&f, &live.profile)).await;
    assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
    println!("Clipboard Content после ⌘C: {:?}", outcome.exported["clip"]);
    assert_eq!((outcome.exported["inner"].clone(), outcome.exported["outer"].clone()), (json!("Страница B"), json!("Страница A")));
    assert_eq!(outcome.exported["pasted"], json!("буфер"), "⌘C и ⌘V внутри страницы");
    // Без окна у браузера свой буфер (⌘V выше вставил из него): системный остаётся с меткой; с окном там «буфер».
    let clip = &outcome.exported["clip"];
    assert!(clip == MARK || clip == "буфер", "Clipboard Content читает системный буфер: {outcome:?}");
    assert!(live.core.supervisor.running_info(&live.quiet).is_none(), "второй профиль погашен");
    let journal = std::fs::read_to_string(live.core.paths.flow_log("live")).unwrap();
    assert!(journal.contains("браузер профиля закрыт после тела"), "тело шло в браузере, который запустил узел: {journal}");
}

/// Профиль со своей записью прокси фикстуры: прокси занимает один работающий профиль.
pub(crate) fn routed_profile(core: &Core<OmbraEngine>, name: &str, proxy_port: u16) -> crate::store::profiles::ProfileRecord {
    core.store.create_profile(&routed_input(core, name, proxy_port), 1).unwrap()
}

/// Данные такого профиля (запись прокси уже создана) — до создания: свои настройки поверх.
pub(crate) fn routed_input(core: &Core<OmbraEngine>, name: &str, proxy_port: u16) -> ProfileInput {
    let proxy = ProxyInput {
        kind: ProxyKind::Http,
        host: "127.0.0.1".into(),
        port: proxy_port,
        username: Some("user".into()),
        password: SecretField::Set(Secret::new("pass")),
        note: String::new(),
        enabled: true,
    };
    let proxy_id = core.store.create_proxy(&proxy).unwrap().id;
    let fingerprint = generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap();
    let mut input = ProfileInput::new(name, fingerprint.to_json());
    input.proxy_id = Some(proxy_id);
    input
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib flow::live -- --ignored --nocapture --test-threads=1"]
async fn live_cli_runs_the_login_example_on_20_profiles() {
    use crate::cli;
    let live = live().await;
    let profiles: Vec<_> = (1..=20).map(|i| routed_profile(&live.core, &format!("Вход {i}"), live.proxy_port)).collect();
    let _cleanup: Vec<_> = profiles.iter().map(|p| KillProfileOnPanic(live.core.paths.profile_dir(&p.id))).collect();
    let dir = tempfile::tempdir().unwrap();
    // Пример из `tools/flows/examples` — без окна.
    let example = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../tools/flows/examples/login-csv.json")).unwrap();
    let flow_file = dir.path().join("login.json");
    std::fs::write(&flow_file, serde_json::to_string(&windowless(parse(&example).unwrap())).unwrap()).unwrap();
    let csv: Vec<String> = std::iter::once("login,pass".to_string()).chain((1..=20).map(|i| format!("user{i},pw{i}"))).collect();
    let data = dir.path().join("rows.csv");
    std::fs::write(&data, csv.join("\n")).unwrap();
    let args = |serials: String| -> Vec<String> {
        let (flow, data) = (flow_file.display().to_string(), data.display().to_string());
        ["run", flow.as_str(), "--serial", serials.as_str(), "--data", data.as_str(), "--threads", "5", "--json"].map(String::from).to_vec()
    };
    let all = profiles.iter().map(|p| p.serial.to_string()).collect::<Vec<_>>().join(",");
    let err: Arc<std::sync::Mutex<Vec<u8>>> = Arc::default();
    let stderr = |err: &Arc<std::sync::Mutex<Vec<u8>>>| String::from_utf8_lossy(&err.lock().unwrap()).into_owned();

    // 20 профилей, у каждого — своя строка, своё приветствие и свой снимок.
    let mut out = Vec::new();
    let code = cli::run(&live.core, cli::args::parse(&args(all.clone())).unwrap(), &mut out, err.clone(), CancellationToken::new()).await;
    assert_eq!(code, 0, "{}", stderr(&err));
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let runs = report["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 20);
    for (i, run) in runs.iter().enumerate() {
        assert_eq!(run["acc_id"], json!(profiles[i].id));
        assert_eq!(run["exported"]["greeting"], json!(format!("Здравствуйте, user{}", i + 1)), "строка {i} — профилю {i}");
    }
    let shots: Vec<String> = std::fs::read_dir(report["files_dir"].as_str().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    for p in &profiles {
        assert!(shots.iter().any(|name| name.contains(&p.id) && name.ends_with(".png")), "снимок {}: {shots:?}", p.id);
        assert!(find_profile_processes(&live.core.paths.profile_dir(&p.id)).await.unwrap().is_empty(), "браузер {} погашен", p.id);
    }
    assert!(!stderr(&err).contains("pw1"), "пароли таблицы не печатаются");

    // «Стоп» посреди пакета: код 130, браузеров не остаётся.
    let cancel = CancellationToken::new();
    let stopper = {
        let cancel = cancel.clone();
        async move {
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            cancel.cancel();
        }
    };
    let mut out = Vec::new();
    let (code, ()) = tokio::join!(cli::run(&live.core, cli::args::parse(&args(all)).unwrap(), &mut out, err.clone(), cancel), stopper);
    assert_eq!(code, 130, "{}", stderr(&err));
    for p in &profiles {
        assert!(find_profile_processes(&live.core.paths.profile_dir(&p.id)).await.unwrap().is_empty(), "после «Стоп» браузер {} погашен", p.id);
    }

    // Рядом «окно Ombra»: второе ядро на той же папке держит её блокировку и свой профиль.
    let _window_lock = crate::lock::DirLock::acquire(live.core.paths.root()).unwrap();
    let window = Core::new(
        live.core.paths.clone(),
        live.core.store.clone(),
        live.core.engine.clone(),
        live.core.geo.clone(),
        Arc::new(|_| {}),
        Timings::default(),
    );
    let _window_cleanup = KillProfileOnPanic(live.core.paths.profile_dir(&live.quiet));
    let options = crate::supervisor::StartOptions { headless: std::env::var_os("OMBRA_PROBE_HEADFUL").is_none(), ..Default::default() };
    window.supervisor.start_with(&live.quiet, options).await.unwrap();
    let quiet = live.core.store.get_profile(&live.quiet).unwrap().unwrap();
    let mut out = Vec::new();
    let mixed = args(format!("{},{}", quiet.serial, profiles[0].serial));
    let code = cli::run(&live.core, cli::args::parse(&mixed).unwrap(), &mut out, err.clone(), CancellationToken::new()).await;
    assert_eq!(code, 1, "{}", stderr(&err));
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["runs"][0]["error"], json!("профиль открыт в другом процессе Ombra"));
    assert_eq!(report["runs"][1]["status"], json!("done"));
    assert!(window.supervisor.running_info(&live.quiet).is_some(), "браузер «окна» жив");
    window.supervisor.stop_all().await;
}
