pub mod args;
pub mod bookmarks;
pub mod cdp;
#[cfg(test)]
pub(crate) mod fake_cdp;
pub mod login_data;
pub mod network;
pub mod ombra;
pub mod page;
pub mod prefs;
pub mod process;
pub mod sync;

use crate::fingerprint::Resolved;
use serde::Serialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Что нужно, чтобы собрать команду запуска профиля.
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    /// Имя профиля: движок показывает его в адресной строке каждого окна.
    pub profile_name: String,
    pub profile_dir: PathBuf,
    pub log_path: PathBuf,
    pub resolved: Resolved,
    pub relay_addr: SocketAddr,
    pub headless: bool,
    /// Открыть вкладки прошлой сессии. Стартовые вкладки сюда не входят: их открывает супервизор через CDP после cookies.
    pub restore_tabs: bool,
    pub extra_args: Vec<String>,
    /// Папки расширений набора профиля (распакованные, с `manifest.json`).
    pub extensions: Vec<PathBuf>,
    /// Очеловечивание ввода; `None` — выключено, движок штатный байт в байт.
    pub humanize: Option<HumanizeLaunch>,
}

/// Очеловечивание ввода запуска (этап Э5 движка).
#[derive(Debug, Clone, PartialEq)]
pub struct HumanizeLaunch {
    pub seed: u64,
    pub settings: crate::humanize::Humanize,
    /// Журнал плана и факта для стенда движка (`OMBRA_HUMANIZE_DEBUG`); пишет его только сборка с отладкой.
    pub debug_log: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub profile_dir: PathBuf,
    pub log_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EngineBuild {
    pub version: String,
    pub binary: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StopReport {
    pub exit: ExitInfo,
    pub forced: bool,
    pub stop_ms: u64,
    pub leftover_pids: Vec<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("движок не установлен")]
    NotInstalled,
    #[error(transparent)]
    Args(#[from] args::ArgsError),
    #[error("файл: {0}")]
    Io(#[from] std::io::Error),
    #[error("установка движка: {0}")]
    Install(String),
    #[error("{0}")]
    Update(&'static str),
    #[error("браузер завершился до готовности: код {:?}, сигнал {:?}", .0.code, .0.signal)]
    ExitedEarly(ExitInfo),
    #[error("Браузер не ответил за отведённое время")]
    ReadyTimeout,
    #[error("прежний браузер требует ключ; обновите браузер для работы без ключа")]
    AccessRequired,
    #[error("прежний браузер отклонил ключ; обновите браузер для работы без ключа")]
    AccessInvalid,
    #[error("{}", free_device_message(.owner_hint.as_deref()))]
    FreeDeviceBound { owner_hint: Option<String> },
    #[error("прежний браузер требует системный ID; обновите браузер для работы без него")]
    DeviceIdentityUnavailable,
    #[error("запуск отменён")]
    Cancelled,
}

pub(crate) fn masked_owner_hint(value: Option<&str>) -> Option<String> {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let value = value?;
    let pattern = PATTERN.get_or_init(|| regex::Regex::new(
        r"(?i)\A[\p{L}\p{N}._+-]{0,2}\*{3}[\p{L}\p{N}._+-]?@[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)+\z"
    ).expect("masked account pattern"));
    (value.len() <= 254 && pattern.is_match(value)).then(|| value.to_string())
}

fn free_device_message(value: Option<&str>) -> String {
    match masked_owner_hint(value) {
        Some(hint) => format!("прежний браузер связывает устройство с {hint}; обновите браузер для работы без привязки"),
        None => "прежний браузер применяет привязку устройства; обновите браузер для работы без привязки".into(),
    }
}

/// Связь запуска с супервизором: пока браузер ждёт поток лицензии, запуск сообщает его состояние (`no_slot`,
/// `offline`, …) и не ограничен таймаутом готовности; «Стоп» отменяет запуск токеном.
pub struct LaunchHooks {
    pub on_license: Arc<dyn Fn(&str) + Send + Sync>,
    pub cancel: CancellationToken,
}

impl LaunchHooks {
    /// Запуск без супервизора: состояние лицензии никуда не идёт, отмены нет.
    pub fn none() -> Self {
        Self { on_license: Arc::new(|_| {}), cancel: CancellationToken::new() }
    }
}

use std::time::Duration;
use tokio::sync::watch;

/// Запущенный браузер. Клон ждёт того же процесса: так супервизор следит за падением, пока другой код его останавливает.
#[derive(Debug, Clone)]
pub struct EngineProcess {
    pub pid: u32,
    pub debug_port: u16,
    pub ws_endpoint: String,
    pub profile_dir: PathBuf,
    exit: watch::Receiver<Option<ExitInfo>>,
}

impl EngineProcess {
    /// Для подставных движков в тестах.
    pub fn from_parts(
        pid: u32,
        debug_port: u16,
        ws_endpoint: String,
        profile_dir: PathBuf,
        exit: watch::Receiver<Option<ExitInfo>>,
    ) -> Self {
        Self { pid, debug_port, ws_endpoint, profile_dir, exit }
    }

    /// Статус процесса: `None`, пока он работает.
    pub fn exited(&self) -> Option<ExitInfo> {
        *self.exit.borrow()
    }

    pub async fn wait_exit(&mut self) -> ExitInfo {
        let waited = self.exit.wait_for(Option::is_some).await.map(|info| *info);
        waited.ok().flatten().or(*self.exit.borrow()).unwrap_or(ExitInfo { code: None, signal: None })
    }
}

/// Движок браузера (ТЗ, «Движок и адаптер»). Названия флагов знает только реализация.
pub trait Engine: Send + Sync + 'static {
    fn installed(&self) -> Option<EngineBuild>;
    /// Импорт архива сборки (`.tar.gz` с `Chromium.app` в корне) в слот движка; возвращает установленную сборку.
    fn import_archive(&self, archive: &Path) -> impl Future<Output = Result<EngineBuild, EngineError>> + Send;
    fn build_command(&self, req: &LaunchRequest) -> Result<LaunchCommand, EngineError>;
    fn launch(&self, cmd: LaunchCommand, hooks: LaunchHooks) -> impl Future<Output = Result<EngineProcess, EngineError>> + Send;
    /// Ответ `/json/version` браузера на порту отладки: готов, ждёт поток лицензии или ещё не ответил.
    fn probe(&self, port: u16) -> impl Future<Output = process::Probe> + Send;
    /// Принимает ли порт отладки соединения; подставной движок проверяет свой браузер без гонок портов ОС.
    fn debug_port_answers(&self, port: u16) -> impl Future<Output = bool> + Send {
        process::debug_port_answers(port)
    }
    fn stop(
        &self,
        process: &mut EngineProcess,
        grace: Duration,
    ) -> impl Future<Output = Result<StopReport, EngineError>> + Send;
}
