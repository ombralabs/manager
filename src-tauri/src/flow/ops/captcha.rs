//! 2Captcha: обычная капча по картинке и капчи с токеном — reCAPTCHA v2/v3, hCaptcha, Cloudflare Turnstile
//! (спецификация А4, «А4-2»). API v2 (`createTask`, `getTaskResult`), задачи без прокси: прокси профиля сервису не
//! передаётся. Всё нужное со страницы (ключ сайта, картинка, поле ответа) берётся до обращения к сервису — нет капчи,
//! деньги не тратятся. Токен попадает только в виджет, для которого решалась капча. Сеть — `services::http`.
use crate::engine::page::ElementRef;
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::files::absolute;
use crate::flow::ops::openai::hide;
use crate::flow::ops::web::{tabs, type_text};
use crate::flow::ops::{Cfg, OpError, target};
use crate::flow::services;
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde_json::{Value, json};
use std::time::Duration;

/// Варианты поля `numeric` по порядку кодов 2Captcha 0–4.
pub(crate) const NUMERIC: &[&str] = &["any", "digits", "letters", "digits_or_letters", "digits_and_letters"];

#[cfg_attr(test, allow(dead_code))]
const BASE: &str = "https://api.2captcha.com";
/// Опрос результата и предел ожидания решения.
#[cfg_attr(test, allow(dead_code))]
const TIMING: (Duration, Duration) = (Duration::from_secs(5), Duration::from_secs(180));

#[cfg(not(test))]
fn base() -> String {
    BASE.to_string()
}

#[cfg(test)]
fn base() -> String {
    tests::BASE_URL.with(|url| url.borrow().clone())
}

#[cfg(not(test))]
fn timing() -> (Duration, Duration) {
    TIMING
}

#[cfg(test)]
fn timing() -> (Duration, Duration) {
    tests::TIMING.with(|t| t.get())
}

/// Поиск на странице (текст скрипта — в `captcha.js`): `{key, widget, enterprise, invisible}` — ключ сайта (из поля
/// `sitekey` или первого виджета вида), взят ли виджет из разметки (он помечен в памяти свойством `mark`), Enterprise ли
/// он и невидимый ли; нет ключа — `null`.
const FIND: &str = concat!("(", include_str!("captcha.js"), ")(kind, sitekey, mark, null, null)");

/// Внедрение токена только в помеченный поиском виджет: его поля ответа и его обработчики. Ответ — `[полей,
/// обработчиков, виджет пропал]`.
const INJECT: &str = concat!("(", include_str!("captcha.js"), ")(kind, sitekey, mark, token, widget)");

pub(crate) async fn exec(ctx: &mut Ctx, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let store = ctx.store.clone().ok_or("у запуска нет настроек сервисов")?;
    let key = cfg.text(&ctx.scope, "api_key")?;
    if key.is_empty() {
        return Err("не задан ключ API".to_string().into());
    }
    let kind = cfg.string("kind");
    // Обычной капче — поле ввода ответа; капче с токеном — её виджет на странице: переменные скрипта внедрения из поиска.
    let (task, field, mut inject) = match kind.as_str() {
        "normal" => image_task(ctx, cfg).await.map(|(task, field)| (task, Some(field), Vec::new()))?,
        _ => token_task(ctx, cfg, &kind).await.map(|(task, inject)| (task, None, inject))?,
    };
    let client = services::http(&store)?;
    let solved = tokio::select! {
        solved = solve(&client, &key, task) => solved?,
        _ = ctx.cancel.cancelled() => return Err(OpError::Cancelled),
    };
    let answer = match solved {
        Solved::Answer(answer) => answer,
        Solved::Timeout(last) => {
            let note = ctx.save(cfg, Value::Bool(false))?;
            let last = last.map(|e| format!(" (последний сбой опроса: {e})")).unwrap_or_default();
            return Ok((Step::Port("next"), format!("не решена за {} с{last} {note}", timing().1.as_secs())));
        }
    };
    // Ответ оплачен: сначала — в `answer_to`, исключение страницы при вводе или внедрении его не теряет.
    let answer_to = cfg.var_out("answer_to");
    if let Some(name) = &answer_to {
        ctx.scope.set(name, Value::String(answer.clone()))?;
    }
    let done = match field {
        Some(field) => {
            type_text(ctx, &field, &answer, true, |_| Ok(TYPE_PAUSE)).await?;
            format!("ответ введён: {} симв.", answer.chars().count())
        }
        None => {
            inject.push(("token".to_string(), json!(answer)));
            let done = tabs(ctx)?.page().await?.eval_main(INJECT, &inject).await?;
            let gone = done[2] == json!(true);
            let nowhere = done[0] == json!(0) && done[1] == json!(0) && kind != "recaptcha_v3";
            match (gone, nowhere, answer_to.is_some()) {
                (true, _, true) => "Капча исчезла со страницы во время обработки. Ответ сохранён в указанную переменную.".to_string(),
                (true, _, false) => "Капча исчезла со страницы во время обработки. Чтобы сохранить ответ, заполните поле «Сохранить ответ в».".to_string(),
                (false, true, true) => "Поле ответа на странице не найдено. Ответ сохранён в указанную переменную.".to_string(),
                (false, true, false) => "Поле ответа на странице не найдено. Заполните поле «Сохранить ответ в», чтобы получить результат.".to_string(),
                _ => format!("полей ответа: {}, обработчиков: {}", done[0], done[1]),
            }
        }
    };
    let note = ctx.save(cfg, Value::Bool(true))?;
    Ok((Step::Port("next"), format!("{done} {note}")))
}

/// Пауза после символа ответа без очеловечивания — как умолчание `interval_ms` у Input.
const TYPE_PAUSE: Duration = Duration::from_millis(300);

/// Задача по картинке (`ImageToTextTask`) и поле ответа `answer_target`. Картинка — снимок элемента `image` или файл
/// `image_path` (одно из двух); длина ответа `min_len`–`max_len` — 0–6, 0 — без ограничения.
async fn image_task(ctx: &mut Ctx, cfg: &Cfg<'_>) -> Result<(Value, ElementRef), OpError> {
    let mut length = [0; 2];
    for (n, name) in ["min_len", "max_len"].into_iter().enumerate() {
        length[n] = cfg.int(&ctx.scope, name)?;
        if !(0..=6).contains(&length[n]) {
            return Err(format!("поле {name}: от 0 до 6").into());
        }
    }
    if length[0] > length[1] && length[1] != 0 {
        return Err("поле min_len больше max_len".into());
    }
    let (image, path) = (cfg.raw("image"), cfg.text(&ctx.scope, "image_path")?);
    match (image.is_null(), path.trim().is_empty()) {
        (true, true) => return Err("поле image или image_path не заполнено".into()),
        (false, false) => return Err("поля image и image_path заданы оба — нужно одно".into()),
        _ => {}
    }
    if cfg.raw("answer_target").is_null() {
        return Err({
            log::warn!("поле answer_target не заполнено");
            "Заполните поле «Поле для ответа»"
        }.into());
    }
    let bytes = match image.is_null() {
        true => std::fs::read(absolute(path.trim(), "путь к изображению")?).map_err(|e| format!("Не удалось прочитать изображение: {e}"))?,
        false => {
            let element = target::element(ctx, &image).await?;
            tabs(ctx)?.page().await?.screenshot_element(&element).await?
        }
    };
    let field = target::element(ctx, &cfg.raw("answer_target")).await?;
    let numeric = NUMERIC.iter().position(|choice| *choice == cfg.string("numeric")).unwrap_or(0);
    let task = json!({
        "type": "ImageToTextTask",
        "body": B64.encode(bytes),
        "phrase": cfg.flag("phrase"),
        "case": cfg.flag("case_sensitive"),
        "numeric": numeric,
        "math": cfg.flag("math"),
        "minLength": length[0],
        "maxLength": length[1],
    });
    Ok((task, field))
}

/// Задача 2Captcha для капчи с токеном и переменные скрипта внедрения (вид, ключ, метка виджета, взят ли он): адрес —
/// текущей вкладки, ключ — из поля `sitekey` (по нему же ищется виджет) или первого виджета вида на странице. Метка
/// своя у каждого решения: виджет помечается свойством с этим именем, не атрибутом DOM.
async fn token_task(ctx: &mut Ctx, cfg: &Cfg<'_>, kind: &str) -> Result<(Value, Vec<(String, Value)>), OpError> {
    let label = match kind {
        "recaptcha_v3" => "reCAPTCHA v3",
        "hcaptcha" => "hCaptcha",
        "turnstile" => "Cloudflare Turnstile",
        _ => "reCAPTCHA v2",
    };
    let written = cfg.text(&ctx.scope, "sitekey")?.trim().to_string();
    let page = tabs(ctx)?.page().await?;
    let url = page.url().await?;
    let mark = format!("_{:016x}", getrandom::u64().unwrap_or_default());
    let mut script = vec![("kind".to_string(), json!(kind)), ("sitekey".to_string(), json!(written)), ("mark".to_string(), json!(mark))];
    let found = page.eval_main(FIND, &script).await?;
    let sitekey = match (written.is_empty(), found["key"].as_str()) {
        (false, _) => written,
        (true, Some(key)) if !key.is_empty() => key.to_string(),
        _ => return Err(format!("на странице не найдена капча {label} (ключ сайта); его можно задать полем sitekey").into()),
    };
    // reCAPTCHA Enterprise (фрейм `/recaptcha/enterprise/` или `grecaptcha.enterprise`) — задачей Enterprise: обычный
    // токен такой сайт отвергнет, а деньги за него уже уйдут.
    let enterprise = found["enterprise"] == json!(true);
    let task = match (kind, enterprise) {
        ("recaptcha_v3", _) => "RecaptchaV3TaskProxyless",
        ("hcaptcha", _) => "HCaptchaTaskProxyless",
        ("turnstile", _) => "TurnstileTaskProxyless",
        (_, true) => "RecaptchaV2EnterpriseTaskProxyless",
        _ => "RecaptchaV2TaskProxyless",
    };
    let mut task = json!({ "type": task, "websiteURL": url, "websiteKey": sitekey });
    if kind == "recaptcha_v3" && enterprise {
        task["isEnterprise"] = json!(true);
    }
    // Невидимая reCAPTCHA v2 (и Enterprise v2) решается своим режимом: 2Captcha требует `isInvisible`.
    if kind == "recaptcha_v2" && found["invisible"] == json!(true) {
        task["isInvisible"] = json!(true);
    }
    if kind == "recaptcha_v3" {
        let score = cfg.text(&ctx.scope, "min_score")?;
        task["minScore"] = json!(score.trim().parse::<f64>().map_err(|_| "поле min_score: ожидается число")?);
        let action = cfg.text(&ctx.scope, "action")?;
        if !action.trim().is_empty() {
            task["pageAction"] = json!(action.trim());
        }
    }
    script[1].1 = json!(sitekey);
    script.push(("widget".to_string(), json!(found["widget"] == json!(true))));
    Ok((task, script))
}

/// Итог ожидания решения: ответ или предел без решения — с последним сбоем опроса, если опрос кончился сбоем.
pub(crate) enum Solved {
    Answer(String),
    Timeout(Option<String>),
}

/// Решение задачи 2Captcha: токен (`gRecaptchaResponse` / `token`) или текст ответа; предел — 180 с. Ошибка сервиса — его
/// код и описание без ключа. Сбой опроса (связь, HTTP без `errorId`, ответ не JSON или с незнакомым `status`) — повтор
/// до предела: задача создана и будет оплачена, бросать её рано. «Стоп» — снаружи: брошенный вызов прекращает опрос.
pub(crate) async fn solve(http: &reqwest::Client, key: &str, task: Value) -> Result<Solved, String> {
    let created = call(http, key, "createTask", json!({ "clientKey": key, "task": task })).await.map_err(Fault::text)?;
    let id = created.get("taskId").filter(|id| !id.is_null()).cloned().ok_or("в ответе нет taskId")?;
    let (poll, limit) = timing();
    let mut last = None;
    let wait = async {
        loop {
            tokio::time::sleep(poll).await;
            let answer = match call(http, key, "getTaskResult", json!({ "clientKey": key, "taskId": id })).await {
                Ok(answer) => answer,
                Err(Fault::Again(text)) => {
                    last = Some(text);
                    continue;
                }
                Err(Fault::Stop(text)) => return Err(text),
            };
            match answer["status"].as_str() {
                Some("processing") => last = None,
                Some("ready") => {
                    let solution = &answer["solution"];
                    let found = ["gRecaptchaResponse", "token", "text"].iter().find_map(|name| solution[name].as_str());
                    return found.map(|text| Solved::Answer(text.to_string())).ok_or_else(|| "в ответе нет решения".to_string());
                }
                _ => last = Some("Сервис вернул ответ в неподдерживаемом формате".to_string()),
            }
        }
    };
    match tokio::time::timeout(limit, wait).await {
        Ok(done) => done,
        Err(_) => Ok(Solved::Timeout(last)),
    }
}

/// Отказ вызова API: `Again` — сбой связи, HTTP-ошибка без `errorId` или ответ не JSON (опрос повторяет), `Stop` —
/// ошибка сервиса с кодом.
enum Fault {
    Again(String),
    Stop(String),
}

impl Fault {
    fn text(self) -> String {
        match self {
            Fault::Again(text) | Fault::Stop(text) => text,
        }
    }
}

/// Вызов метода API v2. `errorId` ≠ 0 — ошибка с кодом (`ERROR_ZERO_BALANCE`, `ERROR_KEY_DOES_NOT_EXIST`, …).
async fn call(http: &reqwest::Client, key: &str, method: &str, body: Value) -> Result<Value, Fault> {
    let sent = http.post(format!("{}/{method}", base())).json(&body).send().await;
    let response = sent.map_err(|e| Fault::Again(hide(format!("сервис не ответил: {}", e.without_url()), key)))?;
    let status = response.status();
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    if answer["errorId"].as_i64().is_some_and(|id| id != 0) {
        let code = answer["errorCode"].as_str().unwrap_or("ошибка сервиса");
        let text = match answer["errorDescription"].as_str() {
            Some(about) if !about.is_empty() => format!("{code}: {about}"),
            _ => code.to_string(),
        };
        return Err(Fault::Stop(hide(text, key)));
    }
    if !status.is_success() {
        return Err(Fault::Again(format!("HTTP {status}")));
    }
    answer.is_object().then_some(answer).ok_or_else(|| Fault::Again("Сервис вернул ответ в неподдерживаемом формате".into()))
}

#[cfg(test)]
mod tests {
    use super::{FIND, INJECT};
    use crate::engine::page::ElementRef;
    use crate::engine::page::tests::{Mock, base};
    use crate::flow::interp::{Ctx, Exit, Fail};
    use crate::flow::model::Flow;
    use crate::flow::ops::web::tests::{chain, mocked};
    use crate::store::tests::memory_store;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use serde_json::{Value, json};
    use std::cell::{Cell, RefCell};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tokio::net::TcpListener;

    thread_local! {
        pub(super) static BASE_URL: RefCell<String> = RefCell::new("http://127.0.0.1:9".into());
        /// Опрос и предел ожидания решения: в тестах — миллисекунды вместо 5 и 180 с.
        pub(super) static TIMING: Cell<(Duration, Duration)> = const { Cell::new((Duration::from_millis(10), Duration::from_secs(5))) };
    }

    const PAGE: &str = "https://site.test/login?next=1";
    const KEY: &str = "KEY-secret";
    /// Снимок элемента в base64 (начало PNG).
    const SHOT: &str = "iVBORw0KGgo=";

    type Seen = Arc<Mutex<Vec<(String, Value)>>>;

    /// Подставной 2Captcha: `createTask` отвечает `created`, `getTaskResult` — по списку `results` (последний — дальше
    /// всегда); ответ `http(код, тело)` — не JSON, с этим кодом. Записывает путь и тело каждого запроса.
    async fn fake(created: Value, results: Vec<Value>) -> Seen {
        let seen: Seen = Default::default();
        let log = seen.clone();
        let results = Arc::new(results);
        let handler = move |uri: axum::http::Uri, axum::Json(body): axum::Json<Value>| {
            let (log, created, results) = (log.clone(), created.clone(), results.clone());
            async move {
                let path = uri.path().to_string();
                let polls = {
                    let mut log = log.lock().unwrap();
                    log.push((path.clone(), body));
                    log.iter().filter(|(p, _)| p == "/getTaskResult").count()
                };
                let reply = if path == "/createTask" { created } else { results[(polls - 1).min(results.len() - 1)].clone() };
                match reply["$status"].as_u64() {
                    Some(code) => (StatusCode::from_u16(code as u16).unwrap(), reply["$body"].as_str().unwrap_or_default().to_string()).into_response(),
                    None => axum::Json(reply).into_response(),
                }
            }
        };
        let app = axum::Router::new().route("/createTask", axum::routing::post(handler.clone())).route("/getTaskResult", axum::routing::post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        BASE_URL.with(|base| *base.borrow_mut() = url);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        seen
    }

    fn created() -> Value {
        json!({ "errorId": 0, "taskId": 7 })
    }

    fn ready(solution: Value) -> Value {
        json!({ "errorId": 0, "status": "ready", "solution": solution })
    }

    fn processing() -> Value {
        json!({ "errorId": 0, "status": "processing" })
    }

    /// Ответ подставного 2Captcha не JSON: код HTTP и тело.
    fn http(code: u16, body: &str) -> Value {
        json!({ "$status": code, "$body": body })
    }

    /// Подставной браузер: вкладка на `PAGE`; поиск ключа сайта отвечает `found` (строка — ключ взятого виджета, не
    /// Enterprise), внедрение токена — `[1, 1]`.
    async fn browser(found: Value) -> Mock {
        browser_with(found, json!({ "result": { "type": "object", "value": [1, 1] } })).await
    }

    /// Как `browser`, но внедрение токена отвечает `injected` (ответ `Runtime.evaluate`). Для обычной капчи:
    /// сохранённые элементы на месте, рамка картинки — 100×40 в точке (10, 20), снимок — `SHOT`, ввод принимается.
    async fn browser_with(found: Value, injected: Value) -> Mock {
        let found = match found {
            Value::String(key) => json!({ "key": key, "widget": true, "enterprise": false }),
            other => other,
        };
        Mock::spawn(move |method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Target.getTargetInfo" => json!({ "targetInfo": { "url": PAGE } }),
                "Runtime.evaluate" if script(params).contains("const token") => injected.clone(),
                "Runtime.evaluate" => json!({ "result": { "value": found } }),
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "DOM.resolveNode" => json!({ "object": { "objectId": "O1" } }),
                "Runtime.callFunctionOn" => json!({ "result": { "type": "boolean", "value": true } }),
                "DOM.getContentQuads" => json!({ "quads": [[10, 20, 110, 20, 110, 60, 10, 60]] }),
                "Page.getLayoutMetrics" => json!({ "cssVisualViewport": { "pageX": 0, "pageY": 0 } }),
                "Page.captureScreenshot" => json!({ "data": SHOT }),
                _ => json!({}),
            })
        })
        .await
    }

    fn script(params: &Value) -> &str {
        params["expression"].as_str().unwrap_or_default()
    }

    /// Скрипты страницы: поиск ключа и внедрение токена.
    fn scripts(mock: &Mock) -> (Vec<String>, Vec<String>) {
        let all: Vec<String> = mock.sent("Runtime.evaluate").iter().map(|p| script(p).to_string()).collect();
        all.into_iter().partition(|s| !s.contains("const token"))
    }

    async fn ctx(mock: &Mock) -> Ctx {
        let mut ctx = mocked(mock).await;
        ctx.store = Some(Arc::new(memory_store()));
        ctx
    }

    fn flow(config: Value) -> Flow {
        let mut full = json!({ "api_key": KEY, "save_to": "ok", "answer_to": "token" });
        full.as_object_mut().unwrap().extend(config.as_object().unwrap().clone());
        chain(vec![json!({ "id": "c", "op": "ext.2captcha", "config": full })])
    }

    fn bodies(seen: &Seen, path: &str) -> Vec<Value> {
        seen.lock().unwrap().iter().filter(|(p, _)| p == path).map(|(_, body)| body.clone()).collect()
    }

    #[tokio::test]
    async fn creates_the_task_of_the_kind_for_the_tab_address() {
        for (kind, task) in [
            ("recaptcha_v2", "RecaptchaV2TaskProxyless"),
            ("recaptcha_v3", "RecaptchaV3TaskProxyless"),
            ("hcaptcha", "HCaptchaTaskProxyless"),
            ("turnstile", "TurnstileTaskProxyless"),
        ] {
            let seen = fake(created(), vec![ready(json!({ "token": "T" }))]).await;
            let mock = browser(json!("site-key")).await;
            let mut ctx = ctx(&mock).await;
            assert_eq!(ctx.run(&flow(json!({ "kind": kind }))).await, Ok(Exit::Done), "{kind}");
            let sent = &bodies(&seen, "/createTask")[0];
            assert_eq!(sent["clientKey"], KEY);
            assert_eq!(sent["task"]["type"], task);
            assert_eq!(sent["task"]["websiteURL"], PAGE);
            assert_eq!(sent["task"]["websiteKey"], "site-key");
            assert_eq!(sent["task"].get("minScore").is_some(), kind == "recaptcha_v3", "{kind}");
            let (find, _) = scripts(&mock);
            assert!(find[0].contains(&format!("const kind = \"{kind}\";")), "вид — из поля, без угадывания");
        }
        let seen = fake(created(), vec![ready(json!({ "gRecaptchaResponse": "T" }))]).await;
        let mock = browser(json!("site-key")).await;
        ctx(&mock).await.run(&flow(json!({ "kind": "recaptcha_v3", "action": "login", "min_score": "0.7" }))).await.unwrap();
        let task = &bodies(&seen, "/createTask")[0]["task"];
        assert_eq!((task["pageAction"].clone(), task["minScore"].clone()), (json!("login"), json!(0.7)));
        let seen = fake(created(), vec![ready(json!({ "gRecaptchaResponse": "T" }))]).await;
        let mock = browser(json!("site-key")).await;
        ctx(&mock).await.run(&flow(json!({ "kind": "recaptcha_v3" }))).await.unwrap();
        let task = &bodies(&seen, "/createTask")[0]["task"];
        assert_eq!((task.get("pageAction"), task["minScore"].clone()), (None, json!(0.3)), "умолчание min_score — 0.3");
    }

    #[tokio::test]
    async fn polls_until_ready_and_hands_the_token_to_the_page_and_answer_to() {
        let seen = fake(created(), vec![processing(), processing(), ready(json!({ "gRecaptchaResponse": "TOKEN-1" }))]).await;
        let mock = browser(json!("site-key")).await;
        let mut ctx = ctx(&mock).await;
        assert_eq!(ctx.run(&flow(json!({}))).await, Ok(Exit::Done));
        let polls = bodies(&seen, "/getTaskResult");
        assert_eq!(polls.len(), 3);
        assert!(polls.iter().all(|body| *body == json!({ "clientKey": KEY, "taskId": 7 })), "{polls:?}");
        assert_eq!(ctx.scope.get("ok"), Some(&json!(true)));
        assert_eq!(ctx.scope.get("token"), Some(&json!("TOKEN-1")));
        let (_, inject) = scripts(&mock);
        assert_eq!(inject.len(), 1);
        assert!(inject[0].contains("const kind = \"recaptcha_v2\";") && inject[0].contains("const token = \"TOKEN-1\";"), "{}", inject[0]);
        assert!(inject[0].contains("const sitekey = \"site-key\";") && inject[0].contains("const widget = true;"), "виджет, взятый поиском");
        for part in ["g-recaptcha-response", "h-captcha-response", "cf-turnstile-response", "data-callback", "___grecaptcha_cfg"] {
            assert!(inject[0].contains(part), "{part}");
        }
        let entries = format!("{:?}", ctx.journal.entries());
        assert!(!entries.contains(KEY), "ключа нет в журнале");
    }

    #[tokio::test]
    async fn service_errors_carry_the_code_without_the_key() {
        let refusals = [
            json!({ "errorId": 10, "errorCode": "ERROR_ZERO_BALANCE", "errorDescription": "You don't have funds on your account." }),
            json!({ "errorId": 1, "errorCode": "ERROR_KEY_DOES_NOT_EXIST", "errorDescription": format!("The key {KEY} is wrong") }),
        ];
        for refusal in refusals {
            let seen = fake(refusal.clone(), vec![processing()]).await;
            let mock = browser(json!("site-key")).await;
            let mut ctx = ctx(&mock).await;
            let result = ctx.run(&flow(json!({}))).await;
            let Err(Fail::Node { message, .. }) = result else { panic!("{result:?}") };
            let code = refusal["errorCode"].as_str().unwrap();
            assert!(message.starts_with(&format!("2Captcha: {code}")), "{message}");
            assert!(!message.contains(KEY), "{message}");
            assert!(bodies(&seen, "/getTaskResult").is_empty(), "после отказа не опрашивает");
            assert_eq!(ctx.scope.get("ok"), None);
            assert!(scripts(&mock).1.is_empty(), "токена нет — страницу не трогает");
        }
        fake(created(), vec![processing(), json!({ "errorId": 12, "errorCode": "ERROR_CAPTCHA_UNSOLVABLE" })]).await;
        let mock = browser(json!("site-key")).await;
        let Err(Fail::Node { message, .. }) = ctx(&mock).await.run(&flow(json!({}))).await else { panic!() };
        assert_eq!(message, "2Captcha: ERROR_CAPTCHA_UNSOLVABLE");
    }

    #[tokio::test]
    async fn unsolved_in_time_is_false_without_an_error() {
        TIMING.with(|t| t.set((Duration::from_millis(10), Duration::from_millis(150))));
        let seen = fake(created(), vec![processing()]).await;
        let mock = browser(json!("site-key")).await;
        let mut ctx = ctx(&mock).await;
        let result = ctx.run(&flow(json!({}))).await;
        TIMING.with(|t| t.set((Duration::from_millis(10), Duration::from_secs(5))));
        assert_eq!(result, Ok(Exit::Done));
        assert_eq!(ctx.scope.get("ok"), Some(&json!(false)));
        assert_eq!(ctx.scope.get("token"), None);
        assert!(bodies(&seen, "/getTaskResult").len() >= 2, "опрашивал до предела");
        assert!(scripts(&mock).1.is_empty());
    }

    #[tokio::test]
    async fn stop_interrupts_polling() {
        let seen = fake(created(), vec![processing()]).await;
        let mock = browser(json!("site-key")).await;
        let mut ctx = ctx(&mock).await;
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });
        let started = Instant::now();
        assert_eq!(ctx.run(&flow(json!({}))).await, Err(Fail::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(2), "не ждал предела");
        // Запрос, ушедший до «Стоп», сервер может записать чуть позже.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let polls = bodies(&seen, "/getTaskResult").len();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(bodies(&seen, "/getTaskResult").len(), polls, "опрос прекращён");
    }

    #[tokio::test]
    async fn the_site_key_is_found_on_the_page_or_taken_from_the_field() {
        let seen = fake(created(), vec![ready(json!({ "token": "T" }))]).await;
        let mock = browser(json!("6Lc-page")).await;
        ctx(&mock).await.run(&flow(json!({}))).await.unwrap();
        assert_eq!(bodies(&seen, "/createTask")[0]["task"]["websiteKey"], "6Lc-page");
        let (find, _) = scripts(&mock);
        for part in ["[data-sitekey]", ".g-recaptcha", ".h-captcha", ".cf-turnstile", "iframe[src]", "'k'", "'sitekey'"] {
            assert!(find[0].contains(part), "{part}");
        }
        let seen = fake(created(), vec![ready(json!({ "token": "T" }))]).await;
        let mock = browser(json!("6Lc-page")).await;
        ctx(&mock).await.run(&flow(json!({ "sitekey": " 6Lc-field " }))).await.unwrap();
        assert_eq!(bodies(&seen, "/createTask")[0]["task"]["websiteKey"], "6Lc-field");
        let (find, inject) = scripts(&mock);
        assert!(find[0].contains("const sitekey = \"6Lc-field\";"), "по ключу из поля ищется виджет для внедрения");
        assert!(inject[0].contains("const sitekey = \"6Lc-field\";"));
    }

    #[tokio::test]
    async fn invisible_recaptcha_v2_is_sent_as_invisible() {
        for (kind, enterprise, invisible, task, flag) in [
            ("recaptcha_v2", false, true, "RecaptchaV2TaskProxyless", Some(json!(true))),
            ("recaptcha_v2", true, true, "RecaptchaV2EnterpriseTaskProxyless", Some(json!(true))),
            ("recaptcha_v2", false, false, "RecaptchaV2TaskProxyless", None),
            ("recaptcha_v3", false, true, "RecaptchaV3TaskProxyless", None),
        ] {
            let seen = fake(created(), vec![ready(json!({ "gRecaptchaResponse": "T" }))]).await;
            let mock = browser(json!({ "key": "k", "widget": true, "enterprise": enterprise, "invisible": invisible })).await;
            ctx(&mock).await.run(&flow(json!({ "kind": kind }))).await.unwrap();
            let sent = &bodies(&seen, "/createTask")[0]["task"];
            assert_eq!((sent["type"].as_str(), sent.get("isInvisible").cloned()), (Some(task), flag), "{kind} {enterprise} {invisible}");
        }
    }

    #[tokio::test]
    async fn the_token_goes_to_the_widget_the_search_marked() {
        for widget in [true, false] {
            fake(created(), vec![ready(json!({ "token": "T" }))]).await;
            let mock = browser(json!({ "key": "k2", "widget": widget, "enterprise": false })).await;
            ctx(&mock).await.run(&flow(json!({}))).await.unwrap();
            let (find, inject) = scripts(&mock);
            let mark = find[0].lines().find(|line| line.starts_with("const mark = \"_")).expect("метка виджета");
            assert!(inject[0].lines().any(|line| line == mark), "та же метка: {mark}");
            assert!(inject[0].contains("const sitekey = \"k2\";") && inject[0].contains(&format!("const widget = {widget};")), "{}", inject[0]);
        }
        let mock = browser(json!("k")).await;
        fake(created(), vec![ready(json!({ "token": "T" }))]).await;
        ctx(&mock).await.run(&flow(json!({}))).await.unwrap();
        let other = browser(json!("k")).await;
        fake(created(), vec![ready(json!({ "token": "T" }))]).await;
        ctx(&other).await.run(&flow(json!({}))).await.unwrap();
        let mark = |mock: &Mock| scripts(mock).0[0].lines().find(|line| line.starts_with("const mark")).unwrap().to_string();
        assert_ne!(mark(&mock), mark(&other), "метка своя у каждого решения");
    }

    #[tokio::test]
    async fn a_widget_gone_while_solving_is_noted_and_the_token_kept() {
        fake(created(), vec![ready(json!({ "token": "TOKEN-7" }))]).await;
        let mock = browser_with(json!("site-key"), json!({ "result": { "type": "object", "value": [0, 0, true] } })).await;
        let mut ctx = ctx(&mock).await;
        assert_eq!(ctx.run(&flow(json!({}))).await, Ok(Exit::Done));
        assert_eq!((ctx.scope.get("ok"), ctx.scope.get("token")), (Some(&json!(true)), Some(&json!("TOKEN-7"))));
        let notes: Vec<&str> = ctx.journal.entries().iter().filter(|e| e.node == "c").map(|e| e.msg.as_str()).collect();
        assert!(notes.iter().any(|msg| msg.contains("Капча исчезла со страницы во время обработки. Ответ сохранён в указанную переменную.")), "{notes:?}");
    }

    #[tokio::test]
    async fn enterprise_recaptcha_buys_the_enterprise_task() {
        for (kind, task, flag) in [
            ("recaptcha_v2", "RecaptchaV2EnterpriseTaskProxyless", None),
            ("recaptcha_v3", "RecaptchaV3TaskProxyless", Some(json!(true))),
        ] {
            let seen = fake(created(), vec![ready(json!({ "gRecaptchaResponse": "T" }))]).await;
            let mock = browser(json!({ "key": "ek", "widget": 0, "enterprise": true })).await;
            ctx(&mock).await.run(&flow(json!({ "kind": kind }))).await.unwrap();
            let sent = &bodies(&seen, "/createTask")[0]["task"];
            assert_eq!((sent["type"].as_str(), sent.get("isEnterprise").cloned()), (Some(task), flag), "{kind}");
        }
        let seen = fake(created(), vec![ready(json!({ "gRecaptchaResponse": "T" }))]).await;
        let mock = browser(json!("k")).await;
        ctx(&mock).await.run(&flow(json!({ "kind": "recaptcha_v3" }))).await.unwrap();
        assert_eq!(bodies(&seen, "/createTask")[0]["task"].get("isEnterprise"), None);
    }

    #[tokio::test]
    async fn no_captcha_on_the_page_is_an_error_before_the_service() {
        let seen = fake(created(), vec![ready(json!({ "token": "T" }))]).await;
        let mock = browser(Value::Null).await;
        let result = ctx(&mock).await.run(&flow(json!({ "kind": "hcaptcha" }))).await;
        let Err(Fail::Node { message, .. }) = result else { panic!("{result:?}") };
        assert!(message.starts_with("2Captcha: на странице не найдена капча hCaptcha"), "{message}");
        assert!(seen.lock().unwrap().is_empty(), "2Captcha не вызывался — деньги не потрачены");
    }

    #[tokio::test]
    async fn key_and_browser_are_checked_before_the_service() {
        let seen = fake(created(), vec![ready(json!({ "token": "T" }))]).await;
        let mock = browser(json!("site-key")).await;
        let Err(Fail::Node { message, .. }) = ctx(&mock).await.run(&flow(json!({ "api_key": "" }))).await else { panic!() };
        assert_eq!(message, "2Captcha: не задан ключ API");
        let mut bare = Ctx::bare();
        bare.store = Some(Arc::new(memory_store()));
        let Err(Fail::Node { message, .. }) = bare.run(&flow(json!({}))).await else { panic!() };
        assert_eq!(message, "2Captcha: у запуска нет браузера");
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_page_exception_while_injecting_keeps_the_paid_token() {
        fake(created(), vec![ready(json!({ "token": "PAID" }))]).await;
        let thrown = json!({ "result": { "type": "object" }, "exceptionDetails": { "text": "Uncaught", "exception": { "description": "TypeError: x" } } });
        let mock = browser_with(json!("site-key"), thrown).await;
        let mut ctx = ctx(&mock).await;
        assert!(matches!(ctx.run(&flow(json!({}))).await, Err(Fail::Node { .. })));
        assert_eq!(ctx.scope.get("token"), Some(&json!("PAID")), "токен в answer_to до внедрения");
        let (_, inject) = scripts(&mock);
        assert!(inject[0].contains("try { fn(token); } catch {}") && inject[0].contains("anchor) try {"), "обработчики сайта — в try");
    }

    #[test]
    fn the_site_key_comes_from_the_own_class_then_the_own_frame_then_any_element() {
        let at = |part: &str| FIND.find(part).unwrap_or_else(|| panic!("{part}"));
        assert!(at("${own}[data-sitekey]") < at("iframe[src]"));
        assert!(at("iframe[src]") < at("querySelectorAll('[data-sitekey]')"), "голый [data-sitekey] — после фрейма своего вида");
        // Куда попадает токен — в DOM: captcha.test.ts (vitest).
        assert!(INJECT.ends_with("(kind, sitekey, mark, token, widget)") && FIND.ends_with("(kind, sitekey, mark, null, null)"));
    }

    #[tokio::test]
    async fn odd_poll_replies_are_retried_and_the_last_failure_is_noted() {
        let odd = json!({ "errorId": 0, "status": "queued" });
        let seen = fake(created(), vec![http(200, "<html>"), odd.clone(), ready(json!({ "token": "T" }))]).await;
        let mock = browser(json!("site-key")).await;
        let mut run = ctx(&mock).await;
        assert_eq!(run.run(&flow(json!({}))).await, Ok(Exit::Done));
        assert_eq!((bodies(&seen, "/getTaskResult").len(), run.scope.get("token")), (3, Some(&json!("T"))));
        TIMING.with(|t| t.set((Duration::from_millis(10), Duration::from_millis(150))));
        fake(created(), vec![processing(), http(200, "<html>")]).await;
        let mock = browser(json!("site-key")).await;
        let mut run = ctx(&mock).await;
        let result = run.run(&flow(json!({}))).await;
        TIMING.with(|t| t.set((Duration::from_millis(10), Duration::from_secs(5))));
        assert_eq!((result, run.scope.get("ok")), (Ok(Exit::Done), Some(&json!(false))));
        let notes: Vec<&str> = run.journal.entries().iter().filter(|e| e.node == "c").map(|e| e.msg.as_str()).collect();
        assert!(notes.iter().any(|msg| msg.contains("не решена за 0 с (последний сбой опроса: Сервис вернул ответ в неподдерживаемом формате)")), "{notes:?}");
    }

    #[tokio::test]
    async fn transient_poll_failures_are_retried_and_service_errors_are_not() {
        let seen = fake(created(), vec![http(502, "Bad Gateway"), http(500, "{}"), processing(), ready(json!({ "token": "T" }))]).await;
        let mock = browser(json!("site-key")).await;
        let mut run = ctx(&mock).await;
        assert_eq!(run.run(&flow(json!({}))).await, Ok(Exit::Done));
        assert_eq!(bodies(&seen, "/getTaskResult").len(), 4);
        assert_eq!(run.scope.get("token"), Some(&json!("T")));
        fake(created(), vec![http(503, ""), json!({ "errorId": 1, "errorCode": "ERROR_KEY_DOES_NOT_EXIST" })]).await;
        let mock = browser(json!("site-key")).await;
        let Err(Fail::Node { message, .. }) = ctx(&mock).await.run(&flow(json!({}))).await else { panic!() };
        assert_eq!(message, "2Captcha: ERROR_KEY_DOES_NOT_EXIST");
        for (reply, expected) in [(http(502, "Bad Gateway"), "2Captcha: HTTP 502 Bad Gateway"), (http(200, "<html>"), "2Captcha: Сервис вернул ответ в неподдерживаемом формате")] {
            let seen = fake(reply, vec![processing()]).await;
            let mock = browser(json!("site-key")).await;
            let Err(Fail::Node { message, .. }) = ctx(&mock).await.run(&flow(json!({}))).await else { panic!() };
            assert_eq!(message, expected);
            assert!(bodies(&seen, "/getTaskResult").is_empty(), "задача не создана — не опрашивает");
        }
    }

    #[tokio::test]
    async fn a_token_with_nowhere_to_go_is_solved_with_a_note() {
        for (kind, noted) in [("recaptcha_v2", true), ("hcaptcha", true), ("turnstile", true), ("recaptcha_v3", false)] {
            fake(created(), vec![ready(json!({ "token": "TOKEN-9" }))]).await;
            let mock = browser_with(json!("site-key"), json!({ "result": { "type": "object", "value": [0, 0] } })).await;
            let mut ctx = ctx(&mock).await;
            assert_eq!(ctx.run(&flow(json!({ "kind": kind }))).await, Ok(Exit::Done), "{kind}");
            assert_eq!(ctx.scope.get("ok"), Some(&json!(true)), "{kind}");
            let notes: Vec<&str> = ctx.journal.entries().iter().filter(|e| e.node == "c").map(|e| e.msg.as_str()).collect();
            assert_eq!(notes.iter().any(|msg| msg.contains("Поле ответа на странице не найдено. Ответ сохранён в указанную переменную.")), noted, "{kind}: {notes:?}");
            assert!(notes.iter().all(|msg| !msg.contains("TOKEN-9")), "токена нет в журнале");
        }
    }

    /// Картинка капчи и поле ответа — сохранённые элементы `${img}` (узел 5) и `${field}` (узел 6).
    async fn normal_ctx(mock: &Mock) -> Ctx {
        let mut ctx = ctx(mock).await;
        let tabs = ctx.tabs.as_mut().unwrap();
        tabs.page().await.unwrap();
        let (img, field) = (tabs.keep(ElementRef::test(5)), tabs.keep(ElementRef::test(6)));
        ctx.scope.set("img", img).unwrap();
        ctx.scope.set("field", field).unwrap();
        ctx
    }

    fn typed(mock: &Mock) -> String {
        mock.sent("Input.dispatchKeyEvent").iter().filter(|e| e["type"] == "keyDown").filter_map(|e| e["text"].as_str()).collect()
    }

    #[tokio::test]
    async fn normal_captcha_sends_the_element_shot_with_the_flags_and_types_the_answer() {
        let seen = fake(created(), vec![processing(), ready(json!({ "text": "W9xk" }))]).await;
        let mock = browser(Value::Null).await;
        let mut ctx = normal_ctx(&mock).await;
        let config = json!({
            "kind": "normal", "image": { "element": "${img}" }, "answer_target": { "element": "${field}" },
            "phrase": true, "case_sensitive": true, "numeric": "digits_and_letters", "min_len": 4, "max_len": "${max}",
        });
        ctx.scope.set("max", json!(6)).unwrap();
        assert_eq!(ctx.run(&flow(config)).await, Ok(Exit::Done));
        let task = &bodies(&seen, "/createTask")[0]["task"];
        let expected = json!({ "type": "ImageToTextTask", "body": SHOT, "phrase": true, "case": true, "numeric": 4, "math": false, "minLength": 4, "maxLength": 6 });
        assert_eq!(*task, expected);
        assert_eq!(mock.sent("Page.captureScreenshot")[0]["clip"]["width"], json!(100.0), "снимок — по рамке картинки");
        assert_eq!(mock.sent("DOM.focus"), [json!({ "backendNodeId": 6 })], "ввод — в поле ответа");
        assert_eq!(typed(&mock), "W9xk", "по символам, как Input");
        assert_eq!((ctx.scope.get("token"), ctx.scope.get("ok")), (Some(&json!("W9xk")), Some(&json!(true))));
        assert!(scripts(&mock).0.is_empty() && scripts(&mock).1.is_empty(), "ключ сайта не ищется, токен не внедряется");
        for (numeric, code) in [("any", 0), ("digits", 1), ("letters", 2), ("digits_or_letters", 3)] {
            let seen = fake(created(), vec![ready(json!({ "text": "1" }))]).await;
            let mock = browser(Value::Null).await;
            let mut ctx = normal_ctx(&mock).await;
            let config = json!({ "kind": "normal", "image": { "element": "${img}" }, "answer_target": { "element": "${field}" }, "numeric": numeric, "math": true });
            ctx.run(&flow(config)).await.unwrap();
            let task = &bodies(&seen, "/createTask")[0]["task"];
            assert_eq!((task["numeric"].clone(), task["math"].clone(), task["minLength"].clone(), task["phrase"].clone()), (json!(code), json!(true), json!(0), json!(false)));
        }
    }

    #[tokio::test]
    async fn normal_captcha_reads_the_image_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("captcha.jpg");
        std::fs::write(&file, b"\xff\xd8jpeg").unwrap();
        let seen = fake(created(), vec![ready(json!({ "text": "42" }))]).await;
        let mock = browser(Value::Null).await;
        let mut ctx = normal_ctx(&mock).await;
        let config = json!({ "kind": "normal", "image_path": file.to_str().unwrap(), "answer_target": { "element": "${field}" } });
        let started = Instant::now();
        assert_eq!(ctx.run(&flow(config)).await, Ok(Exit::Done));
        assert_eq!(bodies(&seen, "/createTask")[0]["task"]["body"], "/9hqcGVn");
        assert!(mock.sent("Page.captureScreenshot").is_empty());
        assert_eq!(typed(&mock), "42");
        assert!(started.elapsed() >= Duration::from_millis(600), "без очеловечивания — 300 мс на символ, как у Input");
    }

    #[tokio::test]
    async fn normal_captcha_fields_are_checked_before_the_service() {
        let seen = fake(created(), vec![ready(json!({ "text": "x" }))]).await;
        let field = json!({ "element": "${field}" });
        let image = json!({ "element": "${img}" });
        for (config, expected) in [
            (json!({ "kind": "normal", "answer_target": field }), "2Captcha: поле image или image_path не заполнено"),
            (json!({ "kind": "normal", "image_path": "  ", "answer_target": field }), "2Captcha: поле image или image_path не заполнено"),
            (json!({ "kind": "normal", "image": image }), "2Captcha: Заполните поле «Поле для ответа»"),
            (json!({ "kind": "normal", "image_path": "captcha.png", "answer_target": field }), "2Captcha: путь к изображению — абсолютный путь"),
            (json!({ "kind": "normal", "image": image, "answer_target": field, "max_len": 7 }), "2Captcha: поле max_len: от 0 до 6"),
            (json!({ "kind": "normal", "image": image, "answer_target": field, "min_len": -1 }), "2Captcha: поле min_len: от 0 до 6"),
            (json!({ "kind": "normal", "image": image, "answer_target": field, "min_len": 5, "max_len": 3 }), "2Captcha: поле min_len больше max_len"),
            (json!({ "kind": "normal", "image": image, "image_path": "/tmp/c.png", "answer_target": field }), "2Captcha: поля image и image_path заданы оба — нужно одно"),
        ] {
            let mock = browser(Value::Null).await;
            let result = normal_ctx(&mock).await.run(&flow(config.clone())).await;
            let Err(Fail::Node { message, .. }) = result else { panic!("{config}: {result:?}") };
            assert_eq!(message, expected, "{config}");
            assert!(mock.sent("Page.captureScreenshot").is_empty(), "{config}");
        }
        assert!(seen.lock().unwrap().is_empty(), "2Captcha не вызывался — деньги не потрачены");
    }
}
