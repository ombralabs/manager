//! Параметры запуска на профиле (основная спецификация, «Задачи и параметры»; спецификация А2): значения задачи поверх
//! умолчаний процесса и строка таблицы на профиль поверх них. Ошибки — до запуска первого браузера.
use crate::flow::model::{Flow, VarDecl, VarType};
use crate::flow::vars::{self, SYSTEM, Scope};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Строка таблицы и значения параметров: имя переменной → значение.
pub type Row = Map<String, Value>;

/// Колонки, по которым строка находит свой профиль; переменными они не становятся.
const KEYS: [&str; 2] = ["serial_number", "acc_id"];

/// Старые задачи запускают процесс на каждом профиле; очередь выдаёт каждый элемент одному профилю.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Distribution {
    #[default]
    PerProfile,
    Queue,
}

impl Distribution {
    pub fn as_str(self) -> &'static str {
        match self { Self::PerProfile => "per_profile", Self::Queue => "queue" }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text { "per_profile" => Some(Self::PerProfile), "queue" => Some(Self::Queue), _ => None }
    }
}

/// Порядок профилей в пакете.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    #[default]
    Ordered,
    Random,
}

impl Order {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ordered => "ordered",
            Self::Random => "random",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "ordered" => Some(Self::Ordered),
            "random" => Some(Self::Random),
            _ => None,
        }
    }
}

/// Нет строки таблицы для профиля: пропустить запуск или взять случайную строку.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingRow {
    #[default]
    Skip,
    Random,
}

impl MissingRow {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Random => "random",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "skip" => Some(Self::Skip),
            "random" => Some(Self::Random),
            _ => None,
        }
    }
}

/// Таблица на профиль: строки и колонка, по которой строка находит профиль (`None` — по порядку выбора).
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    rows: Vec<Row>,
    key: Option<&'static str>,
}

impl Table {
    /// Общая очередь: ссылки сравниваются целиком без пробелов по краям; без колонки ссылок — вся строка.
    /// Привязки к профилям нет, одинаковые задания оставляем в одном экземпляре.
    pub fn queue(rows: Vec<Row>) -> Result<Self, String> {
        if rows.is_empty() { return Err("таблица пустая".into()); }
        let link = ["url", "search_url", "offer_url", "link"].into_iter()
            .find(|key| rows.iter().any(|row| row.contains_key(*key)));
        if let Some(key) = link {
            for (index, row) in rows.iter().enumerate() {
                if !row.get(key).and_then(Value::as_str).is_some_and(|v| !v.trim().is_empty()) {
                    return Err(format!("очередь, строка {}: в колонке {key} нужна непустая ссылка", index + 1));
                }
            }
        }
        let mut seen = HashSet::new();
        let rows = rows.into_iter().filter(|row| {
            let key = match link {
                Some(key) => row[key].as_str().unwrap().trim().to_string(),
                None => serde_json::to_string(&row.iter().collect::<std::collections::BTreeMap<_, _>>()).expect("JSON строки"),
            };
            seen.insert(key)
        }).collect();
        Ok(Self { rows, key: link })
    }

    /// Пустая таблица и две строки на один профиль (повтор в любой из ключевых колонок) — ошибки. Номер строки —
    /// среди строк данных, без заголовка.
    pub fn new(rows: Vec<Row>) -> Result<Self, String> {
        if rows.is_empty() {
            return Err("таблица пустая".into());
        }
        for key in KEYS {
            let mut seen = HashSet::new();
            for (i, row) in rows.iter().enumerate() {
                // Строка без ключа не находит профиля.
                let Some(value) = row.get(key).map(key_text) else { continue };
                if !seen.insert(value.clone()) {
                    return Err(format!("таблица: {key} {value} повторяется (строка {})", i + 1));
                }
            }
        }
        let key = KEYS.into_iter().find(|key| rows.iter().any(|row| row.contains_key(*key)));
        Ok(Self { rows, key })
    }

    /// Ключевая колонка таблицы; `None` — строки берутся по порядку выбора.
    pub fn key(&self) -> Option<&'static str> {
        self.key
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Строка профиля: по ключевой колонке, иначе `index`-я — по месту профиля в выборе задачи (от `random` и повтора
    /// не зависит). Нет строки — `None` или, с `MissingRow::Random`, случайная.
    pub fn row(&self, index: usize, serial: i64, acc_id: &str, missing: MissingRow) -> Option<&Row> {
        let found = match self.key {
            Some("serial_number") => {
                let serial = serial.to_string();
                self.rows.iter().find(|row| row.get("serial_number").map(key_text).as_deref() == Some(serial.as_str()))
            }
            Some(_) => self.rows.iter().find(|row| row.get("acc_id").map(key_text).as_deref() == Some(acc_id)),
            None => self.rows.get(index),
        };
        match (found, missing) {
            (Some(row), _) => Some(row),
            (None, MissingRow::Random) => Some(&self.rows[vars::random_index(self.rows.len())]),
            (None, MissingRow::Skip) => None,
        }
    }
}

/// Ключ строкой: номер из CSV — текст, из JSON — число.
fn key_text(value: &Value) -> String {
    vars::to_text(value).trim().to_string()
}

/// Таблица из файла: `.csv` (UTF-8, первая строка — имена колонок), `.xlsx` (первый лист) или `.json` (массив объектов).
pub fn read_table(path: &Path) -> Result<Vec<Row>, String> {
    if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("xlsx")) {
        return crate::flow::ops::excel::read_sheet(path);
    }
    let bytes = std::fs::read(path).map_err(|e| format!("таблица {} не прочитана: {e}", path.display()))?;
    // Excel с русской локалью по умолчанию сохраняет «CSV» в cp1251.
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("таблица {} не в UTF-8 — сохраните её как «CSV UTF-8»", path.display()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("csv") => parse_csv(text),
        Some("json") => parse_json(text),
        _ => Err(format!("таблица {} — файл .csv, .xlsx или .json", path.display())),
    }
}

/// CSV: первая строка — имена колонок; значения — строки как есть.
pub fn parse_csv(text: &str) -> Result<Vec<Row>, String> {
    // Excel с русской локалью сохраняет CSV через точку с запятой.
    let header = text.lines().next().unwrap_or_default();
    let delimiter = if header.contains(';') && !header.contains(',') { b';' } else { b',' };
    let mut reader = csv::ReaderBuilder::new().delimiter(delimiter).from_reader(text.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| format!("таблица не читается как CSV: {e}"))?
        .iter()
        .map(|name| name.trim().to_string())
        .collect();
    let mut seen = HashSet::new();
    for (i, name) in headers.iter().enumerate() {
        if name.is_empty() {
            return Err(format!("таблица: у колонки {} нет имени", i + 1));
        }
        if !seen.insert(name) {
            return Err(format!("таблица: колонка {name} повторяется"));
        }
    }
    let mut rows = Vec::new();
    for (i, record) in reader.records().enumerate() {
        // Строка данных, как в остальных ошибках таблицы: заголовок не считается.
        let record = record.map_err(|e| match e.kind() {
            csv::ErrorKind::UnequalLengths { expected_len, len, .. } => {
                format!("таблица, строка {}: значений {len}, а колонок {expected_len}", i + 1)
            }
            _ => format!("таблица, строка {}: {e}", i + 1),
        })?;
        rows.push(headers.iter().cloned().zip(record.iter().map(|value| Value::String(value.to_string()))).collect());
    }
    Ok(rows)
}

/// JSON: массив объектов; значения — своих типов.
pub fn parse_json(text: &str) -> Result<Vec<Row>, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("таблица не читается как JSON: {e}"))?;
    let Value::Array(items) = value else { return Err("таблица JSON — массив объектов".into()) };
    items
        .into_iter()
        .enumerate()
        .map(|(i, item)| match item {
            Value::Object(row) => Ok(row),
            _ => Err(format!("таблица JSON, элемент {}: ожидается объект", i + 1)),
        })
        .collect()
}

/// Колонки таблицы без объявления в процессе становятся переменными `string` (как Import Data from Excel). Ключевые
/// колонки и имена системных переменных — нет: вторые дадут ошибку в `check`.
pub fn widen(flow: &mut Flow, table: &Table) {
    for row in table.rows() {
        for name in row.keys() {
            let declared = flow.variables.iter().any(|decl| decl.name == *name);
            if !declared && !KEYS.contains(&name.as_str()) && !SYSTEM.contains(&name.as_str()) {
                flow.variables.push(VarDecl {
                    name: name.clone(),
                    ty: VarType::String,
                    default: Value::String(String::new()),
                    description: String::new(),
                });
            }
        }
    }
}

/// Значения запуска: параметры задачи, поверх — строка таблицы без ключевых колонок.
pub fn layer(params: &Row, row: Option<&Row>) -> Row {
    let mut values = params.clone();
    if let Some(row) = row {
        values.extend(row.iter().filter(|(name, _)| !KEYS.contains(&name.as_str())).map(|(k, v)| (k.clone(), v.clone())));
    }
    values
}

/// Параметры и каждая строка таблицы проверяются так же, как их примет запуск (`vars::declare`). `flow` — после `widen`.
pub fn check(flow: &Flow, params: &Row, table: Option<&Table>) -> Result<(), String> {
    let declare = |values: Row| vars::declare(&mut Scope::new(HashMap::new()), &flow.variables, values);
    declare(params.clone())?;
    for (i, row) in table.map(Table::rows).unwrap_or_default().iter().enumerate() {
        declare(layer(params, Some(row))).map_err(|e| format!("таблица, строка {}: {e}", i + 1))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shared_queue_deduplicates_links_and_does_not_bind_rows_to_profile_keys() {
        let table = Table::queue(vec![
            row(json!({ "search_url": "https://example.test/a", "serial_number": "1" })),
            row(json!({ "search_url": " https://example.test/a ", "serial_number": "2" })),
            row(json!({ "search_url": "https://example.test/b", "serial_number": "1" })),
        ]).unwrap();
        assert_eq!(table.key(), Some("search_url"));
        assert_eq!(table.rows().len(), 2);
        let table = Table::queue(vec![row(json!({ "a": 1, "b": 2 })), row(json!({ "b": 2, "a": 1 }))]).unwrap();
        assert_eq!(table.rows().len(), 1, "порядок колонок JSON не создаёт новое задание");
        assert!(Table::queue(Vec::new()).is_err());
        assert!(Table::queue(vec![row(json!({ "url": "a" })), row(json!({ "url": "" }))]).unwrap_err().contains("строка 2"));
    }

    fn flow(variables: Value) -> Flow {
        serde_json::from_value(json!({ "format": "ombra-flow/1", "name": "Т", "variables": variables, "nodes": [{ "id": "s", "op": "start" }] }))
            .unwrap()
    }

    fn row(value: Value) -> Row {
        value.as_object().unwrap().clone()
    }

    fn declared(flow: &Flow, values: Row) -> Scope {
        let mut scope = Scope::new(HashMap::new());
        vars::declare(&mut scope, &flow.variables, values).unwrap();
        scope
    }

    #[test]
    fn set_values_are_coerced_by_type() {
        let f = flow(json!([
            { "name": "n", "type": "number", "default": 1 },
            { "name": "ok", "type": "boolean", "default": false },
            { "name": "list", "type": "array", "default": [] },
            { "name": "s", "type": "string", "default": "" }
        ]));
        let params = row(json!({ "n": "7", "ok": "true", "list": "[1,2]", "s": "x" }));
        check(&f, &params, None).unwrap();
        let scope = declared(&f, layer(&params, None));
        assert_eq!(scope.get("n"), Some(&json!(7)));
        assert_eq!(scope.get("ok"), Some(&json!(true)));
        assert_eq!(scope.get("list"), Some(&json!([1, 2])));
        assert_eq!(scope.get("s"), Some(&json!("x")));
    }

    #[test]
    fn unknown_and_system_names_and_wrong_types_fail() {
        let f = flow(json!([{ "name": "n", "type": "number", "default": 1 }]));
        assert!(check(&f, &row(json!({ "nope": "1" })), None).unwrap_err().contains("nope"));
        assert!(check(&f, &row(json!({ "acc_id": "x" })), None).unwrap_err().contains("системная"));
        assert!(check(&f, &row(json!({ "n": "abc" })), None).unwrap_err().contains("переменная n"));
    }

    #[test]
    fn rows_find_profiles_by_serial_acc_id_or_selection_order() {
        let by_serial = Table::new(vec![row(json!({ "serial_number": "12", "v": "a" })), row(json!({ "serial_number": 15, "v": "b" }))]).unwrap();
        assert_eq!(by_serial.row(0, 15, "x", MissingRow::Skip).unwrap()["v"], json!("b"), "номер из JSON — числом");
        assert_eq!(by_serial.row(0, 12, "x", MissingRow::Skip).unwrap()["v"], json!("a"), "номер из CSV — строкой");
        assert!(by_serial.row(0, 99, "x", MissingRow::Skip).is_none());
        assert!(by_serial.row(0, 99, "x", MissingRow::Random).is_some(), "случайная строка");
        let by_acc = Table::new(vec![row(json!({ "acc_id": "k7f2q9", "v": "a" }))]).unwrap();
        assert_eq!(by_acc.row(5, 1, "k7f2q9", MissingRow::Skip).unwrap()["v"], json!("a"));
        let by_order = Table::new(vec![row(json!({ "v": "a" })), row(json!({ "v": "b" }))]).unwrap();
        assert_eq!(by_order.row(1, 1, "x", MissingRow::Skip).unwrap()["v"], json!("b"));
        assert!(by_order.row(2, 1, "x", MissingRow::Skip).is_none(), "профилей больше строк");
    }

    #[test]
    fn duplicate_keys_and_empty_tables_are_rejected() {
        let err = Table::new(vec![row(json!({ "serial_number": "3" })), row(json!({ "serial_number": 3 }))]).unwrap_err();
        assert!(err.contains("повторяется") && err.contains("строка 2"), "{err}");
        assert!(Table::new(vec![row(json!({ "acc_id": "a" })), row(json!({ "acc_id": "a" }))]).is_err());
        let both = vec![row(json!({ "serial_number": "1", "acc_id": "a" })), row(json!({ "serial_number": "2", "acc_id": "a" }))];
        assert!(Table::new(both).unwrap_err().contains("acc_id a повторяется (строка 2)"), "вторая ключевая колонка тоже проверяется");
        assert!(Table::new(Vec::new()).unwrap_err().contains("пустая"));
    }

    #[test]
    fn undeclared_columns_become_string_variables_but_keys_do_not() {
        let mut f = flow(json!([{ "name": "n", "type": "number", "default": 1 }]));
        let table = Table::new(vec![row(json!({ "serial_number": "1", "login": "u1", "n": "5" }))]).unwrap();
        widen(&mut f, &table);
        let names: Vec<&str> = f.variables.iter().map(|decl| decl.name.as_str()).collect();
        assert_eq!(names, ["n", "login"]);
        check(&f, &Row::new(), Some(&table)).unwrap();
        let scope = declared(&f, layer(&Row::new(), table.rows().first()));
        assert_eq!((scope.get("login"), scope.get("n")), (Some(&json!("u1")), Some(&json!(5))));
        // Колонка с именем системной переменной переменной не становится и даёт ошибку.
        let mut f = flow(json!([]));
        let table = Table::new(vec![row(json!({ "password": "x" }))]).unwrap();
        widen(&mut f, &table);
        assert!(f.variables.is_empty());
        assert!(check(&f, &Row::new(), Some(&table)).unwrap_err().contains("системная"));
    }

    #[test]
    fn row_errors_name_the_row_and_rows_override_params() {
        let f = flow(json!([{ "name": "n", "type": "number", "default": 1 }]));
        let table = Table::new(vec![row(json!({ "n": "2" })), row(json!({ "n": "x" }))]).unwrap();
        let err = check(&f, &row(json!({ "n": "1" })), Some(&table)).unwrap_err();
        assert!(err.starts_with("таблица, строка 2"), "{err}");
        let values = layer(&row(json!({ "n": "1", "keep": "p" })), Some(&row(json!({ "n": "2", "serial_number": "9" }))));
        assert_eq!(values, row(json!({ "n": "2", "keep": "p" })), "строка поверх параметров, ключ — не переменная");
    }

    #[test]
    fn csv_from_excel_and_json_tables_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows.csv");
        std::fs::write(&path, "\u{feff}login,pass\nu1, p 1\nu2,p2\n").unwrap();
        assert_eq!(read_table(&path).unwrap(), vec![row(json!({ "login": "u1", "pass": " p 1" })), row(json!({ "login": "u2", "pass": "p2" }))]);
        assert_eq!(parse_csv("login;pass\nu1;p1\n").unwrap(), vec![row(json!({ "login": "u1", "pass": "p1" }))], "Excel с русской локалью");
        let path = dir.path().join("rows.JSON");
        std::fs::write(&path, r#"[{"login":"u1","n":5}]"#).unwrap();
        assert_eq!(read_table(&path).unwrap(), vec![row(json!({ "login": "u1", "n": 5 }))]);
    }

    #[test]
    fn broken_tables_fail_before_start() {
        let ragged = parse_csv("a,b\n1,2\n3\n").unwrap_err();
        assert_eq!(ragged, "таблица, строка 2: значений 1, а колонок 2", "рваная строка — номер среди строк данных");
        assert!(parse_csv("a,a\n1,2\n").unwrap_err().contains("повторяется"));
        assert!(parse_csv("a,\n1,2\n").unwrap_err().contains("нет имени"));
        assert!(parse_json("{}").unwrap_err().contains("массив"));
        assert!(parse_json("[1]").unwrap_err().contains("элемент 1"));
        assert!(parse_json("[").is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows.txt");
        std::fs::write(&path, "a\n1\n").unwrap();
        assert!(read_table(&path).unwrap_err().contains(".csv, .xlsx или .json"));
        assert!(read_table(&dir.path().join("нет.csv")).is_err());
        // «CSV» из Excel в cp1251: «Логин» не в UTF-8.
        let path = dir.path().join("cp1251.csv");
        std::fs::write(&path, b"\xcb\xee\xe3\xe8\xed\n1\n").unwrap();
        let err = read_table(&path).unwrap_err();
        assert!(err.ends_with("не в UTF-8 — сохраните её как «CSV UTF-8»") && err.contains("cp1251.csv"), "{err}");
    }

    #[test]
    fn order_and_missing_row_round_trip() {
        for order in [Order::Ordered, Order::Random] {
            assert_eq!(Order::parse(order.as_str()), Some(order));
        }
        for missing in [MissingRow::Skip, MissingRow::Random] {
            assert_eq!(MissingRow::parse(missing.as_str()), Some(missing));
        }
        assert_eq!((Order::default(), MissingRow::default()), (Order::Ordered, MissingRow::Skip));
        assert_eq!(Order::parse("shuffle"), None);
    }
}
