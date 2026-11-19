// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The source editor's per-frame work on its findings is one pass, not every mark again for
//! every part of the text and every glyph, and it draws exactly what the pass over everything
//! drew: the same underlines in the same colours, the same rectangles for each mark, for marks
//! that overlap, touch, cross rows and lines, are empty or reversed, fall off a character
//! boundary or past the text.

use std::ops::Range;

use egui::text::LayoutJob;
use egui::{Color32, FontId, Stroke};
use mailcraft_html::compat::{Rule, Severity};

use super::{Look, Mark, layout_job, mark_rects};

const TEXT: &str = "<p>Prïyà Ramán — Studio Two</p>\n<img style=\"width:120px\">\n\n<p style=\"display:flex\">a b c d e f g h</p>";

fn look() -> Look {
    Look { font: FontId::monospace(13.0), color: Color32::WHITE, line_height: 16.0, error: Color32::RED, warning: Color32::YELLOW }
}

fn mark(bytes: Range<usize>, chars: Range<usize>, severity: Severity) -> Mark {
    Mark { rule: Rule::Flexbox, severity, bytes, chars, fix: None }
}

/// A small, fixed sequence of numbers, so the marks are many and the same on every run.
struct Numbers(u64);

impl Numbers {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).unwrap_or(0) % n.max(1)
    }
}

/// The underline of each byte as it was drawn before: every mark looked at for every part.
fn underlines_looking_at_every_mark(text: &str, marks: &[Mark], look: &Look) -> Vec<Stroke> {
    let usable = |m: &&Mark| m.bytes.start < m.bytes.end && text.get(m.bytes.clone()).is_some();
    let mut cuts: Vec<usize> = vec![0, text.len()];
    for m in marks.iter().filter(usable) {
        cuts.extend([m.bytes.start, m.bytes.end]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let rank = |s: Severity| match s {
        Severity::Error => 0,
        Severity::Warning => 1,
    };
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let [from, to] = pair else { continue };
        if text.get(*from..*to).is_none() {
            continue;
        }
        let worst = marks.iter().filter(usable).filter(|m| m.bytes.start <= *from && *to <= m.bytes.end).map(|m| m.severity).min_by_key(|s| rank(*s));
        let stroke = worst.map_or(Stroke::NONE, |s| Stroke::new(1.0, if s == Severity::Error { look.error } else { look.warning }));
        out.extend(std::iter::repeat_n(stroke, to - from));
    }
    out
}

/// The underline of each byte of the job's text (egui joins parts of one look into one section).
fn underlines(job: &LayoutJob) -> Vec<Stroke> {
    job.sections.iter().flat_map(|s| std::iter::repeat_n(s.format.underline, s.byte_range.end.0 - s.byte_range.start.0)).collect()
}

#[test]
fn the_underlines_are_those_of_every_mark_looked_at_for_every_part() {
    let look = look();
    let mut numbers = Numbers(1652);
    for round in 0..400 {
        let count = numbers.below(14);
        let marks: Vec<Mark> = (0..count)
            .map(|_| {
                // Ends anywhere, on or off a character boundary, past the text, reversed or empty.
                let (a, b) = (numbers.below(TEXT.len() + 4), numbers.below(TEXT.len() + 4));
                let severity = if numbers.below(3) == 0 { Severity::Error } else { Severity::Warning };
                mark(a..b, 0..0, severity)
            })
            .collect();
        let job = layout_job(TEXT, &marks, &look, 120.0);
        let job_text: &str = &job.text;
        assert_eq!(job_text, TEXT, "round {round}: the text laid out is not the source");
        assert_eq!(underlines(&job), underlines_looking_at_every_mark(TEXT, &marks, &look), "round {round}: {marks:?}");
    }
    let empty = layout_job("", &[mark(0..3, 0..3, Severity::Error)], &look, 120.0);
    assert_eq!(empty.sections.len(), 1, "an empty source has one row");
    assert!(underlines(&empty).is_empty());
}

/// A mark's rectangles as they were found before: every glyph of every row looked at.
fn rects_looking_at_every_glyph(galley: &egui::Galley, pos: egui::Pos2, chars: &Range<usize>) -> Vec<egui::Rect> {
    let mut out = Vec::new();
    let mut at = 0usize;
    for row in &galley.rows {
        let mut span: Option<(f32, f32)> = None;
        for (i, glyph) in row.glyphs.iter().enumerate() {
            if chars.contains(&at.saturating_add(i)) {
                let x = pos.x + row.pos.x + glyph.pos.x;
                span = Some(span.map_or((x, x + glyph.advance_width), |(l, r)| (l.min(x), r.max(x + glyph.advance_width))));
            }
        }
        if let Some((left, right)) = span {
            let top = pos.y + row.pos.y;
            out.push(egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, top + row.size.y)));
        }
        at = at.saturating_add(row.glyphs.len()).saturating_add(usize::from(row.ends_with_newline));
    }
    out
}

#[test]
fn each_marks_rectangles_are_those_of_every_glyph_looked_at() {
    let ctx = egui::Context::default();
    let mut galley = None;
    let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
        // Narrow, so the text is wrapped into many rows as well as broken into lines.
        galley = Some(ui.fonts_mut(|f| f.layout_job(layout_job(TEXT, &[], &look(), 90.0))));
    });
    // (No renderer takes the font's texture here.)
    frame.textures_delta.clear();
    let galley = galley.expect("laid out");
    assert!(galley.rows.len() > 6, "the fixture is not wrapped into rows: {}", galley.rows.len());
    let pos = egui::pos2(12.5, 40.0);
    let n = TEXT.chars().count() + 3;
    // Every range of characters, reversed and past the end too.
    let marks: Vec<Mark> = (0..n).flat_map(|a| (0..n).map(move |b| mark(0..0, a..b, Severity::Warning))).collect();
    let found = mark_rects(&galley, pos, &marks);
    assert_eq!(found.len(), marks.len());
    for (m, rects) in marks.iter().zip(&found) {
        assert_eq!(rects, &rects_looking_at_every_glyph(&galley, pos, &m.chars), "characters {:?}", m.chars);
    }
    assert!(found.iter().any(|r| r.len() > 2), "no mark crosses rows");
}
