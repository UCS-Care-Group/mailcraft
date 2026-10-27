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
