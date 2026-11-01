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
    // (Made current as a command does it: the reading pane shows it from now on, #286.)
    crate::windows::compose_window::settle_pane(h.state_mut());
    h.run_steps(3);
    assert_eq!(editor::id(h.state()), editor::id_of(first));
    assert!(!h.ctx.memory(|m| m.has_focus(editor::id_of(first))));
}

#[test]
fn the_keyboard_asked_for_one_message_is_not_taken_by_another() {
    let (mut h, first, _) = two();
    h.state_mut().want_body_focus();
    assert!(h.state_mut().session.show_compose(first));
    // (Made current as a command does it: the reading pane shows it from now on, #286.)
    crate::windows::compose_window::settle_pane(h.state_mut());
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

/// Add an account of `kind` (`imap`, `local`, `microsoft`): its id.
fn account(a: &mut crate::MailApp, email: &str, kind: &str) -> u64 {
    let mut p = json!({"email": email, "kind": kind});
    if kind == "imap" {
        p["incoming"] = json!({"host": "imap.mail.example"});
    }
    a.execute("account.add", p).unwrap()["id"].as_u64().unwrap()
}

/// The background saves (#286), as a frame does them, `secs` after the clock's `start`.
fn tick_at(a: &mut crate::MailApp, start: i64, secs: i64) {
    a.session.clock.now = Some(start + secs);
    a.draft_autosave_tick(&egui::Context::default());
}

#[test]
fn every_message_with_changes_is_saved_by_itself_every_five_minutes_for_every_kind_of_account() {
    let mut a = app();
    let start = a.session.now();
    let imap = account(&mut a, "one@mail.example", "imap");
    let local = account(&mut a, "two@mail.example", "local");
    let microsoft = account(&mut a, "three@mail.example", "microsoft");
    let on_imap = a.execute("mail.new", json!({"account": imap, "subject": "On IMAP"})).unwrap()["compose"].as_u64().unwrap();
    a.execute("compose.set", json!({"body": "One"})).unwrap();
    let here = a.execute("mail.new", json!({"account": local, "subject": "Kept here"})).unwrap()["compose"].as_u64().unwrap();
    a.execute("compose.set", json!({"body": "Two"})).unwrap();
    // The third, the current one, has nothing in it but what it started with.
    let clean = a.execute("mail.new", json!({"account": microsoft})).unwrap()["compose"].as_u64().unwrap();
    // (Opening the next saved the one before: written in again, it has changes again.)
    a.execute("compose.set", json!({"compose": on_imap, "body": "One, again"})).unwrap();
    a.execute("compose.set", json!({"compose": here, "body": "Two, again"})).unwrap();
    let c = |a: &crate::MailApp, id: u64| a.session.composing_by_id(id).cloned().unwrap();
    let saved_as = |a: &crate::MailApp, id: u64| c(a, id).draft.and_then(|d| a.session.mailbox.message(d).cloned());
    let before = (saved_as(&a, on_imap).unwrap().body, saved_as(&a, here).unwrap().body);
    assert_eq!((c(&a, on_imap).dirty, c(&a, here).dirty, c(&a, clean).dirty), (true, true, false));
    // Not before five minutes are up.
    tick_at(&mut a, start, 0);
    tick_at(&mut a, start, 299);
    assert!(c(&a, on_imap).dirty && c(&a, here).dirty);
    assert_eq!((saved_as(&a, on_imap).unwrap().body, saved_as(&a, here).unwrap().body), before);
    // Then both are saved to their account's Drafts, as Save Draft saves them, the one put aside
    // longest too; the one without changes isn't touched.
    tick_at(&mut a, start, 300);
    assert_eq!(a.session.compose_id(), Some(clean), "the current one stays current");
    for (id, account, body) in [(on_imap, imap, "One, again"), (here, local, "Two, again")] {
        assert!(!c(&a, id).dirty);
        let m = saved_as(&a, id).unwrap();
        assert!(m.draft && m.body.text.contains(body), "{}", m.body.text);
        assert_eq!(Some(m.folder), a.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Drafts));
    }
    assert_eq!((c(&a, clean).draft, c(&a, clean).dirty), (None, false));
    assert!(!a.session.mailbox.messages.values().any(|m| m.draft && m.account == microsoft));
}

#[test]
fn a_microsoft_accounts_drafts_saved_by_themselves_wait_to_go_up() {
    let mut a = app();
    let start = a.session.now();
    let account = account(&mut a, "one@example.org", "microsoft");
    let first = a.execute("mail.new", json!({"account": account, "subject": "First"})).unwrap()["compose"].as_u64().unwrap();
    a.execute("mail.new", json!({"account": account, "subject": "Second"})).unwrap();
    a.execute("compose.set", json!({"body": "Two"})).unwrap();
    a.execute("compose.set", json!({"compose": first, "body": "One"})).unwrap();
    assert!(a.session.all_composing().all(|c| c.dirty));
    tick_at(&mut a, start, 0);
    tick_at(&mut a, start, 5 * 60);
    assert!(a.session.all_composing().all(|c| !c.dirty && c.draft.is_some()));
    // Each waits for the next Send/Receive to take it up to the server (R3.4.1).
    for c in a.session.all_composing() {
        let m = c.draft.and_then(|d| a.session.mailbox.message(d)).unwrap();
        assert!(m.server_draft.as_ref().is_some_and(|sd| sd.pending), "{}", m.subject);
    }
    assert!(mailcraft_engine::cmd::account::unpushed(&a.session, account) >= 2);
    // With nothing left to save the five minutes stop; the next change starts them again.
    tick_at(&mut a, start, 6 * 60);
    a.execute("compose.set", json!({"body": "Two, again"})).unwrap();
    tick_at(&mut a, start, 7 * 60);
    tick_at(&mut a, start, 11 * 60);
    assert!(a.session.compose.as_ref().unwrap().dirty);
    tick_at(&mut a, start, 12 * 60);
    assert!(!a.session.compose.as_ref().unwrap().dirty);
}
