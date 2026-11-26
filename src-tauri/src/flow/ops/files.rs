//! Файлы процесса: Save to Txt, Import Data from Txt, Download File, общие правила имён и папок (спецификация
//! автоматизации, «Файлы», «Получение и обработка данных»).
use crate::flow::interp::{Ctx, Step};
use crate::flow::ops::{Cfg, OpError};
use crate::flow::ops::web::tabs;
use crate::flow::vars::to_text;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Дописывание Save to Txt по очереди: запуски пакета пишут в один файл.
/// ponytail: один замок на все файлы; по замку на файл — если запись станет узким местом.
static SAVE_TXT: Mutex<()> = Mutex::new(());

pub(crate) async fn exec(ctx: &mut Ctx, id: &str, cfg: &Cfg<'_>) -> Result<(Step, String), OpError> {
    let note = match id {
        "data.save_txt" => {
            let name = file_name(&cfg.text(&ctx.scope, "file_name")?)?;
            let line = cfg.text(&ctx.scope, "template")?;
            let task = ctx.scope.get("task_id").map(to_text).unwrap_or_default();
            let file = ctx.files.join(if task.is_empty() { format!("{name}.txt") } else { format!("{name}-{task}.txt") });
            append_line(&file, &line)?;
            format!("строка {} символов", line.chars().count())
        }
        "data.import_txt" => {
            let path = absolute(&cfg.text(&ctx.scope, "path")?, "путь файла")?;
            let bytes = std::fs::read(&path).map_err(|e| format!("файл не прочитан: {e}"))?;
            let text = String::from_utf8(bytes).map_err(|_| "файл не в UTF-8")?;
            return Ok((Step::Port("next"), ctx.save(cfg, Value::from(lines(&text)))?));
        }
        "data.download" => {
            let url = cfg.text(&ctx.scope, "url")?;
            let folder = folder(ctx, cfg, "folder", "папка загрузки")?;
            let relay = tabs(ctx)?.relay();
            let path = download(relay, &url, &folder, DOWNLOAD_TIMEOUT).await?.ok_or("Истекло время загрузки файла")?;
            format!("сохранено {} байт", std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0))
        }
        other => return Err(OpError::Failed(format!("операция {other} не исполняется"))),
    };
    Ok((Step::Port("next"), note))
}

/// Строка в конец файла; папка создаётся.
fn append_line(file: &Path, line: &str) -> Result<(), OpError> {
    let _turn = SAVE_TXT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("папка файла не создана: {e}"))?;
    }
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(file).map_err(|e| format!("файл не открыт: {e}"))?;
    writeln!(out, "{line}").map_err(|e| format!("строка не записана: {e}"))?;
    Ok(())
}

/// Строки Import Data from Txt: метка UTF-8 (BOM) в начале и `\r` в конце строк срезаются, последняя пустая строка
/// отбрасывается.
fn lines(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines: Vec<String> = text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line).to_string()).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// Имя файла без папок: в чужую папку файл попадает только через поле папки.
pub(crate) fn file_name(name: &str) -> Result<String, OpError> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err("имя файла — без «/» и не «..»".into());
    }
    Ok(name.to_string())
}

/// Абсолютный путь из поля; `what` — для текста ошибки.
pub(crate) fn absolute(path: &str, what: &str) -> Result<PathBuf, OpError> {
    let path = PathBuf::from(path);
    if path.is_absolute() { Ok(path) } else { Err(OpError::Failed(format!("{what} — абсолютный путь"))) }
}

/// Папка из поля `field`: пусто после подстановки — папка пакета, иначе абсолютный путь.
pub(crate) fn folder(ctx: &Ctx, cfg: &Cfg<'_>, field: &str, what: &str) -> Result<PathBuf, OpError> {
    let folder = match cfg.raw(field) {
        Value::Null => String::new(),
        _ => cfg.text(&ctx.scope, field)?,
    };
    if folder.is_empty() { Ok(ctx.files.clone()) } else { absolute(&folder, what) }
}

/// Сколько ждать Download File.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// GET по `url` через прослойку профиля (`relay` — её порт на 127.0.0.1): сайт видит IP профиля. Файл — в `folder`
/// под именем из `Content-Disposition` или адреса; занятое имя не перетирается; тело пишется в файл по частям, не
/// копится в памяти. `None` — не уложилось в `wait`. Недокачанный файл (ошибка, срок, «Стоп») удаляется.
pub(crate) async fn download(relay: Option<u16>, url: &str, folder: &Path, wait: Duration) -> Result<Option<PathBuf>, OpError> {
    let relay = relay.ok_or("Подключение профиля для загрузки файла недоступно")?;
    let proxy = reqwest::Proxy::all(format!("http://127.0.0.1:{relay}")).map_err(|_| "Подключение профиля для загрузки файла недоступно")?;
    let client = reqwest::Client::builder().proxy(proxy).timeout(wait).build().map_err(|_| "загрузчик не создан")?;
    let mut response = match client.get(url).send().await {
        Ok(response) => response,
        Err(e) if e.is_timeout() => return Ok(None),
        Err(e) => return Err(OpError::Failed(net_error(&e))),
    };
    if !response.status().is_success() {
        return Err(OpError::Failed(format!("сервер ответил {}", response.status().as_u16())));
    }
    let name = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .and_then(disposition_name)
        .or_else(|| url_name(response.url()))
        .unwrap_or_else(|| "download".into());
    std::fs::create_dir_all(folder).map_err(|e| format!("папка загрузки не создана: {e}"))?;
    let (path, mut file) = create_new(folder, &name)?;
    let mut partial = Partial(Some(path));
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => file.write_all(&chunk).map_err(|e| format!("файл не записан: {e}"))?,
            Ok(None) => break,
            Err(e) if e.is_timeout() => return Ok(None),
            Err(e) => return Err(OpError::Failed(net_error(&e))),
        }
    }
    Ok(partial.0.take())
}

/// Файл загрузки, пока он не докачан: брошенный (ошибка, срок, отмена узла) удаляется.
struct Partial(Option<PathBuf>);

impl Drop for Partial {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Ошибка сети без адреса: в нём могут быть подставленные значения.
fn net_error(error: &reqwest::Error) -> String {
    if error.is_connect() { "Не удалось установить соединение через подключение профиля".into() } else { "загрузка не удалась".into() }
}

/// Имя из `Content-Disposition`: `filename*=UTF-8''…` важнее `filename="…"`; только последняя часть пути.
fn disposition_name(header: &str) -> Option<String> {
    let mut plain = None;
    for part in header.split(';').map(str::trim) {
        if let Some(value) = part.strip_prefix("filename*=") {
            let encoded = value.rsplit("''").next().unwrap_or(value);
            if let Some(name) = clean_name(&percent_decode(encoded)) {
                return Some(name);
            }
        } else if let Some(value) = part.strip_prefix("filename=") {
            plain = clean_name(value.trim_matches('"'));
        }
    }
    plain
}

/// Имя из последней части пути адреса.
fn url_name(url: &reqwest::Url) -> Option<String> {
    url.path_segments()?.next_back().and_then(|segment| clean_name(&percent_decode(segment)))
}

/// Последняя часть пути; пустое, `.` и `..` — имени нет.
fn clean_name(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    (!name.is_empty() && name != "." && name != "..").then(|| name.to_string())
}

/// `%XX` → байты, затем UTF-8 с заменой.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = if bytes[i] == b'%' { text.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) } else { None };
        match hex {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Новый файл `a.txt`, `a-1.txt`, `a-2.txt`…: `create_new` занимает имя атомарно, поэтому одновременные загрузки одного
/// имени не перетирают друг друга.
pub(crate) fn create_new(folder: &Path, name: &str) -> Result<(PathBuf, std::fs::File), OpError> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 0.. {
        let path = if n == 0 { folder.join(name) } else { folder.join(format!("{stem}-{n}{ext}")) };
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(OpError::Failed(format!("файл не записан: {e}"))),
        }
    }
    unreachable!()
}

/// Случайный файл папки: без вложенных папок и скрытых файлов.
pub(crate) fn random_file(folder: &Path) -> Result<PathBuf, OpError> {
    let entries = std::fs::read_dir(folder).map_err(|e| format!("папка не прочитана: {e}"))?;
    let files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()) && !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .collect();
    if files.is_empty() {
        return Err("в папке нет файлов".into());
    }
    Ok(files[crate::flow::vars::random_index(files.len())].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn import_lines_drop_cr_and_the_last_empty_line() {
        assert_eq!(lines("a\r\nb\n\nc\n"), ["a", "b", "", "c"]);
        assert_eq!(lines(""), Vec::<String>::new());
        assert_eq!(lines("один"), ["один"]);
        assert_eq!(lines("\u{feff}a\r\nb"), ["a", "b"], "метка UTF-8 из Блокнота Windows — не часть первой строки");
    }

    #[test]
    fn names_and_folders() {
        assert_eq!(file_name("снимок"), Ok("снимок".to_string()));
        for bad in ["../x", "a/b", "..", ".", ""] {
            assert!(file_name(bad).is_err(), "{bad}");
        }
        assert!(absolute("/tmp/x", "путь").is_ok());
        assert_eq!(absolute("x", "путь файла"), Err(OpError::Failed("путь файла — абсолютный путь".into())));
    }

    #[tokio::test]
    async fn save_txt_appends_lines_to_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = Ctx::bare();
        ctx.files = dir.path().to_path_buf();
        ctx.scope.set("a", json!("строка")).unwrap();
        let flow = one("data.save_txt", json!({ "file_name": "out", "template": "${a} 1" }));
        ctx.run(&flow).await.unwrap();
        ctx.run(&flow).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "строка 1\nстрока 1\n", "вне задачи — без номера задачи");
        let bad = ctx.run(&one("data.save_txt", json!({ "file_name": "../out", "template": "x" }))).await;
        assert!(matches!(bad, Err(Fail::Node { ref message, .. }) if message == "Save to Txt: имя файла — без «/» и не «..»"), "{bad:?}");
    }

    #[tokio::test]
    async fn concurrent_saves_keep_whole_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("many.txt");
        let line = "x".repeat(10_000);
        let tasks: Vec<_> = (0..8).map(|_| {
            let (file, line) = (file.clone(), line.clone());
            tokio::task::spawn_blocking(move || append_line(&file, &line).unwrap())
        }).collect();
        for task in tasks {
            task.await.unwrap();
        }
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), 8);
        assert!(text.lines().all(|l| l == line));
    }

    #[tokio::test]
    async fn import_txt_reads_lines_of_an_absolute_utf8_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("in.txt");
        std::fs::write(&path, "a\r\nб\n").unwrap();
        let mut ctx = Ctx::bare();
        ctx.run(&one("data.import_txt", json!({ "path": path.to_str().unwrap(), "save_to": "v" }))).await.unwrap();
        assert_eq!(ctx.scope.get("v"), Some(&json!(["a", "б"])));
        let relative = ctx.run(&one("data.import_txt", json!({ "path": "in.txt", "save_to": "v" }))).await;
        assert!(matches!(relative, Err(Fail::Node { ref message, .. }) if message == "Import Data from Txt: путь файла — абсолютный путь"), "{relative:?}");
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let binary = ctx.run(&one("data.import_txt", json!({ "path": path.to_str().unwrap(), "save_to": "v" }))).await;
        assert!(matches!(binary, Err(Fail::Node { ref message, .. }) if message == "Import Data from Txt: файл не в UTF-8"), "{binary:?}");
    }

    #[test]
    fn names_from_headers_and_addresses() {
        assert_eq!(disposition_name(r#"attachment; filename="отчёт.pdf""#), Some("отчёт.pdf".into()));
        assert_eq!(disposition_name("attachment; filename*=UTF-8''%D0%BE%D1%82%D1%87%D1%91%D1%82.pdf; filename=\"x.pdf\""), Some("отчёт.pdf".into()));
        assert_eq!(disposition_name(r#"attachment; filename="../../etc/passwd""#), Some("passwd".into()), "только последняя часть пути");
        assert_eq!(disposition_name("inline"), None);
        assert_eq!(url_name(&reqwest::Url::parse("https://a.test/").unwrap()), None);
        assert_eq!(url_name(&reqwest::Url::parse("https://a.test/d/%D1%84.txt?x=1").unwrap()), Some("ф.txt".into()));
    }

    #[tokio::test]
    async fn download_goes_through_the_relay_and_never_overwrites() {
        // Сервер-«прослойка»: принимает запрос в абсолютной форме, как прокси, и пишет его в журнал.
        let (relay, log) = crate::relay::test_support::spawn_origin(200, "содержимое").await;
        let dir = tempfile::tempdir().unwrap();
        let first = download(Some(relay.port()), "http://files.test/docs/report.txt?x=1", dir.path(), Duration::from_secs(5)).await.unwrap().unwrap();
        let second = download(Some(relay.port()), "http://files.test/docs/report.txt", dir.path(), Duration::from_secs(5)).await.unwrap().unwrap();
        assert_eq!(first, dir.path().join("report.txt"));
        assert_eq!(second, dir.path().join("report-1.txt"), "занятое имя не перетирается");
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "содержимое");
        assert!(log.lock().unwrap().iter().any(|head| head.contains("http://files.test/docs/report.txt")), "запрос ушёл через прослойку");
        assert_eq!(download(None, "http://files.test/x", dir.path(), Duration::from_secs(1)).await, Err(OpError::Failed("Подключение профиля для загрузки файла недоступно".into())));
    }

    #[tokio::test]
    async fn download_reports_timeouts_and_server_errors_without_the_address() {
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = silent.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = silent.accept().await {
                held.push(socket);
            }
        });
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(download(Some(port), "http://files.test/secret-token", dir.path(), Duration::from_millis(300)).await, Ok(None));
        let (relay, _) = crate::relay::test_support::spawn_origin(404, "").await;
        let missing = download(Some(relay.port()), "http://files.test/secret-token", dir.path(), Duration::from_secs(5)).await;
        assert_eq!(missing, Err(OpError::Failed("сервер ответил 404".into())));
    }

    #[tokio::test]
    async fn download_cut_short_leaves_no_file() {
        use tokio::io::AsyncWriteExt;
        // «Прослойка» обещает 1000 байт и отдаёт три: первая закрывает соединение, вторая молчит дальше.
        let partial = |hold: bool| async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                crate::relay::test_support::read_head(&mut socket).await;
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nabc").await.unwrap();
                if hold {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            });
            port
        };
        let dir = tempfile::tempdir().unwrap();
        let closed = download(Some(partial(false).await), "http://files.test/big.bin", dir.path(), Duration::from_secs(5)).await;
        assert_eq!(closed, Err(OpError::Failed("загрузка не удалась".into())));
        let port = partial(true).await;
        let (slow, seen) = tokio::join!(download(Some(port), "http://files.test/big.bin", dir.path(), Duration::from_millis(300)), async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            std::fs::read(dir.path().join("big.bin")).ok()
        });
        assert_eq!(seen.as_deref(), Some(&b"abc"[..]), "тело идёт в файл по частям, не копится в памяти");
        assert_eq!(slow, Ok(None), "срок вышел посреди тела — дальше без ошибки");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "недокачанные файлы удалены");
    }

    #[test]
    fn random_file_skips_folders_and_hidden_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join(".hidden"), "x").unwrap();
        assert_eq!(random_file(dir.path()), Err(OpError::Failed("в папке нет файлов".into())));
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        assert_eq!(random_file(dir.path()).unwrap(), dir.path().join("a.txt"));
    }

    #[tokio::test]
    async fn concurrent_downloads_of_one_name_get_distinct_files() {
        let (relay, _) = crate::relay::test_support::spawn_origin(200, "содержимое").await;
        let dir = tempfile::tempdir().unwrap();
        let go = || download(Some(relay.port()), "http://files.test/report.txt", dir.path(), Duration::from_secs(5));
        let (a, b) = tokio::join!(go(), go());
        let (a, b) = (a.unwrap().unwrap(), b.unwrap().unwrap());
        assert_ne!(a, b);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "содержимое");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "содержимое");
    }
}
