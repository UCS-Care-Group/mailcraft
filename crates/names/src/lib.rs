// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Names: the people, folders, categories, calendars, accounts, signatures and rules that come
//! into MailCraft from messages, servers, files and what the owner types (#454).
//!
//! [`is_format`] is the one list of Unicode's format characters (general category Cf, Unicode
//! 17.0): bidirectional marks, overrides and isolates, zero-width marks and joiners, the
//! byte-order mark, the soft hyphen, tags and the rest. Most show nothing themselves (a few, such
//! as the Arabic number signs, are drawn across the digits after them), but they can turn the text
//! around them, or make two names that look the same differ.
//!
//! A name takes two steps, both from the same cleaning:
//! - [`incoming`], for a name as it is kept: one line, without control or format characters,
//!   trimmed, at most a given length; nothing when nothing is left.
//! - [`shown`], for a name as it is shown: the same, and cut with an ellipsis where it doesn't
//!   fit. What is kept is not changed.
//!
//! A zero-width joiner or non-joiner ([`is_joiner`]) stays where it stands between two
//! characters that show, inside a word: an emoji made of several, and words of Persian and
//! the Indic scripts, need it there. Anywhere else it goes.
//!
//! Characters that show nothing but are not format characters (the Hangul fillers, the braille
//! blank, a variation selector) are letters, marks and symbols to Unicode, and are kept.
//!
//! Lengths are in characters (Unicode scalar values), so a cut may part a letter from an accent
//! written after it. What is made is bounded by the length asked for, whatever the text, and
//! nothing panics.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(test)]
mod tests;

/// Whether `c` is one of Unicode's format characters (general category Cf, Unicode 17.0): the
/// soft hyphen, the Arabic number signs and end-of-ayah marks, the Syriac abbreviation mark, the
/// Arabic pound and piastre marks above, the Mongolian vowel separator, zero-width marks and
/// joiners, bidirectional marks, overrides and isolates, the word joiner and invisible operators,
/// the deprecated shaping controls, the byte-order mark, interlinear annotations, the Kaithi
/// number signs, the Egyptian hieroglyph, shorthand and musical format controls, and tags.
pub fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061c}'
            | '\u{06dd}'
            | '\u{070f}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// Whether `c` is the zero-width non-joiner or joiner (U+200C, U+200D): format characters that
/// are part of a word where they stand inside one.
pub fn is_joiner(c: char) -> bool {
    matches!(c, '\u{200c}' | '\u{200d}')
}

/// A name as it is kept: one line (a line break, a tab or any other white space parts words, and
/// words are parted by one space), without control or format characters (a joiner inside a word
/// stays), trimmed, at most `max` characters. `None` when nothing is left.
pub fn incoming(text: &str, max: usize) -> Option<String> {
    let (name, _) = clean(text, max);
    (!name.is_empty()).then_some(name)
}

/// A name as it is shown: cleaned as [`incoming`] cleans it, and at most `max` characters, the
/// last of them an ellipsis when the name was longer. Empty when nothing is left.
pub fn shown(text: &str, max: usize) -> String {
    let (name, cut) = clean(text, max);
    if !cut || max == 0 {
        return name;
    }
    let (mut name, _) = clean(text, max.saturating_sub(1));
    name.push('…');
    name
}

/// `text` cleaned and cut to at most `max` characters, and whether a character that shows was
/// left out for the length. Nothing past that character is read.
fn clean(text: &str, max: usize) -> (String, bool) {
    let mut out = String::new();
    let mut count = 0usize;
    // A space between words, and a joiner inside one, come only with the character after them.
    let mut space = false;
    let mut joiner: Option<char> = None;
    for c in text.chars() {
        if is_joiner(c) {
            if count > 0 && !space {
                joiner = Some(c);
            }
        } else if c.is_whitespace() {
            space = count > 0;
            joiner = None;
        } else if !c.is_control() && !is_format(c) {
            let needed = 1 + usize::from(space) + usize::from(joiner.is_some());
            if count.saturating_add(needed) > max {
                return (out, true);
            }
            if std::mem::take(&mut space) {
                out.push(' ');
            }
            if let Some(j) = joiner.take() {
                out.push(j);
            }
            out.push(c);
            count = count.saturating_add(needed);
        }
    }
    (out, false)
}
