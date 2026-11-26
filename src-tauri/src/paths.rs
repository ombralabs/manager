use std::path::{Path, PathBuf};

/// Папки кэша Chromium внутри папки профиля: страницы, код, GPU и шейдеры. Cookies, хранилище сайтов и история — не здесь.
pub const CACHE_DIRS: &[&str] = &[
    "Default/Cache",
    "Default/Code Cache",
    "Default/GPUCache",
    "Default/DawnGraphiteCache",
    "Default/DawnWebGPUCache",
    "GrShaderCache",
    "ShaderCache",
    "GraphiteDawnCache",
];

/// Хранилище cookies Chromium в папке профиля: с 96-й версии — `Default/Network`, `Default/Cookies` — прежнее место.
pub const CHROMIUM_COOKIE_FILES: &[&str] =
    &["Default/Network/Cookies", "Default/Network/Cookies-journal", "Default/Cookies", "Default/Cookies-journal"];

/// Раскладка папки данных Ombra (спецификация этапа 1, раздел «Структура»).
#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
    data: Option<PathBuf>,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), data: None }
    }

    /// Данные доступны только внутри смонтированного после 2FA хранилища.
    pub(crate) fn in_vault(&self) -> Self {
        Self { root: self.root.clone(), data: Some(self.vault_mount().join("data")) }
    }

    pub fn is_protected(&self) -> bool { self.data.is_some() }
    pub fn data_root(&self) -> &Path { self.data.as_deref().unwrap_or(&self.root) }
    pub fn security_config(&self) -> PathBuf { self.root.join("security.json") }
    pub fn vault_image(&self) -> PathBuf { self.root.join("Profiles.sparsebundle") }
    pub fn vault_mount(&self) -> PathBuf {
        use sha2::{Digest, Sha256};
        let id = Sha256::digest(self.root.as_os_str().as_encoded_bytes());
        let name: String = id[..12].iter().map(|b| format!("{b:02x}")).collect();
        // Вне Application Support: Chromium иначе выносит кэш в незашифрованный ~/Library/Caches.
        std::env::temp_dir().join(format!("ombra-vault-{name}"))
    }

    /// Platform-specific application data directory.
    pub fn default_root() -> PathBuf {
        #[cfg(target_os = "macos")]
        { PathBuf::from(std::env::var_os("HOME").expect("HOME не задан")).join("Library/Application Support/Ombra") }
        #[cfg(windows)]
        { PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA не задан")).join("Ombra") }
        #[cfg(target_os = "linux")]
        {
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
                .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME не задан")).join(".local/share"))
                .join("Ombra")
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db(&self) -> PathBuf {
        self.data_root().join("ombra.db")
    }

    pub fn profiles(&self) -> PathBuf {
        self.data_root().join("profiles")
    }

    pub fn profile_dir(&self, id: &str) -> PathBuf {
        self.profiles().join(id)
    }

    pub fn engines(&self) -> PathBuf {
        self.root.join("engines")
    }

    /// Библиотека расширений: `<id>/<version>/` — распакованное расширение.
    pub fn extensions(&self) -> PathBuf {
        self.data_root().join("extensions")
    }

    pub fn extension_dir(&self, id: &str, version: &str) -> PathBuf {
        self.extensions().join(id).join(version)
    }

    pub fn run_dir(&self) -> PathBuf {
        self.data_root().join("run")
    }

    /// Блокировка прокси на время запуска профиля: общая для окна и команд `ombra flow` (спецификация А2).
    pub fn proxy_lock_dir(&self, id: i64) -> PathBuf {
        self.run_dir().join("proxies").join(id.to_string())
    }

    pub fn logs(&self) -> PathBuf {
        self.data_root().join("logs")
    }

    pub fn relay_log(&self, id: &str) -> PathBuf {
        self.logs().join("relay").join(format!("{id}.jsonl"))
    }

    /// Журнал запуска процесса автоматизации.
    pub fn flow_log(&self, run: &str) -> PathBuf {
        self.logs().join("flows").join(format!("{run}.jsonl"))
    }

    /// Файлы запуска процесса вне задачи (А1): снимки и выгрузки по умолчанию.
    pub fn flow_files(&self, run: &str) -> PathBuf {
        self.data_root().join("flows").join("files").join("debug").join(run)
    }

    /// Файлы пакета задачи (А2): снимки и выгрузки по умолчанию.
    pub fn task_files(&self, task: i64, batch: i64) -> PathBuf {
        self.data_root().join("flows").join("files").join(task.to_string()).join(batch.to_string())
    }

    pub fn chromium_log(&self, id: &str) -> PathBuf {
        self.logs().join("chromium").join(format!("{id}.log"))
    }

    /// Кэш, который Chromium на macOS выносит из `~/Library/Application Support/…` в такой же путь под `~/Library/Caches`.
    /// `None` — папка данных лежит не в `Application Support` (тесты, своя папка).
    pub fn chromium_cache_dir(&self, id: &str) -> Option<PathBuf> {
        if !cfg!(target_os = "macos") { return None; }
        let home = PathBuf::from(std::env::var_os("HOME")?);
        mapped_cache_dir(
            &self.profile_dir(id),
            &home.join("Library/Application Support"),
            &home.join("Library/Caches"),
        )
    }

    /// Всё, что удаляет «Очистить кэш»: папки `CACHE_DIRS` внутри папки профиля и вынесенный кэш в `~/Library/Caches`.
    pub fn cache_paths(&self, id: &str) -> Vec<PathBuf> {
        let dir = self.profile_dir(id);
        let mut paths: Vec<PathBuf> = CACHE_DIRS.iter().map(|cache| dir.join(cache)).collect();
        paths.extend(self.chromium_cache_dir(id));
        paths
    }

    pub fn local_key_file(&self) -> PathBuf { self.data_root().join("secrets.key") }
    pub fn device_key_file(&self) -> PathBuf { self.root.join("vault-device.key") }

    pub fn dev_key_file(&self) -> PathBuf {
        self.data_root().join("dev-secrets.key")
    }
    pub fn vault_key_file(&self) -> PathBuf { self.data_root().join("vault-secrets.key") }
}

/// `profile_dir` внутри `app_support` → тот же относительный путь внутри `caches` (так делает Chromium на macOS).
pub fn mapped_cache_dir(profile_dir: &Path, app_support: &Path, caches: &Path) -> Option<PathBuf> {
    let relative = profile_dir.strip_prefix(app_support).ok()?;
    (!relative.as_os_str().is_empty()).then(|| caches.join(relative))
}

/// Удалить файл или папку целиком; отсутствие пути — не ошибка.
pub fn remove_path(path: &Path) -> std::io::Result<()> {
    let result = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
    match result {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_follows_spec() {
        let p = Paths::new("/data/Ombra");
        assert_eq!(p.db(), PathBuf::from("/data/Ombra/ombra.db"));
        assert_eq!(p.profile_dir("k7f2q9"), PathBuf::from("/data/Ombra/profiles/k7f2q9"));
        assert_eq!(p.engines(), PathBuf::from("/data/Ombra/engines"));
        assert_eq!(p.run_dir(), PathBuf::from("/data/Ombra/run"));
        assert_eq!(p.proxy_lock_dir(7), PathBuf::from("/data/Ombra/run/proxies/7"));
        assert_eq!(p.relay_log("k7f2q9"), PathBuf::from("/data/Ombra/logs/relay/k7f2q9.jsonl"));
        assert_eq!(p.chromium_log("k7f2q9"), PathBuf::from("/data/Ombra/logs/chromium/k7f2q9.log"));
        assert_eq!(p.dev_key_file(), PathBuf::from("/data/Ombra/dev-secrets.key"));
        #[cfg(target_os = "macos")]
        assert!(Paths::default_root().ends_with("Library/Application Support/Ombra"));
        assert!(Paths::default_root().is_absolute());
    }

    #[test]
    fn chromium_cache_mirrors_application_support() {
        let support = Path::new("/Users/u/Library/Application Support");
        let caches = Path::new("/Users/u/Library/Caches");
        assert_eq!(
            mapped_cache_dir(&support.join("Ombra/profiles/k7f2q9"), support, caches),
            Some(caches.join("Ombra/profiles/k7f2q9"))
        );
        assert_eq!(mapped_cache_dir(Path::new("/tmp/x/profiles/k7f2q9"), support, caches), None);
        assert_eq!(mapped_cache_dir(support, support, caches), None, "сама папка — не профиль");
        let real = Paths::new(Paths::default_root()).chromium_cache_dir("k7f2q9");
        #[cfg(target_os = "macos")]
        assert!(real.unwrap().ends_with("Library/Caches/Ombra/profiles/k7f2q9"));
        #[cfg(not(target_os = "macos"))]
        assert_eq!(real, None, "only macOS mirrors Application Support into Library/Caches");
        assert_eq!(Paths::new("/data/Ombra").chromium_cache_dir("k7f2q9"), None);
        assert!(CACHE_DIRS.contains(&"Default/Cache") && !CACHE_DIRS.iter().any(|d| d.contains("Cookies")));
    }

    #[test]
    fn cache_paths_cover_profile_and_mirrored_cache() {
        let local = Paths::new("/data/Ombra").cache_paths("k7f2q9");
        assert_eq!(local.len(), CACHE_DIRS.len(), "вне Application Support кэша в ~/Library/Caches нет");
        assert!(local.iter().all(|p| p.starts_with("/data/Ombra/profiles/k7f2q9")), "{local:?}");
        assert!(local.contains(&PathBuf::from("/data/Ombra/profiles/k7f2q9/Default/Cache")));
        assert!(local.contains(&PathBuf::from("/data/Ombra/profiles/k7f2q9/GrShaderCache")));

        let real = Paths::new(Paths::default_root()).cache_paths("k7f2q9");
        assert_eq!(real.len(), CACHE_DIRS.len() + usize::from(cfg!(target_os = "macos")));
        #[cfg(target_os = "macos")]
        assert!(real.last().unwrap().ends_with("Library/Caches/Ombra/profiles/k7f2q9"), "{real:?}");
        #[cfg(not(target_os = "macos"))]
        assert!(real.iter().all(|p| p.starts_with(Paths::default_root().join("profiles/k7f2q9"))), "{real:?}");
    }

    #[test]
    fn remove_path_handles_files_dirs_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        remove_path(&file).unwrap();
        assert!(!file.exists());

        let nested = dir.path().join("d/sub");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("x"), b"x").unwrap();
        remove_path(&dir.path().join("d")).unwrap();
        assert!(!dir.path().join("d").exists());

        remove_path(&dir.path().join("missing")).unwrap();
    }
}
