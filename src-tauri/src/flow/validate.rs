//! Проверка процесса перед запуском (спецификация автоматизации, «Проверка»): ошибки называют узел и поле;
//! предупреждения запуск не останавливают.
use crate::flow::model::{Edge, FORMAT, Flow, Graph, Node, NodePolicy};
use crate::flow::ops::{self, Cfg, Kind, Shape};
use crate::flow::vars::{SYSTEM, coerce, is_var_name, refs, whole_ref};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Issue {
    pub node: Option<String>,
    pub field: Option<String>,
    pub message: String,
}

impl Issue {
    fn new(node: Option<&str>, field: Option<&str>, message: impl Into<String>) -> Self {
        Self { node: node.map(str::to_string), field: field.map(str::to_string), message: message.into() }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.node, &self.field) {
            (Some(node), Some(field)) => write!(f, "узел {node}, поле {field}: {}", self.message),
            (Some(node), None) => write!(f, "узел {node}: {}", self.message),
            (None, Some(field)) => write!(f, "поле {field}: {}", self.message),
            (None, None) => f.write_str(&self.message),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Report {
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Ошибки одной строкой — для итога запуска.
    pub fn summary(&self) -> String {
        self.errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
    }

    fn error(&mut self, node: Option<&str>, field: Option<&str>, message: impl Into<String>) {
        self.errors.push(Issue::new(node, field, message));
    }

    fn warn(&mut self, node: Option<&str>, field: Option<&str>, message: impl Into<String>) {
        self.warnings.push(Issue::new(node, field, message));
    }
}

/// `lookup` — процесс из базы по id; `subflows` самого процесса просматриваются раньше базы.
pub fn validate(flow: &Flow, lookup: &dyn Fn(i64) -> Option<Flow>) -> Report {
    let mut report = Report::default();
    check_flow(flow, &mut report);
    let find = |id: i64| flow.subflows.get(&id.to_string()).cloned().or_else(|| lookup(id));
    check_calls(flow, &find, &mut Vec::new(), &mut HashSet::new(), &mut report);
    report
}

fn check_flow(flow: &Flow, r: &mut Report) {
    if flow.format != FORMAT {
        r.error(None, Some("format"), format!("ожидается {FORMAT}"));
    }
    if flow.name.trim().is_empty() || flow.name.chars().count() > 100 {
        r.error(None, Some("name"), "Название сценария должно содержать от 1 до 100 символов");
    }
    let mut names = HashSet::new();
    for decl in &flow.variables {
        let field = format!("variables.{}", decl.name);
        if !is_var_name(&decl.name) {
            r.error(None, Some(field.as_str()), "имя — латиница, цифры и _, не с цифры");
        } else if SYSTEM.contains(&decl.name.as_str()) {
            r.error(None, Some(field.as_str()), "совпадает с системной переменной");
        } else if !names.insert(decl.name.as_str()) {
            r.error(None, Some(field.as_str()), "переменная объявлена дважды");
        }
        if let Err(e) = coerce(decl.ty, decl.default.clone()) {
            r.error(None, Some(field.as_str()), format!("Значение по умолчанию: {e}"));
        }
    }
    unique_ids(&flow.nodes, &mut HashSet::new(), r);
    check_scope(flow.graph(), true, false, r);
    unreachable(flow, r);
    unknown_refs(flow, r);
}

fn unique_ids<'f>(nodes: &'f [Node], seen: &mut HashSet<&'f str>, r: &mut Report) {
    for node in nodes {
        if !seen.insert(&node.id) {
            r.error(Some(node.id.as_str()), None, "id узла повторяется");
        }
        unique_ids(&node.nodes, seen, r);
    }
}

/// Один уровень: процесс (`top`) или группа. `in_loop` — уровень лежит в теле цикла внешнего уровня.
fn check_scope(graph: Graph<'_>, top: bool, in_loop: bool, r: &mut Report) {
    let starts: Vec<&Node> = graph.nodes.iter().filter(|node| node.op == "start").collect();
    if top && starts.len() != 1 {
        r.error(None, None, "В сценарии должен быть ровно один блок «Начало»");
    }
    if !top {
        for node in &starts {
            r.error(Some(node.id.as_str()), None, "Блок «Начало» нельзя размещать внутри группы");
        }
    }
    for node in graph.nodes {
        check_node(node, r);
    }
    let shapes: HashMap<&str, Shape> =
        graph.nodes.iter().filter_map(|node| ops::spec(&node.op).map(|spec| (node.id.as_str(), spec.shape))).collect();
    let mut used = HashSet::new();
    for edge in graph.edges {
        check_edge(graph, &shapes, edge, &mut used, r);
    }
    if has_cycle(graph) {
        r.error(None, None, "цикл в графе без узла цикла: связи возвращаются только во вход loop");
    }
    let mut in_body: HashSet<&str> = HashSet::new();
    let loops: Vec<(&str, HashSet<&str>)> = graph
        .nodes
        .iter()
        .filter(|node| shapes.get(node.id.as_str()) == Some(&Shape::Loop))
        .map(|node| (node.id.as_str(), body_of(graph, &node.id)))
        .collect();
    for (id, body) in &loops {
        for edge in graph.edges.iter().filter(|edge| edge.to == *id && edge.to_port == "loop") {
            if !body.contains(edge.from.as_str()) {
                r.error(Some(edge.from.as_str()), None, format!("связь во вход loop узла {id} идёт не из его тела"));
            }
            // Связь из тела вложенного цикла интерпретатор засчитает вложенному: итерацию кончит не тот цикл.
            else if let Some((inner, _)) = loops.iter().find(|(m, inner_body)| m != id && body.contains(m) && inner_body.contains(edge.from.as_str())) {
                r.error(Some(edge.from.as_str()), None, format!("связь во вход loop узла {id} идёт из тела вложенного цикла {inner}"));
            }
        }
        in_body.extend(body);
    }
    for node in graph.nodes {
        let looped = in_loop || in_body.contains(node.id.as_str());
        if node.op == "control.break" && !looped {
            r.error(Some(node.id.as_str()), None, "Блок «Выход из цикла» должен находиться внутри цикла");
        }
        if node.op == "control.group" {
            check_group(node, looped, r);
        }
    }
}

fn check_node(node: &Node, r: &mut Report) {
    let Some(spec) = ops::spec(&node.op) else {
        r.error(Some(node.id.as_str()), Some("op"), format!("неизвестная операция {}", node.op));
        return;
    };
    if node.description.chars().count() > 100 {
        r.error(Some(node.id.as_str()), Some("description"), "описание — до 100 символов");
    }
    for (field, message) in ops::check_config(spec, &node.config) {
        r.error(Some(node.id.as_str()), Some(field.as_str()), message);
    }
    if node.op != "control.group" && (!node.nodes.is_empty() || !node.edges.is_empty()) {
        r.error(Some(node.id.as_str()), None, "вложенные узлы бывают только у группы");
    }
    if spec.shape == Shape::Start && node.on_error.is_some() {
        r.error(Some(node.id.as_str()), Some("on_error"), "Для блока «Начало» нельзя задать обработку ошибок");
    }
}

fn check_edge<'g>(
    graph: Graph<'g>,
    shapes: &HashMap<&str, Shape>,
    edge: &'g Edge,
    used: &mut HashSet<(&'g str, &'g str)>,
    r: &mut Report,
) {
    let (Some(from), Some(to)) = (graph.node(&edge.from), graph.node(&edge.to)) else {
        r.error(Some(edge.from.as_str()), None, format!("связь {} → {}: узла нет на этом уровне", edge.from, edge.to));
        return;
    };
    if let Some(shape) = shapes.get(from.id.as_str()) {
        let error_port = edge.port == "error" && from.on_error == Some(NodePolicy::Branch);
        if !shape.outputs().contains(&edge.port.as_str()) && !error_port {
            r.error(Some(from.id.as_str()), Some("port"), format!("у узла нет выхода {}", edge.port));
        }
    }
    if let Some(shape) = shapes.get(to.id.as_str())
        && !shape.inputs().contains(&edge.to_port.as_str())
    {
        r.error(Some(to.id.as_str()), Some("to_port"), format!("у узла нет входа {}", edge.to_port));
    }
    if !used.insert((edge.from.as_str(), edge.port.as_str())) {
        r.error(Some(edge.from.as_str()), Some("port"), format!("из выхода {} больше одной связи", edge.port));
    }
}

/// Цикл без учёта связей во входы `loop`.
fn has_cycle(graph: Graph<'_>) -> bool {
    fn visit<'g>(graph: Graph<'g>, id: &'g str, state: &mut HashMap<&'g str, bool>) -> bool {
        // `false` — узел в пути обхода, `true` — пройден.
        match state.get(id) {
            Some(false) => return true,
            Some(true) => return false,
            None => {}
        }
        state.insert(id, false);
        let cyclic = graph.edges.iter().filter(|edge| edge.from == id && edge.to_port != "loop").any(|edge| visit(graph, &edge.to, state));
        state.insert(id, true);
        cyclic
    }
    let mut state = HashMap::new();
    graph.nodes.iter().any(|node| visit(graph, &node.id, &mut state))
}

/// Узлы тела цикла: достижимы из его выхода `body` без связей во входы `loop` и без прохода через сам цикл.
pub(crate) fn body_of<'g>(graph: Graph<'g>, loop_id: &str) -> HashSet<&'g str> {
    let mut seen = HashSet::new();
    let mut queue: VecDeque<&'g str> = graph
        .edges
        .iter()
        .filter(|edge| edge.from == loop_id && edge.port == "body" && edge.to != loop_id)
        .map(|edge| edge.to.as_str())
        .collect();
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        queue.extend(
            graph.edges.iter().filter(|edge| edge.from == id && edge.to_port != "loop" && edge.to != loop_id).map(|edge| edge.to.as_str()),
        );
    }
    seen
}

fn check_group(node: &Node, in_loop: bool, r: &mut Report) {
    let inner = node.inner();
    if inner.nodes.is_empty() {
        r.error(Some(node.id.as_str()), None, "в группе нет узлов");
        return;
    }
    // Проверка только ослабляется: годится группа, верная по старому или по новому правилу выхода.
    if !one_chain(inner, false) && !one_chain(inner, true) {
        r.error(Some(node.id.as_str()), None, "Соедините блоки группы последовательно: один начальный и один конечный блок.");
    }
    check_scope(inner, false, in_loop, r);
}

/// Один вход (узел без входящих связей, кроме `loop`) и один выход. Старое правило (`loops = false`): выход — узел без
/// исходящих связей. Новое: узлы тела цикла не в счёт — их тупик кончает итерацию, а не группу, — а цикл (New Browser)
/// со свободным `next` — выход: обход группы идёт за цикл дальше. Это же правило у «Сгруппировать» (`groupItems`).
fn one_chain(inner: Graph<'_>, loops: bool) -> bool {
    let entries = inner.nodes.iter().filter(|n| !inner.edges.iter().any(|e| e.to == n.id && e.to_port != "loop")).count();
    let out = |n: &Node, port: Option<&str>| inner.edges.iter().any(|e| e.from == n.id && port.is_none_or(|p| e.port == p));
    let looped = |n: &Node| loops && ops::spec(&n.op).is_some_and(|spec| spec.shape == Shape::Loop);
    let bodies: HashSet<&str> = inner.nodes.iter().filter(|n| looped(n)).flat_map(|n| body_of(inner, &n.id)).collect();
    let exits = inner
        .nodes
        .iter()
        .filter(|n| !bodies.contains(n.id.as_str()) && (!out(n, None) || (looped(n) && !out(n, Some("next")))))
        .count();
    entries == 1 && exits == 1
}

fn unreachable(flow: &Flow, r: &mut Report) {
    let graph = flow.graph();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = graph.nodes.iter().filter(|n| n.op == "start").map(|n| n.id.as_str()).collect();
    while let Some(id) = queue.pop_front() {
        if seen.insert(id) {
            queue.extend(graph.edges.iter().filter(|e| e.from == id).map(|e| e.to.as_str()));
        }
    }
    for node in graph.nodes.iter().filter(|n| !seen.contains(n.id.as_str())) {
        r.warn(Some(node.id.as_str()), None, "Блок не соединён с началом сценария");
    }
}

/// `${x}`, которую не задаёт ни Start, ни «Save to», ни системные, — предупреждение: её может дать таблица задачи.
fn unknown_refs(flow: &Flow, r: &mut Report) {
    let mut known: HashSet<String> = SYSTEM.iter().map(|name| name.to_string()).collect();
    known.extend(flow.variables.iter().map(|decl| decl.name.clone()));
    outputs(&flow.nodes, &mut known);
    scan(&flow.nodes, &known, r);
}

fn outputs(nodes: &[Node], known: &mut HashSet<String>) {
    for node in nodes {
        if let Some(spec) = ops::spec(&node.op) {
            let cfg = Cfg::new(spec, &node.config);
            for field in spec.fields.iter().filter(|field| field.kind == Kind::VarOut) {
                known.extend(cfg.var_out(field.name));
            }
            let rows = node.config.get("vars").and_then(Value::as_array).into_iter().flatten();
            known.extend(rows.filter_map(|row| row["name"].as_str()).map(str::to_string));
        }
        outputs(&node.nodes, known);
    }
}

fn scan(nodes: &[Node], known: &HashSet<String>, r: &mut Report) {
    for node in nodes {
        let spec = ops::spec(&node.op);
        for (key, value) in &node.config {
            let kind = spec.and_then(|s| s.fields.iter().find(|f| f.name == key.as_str())).map(|f| f.kind);
            if kind == Some(Kind::Code) {
                continue;
            }
            let mut texts = Vec::new();
            strings(value, &mut texts);
            // Секрет: имя из скобок может быть куском набранного пароля — не цитируется; ссылка целиком — с именем.
            if kind == Some(Kind::Secret) && value.as_str().is_none_or(|text| whole_ref(text).is_none()) {
                if texts.into_iter().flat_map(refs).any(|name| !known.contains(name)) {
                    r.warn(Some(node.id.as_str()), Some(key.as_str()), ops::BAD_SECRET_REF);
                }
                continue;
            }
            for name in texts.into_iter().flat_map(refs).filter(|name| !known.contains(*name)) {
                r.warn(Some(node.id.as_str()), Some(key.as_str()), format!("Для переменной {name} не задано значение. Укажите его в таблице задачи."));
            }
        }
        scan(&node.nodes, known, r);
    }
}

fn strings<'v>(value: &'v Value, out: &mut Vec<&'v str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(items) => items.iter().for_each(|item| strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| strings(item, out)),
        _ => {}
    }
}

/// `control.call`: процесс есть, входы объявлены, рекурсии нет; ошибки подпроцессов — с их именем.
fn check_calls(flow: &Flow, find: &dyn Fn(i64) -> Option<Flow>, stack: &mut Vec<i64>, checked: &mut HashSet<i64>, r: &mut Report) {
    let mut calls = Vec::new();
    collect_calls(&flow.nodes, &mut calls);
    for node in calls {
        // Неверное поле `process` уже названо проверкой полей.
        let Some(id) = node.config.get("process").and_then(Value::as_i64) else { continue };
        let Some(sub) = find(id) else {
            r.error(Some(node.id.as_str()), Some("process"), format!("Сценарий {id} не найден"));
            continue;
        };
        if let Some(inputs) = node.config.get("inputs").and_then(Value::as_object) {
            for name in inputs.keys().filter(|name| !sub.variables.iter().any(|decl| &decl.name == *name)) {
                r.error(Some(node.id.as_str()), Some("inputs"), format!("В сценарии {id} нет переменной {name}"));
            }
        }
        if stack.contains(&id) {
            r.error(Some(node.id.as_str()), Some("process"), format!("Сценарий {id} вызывает сам себя. Удалите повторный вызов."));
            continue;
        }
        if !checked.insert(id) {
            continue;
        }
        let mut inner = Report::default();
        check_flow(&sub, &mut inner);
        for issue in inner.errors {
            r.error(None, None, format!("Вложенный сценарий «{}»: {issue}", sub.name));
        }
        stack.push(id);
        check_calls(&sub, find, stack, checked, r);
        stack.pop();
    }
}

fn collect_calls<'f>(nodes: &'f [Node], out: &mut Vec<&'f Node>) {
    for node in nodes {
        if node.op == "control.call" {
            out.push(node);
        }
        collect_calls(&node.nodes, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn flow(nodes: Value, edges: Value) -> Flow {
        serde_json::from_value(json!({ "format": "ombra-flow/1", "name": "t", "nodes": nodes, "edges": edges })).unwrap()
    }

    fn start() -> Value {
        json!({ "id": "s", "op": "start" })
    }

    fn errors(flow: &Flow) -> Vec<String> {
        validate(flow, &|_| None).errors.iter().map(ToString::to_string).collect()
    }

    fn has(list: &[String], needle: &str) -> bool {
        list.iter().any(|line| line.contains(needle))
    }

    #[test]
    fn minimal_flow_is_valid() {
        let f = flow(
            json!([start(), { "id": "g", "op": "web.goto", "config": { "url": "https://a.test" } }, { "id": "q", "op": "control.quit_browser" }]),
            json!([{ "from": "s", "to": "g" }, { "from": "g", "to": "q" }]),
        );
        let report = validate(&f, &|_| None);
        assert!(report.is_ok(), "{:?}", report.errors);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    #[test]
    fn root_fields_and_start_are_checked() {
        let mut f = flow(json!([]), json!([]));
        f.format = "ombra-flow/2".into();
        f.name = " ".into();
        let e = errors(&f);
        assert!(has(&e, "поле format"), "{e:?}");
        assert!(has(&e, "поле name"), "{e:?}");
        assert!(has(&e, "ровно один блок «Начало»"), "{e:?}");
        assert!(has(&errors(&flow(json!([start(), { "id": "s2", "op": "start" }]), json!([]))), "ровно один блок «Начало»"));
        let nested = flow(json!([start(), { "id": "g", "op": "control.group", "nodes": [{ "id": "s3", "op": "start" }] }]), json!([{ "from": "s", "to": "g" }]));
        assert!(has(&errors(&nested), "узел s3: Блок «Начало» нельзя размещать внутри группы"));
    }

    #[test]
    fn node_ids_ops_and_fields_are_checked() {
        let f = flow(
            json!([
                start(),
                { "id": "a", "op": "web.teleport" },
                { "id": "b", "op": "web.goto", "config": { "colour": 1 } },
                { "id": "g", "op": "control.group", "nodes": [{ "id": "b", "op": "web.reload" }] },
                { "id": "c", "op": "web.reload", "description": "я".repeat(101) },
                { "id": "d", "op": "web.reload", "nodes": [{ "id": "e", "op": "web.reload" }] }
            ]),
            json!([]),
        );
        let e = errors(&f);
        assert!(has(&e, "узел a, поле op: неизвестная операция web.teleport"), "{e:?}");
        assert!(has(&e, "узел b, поле colour: Неизвестное поле"), "{e:?}");
        assert!(has(&e, "узел b, поле url: обязательное поле не заполнено"), "{e:?}");
        assert!(has(&e, "узел b: id узла повторяется"), "{e:?}");
        assert!(has(&e, "узел c, поле description"), "{e:?}");
        assert!(has(&e, "узел d: вложенные узлы бывают только у группы"), "{e:?}");
    }

    #[test]
    fn edges_follow_ports() {
        let f = flow(
            json!([
                start(),
                { "id": "i", "op": "control.if", "config": { "variable": "x", "condition": "exist" } },
                { "id": "a", "op": "web.reload" },
                { "id": "b", "op": "web.reload", "on_error": "branch" },
                { "id": "c", "op": "web.reload" }
            ]),
            json!([
                { "from": "s", "to": "i" },
                { "from": "i", "to": "a" },
                { "from": "i", "port": "true", "to": "z" },
                { "from": "a", "port": "error", "to": "c" },
                { "from": "b", "port": "error", "to": "c" },
                { "from": "s", "to": "a" },
                { "from": "c", "to": "a", "to_port": "loop" }
            ]),
        );
        let e = errors(&f);
        assert!(has(&e, "узел i, поле port: у узла нет выхода next"), "{e:?}");
        assert!(has(&e, "связь i → z: узла нет на этом уровне"), "{e:?}");
        assert!(has(&e, "узел a, поле port: у узла нет выхода error"), "{e:?}");
        assert!(!has(&e, "узел b, поле port"), "у узла с branch есть выход error: {e:?}");
        assert!(has(&e, "узел s, поле port: из выхода next больше одной связи"), "{e:?}");
        assert!(has(&e, "узел a, поле to_port: у узла нет входа loop"), "{e:?}");
    }

    #[test]
    fn cycles_go_only_through_loop_inputs() {
        let plain = flow(
            json!([start(), { "id": "a", "op": "web.reload" }, { "id": "b", "op": "web.reload" }]),
            json!([{ "from": "s", "to": "a" }, { "from": "a", "to": "b" }, { "from": "b", "to": "a" }]),
        );
        assert!(has(&errors(&plain), "цикл в графе без узла цикла"));
        let nodes = json!([start(), { "id": "l", "op": "control.for_times", "config": { "times": 2 } }, { "id": "a", "op": "web.reload" }, { "id": "o", "op": "web.reload" }]);
        let looped = flow(
            nodes.clone(),
            json!([{ "from": "s", "to": "l" }, { "from": "l", "port": "body", "to": "a" }, { "from": "a", "to": "l", "to_port": "loop" }, { "from": "l", "to": "o" }]),
        );
        assert!(errors(&looped).is_empty(), "{:?}", errors(&looped));
        let outside = flow(
            nodes,
            json!([{ "from": "s", "to": "l" }, { "from": "l", "port": "body", "to": "a" }, { "from": "l", "to": "o" }, { "from": "o", "to": "l", "to_port": "loop" }]),
        );
        assert!(has(&errors(&outside), "узел o: связь во вход loop узла l идёт не из его тела"));
    }

    #[test]
    fn nested_loops_keep_their_own_loop_edges() {
        let nodes = json!([
            start(),
            { "id": "l1", "op": "control.for_times", "config": { "times": 2 } },
            { "id": "l2", "op": "control.for_times", "config": { "times": 2 } },
            { "id": "x", "op": "web.reload" },
            { "id": "y", "op": "web.reload" }
        ]);
        let edges = |tail: Vec<Value>| {
            let mut all = vec![json!({ "from": "s", "to": "l1" }), json!({ "from": "l1", "port": "body", "to": "l2" }), json!({ "from": "l2", "port": "body", "to": "x" })];
            all.extend(tail);
            Value::Array(all)
        };
        let bad = flow(nodes.clone(), edges(vec![json!({ "from": "x", "to": "l1", "to_port": "loop" })]));
        assert!(has(&errors(&bad), "узел x: связь во вход loop узла l1 идёт из тела вложенного цикла l2"), "{:?}", errors(&bad));
        let good = flow(
            nodes,
            edges(vec![
                json!({ "from": "x", "to": "l2", "to_port": "loop" }),
                json!({ "from": "l2", "to": "y" }),
                json!({ "from": "y", "to": "l1", "to_port": "loop" }),
            ]),
        );
        assert!(errors(&good).is_empty(), "{:?}", errors(&good));
    }

    #[test]
    fn exit_loop_only_inside_a_loop_body() {
        let outside = flow(json!([start(), { "id": "b", "op": "control.break" }]), json!([{ "from": "s", "to": "b" }]));
        assert!(has(&errors(&outside), "узел b: Блок «Выход из цикла» должен находиться внутри цикла"));
        let inside = flow(
            json!([start(), { "id": "l", "op": "control.for_times", "config": { "times": 2 } }, { "id": "g", "op": "control.group", "nodes": [{ "id": "b", "op": "control.break" }] }]),
            json!([{ "from": "s", "to": "l" }, { "from": "l", "port": "body", "to": "g" }]),
        );
        assert!(errors(&inside).is_empty(), "{:?}", errors(&inside));
    }

    #[test]
    fn groups_hold_one_chain() {
        let empty = flow(json!([start(), { "id": "g", "op": "control.group" }]), json!([{ "from": "s", "to": "g" }]));
        assert!(has(&errors(&empty), "узел g: в группе нет узлов"));
        let two = json!([{ "id": "a", "op": "web.reload" }, { "id": "b", "op": "web.reload" }]);
        let split = flow(json!([start(), { "id": "g", "op": "control.group", "nodes": two }]), json!([{ "from": "s", "to": "g" }]));
        assert!(has(&errors(&split), "узел g: Соедините блоки группы последовательно"));
        let chain = flow(
            json!([start(), { "id": "g", "op": "control.group", "nodes": two, "edges": [{ "from": "a", "to": "b" }] }]),
            json!([{ "from": "s", "to": "g" }]),
        );
        assert!(errors(&chain).is_empty(), "{:?}", errors(&chain));
    }

    #[test]
    fn group_may_end_with_a_loop_whose_next_is_free() {
        let group = |nodes: Value, edges: Value| {
            flow(json!([start(), { "id": "g", "op": "control.group", "nodes": nodes, "edges": edges }]), json!([{ "from": "s", "to": "g" }]))
        };
        let a = json!({ "id": "a", "op": "web.reload" });
        let b1 = json!({ "id": "b1", "op": "web.reload" });
        let looped = |op: &str, config: Value| json!({ "id": "l", "op": op, "config": config });
        let body = json!([{ "from": "a", "to": "l" }, { "from": "l", "port": "body", "to": "b1" }, { "from": "b1", "to": "l", "to_port": "loop" }]);
        for l in [looped("control.for_times", json!({ "times": 2 })), looped("control.new_browser", json!({ "serial": 2 }))] {
            let f = group(json!([a.clone(), l, b1.clone()]), body.clone());
            assert!(errors(&f).is_empty(), "{:?}", errors(&f));
        }
        // Занятый `next` цикла — не выход; выход один — узел после цикла.
        let after = group(
            json!([a.clone(), looped("control.for_times", json!({ "times": 2 })), b1.clone(), { "id": "z", "op": "web.reload" }]),
            json!([{ "from": "a", "to": "l" }, { "from": "l", "port": "body", "to": "b1" }, { "from": "b1", "to": "l", "to_port": "loop" }, { "from": "l", "to": "z" }]),
        );
        assert!(errors(&after).is_empty(), "{:?}", errors(&after));
        // Цикл со свободным `next` и ещё тупик: по новому правилу два выхода, по старому — один (y); годится.
        let two = group(
            json!([{ "id": "i", "op": "control.if", "config": { "variable": "x", "condition": "eq", "value": "1" } }, looped("control.for_times", json!({ "times": 2 })), b1.clone(), { "id": "y", "op": "web.reload" }]),
            json!([{ "from": "i", "port": "true", "to": "l" }, { "from": "i", "port": "false", "to": "y" }, { "from": "l", "port": "body", "to": "b1" }, { "from": "b1", "to": "l", "to_port": "loop" }]),
        );
        assert!(errors(&two).is_empty(), "{:?}", errors(&two));
    }

    #[test]
    fn dead_end_in_a_loop_body_is_not_a_group_exit() {
        // Тупик в теле кончает итерацию, а не группу: выход — только цикл со свободным `next`.
        let group = |nodes: Value, edges: Value| {
            flow(json!([start(), { "id": "g", "op": "control.group", "nodes": nodes, "edges": edges }]), json!([{ "from": "s", "to": "g" }]))
        };
        let browser = group(
            json!([{ "id": "b", "op": "control.new_browser", "config": { "serial": 2 } }, { "id": "i", "op": "web.reload" }]),
            json!([{ "from": "b", "port": "body", "to": "i" }]),
        );
        assert!(errors(&browser).is_empty(), "{:?}", errors(&browser));
        let looped = group(
            json!([{ "id": "a", "op": "web.reload" }, { "id": "l", "op": "control.for_times", "config": { "times": 2 } }, { "id": "b1", "op": "web.reload" }]),
            json!([{ "from": "a", "to": "l" }, { "from": "l", "port": "body", "to": "b1" }]),
        );
        assert!(errors(&looped).is_empty(), "{:?}", errors(&looped));
    }

    #[test]
    fn group_check_only_relaxes_the_old_exit_rule() {
        // Формы групп из тестов выше и новые: (узлы, связи, годится ли сейчас).
        let w = |id: &str| json!({ "id": id, "op": "web.reload" });
        let times = json!({ "id": "l", "op": "control.for_times", "config": { "times": 2 } });
        let browser = json!({ "id": "l", "op": "control.new_browser", "config": { "serial": 2 } });
        let iff = json!({ "id": "i", "op": "control.if", "config": { "variable": "x", "condition": "eq", "value": "1" } });
        let back = || vec![json!({ "from": "a", "to": "l" }), json!({ "from": "l", "port": "body", "to": "b1" }), json!({ "from": "b1", "to": "l", "to_port": "loop" })];
        let with = |mut edges: Vec<Value>, extra: Value| {
            edges.push(extra);
            Value::Array(edges)
        };
        let shapes = [
            (json!([w("b")]), json!([]), true),
            (json!([w("a"), w("b")]), json!([]), false),
            (json!([w("a"), w("b")]), json!([{ "from": "a", "to": "b" }]), true),
            (json!([w("a"), times.clone(), w("b1")]), Value::Array(back()), true),
            (json!([w("a"), browser.clone(), w("b1")]), Value::Array(back()), true),
            (json!([w("a"), times.clone(), w("b1"), w("z")]), with(back(), json!({ "from": "l", "to": "z" })), true),
            (
                json!([iff.clone(), times.clone(), w("b1"), w("y")]),
                json!([{ "from": "i", "port": "true", "to": "l" }, { "from": "i", "port": "false", "to": "y" }, { "from": "l", "port": "body", "to": "b1" }, { "from": "b1", "to": "l", "to_port": "loop" }]),
                true,
            ),
            (json!([browser.clone(), w("b1")]), json!([{ "from": "l", "port": "body", "to": "b1" }]), true),
            (json!([w("a"), times.clone(), w("b1")]), json!([{ "from": "a", "to": "l" }, { "from": "l", "port": "body", "to": "b1" }]), true),
            (json!([iff, w("x"), w("y")]), json!([{ "from": "i", "port": "true", "to": "x" }, { "from": "i", "port": "false", "to": "y" }]), false),
        ];
        for (nodes, edges, valid) in shapes {
            let f = flow(json!([start(), { "id": "g", "op": "control.group", "nodes": nodes, "edges": edges }]), json!([{ "from": "s", "to": "g" }]));
            let now = !has(&errors(&f), "Соедините блоки группы последовательно");
            let old = one_chain(f.nodes[1].inner(), false);
            assert!(!old || now, "по старому правилу годилась, теперь нет: {:?}", f.nodes[1]);
            assert_eq!(now, valid, "{:?}: {:?}", f.nodes[1], errors(&f));
        }
    }

    #[test]
    fn variables_are_checked() {
        let mut f = flow(json!([start()]), json!([]));
        f.variables = serde_json::from_value(json!([
            { "name": "1x", "type": "string" },
            { "name": "password", "type": "string" },
            { "name": "n", "type": "number", "default": "abc" },
            { "name": "ok", "type": "array" },
            { "name": "ok", "type": "string" }
        ]))
        .unwrap();
        let e = errors(&f);
        assert!(has(&e, "поле variables.1x: имя"), "{e:?}");
        assert!(has(&e, "поле variables.password: совпадает с системной переменной"), "{e:?}");
        assert!(has(&e, "поле variables.n: Значение по умолчанию: ожидается число"), "{e:?}");
        assert!(has(&e, "поле variables.ok: переменная объявлена дважды"), "{e:?}");
    }

    #[test]
    fn calls_need_existing_processes_without_recursion() {
        let call = |process: i64, inputs: Value| {
            flow(
                json!([start(), { "id": "c", "op": "control.call", "config": { "process": process, "inputs": inputs } }]),
                json!([{ "from": "s", "to": "c" }]),
            )
        };
        assert!(has(&errors(&call(5, json!({}))), "узел c, поле process: Сценарий 5 не найден"));
        let mut sub = flow(json!([start()]), json!([]));
        sub.name = "Вход".into();
        sub.variables = serde_json::from_value(json!([{ "name": "q", "type": "string" }])).unwrap();
        let mut main = call(5, json!({ "q": "x", "z": "y" }));
        main.subflows.insert("5".into(), sub.clone());
        assert!(has(&errors(&main), "узел c, поле inputs: В сценарии 5 нет переменной z"));
        let looping = call(7, json!({}));
        let found = looping.clone();
        let report = validate(&looping, &move |id| (id == 7).then(|| found.clone()));
        assert!(report.errors.iter().any(|e| e.to_string().contains("вызывает сам себя")), "{:?}", report.errors);
        let mut broken = sub;
        broken.nodes.push(serde_json::from_value(json!({ "id": "x", "op": "web.teleport" })).unwrap());
        let mut main = call(5, json!({}));
        main.subflows.insert("5".into(), broken);
        assert!(has(&errors(&main), "Вложенный сценарий «Вход»: узел x, поле op: неизвестная операция"));
    }

    #[test]
    fn warnings_do_not_block() {
        let f = flow(
            json!([
                start(),
                { "id": "g", "op": "web.goto", "config": { "url": "${site}" } },
                { "id": "j", "op": "web.js", "config": { "code": "`${x}`", "save_to": "r" } },
                { "id": "u", "op": "web.reload" },
                { "id": "k", "op": "web.goto", "config": { "url": "${r}" } }
            ]),
            json!([{ "from": "s", "to": "g" }, { "from": "g", "to": "j" }, { "from": "j", "to": "k" }]),
        );
        let report = validate(&f, &|_| None);
        assert!(report.is_ok(), "{:?}", report.errors);
        let warnings: Vec<String> = report.warnings.iter().map(ToString::to_string).collect();
        assert!(has(&warnings, "узел g, поле url: Для переменной site не задано значение"), "{warnings:?}");
        assert!(has(&warnings, "узел u: Блок не соединён с началом сценария"), "{warnings:?}");
        assert!(!has(&warnings, "Для переменной x"), "в коде JS `${{}}` — шаблон JavaScript: {warnings:?}");
        assert!(!has(&warnings, "Для переменной r"), "r задаёт save_to: {warnings:?}");
    }

    #[test]
    fn secret_fields_never_quote_their_text() {
        let mail = |id: &str, password: &str| json!({ "id": id, "op": "data.email", "config": { "email": "a@b.test", "server": "s", "password": password, "save_to": "o" } });
        let f = flow(
            json!([start(), mail("m", "x${a-b}"), mail("n", "Ab${12"), mail("p", "${mail_pw}"), { "id": "t", "op": "data.totp", "config": { "secret": "Q${qz}", "save_to": "c" } }]),
            json!([{ "from": "s", "to": "m" }, { "from": "m", "to": "n" }, { "from": "n", "to": "p" }, { "from": "p", "to": "t" }]),
        );
        let report = validate(&f, &|_| None);
        let all = format!("{:?} {:?}", report.errors, report.warnings);
        for fragment in ["a-b", "Ab", "12", "qz"] {
            assert!(!all.contains(fragment), "{fragment}: {all}");
        }
        let warnings: Vec<String> = report.warnings.iter().map(ToString::to_string).collect();
        assert!(has(&warnings, "узел m, поле password: неверная ссылка на переменную"), "{warnings:?}");
        assert!(has(&warnings, "узел t, поле secret: неверная ссылка на переменную"), "{warnings:?}");
        assert!(has(&warnings, "узел p, поле password: Для переменной mail_pw не задано значение"), "ссылка целиком — с именем: {warnings:?}");
        // Не строка (правка файла руками) — тоже без имени из скобок.
        let mut f = flow(json!([start(), mail("m", "")]), json!([{ "from": "s", "to": "m" }]));
        f.nodes[1].config.insert("password".into(), json!(["x${a-b}"]));
        let report = validate(&f, &|_| None);
        let all = format!("{:?} {:?}", report.errors, report.warnings);
        assert!(!all.contains("a-b"), "{all}");
    }
}
