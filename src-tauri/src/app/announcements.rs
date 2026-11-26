//! Каталог внутренних рекламных кампаний. Окно получает только действующие объявления, не произвольные HTML.
use super::CommandError;
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::io::Read;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Localized { pub ru: String, pub en: String }
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone { Info, Promotion }
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Announcement {
    pub id: String,
    pub revision: String,
    pub title: Localized,
    pub body: Localized,
    pub action_label: Option<Localized>,
    pub url: Option<String>,
    pub tone: Tone,
    pub dismissible: bool,
    pub starts_at: Option<i64>,
    pub ends_at: Option<i64>,
}

fn valid_text(value: &Localized, limit: usize) -> bool {
    [&value.ru, &value.en].iter().all(|text| !text.trim().is_empty() && text.chars().count() <= limit)
}
fn valid_url(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| url.scheme() == "https" && url.host_str().is_some() && url.username().is_empty() && url.password().is_none())
}
pub fn list(paths: &Paths, now: i64) -> Result<Vec<Announcement>, CommandError> {
    let path = paths.root().join("announcements.json");
    if !path.exists() { return Ok(vec![]); }
    let mut data = String::new();
    std::fs::File::open(path)?.take(128 * 1024 + 1).read_to_string(&mut data)?;
    if data.len() > 128 * 1024 { return Err(invalid()); }
    let items: Vec<Announcement> = serde_json::from_str(&data).map_err(|_| invalid())?;
    if items.len() > 30 { return Err(invalid()); }
    let mut ids = std::collections::HashSet::new();
    for item in &items {
        if item.id.is_empty() || item.id.len() > 64 || !item.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || !ids.insert(&item.id) || item.revision.is_empty() || item.revision.len() > 64
            || !valid_text(&item.title, 120) || !valid_text(&item.body, 600)
            || item.action_label.as_ref().is_some_and(|text| !valid_text(text, 40))
            || item.url.as_ref().is_some_and(|url| !valid_url(url))
            || item.url.is_some() != item.action_label.is_some()
            || item.starts_at.zip(item.ends_at).is_some_and(|(a, b)| a >= b) { return Err(invalid()); }
    }
    Ok(items.into_iter().filter(|item| item.starts_at.is_none_or(|start| start <= now) && item.ends_at.is_none_or(|end| end > now)).take(3).collect())
}
pub fn open(paths: &Paths, id: &str, now: i64) -> Result<(), CommandError> {
    let url = list(paths, now)?.into_iter().find(|item| item.id == id).and_then(|item| item.url).ok_or_else(invalid)?;
    let url = reqwest::Url::parse(&url).map_err(|_| invalid())?;
    ::open::that(url.as_str())?;
    Ok(())
}
fn invalid() -> CommandError { CommandError::new("BadParams", "каталог объявлений не прошёл проверку") }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn campaigns_obey_dates_and_reject_unsafe_links() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let mut item = serde_json::json!({ "id": "internal", "revision": "1", "title": {"ru": "Продукт", "en": "Product"}, "body": {"ru": "Описание", "en": "Description"},
            "action_label": {"ru": "Открыть", "en": "Open"}, "url": "https://example.test/product", "tone": "promotion", "dismissible": true, "starts_at": 10, "ends_at": 20 });
        let write = |item: &serde_json::Value| std::fs::write(paths.root().join("announcements.json"), serde_json::to_vec(&vec![item]).unwrap()).unwrap();
        write(&item);
        assert_eq!(list(&paths, 15).unwrap().len(), 1);
        assert!(list(&paths, 9).unwrap().is_empty());
        assert!(list(&paths, 20).unwrap().is_empty());
        for url in ["javascript:alert(1)", "file:///etc/passwd", "http://example.test", "https://user:secret@example.test"] {
            item["url"] = url.into(); write(&item); assert!(list(&paths, 15).is_err());
        }
    }
}
