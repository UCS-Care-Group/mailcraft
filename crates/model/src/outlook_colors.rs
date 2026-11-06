// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The colours a Microsoft account's categories can have (#661): Outlook numbers 25 of them, 0 to
//! 24 (red, orange, brown, yellow, green, teal, olive, blue, purple, cranberry, steel, dark steel,
//! gray, dark gray, black, then the dark ones of the first ten), as Graph names them (`preset0` to
//! `preset24`) and a .pst file's list of categories numbers them, and leaves the shade to the
//! program that shows them. These are our own shades for them, the one table of them.

use crate::Rgb;

/// Our own colour for each of Graph's numbered colours, `preset0` first.
pub const OUTLOOK_COLORS: [Rgb; 25] = [
    Rgb(0xD1, 0x3B, 0x3B),
    Rgb(0xE0, 0x7B, 0x1F),
    Rgb(0x9C, 0x6B, 0x3F),
    Rgb(0xD9, 0xA4, 0x0C),
    Rgb(0x2E, 0x8B, 0x57),
    Rgb(0x1A, 0x9B, 0xA6),
    Rgb(0x7A, 0x8B, 0x2E),
    Rgb(0x2F, 0x6F, 0xD0),
    Rgb(0x7E, 0x57, 0xC2),
    Rgb(0xB0, 0x2E, 0x6B),
    Rgb(0x6E, 0x8A, 0xA5),
    Rgb(0x4A, 0x5F, 0x73),
    Rgb(0x8A, 0x8A, 0x8A),
    Rgb(0x5A, 0x5A, 0x5A),
    Rgb(0x2B, 0x2B, 0x2B),
    Rgb(0x8E, 0x1F, 0x1F),
    Rgb(0xA8, 0x52, 0x0C),
    Rgb(0x6B, 0x45, 0x23),
    Rgb(0x9A, 0x73, 0x00),
    Rgb(0x1C, 0x5E, 0x38),
    Rgb(0x0F, 0x66, 0x6E),
    Rgb(0x52, 0x5E, 0x1A),
    Rgb(0x1B, 0x45, 0x8A),
    Rgb(0x52, 0x35, 0x8A),
    Rgb(0x78, 0x1B, 0x47),
];

/// The colour Outlook numbers `n` (0 to 24); `None` for any other.
pub fn preset_category_color(n: i64) -> Option<Rgb> {
    usize::try_from(n).ok().and_then(|n| OUTLOOK_COLORS.get(n).copied())
}

/// The colour a server gives as `name` (`preset7`, any case); grey for `none` or anything else.
pub fn outlook_color(name: &str) -> Rgb {
    let number = name.trim().to_ascii_lowercase().strip_prefix("preset").and_then(|n| n.parse::<i64>().ok());
    number.and_then(preset_category_color).unwrap_or(crate::GREY_CATEGORY)
}

/// Which of the 25 `color` is (`Some(7)` for `preset7`), when it is one.
pub fn outlook_preset(color: Rgb) -> Option<usize> {
    OUTLOOK_COLORS.iter().position(|c| *c == color)
}

/// The one of the 25 nearest to `color`: what a category of another colour becomes when it goes
/// to a Microsoft account's server.
pub fn nearest_outlook_color(color: Rgb) -> Rgb {
    let distance = |c: &Rgb| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(c.0, color.0) + d(c.1, color.1) + d(c.2, color.2)
    };
    OUTLOOK_COLORS.iter().min_by_key(|c| distance(c)).copied().unwrap_or(crate::GREY_CATEGORY)
}
