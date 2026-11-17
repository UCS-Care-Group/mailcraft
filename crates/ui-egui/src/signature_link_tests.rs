// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Signatures › Link: the signature editor gets the composer's own link dialog (#1552).

use egui::{Key, Modifiers, PointerButton, vec2};
use egui_kittest::kittest::Queryable as _;
use serde_json::json;

use crate::people_view::testing::{self, Window};

/// The window on the sample mailbox with a `Standard` signature open on File › Signatures.
fn signatures() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().execute("signature.set", json!({"name": "Standard", "text": "Avery Quinn"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.run_steps(3);
    w
}

/// The signature's body gets the keyboard and "Avery" is selected.
fn select_avery(w: &mut Window) {
    let at = w.state().hit.rect("signatureBody", 0).expect("the signature body").min + vec2(1.0, 8.0);
    testing::click(w, at, PointerButton::Primary, 1);
    w.state_mut().execute("compose.select", json!({"from": 0, "to": 5})).unwrap();
    w.run_steps(2);
}

/// Where the text at `at` leads, if it leads anywhere.
fn link_at(w: &Window, at: usize) -> Option<String> {
    w.state().session.signature_edit.as_ref().and_then(|e| e.body.text().style_at(at).link.clone())
}

/// #1552: the Link button of the signature editor opens the same dialog as the composer's, and
/// what is typed in it goes onto the signature's selection.
#[test]
fn link_button_opens_the_dialog_and_sets_the_link() {
    let mut w = signatures();
    select_avery(&mut w);
    w.get_by_label("Link").click();
    w.run_steps(2);
    assert_eq!(testing::dialog(&w), Some("link"), "the Link button did nothing");
    testing::text(&mut w, "www.example.org");
    testing::key(&mut w, Key::Enter);
    assert_eq!(testing::dialog(&w), None, "the dialog is still open");
    assert_eq!(link_at(&w, 1).as_deref(), Some("https://www.example.org"));
    // What follows the link is not linked.
    assert_eq!(link_at(&w, 7), None);
    // The keyboard goes back to the signature's text, as it does for a message.
    assert!(crate::compose::editor::has_focus(&w.ctx), "the keyboard is not back in the signature");
    // The signature is changed, and the link is kept when it is saved.
    assert!(w.state().session.signature_edit.as_ref().is_some_and(|e| e.dirty));
    w.state_mut().run("signature.save", json!({})).unwrap();
    let saved = w.state().session.mailbox.signature("Standard").expect("the signature");
    assert_eq!(saved.body().style_at(1).link.as_deref(), Some("https://www.example.org"), "the saved signature lost the link");
}

/// #1552: with the caret in a link, the dialog says where it leads and Remove Link takes it back.
#[test]
fn the_dialog_shows_the_link_the_caret_is_in_and_removes_it() {
    let mut w = signatures();
    select_avery(&mut w);
    w.state_mut().run("compose.link", json!({"url": "https://www.example.org"})).unwrap();
    w.run_steps(2);
    // The caret sits in the link now.
    w.state_mut().execute("compose.select", json!({"from": 2})).unwrap();
    w.run_steps(2);
    w.get_by_label("Link").click();
    w.run_steps(2);
    assert_eq!(testing::dialog(&w), Some("link"));
    let shown = match w.state().dialog.as_ref() {
        Some(crate::dialogs::Dialog::Link { url, linked, .. }) => {
            assert!(*linked, "the dialog doesn't know the caret is in a link");
            url.clone()
        }
        other => panic!("not the link dialog: {}", other.map_or("none", |d| d.name())),
    };
    assert_eq!(shown, "https://www.example.org");
    w.get_by_label("Remove Link").click();
    w.run_steps(2);
    assert_eq!(testing::dialog(&w), None);
    assert_eq!(link_at(&w, 1), None, "the link is still there");
}

/// #1552: Ctrl+K in the signature's text opens the dialog too, as it does in a message.
#[test]
fn ctrl_k_in_the_signature_text_opens_the_dialog() {
    let mut w = signatures();
    select_avery(&mut w);
    w.key_press_modifiers(Modifiers::COMMAND, Key::K);
    w.run_steps(2);
    assert_eq!(testing::dialog(&w), Some("link"));
}

/// A plain-text message has no links: its Link button stays disabled and opens nothing. The
/// signature editor is always HTML, so it always gets the dialog.
#[test]
fn a_plain_message_still_gets_no_dialog() {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().execute("mail.new", json!({"to": "maya@lumen.example"})).unwrap();
    w.state_mut().execute("compose.set", json!({"body": "Hello"})).unwrap();
    w.state_mut().execute("compose.formatPlain", json!({})).unwrap();
    w.run_steps(2);
    assert!(!w.state().enabled("compose.link"));
    let _ = w.state_mut().run("compose.link", json!({}));
    assert_eq!(testing::dialog(&w), None);
}
