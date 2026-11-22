// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Signatures › Rename: the new name is kept (#1156), however the field is left (#1651).

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

/// Rename, select all and type `to`, leaving the field open (nothing pressed to finish).
fn type_new_name(w: &mut Window, to: &str) {
    w.get_by_label("Rename").click();
    w.run_steps(3);
    w.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    w.run_steps(2);
    testing::text(w, to);
}

/// #1651: a signature made with New, renamed, and the name finished with Enter.
#[test]
fn a_new_signature_renamed_with_enter_keeps_the_name() {
    let mut w = signatures();
    w.get_by_label("New").click();
    w.run_steps(3);
    assert_eq!(names(&w), vec!["Standard".to_string(), "Signature".to_string()]);
    rename_through_field(&mut w, "Work");
    assert_eq!(names(&w), vec!["Standard".to_string(), "Work".to_string()]);
}

/// #1651: leaving the field with a click elsewhere on the page keeps the typed name.
#[test]
fn a_click_elsewhere_keeps_the_typed_name() {
    let mut w = signatures();
    type_new_name(&mut w, "Renamed");
    let at = w.get_by_label("Choose default signatures").rect().center();
    testing::click(&mut w, at, egui::PointerButton::Primary, 1);
    w.run_steps(3);
    assert_eq!(names(&w), vec!["Renamed".to_string()]);
}

/// #1651: leaving the field with Tab keeps the typed name.
#[test]
fn tab_keeps_the_typed_name() {
    let mut w = signatures();
    type_new_name(&mut w, "Renamed");
    testing::key(&mut w, Key::Tab);
    w.run_steps(3);
    assert_eq!(names(&w), vec!["Renamed".to_string()]);
}

/// #1651: choosing another signature while the field is open keeps the typed name.
#[test]
fn choosing_another_signature_keeps_the_typed_name() {
    let mut w = signatures();
    w.state_mut().execute("signature.set", json!({"name": "Other", "text": "Avery"})).unwrap();
    w.run_steps(3);
    type_new_name(&mut w, "Renamed");
    w.get_by_label("Other").click();
    w.run_steps(3);
    assert_eq!(names(&w), vec!["Renamed".to_string(), "Other".to_string()]);
}

/// #1651: Save, with a change in the signature, keeps the typed name too.
#[test]
fn save_keeps_the_typed_name() {
    let mut w = signatures();
    w.state_mut().execute("signature.html", json!({"html": "<p>Avery</p>"})).unwrap();
    w.run_steps(3);
    type_new_name(&mut w, "Renamed");
    w.get_by_label("Save").click();
    w.run_steps(3);
    assert_eq!(names(&w), vec!["Renamed".to_string()]);
}

/// #1651: leaving File with Back while the field is open keeps the typed name.
#[test]
fn back_keeps_the_typed_name() {
    let mut w = signatures();
    type_new_name(&mut w, "Renamed");
    let top = if w.state().integrated_titlebar { crate::chrome::MAC_TITLE_BAR } else { 0.0 };
    testing::click(&mut w, egui::pos2(100.0, top + 30.0), egui::PointerButton::Primary, 1);
    w.run_steps(3);
    assert!(!w.state().ui.backstage, "Back was not clicked");
    assert_eq!(names(&w), vec!["Renamed".to_string()]);
}

/// #1651: Delete while the field is open deletes the signature, though leaving the field
/// renamed it first.
#[test]
fn delete_while_renaming_deletes_it() {
    let mut w = signatures();
    type_new_name(&mut w, "Renamed");
    w.get_by_label("Delete").click();
    w.run_steps(3);
    assert!(names(&w).is_empty(), "{:?}", names(&w));
}

/// #1651: Escape lets go of the field and keeps the old name.
#[test]
fn escape_keeps_the_old_name() {
    let mut w = signatures();
    type_new_name(&mut w, "Renamed");
    testing::key(&mut w, Key::Escape);
    w.run_steps(3);
    assert_eq!(names(&w), vec!["Standard".to_string()]);
    assert!(w.state().ui.backstage, "Escape in the field closed File");
    assert!(w.ctx.memory(|m| m.focused()).is_none(), "the field kept the keyboard");
    assert!(w.state().signature_rename.is_none(), "the field is still open");
}
