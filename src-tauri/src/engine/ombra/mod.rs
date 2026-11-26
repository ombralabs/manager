//! Наш движок: форк Chromium, каталог сохранённых версий и выбор сборки для новых запусков.
pub mod catalog;
pub mod update;
pub mod version;
pub mod flags;
pub mod install;

use crate::engine::{
    Engine, EngineBuild, EngineError, EngineProcess, LaunchCommand, LaunchHooks, LaunchRequest, StopReport, args, process,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct OmbraEngine {
    engines_dir: PathBuf,
    /// Одна операция установки или переключения в этом экземпляре.
    import_lock: tokio::sync::Mutex<()>,
    update_cancel: std::sync::Mutex<Option<tokio_util::sync::CancellationToken>>,
    build_override: Option<EngineBuild>,
    ready_timeout: Duration,
    settle: Duration,
}

impl OmbraEngine {
    pub fn new(engines_dir: PathBuf) -> Self {
        Self {
            engines_dir,
            import_lock: tokio::sync::Mutex::new(()),
            update_cancel: std::sync::Mutex::new(None),
            build_override: None,
            ready_timeout: Duration::from_secs(60),
            settle: Duration::from_secs(3),
        }
    }

    pub fn catalog(&self) -> Result<catalog::Catalog, EngineError> {
        catalog::list(&self.engines_dir)
    }

    pub async fn select_version(&self, id: String) -> Result<EngineBuild, EngineError> {
        let _one_at_a_time = self.import_lock.lock().await;
        let dir = self.engines_dir.clone();
        tokio::task::spawn_blocking(move || catalog::select(&dir, &id, false)).await
            .map_err(|_| EngineError::Install("выбор версии прерван".into()))?
    }

    pub fn set_automatic(&self, automatic: bool) -> Result<(), EngineError> {
        catalog::set_automatic(&self.engines_dir, automatic)
    }

    pub async fn update_stable(&self, background: bool, progress: impl Fn(update::Progress) + Send + Sync) -> Result<update::Outcome, EngineError> {
        let _operation = self.import_lock.try_lock().map_err(|_| EngineError::Update("EngineUpdateBusy"))?;
        let source = update::Source::from_build()?;
        let cancel = tokio_util::sync::CancellationToken::new();
        *self.update_cancel.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cancel.clone());
        let result = update::ensure_latest(&self.engines_dir, &source, background, cancel, progress).await;
        self.update_cancel.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        result
    }

    pub fn cancel_update(&self) {
        if let Some(cancel) = self.update_cancel.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            cancel.cancel();
        }
    }

    pub fn with_timeouts(mut self, ready: Duration, settle: Duration) -> Self {
        self.ready_timeout = ready;
        self.settle = settle;
        self
    }

    /// Использовать указанный бинарник вместо слота (тесты).
    pub fn with_build(mut self, build: EngineBuild) -> Self {
        self.build_override = Some(build);
        self
    }
}

impl Engine for OmbraEngine {
    fn installed(&self) -> Option<EngineBuild> {
        self.build_override.clone().or_else(|| install::installed(&self.engines_dir))
    }

    async fn import_archive(&self, archive: &Path) -> Result<EngineBuild, EngineError> {
        let _one_at_a_time = self.import_lock.lock().await;
        let engines_dir = self.engines_dir.clone();
        let archive = archive.to_path_buf();
        // Распаковка сотен мегабайт — не в потоке tokio.
        tokio::task::spawn_blocking(move || install::import_archive(&engines_dir, &archive))
            .await
            .map_err(|e| EngineError::Install(format!("импорт прерван: {e}")))?
    }

    fn build_command(&self, req: &LaunchRequest) -> Result<LaunchCommand, EngineError> {
        let build = self.installed().ok_or(EngineError::NotInstalled)?;
        args::check_extra_args(&req.extra_args)?;
        Ok(LaunchCommand {
            // A development slot may be a symlink. Launch its real bundle so Dock uses the app icon
            // without the alias badge macOS adds to icons reached through a symbolic link.
            program: build.binary.canonicalize()?,
            args: flags::build_args(req, install::major(&build.version)),
            profile_dir: req.profile_dir.clone(),
            log_path: req.log_path.clone(),
        })
    }

    fn launch(&self, cmd: LaunchCommand, hooks: LaunchHooks) -> impl Future<Output = Result<EngineProcess, EngineError>> + Send {
        process::launch(cmd, self.ready_timeout, hooks)
    }

    fn probe(&self, port: u16) -> impl Future<Output = process::Probe> + Send {
        process::probe(port)
    }

    fn stop(
        &self,
        process: &mut EngineProcess,
        grace: Duration,
    ) -> impl Future<Output = Result<StopReport, EngineError>> + Send {
        process::stop(process, grace, self.settle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use install::tests::{archive_of, fake_app};
    use install::APP_NAME;

    #[test]
    fn command_comes_from_the_slot() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        let missing = OmbraEngine::new(engines.clone());
        assert!(matches!(missing.build_command(&flags::tests::request()), Err(EngineError::NotInstalled)));
        fake_app(&engines, Some("150.0.7871.114"));
        let engine = OmbraEngine::new(engines.clone());
        let command = engine.build_command(&flags::tests::request()).unwrap();
        assert_eq!(command.program, install::binary_path(&install::app_dir(&engines)).canonicalize().unwrap());
        assert!(command.args.contains(&"--fingerprint-tls-profile=chrome-150".to_string()));
        let mut bad = flags::tests::request();
        bad.extra_args = vec!["--timezone=UTC".into()];
        assert!(matches!(engine.build_command(&bad), Err(EngineError::Args(_))));
    }

    #[test]
    fn command_resolves_a_symlink_slot_to_the_real_app_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        let build_dir = dir.path().join("out");
        let app = fake_app(&build_dir, Some("150.0.7871.114"));
        std::fs::create_dir_all(&engines).unwrap();
        std::os::unix::fs::symlink(&app, install::app_dir(&engines)).unwrap();
        let engine = OmbraEngine::new(engines.clone());
        let request = flags::tests::request();
        let command = engine.build_command(&request).unwrap();
        assert_eq!(command.program, install::binary_path(&app).canonicalize().unwrap());
        assert_ne!(command.program, install::binary_path(&install::app_dir(&engines)));
        assert_eq!(command.profile_dir, request.profile_dir);
        assert_eq!(command.args, flags::build_args(&request, 150));
        assert!(install::app_dir(&engines).symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[tokio::test]
    async fn import_goes_through_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let engines = dir.path().join("engines");
        let src = dir.path().join("src");
        fake_app(&src, Some("150.0.7871.114"));
        let archive = archive_of(dir.path(), &src.join(APP_NAME), APP_NAME);
        let engine = OmbraEngine::new(engines.clone());
        assert!(engine.installed().is_none());
        assert_eq!(engine.import_archive(&archive).await.unwrap().version, "150.0.7871.114");
        assert_eq!(engine.installed().unwrap().version, "150.0.7871.114");
    }
}
