use super::locale::language_for_country;
use super::model::{Fingerprint, Setting, WebRtc};

/// Геоданные шлюза, нужные отпечатку (приходят из `relay::geo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeoFacts<'a> {
    pub ip: &'a str,
    pub country: &'a str,
    pub timezone: &'a str,
}

/// Отпечаток с заполненными полями `auto` — то, что получает движок.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub fingerprint: Fingerprint,
    pub timezone: String,
    pub language: String,
    pub webrtc_ip: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    #[error("для поля {0} нужны геоданные шлюза")]
    NeedsGeo(&'static str),
}

/// Геоданные нужны, если таймзона или язык в режиме «по IP шлюза».
pub fn needs_geo(fp: &Fingerprint) -> bool {
    matches!(fp.timezone, Setting::Auto) || matches!(fp.language, Setting::Auto)
}

pub fn resolve(fp: &Fingerprint, geo: Option<GeoFacts<'_>>) -> Result<Resolved, ResolveError> {
    let timezone = match &fp.timezone {
        Setting::Manual { value } => value.clone(),
        Setting::Auto => geo.map(|g| g.timezone.to_string()).ok_or(ResolveError::NeedsGeo("timezone"))?,
    };
    let language = match &fp.language {
        Setting::Manual { value } => value.clone(),
        Setting::Auto => geo
            .map(|g| language_for_country(g.country).to_string())
            .ok_or(ResolveError::NeedsGeo("language"))?,
    };
    let webrtc_ip = match &fp.webrtc {
        WebRtc::Manual { ip } => Some(ip.clone()),
        WebRtc::Auto => geo.map(|g| g.ip.to_string()),
    };
    Ok(Resolved { fingerprint: fp.clone(), timezone, language, webrtc_ip })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::validate::tests::apple_fp;

    const GEO: GeoFacts<'static> = GeoFacts { ip: "203.0.113.24", country: "DE", timezone: "Europe/Berlin" };

    #[test]
    fn auto_fields_come_from_geo() {
        let r = resolve(&apple_fp(), Some(GEO)).unwrap();
        assert_eq!(r.timezone, "Europe/Berlin");
        assert_eq!(r.language, "de-DE");
        assert_eq!(r.webrtc_ip.as_deref(), Some("203.0.113.24"));
    }

    #[test]
    fn manual_fields_win_and_skip_geo() {
        let mut fp = apple_fp();
        fp.timezone = Setting::Manual { value: "Asia/Tokyo".into() };
        fp.language = Setting::Manual { value: "ja-JP".into() };
        assert!(!needs_geo(&fp));
        let r = resolve(&fp, None).unwrap();
        assert_eq!((r.timezone.as_str(), r.language.as_str()), ("Asia/Tokyo", "ja-JP"));
        assert_eq!(r.webrtc_ip, None);
        assert_eq!(resolve(&fp, Some(GEO)).unwrap().timezone, "Asia/Tokyo");
    }

    #[test]
    fn auto_without_geo_is_an_error() {
        assert!(needs_geo(&apple_fp()));
        assert_eq!(resolve(&apple_fp(), None), Err(ResolveError::NeedsGeo("timezone")));
    }
}
