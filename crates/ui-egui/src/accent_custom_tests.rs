// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › General › Accent: the fifth swatch, "Custom color", its picker with the hex
//! field, and choosing another accent again (#1737).

use egui::accesskit::Role;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::people_view::testing::{self, Window};
use crate::theme::Accent;

/// The window on the sample mailbox, with File › Options open on General.
fn options_window() -> Window {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    w.run_steps(5);
    w
}

fn swatch<'a>(w: &'a Window, name: &'a str) -> egui_kittest::Node<'a> {
    w.get_by_role_and_label(Role::RadioButton, name)
}

/// A shot of the window when `MAILCRAFT_SHOTS_DIR` says where (the headless shots are taken the
/// same way, in `ui_shot`).
fn shot(w: &mut Window, name: &str) {
    if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
        w.render().unwrap().save(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
    }
}

#[test]
fn the_accent_row_has_five_swatches_and_the_fifth_opens_a_picker() {
    let mut w = options_window();
    for name in [tl!("Blue"), tl!("Teal"), tl!("Violet"), tl!("Graphite"), tl!("Custom color")] {
        swatch(&w, name);
    }
    assert_eq!(w.state().ui.accent, Accent::Blue);
    assert!(!egui::Popup::is_any_open(&w.ctx));
    swatch(&w, tl!("Custom color")).click();
    w.run_steps(4);
    // The fifth swatch is chosen, and its picker is open under it.
    assert_eq!(w.state().ui.accent, Accent::Custom);
    assert!(egui::Popup::is_any_open(&w.ctx), "the picker opened");
    shot(&mut w, "options-custom-picker");
    // No colour picked yet: the default custom colour shows, and the command says so.
    assert_eq!(w.state().ui.custom_accent, None);
    assert_eq!(w.state().ui.custom_accent_color(), crate::theme::DEFAULT_CUSTOM_COLOR);
}

#[test]
fn the_picker_takes_a_colour_from_its_hex_field_and_applies_it_at_once() {
    let mut w = options_window();
    swatch(&w, tl!("Custom color")).click();
    w.run_steps(4);
    w.get_by_role_and_label(Role::TextInput, tl!("Accent colour as hex")).set_text_value_accesskit("#00FF88");
    w.run_steps(4);
    assert_eq!(w.state().ui.accent, Accent::Custom);
    assert_eq!(w.state().ui.custom_accent, Some(egui::Color32::from_rgb(0x00, 0xFF, 0x88)));
    // The window shows it at once.
    let t = crate::theme::Tokens::get(&w.ctx);
    let want = crate::theme::Tokens::build_choice(t.dark, Accent::Custom, Some(egui::Color32::from_rgb(0x00, 0xFF, 0x88)), t.high_contrast);
    assert_eq!(t.accent, want.accent);
    shot(&mut w, "options-custom-applied");
    // A colour that doesn't parse changes nothing.
    w.get_by_role_and_label(Role::TextInput, tl!("Accent colour as hex")).set_text_value_accesskit("not a colour");
    w.run_steps(4);
    assert_eq!(w.state().ui.custom_accent, Some(egui::Color32::from_rgb(0x00, 0xFF, 0x88)));
    // Escape closes the picker; the accent stays.
    testing::key(&mut w, egui::Key::Escape);
    w.run_steps(2);
    assert!(!egui::Popup::is_any_open(&w.ctx));
    assert_eq!(w.state().ui.accent, Accent::Custom);
}

#[test]
fn choosing_a_preset_again_closes_the_picker_and_sets_the_preset() {
    let mut w = options_window();
    w.state_mut().execute("ui.accent", json!({"accent": "custom", "color": "#123456"})).unwrap();
    w.run_steps(3);
    swatch(&w, tl!("Custom color")).click();
    w.run_steps(4);
    assert!(egui::Popup::is_any_open(&w.ctx));
    // The custom swatch shows the colour picked, as its disc.
    swatch(&w, tl!("Teal")).click();
    w.run_steps(4);
    assert_eq!(w.state().ui.accent, Accent::Teal);
    assert_eq!(w.state().ui.custom_accent, Some(egui::Color32::from_rgb(0x12, 0x34, 0x56)));
    assert!(!egui::Popup::is_any_open(&w.ctx), "the picker closed");
    shot(&mut w, "options-teal-again");
}

#[test]
fn a_command_that_switches_the_accent_away_closes_the_picker_too() {
    let mut w = options_window();
    swatch(&w, tl!("Custom color")).click();
    w.run_steps(4);
    assert!(egui::Popup::is_any_open(&w.ctx));
    // No click is involved here (the control channel, MCP): the picker must still close.
    w.state_mut().execute("ui.accent", json!({"accent": "violet"})).unwrap();
    w.run_steps(4);
    assert_eq!(w.state().ui.accent, Accent::Violet);
    assert!(!egui::Popup::is_any_open(&w.ctx), "the picker closed when the command switched the accent");
}

#[test]
fn the_picker_starts_from_the_saved_colour_and_follows_external_changes() {
    let mut w = options_window();
    w.state_mut().execute("ui.accent", json!({"accent": "custom", "color": "#FF0000"})).unwrap();
    w.run_steps(3);
    let hex_field = |w: &Window| w.get_by_role_and_label(Role::TextInput, tl!("Accent colour as hex")).value();
    // Open, close, change the colour through the command, open again: the picker shows the new one.
    swatch(&w, tl!("Custom color")).click();
    w.run_steps(4);
    assert_eq!(hex_field(&w).as_deref(), Some("#FF0000"));
    testing::key(&mut w, egui::Key::Escape);
    w.run_steps(2);
    assert!(!egui::Popup::is_any_open(&w.ctx));
    w.state_mut().execute("ui.accent", json!({"accent": "#0000FF"})).unwrap();
    w.run_steps(2);
    swatch(&w, tl!("Custom color")).click();
    w.run_steps(4);
    assert!(egui::Popup::is_any_open(&w.ctx));
    assert_eq!(hex_field(&w).as_deref(), Some("#0000FF"), "reopened on the colour the command set");
    // A change from outside while the picker is open synchronizes the field too.
    w.state_mut().execute("ui.accent", json!({"accent": "custom", "color": "#00FF00"})).unwrap();
    w.run_steps(4);
    assert_eq!(hex_field(&w).as_deref(), Some("#00FF00"));
    // An incomplete hex edit is kept while the saved colour has not changed.
    w.get_by_role_and_label(Role::TextInput, tl!("Accent colour as hex")).set_text_value_accesskit("#12");
    w.run_steps(4);
    assert_eq!(hex_field(&w).as_deref(), Some("#12"));
    assert_eq!(w.state().ui.custom_accent, Some(egui::Color32::from_rgb(0x00, 0xFF, 0x00)));
}
