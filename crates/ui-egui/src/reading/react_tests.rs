// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! React (#1544): the button beside the reply buttons and the row of six emoji it opens.

use egui::{PointerButton, Rect};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::people_view::testing::{self, Window, click};

/// A message of the sample mailbox, in the reading pane.
const A: Id = 13;

fn window() -> Window {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.session.mailbox.message_mut(A).unwrap().message_id = "sample-1544@example.org".into();
    app.run("mail.select", json!({"id": A})).unwrap();
    h.run_steps(5);
    h
}

fn button(h: &Window) -> Rect {
    h.state().hit.rect_in(0, "react", A).expect("the React button")
}

fn option(h: &Window, index: u64) -> Rect {
    h.state().hit.rect_in(0, "reaction", index).unwrap_or_else(|| panic!("no emoji {index}"))
}

/// The reactions in the mailbox: its sent mail with `reaction` set.
fn reactions(h: &Window) -> Vec<mailcraft_model::Message> {
    h.state().session.mailbox.messages.values().filter(|m| m.reaction).map(|m| m.as_ref().clone()).collect()
}

#[test]
fn the_button_opens_six_emoji_and_one_click_sends_it() {
    let mut h = window();
    assert!(!egui::Popup::is_any_open(&h.ctx));
    let at = button(&h).center();
    click(&mut h, at, PointerButton::Primary, 1);
    assert!(egui::Popup::is_any_open(&h.ctx));
    for index in 0..6 {
        option(&h, index);
    }
    assert!(h.state().hit.rect_in(0, "reaction", 6).is_none(), "six, no more");
    // The heart is the second.
    let at = option(&h, 1).center();
    click(&mut h, at, PointerButton::Primary, 1);
    let sent = reactions(&h);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].body.text, "\u{2764}\u{FE0F}");
    assert_eq!(sent[0].in_reply_to, "sample-1544@example.org");
    let sent_items = h.state().session.mailbox.folder_of_kind(sent[0].account, FolderKind::Sent).unwrap();
    assert_eq!(sent[0].folder, sent_items, "filed in Sent Items like any mail");
    assert!(!egui::Popup::is_any_open(&h.ctx), "the row closed");
}

#[test]
fn mail_the_owner_wrote_has_a_greyed_button_that_opens_nothing() {
    let mut h = window();
    let sent_folder = {
        let mb = &h.state().session.mailbox;
        let account = mb.message(A).unwrap().account;
        mb.folder_of_kind(account, FolderKind::Sent).unwrap()
    };
    let own = h.state().session.mailbox.messages_in(sent_folder).next().map(|m| m.id).expect("a message in Sent Items");
    h.state_mut().run("mail.select", json!({"id": own})).unwrap();
    h.run_steps(5);
    let r = h.state().hit.rect_in(0, "react", own).expect("the button is there, greyed");
    click(&mut h, r.center(), PointerButton::Primary, 1);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    assert!(h.state().hit.rect_in(0, "reaction", 0).is_none());
    assert!(reactions(&h).is_empty());
}
