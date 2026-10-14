//! Spelling: the words of a text that are not in the word list, and words they could have been.
//!
//! The word list is American English from SCOWL (`assets/dictionaries/`, attributed in
//! ATTRIBUTION.md), read once on first use. A word is a run of letters with apostrophes inside
//! (`don't`); words with digits or capitals inside (`MP3`, `iPhone`), words in capitals only
//! (`NASA`), single letters, words of letters outside ASCII (another language: nothing is known
//! of it), and anything in an address or a link are left alone. Case doesn't matter.

use std::collections::HashSet;
use std::sync::OnceLock;

/// The word list, front-coded (see `assets/dictionaries/README.md`).
const WORDS: &str = include_str!("../../../assets/dictionaries/en-US.words");

/// Most misspellings one check reports.
pub const MAX_REPORTED: usize = 500;

/// Longest word looked at, in characters; longer ones are left alone.
const MAX_WORD: usize = 40;

fn dictionary() -> &'static HashSet<String> {
    static WORDS_SET: OnceLock<HashSet<String>> = OnceLock::new();
    WORDS_SET.get_or_init(|| {
        let mut set = HashSet::with_capacity(80_000);
        let mut prev = String::new();
        for line in WORDS.lines() {
            let mut chars = line.chars();
            let shared = chars.next().and_then(|c| c.to_digit(10)).unwrap_or(0) as usize;
            let mut word: String = prev.chars().take(shared).collect();
            word.push_str(chars.as_str());
            if !word.is_empty() {
                set.insert(word.clone());
            }
            prev = word;
        }
        set
    })
}

/// Whether a word is spelt as the word list has it (in any case).
pub fn is_known(word: &str) -> bool {
    let lower = word.to_lowercase().replace('’', "'");
    let dict = dictionary();
    dict.contains(&lower)
        // `Ada's`, `teachers'`: the word without its possessive ending.
        || lower.strip_suffix("'s").is_some_and(|w| dict.contains(w))
        || lower.strip_suffix('\'').is_some_and(|w| dict.contains(w))
}

/// A word that is not in the word list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Misspelling {
    /// Where it is in the text, in characters.
    pub start: usize,
    pub end: usize,
    pub word: String,
}

/// Whether a word is one the check judges at all.
fn judged(word: &str) -> bool {
    let letters = word.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 2 || word.chars().count() > MAX_WORD {
        return false;
    }
    if !word.chars().all(|c| c.is_ascii_alphabetic() || c == '\'' || c == '’') {
        return false;
    }
    // Capitals only (an abbreviation), or a capital after the first letter (a name of a thing).
    let upper = word.chars().filter(|c| c.is_ascii_uppercase()).count();
    if upper == letters || word.chars().skip(1).any(|c| c.is_ascii_uppercase()) {
        return false;
    }
    true
}

/// The misspelt words of `text`, in order, at most [`MAX_REPORTED`].
pub fn check(text: &str) -> Vec<Misspelling> {
    let mut out = Vec::new();
    // Each piece between white space: an address or a link is left whole.
    let mut index = 0usize;
    for token in text.split_inclusive(char::is_whitespace) {
        let start = index;
        index += token.chars().count();
        let bare = token.trim_end();
        if bare.contains(['@', '/', '\\', '_', '=']) || bare.contains("www.") || bare.contains("://") {
            continue;
        }
        // The words of the piece: letters with apostrophes between them.
        let chars: Vec<char> = bare.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if !chars.get(i).is_some_and(|c| c.is_alphanumeric()) {
                i += 1;
                continue;
            }
            let from = i;
            while i < chars.len() {
                let here = chars.get(i).copied().unwrap_or(' ');
                let inner_apostrophe = matches!(here, '\'' | '’') && chars.get(i + 1).is_some_and(|c| c.is_alphabetic());
                if here.is_alphanumeric() || inner_apostrophe {
                    i += 1;
                } else {
                    break;
                }
            }
            let word: String = chars.get(from..i).unwrap_or(&[]).iter().collect();
            // A word joined to a number or a dotted name (`v2.1`, `example.org`) is left alone.
            let dotted = (chars.get(i) == Some(&'.') && chars.get(i + 1).is_some_and(|c| c.is_alphanumeric()))
                || (from > 1 && chars.get(from - 1) == Some(&'.') && chars.get(from - 2).is_some_and(|c| c.is_alphanumeric()));
            if judged(&word) && !dotted && !is_known(&word) {
                out.push(Misspelling { start: start + from, end: start + i, word });
                if out.len() >= MAX_REPORTED {
                    return out;
                }
            }
        }
    }
    out
}

/// Words from the list that `word` may have been meant as: one letter away (left out, added,
/// changed or two swapped), then two letters away when nothing is closer; at most `limit`,
/// in the case the word was written in.
pub fn suggest(word: &str, limit: usize) -> Vec<String> {
    let lower = word.to_lowercase();
    if lower.is_empty() || lower.chars().count() > MAX_WORD || !lower.is_ascii() {
        return Vec::new();
    }
    let dict = dictionary();
    let mut found: Vec<String> = Vec::new();
    let near = edits(&lower);
    for w in &near {
        if dict.contains(w) && !found.contains(w) {
            found.push(w.clone());
        }
    }
    if found.is_empty() {
        'outer: for w in near.iter().take(4000) {
            for v in edits(w) {
                if dict.contains(&v) && !found.contains(&v) {
                    found.push(v);
                    if found.len() >= limit {
                        break 'outer;
                    }
                }
            }
        }
    }
    found.truncate(limit);
    // As it was written: `Teh` → `The`.
    let capital = word.chars().next().is_some_and(char::is_uppercase);
    if capital {
        for f in &mut found {
            let mut chars = f.chars();
            if let Some(first) = chars.next() {
                *f = first.to_uppercase().chain(chars).collect();
            }
        }
    }
    found
}

/// Every string one edit away from `word` (ASCII lower-case letters only).
fn edits(word: &str) -> Vec<String> {
    let bytes = word.as_bytes();
    let n = bytes.len();
    let mut out = Vec::with_capacity(n * 54 + 26);
    let letters = b"abcdefghijklmnopqrstuvwxyz";
    let make = |parts: &[&[u8]]| -> Option<String> { String::from_utf8(parts.concat()).ok() };
    // The likeliest slips first: two letters swapped, one changed, one added, one left out.
    for i in 0..n {
        let (head, tail) = bytes.split_at(i);
        if let (Some(&a), Some(&b)) = (tail.first(), tail.get(1)) {
            out.extend(make(&[head, &[b, a], tail.get(2..).unwrap_or(&[])]));
        }
    }
    for i in 0..n {
        let (head, tail) = bytes.split_at(i);
        let rest = tail.get(1..).unwrap_or(&[]);
        for &l in letters {
            out.extend(make(&[head, &[l], rest]));
        }
    }
    for i in 0..=n {
        let (head, tail) = bytes.split_at(i);
        for &l in letters {
            out.extend(make(&[head, &[l], tail]));
        }
    }
    for i in 0..n {
        let (head, tail) = bytes.split_at(i);
        out.extend(make(&[head, tail.get(1..).unwrap_or(&[])]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_word_list_is_read() {
        assert!(dictionary().len() > 50_000);
        for w in ["the", "message", "Tuesday", "don't", "Ada's", "QUICK", "receive"] {
            assert!(is_known(w), "{w}");
        }
        assert!(!is_known("recieve") && !is_known("teh"));
    }

    #[test]
    fn misspellings_are_found_with_their_places() {
        let text = "Teh meeting is on Tuesday — please recieve it.";
        let found = check(text);
        assert_eq!(found.iter().map(|m| m.word.as_str()).collect::<Vec<_>>(), vec!["Teh", "recieve"]);
        let m = &found[1];
        assert_eq!(text.chars().skip(m.start).take(m.end - m.start).collect::<String>(), "recieve");
    }

    #[test]
    fn names_numbers_addresses_and_other_languages_are_left_alone() {
        let text = "Mail ada@example.org at https://example.org/xyzzy, NASA and iPhone, MP3 v2.1 example.org, café straße שלום x";
        assert!(check(text).is_empty(), "{:?}", check(text));
    }

    #[test]
    fn suggestions_are_near_words() {
        assert!(suggest("recieve", 5).contains(&"receive".to_string()));
        assert_eq!(suggest("Teh", 8).first().map(|s| s.chars().next()), Some(Some('T')));
        assert!(suggest("Teh", 8).contains(&"The".to_string()));
        assert!(suggest("", 5).is_empty());
        assert!(suggest(&"z".repeat(500), 5).is_empty());
    }
}
