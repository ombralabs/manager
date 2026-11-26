//! Переменные запуска: значения, подстановка `${…}`, системные переменные, условия If и While
//! (спецификация автоматизации, «Переменные»).
use crate::flow::model::{VarDecl, VarType};
use serde_json::{Map, Number, Value};
use std::collections::HashMap;

/// Системные переменные: восемь из документации AdsPower и `last_error`. Только для чтения.
pub const SYSTEM: [&str; 9] =
    ["task_id", "task_name", "serial_number", "browser_name", "acc_id", "comment", "user_name", "password", "last_error"];

/// Условия узлов Statement if и While Loop.
pub const CONDITIONS: &[&str] =
    &["exist", "not_exist", "lt", "le", "gt", "ge", "eq", "ne", "contains", "not_contains", "one_of", "not_one_of"];

/// Ключ значения-ссылки на элемент страницы `{"$ombra_element": n}`: n — номер в элементах запуска.
pub const ELEMENT_KEY: &str = "$ombra_element";

/// Значение-ссылка на элемент запуска с номером `index`.
pub fn element_value(index: usize) -> Value {
    let mut map = Map::new();
    map.insert(ELEMENT_KEY.to_string(), Value::from(index));
    Value::Object(map)
}

/// Номер элемента, если значение — ссылка на элемент.
pub fn element_index(value: &Value) -> Option<usize> {
    let map = value.as_object().filter(|map| map.len() == 1)?;
    map.get(ELEMENT_KEY)?.as_u64().map(|index| index as usize)
}

/// Значение для итога и экспорта: ссылки на элементы — строкой `"<element>"`, вне запуска они ничего не значат.
pub fn public(value: Value) -> Value {
    if element_index(&value).is_some() {
        return Value::String("<element>".into());
    }
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(public).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(key, item)| (key, public(item))).collect()),
        other => other,
    }
}

/// Имя переменной: латиница, цифры и `_`, не с цифры (правило AdsPower).
pub fn is_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic()) && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Переменные одного процесса: свои и системные.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    vars: HashMap<String, Value>,
    system: HashMap<&'static str, Value>,
}

impl Scope {
    /// `system` — значения системных переменных запуска; незнакомые имена отбрасываются.
    pub fn new(system: HashMap<&'static str, Value>) -> Self {
        Self { vars: HashMap::new(), system: system.into_iter().filter(|(name, _)| SYSTEM.contains(name)).collect() }
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.vars.get(name).or_else(|| self.system.get(name))
    }

    /// Запись процессом; системную переменную задать нельзя.
    pub fn set(&mut self, name: &str, value: Value) -> Result<(), String> {
        if SYSTEM.contains(&name) {
            return Err(format!("{name} — системная переменная, её нельзя задать"));
        }
        self.vars.insert(name.to_string(), value);
        Ok(())
    }

    /// `last_error` пишет только интерпретатор — для ветки `error`.
    pub fn set_last_error(&mut self, message: &str) {
        self.system.insert("last_error", Value::String(message.to_string()));
    }

    /// Пространство подпроцесса: те же системные переменные, своих нет.
    pub fn child(&self) -> Self {
        Self { vars: HashMap::new(), system: self.system.clone() }
    }

    /// Поле с подстановкой: строка целиком `${name}` — значение своего типа, иначе текст; массивы и объекты — поэлементно.
    pub fn resolve(&self, field: &Value) -> Result<Value, String> {
        match field {
            Value::String(text) => match whole_ref(text) {
                Some(name) => self.get(name).cloned().ok_or_else(|| unset(name)),
                None => self.render(text).map(Value::String),
            },
            Value::Array(items) => items.iter().map(|item| self.resolve(item)).collect::<Result<Vec<_>, _>>().map(Value::Array),
            Value::Object(map) => map
                .iter()
                .map(|(key, item)| Ok((key.clone(), self.resolve(item)?)))
                .collect::<Result<Map<_, _>, String>>()
                .map(Value::Object),
            other => Ok(other.clone()),
        }
    }

    /// Текст с подставленными значениями; `$${` — буквальное `${`.
    pub fn render(&self, text: &str) -> Result<String, String> {
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find('$') {
            out.push_str(&rest[..at]);
            rest = &rest[at..];
            if let Some(tail) = rest.strip_prefix("$${") {
                out.push_str("${");
                rest = tail;
            } else if let Some(tail) = rest.strip_prefix("${") {
                let end = tail.find('}').ok_or_else(|| format!("незакрытая ссылка на переменную в «{text}»"))?;
                let name = &tail[..end];
                if !is_var_name(name) {
                    return Err(format!("неверное имя переменной «{name}»"));
                }
                let value = self.get(name).ok_or_else(|| unset(name))?;
                if element_index(value).is_some() {
                    return Err(format!("переменная {name} — элемент, его нельзя вставить в текст"));
                }
                out.push_str(&to_text(value));
                rest = &tail[end + 1..];
            } else {
                out.push('$');
                rest = &rest[1..];
            }
        }
        out.push_str(rest);
        Ok(out)
    }
}

/// Имена из `${…}` в тексте; `$${` — не ссылка.
pub fn refs(text: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        rest = &rest[at..];
        if let Some(tail) = rest.strip_prefix("$${") {
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("${") {
            let Some(end) = tail.find('}') else { break };
            names.push(&tail[..end]);
            rest = &tail[end + 1..];
        } else {
            rest = &rest[1..];
        }
    }
    names
}

pub(crate) fn whole_ref(text: &str) -> Option<&str> {
    let name = text.strip_prefix("${")?.strip_suffix('}')?;
    is_var_name(name).then_some(name)
}

fn unset(name: &str) -> String {
    format!("переменная {name} не задана")
}

/// Значение текстом: строка как есть, `null` — пусто, остальное — компактный JSON.
pub fn to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Значение к типу переменной; `null` — пустое значение типа. Строки разбираются: число, `true`/`false`, JSON.
pub fn coerce(ty: VarType, value: Value) -> Result<Value, String> {
    use VarType::*;
    match (ty, value) {
        (_, Value::Null) => Ok(empty(ty)),
        (String | Text | File | Folder, v @ (Value::String(_) | Value::Number(_) | Value::Bool(_))) => Ok(Value::String(to_text(&v))),
        (Number, v @ Value::Number(_)) | (Boolean, v @ Value::Bool(_)) | (Array, v @ Value::Array(_)) | (Object, v @ Value::Object(_)) => Ok(v),
        (Number, Value::String(text)) => number(&text).ok_or_else(|| expected(ty)),
        (Boolean, Value::String(text)) => match text.trim() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(expected(ty)),
        },
        (Array | Object, Value::String(text)) => match serde_json::from_str::<Value>(&text) {
            Ok(v @ Value::Array(_)) if ty == Array => Ok(v),
            Ok(v @ Value::Object(_)) if ty == Object => Ok(v),
            _ => Err(expected(ty)),
        },
        _ => Err(expected(ty)),
    }
}

fn empty(ty: VarType) -> Value {
    match ty {
        VarType::Number => Value::from(0),
        VarType::Boolean => Value::Bool(false),
        VarType::Array => Value::Array(Vec::new()),
        VarType::Object => Value::Object(Map::new()),
        _ => Value::String(String::new()),
    }
}

fn expected(ty: VarType) -> String {
    match ty {
        VarType::Number => "ожидается число",
        VarType::Boolean => "ожидается true или false",
        VarType::Array => "ожидается массив JSON",
        VarType::Object => "ожидается объект JSON",
        _ => "ожидается строка",
    }
    .to_string()
}

fn number(text: &str) -> Option<Value> {
    let text = text.trim();
    text.parse::<i64>().ok().map(Value::from).or_else(|| text.parse::<f64>().ok().and_then(Number::from_f64).map(Value::Number))
}

/// Переменные процесса: умолчания, перекрытые `values`. Имя без объявления или системное — ошибка.
pub fn declare(scope: &mut Scope, decls: &[VarDecl], mut values: Map<String, Value>) -> Result<(), String> {
    for decl in decls {
        let value = values.remove(&decl.name).unwrap_or_else(|| decl.default.clone());
        let value = coerce(decl.ty, value).map_err(|e| format!("переменная {}: {e}", decl.name))?;
        scope.set(&decl.name, value)?;
    }
    match values.keys().next() {
        Some(name) if SYSTEM.contains(&name.as_str()) => Err(format!("{name} — системная переменная, её нельзя задать")),
        Some(name) => Err(format!("Переменная {name} не объявлена в сценарии")),
        None => Ok(()),
    }
}

/// Условие If и While: переменная `name` (её значение — `left`) против `right` (спецификация, таблица условий).
pub fn check(name: &str, condition: &str, left: Option<&Value>, right: &Value) -> Result<bool, String> {
    let filled = left.is_some_and(|v| !v.is_null() && v.as_str() != Some(""));
    match condition {
        "exist" => return Ok(filled),
        "not_exist" => return Ok(!filled),
        _ => {}
    }
    let left = left.ok_or_else(|| unset(name))?;
    Ok(match condition {
        "lt" | "le" | "gt" | "ge" => {
            // Целые сравниваются как целые: f64 не различает числа больше 2^53.
            if let (Some(x), Some(y)) = (as_integer(left), as_integer(right)) {
                let order = x.cmp(&y);
                return Ok(match condition {
                    "lt" => order.is_lt(),
                    "le" => order.is_le(),
                    "gt" => order.is_gt(),
                    _ => order.is_ge(),
                });
            }
            let (Some(a), Some(b)) = (as_number(left), as_number(right)) else {
                return Err(format!("условие {condition}: {name} и значение должны быть числами"));
            };
            match condition {
                "lt" => a < b,
                "le" => a <= b,
                "gt" => a > b,
                _ => a >= b,
            }
        }
        "eq" => equal(left, right),
        "ne" => !equal(left, right),
        "contains" | "not_contains" => {
            let found = match left {
                Value::String(text) => text.contains(&to_text(right)),
                Value::Array(items) => items.iter().any(|item| equal(item, right)),
                _ => return Err(format!("условие {condition}: {name} — не строка и не массив")),
            };
            found == (condition == "contains")
        }
        "one_of" | "not_one_of" => {
            let found = to_text(right).split(',').map(str::trim).any(|part| equal(left, &Value::String(part.to_string())));
            found == (condition == "one_of")
        }
        other => return Err(format!("неизвестное условие {other}")),
    })
}

fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        // «nan», «inf», «infinity» разбираются в f64, но числами в процессе не считаются.
        Value::String(text) => text.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => None,
    }
}

fn as_integer(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Целые — как целые (f64 путает числа больше 2^53), прочие числа — как f64, остальное — по текстовой записи.
fn equal(a: &Value, b: &Value) -> bool {
    if let (Some(x), Some(y)) = (as_integer(a), as_integer(b)) {
        return x == y;
    }
    match (as_number(a), as_number(b)) {
        (Some(x), Some(y)) => x == y,
        _ => to_text(a) == to_text(b),
    }
}

/// Случайное целое из `[min, max]` включительно.
pub fn random_in(min: i64, max: i64) -> Result<i64, String> {
    if min > max {
        return Err("Минимальное значение интервала больше максимального".to_string());
    }
    let span = (max as i128 - min as i128 + 1) as u128;
    let roll = getrandom::u64().expect("нет системного источника случайности") as u128 % span;
    Ok((min as i128 + roll as i128) as i64)
}

/// Случайный индекс в `0..len`; `len` > 0.
pub fn random_index(len: usize) -> usize {
    (getrandom::u64().expect("нет системного источника случайности") % len as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scope() -> Scope {
        let mut scope = Scope::new(HashMap::from([("acc_id", json!("k1")), ("password", json!("pw"))]));
        scope.set("site", json!("https://a.test")).unwrap();
        scope.set("n", json!(5)).unwrap();
        scope.set("list", json!(["x", "y"])).unwrap();
        scope.set("empty", Value::Null).unwrap();
        scope
    }

    fn decls(value: Value) -> Vec<VarDecl> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn names_follow_the_adspower_rule() {
        for good in ["a", "_x", "a_1", "Name2"] {
            assert!(is_var_name(good), "{good}");
        }
        for bad in ["", "1a", "a-b", "имя", "a b"] {
            assert!(!is_var_name(bad), "{bad}");
        }
    }

    #[test]
    fn whole_reference_keeps_the_type() {
        let s = scope();
        assert_eq!(s.resolve(&json!("${list}")).unwrap(), json!(["x", "y"]));
        assert_eq!(s.resolve(&json!("${n}")).unwrap(), json!(5));
        assert_eq!(s.resolve(&json!("${acc_id}")).unwrap(), json!("k1"), "системная переменная");
        assert_eq!(s.resolve(&json!(7)).unwrap(), json!(7), "не строка — как есть");
    }

    #[test]
    fn references_inside_text_become_text() {
        let s = scope();
        assert_eq!(s.render("${site}/p?n=${n}&l=${list}&e=${empty}").unwrap(), "https://a.test/p?n=5&l=[\"x\",\"y\"]&e=");
        assert_eq!(s.render("$${site} и ${n}$").unwrap(), "${site} и 5$", "$${{ — буквальное ${{");
    }

    #[test]
    fn bad_references_are_errors() {
        let s = scope();
        assert_eq!(s.render("${nope}").unwrap_err(), "переменная nope не задана");
        assert!(s.render("a ${site").unwrap_err().contains("незакрытая ссылка"));
        assert!(s.render("${a-b}").unwrap_err().contains("неверное имя переменной"));
    }

    #[test]
    fn nested_values_are_resolved() {
        assert_eq!(scope().resolve(&json!({ "u": ["${site}", 1] })).unwrap(), json!({ "u": ["https://a.test", 1] }));
    }

    #[test]
    fn refs_lists_names_without_literals() {
        assert_eq!(refs("${a} $${b} ${c}x ${"), vec!["a", "c"]);
    }

    #[test]
    fn system_variables_are_read_only() {
        let mut s = scope();
        assert_eq!(s.set("password", json!("x")).unwrap_err(), "password — системная переменная, её нельзя задать");
        s.set_last_error("boom");
        assert_eq!(s.get("last_error"), Some(&json!("boom")));
        let child = s.child();
        assert!(child.get("site").is_none(), "у подпроцесса своё пространство");
        assert_eq!(child.get("acc_id"), Some(&json!("k1")), "системные — те же");
    }

    #[test]
    fn coerce_parses_strings_by_type() {
        assert_eq!(coerce(VarType::Number, json!("7")), Ok(json!(7)));
        assert_eq!(coerce(VarType::Number, json!(" 2.5 ")), Ok(json!(2.5)));
        assert_eq!(coerce(VarType::Number, json!("x")), Err("ожидается число".into()));
        assert_eq!(coerce(VarType::Boolean, json!("true")), Ok(json!(true)));
        assert_eq!(coerce(VarType::Array, json!("[1,2]")), Ok(json!([1, 2])));
        assert_eq!(coerce(VarType::Array, json!("{}")), Err("ожидается массив JSON".into()));
        assert_eq!(coerce(VarType::Object, json!({ "a": 1 })), Ok(json!({ "a": 1 })));
        assert_eq!(coerce(VarType::String, json!(5)), Ok(json!("5")));
        assert_eq!(coerce(VarType::Text, json!([1])), Err("ожидается строка".into()));
        assert_eq!(coerce(VarType::Folder, Value::Null), Ok(json!("")), "null — пустое значение типа");
        assert_eq!(coerce(VarType::Number, Value::Null), Ok(json!(0)));
        assert_eq!(coerce(VarType::Array, Value::Null), Ok(json!([])));
    }

    #[test]
    fn declare_layers_values_over_defaults() {
        let list = decls(json!([{ "name": "a", "type": "number", "default": 1 }, { "name": "b", "type": "string", "default": "x" }]));
        let mut s = Scope::new(HashMap::new());
        declare(&mut s, &list, Map::from_iter([("a".to_string(), json!("7"))])).unwrap();
        assert_eq!((s.get("a"), s.get("b")), (Some(&json!(7)), Some(&json!("x"))));
        let mut s = Scope::new(HashMap::new());
        assert_eq!(declare(&mut s, &list, Map::from_iter([("c".to_string(), json!(1))])).unwrap_err(), "Переменная c не объявлена в сценарии");
        assert_eq!(
            declare(&mut s, &list, Map::from_iter([("password".to_string(), json!(1))])).unwrap_err(),
            "password — системная переменная, её нельзя задать"
        );
        assert_eq!(declare(&mut s, &list, Map::from_iter([("a".to_string(), json!("q"))])).unwrap_err(), "переменная a: ожидается число");
    }

    #[test]
    fn comparisons_ignore_non_finite_numbers_and_keep_big_integers_exact() {
        let eq = |a: &str, b: &str| check("v", "eq", Some(&json!(a)), &json!(b)).unwrap();
        assert!(eq("NaN", "NaN"), "текст равен тексту");
        assert!(!eq("inf", "infinity"));
        assert!(!eq("12345678901234567", "12345678901234568"));
        assert!(eq("12345678901234567", "12345678901234567"));
        assert!(eq("1.0", "1"));
    }

    #[test]
    fn conditions_follow_the_spec_table() {
        let n = json!(5);
        let word = json!("кофе с молоком");
        let list = json!(["a", 2]);
        assert!(check("x", "exist", Some(&n), &Value::Null).unwrap());
        assert!(check("x", "not_exist", None, &Value::Null).unwrap());
        assert!(check("x", "not_exist", Some(&json!("")), &Value::Null).unwrap(), "пустая строка — не задана");
        assert!(check("x", "lt", Some(&n), &json!("10")).unwrap());
        assert!(check("x", "ge", Some(&json!("5")), &json!(5)).unwrap());
        assert!(check("x", "lt", Some(&word), &json!(1)).unwrap_err().contains("числами"));
        assert!(check("x", "lt", Some(&json!(9007199254740992_i64)), &json!("9007199254740993")).unwrap(), "целые точно");
        assert!(!check("x", "ge", Some(&json!(9007199254740992_i64)), &json!(9007199254740993_i64)).unwrap());
        assert!(check("x", "eq", Some(&json!("5.0")), &json!(5)).unwrap(), "числа сравниваются как числа");
        assert!(check("x", "ne", Some(&word), &json!("чай")).unwrap());
        assert!(check("x", "contains", Some(&word), &json!("молок")).unwrap());
        assert!(check("x", "contains", Some(&list), &json!("2")).unwrap(), "элемент массива по правилу eq");
        assert!(check("x", "not_contains", Some(&word), &json!("чай")).unwrap());
        assert!(check("x", "one_of", Some(&json!("b")), &json!("a, b ,c")).unwrap());
        assert!(check("x", "not_one_of", Some(&json!("d")), &json!("a,b")).unwrap());
        assert_eq!(check("x", "eq", None, &n).unwrap_err(), "переменная x не задана");
    }

    #[test]
    fn random_stays_in_range() {
        for _ in 0..200 {
            assert!((3..=5).contains(&random_in(3, 5).unwrap()));
            assert!(random_index(3) < 3);
        }
        assert_eq!(random_in(4, 4).unwrap(), 4);
        assert!(random_in(5, 3).is_err());
        assert!(random_in(i64::MIN, i64::MAX).is_ok(), "весь диапазон без переполнения");
    }

    #[test]
    fn element_values_are_references_not_text() {
        let el = element_value(3);
        assert_eq!(el, json!({ "$ombra_element": 3 }));
        assert_eq!(element_index(&el), Some(3));
        assert_eq!(element_index(&json!({ "$ombra_element": 3, "x": 1 })), None, "лишние ключи — обычный объект");
        assert_eq!(element_index(&json!("$ombra_element")), None);
        let mut s = scope();
        s.set("el", el.clone()).unwrap();
        assert_eq!(s.resolve(&json!("${el}")).unwrap(), el, "целиком — ссылка как есть");
        assert_eq!(s.render("клик по ${el}").unwrap_err(), "переменная el — элемент, его нельзя вставить в текст");
    }

    #[test]
    fn public_values_hide_element_references() {
        let value = json!({ "a": element_value(0), "b": [1, element_value(2)], "c": "x" });
        assert_eq!(public(value), json!({ "a": "<element>", "b": [1, "<element>"], "c": "x" }));
    }
}
