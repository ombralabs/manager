//! Ожидание элемента: Element appears (спецификация автоматизации, «Ожидания»): по тайм-ауту — дальше с `false`.
use crate::engine::page::Read;
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::target::{self, Target};
use crate::flow::ops::web::{tabs, wait};
use crate::flow::ops::{Cfg, OpError};
use serde_json::Value;
use std::time::Duration;

/// Как часто проверять, появился ли элемент.
const POLL: Duration = Duration::from_millis(100);

pub(crate) async fn element(ctx: &mut Ctx, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let visible = cfg.flag("visible");
    let deadline = tokio::time::Instant::now() + wait(cfg.int(&ctx.scope, "timeout_ms")?)?;
    // Нет браузера или вкладки, ошибка в переменных цели — сразу, а не после тайм-аута.
    tabs(ctx)?.page().await?;
    let written = cfg.raw("target");
    let resolved = target::resolve(ctx, &written)?;
    // Ошибки опроса не фатальны: сразу после перехода страницы они значат «ещё нет» (контекст уничтожен, фрейм не готов).
    // Если же все попытки провалились (например, неразобранный селектор), отдаём последнюю ошибку, а не `false`.
    // Новую попытку после срока не начинаем; начатая до срока может его пережить (вызов CDP, сессия межсайтового фрейма).
    let mut last_error = None;
    let mut clean = false;
    let found = loop {
        match attempt(ctx, &resolved, &written, visible).await {
            Ok(true) => break true,
            Ok(false) => clean = true,
            Err(e @ (OpError::BrowserClosed | OpError::Cancelled)) => return Err(e),
            Err(e) => last_error = Some(e),
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            match last_error {
                Some(e) if !clean => return Err(e),
                _ => break false,
            }
        }
        tokio::time::sleep(POLL.min(deadline - now)).await;
    };
    let note = ctx.save(cfg, Value::Bool(found))?;
    let note = match (found, note.is_empty()) {
        (true, _) => note,
        (false, true) => "Истекло время ожидания".into(),
        (false, false) => format!("Истекло время ожидания: {note}"),
    };
    Ok((Step::Port("next"), note))
}

/// Одна попытка: элемент найден (и виден, если нужно).
async fn attempt(ctx: &mut Ctx, resolved: &Target, written: &Value, visible: bool) -> Result<bool, OpError> {
    let Some(element) = target::find(ctx, resolved, written).await? else { return Ok(false) };
    Ok(!visible || tabs(ctx)?.page().await?.read(&element, Read::Visible).await? == Value::Bool(true))
}

#[cfg(test)]
mod tests {
    use crate::engine::page::ElementRef;
    use crate::engine::page::tests::{Mock, base};
    use crate::flow::interp::{Exit, Fail};
    use crate::flow::ops::web::tests::{chain, mocked};
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn no_attempt_starts_after_the_deadline() {
        // Элемент появляется через 75 мс после первой попытки; ждём 30 мс.
        let first: Arc<Mutex<Option<Instant>>> = Arc::default();
        let mock = Mock::spawn(move |method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "Runtime.callFunctionOn" => {
                    let started = *first.lock().unwrap().get_or_insert_with(Instant::now);
                    if started.elapsed() >= Duration::from_millis(75) {
                        json!({ "result": { "type": "object", "objectId": "O1" } })
                    } else {
                        json!({ "result": { "type": "object", "subtype": "null", "value": null } })
                    }
                }
                "DOM.describeNode" => json!({ "node": { "backendNodeId": 5 } }),
                _ => json!({}),
            })
        })
        .await;
        let mut ctx = mocked(&mock).await;
        let flow = chain(vec![json!({ "id": "w", "op": "wait.element", "config": { "target": { "by": "css", "selector": "#late" }, "timeout_ms": 30, "save_to": "found" } })]);
        assert_eq!(ctx.run(&flow).await, Ok(Exit::Done));
        assert_eq!(ctx.scope.get("found"), Some(&json!(false)), "появившийся после срока не найден");
        assert_eq!(mock.sent("Runtime.callFunctionOn").len(), 2, "в начале и ровно на сроке");
    }

    #[tokio::test]
    async fn target_variable_errors_surface_at_once() {
        let mock = Mock::spawn(|method, _, _| base(method).unwrap_or_else(|| json!({}))).await;
        let mut ctx = mocked(&mock).await;
        ctx.scope.set("text", json!("строка")).unwrap();
        for (target, expected) in [
            (json!({ "by": "css", "selector": "#${nope}" }), "Element appears: переменная nope не задана"),
            (json!({ "element": "${text}" }), "Element appears: значение не элемент"),
        ] {
            let flow = chain(vec![json!({ "id": "w", "op": "wait.element", "config": { "target": target, "timeout_ms": 5000 } })]);
            let started = Instant::now();
            let result = ctx.run(&flow).await;
            assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == expected), "{result:?}");
            assert!(started.elapsed() < Duration::from_secs(1), "не после тайм-аута");
        }
    }

    #[tokio::test]
    async fn a_removed_saved_element_is_neither_awaited_nor_used() {
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "DOM.resolveNode" => json!({ "object": { "objectId": "O1" } }),
                // `isConnected` — узел убран из DOM.
                "Runtime.callFunctionOn" => json!({ "result": { "type": "boolean", "value": false } }),
                _ => json!({}),
            })
        })
        .await;
        let mut ctx = mocked(&mock).await;
        let saved = {
            let tabs = ctx.tabs.as_mut().unwrap();
            tabs.page().await.unwrap();
            tabs.keep(ElementRef::test(5))
        };
        ctx.scope.set("el", saved).unwrap();
        let flow = chain(vec![
            json!({ "id": "w", "op": "wait.element", "config": { "target": { "element": "${el}" }, "timeout_ms": 0, "save_to": "there" } }),
            json!({ "id": "d", "op": "data.element", "config": { "target": { "element": "${el}" }, "save_to": "v" } }),
        ]);
        let result = ctx.run(&flow).await;
        assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == "Element: Элемент больше не доступен на странице"), "{result:?}");
        assert_eq!(ctx.scope.get("there"), Some(&json!(false)));
    }
}
