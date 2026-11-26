//! Excel: Save to Excel, Import Data from Excel и чтение листа для таблицы задачи (спецификация А4, «А4-1»).
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::files::{absolute, file_name};
use crate::flow::ops::{Cfg, OpError};
use crate::flow::params::Row;
use crate::flow::vars::{SYSTEM, is_var_name, to_text};
use calamine::{Data, Reader};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Дописывание Save to Excel по очереди: запуски пакета пишут в один файл (читаем и переписываем целиком).
/// ponytail: один замок на все файлы; по замку на файл — если запись станет узким местом.
static SAVE_EXCEL: Mutex<()> = Mutex::new(());

pub(crate) async fn exec(ctx: &mut Ctx, id: &str, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let note = match id {
        "data.save_excel" => {
            let name = file_name(&cfg.text(&ctx.scope, "file_name")?)?;
            let names: Vec<String> = cfg.raw("columns").as_array().map(|list| list.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
            if names.is_empty() {
                return Err("колонки не заданы".into());
            }
            let values: Vec<Value> = names.iter().map(|name| ctx.scope.get(name).cloned().unwrap_or(Value::Null)).collect();
            let task = ctx.scope.get("task_id").map(to_text).unwrap_or_default();
            let file = ctx.files.join(if task.is_empty() { format!("{name}.xlsx") } else { format!("{name}-{task}.xlsx") });
            let note = format!("строка {} колонок", names.len());
            // Чтение и перезапись целиком под общим замком — в блокирующем потоке: рабочие потоки tokio не ждут замок.
            let cancel = ctx.cancel.clone();
            tokio::task::spawn_blocking(move || append_row(&file, &names, &values, &cancel)).await.map_err(|e| format!("файл не записан: {e}"))??;
            note
        }
        "data.import_excel" => {
            let path = absolute(&cfg.text(&ctx.scope, "path")?, "путь файла")?;
            // Распаковка и разбор большого XLSX — в блокирующем потоке.
            let rows = tokio::task::spawn_blocking(move || read_sheet(&path)).await.map_err(|e| format!("таблица не прочитана: {e}"))??;
            if let Some(bad) = rows.first().and_then(|row| row.keys().find(|name| !is_var_name(name))) {
                return Err(OpError::Failed(format!("заголовок {bad}: буквы, цифры и «_», не с цифры")));
            }
            let serial = rows.first().is_some_and(|row| row.contains_key("serial_number"));
            // Колонки станут переменными: все имена проверяются до первой записи — половина строки не применится.
            if serial {
                let save_to = cfg.var_out("save_to");
                for name in rows[0].keys().filter(|name| name.as_str() != "serial_number") {
                    if save_to.as_deref() == Some(name.as_str()) {
                        return Err(OpError::Failed(format!("колонка «{name}» совпадает с «Сохранить в»")));
                    }
                    if SYSTEM.contains(&name.as_str()) {
                        return Err(OpError::Failed(format!("колонка «{name}» — системная переменная")));
                    }
                }
            }
            let mut note = ctx.save(cfg, Value::Array(rows.iter().cloned().map(Value::Object).collect()))?;
            if serial {
                let serial = ctx.scope.get("serial_number").map(|v| to_text(v).trim().to_string()).unwrap_or_default();
                let found = rows.iter().find(|row| row.get("serial_number").map(|v| to_text(v).trim().to_string()).as_deref() == Some(serial.as_str()));
                let chosen = match found {
                    Some(row) => Some(row),
                    None if cfg.flag("skip_missing_serial") => None,
                    None => Some(&rows[crate::flow::vars::random_index(rows.len())]),
                };
                match chosen {
                    Some(row) => {
                        for (name, value) in row.iter().filter(|(name, _)| name.as_str() != "serial_number") {
                            ctx.scope.set(name, value.clone())?;
                        }
                    }
                    None => note = format!("{note}; строки для профиля нет"),
                }
            }
            note
        }
        other => return Err(OpError::Failed(format!("операция {other} не исполняется"))),
    };
    Ok((Step::Port("next"), note))
}

/// Значение ячейки текстом: целые без «.0», даты `YYYY-MM-DD` (с временем — `YYYY-MM-DD HH:MM:SS`), пустая — пусто.
fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        Data::DateTime(date) if date.is_datetime() => {
            let (y, mo, d, h, mi, s, _) = date.to_ymd_hms_milli();
            if (h, mi, s) == (0, 0, 0) { format!("{y:04}-{mo:02}-{d:02}") } else { format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}") }
        }
        other => other.to_string(),
    }
}

/// Первый лист файла: первая строка — заголовки, остальные — объекты строк; полностью пустые строки пропускаются.
pub(crate) fn read_sheet(path: &Path) -> Result<Vec<Row>, String> {
    let shown = path.display();
    let mut book = calamine::open_workbook_auto(path).map_err(|e| format!("таблица {shown} не прочитана: {e}"))?;
    let range = book.worksheet_range_at(0).ok_or_else(|| format!("таблица {shown}: в книге нет листов"))?.map_err(|e| format!("таблица {shown} не прочитана: {e}"))?;
    let mut lines = range.rows();
    let headers: Vec<String> = lines.next().map(|row| row.iter().map(|cell| cell_text(cell).trim().to_string()).collect()).unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    for (i, name) in headers.iter().enumerate() {
        if name.is_empty() {
            return Err(format!("таблица: у колонки {} нет имени", i + 1));
        }
        if !seen.insert(name) {
            return Err(format!("таблица: колонка {name} повторяется"));
        }
    }
    Ok(lines
        .map(|row| row.iter().map(cell_text).collect::<Vec<_>>())
        .filter(|row| row.iter().any(|v| !v.is_empty()))
        .map(|row| headers.iter().cloned().zip(row.into_iter().map(Value::String)).collect())
        .collect())
}

/// Строка значений в конец первого листа; нет файла — он создаётся с заголовком. У файла свой заголовок: те же колонки
/// в другом порядке — значения по нему, другие колонки — ошибка без записи. Остановленный, пока ждал замок, запуск
/// ничего не пишет.
fn append_row(file: &Path, headers: &[String], values: &[Value], cancel: &CancellationToken) -> Result<(), OpError> {
    let _turn = SAVE_EXCEL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if cancel.is_cancelled() {
        return Err(OpError::Cancelled);
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("папка файла не создана: {e}"))?;
    }
    let mut table: Vec<Vec<Data>> = if file.exists() {
        let mut book = calamine::open_workbook_auto(file).map_err(|e| format!("файл не прочитан: {e}"))?;
        let range = book.worksheet_range_at(0).ok_or("в файле нет листов")?.map_err(|e| format!("файл не прочитан: {e}"))?;
        range.rows().map(<[Data]>::to_vec).collect()
    } else {
        Vec::new()
    };
    if table.is_empty() {
        table.push(headers.iter().cloned().map(Data::String).collect());
    }
    let head: Vec<String> = table[0].iter().map(|cell| cell_text(cell).trim().to_string()).collect();
    let (mut have, mut want) = (head.clone(), headers.to_vec());
    have.sort();
    want.sort();
    if have != want {
        let shown = file.file_name().unwrap_or_default().to_string_lossy();
        return Err(OpError::Failed(format!("в файле «{shown}» другие колонки: {}", head.join(", "))));
    }
    table.push(head.iter().map(|name| headers.iter().position(|h| h == name).map_or(Data::Empty, |i| cell_data(&values[i]))).collect());
    let mut book = rust_xlsxwriter::Workbook::new();
    let sheet = book.add_worksheet();
    for (r, row) in table.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let (r, c) = (r as u32, c as u16);
            let wrote = match cell {
                Data::Empty => continue,
                Data::Int(n) => sheet.write_number(r, c, *n as f64),
                Data::Float(n) => sheet.write_number(r, c, *n),
                Data::Bool(b) => sheet.write_boolean(r, c, *b),
                other => sheet.write_string(r, c, other.to_string()),
            };
            wrote.map_err(|e| format!("ячейка не записана: {e}"))?;
        }
    }
    Ok(replace(file, |tmp| book.save(tmp).map_err(|e| format!("файл не записан: {e}")))?)
}

/// Через временный файл: оборванная запись не портит накопленное, а неудачная не оставляет `*.xlsx.tmp`.
fn replace(file: &Path, write: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    let tmp = file.with_extension("xlsx.tmp");
    let done = write(&tmp).and_then(|()| std::fs::rename(&tmp, file).map_err(|e| format!("файл не записан: {e}")));
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

/// Значение переменной в ячейку: число и логическое как есть, массив и объект — JSON-текстом.
fn cell_data(value: &Value) -> Data {
    match value {
        Value::Null => Data::Empty,
        Value::Bool(b) => Data::Bool(*b),
        Value::Number(n) => n.as_f64().map(Data::Float).unwrap_or_else(|| Data::String(n.to_string())),
        Value::String(s) => Data::String(s.clone()),
        other => Data::String(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::interp::{Ctx, Fail};
    use crate::flow::model::Flow;
    use serde_json::json;

    fn one(op: &str, config: Value) -> Flow {
        serde_json::from_value(json!({
            "format": "ombra-flow/1", "name": "t",
            "nodes": [{ "id": "s", "op": "start" }, { "id": "x", "op": op, "config": config }],
            "edges": [{ "from": "s", "to": "x" }]
        }))
        .unwrap()
    }

    /// Лист для проверок: строки текста и чисел, дата, дата со временем, формула, пустая ячейка, `1.0`.
    fn fixture(path: &Path, headers: &[&str]) {
        use rust_xlsxwriter::{ExcelDateTime, Format, Formula, Workbook};
        let mut book = Workbook::new();
        let sheet = book.add_worksheet();
        for (c, h) in headers.iter().enumerate() {
            sheet.write_string(0, c as u16, *h).unwrap();
        }
        let date = Format::new().set_num_format("yyyy-mm-dd");
        let stamp = Format::new().set_num_format("yyyy-mm-dd hh:mm:ss");
        sheet.write_string(1, 0, "7").unwrap();
        sheet.write_number(1, 1, 1.0).unwrap();
        sheet.write_datetime_with_format(1, 2, ExcelDateTime::from_ymd(2026, 10, 21).unwrap(), &date).unwrap();
        sheet.write_datetime_with_format(1, 3, ExcelDateTime::from_ymd(2026, 10, 21).unwrap().and_hms(13, 5, 9).unwrap(), &stamp).unwrap();
        sheet.write_formula(1, 4, Formula::new("=1+1").set_result("2")).unwrap();
        sheet.write_number(1, 6, 2.5).unwrap();
        book.save(path).unwrap();
    }

    #[test]
    fn sheet_values_are_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.xlsx");
        fixture(&path, &["serial_number", "n", "d", "dt", "f", "e", "x"]);
        let rows = read_sheet(&path).unwrap();
        assert_eq!(
            Value::Array(rows.into_iter().map(Value::Object).collect()),
            json!([{ "serial_number": "7", "n": "1", "d": "2026-10-21", "dt": "2026-10-21 13:05:09", "f": "2", "e": "", "x": "2.5" }])
        );
    }

    #[test]
    fn task_table_reads_xlsx_in_sheet_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.xlsx");
        fixture(&path, &["zeta", "alpha", "mid", "a", "b", "c", "d"]);
        let rows = crate::flow::params::read_table(&path).unwrap();
        assert_eq!(rows[0].keys().take(3).collect::<Vec<_>>(), ["zeta", "alpha", "mid"]);
        let twin = dir.path().join("twin.xlsx");
        fixture(&twin, &["a", "a", "c", "d", "e", "f", "g"]);
        assert!(crate::flow::params::read_table(&twin).unwrap_err().contains("колонка a повторяется"));
    }

    #[tokio::test]
    async fn import_without_serial_sets_only_save_to() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.xlsx");
        fixture(&path, &["a", "b", "c", "d", "e", "f", "g"]);
        let mut ctx = Ctx::bare();
        ctx.run(&one("data.import_excel", json!({ "path": path.to_str().unwrap(), "save_to": "v" }))).await.unwrap();
        assert_eq!(ctx.scope.get("v").unwrap()[0]["b"], json!("1"));
        assert_eq!(ctx.scope.get("a"), None, "переменных-колонок нет");
    }

    #[tokio::test]
    async fn import_with_serial_picks_the_profile_row_or_random_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.xlsx");
        fixture(&path, &["serial_number", "n", "d", "dt", "f", "e", "x"]);
        let flow = |skip: bool| one("data.import_excel", json!({ "path": path.to_str().unwrap(), "skip_missing_serial": skip, "save_to": "v" }));
        let mut ctx = Ctx::bare();
        ctx.scope = crate::flow::vars::Scope::new(std::collections::HashMap::from([("serial_number", json!("7"))]));
        ctx.run(&flow(false)).await.unwrap();
        assert_eq!((ctx.scope.get("n"), ctx.scope.get("d")), (Some(&json!("1")), Some(&json!("2026-10-21"))));
        let mut other = Ctx::bare();
        other.scope = crate::flow::vars::Scope::new(std::collections::HashMap::from([("serial_number", json!("8"))]));
        other.run(&flow(false)).await.unwrap();
        assert_eq!(other.scope.get("n"), Some(&json!("1")), "строки нет — случайная");
        let mut skipped = Ctx::bare();
        skipped.scope = crate::flow::vars::Scope::new(std::collections::HashMap::from([("serial_number", json!("8"))]));
        skipped.run(&flow(true)).await.unwrap();
        assert_eq!(skipped.scope.get("n"), None);
        assert!(skipped.scope.get("v").is_some());
    }

    #[tokio::test]
    async fn import_rejects_a_bad_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.xlsx");
        fixture(&path, &["1bad", "b", "c", "d", "e", "f", "g"]);
        let bad = Ctx::bare().run(&one("data.import_excel", json!({ "path": path.to_str().unwrap(), "save_to": "v" }))).await;
        assert!(matches!(bad, Err(Fail::Node { ref message, .. }) if message.contains("заголовок 1bad")), "{bad:?}");
    }

    #[tokio::test]
    async fn import_refuses_a_column_named_like_save_to_or_a_system_variable_before_setting_anything() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.xlsx");
        for (headers, want) in [
            (["serial_number", "a", "v", "d", "e", "f", "g"], "колонка «v» совпадает с «Сохранить в»"),
            (["serial_number", "a", "password", "d", "e", "f", "g"], "колонка «password» — системная переменная"),
        ] {
            fixture(&path, &headers);
            let mut ctx = Ctx::bare();
            ctx.scope = crate::flow::vars::Scope::new(std::collections::HashMap::from([("serial_number", json!("7"))]));
            let bad = ctx.run(&one("data.import_excel", json!({ "path": path.to_str().unwrap(), "save_to": "v" }))).await;
            assert!(matches!(bad, Err(Fail::Node { ref message, .. }) if *message == format!("Import Data from Excel: {want}")), "{bad:?}");
            assert_eq!((ctx.scope.get("a"), ctx.scope.get("v")), (None, None), "{want}: ничего не задано");
        }
    }

    #[test]
    fn failed_replace_leaves_no_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        // На место непустой папки файл не переименовать.
        let file = dir.path().join("out.xlsx");
        std::fs::create_dir(&file).unwrap();
        std::fs::write(file.join("keep"), b"x").unwrap();
        let err = replace(&file, |tmp| std::fs::write(tmp, b"new").map_err(|e| e.to_string())).unwrap_err();
        assert!(err.starts_with("файл не записан"), "{err}");
        assert!(!dir.path().join("out.xlsx.tmp").exists(), "временный файл удалён");
        let err = replace(&dir.path().join("b.xlsx"), |tmp| {
            std::fs::write(tmp, b"half").unwrap();
            Err("файл не записан: диск полон".into())
        });
        assert_eq!(err, Err("файл не записан: диск полон".into()));
        assert!(!dir.path().join("b.xlsx.tmp").exists(), "временный файл удалён");
    }

    /// Пока замок занят другим запуском, поток tokio свободен: держатель отпускает замок только по сигналу из потока
    /// tokio, а если сигнала за 5 с нет — сам и отвечает `false`.
    #[tokio::test(flavor = "current_thread")]
    async fn save_waits_for_the_lock_off_the_runtime_thread() {
        let dir = tempfile::tempdir().unwrap();
        let (locked, is_locked) = std::sync::mpsc::channel();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _turn = SAVE_EXCEL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            locked.send(()).unwrap();
            wait.recv_timeout(std::time::Duration::from_secs(5)).is_ok()
        });
        is_locked.recv().unwrap();
        let mut ctx = Ctx::bare();
        ctx.files = dir.path().to_path_buf();
        ctx.scope.set("a", json!(1)).unwrap();
        let flow = one("data.save_excel", json!({ "file_name": "out", "columns": ["a"] }));
        tokio::select! {
            biased;
            saved = ctx.run(&flow) => { saved.unwrap(); }
            _ = async { let _ = go.send(()); std::future::pending::<()>().await } => unreachable!(),
        }
        assert!(holder.join().unwrap(), "поток tokio стоял на замке");
        assert_eq!(read_sheet(&dir.path().join("out.xlsx")).unwrap().len(), 1);
    }

    /// «Стоп», пока запись ждёт замок: после замка она ничего не пишет.
    #[test]
    fn save_stopped_while_waiting_for_the_lock_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let turn = SAVE_EXCEL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = runtime.block_on(async {
            let mut ctx = Ctx::bare();
            ctx.files = dir.path().to_path_buf();
            ctx.scope.set("a", json!(1)).unwrap();
            let cancel = ctx.cancel.clone();
            let flow = one("data.save_excel", json!({ "file_name": "out", "columns": ["a"] }));
            tokio::select! {
                biased;
                result = ctx.run(&flow) => result,
                // Первый опрос узла уже отдал запись блокирующему потоку, она ждёт замок.
                _ = async { cancel.cancel(); std::future::pending::<()>().await } => unreachable!(),
            }
        });
        assert_eq!(result, Err(Fail::Cancelled));
        drop(turn);
        // Среда при выходе дожидается блокирующих задач: отпущенная запись доходит до конца.
        drop(runtime);
        assert!(!dir.path().join("out.xlsx").exists(), "остановленный запуск строку не дописал");
    }

    #[tokio::test]
    async fn save_into_an_existing_file_follows_its_header() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = Ctx::bare();
        ctx.files = dir.path().to_path_buf();
        for (name, value) in [("a", 1), ("b", 2), ("c", 3)] {
            ctx.scope.set(name, json!(value)).unwrap();
        }
        let save = |columns: Value| one("data.save_excel", json!({ "file_name": "out", "columns": columns }));
        ctx.run(&save(json!(["a", "b"]))).await.unwrap();
        ctx.run(&save(json!(["b", "a"]))).await.unwrap();
        let rows = read_sheet(&dir.path().join("out.xlsx")).unwrap();
        assert_eq!((rows[1]["a"].clone(), rows[1]["b"].clone()), (json!("1"), json!("2")), "те же колонки в другом порядке — по заголовку файла");
        let bad = ctx.run(&save(json!(["a", "c"]))).await;
        assert!(matches!(bad, Err(Fail::Node { ref message, .. }) if message == "Save to Excel: в файле «out.xlsx» другие колонки: a, b"), "{bad:?}");
        assert_eq!(read_sheet(&dir.path().join("out.xlsx")).unwrap().len(), 2, "строка не дописана");
    }

    /// Чтение листа не занимает поток tokio: открытие канала FIFO на чтение ждёт писателя, а писатель приходит только по
    /// сигналу из потока tokio (или через 5 с — и отвечает `false`).
    #[tokio::test(flavor = "current_thread")]
    async fn import_reads_the_file_off_the_runtime_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.xlsx");
        let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: строка с нулём в конце живёт до конца вызова.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let fifo = path.clone();
        let writer = std::thread::spawn(move || {
            let signalled = wait.recv_timeout(std::time::Duration::from_secs(5)).is_ok();
            drop(std::fs::OpenOptions::new().write(true).open(&fifo).unwrap());
            signalled
        });
        let flow = one("data.import_excel", json!({ "path": path.to_str().unwrap(), "save_to": "v" }));
        let mut ctx = Ctx::bare();
        tokio::select! {
            biased;
            _ = ctx.run(&flow) => {}
            _ = async { let _ = go.send(()); std::future::pending::<()>().await } => unreachable!(),
        }
        assert!(writer.join().unwrap(), "поток tokio стоял на чтении файла");
    }

    #[tokio::test]
    async fn save_appends_rows_named_by_task_and_rejects_bad_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = Ctx::bare();
        ctx.files = dir.path().to_path_buf();
        ctx.scope.set("a", json!("текст")).unwrap();
        ctx.scope.set("b", json!(3)).unwrap();
        ctx.scope.set("c", json!([1, "x"])).unwrap();
        let flow = one("data.save_excel", json!({ "file_name": "out", "columns": ["a", "b", "c"] }));
        ctx.run(&flow).await.unwrap();
        ctx.run(&flow).await.unwrap();
        let rows = read_sheet(&dir.path().join("out.xlsx")).unwrap();
        assert_eq!(rows.len(), 2, "вне задачи — без суффикса");
        assert_eq!((rows[1]["a"].clone(), rows[1]["b"].clone(), rows[1]["c"].clone()), (json!("текст"), json!("3"), json!("[1,\"x\"]")));
        let bad = ctx.run(&one("data.save_excel", json!({ "file_name": "../out", "columns": ["a"] }))).await;
        assert!(matches!(bad, Err(Fail::Node { ref message, .. }) if message == "Save to Excel: имя файла — без «/» и не «..»"), "{bad:?}");
        let mut task = Ctx::bare();
        task.files = dir.path().to_path_buf();
        task.scope = crate::flow::vars::Scope::new(std::collections::HashMap::from([("task_id", json!("5"))]));
        task.scope.set("a", json!("z")).unwrap();
        task.run(&one("data.save_excel", json!({ "file_name": "out", "columns": ["a"] }))).await.unwrap();
        assert!(dir.path().join("out-5.xlsx").exists());
    }

    #[tokio::test]
    async fn parallel_saves_keep_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("many.xlsx");
        let jobs: Vec<_> = (0..2)
            .map(|i| {
                let file = file.clone();
                tokio::task::spawn_blocking(move || {
                    for n in 0..10 {
                        append_row(&file, &["who".into(), "n".into()], &[json!(i), json!(n)], &CancellationToken::new()).unwrap();
                    }
                })
            })
            .collect();
        for job in jobs {
            job.await.unwrap();
        }
        assert_eq!(read_sheet(&file).unwrap().len(), 20);
    }
}
