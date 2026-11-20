// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One spelling filter for the composer and for anything else that asks the engine (#1797).
//!
//! A host ([`mailcraft_spell::HostChecker`]) returns byte ranges. This module turns them into
//! character offsets, skips a signature, a link, a picture, a web address and an e-mail address,
//! and drops words the owner has accepted. The same rules apply to the built-in list and to an
//! operating-system dictionary. Message text is data: nothing here is an instruction.

use std::collections::BTreeSet;
use std::ops::Range;

use mailcraft_model::rich::{OBJECT, Paragraph};
use mailcraft_spell::{HostChecker, MAX_ISSUES};

/// A paragraph longer than this is not sent to the host. The report is partial and empty.
pub const MAX_PARAGRAPH_BYTES: usize = 8192;

/// How many words a custom dictionary keeps. Callers enforce it; checking does not grow the list.
pub const MAX_CUSTOM_WORDS: usize = 10_000;

/// The most characters [`clean_custom_word`] keeps in one entry.
pub const MAX_WORD_CHARS: usize = 64;

/// A misspelt word. `span` counts characters in the paragraph, or in the plain line
/// ([`check_plain`] turns newlines into spaces first, one for one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub span: Range<usize>,
    pub word: String,
}

/// What a check found. `available` is false when the host has no dictionary for the locale.
/// `partial` is true when the text was too long, or the host had more than [`MAX_ISSUES`] words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub issues: Vec<Issue>,
    pub available: bool,
    pub partial: bool,
}

impl Report {
    fn none() -> Self {
        Self { issues: Vec::new(), available: true, partial: false }
    }

    fn too_long() -> Self {
        Self { issues: Vec::new(), available: true, partial: true }
    }
}

/// Words the owner has accepted, and the two switches that skip a class of words.
///
/// A default filter reports every word the host and the address rules leave.
#[derive(Debug)]
pub struct Filter<'a> {
    pub custom: &'a [String],
    pub ignored: &'a BTreeSet<String>,
    pub ignore_uppercase: bool,
    pub ignore_numbers: bool,
}

fn empty_ignored() -> &'static BTreeSet<String> {
    static EMPTY: std::sync::OnceLock<BTreeSet<String>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(BTreeSet::new)
}

impl Default for Filter<'static> {
    fn default() -> Self {
        Self { custom: &[], ignored: empty_ignored(), ignore_uppercase: false, ignore_numbers: false }
    }
}

/// Check one paragraph. A signature is skipped. Link and picture runs are spaces, so the host
/// never sees them; an address that is only partly linked is still skipped, from the text itself.
pub fn check_paragraph(host: &dyn HostChecker, locale: &str, paragraph: &Paragraph, filter: &Filter<'_>) -> Report {
    let mut bytes = 0usize;
    for run in &paragraph.runs {
        bytes = bytes.saturating_add(run.text.len());
    }
    if paragraph.signature || bytes == 0 {
        return Report::none();
    }
    if bytes > MAX_PARAGRAPH_BYTES {
        return Report::too_long();
    }
    let text = paragraph.text();
    let mut input = String::new();
    input.reserve(bytes.min(MAX_PARAGRAPH_BYTES));
    for run in &paragraph.runs {
        if run.style.link.is_some() || run.picture.is_some() {
            input.extend(std::iter::repeat_n(' ', run.text.len()));
        } else {
            input.push_str(&run.text);
        }
    }
    check_input(host, locale, &text, &input, filter)
}

/// Check a plain line, such as the Subject. Newlines count as spaces (one character each), so a
/// word is not joined across a line and the offsets still name the original characters. There are
/// no runs; the length cap and the address rules are the same as for a paragraph.
pub fn check_plain(host: &dyn HostChecker, locale: &str, text: &str, filter: &Filter<'_>) -> Report {
    if text.len() > MAX_PARAGRAPH_BYTES {
        return Report::too_long();
    }
    if text.is_empty() {
        return Report::none();
    }
    if text.contains(['\n', '\r']) {
        let flat: String = text.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
        check_input(host, locale, &flat, &flat, filter)
    } else {
        check_input(host, locale, text, text, filter)
    }
}

fn check_input(host: &dyn HostChecker, locale: &str, text: &str, input: &str, filter: &Filter<'_>) -> Report {
    let Some(mut ranges) = host.check(locale, input) else {
        return Report { issues: Vec::new(), available: false, partial: false };
    };
    let partial = ranges.len() > MAX_ISSUES;
    ranges.truncate(MAX_ISSUES);
    ranges.sort_unstable_by_key(|range| range.start);
    let mut issues = Vec::new();
    let mut last = 0usize;
    let mut offset = 0usize;
    for range in ranges {
        let Some(word) = text.get(range.clone()).filter(|word| word.chars().any(char::is_alphabetic)) else { continue };
        if range.start < last || input.get(range.clone()) != Some(word) || word.contains(OBJECT) {
            continue;
        }
        if within_address(text, &range) {
            continue;
        }
        offset = offset.saturating_add(text.get(last..range.start).unwrap_or_default().chars().count());
        let end = offset.saturating_add(word.chars().count());
        if !suppressed(word, filter) {
            issues.push(Issue { span: offset..end, word: word.to_string() });
        }
        (last, offset) = (range.end, end);
    }
    Report { issues, available: true, partial }
}

/// Whether `entry` names `word`.
///
/// Both are compared with `’` written as `'`. An entry with no capital matches any casing.
/// An entry with a capital matches that casing, and the same word in all capitals.
pub fn word_matches(entry: &str, word: &str) -> bool {
    if entry.is_empty() || word.is_empty() {
        return false;
    }
    let entry = fold_apostrophe(entry);
    let word = fold_apostrophe(word);
    if !entry.chars().any(char::is_uppercase) {
        return same_word(&entry, &word);
    }
    entry == word || (uppercase_word(&word, 1) && same_word(&entry, &word))
}

/// `word` as a custom-dictionary entry: trimmed, 1 to [`MAX_WORD_CHARS`] characters, and only
/// letters, digits, `'`, `’` and `-`. Anything else, including a control character, is refused.
pub fn clean_custom_word(word: &str) -> Option<String> {
    let word = word.trim();
    let mut count = 0usize;
    for c in word.chars() {
        count = count.saturating_add(1);
        if count > MAX_WORD_CHARS || c.is_control() || !custom_char(c) {
            return None;
        }
    }
    if count == 0 { None } else { Some(word.to_string()) }
}

fn custom_char(c: char) -> bool {
    c.is_alphabetic() || c.is_ascii_digit() || matches!(c, '\'' | '\u{2019}' | '-')
}

fn fold_apostrophe(s: &str) -> std::borrow::Cow<'_, str> {
    if s.chars().any(|c| c == '\u{2019}') {
        std::borrow::Cow::Owned(s.chars().map(|c| if c == '\u{2019}' { '\'' } else { c }).collect())
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

fn same_word(a: &str, b: &str) -> bool {
    a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

/// `least` or more letters, and every letter uppercase. Digits and marks are ignored.
fn uppercase_word(word: &str, least: usize) -> bool {
    let mut letters = 0usize;
    for c in word.chars() {
        if !c.is_alphabetic() {
            continue;
        }
        if !c.is_uppercase() {
            return false;
        }
        letters = letters.saturating_add(1);
    }
    letters >= least
}

fn has_digit(word: &str) -> bool {
    word.chars().any(char::is_numeric)
}

fn suppressed(word: &str, filter: &Filter<'_>) -> bool {
    if filter.ignore_numbers && has_digit(word) {
        return true;
    }
    if filter.ignore_uppercase && uppercase_word(word, 2) {
        return true;
    }
    filter.custom.iter().any(|entry| word_matches(entry, word)) || filter.ignored.iter().any(|entry| word_matches(entry, word))
}

/// Links are masked before the host sees them. The original text still finds the address, so a
/// change of formatting in its middle does not unprotect the visible half.
fn within_address(text: &str, word: &Range<usize>) -> bool {
    let boundary = |ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | OBJECT);
    let left = text.get(..word.start).unwrap_or_default().rsplit(boundary).next().unwrap_or_default();
    let right = text.get(word.end..).unwrap_or_default().split(boundary).next().unwrap_or_default();
    let start = word.start.saturating_sub(left.len());
    let end = word.end.saturating_add(right.len());
    let Some(token) = text.get(start..end) else { return false };
    let offset = word.start.saturating_sub(start)..word.end.saturating_sub(start);
    let schemes = token.match_indices("://").filter_map(|(at, _)| {
        let prefix = token.get(..at)?;
        let scheme = prefix.rsplit(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))).next().unwrap_or_default();
        scheme.as_bytes().first().is_some_and(u8::is_ascii_alphabetic).then_some(at.saturating_sub(scheme.len()))
    });
    let punctuation = ['—', '–', '(', ')', '[', ']', ',', ';'];
    let web_starts = std::iter::once(0)
        .chain(token.match_indices(punctuation).map(|(at, mark)| at.saturating_add(mark.len())))
        .filter(|&at| token.get(at..at.saturating_add(4)).is_some_and(|prefix| prefix.eq_ignore_ascii_case("www.")));
    if schemes.chain(web_starts).any(|at| at < offset.end && offset.start < url_end(token, at)) {
        return true;
    }

    let email_left = left.rsplit(punctuation).next().unwrap_or_default();
    let email_right = right.split(punctuation).next().unwrap_or_default();
    let email_start = word.start.saturating_sub(email_left.len());
    let email_end = word.end.saturating_add(email_right.len());
    text.get(email_start..email_end).is_some_and(|address| address.contains('@'))
}

/// A closing parenthesis outside the URL ends a surrounding prose parenthesis.
/// Parentheses that open inside a URL can close within its path.
fn url_end(token: &str, start: usize) -> usize {
    let Some(url) = token.get(start..) else { return start };
    let mut depth = 0usize;
    for (at, ch) in url.char_indices() {
        match ch {
            '(' => depth = depth.saturating_add(1),
            ')' if depth == 0 => return start.saturating_add(at),
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    token.len()
}
