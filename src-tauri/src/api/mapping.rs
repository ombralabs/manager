//! Перевод `fingerprint_config` и `user_proxy_config` AdsPower ⇄ модель Ombra (спецификация 2Б, «`fingerprint_config`»).
//! Чистые функции: без базы и сервера.
use super::envelope::{ApiFailure, s};
use super::params::{Params, scalar_text};
use crate::app::patch::ProxyConfig;
use crate::fingerprint::{self, Arch, Field, FieldError, Fingerprint, Gpu, Location, Screen, Setting, WebRtc};
use crate::store::proxies::{ProxyCredentials, ProxyKind};
use crate::store::secrets::Secret;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {reason}")]
pub struct MappingError {
    pub field: String,
    pub reason: &'static str,
}

impl MappingError {
    fn new(field: impl Into<String>, reason: &'static str) -> Self {
        Self { field: field.into(), reason }
    }
}

impl From<MappingError> for ApiFailure {
    fn from(e: MappingError) -> Self {
        ApiFailure(e.to_string())
    }
}

/// Поля, которые Ombra не применяет, но хранит в `api_extra` и возвращает как присланы.
pub const STORED_FIELDS: &[&str] = &[
    "fonts",
    "media_devices",
    "media_devices_num",
    "mac_address_config",
    "device_name_switch",
    "device_name",
    "speech_switch",
    "tls_switch",
    "tls",
    "do_not_track",
    "scan_port_type",
    "allow_scan_ports",
    "flash",
    "gpu",
    "location",
    "accuracy",
    "page_language",
    "browser_kernel_config",
    "webrtc",
];

/// Переключатели шума AdsPower: шум выключается, только если все четыре — `0`.
const NOISE_FIELDS: [&str; 4] = ["canvas", "webgl_image", "audio", "client_rects"];
/// Применяемые поля со значением-строкой (число тоже принимается). Массив или объект в них — ошибка, а не пропуск:
/// иначе `"ua": ["…Windows…"]` прошёл бы мимо проверки «только macOS».
const SCALAR_FIELDS: [&str; 12] = [
    "automatic_timezone",
    "timezone",
    "language_switch",
    "screen_resolution",
    "hardware_concurrency",
    "device_memory",
    "webgl",
    "ua",
    "canvas",
    "webgl_image",
    "audio",
    "client_rects",
];
const UNSUPPORTED_UA: &str = "only macOS Chrome profiles are supported";
const NOT_SUPPORTED: &str = "value not supported";
/// Язык, если `language_switch` = 0 без списка, — умолчание AdsPower.
const DEFAULT_LANGUAGE: &str = "en-US";

#[derive(Debug, Clone)]
pub struct MappedFingerprint {
    pub fingerprint: Fingerprint,
    /// Новое содержимое `api_extra`.
    pub extra: Map<String, Value>,
}

/// `base` — текущий отпечаток (правка) или свежий от генератора (создание); `extra` — прежний `api_extra`.
/// Явно заданные применяемые поля помечаются «вручную»; итог проверяется так же, как в редакторе.
pub fn apply_fingerprint_config(
    base: &Fingerprint,
    extra: &Map<String, Value>,
    config: &Map<String, Value>,
    arch: Arch,
) -> Result<MappedFingerprint, MappingError> {
    let config = Params::from_map(config.clone());
    check_scalars(&config)?;
    let mut fp = base.clone();
    let mut extra = extra.clone();
    apply_timezone(&mut fp, &config)?;
    apply_language(&mut fp, &config)?;
    apply_screen(&mut fp, &config)?;
    if let Some(cores) = number(&config, "hardware_concurrency")? {
        fp.hardware_concurrency = cores;
        mark(&mut fp, Field::HardwareConcurrency);
    }
    if let Some(memory) = number(&config, "device_memory")? {
        fp.device_memory = memory;
        mark(&mut fp, Field::DeviceMemory);
    }
    apply_webgl(&mut fp, &config)?;
    check_user_agent(&config)?;
    apply_noise(&mut fp, &config)?;
    apply_location(&mut fp, &config)?;
    if config.get("webrtc").is_some() {
        // В Ombra WebRTC всегда отдаёт IP шлюза; присланное значение только хранится.
        fp.webrtc = WebRtc::Auto;
        unmark(&mut fp, Field::Webrtc);
    }
    for name in STORED_FIELDS {
        if let Some(value) = config.get(name) {
            extra.insert((*name).to_string(), value.clone());
        }
    }
    if let Some(error) = fingerprint::validate(&fp, arch).into_iter().next() {
        return Err(validation_error(error));
    }
    Ok(MappedFingerprint { fingerprint: fp, extra })
}

/// `fingerprint_config` для ответов: фактический отпечаток и хранимые поля (спецификация 2Б, «В ответах запроса»).
pub fn fingerprint_config(fp: &Fingerprint, extra: &Map<String, Value>, engine_version: &str) -> Map<String, Value> {
    let mut config = Map::new();
    let mut put = |name: &str, value: Value| {
        config.insert(name.to_string(), value);
    };
    match &fp.timezone {
        Setting::Auto => {
            put("automatic_timezone", s(1));
            put("timezone", s(""));
        }
        Setting::Manual { value } => {
            put("automatic_timezone", s(0));
            put("timezone", s(value));
        }
    }
    match &fp.language {
        Setting::Auto => {
            put("language_switch", s(1));
            put("language", json!([]));
        }
        Setting::Manual { value } => {
            put("language_switch", s(0));
            put("language", languages(value));
        }
    }
    put("screen_resolution", s(format!("{}_{}", fp.screen.width, fp.screen.height)));
    put("hardware_concurrency", s(fp.hardware_concurrency));
    put("device_memory", s(fp.device_memory));
    put("webgl", s(2));
    put("webgl_config", json!({"unmasked_vendor": fp.gpu.vendor, "unmasked_renderer": fp.gpu.renderer}));
    put("webrtc", s("proxy"));
    put("ua", Value::String(chrome_mac_ua(engine_version)));
    let noise = if fp.noise { "1" } else { "0" };
    for name in NOISE_FIELDS {
        put(name, s(noise));
    }
    match &fp.location {
        Location::Off => put("location_switch", s(1)),
        Location::Manual { lat, lon } => {
            put("location_switch", s(0));
            put("latitude", s(lat));
            put("longitude", s(lon));
        }
    }
    for (name, value) in extra {
        if STORED_FIELDS.contains(&name.as_str()) {
            put(name.as_str(), value.clone());
        }
    }
    config
}

/// UA Chrome на macOS, как его отдаёт сборка: Chrome урезает версию до мажорной, а macOS — до 10_15_7.
pub fn chrome_mac_ua(engine_version: &str) -> String {
    let major = engine_version.split('.').next().filter(|major| !major.is_empty()).unwrap_or(engine_version);
    format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"
    )
}

/// `None` — `proxy_soft: no_proxy`: снять закрепление, при запуске профиль получит случайный прокси.
pub fn parse_user_proxy_config(config: &Map<String, Value>) -> Result<Option<ProxyConfig>, MappingError> {
    let config = Params::from_map(config.clone());
    if text(config.get("proxy_soft")).is_some_and(|soft| soft.eq_ignore_ascii_case("no_proxy")) {
        return Ok(None);
    }
    let kind = text(config.get("proxy_type"))
        .ok_or_else(|| MappingError::new("user_proxy_config.proxy_type", "is required"))?;
    let kind = ProxyKind::parse(&kind.to_ascii_lowercase())
        .ok_or_else(|| MappingError::new("user_proxy_config.proxy_type", "unsupported proxy type"))?;
    let host =
        text(config.get("proxy_host")).ok_or_else(|| MappingError::new("user_proxy_config.proxy_host", "is required"))?;
    let port = text(config.get("proxy_port"))
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| MappingError::new("user_proxy_config.proxy_port", "must be 1..65535"))?;
    let username = text(config.get("proxy_user"));
    let password = raw_text(config.get("proxy_password")).map(Secret::new);
    if password.is_some() && username.is_none() {
        return Err(MappingError::new("user_proxy_config.proxy_user", "is required with proxy_password"));
    }
    Ok(Some(ProxyConfig { kind, host, port, username, password }))
}

/// Для ответов: поля прокси с паролем открытым текстом (решение заказчика); без прокси — `no_proxy`.
pub fn user_proxy_config(creds: Option<&ProxyCredentials>) -> Value {
    let Some(proxy) = creds else {
        return json!({"proxy_soft": "no_proxy"});
    };
    json!({
        "proxy_soft": "other",
        "proxy_type": proxy.kind.as_str(),
        "proxy_host": proxy.host,
        "proxy_port": s(proxy.port),
        "proxy_user": proxy.username.clone().unwrap_or_default(),
        "proxy_password": proxy.password.as_ref().map(|p| p.expose().to_string()).unwrap_or_default(),
    })
}

fn apply_timezone(fp: &mut Fingerprint, config: &Params) -> Result<(), MappingError> {
    match switch(config, "automatic_timezone")? {
        None => {}
        Some(true) => {
            fp.timezone = Setting::Auto;
            mark(fp, Field::Timezone);
        }
        Some(false) => {
            let zone = text(config.get("timezone"))
                .ok_or_else(|| MappingError::new(field("timezone"), "is required when automatic_timezone is 0"))?;
            fp.timezone = Setting::Manual { value: zone };
            mark(fp, Field::Timezone);
        }
    }
    Ok(())
}

fn apply_language(fp: &mut Fingerprint, config: &Params) -> Result<(), MappingError> {
    match switch(config, "language_switch")? {
        None => {}
        Some(true) => {
            fp.language = Setting::Auto;
            mark(fp, Field::Language);
        }
        Some(false) => {
            let first = first_language(config.get("language"))?.unwrap_or_else(|| DEFAULT_LANGUAGE.to_string());
            fp.language = Setting::Manual { value: first };
            mark(fp, Field::Language);
        }
    }
    Ok(())
}

fn first_language(value: Option<&Value>) -> Result<Option<String>, MappingError> {
    let not_a_list = || MappingError::new(field("language"), "must be a list");
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => Ok(items.iter().find_map(|item| text(Some(item)))),
        Some(Value::String(list)) if list.trim_start().starts_with('[') => serde_json::from_str::<Vec<String>>(list)
            .map(|items| items.into_iter().map(|item| item.trim().to_string()).find(|item| !item.is_empty()))
            .map_err(|_| not_a_list()),
        Some(Value::String(list)) => Ok(list.split(',').map(str::trim).find(|item| !item.is_empty()).map(str::to_string)),
        Some(_) => Err(not_a_list()),
    }
}

fn apply_screen(fp: &mut Fingerprint, config: &Params) -> Result<(), MappingError> {
    let Some(value) = text(config.get("screen_resolution")) else {
        return Ok(());
    };
    match value.to_ascii_lowercase().as_str() {
        "none" | "random" => unmark(fp, Field::Screen),
        custom => {
            let (width, height) = custom
                .split_once('_')
                .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)))
                .ok_or_else(|| MappingError::new(field("screen_resolution"), "expected width_height"))?;
            fp.screen = Screen { width, height };
            mark(fp, Field::Screen);
        }
    }
    Ok(())
}

fn apply_webgl(fp: &mut Fingerprint, config: &Params) -> Result<(), MappingError> {
    match text(config.get("webgl")).as_deref() {
        None => Ok(()),
        Some("0" | "3") => {
            unmark(fp, Field::Gpu);
            Ok(())
        }
        Some("2") => {
            let custom = Params::from_map(object(config, "webgl_config")?.unwrap_or_default());
            match (text(custom.get("unmasked_vendor")), text(custom.get("unmasked_renderer"))) {
                (Some(vendor), Some(renderer)) => {
                    fp.gpu = Gpu { vendor, renderer };
                    mark(fp, Field::Gpu);
                    Ok(())
                }
                _ => Err(MappingError::new(field("webgl_config"), "unmasked_vendor and unmasked_renderer are required")),
            }
        }
        Some(_) => Err(MappingError::new(field("webgl"), NOT_SUPPORTED)),
    }
}

/// Персона Ombra — только macOS и Chrome (решение заказчика): чужой `ua` или `random_ua` — ошибка.
fn check_user_agent(config: &Params) -> Result<(), MappingError> {
    if let Some(ua) = text(config.get("ua")) {
        // Как у AdsPower: при своём ua `random_ua` не смотрится.
        return if is_mac_chrome(&ua) { Ok(()) } else { Err(MappingError::new(field("ua"), UNSUPPORTED_UA)) };
    }
    let Some(random) = object(config, "random_ua")? else {
        return Ok(());
    };
    let random = Params::from_map(random);
    let unsupported = || MappingError::new(field("random_ua"), UNSUPPORTED_UA);
    // Пустой список у AdsPower значит «любой»: проверяется только непустой.
    if let Some(browsers) = strings(random.get("ua_browser"))
        && !browsers.is_empty()
        && !browsers.iter().any(|browser| browser.eq_ignore_ascii_case("chrome"))
    {
        return Err(unsupported());
    }
    if let Some(systems) = strings(random.get("ua_system_version"))
        && !systems.is_empty()
        && !systems.iter().any(|system| system.to_ascii_lowercase().starts_with("mac os x"))
    {
        return Err(unsupported());
    }
    Ok(())
}

fn is_mac_chrome(ua: &str) -> bool {
    const OTHER_BRANDS: [&str; 7] = ["Edg/", "OPR/", "Opera", "YaBrowser", "Vivaldi", "SamsungBrowser", "Firefox/"];
    const OTHER_SYSTEMS: [&str; 5] = ["Windows", "Linux", "Android", "iPhone", "iPad"];
    ua.contains("Macintosh")
        && ua.contains("Chrome/")
        && !OTHER_BRANDS.iter().any(|brand| ua.contains(brand))
        && !OTHER_SYSTEMS.iter().any(|system| ua.contains(system))
}

fn apply_noise(fp: &mut Fingerprint, config: &Params) -> Result<(), MappingError> {
    let mut given = 0;
    let mut all_zero = true;
    for name in NOISE_FIELDS {
        match switch(config, name)? {
            None => all_zero = false,
            Some(on) => {
                given += 1;
                all_zero &= !on;
            }
        }
    }
    if given > 0 {
        fp.noise = !all_zero;
        mark(fp, Field::Noise);
    }
    Ok(())
}

/// Число из JSON-числа или строки.
fn num(config: &Params, key: &str) -> Option<f64> {
    match config.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// `location_switch` по AdsPower: «0» — свои координаты (`latitude`/`longitude` верхнего уровня, число или строка),
/// «1» — по IP (умолчание); в Ombra это персона движка. `location` — разрешение (`ask`/`allow`/`block`), только хранится.
fn apply_location(fp: &mut Fingerprint, config: &Params) -> Result<(), MappingError> {
    match switch(config, "location_switch")? {
        Some(true) => {
            fp.location = Location::Off;
            unmark(fp, Field::Location);
        }
        Some(false) => {
            let coordinate = |name: &'static str, limit: f64| match num(config, name) {
                Some(v) if v.abs() <= limit => Ok(v),
                _ => Err(MappingError::new(field(name), "required (valid number) when location_switch is 0")),
            };
            let lat = coordinate("latitude", 90.0)?;
            let lon = coordinate("longitude", 180.0)?;
            fp.location = Location::Manual { lat, lon };
            mark(fp, Field::Location);
        }
        None => {}
    }
    Ok(())
}

fn check_scalars(config: &Params) -> Result<(), MappingError> {
    match SCALAR_FIELDS.iter().find(|name| matches!(config.get(name), Some(Value::Array(_) | Value::Object(_)))) {
        Some(name) => Err(MappingError::new(field(name), "must be a string")),
        None => Ok(()),
    }
}

/// Ошибка проверки согласованности — именем поля AdsPower.
fn validation_error(error: FieldError) -> MappingError {
    let reason = match (error.field, error.code) {
        ("timezone", _) => "unknown time zone",
        ("language", _) => "invalid language tag",
        ("gpu", "arch_mismatch" | "apple_requires_macos") => "GPU does not match this Mac",
        _ => NOT_SUPPORTED,
    };
    MappingError::new(fingerprint_config_field(error.field).unwrap_or_else(|| field(error.field)), reason)
}

/// Поле отпечатка Ombra (`fingerprint::validate`) → имя в `fingerprint_config` AdsPower; не поле отпечатка — `None`.
/// Тем же именем Local API называет поле отпечатка в ошибке проверки ядра (`profiles::api_field_name`).
pub(crate) fn fingerprint_config_field(name: &str) -> Option<String> {
    let adspower = match name {
        "screen" => "screen_resolution",
        "gpu" => "webgl_config",
        "seed" | "os" | "os_version" | "brand" | "hardware_concurrency" | "device_memory" | "timezone" | "language"
        | "webrtc" | "storage_quota_mb" => name,
        _ => return None,
    };
    Some(field(adspower))
}

fn field(name: &str) -> String {
    format!("fingerprint_config.{name}")
}

fn mark(fp: &mut Fingerprint, field: Field) {
    if !fp.manual.contains(&field) {
        fp.manual.push(field);
    }
}

fn unmark(fp: &mut Fingerprint, field: Field) {
    fp.manual.retain(|f| *f != field);
}

/// Строка (без пробелов по краям), число или `true`/`false` (`"1"`/`"0"`) — текстом; пустое и прочее — `None`.
fn text(value: Option<&Value>) -> Option<String> {
    scalar_text(value?).map(|text| text.trim().to_string()).filter(|text| !text.is_empty())
}

/// Как `text`, но без обрезки пробелов: пароль приходит как есть.
fn raw_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn switch(config: &Params, name: &str) -> Result<Option<bool>, MappingError> {
    match text(config.get(name)).as_deref() {
        None => Ok(None),
        Some("1") => Ok(Some(true)),
        Some("0") => Ok(Some(false)),
        Some(_) => Err(MappingError::new(field(name), "must be 0 or 1")),
    }
}

fn number(config: &Params, name: &str) -> Result<Option<u32>, MappingError> {
    match text(config.get(name)) {
        None => Ok(None),
        Some(value) => value.parse().map(Some).map_err(|_| MappingError::new(field(name), NOT_SUPPORTED)),
    }
}

/// Объект или строка с JSON-объектом.
fn object(config: &Params, name: &str) -> Result<Option<Map<String, Value>>, MappingError> {
    config.object(name).map_err(|_| MappingError::new(field(name), "must be an object"))
}

/// Массив строк или одна строка.
fn strings(value: Option<&Value>) -> Option<Vec<String>> {
    match value? {
        Value::Array(items) => Some(items.iter().filter_map(|item| text(Some(item))).collect()),
        single => text(Some(single)).map(|item| vec![item]),
    }
}

/// `de-DE` → `["de-DE", "de"]`, как список языков в профиле AdsPower.
fn languages(tag: &str) -> Value {
    match tag.split_once('-') {
        Some((primary, _)) => json!([tag, primary]),
        None => json!([tag]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::{Catalog, OsRandom, generate};
    use serde_json::json;

    const VERSION: &str = "145.0.7632.109.2";
    const MAC_CHROME: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) \
                              Chrome/131.0.0.0 Safari/537.36";
    const WIN_CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
                              Chrome/87.0.4280.141 Safari/537.36";
    const MAC_EDGE: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) \
                            Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";
    const MAC_SAFARI: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) \
                              Version/17.0 Safari/605.1.15";

    /// Свежий отпечаток от генератора без ручных полей: таймзона и язык по IP, шум включён.
    fn base() -> Fingerprint {
        let mut fp = generate(Catalog::builtin(), Arch::host(), &mut OsRandom).unwrap();
        fp.manual.clear();
        fp.timezone = Setting::Auto;
        fp.language = Setting::Auto;
        fp.webrtc = WebRtc::Auto;
        fp.noise = true;
        fp
    }

    /// Видеокарта, согласованная с архитектурой этого Mac, и чужая.
    fn host_gpu() -> Gpu {
        match Arch::host() {
            Arch::Arm64 => Gpu {
                vendor: "Google Inc. (Apple)".into(),
                renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M3, Unspecified Version)".into(),
            },
            Arch::X86_64 => Gpu {
                vendor: "Google Inc. (Intel)".into(),
                renderer: "ANGLE (Intel, ANGLE Metal Renderer: Intel(R) Iris(TM) Plus Graphics, Unspecified Version)".into(),
            },
        }
    }

    fn foreign_gpu() -> Gpu {
        match Arch::host() {
            Arch::Arm64 => Gpu {
                vendor: "Google Inc. (Intel)".into(),
                renderer: "ANGLE (Intel, ANGLE Metal Renderer: Intel(R) Iris(TM) Plus Graphics, Unspecified Version)".into(),
            },
            Arch::X86_64 => Gpu {
                vendor: "Google Inc. (Apple)".into(),
                renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M3, Unspecified Version)".into(),
            },
        }
    }

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn apply(fp: &Fingerprint, config: Value) -> Result<MappedFingerprint, MappingError> {
        apply_fingerprint_config(fp, &Map::new(), &map(config), Arch::host())
    }

    fn error(field: &str, reason: &'static str) -> MappingError {
        MappingError { field: field.to_string(), reason }
    }

    #[test]
    fn empty_config_keeps_fingerprint_and_extra() {
        let fp = base();
        let extra = map(json!({"fonts": ["all"]}));
        let mapped = apply_fingerprint_config(&fp, &extra, &Map::new(), Arch::host()).unwrap();
        assert_eq!(mapped.fingerprint, fp);
        assert_eq!(mapped.extra, extra);
    }

    #[test]
    fn timezone_switch_and_value() {
        let fp = base();
        let manual = apply(&fp, json!({"automatic_timezone": "0", "timezone": "Europe/Berlin"})).unwrap().fingerprint;
        assert_eq!(manual.timezone, Setting::Manual { value: "Europe/Berlin".into() });
        assert!(manual.manual.contains(&Field::Timezone));
        let auto = apply(&manual, json!({"automatic_timezone": 1})).unwrap().fingerprint;
        assert_eq!(auto.timezone, Setting::Auto);
        assert_eq!(apply(&fp, json!({"timezone": "Asia/Tokyo"})).unwrap().fingerprint.timezone, Setting::Auto, "без переключателя");
        assert_eq!(
            apply(&fp, json!({"automatic_timezone": "0"})).unwrap_err(),
            error("fingerprint_config.timezone", "is required when automatic_timezone is 0")
        );
        assert_eq!(
            apply(&fp, json!({"automatic_timezone": "0", "timezone": "Mars/Base"})).unwrap_err(),
            error("fingerprint_config.timezone", "unknown time zone")
        );
        assert_eq!(
            apply(&fp, json!({"automatic_timezone": "2"})).unwrap_err(),
            error("fingerprint_config.automatic_timezone", "must be 0 or 1")
        );
    }

    #[test]
    fn language_switch_and_list() {
        let fp = base();
        let german = apply(&fp, json!({"language_switch": "0", "language": ["de-DE", "de"]})).unwrap().fingerprint;
        assert_eq!(german.language, Setting::Manual { value: "de-DE".into() });
        assert!(german.manual.contains(&Field::Language));
        let default = apply(&fp, json!({"language_switch": "0"})).unwrap().fingerprint;
        assert_eq!(default.language, Setting::Manual { value: "en-US".into() }, "умолчание AdsPower");
        let from_text = apply(&fp, json!({"language_switch": 0, "language": "[\"ja-JP\",\"ja\"]"})).unwrap().fingerprint;
        assert_eq!(from_text.language, Setting::Manual { value: "ja-JP".into() });
        assert_eq!(apply(&german, json!({"language_switch": "1"})).unwrap().fingerprint.language, Setting::Auto);
        assert_eq!(
            apply(&fp, json!({"language_switch": "0", "language": ["xx_YY"]})).unwrap_err(),
            error("fingerprint_config.language", "invalid language tag")
        );
    }

    #[test]
    fn screen_resolution() {
        let fp = base();
        let custom = apply(&fp, json!({"screen_resolution": "1920_1080"})).unwrap().fingerprint;
        assert_eq!(custom.screen, Screen { width: 1920, height: 1080 });
        assert!(custom.manual.contains(&Field::Screen));
        let random = apply(&custom, json!({"screen_resolution": "random"})).unwrap().fingerprint;
        assert_eq!(random.screen, custom.screen, "экран не меняется, но снова от генератора");
        assert!(!random.manual.contains(&Field::Screen));
        assert!(!apply(&custom, json!({"screen_resolution": "none"})).unwrap().fingerprint.manual.contains(&Field::Screen));
        assert_eq!(
            apply(&fp, json!({"screen_resolution": "big"})).unwrap_err(),
            error("fingerprint_config.screen_resolution", "expected width_height")
        );
        assert_eq!(
            apply(&fp, json!({"screen_resolution": "100_100"})).unwrap_err(),
            error("fingerprint_config.screen_resolution", "value not supported")
        );
    }

    #[test]
    fn cores_and_memory() {
        let fp = base();
        let custom = apply(&fp, json!({"hardware_concurrency": "6", "device_memory": 16})).unwrap().fingerprint;
        assert_eq!((custom.hardware_concurrency, custom.device_memory), (6, 16));
        assert!(custom.manual.contains(&Field::HardwareConcurrency) && custom.manual.contains(&Field::DeviceMemory));
        assert_eq!(
            apply(&fp, json!({"device_memory": "6"})).unwrap_err(),
            error("fingerprint_config.device_memory", "value not supported")
        );
        assert_eq!(
            apply(&fp, json!({"hardware_concurrency": "many"})).unwrap_err(),
            error("fingerprint_config.hardware_concurrency", "value not supported")
        );
        assert_eq!(
            apply(&fp, json!({"hardware_concurrency": "64"})).unwrap_err(),
            error("fingerprint_config.hardware_concurrency", "value not supported")
        );
    }

    #[test]
    fn webgl_metadata() {
        let fp = base();
        let gpu = host_gpu();
        let config = json!({"webgl": "2", "WebGL_Config": {"Unmasked_Vendor": gpu.vendor, "unmasked_renderer": gpu.renderer}});
        let custom = apply(&fp, config).unwrap().fingerprint;
        assert_eq!(custom.gpu, gpu);
        assert!(custom.manual.contains(&Field::Gpu));
        let random = apply(&custom, json!({"webgl": "3"})).unwrap().fingerprint;
        assert!(!random.manual.contains(&Field::Gpu));
        assert_eq!(
            apply(&fp, json!({"webgl": "2"})).unwrap_err(),
            error("fingerprint_config.webgl_config", "unmasked_vendor and unmasked_renderer are required")
        );
        assert_eq!(apply(&fp, json!({"webgl": "5"})).unwrap_err(), error("fingerprint_config.webgl", "value not supported"));
        let foreign = foreign_gpu();
        assert_eq!(
            apply(&fp, json!({"webgl": 2, "webgl_config": {"unmasked_vendor": foreign.vendor, "unmasked_renderer": foreign.renderer}}))
                .unwrap_err(),
            error("fingerprint_config.webgl_config", "GPU does not match this Mac")
        );
        let unchanged = apply(&fp, json!({"webgl_config": {"unmasked_vendor": "x", "unmasked_renderer": "y"}})).unwrap();
        assert_eq!(unchanged.fingerprint.gpu, fp.gpu, "webgl_config без webgl=2 не действует");
    }

    #[test]
    fn only_mac_chrome_user_agents() {
        let fp = base();
        assert!(apply(&fp, json!({"ua": MAC_CHROME})).is_ok());
        let unsupported = |field: &str| error(field, "only macOS Chrome profiles are supported");
        for ua in [WIN_CHROME, MAC_EDGE, MAC_SAFARI] {
            assert_eq!(apply(&fp, json!({"ua": ua})).unwrap_err(), unsupported("fingerprint_config.ua"), "{ua}");
        }
        let windows = json!({"random_ua": {"ua_browser": ["chrome"], "ua_version": ["80"], "ua_system_version": ["Windows 10"]}});
        assert_eq!(apply(&fp, windows).unwrap_err(), unsupported("fingerprint_config.random_ua"));
        let firefox = json!({"random_ua": {"ua_browser": ["firefox"]}});
        assert_eq!(apply(&fp, firefox).unwrap_err(), unsupported("fingerprint_config.random_ua"));
        let mixed = json!({"random_ua": {"UA_Browser": ["Chrome"], "ua_system_version": ["Windows 10", "Mac OS X 13"]}});
        assert!(apply(&fp, mixed).is_ok());
        let ua_wins = json!({"ua": MAC_CHROME, "random_ua": {"ua_system_version": ["Windows 10"]}});
        assert!(apply(&fp, ua_wins).is_ok(), "свой ua важнее random_ua, как у AdsPower");
        assert!(apply(&fp, json!({"random_ua": {}})).is_ok());
        assert!(apply(&fp, json!({"random_ua": {"ua_browser": [], "ua_system_version": []}})).is_ok(), "пустой список — любой");
    }

    #[test]
    fn applied_fields_must_be_scalars() {
        let fp = base();
        let must_be_string = |name: &str| error(&format!("fingerprint_config.{name}"), "must be a string");
        // Windows UA в массиве не проходит мимо проверки «только macOS».
        assert_eq!(apply(&fp, json!({"ua": [WIN_CHROME]})).unwrap_err(), must_be_string("ua"));
        let wrong: [(&str, Value); 12] = [
            ("ua", json!({"value": MAC_CHROME})),
            ("automatic_timezone", json!(["0"])),
            ("timezone", json!(["Europe/Berlin"])),
            ("language_switch", json!({"on": 1})),
            ("screen_resolution", json!(["1920_1080"])),
            ("hardware_concurrency", json!([8])),
            ("device_memory", json!({"gb": 8})),
            ("webgl", json!(["2"])),
            ("canvas", json!([0])),
            ("webgl_image", json!({"x": 0})),
            ("audio", json!([0])),
            ("client_rects", json!([0])),
        ];
        for (name, value) in wrong {
            let mut config = Map::new();
            config.insert(name.to_string(), value);
            let err = apply_fingerprint_config(&fp, &Map::new(), &config, Arch::host()).unwrap_err();
            assert_eq!(err, must_be_string(name), "{name}");
        }
        assert_eq!(
            apply(&fp, json!({"automatic_timezone": "0", "timezone": ["Europe/Berlin"]})).unwrap_err(),
            must_be_string("timezone")
        );
        // Числа и `null` (нет значения) — как раньше; массивы и объекты там, где они положены, — не ошибка.
        assert!(apply(&fp, json!({"hardware_concurrency": 8, "automatic_timezone": 1, "ua": null})).is_ok());
        assert!(apply(&fp, json!({"language_switch": "0", "language": ["de-DE"], "random_ua": {"ua_browser": ["chrome"]}})).is_ok());
    }

    #[test]
    fn noise_is_off_only_when_all_four_are_zero() {
        let fp = base();
        let off = apply(&fp, json!({"canvas": "0", "webgl_image": "0", "audio": 0, "client_rects": "0"})).unwrap().fingerprint;
        assert!(!off.noise);
        assert!(off.manual.contains(&Field::Noise));
        let three = apply(&off, json!({"canvas": "0", "webgl_image": "0", "audio": "0"})).unwrap().fingerprint;
        assert!(three.noise, "четвёртое не передано — шум включён");
        assert!(apply(&off, json!({"canvas": "1"})).unwrap().fingerprint.noise);
        assert!(!apply(&off, json!({"fonts": ["all"]})).unwrap().fingerprint.noise, "без полей шума — как было");
        assert_eq!(apply(&fp, json!({"canvas": "x"})).unwrap_err(), error("fingerprint_config.canvas", "must be 0 or 1"));
    }

    #[test]
    fn webrtc_and_unapplied_fields_are_stored() {
        let mut fp = base();
        fp.webrtc = WebRtc::Manual { ip: "203.0.113.7".into() };
        fp.manual.push(Field::Webrtc);
        let extra = map(json!({"fonts": ["Arial"], "do_not_track": "true"}));
        let config = map(json!({"webrtc": "disabled", "fonts": ["all"], "flash": "allow", "Mac_Address_Config": {"model": "1"}, "nonsense": 1}));
        let mapped = apply_fingerprint_config(&fp, &extra, &config, Arch::host()).unwrap();
        assert_eq!(mapped.fingerprint.webrtc, WebRtc::Auto, "WebRTC всегда отдаёт IP шлюза");
        assert!(!mapped.fingerprint.manual.contains(&Field::Webrtc));
        assert_eq!(
            Value::Object(mapped.extra),
            json!({"fonts": ["all"], "do_not_track": "true", "webrtc": "disabled", "flash": "allow", "mac_address_config": {"model": "1"}})
        );
    }

    /// AdsPower: `location_switch` «0» — свои координаты (`latitude`/`longitude` верхнего уровня), «1» — по IP (умолчание).
    #[test]
    fn location_follows_adspower_semantics() {
        let fp = base();
        let apply_to = |from: &Fingerprint, extra: &Map<String, Value>, v: Value| apply_fingerprint_config(from, extra, &map(v), Arch::host());
        // Умолчание AdsPower принимается; `location` — разрешение, хранится и возвращается строкой.
        let default = apply_to(&fp, &Map::new(), json!({"location_switch": "1", "location": "ask", "accuracy": "100"})).unwrap();
        assert_eq!(default.fingerprint.location, Location::Off);
        let config = fingerprint_config(&default.fingerprint, &default.extra, "150.0.0.0");
        assert_eq!((&config["location_switch"], &config["location"], &config["accuracy"]), (&json!("1"), &json!("ask"), &json!("100")));
        assert!(config.get("latitude").is_none() && config.get("longitude").is_none());
        // Правка с умолчанием не ломается и сбрасывает ручные координаты.
        // Число и строка; координаты не уходят в api_extra.
        let manual = apply_to(&fp, &Map::new(), json!({"location_switch": "0", "latitude": "52.52", "longitude": 13.405})).unwrap();
        assert_eq!(manual.fingerprint.location, Location::Manual { lat: 52.52, lon: 13.405 });
        assert!(manual.extra.get("latitude").is_none() && manual.extra.get("longitude").is_none());
        let config = fingerprint_config(&manual.fingerprint, &manual.extra, "150.0.0.0");
        assert_eq!((&config["location_switch"], &config["latitude"], &config["longitude"]), (&json!("0"), &json!("52.52"), &json!("13.405")));
        let back = apply_to(&manual.fingerprint, &manual.extra, json!({"location_switch": "1", "location": "allow"})).unwrap();
        assert_eq!(back.fingerprint.location, Location::Off, "«1» — координаты игнорируются");
        assert_eq!(back.extra["location"], json!("allow"));
        // Без location_switch ничего не меняется.
        let same = apply_to(&manual.fingerprint, &manual.extra, json!({"location": "block"})).unwrap();
        assert_eq!(same.fingerprint.location, manual.fingerprint.location);
        // «0» без координат или с неверными — ошибка с именем поля.
        for bad in [json!({"location_switch": "0"}), json!({"location_switch": "0", "latitude": "1"}), json!({"location_switch": "0", "latitude": "x", "longitude": "2"})] {
            let e = apply_to(&fp, &Map::new(), bad).unwrap_err();
            assert!(e.field.ends_with("latitude") || e.field.ends_with("longitude"), "{e:?}");
        }
        let e = apply_to(&fp, &Map::new(), json!({"location_switch": "0", "latitude": 91, "longitude": 0})).unwrap_err();
        assert!(e.field.ends_with("latitude"), "{e:?}");
        let e = apply_to(&fp, &Map::new(), json!({"location_switch": "0", "latitude": 0, "longitude": 181})).unwrap_err();
        assert!(e.field.ends_with("longitude"), "{e:?}");
    }

    #[test]
    fn config_for_responses() {
        let mut fp = base();
        fp.timezone = Setting::Manual { value: "Europe/Berlin".into() };
        fp.language = Setting::Manual { value: "de-DE".into() };
        fp.screen = Screen { width: 1920, height: 1080 };
        fp.hardware_concurrency = 8;
        fp.device_memory = 16;
        fp.gpu = host_gpu();
        fp.noise = false;
        let extra = map(json!({"fonts": ["all"], "webrtc": "disabled"}));
        let gpu = host_gpu();
        assert_eq!(
            Value::Object(fingerprint_config(&fp, &extra, VERSION)),
            json!({
                "location_switch": "1",
                "automatic_timezone": "0", "timezone": "Europe/Berlin",
                "language_switch": "0", "language": ["de-DE", "de"],
                "screen_resolution": "1920_1080", "hardware_concurrency": "8", "device_memory": "16",
                "webgl": "2", "webgl_config": {"unmasked_vendor": gpu.vendor, "unmasked_renderer": gpu.renderer},
                "webrtc": "disabled", "ua": chrome_mac_ua(VERSION),
                "canvas": "0", "webgl_image": "0", "audio": "0", "client_rects": "0",
                "fonts": ["all"]
            })
        );
        let auto = fingerprint_config(&base(), &Map::new(), VERSION);
        assert_eq!((auto["automatic_timezone"].clone(), auto["timezone"].clone()), (json!("1"), json!("")));
        assert_eq!((auto["language_switch"].clone(), auto["language"].clone()), (json!("1"), json!([])));
        assert_eq!((auto["webrtc"].clone(), auto["canvas"].clone()), (json!("proxy"), json!("1")));
    }

    #[test]
    fn config_round_trips_through_apply() {
        let mut fp = base();
        fp.timezone = Setting::Manual { value: "Asia/Tokyo".into() };
        fp.language = Setting::Manual { value: "ja-JP".into() };
        fp.screen = Screen { width: 2560, height: 1440 };
        fp.gpu = host_gpu();
        fp.noise = false;
        let extra = map(json!({"fonts": ["all"], "webrtc": "forward"}));
        let config = fingerprint_config(&fp, &extra, VERSION);
        let back = apply_fingerprint_config(&base(), &Map::new(), &config, Arch::host()).unwrap();
        let got = back.fingerprint;
        assert_eq!(
            (&got.timezone, &got.language, got.screen, got.hardware_concurrency, got.device_memory, &got.gpu, got.noise),
            (&fp.timezone, &fp.language, fp.screen, fp.hardware_concurrency, fp.device_memory, &fp.gpu, fp.noise)
        );
        assert_eq!(Value::Object(back.extra), json!({"fonts": ["all"], "webrtc": "forward"}));
    }

    #[test]
    fn chrome_ua_uses_the_build_major_version() {
        assert_eq!(
            chrome_mac_ua(VERSION),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/145.0.0.0 Safari/537.36"
        );
        assert!(chrome_mac_ua(VERSION).contains("Chrome/145.0.0.0"));
    }

    #[test]
    fn user_proxy_config_is_parsed() {
        let full = parse_user_proxy_config(&map(json!({
            "proxy_soft": "other", "Proxy_Type": "SOCKS5", "proxy_host": "gw.example.net", "proxy_port": "1080",
            "proxy_user": "alice", "proxy_password": " p@ss "
        })))
        .unwrap()
        .unwrap();
        assert_eq!((full.kind, full.host.as_str(), full.port), (ProxyKind::Socks5, "gw.example.net", 1080));
        assert_eq!(full.username.as_deref(), Some("alice"));
        assert_eq!(full.password.as_ref().map(Secret::expose), Some(" p@ss "), "пароль не обрезается");
        let numeric = parse_user_proxy_config(&map(json!({"proxy_soft": "brightdata", "proxy_type": "http", "proxy_host": "h", "proxy_port": 8080})))
            .unwrap()
            .unwrap();
        assert_eq!((numeric.kind, numeric.port, numeric.username.is_none(), numeric.password.is_none()), (ProxyKind::Http, 8080, true, true));
        assert!(parse_user_proxy_config(&map(json!({"proxy_soft": "no_proxy"}))).unwrap().is_none());
        assert!(parse_user_proxy_config(&map(json!({"proxy_soft": "NO_PROXY", "proxy_host": "ignored"}))).unwrap().is_none());
    }

    #[test]
    fn user_proxy_config_errors() {
        let check = |config: Value, field: &str, reason: &'static str| {
            assert_eq!(parse_user_proxy_config(&map(config.clone())).err(), Some(error(field, reason)), "{config}");
        };
        check(json!({"proxy_soft": "other", "proxy_type": "ssh", "proxy_host": "h", "proxy_port": "22"}), "user_proxy_config.proxy_type", "unsupported proxy type");
        check(json!({"proxy_soft": "other", "proxy_host": "h", "proxy_port": "22"}), "user_proxy_config.proxy_type", "is required");
        check(json!({"proxy_type": "http", "proxy_port": "80"}), "user_proxy_config.proxy_host", "is required");
        check(json!({"proxy_type": "http", "proxy_host": "h", "proxy_port": "0"}), "user_proxy_config.proxy_port", "must be 1..65535");
        check(json!({"proxy_type": "http", "proxy_host": "h", "proxy_port": "x"}), "user_proxy_config.proxy_port", "must be 1..65535");
        check(
            json!({"proxy_type": "http", "proxy_host": "h", "proxy_port": "80", "proxy_password": "p"}),
            "user_proxy_config.proxy_user",
            "is required with proxy_password",
        );
    }

    #[test]
    fn user_proxy_config_for_responses() {
        let creds = ProxyCredentials {
            id: 3,
            kind: ProxyKind::Http,
            host: "gw.example.net".into(),
            port: 8080,
            username: Some("alice".into()),
            password: Some(Secret::new("hunter2")),
        };
        assert_eq!(
            user_proxy_config(Some(&creds)),
            json!({
                "proxy_soft": "other", "proxy_type": "http", "proxy_host": "gw.example.net", "proxy_port": "8080",
                "proxy_user": "alice", "proxy_password": "hunter2"
            })
        );
        assert_eq!(user_proxy_config(None), json!({"proxy_soft": "no_proxy"}));
    }

    #[test]
    fn mapping_errors_become_api_messages() {
        let failure = ApiFailure::from(error("fingerprint_config.ua", "only macOS Chrome profiles are supported"));
        assert_eq!(failure.0, "fingerprint_config.ua: only macOS Chrome profiles are supported");
    }
}
