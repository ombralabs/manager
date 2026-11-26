//! Пакет CRX3 (`components/crx_file/crx3.proto`): `Cr24`, версия 3 (uint32 LE), длина заголовка (uint32 LE),
//! заголовок `CrxFileHeader` в protobuf, затем zip. Подписи не проверяются.
// ponytail: подписи CRX не проверяются — пакет из магазина приходит по TLS с серверов Google, локальный файл выбирает
// сам пользователь; проверка RSA-PSS по заголовку, если появится сторонний источник пакетов.
use super::FormatError;
use super::id::{extension_id, id_from_hash};

pub struct Crx<'a> {
    /// Открытый ключ разработчика (SubjectPublicKeyInfo DER) — уходит в поле `key` манифеста.
    pub key: Vec<u8>,
    pub id: String,
    pub zip: &'a [u8],
}

pub fn parse(bytes: &[u8]) -> Result<Crx<'_>, FormatError> {
    let bad = || FormatError::Crx;
    if bytes.get(..4) != Some(b"Cr24".as_slice()) {
        return Err(bad());
    }
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if word(4) != Some(3) {
        return Err(bad());
    }
    let header_len = word(8).ok_or_else(bad)? as usize;
    let end = 12usize.checked_add(header_len).ok_or_else(bad)?;
    let header = bytes.get(12..end).ok_or_else(bad)?;
    let fields = length_fields(header).ok_or_else(bad)?;
    // `signed_header_data = 10000` — байты `SignedData { crx_id = 1 }`, 16 байт хеша ключа.
    let signed = fields.iter().find(|(number, _)| *number == 10000).map(|(_, b)| *b).ok_or_else(bad)?;
    let crx_id = length_fields(signed)
        .ok_or_else(bad)?
        .into_iter()
        .find(|(number, _)| *number == 1)
        .map(|(_, b)| b)
        .filter(|b| b.len() == 16)
        .ok_or_else(bad)?;
    let id = id_from_hash(crx_id);
    // `sha256_with_rsa = 2`: в пакете из магазина есть и ключ Google; ключ разработчика — тот, чей идентификатор равен `crx_id`.
    let key = fields
        .iter()
        .filter(|(number, _)| *number == 2)
        .filter_map(|(_, proof)| length_fields(proof)?.into_iter().find(|(n, _)| *n == 1).map(|(_, k)| k.to_vec()))
        .find(|key| extension_id(key) == id)
        .ok_or(FormatError::IdMismatch)?;
    Ok(Crx { key, id, zip: &bytes[end..] })
}

fn varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Поля сообщения protobuf с длиной (тип 2): номер и байты. Остальные типы пропускаются; обрыв — `None`.
fn length_fields(buf: &[u8]) -> Option<Vec<(u64, &[u8])>> {
    let mut pos = 0;
    let mut out = Vec::new();
    while pos < buf.len() {
        let tag = varint(buf, &mut pos)?;
        match tag & 7 {
            0 => {
                varint(buf, &mut pos)?;
            }
            1 => pos = pos.checked_add(8)?,
            2 => {
                let len = usize::try_from(varint(buf, &mut pos)?).ok()?;
                let end = pos.checked_add(len)?;
                out.push((tag >> 3, buf.get(pos..end)?));
                pos = end;
            }
            5 => pos = pos.checked_add(4)?,
            _ => return None,
        }
    }
    (pos == buf.len()).then_some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn varint(mut value: u64, out: &mut Vec<u8>) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn field(number: u64, bytes: &[u8], out: &mut Vec<u8>) {
        varint((number << 3) | 2, out);
        varint(bytes.len() as u64, out);
        out.extend_from_slice(bytes);
    }

    /// CRX3: ключ Google и ключ разработчика `key`; `crx_id` — от `id_key` (равен `key` в правильном пакете).
    pub(crate) fn build_crx(key: &[u8], id_key: &[u8], zip: &[u8]) -> Vec<u8> {
        let mut google = Vec::new();
        field(1, b"google key", &mut google);
        let mut proof = Vec::new();
        field(1, key, &mut proof);
        field(2, b"signature", &mut proof);
        let mut signed = Vec::new();
        field(1, &Sha256::digest(id_key)[..16], &mut signed);
        let mut header = Vec::new();
        field(2, &google, &mut header);
        field(2, &proof, &mut header);
        // Постороннее поле varint — пропускается.
        varint(7 << 3, &mut header);
        varint(1, &mut header);
        field(10000, &signed, &mut header);
        let mut out = b"Cr24".to_vec();
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(zip);
        out
    }

    #[test]
    fn parses_developer_key_id_and_archive() {
        let crx = build_crx(b"developer key", b"developer key", b"PK-archive");
        let parsed = parse(&crx).unwrap();
        assert_eq!(parsed.key, b"developer key");
        assert_eq!(parsed.id, extension_id(b"developer key"));
        assert_eq!(parsed.zip, b"PK-archive");
    }

    #[test]
    fn rejects_bad_headers_without_panicking() {
        let good = build_crx(b"k", b"k", b"zip");
        let mut wrong_magic = good.clone();
        wrong_magic[0] = b'X';
        let mut version_2 = good.clone();
        version_2[4] = 2;
        let mut huge_header = good.clone();
        huge_header[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        for bad in [wrong_magic, version_2, huge_header, b"Cr24".to_vec(), Vec::new()] {
            assert_eq!(parse(&bad).err(), Some(FormatError::Crx));
        }
        assert_eq!(parse(&build_crx(b"k", b"other", b"zip")).err(), Some(FormatError::IdMismatch));
        for cut in 0..good.len() - 3 {
            let _ = parse(&good[..cut]);
        }
    }
}
