// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent doesn't take the window off what the owner is changing without asking (#494): the
//! message being written, and an appointment, a contact or a task with changes that aren't saved
//! in the dialog. The owner is asked over the window; a no leaves everything as it was.

use std::sync::mpsc::Receiver;

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use crate::agent_gate::answer;
use crate::control::{ControlRequest, Outcome};
use crate::dialogs::Dialog;
use crate::people_view::testing;
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a
}

/// `command` sent by an agent allowed nothing in advance: its answer, when it has one already.
fn agent(a: &mut MailApp, command: &str, params: Value) -> (Option<Value>, Receiver<Value>) {
    let (req, rx) = ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    let out = crate::control::handle(a, &egui::Context::default(), &req.from_agent(Vec::new()));
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

/// The question on screen: its class and what it says the agent wants.
fn question(a: &MailApp) -> Option<(Class, String)> {
    a.asking.front().map(|q| (q.class, q.what.clone()))
}

/// The owner has typed in a new message and not saved it.
fn written(a: &mut MailApp, subject: &str) {
    a.execute("mail.new", json!({})).unwrap();
    a.execute("compose.set", json!({"to": "ada@example.org", "subject": subject, "body": "Half a thought"})).unwrap();
}

fn draft_with(a: &MailApp, subject: &str) -> Option<Id> {
    a.session.mailbox.messages.values().find(|m| m.draft && m.subject == subject).map(|m| m.id)
}

fn received(a: &MailApp) -> Id {
    let account = a.session.mailbox.default_account().unwrap().id;
    let inbox = a.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    a.session.mailbox.messages.values().find(|m| m.folder == inbox && !m.draft && !m.incomplete && !m.unfetched).unwrap().id
}

/// What an agent can run that closes the message being written or puts another in its place.
fn leaving(a: &MailApp) -> Vec<(&'static str, Value)> {
    let other = received(a);
    let draft = a.session.mailbox.messages.values().find(|m| m.draft).unwrap().id;
    let contact = a.session.mailbox.contacts.values().find(|c| c.address().is_some()).unwrap().id;
    vec![
        ("compose.close", json!({})),
        ("mail.new", json!({})),
        ("mail.reply", json!({"id": other})),
        ("mail.replyAll", json!({"id": other})),
        ("mail.forward", json!({"id": other})),
        ("mail.forwardAsAttachment", json!({"id": other})),
        ("mail.editDraft", json!({"id": draft})),
        ("people.email", json!({"id": contact})),
    ]
}

#[test]
fn an_agent_waits_for_the_owners_yes_before_it_leaves_their_message() {
    let mut a = app();
    written(&mut a, "Mine");
    for (id, params) in leaving(&a) {
        let before = (a.session.compose.clone(), a.session.mailbox.clone());
        let (now, rx) = agent(&mut a, id, params.clone());
        assert!(now.is_none(), "{id}: asked first");
        let (class, what) = question(&a).unwrap();
        assert_eq!(class, Class::Leave, "{id}");
        assert!(what.contains("\u{201c}Mine\u{201d}") && what.contains("Drafts"), "{id}: {what}");
        assert_eq!(a.session.compose, before.0, "{id}: nothing happens before the answer");
        answer(&mut a, false);
        assert_eq!(rx.try_recv().unwrap()["ok"], false, "{id}");
        assert_eq!(a.session.compose, before.0, "{id}: no leaves the owner in their message");
        assert!(a.session.mailbox == before.1, "{id}");
    }
    // Yes: it is saved to Drafts, and the agent's command goes on.
    let (_, rx) = agent(&mut a, "mail.new", json!({}));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert!(draft_with(&a, "Mine").is_some());
    assert!(a.session.compose.as_ref().is_some_and(|c| c.subject.is_empty() && !c.dirty));
}

#[test]
fn an_agent_that_would_throw_the_owners_message_away_asks_for_delete() {
    let mut a = app();
    written(&mut a, "Mine");
    for (id, mut params) in leaving(&a) {
        params["discard"] = json!(true);
        let (now, _) = agent(&mut a, id, params);
        assert!(now.is_none(), "{id}: asked first");
        let (class, what) = question(&a).unwrap();
        assert_eq!(class, Class::Delete, "{id}");
        assert!(what.contains("without saving what the owner wrote"), "{id}: {what}");
        answer(&mut a, false);
        assert_eq!(a.session.compose.as_ref().map(|c| c.subject.as_str()), Some("Mine"), "{id}");
    }
    let (_, rx) = agent(&mut a, "compose.close", json!({"discard": true}));
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert!(a.session.compose.is_none() && draft_with(&a, "Mine").is_none());
}

#[test]
fn an_agents_own_message_is_left_without_a_question() {
    let mut a = app();
    let (now, _) = agent(&mut a, "mail.new", json!({}));
    assert_eq!(now.unwrap()["ok"], true);
    let (now, _) = agent(&mut a, "compose.set", json!({"subject": "The agent's", "body": "Written by the agent"}));
    assert_eq!(now.unwrap()["ok"], true);
    let (now, _) = agent(&mut a, "mail.new", json!({}));
    assert_eq!(now.unwrap()["ok"], true, "not asked");
    assert!(a.asking.is_empty());
    assert!(draft_with(&a, "The agent's").is_some(), "saved to Drafts first");
}

/// The window with the dialog `name` open, as the owner opened it, and `typed` typed into its
/// first box.
fn form(name: &str, typed: &str) -> testing::Window {
    let mut w = testing::window("view.people");
    w.state_mut().execute("ui.dialog", json!({"name": name})).unwrap();
    w.run_steps(3);
    if !typed.is_empty() {
        testing::text(&mut w, typed);
    }
    w
}

fn form_open(w: &testing::Window) -> bool {
    matches!(w.state().dialog, Some(Dialog::Form(_)))
}

#[test]
fn an_agent_asks_before_it_closes_a_form_the_owner_changed() {
    for (name, kind) in [("contact", "contact"), ("task", "task"), ("event", "appointment")] {
        let mut w = form(name, "Ada");
        let (now, rx) = agent(w.state_mut(), "ui.closeDialog", json!({}));
        assert!(now.is_none(), "{name}: asked first");
        let (class, what) = question(w.state()).unwrap();
        assert_eq!(class, Class::Delete, "{name}");
        assert_eq!(what, format!("close the {kind} \u{201c}Ada\u{201d} without saving the changes made in it"), "{name}");
        answer(w.state_mut(), false);
        assert_eq!(rx.try_recv().unwrap()["ok"], false);
        assert!(form_open(&w), "{name}: no leaves it open");
        // Another dialog in its place is asked about too; Send/Receive Progress isn't one.
        let (now, _) = agent(w.state_mut(), "ui.dialog", json!({"name": "rules"}));
        assert!(now.is_none(), "{name}: asked first");
        assert!(question(w.state()).unwrap().1.starts_with("open the dialog \u{201c}rules\u{201d} in place of the"), "{name}");
        answer(w.state_mut(), false);
        assert!(form_open(&w), "{name}");
        let (now, _) = agent(w.state_mut(), "ui.dialog", json!({"name": "syncProgress"}));
        assert_eq!(now.unwrap()["ok"], true, "{name}");
        assert!(form_open(&w), "{name}");
        // Yes closes it.
        let (_, rx) = agent(w.state_mut(), "ui.closeDialog", json!({}));
        answer(w.state_mut(), true);
        assert_eq!(rx.try_recv().unwrap()["ok"], true);
        assert!(w.state().dialog.is_none(), "{name}");
    }
}

#[test]
fn a_form_with_nothing_changed_since_it_opened_is_closed_without_a_question() {
    // As the owner left it.
    let mut w = form("contact", "");
    let (now, _) = agent(w.state_mut(), "ui.closeDialog", json!({}));
    assert_eq!(now.unwrap()["ok"], true);
    assert!(w.state().dialog.is_none());
    // Filled in by the agent as it opened it.
    let (now, _) = agent(w.state_mut(), "ui.dialog", json!({"name": "task", "title": "The agent's"}));
    assert_eq!(now.unwrap()["ok"], true);
    assert!(form_open(&w));
    let (now, _) = agent(w.state_mut(), "ui.closeDialog", json!({}));
    assert_eq!(now.unwrap()["ok"], true);
    assert!(w.state().dialog.is_none());
}

#[test]
fn what_the_dialog_holds_is_the_windows_to_say() {
    let mut w = form("contact", "Ada");
    // An agent that says there is nothing unsaved is asked even so.
    let (now, _) = agent(w.state_mut(), "ui.closeDialog", json!({"unsaved": null}));
    assert!(now.is_none());
    answer(w.state_mut(), false);
    let (now, _) = agent(w.state_mut(), "ui.closeDialog", Value::Null);
    assert!(now.is_none());
    answer(w.state_mut(), false);
    assert!(form_open(&w));
}
