// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Emoji in every font of the window (#832).

use egui::{FontFamily, FontId};

use super::{SystemFont, font_definitions_with};

/// Emoji as messages send them: a wink, a smile with sweat, a thumbs up, a heart.
const EMOJI: &str = "😉😅👍❤";

/// Every family of the window draws emoji: since egui 0.37
/// its emoji faces come only with their own feature, and without them each was an empty box.
#[test]
fn every_family_draws_emoji() {
    let fonts = font_definitions_with(&SystemFont::none());
    let families: Vec<FontFamily> = fonts.families.keys().cloned().collect();
    let ctx = egui::Context::default();
    ctx.set_fonts(fonts);
    ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
    for family in families {
        let id = FontId::new(12.0, family.clone());
        assert!(ctx.fonts_mut(|f| f.has_glyphs(&id, EMOJI)), "{family:?} lacks emoji");
    }
}
