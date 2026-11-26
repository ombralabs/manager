/// Основной язык страны для режима «язык по IP шлюза». Неизвестная страна — `en-US`.
pub const COUNTRY_LANGUAGE: &[(&str, &str)] = &[
    ("AE", "ar-AE"), ("AR", "es-AR"), ("AT", "de-AT"), ("AU", "en-AU"), ("AZ", "az-AZ"),
    ("BE", "nl-BE"), ("BG", "bg-BG"), ("BR", "pt-BR"), ("BY", "ru-BY"), ("CA", "en-CA"),
    ("CH", "de-CH"), ("CL", "es-CL"), ("CN", "zh-CN"), ("CO", "es-CO"), ("CY", "el-CY"),
    ("CZ", "cs-CZ"), ("DE", "de-DE"), ("DK", "da-DK"), ("EE", "et-EE"), ("EG", "ar-EG"),
    ("ES", "es-ES"), ("FI", "fi-FI"), ("FR", "fr-FR"), ("GB", "en-GB"), ("GE", "ka-GE"),
    ("GR", "el-GR"), ("HK", "zh-HK"), ("HR", "hr-HR"), ("HU", "hu-HU"), ("ID", "id-ID"),
    ("IE", "en-IE"), ("IL", "he-IL"), ("IN", "en-IN"), ("IT", "it-IT"), ("JP", "ja-JP"),
    ("KR", "ko-KR"), ("KZ", "ru-KZ"), ("LT", "lt-LT"), ("LU", "fr-LU"), ("LV", "lv-LV"),
    ("MD", "ro-MD"), ("MX", "es-MX"), ("MY", "ms-MY"), ("NL", "nl-NL"), ("NO", "nb-NO"),
    ("NZ", "en-NZ"), ("PE", "es-PE"), ("PH", "en-PH"), ("PL", "pl-PL"), ("PT", "pt-PT"),
    ("RO", "ro-RO"), ("RS", "sr-RS"), ("RU", "ru-RU"), ("SA", "ar-SA"), ("SE", "sv-SE"),
    ("SG", "en-SG"), ("SI", "sl-SI"), ("SK", "sk-SK"), ("TH", "th-TH"), ("TR", "tr-TR"),
    ("TW", "zh-TW"), ("UA", "uk-UA"), ("US", "en-US"), ("UZ", "uz-UZ"), ("VN", "vi-VN"),
    ("ZA", "en-ZA"),
];

pub fn language_for_country(country: &str) -> &'static str {
    COUNTRY_LANGUAGE
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(country))
        .map(|(_, lang)| *lang)
        .unwrap_or("en-US")
}

/// Встроенная база IANA работает и без /usr/share/zoneinfo (Windows, минимальный Linux).
pub fn is_known_timezone(tz: &str) -> bool {
    // Поиск Jiff нечувствителен к регистру, а имя для ICU должно совпадать точно.
    jiff_tzdb::get(tz).is_some_and(|(name, _)| name == tz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown_countries() {
        assert_eq!(language_for_country("DE"), "de-DE");
        assert_eq!(language_for_country("us"), "en-US");
        assert_eq!(language_for_country("BR"), "pt-BR");
        assert_eq!(language_for_country("RU"), "ru-RU");
        assert_eq!(language_for_country("ZZ"), "en-US");
    }

    #[test]
    fn every_table_language_is_valid() {
        for (_, lang) in COUNTRY_LANGUAGE {
            assert!(crate::fingerprint::validate::is_bcp47(lang), "{lang}");
        }
    }

    #[test]
    fn known_and_unknown_zones() {
        for tz in ["Europe/Moscow", "Europe/Berlin", "Europe/Kyiv", "US/Pacific", "UTC", "Etc/GMT+3", "America/Argentina/Buenos_Aires"] {
            assert!(is_known_timezone(tz), "{tz}");
        }
        for tz in ["Mars/Olympus", "../etc/passwd", "/usr/share/zoneinfo/UTC", "zone.tab", "posixrules", "europe/moscow", "Europe/moscow", "EUROPE/MOSCOW", "Europe//Moscow", ""] {
            assert!(!is_known_timezone(tz), "{tz}");
        }
    }
}
