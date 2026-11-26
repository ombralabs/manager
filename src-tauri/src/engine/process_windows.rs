use std::{io, path::Path};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM},
    System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE},
    UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE},
};
use windows_sys::core::BOOL;

/// Query the command lines as JSON so spaces in profile paths retain their meaning.
pub async fn find_profile_processes(profile: &Path) -> io::Result<Vec<u32>> {
    let mut command = tokio::process::Command::new("powershell.exe");
    command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command",
        "$ErrorActionPreference = 'Stop'; [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(); @(Get-CimInstance Win32_Process -Filter \"Name = 'chrome.exe'\" | Select-Object ProcessId,CommandLine) | ConvertTo-Json -Compress"])
        .creation_flags(0x08000000).kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output()).await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "process enumeration timed out"))??;
    if !output.status.success() { return Err(io::Error::other("process enumeration failed")); }
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() { return Ok(vec![]); }
    let json: serde_json::Value = serde_json::from_str(text.trim()).map_err(io::Error::other)?;
    let rows = if let Some(rows) = json.as_array() { rows.clone() } else { vec![json] };
    let needle = format!("--user-data-dir={}", profile.display());
    Ok(rows.iter().filter_map(|row| {
        let pid = row["ProcessId"].as_u64()?.try_into().ok()?;
        let command = row["CommandLine"].as_str()?;
        // Chromium quotes the whole argument when the path contains spaces.
        let quoted = format!("\"{needle}\"");
        let quoted_value = format!("--user-data-dir=\"{}\"", profile.display());
        let matches = command.contains(&quoted) || command.contains(&quoted_value)
            || super::matches_profile(command, profile);
        (matches && pid != std::process::id()).then_some(pid)
    }).collect())
}

pub(super) fn signal(pid: u32, signal: i32) {
    unsafe {
        if signal == super::TERM_SIGNAL {
            unsafe extern "system" fn close_window(window: HWND, pid: LPARAM) -> BOOL {
                let mut owner = 0;
                unsafe {
                    GetWindowThreadProcessId(window, &mut owner);
                    if owner == pid as u32 { PostMessageW(window, WM_CLOSE, 0, 0); }
                }
                1
            }
            EnumWindows(Some(close_window), pid as LPARAM);
        } else {
            let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !process.is_null() { TerminateProcess(process, 1); CloseHandle(process); }
        }
    }
}
