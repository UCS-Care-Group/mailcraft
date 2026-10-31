// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding text in a document and putting other text in its place (Find and Replace in the
//! message being written).
//!
//! What is looked for is plain text on one line: a place never runs over a paragraph break or a
//! picture. Letter case counts only when asked; otherwise each character is compared in lower
//! case (a character whose lower case is more than one character is compared as it is), so a
//! place is always as long as what was looked for. Both texts are capped at [`MAX_FIND`]
//! characters, and the work is linear in the document's length.

use super::{MAX_CHARS, Paragraph, RichText, Run, clean_line};
use crate::ModelError;

/// Longest text looked for, or put in its place, in characters.
pub const MAX_FIND: usize = 255;

/// A place in the text, `[start, end)`.
pub type Place = (usize, usize);

/// `c` as it is compared when letter case doesn't count.
fn fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// Text as it is looked for: one line, no control characters but the tab, no picture.
fn usable(needle: &str) -> bool {
    !needle.is_empty() && needle.chars().count() <= MAX_FIND && clean_line(needle) == needle
}

impl RichText {
    /// Where `needle` stands within `[from, to)`: each place wholly inside it, in order, apart.
    /// Nothing for an empty text, one longer than [`MAX_FIND`], or one with a line break, a
    /// control character (but the tab) or a picture in it.
    pub fn find(&self, needle: &str, match_case: bool, from: usize, to: usize) -> Vec<Place> {
        if !usable(needle) || from >= to {
            return Vec::new();
        }
        let folded = |s: &str| -> String { if match_case { s.to_string() } else { s.chars().map(fold).collect() } };
        let needle = folded(needle);
        let mut out = Vec::new();
        let mut base = 0usize;
        for p in &self.paragraphs {
            let n = p.chars();
            // Only the paragraphs `[from, to)` reaches into.
            if base + n >= from && base < to {
                let hay = folded(&p.text());
                // Byte offsets to characters, walking forward once (the places come in order).
                let mut chars = hay.char_indices().map(|(b, _)| b).enumerate().peekable();
                let needle_chars = needle.chars().count();
                for (byte, _) in hay.match_indices(needle.as_str()) {
                    let mut at = None;
                    while let Some((i, b)) = chars.peek().copied() {
                        if b >= byte {
                            at = (b == byte).then_some(i);
                            break;
                        }
                        chars.next();
                    }
                    let Some(at) = at else { continue };
                    let place = (base + at, base + at + needle_chars);
                    if place.0 >= from && place.1 <= to {
                        out.push(place);
                    }
                }
            }
            base += n + 1;
            if base > to {
                break;
            }
        }
        out
    }

    /// Put `with` (cleaned to one line) in place of each of `places`, each in the look of its
    /// place's first character, a link and all. The places are in order, apart, not empty, and
    /// each within one paragraph and holding no picture, as [`RichText::find`] gives them;
    /// anything else is an error and changes nothing.
    pub fn replace_places(&mut self, places: &[Place], with: &str) -> Result<(), ModelError> {
        let with = clean_line(with);
        let with_chars = with.chars().count();
        let bad = || ModelError::Invalid("the places to replace are not places in the text".into());
        if places.windows(2).any(|w| matches!(w, [a, b] if a.1 > b.0)) || places.iter().any(|(a, b)| a >= b) {
            return Err(bad());
        }
        let taken: usize = places.iter().map(|(a, b)| b - a).sum();
        let len = self.len().saturating_sub(taken).saturating_add(with_chars.saturating_mul(places.len()));
        if len > MAX_CHARS {
            return Err(ModelError::Invalid("the message can't hold that much text".into()));
        }
        let mut next = self.paragraphs.clone();
        let mut rest = places;
        let mut base = 0usize;
        for p in &mut next {
            let n = p.chars();
            let inside = rest.iter().take_while(|(a, _)| *a <= base + n).count();
            let (here, after) = rest.split_at(inside);
            if here.iter().any(|(_, b)| *b > base + n) {
                return Err(bad());
            }
            if !here.is_empty() {
                let local: Vec<Place> = here.iter().map(|(a, b)| (a - base, b - base)).collect();
                *p = replaced(p, &local, &with).ok_or_else(bad)?;
            }
            rest = after;
            base += n + 1;
        }
        if !rest.is_empty() {
            return Err(bad());
        }
        self.paragraphs = next;
        Ok(())
    }
}

/// `p` with `with` in place of each of `places` (offsets in it, in order, apart); `None` when a
/// place holds a picture.
fn replaced(p: &Paragraph, places: &[Place], with: &str) -> Option<Paragraph> {
    let mut runs: Vec<Run> = Vec::with_capacity(p.runs.len() + places.len() * 2);
    let mut places = places.iter().peekable();
    let mut seen = 0usize;
    for run in &p.runs {
        if run.picture.is_some() {
            if places.peek().is_some_and(|(a, b)| seen >= *a && seen < *b) {
                return None;
            }
            runs.push(run.clone());
            seen += 1;
            continue;
        }
        let mut kept = String::new();
        for c in run.text.chars() {
            if places.peek().is_some_and(|(a, _)| *a == seen) {
                runs.push(Run::new(std::mem::take(&mut kept), run.style.clone()));
                runs.push(Run::new(with, run.style.clone()));
            }
            if !places.peek().is_some_and(|(a, b)| seen >= *a && seen < *b) {
                kept.push(c);
            }
            seen += 1;
            if places.peek().is_some_and(|(_, b)| *b == seen) {
                places.next();
            }
        }
        runs.push(Run::new(kept, run.style.clone()));
    }
    if places.next().is_some() {
        return None;
    }
    let mut out = Paragraph { runs, ..p.clone() };
    out.normalize();
    Some(out)
}

/// Where `pos` is once `places` have each become `with_chars` characters long: moved by what
/// changed before it, and to the end of what went in for a place it was inside.
pub fn moved(pos: usize, places: &[Place], with_chars: usize) -> usize {
    let mut out = pos;
    for (a, b) in places {
        if *b <= pos {
            out = out.saturating_sub(b - a).saturating_add(with_chars);
        } else if *a < pos {
            out = out.saturating_sub(pos - a).saturating_add(with_chars);
        }
    }
    out
}
