use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Macos,
    Windows,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    pub vendor: String,
    pub renderer: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
}

/// Таймзона и язык: «по IP шлюза» или вручную.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Setting {
    Auto,
    Manual { value: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum WebRtc {
    Auto,
    Manual { ip: String },
}

/// Геолокация: «нет» — персона движка, «вручную» — координаты (`--fingerprint-location`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Location {
    #[default]
    Off,
    Manual { lat: f64, lon: f64 },
}

fn default_dpr() -> u32 {
    2
}

/// Поля, которые пользователь может поменять руками; «Сгенерировать заново» их не трогает.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    OsVersion,
    Gpu,
    HardwareConcurrency,
    DeviceMemory,
    Screen,
    Timezone,
    Language,
    Webrtc,
    Noise,
    StorageQuotaMb,
    DevicePixelRatio,
    MaxTouchPoints,
    Location,
}

/// Модель отпечатка из спецификации этапа 1. Версию браузера и UA даёт сборка движка.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub seed: u32,
    pub os: Os,
    pub os_version: String,
    pub brand: String,
    pub gpu: Gpu,
    pub hardware_concurrency: u32,
    pub device_memory: u32,
    pub screen: Screen,
    pub timezone: Setting,
    pub language: Setting,
    pub webrtc: WebRtc,
    pub noise: bool,
    pub storage_quota_mb: Option<u32>,
    #[serde(default = "default_dpr")]
    pub device_pixel_ratio: u32,
    #[serde(default)]
    pub max_touch_points: u32,
    #[serde(default)]
    pub location: Location,
    #[serde(default)]
    pub manual: Vec<Field>,
}

impl Fingerprint {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("отпечаток сериализуется")
    }

    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// JSON из спецификации этапа 1, раздел «Отпечаток».
    pub(crate) const SPEC_JSON: &str = r#"{
      "seed": 48213,
      "os": "macos", "os_version": "15.2.0",
      "brand": "Chrome",
      "gpu": { "vendor": "Google Inc. (Apple)", "renderer": "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)" },
      "hardware_concurrency": 8, "device_memory": 8,
      "screen": { "width": 1440, "height": 900 },
      "timezone": { "mode": "auto" },
      "language": { "mode": "manual", "value": "de-DE" },
      "webrtc": { "mode": "auto" },
      "noise": true,
      "storage_quota_mb": null,
      "manual": ["hardware_concurrency"]
    }"#;

    #[test]
    fn spec_json_round_trips() {
        let fp = Fingerprint::from_json(SPEC_JSON).unwrap();
        assert_eq!(fp.seed, 48213);
        assert_eq!(fp.language, Setting::Manual { value: "de-DE".into() });
        assert_eq!(fp.timezone, Setting::Auto);
        assert_eq!(fp.manual, vec![Field::HardwareConcurrency]);
        assert_eq!(Fingerprint::from_json(&fp.to_json()).unwrap(), fp);
    }

    #[test]
    fn old_json_gets_defaults_for_new_fields() {
        let fp = Fingerprint::from_json(SPEC_JSON).unwrap();
        assert_eq!(fp.device_pixel_ratio, 2, "macOS — 2");
        assert_eq!(fp.max_touch_points, 0);
        assert_eq!(fp.location, Location::Off);
        let json = serde_json::to_string(&Location::Manual { lat: 52.52, lon: 13.405 }).unwrap();
        assert_eq!(json, r#"{"mode":"manual","lat":52.52,"lon":13.405}"#);
    }

    #[test]
    fn manual_webrtc_uses_ip_key() {
        let json = serde_json::to_string(&WebRtc::Manual { ip: "203.0.113.7".into() }).unwrap();
        assert_eq!(json, r#"{"mode":"manual","ip":"203.0.113.7"}"#);
    }
}
