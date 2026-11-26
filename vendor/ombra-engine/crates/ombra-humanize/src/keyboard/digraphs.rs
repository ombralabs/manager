//! The 40 most frequent digraphs of each layout's language: where the persona's signature applies
//! (spec 6.3). en-US: Norvig's Google Books counts (norvig.com/mayzner.html). ru: lemmas of the
//! Russian National Corpus frequency dictionary (Lyashevskaya, Sharov 2009, dict.ruslang.ru
//! Freq2011.zip, sha256 1ae29509…7c6e) weighted by frequency. Stage Э6 recalibrates both against
//! the stand's recordings.

pub static EN_US: [&str; 40] = [
    "th", "he", "in", "er", "an", "re", "on", "at", "en", "nd", "ti", "es", "or", "te", "of", "ed", "is", "it", "al", "ar",
    "st", "to", "nt", "ng", "se", "ha", "as", "ou", "io", "le", "ve", "co", "me", "de", "hi", "ri", "ro", "ic", "ne", "ea",
];

pub static RU: [&str; 40] = [
    "ть", "ат", "ст", "то", "на", "ый", "ни", "по", "ит", "ра", "но", "ен", "ро", "от", "ов", "пр", "ка", "во", "ко", "ны",
    "он", "не", "ре", "ос", "ер", "та", "ва", "ор", "ол", "од", "ес", "ль", "ве", "ий", "ся", "ьс", "ри", "ие", "ти", "ан",
];

/// The digraph list of a layout; empty for a layout without one.
pub fn top_digraphs(layout_id: &str) -> &'static [&'static str] {
    match layout_id {
        "en-US" => &EN_US,
        "ru" => &RU,
        _ => &[],
    }
}

/// Rank of the digraph `a` `b` (case-insensitive) in the layout's list.
pub fn digraph_rank(layout_id: &str, a: char, b: char) -> Option<usize> {
    let pair: String = a.to_lowercase().chain(b.to_lowercase()).collect();
    top_digraphs(layout_id).iter().position(|d| *d == pair)
}
