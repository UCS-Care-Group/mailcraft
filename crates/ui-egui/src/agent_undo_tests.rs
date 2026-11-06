// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent's undo that puts the accounts back and takes messages away is asked about once, for
//! both, in the window (#473): the question names the messages, and a yes covers taking them.

use std::sync::mpsc::Receiver;

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use crate::agent_gate::answer;
use crate::control::{ControlRequest, Outcome};
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a
}

/// `command` sent by an agent the owner allowed nothing in advance: its answer, when it has one.
fn agent(a: &mut MailApp, command: &str) -> (Option<Value>, Receiver<Value>) {
    let (req, rx) = ControlRequest::new("engine.execute", json!({"command": command, "params": {}}));
    let out = crate::control::handle(a, &egui::Context::default(), &req.from_agent(Vec::new()));
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

/// A message that isn't a draft, and a folder of its account it isn't in.
fn message_and_other_folder(a: &MailApp) -> (Id, Id) {
    let mb = &a.session.mailbox;
    let m = mb.messages.values().find(|m| !m.draft && !m.subject.trim().is_empty()).unwrap();
    let other = mb.folders.iter().find(|f| f.account == m.account && f.id != m.folder && f.kind == FolderKind::Custom).unwrap().id;
    (m.id, other)
}

/// The accounts changed since the last step, by no step of their own: undoing it puts them back.
fn rename_an_account(a: &mut MailApp) {
    if let Some(x) = a.session.mailbox.accounts.first_mut() {
        x.name.push_str(" (renamed)");
    }
}

#[test]
fn a_yes_to_putting_the_accounts_back_covers_the_messages_the_question_named() {
    let mut a = app();
    let (m, other) = message_and_other_folder(&a);
    a.execute("mail.copy", json!({"ids": [m], "folder": other})).unwrap();
    rename_an_account(&mut a);
    let n = a.session.mailbox.messages.len();
    let (now, rx) = agent(&mut a, "edit.undo");
    assert!(now.is_none(), "asked first");
    let q = a.asking.front().unwrap();
    assert_eq!(q.class, Class::Accounts);
    assert!(q.what.contains("the accounts") && q.what.contains("takes away 1 message"), "{}", q.what);
    answer(&mut a, true);
    let reply = rx.try_recv().unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(a.session.mailbox.messages.len(), n - 1, "the copy is gone");
    assert!(!a.session.mailbox.accounts.iter().any(|x| x.name.ends_with(" (renamed)")));
}

#[test]
fn a_no_takes_nothing_away() {
    let mut a = app();
    let (m, other) = message_and_other_folder(&a);
    a.execute("mail.copy", json!({"ids": [m], "folder": other})).unwrap();
    rename_an_account(&mut a);
    let before = a.session.mailbox.clone();
    let (_, rx) = agent(&mut a, "edit.undo");
    answer(&mut a, false);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert!(a.session.mailbox == before);
}

#[test]
fn a_yes_doesnt_cover_other_messages_than_those_the_question_named() {
    // Asked about taking one copy away; by the answer the undo would take another.
    let mut a = app();
    let (m, other) = message_and_other_folder(&a);
    a.execute("mail.copy", json!({"ids": [m], "folder": other})).unwrap();
    rename_an_account(&mut a);
    let (_, rx) = agent(&mut a, "edit.undo");
    assert!(a.asking.front().unwrap().what.contains("takes away 1 message"));
    a.execute("mail.copy", json!({"ids": [m], "folder": other})).unwrap();
    rename_an_account(&mut a);
    let before = a.session.mailbox.clone();
    answer(&mut a, true);
    let reply = rx.try_recv().unwrap();
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(a.session.mailbox == before);
}

#[test]
fn with_the_accounts_allowed_in_advance_the_question_is_about_delete() {
    let mut a = app();
    let (m, other) = message_and_other_folder(&a);
    a.execute("mail.copy", json!({"ids": [m], "folder": other})).unwrap();
    rename_an_account(&mut a);
    let n = a.session.mailbox.messages.len();
    let (req, rx) = ControlRequest::new("engine.execute", json!({"command": "edit.undo", "params": {}}));
    let out = crate::control::handle(&mut a, &egui::Context::default(), &req.from_agent(vec![Class::Accounts]));
    assert!(matches!(out, Outcome::Pending));
    assert_eq!(a.asking.front().unwrap().class, Class::Delete);
    answer(&mut a, true);
    let reply = rx.try_recv().unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(a.session.mailbox.messages.len(), n - 1);
}

#[test]
fn a_yes_to_the_accounts_alone_doesnt_cover_messages_the_step_takes_since() {
    // Asked when the undo took no mail; by the answer it would: the yes is refused, nothing moves.
    let mut a = app();
    let (m, other) = message_and_other_folder(&a);
    a.execute("mail.flag", json!({"ids": [m]})).unwrap();
    rename_an_account(&mut a);
    let (now, rx) = agent(&mut a, "edit.undo");
    assert!(now.is_none());
    let q = a.asking.front().unwrap();
    assert_eq!(q.class, Class::Accounts);
    assert!(!q.what.contains("takes away"), "{}", q.what);
    // Meanwhile the owner copies a message, and the accounts change again.
    a.execute("mail.copy", json!({"ids": [m], "folder": other})).unwrap();
    rename_an_account(&mut a);
    let before = a.session.mailbox.clone();
    answer(&mut a, true);
    let reply = rx.try_recv().unwrap();
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(reply["error"].as_str().is_some_and(|e| e.contains("more than the owner was asked about")), "{reply}");
    assert!(a.session.mailbox == before);
}

#[test]
fn a_yes_is_about_the_step_asked_about_and_no_other_said_the_same_way() {
    // Asked about undoing a new contact; meanwhile the owner undoes it and makes another, which
    // the same question would describe word for word: the yes doesn't reach it.
    let mut a = app();
    a.execute("people.new", json!({"first": "Noor", "email": "noor@example.org"})).unwrap();
    let (now, rx) = agent(&mut a, "edit.undo");
    assert!(now.is_none(), "asked first");
    assert_eq!(a.asking.front().unwrap().class, Class::Delete);
    a.execute("edit.undo", json!({})).unwrap();
    a.execute("people.new", json!({"first": "Noor", "email": "noor@example.org"})).unwrap();
    let before = a.session.mailbox.clone();
    answer(&mut a, true);
    let reply = rx.try_recv().unwrap();
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(a.session.mailbox == before, "the owner's new contact stays");
}

#[test]
fn a_contact_that_came_down_while_it_was_asked_stays_after_the_yes() {
    // A contact downloaded while the question is up isn't in the step, and the undo keeps what
    // the step didn't touch: the yes takes away only the contact the step made.
    let mut a = app();
    a.execute("people.new", json!({"first": "Noor", "email": "noor@example.org"})).unwrap();
    let made = *a.session.mailbox.contacts.keys().max().unwrap();
    let (now, rx) = agent(&mut a, "edit.undo");
    assert!(now.is_none(), "asked first");
    let mb = &mut a.session.mailbox;
    let mut came = (**mb.contacts.values().next().unwrap()).clone();
    came.id = mb.contacts.keys().max().unwrap() + 1_000;
    let came_id = came.id;
    mb.contacts.insert(came_id, std::sync::Arc::new(came));
    answer(&mut a, true);
    let reply = rx.try_recv().unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(!a.session.mailbox.contacts.contains_key(&made));
    assert!(a.session.mailbox.contacts.contains_key(&came_id));
}

#[test]
fn a_yes_about_the_step_still_there_runs() {
    let mut a = app();
    a.execute("people.new", json!({"first": "Noor", "email": "noor@example.org"})).unwrap();
    let contacts = a.session.mailbox.contacts.len();
    let (_, rx) = agent(&mut a, "edit.undo");
    answer(&mut a, true);
    let reply = rx.try_recv().unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(a.session.mailbox.contacts.len(), contacts - 1);
}
