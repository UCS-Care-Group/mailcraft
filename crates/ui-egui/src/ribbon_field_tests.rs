// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Menus that hold a text field keep a click made inside them (#1493): the click is for the
//! field, not a way out of the menu. Menus whose items are plain buttons — a group folded behind
//! `…` — still shut on a click anywhere inside them.

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

/// The whole window, its frames a sixtieth of a second apart, with the sample's reminders put away
/// so that the ribbon's left end is free to click.
fn window(width: f32) -> Harness {
    let state = app();
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(egui::vec2(width, 900.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            state,
        );
    harness.run_steps(3);
    // The sample mailbox's reminders float over the ribbon's left end.
    crate::reminders::dismiss(harness.state_mut(), None);
    harness.run_steps(2);
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

/// A new message, and its ribbon open at a menu of the Message tab; where the menu hangs from.
fn menu(harness: &mut Harness, key: &'static str) -> egui::Rect {
    let _ = harness.state_mut().run("mail.new", json!({}));
    harness.run_steps(3);
    // A button that also runs a command opens its menu from the `▾` half of it.
    let rect = harness
        .state()
        .hit
        .rect("ribbonMenu", hit_id(key))
        .or_else(|| harness.state().hit.rect("ribbon", hit_id(key)))
        .unwrap_or_else(|| panic!("the ribbon's {key} menu"));
    click(harness, rect.center());
    assert!(egui::Popup::is_any_open(&harness.ctx), "the {key} menu is open");
    rect
}

/// Where the text box a menu holds is: the one drawn nearest under where the menu hangs from.
fn field(harness: &mut Harness, under: egui::Rect) -> egui::Rect {
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .map(|b| (b.rect(), b.rect().center().distance(under.max)))
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(rect, _)| rect)
        .unwrap_or_else(|| panic!("no text box to type in"))
}

/// The Font Color menu stays open while its field is clicked and typed in; an item of it still
/// shuts it, and so does a click outside it (#1493).
#[test]
fn a_menu_with_a_field_keeps_a_click_made_inside_it() {
    let mut harness = window(1400.0);
    let under = menu(&mut harness, "fontColor");
    let box_rect = field(&mut harness, under);
    click(&mut harness, box_rect.center());
    assert!(egui::Popup::is_any_open(&harness.ctx), "clicking the field keeps the menu open");
    type_text(&mut harness, "800000");
    assert!(egui::Popup::is_any_open(&harness.ctx), "typing in the field keeps the menu open");
    // What was typed is what the field applies: it went into the field, not into the composer.
    harness.get_by_label_contains("Apply").click();
    harness.run_steps(3);
    assert_eq!(crate::ribbon::look_now(harness.state()).color, Some(Some("#800000".to_string())), "the colour typed was applied");
    click(&mut harness, egui::pos2(1200.0, 700.0));
    assert!(!egui::Popup::is_any_open(&harness.ctx), "a click outside the menu shuts it");
}

/// An item of a menu that holds a field shuts it as any item does (#1493).
#[test]
fn an_item_of_a_menu_with_a_field_still_shuts_it() {
    let mut harness = window(1400.0);
    menu(&mut harness, "fontColor");
    harness.get_by_label_contains("Automatic").click();
    harness.run_steps(3);
    assert!(!egui::Popup::is_any_open(&harness.ctx), "Automatic, chosen, shuts the menu");
}

/// File › Signatures › Colour holds the same field, and keeps a click in it too: it shut on that
/// click, so a colour of the owner's own could not be typed (#1464).
#[test]
fn the_signature_colour_menu_keeps_a_click_in_its_field() {
    let mut harness = window(1400.0);
    let app = harness.state_mut();
    app.run("signature.set", json!({"name": "Mine", "text": "Avery"})).unwrap();
    app.run("signature.open", json!({"name": "Mine"})).unwrap();
    app.run("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    harness.run_steps(3);
    harness.state_mut().run("compose.select", json!({"from": 0, "to": 5})).unwrap();
    harness.run_steps(3);
    let under = harness.get_by_label("Colour").rect();
    click(&mut harness, under.center());
    assert!(egui::Popup::is_any_open(&harness.ctx), "the Colour menu is open");
    let box_rect = field(&mut harness, under);
    click(&mut harness, box_rect.center());
    assert!(egui::Popup::is_any_open(&harness.ctx), "clicking the field keeps the menu open");
    type_text(&mut harness, "#fdb200");
    harness.get_by_label_contains("Apply").click();
    harness.run_steps(3);
    let body = harness.state().session.signature_edit.as_ref().unwrap().body.text().clone();
    let first = body.paragraphs().first().and_then(|p| p.runs.first()).and_then(|r| r.style.color.clone());
    assert_eq!(first.as_deref(), Some("#fdb200"), "the colour typed was applied: {body:?}");
}
