//! Сеть в процессе: Request to finish, Listener Request, Listener Request Response, Stop Listener Request, Get и Clear
//! Page Cookies (спецификация автоматизации, «Получение и обработка данных»). Слушают запросы вкладки, которая была
//! текущей при регистрации слушателя.
use crate::engine::network::{NetEvent, NetGuard, Network};
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::data::query_param;
use crate::flow::ops::web::{tabs, wait};
use crate::flow::ops::{Cfg, OpError};
use crate::flow::vars::to_text;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Найденное слушателями и ещё не перенесённое в переменные: (имя, значение) по порядку.
/// Третье поле — глубина вызовов подпроцессов при регистрации слушателя.
type Pending = Arc<Mutex<Vec<(usize, String, Value)>>>;

/// Слушатели запуска: каждый — задача на событиях вкладки. Найденное ждёт в `pending`, пока интерпретатор не перенесёт
/// его в переменные перед следующим узлом — переменная не меняется посреди узла.
#[derive(Default)]
pub(crate) struct Listeners {
    pending: Pending,
    /// Снятие слушателей: (номер регистрации, сигнал).
    stops: Vec<(u64, CancellationToken)>,
    /// Номер следующего слушателя.
    next: u64,
}

impl Listeners {
    /// Найденное с прошлого раза на этой глубине вызовов. Найденное родителем (мельче) ждёт возврата из подпроцесса;
    /// найденное завершившимся подпроцессом (глубже) отбрасывается: его переменных уже нет.
    pub(crate) fn take(&self, depth: usize) -> Vec<(String, Value)> {
        let mut pending = self.pending.lock().expect("мьютекс слушателей отравлен");
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *pending).into_iter().partition(|item| item.0 == depth);
        *pending = rest.into_iter().filter(|item| item.0 < depth).collect();
        mine.into_iter().map(|(_, name, value)| (name, value)).collect()
    }

    /// Номер, с которого пойдут слушатели, зарегистрированные дальше: подпроцесс и тело New Browser снимают своих
    /// (`stop_since`), когда кончаются, — иначе те держали бы сеть и писали бы в переменные следующего вызова.
    pub(crate) fn mark(&self) -> u64 {
        self.next
    }

    /// Снять слушателей, зарегистрированных начиная с `mark`; сколько снято.
    pub(crate) fn stop_since(&mut self, mark: u64) -> usize {
        let before = self.stops.len();
        self.stops.retain(|(number, stop)| {
            if *number >= mark {
                stop.cancel();
            }
            *number < mark
        });
        before - self.stops.len()
    }

    /// Снять всех слушателей; сколько было.
    pub(crate) fn stop(&mut self) -> usize {
        self.stop_since(0)
    }

    fn add(&mut self, stop: CancellationToken) {
        self.stops.push((self.next, stop));
        self.next += 1;
    }
}

impl Drop for Listeners {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Сеть вкладки, включённая заранее — перед узлом, за которым идёт Request to finish: события копятся с начала этого
/// узла, и запрос, который он сам вызвал (клик), не проходит мимо ожидания. Отпущенная — выключает сеть.
/// ponytail: события копятся без предела весь узел; для долгого узла перед ожиданием (цикл) — фильтр по адресу.
pub(crate) struct Armed {
    net: Network,
    events: mpsc::UnboundedReceiver<NetEvent>,
    _guard: NetGuard,
}

/// Включить сеть и копить события вкладки, пока `Armed` жив.
pub(crate) async fn arm(net: Network) -> Result<Armed, OpError> {
    let mut events = net.events();
    let guard = net.acquire().await?;
    let (sender, receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = sender.closed() => return,
                event = events.next() => {
                    // Соединение закрыто — ожидание узнает об этом по концу канала.
                    let Some(event) = event else { return };
                    if sender.send(event).is_err() {
                        return;
                    }
                }
            }
        }
    });
    Ok(Armed { net, events: receiver, _guard: guard })
}

/// Что слушатель берёт из запроса.
#[derive(Clone)]
enum Pick {
    Url,
    Headers,
    Param(String),
    PostData,
    Response,
}

pub(crate) async fn exec(ctx: &mut Ctx, id: &str, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let note = match id {
        "wait.request" => {
            let needle = cfg.text(&ctx.scope, "url_contains")?;
            let ms = cfg.int(&ctx.scope, "timeout_ms")?;
            let wait = wait(ms)?;
            let net = tabs(ctx)?.page().await?.network();
            // Включённая заранее сеть — если узел перед этим остался на той же вкладке; иначе включаем сейчас.
            let armed = match ctx.armed.take() {
                Some(armed) if armed.net.same_page(&net) => armed,
                _ => arm(net).await?,
            };
            if finished(armed, &needle, wait).await? { String::new() } else { format!("Ответ не получен за {ms} мс") }
        }
        "data.listen_request" | "data.listen_response" => {
            let needle = cfg.text(&ctx.scope, "url_contains")?;
            let pick = match (id, cfg.string("kind").as_str()) {
                ("data.listen_response", _) => Pick::Response,
                (_, "headers") => Pick::Headers,
                (_, "get_param") => {
                    if cfg.raw("param").is_null() {
                        return Err({
                            log::warn!("поле param не заполнено");
                            "Заполните поле «Параметр»"
                        }.into());
                    }
                    Pick::Param(cfg.text(&ctx.scope, "param")?)
                }
                (_, "post_data") => Pick::PostData,
                _ => Pick::Url,
            };
            let name = cfg.var_out("save_to").ok_or_else(|| {
                log::warn!("поле save_to не заполнено");
                "Заполните поле «Сохранить в»"
            })?;
            let net = tabs(ctx)?.page().await?.network();
            let stop = listen(net, needle, pick, name, ctx.listeners.pending.clone(), ctx.depth()).await?;
            ctx.listeners.add(stop);
            format!("Отслеживаются адреса, содержащие {}", to_text(&cfg.raw("url_contains")))
        }
        "data.listen_stop" => format!("Остановлено отслеживаний: {}", ctx.listeners.stop()),
        "data.cookies_get" | "data.cookies_clear" => {
            let page = tabs(ctx)?.page().await?;
            let url = page.url().await?;
            let list = page.cookies(&url).await?;
            if id == "data.cookies_clear" {
                page.delete_cookies(&list).await?;
                format!("удалено: {}", list.len())
            } else {
                let value = Value::Array(crate::cookies::json::to_values(&list));
                return Ok((Step::Port("next"), ctx.save(cfg, value)?));
            }
        }
        other => return Err(OpError::Failed(format!("операция {other} не исполняется"))),
    };
    Ok((Step::Port("next"), note))
}

/// Конец первого запроса с `needle` в адресе, начавшегося с включения сети `armed`; `false` — не дождались.
async fn finished(mut armed: Armed, needle: &str, wait: Duration) -> Result<bool, OpError> {
    let watch = async {
        let mut ours = None;
        while let Some(event) = armed.events.recv().await {
            match event {
                NetEvent::Sent { id, url, .. } if ours.is_none() && url.contains(needle) => ours = Some(id),
                NetEvent::Done { id, .. } if ours.as_ref() == Some(&id) => return Ok(true),
                _ => {}
            }
        }
        Err(OpError::BrowserClosed)
    };
    tokio::time::timeout(wait, watch).await.unwrap_or(Ok(false))
}

/// Слушатель: задача на событиях вкладки; каждый подходящий запрос перезаписывает переменную `name`.
async fn listen(net: Network, needle: String, pick: Pick, name: String, pending: Pending, depth: usize) -> Result<CancellationToken, OpError> {
    let mut events = net.events();
    let guard = net.acquire().await?;
    let stop = CancellationToken::new();
    let stopped = stop.clone();
    tokio::spawn(async move {
        let _guard = guard;
        let mut ours = HashSet::new();
        loop {
            let event = tokio::select! {
                _ = stopped.cancelled() => return,
                event = events.next() => match event {
                    Some(event) => event,
                    None => return,
                },
            };
            let value = match (&pick, event) {
                (Pick::Response, NetEvent::Sent { id, url, .. }) => {
                    if url.contains(&needle) {
                        ours.insert(id);
                    }
                    continue;
                }
                (Pick::Response, NetEvent::Done { id, failed }) => {
                    if !ours.remove(&id) || failed {
                        continue;
                    }
                    let body = tokio::select! {
                        _ = stopped.cancelled() => return,
                        body = net.response_body(&id) => body,
                    };
                    match body {
                        Ok(body) => Value::String(body),
                        Err(_) => continue,
                    }
                }
                (_, NetEvent::Done { .. }) => continue,
                (_, NetEvent::Sent { url, .. }) if !url.contains(&needle) => continue,
                (Pick::Url, NetEvent::Sent { url, .. }) => Value::String(url),
                (Pick::Headers, NetEvent::Sent { headers, .. }) => headers,
                (Pick::Param(param), NetEvent::Sent { url, .. }) => Value::String(query_param(&url, param)),
                (Pick::PostData, NetEvent::Sent { id, post_data, has_post_data, .. }) => Value::String(match post_data {
                    Some(body) => body,
                    None if has_post_data => tokio::select! {
                        _ = stopped.cancelled() => return,
                        body = net.post_data(&id) => body.unwrap_or_default(),
                    },
                    None => String::new(),
                }),
            };
            if stopped.is_cancelled() {
                return;
            }
            pending.lock().expect("мьютекс слушателей отравлен").push((depth, name.clone(), value));
        }
    });
    Ok(stop)
}

#[cfg(test)]
mod tests {
    use crate::engine::page::Browser;
    use crate::engine::page::tests::{Mock, base};
    use crate::flow::interp::{Ctx, Exit, Fail};
    use crate::flow::model::Flow;
    use crate::flow::ops::web::Tabs;
    use serde_json::{Map, Value, json};
    use std::time::{Duration, Instant};

    fn chain(nodes: Value) -> Flow {
        let mut all = vec![json!({ "id": "s", "op": "start" })];
        let mut edges = Vec::new();
        let mut prev = "s".to_string();
        for node in nodes.as_array().unwrap() {
            let id = node["id"].as_str().unwrap().to_string();
            edges.push(json!({ "from": prev, "to": id }));
            prev = id;
            all.push(node.clone());
        }
        serde_json::from_value(json!({ "format": "ombra-flow/1", "name": "t", "nodes": all, "edges": edges })).unwrap()
    }

    async fn tab_ctx(mock: &Mock) -> Ctx {
        let mut ctx = Ctx::bare();
        ctx.tabs = Some(Tabs::new(Browser::connect(&mock.url).await.unwrap(), Some("T1".into())));
        ctx
    }

    fn mock_answers(method: &str) -> Value {
        base(method).unwrap_or_else(|| match method {
            "Network.getResponseBody" => json!({ "body": "{\"ok\":1}", "base64Encoded": false }),
            "Target.getTargetInfo" => json!({ "targetInfo": { "url": "https://a.test/x" } }),
            "Network.getCookies" => json!({ "cookies": [{ "name": "sid", "value": "1", "domain": "a.test", "path": "/", "expires": -1, "session": true }] }),
            _ => json!({}),
        })
    }

    fn sent(id: &str, url: &str) -> Value {
        json!({ "method": "Network.requestWillBeSent", "sessionId": "S1",
                "params": { "requestId": id, "request": { "url": url, "headers": {}, "postData": "a=1" } } })
    }

    fn done(id: &str) -> Value {
        json!({ "method": "Network.loadingFinished", "sessionId": "S1", "params": { "requestId": id } })
    }

    fn exported(exit: Exit) -> Map<String, Value> {
        match exit {
            Exit::Export(values) => values,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn listener_writes_each_matching_request_before_the_next_node() {
        let mock = Mock::spawn(|method, _, _| mock_answers(method)).await;
        let mut ctx = tab_ctx(&mock).await;
        let flow = chain(json!([
            { "id": "l", "op": "data.listen_request", "config": { "url_contains": "api", "kind": "get_param", "param": "k", "save_to": "k" } },
            { "id": "p", "op": "data.listen_request", "config": { "url_contains": "api", "kind": "post_data", "save_to": "body" } },
            { "id": "r", "op": "data.listen_response", "config": { "url_contains": "api", "save_to": "resp" } },
            { "id": "w", "op": "wait.time", "config": { "ms": 400 } },
            { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["k", "body", "resp"] } }
        ]));
        let events = mock.events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            for event in [sent("R1", "https://a.test/api?k=1"), sent("R2", "https://a.test/other?k=9"), sent("R3", "https://a.test/api?k=2"), done("R3")] {
                events.send(event).unwrap();
            }
        });
        let values = exported(ctx.run(&flow).await.unwrap());
        assert_eq!(values["k"], json!("2"), "последний подходящий запрос перезаписал переменную");
        assert_eq!(values["body"], json!("a=1"));
        assert_eq!(values["resp"], json!("{\"ok\":1}"));
        assert_eq!(mock.sent("Network.enable").len(), 1, "три слушателя — одно включение");
    }

    #[tokio::test]
    async fn stop_and_wait_release_the_network() {
        let mock = Mock::spawn(|method, _, _| mock_answers(method)).await;
        let mut ctx = tab_ctx(&mock).await;
        let events = mock.events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            events.send(sent("R1", "https://a.test/api")).unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            events.send(done("R1")).unwrap();
        });
        let started = Instant::now();
        ctx.run(&chain(json!([{ "id": "w", "op": "wait.request", "config": { "url_contains": "api", "timeout_ms": 3000 } }]))).await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(190) && started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        let started = Instant::now();
        ctx.run(&chain(json!([{ "id": "w", "op": "wait.request", "config": { "url_contains": "never", "timeout_ms": 200 } }]))).await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(200), "по тайм-ауту — дальше, без ошибки");
        ctx.run(&chain(json!([
            { "id": "l", "op": "data.listen_request", "config": { "url_contains": "api", "save_to": "u" } },
            { "id": "st", "op": "data.listen_stop" }
        ])))
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(mock.sent("Network.enable").len(), mock.sent("Network.disable").len(), "каждое включение выключено");
    }

    #[tokio::test]
    async fn stopped_listener_does_not_update_the_variable() {
        let mock = Mock::spawn(|method, _, _| mock_answers(method)).await;
        let mut ctx = tab_ctx(&mock).await;
        let flow = chain(json!([
            { "id": "l", "op": "data.listen_request", "config": { "url_contains": "api", "save_to": "u" } },
            { "id": "w", "op": "wait.time", "config": { "ms": 300 } },
            { "id": "st", "op": "data.listen_stop" },
            { "id": "w2", "op": "wait.time", "config": { "ms": 300 } },
            { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["u"] } }
        ]));
        let events = mock.events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            events.send(sent("R1", "https://a.test/api?first")).unwrap();
            tokio::time::sleep(Duration::from_millis(350)).await;
            events.send(sent("R2", "https://a.test/api?second")).unwrap();
        });
        let values = exported(ctx.run(&flow).await.unwrap());
        assert_eq!(values["u"], json!("https://a.test/api?first"));
    }

    #[tokio::test]
    async fn wait_after_a_node_catches_the_request_that_node_started() {
        let mock = Mock::spawn(|method, _, _| mock_answers(method)).await;
        let mut ctx = tab_ctx(&mock).await;
        // Узел A (300 мс) сам вызывает запрос: начало — во время A, конец — уже во время ожидания.
        let flow = chain(json!([
            { "id": "a", "op": "wait.time", "config": { "ms": 300 } },
            { "id": "w", "op": "wait.request", "config": { "url_contains": "api", "timeout_ms": 3000 } }
        ]));
        let (events, requests) = (mock.events.clone(), mock.requests.clone());
        let during_a = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let enabled = requests.lock().unwrap().iter().any(|r| r["method"] == "Network.enable");
            events.send(sent("R1", "https://a.test/api")).unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            events.send(done("R1")).unwrap();
            enabled
        });
        let started = Instant::now();
        ctx.run(&flow).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(1), "дождались конца запроса, а не тайм-аута: {:?}", started.elapsed());
        assert!(during_a.await.unwrap(), "сеть включена до конца узла A");
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(mock.sent("Network.enable").len(), 1);
        assert_eq!(mock.sent("Network.disable").len(), 1, "после ожидания сеть выключена");
        let methods: Vec<String> =
            mock.requests.lock().unwrap().iter().filter_map(|r| r["method"].as_str()).filter(|m| m.starts_with("Network.")).map(str::to_string).collect();
        assert_eq!(methods, ["Network.enable", "Network.disable"]);
    }

    #[tokio::test]
    async fn network_armed_for_a_wait_is_released_when_the_flow_goes_elsewhere() {
        let mock = Mock::spawn(|method, _, _| mock_answers(method)).await;
        let mut ctx = tab_ctx(&mock).await;
        // A падает в ветку ошибки: ожидание за A не начнётся, включённая для него сеть гаснет.
        let flow: Flow = serde_json::from_value(json!({
            "format": "ombra-flow/1", "name": "t",
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "a", "op": "key.combo", "on_error": "branch", "config": { "modifiers": ["cmd"], "key": "ж" } },
                { "id": "w", "op": "wait.request", "config": { "url_contains": "api", "timeout_ms": 3000 } },
                { "id": "e", "op": "wait.time", "config": { "ms": 10 } }
            ],
            "edges": [{ "from": "s", "to": "a" }, { "from": "a", "to": "w" }, { "from": "a", "port": "error", "to": "e" }]
        }))
        .unwrap();
        ctx.run(&flow).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!((mock.sent("Network.enable").len(), mock.sent("Network.disable").len()), (1, 1));
        assert!(ctx.armed.is_none());
        // Ветка ошибки без связи — тупик: цепочка кончается, включённое гаснет.
        let flow: Flow = serde_json::from_value(json!({
            "format": "ombra-flow/1", "name": "t",
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "a", "op": "key.combo", "on_error": "branch", "config": { "modifiers": ["cmd"], "key": "ж" } },
                { "id": "w", "op": "wait.request", "config": { "url_contains": "api", "timeout_ms": 3000 } }
            ],
            "edges": [{ "from": "s", "to": "a" }, { "from": "a", "to": "w" }]
        }))
        .unwrap();
        ctx.run(&flow).await.unwrap();
        assert!(ctx.armed.is_none(), "тупик после ветки ошибки");
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!((mock.sent("Network.enable").len(), mock.sent("Network.disable").len()), (2, 2));
    }

    fn timed_out(ctx: &Ctx, node: &str) -> bool {
        ctx.journal.entries().iter().any(|entry| entry.node == node && entry.msg.contains("Ответ не получен"))
    }

    #[tokio::test]
    async fn consecutive_waits_do_not_finish_on_one_request() {
        let mock = Mock::spawn(|method, _, _| mock_answers(method)).await;
        let mut ctx = tab_ctx(&mock).await;
        let flow = chain(json!([
            { "id": "w1", "op": "wait.request", "config": { "url_contains": "api", "timeout_ms": 3000 } },
            { "id": "w2", "op": "wait.request", "config": { "url_contains": "api", "timeout_ms": 300 } }
        ]));
        let events = mock.events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            events.send(sent("R1", "https://a.test/api")).unwrap();
            events.send(done("R1")).unwrap();
        });
        ctx.run(&flow).await.unwrap();
        assert!(!timed_out(&ctx, "w1"), "первое ожидание дождалось");
        assert!(timed_out(&ctx, "w2"), "второе не кончилось на запросе первого");
    }

    #[tokio::test]
    async fn network_is_not_armed_for_a_container_body() {
        let mock = Mock::spawn(|method, _, _| mock_answers(method)).await;
        let mut ctx = tab_ctx(&mock).await;
        let flow: Flow = serde_json::from_value(json!({
            "format": "ombra-flow/1", "name": "t",
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "l", "op": "control.for_times", "config": { "times": 1 } },
                { "id": "b", "op": "wait.time", "config": { "ms": 300 } },
                { "id": "w", "op": "wait.request", "config": { "url_contains": "api", "timeout_ms": 200 } }
            ],
            "edges": [
                { "from": "s", "to": "l" }, { "from": "l", "port": "body", "to": "b" },
                { "from": "b", "to": "l", "to_port": "loop" }, { "from": "l", "to": "w" }
            ]
        }))
        .unwrap();
        let (events, requests) = (mock.events.clone(), mock.requests.clone());
        let during_body = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let enabled = requests.lock().unwrap().iter().any(|r| r["method"] == "Network.enable");
            events.send(sent("R1", "https://a.test/api")).unwrap();
            events.send(done("R1")).unwrap();
            enabled
        });
        ctx.run(&flow).await.unwrap();
        assert!(!during_body.await.unwrap(), "сеть не включена на всё тело цикла");
        assert!(timed_out(&ctx, "w"), "запрос из тела цикла ожидание не видит");
    }

    #[tokio::test]
    async fn stop_interrupts_arming() {
        let mock = Mock::spawn(|method, _, _| match method {
            "Network.enable" => json!({ "silent": true }),
            _ => mock_answers(method),
        })
        .await;
        let mut ctx = tab_ctx(&mock).await;
        let flow = chain(json!([
            { "id": "a", "op": "wait.time", "config": { "ms": 50 } },
            { "id": "w", "op": "wait.request", "config": { "url_contains": "api", "timeout_ms": 3000 } }
        ]));
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });
        let started = Instant::now();
        assert_eq!(ctx.run(&flow).await, Err(Fail::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(2), "«Стоп» не ждёт ответа на включение сети: {:?}", started.elapsed());
    }

    fn parse(value: Value) -> Flow {
        serde_json::from_value(value).unwrap()
    }

    #[tokio::test]
    async fn listener_of_a_finished_sub_process_does_not_reach_the_next_call() {
        use crate::flow::journal::Journal;
        use crate::flow::model::ErrorPolicy;
        use crate::flow::vars::Scope;
        let mock = Mock::spawn(|method, _, _| mock_answers(method)).await;
        // Первый подпроцесс ставит слушателя и кончается; второй на той же глубине ждёт, пока идёт подходящий запрос.
        let listens = parse(json!({ "format": "ombra-flow/1", "name": "a",
            "nodes": [{ "id": "s", "op": "start" }, { "id": "l", "op": "data.listen_request", "config": { "url_contains": "api", "save_to": "u" } }],
            "edges": [{ "from": "s", "to": "l" }] }));
        let waits = parse(json!({ "format": "ombra-flow/1", "name": "b",
            "variables": [{ "name": "u", "type": "string", "default": "" }],
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "w", "op": "wait.time", "config": { "ms": 400 } },
                { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["u"] } }
            ],
            "edges": [{ "from": "s", "to": "w" }, { "from": "w", "to": "x" }] }));
        let subs = std::collections::HashMap::from([(1, listens), (2, waits)]);
        let mut ctx = Ctx::new(
            Scope::new(Default::default()),
            ErrorPolicy::Interrupt,
            Box::new(move |id| subs.get(&id).cloned()),
            Journal::memory(),
            tokio_util::sync::CancellationToken::new(),
        );
        ctx.tabs = Some(Tabs::new(Browser::connect(&mock.url).await.unwrap(), Some("T1".into())));
        let flow = chain(json!([
            { "id": "a", "op": "control.call", "config": { "process": 1 } },
            { "id": "b", "op": "control.call", "config": { "process": 2, "save_to": "out" } },
            { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["out"] } }
        ]));
        let events = mock.events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            events.send(sent("R1", "https://a.test/api")).unwrap();
        });
        let values = exported(ctx.run(&flow).await.unwrap());
        assert_eq!(values["out"], json!({ "u": "" }), "слушатель первого вызова снят с его концом");
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!((mock.sent("Network.enable").len(), mock.sent("Network.disable").len()), (1, 1), "его сеть выключена");
    }

    #[tokio::test]
    async fn listener_of_a_new_browser_body_ends_with_the_body() {
        use crate::flow::host::{Opened, tests::FakeHost};
        let main = Mock::spawn(|method, _, _| mock_answers(method)).await;
        let second = Mock::spawn(|method, _, _| mock_answers(method)).await;
        let mut ctx = tab_ctx(&main).await;
        let browser = Browser::connect(&second.url).await.unwrap();
        let opened = Opened { profile_id: "p2".into(), browser, tab: "T1".into(), relay_port: 0, launched: false, busy: None, humanize: false };
        ctx.host = Some(std::sync::Arc::new(FakeHost { opened: std::sync::Mutex::new(Some(opened)), ..Default::default() }));
        ctx.scope.set("u", json!("")).unwrap();
        let flow = parse(json!({ "format": "ombra-flow/1", "name": "t",
            "nodes": [
                { "id": "s", "op": "start" },
                { "id": "b", "op": "control.new_browser", "config": { "serial": 2 } },
                { "id": "l", "op": "data.listen_request", "config": { "url_contains": "api", "save_to": "u" } },
                { "id": "w", "op": "wait.time", "config": { "ms": 400 } },
                { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["u"] } }
            ],
            "edges": [{ "from": "s", "to": "b" }, { "from": "b", "port": "body", "to": "l" }, { "from": "b", "to": "w" }, { "from": "w", "to": "x" }] }));
        let events = second.events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            // Соединение со вторым браузером могло уже закрыться вместе с телом.
            let _ = events.send(sent("R1", "https://a.test/api"));
        });
        let values = exported(ctx.run(&flow).await.unwrap());
        assert_eq!(values["u"], json!(""), "слушатель тела снят с концом тела");
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!((second.sent("Network.enable").len(), second.sent("Network.disable").len()), (1, 1), "сеть второго браузера выключена");
    }

    #[test]
    fn a_scope_stops_only_the_listeners_registered_inside_it() {
        use tokio_util::sync::CancellationToken;
        let mut listeners = super::Listeners::default();
        let (outer, inner, later) = (CancellationToken::new(), CancellationToken::new(), CancellationToken::new());
        listeners.add(outer.clone());
        let mark = listeners.mark();
        listeners.add(inner.clone());
        assert_eq!(listeners.stop_since(mark), 1);
        assert!(inner.is_cancelled() && !outer.is_cancelled(), "слушатель снаружи остаётся");
        // Stop Listener Request внутри снял всех, затем внутри поставлен новый: снимается и он.
        let mark = listeners.mark();
        assert_eq!(listeners.stop(), 1);
        listeners.add(later.clone());
        assert_eq!(listeners.stop_since(mark), 1);
        assert!(later.is_cancelled());
    }

    #[test]
    fn take_keeps_parent_items_and_drops_finished_children() {
        let listeners = super::Listeners::default();
        let push = |depth: usize, name: &str| listeners.pending.lock().unwrap().push((depth, name.into(), json!(1)));
        push(0, "a");
        push(1, "b");
        push(0, "c");
        let names = |items: Vec<(String, Value)>| items.into_iter().map(|i| i.0).collect::<Vec<_>>();
        assert_eq!(names(listeners.take(1)), ["b"]);
        assert_eq!(names(listeners.take(0)), ["a", "c"], "родительское дождалось возврата");
        push(1, "d");
        assert!(listeners.take(0).is_empty(), "глубже — отброшено");
        assert!(listeners.take(0).is_empty());
    }

    #[tokio::test]
    async fn page_cookies_are_saved_in_the_ombra_export_format_and_cleared() {
        let mock = Mock::spawn(|method, _, _| mock_answers(method)).await;
        let mut ctx = tab_ctx(&mock).await;
        let flow = chain(json!([
            { "id": "g", "op": "data.cookies_get", "config": { "save_to": "c" } },
            { "id": "d", "op": "data.cookies_clear" },
            { "id": "x", "op": "proc.vars", "config": { "mode": "export", "names": ["c"] } }
        ]));
        let values = exported(ctx.run(&flow).await.unwrap());
        assert_eq!(values["c"][0]["name"], json!("sid"));
        assert_eq!(values["c"][0]["domain"], json!("a.test"));
        assert_eq!(mock.sent("Network.deleteCookies").len(), 1);
    }
}
