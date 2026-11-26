//! Подставной движок и окружение для тестов супервизора и ядра приложения.
use crate::engine::{
    Engine, EngineBuild, EngineError, EngineProcess, ExitInfo, LaunchCommand, LaunchHooks, LaunchRequest, StopReport,
    args,
};
use crate::engine::process::Probe;
use crate::engine::fake_cdp::FakeCdp;
use crate::fingerprint::{Arch, Catalog, OsRandom, generate};
use crate::relay::geo::{GeoClient, Provider};
use crate::relay::test_support::{closed_port, spawn_http_proxy, spawn_origin};
use crate::store::Store;
use crate::store::profiles::ProfileInput;
use crate::store::proxies::{ProxyInput, ProxyKind};
use crate::store::secrets::{Secret, SecretField};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, watch};

pub(crate) const VERSION: &str = "150.0.7871.114";
pub(crate) const IPWHOIS_BERLIN: &str =
    r#"{"ip":"203.0.113.24","success":true,"country_code":"DE","timezone":{"id":"Europe/Berlin"}}"#;

/// Ничего не запускает: «процесс» живёт, пока тест не вызовет `crash` или супервизор `stop`.
/// У каждого профиля свой подставной CDP: его «банка» cookies переживает перезапуск, как папка профиля.
pub(crate) struct FakeEngine {
    pub installed: AtomicBool,
    /// Сколько раз вызвали `import_archive`.
    imports: AtomicU32,
    pub fail_next_launch: Mutex<Option<ExitInfo>>,
    /// Следующий `stop` вернёт ошибку, а «процесс» продолжит жить.
    pub fail_next_stop: AtomicBool,
    /// Запуск отдаёт адрес CDP, по которому никто не слушает.
    pub cdp_down: AtomicBool,
    /// Следующий запуск ждёт `notify_one` на этом сигнале, прежде чем отдать «процесс».
    pub hold_next_launch: Mutex<Option<Arc<Notify>>>,
    /// Следующий запуск паникует (после `hold_next_launch`): проверка уборки оборванной задачи запуска.
    pub panic_next_launch: AtomicBool,
    /// Следующий `stop` ждёт `notify_one` на этом сигнале, прежде чем завершить «процесс».
    pub hold_next_stop: Mutex<Option<Arc<Notify>>>,
    /// Имена cookies, которые отвергнет `FakeCdp` каждого следующего запуска.
    rejected: Mutex<Vec<String>>,
    /// Сколько «запускается» каждый `launch` (по умолчанию сразу).
    pub launch_delay: Mutex<Duration>,
    /// Что браузер отвечает на `/json/version` (`answer`): `launch` ждёт, пока это `License` не с `ok`, и сообщает
    /// состояние в `on_license`; `probe` работающего браузера отдаёт его как есть. По умолчанию `Ready`.
    answer: Mutex<Probe>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    launches: AtomicU32,
    last_request: Mutex<Option<LaunchRequest>>,
    exits: Mutex<HashMap<PathBuf, watch::Sender<Option<ExitInfo>>>>,
    /// Порт отладки «процесса»: слушает, пока процесс жив, и закрывается сразу при выходе, как у настоящего браузера.
    debug_ports: Mutex<HashMap<PathBuf, std::net::TcpListener>>,
    cdps: Mutex<HashMap<PathBuf, FakeCdp>>,
    next_pid: AtomicU32,
}

impl FakeEngine {
    pub fn imports(&self) -> u32 {
        self.imports.load(Ordering::SeqCst)
    }

    pub fn new() -> Self {
        Self {
            installed: AtomicBool::new(true),
            imports: AtomicU32::new(0),
            fail_next_launch: Mutex::new(None),
            fail_next_stop: AtomicBool::new(false),
            cdp_down: AtomicBool::new(false),
            hold_next_launch: Mutex::new(None),
            panic_next_launch: AtomicBool::new(false),
            hold_next_stop: Mutex::new(None),
            rejected: Mutex::new(Vec::new()),
            launch_delay: Mutex::new(Duration::ZERO),
            answer: Mutex::new(Probe::Ready(String::new())),
            in_flight: Arc::default(),
            max_in_flight: Arc::default(),
            launches: AtomicU32::new(0),
            last_request: Mutex::new(None),
            exits: Mutex::new(HashMap::new()),
            debug_ports: Mutex::new(HashMap::new()),
            cdps: Mutex::new(HashMap::new()),
            next_pid: AtomicU32::new(50_000),
        }
    }

    /// Браузер профиля «падает» с кодом выхода (`0` — закрыт пользователем или `browser.close()`);
    /// его порт отладки и CDP закрываются.
    pub fn crash(&self, profile_dir: &Path, code: i32) {
        self.exit(profile_dir, ExitInfo { code: Some(code), signal: None });
    }

    /// Браузер получил `Browser.close`: порт отладки уже закрыт, а процесс ещё не вышел (выход — `crash`).
    pub fn close_debug_port(&self, profile_dir: &Path) {
        self.debug_ports.lock().unwrap().remove(profile_dir);
    }

    /// «Процесс» вышел: порт отладки закрывается сразу, супервизор узнаёт о выходе, CDP закрывается.
    fn exit(&self, profile_dir: &Path, info: ExitInfo) {
        self.close_debug_port(profile_dir);
        if let Some(tx) = self.exits.lock().unwrap().remove(profile_dir) {
            let _ = tx.send(Some(info));
        }
        self.cdp(profile_dir).go_down();
    }

    /// Сколько раз движок запускал браузер.
    pub fn launches(&self) -> u32 {
        self.launches.load(Ordering::SeqCst)
    }

    /// Наибольшее число одновременно идущих `launch` за время теста.
    pub fn max_parallel_launches(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }

    /// С этого момента браузеры отвечают на `/json/version` так.
    pub fn answer(&self, probe: Probe) {
        *self.answer.lock().unwrap() = probe;
    }

    /// Браузеры следующих запусков отвергнут cookie с этим именем.
    pub fn reject_cookie(&self, name: &str) {
        self.rejected.lock().unwrap().push(name.to_string());
    }

    /// «Процесс» профиля ещё жив: его не остановили и он не «упал».
    pub fn is_running(&self, profile_dir: &Path) -> bool {
        self.exits.lock().unwrap().contains_key(profile_dir)
    }

    pub fn last_request(&self) -> LaunchRequest {
        self.last_request.lock().unwrap().clone().expect("запуска не было")
    }

    /// Подставной CDP профиля; до первого запуска — незапущенный, чтобы тест мог настроить его заранее.
    pub fn cdp(&self, profile_dir: &Path) -> FakeCdp {
        self.cdps.lock().unwrap().entry(profile_dir.to_path_buf()).or_default().clone()
    }
}

/// Отметка «идёт запуск»: счётчик растёт при создании и падает, когда будущее `launch` завершилось или брошено.
struct InFlight(Arc<AtomicUsize>);

impl InFlight {
    fn enter(current: &Arc<AtomicUsize>, max: &AtomicUsize) -> Self {
        let now = current.fetch_add(1, Ordering::SeqCst) + 1;
        max.fetch_max(now, Ordering::SeqCst);
        Self(current.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Engine for FakeEngine {
    fn installed(&self) -> Option<EngineBuild> {
        self.installed
            .load(Ordering::SeqCst)
            .then(|| EngineBuild { version: VERSION.into(), binary: PathBuf::from("/fake/Chromium") })
    }

    fn import_archive(&self, _archive: &Path) -> impl Future<Output = Result<EngineBuild, EngineError>> + Send {
        self.imports.fetch_add(1, Ordering::SeqCst);
        self.installed.store(true, Ordering::SeqCst);
        let build = self.installed();
        async move { build.ok_or(EngineError::NotInstalled) }
    }

    fn build_command(&self, req: &LaunchRequest) -> Result<LaunchCommand, EngineError> {
        args::check_extra_args(&req.extra_args)?;
        *self.last_request.lock().unwrap() = Some(req.clone());
        Ok(LaunchCommand {
            program: PathBuf::from("/fake/Chromium"),
            // По флагу `launch` решает, открыть ли окно с новой вкладкой.
            args: if req.headless { vec!["--headless".into()] } else { Vec::new() },
            profile_dir: req.profile_dir.clone(),
            log_path: req.log_path.clone(),
        })
    }

    fn launch(&self, cmd: LaunchCommand, hooks: LaunchHooks) -> impl Future<Output = Result<EngineProcess, EngineError>> + Send {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let failure = self.fail_next_launch.lock().unwrap().take();
        let hold = self.hold_next_launch.lock().unwrap().take();
        let panic = self.panic_next_launch.swap(false, Ordering::SeqCst);
        let pid = self.next_pid.fetch_add(1, Ordering::SeqCst);
        let delay = *self.launch_delay.lock().unwrap();
        let in_flight = InFlight::enter(&self.in_flight, &self.max_in_flight);
        let (tx, rx) = watch::channel(None);
        let debug = std::net::TcpListener::bind("127.0.0.1:0").expect("порт отладки подставного браузера");
        let debug_port = debug.local_addr().expect("адрес порта отладки").port();
        if failure.is_none() {
            self.exits.lock().unwrap().insert(cmd.profile_dir.clone(), tx);
            self.debug_ports.lock().unwrap().insert(cmd.profile_dir.clone(), debug);
        }
        let cdp = (!self.cdp_down.load(Ordering::SeqCst)).then(|| self.cdp(&cmd.profile_dir));
        let windowed = !cmd.args.iter().any(|arg| arg == "--headless");
        if let Some(cdp) = &cdp {
            for name in self.rejected.lock().unwrap().iter() {
                cdp.reject_cookie(name);
            }
        }
        async move {
            let _in_flight = in_flight;
            tokio::time::sleep(delay).await;
            if let Some(hold) = hold {
                hold.notified().await;
            }
            if panic {
                panic!("подставной движок: паника запуска по заказу теста");
            }
            if let Some(info) = failure {
                return Err(EngineError::ExitedEarly(info));
            }
            // Как `process::launch`: пока браузер ждёт поток, о смене состояния узнаёт супервизор; «Стоп» убивает.
            let mut reported = None;
            loop {
                let answer = self.answer.lock().unwrap().clone();
                match answer {
                    Probe::License { state: license, .. } if license != "ok" => {
                        if reported.as_ref() != Some(&license) {
                            (hooks.on_license)(&license);
                            reported = Some(license);
                        }
                    }
                    _ => break,
                }
                tokio::select! {
                    _ = hooks.cancel.cancelled() => {
                        self.exit(&cmd.profile_dir, ExitInfo { code: None, signal: Some(libc::SIGKILL) });
                        return Err(EngineError::Cancelled);
                    }
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                }
            }
            let ws_endpoint = match cdp {
                Some(cdp) => {
                    cdp.start().await;
                    // Браузер с окнами открывается с новой вкладкой, без окна — пустым.
                    if windowed {
                        cdp.add_page("chrome://newtab/");
                    }
                    cdp.ws_url()
                }
                None => format!("ws://127.0.0.1:{}/devtools/browser/down", closed_port().await),
            };
            Ok(EngineProcess::from_parts(pid, debug_port, ws_endpoint, cmd.profile_dir, rx))
        }
    }

    fn probe(&self, _port: u16) -> impl Future<Output = Probe> + Send {
        let answer = self.answer.lock().unwrap().clone();
        async move { answer }
    }

    fn debug_port_answers(&self, port: u16) -> impl Future<Output = bool> + Send {
        // Смотрим на собственный браузер: освобождённый TCP-порт ОС может отдать другому тесту.
        let answers = self.debug_ports.lock().unwrap().values()
            .any(|listener| listener.local_addr().unwrap().port() == port);
        async move { answers }
    }

    fn stop(
        &self,
        process: &mut EngineProcess,
        _grace: Duration,
    ) -> impl Future<Output = Result<StopReport, EngineError>> + Send {
        let fail = self.fail_next_stop.swap(false, Ordering::SeqCst);
        let hold = self.hold_next_stop.lock().unwrap().take();
        async move {
            // Пока тест держит остановку, «процесс» жив: порт отладки и CDP отвечают.
            if let Some(hold) = hold {
                hold.notified().await;
            }
            if fail {
                return Err(EngineError::Io(std::io::Error::other("подставной сбой остановки")));
            }
            self.exit(&process.profile_dir, ExitInfo { code: None, signal: Some(libc::SIGTERM) });
            let exit = process.wait_exit().await;
            Ok(StopReport { exit, forced: false, stop_ms: 1, leftover_pids: Vec::new() })
        }
    }
}

/// Геосервис и HTTP-прокси с авторизацией `user:pass`, через которые ходят профили тестов.
pub(crate) struct GeoStack {
    pub geo: GeoClient,
    pub proxy: SocketAddr,
}

/// `geo_status` — что отвечает геосервис: 200 (Берлин) или код ошибки.
pub(crate) async fn geo_stack(geo_status: u16) -> GeoStack {
    let body = if geo_status == 200 { IPWHOIS_BERLIN } else { "{}" };
    let (origin, _) = spawn_origin(geo_status, body).await;
    let (proxy, _) = spawn_http_proxy(Some("dXNlcjpwYXNz")).await;
    let geo = GeoClient::with_endpoints(vec![
        (Provider::IpWhoIs, format!("http://{origin}/")),
        (Provider::IpapiIs, format!("http://{origin}/")),
    ]);
    GeoStack { geo, proxy }
}

/// HTTP-прокси `user:<password>` на `127.0.0.1:<port>` в общем списке.
pub(crate) fn add_proxy(store: &Store, port: u16, password: &str) -> i64 {
    store
        .create_proxy(&ProxyInput {
            kind: ProxyKind::Http,
            host: "127.0.0.1".into(),
            port,
            username: Some("user".into()),
            password: SecretField::Set(Secret::new(password)),
            note: String::new(),
            enabled: true,
        })
        .unwrap()
        .id
}

/// Профиль «Магазин» со свежим отпечатком (таймзона и язык «по IP шлюза»), стартовой вкладкой и закреплённым `proxy_id`.
pub(crate) fn add_profile_with(store: &Store, proxy_id: Option<i64>) -> String {
    let fingerprint = generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap();
    let mut input = ProfileInput::new("Магазин", fingerprint.to_json());
    input.proxy_id = proxy_id;
    input.start_urls = vec!["https://example.com/".into()];
    store.create_profile(&input, 1).unwrap().id
}

/// Профиль со своей записью HTTP-прокси `user:<password>`, если задан порт.
pub(crate) fn add_profile(store: &Store, proxy_port: Option<u16>, password: &str) -> String {
    let proxy_id = proxy_port.map(|port| add_proxy(store, port, password));
    add_profile_with(store, proxy_id)
}

/// Таймауты супервизора для тестов: короткие, но с запасом на медленную сборку.
pub(crate) fn fast_timings() -> crate::supervisor::Timings {
    crate::supervisor::Timings {
        stop_grace: Duration::from_millis(500),
        orphan_grace: Duration::from_millis(500),
        settle: Duration::from_millis(200),
        ready_wait: Duration::from_secs(2),
        cookie_interval: Duration::from_millis(100),
        license_poll: Duration::from_millis(20),
    }
}

/// Ждёт, пока `cond` станет истинным (до 150 проверок с шагом 20 мс); иначе паника с текстом `what`.
pub(crate) async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..150 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("не дождались: {what}");
}
