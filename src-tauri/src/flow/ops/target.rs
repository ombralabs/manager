//! Цель операции: элемент по селектору (CSS, XPath, видимый текст; во фрейме из `frame`) или сохранённый в
//! переменной (спецификация автоматизации, «Каталог операций» и «Контекст страницы»). Тексты ошибок — с селектором
//! как записан в процессе.
use crate::engine::cdp::CdpError;
use crate::engine::page::{By, Doc, ElementRef};
use crate::flow::interp::Ctx;
use crate::flow::ops::web::tabs;
use crate::flow::ops::{OpError, int_value};
use crate::flow::vars;
use serde_json::Value;

/// Цель с подставленными переменными: Element appears разбирает её один раз, до опроса.
pub(crate) enum Target {
    /// Элемент из переменной.
    Stored(ElementRef),
    Search { by: By, selector: String, index: Index, frame: Option<ElementRef> },
}

/// Номер элемента среди найденных (с 1): заданный или случайный из интервала.
pub(crate) enum Index {
    Fixed(i64),
    Range(i64, i64),
}

/// Элемент цели; не найден — ошибка узла «нет элемента N для селектора S», сохранённый пропал — «элемент больше не
/// на странице».
pub(crate) async fn element(ctx: &mut Ctx, target: &Value) -> Result<ElementRef, OpError> {
    let resolved = resolve(ctx, target)?;
    match find(ctx, &resolved, target).await? {
        Some(element) => Ok(element),
        None if matches!(resolved, Target::Stored(_)) => Err("Элемент больше не доступен на странице".into()),
        None => {
            let index = target.get("index").map(vars::to_text).unwrap_or_else(|| "1".into());
            Err(OpError::Failed(format!("нет элемента {index} для селектора {}", target["selector"].as_str().unwrap_or_default())))
        }
    }
}

/// Подстановка переменных цели: ошибки поля (не задана, не элемент, элемент другой вкладки) — сразу.
pub(crate) fn resolve(ctx: &mut Ctx, target: &Value) -> Result<Target, OpError> {
    if let Some(stored) = target.get("element") {
        let value = ctx.scope.resolve(stored)?;
        return Ok(Target::Stored(tabs(ctx)?.element(&value)?));
    }
    let selector = ctx.scope.render(target["selector"].as_str().unwrap_or_default())?;
    let by = by(target)?;
    let index = match target.get("index").filter(|index| !index.is_null()) {
        None => Index::Fixed(1),
        Some(Value::Object(range)) => {
            // Random interval: в пределах найденного — интервал шире списка сужается до него.
            let min = int_value(&ctx.scope, range.get("min").unwrap_or(&Value::Null), "index")?.max(1);
            let max = int_value(&ctx.scope, range.get("max").unwrap_or(&Value::Null), "index")?;
            Index::Range(min, max)
        }
        Some(fixed) => Index::Fixed(int_value(&ctx.scope, fixed, "index")?),
    };
    if matches!(index, Index::Fixed(position) if position < 1) {
        return Err("цель: index — с 1".into());
    }
    let frame = match target.get("frame").filter(|frame| !frame.is_null()) {
        None => None,
        Some(frame) => Some(frame_owner(ctx, frame)?),
    };
    Ok(Target::Search { by, selector, index, frame })
}

/// Элемент цели или `None`, если его пока нет (Element appears); сохранённый — пока он на странице. `written` — цель
/// как записана в процессе (для текста ошибки селектора).
pub(crate) async fn find(ctx: &mut Ctx, target: &Target, written: &Value) -> Result<Option<ElementRef>, OpError> {
    let page = tabs(ctx)?.page().await?;
    let (by, selector, index, frame) = match target {
        Target::Stored(element) => return Ok(page.connected(element).await?.then(|| element.clone())),
        Target::Search { by, selector, index, frame } => (*by, selector, index, frame),
    };
    // Документ фрейма — заново при каждой попытке: межсайтовый фрейм может подключиться позже.
    let doc = match frame {
        None => page.main_doc(),
        Some(owner) => page.frame_doc(owner).await?,
    };
    let position = match *index {
        Index::Fixed(position) => position,
        Index::Range(min, max) => {
            let count = selector_error(page.count(&doc, by, selector).await, &written["selector"])? as i64;
            if count < min {
                return Ok(None);
            }
            vars::random_in(min, max.min(count))?
        }
    };
    selector_error(page.nth(&doc, by, selector, (position - 1) as usize).await, &written["selector"])
}

/// Все элементы цели по порядку документа — список For Loop Elements, снятый перед первой итерацией.
pub(crate) async fn all(ctx: &mut Ctx, target: &Value) -> Result<Vec<ElementRef>, OpError> {
    let selector = ctx.scope.render(target["selector"].as_str().unwrap_or_default())?;
    let by = by(target)?;
    let doc = doc(ctx, target).await?;
    selector_error(tabs(ctx)?.page().await?.all(&doc, by, &selector).await, &target["selector"])
}

/// Исключение поиска — неразобранный селектор. Текст страницы содержит его с подставленными значениями (правило
/// секретов), поэтому в ошибке — селектор как записан в процессе.
pub(crate) fn selector_error<T>(result: Result<T, CdpError>, selector: &Value) -> Result<T, OpError> {
    result.map_err(|e| match e {
        CdpError::Script(_) => OpError::Failed(format!("селектор {} не разобран", vars::to_text(selector))),
        other => other.into(),
    })
}

/// Документ цели: главный или фрейма из `frame`.
pub(crate) async fn doc(ctx: &mut Ctx, target: &Value) -> Result<Doc, OpError> {
    match target.get("frame").filter(|frame| !frame.is_null()) {
        None => Ok(tabs(ctx)?.page().await?.main_doc()),
        Some(frame) => frame_doc(ctx, frame).await,
    }
}

/// Документ фрейма по полю `frame` — `${f}` с элементом-фреймом (Element → iFrame).
pub(crate) async fn frame_doc(ctx: &mut Ctx, frame: &Value) -> Result<Doc, OpError> {
    let owner = frame_owner(ctx, frame)?;
    Ok(tabs(ctx)?.page().await?.frame_doc(&owner).await?)
}

fn frame_owner(ctx: &mut Ctx, frame: &Value) -> Result<ElementRef, OpError> {
    let value = ctx.scope.resolve(frame)?;
    tabs(ctx)?.element(&value)
}

fn by(target: &Value) -> Result<By, OpError> {
    match target["by"].as_str() {
        Some("css") => Ok(By::Css),
        Some("xpath") => Ok(By::XPath),
        Some("text") => Ok(By::Text),
        _ => Err("цель: by — css, xpath или text".into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::page::ElementRef;
    use crate::engine::page::tests::{Mock, base};
    use crate::flow::interp::Fail;
    use crate::flow::ops::web::tests::{chain, mocked};
    use serde_json::{Value, json};

    #[tokio::test]
    async fn invalid_selectors_are_named_as_written_not_as_substituted() {
        // Исключение браузера повторяет селектор с подставленным значением.
        let mock = Mock::spawn(|method, params, _| {
            base(method).unwrap_or_else(|| match method {
                "Page.createIsolatedWorld" => json!({ "executionContextId": 1 }),
                "DOM.resolveNode" => json!({ "object": { "objectId": "O1" } }),
                // Проверка `isConnected` сохранённого элемента — без аргументов.
                "Runtime.callFunctionOn" if params["arguments"] == json!([]) => json!({ "result": { "type": "boolean", "value": true } }),
                "Runtime.callFunctionOn" => {
                    let args: Vec<&str> = params["arguments"].as_array().into_iter().flatten().filter_map(|a| a["value"].as_str()).collect();
                    let description = format!("SyntaxError: '{}' is not a valid selector", args.join(" "));
                    json!({ "result": { "type": "object" }, "exceptionDetails": { "text": "Uncaught", "exception": { "description": description } } })
                }
                _ => json!({}),
            })
        })
        .await;
        let mut ctx = mocked(&mock).await;
        ctx.scope.set("pw", json!("секрет")).unwrap();
        let saved = {
            let tabs = ctx.tabs.as_mut().unwrap();
            tabs.page().await.unwrap();
            tabs.keep(ElementRef::test(5))
        };
        ctx.scope.set("el", saved).unwrap();
        let bad = json!({ "by": "css", "selector": "#${pw} >" });
        for (node, expected) in [
            (json!({ "op": "data.element", "config": { "target": bad, "save_to": "v" } }), "Element: селектор #${pw} > не разобран"),
            (
                json!({ "op": "data.element", "config": { "target": { "by": "css", "selector": "#${pw} >", "index": { "min": 1, "max": 3 } }, "save_to": "v" } }),
                "Element: селектор #${pw} > не разобран",
            ),
            (json!({ "op": "control.for_elements", "config": { "target": bad } }), "For Loop Elements: селектор #${pw} > не разобран"),
            (
                json!({ "op": "data.element", "config": { "target": { "element": "${el}" }, "kind": "child", "child_selector": "${pw} >", "save_to": "v" } }),
                "Element: селектор ${pw} > не разобран",
            ),
        ] {
            let mut node: Value = node;
            node["id"] = json!("n");
            let result = ctx.run(&chain(vec![node])).await;
            assert!(matches!(result, Err(Fail::Node { ref message, .. }) if message == expected && !message.contains("секрет")), "{result:?}");
        }
    }
}
