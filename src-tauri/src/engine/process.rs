use super::{EngineError, EngineProcess, ExitInfo, LaunchCommand, LaunchHooks, StopReport};
use std::io;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::sync::watch;

#[cfg(unix)]
const KILL_SIGNAL: i32 = libc::SIGKILL;
#[cfg(unix)]
const TERM_SIGNAL: i32 = libc::SIGTERM;
#[cfg(windows)]
const KILL_SIGNAL: i32 = 9;
#[cfg(windows)]
const TERM_SIGNAL: i32 = 15;
#[cfg(windows)]
#[path = "process_windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::find_profile_processes;
#[cfg(windows)]
use windows::signal;

/// Процесс относится к профилю, если в его командной строке есть `--user-data-dir=<папка>` целиком.
/// Так себя помечают главный процесс и все помощники Chromium (проверено на 145).
pub fn matches_profile(command: &str, profile_dir: &Path) -> bool {
    let needle = format!("--user-data-dir={}", profile_dir.display());
    command.match_indices(&needle).any(|(start, _)| {
        let rest = &command[start + needle.len()..];
        rest.is_empty() || rest.starts_with(' ')
    })
}

pub fn parse_ps(text: &str) -> Vec<(u32, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, command) = line.split_once(' ')?;
            Some((pid.parse().ok()?, command.trim_start().to_string()))
        })
        .collect()
}

#[cfg(unix)]
pub async fn find_profile_processes(profile_dir: &Path) -> io::Result<Vec<u32>> {
    let output = tokio::process::Command::new("ps").args(["-axww", "-o", "pid=,command="]).output().await?;
    let me = std::process::id();
    Ok(parse_ps(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .filter(|(pid, command)| *pid != me && matches_profile(command, profile_dir))
        .map(|(pid, _)| pid)
        .collect())
}

#[cfg(unix)]
fn signal(pid: u32, sig: i32) {
    unsafe {
        libc::kill(pid as libc::pid_t, sig);
    }
}

/// Ждёт до `settle`, пока процессы профиля исчезнут сами; оставшихся добивает SIGKILL и возвращает их PID.
pub async fn reap_profile_processes(profile_dir: &Path, settle: Duration) -> io::Result<Vec<u32>> {
    let deadline = Instant::now() + settle;
    loop {
        let pids = find_profile_processes(profile_dir).await?;
        if pids.is_empty() {
            return Ok(Vec::new());
        }
        if Instant::now() >= deadline {
            for pid in &pids {
                signal(*pid, KILL_SIGNAL);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            return Ok(pids);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// SIGTERM всем процессам профиля, ожидание до `grace`, затем SIGKILL оставшимся.
/// Нужна для «сирот» при старте приложения: Chromium успевает записать cookies. Возвращает PID, которые пришлось добить.
pub async fn terminate_profile_processes(profile_dir: &Path, grace: Duration) -> io::Result<Vec<u32>> {
    let pids = find_profile_processes(profile_dir).await?;
    if pids.is_empty() {
        return Ok(Vec::new());
    }
    for pid in &pids {
        signal(*pid, TERM_SIGNAL);
    }
    reap_profile_processes(profile_dir, grace).await
}

pub fn parse_devtools_active_port(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim().to_string();
    path.starts_with("/devtools/browser/").then_some((port, path))
}

impl ExitInfo {
    fn from_status(status: std::process::ExitStatus) -> Self {
        Self { code: status.code(), signal: {
            #[cfg(unix)] { status.signal() }
            #[cfg(windows)] { None }
        } }
    }
}

/// Ответ `/json/version`. Браузер с патчем 980 без потока лицензии отвечает `503` с телом
/// `{"ombra":{"license":"<состояние>"}}`; `ok` — профиль ещё создаётся (доли секунды).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// `200`: адрес CDP браузера.
    Ready(String),
    /// `503` с состоянием лицензии: `ok`, `no_key`, `invalid`, `no_slot`, `offline` или новое.
    License { state: String, owner_hint: Option<String> },
    /// Не ответил, ответил иначе или ещё не слушает.
    NotYet,
}

fn probe_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(5)).build()
}

/// Порт отладки принимает соединения. Отказ — браузер закрывается; таймаут — считаем живым,
/// чтобы зависший браузер, как и раньше, отдавался скрипту, а не держал запуск.
pub async fn debug_port_answers(port: u16) -> bool {
    let connect = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port));
    !matches!(tokio::time::timeout(Duration::from_millis(300), connect).await, Ok(Err(_)))
}

pub async fn probe(port: u16) -> Probe {
    match probe_client() {
        Ok(client) => probe_with(&client, port).await,
        Err(_) => Probe::NotYet,
    }
}

/// Запуск опрашивает одним клиентом: новый на каждый опрос — новое соединение и сокет в TIME_WAIT.
async fn probe_with(client: &reqwest::Client, port: u16) -> Probe {
    #[derive(serde::Deserialize)]
    struct Version {
        #[serde(rename = "webSocketDebuggerUrl")]
        ws: String,
    }
    #[derive(serde::Deserialize)]
    struct Waiting {
        ombra: Ombra,
    }
    #[derive(serde::Deserialize)]
    struct Ombra {
        license: String,
        #[serde(rename = "ownerHint", default)]
        owner_hint: serde_json::Value,
    }
    let Ok(response) = client.get(format!("http://127.0.0.1:{port}/json/version")).send().await else {
        return Probe::NotYet;
    };
    match response.status() {
        reqwest::StatusCode::OK => response.json::<Version>().await.map_or(Probe::NotYet, |v| Probe::Ready(v.ws)),
        reqwest::StatusCode::SERVICE_UNAVAILABLE => {
            response.json::<Waiting>().await.map_or(Probe::NotYet, |w| {
                let owner_hint = if w.ombra.license == "device_bound" { super::masked_owner_hint(w.ombra.owner_hint.as_str()) } else { None };
                Probe::License { state: w.ombra.license, owner_hint }
            })
        }
        _ => Probe::NotYet,
    }
}

fn terminal_license_error(state: &str, owner_hint: Option<&str>) -> Option<EngineError> {
    match state {
        "no_key" => Some(EngineError::AccessRequired),
        "invalid" => Some(EngineError::AccessInvalid),
        "device_bound" => Some(EngineError::FreeDeviceBound { owner_hint: super::masked_owner_hint(owner_hint) }),
        "no_device" => Some(EngineError::DeviceIdentityUnavailable),
        _ => None,
    }
}

/// Убивает главный процесс, если запуск прерван (будущее отброшено) до готовности.
struct KillOnDrop(Option<u32>);

impl KillOnDrop {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            signal(pid, KILL_SIGNAL);
        }
    }
}

/// Команда процесса браузера без вывода: программа и аргументы из `LaunchCommand`.
fn command_for(cmd: &LaunchCommand) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&cmd.program);
    command.args(&cmd.args);
    command.stdin(Stdio::null());
    // Ctrl+C терминала достаётся команде, а не браузерам: первый сигнал — «Стоп» с действиями «после задачи».
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x00000200 | 0x08000000); // NEW_PROCESS_GROUP | NO_WINDOW
    command
}

/// Сколько после первого `503` состояние `offline` значит «ещё запускается», как `ok`: до первого ответа сервера
/// лицензий браузер отвечает `offline` (сам он ждёт этот ответ до 3 с).
const OFFLINE_GRACE: Duration = Duration::from_secs(3);
/// Опрос `/json/version` при запуске и, реже, пока браузер ждёт поток (свой поток он перепроверяет раз в 15 с).
const STARTUP_POLL: Duration = Duration::from_millis(100);
const WAITING_POLL: Duration = Duration::from_secs(1);

/// Запуск до ответа `200` на `/json/version`. Таймаут готовности действует до первого `503` с состоянием лицензии
/// не `ok` (и не `offline` в первые `OFFLINE_GRACE`); дальше запуск ждёт поток без таймаута — до `200`, выхода браузера
/// или отмены (`hooks.cancel`). Окно ожидания закрыли (выход с кодом 0) — это отмена, а не сбой.
pub async fn launch(cmd: LaunchCommand, ready_timeout: Duration, hooks: LaunchHooks) -> Result<EngineProcess, EngineError> {
    tokio::fs::create_dir_all(&cmd.profile_dir).await?;
    let port_file = cmd.profile_dir.join("DevToolsActivePort");
    match tokio::fs::remove_file(&port_file).await {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    super::prefs::apply_webrtc_policy(&cmd.profile_dir)?;
    let client = probe_client().map_err(io::Error::other)?;
    if let Some(dir) = cmd.log_path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let log = std::fs::File::create(&cmd.log_path)?;
    let mut child = command_for(&cmd).stdout(Stdio::from(log.try_clone()?)).stderr(Stdio::from(log)).spawn()?;
    let pid = child.id().ok_or_else(|| EngineError::Io(io::Error::other("у процесса нет PID")))?;
    let mut guard = KillOnDrop(Some(pid));
    let (tx, mut rx) = watch::channel(None);
    tokio::spawn(async move {
        let info = match child.wait().await {
            Ok(status) => ExitInfo::from_status(status),
            Err(_) => ExitInfo { code: None, signal: None },
        };
        let _ = tx.send(Some(info));
    });

    let mut deadline = Some(Instant::now() + ready_timeout);
    let mut first_refusal: Option<Instant> = None;
    let mut license: Option<String> = None;
    let failure = loop {
        let exited = *rx.borrow();
        if let Some(info) = exited {
            guard.disarm();
            reap_profile_processes(&cmd.profile_dir, Duration::ZERO).await?;
            if license.is_some() && info.code == Some(0) && info.signal.is_none() {
                return Err(EngineError::Cancelled);
            }
            return Err(EngineError::ExitedEarly(info));
        }
        if let Ok(text) = tokio::fs::read_to_string(&port_file).await
            && let Some((port, _)) = parse_devtools_active_port(&text)
        {
            // Зависший сервер DevTools не держит «Стоп» до таймаута опроса.
            let answer = tokio::select! {
                _ = hooks.cancel.cancelled() => break EngineError::Cancelled,
                answer = probe_with(&client, port) => answer,
            };
            match answer {
                Probe::Ready(ws) => {
                    guard.disarm();
                    return Ok(EngineProcess::from_parts(pid, port, ws, cmd.profile_dir.clone(), rx));
                }
                Probe::License { state, owner_hint } => {
                    // Key and device denials cannot recover by waiting for a free slot.
                    // Reap the failed launch so it does not occupy a profile or proxy indefinitely.
                    if let Some(error) = terminal_license_error(&state, owner_hint.as_deref()) {
                        break error;
                    }
                    let since = *first_refusal.get_or_insert_with(Instant::now);
                    let starting = state == "ok" || (state == "offline" && since.elapsed() < OFFLINE_GRACE);
                    if !starting {
                        deadline = None;
                        if license.as_ref() != Some(&state) {
                            (hooks.on_license)(&state);
                            license = Some(state);
                        }
                    }
                }
                Probe::NotYet => {}
            }
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            log::warn!("Истекло время ожидания подключения CDP к браузеру");
            break EngineError::ReadyTimeout;
        }
        let pause = if license.is_some() { WAITING_POLL } else { STARTUP_POLL };
        tokio::select! {
            _ = hooks.cancel.cancelled() => break EngineError::Cancelled,
            _ = tokio::time::sleep(pause) => {}
        }
    };
    signal(pid, KILL_SIGNAL);
    guard.disarm();
    let _ = rx.wait_for(|v| v.is_some()).await;
    reap_profile_processes(&cmd.profile_dir, Duration::ZERO).await?;
    Err(failure)
}

pub async fn stop(process: &mut EngineProcess, grace: Duration, settle: Duration) -> Result<StopReport, EngineError> {
    let started = Instant::now();
    let mut forced = false;
    if process.exited().is_none() {
        signal(process.pid, TERM_SIGNAL);
        if tokio::time::timeout(grace, process.wait_exit()).await.is_err() {
            forced = true;
            signal(process.pid, KILL_SIGNAL);
        }
    }
    let exit = process.wait_exit().await;
    let leftover_pids = reap_profile_processes(&process.profile_dir, settle).await?;
    Ok(StopReport { exit, forced, stop_ms: started.elapsed().as_millis() as u64, leftover_pids })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ombra::OmbraEngine;
    use crate::engine::{Engine, EngineBuild};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn debug_port_probe_accepts_a_listening_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        assert!(debug_port_answers(listener.local_addr().unwrap().port()).await);
    }

    #[tokio::test]
    async fn debug_port_probe_rejects_an_unreachable_port() {
        // Порт 0 не может достаться соседнему тесту после закрытия listener.
        assert!(!debug_port_answers(0).await);
    }

    #[tokio::test]
    async fn browser_gets_its_own_process_group() {
        let cmd = LaunchCommand {
            program: "/bin/sleep".into(),
            args: vec!["5".into()],
            profile_dir: std::env::temp_dir(),
            log_path: std::env::temp_dir().join("unused.log"),
        };
        let mut child = command_for(&cmd).spawn().unwrap();
        let pid = child.id().unwrap() as libc::pid_t;
        let (child_group, own_group) = unsafe { (libc::getpgid(pid), libc::getpgrp()) };
        child.kill().await.unwrap();
        assert_ne!(child_group, own_group);
        assert_eq!(child_group, pid);
    }

    #[test]
    fn matches_profile_exactly_with_spaces() {
        let dir = Path::new("/Users/u/Library/Application Support/Ombra/profiles/abc");
        let own = "/X/Chromium --user-data-dir=/Users/u/Library/Application Support/Ombra/profiles/abc --type=renderer";
        let last = "/X/Chromium --user-data-dir=/Users/u/Library/Application Support/Ombra/profiles/abc";
        let neighbour = "/X/Chromium --user-data-dir=/Users/u/Library/Application Support/Ombra/profiles/abcd --type=gpu";
        assert!(matches_profile(own, dir));
        assert!(matches_profile(last, dir));
        assert!(!matches_profile(neighbour, dir));
        assert!(!matches_profile("/bin/zsh", dir));
    }

    #[test]
    fn parses_ps_and_port_file() {
        let rows = parse_ps("  101 /X/Chromium --a\n 102 sleep 30\nbad line\n");
        assert_eq!(rows, vec![(101, "/X/Chromium --a".to_string()), (102, "sleep 30".to_string())]);
        assert_eq!(parse_devtools_active_port("53111\n/devtools/browser/abc\n"), Some((53111, "/devtools/browser/abc".to_string())));
        assert_eq!(parse_devtools_active_port("53111\n"), None);
        assert_eq!(parse_devtools_active_port("x\n/devtools/browser/abc\n"), None);
    }

    /// Отвечает на любой запрос тем, что вернёт `answer(порт)`: строка статуса и тело JSON.
    async fn stub_server(answer: impl Fn(u16) -> (&'static str, String) + Send + Sync + 'static) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let answer = Arc::new(answer);
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let answer = answer.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = s.read(&mut buf).await;
                    let (status, body) = answer(port);
                    let _ = s
                        .write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
                        .await;
                });
            }
        });
        port
    }

    fn ready_body(port: u16) -> String {
        format!(r#"{{"Browser":"Chrome/150","webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/test"}}"#)
    }

    /// Отвечает на /json/version, как Chromium.
    async fn version_server() -> u16 {
        stub_server(|port| ("200 OK", ready_body(port))).await
    }

    const UNAVAILABLE: &str = "503 Service Unavailable";
    const NO_SLOT: &str = r#"{"ombra":{"license":"no_slot"}}"#;

    /// Браузер с патчем 980: `503` с телом `waiting`, пока не прошло `until` со старта сервера, затем `200`.
    async fn license_server(waiting: &'static str, until: Duration) -> u16 {
        let started = Instant::now();
        stub_server(move |port| {
            if started.elapsed() < until { (UNAVAILABLE, waiting.to_string()) } else { ("200 OK", ready_body(port)) }
        })
        .await
    }

    /// Хуки, которые записывают каждое состояние лицензии.
    fn recording_hooks() -> (LaunchHooks, Arc<Mutex<Vec<String>>>) {
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let record = seen.clone();
        let hooks = LaunchHooks { on_license: Arc::new(move |license| record.lock().unwrap().push(license.to_string())), ..LaunchHooks::none() };
        (hooks, seen)
    }

    /// «Браузер» — оболочка с флагом папки профиля, по которому его находит `find_profile_processes`. Файл порта
    /// отладки пишет сам тест, когда запуск уже убрал старый (журнал создаётся после удаления).
    async fn launch_shell(profile: &Path, port: u16, ready: Duration, hooks: LaunchHooks) -> Result<EngineProcess, EngineError> {
        launch_script(profile, "while :; do sleep 1; done", port, ready, hooks).await
    }

    /// То же со своим сценарием оболочки.
    async fn launch_script(
        profile: &Path,
        script: &str,
        port: u16,
        ready: Duration,
        hooks: LaunchHooks,
    ) -> Result<EngineProcess, EngineError> {
        let cmd = LaunchCommand {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                script.into(),
                "browser".into(),
                format!("--user-data-dir={}", profile.display()),
            ],
            profile_dir: profile.to_path_buf(),
            log_path: profile.join("chromium.log"),
        };
        let launching = tokio::spawn(launch(cmd, ready, hooks));
        while !profile.join("chromium.log").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        std::fs::write(profile.join("DevToolsActivePort"), format!("{port}\n/devtools/browser/test\n")).unwrap();
        launching.await.unwrap()
    }

    #[tokio::test]
    async fn probe_reads_license_state_from_503() {
        let waiting = stub_server(|_| (UNAVAILABLE, NO_SLOT.to_string())).await;
        assert_eq!(probe(waiting).await, Probe::License { state: "no_slot".into(), owner_hint: None });
        let ready = version_server().await;
        assert_eq!(probe(ready).await, Probe::Ready(format!("ws://127.0.0.1:{ready}/devtools/browser/test")));
        let unknown = stub_server(|_| (UNAVAILABLE, "{}".to_string())).await;
        assert_eq!(probe(unknown).await, Probe::NotYet);
    }

    #[tokio::test]
    async fn missing_or_rejected_access_ends_launch_and_reaps_the_process() {
        for (state, expected) in [("no_key", "BrowserAccessRequired"), ("invalid", "BrowserAccessInvalid"),
            ("device_bound", "FreeDeviceBound"), ("no_device", "DeviceIdentityUnavailable")] {
            let dir = tempfile::tempdir().unwrap();
            let port = stub_server(move |_| (UNAVAILABLE, format!(r#"{{"ombra":{{"license":"{state}"}}}}"#))).await;
            let (hooks, seen) = recording_hooks();
            let result = tokio::time::timeout(Duration::from_secs(5), launch_shell(dir.path(), port, Duration::from_secs(30), hooks))
                .await.expect("terminal access errors must not wait for a slot");
            let error = result.expect_err("the browser rejected access");
            assert_eq!(crate::app::CommandError::from(crate::supervisor::SupervisorError::Launch(error)).code, expected);
            assert!(seen.lock().unwrap().is_empty(), "terminal failures are not waiting states");
            assert!(find_profile_processes(dir.path()).await.unwrap().is_empty(), "failed launch must release its processes");
        }
    }

    #[tokio::test]
    async fn masked_owner_reaches_both_launch_error_payloads() {
        for supervisor in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let port = stub_server(|_| (UNAVAILABLE,
                r#"{"ombra":{"license":"device_bound","ownerHint":"iv***v@example.com"}}"#.into())).await;
            let (hooks, _) = recording_hooks();
            let error = launch_shell(dir.path(), port, Duration::from_secs(5), hooks).await.expect_err("device denial");
            let error = if supervisor { crate::app::CommandError::from(crate::supervisor::SupervisorError::Launch(error)) }
                else { crate::app::CommandError::from(error) };
            assert_eq!(serde_json::to_value(error).unwrap()["ownerHint"], "iv***v@example.com");
        }
    }

    #[tokio::test]
    async fn unsafe_owner_hints_do_not_hide_a_terminal_device_denial() {
        for hint in [serde_json::json!("ivan@example.com"), serde_json::json!(true), serde_json::json!("iv***v@example.com\n")] {
            let body = serde_json::json!({"ombra": {"license": "device_bound", "ownerHint": hint}}).to_string();
            let port = stub_server(move |_| (UNAVAILABLE, body.clone())).await;
            assert_eq!(probe(port).await, Probe::License { state: "device_bound".into(), owner_hint: None });
        }
    }

    #[test]
    fn device_denial_has_a_stable_error_in_all_launch_paths() {
        for (state, code) in [("device_bound", "FreeDeviceBound"), ("no_device", "DeviceIdentityUnavailable")] {
            assert_eq!(crate::app::CommandError::from(terminal_license_error(state, None).unwrap()).code, code);
            assert_eq!(crate::supervisor::SupervisorError::Launch(terminal_license_error(state, None).unwrap()).code(), code);
        }
        for state in ["ok", "offline", "no_slot", "unknown"] {
            assert!(terminal_license_error(state, None).is_none());
        }
    }

    #[tokio::test]
    async fn launch_waits_past_ready_timeout_while_license_waits() {
        let dir = tempfile::tempdir().unwrap();
        let port = license_server(NO_SLOT, Duration::from_secs(1)).await;
        let (hooks, seen) = recording_hooks();
        let mut process = launch_shell(&dir.path().join("p"), port, Duration::from_millis(300), hooks).await.unwrap();
        stop(&mut process, Duration::from_secs(2), Duration::ZERO).await.unwrap();
        assert_eq!(process.ws_endpoint, format!("ws://127.0.0.1:{port}/devtools/browser/test"));
        assert_eq!(*seen.lock().unwrap(), vec!["no_slot".to_string()], "одно состояние — один вызов");
    }

    #[tokio::test]
    async fn launch_cancel_kills_and_returns_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let port = stub_server(|_| (UNAVAILABLE, NO_SLOT.to_string())).await;
        let cancel = CancellationToken::new();
        let stop_on_wait = cancel.clone();
        let hooks = LaunchHooks { on_license: Arc::new(move |_| stop_on_wait.cancel()), cancel };
        let profile = dir.path().join("p");
        let err = launch_shell(&profile, port, Duration::from_secs(30), hooks).await.unwrap_err();
        let left = find_profile_processes(&profile).await.unwrap();
        assert!(matches!(err, EngineError::Cancelled), "{err}");
        assert!(left.is_empty(), "остались процессы: {left:?}");
    }

    #[tokio::test]
    async fn license_ok_window_stays_starting() {
        let dir = tempfile::tempdir().unwrap();
        let port = license_server(r#"{"ombra":{"license":"ok"}}"#, Duration::from_millis(300)).await;
        let (hooks, seen) = recording_hooks();
        let mut process = launch_shell(&dir.path().join("p"), port, Duration::from_secs(5), hooks).await.unwrap();
        stop(&mut process, Duration::from_secs(2), Duration::ZERO).await.unwrap();
        assert!(seen.lock().unwrap().is_empty(), "«ok» — профиль ещё создаётся: {:?}", seen.lock().unwrap());
    }

    const OFFLINE: &str = r#"{"ombra":{"license":"offline"}}"#;

    #[tokio::test]
    async fn license_offline_window_stays_starting() {
        let dir = tempfile::tempdir().unwrap();
        // До первого ответа сервера лицензий браузер говорит `offline`.
        let port = license_server(OFFLINE, Duration::from_millis(300)).await;
        let (hooks, seen) = recording_hooks();
        let mut process = launch_shell(&dir.path().join("p"), port, Duration::from_secs(5), hooks).await.unwrap();
        stop(&mut process, Duration::from_secs(2), Duration::ZERO).await.unwrap();
        assert!(seen.lock().unwrap().is_empty(), "стартовое «offline» — не ожидание: {:?}", seen.lock().unwrap());
    }

    #[tokio::test]
    async fn offline_past_grace_waits_for_a_thread() {
        let dir = tempfile::tempdir().unwrap();
        let port = stub_server(|_| (UNAVAILABLE, OFFLINE.to_string())).await;
        let (mut hooks, seen) = recording_hooks();
        let record = hooks.on_license.clone();
        let stop_on_wait = hooks.cancel.clone();
        hooks.on_license = Arc::new(move |license| {
            record(license);
            stop_on_wait.cancel();
        });
        let started = Instant::now();
        let err = launch_shell(&dir.path().join("p"), port, Duration::from_secs(10), hooks).await.unwrap_err();
        assert!(matches!(err, EngineError::Cancelled), "{err}");
        assert_eq!(*seen.lock().unwrap(), vec!["offline".to_string()]);
        assert!(started.elapsed() >= Duration::from_secs(3), "ожидание — только после окна: {:?}", started.elapsed());
    }

    #[tokio::test]
    async fn endless_ok_window_hits_ready_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let port = stub_server(|_| (UNAVAILABLE, r#"{"ombra":{"license":"ok"}}"#.to_string())).await;
        let (hooks, seen) = recording_hooks();
        let err = launch_shell(&dir.path().join("p"), port, Duration::from_millis(500), hooks).await.unwrap_err();
        assert!(matches!(err, EngineError::ReadyTimeout), "{err}");
        assert!(seen.lock().unwrap().is_empty());
    }

    /// Принимает соединения и не отвечает: зависший сервер DevTools.
    async fn stalled_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    drop(s);
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn cancel_does_not_wait_for_a_stalled_probe() {
        let dir = tempfile::tempdir().unwrap();
        let port = stalled_server().await;
        let cancel = CancellationToken::new();
        let hooks = LaunchHooks { cancel: cancel.clone(), ..LaunchHooks::none() };
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            cancel.cancel();
        });
        let started = Instant::now();
        let err = launch_shell(&dir.path().join("p"), port, Duration::from_secs(30), hooks).await.unwrap_err();
        assert!(matches!(err, EngineError::Cancelled), "{err}");
        assert!(started.elapsed() < Duration::from_secs(2), "«Стоп» ждал ответа: {:?}", started.elapsed());
    }

    #[tokio::test]
    async fn closing_the_waiting_browser_is_a_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let port = stub_server(|_| (UNAVAILABLE, NO_SLOT.to_string())).await;
        let (hooks, seen) = recording_hooks();
        // Пользователь закрыл окно ожидания потока (⌘Q): браузер выходит с кодом 0.
        let err = launch_script(&dir.path().join("p"), "sleep 1; exit 0", port, Duration::from_secs(30), hooks).await.unwrap_err();
        assert!(matches!(err, EngineError::Cancelled), "{err}");
        assert_eq!(*seen.lock().unwrap(), vec!["no_slot".to_string()]);
    }

    /// Поддельный «Chromium»: сценарий оболочки, получающий те же аргументы.
    fn fake_engine(dir: &Path, body: &str) -> OmbraEngine {
        let script = dir.join("fake-chromium.sh");
        let text = format!(
            "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in --user-data-dir=*) DIR=\"${{a#--user-data-dir=}}\";; esac; done\n{body}\n"
        );
        std::fs::write(&script, text).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        OmbraEngine::new(dir.join("engines"))
            .with_build(EngineBuild { version: "150.0.7871.114".into(), binary: script })
            .with_timeouts(Duration::from_secs(5), Duration::from_millis(500))
    }

    fn command(engine: &OmbraEngine, profile: &Path) -> LaunchCommand {
        let mut req = crate::engine::ombra::flags::tests::request();
        req.profile_dir = profile.to_path_buf();
        req.log_path = profile.join("chromium.log");
        engine.build_command(&req).unwrap()
    }

    #[tokio::test]
    async fn early_exit_is_reported_with_code() {
        let dir = tempfile::tempdir().unwrap();
        let engine = fake_engine(dir.path(), "exit 76");
        let started = Instant::now();
        let err = engine.launch(command(&engine, &dir.path().join("p 1")), LaunchHooks::none()).await.unwrap_err();
        assert!(matches!(err, EngineError::ExitedEarly(ExitInfo { code: Some(76), .. })), "{err}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn launch_waits_for_cdp_then_stop_reaps_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let port = version_server().await;
        let body = format!(
            "/bin/sh -c 'trap \"\" TERM; while :; do sleep 1; done' helper \"--user-data-dir=$DIR\" &\n\
             printf '{port}\\n/devtools/browser/test\\n' > \"$DIR/DevToolsActivePort\"\nexec sleep 30"
        );
        let engine = fake_engine(dir.path(), &body);
        let profile = dir.path().join("profile dir");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("DevToolsActivePort"), "1\n/devtools/browser/stale\n").unwrap();

        let mut process = engine.launch(command(&engine, &profile), LaunchHooks::none()).await.unwrap();
        assert_eq!(process.debug_port, port);
        assert_eq!(process.ws_endpoint, format!("ws://127.0.0.1:{port}/devtools/browser/test"));
        assert!(process.exited().is_none());
        let prefs = std::fs::read_to_string(profile.join("Default/Preferences")).unwrap();
        assert!(prefs.contains("disable_non_proxied_udp"));
        assert!(!find_profile_processes(&profile).await.unwrap().is_empty(), "помощник должен быть виден по папке");

        let report = engine.stop(&mut process, Duration::from_secs(2)).await.unwrap();
        assert_eq!(report.exit.signal, Some(TERM_SIGNAL));
        assert!(!report.forced);
        assert_eq!(report.leftover_pids.len(), 1);
        assert!(find_profile_processes(&profile).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stop_forces_kill_when_term_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let port = version_server().await;
        let body = format!(
            "trap '' TERM\nprintf '{port}\\n/devtools/browser/test\\n' > \"$DIR/DevToolsActivePort\"\nwhile :; do sleep 1; done"
        );
        let engine = fake_engine(dir.path(), &body);
        let mut process = engine.launch(command(&engine, &dir.path().join("p")), LaunchHooks::none()).await.unwrap();
        let report = engine.stop(&mut process, Duration::from_millis(300)).await.unwrap();
        assert!(report.forced);
        assert_eq!(report.exit.signal, Some(KILL_SIGNAL));
    }

    #[tokio::test]
    async fn crash_after_ready_is_visible_to_waiters() {
        let dir = tempfile::tempdir().unwrap();
        let port = version_server().await;
        let body = format!("printf '{port}\\n/devtools/browser/test\\n' > \"$DIR/DevToolsActivePort\"\nsleep 1\nexit 3");
        let engine = fake_engine(dir.path(), &body);
        let process = engine.launch(command(&engine, &dir.path().join("p")), LaunchHooks::none()).await.unwrap();
        let mut watcher = process.clone();
        assert_eq!(watcher.wait_exit().await, ExitInfo { code: Some(3), signal: None });
        assert_eq!(process.exited(), Some(ExitInfo { code: Some(3), signal: None }));
    }

    #[tokio::test]
    async fn not_ready_in_time_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let engine = fake_engine(dir.path(), "exec sleep 30").with_timeouts(Duration::from_millis(500), Duration::from_millis(200));
        let err = engine.launch(command(&engine, &dir.path().join("p")), LaunchHooks::none()).await.unwrap_err();
        assert!(matches!(err, EngineError::ReadyTimeout), "{err}");
    }

    const HELPER: &str = "/bin/sh -c 'trap \"\" TERM; while :; do sleep 1; done' helper \"--user-data-dir=$DIR\" &\n";

    #[tokio::test]
    async fn ready_timeout_reaps_profile_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let engine = fake_engine(dir.path(), &format!("{HELPER}exec sleep 30"))
            .with_timeouts(Duration::from_millis(500), Duration::from_millis(200));
        let profile = dir.path().join("p");
        let err = engine.launch(command(&engine, &profile), LaunchHooks::none()).await.unwrap_err();
        let left = find_profile_processes(&profile).await.unwrap();
        let _ = reap_profile_processes(&profile, Duration::ZERO).await;
        assert!(matches!(err, EngineError::ReadyTimeout), "{err}");
        assert!(left.is_empty(), "остались процессы: {left:?}");
    }

    #[tokio::test]
    async fn early_exit_reaps_profile_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let engine = fake_engine(dir.path(), &format!("{HELPER}exit 76"));
        let profile = dir.path().join("p");
        let err = engine.launch(command(&engine, &profile), LaunchHooks::none()).await.unwrap_err();
        let left = find_profile_processes(&profile).await.unwrap();
        let _ = reap_profile_processes(&profile, Duration::ZERO).await;
        assert!(matches!(err, EngineError::ExitedEarly(ExitInfo { code: Some(76), .. })), "{err}");
        assert!(left.is_empty(), "остались процессы: {left:?}");
    }

    #[tokio::test]
    async fn cancelled_launch_kills_browser() {
        let dir = tempfile::tempdir().unwrap();
        let engine = fake_engine(dir.path(), "while :; do sleep 1; done");
        let profile = dir.path().join("p");
        let cmd = command(&engine, &profile);
        let res = tokio::time::timeout(Duration::from_millis(300), engine.launch(cmd, LaunchHooks::none())).await;
        assert!(res.is_err(), "запуск должен был быть прерван");
        tokio::time::sleep(Duration::from_millis(500)).await;
        let left = find_profile_processes(&profile).await.unwrap();
        let _ = reap_profile_processes(&profile, Duration::ZERO).await;
        assert!(left.is_empty(), "остались процессы: {left:?}");
    }

    #[tokio::test]
    async fn build_command_checks_install_and_args() {
        let dir = tempfile::tempdir().unwrap();
        let missing = OmbraEngine::new(dir.path().join("engines"));
        let req = crate::engine::ombra::flags::tests::request();
        assert!(matches!(missing.build_command(&req), Err(EngineError::NotInstalled)));
        let engine = fake_engine(dir.path(), "exit 0");
        let mut bad = req.clone();
        bad.extra_args = vec!["--proxy-server=direct://".into()];
        assert!(matches!(engine.build_command(&bad), Err(EngineError::Args(_))));
        assert!(engine.build_command(&req).unwrap().args.contains(&"--use-mock-keychain".to_string()));
    }

    #[tokio::test]
    async fn terminate_asks_politely_then_kills() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("orphan profile");
        let flag = format!("--user-data-dir={}", profile.display());
        let mut polite = std::process::Command::new("/bin/sh")
            .args(["-c", "while :; do sleep 1; done", "polite", &flag])
            .spawn()
            .unwrap();
        let mut stubborn = std::process::Command::new("/bin/sh")
            .args(["-c", "trap '' TERM; while :; do sleep 1; done", "stubborn", &flag])
            .spawn()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let killed = terminate_profile_processes(&profile, Duration::from_millis(1500)).await.unwrap();
        let left = find_profile_processes(&profile).await.unwrap();
        let _ = polite.wait();
        let _ = stubborn.wait();
        assert_eq!(killed, vec![stubborn.id()], "SIGKILL нужен только тому, кто игнорирует SIGTERM");
        assert!(left.is_empty(), "остались процессы: {left:?}");
        assert!(terminate_profile_processes(&profile, Duration::from_millis(100)).await.unwrap().is_empty());
    }
}
