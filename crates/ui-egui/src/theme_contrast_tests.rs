// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contrast of the colours the interface sets against each other (WCAG 2.x relative
//! luminance), in every palette and accent: text 4.5:1, icons and the outlines of controls 3:1. Disabled controls are exempt
//! from the rule (checked at 3:1 so that they stay legible).

use egui::Color32;

use super::{Accent, Tokens};

const TEXT: f32 = 4.5;
const GRAPHIC: f32 = 3.0;
const STRONG: f32 = 7.0;

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
    ("accent_text on window", |t| t.accent_text, |t| t.canvas, TEXT),
    ("accent_text on selection", |t| t.accent_text, |t| t.selection, TEXT),
    ("on_accent on accent", |t| t.on_accent, |t| t.accent, TEXT),
    ("overdue on surface", |t| t.overdue, |t| t.surface, TEXT),
    ("overdue on flagged_row", |t| t.overdue, |t| t.flagged_row, TEXT),
    // The message list's row backgrounds under its senders (#1657): an unread one is in the
    // accent's text colour, on a flagged row or on a shaded one.
    ("accent_text on flagged_row", |t| t.accent_text, |t| t.flagged_row, TEXT),
    ("text on row_shade", |t| t.text, |t| t.row_shade(), TEXT),
    ("text_dim on row_shade", |t| t.text_dim, |t| t.row_shade(), TEXT),
    ("accent_text on row_shade", |t| t.accent_text, |t| t.row_shade(), TEXT),
    ("overdue on row_shade", |t| t.overdue, |t| t.row_shade(), TEXT),
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
    ("input_border on ribbon", |t| t.input_border, |t| t.ribbon, GRAPHIC),
    ("field_edge on window", |t| t.field_edge, |t| t.canvas, GRAPHIC),
    ("accent on surface", |t| t.accent, |t| t.surface, GRAPHIC),
    ("accent on panel", |t| t.accent, |t| t.panel, GRAPHIC),
    ("accent_soft_text on accent_soft", |t| t.accent_soft_text, |t| t.accent_soft, TEXT),
    ("text on accent_soft", |t| t.text, |t| t.accent_soft, TEXT),
    ("text_dim on pill_quiet", |t| t.text_dim, |t| t.pill_quiet, TEXT),
    ("text_dim on accent_soft", |t| t.text_dim, |t| t.accent_soft, TEXT),
    ("backstage_ink on column", |t| t.backstage_ink, |t| t.backstage_column, TEXT),
    ("text_disabled on window", |t| t.text_disabled, |t| t.canvas, GRAPHIC),
];

/// Pairs that only high contrast promises: its borders and hairlines are lines you can see.
const STRONG_PAIRS: &[Pair] = &[
    ("border on surface", |t| t.border, |t| t.surface, STRONG),
    ("border on window", |t| t.border, |t| t.canvas, STRONG),
    ("border on ribbon", |t| t.border, |t| t.ribbon, STRONG),
    ("border_strong on surface", |t| t.border_strong, |t| t.surface, STRONG),
    ("ribbon_edge on window", |t| t.ribbon_edge.color, |t| t.canvas, STRONG),
    ("rule on surface", |t| t.rule, |t| t.surface, GRAPHIC),
    ("border_faint on surface", |t| t.border_faint, |t| t.surface, GRAPHIC),
    ("selection_edge on selection", |t| t.selection_edge, |t| t.selection, STRONG),
    ("selection_edge_unfocused on selection_unfocused", |t| t.selection_edge_unfocused, |t| t.selection_unfocused, GRAPHIC),
    ("accent_soft_text on checked", |t| t.accent_soft_text, |t| t.checked, TEXT),
    ("text on pill_quiet", |t| t.text, |t| t.pill_quiet, TEXT),
];

/// What Classic Light promises on top (#1203): the edge of a selected row is seen on its wash,
/// with the keyboard's focus in the list and without it.
const CLASSIC_PAIRS: &[Pair] = &[
    ("selection_edge on selection", |t| t.selection_edge, |t| t.selection, GRAPHIC),
    ("selection_edge_unfocused on selection_unfocused", |t| t.selection_edge_unfocused, |t| t.selection_unfocused, GRAPHIC),
];

/// What a theme that fills the status bar with the accent promises (#1203): its words and the
/// pointer's shade on it read, and so does the File tab filled with the accent under the pointer.
const BAND_PAIRS: &[Pair] = &[
    ("text on the status band", |t| t.on_band(t.accent).text, |t| t.on_band(t.accent).status_bar, TEXT),
    ("text on the status band's hover", |t| t.on_band(t.accent).text, |t| t.on_band(t.accent).hover, TEXT),
    ("text on the status band pressed", |t| t.on_band(t.accent).text, |t| t.on_band(t.accent).pressed, TEXT),
    ("on_accent on the File tab under the pointer", |t| t.on_accent, |t| t.accent.lerp_to_gamma(Tokens::away_from(t.on_accent), 0.12), TEXT),
];

/// Every accent, light and dark (in both its styles, #1384) and Classic Light (#1203), with high
/// contrast off and on.
fn every_theme() -> Vec<(String, Tokens, bool)> {
    let mut all = Vec::new();
    for accent in Accent::ALL {
        for dark in [false, true] {
            for high in [false, true] {
                let name = format!("{} {} {}", accent.code(), if dark { "dark" } else { "light" }, if high { "high contrast" } else { "normal" });
                all.push((name, Tokens::build(dark, accent, high), high));
                if dark {
                    let name = format!("{} blue dark {}", accent.code(), if high { "high contrast" } else { "normal" });
                    all.push((name, Tokens::build_dark(super::DarkStyle::Blue, accent, high), high));
                } else {
                    let name = format!("{} classic light {}", accent.code(), if high { "high contrast" } else { "normal" });
                    all.push((name, Tokens::build_classic(accent, None, high), high));
                }
            }
        }
    }
    all
}

#[test]
fn blue_normal_is_the_look_that_shipped() {
    let (light, dark) = (Tokens::build(false, Accent::Blue, false), Tokens::build(true, Accent::Blue, false));
    assert_eq!(light.accent, Tokens::light().accent);
    assert_eq!(light.selection, Tokens::light().selection);
    assert_eq!(dark.accent_soft, Tokens::dark().accent_soft);
    assert_eq!(dark.text_selection, Tokens::dark().text_selection);
}

/// The pairs of one palette that fall short, with the mode named: the rules of `PAIRS` and, in
/// high contrast, of `STRONG_PAIRS`, with Classic Light's own (`CLASSIC_PAIRS`, and `BAND_PAIRS`
/// where the status bar is filled with the accent, #1203) (disabled controls are exempt: they stay
/// at 3:1 in both; in high contrast text and outlines need 7:1, icons stay at 3:1).
fn wrong_pairs(mode: &str, t: &Tokens, high: bool) -> Vec<String> {
    let mut wrong = Vec::new();
    let extra: &[Pair] = if high { STRONG_PAIRS } else { &[] };
    let classic: &[Pair] = if t.dense { CLASSIC_PAIRS } else { &[] };
    let band: &[Pair] = if t.status_band { BAND_PAIRS } else { &[] };
    for (name, fg, bg, min) in PAIRS.iter().chain(extra).chain(classic).chain(band) {
        let outline = name.contains("border") || name.contains("edge") || name.starts_with("accent on");
        let min = if !high || name.contains("disabled") {
            *min
        } else if *min == TEXT || (*min == GRAPHIC && outline && !name.starts_with("rule") && !name.contains("faint") && !name.contains("unfocused"))
        {
            STRONG
        } else {
            *min
        };
        let got = ratio(fg(t), bg(t));
        if got < min {
            wrong.push(format!("{name} ({mode}) is {got:.2}, needs {min}"));
        }
    }
    wrong
}

#[test]
fn token_pairs_meet_their_contrast() {
    let mut wrong = Vec::new();
    for (mode, t, high) in every_theme() {
        wrong.extend(wrong_pairs(&mode, &t, high));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A custom accent colour that already meets the contrast bound is kept as it is: the fit clamps
/// against the bound and does not move the colour to it (black stays black on a light window,
/// white stays white on a dark one), while an unsuitable one is still adjusted.
#[test]
fn custom_colours_that_already_pass_are_kept_as_they_are() {
    for high in [false, true] {
        assert_eq!(Tokens::build_choice(false, Accent::Custom, Some(Color32::BLACK), high).accent, Color32::BLACK);
        assert_eq!(Tokens::build_choice(true, Accent::Custom, Some(Color32::WHITE), high).accent, Color32::WHITE);
        assert_eq!(Tokens::build_dark_choice(super::DarkStyle::Blue, Accent::Custom, Some(Color32::WHITE), high).accent, Color32::WHITE);
    }
    // Yellow on a light window does not pass and is darkened; black on a dark window does not pass
    // and is lightened.
    let yellow = Color32::from_rgb(0xFF, 0xFF, 0x00);
    let fitted_light = Tokens::build_choice(false, Accent::Custom, Some(yellow), false).accent;
    assert!(luminance(fitted_light) < luminance(yellow));
    let fitted_dark = Tokens::build_choice(true, Accent::Custom, Some(Color32::BLACK), false).accent;
    assert!(luminance(fitted_dark) > 0.0);
}

/// The tick on the custom accent's swatch, which shows the colour as chosen: black or white,
/// whichever stands out more, and so never under 4.5:1 (#1737).
#[test]
fn the_tick_on_a_chosen_colour_is_the_better_of_black_and_white() {
    let mut colors = vec![Color32::BLACK, Color32::WHITE, Color32::from_rgb(0xFF, 0xFF, 0x00), Color32::from_rgb(0x12, 0x34, 0x56)];
    let mut x: u32 = 0x8765_4321;
    for _ in 0..256 {
        let mut next = || {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        };
        colors.push(Color32::from_rgb(next(), next(), next()));
    }
    for c in colors {
        let tick = super::black_or_white_on(c);
        let best = ratio(Color32::BLACK, c).max(ratio(Color32::WHITE, c));
        assert!((ratio(tick, c) - best).abs() < 1e-4, "{}", super::hex_string(c));
        assert!(ratio(tick, c) >= TEXT, "{}", super::hex_string(c));
    }
    // Pale yellow gets a black tick (the light theme's white ink would vanish on it), navy a white one.
    assert_eq!(super::black_or_white_on(Color32::from_rgb(0xFF, 0xF5, 0x9D)), Color32::BLACK);
    assert_eq!(super::black_or_white_on(Color32::from_rgb(0x12, 0x34, 0x56)), Color32::WHITE);
}

/// A custom accent: whatever colour the owner picks — the extremes of the cube and a
/// deterministic sweep of it — is fitted so the same pairs pass as for the presets, in the light
/// theme, in Classic Light (#1203) and in both greys of the dark one, with high contrast off and on.
#[test]
fn custom_accent_colours_meet_their_contrast() {
    let mut colors = vec![
        Color32::BLACK,
        // White in the light theme is the case where the window (canvas) is the strictest bound
        // for the accent as text, stricter than surface and panel.
        Color32::WHITE,
        Color32::RED,
        Color32::GREEN,
        Color32::BLUE,
        Color32::from_rgb(0xFF, 0xFF, 0x00),
        Color32::from_rgb(0x00, 0xFF, 0xFF),
        Color32::from_rgb(0xFF, 0x00, 0xFF),
        Color32::from_rgb(0x80, 0x80, 0x80),
        Color32::from_rgb(0x12, 0x34, 0x56),
        Color32::from_rgb(0xAB, 0xCD, 0xEF),
        // A reviewer found an unread sender in this one short on the shaded band in Dark High
        // Contrast (#1657).
        Color32::from_rgb(0xA3, 0xA3, 0xA3),
        super::DEFAULT_CUSTOM_COLOR,
    ];
    let mut x: u32 = 0x1234_5678;
    for _ in 0..96 {
        let mut next = || {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        };
        colors.push(Color32::from_rgb(next(), next(), next()));
    }
    let mut wrong = Vec::new();
    for c in colors {
        let hex = super::hex_string(c);
        for dark in [false, true] {
            for high in [false, true] {
                let hc = if high { "high contrast" } else { "normal" };
                let mut modes = vec![(format!("custom {hex} light {hc}"), Tokens::build_choice(false, Accent::Custom, Some(c), high))];
                if !dark {
                    modes.push((format!("custom {hex} classic light {hc}"), Tokens::build_classic(Accent::Custom, Some(c), high)));
                }
                if dark {
                    modes.push((format!("custom {hex} dark {hc}"), Tokens::build_choice(true, Accent::Custom, Some(c), high)));
                    modes.push((
                        format!("custom {hex} blue dark {hc}"),
                        Tokens::build_dark_choice(super::DarkStyle::Blue, Accent::Custom, Some(c), high),
                    ));
                }
                for (mode, t) in modes {
                    wrong.extend(wrong_pairs(&mode, &t, high));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The shaded band (#1657) is there to see against the plain row in every theme: 1.08:1 in the
/// light one, more in the others. It is the same whatever the accent, which is fitted to it.
#[test]
fn the_shaded_band_stays_visible() {
    let faint: Vec<String> = every_theme()
        .iter()
        .filter(|(_, t, _)| ratio(t.row_shade(), t.surface) < 1.06)
        .map(|(mode, t, _)| format!("{mode}: {:.3}", ratio(t.row_shade(), t.surface)))
        .collect();
    assert!(faint.is_empty(), "{}", faint.join("\n"));
}

/// A message-list row's text on whatever is under it (#1657): the list or its shaded band, a
/// flag's tint, a selection (with and without the focus, over a flag's tint or not) or the
/// pointer's shade. Plain and dim text read on every one as they are; the coloured inks (the
/// accent's for an unread sender, overdue's, the red "[Draft]") are fitted to it, in every preset
/// and a sweep of custom accents.
#[test]
fn a_list_rows_text_reads_on_whatever_is_under_it() {
    let mut themes = every_theme();
    // #A3A3A3 first, the one a reviewer found an unread sender short with on the shaded band in
    // Dark High Contrast, then a sweep of 60 more.
    let mut customs = vec![Color32::from_rgb(0xA3, 0xA3, 0xA3)];
    let mut x: u32 = 0x1234_5678;
    for _ in 0..60 {
        let mut next = || {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        };
        customs.push(Color32::from_rgb(next(), next(), next()));
    }
    for c in customs {
        let hex = super::hex_string(c);
        for high in [false, true] {
            themes.push((format!("custom {hex} light {high}"), Tokens::build_choice(false, Accent::Custom, Some(c), high), high));
            themes.push((format!("custom {hex} classic light {high}"), Tokens::build_classic(Accent::Custom, Some(c), high), high));
            themes.push((format!("custom {hex} dark {high}"), Tokens::build_choice(true, Accent::Custom, Some(c), high), high));
            let blue = Tokens::build_dark_choice(super::DarkStyle::Blue, Accent::Custom, Some(c), high);
            themes.push((format!("custom {hex} blue dark {high}"), blue, high));
        }
    }
    let mut wrong = Vec::new();
    for (mode, t, high) in &themes {
        let need = if *high { STRONG } else { TEXT };
        for (shaded, flagged, selected, hovered) in row_unders() {
            let under = t.list_row_layers(shaded, flagged, selected, hovered).under();
            let inks = [
                ("text", t.text),
                ("text_dim", t.text_dim),
                ("accent_text", t.text_ink_on(t.accent_text, under)),
                ("overdue", t.text_ink_on(t.overdue, under)),
                ("[Draft]", t.text_ink_on(t.red, under)),
            ];
            for (name, ink) in inks {
                let r = ratio(ink, under);
                if r < need {
                    wrong.push(format!("{mode}: {name} shaded {shaded} flagged {flagged} selected {selected:?} hovered {hovered}: {r:.2}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every way a row can be laid: shaded or not, flagged or not, not selected (under the pointer
/// or not) or selected with the list's focus or without.
fn row_unders() -> Vec<(bool, bool, Option<bool>, bool)> {
    let mut all = Vec::new();
    for shaded in [false, true] {
        for flagged in [false, true] {
            for (selected, hovered) in [(None, false), (None, true), (Some(false), false), (Some(true), false)] {
                all.push((shaded, flagged, selected, hovered));
            }
        }
    }
    all
}
