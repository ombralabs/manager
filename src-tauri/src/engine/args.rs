/// Аргументы, которые позволили бы пустить трафик мимо прослойки, перехватить отладку, сломать персону движка
/// или увести содержимое canvas и страниц на внешние адреса (`--canvas-bridge-*`, `--agent-llm-*`).
const FORBIDDEN: &[&str] = &[
    "--proxy",
    "--no-proxy-server",
    "--user-data-dir",
    "--profile-directory",
    "--disk-cache-dir",
    "--remote-debugging",
    "--fingerprint",
    "--use-mock-keychain",
    "--timezone",
    "--webrtc-ip",
    "--accept-lang",
    "--disable-fingerprint-noise",
    "--disable-canvas-noise",
    "--disable-gpu-fingerprint",
    "--disable-gpu-string-spoof",
    "--cc-no-farble",
    "--canvas-bridge",
    "--agent-llm",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgsError {
    #[error("аргумент запрещён: {0}")]
    Forbidden(String),
    #[error("дополнительный аргумент должен начинаться с --: {0}")]
    NotAFlag(String),
    #[error("стартовая вкладка не может быть пустой или начинаться с «-»: {0}")]
    BadStartUrl(String),
}

/// Chromium понимает и `-switch`, поэтому принимаются только `--…`; сравнение без учёта регистра.
pub fn check_extra_args(args: &[String]) -> Result<(), ArgsError> {
    for arg in args {
        if !arg.starts_with("--") || arg.len() < 3 {
            return Err(ArgsError::NotAFlag(arg.clone()));
        }
        let lower = arg.to_ascii_lowercase();
        if FORBIDDEN.iter().any(|prefix| lower.starts_with(prefix)) {
            return Err(ArgsError::Forbidden(arg.clone()));
        }
    }
    Ok(())
}

/// Стартовая вкладка, начинающаяся с «-» (после срезания пробелов, как это делает Chromium), стала бы ключом Chromium (`--proxy-server=…`) и увела бы трафик мимо прослойки.
pub fn check_start_urls(urls: &[String]) -> Result<(), ArgsError> {
    for url in urls {
        // Chromium перед разбором обрезает ASCII-пробелы, включая \x0B, которого нет в `is_ascii_whitespace`.
        let trimmed = url.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r'));
        if trimmed.is_empty() || trimmed.starts_with('-') {
            return Err(ArgsError::BadStartUrl(url.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(args: &[&str]) -> Result<(), ArgsError> {
        check_extra_args(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn harmless_args_pass() {
        assert_eq!(check(&["--disable-sync", "--lang=de", "--headless"]), Ok(()));
        assert_eq!(check(&[]), Ok(()));
    }

    #[test]
    fn forbidden_args_in_any_form_are_rejected() {
        for bad in [
            "--proxy-server=socks5://1.2.3.4:1080",
            "--Proxy-Server=x",
            "--proxy-bypass-list=*",
            "--proxy-pac-url=http://x/p.pac",
            "--no-proxy-server",
            "--user-data-dir=/tmp/x",
            "--profile-directory=../../outside",
            "--disk-cache-dir=/tmp/plaintext-cache",
            "--remote-debugging-port=9222",
            "--remote-debugging-address=0.0.0.0",
            "--fingerprint=1",
            "--fingerprint-noise=false",
            "--use-mock-keychain",
        ] {
            assert!(matches!(check(&[bad]), Err(ArgsError::Forbidden(_))), "{bad}");
        }
        for not_flag in ["-proxy-server=x", "https://example.com", "proxy-server", ""] {
            assert!(matches!(check(&[not_flag]), Err(ArgsError::NotAFlag(_))), "{not_flag}");
        }
    }

    #[test]
    fn start_urls_cannot_be_switches() {
        let ok = vec!["https://example.com/".to_string(), "about:blank".to_string()];
        assert_eq!(check_start_urls(&ok), Ok(()));
        for bad in [
            "--proxy-server=direct://",
            "-proxy-server=x",
            "",
            " --proxy-server=direct://",
            "\t--proxy-server=x",
            "\n--proxy-server=x",
            "\u{0B}--proxy-server=x",
            "   ",
        ] {
            assert!(matches!(check_start_urls(&[bad.to_string()]), Err(ArgsError::BadStartUrl(_))), "{bad:?}");
        }
    }

    #[test]
    fn engine_persona_and_exfiltration_args_are_forbidden() {
        for arg in [
            "--timezone=UTC",
            "--webrtc-ip=203.0.113.1",
            "--accept-lang=en-US",
            "--Disable-Fingerprint-Noise",
            "--disable-canvas-noise",
            "--disable-gpu-fingerprint",
            "--disable-gpu-string-spoof",
            "--cc-no-farble",
            "--canvas-bridge-url=ws://127.0.0.1:9/",
            "--agent-llm-key=abc",
        ] {
            assert_eq!(check(&[arg]), Err(ArgsError::Forbidden(arg.to_string())), "{arg}");
        }
        assert_eq!(check(&["--disable-sync", "--lang=de"]), Ok(()));
    }
}
