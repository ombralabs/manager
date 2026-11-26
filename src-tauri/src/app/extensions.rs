//! Библиотека расширений, наборы и назначение профилям (спецификация 3Б).
use super::bulk::BulkReport;
use super::{CommandError, Core};
use crate::engine::Engine;
use crate::extensions::id::{is_extension_id, store_id};
use crate::extensions::{FormatError, PackageError, crx, package};
use crate::fingerprint::FieldError;
use crate::paths::Paths;
use crate::store::Store;
use crate::store::extensions::{ExtCategory, ExtensionRecord};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Адрес скачивания пакета из Chrome Web Store: `{id}` — расширение, `{version}` — версия установленного движка.
pub const STORE_URL: &str = "https://clients2.google.com/service/update2/crx?response=redirect&prodversion={version}\
                             &acceptformat=crx3&x=id%3D{id}%26installsource%3Dondemand%26uc";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// Поиск Chrome Web Store для системного браузера; пустой запрос — главная магазина.
pub fn store_search_url(query: &str) -> String {
    let mut url = reqwest::Url::parse("https://chromewebstore.google.com/").expect("адрес магазина");
    let query = query.trim();
    if !query.is_empty() {
        // `push` кодирует сегмент: `/`, `?`, `#` и `%` из запроса не меняют адрес.
        url.path_segments_mut().expect("адрес с путём").clear().push("search").push(query);
    }
    url.into()
}

/// Страница расширения в Chrome Web Store — ссылка «Chrome Web Store» на карточке; принимает только сам ID.
pub fn store_page_url(id: &str) -> Option<String> {
    is_extension_id(id).then(|| format!("https://chromewebstore.google.com/detail/{id}"))
}

/// Операции библиотеки идут по одной; запуск профилей замок не берёт.
pub struct ExtensionLibrary {
    lock: tokio::sync::Mutex<()>,
    /// Шаблон адреса скачивания; тесты подменяют его локальным сервером.
    pub(crate) store_url: String,
}

impl Default for ExtensionLibrary {
    fn default() -> Self {
        Self { lock: tokio::sync::Mutex::new(()), store_url: STORE_URL.into() }
    }
}

enum Package {
    Crx(Vec<u8>),
    Zip(Vec<u8>),
    Folder(PathBuf),
}

impl From<PackageError> for CommandError {
    fn from(e: PackageError) -> Self {
        match e {
            PackageError::Format(format) => format_error(format),
            PackageError::Io(io) => io.into(),
        }
    }
}

fn format_error(e: FormatError) -> CommandError {
    CommandError {
        code: "ExtensionFormat".into(),
        message: e.to_string(),
        fields: Some(vec![FieldError { field: "package", code: e.reason() }]),
        owner_hint: None,
    }
}

fn download_error(message: impl Into<String>) -> CommandError {
    CommandError::new("ExtensionDownload", message)
}

fn field_error(field: &'static str) -> CommandError {
    CommandError::validation(vec![FieldError { field, code: "invalid" }])
}

impl<E: Engine> Core<E> {
    pub fn list_extensions(&self) -> Result<Vec<ExtensionRecord>, CommandError> {
        Ok(self.store.list_extensions()?)
    }

    /// `input` — идентификатор или ссылка Chrome Web Store.
    pub async fn add_store_extension(&self, input: &str, now: i64) -> Result<ExtensionRecord, CommandError> {
        let id = store_id(input).ok_or_else(|| field_error("input"))?;
        let _one = self.library.lock.lock().await;
        self.install_from_store(&id, now).await
    }

    /// `.crx`, `.zip` или папка с `manifest.json`.
    pub async fn add_file_extension(&self, path: &Path, now: i64) -> Result<ExtensionRecord, CommandError> {
        // Имя файла или папки — строка «Источник» на карточке.
        let file_name = path.file_name().map(|name| name.to_string_lossy().into_owned());
        let package = if path.is_dir() {
            Package::Folder(path.to_path_buf())
        } else {
            let kind = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
            if !matches!(kind.as_deref(), Some("crx" | "zip")) {
                return Err(format_error(FormatError::Zip));
            }
            if tokio::fs::metadata(path).await?.len() > package::LIMITS.bytes {
                return Err(format_error(FormatError::TooLarge));
            }
            let bytes = tokio::fs::read(path).await?;
            if kind.as_deref() == Some("crx") { Package::Crx(bytes) } else { Package::Zip(bytes) }
        };
        let _one = self.library.lock.lock().await;
        self.install(package, None, "file", file_name, now).await
    }

    /// Только у расширений из магазина: скачать заново; версия не новее — запись прежняя.
    pub async fn update_extension(&self, id: &str, now: i64) -> Result<ExtensionRecord, CommandError> {
        let _one = self.library.lock.lock().await;
        let current = self.store.get_extension(id)?.ok_or_else(|| CommandError::new("NotFound", "расширение не найдено"))?;
        if current.source != "store" {
            return Err(field_error("source"));
        }
        self.install_from_store(id, now).await
    }

    /// Из библиотеки и всех наборов; файлы удалит очистка при старте, данные расширения в профилях остаются.
    pub async fn remove_extension(&self, id: &str) -> Result<(), CommandError> {
        let _one = self.library.lock.lock().await;
        Ok(self.store.delete_extension(id)?)
    }

    /// Чистка при старте окна (`sweep_when_alone`) — под замком библиотеки: Local API и окно уже принимают команды,
    /// и установка, начатая во время уборки сирот, не потеряет свою `.tmp-*` и новую версию.
    pub async fn sweep_extensions_when_alone(&self) -> Result<bool, CommandError> {
        let _one = self.library.lock.lock().await;
        let (paths, store) = (self.paths.clone(), self.store.clone());
        tokio::task::spawn_blocking(move || sweep_when_alone(&paths, &store))
            .await
            .map_err(|e| CommandError::new("Internal", format!("Очистка библиотеки прервана: {e}")))?
    }

    async fn install_from_store(&self, id: &str, now: i64) -> Result<ExtensionRecord, CommandError> {
        let version = self
            .engine
            .installed()
            .map(|build| build.version)
            .unwrap_or_else(|| crate::engine::ombra::install::EXPECTED_VERSION.to_string());
        let url = self.library.store_url.replace("{id}", id).replace("{version}", &version);
        let bytes = download(&url).await?;
        self.install(Package::Crx(bytes), Some(id.to_string()), "store", None, now).await
    }

    async fn install(
        &self,
        package: Package,
        expect: Option<String>,
        source: &'static str,
        file_name: Option<String>,
        now: i64,
    ) -> Result<ExtensionRecord, CommandError> {
        let (paths, store) = (self.paths.clone(), self.store.clone());
        tokio::task::spawn_blocking(move || install_blocking(&paths, &store, package, expect.as_deref(), source, file_name, now))
            .await
            .map_err(|e| CommandError::new("Internal", format!("фоновая задача прервана: {e}")))?
    }
}

/// Без прокси, не больше `LIMITS.bytes`; сеть, не 2xx или пустой ответ — `ExtensionDownload`.
async fn download(url: &str) -> Result<Vec<u8>, CommandError> {
    let failed = |_: reqwest::Error| download_error("не удалось скачать расширение из Chrome Web Store");
    let client = reqwest::Client::builder().no_proxy().timeout(DOWNLOAD_TIMEOUT).build().map_err(failed)?;
    let mut response = client.get(url).send().await.map_err(failed)?;
    if !response.status().is_success() {
        return Err(download_error(format!("Chrome Web Store ответил {}", response.status().as_u16())));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(failed)? {
        body.extend_from_slice(&chunk);
        if body.len() as u64 > package::LIMITS.bytes {
            return Err(format_error(FormatError::TooLarge));
        }
    }
    if body.is_empty() {
        return Err(download_error("Chrome Web Store вернул пустой ответ"));
    }
    Ok(body)
}

/// Распаковка во временную папку библиотеки, манифест, перенос в `<id>/<version>/`, запись в базу.
/// Та же или старшая версия уже в библиотеке и её папка цела — возвращается существующая запись.
fn install_blocking(
    paths: &Paths,
    store: &Store,
    package: Package,
    expect: Option<&str>,
    source: &str,
    file_name: Option<String>,
    now: i64,
) -> Result<ExtensionRecord, CommandError> {
    let library = paths.extensions();
    std::fs::create_dir_all(&library)?;
    // `TempDir` удаляет папку на любом пути выхода, в том числе после переноса корня.
    let temp = tempfile::Builder::new().prefix(".tmp-").tempdir_in(&library)?;
    let key = match package {
        Package::Crx(bytes) => {
            let parsed = crx::parse(&bytes).map_err(format_error)?;
            if expect.is_some_and(|id| id != parsed.id) {
                return Err(format_error(FormatError::IdMismatch));
            }
            package::unzip(parsed.zip, temp.path(), &package::LIMITS)?;
            Some(parsed.key)
        }
        Package::Zip(bytes) => {
            package::unzip(&bytes, temp.path(), &package::LIMITS)?;
            None
        }
        Package::Folder(dir) => {
            package::copy_tree(&dir, temp.path(), &package::LIMITS)?;
            None
        }
    };
    let root = package::root(temp.path()).map_err(format_error)?;
    let unpacked = package::finish(&root, key.as_deref())?;
    let existing = store.get_extension(&unpacked.id)?;
    if let Some(current) = &existing
        && !package::newer(&unpacked.version, &current.version)
        && paths.extension_dir(&current.id, &current.version).join("manifest.json").is_file()
    {
        return Ok(current.clone());
    }
    let target = paths.extension_dir(&unpacked.id, &unpacked.version);
    // Папка той же версии могла остаться от удалённой записи до очистки при старте.
    crate::paths::remove_path(&target)?;
    std::fs::create_dir_all(target.parent().expect("у папки версии есть родитель"))?;
    std::fs::rename(&root, &target)?;
    let record = ExtensionRecord {
        id: unpacked.id,
        name: unpacked.name,
        version: unpacked.version,
        source: source.into(),
        manifest_version: unpacked.manifest_version,
        gateway_risk: unpacked.gateway_risk,
        added_at: existing.map_or(now, |e| e.added_at),
        updated_at: now,
        description: unpacked.description,
        file_name,
    };
    store.upsert_extension(&record)?;
    Ok(record)
}

/// Чистка библиотеки, только если браузеров других процессов Ombra нет: после уборки сирот pid-файлы остаются лишь у
/// живых браузеров (окно рядом с `ombra flow`, спецификация А2), а те могут держать старую версию расширения.
/// `Ok(false)` — чистка пропущена.
pub fn sweep_when_alone(paths: &Paths, store: &Store) -> Result<bool, CommandError> {
    if !crate::supervisor::pidfile::list(&paths.run_dir())?.is_empty() {
        return Ok(false);
    }
    sweep_library(paths, store)?;
    Ok(true)
}

/// Очистка при старте приложения, пока профили не запущены: папки расширений, которых нет в базе, версии, кроме
/// текущей, и временные папки. Работающий браузер держит файлы старой версии — раньше старта их не удалить.
pub fn sweep_library(paths: &Paths, store: &Store) -> Result<(), CommandError> {
    let entries = match std::fs::read_dir(paths.extensions()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        match store.get_extension(&name)? {
            Some(record) if entry.file_type()?.is_dir() => {
                for version in std::fs::read_dir(entry.path())? {
                    let version = version?;
                    if version.file_name().to_string_lossy() != record.version {
                        crate::paths::remove_path(&version.path())?;
                    }
                }
                // Записи до миграции 007 — без описания: берём его из манифеста текущей версии.
                if record.description.is_empty()
                    && let Some(text) = package::description(&paths.extension_dir(&record.id, &record.version))
                    && !text.is_empty()
                {
                    store.set_extension_description(&record.id, &text)?;
                }
            }
            _ => crate::paths::remove_path(&entry.path())?,
        }
    }
    Ok(())
}

impl<E: Engine> Core<E> {
    pub fn list_ext_categories(&self) -> Result<Vec<ExtCategory>, CommandError> {
        Ok(self.store.list_ext_categories()?)
    }

    pub fn create_ext_category(&self, name: &str, remark: &str, now: i64) -> Result<ExtCategory, CommandError> {
        Ok(self.store.create_ext_category(name, remark, now)?)
    }

    pub fn update_ext_category(&self, id: i64, name: &str, remark: &str) -> Result<ExtCategory, CommandError> {
        Ok(self.store.update_ext_category(id, name, remark)?)
    }

    pub fn set_ext_category_items(&self, id: i64, extension_ids: &[String]) -> Result<ExtCategory, CommandError> {
        Ok(self.store.set_ext_category_items(id, extension_ids)?)
    }

    /// Профили набора переходят в набор 0.
    pub fn delete_ext_category(&self, id: i64) -> Result<(), CommandError> {
        Ok(self.store.delete_ext_category(id)?)
    }

    /// Набор выбранным профилям; несуществующие и профили в корзине — `NotFound` в отчёте.
    pub fn set_profiles_ext_category(&self, ids: &[String], category_id: i64) -> Result<BulkReport, CommandError> {
        let (existing, mut report) = self.split_existing(ids)?;
        self.store.set_profiles_ext_category(&existing, category_id)?;
        report.ok = existing;
        Ok(report)
    }

    /// «Установить в группу»: набор всем живым профилям группы (`None` — «Без группы»). Профиль, который позже
    /// попадёт в группу, набор не получает — как в AdsPower.
    pub fn set_group_ext_category(&self, group_id: Option<i64>, category_id: i64) -> Result<BulkReport, CommandError> {
        let ids = self.group_profile_ids(group_id)?;
        self.set_profiles_ext_category(&ids, category_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::test_core;
    use crate::extensions::crx::tests::build_crx;
    use crate::extensions::id::extension_id;
    use crate::extensions::package::tests::{manifest_v3, zip_of};
    use std::net::SocketAddr;
    use tokio::io::AsyncWriteExt;

    /// HTTP-сервер, который на любой запрос отвечает `status` и `body`.
    async fn serve(status: u16, body: Vec<u8>) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let body = body.clone();
                tokio::spawn(async move {
                    crate::relay::test_support::read_head(&mut socket).await;
                    let head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        addr
    }

    fn crx(key: &[u8], version: &str) -> Vec<u8> {
        build_crx(key, key, &zip_of(&[("manifest.json", &manifest_v3("Wallet", version))]))
    }

    fn reason(e: &CommandError) -> (&str, Option<&'static str>) {
        (e.code.as_str(), e.fields.as_ref().and_then(|f| f.first()).map(|f| f.code))
    }

    fn leftovers(core: &Core<crate::supervisor::test_engine::FakeEngine>) -> Vec<String> {
        std::fs::read_dir(core.paths.extensions())
            .map(|d| d.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(".tmp")).collect())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn file_imports_zip_folder_and_crx() {
        let (core, _, dir) = test_core().await;
        let zip_path = dir.path().join("w.zip");
        std::fs::write(&zip_path, zip_of(&[("w/manifest.json", &manifest_v3("Wallet", "1.0"))])).unwrap();
        let from_zip = core.add_file_extension(&zip_path, 5).await.unwrap();
        assert_eq!((from_zip.source.as_str(), from_zip.version.as_str(), from_zip.added_at), ("file", "1.0", 5));
        let manifest = std::fs::read_to_string(core.paths.extension_dir(&from_zip.id, "1.0").join("manifest.json")).unwrap();
        assert!(manifest.contains("\"key\""));
        assert_eq!(from_zip.file_name.as_deref(), Some("w.zip"));
        let folder = dir.path().join("plain");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("manifest.json"), manifest_v3("Plain", "3")).unwrap();
        let from_folder = core.add_file_extension(&folder, 6).await.unwrap();
        assert_ne!(from_folder.id, from_zip.id);
        assert_eq!(from_folder.file_name.as_deref(), Some("plain"));
        let crx_path = dir.path().join("x.CRX");
        std::fs::write(&crx_path, crx(b"dev key", "1.0")).unwrap();
        let from_crx = core.add_file_extension(&crx_path, 7).await.unwrap();
        assert_eq!((from_crx.id, from_crx.file_name.as_deref()), (extension_id(b"dev key"), Some("x.CRX")));
        assert_eq!(core.list_extensions().unwrap().len(), 3);
        assert!(leftovers(&core).is_empty());
    }

    #[tokio::test]
    async fn bad_packages_are_format_errors_and_leave_nothing() {
        let (core, _, dir) = test_core().await;
        let cases: [(&str, Vec<u8>, &str); 3] = [
            ("a.zip", b"not a zip".to_vec(), "zip"),
            ("b.zip", zip_of(&[("readme.txt", b"no manifest")]), "manifest"),
            ("c.crx", b"Cr24junk".to_vec(), "crx"),
        ];
        for (name, bytes, expected) in cases {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let e = core.add_file_extension(&path, 1).await.unwrap_err();
            assert_eq!(reason(&e), ("ExtensionFormat", Some(expected)), "{name}");
        }
        let txt = dir.path().join("x.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert_eq!(reason(&core.add_file_extension(&txt, 1).await.unwrap_err()).0, "ExtensionFormat");
        assert!(core.list_extensions().unwrap().is_empty() && leftovers(&core).is_empty());
    }

    #[tokio::test]
    async fn store_install_checks_the_id_and_updates_only_to_newer() {
        let (mut core, _, _dir) = test_core().await;
        let id = extension_id(b"store key");
        let v1 = serve(200, crx(b"store key", "1.0")).await;
        core.library.store_url = format!("http://{v1}/{{id}}.crx?prodversion={{version}}");
        let link = format!("https://chromewebstore.google.com/detail/wallet/{id}");
        let installed = core.add_store_extension(&link, 10).await.unwrap();
        assert_eq!((installed.id.as_str(), installed.source.as_str()), (id.as_str(), "store"));
        assert_eq!(installed.file_name, None);
        let other = extension_id(b"someone else");
        assert_eq!(reason(&core.add_store_extension(&other, 11).await.unwrap_err()), ("ExtensionFormat", Some("id_mismatch")));
        // Та же версия — запись прежняя.
        assert_eq!(core.update_extension(&id, 12).await.unwrap().updated_at, 10);
        let v2 = serve(200, crx(b"store key", "2.0")).await;
        core.library.store_url = format!("http://{v2}/{{id}}.crx");
        let updated = core.update_extension(&id, 13).await.unwrap();
        assert_eq!((updated.version.as_str(), updated.added_at, updated.updated_at), ("2.0", 10, 13));
        // Старая версия остаётся до очистки при старте: её может держать работающий браузер.
        assert!(core.paths.extension_dir(&id, "1.0").is_dir() && core.paths.extension_dir(&id, "2.0").is_dir());
        std::fs::create_dir_all(core.paths.extensions().join(".tmp-left")).unwrap();
        std::fs::create_dir_all(core.paths.extension_dir(&"p".repeat(32), "1.0")).unwrap();
        sweep_library(&core.paths, &core.store).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(core.paths.extensions()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, vec![id.clone()]);
        assert!(!core.paths.extension_dir(&id, "1.0").exists() && core.paths.extension_dir(&id, "2.0").is_dir());
    }

    #[tokio::test]
    async fn sweep_waits_until_no_other_process_runs_browsers() {
        let (core, _, _dir) = test_core().await;
        let left = core.paths.extensions().join(".tmp-left");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(core.paths.run_dir()).unwrap();
        // После уборки сирот pid-файл остался — его браузер работает в другом процессе Ombra.
        std::fs::write(core.paths.run_dir().join("other01.pid"), "{").unwrap();
        assert!(!sweep_when_alone(&core.paths, &core.store).unwrap());
        assert!(left.is_dir(), "чистки не было");
        std::fs::remove_file(core.paths.run_dir().join("other01.pid")).unwrap();
        assert!(sweep_when_alone(&core.paths, &core.store).unwrap());
        assert!(!left.exists());
    }

    #[tokio::test]
    async fn startup_sweep_waits_for_the_library_lock() {
        let (core, _, _dir) = test_core().await;
        let left = core.paths.extensions().join(".tmp-install");
        std::fs::create_dir_all(&left).unwrap();
        // Идёт установка: её временная папка ещё нужна.
        let held = core.library.lock.lock().await;
        let waiting = tokio::time::timeout(Duration::from_millis(100), core.sweep_extensions_when_alone()).await;
        assert!(waiting.is_err(), "чистка не взяла замок библиотеки");
        assert!(left.is_dir());
        drop(held);
        assert!(core.sweep_extensions_when_alone().await.unwrap());
        assert!(!left.exists());
    }

    #[tokio::test]
    async fn store_errors_are_clear() {
        let (mut core, _, dir) = test_core().await;
        let id = extension_id(b"k");
        assert_eq!(core.add_store_extension("not a link", 1).await.unwrap_err().code, "Validation");
        core.library.store_url = format!("http://{}/{{id}}", serve(404, Vec::new()).await);
        assert_eq!(core.add_store_extension(&id, 1).await.unwrap_err().code, "ExtensionDownload");
        core.library.store_url = format!("http://{}/{{id}}", serve(200, Vec::new()).await);
        assert_eq!(core.add_store_extension(&id, 1).await.unwrap_err().code, "ExtensionDownload");
        core.library.store_url = format!("http://127.0.0.1:{}/{{id}}", crate::relay::test_support::closed_port().await);
        assert_eq!(core.add_store_extension(&id, 1).await.unwrap_err().code, "ExtensionDownload");
        // Обновить можно только расширение из магазина.
        let folder = dir.path().join("plain");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("manifest.json"), manifest_v3("Plain", "1")).unwrap();
        let local = core.add_file_extension(&folder, 1).await.unwrap();
        let e = core.update_extension(&local.id, 2).await.unwrap_err();
        assert_eq!(e.fields.unwrap()[0].field, "source");
        assert_eq!(core.update_extension(&"a".repeat(32), 2).await.unwrap_err().code, "NotFound");
        core.remove_extension(&local.id).await.unwrap();
        assert_eq!(core.remove_extension(&local.id).await.unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn remove_waits_for_the_library_lock() {
        let (core, _, dir) = test_core().await;
        let folder = dir.path().join("plain");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("manifest.json"), manifest_v3("Plain", "1")).unwrap();
        let ext = core.add_file_extension(&folder, 1).await.unwrap();
        let held = core.library.lock.lock().await;
        let waiting = tokio::time::timeout(Duration::from_millis(100), core.remove_extension(&ext.id)).await;
        assert!(waiting.is_err(), "удаление не взяло замок библиотеки");
        drop(held);
        core.remove_extension(&ext.id).await.unwrap();
    }

    #[tokio::test]
    async fn re_adding_restores_a_missing_version_folder() {
        let (core, _, dir) = test_core().await;
        let folder = dir.path().join("plain");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("manifest.json"), manifest_v3("Plain", "1")).unwrap();
        let path = dir.path().join("w.crx");
        std::fs::write(&path, crx(b"k", "1.0")).unwrap();
        let first = core.add_file_extension(&path, 1).await.unwrap();
        let target = core.paths.extension_dir(&first.id, "1.0");
        std::fs::remove_dir_all(&target).unwrap();
        let again = core.add_file_extension(&path, 2).await.unwrap();
        assert!(target.join("manifest.json").is_file());
        assert_eq!((again.version.as_str(), again.added_at, again.updated_at), ("1.0", 1, 2));
        // Целая папка — запись прежняя.
        assert_eq!(core.add_file_extension(&path, 3).await.unwrap().updated_at, 2);
    }

    #[tokio::test]
    async fn sweep_does_not_follow_a_symlink_named_like_an_extension() {
        let (core, _, dir) = test_core().await;
        let path = dir.path().join("w.crx");
        std::fs::write(&path, crx(b"k", "1.0")).unwrap();
        let ext = core.add_file_extension(&path, 1).await.unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(outside.join("sub")).unwrap();
        std::fs::write(outside.join("a.txt"), b"a").unwrap();
        std::fs::write(outside.join("sub/b.txt"), b"b").unwrap();
        let link = core.paths.extensions().join(&ext.id);
        crate::paths::remove_path(&link).unwrap();
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        sweep_library(&core.paths, &core.store).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err());
        assert!(outside.join("a.txt").is_file() && outside.join("sub/b.txt").is_file());
    }

    #[tokio::test]
    async fn sweep_fills_missing_descriptions_from_the_library() {
        let (core, _, dir) = test_core().await;
        let folder = dir.path().join("plain");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(
            folder.join("manifest.json"),
            r#"{"manifest_version": 3, "name": "Plain", "version": "1", "description": "Из манифеста"}"#,
        )
        .unwrap();
        let ext = core.add_file_extension(&folder, 1).await.unwrap();
        assert_eq!(ext.description, "Из манифеста");
        // Запись до миграции 007: описания нет, папка версии на месте.
        core.store.set_extension_description(&ext.id, "").unwrap();
        sweep_library(&core.paths, &core.store).unwrap();
        assert_eq!(core.store.get_extension(&ext.id).unwrap().unwrap().description, "Из манифеста");
        // Папки нет — описание остаётся пустым, очистка не падает.
        core.store.set_extension_description(&ext.id, "").unwrap();
        crate::paths::remove_path(&core.paths.extensions().join(&ext.id)).unwrap();
        sweep_library(&core.paths, &core.store).unwrap();
        assert_eq!(core.store.get_extension(&ext.id).unwrap().unwrap().description, "");
    }

    /// Настоящий Chrome Web Store: uBlock Origin Lite (MV3, несколько мегабайт) скачивается, CRX3 разбирается,
    /// идентификатор совпадает с запрошенным.
    #[tokio::test]
    #[ignore = "ходит в сеть; cargo test --lib live_store_download -- --ignored --nocapture"]
    async fn live_store_download() {
        let (core, _, _dir) = test_core().await;
        let id = "ddkjiahejlhfcafbddmgiahcphecmpfh";
        let record = core.add_store_extension(&format!("https://chromewebstore.google.com/detail/ublock-origin-lite/{id}"), 1).await.unwrap();
        println!("{record:?}");
        assert_eq!((record.id.as_str(), record.source.as_str(), record.manifest_version), (id, "store", 3));
        assert!(core.paths.extension_dir(id, &record.version).join("manifest.json").is_file());
    }

    use crate::app::tests::form;
    use crate::store::extensions::DEFAULT_CATEGORY;

    #[tokio::test]
    async fn categories_through_the_core() {
        let (core, _, dir) = test_core().await;
        let folder = dir.path().join("plain");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("manifest.json"), manifest_v3("Plain", "1")).unwrap();
        let ext = core.add_file_extension(&folder, 1).await.unwrap();
        let shop = core.create_ext_category("Магазин", "для магазина", 2).unwrap();
        let shop = core.set_ext_category_items(shop.id, std::slice::from_ref(&ext.id)).unwrap();
        assert_eq!(shop.extension_ids, vec![ext.id.clone()]);
        let names: Vec<String> = core.list_ext_categories().unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["Общий".to_string(), "Магазин".to_string()]);
        let e = core.create_ext_category("магазин", "", 3).unwrap_err();
        assert_eq!(e.fields.unwrap()[0].field, "name");
        assert_eq!(core.update_ext_category(shop.id, "Shop", "").unwrap().name, "Shop");
        assert_eq!(core.delete_ext_category(DEFAULT_CATEGORY).unwrap_err().code, "Validation");
        core.delete_ext_category(shop.id).unwrap();
        assert_eq!(core.delete_ext_category(shop.id).unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn file_name_follows_the_written_version() {
        let (core, _, dir) = test_core().await;
        let first = dir.path().join("wallet-1.crx");
        std::fs::write(&first, crx(b"k", "1.0")).unwrap();
        assert_eq!(core.add_file_extension(&first, 1).await.unwrap().file_name.as_deref(), Some("wallet-1.crx"));
        // Та же версия из другого файла — запись прежняя, имя тоже.
        let copy = dir.path().join("copy.crx");
        std::fs::write(&copy, crx(b"k", "1.0")).unwrap();
        assert_eq!(core.add_file_extension(&copy, 2).await.unwrap().file_name.as_deref(), Some("wallet-1.crx"));
        let second = dir.path().join("wallet-2.crx");
        std::fs::write(&second, crx(b"k", "2.0")).unwrap();
        let updated = core.add_file_extension(&second, 3).await.unwrap();
        assert_eq!((updated.version.as_str(), updated.file_name.as_deref()), ("2.0", Some("wallet-2.crx")));
        assert_eq!(core.store.get_extension(&updated.id).unwrap().unwrap().file_name.as_deref(), Some("wallet-2.crx"));
    }

    #[test]
    fn store_page_url_takes_only_an_id() {
        let id = "cjpalhdlnbpafiamejdnhcphjbkeiagm";
        assert_eq!(
            store_page_url(id).as_deref(),
            Some("https://chromewebstore.google.com/detail/cjpalhdlnbpafiamejdnhcphjbkeiagm")
        );
        for bad in [format!("https://chromewebstore.google.com/detail/x/{id}"), format!(" {id}"), "ublock".into(), String::new()] {
            assert_eq!(store_page_url(&bad), None, "{bad}");
        }
    }

    #[test]
    fn store_search_url_encodes_the_query() {
        assert_eq!(store_search_url("  "), "https://chromewebstore.google.com/");
        assert_eq!(store_search_url(" ublock origin "), "https://chromewebstore.google.com/search/ublock%20origin");
        assert_eq!(store_search_url("a/b?c#d%"), "https://chromewebstore.google.com/search/a%2Fb%3Fc%23d%25");
        assert_eq!(store_search_url("кошелёк"), "https://chromewebstore.google.com/search/%D0%BA%D0%BE%D1%88%D0%B5%D0%BB%D1%91%D0%BA");
    }

    #[tokio::test]
    async fn sets_are_assigned_to_profiles_and_groups() {
        let (core, _, _dir) = test_core().await;
        let shop = core.create_ext_category("Магазин", "", 1).unwrap();
        let group = core.create_group(&crate::app::groups::GroupForm { name: "G".into(), color: "#112233".into(), note: String::new(), quick_access: false }, 1).unwrap();
        let mut in_group = form("in", None);
        in_group.group_id = Some(Some(group.id));
        let a = core.create_profile(&in_group, 1).unwrap();
        let b = core.create_profile(&in_group, 1).unwrap();
        let outside = core.create_profile(&form("out", None), 1).unwrap();
        assert_eq!(a.ext_category_id, DEFAULT_CATEGORY);
        let report = core.set_group_ext_category(Some(group.id), shop.id).unwrap();
        assert_eq!(report.ok.len(), 2);
        assert_eq!(core.store.get_profile(&outside.id).unwrap().unwrap().ext_category_id, DEFAULT_CATEGORY);
        assert_eq!(core.store.get_profile(&b.id).unwrap().unwrap().ext_category_id, shop.id);
        // Панель группы на «Группах» видит набор каждого профиля из строки списка.
        let rows = core.list_profiles(&Default::default()).unwrap().rows;
        let set_of = |id: &str| rows.iter().find(|row| row.id == id).unwrap().ext_category_id;
        assert_eq!((set_of(&b.id), set_of(&outside.id)), (shop.id, DEFAULT_CATEGORY));
        let report = core.set_group_ext_category(None, shop.id).unwrap();
        assert_eq!(report.ok, vec![outside.id.clone()]);
        let report = core.set_profiles_ext_category(&[a.id.clone(), "nope".into()], DEFAULT_CATEGORY).unwrap();
        assert_eq!((report.ok.len(), report.failed.len()), (1, 1));
        assert_eq!(core.set_profiles_ext_category(std::slice::from_ref(&a.id), 77).unwrap_err().fields.unwrap()[0].field, "ext_category_id");
        assert_eq!(core.set_group_ext_category(Some(999), shop.id).unwrap_err().code, "NotFound");
    }

    #[tokio::test]
    async fn the_form_and_the_clone_carry_the_set() {
        let (core, _, _dir) = test_core().await;
        let shop = core.create_ext_category("Магазин", "", 1).unwrap();
        let mut f = form("p", None);
        f.ext_category_id = Some(shop.id);
        let rec = core.create_profile(&f, 1).unwrap();
        assert_eq!(rec.ext_category_id, shop.id);
        // Нет поля — набор прежний.
        let mut keep = form("p2", None);
        keep.ext_category_id = None;
        assert_eq!(core.update_profile(&rec.id, &keep).unwrap().ext_category_id, shop.id);
        assert_eq!(core.clone_profile(&rec.id, 2).unwrap().ext_category_id, shop.id);
        let mut bad = form("p3", None);
        bad.ext_category_id = Some(77);
        assert_eq!(core.create_profile(&bad, 1).unwrap_err().fields.unwrap()[0].field, "ext_category_id");
    }
}
