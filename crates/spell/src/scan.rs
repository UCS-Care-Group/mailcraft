// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding misspelt words in a piece of text.

use std::ops::Range;

use crate::dict::Dictionary;

const MAX_SCAN: usize = 1024 * 1024;

impl Dictionary {
    /// Byte ranges of misspelt words, at most `limit` of them.
    ///
    /// A word is a run of letters, digits, the affix file's `WORDCHARS`, and an ASCII
    /// apostrophe or hyphen that is not at either edge. Tokens that are only digits, and
    /// tokens that contain a letter from outside the Latin script, are left alone.
    /// Only the first mebibyte is scanned, cut on a character boundary.
    pub fn check_text(&self, text: &str, limit: usize) -> Vec<Range<usize>> {
        if limit == 0 || text.is_empty() {
            return Vec::new();
        }
        let end = scan_end(text);
        let Some(text) = text.get(..end) else { return Vec::new() };
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < text.len() {
            let Some(ch) = text.get(i..).and_then(|rest| rest.chars().next()) else { break };
            let width = ch.len_utf8();
            if in_token(self.word_chars(), ch) {
                let start = i;
                i = i.saturating_add(width);
                while i < text.len() {
                    let Some(ch) = text.get(i..).and_then(|rest| rest.chars().next()) else { break };
                    if !in_token(self.word_chars(), ch) {
                        break;
                    }
                    i = i.saturating_add(ch.len_utf8());
                }
                if let Some(range) = self.misspelt(text, start, i) {
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
        if !worth_checking(word) {
            return None;
        }
        if self.is_correct(word) {
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

fn in_token(word_chars: &[char], c: char) -> bool {
    is_latin_letter(c) || is_other_letter(c) || c.is_ascii_digit() || c == '\'' || c == '-' || word_chars.contains(&c)
}

fn trim_edges(slice: &str) -> (usize, usize) {
    let bytes = slice.as_bytes();
    let mut lo = 0usize;
    let mut hi = bytes.len();
    while lo < hi && matches!(bytes.get(lo).copied(), Some(b'\'' | b'-')) {
        lo = lo.saturating_add(1);
    }
    while hi > lo && matches!(bytes.get(hi.saturating_sub(1)).copied(), Some(b'\'' | b'-')) {
        hi = hi.saturating_sub(1);
    }
    (lo, hi)
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
