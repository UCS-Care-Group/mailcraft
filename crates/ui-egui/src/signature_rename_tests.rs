// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Signatures › Rename: the new name is kept (#1156).

use egui::Key;
use egui_kittest::kittest::Queryable as _;
use serde_json::json;

use crate::people_view::testing::{self, Window};

/// The window on the sample mailbox with a `Standard` signature, on File › Signatures.
fn signatures() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().execute("signature.set", json!({"name": "Standard", "text": "Avery"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.run_steps(3);
    w
}

/// Rename through the field: Rename, select all, type the new name, Enter.
fn rename_through_field(w: &mut Window, to: &str) {
    w.get_by_label("Rename").click();
    w.run_steps(3);
    w.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    w.run_steps(2);
    testing::text(w, to);
    testing::key(w, Key::Enter);
}

fn names(w: &Window) -> Vec<String> {
    w.state().session.mailbox.signatures.iter().map(|s| s.name.clone()).collect()
}

/// #1156: after Rename, typing a new name and pressing Enter, the open signature and the
/// list show the new name, and `signature.rename` has renamed it in the mailbox (with the
/// account defaults following it). Reopening the page or saving/reopening the mailbox keeps
/// the new name.
#[test]
fn rename_with_enter_keeps_the_new_name() {
    let mut w = signatures();
    rename_through_field(&mut w, "Renamed");
    assert_eq!(names(&w), vec!["Renamed".to_string()]);
    assert!(w.state().session.mailbox.signature("Standard").is_none());
    assert_eq!(w.state().session.signature_edit.as_ref().map(|e| e.signature.name.as_str()), Some("Renamed"));
    // The account defaults followed the rename.
    let account = w.state().session.mailbox.default_account().unwrap().clone();
    assert_eq!(account.signature.as_str(), "Renamed");
    assert_eq!(account.reply_signature(), "Renamed");
    // Reopening the page keeps the new name.
    w.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    w.run_steps(2);
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.run_steps(3);
    assert_eq!(names(&w), vec!["Renamed".to_string()]);
    // Saving and reopening the mailbox keeps it too.
    let json = serde_json::to_string(&w.state().session.mailbox).unwrap();
    let back: mailcraft_model::Mailbox = serde_json::from_str(&json).unwrap();
    assert_eq!(back.signatures.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Renamed"]);
}

/// #1156: pressing the Rename button a second time, while the field holds the new name,
/// keeps what was typed instead of resetting the field to the old name.
#[test]
fn rename_button_commits_the_typed_name() {
    let mut w = signatures();
    w.get_by_label("Rename").click();
    w.run_steps(3);
    w.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    w.run_steps(2);
    testing::text(&mut w, "Renamed");
    w.get_by_label("Rename").click();
    w.run_steps(3);
    assert_eq!(names(&w), vec!["Renamed".to_string()], "the second Rename discarded what was typed");
    assert!(w.state().session.mailbox.signature("Standard").is_none());
    assert_eq!(w.state().session.signature_edit.as_ref().map(|e| e.signature.name.as_str()), Some("Renamed"));
}
