//! Синхронизатор окон: действия главного окна повторяются в ведомых.
pub mod model;
mod replay;
pub mod session;
#[cfg(test)]
mod live_tests;

use crate::app::CommandError;
use crate::fingerprint::FieldError;
use crate::store::Store;
use serde::{Deserialize, Serialize};

/// Ключ настроек синхронизатора в таблице настроек.
pub const SETTINGS_KEY: &str = "sync.settings";
/// Наибольшая задержка, мс.
const MAX_DELAY: u32 = 10_000;

/// «Полная копия» повторяет всё как есть, «С вариациями» собирает набор текста в куски.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    FullCopy,
    /// Прежнее имя в сохранённых настройках — `adspower`.
    #[serde(alias = "adspower")]
    Varied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncSettings {
    pub mode: Mode,
    pub clicks: bool,
    pub typing: bool,
    pub scroll: bool,
    pub mouse: bool,
    /// Разброс задержки перед кликом, мс: `[от, до]`.
    pub click_delay: [u32; 2],
    pub typing_delay: [u32; 2],
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self { mode: Mode::FullCopy, clicks: true, typing: true, scroll: true, mouse: true, click_delay: [100, 600], typing_delay: [100, 600] }
    }
}

/// Сохранённые настройки; нет записи или она негодна — умолчание.
pub fn load(store: &Store) -> SyncSettings {
    store.get_setting(SETTINGS_KEY).ok().flatten().and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
}

pub fn save(store: &Store, s: &SyncSettings) -> Result<(), CommandError> {
    let bad = |d: [u32; 2]| d[0] > d[1] || d[1] > MAX_DELAY;
    let fields: Vec<FieldError> = [("click_delay", s.click_delay), ("typing_delay", s.typing_delay)]
        .into_iter()
        .filter(|(_, d)| bad(*d))
        .map(|(field, _)| FieldError { field, code: "out_of_range" })
        .collect();
    if !fields.is_empty() {
        return Err(CommandError::validation(fields));
    }
    // Сериализация не падает: только числа, флаги и перечисление.
    store.set_setting(SETTINGS_KEY, &serde_json::to_string(s).expect("настройки сериализуются"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::memory_store;

    #[test]
    fn settings_default_roundtrip_and_validation() {
        let store = memory_store();
        assert_eq!(load(&store), SyncSettings::default());
        let s = SyncSettings { mode: Mode::Varied, typing: false, click_delay: [10, 20], ..Default::default() };
        save(&store, &s).unwrap();
        assert_eq!(load(&store), s);
        assert!(store.get_setting(SETTINGS_KEY).unwrap().unwrap().contains("\"varied\""));

        let field = |s: SyncSettings| {
            let e = save(&store, &s).unwrap_err();
            assert_eq!(e.code, "Validation");
            let f = e.fields.unwrap();
            assert_eq!(f[0].code, "out_of_range");
            f[0].field
        };
        assert_eq!(field(SyncSettings { click_delay: [700, 600], ..Default::default() }), "click_delay");
        assert_eq!(field(SyncSettings { typing_delay: [0, 20000], ..Default::default() }), "typing_delay");
        assert_eq!(load(&store), s, "отказ не затирает сохранённое");

        store.set_setting(SETTINGS_KEY, "мусор").unwrap();
        assert_eq!(load(&store), SyncSettings::default());

        store.set_setting(SETTINGS_KEY, r#"{"mode":"adspower"}"#).unwrap();
        assert_eq!(load(&store).mode, Mode::Varied, "прежнее имя режима читается");
    }
}
