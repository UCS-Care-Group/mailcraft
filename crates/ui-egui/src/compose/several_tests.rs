// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Several messages being written (#286), as the window has them: each its own body (its id, its
//! caret and the keyboard), its own Bcc line; `ui.inspect` lists them; and quitting asks once
//! about every one with changes, Save Draft saving each.

use serde_json::json;

use super::editor;
use super::tests::{Window, app, plain, type_in, window};

/// Two messages being written, the owner's text in each, both with changes not saved (the first
/// was saved when the second opened, and written in since): their ids, the current one last.
fn two() -> (Window, u64, u64) {
    let mut a = app();
    let first = a.execute("mail.new", json!({"to": "maya@lumen.example", "subject": "First"})).unwrap()["compose"].as_u64().unwrap();
    a.execute("compose.set", json!({"body": "One"})).unwrap();
    let second = a.execute("mail.new", json!({"to": "maya@lumen.example", "subject": "Second"})).unwrap()["compose"].as_u64().unwrap();
    a.execute("compose.set", json!({"body": "Two"})).unwrap();
    a.execute("compose.set", json!({"compose": first, "body": "One, again"})).unwrap();
    (window(a), first, second)
}

#[test]
fn each_message_has_a_body_of_its_own() {
    let (mut h, first, second) = two();
    assert_ne!(editor::id_of(first), editor::id_of(second));
    assert_eq!(editor::id(h.state()), editor::id_of(second));
    // The keyboard in the current one's body types there alone.
    h.state_mut().execute("compose.select", json!({"from": 3})).unwrap();
    h.state_mut().want_body_focus();
    h.run_steps(3);
    assert!(editor::has_focus(&h.ctx));
    type_in(&mut h, " more");
    assert!(plain(&h).starts_with("Two more"), "{}", plain(&h));
    let other = h.state().session.composing_by_id(first).unwrap().body.plain();
    assert!(other.starts_with("One") && !other.contains("more"), "{other}");
    // The other one current, its body is another: the keyboard isn't carried over to it.
    assert!(h.state_mut().session.show_compose(first));
    h.run_steps(3);
    assert_eq!(editor::id(h.state()), editor::id_of(first));
    assert!(!h.ctx.memory(|m| m.has_focus(editor::id_of(first))));
}

#[test]
fn the_keyboard_asked_for_one_message_is_not_taken_by_another() {
    let (mut h, first, _) = two();
    h.state_mut().want_body_focus();
    assert!(h.state_mut().session.show_compose(first));
    h.run_steps(3);
    assert!(!editor::has_focus(&h.ctx), "asked for the other message's body");
}

#[test]
fn the_bcc_line_is_shown_for_the_message_it_was_asked_for() {
    let (mut h, first, second) = two();
    assert!(!h.state().bcc_shown());
    h.state_mut().compose_bcc.insert(second);
    assert!(h.state().bcc_shown());
    assert!(h.state_mut().session.show_compose(first));
    assert!(!h.state().bcc_shown(), "not the other's");
    // Closed, what was kept of it goes.
    h.state_mut().run("compose.close", json!({"compose": second, "discard": true})).unwrap();
    assert!(!h.state().compose_bcc.contains(&second));
}

#[test]
fn ui_inspect_lists_every_message_being_written() {
    let (mut h, first, second) = two();
    let ctx = h.ctx.clone();
    let said = crate::control::inspect(h.state_mut(), &ctx);
    let open = said["beingWritten"].as_array().unwrap();
    assert_eq!(open.len(), 2);
    assert_eq!((open[0]["compose"].as_u64(), open[0]["current"].as_bool()), (Some(second), Some(true)));
    assert_eq!((open[1]["compose"].as_u64(), open[1]["current"].as_bool()), (Some(first), Some(false)));
    assert_eq!(open[1]["subject"], "First");
    assert_eq!(said["composing"], true);
}

#[test]
fn quitting_saves_them_all_to_drafts_unasked() {
    // Closing the window (#286, slice 3) asks nothing: every message being written goes to Drafts.
    let draft = |h: &Window, subject: &str| h.state().session.mailbox.messages.values().any(|m| m.draft && m.subject == subject);
    let (mut h, _, _) = two();
    assert!(h.state_mut().close_requested());
    assert!(h.state().dialog.is_none());
    assert_eq!((draft(&h, "First"), draft(&h, "Second")), (true, true));
}

#[test]
fn every_message_with_changes_is_saved_by_itself_the_ones_put_aside_too() {
    let mut a = app();
    let account = a.execute("account.add", json!({"email": "one@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let first = a.execute("mail.new", json!({"account": account, "subject": "First"})).unwrap()["compose"].as_u64().unwrap();
    a.execute("mail.new", json!({"account": account, "subject": "Second"})).unwrap();
    a.execute("compose.set", json!({"body": "Two"})).unwrap();
    a.execute("compose.set", json!({"compose": first, "body": "One"})).unwrap();
    assert!(a.session.all_composing().all(|c| c.dirty));
    // Five minutes on, both are saved, not the current one alone.
    a.draft_saved_at = 1.0;
    a.draft_autosave_tick(&egui::Context::default());
    assert!(a.session.all_composing().all(|c| !c.dirty && c.draft.is_some()));
}
