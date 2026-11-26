//! Одноразовые коды 2FA по RFC 6238 (HMAC-SHA1, шаг 30 с, 6 цифр) из Base32-ключа профиля.
use super::{Core, CommandError};
use crate::engine::Engine;
use crate::fingerprint::FieldError;
use serde::Serialize;

const STEP: i64 = 30;

#[derive(Serialize)]
pub struct TotpCode {
    pub code: String,
    pub seconds_left: u32,
}

// Код — секрет: в `Debug` его нет.
impl std::fmt::Debug for TotpCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TotpCode").field("code", &"***").field("seconds_left", &self.seconds_left).finish()
    }
}

/// Base32 по RFC 4648: регистр не важен, пробелы и `=` игнорируются. `None` — ключ пуст или есть чужой символ.
pub fn decode_base32(text: &str) -> Option<Vec<u8>> {
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u32 - 'A' as u32,
            c @ '2'..='7' => c as u32 - '2' as u32 + 26,
            _ => return None,
        };
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Код на момент `unix` и сколько секунд он ещё действует.
pub fn code_at(key: &[u8], unix: i64) -> (String, u32) {
    let mac = ring::hmac::sign(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, key),
        &(unix / STEP).to_be_bytes(),
    );
    let h = mac.as_ref();
    let o = (h[19] & 0xf) as usize;
    let n = u32::from_be_bytes([h[o] & 0x7f, h[o + 1], h[o + 2], h[o + 3]]) % 1_000_000;
    (format!("{n:06}"), (STEP - unix % STEP) as u32)
}

impl<E: Engine> Core<E> {
    pub fn totp_code(&self, id: &str) -> Result<TotpCode, CommandError> {
        let field = |code| CommandError::validation(vec![FieldError { field: "totp", code }]);
        // Корзина для 2FA не существует: `profile_secrets_each` её не отфильтровывает.
        self.store.get_profile(id)?.ok_or_else(super::profiles::not_found)?;
        let secret = self.store.profile_secrets_each(id)?.1?.ok_or_else(|| field("required"))?;
        let key = decode_base32(secret.expose()).ok_or_else(|| field("invalid"))?;
        let (code, seconds_left) = code_at(&key, crate::sys::unix_now());
        Ok(TotpCode { code, seconds_left })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_vectors() {
        let key = b"12345678901234567890";
        assert_eq!(code_at(key, 59).0, "287082");
        assert_eq!(code_at(key, 1_111_111_109).0, "081804");
        assert_eq!(code_at(key, 1_234_567_890).0, "005924");
        assert_eq!(code_at(key, 59).1, 1, "секунд до смены");
        assert_eq!(code_at(key, 60).1, 30);
        assert_eq!(decode_base32("jbsw y3dp ehpk 3pxp").unwrap(), b"Hello!\xDE\xAD\xBE\xEF");
        assert_eq!(decode_base32("MZXW6===").unwrap(), b"foo");
        assert!(decode_base32("not*base32").is_none());
        assert!(decode_base32("  = ").is_none());
    }

    #[test]
    fn debug_hides_code() {
        let t = TotpCode { code: "123456".into(), seconds_left: 5 };
        assert!(!format!("{t:?}").contains("123456"));
    }
}
