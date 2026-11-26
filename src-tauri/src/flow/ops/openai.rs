//! OpenAI: ответ модели (Chat Completions) или картинка (Images: PNG в папке файлов запуска или base64) в переменную
//! (спецификация А4, «А4-2»).
//! Сеть — `services::http`: напрямую или через «Прокси для сервисов». Ключ API в ошибках и журнале не появляется.
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::files::create_new;
use crate::flow::ops::{Cfg, OpError};
use crate::flow::services;
use crate::flow::vars::to_text;
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde_json::{Value, json};
use std::io::Write;
use std::time::Duration;

#[cfg_attr(test, allow(dead_code))]
const BASE: &str = "https://api.openai.com/v1";

#[cfg(not(test))]
fn base() -> String {
    BASE.to_string()
}

#[cfg(test)]
fn base() -> String {
    tests::BASE_URL.with(|url| url.borrow().clone())
}

pub(crate) async fn exec(ctx: &mut Ctx, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let store = ctx.store.clone().ok_or("у запуска нет настроек сервисов")?;
    let key = cfg.text(&ctx.scope, "api_key")?;
    if key.is_empty() {
        return Err("не задан ключ API".to_string().into());
    }
    let image = cfg.string("output") == "image";
    let prompt = cfg.text(&ctx.scope, "prompt")?;
    let model = match cfg.text(&ctx.scope, "model")?.trim() {
        // Пустое поле — действующие модели OpenAI (gpt-image-1 и снимок gpt-4o-mini снимаются с API); любой другой id — как задан.
        "" if image => "gpt-image-2.5-flare".to_string(),
        "" => "gpt-6-luna".to_string(),
        model => model.to_string(),
    };
    let client = services::http(&store)?;
    if !image {
        let max = cfg.int(&ctx.scope, "max_tokens")?;
        let body = json!({ "model": model, "messages": [{ "role": "user", "content": prompt }], "max_completion_tokens": max });
        let answer = tokio::select! {
            answer = ask(&client, "chat/completions", &key, &body) => answer?,
            _ = ctx.cancel.cancelled() => return Err(OpError::Cancelled),
        };
        return Ok((Step::Port("next"), ctx.save(cfg, Value::String(reply(&answer)?))?));
    }
    // Контракт моделей gpt-image: без `response_format`, качество `auto` — умолчание сервиса, не отправляется.
    let mut body = json!({ "model": model, "prompt": prompt, "n": 1, "size": cfg.text(&ctx.scope, "size")? });
    let quality = cfg.string("quality");
    if quality != "auto" {
        body["quality"] = json!(quality);
    }
    let bytes = tokio::select! {
        bytes = picture(&client, &key, &body) => bytes?,
        _ = ctx.cancel.cancelled() => return Err(OpError::Cancelled),
    };
    let value = match cfg.string("format").as_str() {
        "b64_json" => B64.encode(&bytes),
        _ => png(ctx, &bytes)?,
    };
    Ok((Step::Port("next"), ctx.save(cfg, Value::String(value))?))
}

/// Картинка долго рисуется (gpt-image с высоким качеством — больше минуты): общий предел сервисов в 60 с её бы оборвал.
const IMAGE_TIMEOUT: Duration = Duration::from_secs(300);

/// Ответ сервиса как JSON; отказ (не 2xx) — текст сервиса без ключа.
async fn ask(client: &reqwest::Client, path: &str, key: &str, body: &Value) -> Result<Value, String> {
    let mut request = client.post(format!("{}/{path}", base())).bearer_auth(key).json(body);
    if path.starts_with("images/") {
        request = request.timeout(IMAGE_TIMEOUT);
    }
    let response = request.send().await.map_err(|e| hide(format!("сервис не ответил: {}", e.without_url()), key))?;
    let status = response.status();
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let reason = answer["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("HTTP {status}"));
        return Err(hide(reason, key));
    }
    Ok(answer)
}

/// Текст ассистента; отказ модели и пустой ответ (рассуждающей модели не хватило `max_tokens`) — ошибки.
fn reply(answer: &Value) -> Result<String, String> {
    let choice = &answer["choices"][0];
    if let Some(refusal) = choice["message"]["refusal"].as_str().filter(|text| !text.is_empty()) {
        return Err(format!("отказ модели: {refusal}"));
    }
    match choice["message"]["content"].as_str() {
        Some("") => Err(match choice["finish_reason"].as_str() {
            Some(reason) => {
                log::warn!("Причина завершения: {reason}");
                "Модель вернула пустой ответ".to_string()
            },
            None => "Модель вернула пустой ответ".to_string(),
        }),
        Some(text) => Ok(text.to_string()),
        None => Err("в ответе нет результата".to_string()),
    }
}

/// Байты картинки: base64 из ответа (gpt-image) или скачанные по ссылке (dall-e-2).
async fn picture(client: &reqwest::Client, key: &str, body: &Value) -> Result<Vec<u8>, String> {
    let answer = ask(client, "images/generations", key, body).await?;
    let data = &answer["data"][0];
    if let Some(b64) = data["b64_json"].as_str() {
        return B64.decode(b64).map_err(|_| {
            log::warn!("Изображение не соответствует формату Base64.");
            "Не удалось прочитать изображение в ответе сервиса"
        }.to_string());
    }
    let url = data["url"].as_str().ok_or("в ответе нет результата")?;
    let failed = |e: reqwest::Error| format!("Не удалось скачать изображение: {}", e.without_url());
    let mut response = client.get(url).timeout(IMAGE_TIMEOUT).send().await.map_err(failed)?;
    if !response.status().is_success() {
        return Err(format!("Не удалось скачать изображение: HTTP {}", response.status()));
    }
    let too_big = || "Изображение не скачано: размер превышает 50 МБ".to_string();
    if response.content_length().is_some_and(|size| size > IMAGE_MAX as u64) {
        return Err(too_big());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(failed)? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > IMAGE_MAX {
            return Err(too_big());
        }
    }
    Ok(bytes)
}

/// Предел скачиваемой картинки: настоящая — несколько мегабайт, больше — не картинка OpenAI.
const IMAGE_MAX: usize = 50 << 20;

/// PNG в папке файлов запуска: `openai-<acc_id>.png`, занятое имя — `openai-<acc_id>-1.png` и дальше.
fn png(ctx: &Ctx, bytes: &[u8]) -> Result<String, OpError> {
    let acc = ctx.scope.get("acc_id").map(to_text).unwrap_or_default();
    let name = if acc.is_empty() { "openai.png".to_string() } else { format!("openai-{acc}.png") };
    std::fs::create_dir_all(&ctx.files).map_err(|e| format!("папка файлов не создана: {e}"))?;
    let (path, mut file) = create_new(&ctx.files, &name)?;
    if let Err(e) = file.write_all(bytes) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("Не удалось сохранить изображение: {e}").into());
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Ключ в тексте ответа сервиса — звёздочками: сервис повторяет его в сообщении о неверном ключе.
pub(crate) fn hide(text: String, key: &str) -> String {
    text.replace(key, "***")
}

#[cfg(test)]
mod tests {
    use crate::flow::interp::{Ctx, Fail};
    use crate::flow::model::Flow;
    use crate::flow::vars::Scope;
    use crate::store::tests::memory_store;
    use serde_json::{Value, json};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    thread_local! {
        pub(super) static BASE_URL: RefCell<String> = RefCell::new("http://127.0.0.1:9".into());
    }

    type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

    /// Подставной OpenAI: `status` и тело ответа задаёт тест (`$BASE` в нём — адрес самого сервера); записывает путь,
    /// заголовок Authorization и тело запроса. `GET /img.png` отдаёт байты `PNGURL` — картинку по ссылке, `GET /big.png` —
    /// на байт больше 50 МБ.
    async fn fake(status: u16, answer: Value, delay_ms: u64) -> Seen {
        let seen: Seen = Default::default();
        let log = seen.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let answer: Value = serde_json::from_str(&answer.to_string().replace("$BASE", &url)).unwrap();
        let handler = move |uri: axum::http::Uri, headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
            let (log, answer) = (log.clone(), answer.clone());
            async move {
                let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
                log.lock().unwrap().push((uri.path().to_string(), auth, body));
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                (axum::http::StatusCode::from_u16(status).unwrap(), axum::Json(answer))
            }
        };
        let app = axum::Router::new()
            .route("/chat/completions", axum::routing::post(handler.clone()))
            .route("/images/generations", axum::routing::post(handler))
            .route("/img.png", axum::routing::get(|| async { b"PNGURL".to_vec() }))
            .route("/big.png", axum::routing::get(|| async { vec![0u8; (50 << 20) + 1] }));
        BASE_URL.with(|base| *base.borrow_mut() = url);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        seen
    }

    fn flow(config: Value) -> Flow {
        let mut full = json!({ "api_key": "sk-secret", "prompt": "привет", "save_to": "out" });
        full.as_object_mut().unwrap().extend(config.as_object().unwrap().clone());
        serde_json::from_value(json!({
            "format": "ombra-flow/1", "name": "t",
            "nodes": [{ "id": "s", "op": "start" }, { "id": "x", "op": "ext.openai", "config": full }],
            "edges": [{ "from": "s", "to": "x" }]
        }))
        .unwrap()
    }

    fn ctx() -> Ctx {
        let mut ctx = Ctx::bare();
        ctx.store = Some(Arc::new(memory_store()));
        ctx
    }

    /// Запуск профиля `k1` с папкой файлов `files`.
    fn image_ctx(files: &std::path::Path) -> Ctx {
        let mut ctx = ctx();
        ctx.scope = Scope::new(HashMap::from([("acc_id", json!("k1"))]));
        ctx.files = files.to_path_buf();
        ctx
    }

    fn reply(content: &str) -> Value {
        json!({ "choices": [{ "message": { "role": "assistant", "content": content } }] })
    }

    #[tokio::test]
    async fn text_sends_model_prompt_limit_and_key_and_saves_the_answer() {
        let seen = fake(200, reply("Здравствуйте"), 0).await;
        let mut ctx = ctx();
        ctx.run(&flow(json!({ "model": "gpt-x", "max_tokens": 77 }))).await.unwrap();
        assert_eq!(ctx.scope.get("out"), Some(&json!("Здравствуйте")));
        let seen = seen.lock().unwrap();
        let (path, auth, body) = &seen[0];
        assert_eq!((path.as_str(), auth.as_str()), ("/chat/completions", "Bearer sk-secret"));
        assert_eq!(body["model"], "gpt-x");
        assert_eq!(body["max_completion_tokens"], 77);
        assert_eq!(body["messages"][0], json!({ "role": "user", "content": "привет" }));
    }

    #[tokio::test]
    async fn empty_model_takes_the_default_of_the_output() {
        let seen = fake(200, reply("a"), 0).await;
        ctx().run(&flow(json!({}))).await.unwrap();
        assert_eq!(seen.lock().unwrap()[0].2["model"], "gpt-6-luna");
        assert_eq!(seen.lock().unwrap()[0].2["max_completion_tokens"], 2000);
        let dir = tempfile::tempdir().unwrap();
        let seen = fake(200, json!({ "data": [{ "b64_json": "UE5HMQ==" }] }), 0).await;
        image_ctx(dir.path()).run(&flow(json!({ "output": "image", "model": " " }))).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].0, "/images/generations");
        let body = &seen[0].2;
        assert_eq!(body["model"], "gpt-image-2.5-flare");
        assert_eq!(body["size"], "1024x1024");
        assert!(body.get("quality").is_none(), "auto не отправляется: {body}");
        assert!(body.get("response_format").is_none(), "{body}");
    }

    #[tokio::test]
    async fn image_is_written_to_a_png_of_the_run_or_saved_as_base64() {
        let dir = tempfile::tempdir().unwrap();
        let seen = fake(200, json!({ "data": [{ "b64_json": "UE5HMQ==" }] }), 0).await;
        let mut ctx = image_ctx(dir.path());
        let mut paths = Vec::new();
        for _ in 0..2 {
            ctx.run(&flow(json!({ "output": "image", "quality": "high" }))).await.unwrap();
            paths.push(ctx.scope.get("out").and_then(Value::as_str).unwrap().to_string());
        }
        for path in &paths {
            let path = std::path::Path::new(path);
            assert_eq!(path.parent(), Some(dir.path()), "в папке файлов запуска");
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with("openai-k1") && name.ends_with(".png"), "{name}");
            assert_eq!(std::fs::read(path).unwrap(), b"PNG1");
        }
        assert_ne!(paths[0], paths[1], "второй запуск не перетирает первый файл");
        assert_eq!(seen.lock().unwrap()[0].2["quality"], "high");
        ctx.run(&flow(json!({ "output": "image", "format": "b64_json" }))).await.unwrap();
        assert_eq!(ctx.scope.get("out"), Some(&json!("UE5HMQ==")));
        assert!(seen.lock().unwrap().iter().all(|(_, _, body)| body.get("response_format").is_none()));
        // Ссылка вместо base64 (dall-e-2) — картинка скачивается в файл так же.
        fake(200, json!({ "data": [{ "url": "$BASE/img.png" }] }), 0).await;
        let mut ctx = image_ctx(dir.path());
        ctx.run(&flow(json!({ "output": "image", "model": "dall-e-2" }))).await.unwrap();
        assert_eq!(std::fs::read(ctx.scope.get("out").and_then(Value::as_str).unwrap()).unwrap(), b"PNGURL");
    }

    #[tokio::test]
    async fn an_oversized_image_download_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        fake(200, json!({ "data": [{ "url": "$BASE/big.png" }] }), 0).await;
        let mut ctx = image_ctx(dir.path());
        let Err(Fail::Node { message, .. }) = ctx.run(&flow(json!({ "output": "image" }))).await else { panic!() };
        assert_eq!(message, "OpenAI: Изображение не скачано: размер превышает 50 МБ");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "файл не создан");
    }

    #[tokio::test]
    async fn a_refusal_or_an_empty_answer_is_a_node_error() {
        let refusal = json!({ "choices": [{ "message": { "role": "assistant", "content": null, "refusal": "I can't help with that." } }] });
        fake(200, refusal, 0).await;
        let Err(Fail::Node { message, .. }) = ctx().run(&flow(json!({}))).await else { panic!() };
        assert_eq!(message, "OpenAI: отказ модели: I can't help with that.");
        fake(200, json!({ "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "length" }] }), 0).await;
        let mut ctx = ctx();
        let Err(Fail::Node { message, .. }) = ctx.run(&flow(json!({}))).await else { panic!() };
        assert_eq!(message, "OpenAI: Модель вернула пустой ответ");
        assert_eq!(ctx.scope.get("out"), None);
    }

    #[tokio::test]
    async fn refusals_carry_the_service_message_without_the_key() {
        for (status, message) in [(401, "Incorrect API key provided: sk-secret."), (429, "Rate limit reached")] {
            fake(status, json!({ "error": { "message": message, "type": "x" } }), 0).await;
            let mut ctx = ctx();
            let result = ctx.run(&flow(json!({}))).await;
            let Err(Fail::Node { message: got, .. }) = result else { panic!("{result:?}") };
            assert_eq!(got, format!("OpenAI: {}", message.replace("sk-secret", "***")));
            assert!(!format!("{:?}", ctx.journal.entries()).contains("sk-secret"));
        }
        fake(500, json!({}), 0).await;
        let Err(Fail::Node { message, .. }) = ctx().run(&flow(json!({}))).await else { panic!() };
        assert!(message.starts_with("OpenAI: HTTP 500"), "{message}");
    }

    #[tokio::test]
    async fn stop_interrupts_the_request() {
        fake(200, reply("late"), 5000).await;
        let mut ctx = ctx();
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            cancel.cancel();
        });
        let started = std::time::Instant::now();
        assert_eq!(ctx.run(&flow(json!({}))).await, Err(Fail::Cancelled));
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }

    #[tokio::test]
    async fn missing_key_and_bad_secret_reference_are_errors_without_the_key() {
        let Err(Fail::Node { message, .. }) = ctx().run(&flow(json!({ "api_key": "" }))).await else { panic!() };
        assert_eq!(message, "OpenAI: не задан ключ API");
        let mut ctx = ctx();
        let Err(Fail::Node { message, .. }) = ctx.run(&flow(json!({ "api_key": "Ab${12" }))).await else { panic!() };
        assert!(!message.contains("Ab"), "{message}");
    }

    #[test]
    fn image_quality_choices_follow_gpt_image_2_5() {
        let spec = crate::flow::ops::spec("ext.openai").unwrap();
        let quality = spec.fields.iter().find(|field| field.name == "quality").unwrap();
        assert_eq!(quality.kind.options(), ["auto", "low", "medium", "high", "xhigh", "max"]);
        assert_eq!(quality.default, Some(r#""auto""#));
    }

    #[test]
    fn palette_has_the_third_party_group() {
        assert_eq!(crate::flow::ops::group("ext.openai"), "ext");
        let ru: Value = serde_json::from_str(include_str!("../../../../src/i18n/ru.json")).unwrap();
        assert_eq!(ru["automation"]["groups"]["ext"], "Сторонние сервисы");
    }
}
