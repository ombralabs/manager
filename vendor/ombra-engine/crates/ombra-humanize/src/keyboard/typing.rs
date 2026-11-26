//! The typing planner (spec 6.2–6.3): the text of `Input.insertText` becomes the key events a
//! person with this persona would produce, timed from the start of the plan.

use super::layout::{layouts_for_language, mac_key_code, physical, Hand, Keystroke, Layout, PhysicalKey, SpecialKey, BACKSPACE, CONTROL_LEFT, ENTER, META_LEFT, SHIFT_LEFT, SHIFT_RIGHT};
use super::timing::{pair_class, PairClass, Rhythm, BACKSPACE_SPEEDUP};
use super::typos::{corrupt, typo_kind};
use crate::persona::{Os, PasteTraits, Persona, Preset};
use crate::rng::Rng;
use serde::Serialize;

/// After these the next word waits longer (spec 6.3).
const PUNCTUATION: &str = ".,;:!?";
/// Share of typos noticed only after 1–3 more characters (spec 6.3: "иногда").
const LATE_NOTICE_P: f64 = 0.3; // prior
/// The paste chord's modifier stays down at least this long after V's keydown, ms.
const MIN_CHORD_HOLD_MS: f64 = 20.0; // prior

/// The focused field, from `ui::TextInputType`. Fields that take no typed text (date, time and
/// the like) are not planned: the caller keeps the stock `insertText`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Text,
    Search,
    Telephone,
    Password,
    Email,
    Number,
    Url,
    TextArea,
    ContentEditable,
}

impl Field {
    /// Enter types a line break only here; in a one-line field it would submit the form.
    pub fn multiline(self) -> bool {
        matches!(self, Field::TextArea | Field::ContentEditable)
    }

    /// Spec 6.2: no typos in PASSWORD, EMAIL, NUMBER and URL fields.
    pub fn typos(self) -> bool {
        !matches!(self, Field::Password | Field::Email | Field::Number | Field::Url)
    }
}

pub struct TypingContext<'a> {
    pub field: Field,
    /// The profile's language (first of `--accept-lang`): picks the layouts (D11).
    pub language: &'a str,
    /// The layout the previous typing in this tab ended on; `None` starts on the language's first.
    pub layout: Option<&'a str>,
    /// The field has just been focused: the first key waits 300–900 ms.
    pub just_focused: bool,
    /// The last key the tab sent into this field before this text: the rhythm goes on from it.
    /// Ignored in a field just focused.
    pub previous: Option<Previous>,
    /// Enter after the text, whatever the field: the in-browser agent's "type and submit"
    /// (decision D32). Input.insertText never asks for it.
    pub submit: bool,
    /// The latest time from the plan's start a key or a picked character of the text may start at (decision D42):
    /// typing that would start anything later is not typed at all, the whole text is pasted instead (decision D50).
    /// The `submit` Enter is not bound by it. `None` types the whole text.
    pub max_ms: Option<f64>,
    /// Paste the text whatever its typing would take: the executor's rule for long text (decision D50).
    pub paste: bool,
}

/// A key sent before the plan, times from the plan's start. Playwright's and Puppeteer's `type()`
/// send each character outside their US keyboard (Cyrillic) as an `Input.insertText` of its own,
/// so without it every such character would come at once after the last one's keyup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Previous {
    /// The character it typed; `None` for a key that types none.
    pub ch: Option<char>,
    pub down_ms: f64,
    pub up_ms: f64,
}

/// Key event fields as Chrome on macOS reports them — for every persona until stage Э7 (D12).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Key {
    pub code: &'static str,
    pub key: String,
    pub key_code: u16,
    pub location: u8,
}

impl Key {
    fn typed(c: char, key: &'static PhysicalKey) -> Key {
        Key { code: key.code, key: c.to_string(), key_code: mac_key_code(c, key), location: 0 }
    }

    fn special(key: SpecialKey) -> Key {
        Key { code: key.code, key: key.key.to_string(), key_code: key.key_code, location: key.location }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Action {
    /// `rawKeyDown`, then `char` with `text` when the key types one ('\r' for Enter).
    KeyDown { key: Key, text: Option<char> },
    KeyUp { key: Key },
    /// Text through the IME path: characters outside the persona's layouts, and '\t'.
    ImeCommit { text: String },
    /// The browser's paste of `text` as the clipboard (decision D50): right at the chord's keydown, as Chrome on macOS
    /// pastes on Command+V's; the executor serves `text` to that paste's clipboard reads.
    Paste { text: String },
}

/// Where an action sits in a typo episode. Episodes are contiguous blocks: `Typo`, then `Fix`,
/// then `Retype`, either of the last two possibly empty. The executor (stage Э4) cannot know which
/// typed characters the page kept (`maxlength`, input masks, the last box of a code, a field that
/// strips or rewrites what is typed), so it corrects by what the page kept: from the start of a
/// `Typo` block it measures how much the text fields the focus goes through grow (one-character
/// boxes of a code move it on; insertion events are no measure, a field may cancel or undo them),
/// and when the `Fix` block starts it skips one planned Backspace for every character of the `Typo`
/// block the fields did not keep. A skipped Backspace is skipped with its keyup. The final value
/// then equals the client's text as the page would take it (spec 6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Role {
    Plain,
    Typo,
    Fix,
    Retype,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TimedAction {
    /// From the start of the plan.
    pub at_ms: f64,
    pub role: Role,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TypingPlan {
    /// In time order.
    pub actions: Vec<TimedAction>,
    /// The layout on at the end, for the next `TypingContext::layout`.
    pub layout: &'static str,
    /// The text is pasted, not typed (decision D50): `TypingContext::paste`, or typing it would start a key past
    /// `TypingContext::max_ms`.
    pub pasted: bool,
}

/// One step of the text.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Unit<'t> {
    Char(char),
    Enter,
    Tab,
    /// A character outside the layouts, typed through the system picker.
    Picked(&'t str),
}

/// Splits text into user-perceived characters, close enough for typing: a base with its combining
/// marks, variation selectors, emoji modifiers and tags, ZWJ sequences, flag pairs.
/// ponytail: not full UAX #29 (no Hangul jamo or Indic conjuncts); switch to a segmentation crate
/// once Chromium vendors one and such text matters.
fn clusters(text: &str) -> Vec<&str> {
    let extends = |c: char| {
        matches!(c, '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}' | '\u{20D0}'..='\u{20FF}'
            | '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{FE20}'..='\u{FE2F}' | '\u{1F3FB}'..='\u{1F3FF}'
            | '\u{E0020}'..='\u{E007F}' | '\u{E0100}'..='\u{E01EF}')
    };
    let regional = |c: char| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c);
    let mut out = Vec::new();
    let (mut start, mut flags) = (0, 0);
    let mut prev: Option<char> = None;
    for (i, c) in text.char_indices() {
        // Control characters (line breaks, tabs) never join, as in UAX #29.
        let joins = prev.is_some_and(|p| {
            !p.is_control() && !c.is_control() && (p == '\u{200D}' || extends(c) || (regional(c) && regional(p) && flags % 2 == 1))
        });
        if !joins && i > start {
            out.push(&text[start..i]);
            start = i;
            flags = 0;
        }
        flags += usize::from(regional(c));
        prev = Some(c);
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Line breaks: "\r\n" and a lone '\r' are one break; Enter only in a multi-line field. A one-line
/// field takes them as Blink takes `insertText` (text_field_input_type.cc): trailing breaks are
/// dropped, inner ones typed as spaces. Tab is never pressed: it would move the focus.
fn units<'t>(text: &'t str, field: Field, layouts: &[&'static Layout]) -> Vec<Unit<'t>> {
    let text = if field.multiline() { text } else { text.trim_end_matches(['\r', '\n']) };
    let clusters = clusters(text);
    let mut out = Vec::with_capacity(clusters.len());
    for (i, cluster) in clusters.iter().enumerate() {
        let mut chars = cluster.chars();
        let unit = match (chars.next(), chars.next()) {
            (Some('\r'), None) if clusters.get(i + 1) == Some(&"\n") => continue,
            (Some('\n' | '\r'), None) if field.multiline() => Unit::Enter,
            (Some('\n' | '\r'), None) => Unit::Char(' '),
            (Some('\t'), None) => Unit::Tab,
            (Some(c), None) if layouts.iter().any(|l| l.contains(c)) => Unit::Char(c),
            _ => Unit::Picked(cluster),
        };
        out.push(unit);
    }
    out
}

/// The previous key, its keyup still movable for rollover.
#[derive(Clone)]
struct Pending {
    key: Key,
    role: Role,
    down: f64,
    up: f64,
    /// A character key (not Enter or Backspace).
    typed: bool,
}

struct Typist<'a> {
    rng: Rng,
    rhythm: Rhythm<'a>,
    rollover: f64,
    shift_bias: f64,
    layouts: Vec<&'static Layout>,
    layout: &'static Layout,
    role: Role,
    actions: Vec<TimedAction>,
    /// Earliest next keydown.
    cursor: f64,
    /// Latest keyup so far.
    last_up: f64,
    /// The previous key: its character, the key, its keydown time.
    prev: Option<(Option<char>, &'static PhysicalKey, f64)>,
    pending: Option<Pending>,
    shift: Option<SpecialKey>,
    /// For word pauses: the last character typed, the last one before a space, a line break since.
    last_char: Option<char>,
    before_gap: Option<char>,
    gap_newline: bool,
    /// When the previous action was a picked character.
    picked_at: Option<f64>,
}

impl<'a> Typist<'a> {
    fn push(&mut self, at_ms: f64, action: Action) {
        self.actions.push(TimedAction { at_ms, role: self.role, action });
    }

    /// A random gap in `[low, high]` ms under the preset.
    fn gap(&mut self, low: f64, high: f64) -> f64 {
        self.rng.uniform(low, high) * self.rhythm.time_mult()
    }

    /// The latest moment anything happened: the floor for a pause.
    fn idle_since(&self) -> f64 {
        self.prev.map_or(self.cursor, |p| p.2).max(self.last_up).max(self.cursor)
    }

    fn release_pending(&mut self) {
        if let Some(p) = self.pending.take() {
            self.actions.push(TimedAction { at_ms: p.up, role: p.role, action: Action::KeyUp { key: p.key } });
            self.last_up = self.last_up.max(p.up);
        }
    }

    /// Releases the previous key around the next keydown planned at `down` and returns the
    /// keydown time, moved so that the next key comes during the previous hold with `overlap`
    /// (rollover) and after it without. Holds stay as sampled.
    fn settle_pending(&mut self, mut down: f64, overlap: bool) -> f64 {
        let Some(p) = self.pending.take() else { return down };
        // Keys before the previous one are up by the next press: never three keys down, never a
        // key pressed while it is still held.
        let floor = self.last_up + 1.0;
        down = down.max(floor);
        let gap = self.gap(5.0, 25.0); // prior
        if overlap {
            if down >= p.up {
                down = (p.up - gap).max(p.down + 0.5 * (p.up - p.down)).max(floor); // prior
            }
        } else if down < p.up + gap {
            down = p.up + gap;
        }
        self.pending = Some(p);
        self.release_pending();
        down
    }

    fn release_shift(&mut self) {
        if let Some(side) = self.shift.take() {
            self.release_pending();
            let up = self.last_up + self.gap(5.0, 40.0); // prior
            self.push(up, Action::KeyUp { key: Key::special(side) });
            self.last_up = up;
            let gap = self.gap(10.0, 40.0); // prior
            self.cursor = self.cursor.max(up + gap);
        }
    }

    /// Shift on the hand opposite the character's, leaning to the persona's side (spec 6.2).
    fn shift_side(&mut self, hand: Hand) -> SpecialKey {
        let right = match hand {
            Hand::Left => 0.75 + 0.2 * self.shift_bias,
            Hand::Right => 0.05 + 0.2 * self.shift_bias,
            Hand::Thumb => self.shift_bias,
        }; // prior
        if self.rng.chance(right) {
            SHIFT_RIGHT
        } else {
            SHIFT_LEFT
        }
    }

    fn press(&mut self, c: char, stroke: Keystroke) {
        let key = stroke.key;
        let dwell = self.rhythm.dwell(&mut self.rng);
        let at_boundary = c != ' ' && matches!(self.last_char, Some(' ' | '\n'));
        let mut down = self.cursor;
        if let Some((pc, pk, pdown)) = self.prev {
            let signature = pc.map_or(1.0, |pc| self.rhythm.signature(self.layout.id, pc, c));
            let mut interval = self.rhythm.interval(&mut self.rng, pk, key, signature);
            if at_boundary {
                let after_punctuation = self.gap_newline || self.before_gap.is_some_and(|p| PUNCTUATION.contains(p));
                interval *= self.rhythm.boundary(&mut self.rng, after_punctuation);
                if self.role == Role::Plain {
                    interval += self.rhythm.thinking(&mut self.rng);
                }
            }
            down = down.max(pdown + interval);
        }
        match (self.shift, stroke.shift) {
            (Some(_), false) => {
                self.release_shift();
                down = down.max(self.cursor);
            }
            (None, true) => {
                self.release_pending();
                let side = self.shift_side(key.hand);
                let lead = self.gap(30.0, 90.0); // prior
                let after_up = self.last_up + self.gap(10.0, 40.0); // prior
                let at = (down - lead).max(after_up).max(self.cursor);
                self.push(at, Action::KeyDown { key: Key::special(side), text: None });
                self.shift = Some(side);
                let settle = self.gap(20.0, 60.0); // prior
                down = down.max(at + settle);
            }
            _ => {
                // Rollover: only between character keys of one role, never into a word.
                let eligible = !at_boundary
                    && self.pending.as_ref().is_some_and(|p| p.typed && p.role == self.role)
                    && self.prev.is_some_and(|(_, pk, _)| matches!(pair_class(pk, key), PairClass::Alternating | PairClass::SameHand));
                let overlap = eligible && self.rng.chance(self.rollover);
                down = self.settle_pending(down, overlap);
            }
        }
        let typed = Key::typed(c, key);
        self.push(down, Action::KeyDown { key: typed.clone(), text: Some(c) });
        self.pending = Some(Pending { key: typed, role: self.role, down, up: down + dwell, typed: true });
        self.prev = Some((Some(c), key, down));
        self.picked_at = None;
        match c {
            ' ' => {}
            _ => {
                self.before_gap = Some(c);
                self.gap_newline = false;
            }
        }
        self.last_char = Some(c);
    }

    /// Enter or Backspace.
    fn special(&mut self, special: SpecialKey, text: Option<char>, speedup: f64) {
        self.release_shift();
        let key = physical(special.code).expect("special keys are on the keyboard");
        let mut down = self.cursor;
        if let Some((_, pk, pdown)) = self.prev {
            down = down.max(pdown + self.rhythm.interval(&mut self.rng, pk, key, 1.0) * speedup);
        }
        let dwell = self.rhythm.dwell(&mut self.rng);
        down = self.settle_pending(down, false);
        let k = Key::special(special);
        self.push(down, Action::KeyDown { key: k.clone(), text });
        self.pending = Some(Pending { key: k, role: self.role, down, up: down + dwell, typed: false });
        self.prev = Some((None, key, down));
        self.picked_at = None;
    }

    fn enter(&mut self) {
        self.special(ENTER, Some('\r'), 1.0);
        self.last_char = Some('\n');
        self.gap_newline = true;
    }

    fn backspace(&mut self) {
        self.special(BACKSPACE, None, BACKSPACE_SPEEDUP);
        self.last_char = None;
    }

    /// Text in one go through the IME path once the keys are up: a Tab.
    fn commit(&mut self, text: &str) {
        self.release_shift();
        self.release_pending();
        let at = self.idle_since() + self.rhythm.plain_interval(&mut self.rng);
        self.push(at, Action::ImeCommit { text: text.to_string() });
        self.cursor = at + self.rhythm.plain_interval(&mut self.rng);
        self.prev = None;
        self.picked_at = None;
        self.last_char = Some(' ');
    }

    fn pick(&mut self, text: &str) {
        self.release_shift();
        self.release_pending();
        let at = match self.picked_at {
            Some(t) => t + self.rhythm.picker(&mut self.rng, true),
            None => self.idle_since() + self.rhythm.picker(&mut self.rng, false),
        };
        self.push(at, Action::ImeCommit { text: text.to_string() });
        self.picked_at = Some(at);
        self.cursor = at + self.rhythm.back_to_keys(&mut self.rng);
        self.prev = None;
        self.last_char = None;
        self.before_gap = None;
    }

    fn layout_for(&self, c: char) -> &'static Layout {
        if self.layout.contains(c) {
            return self.layout;
        }
        self.layouts.iter().copied().find(|l| l.contains(c)).unwrap_or(self.layout)
    }

    fn use_layout(&mut self, layout: &'static Layout) {
        if layout.id == self.layout.id {
            return;
        }
        self.release_shift();
        self.release_pending();
        self.cursor = self.idle_since() + self.rhythm.layout_switch(&mut self.rng);
        self.layout = layout;
        self.prev = None;
    }

    fn type_char(&mut self, c: char) {
        let layout = self.layout_for(c);
        self.use_layout(layout);
        self.press(c, layout.keystroke(c).expect("units hold only characters of the layouts"));
    }

    /// A typo at the start of `ahead`, noticed and corrected (spec 6.3). Returns how many units of
    /// the text it typed; `None` when no typo fits there.
    fn typo(&mut self, ahead: &[Unit]) -> Option<usize> {
        let Unit::Char(first) = *ahead.first()? else { return None };
        let layout = self.layout_for(first);
        // A slip reads at most five characters: two swapped and three typed before noticing.
        let window: Vec<char> = ahead
            .iter()
            .map_while(|u| match *u {
                Unit::Char(c) if layout.contains(c) => Some(c),
                _ => None,
            })
            .take(5)
            .collect();
        let kind = typo_kind(&mut self.rng);
        let noticed_after = if self.rng.chance(LATE_NOTICE_P) { 1 + self.rng.below(3) } else { 0 };
        let (wrong, right) = corrupt(kind, &window, noticed_after, layout, &mut self.rng)?;
        let keep = wrong.iter().zip(&right).take_while(|(w, r)| w == r).count();
        let stroke = |c: char| layout.keystroke(c).expect("the slip stays on the layout");
        self.use_layout(layout);
        self.role = Role::Typo;
        for &c in &wrong {
            self.press(c, stroke(c));
        }
        self.release_shift();
        self.release_pending();
        let noticed = self.prev.map_or(self.cursor, |p| p.2) + self.rhythm.correction(&mut self.rng);
        let gap = self.gap(20.0, 60.0); // prior
        self.cursor = noticed.max(self.last_up + gap);
        self.role = Role::Fix;
        for _ in keep..wrong.len() {
            self.backspace();
        }
        self.role = Role::Retype;
        for &c in &right[keep..] {
            self.press(c, stroke(c));
        }
        self.role = Role::Plain;
        Some(right.len())
    }

    /// Pastes `text` with the chord of the persona's OS (decision D50): the modifier goes down at the next keydown,
    /// V the persona's `command_to_v` after it, the paste right at V's keydown, V held as the persona holds a key, and
    /// the modifier up the persona's release offset from V's keyup — before it when negative, as the owner's trackpad
    /// hand did in 11 chords of 12 (stage Э6б) — though never before V is down. Chrome on macOS reports V under Command
    /// as `v` in every layout and types nothing with it: its char is a system key Blink drops (no keypress in the paste
    /// reference), so the plan sends none. V released after Command reports the layout's own character (the paste
    /// reference's command-first variant), `v` in a password field, where macOS's secure input keeps an ASCII layout.
    fn paste(&mut self, text: &str, os: Os, field: Field, chord: &PasteTraits) {
        self.release_shift();
        self.release_pending();
        let modifier = Key::special(if os == Os::Macos { META_LEFT } else { CONTROL_LEFT });
        let v = Key { code: "KeyV", key: "v".to_string(), key_code: 86, location: 0 };
        let mult = self.rhythm.time_mult();
        let down = self.idle_since();
        self.push(down, Action::KeyDown { key: modifier.clone(), text: None });
        let v_down = down + chord.command_to_v.sample(&mut self.rng) * mult;
        self.push(v_down, Action::KeyDown { key: v.clone(), text: None });
        self.push(v_down, Action::Paste { text: text.to_string() });
        let v_up = v_down + self.rhythm.dwell(&mut self.rng);
        let offset = (chord.release_mean_ms + chord.release_sd_ms * self.rng.normal()) * mult;
        let up = (v_up + offset).max(v_down + MIN_CHORD_HOLD_MS);
        let v_key = match self.layout.character("KeyV", false) {
            Some(c) if up < v_up && field != Field::Password => Key { key: c.to_string(), ..v },
            _ => v,
        };
        self.push(v_up, Action::KeyUp { key: v_key });
        self.push(up, Action::KeyUp { key: modifier });
        self.last_up = up.max(v_up);
        self.cursor = self.last_up + self.rhythm.plain_interval(&mut self.rng);
        self.prev = None;
        self.picked_at = None;
        self.last_char = None;
        self.before_gap = None;
    }

    /// Typing that starts a key or a picked character past `max_ms` (a release may end past it).
    fn starts_after(&self, max_ms: f64) -> bool {
        self.actions.iter().any(|a| a.at_ms > max_ms && !matches!(a.action, Action::KeyUp { .. }))
    }

    fn finish(mut self, pasted: bool) -> TypingPlan {
        self.release_shift();
        self.release_pending();
        // Stable: the paste stays right after its chord's keydown.
        self.actions.sort_by(|a, b| a.at_ms.total_cmp(&b.at_ms));
        TypingPlan { actions: self.actions, layout: self.layout.id, pasted }
    }
}

/// The typist at the plan's start: the field's layout, the first key's pause, the rhythm going on from the previous key.
fn typist<'a>(persona: &'a Persona, preset: Preset, ctx: &TypingContext, layouts: Vec<&'static Layout>, seed: u64, capacity: usize) -> Typist<'a> {
    let layout = ctx.layout.and_then(|id| layouts.iter().copied().find(|l| l.id == id)).unwrap_or(layouts[0]);
    let mut rng = Rng::new(seed);
    let rhythm = Rhythm::new(persona, preset, &layouts);
    let cursor = if ctx.just_focused { rhythm.first_key(&mut rng) } else { 0.0 };
    let mut t = Typist {
        rng,
        rhythm,
        rollover: persona.keyboard.rollover,
        shift_bias: persona.keyboard.shift_side_bias,
        layouts,
        layout,
        role: Role::Plain,
        actions: Vec::with_capacity(capacity),
        cursor,
        last_up: 0.0,
        prev: None,
        pending: None,
        shift: None,
        last_char: None,
        before_gap: None,
        gap_newline: false,
        picked_at: None,
    };
    if let (Some(p), false) = (ctx.previous, ctx.just_focused) {
        // A line break is Enter, whichever of '\r' and '\n' its event typed: the next line starts after the pause it
        // gets inside a plan.
        let ch = p.ch.map(|c| if c == '\r' { '\n' } else { c });
        // Its key, on whichever layout types it: the pair class needs the physical key only.
        let key = match ch {
            Some('\n') => physical(ENTER.code),
            c => c.and_then(|c| t.layouts.iter().find_map(|l| l.keystroke(c))).map(|s| s.key),
        };
        t.prev = key.map(|k| (ch.filter(|c| *c != '\n'), k, p.down_ms));
        t.last_up = p.up_ms;
        t.last_char = ch;
        t.before_gap = ch.filter(|c| *c != ' ' && *c != '\n');
        t.gap_newline = ch == Some('\n');
        // It is up already, so nothing rolls over it: the first key goes down after its keyup, as after a released key
        // of the plan, and a plain interval after it when the planner cannot place it (a picked character or a key that
        // types none, both sent with no character, or one outside the layouts).
        let mut wait = t.gap(5.0, 25.0); // prior
        if key.is_none() {
            wait = wait.max(t.rhythm.plain_interval(&mut t.rng));
        }
        t.cursor = t.cursor.max(p.up_ms + wait);
    }
    t
}

/// Plans typing `text` into the focused field. `seed` comes from a CSPRNG per call (spec 7.1).
pub fn plan_typing(persona: &Persona, preset: Preset, ctx: &TypingContext, text: &str, seed: u64) -> TypingPlan {
    let layouts = layouts_for_language(ctx.language);
    let units = units(text, ctx.field, &layouts);
    let mut t = typist(persona, preset, ctx, layouts.clone(), seed, units.len() * 2);
    let mut pasted = ctx.paste;
    if !pasted {
        let typos = ctx.field.typos() && preset.typos();
        let mut i = 0;
        while i < units.len() {
            let mut typed = 1;
            match units[i] {
                Unit::Char(c) => {
                    let slip = typos && c != ' ' && t.rng.chance(persona.keyboard.typo_rate);
                    match slip.then(|| t.typo(&units[i..])).flatten() {
                        Some(episode) => typed = episode,
                        None => t.type_char(c),
                    }
                }
                Unit::Enter => t.enter(),
                Unit::Tab => t.commit("\t"),
                Unit::Picked(s) => t.pick(s),
            }
            i += typed;
        }
        // D42, D50: typing that would start a key past the cap is not typed at all — a person pastes such a text.
        if ctx.max_ms.is_some_and(|max_ms| t.starts_after(max_ms)) {
            pasted = true;
            t = typist(persona, preset, ctx, layouts, seed, 8);
        }
    }
    if pasted {
        t.paste(text, persona.os, ctx.field, &persona.keyboard.paste);
    }
    if ctx.submit {
        t.enter();
    }
    t.finish(pasted)
}
