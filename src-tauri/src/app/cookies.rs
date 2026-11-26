//! Импорт и экспорт cookies профиля (спецификация 2А, «Cookies»): напрямую с базой, работающему профилю — сразу и в браузер.

use super::bulk::BulkReport;
use super::profiles::not_found;
use super::{CommandError, Core};
use crate::cookies::{CookieFormat, Skipped};
use crate::engine::Engine;
use crate::paths::{CHROMIUM_COOKIE_FILES, remove_path};
pub use crate::store::cookies::CookieStats;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

/// Итог импорта: сколько добавлено и заменено, какие записи пропущены и ушло ли в работающий браузер.
#[derive(Debug, Clone, Serialize)]
pub struct CookieImportReport {
    pub added: usize,
    pub replaced: usize,
    pub skipped: Vec<Skipped>,
    pub sent_to_browser: bool,
}

/// Куда выгрузить cookies: путь выбирает пользователь в окне сохранения или выбора папки.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportTarget {
    /// Один профиль — в этот файл.
    File { path: PathBuf },
    /// Несколько профилей — по файлу на профиль в этой папке.
    Folder { path: PathBuf },
}

/// Откуда импорт: текст из поля диалога или файл, выбранный в окне (файл читает ядро).
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CookieSource {
    Text { text: String },
    File { path: PathBuf },
}

// В тексте — значения cookies: `Debug` показывает только длину.
impl fmt::Debug for CookieSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { text } => f.debug_struct("Text").field("len", &text.len()).finish(),
            Self::File { path } => f.debug_struct("File").field("path", path).finish(),
        }
    }
}

/// Файл cookies больше этого — `ImportFailed`: экспорт AdsPower на тысячи записей укладывается с запасом.
pub const COOKIE_FILE_MAX: u64 = 10 * 1024 * 1024;

/// Не больше `COOKIE_FILE_MAX` байт: читается на байт больше предела, так что растущий файл предел не обойдёт.
async fn read_cookie_file(path: &Path) -> Result<String, CommandError> {
    let file = tokio::fs::File::open(path).await?;
    let mut bytes = Vec::new();
    file.take(COOKIE_FILE_MAX + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > COOKIE_FILE_MAX {
        return Err(CommandError::new("ImportFailed", "файл cookies больше 10 МБ"));
    }
    String::from_utf8(bytes).map_err(|_| CommandError::new("ImportFailed", "файл cookies не в UTF-8"))
}

const FILE_NAME_CHARS: usize = 100;
/// APFS принимает имена до 255 байт; с `-<id>` и расширением 200 байт имени укладываются.
const FILE_NAME_BYTES: usize = 200;

/// `<имя>[-<id>].json|.txt`: `/`, `:`, `\` и управляющие символы → `_`, имя — не длиннее 100 символов и 200 байт;
/// пустое имя, `.` и `..` → `_`.
pub fn export_file_name(name: &str, id: Option<&str>, format: CookieFormat) -> String {
    let mut base: String = name
        .chars()
        .map(|c| if matches!(c, '/' | ':' | '\\') || c.is_control() { '_' } else { c })
        .take(FILE_NAME_CHARS)
        .collect();
    while base.len() > FILE_NAME_BYTES {
        base.pop();
    }
    if base.trim().is_empty() || base == "." || base == ".." {
        base = "_".into();
    }
    let extension = match format {
        CookieFormat::Json => "json",
        CookieFormat::Netscape => "txt",
    };
    match id {
        Some(id) => format!("{base}-{id}.{extension}"),
        None => format!("{base}.{extension}"),
    }
}

/// Файл экспорта целиком или никак (спецификация 2В-3, «Экспорт cookies»): новый временный файл в папке цели
/// (`O_EXCL`, случайное имя, права 0600), `sync_all`, переименование поверх цели. Символическая ссылка на месте цели
/// заменяется файлом, а не используется; при ошибке прежний файл цел, временный удаляется (`NamedTempFile`).
pub(crate) fn write_export_file(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut file = tempfile::Builder::new().prefix(".ombra-export-").suffix(".tmp").tempfile_in(dir)?;
    file.write_all(text.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

impl<E: Engine> Core<E> {
    /// JSON (AdsPower) или `cookies.txt` — формат определяется сам. Совпадающие по домену, имени и пути заменяются.
    /// Работающему профилю cookies сначала уходят в браузер, потом в базу; если браузер не принял — `CookiesFailed`, база не меняется.
    pub async fn import_cookies(&self, id: &str, text: &str) -> Result<CookieImportReport, CommandError> {
        self.store.get_profile(id)?.ok_or_else(not_found)?;
        let parsed = crate::cookies::parse(text)?;
        let (count, sent_to_browser) = self.supervisor.import_cookies(id, parsed.cookies).await?;
        Ok(CookieImportReport { added: count.added, replaced: count.replaced, skipped: parsed.skipped, sent_to_browser })
    }

    /// Импорт из диалога «Cookies»: текст из поля или файл. Файл больше `COOKIE_FILE_MAX` или не UTF-8 — `ImportFailed`,
    /// не читается — `Io`; дальше — как `import_cookies`.
    pub async fn import_cookies_from(&self, id: &str, source: &CookieSource) -> Result<CookieImportReport, CommandError> {
        self.store.get_profile(id)?.ok_or_else(not_found)?;
        match source {
            CookieSource::Text { text } => self.import_cookies(id, text).await,
            CookieSource::File { path } => {
                let text = read_cookie_file(path).await?;
                self.import_cookies(id, &text).await
            }
        }
    }

    /// Весь набор профиля из базы; браузер для этого не нужен.
    pub fn export_cookies(&self, id: &str, format: CookieFormat) -> Result<String, CommandError> {
        self.store.get_profile(id)?.ok_or_else(not_found)?;
        Ok(crate::cookies::export(&self.store.load_cookies(id)?, format))
    }

    /// Экспорт в файлы (спецификация 2В-1, «Экспорт cookies»): один профиль — в выбранный файл, несколько — по файлу
    /// `<имя>-<id>` в выбранной папке. Существующие файлы заменяются целиком (`write_export_file`); ошибки профилей — в отчёте.
    pub fn export_cookies_to(
        &self,
        ids: &[String],
        format: CookieFormat,
        target: &ExportTarget,
    ) -> Result<BulkReport, CommandError> {
        let mut report = BulkReport::default();
        match target {
            ExportTarget::File { path } => {
                let [id] = ids else {
                    return Err(CommandError::new("Validation", "в файл выгружается один профиль"));
                };
                let result = self
                    .export_cookies(id, format)
                    .and_then(|text| write_export_file(path, &text).map_err(CommandError::from));
                report.push(id.clone(), result);
            }
            ExportTarget::Folder { path } => {
                if !path.is_dir() {
                    return Err(CommandError::new("Io", "папка для экспорта не найдена"));
                }
                for id in ids {
                    let result = self.export_into_folder(id, format, path);
                    report.push(id.clone(), result);
                }
            }
        }
        Ok(report)
    }

    fn export_into_folder(&self, id: &str, format: CookieFormat, folder: &Path) -> Result<(), CommandError> {
        let profile = self.store.get_profile(id)?.ok_or_else(not_found)?;
        let file = folder.join(export_file_name(&profile.name, Some(id), format));
        // Имя очищено от разделителей, но запись вне выбранной папки недопустима при любом имени.
        if file.parent() != Some(folder) {
            return Err(CommandError::new("Io", "файл экспорта оказался вне выбранной папки"));
        }
        write_export_file(&file, &self.export_cookies(id, format)?)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn cookie_count(&self, id: &str) -> Result<i64, CommandError> {
        self.store.get_profile(id)?.ok_or_else(not_found)?;
        Ok(self.store.count_cookies(id)?)
    }

    /// «Cookies профиля: N» и «K записей не удалось прочитать» во вкладке «Cookies».
    pub fn cookie_stats(&self, id: &str) -> Result<CookieStats, CommandError> {
        self.store.get_profile(id)?.ok_or_else(not_found)?;
        Ok(self.store.cookie_stats(id)?)
    }

    /// «Очистить cookies» остановленного профиля: набор в базе и хранилище cookies Chromium в папке профиля —
    /// иначе браузер вернул бы их при следующем запуске, а снимок записал бы обратно. Запущенный — `ProfileRunning`.
    pub async fn clear_cookies(&self, id: &str) -> Result<(), CommandError> {
        let _idle = self.supervisor.lock_idle(id).await?;
        self.store.get_profile(id)?.ok_or_else(not_found)?;
        let dir = self.paths.profile_dir(id);
        for relative in CHROMIUM_COOKIE_FILES {
            remove_path(&dir.join(relative))?;
        }
        self.store.clear_cookies(id)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{create_with_proxy, env, form};
    use crate::cookies::Cookie;

    /// Образец AdsPower: доменная cookie с дробным сроком, сессионная host-only и ошибочная запись без домена.
    const JSON: &str = r#"[
      {"domain": ".shop.example", "name": "sid", "value": "abc", "path": "/", "expirationDate": 1893456000.5,
       "httpOnly": true, "secure": true, "sameSite": "no_restriction", "session": false, "hostOnly": false},
      {"domain": "shop.example", "name": "lang", "value": "ru", "path": "/", "session": true},
      {"name": "без домена", "value": "x"}
    ]"#;

    /// Та же `sid` с новым значением и новая `cart`.
    const NETSCAPE: &str = "# Netscape HTTP Cookie File\n\
        .shop.example\tTRUE\t/\tTRUE\t1893456000\tsid\txyz\n\
        shop.example\tFALSE\t/cart\tFALSE\t0\tcart\t3\n";

    fn key(cookie: &Cookie) -> (String, String, String) {
        (cookie.domain.clone(), cookie.name.clone(), cookie.path.clone())
    }

    #[tokio::test]
    async fn import_into_stopped_profile_updates_store_only() {
        let e = env().await;
        let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
        let report = e.core.import_cookies(&rec.id, JSON).await.unwrap();
        assert_eq!((report.added, report.replaced, report.sent_to_browser), (2, 0, false));
        assert_eq!(report.skipped.iter().map(|s| s.line).collect::<Vec<_>>(), [3]);
        assert_eq!(report.skipped[0].code, crate::cookies::SkipCode::NoDomain, "запись без домена");
        assert_eq!(e.core.cookie_count(&rec.id).unwrap(), 2);

        let report = e.core.import_cookies(&rec.id, NETSCAPE).await.unwrap();
        assert_eq!((report.added, report.replaced, report.skipped.len()), (1, 1, 0));
        let stored = e.core.store.load_cookies(&rec.id).unwrap();
        let sid = stored.iter().find(|c| c.name == "sid").unwrap();
        assert_eq!((sid.value.as_str(), sid.expires, sid.secure, sid.host_only), ("xyz", Some(1893456000), true, false));
        let lang = stored.iter().find(|c| c.name == "lang").unwrap();
        assert_eq!((lang.expires, lang.host_only), (None, true), "сессионная host-only сохраняется как есть");
        assert_eq!(e.core.cookie_count(&rec.id).unwrap(), 3);
    }

    #[tokio::test]
    async fn import_into_running_profile_reaches_browser() {
        let e = env().await;
        let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
        e.core.supervisor.start(&rec.id).await.unwrap();
        let report = e.core.import_cookies(&rec.id, JSON).await.unwrap();
        assert_eq!((report.added, report.sent_to_browser), (2, true));
        let browser = e.core.engine.cdp(&e.core.paths.profile_dir(&rec.id)).browser_cookies();
        assert!(browser.iter().any(|c| c.name == "sid" && c.value == "abc"), "{browser:?}");
        assert_eq!(e.core.cookie_count(&rec.id).unwrap(), 2);
        e.core.supervisor.stop(&rec.id).await.unwrap();
    }

    #[tokio::test]
    async fn broken_file_is_import_failed_and_changes_nothing() {
        let e = env().await;
        let rec = e.core.create_profile(&form("a", None), 1).unwrap();
        e.core.import_cookies(&rec.id, JSON).await.unwrap();
        for text in ["", "[{\"domain\": ", "not a cookie line\nanother bad line"] {
            let err = e.core.import_cookies(&rec.id, text).await.unwrap_err();
            assert_eq!(err.code, "ImportFailed", "{text:?}");
        }
        assert_eq!(e.core.cookie_count(&rec.id).unwrap(), 2, "база не тронута");
    }

    #[tokio::test]
    async fn export_round_trips_in_both_formats() {
        let e = env().await;
        let rec = e.core.create_profile(&form("a", None), 1).unwrap();
        e.core.import_cookies(&rec.id, JSON).await.unwrap();
        e.core.import_cookies(&rec.id, NETSCAPE).await.unwrap();
        let stored = e.core.store.load_cookies(&rec.id).unwrap();
        for format in [CookieFormat::Json, CookieFormat::Netscape] {
            let text = e.core.export_cookies(&rec.id, format).unwrap();
            let mut back = crate::cookies::parse(&text).unwrap().cookies;
            back.sort_by_key(key);
            // В cookies.txt нет SameSite.
            let expected: Vec<Cookie> = stored
                .iter()
                .cloned()
                .map(|mut c| {
                    if format == CookieFormat::Netscape {
                        c.same_site = None;
                    }
                    c
                })
                .collect();
            assert_eq!(back, expected, "{format:?}");
        }
        assert!(!format!("{stored:?}").contains("xyz"), "значения cookies не попадают в Debug");
    }

    #[tokio::test]
    async fn unknown_profile_is_not_found() {
        let e = env().await;
        assert_eq!(e.core.import_cookies("nope00", JSON).await.unwrap_err().code, "NotFound");
        assert_eq!(e.core.export_cookies("nope00", CookieFormat::Json).unwrap_err().code, "NotFound");
        assert_eq!(e.core.cookie_count("nope00").unwrap_err().code, "NotFound");
    }

    fn read(path: &std::path::Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn export_file_names_are_cleaned() {
        assert_eq!(export_file_name("Магазин 7", None, CookieFormat::Json), "Магазин 7.json");
        assert_eq!(export_file_name("a/b:c\\d", Some("k7f2q9"), CookieFormat::Netscape), "a_b_c_d-k7f2q9.txt");
        assert_eq!(export_file_name("../x", Some("k7f2q9"), CookieFormat::Json), ".._x-k7f2q9.json");
        assert_eq!(export_file_name("a\nb\tc", None, CookieFormat::Json), "a_b_c.json");
        assert_eq!(export_file_name("..", None, CookieFormat::Json), "_.json");
        assert_eq!(export_file_name(".", Some("k7f2q9"), CookieFormat::Json), "_-k7f2q9.json");
        assert_eq!(export_file_name("", None, CookieFormat::Netscape), "_.txt");
        assert_eq!(
            export_file_name(&"я".repeat(150), Some("k7f2q9"), CookieFormat::Json),
            format!("{}-k7f2q9.json", "я".repeat(100)),
            "не длиннее 100 символов"
        );
        assert_eq!(
            export_file_name(&"🙂".repeat(100), None, CookieFormat::Json),
            format!("{}.json", "🙂".repeat(50)),
            "не длиннее 200 байт"
        );
    }

    #[tokio::test]
    async fn one_profile_goes_to_the_chosen_file_in_both_formats() {
        let e = env().await;
        let rec = e.core.create_profile(&form("a", None), 1).unwrap();
        e.core.import_cookies(&rec.id, JSON).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        for (format, file) in [(CookieFormat::Json, "c.json"), (CookieFormat::Netscape, "c.txt")] {
            let path = dir.path().join(file);
            std::fs::write(&path, "старое содержимое").unwrap();
            let target = ExportTarget::File { path: path.clone() };
            let report = e.core.export_cookies_to(std::slice::from_ref(&rec.id), format, &target).unwrap();
            assert_eq!((report.ok.len(), report.failed.len()), (1, 0), "{format:?}");
            let text = read(&path);
            assert_eq!(text, e.core.export_cookies(&rec.id, format).unwrap(), "существующий файл перезаписан");
            assert_eq!(crate::cookies::parse(&text).unwrap().cookies.len(), 2, "{format:?}");
        }
    }

    #[tokio::test]
    async fn folder_gets_one_cleaned_file_per_profile_inside_it() {
        let e = env().await;
        let names = ["../x", "a/b:c", "..", "a\nb", "Близнец", "Близнец"];
        let mut ids = Vec::new();
        for name in names {
            ids.push(e.core.create_profile(&form(name, None), 1).unwrap().id);
        }
        e.core.import_cookies(&ids[0], JSON).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("out");
        std::fs::create_dir(&folder).unwrap();
        let report = e.core.export_cookies_to(&ids, CookieFormat::Json, &ExportTarget::Folder { path: folder.clone() }).unwrap();
        assert_eq!(report.ok, ids);
        assert!(report.failed.is_empty());

        let mut files: Vec<String> =
            std::fs::read_dir(&folder).unwrap().map(|f| f.unwrap().file_name().into_string().unwrap()).collect();
        files.sort();
        let mut expected: Vec<String> = [".._x", "a_b_c", "_", "a_b", "Близнец", "Близнец"]
            .iter()
            .zip(&ids)
            .map(|(base, id)| format!("{base}-{id}.json"))
            .collect();
        expected.sort();
        assert_eq!(files, expected, "очищенные имена; у одинаковых имён — разные файлы");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "вне выбранной папки ничего не записано");

        let with_cookies = read(&folder.join(format!(".._x-{}.json", ids[0])));
        assert_eq!(crate::cookies::parse(&with_cookies).unwrap().cookies.len(), 2);
        assert_eq!(read(&folder.join(format!("_-{}.json", ids[2]))).trim(), "[]", "профиль без cookies — пустой набор");
    }

    #[tokio::test]
    async fn export_target_errors() {
        let e = env().await;
        let a = e.core.create_profile(&form("a", None), 1).unwrap();
        let b = e.core.create_profile(&form("b", None), 2).unwrap();
        let both = [a.id.clone(), b.id.clone()];
        let dir = tempfile::tempdir().unwrap();

        let file = ExportTarget::File { path: dir.path().join("x.json") };
        assert_eq!(e.core.export_cookies_to(&both, CookieFormat::Json, &file).unwrap_err().code, "Validation");
        let unknown = e.core.export_cookies_to(&["nope00".to_string()], CookieFormat::Json, &file).unwrap();
        assert_eq!(unknown.failed[0].error.code, "NotFound");
        assert!(!dir.path().join("x.json").exists(), "для неизвестного профиля файл не создаётся");
        let unwritable = ExportTarget::File { path: dir.path().join("нет").join("x.json") };
        let report = e.core.export_cookies_to(std::slice::from_ref(&a.id), CookieFormat::Json, &unwritable).unwrap();
        assert_eq!(report.failed[0].error.code, "Io");

        let missing = ExportTarget::Folder { path: dir.path().join("нет") };
        assert_eq!(e.core.export_cookies_to(&both, CookieFormat::Json, &missing).unwrap_err().code, "Io");
        assert!(!dir.path().join("нет").exists());

        let folder = ExportTarget::Folder { path: dir.path().to_path_buf() };
        let mixed = [a.id.clone(), "nope00".to_string(), b.id.clone()];
        let report = e.core.export_cookies_to(&mixed, CookieFormat::Netscape, &folder).unwrap();
        assert_eq!(report.ok, [a.id.clone(), b.id.clone()]);
        assert_eq!((report.failed[0].id.as_str(), report.failed[0].error.code.as_str()), ("nope00", "NotFound"));
        assert_eq!(read(&dir.path().join(format!("b-{}.txt", b.id))), "# Netscape HTTP Cookie File\n");
    }

    #[tokio::test]
    async fn export_replaces_a_symlink_instead_of_writing_through_it() {
        let e = env().await;
        let rec = e.core.create_profile(&form("a", None), 1).unwrap();
        e.core.import_cookies(&rec.id, JSON).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, "чужой файл").unwrap();

        let link = dir.path().join("c.json");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let target = ExportTarget::File { path: link.clone() };
        let report = e.core.export_cookies_to(std::slice::from_ref(&rec.id), CookieFormat::Json, &target).unwrap();
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(read(&victim), "чужой файл", "по ссылке ничего не записано");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_file(), "ссылка заменена файлом");
        assert_eq!(read(&link), e.core.export_cookies(&rec.id, CookieFormat::Json).unwrap());

        // Папка: ссылка на месте файла профиля указывает за её пределы.
        let folder = dir.path().join("out");
        std::fs::create_dir(&folder).unwrap();
        let planted = folder.join(export_file_name("a", Some(&rec.id), CookieFormat::Netscape));
        std::os::unix::fs::symlink(&victim, &planted).unwrap();
        let target = ExportTarget::Folder { path: folder.clone() };
        let report = e.core.export_cookies_to(std::slice::from_ref(&rec.id), CookieFormat::Netscape, &target).unwrap();
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(read(&victim), "чужой файл");
        assert!(std::fs::symlink_metadata(&planted).unwrap().file_type().is_file());
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1, "временных файлов не осталось");
    }

    #[tokio::test]
    async fn failed_export_keeps_the_previous_file() {
        use std::os::unix::fs::PermissionsExt;
        let e = env().await;
        let rec = e.core.create_profile(&form("a", None), 1).unwrap();
        e.core.import_cookies(&rec.id, JSON).await.unwrap();
        let ids = std::slice::from_ref(&rec.id);
        let dir = tempfile::tempdir().unwrap();

        // На месте файла — непустая папка: переименование поверх неё не проходит, временный файл убирается.
        let occupied = dir.path().join("c.json");
        std::fs::create_dir(&occupied).unwrap();
        std::fs::write(occupied.join("inside"), "x").unwrap();
        let target = ExportTarget::File { path: occupied.clone() };
        let report = e.core.export_cookies_to(ids, CookieFormat::Json, &target).unwrap();
        assert_eq!(report.failed[0].error.code, "Io");
        assert_eq!(read(&occupied.join("inside")), "x");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "временный файл убран");

        // Папка только для чтения: временный файл не создаётся, прежний файл цел.
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let old = locked.join("c.json");
        std::fs::write(&old, "старое содержимое").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::File::create(locked.join("probe")).is_ok() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            return; // root: права не мешают, сбой не воспроизвести
        }
        let target = ExportTarget::File { path: old.clone() };
        let report = e.core.export_cookies_to(ids, CookieFormat::Json, &target).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(report.failed[0].error.code, "Io");
        assert_eq!(read(&old), "старое содержимое", "прежний файл цел");
    }

    #[test]
    fn export_target_comes_from_the_interface_as_tagged_json() {
        let file: ExportTarget = serde_json::from_str(r#"{"kind":"file","path":"/tmp/a.json"}"#).unwrap();
        assert!(matches!(file, ExportTarget::File { path } if path == std::path::Path::new("/tmp/a.json")));
        let folder: ExportTarget = serde_json::from_str(r#"{"kind":"folder","path":"/tmp"}"#).unwrap();
        assert!(matches!(folder, ExportTarget::Folder { .. }));
    }

    #[tokio::test]
    async fn unreadable_cookie_is_skipped_counted_and_does_not_block_launch() {
        let e = env().await;
        let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
        e.core.import_cookies(&rec.id, JSON).await.unwrap();
        e.core
            .store
            .raw()
            .execute("UPDATE cookies SET value_enc = zeroblob(64) WHERE profile_id = ?1 AND name = 'sid'", [&rec.id])
            .unwrap();
        assert_eq!(e.core.cookie_stats(&rec.id).unwrap(), CookieStats { total: 2, unreadable: 1 });
        let names: Vec<String> = e.core.store.load_cookies(&rec.id).unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["lang"]);
        e.core.supervisor.start(&rec.id).await.expect("нечитаемая cookie не мешает запуску");
        let browser = e.core.engine.cdp(&e.core.paths.profile_dir(&rec.id)).browser_cookies();
        assert!(browser.iter().any(|c| c.name == "lang") && !browser.iter().any(|c| c.name == "sid"), "{browser:?}");
        e.core.supervisor.stop(&rec.id).await.unwrap();
        assert_eq!(e.core.cookie_stats("nope00").unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn clear_cookies_only_for_stopped_profile() {
        let e = env().await;
        let rec = create_with_proxy(&e.core, "a", e.proxy_port, 1);
        e.core.import_cookies(&rec.id, JSON).await.unwrap();
        e.core.supervisor.start(&rec.id).await.unwrap();
        assert_eq!(e.core.clear_cookies(&rec.id).await.unwrap_err().code, "ProfileRunning");
        assert_eq!(e.core.cookie_stats(&rec.id).unwrap().total, 2, "у работающего ничего не удалено");
        e.core.supervisor.stop(&rec.id).await.unwrap();

        let dir = e.core.paths.profile_dir(&rec.id);
        let stores: Vec<PathBuf> = CHROMIUM_COOKIE_FILES.iter().map(|relative| dir.join(relative)).collect();
        let history = dir.join("Default/History");
        for path in stores.iter().chain([&history]) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        e.core.clear_cookies(&rec.id).await.unwrap();
        assert_eq!(e.core.cookie_stats(&rec.id).unwrap(), CookieStats { total: 0, unreadable: 0 });
        assert!(stores.iter().all(|path| !path.exists()), "хранилище cookies Chromium удалено");
        assert!(history.exists(), "остальная папка профиля не тронута");
        assert_eq!(e.core.clear_cookies("nope00").await.unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn import_reads_text_or_file_with_a_size_limit() {
        let e = env().await;
        let rec = e.core.create_profile(&form("a", None), 1).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cookies.json");
        std::fs::write(&file, JSON).unwrap();
        let report = e.core.import_cookies_from(&rec.id, &CookieSource::File { path: file }).await.unwrap();
        assert_eq!((report.added, report.skipped.len()), (2, 1));
        let text = CookieSource::Text { text: NETSCAPE.into() };
        let report = e.core.import_cookies_from(&rec.id, &text).await.unwrap();
        assert_eq!((report.added, report.replaced), (1, 1));

        let big = dir.path().join("big.txt");
        std::fs::write(&big, vec![b'#'; 11 * 1024 * 1024]).unwrap();
        let err = e.core.import_cookies_from(&rec.id, &CookieSource::File { path: big }).await.unwrap_err();
        assert_eq!(err.code, "ImportFailed");
        let missing = CookieSource::File { path: dir.path().join("нет.json") };
        assert_eq!(e.core.import_cookies_from(&rec.id, &missing).await.unwrap_err().code, "Io");
        assert_eq!(e.core.cookie_stats(&rec.id).unwrap().total, 3, "ошибки ничего не меняют");
        assert_eq!(e.core.import_cookies_from("nope00", &text).await.unwrap_err().code, "NotFound");
    }

    #[test]
    fn cookie_source_comes_as_tagged_json_and_hides_text() {
        let text: CookieSource = serde_json::from_str(r#"{"kind":"text","text":"sid=s3cr3t-value"}"#).unwrap();
        let shown = format!("{text:?}");
        assert!(!shown.contains("s3cr3t") && shown.contains("16"), "{shown}");
        let file: CookieSource = serde_json::from_str(r#"{"kind":"file","path":"/tmp/c.json"}"#).unwrap();
        assert!(matches!(file, CookieSource::File { path } if path == std::path::Path::new("/tmp/c.json")));
    }
}
