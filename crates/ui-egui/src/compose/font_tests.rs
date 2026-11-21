// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A run is drawn in the font it was given (#582).

use egui::FontFamily;
use mailcraft_model::rich::{Font, Style};

use super::editor::text_format;
use crate::theme::{Tokens, document_family, regular};

fn georgia() -> Style {
    Style { font: Some(Font::Georgia), ..Style::default() }
}

#[test]
fn a_run_is_drawn_in_the_family_registered_for_its_font() {
    let t = Tokens::light();
    let family = document_family(Font::Georgia, false);
    let format = text_format(&georgia(), false, &t, &|f: &FontFamily| *f == family);
    assert_eq!(format.font_id.family, family);
}

#[test]
fn a_bold_run_is_drawn_in_the_bold_family() {
    let t = Tokens::light();
    let style = Style { bold: true, ..georgia() };
    let format = text_format(&style, false, &t, &|_: &FontFamily| true);
    assert_eq!(format.font_id.family, document_family(Font::Georgia, true));
}

#[test]
fn a_run_is_drawn_in_the_windows_font_when_none_was_registered() {
    let t = Tokens::light();
    let format = text_format(&georgia(), false, &t, &|_: &FontFamily| false);
    assert_eq!(format.font_id.family, regular(14.0).family);
    // A plain-text message draws everything in the window's font.
    let plain = text_format(&georgia(), true, &t, &|_: &FontFamily| true);
    assert_eq!(plain.font_id.family, regular(14.0).family);
}

fn courier_new() -> Style {
    Style { font: Font::parse("Courier New"), ..Style::default() }
}

#[test]
fn a_fixed_width_run_stays_fixed_width_when_its_face_isnt_installed() {
    // #969: text art keeps its columns while it is written, Courier New installed or not.
    let t = Tokens::light();
    let format = text_format(&courier_new(), false, &t, &|_: &FontFamily| false);
    assert_eq!(format.font_id.family, FontFamily::Monospace);
    let bold = Style { bold: true, ..courier_new() };
    assert_eq!(text_format(&bold, false, &t, &|_: &FontFamily| false).font_id.family, FontFamily::Monospace);
}

#[test]
fn a_bold_fixed_width_run_without_a_bold_face_keeps_the_regular_face() {
    let t = Tokens::light();
    let regular_face = document_family(Font::parse("Courier New").unwrap(), false);
    let bold = Style { bold: true, ..courier_new() };
    let format = text_format(&bold, false, &t, &|f: &FontFamily| *f == regular_face);
    assert_eq!(format.font_id.family, regular_face);
}
