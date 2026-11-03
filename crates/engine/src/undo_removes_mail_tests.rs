// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent's undo or redo that takes a message away asks for Delete, as `mail.delete` does for
//! the messages it deletes for good (#473).

use std::sync::Arc;

use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::Session;
use crate::agent::*;

fn sample() -> Session {
    Session::new(crate::sample::mailbox(1_791_993_600, 0))
}

/// A message and a folder other than the one it is in, in its account.
fn message_and_other_folder(s: &Session) -> (Id, Id) {
    let m = s.mailbox.messages.values().next().unwrap();
    let other = s.mailbox.folders.iter().find(|f| f.account == m.account && f.id != m.folder && f.kind != FolderKind::Deleted).unwrap().id;
    (m.id, other)
}

fn asks_delete(s: &Session, id: &str) -> bool {
    matches!(check(s, id, &json!({}), &[]), Verdict::Ask(Class::Delete, _))
}

fn eml(subject: &str) -> String {
    format!("From: Bo <bo@example.org>\nTo: ada@example.org\nSubject: {subject}\nMessage-ID: <{subject}@example.org>\n\nHello\n")
}

/// A message that came down after the step, which the undo leaves alone.
fn arrive(s: &mut Session) -> Id {
    let mut late = (**s.mailbox.messages.values().next().unwrap()).clone();
    late.id = s.mailbox.messages.keys().max().unwrap() + 1_000;
    late.subject = "Later".into();
    let id = late.id;
    s.mailbox.messages.insert(id, Arc::new(late));
    id
}

#[test]
fn undo_of_a_copy_asks_for_delete() {
    let mut s = sample();
    let (m, other) = message_and_other_folder(&s);
    s.run("mail.copy", &json!({"ids": [m], "folder": other})).unwrap();
    assert!(asks_delete(&s, "edit.undo"));
    assert_eq!(check(&s, "edit.undo", &json!({}), &[Class::Delete]), Verdict::Run);
    assert!(describe(&s, "edit.undo", &json!({})).contains("messages"));
}

#[test]
fn undo_of_an_import_asks_for_delete_one_message_or_more() {
    let mut s = sample();
    s.run("file.import", &json!({"name": "in.eml", "data": crate::cmd::base64_encode(eml("Single").as_bytes())})).unwrap();
    assert!(asks_delete(&s, "edit.undo"));
    let mut s = sample();
    let mbox: String = ["One", "Two", "Three"].iter().map(|t| format!("From bo@example.org Mon Jan  1 00:00:00 2026\n{}\n", eml(t))).collect();
    let before = s.mailbox.messages.len();
    s.run("file.import", &json!({"name": "in.mbox", "data": crate::cmd::base64_encode(mbox.as_bytes())})).unwrap();
    assert!(s.mailbox.messages.len() >= before + 2);
    assert!(asks_delete(&s, "edit.undo"));
}

#[test]
fn redo_of_a_delete_for_good_asks_for_delete() {
    let mut s = sample();
    let (m, _) = message_and_other_folder(&s);
    s.run("mail.deletePermanently", &json!({"ids": [m]})).unwrap();
    assert!(s.mailbox.message(m).is_none());
    assert!(s.undo());
    assert!(asks_delete(&s, "edit.redo"));
    assert_eq!(check(&s, "edit.redo", &json!({}), &[Class::Delete]), Verdict::Run);
}

#[test]
fn undo_of_a_flag_a_move_and_a_delete_into_deleted_items_stays_safe() {
    let mut s = sample();
    let (m, other) = message_and_other_folder(&s);
    s.run("mail.flag", &json!({"ids": [m]})).unwrap();
    assert_eq!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
    s.run("mail.move", &json!({"ids": [m], "folder": other})).unwrap();
    assert_eq!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
    s.run("mail.delete", &json!({"ids": [m]})).unwrap();
    assert!(s.mailbox.message(m).is_some());
    assert_eq!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
    assert!(s.undo());
    assert_eq!(check(&s, "edit.redo", &json!({}), &[]), Verdict::Run);
}

#[test]
fn mail_that_arrived_after_the_step_is_not_taken_away() {
    let mut s = sample();
    let (m, _) = message_and_other_folder(&s);
    s.run("mail.flag", &json!({"ids": [m]})).unwrap();
    let late = arrive(&mut s);
    assert_eq!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Run);
    assert!(s.undo());
    assert!(s.mailbox.message(late).is_some());
}

#[test]
fn a_copy_is_asked_about_beside_untouched_mail_of_another_account() {
    let mut s = sample();
    let (m, other) = message_and_other_folder(&s);
    let mut elsewhere = (**s.mailbox.messages.get(&m).unwrap()).clone();
    elsewhere.id = s.mailbox.messages.keys().max().unwrap() + 1_000;
    elsewhere.account = elsewhere.account.wrapping_add(77);
    let kept = elsewhere.id;
    s.mailbox.messages.insert(kept, Arc::new(elsewhere));
    s.run("mail.copy", &json!({"ids": [m], "folder": other})).unwrap();
    assert!(asks_delete(&s, "edit.undo"));
    assert!(s.undo());
    assert!(s.mailbox.message(kept).is_some(), "the other account's message stays");
}

#[test]
fn a_step_that_copies_one_message_and_moves_another_asks_for_delete() {
    let mut s = sample();
    let (m, other) = message_and_other_folder(&s);
    s.run("mail.copy", &json!({"ids": [m], "folder": other})).unwrap();
    // The same step also moved another message.
    let mover = *s.mailbox.messages.keys().find(|k| **k != m).unwrap();
    if let Some(step) = s.history.last_mut()
        && let Some(then) = step.mailbox.messages.get_mut(&mover)
    {
        Arc::make_mut(then).folder = other;
    }
    assert!(asks_delete(&s, "edit.undo"));
}

#[test]
fn an_unsaved_undo_that_takes_mail_asks_for_delete_every_time() {
    let mut s = sample();
    let (m, other) = message_and_other_folder(&s);
    s.run("mail.copy", &json!({"ids": [m], "folder": other})).unwrap();
    let params = json!({"unsaved": {"form": "appointment", "title": "Lunch"}});
    assert!(matches!(check(&s, "edit.undo", &params, &[]), Verdict::Ask(Class::Delete, _)));
    assert!(matches!(check(&s, "edit.undo", &params, &[Class::Delete]), Verdict::Ask(..)));
}

#[test]
fn a_step_that_takes_mail_and_reverts_the_accounts_is_described_with_the_accounts() {
    let mut s = sample();
    let (m, other) = message_and_other_folder(&s);
    s.run("mail.copy", &json!({"ids": [m], "folder": other})).unwrap();
    assert!(describe(&s, "edit.undo", &json!({})).contains("messages"));
    if let Some(step) = s.history.last_mut() {
        step.mailbox.accounts.pop();
    }
    assert!(s.undo_takes_mail(false));
    assert!(matches!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Ask(Class::Accounts, _)));
    let d = describe(&s, "edit.undo", &json!({}));
    assert!(d.contains("accounts"), "{d}");
}

#[test]
fn an_agent_without_the_grant_cannot_undo_a_copy() {
    let mut s = sample();
    let (m, other) = message_and_other_folder(&s);
    s.run("mail.copy", &json!({"ids": [m], "folder": other})).unwrap();
    let (before, undo, redo) = (s.mailbox.clone(), s.undo_label().map(str::to_string), s.redo_label().map(str::to_string));
    s.agent = Some(Vec::new());
    assert!(s.run("edit.undo", &json!({})).is_err());
    s.agent = None;
    assert!(s.mailbox == before);
    assert_eq!((s.undo_label().map(str::to_string), s.redo_label().map(str::to_string)), (undo, redo));
}

#[test]
fn the_gate_agrees_with_what_the_undo_drops() {
    type Step = fn(&mut Session, Id, Id);
    let steps: [(&str, Step); 6] = [
        ("flag", |s, m, _| drop(s.run("mail.flag", &json!({"ids": [m]})).unwrap())),
        ("move", |s, m, o| drop(s.run("mail.move", &json!({"ids": [m], "folder": o})).unwrap())),
        ("delete", |s, m, _| drop(s.run("mail.delete", &json!({"ids": [m]})).unwrap())),
        ("copy", |s, m, o| drop(s.run("mail.copy", &json!({"ids": [m], "folder": o})).unwrap())),
        ("deletePermanently", |s, m, _| drop(s.run("mail.deletePermanently", &json!({"ids": [m]})).unwrap())),
        ("arrival", |s, m, _| {
            s.run("mail.flag", &json!({"ids": [m]})).unwrap();
            arrive(s);
        }),
    ];
    for (name, step) in steps {
        let mut s = sample();
        let (m, o) = message_and_other_folder(&s);
        step(&mut s, m, o);
        for redo in [false, true] {
            if redo && !s.undo() {
                continue;
            }
            let says = s.undo_takes_mail(redo);
            let before: Vec<Id> = s.mailbox.messages.keys().copied().collect();
            assert!(if redo { s.redo() } else { s.undo() }, "{name}");
            let dropped = before.iter().any(|k| !s.mailbox.messages.contains_key(k));
            assert_eq!(says, dropped, "{name} redo={redo}");
            if redo {
                s.undo();
            }
        }
    }
}
