//! Модель синхронизатора без ввода-вывода: события скрипта перехвата → действия, подпись цели, куски набора, переходы.
use crate::engine::page::Button;
use serde_json::Value;
use std::time::{Duration, Instant};

/// Пауза набора, закрывающая кусок в режиме «С вариациями».
pub(crate) const TYPING_IDLE: Duration = Duration::from_millis(500);

/// Подпись цели: по ней сверяются окна. `text` — только для текста причины паузы.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Signature {
    pub tag: String,
    pub role: String,
    pub kind: String,
    pub id: String,
    pub name: String,
    pub text: String,
}

impl Signature {
    /// Название цели для причины паузы: «кнопка «Купить»», «ссылка «Подробнее»», иначе тег.
    pub fn describe(&self) -> String {
        let what = match self.tag.as_str() {
            "BUTTON" => "кнопка",
            "A" => "ссылка",
            _ => return self.tag.clone(),
        };
        if self.text.is_empty() {
            what.to_string()
        } else {
            format!("{what} «{}»", self.text)
        }
    }

    pub(crate) fn from_json(v: &Value) -> Self {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        Self { tag: s("tag"), role: s("role"), kind: s("type"), id: s("id"), name: s("name"), text: s("text") }
    }
}

/// Мягкая сверка: тег, `role`, `type`, а `id` и `name` — если они есть у цели главного. Текст и `href` не сверяются.
pub fn soft_match(main: &Signature, follower: Option<&Signature>) -> bool {
    let Some(f) = follower else { return false };
    main.tag == f.tag
        && main.role == f.role
        && main.kind == f.kind
        && (main.id.is_empty() || main.id == f.id)
        && (main.name.is_empty() || main.name == f.name)
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyEvent {
    pub down: bool,
    pub key: String,
    pub code: String,
    pub key_code: i64,
    pub mods: i64,
    pub location: i64,
    pub sig: Option<Signature>,
}

impl KeyEvent {
    /// Один символ без ctrl и meta: он попадает в текст.
    pub fn printable(&self) -> Option<&str> {
        (self.key.chars().count() == 1 && self.mods & (2 | 4) == 0).then_some(self.key.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `buttons` — маска нажатых кнопок: не 0 — перетаскивание.
    Move { x: f64, y: f64, buttons: i64, mods: i64 },
    Down { x: f64, y: f64, button: Button, buttons: i64, count: i64, mods: i64, sig: Signature, select: bool },
    Up { x: f64, y: f64, button: Button, buttons: i64, count: i64, mods: i64, select: bool },
    Wheel { x: f64, y: f64, dx: f64, dy: f64, mods: i64 },
    Key(KeyEvent),
    Text { text: String, sig: Signature },
    Select { x: f64, y: f64, label: String, sig: Signature },
    Visible,
}

/// Событие скрипта перехвата → действие; незнакомое или неполное — `None`.
pub fn parse(event: &Value) -> Option<Action> {
    let f = |k: &str| event[k].as_f64().unwrap_or(0.0);
    let i = |k: &str| event[k].as_i64().unwrap_or(0);
    let sig = || Signature::from_json(&event["sig"]);
    let (x, y) = (f("x"), f("y"));
    // Боковые кнопки (3, 4) — «Назад» и «Вперёд» браузера: шаг по истории ведомым идёт `Nav`, нажатие — не ввод страницы.
    let button = || match i("button") {
        0 => Some(Button::Left),
        1 => Some(Button::Middle),
        2 => Some(Button::Right),
        _ => None,
    };
    Some(match event["t"].as_str()? {
        "move" => Action::Move { x, y, buttons: i("buttons"), mods: i("mods") },
        "down" => Action::Down {
            x,
            y,
            button: button()?,
            buttons: i("buttons"),
            count: i("detail"),
            mods: i("mods"),
            sig: sig(),
            select: event["select"].as_bool().unwrap_or(false),
        },
        "up" => Action::Up {
            x,
            y,
            button: button()?,
            buttons: i("buttons"),
            count: i("detail"),
            mods: i("mods"),
            select: event["select"].as_bool().unwrap_or(false),
        },
        "wheel" => Action::Wheel { x, y, dx: f("dx"), dy: f("dy"), mods: i("mods") },
        "key" => Action::Key(KeyEvent {
            down: event["kind"].as_str()? == "down",
            key: event["key"].as_str()?.to_string(),
            code: event["code"].as_str().unwrap_or_default().to_string(),
            key_code: i("keyCode"),
            mods: i("mods"),
            location: i("location"),
            sig: event["sig"].is_object().then(sig),
        }),
        "select" => Action::Select { x, y, label: event["label"].as_str().unwrap_or_default().to_string(), sig: sig() },
        "visibility" | "visible" => Action::Visible,
        _ => return None,
    })
}

/// Режим «С вариациями»: печатные нажатия копятся в `Text`, подряд идущие `Move` схлопываются в последний.
#[derive(Default)]
pub struct Chunker {
    text: String,
    sig: Option<Signature>,
    last_key: Option<Instant>,
    mv: Option<Action>,
}

impl Chunker {
    pub fn push(&mut self, action: Action, at: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        // Пауза набора закрывает кусок до нового действия.
        if !self.text.is_empty() && self.last_key.is_some_and(|t| at.saturating_duration_since(t) >= TYPING_IDLE) {
            out.push(self.take_text());
        }
        match action {
            Action::Move { .. } => {
                out.extend(self.take_text_opt());
                self.mv = Some(action);
            }
            Action::Key(ref k) if k.printable().is_some() => {
                out.extend(self.mv.take());
                // Отпускание печатной клавиши в `Text` не нужно.
                if k.down {
                    self.text.push_str(k.printable().unwrap_or_default());
                    self.sig.get_or_insert_with(|| k.sig.clone().unwrap_or_default());
                    self.last_key = Some(at);
                }
            }
            other => {
                out.extend(self.mv.take());
                out.extend(self.take_text_opt());
                out.push(other);
            }
        }
        out
    }

    /// Всё накопленное, когда действий больше не будет.
    pub fn flush(&mut self) -> Vec<Action> {
        self.mv.take().into_iter().chain(self.take_text_opt()).collect()
    }

    fn take_text_opt(&mut self) -> Option<Action> {
        (!self.text.is_empty()).then(|| self.take_text())
    }

    fn take_text(&mut self) -> Action {
        Action::Text { text: std::mem::take(&mut self.text), sig: self.sig.take().unwrap_or_default() }
    }
}

/// Снимок истории вкладки (`Page.getNavigationHistory`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    pub index: usize,
    pub ids: Vec<i64>,
    pub transitions: Vec<String>,
    pub urls: Vec<String>,
    /// Адрес, набранный пользователем (`userTypedURL`; пусто — не набирали): `urls` — уже после перенаправлений.
    pub typed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavStep {
    Reload,
    History(i64),
    Goto(String),
}

/// Что сделал пользователь в главном окне: перезагрузка, шаг по истории или ввод адреса. Переход по ссылке, отправка
/// формы и прочее — клики, их повторяет ввод, поэтому `None`.
pub fn classify_navigation(before: &History, after: &History) -> Option<NavStep> {
    let id = *after.ids.get(after.index)?;
    if before.ids.contains(&id) {
        return match after.index as i64 - before.index as i64 {
            0 => Some(NavStep::Reload),
            d => Some(NavStep::History(d)),
        };
    }
    let typed = ["typed", "address_bar", "auto_bookmark", "generated", "keyword", "keyword_generated"];
    if !typed.contains(&after.transitions.get(after.index)?.as_str()) {
        return None;
    }
    // Набранный адрес, а не конечный: перенаправления (например, по стране) ведомые проходят сами.
    let url = after.typed.get(after.index).filter(|url| !url.is_empty()).or(after.urls.get(after.index))?;
    Some(NavStep::Goto(url.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sig(tag: &str, id: &str, text: &str) -> Signature {
        Signature { tag: tag.into(), id: id.into(), text: text.into(), ..Default::default() }
    }

    #[test]
    fn soft_match_ignores_text_and_href() {
        assert!(soft_match(&sig("BUTTON", "", "Купить за 100"), Some(&sig("BUTTON", "", "Купить за 90"))));
        assert!(!soft_match(&sig("BUTTON", "", ""), Some(&sig("A", "", ""))));
        assert!(!soft_match(&sig("BUTTON", "a", ""), Some(&sig("BUTTON", "b", ""))));
        assert!(soft_match(&sig("BUTTON", "", ""), Some(&sig("BUTTON", "z", ""))));
        assert!(!soft_match(&sig("BUTTON", "", ""), None));
        assert_eq!(sig("BUTTON", "", "Купить").describe(), "кнопка «Купить»");
        assert_eq!(sig("A", "", "Подробнее").describe(), "ссылка «Подробнее»");
        assert_eq!(sig("DIV", "", "x").describe(), "DIV");
    }

    #[test]
    fn parse_reads_script_events() {
        let s = json!({"tag":"BUTTON","role":"r","type":"submit","id":"i","name":"n","text":"Да"});
        let want = Signature { tag: "BUTTON".into(), role: "r".into(), kind: "submit".into(), id: "i".into(), name: "n".into(), text: "Да".into() };
        assert_eq!(parse(&json!({"t":"move","x":1.5,"y":2.0})), Some(Action::Move { x: 1.5, y: 2.0, buttons: 0, mods: 0 }));
        assert_eq!(parse(&json!({"t":"move","x":1,"y":2,"buttons":1,"mods":8})), Some(Action::Move { x: 1.0, y: 2.0, buttons: 1, mods: 8 }));
        assert_eq!(
            parse(&json!({"t":"down","x":3,"y":4,"button":2,"buttons":2,"detail":2,"mods":8,"sig":s,"select":true})),
            Some(Action::Down { x: 3.0, y: 4.0, button: Button::Right, buttons: 2, count: 2, mods: 8, sig: want.clone(), select: true })
        );
        assert_eq!(
            parse(&json!({"t":"up","x":3,"y":4,"button":0,"buttons":0,"detail":1,"mods":0,"sig":s,"select":false})),
            Some(Action::Up { x: 3.0, y: 4.0, button: Button::Left, buttons: 0, count: 1, mods: 0, select: false })
        );
        assert_eq!(
            parse(&json!({"t":"wheel","x":1,"y":2,"dx":0,"dy":120.0,"mods":0})),
            Some(Action::Wheel { x: 1.0, y: 2.0, dx: 0.0, dy: 120.0, mods: 0 })
        );
        assert_eq!(
            parse(&json!({"t":"key","kind":"up","key":"a","code":"KeyA","keyCode":65,"mods":2,"repeat":false,"location":0,"sig":s})),
            Some(Action::Key(KeyEvent { down: false, key: "a".into(), code: "KeyA".into(), key_code: 65, mods: 2, location: 0, sig: Some(want.clone()) }))
        );
        assert_eq!(
            parse(&json!({"t":"key","kind":"down","key":"a","code":"KeyA","keyCode":65,"mods":0,"location":0,"sig":null})),
            Some(Action::Key(KeyEvent { down: true, key: "a".into(), code: "KeyA".into(), key_code: 65, mods: 0, location: 0, sig: None }))
        );
        assert_eq!(
            parse(&json!({"t":"select","x":5,"y":6,"label":"Бета","sig":s})),
            Some(Action::Select { x: 5.0, y: 6.0, label: "Бета".into(), sig: want })
        );
        assert_eq!(parse(&json!({"t":"visible"})), Some(Action::Visible));
        assert_eq!(parse(&json!({"t":"nope"})), None);
        // Боковые кнопки мыши — «Назад» и «Вперёд» браузера: шаг по истории придёт ведомым `Nav`, а не кликом.
        for (t, button) in [("down", 3), ("up", 3), ("down", 4), ("up", 4)] {
            assert_eq!(parse(&json!({"t":t,"x":3,"y":4,"button":button,"buttons":0,"detail":1,"mods":0,"sig":s})), None, "{t} {button}");
        }
    }

    fn key(down: bool, k: &str, mods: i64) -> Action {
        Action::Key(KeyEvent { down, key: k.into(), code: String::new(), key_code: 0, mods, location: 0, sig: Some(sig("INPUT", "q", "")) })
    }

    fn text(t: &str) -> Action {
        Action::Text { text: t.into(), sig: sig("INPUT", "q", "") }
    }

    #[test]
    fn chunker_merges_typing_and_moves() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut c = Chunker::default();
        assert!(c.push(key(true, "а", 0), t0).is_empty());
        assert!(c.push(key(false, "а", 0), t0 + ms(50)).is_empty());
        assert!(c.push(key(true, "б", 0), t0 + ms(100)).is_empty());
        assert_eq!(c.push(key(true, "Enter", 0), t0 + ms(200)), [text("аб"), key(true, "Enter", 0)]);
        assert_eq!(c.push(key(false, "Enter", 0), t0 + ms(210)), [key(false, "Enter", 0)]);

        // Пауза набора закрывает кусок до следующей клавиши.
        let mut c = Chunker::default();
        c.push(key(true, "а", 0), t0);
        assert!(c.push(key(false, "а", 0), t0 + ms(30)).is_empty());
        assert_eq!(c.push(key(true, "б", 0), t0 + ms(630)), [text("а")]);
        assert_eq!(c.flush(), [text("б")]);

        let mv = |x| Action::Move { x, y: 0.0, buttons: 0, mods: 0 };
        let up = Action::Up { x: 0.0, y: 0.0, button: Button::Left, buttons: 0, count: 1, mods: 0, select: false };
        let mut c = Chunker::default();
        assert!(c.push(mv(1.0), t0).is_empty());
        assert!(c.push(mv(2.0), t0).is_empty());
        assert!(c.push(mv(3.0), t0).is_empty());
        assert_eq!(c.push(up.clone(), t0), [mv(3.0), up]);
        assert!(c.push(mv(4.0), t0).is_empty());
        assert_eq!(c.flush(), [mv(4.0)]);

        // ⌘A — не печатное.
        assert!(matches!(key(true, "a", 4), Action::Key(k) if k.printable().is_none()));
        let mut c = Chunker::default();
        assert_eq!(c.push(key(true, "a", 4), t0), [key(true, "a", 4)]);
    }

    fn hist(index: usize, ids: &[i64], tr: &[&str]) -> History {
        let urls = ids.iter().map(|i| format!("u{i}")).collect();
        History { index, ids: ids.into(), transitions: tr.iter().map(|s| s.to_string()).collect(), urls, typed: vec![String::new(); ids.len()] }
    }

    #[test]
    fn classify_navigation_cases() {
        let a = hist(1, &[1, 2, 3], &["typed", "link", "link"]);
        assert_eq!(classify_navigation(&a, &a), Some(NavStep::Reload));
        assert_eq!(classify_navigation(&hist(2, &[1, 2, 3], &["typed", "link", "link"]), &a), Some(NavStep::History(-1)));
        assert_eq!(classify_navigation(&a, &hist(3, &[1, 2, 3, 4], &["typed", "link", "link", "typed"])), Some(NavStep::Goto("u4".into())));
        assert_eq!(classify_navigation(&a, &hist(3, &[1, 2, 3, 4], &["typed", "link", "link", "link"])), None);
        assert_eq!(classify_navigation(&a, &hist(3, &[1, 2, 3, 4], &["typed", "link", "link", "form_submit"])), None);
        // Набранный адрес, а не конечный: ведомые проходят свои перенаправления сами.
        let mut typed = hist(3, &[1, 2, 3, 4], &["typed", "link", "link", "typed"]);
        typed.typed[3] = "t4".into();
        assert_eq!(classify_navigation(&a, &typed), Some(NavStep::Goto("t4".into())));
    }
}
