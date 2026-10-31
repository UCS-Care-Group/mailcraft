// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contrast of the colours the interface sets against each other (WCAG 2.x relative
//! luminance): text 4.5:1, icons and the outlines of controls 3:1. Disabled controls are exempt
//! from the rule but listed so that they are watched.

use egui::Color32;

use super::{Palette, Tokens};

const TEXT: f32 = 4.5;
const GRAPHIC: f32 = 3.0;

fn channel(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn luminance(c: Color32) -> f32 {
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// The contrast ratio of two opaque colours.
fn ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

type Pair = (&'static str, fn(&Tokens) -> Color32, fn(&Tokens) -> Color32, f32);

const PAIRS: &[Pair] = &[
    ("text on surface", |t| t.text, |t| t.surface, TEXT),
    ("text on panel", |t| t.text, |t| t.panel, TEXT),
    ("text on window", |t| t.text, |t| t.canvas, TEXT),
    ("text on ribbon", |t| t.text, |t| t.ribbon, TEXT),
    ("text on selection", |t| t.text, |t| t.selection, TEXT),
    ("text on flagged_row", |t| t.text, |t| t.flagged_row, TEXT),
    ("text on hover", |t| t.text, |t| t.hover, TEXT),
    ("text on checked", |t| t.text, |t| t.checked, TEXT),
    ("text on today", |t| t.text, |t| t.today, TEXT),
    ("text_dim on surface", |t| t.text_dim, |t| t.surface, TEXT),
    ("text_dim on panel", |t| t.text_dim, |t| t.panel, TEXT),
    ("text_dim on window", |t| t.text_dim, |t| t.canvas, TEXT),
    ("text_dim on selection", |t| t.text_dim, |t| t.selection, TEXT),
    ("text_dim on flagged_row", |t| t.text_dim, |t| t.flagged_row, TEXT),
    ("text_disabled on surface", |t| t.text_disabled, |t| t.surface, GRAPHIC),
    ("text_disabled on ribbon", |t| t.text_disabled, |t| t.ribbon, GRAPHIC),
    ("group_label on ribbon", |t| t.group_label, |t| t.ribbon, TEXT),
    ("group_label on window", |t| t.group_label, |t| t.canvas, TEXT),
    ("accent_text on surface", |t| t.accent_text, |t| t.surface, TEXT),
    ("accent_text on panel", |t| t.accent_text, |t| t.panel, TEXT),
    ("accent_text on selection", |t| t.accent_text, |t| t.selection, TEXT),
    ("on_accent on accent", |t| t.on_accent, |t| t.accent, TEXT),
    ("overdue on surface", |t| t.overdue, |t| t.surface, TEXT),
    ("overdue on flagged_row", |t| t.overdue, |t| t.flagged_row, TEXT),
    ("icon on ribbon", |t| t.icon, |t| t.ribbon, GRAPHIC),
    ("input_border on surface", |t| t.input_border, |t| t.surface, GRAPHIC),
    ("red on surface", |t| t.red, |t| t.surface, TEXT),
    ("green on surface", |t| t.green, |t| t.surface, TEXT),
    ("orange on surface", |t| t.orange, |t| t.surface, GRAPHIC),
    ("blue on surface", |t| t.blue, |t| t.surface, TEXT),
    ("icon_red on surface", |t| t.icon_red, |t| t.surface, GRAPHIC),
    ("icon_yellow on surface", |t| t.icon_yellow, |t| t.surface, GRAPHIC),
    ("icon_green on surface", |t| t.icon_green, |t| t.surface, GRAPHIC),
    ("icon_purple on surface", |t| t.icon_purple, |t| t.surface, GRAPHIC),
];

/// What falls short today, as (pair, dark). Each is asserted to still fall short, so that the
/// slice that fixes one has to take it off this list.
const KNOWN_FAILURES: &[(&str, bool)] = &[
    ("text_disabled on surface", false),
    ("text_disabled on ribbon", false),
    ("text_disabled on ribbon", true),
    ("group_label on window", false),
    ("input_border on surface", false),
    ("input_border on surface", true),
    ("green on surface", false),
    ("orange on surface", false),
];

#[test]
fn token_pairs_meet_their_contrast_or_are_known_not_to() {
    let palette = Palette::builtin();
    let mut wrong = Vec::new();
    for dark in [false, true] {
        let t = palette.tokens(dark);
        for (name, fg, bg, min) in PAIRS {
            let got = ratio(fg(&t), bg(&t));
            let known = KNOWN_FAILURES.contains(&(*name, dark));
            let mode = if dark { "dark" } else { "light" };
            if known && got >= *min {
                wrong.push(format!("{name} ({mode}) is {got:.2}, now passes {min}: take it off the list"));
            } else if !known && got < *min {
                wrong.push(format!("{name} ({mode}) is {got:.2}, needs {min}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_known_failures_name_real_pairs() {
    for (name, _) in KNOWN_FAILURES {
        assert!(PAIRS.iter().any(|p| p.0 == *name), "{name}");
    }
}
