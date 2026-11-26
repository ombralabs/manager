use serde_json::{Map, Value, json};
use std::io;
use std::path::Path;

/// Записывает в `Default/Preferences` запрет UDP мимо прокси.
/// Ключ командной строки `--force-webrtc-ip-handling-policy` этого не делает (этап 0, контроль на Chrome 146).
pub fn apply_webrtc_policy(profile_dir: &Path) -> io::Result<()> {
    let path = profile_dir.join("Default").join("Preferences");
    let mut root = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({})),
        Err(e) if e.kind() == io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e),
    };
    if !root.is_object() {
        root = json!({});
    }
    let object = root.as_object_mut().expect("проверено выше");
    let webrtc = object.entry("webrtc").or_insert_with(|| Value::Object(Map::new()));
    if !webrtc.is_object() {
        *webrtc = Value::Object(Map::new());
    }
    webrtc
        .as_object_mut()
        .expect("проверено выше")
        .insert("ip_handling_policy".into(), json!("disable_non_proxied_udp"));

    std::fs::create_dir_all(path.parent().expect("у Preferences есть папка"))?;
    let tmp = path.with_extension("ombra-tmp");
    std::fs::write(&tmp, serde_json::to_vec(&root)?)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(dir: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("Default/Preferences")).unwrap()).unwrap()
    }

    #[test]
    fn webrtc_policy_survives_missing_and_corrupt_preferences() {
        let dir = tempfile::tempdir().unwrap();
        apply_webrtc_policy(dir.path()).unwrap();
        assert_eq!(read(dir.path())["webrtc"]["ip_handling_policy"], "disable_non_proxied_udp");

        std::fs::write(dir.path().join("Default/Preferences"), "{not json").unwrap();
        apply_webrtc_policy(dir.path()).unwrap();
        assert_eq!(read(dir.path())["webrtc"]["ip_handling_policy"], "disable_non_proxied_udp");
    }

    #[test]
    fn other_keys_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Default")).unwrap();
        std::fs::write(
            dir.path().join("Default/Preferences"),
            r#"{"profile":{"name":"Магазин"},"webrtc":{"ip_handling_policy":"default","multiple_routes_enabled":false}}"#,
        )
        .unwrap();
        apply_webrtc_policy(dir.path()).unwrap();
        let prefs = read(dir.path());
        assert_eq!(prefs["profile"]["name"], "Магазин");
        assert_eq!(prefs["webrtc"]["multiple_routes_enabled"], false);
        assert_eq!(prefs["webrtc"]["ip_handling_policy"], "disable_non_proxied_udp");
    }
}
