// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashSet;

use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::json;

use super::sending::*;
use crate::Session;

fn session() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    (s, account, outbox)
}

fn queued(s: &mut Session, account: Id, outbox: Id, subject: &str) -> Id {
    let m = Message {
        account,
        folder: outbox,
        subject: subject.into(),
        to: vec![Address::new("", "kit@example.net")],
        message_id: format!("{}@example.org", subject.to_lowercase()),
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

fn marked(s: &Session, id: Id) -> bool {
    s.mailbox.message(id).is_some_and(|m| m.sending)
}

#[test]
fn what_goes_is_marked_first_and_a_new_mark_asks_for_a_save() {
    let (mut s, account, outbox) = session();
    let a = queued(&mut s, account, outbox, "One");
    let before = s.rev();
    let rev = s.send_marking(account, &[a]).expect("a mark is new");
    assert!(marked(&s, a) && rev > before && rev == s.rev());
    assert_eq!(s.send_marking(account, &[a]), None, "already marked: nothing to save");
    assert_eq!(s.send_marking(account, &[]), None);
}

#[test]
fn the_gate_opens_when_the_marks_are_on_disk_and_not_before() {
    let (mut s, account, outbox) = session();
    let a = queued(&mut s, account, outbox, "One");
    assert_eq!(s.send_gate_step(0, false), GateStep::Idle);
    let rev = s.send_marking(account, &[a]).unwrap();
    s.send_gate_wait(account, rev);
    assert!(s.driver.sending.gate_waits());
    assert_eq!(s.send_gate_step(rev - 1, false), GateStep::Waiting);
    assert_eq!(s.send_gate_step(rev, false), GateStep::Open(account));
    assert_eq!(s.send_gate_step(rev, false), GateStep::Idle, "once");
    assert!(marked(&s, a), "the mark stays until the run's answer is taken in");
    // The run was called off while it waited: nothing went, so nothing is in doubt.
    s.send_gate_went(account, true);
    assert!(marked(&s, a));
    s.send_gate_went(account, false);
    assert!(!marked(&s, a));
}

#[test]
fn a_save_that_fails_sends_nothing_and_takes_the_marks_off() {
    let (mut s, account, outbox) = session();
    let a = queued(&mut s, account, outbox, "One");
    let rev = s.send_marking(account, &[a]).unwrap();
    s.send_gate_wait(account, rev);
    assert_eq!(s.send_gate_step(0, true), GateStep::Failed(account));
    assert!(!marked(&s, a) && !s.driver.sending.gate_waits());
}

#[test]
fn a_gate_of_another_mailbox_lets_go_and_marks_nothing() {
    let (mut s, account, outbox) = session();
    let a = queued(&mut s, account, outbox, "One");
    let rev = s.send_marking(account, &[a]).unwrap();
    s.send_gate_wait(account, rev);
    s.replace_mailbox(mailcraft_model::Mailbox::with_local_account("", ""));
    assert_eq!(s.send_gate_step(u64::MAX, false), GateStep::Idle, "the driver forgot it with the mailbox");
    // (And a gate that outlives its opening some other way is let go of, not opened.)
    s.send_gate_wait(1, 1);
    s.driver.sending.forget_gate();
    assert_eq!(s.send_gate_step(u64::MAX, false), GateStep::Idle);
}

#[test]
fn a_refused_piece_leaves_the_marks_for_the_next_run_to_hold() {
    let (mut s, account, outbox) = session();
    let a = queued(&mut s, account, outbox, "One");
    s.send_marking(account, &[a]).unwrap();
    s.driver.sending.doubted();
    s.send_run_answered(account);
    assert!(marked(&s, a), "in doubt");
    // The next run holds it, and says so.
    let said = s.send_hold_unanswered(account);
    assert!(said.is_some_and(|line| line.contains("may have been sent")));
    assert!(!marked(&s, a) && s.mailbox.message(a).is_some_and(|m| !m.held.is_empty()));
    assert_eq!(s.send_hold_unanswered(account), None);
    // A run whose pieces were all taken in clears the marks.
    let b = queued(&mut s, account, outbox, "Two");
    s.send_marking(account, &[b]).unwrap();
    s.send_run_answered(account);
    assert!(!marked(&s, b));
    // A new run forgets what an earlier run's refused piece said.
    s.driver.sending.doubted();
    s.send_marking(account, &[]);
    s.send_marking(account, &[b]).unwrap();
    s.send_run_answered(account);
    assert!(!marked(&s, b));
}

/// A session with a shared mailbox opened through a Microsoft account, and a message in it.
fn shared() -> (Session, Id, Id) {
    let (mut s, owner, _) = session();
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    let outbox = s.mailbox.folder_of_kind(shared, FolderKind::Outbox).unwrap();
    let message = queued(&mut s, shared, outbox, "Hello");
    (s, shared, message)
}

#[test]
fn pressing_send_in_a_shared_mailbox_is_noted_or_asks_at_once() {
    let (mut s, shared, message) = shared();
    assert!(s.mailbox.account(shared).unwrap().is_shared());
    assert_eq!(s.shared_send_pressed_in(message), Pressed::Noted);
    assert_eq!(s.shared_send_pressed_in(9_999), Pressed::Nothing, "no such message");
    // The run that took it out found sending isn't allowed yet: the owner pressed, so he is asked.
    assert_eq!(s.shared_send_missing(shared), Some(true));
    assert!(s.shared_send_waits(shared));
    assert_eq!(s.shared_send_missing(shared), Some(false), "pressed once");
    assert_eq!(s.shared_send_pressed_in(message), Pressed::Ask(shared), "known now: at once");
    assert!(s.driver.sending.allowed(shared) && !s.shared_send_waits(shared));
    assert_eq!(s.shared_send_missing(Id::MAX), None, "no such account");
}

#[test]
fn what_was_pressed_is_forgotten_once_the_outbox_is_empty_and_for_an_own_mailbox_nothing_is_noted() {
    let (mut s, shared, message) = shared();
    s.shared_send_pressed_in(message);
    s.shared_send_none_waits(shared);
    assert_eq!(s.shared_send_missing(shared), Some(true), "still something waiting: still pressed");
    s.driver.sending.allowed(shared);
    s.mailbox.remove_message(message).unwrap();
    s.shared_send_none_waits(shared);
    assert_eq!(s.shared_send_missing(shared), Some(false));
    let own = s.mailbox.accounts.iter().find(|a| !a.is_shared()).map(|a| a.id).unwrap();
    let outbox = s.mailbox.folder_of_kind(own, FolderKind::Outbox).unwrap();
    let mine = queued(&mut s, own, outbox, "Mine");
    assert_eq!(s.shared_send_pressed_in(mine), Pressed::Nothing);
}

#[test]
fn removing_an_account_forgets_what_waited_for_it() {
    let (mut s, shared, message) = shared();
    s.shared_send_pressed_in(message);
    s.shared_send_missing(shared);
    s.shared_send_pressed_in(message);
    let rev = s.send_marking(shared, &[message]).unwrap();
    s.send_gate_wait(shared, rev);
    s.driver.sending.forget_accounts(&HashSet::from([shared]));
    assert_eq!(format!("{:?}", s.driver.sending), format!("{:?}", SendingDriver::default()));
}
