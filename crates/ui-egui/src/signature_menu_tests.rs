// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1493: the menus of the signature editor's formatting bar. It isn't in a menu bar, so each of
//! them asks for the keeping of a click made inside it on its own.

use egui_kittest::kittest::Queryable as _;

use super::keeps_clicks;

#[test]
fn a_menu_of_the_formatting_bar_keeps_a_click_made_in_its_field() {
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(420.0, 320.0)).build_ui(|ui| {
            // What is typed is kept where a window keeps what a menu's field holds.
            let key = egui::Id::unique("signature-menu-field");
            let mut text: String = ui.data(|d| d.get_temp(key)).unwrap_or_default();
            keeps_clicks(ui, "Font", |ui| {
                ui.label("Arial");
                ui.add(egui::TextEdit::singleline(&mut text).hint_text("Find a font").desired_width(160.0));
            });
            ui.data_mut(|d| d.insert_temp(key, text));
        });
    harness.run();
    harness.get_by_label_contains("Font").click();
    harness.run();
    assert!(egui::Popup::is_any_open(&harness.ctx), "the menu is open");
    let field = harness.get_all_by_role(egui::accesskit::Role::TextInput).last().expect("the menu's field").rect();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: field.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        harness.step();
    }
    harness.run();
    assert!(egui::Popup::is_any_open(&harness.ctx), "clicking the field keeps the menu open (#1493)");
    harness.event(egui::Event::Text("fir".into()));
    harness.run();
    assert!(harness.get_all_by_value("fir").next().is_some(), "what is typed goes into the field");
    assert!(egui::Popup::is_any_open(&harness.ctx), "typing keeps the menu open");
}
