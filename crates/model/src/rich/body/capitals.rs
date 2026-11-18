// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::RichText;
use crate::ModelError;

/// Only the end of the current paragraph is needed. Long words and long runs of
/// whitespace can leave a sentence uncapitalized, so finishing a word stays cheap.
const CONTEXT_CHARS: usize = 256;

pub(super) struct Initial {
    at: usize,
    uppercase: String,
    dutch_ij: bool,
}

impl Initial {
    pub(super) fn extra_chars(&self) -> usize {
        self.uppercase.chars().count().saturating_sub(1)
    }

    pub(super) fn replace(self, text: &mut RichText) -> Result<(), ModelError> {
        if self.dutch_ij {
            replace_letter(text, self.at + 1, "J")?;
        }
        replace_letter(text, self.at, &self.uppercase)
    }
}

fn replace_letter(text: &mut RichText, at: usize, replacement: &str) -> Result<(), ModelError> {
    let style = text.style_at(at + 1);
    text.delete(at, at + 1)?;
    text.insert(at, replacement, &style)?;
    Ok(())
}

pub(super) fn find(text: &RichText, caret: usize, language: &str) -> Option<Initial> {
    let (index, offset) = text.locate(caret).ok()?;
    let paragraph = text.paragraphs().get(index)?;
    if paragraph.signature {
        return None;
    }
    let start = offset.saturating_sub(CONTEXT_CHARS);
    let context: String = paragraph.runs.iter().flat_map(|run| run.text.chars()).skip(start).take(offset - start).collect();
    let split = context.rmatch_indices(char::is_whitespace).next().map_or(0, |(idx, space)| idx + space.len());
    let token = context.get(split..)?;
    let word = token.trim_start_matches(opening);
    let body = word.trim_end_matches(|c| closing(c) || matches!(c, '.' | '!' | '?' | ',' | ':' | ';' | '…'));
    let mut letters = body.chars();
    let first = letters.next().filter(|ch| ch.is_lowercase())?;
    // A complete word can have apostrophes, hyphens and combining accents. URLs,
    // addresses, paths and dotted identifiers contain other punctuation.
    if !letters.all(word_character) {
        return None;
    }
    let before = context.get(..split)?.trim_end();
    let starts_paragraph = start == 0 && before.trim_start_matches(|c: char| c.is_whitespace() || opening(c)).is_empty();
    let before_sentence = before.trim_end_matches(|c: char| c.is_whitespace() || opening(c));
    if !starts_paragraph && !sentence_ends(before_sentence) {
        return None;
    }
    let position = split + token.len() - word.len();
    let at = caret - offset + start + context.get(..position)?.chars().count();
    if text.style_at(at + 1).link.is_some() {
        return None;
    }
    let language = language.split(['-', '_']).next().unwrap_or_default();
    let dutch_ij = language.eq_ignore_ascii_case("nl") && body.starts_with("ij") && text.style_at(at + 2).link.is_none();
    let dotted_i = first == 'i' && (language.eq_ignore_ascii_case("tr") || language.eq_ignore_ascii_case("az"));
    let uppercase = if dotted_i { "İ".to_string() } else { first.to_uppercase().collect() };
    Some(Initial { at, uppercase, dutch_ij })
}

fn opening(c: char) -> bool {
    matches!(c, '"' | '\'' | '‘' | '“' | '„' | '«' | '(' | '[' | '¿' | '¡')
}

fn closing(c: char) -> bool {
    matches!(c, '"' | '\'' | '’' | '”' | '»' | ')' | ']')
}

fn word_character(c: char) -> bool {
    c.is_alphabetic() || matches!(c, '\'' | '’' | '-' | '\u{0300}'..='\u{036f}' | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}')
}

fn sentence_ends(before: &str) -> bool {
    let before = before.trim_end_matches(|c: char| c.is_whitespace() || closing(c));
    if before.ends_with(['!', '?', '。', '！', '？']) {
        return true;
    }
    let Some(stem) = before.strip_suffix('.') else { return false };
    if stem.ends_with('.') {
        return false;
    }
    let last = stem.split_whitespace().next_back().unwrap_or_default().trim_start_matches(opening);
    // Initials and dotted abbreviations like e.g. and z.B.
    if last.is_empty() || last.split('.').all(|part| part.chars().count() == 1) {
        return false;
    }
    let abbreviations = [
        "approx", "art", "avv", "bijv", "bzw", "cf", "dhr", "dott", "dott.ssa", "dr", "dra", "ecc", "ej", "es", "etc", "fig", "hr", "ing", "mevr",
        "mlle", "mme", "mr", "mrs", "ms", "nr", "núm", "pag", "pág", "prof", "rif", "sig", "sig.na", "sig.ra", "sr", "sra", "srta", "st", "tel",
        "ud", "uds", "vol",
    ];
    !abbreviations.contains(&last.to_lowercase().as_str())
}
