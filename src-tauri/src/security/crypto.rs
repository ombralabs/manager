use crate::app::CommandError;
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce, aead::{Aead, Payload}};
use ring::{hmac, pbkdf2};
use std::num::NonZeroU32;
use zeroize::Zeroizing;

pub(super) type Key = Zeroizing<[u8; 32]>;
const DOMAIN: &[u8] = b"Ombra profile vault v1";
pub(super) fn error() -> CommandError { CommandError::new("SecurityCredentials", "неверный пароль или код подтверждения") }
pub(super) fn random<const N: usize>() -> Result<[u8; N], CommandError> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| CommandError::new("SecurityStorage", "не удалось создать ключ защиты"))?;
    Ok(bytes)
}
pub(super) fn encode(bytes: &[u8]) -> String { B64.encode(bytes) }
pub(super) fn decode(text: &str) -> Result<Vec<u8>, CommandError> { B64.decode(text).map_err(|_| error()) }
pub(super) fn password_key(password: &str, salt: &str) -> Result<Key, CommandError> {
    let mut key = Zeroizing::new([0; 32]);
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(600_000).unwrap(), &decode(salt)?, password.as_bytes(), key.as_mut());
    Ok(key)
}
pub(super) fn combined(password: &[u8; 32], factor: &[u8]) -> Key {
    let mac = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, password), &[DOMAIN, factor].concat());
    Zeroizing::new(mac.as_ref().try_into().unwrap())
}
pub(super) fn seal(key: &[u8; 32], data: &[u8]) -> Result<String, CommandError> {
    let nonce = random::<24>()?;
    let bytes = XChaCha20Poly1305::new(key.into()).encrypt(XNonce::from_slice(&nonce), Payload { msg: data, aad: DOMAIN }).map_err(|_| error())?;
    Ok(encode(&[nonce.as_slice(), &bytes].concat()))
}
pub(super) fn unseal(key: &[u8; 32], data: &str) -> Result<Zeroizing<Vec<u8>>, CommandError> {
    let bytes = decode(data)?;
    if bytes.len() < 40 { return Err(error()); }
    XChaCha20Poly1305::new(key.into()).decrypt(XNonce::from_slice(&bytes[..24]), Payload { msg: &bytes[24..], aad: DOMAIN }).map(Zeroizing::new).map_err(|_| error())
}
pub(super) fn base32(bytes: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut out, mut acc, mut bits) = (String::new(), 0u32, 0u32);
    for byte in bytes {
        acc = (acc << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 { bits -= 5; out.push(CHARS[((acc >> bits) & 31) as usize] as char); }
        acc &= (1 << bits) - 1;
    }
    if bits != 0 { out.push(CHARS[((acc << (5 - bits)) & 31) as usize] as char); }
    out
}
pub(super) fn code_step(seed: &str, code: &str, now: i64, after: i64) -> Option<i64> {
    if code.len() != 6 || !code.bytes().all(|c| c.is_ascii_digit()) || now < 30 { return None; }
    let bytes = Zeroizing::new(crate::app::totp::decode_base32(seed)?);
    (-1..=1).map(|offset| now / 30 + offset).filter(|step| *step > after).find(|step| {
        let expected = crate::app::totp::code_at(&bytes, step * 30).0;
        // HMAC verification compares fixed-size digests in constant time.
        let key = hmac::Key::new(hmac::HMAC_SHA256, &bytes);
        hmac::verify(&key, code.as_bytes(), hmac::sign(&key, expected.as_bytes()).as_ref()).is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn envelopes_require_both_factors_and_authenticate_the_payload() {
        let salt = encode(&random::<32>().unwrap());
        let password = password_key("a long example passphrase", &salt).unwrap();
        let factor = random::<32>().unwrap();
        let key = combined(&password, &factor);
        let blob = seal(&key, b"profile-private-data").unwrap();
        assert_eq!(&**unseal(&key, &blob).unwrap(), b"profile-private-data");
        assert!(unseal(&password, &blob).is_err());
        assert!(unseal(&combined(&password, &random::<32>().unwrap()), &blob).is_err());
        let mut altered = decode(&blob).unwrap();
        *altered.last_mut().unwrap() ^= 1;
        assert!(unseal(&key, &encode(&altered)).is_err());
    }
    #[test]
    fn totp_window_rejects_replay_and_malformed_codes() {
        let seed = base32(b"12345678901234567890");
        assert_eq!(code_step(&seed, "287082", 59, -1), Some(1));
        assert_eq!(code_step(&seed, "287082", 59, 1), None);
        assert_eq!(code_step(&seed, "287082", 120, -1), None);
        assert_eq!(code_step(&seed, "28708x", 59, -1), None);
    }
}
