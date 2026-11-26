//! Веб-действия: вкладки, переходы, JS процесса, действия с элементами (спецификация автоматизации, «Контекст страницы», «Ввод»).
use crate::engine::cdp::CdpError;
use crate::engine::page::{Browser, Button, ElementRef, JS_TIMEOUT, Page, Tab};
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::{Cfg, OpError, files, int_or_range, target};
use crate::flow::vars::{self, to_text};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Тайм-аут Refresh Webpage и Go Back — как умолчание Access Website.
pub(crate) const NAV_TIMEOUT_MS: i64 = 30_000;

/// Вкладки запуска: текущая и сессии CDP вкладок, где процесс работал. Сессия подключается при первой операции
/// страницы и живёт, пока вкладка открыта: элементы вкладки годятся и после возврата на неё.
pub struct Tabs {
    browser: Browser,
    current: Option<String>,
    /// Сессии вкладок: id вкладки → страница.
    pages: HashMap<String, Page>,
    /// Элементы запуска и вкладки, где они найдены: на них ссылаются значения `{"$ombra_element": n}`.
    elements: Vec<(String, ElementRef)>,
    /// Порт прослойки профиля этого браузера на 127.0.0.1: загрузки процесса идут через неё.
    relay: Option<u16>,
}

impl Tabs {
    pub fn new(browser: Browser, current: Option<String>) -> Self {
        Self { browser, current, pages: HashMap::new(), elements: Vec::new(), relay: None }
    }

    pub fn with_relay(mut self, relay: Option<u16>) -> Self {
        self.relay = relay;
        self
    }

    /// Список найденных элементов — New Browser переносит его во вложенные вкладки и обратно, чтобы `${el}` снаружи
    /// не указывал внутри на другой элемент (проверка вкладки отвергнет его в чужом браузере).
    pub(crate) fn take_elements(&mut self) -> Vec<(String, ElementRef)> {
        std::mem::take(&mut self.elements)
    }

    pub(crate) fn put_elements(&mut self, elements: Vec<(String, ElementRef)>) {
        self.elements = elements;
    }

    pub(crate) fn relay(&self) -> Option<u16> {
        self.relay
    }

    fn switch(&mut self, target: Option<String>) {
        self.current = target;
    }

    /// Вкладку закрывает процесс: её сессия отцепляется (после закрытия ошибка ожидаема, поэтому её игнорируем).
    async fn forget(&mut self, target: &str) {
        if let Some(page) = self.pages.remove(target) {
            let _ = page.detach().await;
        }
    }

    /// Сессии вкладок, которых больше нет (закрыла страница или пользователь), отпускаются.
    fn prune(&mut self, alive: &[Tab]) {
        self.pages.retain(|id, _| alive.iter().any(|tab| tab.target_id == *id));
    }

    fn current(&self) -> Result<String, OpError> {
        self.current.clone().ok_or_else(|| OpError::Failed("Нет открытых вкладок: сценарий закрыл их все".into()))
    }

    /// Элемент текущей вкладки в переменную: значение-ссылка на него.
    pub(crate) fn keep(&mut self, element: ElementRef) -> Value {
        self.elements.push((self.current.clone().unwrap_or_default(), element));
        vars::element_value(self.elements.len() - 1)
    }

    /// Элемент по значению-ссылке — только на текущей вкладке; вкладка закрыта — элемента больше нет.
    pub(crate) fn element(&self, value: &Value) -> Result<ElementRef, OpError> {
        let (tab, element) =
            vars::element_index(value).and_then(|index| self.elements.get(index)).ok_or_else(|| OpError::Failed("значение не элемент".into()))?;
        if self.current.as_ref() == Some(tab) {
            Ok(element.clone())
        } else if self.pages.contains_key(tab) {
            Err("элемент на другой вкладке".into())
        } else {
            Err("Элемент больше не доступен на странице".into())
        }
    }

    pub(crate) async fn page(&mut self) -> Result<&Page, OpError> {
        let target = self.current()?;
        if !self.pages.contains_key(&target) {
            let page = self.browser.attach(&target).await?;
            self.pages.insert(target.clone(), page);
        }
        Ok(&self.pages[&target])
    }
}

pub(crate) async fn exec(ctx: &mut Ctx, id: &str, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let note = match id {
        "web.goto" => {
            let url = cfg.text(&ctx.scope, "url")?;
            let ms = cfg.int(&ctx.scope, "timeout_ms")?;
            let wait = wait(ms)?;
            navigation(tabs(ctx)?.page().await?.goto(&url, wait).await, ms)?;
            String::new()
        }
        "web.reload" => {
            navigation(tabs(ctx)?.page().await?.reload(wait(NAV_TIMEOUT_MS)?).await, NAV_TIMEOUT_MS)?;
            String::new()
        }
        "web.back" => {
            let moved = navigation(tabs(ctx)?.page().await?.back(wait(NAV_TIMEOUT_MS)?).await, NAV_TIMEOUT_MS)?;
            if moved { String::new() } else { "истории нет".into() }
        }
        "web.new_tab" => {
            let tabs = tabs(ctx)?;
            let target = tabs.browser.new_tab("about:blank").await?;
            tabs.switch(Some(target));
            String::new()
        }
        "web.close_tab" => {
            let tabs = tabs(ctx)?;
            let closed = tabs.current()?;
            tabs.forget(&closed).await;
            tabs.browser.close_tab(&closed).await?;
            // Закрытая вкладка ещё может быть в списке: текущей её не делаем.
            let rest = match tabs.browser.tabs().await {
                Ok(rest) => rest,
                Err(e) => {
                    tabs.switch(None);
                    return Err(e.into());
                }
            };
            tabs.prune(&rest);
            tabs.switch(rest.into_iter().rfind(|tab| tab.target_id != closed).map(|tab| tab.target_id));
            String::new()
        }
        "web.close_other_tabs" => {
            let tabs = tabs(ctx)?;
            let current = tabs.current()?;
            let all = tabs.browser.tabs().await?;
            tabs.prune(&all);
            for tab in all.into_iter().filter(|tab| tab.target_id != current) {
                tabs.forget(&tab.target_id).await;
                tabs.browser.close_tab(&tab.target_id).await?;
            }
            String::new()
        }
        "web.switch_tab" => {
            let value = cfg.text(&ctx.scope, "value")?;
            let (by, how) = (cfg.string("by"), cfg.string("match"));
            let written = to_text(&cfg.raw("value"));
            let tabs = tabs(ctx)?;
            let all = tabs.browser.tabs().await?;
            tabs.prune(&all);
            let found = all
                .into_iter()
                .find(|tab| matches(if by == "title" { &tab.title } else { &tab.url }, &how, &value))
                .ok_or_else(|| format!("нет вкладки, где {by} {how} {written}"))?;
            tabs.switch(Some(found.target_id));
            String::new()
        }
        "web.js" => {
            let code = cfg.string("code");
            let mut inject = Vec::new();
            for name in cfg.raw("inject").as_array().into_iter().flatten().filter_map(Value::as_str) {
                let value = ctx.scope.get(name).cloned().ok_or_else(|| format!("переменная {name} не задана"))?;
                if vars::element_index(&value).is_some() {
                    return Err(OpError::Failed(format!("Переменная {name} содержит элемент страницы. Передать его в JavaScript таким способом нельзя.")));
                }
                inject.push((name.to_string(), value));
            }
            let frame = cfg.raw("frame");
            let result = if frame.is_null() {
                tabs(ctx)?.page().await?.eval_main(&code, &inject).await
            } else {
                // Во фрейме — изолированный мир: DOM доступен, объекты сайта — нет.
                let doc = target::frame_doc(ctx, &frame).await?;
                tabs(ctx)?.page().await?.eval_in(&doc, &code, &inject).await
            };
            // Исключение скрипта (`Script`) — его текстом, как и прочие свои тексты страницы.
            let value = result.map_err(|e| match e {
                CdpError::Timeout => OpError::Failed(format!("скрипт не закончился за {} с", JS_TIMEOUT.as_secs())),
                other => other.into(),
            })?;
            return Ok((Step::Port("next"), ctx.save(cfg, value)?));
        }
        "web.hover" => {
            let element = target::element(ctx, &cfg.raw("target")).await?;
            let page = tabs(ctx)?.page().await?;
            let (x, y) = page.point(&element, None).await?;
            page.mouse_move(x, y).await?;
            String::new()
        }
        "web.focus" => {
            let element = target::element(ctx, &cfg.raw("target")).await?;
            tabs(ctx)?.page().await?.focus(&element).await?;
            String::new()
        }
        "web.click" => {
            let element = target::element(ctx, &cfg.raw("target")).await?;
            let mouse = cfg.raw("use_mouse");
            let (at, hold) = if mouse.is_object() {
                let x = int_or_range(&ctx.scope, &mouse["x"], "use_mouse.x")? as f64;
                let y = int_or_range(&ctx.scope, &mouse["y"], "use_mouse.y")? as f64;
                let hold = if mouse["release_ms"].is_null() { 0 } else { int_or_range(&ctx.scope, &mouse["release_ms"], "use_mouse.release_ms")? };
                (Some((x, y)), wait(hold)?)
            } else {
                (None, Duration::ZERO)
            };
            let button = match cfg.string("button").as_str() {
                "middle" => Button::Middle,
                "right" => Button::Right,
                _ => Button::Left,
            };
            let count = if cfg.string("kind") == "double" { 2 } else { 1 };
            let page = tabs(ctx)?.page().await?;
            let (x, y) = page.point(&element, at).await?;
            page.click(x, y, button, count, hold).await?;
            String::new()
        }
        "web.select" => {
            let value = cfg.text(&ctx.scope, "value")?;
            let written = to_text(&cfg.raw("value"));
            let element = target::element(ctx, &cfg.raw("target")).await?;
            if !tabs(ctx)?.page().await?.select(&element, &value).await? {
                return Err(OpError::Failed(format!("нет варианта {written}")));
            }
            String::new()
        }
        "web.input" => {
            let value = input_value(ctx, cfg)?;
            let element = target::element(ctx, &cfg.raw("target")).await?;
            type_text(ctx, &element, &value, cfg.flag("clear"), |ctx| wait(cfg.int_range(&ctx.scope, "interval_ms")?)).await?
        }
        "web.scroll" => {
            let element = if cfg.string("area") == "target" {
                if cfg.raw("target").is_null() {
                    return Err({
                        log::warn!("поле target не заполнено");
                        "Заполните поле «Элемент»"
                    }.into());
                }
                Some(target::element(ctx, &cfg.raw("target")).await?)
            } else {
                None
            };
            let pixels = cfg.int(&ctx.scope, "pixels")? as f64;
            let (position, smooth) = (cfg.string("position"), cfg.string("kind") == "smooth");
            let (top, max) = tabs(ctx)?.page().await?.scroll_state(element.as_ref()).await?;
            let goal = scroll_goal(&position, pixels, top, max);
            if !smooth {
                tabs(ctx)?.page().await?.scroll_to(element.as_ref(), goal).await?;
                return Ok((Step::Port("next"), String::new()));
            }
            let (x, y) = {
                let page = tabs(ctx)?.page().await?;
                match &element {
                    Some(element) => page.point(element, None).await?,
                    None => page.viewport_center().await?,
                }
            };
            let mut now = top;
            while (goal - now).abs() >= 1.0 {
                let step = (cfg.int_range(&ctx.scope, "step_px")?.max(1) as f64).min((goal - now).abs());
                let pause = cfg.int_range(&ctx.scope, "step_ms")?;
                let page = tabs(ctx)?.page().await?;
                page.wheel(x, y, if goal > now { step } else { -step }).await?;
                tokio::time::sleep(wait(pause)?).await;
                // Упёрлись: страница дальше не едет (догрузка, край).
                let Some(after) = moved(ctx, element.as_ref(), now).await? else { break };
                now = after;
            }
            String::new()
        }
        "web.screenshot" => {
            let (folder, name) = shot_path(ctx, cfg)?;
            let format = cfg.string("format");
            let quality = cfg.int(&ctx.scope, "quality")?;
            let full_page = cfg.flag("full_page");
            let bytes = tabs(ctx)?.page().await?.screenshot(&format, quality, full_page).await?;
            std::fs::create_dir_all(&folder).map_err(|e| format!("папка снимков не создана: {e}"))?;
            let extension = if format == "jpeg" { "jpg" } else { "png" };
            std::fs::write(folder.join(format!("{name}.{extension}")), bytes).map_err(|e| format!("снимок не записан: {e}"))?;
            String::new()
        }
        "web.upload" => {
            let element = target::element(ctx, &cfg.raw("target")).await?;
            let ms = cfg.int(&ctx.scope, "timeout_ms")?;
            let file = match cfg.string("source").as_str() {
                "url" => {
                    if cfg.raw("url").is_null() {
                        return Err({
                            log::warn!("поле url не заполнено");
                            "Заполните поле «Адрес»"
                        }.into());
                    }
                    let url = cfg.text(&ctx.scope, "url")?;
                    let relay = tabs(ctx)?.relay();
                    match files::download(relay, &url, &ctx.files.join("uploads"), wait(ms)?).await? {
                        Some(path) => path,
                        None => return Ok((Step::Port("next"), format!("Файл не загружен за {ms} мс"))),
                    }
                }
                source => {
                    if cfg.raw("path").is_null() {
                        return Err({
                            log::warn!("поле path не заполнено");
                            "Заполните поле «Путь»"
                        }.into());
                    }
                    let path = files::absolute(&cfg.text(&ctx.scope, "path")?, "путь файла")?;
                    if source == "folder_random" {
                        files::random_file(&path)?
                    } else if path.is_file() {
                        path
                    } else {
                        return Err("файла нет".into());
                    }
                }
            };
            tabs(ctx)?.page().await?.set_files(&element, &[file.to_string_lossy().into_owned()]).await?;
            String::new()
        }
        other => return Err(OpError::Failed(format!("операция {other} не исполняется"))),
    };
    Ok((Step::Port("next"), note))
}

pub(crate) fn tabs(ctx: &mut Ctx) -> Result<&mut Tabs, OpError> {
    ctx.tabs.as_mut().ok_or_else(|| OpError::Failed("у запуска нет браузера".into()))
}

/// Ввод текста в элемент, как у Input: фокус; `clear` — выделить всё и удалить; с очеловечиванием — одной командой
/// (темп — по персоне движка), иначе по символам с паузой `pause` после каждого. Ответ — заметка журнала.
pub(crate) async fn type_text(
    ctx: &mut Ctx,
    element: &ElementRef,
    value: &str,
    clear: bool,
    pause: impl Fn(&Ctx) -> Result<Duration, OpError>,
) -> Result<String, OpError> {
    let humanize = ctx.humanize;
    let page = tabs(ctx)?.page().await?;
    page.focus(element).await?;
    if clear {
        page.press("a", &["cmd"]).await?;
        page.press("Delete", &[]).await?;
    }
    if humanize {
        page.insert_text(value).await?;
        return Ok("Скорость ввода определяется стилем ввода профиля".into());
    }
    for ch in value.chars() {
        let pause = pause(ctx)?;
        tabs(ctx)?.page().await?.type_char(ch).await?;
        tokio::time::sleep(pause).await;
    }
    Ok(String::new())
}

pub(crate) fn wait(ms: i64) -> Result<Duration, OpError> {
    u64::try_from(ms).map(Duration::from_millis).map_err(|_| OpError::Failed("тайм-аут отрицательный".into()))
}

/// Тайм-аут перехода — своим текстом.
pub(crate) fn navigation<T>(result: Result<T, CdpError>, ms: i64) -> Result<T, OpError> {
    result.map_err(|e| match e {
        CdpError::Timeout => OpError::Failed(format!("страница не загрузилась за {ms} мс")),
        other => other.into(),
    })
}

/// Сколько ждать сдвига после шага колеса и как часто перечитывать позицию: очеловеченное колесо движка отвечает
/// раньше, чем кончится его серия событий, и один замер сразу после паузы шага принял бы это за край страницы.
const SCROLL_SETTLE: Duration = Duration::from_secs(1);
const SCROLL_POLL: Duration = Duration::from_millis(100);

/// Позиция, как только она отошла от `now` хотя бы на 1 px; за `SCROLL_SETTLE` не отошла — `None` (упёрлись).
async fn moved(ctx: &mut Ctx, element: Option<&ElementRef>, now: f64) -> Result<Option<f64>, OpError> {
    let deadline = tokio::time::Instant::now() + SCROLL_SETTLE;
    loop {
        let (after, _) = tabs(ctx)?.page().await?.scroll_state(element).await?;
        if (after - now).abs() >= 1.0 {
            return Ok(Some(after));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(SCROLL_POLL).await;
    }
}

/// Цель прокрутки: край, середина или сдвиг от текущей позиции в пределах `[0, max]`.
fn scroll_goal(position: &str, pixels: f64, top: f64, max: f64) -> f64 {
    match position {
        "top" => 0.0,
        "middle" => max / 2.0,
        "bottom" => max,
        _ => (top + pixels).clamp(0.0, max.max(0.0)),
    }
}

/// Папка и имя снимка: пустые после подстановки — умолчания (папка файлов запуска, `<task_id>_<acc_id>_<время>`);
/// папка — только абсолютным путём (относительный ушёл бы в рабочую папку процесса приложения).
fn shot_path(ctx: &Ctx, cfg: &Cfg<'_>) -> Result<(PathBuf, String), OpError> {
    let folder = files::folder(ctx, cfg, "folder", "папка снимков")?;
    let name = match cfg.raw("name") {
        Value::Null => String::new(),
        _ => cfg.text(&ctx.scope, "name")?,
    };
    let name = if name.is_empty() {
        let part = |name: &str| ctx.scope.get(name).map(to_text).unwrap_or_default();
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0).to_string();
        [part("task_id"), part("acc_id"), stamp].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("_")
    } else {
        name
    };
    Ok((folder, files::file_name(&name)?))
}

fn matches(field: &str, how: &str, value: &str) -> bool {
    match how {
        "eq" => field == value,
        "ne" => field != value,
        "contains" => field.contains(value),
        _ => !field.contains(value),
    }
}

/// Значение Input: строка с номером профиля в задаче (по кругу), случайная строка или случайное число.
fn input_value(ctx: &Ctx, cfg: &Cfg<'_>) -> Result<String, OpError> {
    if cfg.string("mode") == "random_number" {
        let number = cfg.raw("number");
        if number.is_null() {
            return Err({
                log::warn!("поле number не заполнено");
                "Заполните поле «Число»"
            }.into());
        }
        return Ok(int_or_range(&ctx.scope, &number, "number")?.to_string());
    }
    if cfg.raw("content").is_null() {
        return Err({
            log::warn!("поле content не заполнено");
            "Заполните поле «Текст»"
        }.into());
    }
    let content = cfg.text(&ctx.scope, "content")?;
    // Строки с переводом CRLF: `\r` на конце не печатается.
    let lines: Vec<&str> = content.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect();
    if lines.len() > 50 {
        return Err("в поле content больше 50 строк".into());
    }
    if lines.iter().any(|line| line.chars().count() > 500) {
        return Err("строка поля content длиннее 500 символов".into());
    }
    let line = if cfg.string("mode") == "random" { lines[vars::random_index(lines.len())] } else { lines[ctx.position % lines.len()] };
    Ok(line.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::engine::fake_cdp::FakeCdp;
    use crate::engine::page::tests::{Mock, base};
    use crate::flow::interp::{Exit, Fail};
    use crate::flow::journal::Status;
    use crate::flow::model::Flow;
    use serde_json::{Map, json};
    use tokio::net::TcpListener;

    #[test]
    fn scroll_goal_by_position() {
        assert_eq!(scroll_goal("top", 0.0, 300.0, 1000.0), 0.0);
        assert_eq!(scroll_goal("middle", 0.0, 300.0, 1000.0), 500.0);
        assert_eq!(scroll_goal("bottom", 0.0, 300.0, 1000.0), 1000.0);
        assert_eq!(scroll_goal("pixels", 500.0, 300.0, 1000.0), 800.0);
        assert_eq!(scroll_goal("pixels", -900.0, 300.0, 1000.0), 0.0, "за край не уходит");
    }

    /// Подставной браузер, где позиция прокрутки страницы идёт по списку (последняя остаётся навсегда).
    async fn scrolling(positions: Vec<f64>) -> Ctx {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/devtools/browser/x", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut reads = 0;
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let request: Value = serde_json::from_str(text.as_str()).unwrap();
                let result = if request["method"] == "Page.createIsolatedWorld" {
                    json!({ "executionContextId": 1 })
                } else {
                    reads += 1;
                    json!({ "result": { "value": [positions[(reads - 1).min(positions.len() - 1)], 1000] } })
                };
                let _ = socket.send(Message::text(json!({ "id": request["id"], "result": result }).to_string())).await;
            }
        });
        let browser = Browser::connect(&url).await.unwrap();
        let mut tabs = Tabs::new(browser.clone(), Some("T0".into()));
        tabs.pages.insert("T0".into(), Page::detached(&browser));
        let mut ctx = bare();
        ctx.tabs = Some(tabs);
        ctx
    }

    #[tokio::test]
    async fn scroll_waits_for_a_late_move_and_stops_when_stuck() {
        // Очеловеченное колесо ответило раньше конца серии: два замера ещё на месте, потом страница сдвинулась.
        let mut late = scrolling(vec![500.0, 500.4, 120.0]).await;
        assert_eq!(moved(&mut late, None, 500.0).await, Ok(Some(120.0)));
        let mut stuck = scrolling(vec![0.0]).await;
        let started = std::time::Instant::now();
        assert_eq!(moved(&mut stuck, None, 0.0).await, Ok(None), "край: позиция не сдвинулась");
        assert!(started.elapsed() >= SCROLL_SETTLE, "перечитывала до срока");
    }

    #[test]
    fn empty_screenshot_fields_take_defaults_and_folders_are_absolute() {
        let spec = crate::flow::ops::spec("web.screenshot").unwrap();
        let mut ctx = bare();
        ctx.files = PathBuf::from("/data/files/run");
        ctx.scope.set("empty", json!("")).unwrap();
        ctx.scope.set("dir", json!("снимки")).unwrap();
        let path = |config: Value| shot_path(&ctx, &Cfg::new(spec, config.as_object().unwrap()));
        let (folder, name) = path(json!({ "folder": "${empty}", "name": "${empty}" })).unwrap();
        assert_eq!(folder, PathBuf::from("/data/files/run"), "пустая папка — папка файлов запуска");
        assert!(!name.is_empty() && name.chars().all(|c| c.is_ascii_digit()), "пустое имя — умолчание: {name}");
        assert_eq!(path(json!({ "folder": "/tmp/shots", "name": "a" })).unwrap(), (PathBuf::from("/tmp/shots"), "a".to_string()));
        assert_eq!(path(json!({ "folder": "${dir}" })).unwrap_err(), OpError::Failed("папка снимков — абсолютный путь".into()), "без значения в тексте");
        assert!(path(json!({ "name": "../x" })).is_err());
    }

    /// start → узлы по порядку.
    pub(crate) fn chain(nodes: Vec<Value>) -> Flow {
        let mut all = vec![json!({ "id": "s", "op": "start" })];
        let mut edges = Vec::new();
        let mut prev = "s".to_string();
        for node in nodes {
            let id = node["id"].as_str().unwrap().to_string();
            edges.push(json!({ "from": prev, "to": id }));
            prev = id;
            all.push(node);
        }
        serde_json::from_value(json!({ "format": "ombra-flow/1", "name": "t", "nodes": all, "edges": edges })).unwrap()
    }

    fn bare() -> Ctx {
        Ctx::bare()
    }

    /// Запуск с подставным браузером: текущая вкладка `T1` (её сессия у `Mock` — `S1`).
    pub(crate) async fn mocked(mock: &Mock) -> Ctx {
        let mut ctx = bare();
        ctx.tabs = Some(Tabs::new(Browser::connect(&mock.url).await.unwrap(), Some("T1".into())));
        ctx
    }

    async fn with_fake(fake: &FakeCdp, flow: &Flow) -> (Result<Exit, Fail>, Ctx) {
        let browser = Browser::connect(&fake.ws_url()).await.unwrap();
        let current = browser.tabs().await.unwrap().first().map(|tab| tab.target_id.clone());
        let mut ctx = bare();
        ctx.tabs = Some(Tabs::new(browser, current));
        let result = ctx.run(flow).await;
        (result, ctx)
    }

    fn input_ctx(position: usize) -> Ctx {
        let mut ctx = Ctx::bare();
        ctx.position = position;
        ctx
    }

    fn input_cfg(config: Value) -> Map<String, Value> {
        config.as_object().unwrap().clone()
    }

    #[test]
    fn input_value_by_mode() {
        let spec = crate::flow::ops::spec("web.input").unwrap();
        let config = input_cfg(json!({ "target": { "by": "css", "selector": "#q" }, "content": "a\nb\nc" }));
        assert_eq!(input_value(&input_ctx(4), &Cfg::new(spec, &config)).unwrap(), "b", "In order по кругу: 4 % 3 = 1");
        let random = input_cfg(json!({ "target": { "by": "css", "selector": "#q" }, "mode": "random", "content": "a\nb" }));
        assert!(["a", "b"].contains(&input_value(&input_ctx(0), &Cfg::new(spec, &random)).unwrap().as_str()));
        let number = input_cfg(json!({ "target": { "by": "css", "selector": "#q" }, "mode": "random_number", "number": { "min": 5, "max": 6 } }));
        assert!(["5", "6"].contains(&input_value(&input_ctx(0), &Cfg::new(spec, &number)).unwrap().as_str()));
        let empty = input_cfg(json!({ "target": { "by": "css", "selector": "#q" } }));
        assert_eq!(input_value(&input_ctx(0), &Cfg::new(spec, &empty)).unwrap_err(), OpError::Failed("Заполните поле «Текст»".into()));
        let many = input_cfg(json!({ "target": { "by": "css", "selector": "#q" }, "content": "x\n".repeat(50) + "x" }));
        assert_eq!(input_value(&input_ctx(0), &Cfg::new(spec, &many)).unwrap_err(), OpError::Failed("в поле content больше 50 строк".into()));
        let crlf = input_cfg(json!({ "target": { "by": "css", "selector": "#q" }, "content": "a\r\nb\r\n" }));
        assert_eq!(input_value(&input_ctx(0), &Cfg::new(spec, &crlf)).unwrap(), "a", "\\r строки CRLF не печатается");
    }

    #[tokio::test]
    async fn elements_are_not_text_or_script_constants() {
        let mut ctx = bare();
        ctx.scope.set("el", vars::element_value(0)).unwrap();
        let input = chain(vec![json!({ "id": "i", "op": "web.input", "config": { "target": { "by": "css", "selector": "#q" }, "content": "${el}" } })]);
        let result = ctx.run(&input).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "Input: поле content — элемент, его нельзя вставить в текст"), "{result:?}");
        let js = chain(vec![json!({ "id": "j", "op": "web.js", "config": { "code": "el", "inject": ["el"] } })]);
        let result = ctx.run(&js).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "Execute JavaScript: Переменная el содержит элемент страницы. Передать его в JavaScript таким способом нельзя."), "{result:?}");
    }

    #[tokio::test]
    async fn element_actions_need_a_browser_and_a_stored_element() {
        let f = chain(vec![json!({ "id": "c", "op": "web.click", "config": { "target": { "by": "css", "selector": "#b" } } })]);
        let result = bare().run(&f).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "Click: у запуска нет браузера"), "{result:?}");
        let fake = FakeCdp::spawn().await;
        fake.add_page("https://a.test/");
        let stored = chain(vec![json!({ "id": "c", "op": "web.hover", "config": { "target": { "element": "${el}" } } })]);
        let mut ctx = bare();
        ctx.scope.set("el", json!("строка")).unwrap();
        let browser = Browser::connect(&fake.ws_url()).await.unwrap();
        ctx.tabs = Some(Tabs::new(browser, None));
        let result = ctx.run(&stored).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "Hover: значение не элемент"), "{result:?}");
    }

    #[tokio::test]
    async fn new_and_closed_tabs_move_the_current_tab() {
        let fake = FakeCdp::spawn().await;
        fake.add_page("https://a.test/");
        let f = chain(vec![
            json!({ "id": "n1", "op": "web.new_tab" }),
            json!({ "id": "n2", "op": "web.new_tab" }),
            json!({ "id": "c", "op": "web.close_tab" }),
            json!({ "id": "o", "op": "web.close_other_tabs" }),
        ]);
        let (result, ctx) = with_fake(&fake, &f).await;
        assert_eq!(result, Ok(Exit::Done));
        assert_eq!(fake.pages(), ["about:blank"], "закрыта вторая новая, затем все, кроме ставшей текущей первой новой");
        let tabs = ctx.tabs.as_ref().unwrap();
        assert_eq!(tabs.browser.tabs().await.unwrap()[0].target_id, tabs.current.clone().unwrap());
    }

    #[tokio::test]
    async fn sessions_live_per_tab_and_elements_stay_on_their_tab() {
        let mock = Mock::spawn(|method, params, _| match method {
            "Target.attachToTarget" => json!({ "sessionId": format!("S-{}", params["targetId"].as_str().unwrap_or_default()) }),
            "Target.getTargets" => json!({ "targetInfos": [{ "targetId": "B", "type": "page", "url": "about:blank", "title": "" }] }),
            _ => json!({}),
        })
        .await;
        let mut ctx = bare();
        ctx.tabs = Some(Tabs::new(Browser::connect(&mock.url).await.unwrap(), Some("A".into())));
        let tabs = ctx.tabs.as_mut().unwrap();
        tabs.page().await.unwrap();
        let saved = tabs.keep(ElementRef::test(5));
        tabs.switch(Some("B".into()));
        assert_eq!(tabs.element(&saved), Err(OpError::Failed("элемент на другой вкладке".into())));
        tabs.page().await.unwrap();
        tabs.switch(Some("A".into()));
        assert_eq!(tabs.element(&saved), Ok(ElementRef::test(5)), "вернулись на вкладку — элемент снова доступен");
        tabs.page().await.unwrap();
        assert_eq!(mock.sent("Target.attachToTarget").len(), 2, "к каждой вкладке — одна сессия на весь запуск");
        assert!(mock.sent("Target.detachFromTarget").is_empty(), "переключение сессий не отцепляет");
        // Close Tab: сессия вкладки отцеплена, её элементы пропали вместе с ней.
        assert_eq!(ctx.run(&chain(vec![json!({ "id": "c", "op": "web.close_tab" })])).await, Ok(Exit::Done));
        assert_eq!(mock.sent("Target.detachFromTarget"), [json!({ "sessionId": "S-A" })]);
        let tabs = ctx.tabs.as_ref().unwrap();
        assert_eq!(tabs.current.as_deref(), Some("B"));
        assert_eq!(tabs.pages.keys().collect::<Vec<_>>(), ["B"]);
        assert_eq!(tabs.element(&saved), Err(OpError::Failed("Элемент больше не доступен на странице".into())));
    }

    #[tokio::test]
    async fn an_element_of_a_previous_document_fails_before_dom_calls() {
        let mock = Mock::spawn(|method, _, _| base(method).unwrap_or_else(|| json!({ "error": "unexpected" }))).await;
        let mut ctx = mocked(&mock).await;
        let tabs = ctx.tabs.as_mut().unwrap();
        let saved = {
            tabs.page().await.unwrap();
            tabs.keep(ElementRef::test(5))
        };
        ctx.scope.set("el", saved).unwrap();
        mock.events.send(json!({ "method": "Page.frameNavigated", "sessionId": "S1", "params": { "frame": { "id": "T1", "loaderId": "L2" } } })).unwrap();
        let page = ctx.tabs.as_mut().unwrap().page().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while page.generation("T1") == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("переход не обработан");
        let result = ctx.run(&chain(vec![json!({ "id": "c", "op": "web.click", "config": { "target": { "element": "${el}" } } })])).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "Click: Элемент больше не доступен на странице"), "{result:?}");
        assert!(mock.sent("DOM.resolveNode").is_empty() && mock.sent("DOM.scrollIntoViewIfNeeded").is_empty(), "до DOM.* дело не дошло");
    }

    #[tokio::test]
    async fn switch_tabs_finds_the_first_match() {
        let fake = FakeCdp::spawn().await;
        fake.add_page("https://a.test/");
        fake.add_page("https://b.test/x");
        fake.add_page("https://b.test/y");
        let f = chain(vec![
            json!({ "id": "w", "op": "web.switch_tab", "config": { "match": "contains", "value": "b.test" } }),
            json!({ "id": "c", "op": "web.close_tab" }),
        ]);
        assert_eq!(with_fake(&fake, &f).await.0, Ok(Exit::Done));
        assert_eq!(fake.pages(), ["https://a.test/", "https://b.test/y"]);
        let missing = chain(vec![json!({ "id": "w", "op": "web.switch_tab", "config": { "value": "nope" } })]);
        let (result, _) = with_fake(&fake, &missing).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "Switch Tabs: нет вкладки, где url eq nope"), "{result:?}");
    }

    #[tokio::test]
    async fn page_operations_report_cdp_errors_as_node_errors() {
        // Подставной браузер не даёт сессий вкладок: Access Website — ошибка узла, `skip` её пропускает.
        let fake = FakeCdp::spawn().await;
        fake.add_page("https://a.test/");
        let f = chain(vec![
            json!({ "id": "g", "op": "web.goto", "config": { "url": "https://b.test/" }, "on_error": "skip" }),
            json!({ "id": "z", "op": "wait.time", "config": { "ms": 0 } }),
        ]);
        let (result, ctx) = with_fake(&fake, &f).await;
        assert_eq!(result, Ok(Exit::Done));
        let entry = ctx.journal.entries().iter().find(|e| e.node == "g").unwrap();
        assert_eq!(entry.status, Status::Skip);
        assert!(entry.msg.starts_with("Access Website: Браузер сообщил об ошибке"), "{}", entry.msg);
    }

    #[tokio::test]
    async fn without_a_browser_web_actions_fail() {
        let f = chain(vec![json!({ "id": "n", "op": "web.new_tab" })]);
        let result = bare().run(&f).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "New Tab: у запуска нет браузера"), "{result:?}");
    }

    #[tokio::test]
    async fn closed_browser_ends_the_run_whatever_the_policy() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/devtools/browser/x", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let _ = socket.close(None).await;
        });
        let browser = Browser::connect(&url).await.unwrap();
        let mut ctx = bare();
        ctx.tabs = Some(Tabs::new(browser, None));
        let f = chain(vec![json!({ "id": "n", "op": "web.new_tab", "on_error": "skip" })]);
        assert_eq!(ctx.run(&f).await, Err(Fail::BrowserClosed));
    }

    /// Мок страницы с полем выбора файла `#f` (узел 5).
    async fn upload_mock() -> Mock {
        Mock::spawn(|method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "Runtime.callFunctionOn" if params.get("objectId").is_some() => json!({ "result": { "value": true } }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "object", "objectId": "O1" } }),
                "DOM.describeNode" => json!({ "node": { "backendNodeId": 5 } }),
                "DOM.resolveNode" => json!({ "object": { "objectId": "O5" } }),
                _ => json!({}),
            })
        })
        .await
    }

    async fn upload(relay: Option<u16>, config: Value) -> (Result<Exit, Fail>, Mock) {
        let mock = upload_mock().await;
        let mut ctx = bare();
        ctx.tabs = Some(Tabs::new(Browser::connect(&mock.url).await.unwrap(), Some("T1".into())).with_relay(relay));
        let mut config = config;
        config["target"] = json!({ "by": "css", "selector": "#f" });
        let result = ctx.run(&chain(vec![json!({ "id": "u", "op": "web.upload", "config": config })])).await;
        (result, mock)
    }

    fn node_error(result: &Result<Exit, Fail>, text: &str) -> bool {
        matches!(result, Err(Fail::Node { message, .. }) if message == text)
    }

    #[tokio::test]
    async fn input_file_sends_an_existing_file_and_rejects_bad_sources() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let (ok, mock) = upload(None, json!({ "source": "file", "path": file.to_str().unwrap() })).await;
        assert_eq!(ok, Ok(Exit::Done));
        assert_eq!(mock.sent("DOM.setFileInputFiles")[0], json!({ "files": [file.to_str().unwrap()], "backendNodeId": 5 }));
        let missing = dir.path().join("none.txt");
        let (result, _) = upload(None, json!({ "source": "file", "path": missing.to_str().unwrap() })).await;
        assert!(node_error(&result, "Input File: файла нет"), "{result:?}");
        let empty = tempfile::tempdir().unwrap();
        let (result, _) = upload(None, json!({ "source": "folder_random", "path": empty.path().to_str().unwrap() })).await;
        assert!(node_error(&result, "Input File: в папке нет файлов"), "{result:?}");
        let (result, _) = upload(None, json!({ "source": "url" })).await;
        assert!(node_error(&result, "Input File: Заполните поле «Адрес»"), "{result:?}");
    }

    #[tokio::test]
    async fn input_file_download_timeout_goes_on_without_sending() {
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = silent.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = silent.accept().await {
                held.push(socket);
            }
        });
        let (result, mock) = upload(Some(port), json!({ "source": "url", "url": "http://files.test/f", "timeout_ms": 200 })).await;
        assert_eq!(result, Ok(Exit::Done));
        assert!(mock.sent("DOM.setFileInputFiles").is_empty());
    }
}
