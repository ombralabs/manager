//! Процесс автоматизации — граф узлов по модели RPA Plus (спецификация автоматизации, «Формат процесса»).
//! Тот же JSON лежит в базе, отдаётся экспортом и принимается CLI из файла.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const FORMAT: &str = "ombra-flow/1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flow {
    pub format: String,
    pub name: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub remark: String,
    #[serde(default)]
    pub variables: Vec<VarDecl>,
    #[serde(default)]
    pub settings: Settings,
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    /// Процессы, которые вызывает `control.call`, — только в файле экспорта; ключ — id в базе источника.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub subflows: BTreeMap<String, Flow>,
}

/// Переменная процесса, объявленная на узле Start (глобальная переменная RPA Plus).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VarDecl {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: VarType,
    #[serde(default)]
    pub default: Value,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VarType {
    String,
    Number,
    Boolean,
    Array,
    Object,
    Text,
    File,
    Folder,
}

/// Настройки процесса, как в AdsPower.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub on_error: ErrorPolicy,
    pub after_tabs: AfterTabs,
    pub after_browser: AfterBrowser,
    pub headless: bool,
    pub timeout_s: u64,
    pub delay_min: u64,
    pub launch_args: Vec<String>,
    pub clear_cache_on_quit: bool,
    /// Принимаются и не применяются — как параметры паролей в Local API.
    pub disable_password_autofill: bool,
    pub save_password: bool,
}

/// Политика ошибок процесса по умолчанию.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorPolicy {
    #[default]
    Interrupt,
    Skip,
}

/// Политика ошибок узла; `branch` даёт узлу выход `error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodePolicy {
    Interrupt,
    Skip,
    Branch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AfterTabs {
    #[default]
    Clear,
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AfterBrowser {
    #[default]
    Quit,
    Keep,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub op: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub config: Map<String, Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_error: Option<NodePolicy>,
    /// Координаты на холсте; исполнению не нужны.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f64; 2]>,
    /// Узлы и связи группы (`control.group`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: String,
    #[serde(default = "port_next")]
    pub port: String,
    pub to: String,
    #[serde(default = "port_in")]
    pub to_port: String,
}

fn port_next() -> String {
    "next".into()
}

fn port_in() -> String {
    "in".into()
}

/// Узлы и связи одного уровня: процесса или группы.
#[derive(Debug, Clone, Copy)]
pub struct Graph<'a> {
    pub nodes: &'a [Node],
    pub edges: &'a [Edge],
}

// ponytail: узлы и связи ищутся перебором — процессы в сотни узлов; индекс по id, если графы вырастут.
impl<'a> Graph<'a> {
    pub fn node(&self, id: &str) -> Option<&'a Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// Связь из выхода `port` узла `from`; проверка гарантирует, что она одна.
    pub fn next(&self, from: &str, port: &str) -> Option<&'a Edge> {
        self.edges.iter().find(|edge| edge.from == from && edge.port == port)
    }
}

impl Flow {
    pub fn graph(&self) -> Graph<'_> {
        Graph { nodes: &self.nodes, edges: &self.edges }
    }
}

impl Node {
    /// Уровень внутри группы.
    pub fn inner(&self) -> Graph<'_> {
        Graph { nodes: &self.nodes, edges: &self.edges }
    }
}

pub fn parse(text: &str) -> Result<Flow, String> {
    serde_json::from_str(text).map_err(|e| format!("Не удалось прочитать сценарий: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Пример из спецификации, раздел «Формат процесса».
    const EXAMPLE: &str = r##"{
      "format": "ombra-flow/1",
      "name": "Вход и поиск",
      "group": "Маркетплейсы",
      "remark": "",
      "variables": [
        { "name": "site", "type": "string", "default": "https://example.com", "description": "" },
        { "name": "queries", "type": "array", "default": ["кофе", "чай"], "description": "" }
      ],
      "settings": { "on_error": "interrupt", "after_tabs": "clear", "after_browser": "quit", "headless": false,
                    "timeout_s": 0, "delay_min": 0, "launch_args": [], "clear_cache_on_quit": false,
                    "disable_password_autofill": false, "save_password": false },
      "nodes": [
        { "id": "start", "op": "start", "pos": [0, 0] },
        { "id": "n1", "op": "web.goto", "config": { "url": "${site}", "timeout_ms": 30000 }, "pos": [0, 120] },
        { "id": "n2", "op": "control.for_data", "config": { "data": "${queries}", "item_to": "q", "index_to": "i" }, "pos": [0, 240] },
        { "id": "n3", "op": "web.input", "on_error": "skip", "description": "Строка поиска",
          "config": { "target": { "by": "css", "selector": "#search", "index": 1 }, "mode": "in_order", "content": "${q}", "clear": true },
          "pos": [200, 360] },
        { "id": "end", "op": "control.quit_browser", "pos": [0, 480] }
      ],
      "edges": [
        { "from": "start", "to": "n1" },
        { "from": "n1", "to": "n2" },
        { "from": "n2", "port": "body", "to": "n3" },
        { "from": "n3", "to": "n2", "to_port": "loop" },
        { "from": "n2", "port": "next", "to": "end" }
      ],
      "subflows": {}
    }"##;

    #[test]
    fn spec_example_parses() {
        let flow = parse(EXAMPLE).unwrap();
        assert_eq!(flow.format, FORMAT);
        assert_eq!(flow.variables[1].ty, VarType::Array);
        assert_eq!(flow.settings.on_error, ErrorPolicy::Interrupt);
        let first = &flow.edges[0];
        assert_eq!((first.port.as_str(), first.to_port.as_str()), ("next", "in"), "порты по умолчанию");
        assert_eq!(flow.edges[3].to_port, "loop");
        assert_eq!(flow.nodes[3].on_error, Some(NodePolicy::Skip));
        assert_eq!(flow.nodes[3].description, "Строка поиска");
    }

    #[test]
    fn missing_settings_take_defaults() {
        let flow = parse(r#"{"format":"ombra-flow/1","name":"x","nodes":[]}"#).unwrap();
        assert_eq!(flow.settings, Settings::default());
        assert_eq!((flow.settings.after_tabs, flow.settings.after_browser), (AfterTabs::Clear, AfterBrowser::Quit));
        assert!(flow.variables.is_empty() && flow.edges.is_empty() && flow.subflows.is_empty());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let error = parse(r#"{"format":"ombra-flow/1","name":"x","nodes":[{"id":"a","op":"start","colour":1}]}"#).unwrap_err();
        assert!(error.contains("colour"), "{error}");
        assert!(parse(r#"{"format":"ombra-flow/1","name":"x","nodes":[],"settings":{"retry":1}}"#).is_err());
        assert!(parse(r#"{"format":"ombra-flow/1","name":"x","nodes":[],"edges":[{"from":"a","to":"b","via":"c"}]}"#).is_err());
    }

    #[test]
    fn serialization_skips_empty_node_fields() {
        let flow = parse(r#"{"format":"ombra-flow/1","name":"x","nodes":[{"id":"a","op":"start"}]}"#).unwrap();
        let value = serde_json::to_value(&flow).unwrap();
        assert_eq!(value["nodes"][0], json!({ "id": "a", "op": "start" }));
        assert!(value.get("subflows").is_none());
        assert_eq!(parse(&value.to_string()).unwrap(), flow, "туда и обратно");
    }

    #[test]
    fn graph_follows_edges_by_port() {
        let flow = parse(EXAMPLE).unwrap();
        let graph = flow.graph();
        assert_eq!(graph.next("n2", "body").map(|e| e.to.as_str()), Some("n3"));
        assert_eq!(graph.next("n2", "next").map(|e| e.to.as_str()), Some("end"));
        assert!(graph.next("end", "next").is_none());
        assert_eq!(graph.node("n1").unwrap().op, "web.goto");
        assert!(graph.node("nope").is_none());
    }
}
