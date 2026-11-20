// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! React (#1544): the button beside the reply buttons and the row of six emoji it opens.

use egui::{PointerButton, Rect};
use egui_kittest::kittest::Queryable;
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::people_view::testing::{self, Window, click};

/// The first message of an Inbox in the sample mailbox that isn't from the owner: the one the
/// reading pane shows.
fn target(h: &Window) -> Id {
    let mb = &h.state().session.mailbox;
    let from_owner = |m: &mailcraft_model::Message| mb.accounts.iter().any(|a| a.email.eq_ignore_ascii_case(&m.from.email));
    mb.messages
        .values()
        .filter(|m| !m.draft && !from_owner(m) && mb.folder(m.folder).is_some_and(|f| f.kind == FolderKind::Inbox))
        .map(|m| m.id)
        .min()
        .expect("a received message in the sample")
}

fn window() -> Window {
    let mut h = testing::window("view.mail");
    let a = target(&h);
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.session.mailbox.message_mut(a).unwrap().message_id = "sample-1544@example.org".into();
    app.run("mail.select", json!({"id": a})).unwrap();
    // The tests click React on this message: the sample's must not be one the engine refuses.
    let m = app.session.mailbox.message(a).cloned().unwrap();
    assert_eq!(mailcraft_engine::cmd::react::refusal(&mut app.session, &m), None, "the sample message can be reacted to");
    h.run_steps(5);
    h
}

fn button(h: &Window) -> Rect {
    h.state().hit.rect_in(0, "react", target(h)).expect("the React button")
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
    let target_id = target(&h);
    let sent_folder = {
        let mb = &h.state().session.mailbox;
        let account = mb.message(target_id).unwrap().account;
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

/// Hover the React button of message `id` and let the tooltip show.
fn hover_button(h: &mut Window, id: Id) {
    let r = h.state().hit.rect_in(0, "react", id).expect("the button");
    h.hover_at(r.center() + egui::vec2(0.0, 200.0));
    h.run_steps(5);
    h.hover_at(r.center());
    h.run_steps(60);
}

#[test]
fn the_greyed_button_says_why_in_the_engines_words() {
    let mut h = window();
    let target_id = target(&h);
    let sent_folder = {
        let mb = &h.state().session.mailbox;
        mb.folder_of_kind(mb.message(target_id).unwrap().account, FolderKind::Sent).unwrap()
    };
    let own = h.state().session.mailbox.messages_in(sent_folder).next().map(|m| m.id).expect("a message in Sent Items");
    h.state_mut().run("mail.select", json!({"id": own})).unwrap();
    h.run_steps(5);
    hover_button(&mut h, own);
    assert!(h.query_all_by_label_contains("a reaction can't be sent to a draft or to mail you wrote").next().is_some());
    assert!(h.query_all_by_label_contains("A reaction can't be sent to this message").next().is_none(), "not the general text");
}

#[test]
fn a_message_with_a_sender_warning_has_a_greyed_button_that_says_so() {
    let mut h = window();
    let id = target(&h);
    h.state_mut().session.mailbox.message_mut(id).unwrap().from.name = "billing@bank.example".into();
    h.run_steps(5);
    hover_button(&mut h, id);
    assert!(h.query_all_by_label_contains("warning about its sender").next().is_some());
    let at = button(&h).center();
    click(&mut h, at, PointerButton::Primary, 1);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    assert!(reactions(&h).is_empty());
}

/// The row says whom the reaction goes to: the sender, whatever the Reply-To names.
#[test]
fn the_row_shows_the_sender_not_the_reply_to() {
    let mut h = window();
    let id = target(&h);
    let from = {
        let m = h.state_mut().session.mailbox.message_mut(id).unwrap();
        m.reply_to = vec![mailcraft_model::Address::new("", "x@attacker.example")];
        m.from.email.clone()
    };
    h.run_steps(5);
    let at = button(&h).center();
    click(&mut h, at, PointerButton::Primary, 1);
    assert!(egui::Popup::is_any_open(&h.ctx));
    assert!(h.state().hit.rect_in(0, "reaction-to", id).is_some(), "the recipient line");
    assert!(h.query_all_by_label_contains(&from).next().is_some(), "the sender's address is shown");
    assert!(h.query_all_by_label_contains("attacker.example").next().is_none());
    let at = option(&h, 0).center();
    click(&mut h, at, PointerButton::Primary, 1);
    let sent = reactions(&h);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to[0].email, from);
}

/// The name in the row is cut; the address stays whole.
#[test]
fn a_long_name_is_cut_in_the_row_and_the_address_kept() {
    let long = "N".repeat(500);
    let a = mailcraft_model::Address::new(&long, "bo@example.org");
    let row = super::react::shown(&a);
    assert!(row.chars().count() < 80, "{}", row.len());
    assert!(row.ends_with("<bo@example.org>") && row.contains('\u{2026}'));
    assert_eq!(super::react::shown(&mailcraft_model::Address::new("", "bo@example.org")), "bo@example.org");
    assert_eq!(super::react::shown(&mailcraft_model::Address::new("Bo", "bo@example.org")), "Bo <bo@example.org>");
}
