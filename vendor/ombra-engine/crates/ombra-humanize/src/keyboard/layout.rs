//! Keyboard layouts as data (decision D11) and the physical keyboard under them.
//!
//! A layout maps characters to physical keys; supporting a language means adding a table (and its
//! K-01 reference capture), not code. v1: en-US and ru (macOS "Russian – PC", the Windows ЙЦУКЕН)
//! on an ANSI keyboard. `mac_key_code` reproduces Chrome on macOS (`KeyboardCodeFromNSEvent`,
//! ui/events/keycodes/keyboard_code_conversion_mac.mm); Windows and Linux values arrive with stage
//! Э7 (decision D12).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hand {
    Left,
    Right,
    Thumb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Finger {
    Pinky,
    Ring,
    Middle,
    Index,
    Thumb,
}

/// A key of an ANSI keyboard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysicalKey {
    /// UI Events `code`.
    pub code: &'static str,
    /// Windows virtual-key code Chrome gives the physical key (`KeyboardCodeFromKeyCode`).
    pub vk: u16,
    /// 0 is the digit row, 4 the space bar.
    pub row: u8,
    /// Key centre in key widths from the left edge (standard row stagger).
    pub x: f32,
    pub hand: Hand,
    pub finger: Finger,
}

const fn key(code: &'static str, vk: u16, row: u8, x: f32, hand: Hand, finger: Finger) -> PhysicalKey {
    PhysicalKey { code, vk, row, x, hand, finger }
}

use Finger::{Index, Middle, Pinky, Ring};
use Hand::{Left, Right};

pub static KEYS: [PhysicalKey; 50] = [
    key("Backquote", 192, 0, 0.5, Left, Pinky),
    key("Digit1", 49, 0, 1.5, Left, Pinky),
    key("Digit2", 50, 0, 2.5, Left, Ring),
    key("Digit3", 51, 0, 3.5, Left, Middle),
    key("Digit4", 52, 0, 4.5, Left, Index),
    key("Digit5", 53, 0, 5.5, Left, Index),
    key("Digit6", 54, 0, 6.5, Right, Index),
    key("Digit7", 55, 0, 7.5, Right, Index),
    key("Digit8", 56, 0, 8.5, Right, Middle),
    key("Digit9", 57, 0, 9.5, Right, Ring),
    key("Digit0", 48, 0, 10.5, Right, Pinky),
    key("Minus", 189, 0, 11.5, Right, Pinky),
    key("Equal", 187, 0, 12.5, Right, Pinky),
    key("Backspace", 8, 0, 14.0, Right, Pinky),
    key("KeyQ", 81, 1, 2.0, Left, Pinky),
    key("KeyW", 87, 1, 3.0, Left, Ring),
    key("KeyE", 69, 1, 4.0, Left, Middle),
    key("KeyR", 82, 1, 5.0, Left, Index),
    key("KeyT", 84, 1, 6.0, Left, Index),
    key("KeyY", 89, 1, 7.0, Right, Index),
    key("KeyU", 85, 1, 8.0, Right, Index),
    key("KeyI", 73, 1, 9.0, Right, Middle),
    key("KeyO", 79, 1, 10.0, Right, Ring),
    key("KeyP", 80, 1, 11.0, Right, Pinky),
    key("BracketLeft", 219, 1, 12.0, Right, Pinky),
    key("BracketRight", 221, 1, 13.0, Right, Pinky),
    key("Backslash", 220, 1, 14.25, Right, Pinky),
    key("KeyA", 65, 2, 2.25, Left, Pinky),
    key("KeyS", 83, 2, 3.25, Left, Ring),
    key("KeyD", 68, 2, 4.25, Left, Middle),
    key("KeyF", 70, 2, 5.25, Left, Index),
    key("KeyG", 71, 2, 6.25, Left, Index),
    key("KeyH", 72, 2, 7.25, Right, Index),
    key("KeyJ", 74, 2, 8.25, Right, Index),
    key("KeyK", 75, 2, 9.25, Right, Middle),
    key("KeyL", 76, 2, 10.25, Right, Ring),
    key("Semicolon", 186, 2, 11.25, Right, Pinky),
    key("Quote", 222, 2, 12.25, Right, Pinky),
    key("Enter", 13, 2, 13.6, Right, Pinky),
    key("KeyZ", 90, 3, 2.75, Left, Pinky),
    key("KeyX", 88, 3, 3.75, Left, Ring),
    key("KeyC", 67, 3, 4.75, Left, Middle),
    key("KeyV", 86, 3, 5.75, Left, Index),
    key("KeyB", 66, 3, 6.75, Left, Index),
    key("KeyN", 78, 3, 7.75, Right, Index),
    key("KeyM", 77, 3, 8.75, Right, Index),
    key("Comma", 188, 3, 9.75, Right, Middle),
    key("Period", 190, 3, 10.75, Right, Ring),
    key("Slash", 191, 3, 11.75, Right, Pinky),
    key("Space", 32, 4, 7.0, Hand::Thumb, Finger::Thumb),
];

pub fn physical(code: &str) -> Option<&'static PhysicalKey> {
    KEYS.iter().find(|k| k.code == code)
}

/// A modifier or editing key with no character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpecialKey {
    pub code: &'static str,
    pub key: &'static str,
    pub key_code: u16,
    pub location: u8,
}

pub const BACKSPACE: SpecialKey = SpecialKey { code: "Backspace", key: "Backspace", key_code: 8, location: 0 };
pub const ENTER: SpecialKey = SpecialKey { code: "Enter", key: "Enter", key_code: 13, location: 0 };
pub const SHIFT_LEFT: SpecialKey = SpecialKey { code: "ShiftLeft", key: "Shift", key_code: 16, location: 1 };
pub const SHIFT_RIGHT: SpecialKey = SpecialKey { code: "ShiftRight", key: "Shift", key_code: 16, location: 2 };
/// The paste chord's modifier: Command on macOS (the paste reference, `qa/humanize/reference/macos-paste-*.json`),
/// Control elsewhere (stage Э7 checks it against Windows and Linux).
pub const META_LEFT: SpecialKey = SpecialKey { code: "MetaLeft", key: "Meta", key_code: 91, location: 1 };
pub const CONTROL_LEFT: SpecialKey = SpecialKey { code: "ControlLeft", key: "Control", key_code: 17, location: 1 };

/// A layout: per physical key, its character without and with Shift.
#[derive(Debug, PartialEq)]
pub struct Layout {
    pub id: &'static str,
    pub keys: &'static [(&'static str, char, char)],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keystroke {
    pub key: &'static PhysicalKey,
    pub shift: bool,
}

impl Layout {
    pub fn keystroke(&self, c: char) -> Option<Keystroke> {
        self.keys.iter().find_map(|&(code, base, shifted)| {
            let shift = match c {
                _ if c == base => false,
                _ if c == shifted => true,
                _ => return None,
            };
            Some(Keystroke { key: physical(code)?, shift })
        })
    }

    pub fn contains(&self, c: char) -> bool {
        self.keystroke(c).is_some()
    }

    /// The character a key gives in this layout.
    pub fn character(&self, code: &str, shift: bool) -> Option<char> {
        self.keys.iter().find(|k| k.0 == code).map(|&(_, base, shifted)| if shift { shifted } else { base })
    }

    /// Keys a finger can slip to from `code` (centres within 1.25 key widths) that exist in this
    /// layout: the targets of adjacent-key typos.
    pub fn neighbors(&self, code: &str) -> Vec<&'static PhysicalKey> {
        let Some(from) = physical(code) else { return Vec::new() };
        KEYS.iter()
            .filter(|k| k.code != code && k.row < 4 && self.keys.iter().any(|e| e.0 == k.code))
            .filter(|k| {
                let (dx, dy) = (k.x - from.x, f32::from(k.row) - f32::from(from.row));
                (dx * dx + dy * dy).sqrt() <= 1.25
            })
            .collect()
    }
}

pub static EN_US: Layout = Layout {
    id: "en-US",
    keys: &[
        ("Backquote", '`', '~'), ("Digit1", '1', '!'), ("Digit2", '2', '@'), ("Digit3", '3', '#'), ("Digit4", '4', '$'),
        ("Digit5", '5', '%'), ("Digit6", '6', '^'), ("Digit7", '7', '&'), ("Digit8", '8', '*'), ("Digit9", '9', '('),
        ("Digit0", '0', ')'), ("Minus", '-', '_'), ("Equal", '=', '+'),
        ("KeyQ", 'q', 'Q'), ("KeyW", 'w', 'W'), ("KeyE", 'e', 'E'), ("KeyR", 'r', 'R'), ("KeyT", 't', 'T'),
        ("KeyY", 'y', 'Y'), ("KeyU", 'u', 'U'), ("KeyI", 'i', 'I'), ("KeyO", 'o', 'O'), ("KeyP", 'p', 'P'),
        ("BracketLeft", '[', '{'), ("BracketRight", ']', '}'), ("Backslash", '\\', '|'),
        ("KeyA", 'a', 'A'), ("KeyS", 's', 'S'), ("KeyD", 'd', 'D'), ("KeyF", 'f', 'F'), ("KeyG", 'g', 'G'),
        ("KeyH", 'h', 'H'), ("KeyJ", 'j', 'J'), ("KeyK", 'k', 'K'), ("KeyL", 'l', 'L'), ("Semicolon", ';', ':'),
        ("Quote", '\'', '"'),
        ("KeyZ", 'z', 'Z'), ("KeyX", 'x', 'X'), ("KeyC", 'c', 'C'), ("KeyV", 'v', 'V'), ("KeyB", 'b', 'B'),
        ("KeyN", 'n', 'N'), ("KeyM", 'm', 'M'), ("Comma", ',', '<'), ("Period", '.', '>'), ("Slash", '/', '?'),
        ("Space", ' ', ' '),
    ],
};

/// macOS «Russian – PC» (`com.apple.keylayout.RussianWin`, decision D21): Shift+ё types a Latin Ë
/// (U+00CB), as the K-01 reference shows; the Cyrillic Ё has no key there and comes from the picker.
pub static RU: Layout = Layout {
    id: "ru",
    keys: &[
        ("Backquote", 'ё', '\u{00CB}'), ("Digit1", '1', '!'), ("Digit2", '2', '"'), ("Digit3", '3', '№'), ("Digit4", '4', ';'),
        ("Digit5", '5', '%'), ("Digit6", '6', ':'), ("Digit7", '7', '?'), ("Digit8", '8', '*'), ("Digit9", '9', '('),
        ("Digit0", '0', ')'), ("Minus", '-', '_'), ("Equal", '=', '+'),
        ("KeyQ", 'й', 'Й'), ("KeyW", 'ц', 'Ц'), ("KeyE", 'у', 'У'), ("KeyR", 'к', 'К'), ("KeyT", 'е', 'Е'),
        ("KeyY", 'н', 'Н'), ("KeyU", 'г', 'Г'), ("KeyI", 'ш', 'Ш'), ("KeyO", 'щ', 'Щ'), ("KeyP", 'з', 'З'),
        ("BracketLeft", 'х', 'Х'), ("BracketRight", 'ъ', 'Ъ'), ("Backslash", '\\', '/'),
        ("KeyA", 'ф', 'Ф'), ("KeyS", 'ы', 'Ы'), ("KeyD", 'в', 'В'), ("KeyF", 'а', 'А'), ("KeyG", 'п', 'П'),
        ("KeyH", 'р', 'Р'), ("KeyJ", 'о', 'О'), ("KeyK", 'л', 'Л'), ("KeyL", 'д', 'Д'), ("Semicolon", 'ж', 'Ж'),
        ("Quote", 'э', 'Э'),
        ("KeyZ", 'я', 'Я'), ("KeyX", 'ч', 'Ч'), ("KeyC", 'с', 'С'), ("KeyV", 'м', 'М'), ("KeyB", 'и', 'И'),
        ("KeyN", 'т', 'Т'), ("KeyM", 'ь', 'Ь'), ("Comma", 'б', 'Б'), ("Period", 'ю', 'Ю'), ("Slash", '.', ','),
        ("Space", ' ', ' '),
    ],
};

/// Layouts a profile language types with, the usual starting one first (D11). A language without
/// its own table types Latin through en-US; characters outside every layout go as IME commits.
pub fn layouts_for_language(language: &str) -> Vec<&'static Layout> {
    let primary = language.trim().split([',', ';', '-', '_', ' ']).next().unwrap_or("").to_ascii_lowercase();
    match primary.as_str() {
        "ru" => vec![&RU, &EN_US],
        _ => vec![&EN_US],
    }
}

/// Chrome's `windows_key_code` on macOS (`KeyboardCodeFromNSEvent`): the digit row reports its own
/// key whatever it types; Latin letters and US punctuation map by character; anything else, such as
/// Cyrillic, by the physical key. So «.» typed on the ru Slash key reports 190, not 191.
pub fn mac_key_code(c: char, key: &PhysicalKey) -> u16 {
    if key.code.starts_with("Digit") {
        return key.vk;
    }
    us_character_vk(c).unwrap_or(key.vk)
}

/// `KeyboardCodeFromCharCode` for printable characters.
fn us_character_vk(c: char) -> Option<u16> {
    Some(match c {
        'a'..='z' => c as u16 - 'a' as u16 + 65,
        'A'..='Z' => c as u16 - 'A' as u16 + 65,
        '0'..='9' => c as u16 - '0' as u16 + 48,
        ';' | ':' => 186,
        '=' | '+' => 187,
        ',' | '<' => 188,
        '-' | '_' => 189,
        '.' | '>' => 190,
        '/' | '?' => 191,
        '`' | '~' => 192,
        '[' | '{' => 219,
        '\\' | '|' => 220,
        ']' | '}' => 221,
        '\'' | '"' => 222,
        _ => return None,
    })
}
