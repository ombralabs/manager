//! Typos (spec 6.3): which slip, and what the fingers type instead of the text.

use super::layout::Layout;
use crate::rng::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypoKind {
    /// A neighbouring key instead of the right one.
    Adjacent,
    /// The character is skipped.
    Omission,
    /// The key is pressed twice.
    Double,
    /// The character and the next one swap.
    Transposition,
}

/// Spec 6.3: adjacent key 60 %, omission 15 %, double press 15 %, transposition 10 %.
pub fn typo_kind(rng: &mut Rng) -> TypoKind {
    match rng.unit() {
        u if u < 0.60 => TypoKind::Adjacent,
        u if u < 0.75 => TypoKind::Omission,
        u if u < 0.90 => TypoKind::Double,
        _ => TypoKind::Transposition,
    }
}

/// What gets typed for a slip at the start of `window` noticed `noticed_after` characters later,
/// and the text it stands for: `(wrong, right)`. `window` holds the next characters, all on
/// `layout`. Where `kind` cannot apply (nothing to skip into or to swap), the slip is an adjacent
/// key; `None` when not even that fits.
pub fn corrupt(kind: TypoKind, window: &[char], noticed_after: usize, layout: &Layout, rng: &mut Rng) -> Option<(Vec<char>, Vec<char>)> {
    let c = *window.first()?;
    let kind = match kind {
        TypoKind::Omission if window.len() < 2 => TypoKind::Adjacent,
        TypoKind::Transposition if window.len() < 2 || window[1] == c => TypoKind::Adjacent,
        kind => kind,
    };
    let head = if kind == TypoKind::Transposition { 2 } else { 1 };
    // A skipped character only shows once the next one is typed.
    let tail = noticed_after.max(usize::from(kind == TypoKind::Omission)).min(window.len() - head);
    let mut wrong = match kind {
        TypoKind::Adjacent => {
            let stroke = layout.keystroke(c)?;
            let near = layout.neighbors(stroke.key.code);
            if near.is_empty() {
                return None;
            }
            vec![layout.character(near[rng.below(near.len())].code, stroke.shift)?]
        }
        TypoKind::Omission => Vec::new(),
        TypoKind::Double => vec![c, c],
        TypoKind::Transposition => vec![window[1], c],
    };
    wrong.extend_from_slice(&window[head..head + tail]);
    Some((wrong, window[..head + tail].to_vec()))
}
