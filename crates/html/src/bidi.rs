// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which way a paragraph reads, and so which side it stands at (#603).
//!
//! A paragraph's direction is the one its message gives (`dir`, `direction`), or else the one its
//! first strong letter gives (UAX #9, rules P2 and P3): Hebrew, Arabic, Syriac, Thaana, N'Ko and
//! the other scripts written from the right make it right to left, any other letter left to
//! right, and digits, punctuation and white space say nothing. A paragraph that reads from the
//! right stands at the right, unless the message aligns it to a side of its own: `text-align`
//! `start`, `end` and `justify`, and no alignment at all, follow the direction; `left`, `right`
//! and `center` don't. Nothing here reorders a line: the text stays as it was written, which is
//! what a copy gives. egui 0.37 shapes each run of text in the direction its script suggests, so
//! the Hebrew or Arabic words of a line come out in their order, but a number or a Latin word
//! inside such a line is drawn mirrored, and Hebrew inside a Latin line too: putting the runs of
//! a line in visual order needs the bidirectional algorithm, which is the toolkit's to do.
//!
//! This is a stopgap that stays in this module. The table below is a short list of ranges, not
//! the Unicode character database. When egui lays out text in both directions itself
//! (emilk/egui#8822 and #8818, which bring the `unicode-bidi` crate into epaint), its classes
//! replace [`first_strong`], the reading pane gives the direction to the toolkit instead of
//! mirroring the alignment here, and writing from the right (#483) can build on that.

use crate::Align;

/// The direction an element asks for (`dir`, `direction`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Dir {
    Ltr,
    Rtl,
    /// From the paragraph's own first strong letter: `dir="auto"`, and what a message that says
    /// nothing gets.
    #[default]
    Auto,
}

impl Dir {
    /// The `dir` attribute: `ltr`, `rtl` or `auto`, in any case.
    pub(crate) fn attr(value: &str) -> Option<Dir> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("ltr") {
            Some(Dir::Ltr)
        } else if value.eq_ignore_ascii_case("rtl") {
            Some(Dir::Rtl)
        } else if value.eq_ignore_ascii_case("auto") {
            Some(Dir::Auto)
        } else {
            None
        }
    }

    /// CSS `direction` (lower-cased already): `ltr` or `rtl`.
    pub(crate) fn css(value: &str) -> Option<Dir> {
        match value.trim() {
            "ltr" => Some(Dir::Ltr),
            "rtl" => Some(Dir::Rtl),
            _ => None,
        }
    }
}

/// Where an alignment came from, which decides whether a paragraph's direction turns it round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AlignFrom {
    /// Nothing asked for one: the paragraph stands at its start.
    #[default]
    Nothing,
    /// A side named as such (`left`, `right`, `center`, the `align` attribute, `<center>`).
    Side,
    /// `text-align: start`, `end` or `justify`: from the start of the line, whichever side that is.
    Flow,
}

/// The side a paragraph stands at: `align`, turned round when it reads from the right and the
/// alignment follows the direction.
pub(crate) fn placed(align: Align, from: AlignFrom, rtl: bool) -> Align {
    if !rtl || from == AlignFrom::Side {
        return align;
    }
    match align {
        Align::Left => Align::Right,
        Align::Right => Align::Left,
        Align::Center => Align::Center,
    }
}

/// Whether a paragraph whose element asks for `dir` and whose text is `text` reads from the right.
pub(crate) fn rtl(dir: Dir, text: impl IntoIterator<Item = char>) -> bool {
    match dir {
        Dir::Ltr => false,
        Dir::Rtl => true,
        Dir::Auto => first_strong(text) == Some(Dir::Rtl),
    }
}

/// The direction of the first strong letter in `text` (UAX #9 P2), skipping what is inside an
/// isolate (U+2066 to U+2069); `None` when there is none.
pub(crate) fn first_strong(text: impl IntoIterator<Item = char>) -> Option<Dir> {
    let mut isolates = 0usize;
    for c in text {
        match c {
            '\u{2066}'..='\u{2068}' => isolates = isolates.saturating_add(1),
            '\u{2069}' => isolates = isolates.saturating_sub(1),
            _ if isolates > 0 => {}
            _ => match class(c) {
                Class::Rtl => return Some(Dir::Rtl),
                Class::Ltr => return Some(Dir::Ltr),
                Class::Weak => {}
            },
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// R or AL.
    Rtl,
    /// L.
    Ltr,
    /// Everything else: digits, punctuation, white space, marks, controls.
    Weak,
}

/// The letters written from the right (bidi class R or AL), by range.
const RTL: &[(u32, u32)] = &[
    // Hebrew: a few signs that are letters, and the letters (from U+05D0 the block is R).
    (0x05BE, 0x05BE),
    (0x05C0, 0x05C0),
    (0x05C3, 0x05C3),
    (0x05C6, 0x05C6),
    (0x05D0, 0x05FF),
    // Arabic: the letters and the signs that are letters, not its digits or marks.
    (0x0608, 0x0608),
    (0x060B, 0x060B),
    (0x060D, 0x060D),
    (0x061B, 0x064A),
    (0x066D, 0x066F),
    (0x0671, 0x06D5),
    (0x06E5, 0x06E6),
    (0x06EE, 0x06EF),
    // Arabic's last letters, Syriac.
    (0x06FA, 0x0710),
    (0x0712, 0x072F),
    // Syriac's and Arabic's supplements, Thaana.
    (0x074D, 0x07A5),
    (0x07B1, 0x07B1),
    // N'Ko.
    (0x07C0, 0x07EA),
    (0x07F4, 0x07F5),
    (0x07FA, 0x07FA),
    (0x07FE, 0x0815),
    // Samaritan, Mandaic, Syriac's supplement and Arabic's extensions (their letters).
    (0x081A, 0x081A),
    (0x0824, 0x0824),
    (0x0828, 0x0828),
    (0x0830, 0x0858),
    (0x085E, 0x088E),
    (0x08A0, 0x08C9),
    // Right-to-left mark, Arabic letter mark.
    (0x200F, 0x200F),
    (0x061C, 0x061C),
    // Hebrew and Arabic presentation forms.
    (0xFB1D, 0xFB1D),
    (0xFB1F, 0xFB28),
    (0xFB2A, 0xFD3D),
    (0xFD40, 0xFDCF),
    (0xFDF0, 0xFDFC),
    (0xFE70, 0xFEFE),
    // The historic scripts written from the right, and Adlam, Mende Kikakui and Arabic's
    // mathematical letters.
    (0x10800, 0x10FFF),
    (0x1E800, 0x1EFFF),
];

/// The blocks of the scripts written from the right: what in them isn't in [`RTL`] is a digit, a
/// mark or a sign, which says nothing of the direction.
const RTL_BLOCKS: &[(u32, u32)] = &[(0x0590, 0x08FF), (0xFB1D, 0xFDFF), (0xFE70, 0xFEFF), (0x10800, 0x10FFF), (0x1E800, 0x1EFFF)];

fn within(table: &[(u32, u32)], c: u32) -> bool {
    table.iter().any(|&(low, high)| (low..=high).contains(&c))
}

/// The class of a character, near enough for P2: a letter outside the blocks written from the
/// right reads left to right (a mark that counts as alphabetic does too, which only matters when
/// a paragraph starts with one).
fn class(c: char) -> Class {
    let n = u32::from(c);
    if within(RTL, n) {
        Class::Rtl
    } else if within(RTL_BLOCKS, n) {
        Class::Weak
    } else if c == '\u{200E}' || c.is_alphabetic() {
        Class::Ltr
    } else {
        Class::Weak
    }
}

#[cfg(test)]
#[path = "bidi_tests.rs"]
mod bidi_tests;
