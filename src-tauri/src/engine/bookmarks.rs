use crate::store::bookmarks::Bookmark;
use serde_json::{Map, Value, json};
use std::io;
use std::path::Path;

const FOLDER: &str = "Ombra";

fn skeleton() -> Value {
    let root = |name: &str, id: &str| json!({"children": [], "type": "folder", "name": name, "id": id});
    json!({"roots": {
        "bookmark_bar": root("Bookmarks bar", "1"),
        "other": root("Other bookmarks", "2"),
        "synced": root("Mobile bookmarks", "3"),
    }, "version": 1})
}

fn max_id(v: &Value) -> u64 {
    let own = v.get("id").and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or(0);
    let kids = v.get("children").and_then(Value::as_array).map_or(0, |c| c.iter().map(max_id).max().unwrap_or(0));
    let roots = v.get("roots").and_then(Value::as_object).map_or(0, |r| r.values().map(max_id).max().unwrap_or(0));
    own.max(kids).max(roots)
}

/// Приводит папку «Ombra» на панели закладок к списку профиля; остальное в `Default/Bookmarks` не трогает.
/// Пустой список удаляет папку. `checksum` убирается: Chromium пересчитает его сам.
pub fn write_ombra_folder(profile_dir: &Path, list: &[Bookmark]) -> io::Result<()> {
    let path = profile_dir.join("Default").join("Bookmarks");
    // Каркас — только если файла нет; испорченный файл не трогаем: в нём могут быть закладки пользователя.
    let mut root = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .filter(|v| v["roots"]["bookmark_bar"]["children"].is_array())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Bookmarks не разобран"))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => skeleton(),
        Err(e) => return Err(e),
    };
    root.as_object_mut().expect("объект проверен выше").remove("checksum");
    let mut next = max_id(&root);
    let mut fresh = || {
        next += 1;
        next.to_string()
    };
    let folder_id = fresh();
    let children: Vec<Value> = list
        .iter()
        .map(|b| json!({"type": "url", "name": b.title, "url": b.url, "id": fresh(), "date_added": "0"}))
        .collect();
    let bar = root["roots"]["bookmark_bar"]["children"].as_array_mut().expect("массив проверен выше");
    bar.retain(|c| !(c["type"] == "folder" && c["name"] == FOLDER));
    if !list.is_empty() {
        let mut folder = Map::new();
        folder.insert("type".into(), json!("folder"));
        folder.insert("name".into(), json!(FOLDER));
        folder.insert("id".into(), json!(folder_id));
        folder.insert("date_added".into(), json!("0"));
        folder.insert("children".into(), Value::Array(children));
        bar.push(Value::Object(folder));
    }
    std::fs::create_dir_all(path.parent().expect("у Bookmarks есть папка"))?;
    let tmp = path.with_extension("ombra-tmp");
    std::fs::write(&tmp, serde_json::to_vec(&root)?)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ombra_folder_is_replaced_and_foreign_bookmarks_survive() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Default/Bookmarks");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"checksum":"x","roots":{"bookmark_bar":{"children":[{"type":"url","name":"Mine","url":"https://mine.test/","id":"5"}],"type":"folder","name":"Bookmarks bar","id":"1"},"other":{"children":[],"type":"folder","name":"Other","id":"2"},"synced":{"children":[],"type":"folder","name":"Mobile","id":"3"}},"version":1}"#).unwrap();
        write_ombra_folder(dir.path(), &[Bookmark { title: "Shop".into(), url: "https://shop.test/".into() }]).unwrap();
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let bar = json["roots"]["bookmark_bar"]["children"].as_array().unwrap();
        assert_eq!(bar[0]["name"], "Mine", "чужая закладка цела");
        assert_eq!(bar[1]["name"], "Ombra");
        assert_eq!(bar[1]["children"][0]["url"], "https://shop.test/");
        assert_eq!(bar[1]["id"], "6");
        assert!(json.get("checksum").is_none(), "сумму пересчитает Chromium");
        write_ombra_folder(dir.path(), &[]).unwrap();
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(json["roots"]["bookmark_bar"]["children"].as_array().unwrap().len(), 1, "папка удалена");
        std::fs::remove_file(&file).unwrap();
        write_ombra_folder(dir.path(), &[Bookmark { title: "A".into(), url: "https://a.test/".into() }]).unwrap();
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["roots"]["bookmark_bar"]["children"][0]["name"], "Ombra");
    }

    #[test]
    fn unusable_existing_file_is_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Default/Bookmarks");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let list = [Bookmark { title: "A".into(), url: "https://a.test/".into() }];
        for content in ["{not json", r#"{"roots":{"other":{}},"version":1}"#, "[]"] {
            std::fs::write(&file, content).unwrap();
            let err = write_ombra_folder(dir.path(), &list).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData);
            assert_eq!(std::fs::read_to_string(&file).unwrap(), content);
            assert!(!file.with_extension("ombra-tmp").exists());
        }
    }
}
