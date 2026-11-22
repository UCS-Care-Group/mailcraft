// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The font menu of the composer's ribbon, as the window draws it: it opens, its field narrows
//! the list down while it is typed in, and what is typed stays in the field.

use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use serde_json::json;

use crate::MailApp;
use crate::ribbon::hit_id;

type Harness = egui_kittest::Harness<'static, MailApp>;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(1_791_993_600, 0), 1_791_993_600, 0), crate::Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app
}

/// The whole window, its frames a sixtieth of a second apart.
fn window(app: MailApp) -> Harness {
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(3);
    harness
}

fn click(harness: &mut Harness, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
}

fn type_text(harness: &mut Harness, text: &str) {
    harness.event(egui::Event::Text(text.into()));
    harness.run_steps(2);
}

fn key(harness: &mut Harness, key: egui::Key) {
    for pressed in [true, false] {
        harness.event(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(2);
}

/// A New Email in the reading pane, its Font menu opened, and the keyboard in its field.
fn font_menu() -> Harness {
    let mut harness = window(app());
    // The sample mailbox's reminders float over the ribbon's left end, where Font sits.
    crate::reminders::dismiss(harness.state_mut(), None);
    let _ = harness.state_mut().run("mail.new", json!({}));
    harness.run_steps(3);
    let rect = harness.state().hit.rect("ribbon", hit_id("font")).expect("the ribbon's Font menu");
    click(&mut harness, rect.center());
    assert!(egui::Popup::is_any_open(&harness.ctx), "the Font menu is open");
    harness
}

/// What the menu's field holds.
fn query(harness: &Harness) -> String {
    harness.ctx.data(|d| d.get_temp::<String>(crate::ribbon::font_search_key())).unwrap_or_default()
}

/// Typing in the menu's field keeps the menu open and puts the words in the field (#1493).
#[test]
fn typing_in_the_field_narrows_the_list_and_keeps_the_menu_open() {
    let mut harness = font_menu();
    let field = harness.state().hit.rect("fontSearch", 0).expect("the menu's field");
    click(&mut harness, field.center());
    assert!(harness.ctx.memory(|m| m.focused().is_some()), "the field has the keyboard");
    type_text(&mut harness, "fi");
    assert!(egui::Popup::is_any_open(&harness.ctx), "typing keeps the menu open");
    assert_eq!(query(&harness), "fi");
    type_text(&mut harness, "ra");
    assert!(egui::Popup::is_any_open(&harness.ctx), "more typing keeps the menu open");
    assert_eq!(query(&harness), "fira");
}

/// Backspace takes a letter off the field; it doesn't close the menu or mean anything else.
#[test]
fn backspace_in_the_field_takes_a_letter_off() {
    let mut harness = font_menu();
    let field = harness.state().hit.rect("fontSearch", 0).expect("the menu's field");
    click(&mut harness, field.center());
    type_text(&mut harness, "noto");
    key(&mut harness, egui::Key::Backspace);
    assert!(egui::Popup::is_any_open(&harness.ctx), "Backspace keeps the menu open");
    assert_eq!(query(&harness), "not");
}

/// Escape closes the menu, and nothing else does.
#[test]
fn escape_closes_the_menu() {
    let mut harness = font_menu();
    key(&mut harness, egui::Key::Escape);
    assert!(!egui::Popup::is_any_open(&harness.ctx), "Escape closes the menu");
}

/// A click outside the menu closes it, as it did before the menu could be typed in (#1493).
#[test]
fn a_click_outside_closes_the_menu() {
    let mut harness = font_menu();
    click(&mut harness, egui::pos2(900.0, 600.0));
    assert!(!egui::Popup::is_any_open(&harness.ctx), "a click outside closes the menu");
}

/// A family chosen after typing is the message's font, and the menu shuts (#1493).
#[test]
fn a_family_chosen_after_typing_is_the_font_and_shuts_the_menu() {
    let mut harness = font_menu();
    let field = harness.state().hit.rect("fontSearch", 0).unwrap();
    click(&mut harness, field.center());
    type_text(&mut harness, "co");
    assert!(egui::Popup::is_any_open(&harness.ctx), "the menu is still open after typing");
    assert!(harness.query_by_label_contains("Courier New").is_some(), "the six are still offered, narrowed down");
    harness.get_by_label_contains("Courier New").click();
    harness.run_steps(3);
    assert!(!egui::Popup::is_any_open(&harness.ctx), "choosing a family shuts the menu");
    let font = crate::ribbon::look_now(harness.state()).font;
    assert_eq!(font.as_ref().map(|f| f.as_ref().map(|f| f.label().to_string())), Some(Some("Courier New".to_string())));
}
