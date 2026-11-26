//! Cookies профиля в базе Ombra — основной экземпляр (спецификация 2А, «Cookies»).
//! Значение каждой cookie зашифровано ключом секретов, как пароли.
use super::secrets::Secret;
use super::{Store, StoreError};
use crate::cookies::{Cookie, SameSite, set_hash};
use rusqlite::{OptionalExtension, Statement, params};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct MergeCount {
    pub added: usize,
    pub replaced: usize,
}

/// Сколько cookies у профиля и сколько из них не расшифровываются (база перенесена с другой машины).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct CookieStats {
    pub total: i64,
    pub unreadable: i64,
}

/// Колонки (domain, name, path, value_enc, expires, http_only, secure, same_site, host_only) как в таблице.
type CookieRow = (String, String, String, Vec<u8>, Option<i64>, bool, bool, Option<String>, bool);

const INSERT: &str = "INSERT OR REPLACE INTO cookies \
                      (profile_id, domain, name, path, value_enc, expires, http_only, secure, same_site, host_only) \
                      VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)";

impl Store {
    /// Расшифрованный набор профиля, по (домен, имя, путь). Запись, которую не удаётся расшифровать, пропускается:
    /// профиль запускается с остальными, в журнал — только число пропущенных.
    pub fn load_cookies(&self, profile_id: &str) -> Result<Vec<Cookie>, StoreError> {
        let rows = self.cookie_rows(profile_id)?;
        let total = rows.len();
        // Расшифровка — уже без мьютекса базы.
        let cookies: Vec<Cookie> = rows.into_iter().filter_map(|row| self.decrypt_row(row).ok()).collect();
        let skipped = total - cookies.len();
        if skipped > 0 {
            log::warn!("cookies профиля {profile_id}: пропущено {skipped} нечитаемых записей");
        }
        Ok(cookies)
    }

    /// Всего записей и сколько из них не расшифровываются. Неизвестный профиль — нули.
    pub fn cookie_stats(&self, profile_id: &str) -> Result<CookieStats, StoreError> {
        let blobs: Vec<Vec<u8>> = {
            let conn = self.conn();
            let mut stmt = conn.prepare("SELECT value_enc FROM cookies WHERE profile_id = ?1")?;
            let rows = stmt.query_map([profile_id], |r| r.get(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let unreadable = blobs.iter().filter(|blob| self.secrets.decrypt(blob).is_err()).count();
        Ok(CookieStats { total: blobs.len() as i64, unreadable: unreadable as i64 })
    }

    /// «Очистить cookies»: набор профиля удаляется одной транзакцией. В `cookies_hash` — хэш пустого набора:
    /// первый снимок пустого браузера ничего не пишет.
    pub fn clear_cookies(&self, profile_id: &str) -> Result<(), StoreError> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        if tx.execute("UPDATE profiles SET cookies_hash = ?2 WHERE id = ?1", params![profile_id, set_hash(&[])])? == 0 {
            return Err(not_found(profile_id));
        }
        tx.execute("DELETE FROM cookies WHERE profile_id = ?1", [profile_id])?;
        tx.commit()?;
        Ok(())
    }

    fn cookie_rows(&self, profile_id: &str) -> Result<Vec<CookieRow>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT domain, name, path, value_enc, expires, http_only, secure, same_site, host_only \
             FROM cookies WHERE profile_id = ?1 ORDER BY domain, name, path",
        )?;
        let rows = stmt.query_map([profile_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn decrypt_row(&self, row: CookieRow) -> Result<Cookie, StoreError> {
        let (domain, name, path, value_enc, expires, http_only, secure, same_site, host_only) = row;
        Ok(Cookie {
            domain,
            name,
            value: self.secrets.decrypt(&value_enc)?.expose().to_string(),
            path,
            expires,
            http_only,
            secure,
            same_site: same_site.as_deref().and_then(SameSite::parse),
            host_only,
        })
    }

    pub fn cookies_hash(&self, profile_id: &str) -> Result<Option<String>, StoreError> {
        let hash: Option<Option<String>> = self
            .conn()
            .query_row("SELECT cookies_hash FROM profiles WHERE id = ?1", [profile_id], |r| r.get(0))
            .optional()?;
        hash.ok_or_else(|| not_found(profile_id))
    }

    /// Снимок: весь набор профиля заменяется одной транзакцией, хэш записывается — но только если в базе всё ещё
    /// `expected` (значение `cookies_hash`, прочитанное до запроса набора у браузера). Иначе набор в базе сменился,
    /// пока шёл снимок (импорт), и снимок ничего не пишет. Возвращает, записан ли набор.
    pub fn replace_cookies(
        &self,
        profile_id: &str,
        cookies: &[Cookie],
        hash: &str,
        expected: Option<&str>,
    ) -> Result<bool, StoreError> {
        let encrypted = self.encrypt_values(cookies)?;
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let updated = tx.execute(
            "UPDATE profiles SET cookies_hash = ?2 WHERE id = ?1 AND cookies_hash IS ?3",
            params![profile_id, hash, expected],
        )?;
        if updated == 0 {
            let exists: bool =
                tx.query_row("SELECT EXISTS(SELECT 1 FROM profiles WHERE id = ?1)", [profile_id], |r| r.get(0))?;
            return if exists { Ok(false) } else { Err(not_found(profile_id)) };
        }
        tx.execute("DELETE FROM cookies WHERE profile_id = ?1", [profile_id])?;
        {
            let mut insert = tx.prepare(INSERT)?;
            for (cookie, value_enc) in cookies.iter().zip(&encrypted) {
                insert_one(&mut insert, profile_id, cookie, value_enc)?;
            }
        }
        tx.commit()?;
        Ok(true)
    }

    /// Импорт: добавить или заменить по ключу. Вместо хэша — своя пометка каждого импорта: снимок, начатый до
    /// импорта, не запишет поверх него, а следующий снимок точно запишет набор (пометка не равна хэшу).
    pub fn merge_cookies(&self, profile_id: &str, cookies: &[Cookie]) -> Result<MergeCount, StoreError> {
        let encrypted = self.encrypt_values(cookies)?;
        let mark = import_mark()?;
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        if tx.execute("UPDATE profiles SET cookies_hash = ?2 WHERE id = ?1", params![profile_id, mark])? == 0 {
            return Err(not_found(profile_id));
        }
        let mut count = MergeCount::default();
        {
            let mut exists = tx.prepare(
                "SELECT EXISTS(SELECT 1 FROM cookies WHERE profile_id = ?1 AND domain = ?2 AND name = ?3 AND path = ?4)",
            )?;
            let mut insert = tx.prepare(INSERT)?;
            for (cookie, value_enc) in cookies.iter().zip(&encrypted) {
                let present: bool =
                    exists.query_row(params![profile_id, cookie.domain, cookie.name, cookie.path], |r| r.get(0))?;
                insert_one(&mut insert, profile_id, cookie, value_enc)?;
                if present {
                    count.replaced += 1;
                } else {
                    count.added += 1;
                }
            }
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn count_cookies(&self, profile_id: &str) -> Result<i64, StoreError> {
        Ok(self.conn().query_row("SELECT COUNT(*) FROM cookies WHERE profile_id = ?1", [profile_id], |r| r.get(0))?)
    }

    /// Шифрование — до мьютекса базы.
    fn encrypt_values(&self, cookies: &[Cookie]) -> Result<Vec<Vec<u8>>, StoreError> {
        cookies.iter().map(|c| self.secrets.encrypt(&Secret::new(c.value.clone()))).collect()
    }
}

fn insert_one(stmt: &mut Statement<'_>, profile_id: &str, cookie: &Cookie, value_enc: &[u8]) -> Result<(), StoreError> {
    stmt.execute(params![
        profile_id,
        cookie.domain,
        cookie.name,
        cookie.path,
        value_enc,
        cookie.expires,
        cookie.http_only,
        cookie.secure,
        cookie.same_site.map(SameSite::as_str),
        cookie.host_only
    ])?;
    Ok(())
}

/// `import:` и 32 случайные шестнадцатеричные цифры. В хэше набора двоеточия нет: совпасть они не могут.
fn import_mark() -> Result<String, StoreError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| StoreError::Crypto)?;
    Ok(format!("import:{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
}

fn not_found(profile_id: &str) -> StoreError {
    StoreError::NotFound(format!("профиль {profile_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cookies::tests::cookie;
    use crate::store::profiles::ProfileInput;
    use crate::store::tests::memory_store;

    fn profile(store: &Store) -> String {
        store.create_profile(&ProfileInput::new("p", r#"{"seed":1}"#), 1).unwrap().id
    }

    #[test]
    fn cookies_round_trip_in_key_order() {
        let store = memory_store();
        let id = profile(&store);
        let mut b = cookie(".example.com", "b", "2");
        b.expires = Some(1_767_225_600);
        b.same_site = Some(SameSite::Lax);
        b.http_only = true;
        b.secure = true;
        let a = cookie("example.com", "a", "1");
        let hash = set_hash(&[a.clone(), b.clone()]);
        assert!(store.replace_cookies(&id, &[a.clone(), b.clone()], &hash, None).unwrap());
        // «.example.com» < «example.com»: точка раньше букв.
        assert_eq!(store.load_cookies(&id).unwrap(), [b, a]);
        assert_eq!(store.cookies_hash(&id).unwrap(), Some(hash));
        assert_eq!(store.count_cookies(&id).unwrap(), 2);
    }

    #[test]
    fn values_are_not_stored_in_plain_text() {
        let store = memory_store();
        let id = profile(&store);
        let c = cookie("example.com", "sid", "s3cr3t-cookie-value");
        store.replace_cookies(&id, std::slice::from_ref(&c), &set_hash(std::slice::from_ref(&c)), None).unwrap();
        let blob: Vec<u8> = store
            .raw()
            .query_row("SELECT value_enc FROM cookies WHERE profile_id = ?1", [&id], |r| r.get(0))
            .unwrap();
        assert!(!blob.windows(19).any(|w| w == b"s3cr3t-cookie-value"));
        assert_eq!(store.load_cookies(&id).unwrap()[0].value, "s3cr3t-cookie-value");
    }

    #[test]
    fn snapshot_replaces_the_whole_set() {
        let store = memory_store();
        let id = profile(&store);
        let a = cookie("example.com", "a", "1");
        let b = cookie("example.com", "b", "2");
        let both = [a, b.clone()];
        store.replace_cookies(&id, &both, &set_hash(&both), None).unwrap();
        let only_b = std::slice::from_ref(&b);
        assert!(store.replace_cookies(&id, only_b, &set_hash(only_b), Some(&set_hash(&both))).unwrap());
        assert_eq!(store.load_cookies(&id).unwrap(), std::slice::from_ref(&b), "cookie, удалённая в браузере, уходит из базы");
        assert_eq!(store.cookies_hash(&id).unwrap(), Some(set_hash(&[b])));
    }

    #[test]
    fn import_merges_by_key_and_marks_the_set() {
        let store = memory_store();
        let id = profile(&store);
        let a = cookie("example.com", "a", "1");
        store.replace_cookies(&id, std::slice::from_ref(&a), &set_hash(std::slice::from_ref(&a)), None).unwrap();
        let newer = cookie("example.com", "a", "новое");
        let c = cookie("example.org", "c", "3");
        let count = store.merge_cookies(&id, &[newer.clone(), c.clone()]).unwrap();
        assert_eq!(count, MergeCount { added: 1, replaced: 1 });
        let merged = [newer, c];
        let mark = store.cookies_hash(&id).unwrap();
        assert!(mark.is_some_and(|mark| mark != set_hash(&merged)), "следующий снимок обязан записать набор");
        assert_eq!(store.load_cookies(&id).unwrap(), merged);
    }

    #[test]
    fn snapshot_writes_only_over_the_value_it_read() {
        let store = memory_store();
        let id = profile(&store);
        let a = [cookie("example.com", "a", "1")];
        let b = [cookie("example.com", "b", "2")];
        assert!(store.replace_cookies(&id, &a, &set_hash(&a), None).unwrap(), "у нового профиля хэша нет");
        assert!(
            !store.replace_cookies(&id, &b, &set_hash(&b), Some("другой")).unwrap(),
            "набор в базе сменился после чтения — снимок не пишет"
        );
        assert_eq!(store.load_cookies(&id).unwrap(), a);
        assert_eq!(store.cookies_hash(&id).unwrap(), Some(set_hash(&a)));

        // Импорт, пришедший, пока снимок ждал браузер, не стирается снимком, начатым до него.
        let imported = cookie("example.com", "imp", "3");
        store.merge_cookies(&id, std::slice::from_ref(&imported)).unwrap();
        let mark = store.cookies_hash(&id).unwrap();
        assert!(!store.replace_cookies(&id, &b, &set_hash(&b), Some(&set_hash(&a))).unwrap());
        assert_eq!(store.load_cookies(&id).unwrap(), [a[0].clone(), imported]);
        assert!(store.replace_cookies(&id, &b, &set_hash(&b), mark.as_deref()).unwrap(), "снимок после импорта пишет");
        assert_eq!(store.load_cookies(&id).unwrap(), b);
        assert_eq!(store.cookies_hash(&id).unwrap(), Some(set_hash(&b)));
    }

    #[test]
    fn every_import_leaves_its_own_mark() {
        let store = memory_store();
        let id = profile(&store);
        store.merge_cookies(&id, &[cookie("example.com", "a", "1")]).unwrap();
        let first = store.cookies_hash(&id).unwrap().expect("импорт оставляет пометку");
        store.merge_cookies(&id, &[cookie("example.com", "b", "2")]).unwrap();
        let second = store.cookies_hash(&id).unwrap().expect("импорт оставляет пометку");
        assert_ne!(first, second, "каждый импорт меняет записанное значение");
        let set = store.load_cookies(&id).unwrap();
        assert!(![first, second].contains(&set_hash(&set)), "пометка не совпадает с хэшем набора");
    }

    #[test]
    fn cookies_go_away_with_profile() {
        let store = memory_store();
        let id = profile(&store);
        store.merge_cookies(&id, &[cookie("example.com", "a", "1")]).unwrap();
        store.delete_profile(&id).unwrap();
        let left: i64 = store.raw().query_row("SELECT COUNT(*) FROM cookies", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn missing_profile_is_not_found() {
        let store = memory_store();
        let a = cookie("example.com", "a", "1");
        let missing = store.replace_cookies("nope00", std::slice::from_ref(&a), "x", None);
        assert!(matches!(missing, Err(StoreError::NotFound(_))));
        assert!(matches!(store.merge_cookies("nope00", &[a]), Err(StoreError::NotFound(_))));
        assert!(matches!(store.cookies_hash("nope00"), Err(StoreError::NotFound(_))));
        assert_eq!(store.load_cookies("nope00").unwrap(), Vec::new());
        assert_eq!(store.count_cookies("nope00").unwrap(), 0);
    }

    #[test]
    fn unreadable_rows_are_skipped_and_counted() {
        let store = memory_store();
        let id = profile(&store);
        store.merge_cookies(&id, &[cookie("example.com", "a", "1"), cookie("example.com", "b", "2")]).unwrap();
        assert_eq!(store.cookie_stats(&id).unwrap(), CookieStats { total: 2, unreadable: 0 });
        // База с другой машины: значение зашифровано чужим ключом.
        store.raw().execute("UPDATE cookies SET value_enc = zeroblob(64) WHERE name = 'a'", []).unwrap();
        assert_eq!(store.cookie_stats(&id).unwrap(), CookieStats { total: 2, unreadable: 1 });
        assert_eq!(store.load_cookies(&id).unwrap(), [cookie("example.com", "b", "2")], "остальные читаются");
        assert_eq!(store.cookie_stats("nope00").unwrap(), CookieStats::default());
    }

    #[test]
    fn clear_removes_the_set_and_marks_it_empty() {
        let store = memory_store();
        let id = profile(&store);
        let other = profile(&store);
        store.merge_cookies(&id, &[cookie("example.com", "a", "1")]).unwrap();
        store.merge_cookies(&other, &[cookie("example.com", "a", "1")]).unwrap();
        store.clear_cookies(&id).unwrap();
        assert_eq!(store.count_cookies(&id).unwrap(), 0);
        assert_eq!(store.cookies_hash(&id).unwrap(), Some(set_hash(&[])), "снимок пустого браузера ничего не перепишет");
        assert_eq!(store.count_cookies(&other).unwrap(), 1, "чужой набор не тронут");
        assert!(matches!(store.clear_cookies("nope00"), Err(StoreError::NotFound(_))));
    }
}
