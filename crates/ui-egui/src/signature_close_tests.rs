// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Closing MailCraft with a changed signature open in File › Signatures keeps the change, as
//! leaving the page does (#1746); one that can't be kept holds the first close and says why.

use serde_json::json;

use crate::people_view::testing::{self, Window};

/// The window on the sample mailbox with `Standard` open in File › Signatures and "Dr " typed
/// before its text.
fn changed_signature() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().execute("signature.set", json!({"name": "Standard", "text": "Avery Quinn"})).unwrap();
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.run_steps(3);
    w.state_mut().execute("compose.type", json!({"text": "Dr "})).unwrap();
    w.run_steps(2);
    assert!(w.state().session.signature_edit.as_ref().is_some_and(|e| e.dirty));
    w
}

fn kept_text(w: &Window) -> String {
    w.state().session.mailbox.signature("Standard").map(|s| s.plain()).unwrap_or_default()
}

#[test]
fn closing_with_the_signatures_page_open_keeps_the_change() {
    let mut w = changed_signature();
    assert!(!kept_text(&w).contains("Dr "), "not kept before the close");
    assert!(w.state_mut().close_requested(), "nothing to ask: it closes");
    assert!(kept_text(&w).contains("Dr Avery Quinn"), "kept: {}", kept_text(&w));
    assert!(w.state().session.signature_edit.as_ref().is_some_and(|e| !e.dirty));
}

#[test]
fn a_signature_that_cant_be_kept_holds_the_first_close_and_not_the_second() {
    let mut w = changed_signature();
    // Gone from the mailbox while it is open: `signature.save` refuses it.
    w.state_mut().session.mailbox.signatures.clear();
    w.state_mut().ui.backstage = false;
    assert!(!w.state_mut().close_requested(), "the first close is held");
    assert!(w.state().ui.backstage && w.state().ui.backstage_page == "signatures", "on the page that says why");
    assert!(w.state().signature_note.as_ref().is_some_and(|n| n.error));
    assert!(w.state_mut().close_requested(), "closing again leaves it");
}
