//! Local API на ходу (спецификация 2В-2, «Local API без перезапуска приложения»): включение, порт и ключ
//! меняются без перезапуска приложения. Модуль не знает о Tauri: состояние сервера уходит в приёмник `StatusSink`.
use super::server::{ApiError, ApiServer, ApiSettings, serve};
use crate::app::{CommandError, Core};
use crate::engine::Engine;
use crate::engine::ombra::OmbraEngine;
use crate::fingerprint::FieldError;
use crate::store::{DEFAULT_API_PORT, StoreError};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Наименьший порт Local API: ниже — системные порты.
pub const API_PORT_MIN: u16 = 1024;
/// Длина ключа, который создают «Требовать ключ API» и «Новый ключ».
pub const API_KEY_LEN: usize = 32;
/// Сколько ждать, пока остановленный сервер отпустит порт: приём отменяется в своей задаче, не мгновенно.
const PORT_RELEASE_WAIT: Duration = Duration::from_secs(2);

/// Почему Local API не поднялся — код для интерфейса (`settings.api.failure.<код>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiFailureCode {
    /// Порт занят, в том числе чужим сервером на 0.0.0.0.
    PortBusy,
    /// Настройки не прочитаны: ключ API не расшифровался (база с другой машины) или база не читается.
    SettingsUnreadable,
    /// Прочая ошибка сокета.
    Io,
}

/// Состояние сервера для интерфейса: событие `api-status`, строка «Local API» в боковом меню.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ApiStatus {
    Off,
    Listening { port: u16 },
    /// `code` — интерфейсу, `reason` — русский текст для журнала.
    Failed { port: u16, code: ApiFailureCode, reason: String },
}

/// Куда уходит каждое новое состояние сервера (в приложении — событие Tauri `api-status`).
pub type StatusSink = Arc<dyn Fn(ApiStatus) + Send + Sync>;

/// Карточка «Local API» экрана «Настройки». Ключ в карточке показывается, поэтому здесь он открытым текстом.
#[derive(Clone, Serialize)]
pub struct ApiConfig {
    pub enabled: bool,
    pub port: u16,
    pub require_key: bool,
    /// `None` — ключа нет или он не расшифровался (тогда `require_key` включён: «Новый ключ» заменит его).
    pub key: Option<String>,
    pub status: ApiStatus,
}

// Ключ API — секрет: `Debug` показывает только, задан ли он.
impl fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiConfig")
            .field("enabled", &self.enabled)
            .field("port", &self.port)
            .field("require_key", &self.require_key)
            .field("key", &self.key.as_ref().map(|_| "***"))
            .field("status", &self.status)
            .finish()
    }
}

/// Что меняет карточка «Local API». Сам ключ форма не несёт: его создаёт ядро.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfigForm {
    pub enabled: bool,
    pub port: u16,
    pub require_key: bool,
}

/// 32 символа `[A-Za-z0-9]` из системного генератора. Байты от 248 отбрасываются: остаток от деления на 62 равновероятен.
pub fn generate_api_key() -> String {
    const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut key = String::with_capacity(API_KEY_LEN);
    while key.len() < API_KEY_LEN {
        let mut bytes = [0u8; 64];
        getrandom::fill(&mut bytes).expect("системный генератор случайных чисел недоступен");
        for byte in bytes.into_iter().filter(|byte| *byte < 248).take(API_KEY_LEN - key.len()) {
            key.push(char::from(ALPHABET[usize::from(byte % 62)]));
        }
    }
    key
}

/// Сохранённый ключ API.
enum StoredKey {
    Absent,
    Key(String),
    /// Не расшифровался: база перенесена с другой машины.
    Unreadable,
}

/// Работающий сервер и настройки, с которыми он поднят.
struct Running {
    server: ApiServer,
    settings: ApiSettings,
}

struct Inner {
    running: Option<Running>,
    status: ApiStatus,
}

/// Сервер Local API и его состояние. Все изменения — под одним мьютексом: два «Применить» не поднимут два сервера.
pub struct ApiController<E: Engine = OmbraEngine> {
    core: Arc<Core<E>>,
    sink: StatusSink,
    inner: Mutex<Inner>,
    /// Тесты: следующий подъём сервера с новыми настройками отвечает «порт занят».
    #[cfg(test)]
    fail_next_serve: std::sync::atomic::AtomicBool,
}

impl<E: Engine> ApiController<E> {
    pub fn new(core: Arc<Core<E>>, sink: StatusSink) -> Self {
        Self {
            core,
            sink,
            inner: Mutex::new(Inner { running: None, status: ApiStatus::Off }),
            #[cfg(test)]
            fail_next_serve: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Запуск приложения: сервер по настройкам. Выключен — `Off`; не поднялся — `Failed`, приложение работает дальше.
    pub async fn start(&self) {
        let mut inner = self.inner.lock().await;
        if inner.running.is_some() {
            return;
        }
        let status = match self.saved_settings() {
            Ok(None) => ApiStatus::Off,
            Ok(Some(settings)) => {
                let port = settings.port;
                match serve(self.core.clone(), settings.clone()).await {
                    Ok(server) => {
                        inner.running = Some(Running { server, settings });
                        ApiStatus::Listening { port }
                    }
                    Err(e) => failed(port, &e),
                }
            }
            Err(e) => ApiStatus::Failed {
                port: self.core.store.api_port().unwrap_or(DEFAULT_API_PORT),
                code: ApiFailureCode::SettingsUnreadable,
                reason: format!("настройки Local API не прочитаны: {e}"),
            },
        };
        self.publish(&mut inner, status);
    }

    /// Выход из приложения: порт закрывается сразу, идущие запросы доходят сами (`ApiServer::shutdown`).
    /// Настройка «включён» не меняется.
    pub async fn stop(&self) {
        let mut inner = self.inner.lock().await;
        self.stop_locked(&mut inner);
    }

    /// То же для `RunEvent::Exit`: там нет рантайма tokio, а `blocking_lock` внутри рантайма паникует.
    pub fn stop_blocking(&self) {
        let mut inner = self.inner.blocking_lock();
        self.stop_locked(&mut inner);
    }

    pub async fn status(&self) -> ApiStatus {
        self.inner.lock().await.status.clone()
    }

    pub async fn config(&self) -> Result<ApiConfig, CommandError> {
        let status = self.inner.lock().await.status.clone();
        self.config_with(status)
    }

    /// Новые настройки сохраняются, только если сервер поднялся (или выключен). Иначе работает прежний.
    pub async fn apply(&self, form: &ApiConfigForm) -> Result<ApiConfig, CommandError> {
        if form.port < API_PORT_MIN {
            return Err(field_error("port", "out_of_range"));
        }
        let mut inner = self.inner.lock().await;
        let key = match (form.require_key, self.stored_key()?) {
            (false, _) => None,
            (true, StoredKey::Key(key)) => Some(key),
            // Ключа нет или он не расшифровался — с ним сервер всё равно не поднять.
            (true, StoredKey::Absent | StoredKey::Unreadable) => Some(generate_api_key()),
        };
        self.apply_locked(&mut inner, form, key).await
    }

    /// «Новый ключ» — только при включённом «Требовать ключ API»; сервер перезапускается на том же порту.
    pub async fn regenerate_key(&self) -> Result<ApiConfig, CommandError> {
        let mut inner = self.inner.lock().await;
        if matches!(self.stored_key()?, StoredKey::Absent) {
            return Err(field_error("require_key", "invalid"));
        }
        let store = &self.core.store;
        let form = ApiConfigForm { enabled: store.api_enabled()?, port: store.api_port()?, require_key: true };
        self.apply_locked(&mut inner, &form, Some(generate_api_key())).await
    }

    async fn apply_locked(
        &self,
        inner: &mut Inner,
        form: &ApiConfigForm,
        key: Option<String>,
    ) -> Result<ApiConfig, CommandError> {
        if !form.enabled {
            self.save(form, key.as_deref())?;
            if let Some(old) = inner.running.take() {
                shutdown_and_wait(old).await;
            }
            self.publish(inner, ApiStatus::Off);
            return self.config_with(ApiStatus::Off);
        }
        let settings = ApiSettings { port: form.port, key };
        let previous = inner.running.as_ref().map(|running| running.settings.clone());
        if previous.as_ref() == Some(&settings) {
            // Сервер уже слушает с этими портом и ключом.
            self.save(form, settings.key.as_deref())?;
            return self.config_with(inner.status.clone());
        }
        // Новый порт — сначала новый сервер: занятый порт не трогает работающий.
        // Тот же порт с другим ключом — прежний сначала отпускает порт; новый не поднялся — прежний возвращается.
        let moving = previous.is_some_and(|old| old.port != settings.port);
        let mut stopped = None;
        if !moving && let Some(old) = inner.running.take() {
            stopped = Some(old.settings.clone());
            shutdown_and_wait(old).await;
        }
        let server = match self.serve_new(settings.clone()).await {
            Ok(server) => server,
            Err(e) => {
                // `restore` при том же состоянии ничего не публикует — причину пишем здесь.
                log::warn!("Local API: новые настройки не применены: {e}");
                if let Some(previous) = stopped {
                    self.restore(inner, previous).await;
                }
                return Err(start_error(e));
            }
        };
        if let Err(e) = self.save(form, settings.key.as_deref()) {
            // Настройки не сохранились: новый сервер не нужен, прежний возвращается (при смене порта он и не уходил).
            log::warn!("Local API: новые настройки не применены, не сохранились: {}", e.message);
            shutdown_and_wait(Running { server, settings }).await;
            if let Some(previous) = stopped {
                self.restore(inner, previous).await;
            }
            return Err(e);
        }
        let port = settings.port;
        if let Some(old) = inner.running.replace(Running { server, settings }) {
            old.server.shutdown();
        }
        let status = ApiStatus::Listening { port };
        self.publish(inner, status.clone());
        self.config_with(status)
    }

    /// Прежний сервер остановлен ради нового на том же порту, а новый не поднялся или настройки не сохранились:
    /// прежний поднимается снова с прежними настройками. Не поднялся и он — «не запустился».
    async fn restore(&self, inner: &mut Inner, previous: ApiSettings) {
        let port = previous.port;
        let status = match serve(self.core.clone(), previous.clone()).await {
            Ok(server) => {
                inner.running = Some(Running { server, settings: previous });
                ApiStatus::Listening { port }
            }
            Err(e) => failed(port, &e),
        };
        // Сервер вернулся на тот же порт — для интерфейса ничего не менялось.
        if inner.status != status {
            self.publish(inner, status);
        }
    }

    /// Сервер с новыми настройками. Неудачу на том же порту снаружи не воспроизвести (прежний сервер уже отпустил
    /// порт), поэтому тесты заказывают её полем `fail_next_serve`.
    async fn serve_new(&self, settings: ApiSettings) -> Result<ApiServer, ApiError> {
        if self.serve_must_fail() {
            return Err(ApiError::PortBusy(settings.port));
        }
        serve(self.core.clone(), settings).await
    }

    #[cfg(test)]
    fn serve_must_fail(&self) -> bool {
        self.fail_next_serve.swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(not(test))]
    fn serve_must_fail(&self) -> bool {
        false
    }

    /// Поля формы и ключ — одной транзакцией, после того как сервер поднялся или выключен.
    fn save(&self, form: &ApiConfigForm, key: Option<&str>) -> Result<(), CommandError> {
        Ok(self.core.store.set_api_config(form.port, key, form.enabled)?)
    }

    fn config_with(&self, status: ApiStatus) -> Result<ApiConfig, CommandError> {
        let store = &self.core.store;
        let (require_key, key) = match self.stored_key()? {
            StoredKey::Absent => (false, None),
            StoredKey::Key(key) => (true, Some(key)),
            StoredKey::Unreadable => (true, None),
        };
        Ok(ApiConfig { enabled: store.api_enabled()?, port: store.api_port()?, require_key, key, status })
    }

    fn stored_key(&self) -> Result<StoredKey, CommandError> {
        match self.core.store.api_key() {
            Ok(Some(key)) => Ok(StoredKey::Key(key)),
            Ok(None) => Ok(StoredKey::Absent),
            Err(StoreError::Crypto) => Ok(StoredKey::Unreadable),
            Err(e) => Err(e.into()),
        }
    }

    /// `None` — Local API выключен. Ключ не прочитан — ошибка: сервер без ключа, который задал пользователь,
    /// был бы открыт любому скрипту.
    fn saved_settings(&self) -> Result<Option<ApiSettings>, StoreError> {
        if !self.core.store.api_enabled()? {
            return Ok(None);
        }
        ApiSettings::load(&self.core.store).map(Some)
    }

    fn stop_locked(&self, inner: &mut Inner) {
        if let Some(running) = inner.running.take() {
            running.server.shutdown();
        }
        self.publish(inner, ApiStatus::Off);
    }

    /// Новое состояние — в журнал, в `Inner` и приёмнику.
    fn publish(&self, inner: &mut Inner, status: ApiStatus) {
        match &status {
            ApiStatus::Listening { port } => log::info!("Local API слушает 127.0.0.1:{port}"),
            ApiStatus::Failed { reason, .. } => log::warn!("Local API недоступен: {reason}"),
            ApiStatus::Off => log::info!("Local API выключен"),
        }
        inner.status = status.clone();
        (self.sink)(status);
    }
}

fn field_error(field: &'static str, code: &'static str) -> CommandError {
    CommandError::validation(vec![FieldError { field, code }])
}

/// Код причины ошибки подъёма: порт занят (в том числе `AddrInUse` от `bind`) или прочая ошибка сокета.
fn failure_code(error: &ApiError) -> ApiFailureCode {
    match error {
        ApiError::PortBusy(_) => ApiFailureCode::PortBusy,
        ApiError::Io(e) if e.kind() == std::io::ErrorKind::AddrInUse => ApiFailureCode::PortBusy,
        ApiError::Io(_) => ApiFailureCode::Io,
    }
}

/// «Не запустился» с кодом и текстом ошибки подъёма.
fn failed(port: u16, error: &ApiError) -> ApiStatus {
    ApiStatus::Failed { port, code: failure_code(error), reason: error.to_string() }
}

/// Порт занят — ошибка поля «Порт»; остальное — ошибка файловой системы.
fn start_error(error: ApiError) -> CommandError {
    match failure_code(&error) {
        ApiFailureCode::PortBusy => field_error("port", "busy"),
        ApiFailureCode::SettingsUnreadable | ApiFailureCode::Io => CommandError::new("Io", error.to_string()),
    }
}

/// Остановить сервер и дождаться, пока он отпустит порт: иначе тот же порт сразу после остановки — «занят».
async fn shutdown_and_wait(running: Running) {
    let port = running.settings.port;
    running.server.shutdown();
    let deadline = tokio::time::Instant::now() + PORT_RELEASE_WAIT;
    while tokio::time::Instant::now() < deadline {
        let probe =
            tokio::time::timeout(Duration::from_millis(200), tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)))
                .await;
        if !matches!(probe, Ok(Ok(_))) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::test_core;
    use crate::relay::test_support::{closed_port, port_closes};
    use crate::store::API_KEY_KEY;
    use crate::store::tests::{foreign_secret, put_raw_setting};
    use std::sync::atomic::Ordering;
    use crate::supervisor::test_engine::FakeEngine;
    use std::net::SocketAddr;

    struct Harness {
        core: Arc<Core<FakeEngine>>,
        api: Arc<ApiController<FakeEngine>>,
        /// Все состояния, ушедшие в приёмник, по порядку.
        seen: Arc<std::sync::Mutex<Vec<ApiStatus>>>,
        client: reqwest::Client,
        _dir: tempfile::TempDir,
    }

    /// Контроллер над ядром с подставным движком. Порт Local API в настройках — свободный (не 50325) и возвращается.
    async fn harness() -> (Harness, u16) {
        let (core, _, dir) = test_core().await;
        let core = Arc::new(core);
        let port = closed_port().await;
        core.store.set_api_port(port).unwrap();
        let seen: Arc<std::sync::Mutex<Vec<ApiStatus>>> = Arc::default();
        let record = seen.clone();
        let sink: StatusSink = Arc::new(move |status: ApiStatus| record.lock().unwrap().push(status));
        let api = Arc::new(ApiController::new(core.clone(), sink));
        // Без пула соединений: каждый запрос идёт новым соединением к серверу, который слушает сейчас.
        let client = reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).build().unwrap();
        (Harness { core, api, seen, client, _dir: dir }, port)
    }

    fn form(enabled: bool, port: u16, require_key: bool) -> ApiConfigForm {
        ApiConfigForm { enabled, port, require_key }
    }

    fn local(port: u16) -> SocketAddr {
        (Ipv4Addr::LOCALHOST, port).into()
    }

    /// `code` ответа `/status`: `Some(0)` — принят, `Some(-1)` — отклонён, `None` — порт не отвечает.
    async fn status_code(h: &Harness, port: u16, key: Option<&str>) -> Option<i64> {
        let mut request = h.client.get(format!("http://127.0.0.1:{port}/status")).timeout(Duration::from_secs(2));
        if let Some(key) = key {
            request = request.bearer_auth(key);
        }
        let body: serde_json::Value = request.send().await.ok()?.json().await.ok()?;
        body["code"].as_i64()
    }

    /// Контроллер лежит в `OnceLock` состояния и вызывается из `tauri::async_runtime::spawn`: ему и его futures нужен `Send`.
    #[test]
    fn controller_and_its_futures_are_send() {
        fn send<T: Send>(_: &T) {}
        fn sync<T: Send + Sync>() {}
        sync::<ApiController<FakeEngine>>();
        sync::<ApiController>();
        async fn futures(api: &ApiController<FakeEngine>) {
            let f = ApiConfigForm { enabled: true, port: 50325, require_key: false };
            send(&api.start());
            send(&api.stop());
            send(&api.apply(&f));
            send(&api.config());
            send(&api.regenerate_key());
        }
        let _ = futures;
    }

    #[tokio::test]
    async fn switching_off_frees_the_port_and_on_listens_again() {
        let (h, port) = harness().await;
        h.api.start().await;
        assert_eq!(h.api.status().await, ApiStatus::Listening { port });
        assert_eq!(status_code(&h, port, None).await, Some(0));

        let off = h.api.apply(&form(false, port, false)).await.unwrap();
        assert_eq!((off.enabled, off.status), (false, ApiStatus::Off));
        assert!(!h.core.store.api_enabled().unwrap());
        assert!(port_closes(local(port)).await, "выключенный Local API отпускает порт");

        // Сразу же включить: порт уже свободен, «занят» не будет.
        let on = h.api.apply(&form(true, port, false)).await.unwrap();
        assert_eq!(on.status, ApiStatus::Listening { port });
        assert_eq!(status_code(&h, port, None).await, Some(0));
        assert_eq!(
            *h.seen.lock().unwrap(),
            [ApiStatus::Listening { port }, ApiStatus::Off, ApiStatus::Listening { port }],
            "состояния по порядку"
        );

        h.api.apply(&form(false, port, false)).await.unwrap();
        let next_launch = ApiController::new(h.core.clone(), Arc::new(|_: ApiStatus| {}));
        next_launch.start().await;
        assert_eq!(next_launch.status().await, ApiStatus::Off, "выключенный не поднимается при запуске приложения");
        assert!(port_closes(local(port)).await);
    }

    #[tokio::test]
    async fn new_port_moves_the_server() {
        let (h, first) = harness().await;
        h.api.start().await;
        let second = closed_port().await;
        let config = h.api.apply(&form(true, second, false)).await.unwrap();
        assert_eq!((config.port, config.status), (second, ApiStatus::Listening { port: second }));
        assert_eq!(h.core.store.api_port().unwrap(), second);
        assert_eq!(status_code(&h, second, None).await, Some(0));
        assert!(port_closes(local(first)).await, "прежний порт закрыт");
    }

    #[tokio::test]
    async fn busy_port_keeps_the_running_server() {
        let (h, port) = harness().await;
        h.api.start().await;
        // Порт держит чужая программа, как AdsPower на 50325.
        let foreign = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let busy = foreign.local_addr().unwrap().port();
        let err = h.api.apply(&form(true, busy, false)).await.unwrap_err();
        assert_eq!((err.code.as_str(), err.fields), ("Validation", Some(vec![FieldError { field: "port", code: "busy" }])));
        assert_eq!(h.core.store.api_port().unwrap(), port, "настройки не сохранены");
        assert_eq!(h.api.status().await, ApiStatus::Listening { port });
        assert_eq!(status_code(&h, port, None).await, Some(0), "прежний сервер отвечает");
        assert_eq!(h.seen.lock().unwrap().len(), 1, "состояние не менялось");
        drop(foreign);
    }

    #[tokio::test]
    async fn port_out_of_range_changes_nothing() {
        let (h, port) = harness().await;
        h.api.start().await;
        for bad in [80, API_PORT_MIN - 1] {
            let err = h.api.apply(&form(true, bad, false)).await.unwrap_err();
            assert_eq!(err.fields, Some(vec![FieldError { field: "port", code: "out_of_range" }]), "{bad}");
        }
        assert_eq!(h.core.store.api_port().unwrap(), port);
        assert_eq!(h.api.status().await, ApiStatus::Listening { port });
    }

    #[tokio::test]
    async fn required_key_is_generated_checked_and_replaced() {
        let (h, port) = harness().await;
        h.api.start().await;
        let config = h.api.apply(&form(true, port, true)).await.unwrap();
        let key = config.key.clone().expect("ключ сгенерирован");
        assert!(config.require_key);
        assert_eq!(key.len(), API_KEY_LEN);
        assert!(key.chars().all(|c| c.is_ascii_alphanumeric()), "{key}");
        assert!(!format!("{config:?}").contains(&key), "ключ в Debug");
        assert_eq!(status_code(&h, port, None).await, Some(-1), "без ключа отказ");
        assert_eq!(status_code(&h, port, Some(key.as_str())).await, Some(0));
        let again = h.api.apply(&form(true, port, true)).await.unwrap();
        assert_eq!(again.key, Some(key.clone()), "включённый ключ не меняется");

        let fresh = h.api.regenerate_key().await.unwrap().key.expect("новый ключ");
        assert_ne!(fresh, key);
        assert_eq!(status_code(&h, port, Some(key.as_str())).await, Some(-1), "старый ключ отклоняется");
        assert_eq!(status_code(&h, port, Some(fresh.as_str())).await, Some(0));
        assert_eq!(h.core.store.api_key().unwrap(), Some(fresh));

        let open = h.api.apply(&form(true, port, false)).await.unwrap();
        assert_eq!((open.require_key, open.key), (false, None));
        assert_eq!(h.core.store.api_key().unwrap(), None);
        assert_eq!(status_code(&h, port, None).await, Some(0));
        let err = h.api.regenerate_key().await.unwrap_err();
        assert_eq!(err.fields, Some(vec![FieldError { field: "require_key", code: "invalid" }]));
    }

    #[tokio::test]
    async fn unreadable_key_blocks_the_server_until_replaced() {
        let (h, port) = harness().await;
        put_raw_setting(&h.core.store, API_KEY_KEY, &foreign_secret("old-key"));
        h.api.start().await;
        assert!(
            matches!(
                h.api.status().await,
                ApiStatus::Failed { port: p, code: ApiFailureCode::SettingsUnreadable, .. } if p == port
            ),
            "сервер без ключа, который задал пользователь, не поднимается"
        );
        let config = h.api.config().await.unwrap();
        assert_eq!((config.require_key, config.key), (true, None));
        let fresh = h.api.regenerate_key().await.unwrap();
        assert_eq!(fresh.status, ApiStatus::Listening { port });
        assert_eq!(status_code(&h, port, fresh.key.as_deref()).await, Some(0));
    }

    #[tokio::test]
    async fn stop_closes_the_port_and_keeps_the_setting() {
        let (h, port) = harness().await;
        h.api.start().await;
        h.api.stop().await;
        assert_eq!(h.api.status().await, ApiStatus::Off);
        assert!(port_closes(local(port)).await);
        assert!(h.core.store.api_enabled().unwrap(), "остановка на выходе настройку не меняет");

        h.api.start().await;
        assert_eq!(status_code(&h, port, None).await, Some(0));
        let api = h.api.clone();
        // Как `RunEvent::Exit`: вне рантайма tokio.
        tokio::task::spawn_blocking(move || api.stop_blocking()).await.unwrap();
        assert!(port_closes(local(port)).await);
        assert_eq!(
            *h.seen.lock().unwrap(),
            [ApiStatus::Listening { port }, ApiStatus::Off, ApiStatus::Listening { port }, ApiStatus::Off]
        );
    }

    #[tokio::test]
    async fn failed_restart_on_the_same_port_brings_back_the_previous_server() {
        let (h, port) = harness().await;
        h.api.start().await;
        let key = h.api.apply(&form(true, port, true)).await.unwrap().key.expect("ключ включён");
        let published = h.seen.lock().unwrap().len();

        // Новый сервер на том же порту не поднялся: прежний поднимается снова с прежним ключом.
        h.api.fail_next_serve.store(true, Ordering::SeqCst);
        let err = h.api.regenerate_key().await.unwrap_err();
        assert_eq!(err.fields, Some(vec![FieldError { field: "port", code: "busy" }]));
        assert_eq!(h.core.store.api_key().unwrap(), Some(key.clone()), "настройки не изменены");
        assert_eq!(h.api.status().await, ApiStatus::Listening { port });
        assert_eq!(status_code(&h, port, Some(key.as_str())).await, Some(0), "прежний сервер отвечает");

        // Настройки не сохранились (сбой базы на последней записи): новый сервер закрыт, прежний вернулся.
        h.core
            .store
            .raw()
            .execute_batch(
                "CREATE TRIGGER fail_enabled_insert BEFORE INSERT ON settings WHEN NEW.key = 'api.enabled' \
                 BEGIN SELECT RAISE(ABORT, 'сбой записи'); END; \
                 CREATE TRIGGER fail_enabled_update BEFORE UPDATE ON settings WHEN NEW.key = 'api.enabled' \
                 BEGIN SELECT RAISE(ABORT, 'сбой записи'); END;",
            )
            .unwrap();
        assert_eq!(h.api.regenerate_key().await.unwrap_err().code, "Storage");
        assert_eq!(h.core.store.api_key().unwrap(), Some(key.clone()), "транзакция откатилась целиком");
        assert_eq!(status_code(&h, port, Some(key.as_str())).await, Some(0));
        assert_eq!(h.seen.lock().unwrap().len(), published, "для интерфейса сервер не менялся");
    }

    #[tokio::test]
    async fn unreadable_key_is_replaced_by_require_key() {
        let (h, port) = harness().await;
        put_raw_setting(&h.core.store, API_KEY_KEY, &foreign_secret("old-key"));
        h.api.start().await;
        let config = h.api.apply(&form(true, port, true)).await.unwrap();
        let key = config.key.expect("новый ключ вместо нечитаемого");
        assert_eq!(config.status, ApiStatus::Listening { port });
        assert_eq!(h.core.store.api_key().unwrap(), Some(key.clone()));
        assert_eq!(status_code(&h, port, Some(key.as_str())).await, Some(0));
    }

    #[tokio::test]
    async fn two_applies_at_once_leave_one_server() {
        let (h, first) = harness().await;
        h.api.start().await;
        let (second, third) = (closed_port().await, closed_port().await);
        let (form_a, form_b) = (form(true, second, false), form(true, third, false));
        let (a, b) = tokio::join!(h.api.apply(&form_a), h.api.apply(&form_b));
        a.unwrap();
        b.unwrap();
        let port = h.core.store.api_port().unwrap();
        assert_eq!(h.api.status().await, ApiStatus::Listening { port }, "сохранён порт работающего сервера");
        assert_eq!(status_code(&h, port, None).await, Some(0));
        for other in [first, second, third].into_iter().filter(|other| *other != port) {
            assert!(port_closes(local(other)).await, "порт {other} закрыт");
        }
    }

    #[tokio::test]
    async fn failure_carries_a_code_for_the_interface() {
        let (h, _) = harness().await;
        // Порт держит чужая программа.
        let foreign = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let busy = foreign.local_addr().unwrap().port();
        h.core.store.set_api_port(busy).unwrap();
        h.api.start().await;
        let status = h.api.status().await;
        assert!(
            matches!(&status, ApiStatus::Failed { port, code: ApiFailureCode::PortBusy, .. } if *port == busy),
            "{status:?}"
        );
        drop(foreign);

        put_raw_setting(&h.core.store, API_KEY_KEY, &foreign_secret("old-key"));
        let next_launch = ApiController::new(h.core.clone(), Arc::new(|_: ApiStatus| {}));
        next_launch.start().await;
        let status = next_launch.status().await;
        assert!(matches!(status, ApiStatus::Failed { code: ApiFailureCode::SettingsUnreadable, .. }), "{status:?}");

        let io = |kind: std::io::ErrorKind| failure_code(&ApiError::Io(kind.into()));
        assert_eq!(io(std::io::ErrorKind::AddrInUse), ApiFailureCode::PortBusy);
        assert_eq!(io(std::io::ErrorKind::PermissionDenied), ApiFailureCode::Io);
        assert_eq!(failure_code(&ApiError::PortBusy(busy)), ApiFailureCode::PortBusy);
    }
}
