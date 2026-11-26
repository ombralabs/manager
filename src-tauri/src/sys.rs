//! Системные настройки процесса приложения.
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

/// Секунды UNIX для отметок времени: ядро приложения, супервизор, проверка прокси, массовые операции.
pub(crate) fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Жив ли процесс: `kill(pid, 0)` проверяет его, ничего не посылая. `EPERM` — процесс есть, но чужой.
pub(crate) fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::{OpenProcess, GetExitCodeProcess, PROCESS_QUERY_LIMITED_INFORMATION}};
        // SAFETY: handle is checked and closed; the API writes to a valid local value.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() { return io::Error::last_os_error().raw_os_error() == Some(5); }
            let mut code = 0;
            let ok = GetExitCodeProcess(handle, &mut code) != 0;
            CloseHandle(handle);
            return ok && code == 259; // STILL_ACTIVE
        }
    }
    #[cfg(unix)]
    {
    // SAFETY: сигнал 0 только проверяет существование процесса.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

/// Сигнал игнорируется (`SIG_IGN`) — так его оставляет, например, `nohup` для SIGHUP. Только чтение, ничего не меняет.
#[cfg(unix)]
pub(crate) fn signal_ignored(signal: libc::c_int) -> bool {
    // SAFETY: новое действие — NULL (только запрос), старое пишется в структуру на стеке.
    let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
    unsafe { libc::sigaction(signal, std::ptr::null(), &mut old) == 0 && old.sa_sigaction == libc::SIG_IGN }
}

/// Предел macOS для `RLIMIT_NOFILE` (`OPEN_MAX`): выше `setrlimit` отвечает EINVAL.
#[cfg(target_os = "macos")]
const OPEN_MAX: u64 = 10_240;
#[cfg(all(unix, not(target_os = "macos")))]
const OPEN_MAX: u64 = u64::MAX;

/// Поднимает мягкий лимит открытых файлов до `want`, но не выше жёсткого лимита и `OPEN_MAX`.
/// У приложений из Finder мягкий лимит 256: на нескольких профилях прослойка упёрлась бы в `EMFILE`.
#[cfg(unix)]
pub fn raise_fd_limit(want: u64) -> io::Result<u64> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: `limit` — валидная структура на стеке.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let target = want.min(limit.rlim_max).min(OPEN_MAX);
    if limit.rlim_cur >= target {
        return Ok(limit.rlim_cur);
    }
    limit.rlim_cur = target;
    // SAFETY: то же, структура заполнена значениями из `getrlimit`.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(target)
}

#[cfg(windows)]
pub fn raise_fd_limit(want: u64) -> io::Result<u64> { Ok(want) }

/// Unix permissions are set at creation; Windows inherits the user's profile directory ACL.
pub(crate) fn private_options() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    options
}

pub(crate) fn local_time(time: libc::time_t) -> libc::tm {
    let mut value = unsafe { std::mem::zeroed() };
    #[cfg(unix)]
    unsafe { libc::localtime_r(&time, &mut value); }
    #[cfg(windows)]
    unsafe { libc::localtime_s(&mut value, &time); }
    value
}

/// Normalize a local calendar date using the platform C runtime (including DST transitions).
pub(crate) fn make_local_time(value: &mut libc::tm) -> libc::time_t {
    #[cfg(unix)]
    unsafe { libc::mktime(value) }
    #[cfg(windows)]
    {
        unsafe extern "C" {
            fn _mktime64(value: *mut libc::tm) -> i64;
        }
        // Windows x64's CRT exports the 64-bit function; libc does not expose mktime on Windows.
        unsafe { _mktime64(value) }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn raises_soft_limit_within_hard_limit() {
        let got = raise_fd_limit(1024).unwrap();
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) }, 0);
        assert_eq!(limit.rlim_cur, got);
        assert!(got >= 1024.min(limit.rlim_max), "{got}");
    }

    #[test]
    fn ignored_signal_is_seen() {
        // SIGUSR2 в Ombra не используется: состояние сигнала общее для процесса тестов.
        assert!(!signal_ignored(libc::SIGUSR2));
        unsafe { libc::signal(libc::SIGUSR2, libc::SIG_IGN) };
        assert!(signal_ignored(libc::SIGUSR2));
        unsafe { libc::signal(libc::SIGUSR2, libc::SIG_DFL) };
        assert!(!signal_ignored(libc::SIGUSR2));
    }

    #[test]
    fn own_process_is_alive_and_a_finished_one_is_not() {
        assert!(pid_alive(std::process::id()));
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!pid_alive(pid));
    }
}
