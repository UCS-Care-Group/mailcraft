// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The search box draws each keyword the query language reads as a pill around the keyword and
//! its colon (`[From:] stuart smith [Subject:] iqa`), so the owner sees which part of a message is
//! searched (#196); what is typed after it stays plain text, so a full name reads as words. The
//! text is the query as typed and stays the only truth: a pill is only how the box lays it out
//! ([`mailcraft_model::search::keyword_pills`] says where they are). A pill appears as soon as a
//! known keyword's colon is typed and keeps it while its value is being typed; it holds the same
//! characters as the text (only the keyword's first letter is drawn as a capital), so the cursor
//! and Backspace go through it as through any text.

use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{FontId, Galley, Pos2, Rect, Shape, pos2};

use crate::theme::{Tokens, semibold};

/// Room before a pill and after it.
const PAD: f32 = 6.0;
/// How far a pill's edge is from its text.
const INSET: f32 = 4.0;

/// A keyword and its colon drawn as a pill: where it stands in the text, in characters, with a
/// `-` before the keyword (in `range`, not in `key`) when there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pill {
    pub range: Range<usize>,
    pub key: Range<usize>,
}

/// The pills of `text`, in order, with the cursor at character `cursor` (`None`: the box isn't
/// being edited).
pub(crate) fn pills(text: &str, cursor: Option<usize>) -> Vec<Pill> {
    let chars = |byte: usize| text.get(..byte).map_or(0, |s| s.chars().count());
    let cursor = cursor.map(|c| text.char_indices().nth(c).map_or(text.len(), |(b, _)| b));
    mailcraft_model::search::keyword_pills(text, cursor)
        .into_iter()
        .map(|r| {
            let dash = usize::from(text.get(r.clone()).is_some_and(|h| h.starts_with('-')));
            let range = chars(r.start)..chars(r.end);
            Pill { key: range.start.saturating_add(dash).min(range.end)..range.end, range }
        })
        .collect()
}

/// `text` laid out with `pills`: a pill's keyword in semibold and the accent's colour with its
/// first letter as a capital (the same characters otherwise, so the cursor stands where it
/// would), and room around it for the pill's edge; everything else as the rest of the text.
pub(crate) fn layout(text: &str, pills: &[Pill], font: FontId, t: &Tokens) -> LayoutJob {
    let plain = TextFormat { font_id: font.clone(), color: t.text, ..Default::default() };
    let key = TextFormat { font_id: semibold(font.size), color: t.accent_text, ..Default::default() };
    let chars: Vec<char> = text.chars().collect();
    let part = |r: Range<usize>| -> String { chars.get(r).map(|c| c.iter().collect()).unwrap_or_default() };
    let mut job = LayoutJob { keep_trailing_whitespace: true, ..Default::default() };
    let mut add = |piece: String, lead: f32, format: &TextFormat| {
        if !piece.is_empty() {
            job.append(&piece, lead, format.clone());
        }
    };
    let mut at = 0;
    let mut lead = 0.0;
    for p in pills {
        if p.range.start < at || p.range.end > chars.len() || p.key.start < p.range.start || p.key.end != p.range.end {
            continue;
        }
        add(part(at..p.range.start), lead, &plain);
        // A `-` before the keyword, in the pill.
        add(part(p.range.start..p.key.start), PAD, &plain);
        let mut keyword = part(p.key.clone());
        if let Some(first) = keyword.get(..1).filter(|f| f.is_ascii()) {
            keyword = format!("{}{}", first.to_ascii_uppercase(), keyword.get(1..).unwrap_or_default());
        }
        add(keyword, if p.key.start == p.range.start { PAD } else { 0.0 }, &key);
        at = p.range.end;
        lead = PAD;
    }
    add(part(at..chars.len()), lead, &plain);
    // An empty query still needs a section: a job without one lays out a row with no height, so
    // the caret, which stands on the galley's row, would hang from the middle of the box (#652).
    if job.sections.is_empty() {
        job.append("", 0.0, plain);
    }
    job
}

/// The pills' shapes for `galley`, drawn at `at`: rounded, in a tint of the selection's colour,
/// under the text.
pub(crate) fn shapes(galley: &Arc<Galley>, at: Pos2, pills: &[Pill], t: &Tokens) -> Vec<Shape> {
    let fill = if t.dark { t.selection } else { t.selection.gamma_multiply(0.7) };
    let total = galley.text().chars().count();
    pills
        .iter()
        .map(|p| {
            let start = galley.pos_from_cursor(CCursor::new(p.range.start));
            let end = galley.pos_from_cursor(CCursor::new(p.range.end));
            // The room after a pill belongs to what comes next: the pill's own edge is before it.
            let right = end.min.x - if p.range.end < total { PAD } else { 0.0 };
            let rect = Rect::from_min_max(pos2(start.min.x - INSET, start.min.y - 1.0), pos2(right + INSET, start.max.y + 1.0));
            Shape::rect_filled(rect.translate(at.to_vec2()), rect.height() / 2.0, fill)
        })
        .collect()
}

/// A colour for tests to compare.
#[cfg(test)]
pub(crate) fn key_color(job: &LayoutJob, char_index: usize) -> Option<egui::Color32> {
    let byte = job.text.char_indices().nth(char_index)?.0;
    job.sections.iter().find(|s| s.byte_range.start.0 <= byte && byte < s.byte_range.end.0).map(|s| s.format.color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pill_is_around_the_keyword_alone() {
        let q = "from: stuart smith subject: iqa";
        let expected = [Pill { range: 0..5, key: 0..5 }, Pill { range: 19..27, key: 19..27 }];
        assert_eq!(pills(q, None), expected);
        // The cursor in a value or at the end: the pills are the same.
        for c in [0, 3, 12, 31, 99] {
            assert_eq!(pills(q, Some(c)), expected, "{c}");
        }
        // As soon as the colon is typed.
        assert_eq!(pills("budget from:", Some(12)), [Pill { range: 7..12, key: 7..12 }]);
        // Characters, not bytes; a `-` before the keyword is in its pill.
        assert_eq!(pills("Grüße -from:Đorđe", None), [Pill { range: 6..12, key: 7..12 }]);
        for q in ["re: x", "is:nothing", "10:30", ""] {
            assert!(pills(q, None).is_empty(), "{q}");
        }
    }

    #[test]
    fn a_pill_is_laid_out_with_the_same_characters_and_its_keyword_marked() {
        let t = Tokens::light();
        let q = "budget -from: stuart smith";
        let p = pills(q, None);
        let job = layout(q, &p, egui::FontId::proportional(13.0), &t);
        assert_eq!(job.text, "budget -From: stuart smith", "only the keyword's first letter is a capital");
        assert_eq!(job.text.chars().count(), q.chars().count(), "the cursor stands where it would");
        assert_eq!(key_color(&job, 8), Some(t.accent_text));
        assert_eq!(key_color(&job, 12), Some(t.accent_text), "the colon is in the pill");
        assert_eq!(key_color(&job, 14), Some(t.text), "the value is text");
        assert_eq!(key_color(&job, 0), Some(t.text));
        assert!(job.keep_trailing_whitespace);
        // Nothing to mark: the text as it is.
        assert_eq!(layout("plain words", &[], egui::FontId::proportional(13.0), &t).text, "plain words");
        // Pills that don't fit the text are passed over, never a panic.
        let bad = [Pill { range: 1..9, key: 1..9 }, Pill { range: 0..1, key: 0..1 }, Pill { range: 1..2, key: 0..2 }];
        assert_eq!(layout("ab", &bad, egui::FontId::proportional(13.0), &t).text.to_lowercase(), "ab");
    }
}
