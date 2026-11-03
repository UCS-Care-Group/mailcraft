// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The search box's text layout (#652): an empty query must still lay out one row as tall as the
//! font's, so the caret (which stands on the galley's row) is as tall as the text and centred on it.

use crate::search_pills::layout;
use crate::theme::Tokens;
use egui::FontId;

fn galley_height(text: &str, size: f32) -> f32 {
    let ctx = egui::Context::default();
    let mut height = 0.0;
    let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
        let job = layout(text, &[], FontId::proportional(size), &Tokens::light());
        height = ui.fonts_mut(|f| f.layout_job(job)).size().y;
    });
    frame.textures_delta.clear();
    height
}

#[test]
fn an_empty_query_has_a_row_as_tall_as_the_text() {
    for size in [11.0, 13.0, 17.0] {
        let empty = galley_height("", size);
        let full = galley_height("Ag", size);
        assert!(full > 0.0 && (empty - full).abs() < 0.01, "size {size}: empty {empty}, text {full}");
    }
}
