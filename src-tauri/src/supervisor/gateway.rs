//! Внешний прокси профиля глазами прослойки и кнопка «Проверить».
use crate::relay::geo::{GeoClient, GeoError, GeoInfo};
use crate::relay::{self, Credentials, UpstreamConfig};
use crate::store::proxies::{ProxyCredentials, ProxyRecord};
use std::collections::HashSet;

/// Логин без пароля уходит с пустым паролем: форма такого не сохраняет, но старые записи могли остаться.
pub fn upstream_config(proxy: &ProxyCredentials) -> UpstreamConfig {
    let credentials = proxy.username.as_ref().filter(|u| !u.is_empty()).map(|username| Credentials {
        username: username.clone(),
        password: proxy.password.as_ref().map(|p| p.expose().to_string()).unwrap_or_default(),
    });
    UpstreamConfig {
        proxy_id: proxy.id,
        kind: proxy.kind.into(),
        host: proxy.host.clone(),
        port: proxy.port,
        credentials,
        tls_roots: None,
    }
}

/// Сколько прокси пробует один запуск (спецификация, «Шлюз»).
pub const MAX_GATEWAY_ATTEMPTS: usize = 3;

/// Кандидат в шлюз; `pinned` — это прокси, закреплённый за профилем.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    pub proxy_id: i64,
    pub pinned: bool,
}

/// Закреплённый, если он включён, свободен и не испробован в этом запуске;
/// иначе случайный (`random % n`) из включённых, свободных и неиспробованных — в порядке списка.
pub fn pick_proxy(
    pinned: Option<i64>,
    proxies: &[ProxyRecord],
    busy: &HashSet<i64>,
    tried: &HashSet<i64>,
    random: u64,
) -> Option<Pick> {
    let usable = |p: &&ProxyRecord| p.enabled && !busy.contains(&p.id) && !tried.contains(&p.id);
    if let Some(id) = pinned
        && proxies.iter().filter(usable).any(|p| p.id == id)
    {
        return Some(Pick { proxy_id: id, pinned: true });
    }
    let free: Vec<i64> = proxies.iter().filter(usable).map(|p| p.id).collect();
    if free.is_empty() {
        return None;
    }
    let index = (random % free.len() as u64) as usize;
    Some(Pick { proxy_id: free[index], pinned: false })
}

/// «Проверить»: отдельная временная прослойка без журнала; геоданные запрашиваются через прокси заново.
/// На отдельной прослойке чужие соединения профиля не путают класс ошибки.
pub async fn check(geo: &GeoClient, upstream: UpstreamConfig) -> Result<GeoInfo, GeoError> {
    check_with(geo, upstream, crate::network::IpChecker::Auto).await
}

pub async fn check_with(geo: &GeoClient, upstream: UpstreamConfig, checker: crate::network::IpChecker) -> Result<GeoInfo, GeoError> {
    let relay = relay::start(upstream, None).await.map_err(|_| GeoError::Unavailable)?;
    let result = geo.lookup_with(&relay, checker).await;
    relay.shutdown().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::UpstreamKind;
    use crate::store::proxies::ProxyKind;
    use crate::relay::geo::Provider;
    use crate::relay::test_support::{closed_port, spawn_failing_socks5, spawn_http_proxy, spawn_origin, spawn_status_proxy};
    use crate::store::secrets::Secret;

    const IPWHOIS: &str = r#"{"ip":"203.0.113.24","success":true,"country_code":"DE","timezone":{"id":"Europe/Berlin"}}"#;

    fn creds(kind: ProxyKind, port: u16, username: Option<&str>, password: Option<&str>) -> ProxyCredentials {
        ProxyCredentials {
            id: 7,
            kind,
            host: "127.0.0.1".into(),
            port,
            username: username.map(Into::into),
            password: password.map(Secret::new),
        }
    }

    fn geo_client(origin: std::net::SocketAddr) -> GeoClient {
        GeoClient::with_endpoints(vec![(Provider::IpWhoIs, format!("http://{origin}/"))])
    }

    fn proxy(id: i64, enabled: bool) -> ProxyRecord {
        ProxyRecord {
            id,
            kind: ProxyKind::Http,
            host: "127.0.0.1".into(),
            port: 8000 + id as u16,
            username: None,
            has_password: false,
            note: String::new(),
            enabled,
            check: None,
        }
    }

    fn ids(list: &[i64]) -> HashSet<i64> {
        list.iter().copied().collect()
    }

    #[test]
    fn usable_pinned_proxy_wins() {
        let all = [proxy(1, true), proxy(2, true), proxy(3, true)];
        assert_eq!(pick_proxy(Some(2), &all, &ids(&[]), &ids(&[]), 7), Some(Pick { proxy_id: 2, pinned: true }));
    }

    #[test]
    fn unusable_pinned_proxy_falls_back_to_a_free_one() {
        let all = [proxy(1, true), proxy(2, false), proxy(3, true), proxy(4, true)];
        // Закреплённый выключен — свободны 1, 3, 4.
        assert_eq!(pick_proxy(Some(2), &all, &ids(&[]), &ids(&[]), 0), Some(Pick { proxy_id: 1, pinned: false }));
        // Закреплённый занят — свободны 3, 4.
        assert_eq!(pick_proxy(Some(1), &all, &ids(&[1]), &ids(&[]), 0), Some(Pick { proxy_id: 3, pinned: false }));
        // Закреплённый уже испробован в этом запуске.
        assert_eq!(pick_proxy(Some(1), &all, &ids(&[]), &ids(&[1]), 1), Some(Pick { proxy_id: 4, pinned: false }));
        // Закреплённого больше нет в списке.
        assert_eq!(pick_proxy(Some(9), &all, &ids(&[]), &ids(&[]), 2), Some(Pick { proxy_id: 4, pinned: false }));
    }

    #[test]
    fn random_choice_is_spread_over_free_proxies() {
        let all = [proxy(1, true), proxy(2, true), proxy(3, true)];
        let picked: Vec<i64> =
            (0..6).map(|random| pick_proxy(None, &all, &ids(&[2]), &ids(&[]), random).unwrap().proxy_id).collect();
        assert_eq!(picked, [1, 3, 1, 3, 1, 3]);
    }

    #[test]
    fn nothing_usable_gives_none() {
        let all = [proxy(1, false), proxy(2, true)];
        assert_eq!(pick_proxy(Some(2), &all, &ids(&[2]), &ids(&[]), 5), None);
        assert_eq!(pick_proxy(None, &all, &ids(&[]), &ids(&[2]), 5), None);
        assert_eq!(pick_proxy(None, &[], &ids(&[]), &ids(&[]), 5), None);
    }

    /// К https-адресу reqwest идёт через прокси CONNECT-ом: так проверяется ответ самого прокси, а не сайта.
    fn https_geo() -> GeoClient {
        GeoClient::with_endpoints(vec![(Provider::IpWhoIs, "https://geo.ombra.test/".into())])
    }

    #[tokio::test]
    async fn proxy_answering_403_is_unavailable() {
        let proxy = spawn_status_proxy(403).await;
        let cfg = upstream_config(&creds(ProxyKind::Http, proxy.port(), None, None));
        assert_eq!(check(&https_geo(), cfg).await, Err(GeoError::ProxyUnavailable));
    }

    #[tokio::test]
    async fn socks_general_failure_is_unavailable() {
        let proxy = spawn_failing_socks5(1).await;
        let cfg = upstream_config(&creds(ProxyKind::Socks5, proxy.port(), None, None));
        assert_eq!(check(&https_geo(), cfg).await, Err(GeoError::ProxyUnavailable));
    }

    #[test]
    fn maps_kind_and_credentials() {
        let cfg = upstream_config(&creds(ProxyKind::Socks5, 1080, Some("user"), Some("pass")));
        assert_eq!((cfg.proxy_id, cfg.kind, cfg.port), (7, UpstreamKind::Socks5, 1080));
        assert_eq!(cfg.basic_auth().as_deref(), Some("Basic dXNlcjpwYXNz"));
        assert!(upstream_config(&creds(ProxyKind::Http, 1, None, None)).credentials.is_none());
        assert!(upstream_config(&creds(ProxyKind::Http, 1, Some(""), Some("x"))).credentials.is_none());
        let login_only = upstream_config(&creds(ProxyKind::Https, 1, Some("user"), None));
        assert_eq!(login_only.kind, UpstreamKind::Https);
        assert_eq!(login_only.basic_auth().as_deref(), Some("Basic dXNlcjo="));
    }

    #[tokio::test]
    async fn check_goes_through_proxy_every_time() {
        let (origin, hits) = spawn_origin(200, IPWHOIS).await;
        let (proxy, seen) = spawn_http_proxy(Some("dXNlcjpwYXNz")).await;
        let geo = geo_client(origin);
        let cfg = upstream_config(&creds(ProxyKind::Http, proxy.port(), Some("user"), Some("pass")));
        assert_eq!(check(&geo, cfg.clone()).await.unwrap().timezone, "Europe/Berlin");
        assert_eq!(check(&geo, cfg).await.unwrap().ip, "203.0.113.24");
        assert_eq!(hits.lock().unwrap().len(), 2, "каждая проверка идёт заново, мимо кэша");
        assert!(seen.lock().unwrap().iter().all(|h| h.contains("Proxy-Authorization: Basic dXNlcjpwYXNz")));
    }

    #[tokio::test]
    async fn dead_or_rejecting_proxy_is_proxy_unavailable() {
        let (origin, _) = spawn_origin(200, IPWHOIS).await;
        let geo = geo_client(origin);
        let dead = upstream_config(&creds(ProxyKind::Http, closed_port().await, None, None));
        assert_eq!(check(&geo, dead).await, Err(GeoError::ProxyUnavailable));
        let (proxy, _) = spawn_http_proxy(Some("dXNlcjpwYXNz")).await;
        let wrong = upstream_config(&creds(ProxyKind::Http, proxy.port(), Some("user"), Some("nope")));
        assert_eq!(check(&geo, wrong).await, Err(GeoError::ProxyUnavailable));
    }
}
