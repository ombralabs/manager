/// Названия стран, как их отдаёт ipapi.is без ключа, и частые варианты → код ISO 3166-1.
const NAMES: &[(&str, &str)] = &[
    ("argentina", "AR"), ("armenia", "AM"), ("australia", "AU"), ("austria", "AT"), ("azerbaijan", "AZ"),
    ("belarus", "BY"), ("belgium", "BE"), ("brazil", "BR"), ("bulgaria", "BG"), ("canada", "CA"),
    ("chile", "CL"), ("china", "CN"), ("colombia", "CO"), ("croatia", "HR"), ("cyprus", "CY"),
    ("czechia", "CZ"), ("czech republic", "CZ"), ("denmark", "DK"), ("egypt", "EG"), ("estonia", "EE"),
    ("finland", "FI"), ("france", "FR"), ("georgia", "GE"), ("germany", "DE"), ("greece", "GR"),
    ("hong kong", "HK"), ("hungary", "HU"), ("india", "IN"), ("indonesia", "ID"), ("iran", "IR"),
    ("ireland", "IE"), ("israel", "IL"), ("italy", "IT"), ("japan", "JP"), ("kazakhstan", "KZ"),
    ("korea", "KR"), ("south korea", "KR"), ("republic of korea", "KR"), ("latvia", "LV"), ("lithuania", "LT"),
    ("luxembourg", "LU"), ("malaysia", "MY"), ("mexico", "MX"), ("moldova", "MD"), ("republic of moldova", "MD"),
    ("netherlands", "NL"), ("the netherlands", "NL"), ("new zealand", "NZ"), ("norway", "NO"), ("peru", "PE"),
    ("philippines", "PH"), ("poland", "PL"), ("portugal", "PT"), ("romania", "RO"), ("russia", "RU"),
    ("russian federation", "RU"), ("saudi arabia", "SA"), ("serbia", "RS"), ("singapore", "SG"), ("slovakia", "SK"),
    ("slovenia", "SI"), ("south africa", "ZA"), ("spain", "ES"), ("sweden", "SE"), ("switzerland", "CH"),
    ("taiwan", "TW"), ("thailand", "TH"), ("turkey", "TR"), ("türkiye", "TR"), ("ukraine", "UA"),
    ("united arab emirates", "AE"), ("united kingdom", "GB"), ("great britain", "GB"), ("united states", "US"),
    ("united states of america", "US"), ("uzbekistan", "UZ"), ("vietnam", "VN"), ("viet nam", "VN"),
];

pub fn code_for_name(name: &str) -> Option<&'static str> {
    let trimmed = name.trim();
    if trimmed.len() == 2 && trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
        return NAMES.iter().map(|(_, code)| *code).find(|code| code.eq_ignore_ascii_case(trimmed));
    }
    let lower = trimmed.to_lowercase();
    NAMES.iter().find(|(n, _)| *n == lower).map(|(_, code)| *code)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_aliases() {
        assert_eq!(code_for_name("Germany"), Some("DE"));
        assert_eq!(code_for_name("russian federation"), Some("RU"));
        assert_eq!(code_for_name("Russia"), Some("RU"));
        assert_eq!(code_for_name("United States of America"), Some("US"));
        assert_eq!(code_for_name("Türkiye"), Some("TR"));
        assert_eq!(code_for_name("DE"), Some("DE"));
        assert_eq!(code_for_name("Atlantis"), None);
    }
}
