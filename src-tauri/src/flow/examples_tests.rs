//! Проверка готовых импортируемых примеров. Внешний сайт — отдельная, явно запускаемая проверка.
use super::model::{Flow, parse};
use super::validate::validate;

fn cian() -> Flow {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../tools/flows/examples/cian.json")).unwrap();
    parse(&source).unwrap()
}

#[test]
fn cian_example_is_valid() {
    let flow = cian();
    assert_eq!(flow.settings.after_browser, super::model::AfterBrowser::Quit, "пример не оставляет браузеры после задачи");
    let report = validate(&flow, &|_| None);
    assert!(report.errors.is_empty(), "{}", report.summary());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "реальный Циан и установленный движок: cargo test --lib cian_real_site -- --ignored --nocapture"]
async fn cian_real_site() {
    use crate::app::Core;
    use crate::engine::Engine;
    use crate::engine::ombra::OmbraEngine;
    use crate::fingerprint::{Arch, Catalog, OsRandom, generate};
    use crate::flow::run::{RunRequest, RunStatus, run_once};
    use crate::paths::Paths;
    use crate::relay::geo::GeoClient;
    use crate::relay::test_support::spawn_http_proxy;
    use crate::store::profiles::ProfileInput;
    use crate::store::proxies::{ProxyInput, ProxyKind};
    use crate::store::secrets::SecretField;
    use crate::store::tests::memory_store;
    use crate::supervisor::Timings;
    use crate::supervisor::live_tests::KillProfileOnPanic;
    use serde_json::{Map, json};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    // База, профиль и прокси изолированы от данных пользователя. HTTP CONNECT передаёт HTTPS сайту без расшифровки.
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(memory_store());
    let engine = Arc::new(OmbraEngine::new(Paths::new(Paths::default_root()).engines()));
    engine.installed().expect("для проверки нужен установленный движок Ombra");
    let (proxy, traffic) = spawn_http_proxy(None).await;
    let proxy_id = store.create_proxy(&ProxyInput {
        kind: ProxyKind::Http, host: "127.0.0.1".into(), port: proxy.port(), username: None,
        password: SecretField::Keep, note: "Проверка Циан".into(), enabled: true,
    }).unwrap().id;
    let fingerprint = generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap();
    let mut input = ProfileInput::new("Циан — реальная проверка", fingerprint.to_json());
    input.proxy_id = Some(proxy_id);
    let profile = store.create_profile(&input, 1).unwrap();
    let core = Core::new(Paths::new(root.path()), store, engine, Arc::new(GeoClient::new()), Arc::new(|_| {}), Timings::default());
    let _cleanup = KillProfileOnPanic(core.paths.profile_dir(&profile.id));
    let mut flow = cian();
    flow.settings.headless = true;
    let outcome = run_once(&core, RunRequest {
        flow: &flow, profile_id: &profile.id, params: Map::new(), run_id: "cian-real".into(),
        cancel: CancellationToken::new(), position: 0, task: None, busy: None, on_entry: None, from: None, on_node: None,
    }).await;
    assert!(core.supervisor.running_info(&profile.id).is_none(), "пример закрыл браузер после задачи");
    assert!(crate::engine::process::find_profile_processes(&core.paths.profile_dir(&profile.id)).await.unwrap().is_empty(), "после задачи нет процессов браузера");
    core.supervisor.stop_all().await;

    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/acceptance/cian/native");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join("result.json"), serde_json::to_string_pretty(&outcome).unwrap()).unwrap();
    std::fs::copy(core.paths.flow_log("cian-real"), output.join("journal.jsonl")).unwrap();
    if let Ok(files) = std::fs::read_dir(core.paths.flow_files("cian-real")) {
        for entry in files.flatten() {
            if entry.path().is_file() { std::fs::copy(entry.path(), output.join(entry.file_name())).unwrap(); }
        }
    }
    assert_eq!(outcome.status, RunStatus::Done, "{}; журнал: {}", outcome.error.unwrap_or_default(), output.display());
    let rows = outcome.exported["offers"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for row in rows {
        assert!(row["url"].as_str().unwrap().starts_with("https://www.cian.ru/"));
        assert!(!row["title"].as_str().unwrap().is_empty());
        assert!(row["price_rub"].as_f64().unwrap() > 0.0);
    }
    let table = output.join(outcome.exported["summary"]["file_name"].as_str().unwrap());
    assert_eq!(crate::flow::ops::excel::read_sheet(&table).unwrap().len(), 3);
    assert!(traffic.lock().unwrap().iter().any(|request| request.starts_with("CONNECT www.cian.ru:443")));
    println!("{}", json!({ "status": "done", "count": rows.len(), "auth_statuses": outcome.exported["summary"]["auth_statuses"], "files": output.display().to_string() }));
}

#[test]
fn bundled_library_templates_are_valid() {
    for file in ["login-csv.json", "login-csv.en.json", "cian.json", "cian.en.json", "google-search.json", "google-search.en.json"] {
        let source = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/flows/examples").join(file)).unwrap();
        let flow = parse(&source).unwrap();
        let report = validate(&flow, &|_| None);
        assert!(report.errors.is_empty(), "{file}: {}", report.summary());
        assert!(report.warnings.is_empty(), "{file}: {:?}", report.warnings);
        if file.starts_with("google-search") {
            assert_eq!(flow.settings.after_browser, super::model::AfterBrowser::Keep);
            assert_eq!(flow.settings.after_tabs, super::model::AfterTabs::Keep);
        } else {
            assert_eq!(flow.settings.after_browser, super::model::AfterBrowser::Quit);
        }
    }
}

#[tokio::test]
async fn google_search_graph_branches_and_configured_page_limit() {
    use super::interp::{Ctx, Exit};
    use serde_json::{Value, json};
    let source = include_str!("../../../tools/flows/examples/google-search.json");
    // Browser/DOM responses are fixtures; the real interpreter still executes every branch, loop and group.
    for (limit, count, hit, has_results, safe) in [
        (1, 1, None, false, false), (1, 1, None, true, false), (5, 5, None, true, false),
        (20, 20, None, true, false), (20, 20, Some(19), true, true),
        (20, 1, Some(0), true, false), (3, 3, Some(2), true, true),
    ] {
        let mut flow = parse(source).unwrap();
        fn fixture(node: &mut super::model::Node, safe: bool) {
            for child in &mut node.nodes { fixture(child, safe); }
            if node.op.starts_with("web.") || node.op.starts_with("wait.") || node.op.starts_with("key.") {
                let original = node.id.clone();
                node.op = "proc.vars".into();
                node.config = serde_json::from_value(json!({"mode":"add", "vars":[{"name":format!("seen_{original}"),"type":"boolean","value":true}]})).unwrap();
                if original == "init" {
                    node.config = serde_json::from_value(json!({"vars":[{"name":"configuration","type":"object","value":{"keyword":"sample","target":{}}}]})).unwrap();
                } else if original == "home_ready_sample" {
                    node.config = serde_json::from_value(json!({"vars":[{"name":"google_gate","type":"object","value":{"ready":true,"state":"home"}}]})).unwrap();
                } else if original == "submit_ready" || original == "next_ready" {
                    // No browser in this fixture: eval fails like a destroyed context, exercising the node's error policy.
                    node.op = "web.js".into();
                    node.config = serde_json::from_value(json!({"code":"true"})).unwrap();
                } else if original == "inspect_sample" || original == "prepare_next" {
                    node.op = "proc.field".into();
                    node.config = serde_json::from_value(json!({"data":if original == "inspect_sample" {"${fixtures}"} else {"${next_flags}"},"key":"${page_index}","save_to":if original == "inspect_sample" {"google_gate"} else {"more"}})).unwrap();
                } else if original == "pick_link" {
                    node.config = serde_json::from_value(json!({"vars":[{"name":"has_safe_link","type":"boolean","value":safe}]})).unwrap();
                } else if original == "next_click" {
                    node.config = serde_json::from_value(json!({"vars":[{"name":"last_next_index","type":"number","value":"${page_index}"}]})).unwrap();
                }
            }
        }
        for node in &mut flow.nodes { fixture(node, safe); }
        let mut ctx = Ctx::bare();
        ctx.scope.set("page_limit", json!(limit)).unwrap();
        let pages: Vec<Value> = (0..count).map(|index| json!({"ready":true,"state":"results","has_results":has_results,"matched_url":if hit == Some(index) {"https://example.com/"} else {""}})).collect();
        ctx.scope.set("fixtures", json!(pages)).unwrap();
        ctx.scope.set("next_flags", json!((0..count).map(|index| index + 1 < count).collect::<Vec<_>>())).unwrap();
        let outcome = ctx.run(&flow).await.unwrap();
        assert!(matches!(outcome, Exit::Export(_)));
        assert_eq!(ctx.scope.get("page_index"), Some(&json!(count - 1)));
        assert_eq!(ctx.scope.get("seen_close_empty").is_some(), !has_results);
        assert_eq!(ctx.scope.get("seen_visit").is_some(), hit.is_some());
        assert_eq!(ctx.scope.get("seen_back").is_some(), hit.is_some() && safe);
        assert_eq!(ctx.scope.get("seen_follow_click").is_some(), hit.is_some() && safe);
        if count > 1 { assert_eq!(ctx.scope.get("last_next_index"), Some(&json!(count - 2))); }
    }
}

#[tokio::test]
async fn google_captcha_wait_resumes_and_is_bounded_without_consuming_search_pages() {
    use super::interp::{Ctx, Exit};
    use serde_json::{Value, json};
    let source = include_str!("../../../tools/flows/examples/google-search.json");
    for states in [
        vec![json!({"ready":false,"state":"captcha"}), json!({"ready":true,"state":"home"})],
        vec![json!({"ready":false,"state":"consent"}); 3],
        Vec::<Value>::new(),
    ] {
        let mut flow = parse(source).unwrap();
        let mut gate = flow.nodes.iter().find(|node| node.id == "home_ready").unwrap().clone();
        for node in &mut gate.nodes {
            match node.id.as_str() {
                "home_ready_loop" => { node.config.insert("times".into(), json!(3)); }
                "home_ready_pause" => { node.config.insert("ms".into(), json!(0)); }
                "home_ready_sample" if !states.is_empty() => {
                    node.op = "proc.field".into();
                    node.config = serde_json::from_value(json!({"data":"${gate_fixtures}","key":"${google_wait_index}","save_to":"google_gate"})).unwrap();
                }
                "home_ready_verify" => {
                    // Inspect the gate's result; requireGoogleReady itself is covered by the JS regression test.
                    node.op = "proc.vars".into();
                    node.config = serde_json::from_value(json!({"mode":"export","names":["google_gate","google_wait_index"]})).unwrap();
                }
                _ => {}
            }
        }
        let start = flow.nodes.iter().find(|node| node.op == "start").unwrap().clone();
        flow.edges = vec![serde_json::from_value(json!({"from":start.id,"to":gate.id})).unwrap()];
        flow.nodes = vec![start, gate];
        let mut ctx = Ctx::bare();
        ctx.scope.set("target", json!({})).unwrap();
        ctx.scope.set("page_index", json!(4)).unwrap();
        ctx.scope.set("gate_fixtures", json!(states)).unwrap();
        let Exit::Export(result) = ctx.run(&flow).await.unwrap() else { panic!("gate did not finish") };
        let resumed = states.iter().any(|state| state["ready"] == true);
        assert_eq!(result["google_gate"]["ready"], json!(resumed));
        assert_eq!(result["google_wait_index"], json!(if resumed { 1 } else { 2 }));
        assert_eq!(ctx.scope.get("page_index"), Some(&json!(4)), "CAPTCHA retries do not consume search pages");
        if states.is_empty() { assert_eq!(result["google_gate"]["state"], "loading", "failed CDP samples retain the reset state"); }
    }
}
