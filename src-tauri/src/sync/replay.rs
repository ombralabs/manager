//! Повтор в ведомом окне (спецификация синхронизатора, «Повтор в ведомых окнах», «Отклонение и пауза»): какие действия
//! главного уходят ведомым, задержки «С вариациями» и само действие во вкладке ведомого.
use super::model::{Action, NavStep, Signature, TYPING_IDLE, parse, soft_match};
use super::{Mode, SyncSettings};
use crate::engine::cdp::CdpError;
use crate::engine::page::{Button, Page};
use crate::engine::sync::Captured;
use crate::fingerprint::{OsRandom, RandomSource};
use std::time::{Duration, Instant};

/// Причина паузы: код и подстановка.
pub(super) type Pause = (&'static str, String);

/// Переход ведомого ждёт загрузки не дольше этого: следующий клик — уже по новому документу.
pub(super) const NAV_WAIT: Duration = Duration::from_secs(30);
/// Диалог страницы ведомого должен открыться за это время после ответа главного; лишний — закрыться за столько же.
pub(super) const DIALOG_WAIT: Duration = Duration::from_secs(2);

/// Действие события перехвата, если его вид включён в настройках («Что повторять»).
pub(super) fn wanted(settings: &SyncSettings, captured: &Captured) -> Option<Action> {
    let action = parse(&captured.event)?;
    let on = match &action {
        // «Движения мыши» — наведение; движение с нажатой кнопкой — перетаскивание, часть жеста клика (R19).
        Action::Move { buttons: 0, .. } => settings.mouse,
        Action::Move { .. } | Action::Down { .. } | Action::Up { .. } | Action::Select { .. } => settings.clicks,
        Action::Wheel { .. } => settings.scroll,
        // Нажатия в нативном списке ведомым не идут — вариант выберет `select`; Tab уводит из списка фокус.
        Action::Key(key) if key.sig.as_ref().is_some_and(|sig| sig.tag == "SELECT") && key.key != "Tab" => false,
        Action::Key(_) | Action::Text { .. } => settings.typing,
        Action::Visible => true,
    };
    on.then_some(action)
}

/// «С вариациями»: случайная задержка из «задержки клика» перед кликом, выбором варианта и серией прокрутки (перед
/// первым её шагом, `wheel_series`), из «задержки набора» — перед куском набора и нажатием клавиши.
pub(super) fn delay(settings: &SyncSettings, action: &Action) -> Option<Duration> {
    if settings.mode != Mode::Varied {
        return None;
    }
    let [from, to] = match action {
        Action::Down { .. } | Action::Select { .. } | Action::Wheel { .. } => settings.click_delay,
        Action::Text { .. } => settings.typing_delay,
        Action::Key(key) if key.down => settings.typing_delay,
        _ => return None,
    };
    let span = u64::from(to.saturating_sub(from)) + 1;
    Some(Duration::from_millis(u64::from(from) + OsRandom.next_u64() % span))
}

/// Колесо продолжает серию прокрутки — прежнее было меньше паузы назад (пауза — как у набора): своей задержки у него
/// нет. `last` — когда было прежнее колесо.
pub(super) fn wheel_series(last: &mut Option<Instant>, action: &Action, now: Instant) -> bool {
    matches!(action, Action::Wheel { .. }) && last.replace(now).is_some_and(|at| now.saturating_duration_since(at) < TYPING_IDLE)
}

/// Действие главного во вкладке ведомого доверенным вводом `Input.*`. Клики — по тем же координатам,
/// набор — после сверки поля в фокусе; поле не совпало — действие не делается, ответ — причина паузы.
pub(super) async fn replay(page: &Page, action: &Action, focus: &mut Option<Signature>) -> Result<Option<Pause>, CdpError> {
    let checked = match action {
        Action::Down { .. } | Action::Up { .. } | Action::Select { .. } => {
            *focus = None;
            Ok(None)
        }
        Action::Key(key) if key.down && key.sig.as_ref().is_some_and(|sig| sig.tag != "SELECT") => check_focus(page, key.sig.as_ref().unwrap(), focus).await,
        Action::Text { sig, .. } => check_focus(page, sig, focus).await,
        _ => Ok(None),
    };
    if let Some(pause) = checked? { return Ok(Some(pause)); }
    match action {
        Action::Move { x, y, buttons, mods } => {
            let pressed = [Button::Left, Button::Right, Button::Middle].into_iter().find(|b| buttons & b.mask() != 0);
            page.mouse("mouseMoved", *x, *y, pressed, *buttons, 0, *mods).await?
        }
        Action::Down { x, y, button, buttons, count, mods, select, .. } => {
            if !select { page.mouse("mousePressed", *x, *y, Some(*button), *buttons, *count, *mods).await? }
        }
        Action::Up { select: true, .. } => {}
        Action::Up { x, y, button, buttons, count, mods, .. } => page.mouse("mouseReleased", *x, *y, Some(*button), *buttons, *count, *mods).await?,
        Action::Wheel { x, y, dx, dy, mods } => page.wheel_by(*x, *y, *dx, *dy, *mods).await?,
        Action::Key(key) => page.key(key).await?,
        Action::Text { text, .. } => page.insert_chunk(text).await?,
        Action::Select { x, y, label, .. } => {
            if !page.select_by_label(*x, *y, label).await? { return Ok(Some(("no_option", label.clone()))); }
        }
        Action::Visible => {}
    }
    Ok(None)
}

/// Переход из интерфейса браузера главного; загрузка ждётся, чтобы следующее действие попало в новый документ.
pub(super) async fn navigate(page: &Page, step: &NavStep) -> Result<Option<Pause>, CdpError> {
    match step {
        NavStep::Reload => page.reload(NAV_WAIT).await?,
        NavStep::History(delta) => {
            if !page.go_history(*delta, NAV_WAIT).await? {
                return Ok(Some(("failed", "нет такой записи в истории".into())));
            }
        }
        NavStep::Goto(url) => match page.goto(url, NAV_WAIT).await {
            // Сетевая ошибка — `page_error` сторожа вкладки, если у главного её не было.
            Err(CdpError::Remote(_)) => {}
            other => other?,
        },
    }
    Ok(None)
}

/// Ответ главного на диалог страницы; `false` — у ведомого диалог так и не открылся.
pub(super) async fn answer_dialog(page: &Page, accept: bool, text: &str) -> bool {
    let deadline = Instant::now() + DIALOG_WAIT;
    loop {
        match page.answer_dialog(accept, text).await {
            Ok(()) => return true,
            // Диалога ещё нет («No dialog is showing»): его откроет действие, идущее следом.
            Err(_) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(_) => return false,
        }
    }
}

/// Фокус ведомого — в таком же поле, как у главного. Сверяется на первом нажатии в поле: подпись — без текста, он
/// меняется с каждой буквой.
async fn check_focus(page: &Page, sig: &Signature, checked: &mut Option<Signature>) -> Result<Option<Pause>, CdpError> {
    let field = Signature { text: String::new(), ..sig.clone() };
    if checked.as_ref() != Some(&field) {
        if !soft_match(sig, page.focused_signature().await?.as_ref()) {
            return Ok(Some(("focus", String::new())));
        }
        *checked = Some(field);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::page::Browser;
    use crate::engine::page::tests::{Mock, base};
    use serde_json::json;

    #[tokio::test]
    async fn native_select_stays_in_main() {
        let key = |key: &str, tag: &str| Captured { frame_id: "T1".into(), event: json!({ "t": "key", "kind": "down", "key": key, "sig": { "tag": tag } }) };
        let s = SyncSettings::default();
        assert!(wanted(&s, &key("Б", "SELECT")).is_none(), "буква в списке ведомым не идёт");
        assert!(wanted(&s, &key("Tab", "SELECT")).is_some(), "Tab уводит фокус из списка");
        assert!(wanted(&s, &key("Б", "INPUT")).is_some());

        // Под точкой у ведомого — список.
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 9 }),
                "Runtime.evaluate" => json!({ "result": { "type": "object", "value": { "tag": "SELECT", "type": "select-one", "id": "s" } } }),
                _ => json!({}),
            })
        })
        .await;
        let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
        let sig = |tag: &str| Signature { tag: tag.into(), kind: "select-one".into(), id: "s".into(), ..Default::default() };
        let down = |sig| Action::Down { x: 1.0, y: 2.0, button: crate::engine::page::Button::Left, buttons: 1, count: 1, mods: 0, sig, select: true };
        let up = Action::Up { x: 1.0, y: 2.0, button: crate::engine::page::Button::Left, buttons: 0, count: 1, mods: 0, select: true };
        assert_eq!(replay(&page, &down(sig("SELECT")), &mut None).await.unwrap(), None);
        assert_eq!(replay(&page, &up, &mut None).await.unwrap(), None);
        assert!(mock.sent("Input.dispatchMouseEvent").is_empty(), "нативный список у ведомого не открывается");
        let other = replay(&page, &down(sig("DIV")), &mut None).await.unwrap();
        assert_eq!(other, None, "различие подписи не ставит окно на паузу");
    }

    #[tokio::test]
    async fn clicks_replay_at_the_same_point_despite_different_or_missing_elements() {
        for there in [json!({"tag":"A","id":"other"}), json!({"tag":"BUTTON","id":"variant-b"}), json!(null)] {
            let mock = Mock::spawn(move |method, _, _| base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({"executionContextId":1}),
                "Runtime.evaluate" => json!({"result":{"value":there}}),
                _ => json!({}),
            })).await;
            let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
            let down = Action::Down { x:80.0,y:40.0,button:Button::Left,buttons:1,count:1,mods:0,
                sig:Signature { tag:"BUTTON".into(),id:"variant-a".into(),..Default::default() },select:false };
            let up = Action::Up { x:80.0,y:40.0,button:Button::Left,buttons:0,count:1,mods:0,select:false };
            assert_eq!(replay(&page, &down, &mut None).await.unwrap(), None, "DOM differences must not pause a coordinate click");
            assert_eq!(replay(&page, &up, &mut None).await.unwrap(), None);
            let events = mock.sent("Input.dispatchMouseEvent");
            assert_eq!(events.iter().map(|p| (p["type"].as_str().unwrap(),p["x"].as_f64().unwrap(),p["y"].as_f64().unwrap())).collect::<Vec<_>>(),
                       [("mousePressed",80.0,40.0),("mouseReleased",80.0,40.0)]);
        }
    }

    #[tokio::test]
    async fn typing_into_a_different_field_still_pauses() {
        let mock = Mock::spawn(|method, _, _| base(method).unwrap_or_else(|| match method {
            "Page.createIsolatedWorld" => json!({"executionContextId":1}),
            "Runtime.evaluate" => json!({"result":{"value":{"tag":"INPUT","id":"other"}}}),
            _ => json!({}),
        })).await;
        let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
        let action = Action::Text { text:"hello".into(),sig:Signature { tag:"INPUT".into(),id:"wanted".into(),..Default::default() } };
        assert_eq!(replay(&page, &action, &mut None).await.unwrap(), Some(("focus", String::new())));
        assert!(mock.sent("Input.insertText").is_empty());
    }

    #[tokio::test]
    async fn coordinate_click_invalidates_cached_focus_before_more_typing() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let clicked = AtomicBool::new(false);
        let mock = Mock::spawn(move |method, _, _| base(method).unwrap_or_else(|| match method {
            "Page.createIsolatedWorld" => json!({"executionContextId":1}),
            "Runtime.evaluate" => json!({"result":{"value":{"tag":"INPUT","id":if clicked.load(Ordering::SeqCst) { "other" } else { "wanted" }}}}),
            "Input.dispatchMouseEvent" => { clicked.store(true, Ordering::SeqCst); json!({}) }
            _ => json!({}),
        })).await;
        let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
        let sig = Signature { tag:"INPUT".into(),id:"wanted".into(),..Default::default() };
        let text = Action::Text { text:"hello".into(),sig:sig.clone() };
        let mut focus = None;
        assert_eq!(replay(&page, &text, &mut focus).await.unwrap(), None);
        let down = Action::Down { x:80.0,y:40.0,button:Button::Left,buttons:1,count:1,mods:0,sig,select:false };
        assert_eq!(replay(&page, &down, &mut focus).await.unwrap(), None);
        assert_eq!(replay(&page, &text, &mut focus).await.unwrap(), Some(("focus", String::new())));
        assert_eq!(mock.sent("Input.insertText").len(), 1, "the second text must not reach the changed follower field");
    }

    #[test]
    fn wheel_series_ends_on_a_pause() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let wheel = Action::Wheel { x: 0.0, y: 0.0, dx: 0.0, dy: 100.0, mods: 0 };
        let mut last = None;
        let series: Vec<bool> = [(0, &wheel), (100, &wheel), (300, &Action::Visible), (500, &wheel), (1100, &wheel)]
            .into_iter()
            .map(|(at, action)| wheel_series(&mut last, action, t0 + ms(at)))
            .collect();
        assert_eq!(series, [false, true, false, true, false], "серию кончает пауза, а не другое действие");
    }

    #[tokio::test]
    async fn drag_moves_keep_the_pressed_button() {
        let mock = Mock::spawn(|method, _, _| base(method).unwrap_or_else(|| json!({}))).await;
        let page = Browser::connect(&mock.url).await.unwrap().attach("T1").await.unwrap();
        for buttons in [1, 2, 0] {
            replay(&page, &Action::Move { x: 5.0, y: 6.0, buttons, mods: 8 }, &mut None).await.unwrap();
        }
        let sent: Vec<_> = mock.sent("Input.dispatchMouseEvent").iter().map(|m| (m["type"].clone(), m["button"].clone(), m["buttons"].clone(), m["modifiers"].clone())).collect();
        let moved = |button: &str, buttons: i64| (json!("mouseMoved"), json!(button), json!(buttons), json!(8));
        assert_eq!(sent, [moved("left", 1), moved("right", 2), moved("none", 0)]);
    }

    #[test]
    fn settings_choose_kinds_and_delays() {
        let event = |t: &str| {
            // Перетаскивание — движение с нажатой кнопкой.
            let (t, buttons) = if t == "drag" { ("move", 1) } else { (t, 0) };
            Captured { frame_id: "T1".into(), event: json!({ "t": t, "kind": "down", "key": "Enter", "buttons": buttons }) }
        };
        let kinds = [("move", "mouse"), ("drag", "clicks"), ("down", "clicks"), ("up", "clicks"), ("wheel", "scroll"), ("key", "typing"), ("visible", "")];
        for off in ["mouse", "clicks", "scroll", "typing"] {
            let mut s = SyncSettings::default();
            *match off {
                "mouse" => &mut s.mouse,
                "clicks" => &mut s.clicks,
                "scroll" => &mut s.scroll,
                _ => &mut s.typing,
            } = false;
            for (t, kind) in kinds {
                assert_eq!(wanted(&s, &event(t)).is_some(), kind != off, "выключено {off}, событие {t}");
            }
        }

        let s = SyncSettings { mode: Mode::Varied, click_delay: [100, 200], typing_delay: [300, 300], ..Default::default() };
        let action = |t: &str| parse(&event(t).event).unwrap();
        for _ in 0..20 {
            assert!((100..=200).contains(&delay(&s, &action("down")).unwrap().as_millis()));
        }
        assert_eq!(delay(&s, &action("key")), Some(Duration::from_millis(300)));
        assert!((100..=200).contains(&delay(&s, &action("wheel")).unwrap().as_millis()), "серия прокрутки — как клик");
        assert_eq!(delay(&s, &action("up")), None);
        assert_eq!(delay(&SyncSettings::default(), &action("down")), None, "«Полная копия» повторяет сразу");
    }
}
