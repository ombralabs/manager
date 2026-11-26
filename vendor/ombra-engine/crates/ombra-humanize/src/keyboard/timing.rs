//! Keystroke timing (spec 6.3); pair shapes calibrated on RAW Aalto 136M (D56).

use super::digraphs::{digraph_rank, top_digraphs};
use super::layout::{Hand, Layout, PhysicalKey};
use crate::persona::{KeyboardTraits, Persona, Preset};
use crate::rng::Rng;

/// How two consecutive keys sit on the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairClass {
    Alternating,
    SameHand,
    SameFinger,
    Repeat,
}

pub fn pair_class(a: &PhysicalKey, b: &PhysicalKey) -> PairClass {
    if a.code == b.code {
        PairClass::Repeat
    } else if a.hand != b.hand || a.hand == Hand::Thumb {
        PairClass::Alternating
    } else if a.finger == b.finger {
        PairClass::SameFinger
    } else {
        PairClass::SameHand
    }
}

impl PairClass {
    /// Participant-median IKI and log-sigma ratios to overall IKI, without word boundaries
    /// or editing keys. Aalto shows same-finger pairs slower but less variable, and repeat
    /// pairs much less variable. See qa/humanize/reference/aalto-priors.json.
    fn shape(self) -> (f64, f64) {
        match self {
            PairClass::Alternating => (0.88, 0.93),
            PairClass::SameHand => (1.0, 0.87),
            PairClass::SameFinger => (1.23, 0.71),
            PairClass::Repeat => (0.95, 0.38),
        }
    }
}

/// Interval multiplier inside a run of Backspaces (spec 6.2: corrections go faster).
pub const BACKSPACE_SPEEDUP: f64 = 0.7; // prior

/// A persona's typing rhythm under a preset.
pub struct Rhythm<'a> {
    keyboard: &'a KeyboardTraits,
    time_mult: f64,
    signatures: Vec<(&'static str, Vec<f64>)>,
}

impl<'a> Rhythm<'a> {
    pub fn new(persona: &'a Persona, preset: Preset, layouts: &[&'static Layout]) -> Self {
        let signatures = layouts.iter().map(|l| (l.id, persona.digraph_multipliers(l.id, top_digraphs(l.id).len()))).collect();
        Rhythm { keyboard: &persona.keyboard, time_mult: preset.time_mult(), signatures }
    }

    pub fn time_mult(&self) -> f64 {
        self.time_mult
    }

    /// The persona's multiplier for the digraph `a` `b` of a layout; 1 outside the layout's list.
    pub fn signature(&self, layout_id: &str, a: char, b: char) -> f64 {
        let Some(rank) = digraph_rank(layout_id, a, b) else { return 1.0 };
        self.signatures.iter().find(|s| s.0 == layout_id).map_or(1.0, |s| s.1[rank])
    }

    /// Keydown to keydown from key `a` to key `b`: log-normal around the persona's speed, shaped by
    /// the pair's class and the digraph's `signature`.
    pub fn interval(&self, rng: &mut Rng, a: &PhysicalKey, b: &PhysicalKey, signature: f64) -> f64 {
        let (mult, spread) = pair_class(a, b).shape();
        let median = 1000.0 / self.keyboard.keys_per_sec * mult * signature;
        rng.lognormal(median, self.keyboard.iki_sigma * spread) * self.time_mult
    }

    /// An interval with no key pair behind it (a Tab commit).
    pub fn plain_interval(&self, rng: &mut Rng) -> f64 {
        rng.lognormal(1000.0 / self.keyboard.keys_per_sec, self.keyboard.iki_sigma) * self.time_mult
    }

    pub fn dwell(&self, rng: &mut Rng) -> f64 {
        self.keyboard.dwell.sample(rng) * self.time_mult
    }

    /// Multiplier on the interval into the first character of a word: ×word_pause_mult, or ×2–4
    /// after punctuation or a line break.
    pub fn boundary(&self, rng: &mut Rng, after_punctuation: bool) -> f64 {
        if after_punctuation {
            rng.uniform(2.0, 4.0)
        } else {
            self.keyboard.word_pause_mult
        }
    }

    /// A 0.5–2 s pause before a word with the persona's probability, else 0.
    pub fn thinking(&self, rng: &mut Rng) -> f64 {
        if rng.chance(self.keyboard.thinking_pause_p) {
            rng.uniform(500.0, 2000.0) * self.time_mult
        } else {
            0.0
        }
    }

    /// Focus to the first key, or to the paste chord: the persona's pause (stage Э6б).
    pub fn first_key(&self, rng: &mut Rng) -> f64 {
        self.keyboard.first_key.sample(rng) * self.time_mult
    }

    /// Switching layouts (spec 6.2): a pause and no key events, the OS shortcut never reaches the page.
    pub fn layout_switch(&self, rng: &mut Rng) -> f64 {
        rng.uniform(250.0, 700.0) * self.time_mult // prior
    }

    /// Before a character from the system picker; shorter for the next one while it stays open.
    pub fn picker(&self, rng: &mut Rng, again: bool) -> f64 {
        let (low, high) = if again { (250.0, 700.0) } else { (800.0, 2000.0) }; // prior
        rng.uniform(low, high) * self.time_mult
    }

    /// From the picker back to the keys.
    pub fn back_to_keys(&self, rng: &mut Rng) -> f64 {
        rng.uniform(300.0, 800.0) * self.time_mult // prior
    }

    /// From the last mistyped key to the first correcting Backspace.
    pub fn correction(&self, rng: &mut Rng) -> f64 {
        self.keyboard.correction.sample(rng) * self.time_mult
    }
}
