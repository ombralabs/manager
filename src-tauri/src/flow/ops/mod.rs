//! Реестр операций процесса: id, название AdsPower, форма портов, поля (спецификация автоматизации, «Каталог
//! операций»). Один источник для проверки процесса и умолчаний полей, а в А3 — для форм редактора.
use crate::engine::cdp::CdpError;
use crate::engine::page::KEY_NAMES;
use crate::flow::interp::{Ctx, Step};
use crate::flow::model::{Node, VarType};
use crate::flow::vars::{self, CONDITIONS, SYSTEM, Scope, is_var_name};
use serde_json::{Map, Value};
use std::time::Duration;

pub(crate) mod captcha;
pub(crate) mod data;
pub(crate) mod email;
pub(crate) mod excel;
pub(crate) mod files;
pub(crate) mod keys;
pub(crate) mod net;
pub(crate) mod openai;
pub(crate) mod proc;
pub(crate) mod sheets;
pub(crate) mod profile;
pub(crate) mod target;
pub(crate) mod wait;
pub mod web;

/// Форма узла по портам (RPA Plus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Только выход: `start`.
    Start,
    /// Только вход: Quit Browser.
    End,
    /// Statement if: `true` и `false`.
    Branch,
    /// Циклы и New Browser: Loop Start (`body`) и Loop End (`next`), возврат во вход `loop`.
    Loop,
    Plain,
}

impl Shape {
    pub fn inputs(self) -> &'static [&'static str] {
        match self {
            Shape::Start => &[],
            Shape::Loop => &["in", "loop"],
            _ => &["in"],
        }
    }

    pub fn outputs(self) -> &'static [&'static str] {
        match self {
            Shape::Start | Shape::Plain => &["next"],
            Shape::End => &[],
            Shape::Branch => &["true", "false"],
            Shape::Loop => &["body", "next"],
        }
    }

    /// Куда идёт управление, когда узел пропущен (отключён или ошибка с `skip`).
    pub fn main_exit(self) -> &'static str {
        if self == Shape::Branch { "false" } else { "next" }
    }
}

/// Вид поля операции.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Строка; `${…}` разрешены.
    Text,
    /// Код JavaScript: `${…}` в нём — шаблон JavaScript, не подстановка.
    Code,
    /// Целое или строка с `${…}`.
    Int,
    /// Как `Int` или интервал `{min, max}` (Fixed / Random Interval).
    IntRange,
    Bool,
    Choice(&'static [&'static str]),
    /// Имя переменной для записи («Save to»): не системная.
    VarOut,
    /// Имя читаемой переменной; системные можно.
    VarIn,
    VarInList,
    /// Значение любого вида с подстановкой.
    Any,
    /// id процесса в базе.
    Process,
    /// Объект «переменная подпроцесса → значение».
    Inputs,
    /// Строки Manage Variables: `{name, type, value}`.
    VarRows,
    /// Цель: `{by, selector, index?, frame?}` или `{element}`.
    Target,
    /// Цель без `index` (For Loop Elements).
    Targets,
    /// «Using the mouse»: `{x, y, release_ms?}`.
    Mouse,
    /// Непустой список строк из вариантов.
    ChoiceList(&'static [&'static str]),
    /// Имена переменных для `inject` у `web.js`: ещё и не слова JavaScript — константу с таким именем не объявить.
    JsVarList,
    /// Непустой список непустых строк; `${…}` разрешены (теги Update Profile Tag).
    TextList,
    /// Секрет (ключ 2FA): как `Text`, но в журнале — только «задан».
    Secret,
}

impl Kind {
    /// Имя вида для редактора (snake_case).
    pub fn name(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Code => "code",
            Kind::Int => "int",
            Kind::IntRange => "int_range",
            Kind::Bool => "bool",
            Kind::Choice(_) => "choice",
            Kind::VarOut => "var_out",
            Kind::VarIn => "var_in",
            Kind::VarInList => "var_in_list",
            Kind::Any => "any",
            Kind::Process => "process",
            Kind::Inputs => "inputs",
            Kind::VarRows => "var_rows",
            Kind::Target => "target",
            Kind::Targets => "targets",
            Kind::Mouse => "mouse",
            Kind::ChoiceList(_) => "choice_list",
            Kind::JsVarList => "js_var_list",
            Kind::TextList => "text_list",
            Kind::Secret => "secret",
        }
    }

    /// Варианты `Choice`/`ChoiceList`, у остальных пусто.
    pub fn options(self) -> &'static [&'static str] {
        match self {
            Kind::Choice(o) | Kind::ChoiceList(o) => o,
            _ => &[],
        }
    }
}

/// Группа операции в палитре редактора — по префиксу id; вкладки и файлы выделены из своих префиксов.
pub fn group(id: &str) -> &'static str {
    match id {
        "web.new_tab" | "web.close_tab" | "web.close_other_tabs" | "web.switch_tab" | "web.goto" | "web.reload" | "web.back" => "tabs",
        "data.save_txt" | "data.save_excel" | "data.download" | "data.import_txt" | "data.import_excel" => "files",
        "data.cookies_clear" => "web",
        _ => match id.split('.').next().unwrap_or("") {
            "start" | "control" => "control",
            "web" => "web",
            "key" => "key",
            "wait" => "wait",
            "data" => "data",
            "proc" => "proc",
            "profile" => "profile",
            _ => "ext",
        },
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
    /// Умолчание — JSON-литерал.
    pub default: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct OpSpec {
    pub id: &'static str,
    /// Название в AdsPower: им начинаются ошибки узла в журнале и итоге.
    pub adspower: &'static str,
    pub shape: Shape,
    pub fields: &'static [Field],
}

const fn op(id: &'static str, adspower: &'static str, shape: Shape, fields: &'static [Field]) -> OpSpec {
    OpSpec { id, adspower, shape, fields }
}

const fn req(name: &'static str, kind: Kind) -> Field {
    Field { name, kind, required: true, default: None }
}

const fn opt(name: &'static str, kind: Kind, default: Option<&'static str>) -> Field {
    Field { name, kind, required: false, default }
}

/// Виды извлечения Element и For Loop Elements.
const EXTRACT: &[&str] = &["text", "object", "iframe", "html", "attribute", "child"];
/// Модификаторы Key combination.
const MODIFIERS: &[&str] = &["cmd", "ctrl", "alt", "shift"];
/// Слова, которые JavaScript не даёт именам констант.
const JS_RESERVED: &[&str] = &[
    "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "enum",
    "export", "extends", "false", "finally", "for", "function", "if", "implements", "import", "in", "instanceof", "interface",
    "let", "new", "null", "package", "private", "protected", "public", "return", "static", "super", "switch", "this",
    "throw", "true", "try", "typeof", "var", "void", "while", "with", "yield", "arguments", "eval",
];

/// Операции подэтапов А1-1 и А1-2; остальные из каталога спецификации добавляет А4. Порядок внутри группы — порядок палитры.
pub static OPS: &[OpSpec] = &[
    // Process Management
    op("start", "Start process", Shape::Start, &[]),
    op(
        "control.if",
        "Statement if",
        Shape::Branch,
        &[req("variable", Kind::VarIn), req("condition", Kind::Choice(CONDITIONS)), opt("value", Kind::Any, None)],
    ),
    op("control.for_times", "For Loop Times", Shape::Loop, &[req("times", Kind::Int), opt("index_to", Kind::VarOut, Some(r#""for_times_index""#))]),
    op(
        "control.for_data",
        "For Loop Data",
        Shape::Loop,
        &[
            req("data", Kind::Any),
            opt("item_to", Kind::VarOut, Some(r#""for_data_item""#)),
            opt("index_to", Kind::VarOut, Some(r#""for_data_index""#)),
        ],
    ),
    op(
        "control.for_elements",
        "For Loop Elements",
        Shape::Loop,
        &[
            req("target", Kind::Targets),
            opt("kind", Kind::Choice(EXTRACT), Some(r#""object""#)),
            opt("attribute", Kind::Text, None),
            opt("child_selector", Kind::Text, None),
            opt("item_to", Kind::VarOut, Some(r#""for_elements_item""#)),
            opt("index_to", Kind::VarOut, Some(r#""for_elements_index""#)),
        ],
    ),
    op(
        "control.while",
        "While Loop",
        Shape::Loop,
        &[req("variable", Kind::VarIn), req("condition", Kind::Choice(CONDITIONS)), opt("value", Kind::Any, None)],
    ),
    op("control.break", "Exit Loop", Shape::Plain, &[]),
    op("control.group", "Group", Shape::Plain, &[opt("name", Kind::Text, Some(r#""""#))]),
    op(
        "control.call",
        "Apply Created Process",
        Shape::Plain,
        &[req("process", Kind::Process), opt("inputs", Kind::Inputs, Some("{}")), opt("save_to", Kind::VarOut, None)],
    ),
    op(
        "control.new_browser",
        "New Browser",
        Shape::Loop,
        &[req("serial", Kind::Int), opt("after", Kind::Choice(&["keep", "quit"]), Some(r#""quit""#))],
    ),
    op("control.quit_browser", "Quit Browser", Shape::End, &[]),
    // Data Processing
    op(
        "proc.vars",
        "Manage Variables",
        Shape::Plain,
        &[
            opt("mode", Kind::Choice(&["add", "export"]), Some(r#""add""#)),
            opt("vars", Kind::VarRows, Some("[]")),
            opt("names", Kind::VarInList, Some("[]")),
        ],
    ),
    op(
        "proc.regex",
        "Extract from txt",
        Shape::Plain,
        &[
            req("data", Kind::Any),
            req("pattern", Kind::Text),
            opt("ignore_case", Kind::Bool, Some("false")),
            opt("first_only", Kind::Bool, Some("false")),
            req("save_to", Kind::VarOut),
        ],
    ),
    op("proc.to_json", "Convert to JSON", Shape::Plain, &[req("data", Kind::Any), req("save_to", Kind::VarOut)]),
    op("proc.field", "Extract Field", Shape::Plain, &[req("data", Kind::Any), req("key", Kind::Any), req("save_to", Kind::VarOut)]),
    op("proc.random", "Random Extraction", Shape::Plain, &[req("data", Kind::Any), req("save_to", Kind::VarOut)]),
    // Web Actions
    op("web.new_tab", "New Tab", Shape::Plain, &[]),
    op("web.close_tab", "Close Tab", Shape::Plain, &[]),
    op("web.close_other_tabs", "Close Other Tabs", Shape::Plain, &[]),
    op(
        "web.switch_tab",
        "Switch Tabs",
        Shape::Plain,
        &[
            opt("by", Kind::Choice(&["url", "title"]), Some(r#""url""#)),
            opt("match", Kind::Choice(&["eq", "ne", "contains", "not_contains"]), Some(r#""eq""#)),
            req("value", Kind::Text),
        ],
    ),
    op("web.goto", "Access Website", Shape::Plain, &[req("url", Kind::Text), opt("timeout_ms", Kind::Int, Some("30000"))]),
    op("web.reload", "Refresh Webpage", Shape::Plain, &[]),
    op("web.back", "Go Back", Shape::Plain, &[]),
    op(
        "web.click",
        "Click",
        Shape::Plain,
        &[
            req("target", Kind::Target),
            opt("button", Kind::Choice(&["left", "middle", "right"]), Some(r#""left""#)),
            opt("kind", Kind::Choice(&["click", "double"]), Some(r#""click""#)),
            opt("use_mouse", Kind::Mouse, None),
        ],
    ),
    op(
        "web.input",
        "Input",
        Shape::Plain,
        &[
            req("target", Kind::Target),
            opt("mode", Kind::Choice(&["in_order", "random", "random_number"]), Some(r#""in_order""#)),
            opt("content", Kind::Text, None),
            opt("number", Kind::IntRange, None),
            opt("clear", Kind::Bool, Some("false")),
            opt("interval_ms", Kind::IntRange, Some("300")),
        ],
    ),
    op("web.hover", "Hover", Shape::Plain, &[req("target", Kind::Target)]),
    op("web.select", "Drop-down", Shape::Plain, &[req("target", Kind::Target), req("value", Kind::Text)]),
    op("web.focus", "Focus", Shape::Plain, &[req("target", Kind::Target)]),
    op(
        "web.scroll",
        "Scroll",
        Shape::Plain,
        &[
            opt("area", Kind::Choice(&["page", "target"]), Some(r#""page""#)),
            opt("target", Kind::Target, None),
            opt("position", Kind::Choice(&["top", "middle", "bottom", "pixels"]), Some(r#""bottom""#)),
            opt("pixels", Kind::Int, Some("0")),
            opt("kind", Kind::Choice(&["smooth", "auto"]), Some(r#""smooth""#)),
            opt("step_px", Kind::IntRange, Some(r#"{"min":100,"max":150}"#)),
            opt("step_ms", Kind::IntRange, Some(r#"{"min":200,"max":300}"#)),
        ],
    ),
    op(
        "web.upload",
        "Input File",
        Shape::Plain,
        &[
            req("target", Kind::Target),
            opt("source", Kind::Choice(&["file", "folder_random", "url"]), Some(r#""file""#)),
            opt("path", Kind::Text, None),
            opt("url", Kind::Text, None),
            opt("timeout_ms", Kind::Int, Some("30000")),
        ],
    ),
    op(
        "web.screenshot",
        "Screenshot",
        Shape::Plain,
        &[
            opt("name", Kind::Text, None),
            opt("folder", Kind::Text, None),
            opt("full_page", Kind::Bool, Some("false")),
            opt("format", Kind::Choice(&["png", "jpeg"]), Some(r#""png""#)),
            opt("quality", Kind::Int, Some("50")),
        ],
    ),
    op(
        "web.js",
        "Execute JavaScript",
        Shape::Plain,
        &[
            req("code", Kind::Code),
            opt("inject", Kind::JsVarList, Some("[]")),
            opt("frame", Kind::Text, None),
            opt("save_to", Kind::VarOut, None),
        ],
    ),
    // Keyboard Actions
    op("key.press", "Keys", Shape::Plain, &[req("key", Kind::Choice(KEY_NAMES))]),
    op("key.combo", "Key combination", Shape::Plain, &[req("modifiers", Kind::ChoiceList(MODIFIERS)), req("key", Kind::Text)]),
    // Waits
    op("wait.time", "Time", Shape::Plain, &[req("ms", Kind::IntRange)]),
    op(
        "wait.element",
        "Element appears",
        Shape::Plain,
        &[
            req("target", Kind::Target),
            opt("visible", Kind::Bool, Some("false")),
            opt("timeout_ms", Kind::Int, Some("30000")),
            opt("save_to", Kind::VarOut, None),
        ],
    ),
    // Waits
    op("wait.request", "Request to finish", Shape::Plain, &[req("url_contains", Kind::Text), opt("timeout_ms", Kind::Int, Some("10000"))]),
    // Get Data
    op(
        "data.element",
        "Element",
        Shape::Plain,
        &[
            req("target", Kind::Target),
            opt("kind", Kind::Choice(EXTRACT), Some(r#""text""#)),
            opt("attribute", Kind::Text, None),
            opt("child_selector", Kind::Text, None),
            req("save_to", Kind::VarOut),
        ],
    ),
    op("data.focused", "Focused Element", Shape::Plain, &[req("save_to", Kind::VarOut)]),
    op(
        "data.url",
        "URL",
        Shape::Plain,
        &[opt("kind", Kind::Choice(&["full", "domain", "param"]), Some(r#""full""#)), opt("param", Kind::Text, None), req("save_to", Kind::VarOut)],
    ),
    op("data.clipboard", "Clipboard Content", Shape::Plain, &[req("save_to", Kind::VarOut)]),
    op("data.save_txt", "Save to Txt", Shape::Plain, &[req("file_name", Kind::Text), req("template", Kind::Text)]),
    op("data.save_excel", "Save to Excel", Shape::Plain, &[req("file_name", Kind::Text), req("columns", Kind::VarInList)]),
    op("data.download", "Download File", Shape::Plain, &[req("url", Kind::Text), opt("folder", Kind::Text, None)]),
    op("data.import_txt", "Import Data from Txt", Shape::Plain, &[req("path", Kind::Text), req("save_to", Kind::VarOut)]),
    op(
        "data.import_excel",
        "Import Data from Excel",
        Shape::Plain,
        &[req("path", Kind::Text), opt("skip_missing_serial", Kind::Bool, Some("false")), req("save_to", Kind::VarOut)],
    ),
    op("data.totp", "Verification Code", Shape::Plain, &[opt("secret", Kind::Secret, None), req("save_to", Kind::VarOut)]),
    op(
        "data.email",
        "Email",
        Shape::Plain,
        &[
            req("email", Kind::Text),
            opt("password", Kind::Secret, None),
            req("server", Kind::Text),
            opt("port", Kind::Int, Some("993")),
            opt("ssl", Kind::Bool, Some("true")),
            opt("client_id", Kind::Text, None),
            opt("client_secret", Kind::Secret, None),
            opt("refresh_token", Kind::Secret, None),
            opt("status", Kind::Choice(&["all", "unseen", "seen"]), Some(r#""all""#)),
            opt("mark_read", Kind::Bool, Some("false")),
            opt("spam", Kind::Bool, Some("false")),
            opt("date", Kind::Choice(&["today", "yesterday", "7d", "30d"]), Some(r#""today""#)),
            opt("from", Kind::Text, None),
            opt("subject", Kind::Text, None),
            opt("extract", Kind::Text, None),
            req("save_to", Kind::VarOut),
        ],
    ),
    op(
        "data.listen_request",
        "Listener Request",
        Shape::Plain,
        &[
            req("url_contains", Kind::Text),
            opt("kind", Kind::Choice(&["url", "headers", "get_param", "post_data"]), Some(r#""url""#)),
            opt("param", Kind::Text, None),
            req("save_to", Kind::VarOut),
        ],
    ),
    op("data.listen_response", "Listener Request Response", Shape::Plain, &[req("url_contains", Kind::Text), req("save_to", Kind::VarOut)]),
    op("data.listen_stop", "Stop Listener Request", Shape::Plain, &[]),
    op("data.cookies_get", "Get Page Cookies", Shape::Plain, &[req("save_to", Kind::VarOut)]),
    op("data.cookies_clear", "Clear Page Cookies", Shape::Plain, &[]),
    // Third-Party Tools
    op(
        "ext.openai",
        "OpenAI",
        Shape::Plain,
        &[
            req("api_key", Kind::Secret),
            opt("output", Kind::Choice(&["text", "image"]), Some(r#""text""#)),
            opt("model", Kind::Text, None),
            req("prompt", Kind::Text),
            opt("max_tokens", Kind::Int, Some("2000")),
            opt("size", Kind::Text, Some(r#""1024x1024""#)),
            opt("format", Kind::Choice(&["file", "b64_json"]), Some(r#""file""#)),
            opt("quality", Kind::Choice(&["auto", "low", "medium", "high", "xhigh", "max"]), Some(r#""auto""#)),
            req("save_to", Kind::VarOut),
        ],
    ),
    op(
        "ext.google_sheet",
        "Google Sheet",
        Shape::Plain,
        &[
            opt("operation", Kind::Choice(&["read", "write", "clear"]), Some(r#""read""#)),
            req("spreadsheet_id", Kind::Text),
            opt("sheet", Kind::Text, Some(r#""Sheet1""#)),
            opt("range", Kind::Text, None),
            opt("header", Kind::Bool, Some("true")),
            opt("data", Kind::Any, None),
            opt("save_to", Kind::VarOut, None),
        ],
    ),
    op(
        "ext.2captcha",
        "2Captcha",
        Shape::Plain,
        &[
            req("api_key", Kind::Secret),
            opt("kind", Kind::Choice(&["normal", "recaptcha_v2", "recaptcha_v3", "hcaptcha", "turnstile"]), Some(r#""recaptcha_v2""#)),
            opt("image", Kind::Target, None),
            opt("image_path", Kind::Text, None),
            opt("answer_target", Kind::Target, None),
            opt("phrase", Kind::Bool, Some("false")),
            opt("case_sensitive", Kind::Bool, Some("false")),
            opt("math", Kind::Bool, Some("false")),
            opt("numeric", Kind::Choice(captcha::NUMERIC), Some(r#""any""#)),
            opt("min_len", Kind::Int, Some("0")),
            opt("max_len", Kind::Int, Some("0")),
            opt("sitekey", Kind::Text, None),
            opt("action", Kind::Text, None),
            opt("min_score", Kind::Text, Some(r#""0.3""#)),
            opt("answer_to", Kind::VarOut, None),
            req("save_to", Kind::VarOut),
        ],
    ),
    // Profile Information
    op(
        "profile.remark",
        "Update Profile Remark",
        Shape::Plain,
        &[opt("mode", Kind::Choice(&["append", "replace"]), Some(r#""append""#)), req("text", Kind::Text)],
    ),
    op(
        "profile.tags",
        "Update Profile Tag",
        Shape::Plain,
        &[opt("mode", Kind::Choice(&["append", "replace"]), Some(r#""append""#)), req("tags", Kind::TextList)],
    ),
];

pub fn spec(id: &str) -> Option<&'static OpSpec> {
    OPS.iter().find(|op| op.id == id)
}

/// Ошибки `config` узла по реестру: (поле, текст).
pub fn check_config(spec: &OpSpec, config: &Map<String, Value>) -> Vec<(String, String)> {
    let mut errors = Vec::new();
    for key in config.keys().filter(|key| !spec.fields.iter().any(|field| field.name == key.as_str())) {
        errors.push((key.clone(), "Неизвестное поле".to_string()));
    }
    for field in spec.fields {
        match config.get(field.name).filter(|value| !value.is_null()) {
            None if field.required => errors.push((field.name.to_string(), "обязательное поле не заполнено".to_string())),
            None => {}
            Some(value) => {
                if let Err(message) = check_kind(field.kind, value) {
                    errors.push((field.name.to_string(), message));
                }
            }
        }
    }
    errors
}

fn check_kind(kind: Kind, value: &Value) -> Result<(), String> {
    let ok = match kind {
        Kind::Text | Kind::Code | Kind::Secret => value.is_string(),
        Kind::Int => is_int(value),
        Kind::IntRange => {
            is_int(value)
                || value.as_object().is_some_and(|range| {
                    range.len() == 2 && range.get("min").is_some_and(is_int) && range.get("max").is_some_and(is_int)
                })
        }
        Kind::Bool => value.is_boolean(),
        Kind::Choice(choices) => value.as_str().is_some_and(|v| choices.contains(&v)),
        Kind::VarOut => return var_out(value),
        Kind::VarIn => value.as_str().is_some_and(is_var_name),
        Kind::VarInList => value.as_array().is_some_and(|list| list.iter().all(|v| v.as_str().is_some_and(is_var_name))),
        Kind::Any => true,
        Kind::Process => value.as_i64().is_some_and(|id| id > 0),
        Kind::Inputs => value.as_object().is_some_and(|inputs| {
            inputs.iter().all(|(name, v)| is_var_name(name) && (v.is_string() || v.is_number() || v.is_boolean()))
        }),
        Kind::VarRows => return var_rows(value),
        Kind::Target => return check_target(value, true),
        Kind::Targets => return check_target(value, false),
        Kind::Mouse => return check_mouse(value),
        Kind::ChoiceList(choices) => value
            .as_array()
            .is_some_and(|list| !list.is_empty() && list.iter().all(|v| v.as_str().is_some_and(|v| choices.contains(&v)))),
        Kind::JsVarList => return js_names(value),
        Kind::TextList => value.as_array().is_some_and(|list| !list.is_empty() && list.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty()))),
    };
    if ok { Ok(()) } else { Err(expected(kind)) }
}

fn is_int(value: &Value) -> bool {
    value.as_i64().is_some() || value.as_str().is_some_and(|text| text.contains("${"))
}

fn var_out(value: &Value) -> Result<(), String> {
    let Some(name) = value.as_str() else { return Err(expected(Kind::VarOut)) };
    if !is_var_name(name) {
        Err(format!("неверное имя переменной «{name}»"))
    } else if SYSTEM.contains(&name) {
        Err(format!("{name} — системная переменная"))
    } else {
        Ok(())
    }
}

fn var_rows(value: &Value) -> Result<(), String> {
    let rows = value.as_array().ok_or_else(|| expected(Kind::VarRows))?;
    for row in rows {
        let row = row.as_object().ok_or_else(|| expected(Kind::VarRows))?;
        if let Some(key) = row.keys().find(|key| !["name", "type", "value"].contains(&key.as_str())) {
            return Err(format!("Неизвестное поле строки: {key}"));
        }
        var_out(row.get("name").unwrap_or(&Value::Null))?;
        serde_json::from_value::<VarType>(row.get("type").cloned().unwrap_or(Value::Null))
            .map_err(|_| "тип переменной: string, number, boolean, array, object, text, file или folder".to_string())?;
    }
    Ok(())
}

fn check_target(value: &Value, with_index: bool) -> Result<(), String> {
    let target = value.as_object().ok_or_else(|| expected(Kind::Target))?;
    if target.contains_key("element") && !with_index {
        return Err("В блоке «Цикл по элементам» укажите селектор".into());
    }
    if target.contains_key("element") {
        return if target.len() == 1 && target["element"].is_string() {
            Ok(())
        } else {
            Err("цель-элемент — только {element: \"${переменная}\"}".into())
        };
    }
    if let Some(key) = target.keys().find(|key| !["by", "selector", "frame"].contains(&key.as_str()) && !(with_index && key.as_str() == "index")) {
        return Err(format!("незнакомое поле цели: {key}"));
    }
    if !matches!(target.get("by").and_then(Value::as_str), Some("css" | "xpath" | "text")) {
        return Err("цель: by — css, xpath или text".into());
    }
    if !target.get("selector").and_then(Value::as_str).is_some_and(|selector| !selector.is_empty()) {
        return Err("цель: selector не заполнен".into());
    }
    if let Some(index) = target.get("index") {
        check_kind(Kind::IntRange, index).map_err(|_| "цель: index — целое или интервал {min, max}".to_string())?;
    }
    if target.get("frame").is_some_and(|frame| !frame.is_string()) {
        return Err("цель: frame — ${переменная} с элементом-фреймом".into());
    }
    Ok(())
}

fn check_mouse(value: &Value) -> Result<(), String> {
    let mouse = value.as_object().ok_or_else(|| expected(Kind::Mouse))?;
    let ints = ["x", "y"].iter().all(|key| mouse.get(*key).is_some_and(is_int));
    let release = mouse.get("release_ms").is_none_or(|v| check_kind(Kind::IntRange, v).is_ok());
    let known = mouse.keys().all(|key| ["x", "y", "release_ms"].contains(&key.as_str()));
    if ints && release && known { Ok(()) } else { Err(expected(Kind::Mouse)) }
}

fn js_names(value: &Value) -> Result<(), String> {
    let names = value.as_array().ok_or_else(|| expected(Kind::JsVarList))?;
    for name in names {
        let Some(name) = name.as_str().filter(|name| is_var_name(name)) else { return Err(expected(Kind::JsVarList)) };
        if JS_RESERVED.contains(&name) {
            return Err(format!("{name} — зарезервированное слово JavaScript. Выберите другое имя переменной."));
        }
    }
    Ok(())
}

fn expected(kind: Kind) -> String {
    match kind {
        Kind::Text | Kind::Code | Kind::Secret => "ожидается строка",
        Kind::Int => "ожидается целое число или ${переменная}",
        Kind::IntRange => "ожидается число или интервал {min, max}",
        Kind::Bool => "ожидается true или false",
        Kind::Choice(choices) => return format!("одно из: {}", choices.join(", ")),
        Kind::VarOut | Kind::VarIn => "ожидается имя переменной",
        Kind::VarInList => "ожидается список имён переменных",
        Kind::Any => "",
        Kind::Process => "Укажите идентификатор сценария",
        Kind::Inputs => "ожидается объект «переменная → значение»",
        Kind::VarRows => "ожидаются строки {name, type, value}",
        Kind::Target | Kind::Targets => "ожидается цель {by, selector} или {element}",
        Kind::Mouse => "ожидается {x, y, release_ms?} — целые или ${переменная}",
        Kind::ChoiceList(choices) => return format!("непустой список из: {}", choices.join(", ")),
        Kind::JsVarList => "ожидается список имён переменных",
        Kind::TextList => "ожидается непустой список строк",
    }
    .to_string()
}

/// Ошибка ссылки в секрете: без имени из скобок.
pub const BAD_SECRET_REF: &str = "неверная ссылка на переменную (буквальное «${» — «$${»)";

/// Поля узла с умолчаниями реестра.
pub struct Cfg<'a> {
    spec: &'static OpSpec,
    config: &'a Map<String, Value>,
}

impl<'a> Cfg<'a> {
    pub fn new(spec: &'static OpSpec, config: &'a Map<String, Value>) -> Self {
        Self { spec, config }
    }

    /// Поле как записано в процессе; нет — умолчание реестра; нет и его — `null`.
    pub fn raw(&self, name: &str) -> Value {
        if let Some(value) = self.config.get(name).filter(|value| !value.is_null()) {
            return value.clone();
        }
        self.spec
            .fields
            .iter()
            .find(|field| field.name == name)
            .and_then(|field| field.default)
            .map(|default| serde_json::from_str(default).expect("умолчание реестра — JSON"))
            .unwrap_or(Value::Null)
    }

    /// Перечисления и имена: без подстановки.
    pub fn string(&self, name: &str) -> String {
        self.raw(name).as_str().unwrap_or_default().to_string()
    }

    pub fn flag(&self, name: &str) -> bool {
        self.raw(name).as_bool().unwrap_or(false)
    }

    /// Имя переменной для записи; пусто — поле не задано.
    pub fn var_out(&self, name: &str) -> Option<String> {
        self.raw(name).as_str().filter(|text| !text.is_empty()).map(str::to_string)
    }

    /// Текст поля с подстановкой; элемент (поле целиком `${el}`) текстом не бывает. Ошибка подстановки в секрет — без
    /// его текста: имя в скобках может быть куском набранного пароля. Ссылка целиком (`${pw}`) — с именем.
    pub fn text(&self, scope: &Scope, name: &str) -> Result<String, String> {
        let raw = self.raw(name);
        let secret = self.spec.fields.iter().any(|field| field.name == name && field.kind == Kind::Secret);
        let value = scope.resolve(&raw).map_err(|e| match secret && raw.as_str().is_none_or(|text| vars::whole_ref(text).is_none()) {
            true => format!("поле «{name}»: {BAD_SECRET_REF}"),
            false => e,
        })?;
        if vars::element_index(&value).is_some() {
            return Err(format!("поле {name} — элемент, его нельзя вставить в текст"));
        }
        Ok(vars::to_text(&value))
    }

    pub fn int(&self, scope: &Scope, name: &str) -> Result<i64, String> {
        int_value(scope, &self.raw(name), name)
    }

    /// Фиксированное число или случайное из интервала `{min, max}`.
    pub fn int_range(&self, scope: &Scope, name: &str) -> Result<i64, String> {
        int_or_range(scope, &self.raw(name), name)
    }
}

fn int_of(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Целое из значения поля (число или строка с `${…}`); `name` — для текста ошибки.
pub(crate) fn int_value(scope: &Scope, value: &Value, name: &str) -> Result<i64, String> {
    int_of(&scope.resolve(value)?).ok_or_else(|| format!("поле {name}: ожидается целое число"))
}

/// Фиксированное целое или случайное из интервала `{min, max}`.
pub(crate) fn int_or_range(scope: &Scope, value: &Value, name: &str) -> Result<i64, String> {
    let int = |v: &Value| int_of(&scope.resolve(v)?).ok_or_else(|| format!("поле {name}: ожидается целое число или интервал"));
    match value {
        Value::Object(range) => vars::random_in(int(range.get("min").unwrap_or(&Value::Null))?, int(range.get("max").unwrap_or(&Value::Null))?),
        other => int(other),
    }
}

/// Строка журнала об узле: поля как записаны в процессе (без подстановки — правило секретов), длинные обрезаны;
/// секрет — только «задан».
pub fn describe(spec: &OpSpec, config: &Map<String, Value>) -> String {
    spec.fields
        .iter()
        .filter_map(|field| {
            let value = config.get(field.name).filter(|value| !value.is_null())?;
            Some(match field.kind {
                Kind::Code => format!("{}: {} симв.", field.name, value.as_str().map_or(0, |code| code.chars().count())),
                Kind::Secret => format!("{}: задан", field.name),
                _ => format!("{}={}", field.name, short(&vars::to_text(value))),
            })
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn short(text: &str) -> String {
    const MAX: usize = 60;
    if text.chars().count() <= MAX { text.to_string() } else { format!("{}…", text.chars().take(MAX).collect::<String>()) }
}

/// Ошибка операции. `BrowserClosed` и `Cancelled` политика ошибок не обрабатывает.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpError {
    Failed(String),
    BrowserClosed,
    Cancelled,
}

impl From<String> for OpError {
    fn from(message: String) -> Self {
        OpError::Failed(message)
    }
}

impl From<&str> for OpError {
    fn from(message: &str) -> Self {
        OpError::Failed(message.to_string())
    }
}

/// Оборванное соединение с браузером — браузер закрыт; остальное — ошибка узла.
impl From<CdpError> for OpError {
    fn from(error: CdpError) -> Self {
        match error {
            CdpError::Closed => OpError::BrowserClosed,
            other => OpError::Failed(other.to_string()),
        }
    }
}

/// Исполнение операции узла: куда идти дальше и хвост строки журнала (например, «→ x (string, 3)»).
pub async fn exec(ctx: &mut Ctx, spec: &'static OpSpec, node: &Node) -> Result<(Step, String), OpError> {
    let cfg = Cfg::new(spec, &node.config);
    match spec.id {
        "wait.time" => wait_time(ctx, &cfg).await,
        "wait.element" => wait::element(ctx, &cfg).await,
        "wait.request" | "data.listen_request" | "data.listen_response" | "data.listen_stop" | "data.cookies_get" | "data.cookies_clear" => {
            net::exec(ctx, spec.id, &cfg).await
        }
        "data.save_txt" | "data.import_txt" | "data.download" => files::exec(ctx, spec.id, &cfg).await,
        "data.save_excel" | "data.import_excel" => excel::exec(ctx, spec.id, &cfg).await,
        "data.email" => email::exec(ctx, &cfg).await,
        "ext.openai" => openai::exec(ctx, &cfg).await,
        "ext.google_sheet" => sheets::exec(ctx, &cfg).await,
        "ext.2captcha" => captcha::exec(ctx, &cfg).await,
        id if id.starts_with("data.") => data::exec(ctx, id, &cfg).await,
        id if id.starts_with("profile.") => profile::exec(ctx, id, &cfg).await,
        id if id.starts_with("proc.") => proc::exec(ctx, id, &cfg),
        id if id.starts_with("key.") => keys::exec(ctx, id, &cfg).await,
        id if id.starts_with("web.") => web::exec(ctx, id, &cfg).await,
        other => Err(OpError::Failed(format!("операция {other} не исполняется"))),
    }
}

async fn wait_time(ctx: &mut Ctx, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let ms = cfg.int_range(&ctx.scope, "ms")?;
    let pause = u64::try_from(ms).map_err(|_| "время ожидания отрицательное")?;
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(pause)) => Ok((Step::Port("next"), String::new())),
        _ = ctx.cancel.cancelled() => Err(OpError::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    fn map(value: serde_json::Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn registry_ids_are_unique_and_non_empty() {
        let mut seen = std::collections::HashSet::new();
        assert!(!OPS.is_empty());
        for op in OPS {
            assert!(!op.id.is_empty());
            assert!(seen.insert(op.id), "дубль операции {}", op.id);
            assert!(spec(op.id).is_some());
        }
    }

    #[test]
    fn registry_is_consistent() {
        let mut ids = HashSet::new();
        for op in OPS {
            assert!(ids.insert(op.id), "id повторяется: {}", op.id);
            for field in op.fields {
                if let Some(default) = field.default {
                    let value: Value = serde_json::from_str(default).unwrap_or_else(|_| panic!("{}.{}: умолчание не JSON", op.id, field.name));
                    assert_eq!(check_kind(field.kind, &value), Ok(()), "{}.{}", op.id, field.name);
                }
            }
        }
        assert!(spec("web.goto").is_some());
        assert!(spec("web.upload").is_some() && spec("data.save_excel").is_some() && spec("data.import_excel").is_some());
    }

    #[test]
    fn every_op_prefix_is_a_known_group() {
        // `group` относит незнакомый префикс к `ext`: опечатка в id молча увела бы операцию в «Внешние сервисы».
        for op in OPS {
            let prefix = op.id.split('.').next().unwrap_or_default();
            assert!(group(op.id) != "ext" || prefix == "ext", "{}: префикс {prefix} — не группа палитры", op.id);
        }
        assert_eq!(group("zzz.op"), "ext");
        assert_eq!([group("web.goto"), group("web.click"), group("data.download"), group("data.cookies_clear")], ["tabs", "web", "files", "web"]);
    }

    #[test]
    fn network_profile_and_browser_fields_are_checked() {
        let tags = spec("profile.tags").unwrap();
        assert!(check_config(tags, &map(json!({ "tags": ["VIP", "${t}"] }))).is_empty());
        assert!(!check_config(tags, &map(json!({ "tags": [] }))).is_empty());
        assert!(!check_config(tags, &map(json!({ "tags": [""] }))).is_empty());
        assert!(!check_config(tags, &map(json!({ "tags": "VIP" }))).is_empty());
        let browser = spec("control.new_browser").unwrap();
        assert_eq!(browser.shape, Shape::Loop, "у New Browser входы и выходы цикла");
        assert!(check_config(browser, &map(json!({ "serial": "${n}" }))).is_empty());
        assert!(!check_config(browser, &map(json!({ "serial": 3, "after": "close" }))).is_empty());
        let listen = spec("data.listen_request").unwrap();
        assert!(check_config(listen, &map(json!({ "url_contains": "api", "kind": "get_param", "param": "k", "save_to": "v" }))).is_empty());
        assert!(!check_config(spec("data.listen_response").unwrap(), &map(json!({ "url_contains": "api" }))).is_empty(), "save_to обязателен");
        assert!(check_config(spec("data.listen_stop").unwrap(), &map(json!({}))).is_empty());
        let upload = spec("web.upload").unwrap();
        assert!(check_config(upload, &map(json!({ "target": { "by": "css", "selector": "#f" }, "source": "url", "url": "${u}" }))).is_empty());
    }

    #[test]
    fn targets_and_new_kinds_are_checked() {
        let click = spec("web.click").unwrap();
        assert!(check_config(click, &map(json!({ "target": { "by": "css", "selector": "#b", "index": { "min": 1, "max": "${n}" } } }))).is_empty());
        assert!(check_config(click, &map(json!({ "target": { "element": "${el}" } }))).is_empty());
        assert!(check_config(click, &map(json!({ "target": { "by": "css", "selector": "#b", "frame": "${f}" }, "use_mouse": { "x": 5, "y": 6, "release_ms": { "min": 50, "max": 90 } } }))).is_empty());
        let bad = |target: serde_json::Value| check_config(click, &map(json!({ "target": target })));
        assert!(bad(json!({ "by": "id", "selector": "x" })).iter().any(|(_, m)| m.contains("css, xpath или text")));
        assert!(bad(json!({ "by": "css", "selector": "" })).iter().any(|(_, m)| m.contains("selector")));
        assert!(bad(json!({ "element": "${el}", "by": "css" })).iter().any(|(_, m)| m.contains("цель-элемент")));
        assert!(bad(json!({ "by": "css", "selector": "a", "colour": 1 })).iter().any(|(_, m)| m.contains("colour")));
        let mouse = check_config(click, &map(json!({ "target": { "element": "${e}" }, "use_mouse": { "x": "a" } })));
        assert!(mouse.iter().any(|(f, _)| f == "use_mouse"), "{mouse:?}");
        let each = spec("control.for_elements").unwrap();
        assert!(check_config(each, &map(json!({ "target": { "by": "css", "selector": "li" } }))).is_empty());
        assert!(!check_config(each, &map(json!({ "target": { "by": "css", "selector": "li", "index": 2 } }))).is_empty(), "у For Loop Elements нет index");
        let attribute = json!({ "target": { "by": "css", "selector": "a" }, "kind": "attribute", "attribute": "href" });
        assert!(check_config(each, &map(attribute)).is_empty(), "поля видов извлечения");
        assert!(check_config(each, &map(json!({ "target": { "by": "css", "selector": "li" }, "kind": "child", "child_selector": "span" }))).is_empty());
        let stored = check_config(each, &map(json!({ "target": { "element": "${el}" } })));
        assert_eq!(stored, [("target".to_string(), "В блоке «Цикл по элементам» укажите селектор".to_string())]);
        let combo = spec("key.combo").unwrap();
        assert!(check_config(combo, &map(json!({ "modifiers": ["cmd", "shift"], "key": "z" }))).is_empty());
        assert!(!check_config(combo, &map(json!({ "modifiers": [], "key": "z" }))).is_empty());
        assert!(!check_config(combo, &map(json!({ "modifiers": ["super"], "key": "z" }))).is_empty());
        let js = spec("web.js").unwrap();
        let errors = check_config(js, &map(json!({ "code": "1", "inject": ["class"] })));
        assert!(errors.iter().any(|(f, m)| f == "inject" && m.contains("class")), "{errors:?}");
        let press = spec("key.press").unwrap();
        assert!(check_config(press, &map(json!({ "key": "Enter" }))).is_empty());
        assert!(!check_config(press, &map(json!({ "key": "Win" }))).is_empty());
    }

    #[test]
    fn nested_ints_resolve_like_fields() {
        let mut scope = Scope::new(Default::default());
        scope.set("n", json!("7")).unwrap();
        assert_eq!(int_value(&scope, &json!("${n}"), "x"), Ok(7));
        assert_eq!(int_value(&scope, &json!("q"), "x"), Err("поле x: ожидается целое число".into()));
        let bad = Err("поле x: ожидается целое число или интервал".to_string());
        assert_eq!(int_or_range(&scope, &json!("q"), "x"), bad);
        assert_eq!(int_or_range(&scope, &json!({ "min": "q", "max": 3 }), "x"), bad);
        for _ in 0..20 {
            assert!((3..=7).contains(&int_or_range(&scope, &json!({ "min": 3, "max": "${n}" }), "x").unwrap()));
        }
    }

    #[test]
    fn config_is_checked_by_field_kinds() {
        let goto = spec("web.goto").unwrap();
        assert!(check_config(goto, &map(json!({ "url": "${site}" }))).is_empty());
        let errors = check_config(goto, &map(json!({ "timeout_ms": "abc", "colour": 1 })));
        assert!(errors.contains(&("colour".into(), "Неизвестное поле".into())), "{errors:?}");
        assert!(errors.contains(&("url".into(), "обязательное поле не заполнено".into())), "{errors:?}");
        assert!(errors.iter().any(|(f, m)| f == "timeout_ms" && m.contains("целое")), "{errors:?}");
        let wait = spec("wait.time").unwrap();
        assert!(check_config(wait, &map(json!({ "ms": { "min": 1, "max": "${hi}" } }))).is_empty());
        assert!(!check_config(wait, &map(json!({ "ms": { "min": 1 } }))).is_empty());
        let regex = spec("proc.regex").unwrap();
        let errors = check_config(regex, &map(json!({ "data": "x", "pattern": "\\d", "save_to": "password" })));
        assert!(errors.iter().any(|(f, m)| f == "save_to" && m.contains("системная")), "{errors:?}");
        let vars_op = spec("proc.vars").unwrap();
        assert!(check_config(vars_op, &map(json!({ "vars": [{ "name": "a", "type": "number", "value": "1" }] }))).is_empty());
        assert!(!check_config(vars_op, &map(json!({ "vars": [{ "name": "a", "type": "date" }] }))).is_empty());
        let switch = spec("web.switch_tab").unwrap();
        let errors = check_config(switch, &map(json!({ "by": "host", "value": "x" })));
        assert!(errors.iter().any(|(f, m)| f == "by" && m == "одно из: url, title"), "{errors:?}");
    }

    #[test]
    fn cfg_reads_defaults_and_resolves() {
        let mut scope = Scope::new(Default::default());
        scope.set("t", json!("2500")).unwrap();
        scope.set("lo", json!(3)).unwrap();
        let goto = spec("web.goto").unwrap();
        let config = map(json!({ "url": "https://a.test/${t}" }));
        let cfg = Cfg::new(goto, &config);
        assert_eq!(cfg.int(&scope, "timeout_ms").unwrap(), 30000, "умолчание реестра");
        assert_eq!(cfg.text(&scope, "url").unwrap(), "https://a.test/2500");
        let config = map(json!({ "url": "x", "timeout_ms": "${t}" }));
        assert_eq!(Cfg::new(goto, &config).int(&scope, "timeout_ms").unwrap(), 2500, "число из строки переменной");
        let wait = spec("wait.time").unwrap();
        let config = map(json!({ "ms": { "min": "${lo}", "max": 5 } }));
        for _ in 0..50 {
            assert!((3..=5).contains(&Cfg::new(wait, &config).int_range(&scope, "ms").unwrap()));
        }
        let switch = spec("web.switch_tab").unwrap();
        let config = map(json!({ "value": "x" }));
        assert_eq!((Cfg::new(switch, &config).string("by"), Cfg::new(switch, &config).string("match")), ("url".into(), "eq".into()));
        scope.set("el", vars::element_value(0)).unwrap();
        let config = map(json!({ "value": "${el}" }));
        assert_eq!(Cfg::new(switch, &config).text(&scope, "value"), Err("поле value — элемент, его нельзя вставить в текст".into()));
    }

    #[test]
    fn secret_field_errors_never_quote_the_typed_text() {
        let mut scope = Scope::new(Default::default());
        let email = spec("data.email").unwrap();
        for typed in ["Ab${12", "x${a-b}", "x${qz}y"] {
            let config = map(json!({ "password": typed }));
            let err = Cfg::new(email, &config).text(&scope, "password").unwrap_err();
            assert_eq!(err, "поле «password»: неверная ссылка на переменную (буквальное «${» — «$${»)", "{typed}");
        }
        let config = map(json!({ "password": "${pw}" }));
        assert_eq!(Cfg::new(email, &config).text(&scope, "password").unwrap_err(), "переменная pw не задана", "ссылка целиком — с именем");
        scope.set("pw", json!("p")).unwrap();
        let config = map(json!({ "password": "a$${b}${pw}" }));
        assert_eq!(Cfg::new(email, &config).text(&scope, "password").unwrap(), "a${b}p");
    }

    #[test]
    fn page_texts_reach_users_without_the_cdp_prefix() {
        assert_eq!(OpError::from(CdpError::Page("Элемент больше не доступен на странице".into())), OpError::Failed("Элемент больше не доступен на странице".into()));
        assert_eq!(OpError::from(CdpError::Script("Error: boom".into())), OpError::Failed("Error: boom".into()));
        assert_eq!(OpError::from(CdpError::Remote("x".into())), OpError::Failed("Браузер сообщил об ошибке: x".into()));
    }

    #[test]
    fn describe_keeps_templates_and_shortens_by_characters() {
        let goto = spec("web.goto").unwrap();
        assert_eq!(describe(goto, &map(json!({ "url": "${site}/вход", "timeout_ms": 5000 }))), "url=${site}/вход, timeout_ms=5000");
        let long = "я".repeat(70);
        assert_eq!(describe(goto, &map(json!({ "url": long }))), format!("url={}…", "я".repeat(60)), "обрезка по символам, не байтам");
        let js = spec("web.js").unwrap();
        assert_eq!(describe(js, &map(json!({ "code": "return 1;", "save_to": "r" }))), "code: 9 симв., save_to=r");
        let totp = spec("data.totp").unwrap();
        assert_eq!(describe(totp, &map(json!({ "secret": "JBSWY3DPEHPK3PXP", "save_to": "c" }))), "secret: задан, save_to=c", "ключ 2FA в журнал не попадает");
        assert!(check_config(totp, &map(json!({ "secret": "${key}", "save_to": "c" }))).is_empty());
        assert!(!check_config(totp, &map(json!({ "secret": 7, "save_to": "c" }))).is_empty());
    }
}
