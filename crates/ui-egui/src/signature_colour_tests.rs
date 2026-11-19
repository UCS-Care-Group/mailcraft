// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Signatures: the Colour menu takes a colour typed as hex (#1464).

use egui_kittest::kittest::Queryable as _;
use serde_json::json;

use crate::people_view::testing::{self, Window};

/// The signatures page with a `Standard` signature open, its text selected.
fn signatures() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().execute("signature.set", json!({"name": "Standard", "text": "Avery"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.run_steps(3);
    w.state_mut().execute("compose.select", json!({"from": 0, "to": 5})).unwrap();
    w
}

fn colour_now(w: &Window) -> Option<String> {
    w.state().session.signature_edit.as_ref().and_then(|e| e.body.text().style_at(2).color.clone())
}

/// Open Colour, click in the hex field, type `text`.
fn type_colour(w: &mut Window, text: &str) {
    let button = w.get_by_label("Colour").rect();
    w.get_by_label("Colour").click();
    w.run_steps(3);
    // The text box nearest under the button: the menu's, not the editor's.
    let field = w
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .map(|b| b.rect())
        .min_by(|a, b| a.center().distance(button.center()).total_cmp(&b.center().distance(button.center())))
        .unwrap();
    testing::click(w, field.center(), egui::PointerButton::Primary, 1);
    assert!(egui::Popup::is_any_open(&w.ctx), "a click in the field keeps the menu open");
    testing::text(w, text);
    w.run_steps(2);
}

#[test]
fn a_typed_hex_colour_is_applied_in_the_signature_editor() {
    let mut w = signatures();
    type_colour(&mut w, "#fdb200");
    w.get_by_label("Apply").click();
    w.run_steps(3);
    assert_eq!(colour_now(&w), Some("#fdb200".to_string()));
}

#[test]
fn an_invalid_colour_applies_nothing_and_says_so() {
    let mut w = signatures();
    type_colour(&mut w, "#fdb2");
    assert!(w.query_by_label_contains("Use six digits").is_some(), "the field says what it wants");
    assert_eq!(colour_now(&w), None);
}
