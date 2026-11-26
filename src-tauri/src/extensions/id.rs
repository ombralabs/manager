//! Идентификатор расширения как в Chromium (`components/crx_file/id_util.cc`): первые 16 байт SHA-256 от байтов
//! открытого ключа, каждая шестнадцатеричная цифра 0–f заменяется буквой a–p.
use sha2::{Digest, Sha256};

pub fn extension_id(key: &[u8]) -> String {
    id_from_hash(&Sha256::digest(key)[..16])
}

/// Байты хеша (16) — в буквы a–p; так же Chromium читает `crx_id` из заголовка CRX3.
pub fn id_from_hash(hash: &[u8]) -> String {
    hash.iter().flat_map(|b| [b >> 4, b & 0x0f]).map(|n| (b'a' + n) as char).collect()
}

pub fn is_extension_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// Идентификатор из ввода пользователя: сам идентификатор или ссылка Chrome Web Store (новая и старая).
pub fn store_id(input: &str) -> Option<String> {
    let input = input.trim();
    if is_extension_id(input) {
        return Some(input.to_string());
    }
    let url = input.split(['?', '#']).next()?;
    let rest = url
        .strip_prefix("https://chromewebstore.google.com/detail/")
        .or_else(|| url.strip_prefix("https://chrome.google.com/webstore/detail/"))?;
    rest.split('/').find(|part| is_extension_id(part)).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_matches_chromium_vectors() {
        // components/crx_file/id_util_unittest.cc
        assert_eq!(extension_id(b"test"), "jpignaibiiemhngfjkcpokkamffknabf");
        assert_eq!(extension_id(b"_"), "ncocknphbhhlhkikpnnlmbcnbgdempcd");
        assert!(is_extension_id(&extension_id(b"any key")));
    }

    #[test]
    fn store_input_forms() {
        let id = "cjpalhdlnbpafiamejdnhcphjbkeiagm";
        assert_eq!(store_id(id).as_deref(), Some(id));
        assert_eq!(store_id(&format!("  {id}  ")).as_deref(), Some(id));
        assert_eq!(store_id(&format!("https://chromewebstore.google.com/detail/ublock-origin/{id}")).as_deref(), Some(id));
        assert_eq!(store_id(&format!("https://chromewebstore.google.com/detail/{id}?hl=ru")).as_deref(), Some(id));
        assert_eq!(store_id(&format!("https://chrome.google.com/webstore/detail/ublock-origin/{id}")).as_deref(), Some(id));
        for bad in ["", "abc", "cjpalhdlnbpafiamejdnhcphjbkeiagz", "https://example.com/detail/x/cjpalhdlnbpafiamejdnhcphjbkeiagm"] {
            assert_eq!(store_id(bad), None, "{bad}");
        }
    }
}
