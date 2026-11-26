use super::model::{Field, Fingerprint, Gpu, Location, Os, Screen, Setting, WebRtc};
use super::validate::Arch;
use serde::Deserialize;
use std::sync::OnceLock;

/// Набор комбинаций из `tools/fingerprints/generate.py`.
#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub combos: Vec<Combo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Combo {
    pub weight: u32,
    pub arch: String,
    pub os_version: String,
    pub screen: Screen,
    pub hardware_concurrency: u32,
    pub device_memory: u32,
    pub gpu: Gpu,
}

pub trait RandomSource {
    fn next_u64(&mut self) -> u64;
}

/// Системный источник случайности.
pub struct OsRandom;

impl RandomSource for OsRandom {
    fn next_u64(&mut self) -> u64 {
        getrandom::u64().expect("нет системного источника случайности")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GenerateError {
    #[error("в наборе нет комбинаций для архитектуры {0}")]
    NoCombos(&'static str),
}

impl Catalog {
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn builtin() -> &'static Catalog {
        static CATALOG: OnceLock<Catalog> = OnceLock::new();
        CATALOG.get_or_init(|| {
            Catalog::from_json(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/fingerprints.json")))
                .expect("resources/fingerprints.json повреждён")
        })
    }

    /// Случайная комбинация под архитектуру машины с учётом весов.
    fn pick(&self, arch: Arch, rng: &mut dyn RandomSource) -> Result<&Combo, GenerateError> {
        let candidates: Vec<&Combo> =
            self.combos.iter().filter(|c| c.arch == arch.catalog_name() && c.weight > 0).collect();
        let total: u64 = candidates.iter().map(|c| u64::from(c.weight)).sum();
        if total == 0 {
            return Err(GenerateError::NoCombos(arch.catalog_name()));
        }
        let mut roll = rng.next_u64() % total;
        for combo in candidates {
            let weight = u64::from(combo.weight);
            if roll < weight {
                return Ok(combo);
            }
            roll -= weight;
        }
        unreachable!("бросок меньше суммы весов")
    }
}

pub fn generate(catalog: &Catalog, arch: Arch, rng: &mut dyn RandomSource) -> Result<Fingerprint, GenerateError> {
    let combo = catalog.pick(arch, rng)?;
    Ok(Fingerprint {
        seed: 10_000 + (rng.next_u64() % 90_000) as u32,
        os: Os::Macos,
        os_version: combo.os_version.clone(),
        brand: "Chrome".into(),
        gpu: combo.gpu.clone(),
        hardware_concurrency: combo.hardware_concurrency,
        device_memory: combo.device_memory,
        screen: combo.screen,
        timezone: Setting::Auto,
        language: Setting::Auto,
        webrtc: WebRtc::Auto,
        noise: true,
        storage_quota_mb: None,
        device_pixel_ratio: 2, // ОС сейчас всегда macOS
        max_touch_points: 0,
        location: Location::Off,
        manual: Vec::new(),
    })
}

/// «Сгенерировать заново»: новый seed и комбинация, поля из `manual` остаются как были.
pub fn regenerate(
    current: &Fingerprint,
    catalog: &Catalog,
    arch: Arch,
    rng: &mut dyn RandomSource,
) -> Result<Fingerprint, GenerateError> {
    let mut next = generate(catalog, arch, rng)?;
    for field in &current.manual {
        match field {
            Field::OsVersion => next.os_version = current.os_version.clone(),
            Field::Gpu => next.gpu = current.gpu.clone(),
            Field::HardwareConcurrency => next.hardware_concurrency = current.hardware_concurrency,
            Field::DeviceMemory => next.device_memory = current.device_memory,
            Field::Screen => next.screen = current.screen,
            Field::Timezone => next.timezone = current.timezone.clone(),
            Field::Language => next.language = current.language.clone(),
            Field::Webrtc => next.webrtc = current.webrtc.clone(),
            Field::Noise => next.noise = current.noise,
            Field::StorageQuotaMb => next.storage_quota_mb = current.storage_quota_mb,
            Field::DevicePixelRatio => next.device_pixel_ratio = current.device_pixel_ratio,
            Field::MaxTouchPoints => next.max_touch_points = current.max_touch_points,
            Field::Location => next.location = current.location.clone(),
        }
    }
    next.manual = current.manual.clone();
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::validate::validate;

    const FIXTURE: &str = r#"{
      "generated_at": "2026-09-30", "browserforge": "1.2.4", "chrome_versions": ["147"],
      "combos": [
        {"weight": 1, "arch": "arm", "os_version": "15.6.1", "screen": {"width": 1512, "height": 982},
         "hardware_concurrency": 8, "device_memory": 16,
         "gpu": {"vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple, ANGLE Metal Renderer: Apple M1, Unspecified Version)"}},
        {"weight": 3, "arch": "arm", "os_version": "26.4.1", "screen": {"width": 1728, "height": 1117},
         "hardware_concurrency": 10, "device_memory": 8,
         "gpu": {"vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple, ANGLE Metal Renderer: Apple M4, Unspecified Version)"}},
        {"weight": 2, "arch": "x86", "os_version": "15.7.4", "screen": {"width": 1440, "height": 900},
         "hardware_concurrency": 4, "device_memory": 8,
         "gpu": {"vendor": "Google Inc. (Intel)", "renderer": "ANGLE (Intel, Intel(R) Iris(TM) Plus Graphics 655, OpenGL 4.1)"}}
      ]
    }"#;

    struct Seq(Vec<u64>);

    impl RandomSource for Seq {
        fn next_u64(&mut self) -> u64 {
            self.0.remove(0)
        }
    }

    fn catalog() -> Catalog {
        Catalog::from_json(FIXTURE).unwrap()
    }

    #[test]
    fn weights_decide_the_combo() {
        let c = catalog();
        assert_eq!(generate(&c, Arch::Arm64, &mut Seq(vec![0, 5])).unwrap().os_version, "15.6.1");
        assert_eq!(generate(&c, Arch::Arm64, &mut Seq(vec![1, 5])).unwrap().os_version, "26.4.1");
        assert_eq!(generate(&c, Arch::Arm64, &mut Seq(vec![3, 5])).unwrap().os_version, "26.4.1");
        assert_eq!(generate(&c, Arch::Arm64, &mut Seq(vec![4, 5])).unwrap().os_version, "15.6.1");
    }

    #[test]
    fn generated_fingerprint_has_defaults() {
        let fp = generate(&catalog(), Arch::Arm64, &mut Seq(vec![0, 5])).unwrap();
        assert_eq!(fp.seed, 10_005);
        assert_eq!(fp.brand, "Chrome");
        assert_eq!(fp.timezone, Setting::Auto);
        assert!(fp.manual.is_empty());
        assert!(fp.noise);
        assert!(validate(&fp, Arch::Arm64).is_empty());
    }

    #[test]
    fn arch_filters_combos() {
        let fp = generate(&catalog(), Arch::X86_64, &mut Seq(vec![7, 89_999])).unwrap();
        assert_eq!(fp.os_version, "15.7.4");
        assert_eq!(fp.seed, 99_999);
        let empty = Catalog::from_json(&FIXTURE.replace("\"x86\"", "\"arm\"")).unwrap();
        assert_eq!(generate(&empty, Arch::X86_64, &mut Seq(vec![0, 0])), Err(GenerateError::NoCombos("x86")));
    }

    #[test]
    fn regenerate_keeps_manual_fields() {
        let c = catalog();
        let mut current = generate(&c, Arch::Arm64, &mut Seq(vec![0, 5])).unwrap();
        current.hardware_concurrency = 12;
        current.timezone = Setting::Manual { value: "Asia/Tokyo".into() };
        current.manual = vec![Field::HardwareConcurrency, Field::Timezone];
        let next = regenerate(&current, &c, Arch::Arm64, &mut Seq(vec![1, 77])).unwrap();
        assert_eq!(next.os_version, "26.4.1");
        assert_eq!(next.seed, 10_077);
        assert_eq!(next.hardware_concurrency, 12);
        assert_eq!(next.timezone, Setting::Manual { value: "Asia/Tokyo".into() });
        assert_eq!(next.manual, current.manual);
    }

    #[test]
    fn builtin_catalog_is_consistent() {
        let c = Catalog::builtin();
        for arch in [Arch::Arm64, Arch::X86_64] {
            let combos: Vec<&Combo> = c.combos.iter().filter(|x| x.arch == arch.catalog_name()).collect();
            assert!(!combos.is_empty(), "нет комбинаций для {arch:?}");
            for combo in combos {
                let fp = Fingerprint {
                    seed: 10_000,
                    os: Os::Macos,
                    os_version: combo.os_version.clone(),
                    brand: "Chrome".into(),
                    gpu: combo.gpu.clone(),
                    hardware_concurrency: combo.hardware_concurrency,
                    device_memory: combo.device_memory,
                    screen: combo.screen,
                    timezone: Setting::Auto,
                    language: Setting::Auto,
                    webrtc: WebRtc::Auto,
                    noise: true,
                    storage_quota_mb: None,
                    device_pixel_ratio: 2,
                    max_touch_points: 0,
                    location: Location::Off,
                    manual: Vec::new(),
                };
                assert!(validate(&fp, arch).is_empty(), "{combo:?}: {:?}", validate(&fp, arch));
            }
        }
    }
}
