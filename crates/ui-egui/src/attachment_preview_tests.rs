// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn attachment_safety_warning_mirrors_reading_edge() {
    for rtl in [false, true] {
        let ctx = egui::Context::default();
        let mut icon = egui::Rect::NOTHING;
        let mut short = egui::Rect::NOTHING;
        let mut long = egui::Rect::NOTHING;
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 300.0))), ..Default::default() };
        ctx.run_ui(input, |ui| {
            warning_row(ui, rtl, |ui| {
                icon = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover()).0;
                warning_details(ui, rtl, |ui| {
                    short = ui.label("Trust sender").rect;
                    long = ui.label("A preview may omit content from the file").rect;
                });
            });
        })
        .drop_without_applying_deltas();
        assert!(icon.is_finite() && short.is_finite() && long.is_finite());
        if rtl {
            assert!(long.max.x <= icon.min.x, "RTL icon must be on the right reading edge");
            assert!((short.max.x - long.max.x).abs() < 1.0, "RTL captions share the right edge");
        } else {
            assert!(icon.max.x <= long.min.x, "LTR icon must be on the left reading edge");
            assert!((short.min.x - long.min.x).abs() < 1.0, "LTR captions share the left edge");
        }
    }
}
