use crate::engine::LaunchRequest;
use crate::fingerprint::{Location, Os};

/// Высота окна: экран персоны минус панель macOS, как считает стенд QA движка (900 → 805).
const MAC_TASKBAR: u32 = 95;

/// Команда запуска по контракту движка: таблица `MANAGER_FLAGS` в `qa/ombra_qa/personas.py` и базовые ключи
/// `launcher.py`. `major` — первое число версии сборки, оно задаёт TLS-персону.
pub fn build_args(req: &LaunchRequest, major: u32) -> Vec<String> {
    let fp = &req.resolved.fingerprint;
    let platform = match fp.os {
        Os::Macos => "macos",
        Os::Windows => "windows",
    };
    let mut args = vec![
        format!("--user-data-dir={}", req.profile_dir.display()),
        "--remote-debugging-port=0".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--use-mock-keychain".to_string(),
        format!("--proxy-server=http://{}", req.relay_addr),
        format!("--fingerprint={}", fp.seed),
        format!("--fingerprint-platform={platform}"),
        format!("--fingerprint-platform-version={}", fp.os_version),
        "--fingerprint-brand=chrome".to_string(),
        format!("--fingerprint-gpu-vendor={}", fp.gpu.vendor),
        format!("--fingerprint-gpu-renderer={}", fp.gpu.renderer),
        format!("--fingerprint-hardware-concurrency={}", fp.hardware_concurrency),
        format!("--fingerprint-device-memory={}", fp.device_memory),
        format!("--fingerprint-screen-width={}", fp.screen.width),
        format!("--fingerprint-screen-height={}", fp.screen.height),
        format!("--fingerprint-device-pixel-ratio={}", fp.device_pixel_ratio),
        format!("--fingerprint-max-touch-points={}", fp.max_touch_points),
        format!("--fingerprint-tls-profile=chrome-{major}"),
        format!("--fingerprint-timezone={}", req.resolved.timezone),
        format!("--accept-lang={}", accept_lang(&req.resolved.language)),
        format!("--profile-label={}", req.profile_name.chars().filter(|c| !c.is_control()).collect::<String>()),
    ];
    if let Location::Manual { lat, lon } = fp.location {
        args.push(format!("--fingerprint-location={lat},{lon}"));
    }
    if let Some(ip) = &req.resolved.webrtc_ip {
        args.push(format!("--webrtc-ip={ip}"));
    }
    if let Some(quota) = fp.storage_quota_mb {
        args.push(format!("--fingerprint-storage-quota={quota}"));
    }
    if !fp.noise {
        args.push("--disable-fingerprint-noise".to_string());
    }
    if req.headless {
        args.push("--headless".to_string());
    }
    // Очеловечивание ввода (`HUMANIZE_FLAGS` в `ombra-engine/qa/ombra_qa/personas.py`): у выключенного флагов нет.
    if let Some(h) = &req.humanize {
        args.push("--fingerprint-humanize=on".to_string());
        args.push(format!("--fingerprint-humanize-seed={}", h.seed));
        args.push(format!("--fingerprint-humanize-preset={}", h.settings.preset.as_str()));
        if !h.settings.block_input {
            args.push("--fingerprint-humanize-block-input=off".to_string());
        }
        if !h.settings.paste_key.is_empty() {
            args.push(format!("--fingerprint-humanize-paste-key={}", h.settings.paste_key));
        }
        if let Some(log) = &h.debug_log {
            args.push(format!("--fingerprint-humanize-debug={}", log.display()));
        }
    }
    if req.restore_tabs {
        args.push("--restore-last-session".to_string());
    }
    args.push(format!("--window-size={},{}", fp.screen.width, window_height(fp.screen.height)));
    args.push("--window-position=0,0".to_string());
    // Папки набора и `--load-extension` пользователя (строка запуска, `launch_args`) — одним флагом:
    // Chromium учитывает только последнее вхождение ключа.
    let mut extensions: Vec<String> = req.extensions.iter().map(|dir| dir.display().to_string()).collect();
    for arg in &req.extra_args {
        match arg.strip_prefix("--load-extension=") {
            Some(list) => extensions.extend(list.split(',').filter(|dir| !dir.is_empty()).map(str::to_string)),
            None => args.push(arg.clone()),
        }
    }
    if !extensions.is_empty() {
        args.push(format!("--load-extension={}", extensions.join(",")));
    }
    args
}

fn window_height(screen_height: u32) -> u32 {
    screen_height.saturating_sub(MAC_TASKBAR).max(600)
}

/// `de-DE` → `de-DE,de`: так заголовок `Accept-Language` выглядит у настоящего Chrome.
pub fn accept_lang(language: &str) -> String {
    match language.split_once('-') {
        Some((primary, _)) => format!("{language},{primary}"),
        None => language.to_string(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fingerprint::{Fingerprint, Resolved};
    use std::path::PathBuf;

    const FP: &str = r#"{"seed":48213,"os":"macos","os_version":"15.2.0","brand":"Chrome",
      "gpu":{"vendor":"Google Inc. (Apple)","renderer":"ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)"},
      "hardware_concurrency":8,"device_memory":16,"screen":{"width":1440,"height":900},
      "timezone":{"mode":"auto"},"language":{"mode":"auto"},"webrtc":{"mode":"auto"},
      "noise":false,"storage_quota_mb":5000,"manual":[]}"#;

    pub(crate) fn request() -> LaunchRequest {
        LaunchRequest {
            profile_name: "Профиль 3".into(),
            profile_dir: PathBuf::from("/Users/u/Library/Application Support/Ombra/profiles/k7f2q9"),
            log_path: PathBuf::from("/tmp/chromium.log"),
            resolved: Resolved {
                fingerprint: Fingerprint::from_json(FP).unwrap(),
                timezone: "Europe/Berlin".into(),
                language: "de-DE".into(),
                webrtc_ip: Some("203.0.113.24".into()),
            },
            relay_addr: "127.0.0.1:53111".parse().unwrap(),
            headless: false,
            restore_tabs: false,
            extra_args: vec!["--disable-sync".into()],
            extensions: vec![],
            humanize: None,
        }
    }

    #[test]
    fn args_for_150() {
        assert_eq!(
            build_args(&request(), 150),
            [
                "--user-data-dir=/Users/u/Library/Application Support/Ombra/profiles/k7f2q9",
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
                "--use-mock-keychain",
                "--proxy-server=http://127.0.0.1:53111",
                "--fingerprint=48213",
                "--fingerprint-platform=macos",
                "--fingerprint-platform-version=15.2.0",
                "--fingerprint-brand=chrome",
                "--fingerprint-gpu-vendor=Google Inc. (Apple)",
                "--fingerprint-gpu-renderer=ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)",
                "--fingerprint-hardware-concurrency=8",
                "--fingerprint-device-memory=16",
                "--fingerprint-screen-width=1440",
                "--fingerprint-screen-height=900",
                "--fingerprint-device-pixel-ratio=2",
                "--fingerprint-max-touch-points=0",
                "--fingerprint-tls-profile=chrome-150",
                "--fingerprint-timezone=Europe/Berlin",
                "--accept-lang=de-DE,de",
                "--profile-label=Профиль 3",
                "--webrtc-ip=203.0.113.24",
                "--fingerprint-storage-quota=5000",
                "--disable-fingerprint-noise",
                "--window-size=1440,805",
                "--window-position=0,0",
                "--disable-sync",
            ]
        );
    }

    /// Очеловечивание ввода — флагами движка (D25): seed и пресет профиля, блокировка — только выключенная, горячая
    /// клавиша — только заданная, журнал — только для стенда; выключенное — ни одного флага `--fingerprint-humanize*`.
    #[test]
    fn humanize_flags_follow_the_profile() {
        use crate::engine::HumanizeLaunch;
        use crate::humanize::{Humanize, Preset};
        let mut req = request();
        assert!(!build_args(&req, 150).iter().any(|a| a.starts_with("--fingerprint-humanize")), "выключено — без флагов");
        req.humanize = Some(HumanizeLaunch { seed: u64::MAX, settings: Humanize::default(), debug_log: None });
        let on: Vec<String> = build_args(&req, 150).into_iter().filter(|a| a.starts_with("--fingerprint-humanize")).collect();
        assert_eq!(on, ["--fingerprint-humanize=on", "--fingerprint-humanize-seed=18446744073709551615",
                        "--fingerprint-humanize-preset=default"]);
        req.humanize = Some(HumanizeLaunch {
            seed: 7,
            settings: Humanize { enabled: true, preset: Preset::Careful, block_input: false, paste_key: "Ctrl+Alt+V".into() },
            debug_log: Some(PathBuf::from("/tmp/run/k7f2q9.jsonl")),
        });
        let args = build_args(&req, 150);
        let on: Vec<&str> = args.iter().map(String::as_str).filter(|a| a.starts_with("--fingerprint-humanize")).collect();
        assert_eq!(on, ["--fingerprint-humanize=on", "--fingerprint-humanize-seed=7", "--fingerprint-humanize-preset=careful",
                        "--fingerprint-humanize-block-input=off", "--fingerprint-humanize-paste-key=Ctrl+Alt+V",
                        "--fingerprint-humanize-debug=/tmp/run/k7f2q9.jsonl"]);
        assert_eq!(args.last().map(String::as_str), Some("--disable-sync"), "аргументы профиля — последними");
    }

    #[test]
    fn optional_flags_follow_the_request() {
        let mut req = request();
        req.resolved.fingerprint.noise = true;
        req.resolved.fingerprint.storage_quota_mb = None;
        req.resolved.fingerprint.device_memory = 32;
        req.resolved.webrtc_ip = None;
        req.headless = true;
        req.restore_tabs = true;
        let args = build_args(&req, 150);
        for absent in ["--disable-fingerprint-noise", "--fingerprint-storage-quota=5000", "--webrtc-ip=203.0.113.24"] {
            assert!(!args.iter().any(|a| a == absent), "лишний {absent}");
        }
        assert!(args.contains(&"--fingerprint-device-memory=32".to_string()), "память уходит как есть");
        assert!(args.contains(&"--headless".to_string()));
        let restore = args.iter().position(|a| a == "--restore-last-session").expect("нет флага восстановления вкладок");
        assert_eq!(args.last().map(String::as_str), Some("--disable-sync"), "дополнительные аргументы — последними");
        assert!(restore < args.len() - 1);
        assert!(!args.iter().any(|a| a.contains("://example")), "стартовые вкладки открывает супервизор через CDP");
    }

    /// Имя профиля проверяется только на длину: управляющие символы (`\0` ломает `spawn`) во флаг не идут.
    #[test]
    fn profile_label_drops_control_characters() {
        let mut req = request();
        req.profile_name = "Профиль\0 3\n".into();
        assert!(build_args(&req, 150).contains(&"--profile-label=Профиль 3".to_string()));
    }

    #[test]
    fn windows_persona_gets_its_platform_and_dpr() {
        let mut req = request();
        req.resolved.fingerprint.os = Os::Windows;
        req.resolved.fingerprint.device_pixel_ratio = 1;
        let args = build_args(&req, 150);
        assert!(args.contains(&"--fingerprint-platform=windows".to_string()));
        assert!(args.contains(&"--fingerprint-device-pixel-ratio=1".to_string()));
    }

    #[test]
    fn dpr_touch_and_location_come_from_the_model() {
        let mut req = request();
        req.resolved.fingerprint.device_pixel_ratio = 1;
        req.resolved.fingerprint.max_touch_points = 5;
        req.resolved.fingerprint.location = Location::Manual { lat: 52.52, lon: 13.405 };
        let args = build_args(&req, 150);
        for flag in ["--fingerprint-device-pixel-ratio=1", "--fingerprint-max-touch-points=5", "--fingerprint-location=52.52,13.405"] {
            assert!(args.contains(&flag.to_string()), "нет {flag}");
        }
        assert!(!build_args(&request(), 150).iter().any(|a| a.starts_with("--fingerprint-location")), "без ручного режима флага нет");
    }

    #[test]
    fn helpers() {
        assert_eq!(accept_lang("de-DE"), "de-DE,de");
        assert_eq!(accept_lang("en"), "en");
        assert_eq!(accept_lang("zh-Hant-TW"), "zh-Hant-TW,zh");
        let mut req = request();
        req.resolved.fingerprint.screen.height = 640;
        assert!(build_args(&req, 150).contains(&"--window-size=1440,600".to_string()));
        assert!(build_args(&request(), 155).contains(&"--fingerprint-tls-profile=chrome-155".to_string()));
    }

    #[test]
    fn set_extensions_and_user_load_extension_go_in_one_flag() {
        let mut req = request();
        req.extensions = vec![PathBuf::from("/lib/a/1.0"), PathBuf::from("/lib/b/2")];
        req.extra_args = vec!["--disable-sync".into(), "--load-extension=/mine/x,/mine/y".into(), "--load-extension=".into()];
        let args = build_args(&req, 150);
        let loads: Vec<&String> = args.iter().filter(|a| a.starts_with("--load-extension")).collect();
        assert_eq!(loads, vec!["--load-extension=/lib/a/1.0,/lib/b/2,/mine/x,/mine/y"]);
        assert!(args.contains(&"--disable-sync".to_string()));
        let mut user_only = request();
        user_only.extra_args = vec!["--load-extension=/mine/x".into()];
        assert!(build_args(&user_only, 150).contains(&"--load-extension=/mine/x".to_string()));
        assert!(!build_args(&request(), 150).iter().any(|a| a.starts_with("--load-extension")));
    }
}
