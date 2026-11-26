//! Получение данных со страницы: Element, Focused Element и значения For Loop Elements (спецификация автоматизации,
//! «Получение и обработка данных»).
use crate::app::totp;
use crate::engine::page::{ElementRef, Read};
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::profile;
use crate::flow::ops::web::tabs;
use crate::flow::ops::{Cfg, OpError, target};
use crate::flow::vars::to_text;
use serde_json::Value;

pub(crate) async fn exec(ctx: &mut Ctx, id: &str, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let value = match id {
        "data.element" => {
            needed(cfg)?;
            let element = target::element(ctx, &cfg.raw("target")).await?;
            extract(ctx, element, &cfg.string("kind"), cfg).await?
        }
        "data.focused" => match tabs(ctx)?.page().await?.focused().await? {
            Some(element) => tabs(ctx)?.keep(element),
            None => Value::Null,
        },
        "data.url" => {
            let url = tabs(ctx)?.page().await?.url().await?;
            Value::String(match cfg.string("kind").as_str() {
                "domain" => domain(&url),
                "param" => {
                    if cfg.raw("param").is_null() {
                        return Err({
                            log::warn!("поле param не заполнено");
                            "Заполните поле «Параметр»"
                        }.into());
                    }
                    query_param(&url, &cfg.text(&ctx.scope, "param")?)
                }
                _ => url,
            })
        }
        "data.clipboard" => Value::String(clipboard().await?),
        "data.totp" => Value::String(totp(ctx, cfg)?),
        other => return Err(OpError::Failed(format!("операция {other} не исполняется"))),
    };
    Ok((Step::Port("next"), ctx.save(cfg, value)?))
}

/// Поля, без которых вид извлечения не работает.
fn needed(cfg: &Cfg<'_>) -> Result<(), OpError> {
    match cfg.string("kind").as_str() {
        "attribute" if cfg.raw("attribute").is_null() => Err({
            log::warn!("поле attribute не заполнено");
            "Заполните поле «Атрибут»"
        }.into()),
        "child" if cfg.raw("child_selector").is_null() => Err({
            log::warn!("поле child_selector не заполнено");
            "Заполните поле «Селектор дочернего элемента»"
        }.into()),
        _ => Ok(()),
    }
}

/// Значение элемента по виду: текст, HTML, атрибут — строкой; объект, фрейм, потомок — ссылкой на элемент.
pub(crate) async fn extract(ctx: &mut Ctx, element: ElementRef, kind: &str, cfg: &Cfg<'_>) -> Result<Value, OpError> {
    let attribute = if kind == "attribute" { cfg.text(&ctx.scope, "attribute")? } else { String::new() };
    let child = if kind == "child" { cfg.text(&ctx.scope, "child_selector")? } else { String::new() };
    let tabs = tabs(ctx)?;
    Ok(match kind {
        "text" => tabs.page().await?.read(&element, Read::Text).await?,
        "html" => tabs.page().await?.read(&element, Read::Html).await?,
        "attribute" => tabs.page().await?.read(&element, Read::Attribute(&attribute)).await?,
        "iframe" => {
            let tag = tabs.page().await?.read(&element, Read::Tag).await?;
            if !matches!(tag.as_str(), Some("IFRAME" | "FRAME")) {
                return Err("элемент — не фрейм".into());
            }
            tabs.keep(element)
        }
        "child" => match target::selector_error(tabs.page().await?.child(&element, &child).await, &cfg.raw("child_selector"))? {
            Some(found) => tabs.keep(found),
            None => return Err(OpError::Failed(format!("нет потомка {}", to_text(&cfg.raw("child_selector"))))),
        },
        _ => tabs.keep(element),
    })
}

/// Значения For Loop Elements: элементы сняты перед первой итерацией, значение каждого — по `kind`.
pub(crate) async fn loop_elements(ctx: &mut Ctx, cfg: &Cfg<'_>) -> Result<Vec<Value>, OpError> {
    needed(cfg)?;
    let kind = cfg.string("kind");
    let elements = target::all(ctx, &cfg.raw("target")).await?;
    let mut values = Vec::with_capacity(elements.len());
    for element in elements {
        values.push(extract(ctx, element, &kind, cfg).await?);
    }
    Ok(values)
}

/// Схема и хост адреса (`https://www.example.com`), с портом, если он не стандартный; у адреса без хоста — пусто.
fn domain(url: &str) -> String {
    reqwest::Url::parse(url).ok().map(|url| url.origin()).filter(|origin| origin.is_tuple()).map(|origin| origin.ascii_serialization()).unwrap_or_default()
}

/// Значение параметра запроса или пустая строка.
pub(crate) fn query_param(url: &str, name: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned()))
        .unwrap_or_default()
}

/// Сколько ждать `pbpaste`.
const CLIPBOARD_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Текст системного буфера macOS. У приложения из Finder или launchd нет `LANG`: без него `pbpaste` отдаёт текст не в
/// UTF-8, поэтому кодировка задаётся явно.
async fn clipboard() -> Result<String, OpError> {
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("pbpaste");
    #[cfg(windows)]
    let mut command = {
        let mut cmd = tokio::process::Command::new("powershell.exe");
        cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(); [Console]::Write((Get-Clipboard -Raw))"]);
        cmd.creation_flags(0x08000000);
        cmd
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let mut cmd = tokio::process::Command::new(if wayland { "wl-paste" } else { "xclip" });
        if wayland { cmd.args(["--no-newline", "--type", "text"]); }
        else { cmd.args(["-selection", "clipboard", "-o"]); }
        cmd
    };
    command.env("LC_ALL", "en_US.UTF-8").kill_on_drop(true);
    let output = tokio::time::timeout(CLIPBOARD_WAIT, command.output())
        .await
        .map_err(|_| "буфер обмена не прочитан")?
        .map_err(|e| format!("буфер обмена не прочитан: {e}"))?;
    if !output.status.success() {
        return Err("буфер обмена не прочитан".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Код 2FA: из поля `secret`, а пустое поле — ключ основного профиля.
fn totp(ctx: &Ctx, cfg: &Cfg<'_>) -> Result<String, OpError> {
    let secret = match cfg.raw("secret") {
        Value::Null => String::new(),
        _ => cfg.text(&ctx.scope, "secret")?,
    };
    let secret = if secret.trim().is_empty() {
        profile::host(ctx)?.totp_secret()?.ok_or_else(|| OpError::Failed("у профиля нет ключа 2FA".into()))?
    } else {
        secret
    };
    let key = totp::decode_base32(&secret).ok_or_else(|| OpError::Failed("ключ 2FA не в Base32".into()))?;
    Ok(totp::code_at(&key, crate::sys::unix_now()).0)
}

#[cfg(test)]
mod tests {
    use crate::flow::interp::{Ctx, Fail};
    use crate::flow::model::Flow;
    use serde_json::json;

    fn one(op: &str, config: serde_json::Value) -> Flow {
        serde_json::from_value(json!({
            "format": "ombra-flow/1", "name": "t",
            "nodes": [{ "id": "s", "op": "start" }, { "id": "x", "op": op, "config": config }],
            "edges": [{ "from": "s", "to": "x" }]
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn element_operations_need_a_browser() {
        for (op, config, name) in [
            ("data.element", json!({ "target": { "by": "css", "selector": "a" }, "save_to": "v" }), "Element"),
            ("data.focused", json!({ "save_to": "v" }), "Focused Element"),
            ("wait.element", json!({ "target": { "by": "css", "selector": "a" } }), "Element appears"),
            ("control.for_elements", json!({ "target": { "by": "css", "selector": "li" } }), "For Loop Elements"),
        ] {
            let result = Ctx::bare().run(&one(op, config)).await;
            let expected = format!("{name}: у запуска нет браузера");
            assert!(matches!(result, Err(Fail::Node { ref message, .. }) if *message == expected), "{op}: {result:?}");
        }
    }

    #[tokio::test]
    async fn collecting_loop_elements_stops_on_cancel() {
        use crate::engine::page::tests::{Mock, base};
        // Браузер не отвечает на поиск: без гонки с отменой сбор ждал бы тайм-аута вызова.
        let mock = Mock::spawn(|method, _, _| base(method).unwrap_or_else(|| json!({ "silent": true }))).await;
        let mut ctx = crate::flow::ops::web::tests::mocked(&mock).await;
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            cancel.cancel();
        });
        let started = std::time::Instant::now();
        let result = ctx.run(&one("control.for_elements", json!({ "target": { "by": "css", "selector": "li" } }))).await;
        assert_eq!(result, Err(Fail::Cancelled));
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "«Стоп» не ждал ответа браузера");
    }

    #[test]
    fn attribute_and_child_need_their_fields() {
        let spec = crate::flow::ops::spec("data.element").unwrap();
        let check = |config: serde_json::Value| super::needed(&crate::flow::ops::Cfg::new(spec, config.as_object().unwrap()));
        let failed = |text: &str| Err(crate::flow::ops::OpError::Failed(text.into()));
        assert_eq!(check(json!({ "target": { "by": "css", "selector": "a" }, "kind": "attribute", "save_to": "v" })), failed("Заполните поле «Атрибут»"));
        assert_eq!(check(json!({ "target": { "by": "css", "selector": "a" }, "kind": "child", "save_to": "v" })), failed("Заполните поле «Селектор дочернего элемента»"));
        assert_eq!(check(json!({ "target": { "by": "css", "selector": "a" }, "save_to": "v" })), Ok(()));
    }

    #[test]
    fn url_parts() {
        assert_eq!(super::domain("https://www.example.com/a?b=1"), "https://www.example.com");
        assert_eq!(super::domain("http://flow.ombra.test:8080/x"), "http://flow.ombra.test:8080");
        assert_eq!(super::domain("about:blank"), "");
        assert_eq!(super::query_param("https://a.test/?k=7&x=", "k"), "7");
        assert_eq!(super::query_param("https://a.test/?q=%D1%84", "q"), "ф");
        assert_eq!(super::query_param("https://a.test/", "k"), "");
    }

    #[tokio::test]
    async fn totp_from_the_field_or_the_profile_key() {
        use crate::app::totp::{code_at, decode_base32};
        use crate::flow::host::tests::FakeHost;
        let key = decode_base32("JBSWY3DPEHPK3PXP").unwrap();
        let before = code_at(&key, crate::sys::unix_now()).0;
        let mut ctx = Ctx::bare();
        ctx.host = Some(std::sync::Arc::new(FakeHost { secret: Some("JBSWY3DPEHPK3PXP".into()), ..Default::default() }));
        ctx.run(&one("data.totp", json!({ "save_to": "c" }))).await.unwrap();
        let after = code_at(&key, crate::sys::unix_now()).0;
        let code = ctx.scope.get("c").unwrap().as_str().unwrap().to_string();
        assert!(code == before || code == after, "{code}");
        ctx.run(&one("data.totp", json!({ "secret": "GEZDGNBV", "save_to": "d" }))).await.unwrap();
        assert_eq!(ctx.scope.get("d").unwrap().as_str().unwrap().len(), 6);
        let bad = ctx.run(&one("data.totp", json!({ "secret": "1!", "save_to": "d" }))).await;
        assert!(matches!(bad, Err(Fail::Node { ref message, .. }) if message == "Verification Code: ключ 2FA не в Base32"), "{bad:?}");
        let typed = ctx.run(&one("data.totp", json!({ "secret": "Ab${12", "save_to": "d" }))).await;
        assert!(matches!(typed, Err(Fail::Node { ref message, .. }) if message == "Verification Code: поле «secret»: неверная ссылка на переменную (буквальное «${» — «$${»)"), "{typed:?}");
        let mut none = Ctx::bare();
        none.host = Some(std::sync::Arc::new(FakeHost::default()));
        let missing = none.run(&one("data.totp", json!({ "save_to": "c" }))).await;
        assert!(matches!(missing, Err(Fail::Node { ref message, .. }) if message == "Verification Code: у профиля нет ключа 2FA"), "{missing:?}");
    }

    #[tokio::test]
    async fn url_kinds_of_the_current_tab() {
        use crate::engine::page::Browser;
        use crate::engine::page::tests::{Mock, base};
        use crate::flow::ops::web::Tabs;
        let mock = Mock::spawn(|method, _, _| {
            base(method).unwrap_or_else(|| match method {
                "Target.getTargetInfo" => json!({ "targetInfo": { "url": "https://www.a.test:8443/p?k=7" } }),
                _ => json!({}),
            })
        })
        .await;
        let mut ctx = Ctx::bare();
        ctx.tabs = Some(Tabs::new(Browser::connect(&mock.url).await.unwrap(), Some("T1".into())));
        ctx.run(&one("data.url", json!({ "save_to": "full" }))).await.unwrap();
        ctx.run(&one("data.url", json!({ "kind": "domain", "save_to": "domain" }))).await.unwrap();
        ctx.run(&one("data.url", json!({ "kind": "param", "param": "k", "save_to": "k" }))).await.unwrap();
        assert_eq!(ctx.scope.get("full"), Some(&json!("https://www.a.test:8443/p?k=7")));
        assert_eq!(ctx.scope.get("domain"), Some(&json!("https://www.a.test:8443")));
        assert_eq!(ctx.scope.get("k"), Some(&json!("7")));
        let missing = ctx.run(&one("data.url", json!({ "kind": "param", "save_to": "k" }))).await;
        assert!(matches!(missing, Err(Fail::Node { ref message, .. }) if message == "URL: Заполните поле «Параметр»"), "{missing:?}");
    }

    /// Трогает системный буфер macOS (возвращает прежнее содержимое). Буфер тест пишет и сохраняет в UTF-8 сам; как в
    /// приложении из Finder (без `LANG`) — `env -u LANG -u LC_CTYPE -u LC_ALL cargo test --lib clipboard -- --ignored`.
    #[tokio::test]
    #[ignore = "трогает системный буфер обмена; cargo test --lib clipboard -- --ignored"]
    async fn clipboard_reads_the_system_clipboard() {
        let saved = std::process::Command::new("pbpaste").env("LC_ALL", "en_US.UTF-8").output().unwrap().stdout;
        let copy = |bytes: &[u8]| {
            use std::io::Write;
            let mut child =
                std::process::Command::new("pbcopy").env("LC_ALL", "en_US.UTF-8").stdin(std::process::Stdio::piped()).spawn().unwrap();
            child.stdin.take().unwrap().write_all(bytes).unwrap();
            child.wait().unwrap();
        };
        copy("буфер процесса".as_bytes());
        let mut ctx = Ctx::bare();
        let result = ctx.run(&one("data.clipboard", json!({ "save_to": "c" }))).await;
        copy(&saved);
        result.unwrap();
        assert_eq!(ctx.scope.get("c"), Some(&json!("буфер процесса")));
    }
}
