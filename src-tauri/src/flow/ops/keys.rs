//! Клавиатура: Keys и Key combination (спецификация автоматизации, «Ввод»).
use crate::engine::page::is_key;
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::web::{NAV_TIMEOUT_MS, navigation, tabs, wait};
use crate::flow::ops::{Cfg, OpError};
use crate::flow::vars::to_text;
use serde_json::Value;

pub(crate) async fn exec(ctx: &mut Ctx, id: &str, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    match id {
        "key.press" => {
            let key = cfg.string("key");
            tabs(ctx)?.page().await?.press(&key, &[]).await?;
        }
        "key.combo" => {
            let key = cfg.text(&ctx.scope, "key")?;
            if !is_key(&key) {
                return Err(OpError::Failed(format!("неизвестная клавиша {}", to_text(&cfg.raw("key")))));
            }
            let modifiers: Vec<String> =
                cfg.raw("modifiers").as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let modifiers: Vec<&str> = modifiers.iter().map(String::as_str).collect();
            let page = tabs(ctx)?.page().await?;
            // ⌘R и Ctrl+R — горячие клавиши браузера, по CDP не срабатывают: перезагрузка вкладки.
            if key.eq_ignore_ascii_case("r") && modifiers.iter().any(|m| *m == "cmd" || *m == "ctrl") {
                navigation(page.reload(wait(NAV_TIMEOUT_MS)?).await, NAV_TIMEOUT_MS)?;
            } else {
                page.press(&key, &modifiers).await?;
            }
        }
        other => return Err(OpError::Failed(format!("операция {other} не исполняется"))),
    }
    Ok((Step::Port("next"), String::new()))
}

#[cfg(test)]
mod tests {
    use crate::flow::interp::{Ctx, Fail};
    use crate::flow::model::Flow;
    use serde_json::json;

    fn one(op: &str, config: serde_json::Value) -> Flow {
        serde_json::from_value(json!({
            "format": "ombra-flow/1", "name": "t",
            "nodes": [{ "id": "s", "op": "start" }, { "id": "k", "op": op, "config": config }],
            "edges": [{ "from": "s", "to": "k" }]
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn unknown_keys_fail_before_the_browser() {
        let result = Ctx::bare().run(&one("key.combo", json!({ "modifiers": ["cmd"], "key": "ж" }))).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "Key combination: неизвестная клавиша ж"), "{result:?}");
        let result = Ctx::bare().run(&one("key.press", json!({ "key": "Enter" }))).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "Keys: у запуска нет браузера"), "{result:?}");
    }
}
