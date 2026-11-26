//! Google Sheet: чтение, запись и очистка диапазона (спецификация А4, «А4-2»). Доступ — ключ сервисного аккаунта
//! из настроек (`flow::google`); без ключа `read` берёт публичную выгрузку CSV. Сеть — `services::http`.
use crate::flow::google;
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::{Cfg, OpError};
use crate::flow::services;
use crate::store::Store;
use serde_json::{Map, Value, json};

const SCOPE: &str = "https://www.googleapis.com/auth/spreadsheets";
#[cfg_attr(test, allow(dead_code))]
const SHEETS: &str = "https://sheets.googleapis.com/v4";
#[cfg_attr(test, allow(dead_code))]
const DOCS: &str = "https://docs.google.com";
const NO_KEY: &str = "нужен ключ сервисного аккаунта в настройках";

#[cfg(not(test))]
fn bases() -> (String, String) {
    (SHEETS.to_string(), DOCS.to_string())
}

#[cfg(test)]
fn bases() -> (String, String) {
    tests::BASES.with(|b| b.borrow().clone())
}

pub(crate) async fn exec(ctx: &mut Ctx, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let store = ctx.store.clone().ok_or("у запуска нет настроек сервисов")?;
    let operation = cfg.string("operation");
    let id = sheet_id(&cfg.text(&ctx.scope, "spreadsheet_id")?)?;
    let range = a1(&cfg.text(&ctx.scope, "sheet")?, &cfg.text(&ctx.scope, "range")?);
    let header = cfg.flag("header");
    if operation == "read" && cfg.var_out("save_to").is_none() {
        return Err("для чтения нужно поле «Сохранить в»".to_string().into());
    }
    let write = if operation == "write" { Some(rows(&ctx.scope.resolve(&cfg.raw("data"))?, header)?) } else { None };
    // Каждая операция — явно: неизвестное значение (число из импорта, опечатка) — ошибка, а не очистка.
    let work = async {
        match (operation.as_str(), write) {
            ("read", _) => read(&store, &id, &range, header).await.map(Some),
            ("write", Some(values)) => send(&store, &id, &range, Some(values)).await.map(|_| None),
            ("clear", _) => send(&store, &id, &range, None).await.map(|_| None),
            _ => Err("операция — read, write или clear".to_string()),
        }
    };
    let out = tokio::select! {
        out = work => out?,
        _ = ctx.cancel.cancelled() => return Err(OpError::Cancelled),
    };
    match out {
        Some(value) => Ok((Step::Port("next"), ctx.save(cfg, value)?)),
        None => Ok((Step::Port("next"), String::new())),
    }
}

/// Id из ссылки `…/spreadsheets/d/<id>/…` или сам id. Ссылка публикации (`…/d/e/2PACX…`) — не id таблицы.
fn sheet_id(text: &str) -> Result<String, String> {
    let text = text.trim();
    let id = match text.split_once("/spreadsheets/d/") {
        Some((_, rest)) if rest.starts_with("e/") => return Err("ссылка публикации не подходит — нужна ссылка на таблицу".into()),
        Some((_, rest)) => rest.split(['/', '?', '#']).next().unwrap_or(""),
        None => text,
    };
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("таблица: нужен id или ссылка на таблицу Google".into());
    }
    Ok(id.to_string())
}

/// Лист и ячейки. Лист в `range` (`Лист!A1:B2`, `'Лист 1'!A1`, `'Sales!2026'!A1`) важнее поля `sheet`: делится по `!`
/// вне кавычек, снимаются только обрамляющие кавычки, `''` в имени — одна `'`.
fn a1(sheet: &str, range: &str) -> (String, String) {
    let (sheet, range) = (sheet.trim(), range.trim());
    if let Some(quoted) = range.strip_prefix('\'') {
        let mut name = String::new();
        let mut chars = quoted.char_indices();
        while let Some((i, c)) = chars.next() {
            if c != '\'' {
                name.push(c);
            } else if quoted[i + 1..].starts_with('\'') {
                name.push('\'');
                chars.next();
            } else {
                let rest = &quoted[i + 1..];
                return (name, rest.strip_prefix('!').unwrap_or(rest).to_string());
            }
        }
    } else if let Some((name, cells)) = range.split_once('!') {
        return (name.to_string(), cells.to_string());
    }
    (if sheet.is_empty() { "Sheet1" } else { sheet }.to_string(), range.to_string())
}

/// Имя листа в A1 — всегда в кавычках: `A1` или `Q1` без них — ячейка первого листа, а не лист.
fn quoted(sheet: &str) -> String {
    format!("'{}'", sheet.replace('\'', "''"))
}

/// Массив массивов как есть; массив объектов — со строкой заголовков при `header`.
fn rows(data: &Value, header: bool) -> Result<Vec<Vec<Value>>, String> {
    let list = data.as_array().filter(|l| !l.is_empty()).ok_or("данные для записи: нужен непустой массив массивов или объектов")?;
    let cell = |v: &Value| match v {
        Value::Null => Value::String(String::new()),
        Value::Array(_) | Value::Object(_) => Value::String(v.to_string()),
        other => other.clone(),
    };
    if list.iter().all(Value::is_array) {
        return Ok(list.iter().map(|r| r.as_array().unwrap().iter().map(cell).collect()).collect());
    }
    if !list.iter().all(Value::is_object) {
        return Err("данные для записи: нужен массив массивов или массив объектов".into());
    }
    let mut keys: Vec<&String> = Vec::new();
    for row in list {
        for key in row.as_object().unwrap().keys() {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    let mut out: Vec<Vec<Value>> = Vec::new();
    if header {
        out.push(keys.iter().map(|k| Value::String((*k).clone())).collect());
    }
    for row in list {
        out.push(keys.iter().map(|k| row.get(k.as_str()).map_or(Value::String(String::new()), cell)).collect());
    }
    Ok(out)
}

/// Строки как есть или объекты по первой строке-заголовку. Ключи не теряют столбцов: пустой заголовок (и столбец шире
/// заголовка) — `column_<номер>`, повтор — `имя_2`, `имя_3`.
fn shape(mut table: Vec<Vec<Value>>, header: bool) -> Value {
    if !header {
        return Value::Array(table.into_iter().map(Value::Array).collect());
    }
    if table.is_empty() {
        return json!([]);
    }
    let titles = table.remove(0);
    let width = table.iter().map(Vec::len).chain([titles.len()]).max().unwrap_or(0);
    let mut keys: Vec<String> = Vec::new();
    for i in 0..width {
        let title = titles.get(i).map(crate::flow::vars::to_text).unwrap_or_default();
        let base = if title.trim().is_empty() { format!("column_{}", i + 1) } else { title };
        let (mut key, mut n) = (base.clone(), 1);
        while keys.contains(&key) {
            n += 1;
            key = format!("{base}_{n}");
        }
        keys.push(key);
    }
    Value::Array(
        table
            .into_iter()
            .map(|row| {
                let mut object = Map::new();
                for (i, key) in keys.iter().enumerate() {
                    object.insert(key.clone(), row.get(i).cloned().unwrap_or(Value::String(String::new())));
                }
                Value::Object(object)
            })
            .collect(),
    )
}

fn url(base: &str, segments: &[&str], query: &[(&str, &str)]) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(base).map_err(|e| e.to_string())?;
    url.path_segments_mut().map_err(|_| "адрес сервиса")?.pop_if_empty().extend(segments);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url)
}

/// Текст отказа Google; подсказка открыть таблицу аккаунту — только при отказе в доступе: при выключенном API («has not
/// been used … or it is disabled») она бы увела не туда.
fn fail(status: reqwest::StatusCode, body: &Value, email: &str) -> String {
    let reason = body["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("HTTP {status}"));
    match status == reqwest::StatusCode::FORBIDDEN && reason.to_lowercase().contains("permission") {
        true => format!("{reason}. Предоставьте пользователю {email} доступ к таблице."),
        false => reason,
    }
}

/// Запрос к Sheets API с токеном ключа. 401 — токен отвергнут (истёк, пока компьютер спал, или отозван): он забыт,
/// запрос повторяется один раз с новым. Отказ — текст Google без токена.
async fn authorized(store: &Store, email: &str, request: impl Fn(&str) -> reqwest::RequestBuilder) -> Result<reqwest::Response, String> {
    let mut fresh = false;
    loop {
        let token = google::token(store, SCOPE).await?;
        let resp = request(&token).send().await.map_err(|e| format!("сервис не ответил: {}", e.without_url()))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED && !fresh {
            google::forget(email, SCOPE);
            fresh = true;
            continue;
        }
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        return Err(fail(status, &body, email).replace(&token, "***"));
    }
}

async fn read(store: &Store, id: &str, (sheet, cells): &(String, String), header: bool) -> Result<Value, String> {
    let client = services::http(store)?;
    let (sheets, docs) = bases();
    let Some(email) = google::key_email(store)? else {
        // Без ключа — публичная выгрузка CSV. Число строк заголовка gviz угадывает сама и склеивает текстовые строки
        // данных в подписи столбцов; `headers=1` — ровно первая строка, она же первая строка CSV: `header` работает по
        // строкам как есть.
        let mut query = vec![("tqx", "out:csv"), ("sheet", sheet.as_str()), ("headers", "1")];
        if !cells.is_empty() {
            query.push(("range", cells));
        }
        let resp = client.get(url(&docs, &["spreadsheets", "d", id, "gviz", "tq"], &query)?).send().await;
        let resp = resp.map_err(|e| format!("сервис не ответил: {}", e.without_url()))?;
        let status = resp.status();
        let html = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.contains("html"));
        if !status.is_success() || html {
            return Err("Таблица не найдена или к ней нет доступа. Загрузите ключ сервисного аккаунта в настройках и предоставьте ему доступ к таблице.".into());
        }
        let text = resp.text().await.map_err(|e| e.without_url().to_string())?;
        let mut reader = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(text.as_bytes());
        let table = reader
            .records()
            .map(|r| r.map(|r| r.iter().map(|c| Value::String(c.to_string())).collect()).map_err(|e| format!("выгрузка CSV: {e}")))
            .collect::<Result<Vec<Vec<Value>>, String>>()?;
        return Ok(shape(table, header));
    };
    let range = if cells.is_empty() { quoted(sheet) } else { format!("{}!{cells}", quoted(sheet)) };
    let target = url(&sheets, &["spreadsheets", id, "values", &range], &[("majorDimension", "ROWS")])?;
    let resp = authorized(store, &email, |token| client.get(target.clone()).bearer_auth(token)).await?;
    // Пустой лист — только разобранный ответ без `values`; непрочитанный ответ — ошибка, а не пустая таблица.
    let body: Value = resp.json().await.map_err(|e| format!("ответ Google не прочитан: {}", e.without_url()))?;
    let table = body["values"].as_array().cloned().unwrap_or_default().into_iter().map(|r| r.as_array().cloned().unwrap_or_default()).collect();
    Ok(shape(table, header))
}

/// `Some(values)` — запись, `None` — очистка; оба требуют ключ.
async fn send(store: &Store, id: &str, (sheet, cells): &(String, String), values: Option<Vec<Vec<Value>>>) -> Result<(), String> {
    let Some(email) = google::key_email(store)? else { return Err(NO_KEY.into()) };
    let client = services::http(store)?;
    let (sheets, _) = bases();
    let cells = if cells.is_empty() && values.is_some() { "A1" } else { cells };
    let range = if cells.is_empty() { quoted(sheet) } else { format!("{}!{cells}", quoted(sheet)) };
    match &values {
        Some(values) => {
            let target = url(&sheets, &["spreadsheets", id, "values", &range], &[("valueInputOption", "USER_ENTERED")])?;
            let body = json!({ "range": range, "majorDimension": "ROWS", "values": values });
            authorized(store, &email, |token| client.put(target.clone()).json(&body).bearer_auth(token)).await?
        }
        None => {
            let target = url(&sheets, &["spreadsheets", id, "values", &format!("{range}:clear")], &[])?;
            authorized(store, &email, |token| client.post(target.clone()).json(&json!({})).bearer_auth(token)).await?
        }
    };
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::flow::google::tests::{TOKEN_URL, key_json, store};
    use crate::flow::interp::{Ctx, Fail};
    use crate::flow::model::Flow;
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    thread_local! {
        pub(super) static BASES: RefCell<(String, String)> = RefCell::new(("http://127.0.0.1:9".into(), "http://127.0.0.1:9".into()));
    }

    /// (метод, путь как пришёл, запрос, заголовок Authorization, тело).
    pub(crate) type Seen = Arc<Mutex<Vec<(String, String, String, String, String)>>>;

    /// Подставной Google: токен, Sheets API (`status`/`answer` задаёт тест; `"$not-json"` — тело не JSON) и публичная
    /// выгрузка CSV; токен `stale` — 401, как у истёкшего.
    pub(crate) async fn fake(status: u16, answer: Value, csv: &str) -> Seen {
        let seen: Seen = Default::default();
        let log = seen.clone();
        let csv = csv.to_string();
        let app = axum::Router::new().fallback(move |method: axum::http::Method, uri: axum::http::Uri, headers: axum::http::HeaderMap, body: String| {
            let (log, answer, csv) = (log.clone(), answer.clone(), csv.clone());
            async move {
                let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
                let stale = auth == "Bearer stale";
                log.lock().unwrap().push((method.to_string(), uri.path().to_string(), uri.query().unwrap_or("").to_string(), auth, body));
                if uri.path() == "/token" {
                    return (axum::http::StatusCode::OK, axum::Json(json!({ "access_token": "at-sheet", "expires_in": 3600 }))).into_response();
                }
                if stale {
                    let refused = json!({ "error": { "code": 401, "message": "Request had invalid authentication credentials." } });
                    return (axum::http::StatusCode::UNAUTHORIZED, axum::Json(refused)).into_response();
                }
                if answer == json!("$not-json") {
                    return (axum::http::StatusCode::OK, "<html>").into_response();
                }
                if uri.path().ends_with("/gviz/tq") {
                    return (axum::http::StatusCode::OK, csv).into_response();
                }
                (axum::http::StatusCode::from_u16(status).unwrap(), axum::Json(answer)).into_response()
            }
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        BASES.with(|b| *b.borrow_mut() = (base.clone(), base.clone()));
        TOKEN_URL.with(|u| *u.borrow_mut() = format!("{base}/token"));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        seen
    }

    use axum::response::IntoResponse;

    pub(crate) fn flow(config: Value) -> Flow {
        let mut full = json!({ "spreadsheet_id": "SID123", "save_to": "out" });
        full.as_object_mut().unwrap().extend(config.as_object().unwrap().clone());
        serde_json::from_value(json!({
            "format": "ombra-flow/1", "name": "t",
            "nodes": [{ "id": "s", "op": "start" }, { "id": "x", "op": "ext.google_sheet", "config": full }],
            "edges": [{ "from": "s", "to": "x" }]
        }))
        .unwrap()
    }

    /// Контекст с ключом: адрес уникален в тесте (общий кэш токенов).
    fn ctx(email: Option<&str>) -> Ctx {
        let s = store();
        if let Some(email) = email {
            google::set_key(&s, &key_json(email, "service_account")).unwrap();
        }
        let mut ctx = Ctx::bare();
        ctx.store = Some(Arc::new(s));
        ctx
    }

    #[tokio::test]
    async fn read_with_header_gives_objects_and_encodes_the_range() {
        let seen = fake(200, json!({ "values": [["имя", "возраст"], ["Аня", "30"], ["Боб"]] }), "").await;
        let mut ctx = ctx(Some("read1@x.iam"));
        ctx.run(&flow(json!({ "sheet": "Лист 1", "range": "A1:C10" }))).await.unwrap();
        assert_eq!(ctx.scope.get("out"), Some(&json!([{ "имя": "Аня", "возраст": "30" }, { "имя": "Боб", "возраст": "" }])));
        let seen = seen.lock().unwrap();
        let (method, path, _, auth, _) = &seen[1];
        assert_eq!((method.as_str(), auth.as_str()), ("GET", "Bearer at-sheet"));
        assert!(path.starts_with("/spreadsheets/SID123/values/") && path.contains("%D0%9B") && path.contains("A1:C10"), "{path}");
        assert!(!path.contains(' ') && !path.contains("Лист"), "{path}");
    }

    #[tokio::test]
    async fn read_without_header_gives_arrays_and_plain_sheet_path() {
        let seen = fake(200, json!({ "values": [["a", "b"], ["c"]] }), "").await;
        let mut ctx = ctx(Some("read2@x.iam"));
        ctx.run(&flow(json!({ "header": false, "range": "Sheet1!A1:C10" }))).await.unwrap();
        assert_eq!(ctx.scope.get("out"), Some(&json!([["a", "b"], ["c"]])));
        assert_eq!(seen.lock().unwrap()[1].1, "/spreadsheets/SID123/values/'Sheet1'!A1:C10");
        // Пустой лист
        fake(200, json!({}), "").await;
        let mut ctx = self::ctx(Some("read2@x.iam"));
        ctx.run(&flow(json!({}))).await.unwrap();
        assert_eq!(ctx.scope.get("out"), Some(&json!([])));
    }

    #[tokio::test]
    async fn link_instead_of_id_is_parsed() {
        let seen = fake(200, json!({ "values": [] }), "").await;
        let mut ctx = ctx(Some("link@x.iam"));
        ctx.run(&flow(json!({ "spreadsheet_id": "https://docs.google.com/spreadsheets/d/AbC-1_x/edit#gid=0" }))).await.unwrap();
        assert!(seen.lock().unwrap()[1].1.starts_with("/spreadsheets/AbC-1_x/values/"));
    }

    #[tokio::test]
    async fn write_sends_user_entered_objects_with_header_and_arrays_as_is() {
        let seen = fake(200, json!({ "updatedCells": 4 }), "").await;
        let mut ctx = ctx(Some("write1@x.iam"));
        ctx.run(&flow(json!({ "operation": "write", "data": [{ "a": 1, "b": "x" }, { "a": 2 }] }))).await.unwrap();
        {
            let seen = seen.lock().unwrap();
            let (method, path, query, auth, body) = &seen[1];
            assert_eq!((method.as_str(), auth.as_str(), query.as_str()), ("PUT", "Bearer at-sheet", "valueInputOption=USER_ENTERED"));
            assert_eq!(path, "/spreadsheets/SID123/values/'Sheet1'!A1");
            let body: Value = serde_json::from_str(body).unwrap();
            assert_eq!(body["values"], json!([["a", "b"], [1, "x"], [2, ""]]));
        }
        ctx.run(&flow(json!({ "operation": "write", "header": false, "range": "B2", "data": [{ "a": 1 }, [1, 2]] }))).await.unwrap_err();
        ctx.run(&flow(json!({ "operation": "write", "header": false, "range": "B2", "data": [["p", 5], ["q"]] }))).await.unwrap();
        let seen = seen.lock().unwrap();
        let last = seen.last().unwrap();
        assert_eq!(last.1, "/spreadsheets/SID123/values/'Sheet1'!B2");
        assert_eq!(serde_json::from_str::<Value>(&last.4).unwrap()["values"], json!([["p", 5], ["q"]]));
    }

    #[tokio::test]
    async fn write_needs_an_array() {
        fake(200, json!({}), "").await;
        for data in [json!("текст"), json!(5), json!([]), json!([1, 2])] {
            let Err(Fail::Node { message, .. }) = ctx(Some("write2@x.iam")).run(&flow(json!({ "operation": "write", "data": data }))).await else { panic!() };
            assert!(message.contains("данные для записи"), "{message}");
        }
    }

    #[tokio::test]
    async fn clear_posts_values_clear() {
        let seen = fake(200, json!({}), "").await;
        ctx(Some("clear1@x.iam")).run(&flow(json!({ "operation": "clear", "range": "A1:B2" }))).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!((seen[1].0.as_str(), seen[1].1.as_str()), ("POST", "/spreadsheets/SID123/values/'Sheet1'!A1:B2:clear"));
    }

    #[tokio::test]
    async fn without_key_read_goes_to_public_csv_and_write_clear_fail() {
        let seen = fake(200, json!({}), "имя,возраст\nАня,30\nБоб\n").await;
        let mut c = ctx(None);
        c.run(&flow(json!({ "range": "A1:B3" }))).await.unwrap();
        assert_eq!(c.scope.get("out"), Some(&json!([{ "имя": "Аня", "возраст": "30" }, { "имя": "Боб", "возраст": "" }])));
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].1, "/spreadsheets/d/SID123/gviz/tq");
            assert_eq!(seen[0].2, "tqx=out%3Acsv&sheet=Sheet1&headers=1&range=A1%3AB3", "ровно одна строка заголовка");
        }
        // Без «первая строка — ключи» — строки как есть, первая тоже.
        let mut c = ctx(None);
        c.run(&flow(json!({ "header": false }))).await.unwrap();
        assert_eq!(c.scope.get("out"), Some(&json!([["имя", "возраст"], ["Аня", "30"], ["Боб"]])));
        assert_eq!(seen.lock().unwrap()[1].2, "tqx=out%3Acsv&sheet=Sheet1&headers=1");
        for op in ["write", "clear"] {
            let Err(Fail::Node { message, .. }) = ctx(None).run(&flow(json!({ "operation": op, "data": [[1]] }))).await else { panic!() };
            assert_eq!(message, "Google Sheet: нужен ключ сервисного аккаунта в настройках");
        }
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn forbidden_hints_the_service_account_address() {
        fake(403, json!({ "error": { "message": "The caller does not have permission" } }), "").await;
        let Err(Fail::Node { message, .. }) = ctx(Some("forbid@x.iam")).run(&flow(json!({}))).await else { panic!() };
        assert_eq!(message, "Google Sheet: The caller does not have permission. Предоставьте пользователю forbid@x.iam доступ к таблице.");
        fake(404, json!({}), "").await;
        let Err(Fail::Node { message, .. }) = ctx(Some("forbid@x.iam")).run(&flow(json!({}))).await else { panic!() };
        assert!(message.contains("HTTP 404") && !message.contains("откройте"), "{message}");
        let disabled = "Google Sheets API has not been used in project 123 before or it is disabled. Enable it by visiting …";
        fake(403, json!({ "error": { "message": disabled, "status": "PERMISSION_DENIED" } }), "").await;
        let Err(Fail::Node { message, .. }) = ctx(Some("forbid@x.iam")).run(&flow(json!({}))).await else { panic!() };
        assert_eq!(message, format!("Google Sheet: {disabled}"), "API выключено — открывать таблицу бесполезно");
    }

    #[tokio::test]
    async fn read_needs_save_to_and_a_valid_table() {
        fake(200, json!({}), "").await;
        let Err(Fail::Node { message, .. }) = ctx(Some("v@x.iam")).run(&flow(json!({ "save_to": "" }))).await else { panic!() };
        assert!(message.contains("Сохранить в"), "{message}");
        let Err(Fail::Node { message, .. }) = ctx(Some("v@x.iam")).run(&flow(json!({ "spreadsheet_id": "a/b" }))).await else { panic!() };
        assert!(message.contains("id или ссылка"), "{message}");
    }

    #[test]
    fn sheet_names_are_always_quoted_in_a1() {
        assert_eq!(quoted("Sheet1"), "'Sheet1'");
        assert_eq!(quoted("A1"), "'A1'", "имя-ячейка — всё равно лист");
        assert_eq!(quoted("Мой лист"), "'Мой лист'");
        assert_eq!(quoted("O'Neil"), "'O''Neil'");
    }

    #[test]
    fn range_splits_at_the_bang_outside_a_quoted_sheet_name() {
        let a1 = |sheet: &str, range: &str| a1(sheet, range);
        assert_eq!(a1("X", "'Sales!2026'!A1:B2"), ("Sales!2026".to_string(), "A1:B2".to_string()));
        assert_eq!(a1("X", "'O''Neil'!B2"), ("O'Neil".to_string(), "B2".to_string()));
        assert_eq!(a1("X", "Plain!C3"), ("Plain".to_string(), "C3".to_string()));
        assert_eq!(a1("X", "A1:B2"), ("X".to_string(), "A1:B2".to_string()));
        assert_eq!(a1("X", "'Весь лист'"), ("Весь лист".to_string(), String::new()));
        assert_eq!(a1("", ""), ("Sheet1".to_string(), String::new()));
    }

    #[tokio::test]
    async fn a_cell_like_sheet_name_reads_that_sheet() {
        let seen = fake(200, json!({ "values": [] }), "").await;
        ctx(Some("cell@x.iam")).run(&flow(json!({ "sheet": "A1" }))).await.unwrap();
        assert_eq!(seen.lock().unwrap()[1].1, "/spreadsheets/SID123/values/'A1'");
        let seen = fake(200, json!({ "values": [] }), "").await;
        ctx(Some("cell@x.iam")).run(&flow(json!({ "range": "'Sales!2026'!A1:B2" }))).await.unwrap();
        assert_eq!(seen.lock().unwrap()[1].1, "/spreadsheets/SID123/values/'Sales!2026'!A1:B2");
    }

    #[tokio::test]
    async fn an_unknown_operation_is_an_error_and_clears_nothing() {
        let seen = fake(200, json!({}), "").await;
        for operation in [json!("erase"), json!(1), json!(true)] {
            let Err(Fail::Node { message, .. }) = ctx(Some("op@x.iam")).run(&flow(json!({ "operation": operation }))).await else { panic!() };
            assert!(message.contains("операция"), "{message}");
        }
        assert!(seen.lock().unwrap().is_empty(), "к таблице не обращался");
    }

    #[tokio::test]
    async fn an_expired_token_is_dropped_and_the_request_repeated_once() {
        let seen = fake(200, json!({ "values": [["a"], ["1"]] }), "").await;
        let mut c = ctx(Some("stale@x.iam"));
        google::tests::plant("stale@x.iam", SCOPE, "stale");
        c.run(&flow(json!({}))).await.unwrap();
        assert_eq!(c.scope.get("out"), Some(&json!([{ "a": "1" }])));
        {
            let seen = seen.lock().unwrap();
            let calls: Vec<(&str, &str)> = seen.iter().map(|(_, path, _, auth, _)| (path.as_str(), auth.as_str())).collect();
            assert_eq!(calls, [("/spreadsheets/SID123/values/'Sheet1'", "Bearer stale"), ("/token", ""), ("/spreadsheets/SID123/values/'Sheet1'", "Bearer at-sheet")]);
        }
        let mut c = ctx(Some("stale@x.iam"));
        google::tests::plant("stale@x.iam", SCOPE, "stale");
        c.run(&flow(json!({ "operation": "clear" }))).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!((seen.len(), seen[5].3.as_str()), (6, "Bearer at-sheet"), "очистка — тоже с новым токеном");
    }

    #[tokio::test]
    async fn a_read_answer_that_does_not_decode_is_an_error_not_an_empty_table() {
        fake(200, json!("$not-json"), "").await;
        let mut c = ctx(Some("body@x.iam"));
        let Err(Fail::Node { message, .. }) = c.run(&flow(json!({}))).await else { panic!() };
        assert!(message.starts_with("Google Sheet: ответ Google не прочитан"), "{message}");
        assert_eq!(c.scope.get("out"), None);
    }

    #[test]
    fn header_names_are_unique_and_never_empty() {
        let table = vec![vec![json!("a"), json!("a"), json!(""), json!("a"), json!("a_2")], vec![json!("1"), json!("2"), json!("3"), json!("4"), json!("5")]];
        assert_eq!(shape(table, true), json!([{ "a": "1", "a_2": "2", "column_3": "3", "a_3": "4", "a_2_2": "5" }]));
    }

    #[test]
    fn a_published_link_is_refused_with_a_hint() {
        let e = sheet_id("https://docs.google.com/spreadsheets/d/e/2PACX-1vQabc/pubhtml").unwrap_err();
        assert_eq!(e, "ссылка публикации не подходит — нужна ссылка на таблицу");
        assert_eq!(sheet_id("https://docs.google.com/spreadsheets/d/AbC/edit").unwrap(), "AbC");
    }
}
