// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding misspelt words in a piece of text.

use std::borrow::Cow;
use std::ops::Range;

use crate::dict::Dictionary;

pub(crate) const MAX_SCAN: usize = 1024 * 1024;

impl Dictionary {
    /// Byte ranges of misspelt words, at most `limit` of them.
    ///
    /// A word is a run of letters, digits, combining marks, the affix file's `WORDCHARS`,
    /// and an apostrophe or hyphen that is not at either edge. An apostrophe is `'` or
    /// U+2019. Tokens that are only digits, and tokens that contain a letter from outside
    /// the Latin script, are left alone. Only the first mebibyte is scanned, cut on a
    /// character boundary; a token that continues past that cut is not reported.
    pub fn check_text(&self, text: &str, limit: usize) -> Vec<Range<usize>> {
        if limit == 0 || text.is_empty() {
            return Vec::new();
        }
        let end = scan_end(text);
        let capped = end < text.len();
        let Some(scanned) = text.get(..end) else { return Vec::new() };
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < scanned.len() {
            let Some(ch) = scanned.get(i..).and_then(|rest| rest.chars().next()) else { break };
            let width = ch.len_utf8();
            if in_token(self.word_chars(), ch) {
                let start = i;
                i = i.saturating_add(width);
                while i < scanned.len() {
                    let Some(ch) = scanned.get(i..).and_then(|rest| rest.chars().next()) else { break };
                    if !in_token(self.word_chars(), ch) {
                        break;
                    }
                    i = i.saturating_add(ch.len_utf8());
                }
                let clipped = capped && i == scanned.len() && continues_token(text, end, self.word_chars());
                if !clipped && let Some(range) = self.misspelt(scanned, start, i) {
                    out.push(range);
                    if out.len() >= limit {
                        break;
                    }
                }
            } else {
                i = i.saturating_add(width);
            }
        }
        out
    }

    fn misspelt(&self, text: &str, start: usize, end: usize) -> Option<Range<usize>> {
        let raw = text.get(start..end)?;
        let (lo, hi) = trim_edges(raw);
        if lo >= hi {
            return None;
        }
        let word = raw.get(lo..hi)?;
        let lookup = normalize_marks(word);
        if !worth_checking(lookup.as_ref()) {
            return None;
        }
        if self.is_correct(lookup.as_ref()) {
            return None;
        }
        let from = start.saturating_add(lo);
        let to = start.saturating_add(hi);
        if from < to && text.is_char_boundary(from) && text.is_char_boundary(to) { Some(from..to) } else { None }
    }
}

fn scan_end(text: &str) -> usize {
    if text.len() <= MAX_SCAN {
        return text.len();
    }
    let mut end = MAX_SCAN;
    while end > 0 && !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    end
}

fn continues_token(text: &str, at: usize, word_chars: &[char]) -> bool {
    text.get(at..).and_then(|rest| rest.chars().next()).is_some_and(|ch| in_token(word_chars, ch))
}

fn in_token(word_chars: &[char], c: char) -> bool {
    is_latin_letter(c) || is_other_letter(c) || is_combining_mark(c) || c.is_ascii_digit() || c == '\'' || c == '-' || word_chars.contains(&c)
}

fn is_edge_char(c: char) -> bool {
    c == '\'' || c == '-' || c == '\u{2019}'
}

fn trim_edges(slice: &str) -> (usize, usize) {
    let mut lo = 0usize;
    let mut hi = slice.len();
    while lo < hi {
        let Some(rest) = slice.get(lo..hi) else { break };
        let Some(ch) = rest.chars().next() else { break };
        if !is_edge_char(ch) {
            break;
        }
        lo = lo.saturating_add(ch.len_utf8());
    }
    while hi > lo {
        let Some(rest) = slice.get(lo..hi) else { break };
        let Some((pos, ch)) = rest.char_indices().next_back() else { break };
        if !is_edge_char(ch) {
            break;
        }
        hi = lo.saturating_add(pos);
    }
    (lo, hi)
}

/// Combining marks stay on the letter they follow, and the lookup form uses the
/// precomposed letter when this checker knows that pair (`o` + U+0308 → `ö`).
fn normalize_marks(word: &str) -> Cow<'_, str> {
    if !word.chars().any(is_combining_mark) {
        return Cow::Borrowed(word);
    }
    let mut out = String::with_capacity(word.len());
    for c in word.chars() {
        if !is_combining_mark(c) {
            out.push(c);
            continue;
        }
        let Some(base) = pop_char(&mut out) else { continue };
        out.push(compose_pair(base, c).unwrap_or(base));
    }
    Cow::Owned(out)
}

fn pop_char(text: &mut String) -> Option<char> {
    let ch = text.chars().next_back()?;
    let new_len = text.len().saturating_sub(ch.len_utf8());
    if !text.is_char_boundary(new_len) {
        return None;
    }
    text.truncate(new_len);
    Some(ch)
}

fn is_combining_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}' | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}'
    )
}

fn compose_pair(base: char, mark: char) -> Option<char> {
    const ROWS: &[(char, char, char)] = &[
        ('A', '\u{0300}', 'À'),
        ('A', '\u{0301}', 'Á'),
        ('A', '\u{0302}', 'Â'),
        ('A', '\u{0303}', 'Ã'),
        ('A', '\u{0308}', 'Ä'),
        ('A', '\u{030A}', 'Å'),
        ('A', '\u{0304}', 'Ā'),
        ('A', '\u{0306}', 'Ă'),
        ('A', '\u{0328}', 'Ą'),
        ('C', '\u{0327}', 'Ç'),
        ('C', '\u{0301}', 'Ć'),
        ('C', '\u{0302}', 'Ĉ'),
        ('C', '\u{0307}', 'Ċ'),
        ('C', '\u{030C}', 'Č'),
        ('D', '\u{030C}', 'Ď'),
        ('E', '\u{0300}', 'È'),
        ('E', '\u{0301}', 'É'),
        ('E', '\u{0302}', 'Ê'),
        ('E', '\u{0308}', 'Ë'),
        ('E', '\u{0304}', 'Ē'),
        ('E', '\u{0306}', 'Ĕ'),
        ('E', '\u{0307}', 'Ė'),
        ('E', '\u{0328}', 'Ę'),
        ('E', '\u{030C}', 'Ě'),
        ('G', '\u{0302}', 'Ĝ'),
        ('G', '\u{0306}', 'Ğ'),
        ('G', '\u{0307}', 'Ġ'),
        ('G', '\u{0327}', 'Ģ'),
        ('H', '\u{0302}', 'Ĥ'),
        ('I', '\u{0300}', 'Ì'),
        ('I', '\u{0301}', 'Í'),
        ('I', '\u{0302}', 'Î'),
        ('I', '\u{0308}', 'Ï'),
        ('I', '\u{0303}', 'Ĩ'),
        ('I', '\u{0304}', 'Ī'),
        ('I', '\u{0306}', 'Ĭ'),
        ('I', '\u{0328}', 'Į'),
        ('I', '\u{0307}', 'İ'),
        ('J', '\u{0302}', 'Ĵ'),
        ('K', '\u{0327}', 'Ķ'),
        ('L', '\u{0301}', 'Ĺ'),
        ('L', '\u{0327}', 'Ļ'),
        ('L', '\u{030C}', 'Ľ'),
        ('N', '\u{0301}', 'Ń'),
        ('N', '\u{0303}', 'Ñ'),
        ('N', '\u{0327}', 'Ņ'),
        ('N', '\u{030C}', 'Ň'),
        ('O', '\u{0300}', 'Ò'),
        ('O', '\u{0301}', 'Ó'),
        ('O', '\u{0302}', 'Ô'),
        ('O', '\u{0303}', 'Õ'),
        ('O', '\u{0308}', 'Ö'),
        ('O', '\u{0304}', 'Ō'),
        ('O', '\u{0306}', 'Ŏ'),
        ('O', '\u{030B}', 'Ő'),
        ('R', '\u{0301}', 'Ŕ'),
        ('R', '\u{0327}', 'Ŗ'),
        ('R', '\u{030C}', 'Ř'),
        ('S', '\u{0301}', 'Ś'),
        ('S', '\u{0302}', 'Ŝ'),
        ('S', '\u{0327}', 'Ş'),
        ('S', '\u{030C}', 'Š'),
        ('T', '\u{0327}', 'Ţ'),
        ('T', '\u{030C}', 'Ť'),
        ('U', '\u{0300}', 'Ù'),
        ('U', '\u{0301}', 'Ú'),
        ('U', '\u{0302}', 'Û'),
        ('U', '\u{0308}', 'Ü'),
        ('U', '\u{0303}', 'Ũ'),
        ('U', '\u{0304}', 'Ū'),
        ('U', '\u{0306}', 'Ŭ'),
        ('U', '\u{030A}', 'Ů'),
        ('U', '\u{030B}', 'Ű'),
        ('U', '\u{0328}', 'Ų'),
        ('W', '\u{0302}', 'Ŵ'),
        ('Y', '\u{0301}', 'Ý'),
        ('Y', '\u{0302}', 'Ŷ'),
        ('Y', '\u{0308}', 'Ÿ'),
        ('Z', '\u{0301}', 'Ź'),
        ('Z', '\u{0307}', 'Ż'),
        ('Z', '\u{030C}', 'Ž'),
        ('a', '\u{0300}', 'à'),
        ('a', '\u{0301}', 'á'),
        ('a', '\u{0302}', 'â'),
        ('a', '\u{0303}', 'ã'),
        ('a', '\u{0308}', 'ä'),
        ('a', '\u{030A}', 'å'),
        ('a', '\u{0304}', 'ā'),
        ('a', '\u{0306}', 'ă'),
        ('a', '\u{0328}', 'ą'),
        ('c', '\u{0327}', 'ç'),
        ('c', '\u{0301}', 'ć'),
        ('c', '\u{0302}', 'ĉ'),
        ('c', '\u{0307}', 'ċ'),
        ('c', '\u{030C}', 'č'),
        ('d', '\u{030C}', 'ď'),
        ('e', '\u{0300}', 'è'),
        ('e', '\u{0301}', 'é'),
        ('e', '\u{0302}', 'ê'),
        ('e', '\u{0308}', 'ë'),
        ('e', '\u{0304}', 'ē'),
        ('e', '\u{0306}', 'ĕ'),
        ('e', '\u{0307}', 'ė'),
        ('e', '\u{0328}', 'ę'),
        ('e', '\u{030C}', 'ě'),
        ('g', '\u{0302}', 'ĝ'),
        ('g', '\u{0306}', 'ğ'),
        ('g', '\u{0307}', 'ġ'),
        ('g', '\u{0327}', 'ģ'),
        ('h', '\u{0302}', 'ĥ'),
        ('i', '\u{0300}', 'ì'),
        ('i', '\u{0301}', 'í'),
        ('i', '\u{0302}', 'î'),
        ('i', '\u{0308}', 'ï'),
        ('i', '\u{0303}', 'ĩ'),
        ('i', '\u{0304}', 'ī'),
        ('i', '\u{0306}', 'ĭ'),
        ('i', '\u{0328}', 'į'),
        ('j', '\u{0302}', 'ĵ'),
        ('k', '\u{0327}', 'ķ'),
        ('l', '\u{0301}', 'ĺ'),
        ('l', '\u{0327}', 'ļ'),
        ('l', '\u{030C}', 'ľ'),
        ('n', '\u{0301}', 'ń'),
        ('n', '\u{0303}', 'ñ'),
        ('n', '\u{0327}', 'ņ'),
        ('n', '\u{030C}', 'ň'),
        ('o', '\u{0300}', 'ò'),
        ('o', '\u{0301}', 'ó'),
        ('o', '\u{0302}', 'ô'),
        ('o', '\u{0303}', 'õ'),
        ('o', '\u{0308}', 'ö'),
        ('o', '\u{0304}', 'ō'),
        ('o', '\u{0306}', 'ŏ'),
        ('o', '\u{030B}', 'ő'),
        ('r', '\u{0301}', 'ŕ'),
        ('r', '\u{0327}', 'ŗ'),
        ('r', '\u{030C}', 'ř'),
        ('s', '\u{0301}', 'ś'),
        ('s', '\u{0302}', 'ŝ'),
        ('s', '\u{0327}', 'ş'),
        ('s', '\u{030C}', 'š'),
        ('t', '\u{0327}', 'ţ'),
        ('t', '\u{030C}', 'ť'),
        ('u', '\u{0300}', 'ù'),
        ('u', '\u{0301}', 'ú'),
        ('u', '\u{0302}', 'û'),
        ('u', '\u{0308}', 'ü'),
        ('u', '\u{0303}', 'ũ'),
        ('u', '\u{0304}', 'ū'),
        ('u', '\u{0306}', 'ŭ'),
        ('u', '\u{030A}', 'ů'),
        ('u', '\u{030B}', 'ű'),
        ('u', '\u{0328}', 'ų'),
        ('w', '\u{0302}', 'ŵ'),
        ('y', '\u{0301}', 'ý'),
        ('y', '\u{0302}', 'ŷ'),
        ('y', '\u{0308}', 'ÿ'),
        ('z', '\u{0301}', 'ź'),
        ('z', '\u{0307}', 'ż'),
        ('z', '\u{030C}', 'ž'),
    ];
    ROWS.iter().find(|row| row.0 == base && row.1 == mark).map(|row| row.2)
}

fn worth_checking(word: &str) -> bool {
    let mut latin = false;
    let mut other = false;
    for c in word.chars() {
        if is_latin_letter(c) {
            latin = true;
        } else if is_other_letter(c) {
            other = true;
        }
    }
    latin && !other
}

fn is_latin_letter(c: char) -> bool {
    matches!(
        c,
        'A'..='Z'
            | 'a'..='z'
            | '\u{00C0}'..='\u{00D6}'
            | '\u{00D8}'..='\u{00F6}'
            | '\u{00F8}'..='\u{00FF}'
            | '\u{0100}'..='\u{024F}'
            | '\u{1E00}'..='\u{1EFF}'
            | '\u{2C60}'..='\u{2C7F}'
            | '\u{A720}'..='\u{A7FF}'
    )
}

fn is_other_letter(c: char) -> bool {
    c.is_alphabetic() && !is_latin_letter(c)
}
