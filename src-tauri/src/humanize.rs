//! Очеловечивание ввода профиля. У профиля — seed персоны
//! движка и настройки; движок получает их флагами `--fingerprint-humanize*` (`engine/ombra/flags.rs`) и сам выводит из
//! seed почерк: скорость и ритм набора, опечатки, руку на мыши, колесо или тачпад.

use crate::fingerprint::{FieldError, Os};
use crate::store::StoreError;
use serde::{Deserialize, Serialize};

/// Пресет движка — множители поверх персоны (ТЗ 7.4): `careful` медленнее и с дрейфом курсора в простое, `fast` быстрее
/// и без опечаток.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    #[default]
    Default,
    Careful,
    Fast,
}

impl Preset {
    pub fn as_str(self) -> &'static str {
        match self {
            Preset::Default => "default",
            Preset::Careful => "careful",
            Preset::Fast => "fast",
        }
    }

    pub fn parse(value: &str) -> Option<Preset> {
        [Preset::Default, Preset::Careful, Preset::Fast].into_iter().find(|p| p.as_str() == value)
    }
}

/// Настройки профиля. Поле, которого нет в сохранённом JSON, — по умолчанию: включено у всех профилей, и у прежних
/// (решение владельца 2026-10-11), пресет `default`, реальный ввод во время автоматизации блокируется (ТЗ 8.2 п. 3),
/// горячей клавиши «Вставить как человек» нет (D10). Незнакомый ключ пропускается: столбец — JSON без схемы, его пишут и
/// другие сборки.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Humanize {
    pub enabled: bool,
    pub preset: Preset,
    pub block_input: bool,
    /// Горячая клавиша «Вставить как человек» в форме `Ctrl+Alt+V` (`paste_key_valid`); пусто — нет.
    pub paste_key: String,
}

impl Default for Humanize {
    fn default() -> Self {
        Self { enabled: true, preset: Preset::Default, block_input: true, paste_key: String::new() }
    }
}

impl Humanize {
    /// Значение столбца `profiles.humanize`.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("настройки сериализуются")
    }

    /// Только объект: serde читает и массив как структуру (`[true]` стало бы `enabled: true`).
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        if !json.trim_start().starts_with('{') {
            return Err(serde::de::Error::custom("ожидался объект"));
        }
        serde_json::from_str(json)
    }
}

/// Горячая клавиша в форме, которую принимает движок (`HumanizedPasteAccelerator`, патч 135): ⌃ (Ctrl, Control) и/или
/// ⌥ (Alt, Opt, Option), по желанию ⇧ (Shift) и одна латинская буква или цифра через «+», каждый модификатор не больше
/// одного раза. ⌘ (Cmd, Command, Meta) не принимается: его сочетания занимает меню Chrome (⇧⌘V — «Вставить с сохранением
/// стиля»), и клавиша не срабатывала бы; без ⌃ и ⌥ (один ⇧) сочетание отняло бы у страниц профиля заглавную букву
/// (решения D10, D53 движка). Пусто — клавиши нет.
pub fn paste_key_valid(key: &str) -> bool {
    // Самое длинное верное написание — «Control + Option + Shift + V», 28 байт; пробелы вокруг «+» не должны раздувать
    // значение, которое уходит в командную строку движка.
    const MAX_LEN: usize = 32;
    if key.is_empty() {
        return true;
    }
    if key.len() > MAX_LEN {
        return false;
    }
    let (mut ctrl, mut alt, mut shift, mut keys) = (0, 0, 0, 0);
    for part in key.split('+').map(|p| p.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => ctrl += 1,
            "alt" | "opt" | "option" => alt += 1,
            "shift" => shift += 1,
            p if p.len() == 1 && p.chars().all(|c| c.is_ascii_alphanumeric()) => keys += 1,
            _ => return false,
        }
    }
    ctrl + alt > 0 && ctrl <= 1 && alt <= 1 && shift <= 1 && keys == 1
}

/// Ошибка поля `humanize.paste_key` формы редактора, архива и Local API, если горячая клавиша не годится.
pub fn paste_key_error(key: &str) -> Option<FieldError> {
    (!paste_key_valid(key)).then_some(FieldError { field: "humanize.paste_key", code: "invalid" })
}

/// Новый seed персоны: 64 бита системного источника, как у seed отпечатка. Сбой источника — ошибка хранилища, как у
/// `unused_id`, а не паника: его зовут и под мьютексом базы.
pub fn new_seed() -> Result<u64, StoreError> {
    getrandom::u64().map_err(|_| StoreError::Crypto)
}

/// Почерк профиля в редакторе (ТЗ 7.5: «показ параметров персоны»): параметры персоны, выведенные из seed тем же крейтом
/// `ombra-humanize`, который собран в движок; ОС — отпечатка, как у движка (`--fingerprint-platform`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PersonaView {
    pub generator_version: u32,
    /// Скорость набора, клавиш в секунду.
    pub keys_per_sec: f64,
    /// Опечатки на символ.
    pub typo_rate: f64,
    /// Удержание клавиши, медиана, мс.
    pub dwell_ms: f64,
    /// Множитель длительности движений мыши.
    pub mouse_speed: f64,
    /// `trackpad` или `wheel`.
    pub scroll_device: &'static str,
}

pub fn persona(seed: u64, os: Os) -> PersonaView {
    let os = match os {
        Os::Macos => ombra_humanize::Os::Macos,
        Os::Windows => ombra_humanize::Os::Windows,
    };
    let p = ombra_humanize::Persona::from_seed(seed, os);
    PersonaView {
        generator_version: p.generator_version,
        keys_per_sec: p.keyboard.keys_per_sec,
        typo_rate: p.keyboard.typo_rate,
        dwell_ms: p.keyboard.dwell.median,
        mouse_speed: p.mouse.speed_factor,
        scroll_device: match p.scroll.device {
            ombra_humanize::persona::ScrollDevice::Trackpad => "trackpad",
            ombra_humanize::persona::ScrollDevice::Wheel => "wheel",
        },
    }
}

/// Горячие клавиши, которые принимают и отвергают все пути (форма, архив, Local API): их тесты гоняют одни и те же.
#[cfg(test)]
pub(crate) const ACCEPTED_KEYS: [&str; 3] = ["Ctrl+Alt+V", "Alt+1", "Ctrl+Shift+K"];

#[cfg(test)]
pub(crate) fn refused_keys() -> Vec<String> {
    let mut keys: Vec<String> = ["Shift+V", "Cmd+Shift+V", "Cmd+V", "Ctrl+Ctrl+V"].map(String::from).into();
    keys.push(format!("Ctrl + Alt +{}V", " ".repeat(64)));
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_what_the_json_lacks() {
        assert_eq!(Humanize::from_json("{}").unwrap(), Humanize::default());
        let h = Humanize::from_json(r#"{"preset":"careful","paste_key":"Ctrl+Alt+V"}"#).unwrap();
        assert_eq!((h.enabled, h.preset, h.block_input, h.paste_key.as_str()), (true, Preset::Careful, true, "Ctrl+Alt+V"));
        assert_eq!(Humanize::from_json(&h.to_json()).unwrap(), h);
        assert_eq!(Humanize::from_json(r#"{"speed":2}"#).unwrap(), Humanize::default(), "незнакомый ключ пропускается");
        assert!(
            Humanize::from_json(r#"{"preset":"slow"}"#).is_err(),
            "незнакомый пресет — ошибка самого from_json: к умолчаниям возвращается только читатель строки профиля"
        );
    }

    #[test]
    fn paste_key_takes_ctrl_or_alt_and_refuses_cmd() {
        for good in ACCEPTED_KEYS.iter().copied().chain(["", "ctrl + alt + v", "Option+1", "Control+Option+Shift+P"]) {
            assert!(paste_key_valid(good), "{good}");
            assert_eq!(paste_key_error(good), None, "{good}");
        }
        for bad in refused_keys().iter().map(String::as_str).chain(["V", "Ctrl", "Ctrl+", "Ctrl+VV", "Ctrl+V+B", "Ctrl+F1",
                                                                     "Hyper+V", "Ctrl+Ж", "Meta+Control+P", "Ctrl+Control+V"]) {
            assert!(!paste_key_valid(bad), "{bad}");
            assert_eq!(paste_key_error(bad), Some(FieldError { field: "humanize.paste_key", code: "invalid" }), "{bad}");
        }
    }

    #[test]
    fn persona_is_the_engines_and_stable() {
        let a = persona(42, Os::Macos);
        assert_eq!(a, persona(42, Os::Macos), "один seed — один почерк");
        assert_ne!(a, persona(43, Os::Macos));
        assert_eq!(a.generator_version, ombra_humanize::persona::GENERATOR_VERSION);
        // Приоры движка — набор Aalto 136M (D63 движка): опоры клавиш в секунду и опечаток.
        assert!((2.002002002002002..=11.627906976744185).contains(&a.keys_per_sec) && (0.0..=0.1338742183529251).contains(&a.typo_rate));
    }
}
