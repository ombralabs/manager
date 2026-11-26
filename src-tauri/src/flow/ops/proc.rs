//! Обработка данных: Extract from txt, Convert to JSON, Extract Field, Random Extraction. Manage Variables исполняет
//! интерпретатор: режим `export` завершает процесс.
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::{Cfg, OpError};
use crate::flow::vars::{random_index, to_text};
use serde_json::Value;

pub(crate) fn exec(ctx: &mut Ctx, id: &str, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let value = match id {
        "proc.regex" => extract(ctx, cfg)?,
        "proc.to_json" => match ctx.scope.resolve(&cfg.raw("data"))? {
            Value::String(text) => serde_json::from_str(&text).map_err(|e| format!("не JSON ({e})"))?,
            other => other,
        },
        "proc.field" => field(ctx, cfg)?,
        "proc.random" => random(ctx.scope.resolve(&cfg.raw("data"))?)?,
        other => return Err(OpError::Failed(format!("операция {other} не исполняется"))),
    };
    Ok((Step::Port("next"), ctx.save(cfg, value)?))
}

fn extract(ctx: &Ctx, cfg: &Cfg<'_>) -> Result<Value, String> {
    let data = cfg.text(&ctx.scope, "data")?;
    let pattern = cfg.text(&ctx.scope, "pattern")?;
    let regex = regex::RegexBuilder::new(&pattern)
        .case_insensitive(cfg.flag("ignore_case"))
        .build()
        .map_err(|e| rule_error(cfg, "pattern", &e))?;
    Ok(if cfg.flag("first_only") {
        Value::String(regex.find(&data).map(|found| found.as_str().to_string()).unwrap_or_default())
    } else {
        Value::Array(regex.find_iter(&data).map(|found| Value::String(found.as_str().to_string())).collect())
    })
}

/// Подробность regex печатает шаблон, а подставленный шаблон — значение переменной: тогда только общий текст
/// (правило секретов). Шаблон как записан в поле `field` без `${…}` — подробность можно.
pub(crate) fn rule_error(cfg: &Cfg<'_>, field: &str, e: &regex::Error) -> String {
    if !to_text(&cfg.raw(field)).contains("${") {
        return format!("неверное правило: {e}");
    }
    match e {
        regex::Error::CompiledTooBig(_) => "слишком сложное правило".into(),
        _ => "неверное правило".into(),
    }
}

/// Ключ объекта или индекс массива с 0; в тексте ошибки — ключ как записан в процессе.
fn field(ctx: &Ctx, cfg: &Cfg<'_>) -> Result<Value, String> {
    let key = cfg.text(&ctx.scope, "key")?;
    let written = to_text(&cfg.raw("key"));
    match ctx.scope.resolve(&cfg.raw("data"))? {
        Value::Object(map) => map.get(&key).cloned().ok_or_else(|| format!("нет ключа {written}")),
        Value::Array(list) => key
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|i| list.get(i).cloned())
            .ok_or_else(|| format!("нет элемента {written}")),
        _ => Err("Сначала преобразуйте данные в JSON с помощью блока «Преобразовать в JSON».".into()),
    }
}

fn random(data: Value) -> Result<Value, String> {
    match data {
        Value::Array(mut list) if !list.is_empty() => Ok(list.swap_remove(random_index(list.len()))),
        Value::Object(map) if !map.is_empty() => Ok(map.values().nth(random_index(map.len())).cloned().unwrap_or_default()),
        Value::Array(_) | Value::Object(_) => Err("данные пусты".into()),
        _ => Err("данные не массив и не объект".into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::flow::interp::{Ctx, Exit, Fail};
    use crate::flow::journal::Journal;
    use crate::flow::model::{ErrorPolicy, Flow};
    use crate::flow::vars::{self, Scope};
    use serde_json::{Map, Value, json};
    use tokio_util::sync::CancellationToken;

    fn one(op: &str, config: Value, vars: Value) -> Flow {
        serde_json::from_value(json!({
            "format": "ombra-flow/1", "name": "t", "variables": vars,
            "nodes": [{ "id": "s", "op": "start" }, { "id": "x", "op": op, "config": config }],
            "edges": [{ "from": "s", "to": "x" }]
        }))
        .unwrap()
    }

    async fn run(flow: &Flow) -> (Result<Exit, Fail>, Ctx) {
        let mut scope = Scope::new(Default::default());
        vars::declare(&mut scope, &flow.variables, Map::new()).unwrap();
        let mut ctx = Ctx::new(scope, ErrorPolicy::Interrupt, Box::new(|_| None), Journal::memory(), CancellationToken::new());
        let result = ctx.run(flow).await;
        (result, ctx)
    }

    /// Значение `out` после узла или текст ошибки узла.
    async fn saved(op: &str, config: Value, vars: Value) -> Result<Value, String> {
        match run(&one(op, config, vars)).await {
            (Ok(_), ctx) => Ok(ctx.scope.get("out").cloned().unwrap_or(Value::Null)),
            (Err(Fail::Node { message, .. }), _) => Err(message),
            (other, _) => panic!("{other:?}"),
        }
    }

    fn text(name: &str, value: &str) -> Value {
        json!([{ "name": name, "type": "string", "default": value }])
    }

    #[tokio::test]
    async fn extract_from_txt_finds_all_or_first() {
        let v = text("t", "код 123456, ещё 42");
        assert_eq!(saved("proc.regex", json!({ "data": "${t}", "pattern": "\\d+", "save_to": "out" }), v.clone()).await, Ok(json!(["123456", "42"])));
        assert_eq!(
            saved("proc.regex", json!({ "data": "${t}", "pattern": "\\b\\d{6}\\b", "first_only": true, "save_to": "out" }), v.clone()).await,
            Ok(json!("123456"))
        );
        assert_eq!(saved("proc.regex", json!({ "data": "${t}", "pattern": "x", "first_only": true, "save_to": "out" }), v).await, Ok(json!("")));
        assert_eq!(
            saved("proc.regex", json!({ "data": "ABC abc", "pattern": "abc", "ignore_case": true, "save_to": "out" }), json!([])).await,
            Ok(json!(["ABC", "abc"]))
        );
        let error = saved("proc.regex", json!({ "data": "x", "pattern": "(", "save_to": "out" }), json!([])).await.unwrap_err();
        assert!(error.starts_with("Extract from txt: неверное правило"), "{error}");
    }

    #[tokio::test]
    async fn substituted_bad_rule_does_not_leak_value() {
        let error = saved("proc.regex", json!({ "data": "x", "pattern": "${t}", "save_to": "out" }), text("t", "(sec")).await.unwrap_err();
        assert!(error.starts_with("Extract from txt: неверное правило"), "{error}");
        assert!(!error.contains("sec"), "{error}");
    }

    #[tokio::test]
    async fn convert_to_json_parses_text() {
        assert_eq!(saved("proc.to_json", json!({ "data": "${t}", "save_to": "out" }), text("t", r#"{"a":[1,2]}"#)).await, Ok(json!({ "a": [1, 2] })));
        let list = json!([{ "name": "l", "type": "array", "default": [1] }]);
        assert_eq!(saved("proc.to_json", json!({ "data": "${l}", "save_to": "out" }), list).await, Ok(json!([1])), "уже JSON — как есть");
        let error = saved("proc.to_json", json!({ "data": "{oops", "save_to": "out" }), json!([])).await.unwrap_err();
        assert!(error.starts_with("Convert to JSON: не JSON"), "{error}");
    }

    #[tokio::test]
    async fn extract_field_reads_keys_and_indexes() {
        let v = json!([
            { "name": "o", "type": "object", "default": { "name": "Ann", "tags": ["a", "b"] } },
            { "name": "l", "type": "array", "default": ["x", "y"] }
        ]);
        assert_eq!(saved("proc.field", json!({ "data": "${o}", "key": "name", "save_to": "out" }), v.clone()).await, Ok(json!("Ann")));
        assert_eq!(saved("proc.field", json!({ "data": "${l}", "key": 1, "save_to": "out" }), v.clone()).await, Ok(json!("y")));
        assert_eq!(saved("proc.field", json!({ "data": "${o}", "key": "age", "save_to": "out" }), v).await, Err("Extract Field: нет ключа age".into()));
        let error = saved("proc.field", json!({ "data": "plain", "key": "a", "save_to": "out" }), json!([])).await.unwrap_err();
        assert!(error.contains("Сначала преобразуйте данные в JSON"), "{error}");
    }

    #[tokio::test]
    async fn random_extraction_picks_an_element() {
        let v = json!([
            { "name": "l", "type": "array", "default": ["a", "b", "c"] },
            { "name": "o", "type": "object", "default": { "k": "v" } },
            { "name": "e", "type": "array", "default": [] }
        ]);
        let picked = saved("proc.random", json!({ "data": "${l}", "save_to": "out" }), v.clone()).await.unwrap();
        assert!(["a", "b", "c"].contains(&picked.as_str().unwrap()));
        assert_eq!(saved("proc.random", json!({ "data": "${o}", "save_to": "out" }), v.clone()).await, Ok(json!("v")));
        assert_eq!(saved("proc.random", json!({ "data": "${e}", "save_to": "out" }), v).await, Err("Random Extraction: данные пусты".into()));
    }

    #[tokio::test]
    async fn results_reach_the_journal_as_shape_only() {
        let (_, ctx) = run(&one("proc.regex", json!({ "data": "${t}", "pattern": "\\w+", "save_to": "out" }), text("t", "тайна секрет"))).await;
        let entry = ctx.journal.entries().iter().find(|e| e.node == "x").unwrap();
        assert!(entry.msg.ends_with("→ out (array, 2)"), "{}", entry.msg);
        assert!(!entry.msg.contains("тайна"), "{}", entry.msg);
    }
}
