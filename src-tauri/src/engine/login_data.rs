//! Запись логина и пароля профиля в менеджер паролей Chromium (`Default/Login Data`) — так браузер подставляет их
//! сам. Ключ шифрования под `--use-mock-keychain` известен: PBKDF2-HMAC-SHA1("mock_password", "saltysalt", 1003),
//! схема `v10` + AES-128-CBC (`components/os_crypt`, `crypto/apple/fake_keychain_v2.mm` в Chromium 150).
use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use ring::pbkdf2;
use crate::store::secrets::Secret;
use rusqlite::{Connection, params};
use std::num::NonZeroU32;
use std::path::Path;

const MOCK_PASSWORD: &[u8] = b"mock_password";
const SALT: &[u8] = b"saltysalt";
const ITERATIONS: u32 = 1003;
/// Вектор инициализации os_crypt — 16 пробелов.
const IV: [u8; 16] = [b' '; 16];
const PREFIX: &[u8] = b"v10";

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

pub fn derive_key() -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA1, NonZeroU32::new(ITERATIONS).unwrap(), SALT, MOCK_PASSWORD, &mut key);
    key
}

/// `v10` + AES-128-CBC(PKCS#7) — формат `password_value`.
pub fn encrypt_password(password: &str) -> Vec<u8> {
    let encrypted = Aes128CbcEnc::new(&derive_key().into(), &IV.into()).encrypt_padded_vec_mut::<Pkcs7>(password.as_bytes());
    let mut out = PREFIX.to_vec();
    out.extend(encrypted);
    out
}

pub struct Credential<'a> {
    /// `https://example.com/` — `origin_url` и `signon_realm` одновременно.
    pub realm: &'a str,
    pub username: &'a str,
    pub password: &'a str,
    pub now: i64,
}

/// Домен платформы → `signon_realm`: хост в нижнем регистре, схема по умолчанию `https`, порт сохраняется (кроме
/// стандартного для схемы), путь, запрос и фрагмент отбрасываются, хвост `/`. Пустой хост — пустая строка (нет realm).
pub fn realm_for(domain: &str) -> String {
    let domain = domain.trim();
    let (scheme, rest) = match domain.split_once("://") {
        Some((s, r)) if s.eq_ignore_ascii_case("http") => ("http", r),
        Some((_, r)) => ("https", r),
        None => ("https", domain),
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("").to_lowercase();
    let default_port = if scheme == "http" { ":80" } else { ":443" };
    let host = host.strip_suffix(default_port).unwrap_or(&host);
    if host.is_empty() { String::new() } else { format!("{scheme}://{host}/") }
}

/// Метка строк, записанных Ombra (`username_element`): только их синхронизация меняет и удаляет.
const MARK: &str = "ombra";

/// Подключение к `Login Data` с каскадным удалением дочерних таблиц Chromium.
fn open(login_data: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(login_data)?;
    conn.execute_batch("PRAGMA foreign_keys = ON")?;
    Ok(conn)
}

/// Запись или обновление помеченной строки `logins` для `realm` + `username`. Файл создаёт Chromium.
pub fn upsert(login_data: &Path, cred: &Credential<'_>) -> rusqlite::Result<()> {
    let mut conn = open(login_data)?;
    let tx = conn.transaction()?;
    upsert_on(&tx, cred)?;
    tx.commit()
}

fn upsert_on(conn: &Connection, cred: &Credential<'_>) -> rusqlite::Result<()> {
    let password = encrypt_password(cred.password);
    // Время Chromium: микросекунды от 1601 года.
    let chromium_time = (cred.now + 11_644_473_600) * 1_000_000;
    let updated = conn.execute(
        "UPDATE logins SET password_value = ?3, date_last_used = ?4, date_password_modified = ?4 \
         WHERE signon_realm = ?1 AND username_value = ?2 AND username_element = ?5",
        params![cred.realm, cred.username, password, chromium_time, MARK],
    )?;
    if updated == 0 {
        // `id` назначает AUTOINCREMENT: повторное использование номера цепляло бы чужие `password_notes`.
        conn.execute(
            "INSERT INTO logins (origin_url, action_url, username_element, username_value, password_element, \
             password_value, submit_element, signon_realm, date_created, blacklisted_by_user, scheme, \
             password_type, times_used, form_data, display_name, icon_url, federation_url, skip_zero_click, \
             generation_upload_status, possible_username_pairs, date_last_used, moving_blocked_for, \
             date_password_modified, sender_email, sender_name, date_received, sharing_notification_displayed, \
             keychain_identifier, sender_profile_image_url) \
             VALUES (?1, '', ?5, ?2, '', ?3, '', ?1, ?4, 0, 0, 0, 0, NULL, '', '', '', 0, 0, NULL, \
             ?4, NULL, ?4, '', '', 0, 0, '', '')",
            params![cred.realm, cred.username, password, chromium_time, MARK],
        )?;
    }
    Ok(())
}

/// Копия базы без строк Ombra. База копируется во временную папку вместе с журналами и открывается на запись:
/// после сбоя браузера рядом лежит незакрытый журнал, и без записи SQLite не откатит незавершённую транзакцию.
/// Оригинал в папке профиля не трогается.
pub fn scrubbed_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let copy = tmp.path().join("Login Data");
    std::fs::copy(src, &copy)?;
    for suffix in ["-journal", "-wal", "-shm"] {
        let side = with_suffix(src, suffix);
        if side.is_file() {
            std::fs::copy(&side, with_suffix(&copy, suffix))?;
        }
    }
    scrub(&copy, dst).map_err(std::io::Error::other)
}

fn scrub(copy: &Path, dst: &Path) -> rusqlite::Result<()> {
    open(copy)?.execute("VACUUM INTO ?1", [dst.to_string_lossy().as_ref()])?;
    let conn = open(dst)?;
    conn.execute("DELETE FROM logins WHERE username_element = ?1", [MARK])?;
    conn.execute_batch("VACUUM")
}

/// `Login Data` → `Login Data-journal`.
fn with_suffix(db: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

/// Удаляет помеченные строки Ombra для `realm`; чужие не трогаются.
#[cfg(test)]
pub fn remove(login_data: &Path, realm: &str) -> rusqlite::Result<()> {
    open(login_data)?.execute("DELETE FROM logins WHERE signon_realm = ?1 AND username_element = ?2", params![realm, MARK])?;
    Ok(())
}

/// Правила спеки: домен, логин и пароль — запись для realm и удаление прочих строк Ombra; нет чего-то из трёх —
/// удаляются все строки Ombra. Строки без метки не меняются никогда. Файла нет — ничего (`Ok(false)`).
pub fn sync(
    profile_dir: &Path,
    domain: &str,
    username: Option<&str>,
    password: Option<&Secret>,
    now: i64,
) -> rusqlite::Result<bool> {
    let path = profile_dir.join("Default/Login Data");
    if !path.is_file() {
        return Ok(false);
    }
    let realm = realm_for(domain);
    let mut conn = open(&path)?;
    let tx = conn.transaction()?;
    match (username.map(str::trim).filter(|u| !u.is_empty()), password) {
        (Some(username), Some(password)) if !realm.is_empty() => {
            upsert_on(&tx, &Credential { realm: &realm, username, password: password.expose(), now })?;
            tx.execute(
                "DELETE FROM logins WHERE username_element = ?3 AND NOT (signon_realm = ?1 AND username_value = ?2)",
                params![realm, username, MARK],
            )?;
        }
        _ => {
            tx.execute("DELETE FROM logins WHERE username_element = ?1", [MARK])?;
        }
    }
    tx.commit()?;
    Ok(true)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Известный ответ из `keychain_key_provider_unittest.mm` Chromium.
    #[test]
    fn key_matches_chromium_known_answer() {
        assert_eq!(
            derive_key(),
            [0xAF, 0x0F, 0x76, 0x2A, 0xAF, 0x6D, 0x7D, 0x11, 0x58, 0x1B, 0x7A, 0xA8, 0xCE, 0x72, 0x18, 0xDE]
        );
    }

    #[test]
    fn password_is_v10_prefixed_cbc_block() {
        let blob = encrypt_password("secret");
        assert_eq!(&blob[..3], b"v10");
        assert_eq!(blob.len(), 3 + 16, "один блок PKCS#7");
        assert_ne!(encrypt_password("other"), blob);
        assert_eq!(realm_for("example.com"), "https://example.com/");
        assert_eq!(realm_for("http://shop.test/"), "http://shop.test/");
    }

    /// Схема `logins` в объёме колонок, которые пишет `upsert` (Chromium 150, `login_database.cc`).
    pub(crate) fn fake_login_data(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE logins (origin_url VARCHAR NOT NULL, action_url VARCHAR, username_element VARCHAR, \
             username_value VARCHAR, password_element VARCHAR, password_value BLOB, submit_element VARCHAR, \
             signon_realm VARCHAR NOT NULL, date_created INTEGER NOT NULL, blacklisted_by_user INTEGER NOT NULL, \
             scheme INTEGER NOT NULL, password_type INTEGER, times_used INTEGER, form_data BLOB, display_name VARCHAR, \
             icon_url VARCHAR, federation_url VARCHAR, skip_zero_click INTEGER, generation_upload_status INTEGER, \
             possible_username_pairs BLOB, id INTEGER PRIMARY KEY AUTOINCREMENT, date_last_used INTEGER NOT NULL DEFAULT 0, \
             moving_blocked_for BLOB, date_password_modified INTEGER NOT NULL DEFAULT 0, sender_email VARCHAR, \
             sender_name VARCHAR, date_received INTEGER, sharing_notification_displayed INTEGER NOT NULL DEFAULT 0, \
             keychain_identifier BLOB, sender_profile_image_url VARCHAR, \
             UNIQUE (origin_url, username_element, username_value, password_element, signon_realm))",
        )
        .unwrap();
    }

    #[test]
    fn upsert_inserts_then_updates_and_remove_clears() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Login Data");
        fake_login_data(&path);
        let cred = Credential { realm: "https://shop.test/", username: "ann", password: "p1", now: 1_700_000_000 };
        upsert(&path, &cred).unwrap();
        upsert(&path, &Credential { password: "p2", ..cred }).unwrap();
        let conn = Connection::open(&path).unwrap();
        let (count, blob): (i64, Vec<u8>) = conn
            .query_row("SELECT COUNT(*), MAX(password_value) FROM logins WHERE signon_realm = 'https://shop.test/'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(count, 1, "вторая запись обновила первую");
        assert_eq!(blob, encrypt_password("p2"));
        remove(&path, "https://shop.test/").unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM logins", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn realm_is_normalized() {
        assert_eq!(realm_for("Shop.Test/login"), "https://shop.test/");
        assert_eq!(realm_for("http://Shop.Test:8080/x?y"), "http://shop.test:8080/");
        assert_eq!(realm_for(" https://A.b#f "), "https://a.b/");
    }

    fn profile_with_login_data() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Default")).unwrap();
        let path = dir.path().join("Default/Login Data");
        fake_login_data(&path);
        (dir, path)
    }

    fn insert_foreign(path: &Path, realm: &str, user: &str, element: &str) {
        Connection::open(path)
            .unwrap()
            .execute(
                "INSERT INTO logins (origin_url, username_element, username_value, signon_realm, date_created, \
                 blacklisted_by_user, scheme) VALUES (?1, ?3, ?2, ?1, 0, 0, 0)",
                params![realm, user, element],
            )
            .unwrap();
    }

    /// После сбоя браузера посреди записи рядом с базой лежит незакрытый журнал: копия всё равно делается,
    /// незавершённая запись в неё не попадает, строки Ombra вычищены, журнал оригинала не тронут.
    #[test]
    fn scrubbed_copy_rolls_back_a_hot_journal() {
        let (dir, path) = profile_with_login_data();
        upsert(&path, &Credential { realm: "https://shop.test/", username: "ann", password: "p", now: 1 }).unwrap();
        insert_foreign(&path, "https://mail.test/", "bob", "email");
        let crashed = dir.path().join("crashed");
        std::fs::create_dir(&crashed).unwrap();
        let copy = crashed.join("Login Data");
        {
            let conn = Connection::open(&path).unwrap();
            // Маленький кэш: незафиксированные страницы уходят в файл базы, их прежнее содержимое — в журнал.
            conn.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA cache_size = 1; BEGIN IMMEDIATE;").unwrap();
            for i in 0..200 {
                conn.execute(
                    "INSERT INTO logins (origin_url, username_value, signon_realm, date_created, blacklisted_by_user, \
                     scheme, form_data) VALUES ('https://x.test/', ?1, 'https://x.test/', 0, 0, 0, zeroblob(4096))",
                    [format!("u{i}")],
                )
                .unwrap();
            }
            std::fs::copy(&path, &copy).unwrap();
            std::fs::copy(dir.path().join("Default/Login Data-journal"), crashed.join("Login Data-journal")).unwrap();
            // Соединение закрывается без фиксации, как при сбое браузера.
        }
        let read_only = Connection::open_with_flags(&copy, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert!(
            read_only.query_row("SELECT COUNT(*) FROM logins", [], |r| r.get::<_, i64>(0)).is_err(),
            "журнал незакрыт: без записи база не читается"
        );
        drop(read_only);
        let out = dir.path().join("out");
        scrubbed_copy(&copy, &out).unwrap();
        let conn = Connection::open(&out).unwrap();
        let users: Vec<String> = conn
            .prepare("SELECT username_value FROM logins ORDER BY username_value")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(users, ["bob"], "чужая строка осталась; строка Ombra и незавершённая запись — нет");
        assert!(crashed.join("Login Data-journal").is_file(), "журнал оригинала не тронут");
    }

    #[test]
    fn sync_touches_only_ombra_rows() {
        let (dir, path) = profile_with_login_data();
        let pass = Secret::new("p");
        insert_foreign(&path, "https://gmail.com/", "ann", "Email");
        insert_foreign(&path, "https://shop.test/", "bob", "Email");
        insert_foreign(&path, "https://shop.test/", "ann", "Email");
        let foreign = || -> i64 {
            Connection::open(&path)
                .unwrap()
                .query_row("SELECT COUNT(*) FROM logins WHERE username_element <> 'ombra'", [], |r| r.get(0))
                .unwrap()
        };
        let own = |path: &Path| -> Vec<(String, String)> {
            let conn = Connection::open(path).unwrap();
            let mut st = conn.prepare("SELECT signon_realm, username_value FROM logins WHERE username_element = 'ombra'").unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
        };
        let case = |domain: &str, user: Option<&str>, expect: &[(&str, &str)]| {
            sync(dir.path(), domain, user, Some(&pass), 1).unwrap();
            let expect: Vec<(String, String)> = expect.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
            assert_eq!(own(&path), expect, "{domain} {user:?}");
            assert_eq!(foreign(), 3, "чужие строки целы: {domain} {user:?}");
        };
        case("shop.test", Some("ann"), &[("https://shop.test/", "ann")]);
        case("shop.test", Some("old"), &[("https://shop.test/", "old")]);
        case("other.test", Some("old"), &[("https://other.test/", "old")]);
        case("", Some("old"), &[]);
        case("shop.test", Some("old"), &[("https://shop.test/", "old")]);
        case("third.test", Some("new"), &[("https://third.test/", "new")]);
        sync(dir.path(), "third.test", None, Some(&pass), 2).unwrap();
        assert!(own(&path).is_empty());
        assert_eq!(foreign(), 3);
        sync(dir.path(), "third.test", Some("new"), None, 3).unwrap();
        assert!(own(&path).is_empty());
        let empty = tempfile::tempdir().unwrap();
        assert!(!sync(empty.path(), "shop.test", Some("ann"), Some(&pass), 1).unwrap());
        assert!(!empty.path().join("Default/Login Data").exists());
    }

    #[test]
    fn realm_drops_default_port_and_empty_host() {
        assert_eq!(realm_for("https://Shop.test:443/x"), "https://shop.test/");
        assert_eq!(realm_for("http://shop.test:80"), "http://shop.test/");
        assert_eq!(realm_for("https://"), "");
        assert_eq!(realm_for(""), "");
    }
}
