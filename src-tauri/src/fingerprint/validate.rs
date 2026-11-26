use super::model::{Fingerprint, Gpu, Location, Os, Setting, WebRtc};
use serde::Serialize;
use std::net::IpAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    Arm64,
    X86_64,
}

impl Arch {
    pub fn host() -> Self {
        if cfg!(target_arch = "aarch64") { Arch::Arm64 } else { Arch::X86_64 }
    }

    /// Как архитектура записана в наборе комбинаций (`uaData.architecture` в BrowserForge).
    pub fn catalog_name(self) -> &'static str {
        match self {
            Arch::Arm64 => "arm",
            Arch::X86_64 => "x86",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FieldError {
    pub field: &'static str,
    pub code: &'static str,
}

pub const DEVICE_MEMORY: [u32; 5] = [2, 4, 8, 16, 32];

pub fn is_apple_gpu(gpu: &Gpu) -> bool {
    gpu.vendor.contains("Apple") || gpu.renderer.contains("Apple")
}

/// Проверки согласованности (спецификация этапа 1 с поправками).
pub fn validate(fp: &Fingerprint, arch: Arch) -> Vec<FieldError> {
    let mut errors = Vec::new();
    let mut push = |field: &'static str, code: &'static str| errors.push(FieldError { field, code });

    if !(10_000..=99_999).contains(&fp.seed) {
        push("seed", "out_of_range");
    }
    if fp.os == Os::Windows {
        push("os", "windows_unsupported");
    }
    if !is_version(&fp.os_version) {
        push("os_version", "invalid");
    }
    if fp.brand != "Chrome" {
        push("brand", "unsupported");
    }
    let apple = is_apple_gpu(&fp.gpu);
    if fp.gpu.vendor.trim().is_empty() || fp.gpu.renderer.trim().is_empty() {
        push("gpu", "empty");
    } else if apple && fp.os != Os::Macos {
        push("gpu", "apple_requires_macos");
    } else if apple != (arch == Arch::Arm64) {
        push("gpu", "arch_mismatch");
    }
    if !(2..=32).contains(&fp.hardware_concurrency) {
        push("hardware_concurrency", "out_of_range");
    }
    if !DEVICE_MEMORY.contains(&fp.device_memory) {
        push("device_memory", "invalid");
    }
    if !(1024..=7680).contains(&fp.screen.width) || !(640..=4320).contains(&fp.screen.height) {
        push("screen", "out_of_range");
    }
    if ![1, 2, 3].contains(&fp.device_pixel_ratio) {
        push("device_pixel_ratio", "invalid");
    }
    if ![0, 5, 10].contains(&fp.max_touch_points) {
        push("max_touch_points", "invalid");
    }
    if let Location::Manual { lat, lon } = fp.location {
        if !(-90.0..=90.0).contains(&lat) {
            push("location.lat", "invalid");
        }
        if !(-180.0..=180.0).contains(&lon) {
            push("location.lon", "invalid");
        }
    }
    if let Setting::Manual { value } = &fp.timezone
        && !super::locale::is_known_timezone(value)
    {
        push("timezone", "unknown");
    }
    if let Setting::Manual { value } = &fp.language
        && !is_bcp47(value)
    {
        push("language", "invalid");
    }
    if let WebRtc::Manual { ip } = &fp.webrtc
        && ip.parse::<IpAddr>().is_err()
    {
        push("webrtc", "invalid_ip");
    }
    if fp.storage_quota_mb == Some(0) {
        push("storage_quota_mb", "out_of_range");
    }
    errors
}

fn is_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    (2..=3).contains(&parts.len()) && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Язык, необязательная письменность и регион: `de`, `de-DE`, `zh-Hant-TW`, `es-419`.
pub fn is_bcp47(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let Some(language) = parts.next() else { return false };
    if !(2..=3).contains(&language.len()) || !language.chars().all(|c| c.is_ascii_lowercase()) {
        return false;
    }
    let rest: Vec<&str> = parts.collect();
    let mut index = 0;
    if let Some(script) = rest.get(index)
        && script.len() == 4
        && script.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && script.chars().skip(1).all(|c| c.is_ascii_lowercase())
    {
        index += 1;
    }
    if let Some(region) = rest.get(index)
        && ((region.len() == 2 && region.chars().all(|c| c.is_ascii_uppercase()))
            || (region.len() == 3 && region.chars().all(|c| c.is_ascii_digit())))
    {
        index += 1;
    }
    index == rest.len()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fingerprint::model::{Field, Screen};

    pub(crate) fn apple_fp() -> Fingerprint {
        Fingerprint {
            seed: 48213,
            os: Os::Macos,
            os_version: "15.2.0".into(),
            brand: "Chrome".into(),
            gpu: Gpu {
                vendor: "Google Inc. (Apple)".into(),
                renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)".into(),
            },
            hardware_concurrency: 8,
            device_memory: 16,
            screen: Screen { width: 1512, height: 982 },
            timezone: Setting::Auto,
            language: Setting::Auto,
            webrtc: WebRtc::Auto,
            noise: true,
            storage_quota_mb: None,
            device_pixel_ratio: 2,
            max_touch_points: 0,
            location: Location::Off,
            manual: vec![Field::Screen],
        }
    }

    fn codes(fp: &Fingerprint, arch: Arch) -> Vec<(&'static str, &'static str)> {
        validate(fp, arch).into_iter().map(|e| (e.field, e.code)).collect()
    }

    #[test]
    fn new_fields_are_range_checked() {
        let mut fp = Fingerprint::from_json(crate::fingerprint::model::tests::SPEC_JSON).unwrap();
        fp.device_pixel_ratio = 4;
        fp.max_touch_points = 7;
        fp.location = Location::Manual { lat: 91.0, lon: -200.0 };
        let fields: Vec<&str> = validate(&fp, Arch::Arm64).iter().map(|e| e.field).collect();
        for f in ["device_pixel_ratio", "max_touch_points", "location.lat", "location.lon"] {
            assert!(fields.contains(&f), "нет ошибки {f}: {fields:?}");
        }
    }

    #[test]
    fn valid_fingerprint_passes() {
        assert!(validate(&apple_fp(), Arch::Arm64).is_empty());
    }

    #[test]
    fn gpu_must_match_os_and_arch() {
        let mut fp = apple_fp();
        assert_eq!(codes(&fp, Arch::X86_64), [("gpu", "arch_mismatch")]);
        fp.gpu = Gpu { vendor: "Google Inc. (Intel)".into(), renderer: "ANGLE (Intel, Intel(R) Iris(TM) Plus Graphics 655)".into() };
        assert_eq!(codes(&fp, Arch::Arm64), [("gpu", "arch_mismatch")]);
        assert!(codes(&fp, Arch::X86_64).is_empty());
        let mut fp = apple_fp();
        fp.os = Os::Windows;
        assert_eq!(codes(&fp, Arch::Arm64), [("os", "windows_unsupported"), ("gpu", "apple_requires_macos")]);
    }

    #[test]
    fn hardware_limits() {
        let mut fp = apple_fp();
        fp.hardware_concurrency = 11;
        fp.device_memory = 32;
        assert!(validate(&fp, Arch::Arm64).is_empty());
        fp.hardware_concurrency = 1;
        fp.device_memory = 12;
        fp.screen = Screen { width: 800, height: 600 };
        assert_eq!(
            codes(&fp, Arch::Arm64),
            [("hardware_concurrency", "out_of_range"), ("device_memory", "invalid"), ("screen", "out_of_range")]
        );
    }

    #[test]
    fn manual_values_are_checked() {
        let mut fp = apple_fp();
        fp.timezone = Setting::Manual { value: "Europe/Berlin".into() };
        fp.language = Setting::Manual { value: "zh-Hant-TW".into() };
        fp.webrtc = WebRtc::Manual { ip: "2001:db8::1".into() };
        assert!(validate(&fp, Arch::Arm64).is_empty());
        fp.timezone = Setting::Manual { value: "Mars/Olympus".into() };
        fp.language = Setting::Manual { value: "DE_de".into() };
        fp.webrtc = WebRtc::Manual { ip: "999.1.1.1".into() };
        assert_eq!(
            codes(&fp, Arch::Arm64),
            [("timezone", "unknown"), ("language", "invalid"), ("webrtc", "invalid_ip")]
        );
    }

    #[test]
    fn seed_version_and_brand() {
        let mut fp = apple_fp();
        fp.seed = 42;
        fp.os_version = "fifteen".into();
        fp.brand = "Edge".into();
        assert_eq!(
            codes(&fp, Arch::Arm64),
            [("seed", "out_of_range"), ("os_version", "invalid"), ("brand", "unsupported")]
        );
    }

    #[test]
    fn language_tags() {
        for ok in ["en", "de-DE", "zh-Hant", "zh-Hant-TW", "es-419", "fil"] {
            assert!(is_bcp47(ok), "{ok}");
        }
        for bad in ["", "e", "english", "de_DE", "de-de", "zh-hant-TW", "de-DE-x"] {
            assert!(!is_bcp47(bad), "{bad}");
        }
    }
}
