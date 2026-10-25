// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Escape in the search box while a message is being written (#365): the box's, not the
//! message's; and a dialog's when one is over the window.

use egui::Key;
use serde_json::json;

use crate::people_view::testing::{self, Window};

/// The sample mailbox with a message being written, and the keyboard in the search box.
fn window() -> Window {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.execute("mail.new", json!({"subject": "Half written"})).unwrap();
    app.execute("ui.focusSearch", json!({})).unwrap();
    h.run_steps(4);
    h
}

fn search(h: &Window) -> String {
    h.state().session.view.search.clone()
}

fn writing(h: &Window) -> bool {
    h.state().session.compose.as_ref().is_some_and(|c| c.subject == "Half written")
}

fn refused(h: &mut Window) {
    h.state_mut().dialog = Some(crate::dialogs::Dialog::Refused { text: "Not now".into() });
    h.run_steps(3);
}

/// Escape in the search box clears the search and leaves the message being written open; the
/// box has given up the keyboard, so the next Escape is the message's again and closes it.
#[test]
fn escape_in_the_search_box_leaves_the_message_open() {
    let mut h = window();
    testing::text(&mut h, "budget");
    assert_eq!(search(&h), "budget");
    testing::key(&mut h, Key::Escape);
    assert_eq!(search(&h), "");
    assert!(writing(&h), "the message is still open");
    testing::key(&mut h, Key::Escape);
    assert!(h.state().session.compose.is_none());
}

/// With a dialog over the window, Escape is the dialog's: it closes the dialog, and the search
/// and the message stay as they were.
#[test]
fn escape_for_a_dialog_leaves_the_search() {
    let mut h = window();
    testing::text(&mut h, "budget");
    refused(&mut h);
    testing::key(&mut h, Key::Escape);
    assert!(h.state().dialog.is_none());
    assert_eq!(search(&h), "budget");
    assert!(writing(&h));
}

/// A held Escape is one press. Over a dialog, its repeats leave the search in the box; in the
/// box, they don't close the message once the box has cleared the search; nor over a dialog when
/// the box doesn't have the keyboard.
#[test]
fn a_held_escape_is_one_press() {
    let mut h = window();
    testing::text(&mut h, "budget");
    refused(&mut h);
    testing::held(&mut h, Key::Escape);
    assert!(h.state().dialog.is_none());
    assert_eq!(search(&h), "budget", "the dialog had it");
    assert!(writing(&h));
    testing::held(&mut h, Key::Escape);
    assert_eq!(search(&h), "");
    assert!(writing(&h), "the search box had it");
    refused(&mut h);
    testing::held(&mut h, Key::Escape);
    assert!(h.state().dialog.is_none());
    assert!(writing(&h), "the dialog had it");
}
