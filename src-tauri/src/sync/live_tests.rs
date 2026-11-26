//! Живые тесты синхронизатора (`#[ignore]`): браузеры без окон на страницах фикстуры, главный — без очеловечивания,
//! «пользователь» действует в главном своей сессией CDP.
//! Запуск: `cargo test --lib sync::live -- --ignored --nocapture --test-threads=1`.
use super::session::{Session, SyncState, WindowSpec, WindowStatus};
use super::{Mode, SyncSettings};
use crate::engine::cdp::CdpClient;
use crate::engine::page::{Browser, Button, By, Doc, Page};
use crate::flow::live_tests::{Live, live, live_with_sink, routed_profile};
use crate::supervisor::Event;
use crate::supervisor::StartOptions;
use crate::supervisor::live_tests::KillProfileOnPanic;
use serde_json::{Value, json};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LOAD: Duration = Duration::from_secs(30);
const SITE: &str = "http://sync.ombra.test";

/// Окно теста: запущенный профиль, первая вкладка на странице фикстуры и своя сессия теста в ней.
struct Window {
    id: String,
    ws: String,
    browser: Browser,
    page: Page,
    _cleanup: KillProfileOnPanic,
}

impl Window {
    fn spec(&self) -> WindowSpec {
        WindowSpec { profile_id: self.id.clone(), ws: self.ws.clone() }
    }

    async fn eval(&self, code: &str) -> String {
        self.page.eval_main(code, &[]).await.unwrap().as_str().unwrap_or_default().to_string()
    }

    /// Журнал событий страницы (`LOGGER` фикстуры).
    async fn log(&self) -> String {
        self.eval("document.documentElement.dataset.log || ''").await
    }

    async fn field(&self) -> String {
        self.eval("document.querySelector('#q').value").await
    }

    /// Путь адреса первой вкладки.
    async fn path(&self) -> String {
        self.page.url().await.unwrap().trim_start_matches(SITE).to_string()
    }

    async fn size(&self) -> (u32, u32) {
        let mut cdp = CdpClient::connect(&self.ws).await.unwrap();
        let window = cdp.window_for_first_page().await.unwrap();
        let bounds = cdp.window_bounds(window).await.unwrap();
        (bounds.width, bounds.height)
    }
}

async fn open(live: &Live, id: &str, path: &str) -> Window {
    let cleanup = KillProfileOnPanic(live.core.paths.profile_dir(id));
    let options = StartOptions { headless: std::env::var_os("OMBRA_PROBE_HEADFUL").is_none(), ..Default::default() };
    let info = live.core.supervisor.start_with(id, options).await.unwrap();
    let browser = Browser::connect(&info.cdp_ws).await.unwrap();
    eventually("первая вкладка после готовности CDP", || async { !browser.tabs().await.unwrap().is_empty() }).await;
    let tab = browser.tabs().await.unwrap().remove(0).target_id;
    let page = browser.attach(&tab).await.unwrap();
    page.goto(&format!("{SITE}{path}"), LOAD).await.unwrap();
    Window { id: id.to_string(), ws: info.cdp_ws, browser, page, _cleanup: cleanup }
}

/// Главный (профиль без очеловечивания) на `/sync` и ведомые `f1`, `f2`… на своих страницах; ведомые — с
/// очеловечиванием, как у новых профилей.
async fn windows(live: &Live, followers: &[&str]) -> (Window, Vec<Window>) {
    let main = open(live, &live.quiet, "/sync").await;
    let mut list = Vec::new();
    for (i, path) in followers.iter().enumerate() {
        let id = routed_profile(&live.core, &format!("f{}", i + 1), live.proxy_port).id;
        list.push(open(live, &id, path).await);
    }
    (main, list)
}

/// После чистого старта старые CDP-соединения закрыты; тест подключается к новым процессам.
async fn fresh_session(live: &Live, main: &Window, followers: &[Window]) -> (Window, Vec<Window>) {
    let ids: Vec<_> = std::iter::once(main).chain(followers).map(|w| w.id.clone()).collect();
    if std::env::var_os("OMBRA_PROBE_HEADFUL").is_none() {
        for id in &ids {
            live.core.store.raw().execute("UPDATE profiles SET extra_args = '[\"--headless=new\"]' WHERE id = ?1", [id]).unwrap();
        }
    }
    live.core.sync_start(&main.id, &ids[1..]).await.unwrap();
    let mut windows = Vec::new();
    for id in ids {
        let info = live.core.supervisor.running_info(&id).unwrap();
        let browser = Browser::connect(&info.cdp_ws).await.unwrap();
        let page = browser.attach(&browser.tabs().await.unwrap()[0].target_id).await.unwrap();
        windows.push(Window { _cleanup: KillProfileOnPanic(live.core.paths.profile_dir(&id)), id, ws: info.cdp_ws, browser, page });
    }
    (windows.remove(0), windows)
}

/// Сессия и все состояния, что она сообщила.
async fn start(main: &Window, followers: &[Window], settings: SyncSettings) -> (Session, Arc<Mutex<Vec<SyncState>>>) {
    let seen: Arc<Mutex<Vec<SyncState>>> = Arc::default();
    let sink = seen.clone();
    let specs = followers.iter().map(Window::spec).collect();
    let session = Session::start(main.spec(), specs, settings, Arc::new(move |state| sink.lock().unwrap().push(state))).await.unwrap();
    (session, seen)
}

fn status(session: &Session, id: &str) -> WindowStatus {
    session.state().windows.into_iter().find(|w| w.profile_id == id).expect("окно в сессии").status
}

/// Ждёт условия до 20 с.
async fn eventually<F: Future<Output = bool>>(what: &str, mut check: impl FnMut() -> F) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check().await {
        assert!(Instant::now() < deadline, "не дождались: {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn click(main: &Window, x: f64, y: f64) {
    main.page.click(x, y, Button::Left, 1, Duration::ZERO).await.unwrap();
}

async fn type_text(main: &Window, text: &str) {
    for ch in text.chars() {
        main.page.type_char(ch).await.unwrap();
    }
}

/// Ответ «пользователя» на диалог страницы главного, как только он откроется.
async fn answer(main: &Window, accept: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while main.page.answer_dialog(accept, "").await.is_err() {
        assert!(Instant::now() < deadline, "диалог у главного не открылся");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Переход по адресу, как из адресной строки (`transitionType` `Page.navigate` по умолчанию — `typed`).
async fn navigate(window: &Window, path: &str) {
    window.page.goto(&format!("{SITE}{path}"), LOAD).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_full_copy_repeats_despite_other_element() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync", "/sync-other"]).await;
    let (f1, f2) = (&followers[0], &followers[1]);
    assert_eq!(f1.browser.humanize_enabled().await, Some(true), "очеловечивание нового профиля включено");
    let before = [main.size().await, f1.size().await, f2.size().await];
    let (session, seen) = start(&main, &followers, SyncSettings::default()).await;
    assert_eq!(f1.browser.humanize_enabled().await, Some(false), "«Полная копия» выключает очеловечивание ведомых");
    let after = [main.size().await, f1.size().await, f2.size().await];
    assert_eq!(after, [before[0]; 3], "ведомые повторяют размер мастера; до старта: {before:?}");

    click(&main, 80.0, 40.0).await;
    eventually("клик у f1", || async move { f1.log().await.contains("click:a:true") }).await;
    eventually("клик по ссылке у f2", || async move { f2.log().await.contains("click:x:true") }).await;
    assert_eq!(status(&session, &f2.id), WindowStatus::Synced);
    assert!(seen.lock().unwrap().iter().all(|state| state.windows.iter().all(|w| w.status == WindowStatus::Synced)));

    click(&main, 80.0, 95.0).await;
    type_text(&main, "абв").await;
    eventually("набор у f1", || async move { f1.field().await == "абв" }).await;
    eventually("набор у f2 после клика по другому элементу", || async move { f2.field().await == "абв" }).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    assert_eq!(status(&session, &f2.id), WindowStatus::Synced);

    session.stop().await;
    assert!(!session.state().active);
    assert_eq!(f1.browser.humanize_enabled().await, Some(true), "остановка возвращает очеловечивание");
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_soft_check_passes_other_price() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync-price"]).await;
    let f1 = &followers[0];
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;
    click(&main, 80.0, 40.0).await;
    eventually("клик у f1", || async move { f1.log().await.contains("click:a:true") }).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced, "текст кнопки не сверяется");
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_release_and_resume_does_not_replay_missed() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync"]).await;
    let f1 = &followers[0];
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;

    session.release(&f1.id).await.unwrap();
    assert_eq!(status(&session, &f1.id), WindowStatus::Paused { code: "released".into(), detail: String::new() });
    click(&main, 80.0, 40.0).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!f1.log().await.contains("click:"), "отпущенному окну шаги не идут: {}", f1.log().await);

    session.resume(&f1.id).await.unwrap();
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    click(&main, 80.0, 40.0).await;
    eventually("клик у f1 после «Вернуть»", || async move { f1.log().await.contains("click:a:true") }).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let log = f1.log().await;
    assert_eq!(log.matches("click:").count(), 1, "resume_does_not_replay_missed: пропущенный клик не повторён: {log}");
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn follower_closed_drops_and_others_go_on() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync", "/sync"]).await;
    let (f1, f2) = (&followers[0], &followers[1]);
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;

    live.core.supervisor.stop(&f2.id).await.unwrap();
    let s = &session;
    eventually("f2 выпало", || async move { status(s, &f2.id) == WindowStatus::Dropped { code: "closed".into() } }).await;
    click(&main, 80.0, 40.0).await;
    eventually("клик у f1", || async move { f1.log().await.contains("click:a:true") }).await;
    assert!(session.state().active, "сессия идёт дальше");
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_varied_types_in_chunks() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync"]).await;
    let f1 = &followers[0];
    let settings = SyncSettings { mode: Mode::Varied, click_delay: [0, 0], typing_delay: [0, 0], ..Default::default() };
    let (session, _) = start(&main, &followers, settings).await;
    let state = session.state();
    assert_eq!(state.mode, Mode::Varied);
    assert!(!state.windows.iter().find(|w| w.profile_id == f1.id).unwrap().plain, "у f1 очеловечивание есть");
    assert_eq!(f1.browser.humanize_enabled().await, Some(true), "«С вариациями» очеловечивание не трогает");

    click(&main, 80.0, 95.0).await;
    type_text(&main, "привет").await;
    eventually("набор у f1", || async move { f1.field().await == "привет" }).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_navigation_repeats() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync"]).await;
    let f1 = &followers[0];
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;

    navigate(&main, "/a").await;
    eventually("f1 на /a", || async move { f1.path().await == "/a" }).await;

    // Ссылка /a → /b: ведомый переходит из повтора клика, ядро перехода не повторяет. Клик мгновенный, как касание
    // трекпада: отпускание не должно пропасть вместе с документом, который уводит переход.
    let link = main.page.eval_main("const r = document.querySelector('a').getBoundingClientRect(); [r.x + r.width / 2, r.y + r.height / 2]", &[]).await.unwrap();
    click(&main, link[0].as_f64().unwrap(), link[1].as_f64().unwrap()).await;
    eventually("f1 на /b", || async move { f1.path().await == "/b" }).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let history = f1.page.history().await.unwrap();
    assert_eq!(history.urls.iter().filter(|u| u.ends_with("/b")).count(), 1, "второго перехода нет: {:?}", history.urls);

    main.page.back(LOAD).await.unwrap();
    eventually("f1 назад на /a", || async move { f1.path().await == "/a" }).await;
    main.page.reload(LOAD).await.unwrap();
    let kind = "performance.getEntriesByType('navigation')[0].type";
    eventually("f1 перезагружен", || async move { f1.path().await == "/a" && f1.eval(kind).await == "reload" }).await;

    // ⌘-клик открывает ссылку в новой вкладке: переход этой вкладки сразу за ним начат не страницей.
    let (x, y) = (link[0].as_f64().unwrap(), link[1].as_f64().unwrap());
    main.page.mouse("mousePressed", x, y, Some(Button::Left), 1, 1, 4).await.unwrap();
    main.page.mouse("mouseReleased", x, y, Some(Button::Left), 0, 1, 4).await.unwrap();
    navigate(&main, "/sync").await;
    eventually("f1 на /sync", || async move { f1.path().await == "/sync" }).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

/// Новая вкладка скрыла первые вкладки (не сразу); первая вкладка главного снова видима — у ведомого тоже.
async fn back_to_first(main: &Window, follower: &Window) {
    for window in [main, follower] {
        eventually("первая вкладка скрыта", || async move { window.eval("document.visibilityState").await == "hidden" }).await;
    }
    let first = main.browser.tabs().await.unwrap().remove(0).target_id;
    main.browser.activate(&first).await.unwrap();
    eventually("первая вкладка ведомого видима", || async move { follower.eval("document.visibilityState").await == "visible" }).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real browsers and an isolated QA license; exercises linked tabs with and without a live opener"]
async fn live_page_opened_tabs_keep_replaying_after_activation() {
    assert!(std::env::var_os("OMBRA_PROBE_ENGINES").is_some() && std::env::var_os("OMBRA_LICENSE_KEY").is_some());
    for rel in ["opener", "noopener"] {
        let live = live().await;
        let (main, followers) = windows(&live, &["/sync"]).await;
        let follower = &followers[0];
        for window in [&main, follower] {
            window.page.eval_main("const a=document.querySelector('#blank'); a.href='/sync'; a.rel=rel; true", &[("rel".into(),json!(rel))]).await.unwrap();
        }
        let (session, _) = start(&main, &followers, SyncSettings::default()).await;
        let mut events = main.browser.discover_tabs().await.unwrap();
        click(&main, 80.0, 245.0).await;
        eventually("both linked tabs opened", || async {
            main.browser.tabs().await.unwrap().len()==2 && follower.browser.tabs().await.unwrap().len()==2
        }).await;
        let mut opened = Vec::new();
        while let Ok(event) = events.try_recv() {
            if matches!(event.method.as_str(), "Page.windowOpen" | "Target.targetCreated") {
                opened.push(json!({"method":event.method,"session":event.session,"params":event.params}));
            }
        }
        eprintln!("linked-tab evidence ({rel}): {}", json!(opened));
        let main_target = main.browser.tabs().await.unwrap().into_iter().find(|tab| tab.target_id!=main.page.target()).unwrap().target_id;
        let follower_target = follower.browser.tabs().await.unwrap().into_iter().find(|tab| tab.target_id!=follower.page.target()).unwrap().target_id;
        let main_child = main.browser.attach(&main_target).await.unwrap();
        let follower_child = follower.browser.attach(&follower_target).await.unwrap();
        eventually("linked documents ready", || async {
            main_child.eval_main("document.readyState==='complete' && !!document.querySelector('#q')", &[]).await.unwrap()==json!(true)
                && follower_child.eval_main("document.readyState==='complete' && !!document.querySelector('#q')", &[]).await.unwrap()==json!(true)
        }).await;
        main.browser.activate(&main_target).await.unwrap();
        assert_eq!(status(&session, &follower.id), WindowStatus::Synced);
        main_child.click(80.0,40.0,Button::Left,1,Duration::ZERO).await.unwrap();
        eventually("click replay in linked tab", || async {
            follower_child.eval_main("document.documentElement.dataset.log || ''", &[]).await.unwrap().as_str().unwrap().contains("click:a:true")
        }).await;
        main_child.click(80.0,95.0,Button::Left,1,Duration::ZERO).await.unwrap();
        for ch in "new tab".chars() { main_child.type_char(ch).await.unwrap(); }
        eventually("typing replay in linked tab", || async {
            follower_child.eval_main("document.querySelector('#q').value", &[]).await.unwrap()==json!("new tab")
        }).await;
        main.browser.close_tab(&main_target).await.unwrap();
        eventually("linked follower tab closes", || async { follower.browser.tabs().await.unwrap().len()==1 }).await;
        main.browser.activate(main.page.target()).await.unwrap();
        click(&main,80.0,40.0).await;
        eventually("original tab still replays", || async { follower.log().await.contains("click:a:true") }).await;
        assert_eq!(status(&session,&follower.id),WindowStatus::Synced);
        session.stop().await;
        live.core.supervisor.stop_all().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real browsers and an isolated QA license; includes the bounded wait for filtered opening gestures"]
async fn live_page_opened_tabs_respect_disabled_input_without_duplicates() {
    assert!(std::env::var_os("OMBRA_PROBE_ENGINES").is_some() && std::env::var_os("OMBRA_LICENSE_KEY").is_some());
    for (clicks, typing, keyboard) in [(false,true,false), (true,false,true), (true,false,false), (false,true,true)] {
        let live = live().await;
        let (main, followers) = windows(&live, &["/sync"]).await;
        let follower = &followers[0];
        for window in [&main, follower] {
            window.page.eval_main("const a=document.querySelector('#blank'); a.href='/sync'; a.focus(); true", &[]).await.unwrap();
        }
        let (session, _) = start(&main, &followers, SyncSettings { clicks, typing, ..Default::default() }).await;
        if keyboard { main.page.press("Enter", &[]).await.unwrap(); } else { click(&main,80.0,245.0).await; }
        eventually("main child opens", || async { main.browser.tabs().await.unwrap().len()==2 }).await;
        let main_target = main.browser.tabs().await.unwrap().into_iter().find(|tab| tab.target_id!=main.page.target()).unwrap().target_id;
        let main_child = main.browser.attach(&main_target).await.unwrap();
        let deadline = Instant::now() + super::replay::NAV_WAIT + Duration::from_secs(10);
        let mut moves = 0;
        while follower.browser.tabs().await.unwrap().len() < 2 {
            assert_eq!(status(&session,&follower.id), WindowStatus::Synced);
            assert!(Instant::now() < deadline, "filtered gesture did not copy the linked tab: {clicks}/{typing}/{keyboard}");
            main_child.mouse_move(400.0 + f64::from(moves % 100),300.0).await.unwrap();
            moves += 1;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(follower.browser.tabs().await.unwrap().len(),2);
        let follower_target = follower.browser.tabs().await.unwrap().into_iter().find(|tab| tab.target_id!=follower.page.target()).unwrap().target_id;
        let follower_child = follower.browser.attach(&follower_target).await.unwrap();
        eventually("copied or adopted child document", || async {
            follower_child.eval_main("document.readyState==='complete' && !!document.querySelector('#q')", &[]).await.unwrap()==json!(true)
        }).await;
        for page in [&main_child, &follower_child] {
            page.eval_main("document.body.style.minHeight='3000px'; true", &[]).await.unwrap();
        }
        main.browser.activate(&main_target).await.unwrap();
        main_child.wheel_by(400.0,300.0,0.0,200.0,0).await.unwrap();
        eventually("scroll replays after filtered opening gesture", || async {
            follower_child.eval_main("window.scrollY > 0", &[]).await.unwrap()==json!(true)
        }).await;
        assert_eq!(follower.browser.tabs().await.unwrap().len(),2,"no duplicate native tab");
        if !clicks && !keyboard {
            follower.browser.activate(follower.page.target()).await.unwrap();
            follower.eval("document.querySelector('#blank').focus(); ''").await;
            follower.page.press("Enter", &[]).await.unwrap();
            eventually("late ambiguous popup pauses before it can claim another tab", || async {
                matches!(status(&session,&follower.id),WindowStatus::Paused { code, .. } if code=="failed")
            }).await;
            main.browser.activate(main.page.target()).await.unwrap();
            click(&main,80.0,245.0).await;
            assert!(matches!(status(&session,&follower.id),WindowStatus::Paused { .. }));
        }
        session.stop().await;
        live.core.supervisor.stop_all().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_tabs_open_close() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync"]).await;
    let f1 = &followers[0];
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;
    let before = f1.browser.tabs().await.unwrap().len();

    let tab = main.browser.new_tab(&format!("{SITE}/a")).await.unwrap();
    let urls = || async move { f1.browser.tabs().await.unwrap().into_iter().map(|t| t.url).collect::<Vec<_>>() };
    eventually("новая вкладка у f1 на /a", || async move {
        let urls = urls().await;
        urls.len() == before + 1 && urls.iter().any(|u| u.ends_with("/a"))
    })
    .await;
    back_to_first(&main, f1).await;
    main.browser.close_tab(&tab).await.unwrap();
    eventually("вкладка f1 закрыта", || async move { f1.browser.tabs().await.unwrap().len() == before }).await;

    // Вкладку открыла страница (`openerId`): у f1 она — из повтора клика, без второй от ядра.
    click(&main, 80.0, 245.0).await;
    eventually("вкладка страницы у f1", || async move { f1.browser.tabs().await.unwrap().len() == before + 1 }).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(f1.browser.tabs().await.unwrap().len(), before + 1, "{:?}", urls().await);
    // Ещё вкладка из интерфейса: она вышла вперёд без `visibilitychange`, и возврат на первую — снова смена вкладки.
    main.browser.new_tab(&format!("{SITE}/b")).await.unwrap();
    eventually("ещё вкладка у f1", || async move { f1.browser.tabs().await.unwrap().len() == before + 2 }).await;
    back_to_first(&main, f1).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_open_like_main_and_show_take_the_visible_tab() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync"]).await;
    let (main, followers) = fresh_session(&live, &main, &followers).await;
    let f1 = &followers[0];
    navigate(&main, "/sync").await;
    eventually("первая вкладка выровнена", || async { f1.path().await == "/sync" }).await;
    let before = f1.browser.tabs().await.unwrap().len();
    // Вторая вкладка главного видима, у f1 она тоже открылась.
    let second = main.browser.new_tab(&format!("{SITE}/a")).await.unwrap();
    eventually("вторая вкладка у f1", || async move { f1.browser.tabs().await.unwrap().len() == before + 1 }).await;
    for window in [&main, f1] {
        eventually("первая вкладка скрыта", || async move { window.eval("document.visibilityState").await == "hidden" }).await;
    }

    // Отпущенный f1 не идёт за главным на /b; «Открыть как в главном» меняет только его вторую вкладку.
    live.core.sync_release(&f1.id).await.unwrap();
    main.browser.attach(&second).await.unwrap().goto(&format!("{SITE}/b"), LOAD).await.unwrap();
    live.core.sync_open_like_main(&f1.id).await.unwrap();
    assert_eq!(f1.path().await, "/sync", "первая вкладка f1 на месте");
    let urls: Vec<String> = f1.browser.tabs().await.unwrap().into_iter().map(|t| t.url).collect();
    assert!(urls.contains(&format!("{SITE}/b")), "вторая вкладка f1 — как у главного: {urls:?}");
    live.core.sync_resume(&f1.id).await.unwrap();
    main.browser.attach(&second).await.unwrap().goto(&format!("{SITE}/a"), LOAD).await.unwrap();
    eventually("после паузы сохранён порядок вкладок", || async { f1.browser.tabs().await.unwrap().iter().any(|t| t.url == format!("{SITE}/a")) }).await;
    assert_eq!(f1.path().await, "/sync", "возобновление не переставляет соответствия вкладок");

    // «Показать» выводит видимую вкладку, а не первую.
    for window in [&main, f1] {
        live.core.show_window(&window.id).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    for window in [&main, f1] {
        assert_eq!(window.eval("document.visibilityState").await, "hidden", "первая вкладка осталась позади");
    }
    live.core.sync_stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_start_uses_clean_tabs_and_preserves_saved_profile_settings() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync-other", "/sync"]).await;
    main.browser.new_tab(&format!("{SITE}/a")).await.unwrap();
    let ids: Vec<String> = std::iter::once(&main).chain(&followers).map(|w| w.id.clone()).collect();
    for id in &ids {
        let extra = if std::env::var_os("OMBRA_PROBE_HEADFUL").is_none() { r#"["--headless=new"]"# } else { "[]" };
        live.core.store.raw().execute(
            "UPDATE profiles SET restore_tabs = 1, start_urls = ?2, extra_args = ?3 WHERE id = ?1",
            rusqlite::params![id, format!(r#"["{SITE}/must-not-open"]"#), extra],
        ).unwrap();
    }
    let before = live.core.supervisor.running_info(&main.id).unwrap().pid;
    let started = live.core.sync_start(&main.id, &ids[1..]).await.unwrap();
    assert_eq!(started.main.as_deref(), Some(main.id.as_str()));
    assert_ne!(live.core.supervisor.running_info(&main.id).unwrap().pid, before);
    for id in &ids {
        let browser = Browser::connect(&live.core.supervisor.running_info(id).unwrap().cdp_ws).await.unwrap();
        let urls: Vec<_> = browser.tabs().await.unwrap().into_iter().map(|t| t.url).collect();
        assert_eq!(urls.len(), 1, "только чистая вкладка: {urls:?}");
        assert!(urls[0] == "about:blank" || urls[0].starts_with("chrome://newtab"), "{urls:?}");
        let saved = live.core.store.get_profile(id).unwrap().unwrap();
        assert!(saved.restore_tabs);
        assert_eq!(saved.start_urls, [format!("{SITE}/must-not-open")]);
    }
    live.core.sync_stop().await;
    assert!(ids.iter().all(|id| live.core.supervisor.running_info(id).is_some()), "выключение повтора сохраняет окна");
    // Новая сессия не должна быть очищена сторожем предыдущей.
    live.core.sync_start(&main.id, &ids[1..]).await.unwrap();
    live.core.sync_release(&ids[1]).await.unwrap();
    live.core.supervisor.stop(&main.id).await.unwrap();
    eventually("ведомые закрыты вместе с мастером", || async { ids.iter().all(|id| !live.core.supervisor.status(id).is_active()) }).await;
    eventually("все занятости сняты", || async { ids.iter().all(|id| live.core.flows.claim(id).is_some()) }).await;
    assert!(!live.core.sync_state().active);
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_dialog_answer_repeats() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync-dialog"]).await;
    let f1 = &followers[0];
    navigate(&main, "/sync-dialog").await;
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;

    // Клик с `confirm` отвечает, только когда диалог закрыт: «пользователь» отвечает, пока клик ждёт.
    tokio::join!(click(&main, 80.0, 40.0), answer(&main, false));
    assert_eq!(main.eval("document.documentElement.dataset.answer").await, "false");
    eventually("«отмена» у f1", || async move { f1.eval("document.documentElement.dataset.answer || ''").await == "false" }).await;
    tokio::join!(click(&main, 80.0, 40.0), answer(&main, true));
    eventually("«OK» у f1", || async move { f1.eval("document.documentElement.dataset.answer").await == "true" }).await;
    // Диалог на нажатии клавиши: нажатие уходит ведомому раньше ответа главного и ждёт закрытия диалога — рабочий не
    // должен ждать его, иначе ответ встанет за ним в очередь.
    click(&main, 80.0, 95.0).await;
    tokio::join!(async { main.page.type_char('ж').await.unwrap() }, answer(&main, true));
    eventually("ответ на нажатие у f1", || async move { f1.eval("document.documentElement.dataset.key || ''").await == "true" }).await;
    // `alert` при загрузке: переход ведомого ждёт загрузки, а её держит диалог — рабочий не ждёт перехода. Диалог у
    // ведомого открыт, пока главный думает дольше, чем ведомое ждёт свой диалог, — он не лишний.
    let think = async {
        tokio::time::sleep(Duration::from_secs(3)).await;
        answer(&main, true).await;
    };
    tokio::join!(navigate(&main, "/sync-alert"), think);
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced, "диалог главного был открыт");
    eventually("f1 на /sync-alert", || async move { f1.path().await == "/sync-alert" }).await;
    eventually("загрузка f1 после ответа", || async move { f1.eval("document.documentElement.dataset.loaded || ''").await == "да" }).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

/// Выбор «Бета» клавишами «пользователя»: поле, Tab на список, первая буква варианта.
async fn select_by_keys(settings: SyncSettings) {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync"]).await;
    let f1 = &followers[0];
    let (session, _) = start(&main, &followers, settings).await;

    click(&main, 80.0, 95.0).await;
    main.page.press("Tab", &[]).await.unwrap();
    main.page.type_char('Б').await.unwrap();
    assert_eq!(main.eval("document.querySelector('#s').value").await, "b", "у главного выбрана «Бета»");
    eventually("«Бета» у f1", || async move { f1.eval("document.querySelector('#s').value").await == "b" }).await;
    assert!(f1.log().await.contains("change:s:true"), "{}", f1.log().await);
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_select_by_label() {
    select_by_keys(SyncSettings::default()).await;
}

/// «С вариациями»: буква в списке ушла бы куском набора в `insertText` и ничего бы не выбрала — здесь выбор варианта
/// не подменить повтором нажатий.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_select_by_label_varied() {
    select_by_keys(SyncSettings { mode: Mode::Varied, click_delay: [0, 0], typing_delay: [0, 0], ..Default::default() }).await;
}

/// Перетаскивание идёт с нажатой кнопкой и при выключенных «Движениях мыши»: они — только наведение (R19).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_drag_keeps_the_button_pressed() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync-drag"]).await;
    let f1 = &followers[0];
    navigate(&main, "/sync-drag").await;
    let (session, _) = start(&main, &followers, SyncSettings { mouse: false, ..Default::default() }).await;

    main.page.mouse("mousePressed", 40.0, 40.0, Some(Button::Left), 1, 1, 0).await.unwrap();
    for x in (60..=240).step_by(20) {
        main.page.mouse("mouseMoved", f64::from(x), 40.0, Some(Button::Left), 1, 0, 0).await.unwrap();
    }
    main.page.mouse("mouseReleased", 240.0, 40.0, Some(Button::Left), 0, 1, 0).await.unwrap();
    let value = "document.documentElement.dataset.value || ''";
    assert_eq!(main.eval(value).await, "220", "у главного ползунок дошёл до конца");
    eventually("ползунок f1 там же", || async move { f1.eval(value).await == "220" }).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_file_chooser_pauses() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync-file"]).await;
    let f1 = &followers[0];
    navigate(&main, "/sync-file").await;
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;

    // Без окон выбор файла у главного может и не открыться: ответа клика не ждём.
    let _ = tokio::time::timeout(Duration::from_secs(5), click(&main, 80.0, 40.0)).await;
    let s = &session;
    eventually("пауза f1", || async move { matches!(status(s, &f1.id), WindowStatus::Paused { .. }) }).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Paused { code: "file".into(), detail: String::new() });
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_page_error_pauses() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync-500"]).await;
    let f1 = &followers[0];
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;

    // Ошибка и у главного: не отклонение.
    navigate(&main, "/sync-500?x").await;
    eventually("f1 на /sync-500?x", || async move { f1.path().await == "/sync-500?x" }).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);

    navigate(&main, "/a").await;
    eventually("f1 на /a", || async move { f1.path().await == "/a" }).await;
    // Ошибка только у этого профиля (R5): ведомый уходит на /sync-500 своим соединением теста.
    navigate(f1, "/sync-500").await;
    let s = &session;
    eventually("пауза f1", || async move { matches!(status(s, &f1.id), WindowStatus::Paused { .. }) }).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Paused { code: "page_error".into(), detail: "HTTP 500".into() });
    session.stop().await;
    live.core.supervisor.stop_all().await;
}


#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_dialog_only_in_follower_pauses() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync", "/sync"]).await;
    let (f1, f2) = (&followers[0], &followers[1]);
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;

    // Диалог только у этого профиля: страница f1 открывает его сама, у главного диалога нет.
    let opened = Instant::now();
    f1.page.eval_main("setTimeout(() => alert('Только здесь'), 0); 1", &[]).await.unwrap();
    let s = &session;
    eventually("пауза f1", || async move { matches!(status(s, &f1.id), WindowStatus::Paused { .. }) }).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Paused { code: "dialog".into(), detail: String::new() });
    assert!(opened.elapsed() < Duration::from_secs(4), "2 с ожидания и запас: {:?}", opened.elapsed());
    assert_eq!(status(&session, &f2.id), WindowStatus::Synced);
    f1.page.answer_dialog(true, "").await.unwrap();
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_page_navigation_is_not_repeated() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync"]).await;
    let f1 = &followers[0];
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;
    navigate(&main, "/a").await;
    eventually("f1 на /a", || async move { f1.path().await == "/a" }).await;
    navigate(&main, "/sync-history").await;
    eventually("f1 на /sync-history", || async move { f1.path().await == "/sync-history" }).await;

    // «Назад» самой страницей: ведомый уходит назад из повтора клика, шаг по истории ядро не повторяет.
    click(&main, 80.0, 40.0).await;
    eventually("f1 назад на /a", || async move { f1.path().await == "/a" }).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let history = f1.page.history().await.unwrap();
    assert_eq!(history.index + 2, history.urls.len(), "один шаг назад: {history:?}");

    // Обновление самой страницей — одна загрузка у ведомого.
    navigate(&main, "/sync-history").await;
    let loads = || async move { f1.eval("sessionStorage.loads || ''").await };
    eventually("f1 снова на /sync-history", || async move { f1.path().await == "/sync-history" && !loads().await.is_empty() }).await;
    let before: u32 = loads().await.parse().unwrap();
    click(&main, 80.0, 100.0).await;
    eventually("f1 обновился", || async move { loads().await.parse::<u32>().unwrap_or(0) > before }).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(loads().await, (before + 1).to_string(), "одна загрузка");
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_back_and_forward_inside_a_page() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync-spa"]).await;
    let f1 = &followers[0];
    navigate(&main, "/sync-spa").await;
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;

    // Ссылки приложения меняют адрес `pushState`: ведомый — из повтора клика, ядро перехода не повторяет.
    click(&main, 80.0, 40.0).await;
    eventually("f1 на /sync-spa/1", || async move { f1.path().await == "/sync-spa/1" }).await;
    click(&main, 80.0, 100.0).await;
    eventually("f1 на /sync-spa/2", || async move { f1.path().await == "/sync-spa/2" }).await;
    let at = f1.page.history().await.unwrap();
    assert_eq!(at.index + 1, at.urls.len(), "записи только от `pushState`: {at:?}");

    // `history.back()` страницы — один шаг у ведомого, из повтора клика.
    click(&main, 80.0, 160.0).await;
    eventually("f1 назад на /sync-spa/1", || async move { f1.path().await == "/sync-spa/1" }).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(f1.page.history().await.unwrap().index + 1, at.index, "один шаг назад");

    // «Назад» браузера внутри документа — шаг по истории у ведомого (позже `PAGE_STEP` после клика: иначе он от страницы).
    main.page.back(LOAD).await.unwrap();
    eventually("f1 назад на /sync-spa", || async move { f1.path().await == "/sync-spa" }).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

/// Обновление страницей ещё грузится, а прежний документ зовёт `replaceState`: начало обновления остаётся за ним, и
/// ведомый обновляется один раз — своим повтором клика.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_page_reload_with_replace_state_meanwhile_is_not_repeated() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync-slow?fast"]).await;
    let f1 = &followers[0];
    navigate(&main, "/sync-slow").await;
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;
    let loads = "sessionStorage.loads || ''";
    let (main_before, f1_before): (u32, u32) = (main.eval(loads).await.parse().unwrap(), f1.eval(loads).await.parse().unwrap());

    click(&main, 80.0, 40.0).await;
    let m = &main;
    eventually("главный обновился", || async move { m.eval(loads).await.parse::<u32>().unwrap_or(0) > main_before }).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(f1.eval(loads).await, (f1_before + 1).to_string(), "ведомый обновился один раз");
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

/// Документ фрейма `frame` (`#same` или `#cross` страницы `/frames`) окна; у межсайтового — своя сессия.
async fn frame_doc(window: &Window, frame: &str) -> Doc {
    let owner = window.page.nth(&window.page.main_doc(), By::Css, frame, 0).await.unwrap().expect("фрейм на странице");
    window.page.frame_doc(&owner).await.unwrap()
}

/// Центр элемента `selector` фрейма `frame` в окне вкладки.
async fn frame_point(window: &Window, frame: &str, selector: &str) -> (f64, f64) {
    let doc = frame_doc(window, frame).await;
    let element = window.page.nth(&doc, By::Css, selector, 0).await.unwrap().expect("элемент во фрейме");
    window.page.point(&element, None).await.unwrap()
}

async fn frame_eval(window: &Window, frame: &str, code: &str) -> String {
    let doc = frame_doc(window, frame).await;
    window.page.eval_in(&doc, code, &[]).await.unwrap().as_str().unwrap_or_default().to_string()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_frames_same_and_cross_site() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/frames"]).await;
    let f1 = &followers[0];
    navigate(&main, "/frames").await;
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;

    // Фрейм того же сайта — в процессе вкладки, фрейм `frames.other.test` — в своём, со своей сессией.
    for frame in ["#same", "#cross"] {
        click_in_frame_once(&main, f1, frame).await;
    }
    let (x, y) = frame_point(&main, "#cross", "#ii").await;
    click(&main, x, y).await;
    type_text(&main, "ж").await;
    let value = "document.querySelector('#ii').value";
    eventually("набор во фрейме чужого сайта у f1", || async move { frame_eval(f1, "#cross", value).await == "ж" }).await;

    // Фреймы, появившиеся после старта сессии: главный уходит со страницы и возвращается.
    for path in ["/sync", "/frames"] {
        navigate(&main, path).await;
        eventually(&format!("f1 загрузил {path}"), || async move { f1.path().await == path && f1.eval("document.readyState").await == "complete" }).await;
    }
    click_in_frame_once(&main, f1, "#cross").await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

/// Клик главного по `#ib` фрейма `frame`: у ведомого в журнале фрейма — ровно один доверенный клик по нему.
async fn click_in_frame_once(main: &Window, follower: &Window, frame: &str) {
    let (x, y) = frame_point(main, frame, "#ib").await;
    click(main, x, y).await;
    let log = "document.documentElement.dataset.log || ''";
    eventually(&format!("клик во фрейме {frame} у ведомого"), || async move { frame_eval(follower, frame, log).await.contains("click:ib:true") }).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let log = frame_eval(follower, frame, log).await;
    assert_eq!(log.matches("click:ib:").count(), 1, "клик во фрейме {frame} повторён один раз: {log}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn live_cross_site_frame_dialogs_lists_and_files() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync-frames", "/sync-frames"]).await;
    let (f1, f2) = (&followers[0], &followers[1]);
    navigate(&main, "/sync-frames").await;
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;

    // Диалог фрейма чужого сайта: ответ главного — у ведомых тот же.
    let (x, y) = frame_point(&main, "#cross", "#d").await;
    tokio::join!(click(&main, x, y), answer(&main, false));
    let answered = "document.documentElement.dataset.answer || ''";
    for follower in [f1, f2] {
        eventually("«отмена» во фрейме ведомого", || async move { frame_eval(follower, "#cross", answered).await == "false" }).await;
    }

    // Список во фрейме: поле, Tab на список, первая буква варианта.
    let (x, y) = frame_point(&main, "#cross", "#q").await;
    click(&main, x, y).await;
    main.page.press("Tab", &[]).await.unwrap();
    main.page.type_char('Б').await.unwrap();
    let chosen = "document.querySelector('#s').value";
    assert_eq!(frame_eval(&main, "#cross", chosen).await, "b", "у главного выбрана «Бета»");
    eventually("«Бета» во фрейме f1", || async move { frame_eval(f1, "#cross", chosen).await == "b" }).await;
    assert_eq!(frame_eval(f1, "#cross", "document.querySelector('#q').value").await, "", "в поле фрейма ничего не набрано");
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);

    // Диалог только во фрейме f2: пауза `dialog`, f1 идёт дальше.
    f2.page.eval_in(&frame_doc(f2, "#cross").await, "setTimeout(() => alert('Только здесь'), 0); 1", &[]).await.unwrap();
    let s = &session;
    eventually("пауза f2", || async move { matches!(status(s, &f2.id), WindowStatus::Paused { .. }) }).await;
    assert_eq!(status(&session, &f2.id), WindowStatus::Paused { code: "dialog".into(), detail: String::new() });
    assert_eq!(status(&session, &f1.id), WindowStatus::Synced);

    // Выбор файла во фрейме: у ведомого окно не открывается, пауза `file`.
    let (x, y) = frame_point(&main, "#cross", "#f").await;
    let _ = tokio::time::timeout(Duration::from_secs(5), click(&main, x, y)).await;
    eventually("пауза f1", || async move { matches!(status(s, &f1.id), WindowStatus::Paused { .. }) }).await;
    assert_eq!(status(&session, &f1.id), WindowStatus::Paused { code: "file".into(), detail: String::new() });
    let _ = f2.page.answer_dialog(true, "").await;
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "запускает настоящий браузер; cargo test --lib sync::live -- --ignored --nocapture --test-threads=1"]
async fn main_closed_ends_session_and_frees_profiles() {
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let sink = events.clone();
    let live = live_with_sink(Arc::new(move |e| sink.lock().unwrap().push(e))).await;
    let (main, followers) = windows(&live, &["/sync"]).await;
    let (main, followers) = fresh_session(&live, &main, &followers).await;
    let f1 = &followers[0];
    let started = live.core.sync_state();
    assert!(started.active);
    assert_eq!(f1.browser.humanize_enabled().await, Some(false));
    assert!(live.core.flows.claim(&f1.id).is_none(), "профиль сессии занят");
    assert_eq!(live.core.sync_start(&main.id, std::slice::from_ref(&f1.id)).await.unwrap_err().code, "SyncRunning");

    live.core.sync_release(&f1.id).await.unwrap();
    main.browser.close().await;
    eventually("конец сессии", || async { !live.core.sync_state().active }).await;
    eventually("профили свободны", || async { live.core.flows.claim(&f1.id).is_some() }).await;
    assert!(!live.core.supervisor.status(&f1.id).is_active(), "закрытие мастера закрыло даже приостановленного ведомого");
    let idle = events.lock().unwrap().iter().any(|e| matches!(e, Event::SyncChanged(s) if !s.active));
    assert!(idle, "SyncChanged с active: false не пришёл");
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "настоящий браузер, изолированные профили"]
async fn live_script_tabs_do_not_shift_manual_tab_replay() {
    let live = live().await;
    live.core.store.raw().execute("UPDATE profiles SET extra_args = '[\"--disable-popup-blocking\"]' WHERE id = ?1", [&live.quiet]).unwrap();
    let (main, followers) = windows(&live, &["/sync"]).await;
    let follower = &followers[0];
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;
    for (i, features) in ["", "noopener"].iter().enumerate() {
        main.page.eval_main(&format!("window.open('{SITE}/a', '_blank', '{features}'); true"), &[]).await.unwrap();
        eventually("вкладка скрипта только у мастера", || async { main.browser.tabs().await.unwrap().len() == i + 2 }).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(follower.browser.tabs().await.unwrap().len(), 1);
    let manual = main.browser.new_tab(&format!("{SITE}/b")).await.unwrap();
    eventually("ручная вкладка скопирована", || async { follower.browser.tabs().await.unwrap().len() == 2 }).await;
    let page = main.browser.attach(&manual).await.unwrap();
    page.goto(&format!("{SITE}/js"), LOAD).await.unwrap();
    eventually("переход попал в правильную вкладку", || async { follower.browser.tabs().await.unwrap().iter().any(|t| t.url == format!("{SITE}/js")) }).await;
    assert_eq!(follower.path().await, "/sync");
    main.browser.close_tab(&manual).await.unwrap();
    eventually("закрыта только ручная вкладка", || async { follower.browser.tabs().await.unwrap().len() == 1 }).await;
    assert_eq!(status(&session, &follower.id), WindowStatus::Synced);
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "настоящий браузер, изолированные профили"]
async fn live_followers_follow_master_size_without_resizing_master() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync"]).await;
    let follower = &followers[0];
    let mut cdp = CdpClient::connect(&main.ws).await.unwrap();
    let window = cdp.window_for_first_page().await.unwrap();
    let bounds = cdp.window_bounds(window).await.unwrap();
    cdp.set_window_bounds(window, &crate::engine::cdp::Bounds { width: 1200, height: 850, ..bounds }).await.unwrap();
    let mut other = CdpClient::connect(&follower.ws).await.unwrap();
    let other_window = other.window_for_first_page().await.unwrap();
    let other_bounds = other.window_bounds(other_window).await.unwrap();
    other.set_window_bounds(other_window, &crate::engine::cdp::Bounds { width: 800, height: 600, ..other_bounds }).await.unwrap();
    let (session, _) = start(&main, &followers, SyncSettings::default()).await;
    assert_eq!(main.size().await, (1200, 850), "размер мастера не меняется");
    eventually("начальный размер ведомого", || async { follower.size().await == main.size().await }).await;
    cdp.set_window_bounds(window, &crate::engine::cdp::Bounds { width: 1050, height: 750, ..bounds }).await.unwrap();
    eventually("ведомый повторил изменение размера", || async { follower.size().await == main.size().await }).await;
    assert_eq!(main.size().await, (1050, 750));
    session.stop().await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "настоящий браузер, изолированные профили"]
async fn live_clean_start_ignores_browser_restore_preferences() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync-other"]).await;
    for window in std::iter::once(&main).chain(&followers) {
        live.core.supervisor.stop(&window.id).await.unwrap();
        let path = live.core.paths.profile_dir(&window.id).join("Default/Preferences");
        let mut prefs: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        prefs["session"]["restore_on_startup"] = json!(1);
        std::fs::write(path, serde_json::to_vec(&prefs).unwrap()).unwrap();
    }
    let (main, followers) = fresh_session(&live, &main, &followers).await;
    let urls: Vec<_> = main.browser.tabs().await.unwrap().into_iter().map(|t| t.url).collect();
    let clean = urls.len() == 1 && (urls[0] == "about:blank" || urls[0].starts_with("chrome://newtab"));
    live.core.sync_stop().await;
    live.core.supervisor.stop_all().await;
    assert!(clean, "синхронизация не восстанавливает вкладки даже по настройке Chromium: {urls:?}");
    drop(followers);
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "macOS Accessibility, OMBRA_PROBE_HEADFUL=1 и OMBRA_QA_ROOT с qa_native.py"]
async fn native_plus_and_shortcut_tabs_follow_master() {
    assert!(std::env::var_os("OMBRA_PROBE_HEADFUL").is_some(), "нужны настоящие окна");
    let qa = std::env::var("OMBRA_QA_ROOT").expect("путь репозитория QA с NativeUI");
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync"]).await;
    let (main, followers) = fresh_session(&live, &main, &followers).await;
    let follower = &followers[0];
    let pid = live.core.supervisor.running_info(&main.id).unwrap().pid;
    let control = |action: &str| {
        let script = r#"
import sys,time
sys.path.insert(0,sys.argv[1])
from qa_native import NativeUI
ui=NativeUI(int(sys.argv[2]))
try:
    ui.activate()
    time.sleep(0.3)
    if sys.argv[3]=='plus':
        buttons=[r for r in ui.tree() if r['role']=='AXButton' and any(label in (r['title']+' '+r['description']).lower() for label in ('new tab','новая вкладка'))]
        assert len(buttons)==1, 'Expected the native + button: '+repr([(r['title'],r['description']) for r in buttons])
        ui.press(buttons[0]['element'])
        print('Native +:',buttons[0]['description'] or buttons[0]['title'])
    else:
        ui.key(17,1<<20)
        print('Native Cmd+T')
finally:
    ui.close()
"#;
        let result = std::process::Command::new("python3").args(["-c", script, &qa, &pid.to_string(), action]).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        println!("{}", String::from_utf8_lossy(&result.stdout));
    };
    for (index, action) in ["plus", "shortcut"].into_iter().enumerate() {
        control(action);
        let want = index + 2;
        eventually("нативная вкладка мастера", || async { main.browser.tabs().await.unwrap().len() == want }).await;
        eventually("нативная вкладка повторена", || async { follower.browser.tabs().await.unwrap().len() == want }).await;
        assert!(main.browser.tabs().await.unwrap().iter().all(|tab| !tab.from_page));
        eventually("одинаковые адреса нативных вкладок", || async {
            let mut master: Vec<_> = main.browser.tabs().await.unwrap().into_iter().map(|t| t.url).collect();
            let mut other: Vec<_> = follower.browser.tabs().await.unwrap().into_iter().map(|t| t.url).collect();
            master.sort();
            other.sort();
            master == other
        }).await;
    }
    live.core.supervisor.stop(&main.id).await.unwrap();
    eventually("ведомый закрыт", || async { !live.core.supervisor.status(&follower.id).is_active() }).await;
    live.core.supervisor.stop_all().await;
}

#[tokio::test]
#[ignore = "настоящий браузер, изолированные профили"]
async fn cancelled_sync_stop_finishes_cleanup_and_releases_claims() {
    let live = live().await;
    let (main, followers) = windows(&live, &["/sync"]).await;
    let (main, followers) = fresh_session(&live, &main, &followers).await;
    let session = live.core.sync.lock().await.as_ref().unwrap().0.clone();
    let core = Arc::new(live.core);
    let stopping = core.clone();
    let caller = tokio::spawn(async move { stopping.sync_stop().await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while session.state().active {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled(), "вызов отменён именно во время очистки");
    let ended = tokio::time::timeout(Duration::from_secs(5), session.ended()).await.is_ok();
    if ended {
        eventually("профили освобождены после отмены ожидания", || async {
            std::iter::once(&main).chain(&followers).all(|w| core.flows.claim(&w.id).is_some())
        }).await;
        assert!(core.sync.lock().await.is_none());
        assert!(std::iter::once(&main).chain(&followers).all(|w| core.supervisor.running_info(&w.id).is_some()));
    }
    core.supervisor.stop_all().await;
    assert!(ended, "прерывание вызывающего не должно бросать очистку синхронизатора");
}
