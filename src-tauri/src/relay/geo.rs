use super::RelayHandle;
use super::countries::code_for_name;
use serde_json::Value;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    IpWhoIs,
    IpapiIs,
    Ip2Location,
    IpApi,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GeoInfo {
    pub ip: String,
    /// Код страны ISO 3166-1, заглавными; `ZZ` — страна неизвестна (язык тогда `en-US`).
    pub country: String,
    /// Имя таймзоны IANA.
    pub timezone: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GeoError {
    #[error("прокси недоступен")]
    ProxyUnavailable,
    #[error("сервисы геоданных недоступны")]
    Unavailable,
    #[error("неверный ответ сервиса геоданных")]
    BadResponse,
}

pub fn parse(provider: Provider, body: &str) -> Result<GeoInfo, GeoError> {
    let json: Value = serde_json::from_str(body).map_err(|_| GeoError::BadResponse)?;
    let text = |v: &Value| v.as_str().map(str::to_string);
    let (ip, country, timezone) = match provider {
        Provider::IpWhoIs => {
            if json["success"].as_bool() != Some(true) {
                return Err(GeoError::BadResponse);
            }
            (text(&json["ip"]), text(&json["country_code"]), text(&json["timezone"]["id"]))
        }
        Provider::IpapiIs => {
            let location = &json["location"];
            // Неизвестное или отсутствующее название страны — не отказ: IP и таймзона получены, язык будет en-US.
            let country = text(&location["country_code"]).unwrap_or_else(|| {
                json["country"].as_str().and_then(code_for_name).unwrap_or("ZZ").to_string()
            });
            let country = Some(country);
            let timezone = text(&location["timezone"]).or_else(|| text(&json["timezone"]));
            (text(&json["ip"]), country, timezone)
        }
        Provider::Ip2Location => {
            if !json["error"].is_null() { return Err(GeoError::BadResponse); }
            // Бесплатный API возвращает UTC-смещение, а браузеру нужна зона IANA с правилами DST.
            // Определяем её локально по координатам того же ответа, без запроса к другому чекеру.
            let timezone = text(&json["time_zone_info"]["olson"])
                .or_else(|| text(&json["time_zone"]).filter(|tz| crate::fingerprint::locale::is_known_timezone(tz)))
                .or_else(|| timezone_at(&json));
            (text(&json["ip"]), text(&json["country_code"]), timezone)
        }
        Provider::IpApi => {
            if json["status"].as_str() != Some("success") { return Err(GeoError::BadResponse); }
            (text(&json["query"]), text(&json["countryCode"]), text(&json["timezone"]))
        }
    };
    let (Some(ip), Some(country), Some(timezone)) = (ip, country, timezone) else {
        return Err(GeoError::BadResponse);
    };
    let country = country.to_ascii_uppercase();
    if ip.parse::<std::net::IpAddr>().is_err()
        || country.len() != 2
        || !crate::fingerprint::locale::is_known_timezone(&timezone)
    {
        return Err(GeoError::BadResponse);
    }
    Ok(GeoInfo { ip, country, timezone })
}

fn timezone_at(json: &Value) -> Option<String> {
    static FINDER: std::sync::LazyLock<tzf_rs::EmbeddedFinder> = std::sync::LazyLock::new(tzf_rs::EmbeddedFinder::new);
    let lat = json["latitude"].as_f64()?;
    let lon = json["longitude"].as_f64()?;
    if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) { return None; }
    let tz = FINDER.get_tz_name(lon, lat);
    (!tz.is_empty()).then(|| tz.to_string())
}

/// Геоданные внешнего IP: запрос идёт через прослойку с выбранным подключением профиля.
/// Кэша нет: запрос при каждом запуске заодно проверяет, что прокси жив (спецификация 2А, «Шлюз»).
pub struct GeoClient {
    endpoints: Vec<(Provider, String)>,
}

impl Default for GeoClient {
    fn default() -> Self {
        Self::new()
    }
}

impl GeoClient {
    /// «Авто» использует ipwho.is и ipapi.is. Остальные сервисы вызываются только при явном выборе.
    pub fn new() -> Self {
        Self::with_endpoints(vec![
            (Provider::IpWhoIs, "https://ipwho.is/".to_string()),
            (Provider::IpapiIs, "https://api.ipapi.is/".to_string()),
            // https://www.ip2location.io/ip2location-documentation — без `ip` определяется IP запроса.
            (Provider::Ip2Location, "https://api.ip2location.io/".to_string()),
            // https://ip-api.com/docs/api:json — публичный endpoint доступен по HTTP.
            (Provider::IpApi, "http://ip-api.com/json/?fields=status,message,query,countryCode,timezone".to_string()),
        ])
    }

    pub fn with_endpoints(endpoints: Vec<(Provider, String)>) -> Self {
        Self { endpoints }
    }

    /// Каждый вызов идёт через подключение профиля. Если сервис не ответил, учитываются сбои прокси за время вызова.
    pub async fn lookup(&self, relay: &RelayHandle) -> Result<GeoInfo, GeoError> {
        self.lookup_with(relay, crate::network::IpChecker::Auto).await
    }

    pub async fn lookup_with(&self, relay: &RelayHandle, checker: crate::network::IpChecker) -> Result<GeoInfo, GeoError> {
        let failures_before = relay.stats().upstream_failures;
        let proxy = reqwest::Proxy::all(format!("http://{}", relay.addr())).map_err(|_| GeoError::Unavailable)?;
        let client = reqwest::Client::builder().proxy(proxy).timeout(TIMEOUT).build().map_err(|_| GeoError::Unavailable)?;
        for (provider, url) in &self.endpoints {
            use crate::network::IpChecker;
            if !matches!((checker, provider),
                (IpChecker::Auto, Provider::IpWhoIs | Provider::IpapiIs)
                | (IpChecker::IpWhoIs, Provider::IpWhoIs) | (IpChecker::IpapiIs, Provider::IpapiIs)
                | (IpChecker::Ip2Location, Provider::Ip2Location) | (IpChecker::IpApi, Provider::IpApi)
            ) { continue; }
            let Ok(response) = client.get(url).send().await else { continue };
            if !response.status().is_success() {
                continue;
            }
            let Ok(body) = response.text().await else { continue };
            if let Ok(info) = parse(*provider, &body) {
                return Ok(info);
            }
        }
        if relay.stats().upstream_failures > failures_before {
            Err(GeoError::ProxyUnavailable)
        } else {
            Err(GeoError::Unavailable)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::test_support::{closed_port, spawn_http_proxy, spawn_origin};
    use crate::relay::{UpstreamConfig, UpstreamKind, start};

    const IPWHOIS: &str = r#"{"ip":"203.0.113.24","success":true,"type":"IPv4","country":"Germany","country_code":"DE",
      "timezone":{"id":"Europe/Berlin","abbr":"CEST","utc":"+02:00"}}"#;
    const IPAPI_ANON: &str = r#"{"ip":"203.0.113.24","is_bogon":false,"city":"Frankfurt am Main","country":"Germany","timezone":"Europe/Berlin"}"#;
    const IPAPI_KEYED: &str = r#"{"ip":"203.0.113.24","location":{"country":"Germany","country_code":"DE","timezone":"Europe/Berlin"}}"#;
    const IP2LOCATION: &str = r#"{"ip":"203.0.113.24","country_code":"DE","latitude":52.52,"longitude":13.405,"time_zone":"+02:00"}"#;
    const IP_API: &str = r#"{"status":"success","query":"203.0.113.24","countryCode":"DE","timezone":"Europe/Berlin"}"#;

    fn berlin() -> GeoInfo {
        GeoInfo { ip: "203.0.113.24".into(), country: "DE".into(), timezone: "Europe/Berlin".into() }
    }

    #[test]
    fn parses_both_services() {
        assert_eq!(parse(Provider::IpWhoIs, IPWHOIS), Ok(berlin()));
        assert_eq!(parse(Provider::IpapiIs, IPAPI_ANON), Ok(berlin()));
        assert_eq!(parse(Provider::IpapiIs, IPAPI_KEYED), Ok(berlin()));
        assert_eq!(parse(Provider::IpWhoIs, r#"{"success":false,"message":"Reserved range"}"#), Err(GeoError::BadResponse));
        assert_eq!(
            parse(Provider::IpWhoIs, &IPWHOIS.replace("Europe/Berlin", "Mars/Olympus")),
            Err(GeoError::BadResponse)
        );
        assert_eq!(parse(Provider::IpapiIs, &IPAPI_ANON.replace("Germany", "Atlantis")).map(|g| g.country), Ok("ZZ".to_string()));
    }

    #[test]
    fn extra_checkers_parse_coordinates_and_iana_zones_and_reject_failed_results() {
        assert_eq!(parse(Provider::Ip2Location, IP2LOCATION), Ok(berlin()));
        assert_eq!(parse(Provider::IpApi, IP_API), Ok(berlin()));
        assert_eq!(parse(Provider::Ip2Location, r#"{"error":{"error_code":10000}}"#), Err(GeoError::BadResponse));
        assert_eq!(parse(Provider::IpApi, &IP_API.replace("success", "fail")), Err(GeoError::BadResponse));
        assert_eq!(parse(Provider::IpApi, &IP_API.replace("Europe/Berlin", "+02:00")), Err(GeoError::BadResponse));
        assert_eq!(parse(Provider::Ip2Location, &IP2LOCATION.replace("52.52", "95.0")), Err(GeoError::BadResponse));
        // Одинаковое UTC-смещение не задаёт правила сезонного перевода часов.
        let sydney = IP2LOCATION.replace("52.52", "-33.8688").replace("13.405", "151.2093").replace("+02:00", "+11:00");
        assert_eq!(parse(Provider::Ip2Location, &sydney).unwrap().timezone, "Australia/Sydney");
    }

    #[tokio::test]
    async fn extra_checkers_are_selected_without_requests_to_other_services() {
        let relay = relay_via_http_proxy().await;
        for (provider, checker, body) in [
            (Provider::Ip2Location, crate::network::IpChecker::Ip2Location, IP2LOCATION),
            (Provider::IpApi, crate::network::IpChecker::IpApi, IP_API),
        ] {
            let (other, other_hits) = spawn_origin(200, IPWHOIS).await;
            let (selected, hits) = spawn_origin(200, body).await;
            let geo = GeoClient::with_endpoints(vec![(Provider::IpWhoIs, format!("http://{other}/")), (provider, format!("http://{selected}/"))]);
            assert_eq!(geo.lookup_with(&relay, checker).await, Ok(berlin()));
            assert!(other_hits.lock().unwrap().is_empty());
            assert_eq!(hits.lock().unwrap().len(), 1);
        }
        relay.shutdown().await;
    }

    async fn relay_via_http_proxy() -> crate::relay::RelayHandle {
        let (proxy, _) = spawn_http_proxy(None).await;
        start(
            UpstreamConfig { proxy_id: 1, kind: UpstreamKind::Http, host: "127.0.0.1".into(), port: proxy.port(), credentials: None, tls_roots: None },
            None,
        )
        .await
        .unwrap()
    }

    fn client(primary: std::net::SocketAddr, fallback: std::net::SocketAddr) -> GeoClient {
        GeoClient::with_endpoints(vec![
            (Provider::IpWhoIs, format!("http://{primary}/")),
            (Provider::IpapiIs, format!("http://{fallback}/")),
        ])
    }

    #[tokio::test]
    async fn every_lookup_goes_through_the_proxy() {
        let (primary, hits) = spawn_origin(200, IPWHOIS).await;
        let (fallback, _) = spawn_origin(200, IPAPI_ANON).await;
        let relay = relay_via_http_proxy().await;
        let geo = client(primary, fallback);
        assert_eq!(geo.lookup(&relay).await, Ok(berlin()));
        assert_eq!(geo.lookup(&relay).await, Ok(berlin()));
        assert_eq!(hits.lock().unwrap().len(), 2, "кэша нет: каждый запуск заодно проверяет, что прокси жив");
    }

    #[tokio::test]
    async fn selected_checker_uses_only_that_service_through_the_connection() {
        let (primary, first) = spawn_origin(200, IPWHOIS).await;
        let (fallback, second) = spawn_origin(200, IPAPI_ANON).await;
        let relay = start(UpstreamConfig::direct(), None).await.unwrap();
        let geo = client(primary, fallback);
        assert_eq!(geo.lookup_with(&relay, crate::network::IpChecker::IpapiIs).await, Ok(berlin()));
        assert!(first.lock().unwrap().is_empty());
        assert_eq!(second.lock().unwrap().len(), 1);
        assert_eq!(geo.lookup_with(&relay, crate::network::IpChecker::IpWhoIs).await, Ok(berlin()));
        assert_eq!(first.lock().unwrap().len(), 1);
        relay.shutdown().await;
    }

    #[tokio::test]
    async fn a_selected_failed_checker_does_not_silently_fall_back() {
        let (primary, _) = spawn_origin(503, "{}").await;
        let (fallback, hits) = spawn_origin(200, IPAPI_ANON).await;
        let relay = start(UpstreamConfig::direct(), None).await.unwrap();
        let geo = client(primary, fallback);
        assert_eq!(geo.lookup_with(&relay, crate::network::IpChecker::IpWhoIs).await, Err(GeoError::Unavailable));
        assert!(hits.lock().unwrap().is_empty());
        assert_eq!(geo.lookup(&relay).await, Ok(berlin()));
        assert_eq!(hits.lock().unwrap().len(), 1);
        relay.shutdown().await;
    }

    #[tokio::test]
    async fn rate_limited_primary_falls_back() {
        let (primary, _) = spawn_origin(429, r#"{"success":false}"#).await;
        let (fallback, hits) = spawn_origin(200, IPAPI_ANON).await;
        let relay = relay_via_http_proxy().await;
        assert_eq!(client(primary, fallback).lookup(&relay).await, Ok(berlin()));
        assert_eq!(hits.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn both_services_down_with_working_proxy_is_unavailable() {
        let (primary, _) = spawn_origin(500, "{}").await;
        let (fallback, _) = spawn_origin(503, "{}").await;
        let relay = relay_via_http_proxy().await;
        assert_eq!(client(primary, fallback).lookup(&relay).await, Err(GeoError::Unavailable));
    }

    #[tokio::test]
    async fn dead_proxy_is_reported_as_proxy_unavailable() {
        let port = closed_port().await;
        let relay = start(
            UpstreamConfig { proxy_id: 1, kind: UpstreamKind::Http, host: "127.0.0.1".into(), port, credentials: None, tls_roots: None },
            None,
        )
        .await
        .unwrap();
        let (origin, _) = spawn_origin(200, IPWHOIS).await;
        assert_eq!(client(origin, origin).lookup(&relay).await, Err(GeoError::ProxyUnavailable));
    }
}
