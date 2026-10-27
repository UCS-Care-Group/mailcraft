// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Leaving a message being written asks first (#442): Escape, the ✕, New Email, Reply, Reply All,
//! Forward and Open Draft show the Unsaved Message question that quitting shows, unless the
//! message is empty; Save Draft, Discard and Cancel each do what they say; and a script is never
//! asked.

use egui::Key;
use mailcraft_engine::Session;
use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use super::*;
use crate::Services;
use crate::dialogs::{Dialog, leave_message};
use crate::people_view::testing;

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a
}

fn drafts(a: &MailApp) -> Id {
    let account = a.session.mailbox.default_account().unwrap().id;
    a.session.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap()
}

fn draft_with(a: &MailApp, subject: &str) -> Option<Id> {
    let d = drafts(a);
    a.session.mailbox.messages.values().find(|m| m.folder == d && m.subject == subject).map(|m| m.id)
}

fn received(a: &MailApp) -> Id {
    let account = a.session.mailbox.default_account().unwrap().id;
    let inbox = a.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    a.session.mailbox.messages.values().find(|m| m.folder == inbox && !m.draft && !m.incomplete && !m.unfetched).unwrap().id
}

/// A message the owner has typed in.
fn written(a: &mut MailApp, subject: &str) {
    a.execute("mail.new", json!({})).unwrap();
    a.execute("compose.set", json!({"to": "ada@example.org", "subject": subject, "body": "Half a thought"})).unwrap();
}

fn subject(a: &MailApp) -> Option<String> {
    a.session.compose.as_ref().map(|c| c.subject.clone())
}

/// A draft kept in Drafts, and nothing being written.
fn with_a_draft() -> (MailApp, Id) {
    let mut a = app();
    written(&mut a, "Kept draft");
    a.execute("compose.saveDraft", json!({})).unwrap();
    a.execute("compose.close", json!({})).unwrap();
    let d = draft_with(&a, "Kept draft").unwrap();
    (a, d)
}

/// What the owner can do that leaves the message being written.
fn actions(a: &MailApp, draft: Id) -> Vec<(&'static str, Value)> {
    let other = received(a);
    vec![
        ("compose.close", json!({})),
        ("mail.new", json!({})),
        ("mail.reply", json!({"id": other})),
        ("mail.replyAll", json!({"id": other})),
        ("mail.forward", json!({"id": other})),
        ("mail.forwardAsAttachment", json!({"id": other})),
        ("mail.editDraft", json!({"id": draft})),
        // People's Email starts a new message too (#494).
        ("people.email", json!({"id": a.session.mailbox.contacts.values().find(|c| c.address().is_some()).unwrap().id})),
    ]
}

/// The question the window shows, and what waits for its answer.
fn asked(a: &MailApp) -> Option<(String, Value)> {
    match &a.dialog {
        Some(Dialog::DiscardDraft { then, params }) => Some((then.clone(), params.clone())),
        _ => None,
    }
}

/// What the action leaves being written: nothing for Close, else a message that isn't `was`.
fn went_on(a: &MailApp, id: &str, was: &str, draft: Id) {
    match id {
        "compose.close" => assert!(a.session.compose.is_none(), "{id}"),
        "mail.editDraft" => assert_eq!(a.session.compose.as_ref().unwrap().draft, Some(draft), "{id}"),
        _ => assert!(subject(a).is_some_and(|s| s != was), "{id}: {:?}", subject(a)),
    }
}

#[test]
fn an_empty_message_is_left_at_once() {
    let (mut a, draft) = with_a_draft();
    let original = received(&a);
    for start in ["mail.new", "mail.reply"] {
        for (id, params) in actions(&a, draft) {
            a.execute(start, json!({"id": original})).unwrap();
            let r = a.run(id, params).unwrap();
            assert!(r.get("pending").is_none() && a.dialog.is_none(), "{start} then {id}: nothing to ask about");
        }
    }
}

#[test]
fn a_message_with_changes_asks_and_cancel_leaves_it_as_it_was() {
    let (mut a, draft) = with_a_draft();
    for (id, params) in actions(&a, draft) {
        written(&mut a, "Mine");
        let before = (a.session.compose.clone(), a.session.mailbox.clone());
        let r = a.run(id, params.clone()).unwrap();
        assert_eq!(r["pending"], "discardDraft", "{id}");
        assert_eq!(asked(&a), Some((id.to_string(), params)), "{id}");
        assert_eq!(a.session.compose, before.0, "{id}: nothing happens before the answer");
        a.dialog = None;
        leave_message(&mut a, "cancel", id, json!({}));
        assert_eq!(a.session.compose, before.0, "{id}: Cancel stays in the message");
        assert!(a.session.mailbox == before.1, "{id}");
        a.execute("compose.close", json!({"discard": true})).unwrap();
    }
}

#[test]
fn save_draft_keeps_the_message_and_then_the_action_goes_on() {
    let (mut a, draft) = with_a_draft();
    for (n, (id, params)) in actions(&a, draft).into_iter().enumerate() {
        let mine = format!("Mine {n}");
        written(&mut a, &mine);
        a.run(id, params).unwrap();
        let (then, params) = asked(&a).unwrap();
        a.dialog = None;
        leave_message(&mut a, "save", &then, params);
        assert!(draft_with(&a, &mine).is_some(), "{id}: saved to Drafts");
        went_on(&a, id, &mine, draft);
        assert!(a.dialog.is_none(), "{id}: asked once");
    }
}

#[test]
fn discard_lets_the_message_go_and_then_the_action_goes_on() {
    let (mut a, draft) = with_a_draft();
    for (n, (id, params)) in actions(&a, draft).into_iter().enumerate() {
        let mine = format!("Mine {n}");
        written(&mut a, &mine);
        a.run(id, params).unwrap();
        let (then, params) = asked(&a).unwrap();
        a.dialog = None;
        leave_message(&mut a, "discard", &then, params);
        assert!(draft_with(&a, &mine).is_none(), "{id}: not kept");
        went_on(&a, id, &mine, draft);
        assert!(a.dialog.is_none(), "{id}");
    }
    // An opened draft whose changes are let go of stays in Drafts as it was saved.
    a.execute("mail.editDraft", json!({"id": draft})).unwrap();
    a.execute("compose.set", json!({"body": "Not wanted"})).unwrap();
    a.run("compose.close", json!({})).unwrap();
    leave_message(&mut a, "discard", "compose.close", json!({}));
    assert!(a.session.compose.is_none());
    assert!(!a.session.mailbox.message(draft).unwrap().body.text.contains("Not wanted"));
}

#[test]
fn a_draft_that_cant_be_saved_leaves_the_message_open_and_says_why() {
    let (mut a, draft) = with_a_draft();
    for (id, params) in actions(&a, draft) {
        written(&mut a, "Nowhere to go");
        let d = drafts(&a);
        a.session.mailbox.folders.iter_mut().find(|f| f.id == d).unwrap().kind = FolderKind::Custom;
        a.status_msg = None;
        let compose = a.session.compose.clone();
        a.run(id, params).unwrap();
        let (then, params) = asked(&a).unwrap();
        a.dialog = None;
        leave_message(&mut a, "save", &then, params);
        assert_eq!(a.session.compose, compose, "{id}: still being written");
        assert!(a.status_msg.as_ref().is_some_and(|(said, _)| said.contains("Drafts")), "{id}: {:?}", a.status_msg);
        a.session.mailbox.folders.iter_mut().find(|f| f.id == d).unwrap().kind = FolderKind::Drafts;
        a.execute("compose.close", json!({"discard": true})).unwrap();
    }
}

#[test]
fn quitting_still_asks_and_each_answer_still_quits() {
    for (answer, kept) in [("save", true), ("discard", false)] {
        let mut a = app();
        written(&mut a, "At quitting");
        assert!(!a.close_requested());
        assert_eq!(asked(&a).map(|(then, _)| then), Some("quit".to_string()));
        a.dialog = None;
        leave_message(&mut a, answer, "quit", json!({}));
        assert!(a.quit_requested && a.session.compose.is_none(), "{answer}");
        assert_eq!(draft_with(&a, "At quitting").is_some(), kept, "{answer}");
    }
}

#[test]
fn a_script_is_never_asked_and_what_was_written_is_saved() {
    let (mut a, draft) = with_a_draft();
    for (n, (id, params)) in actions(&a, draft).into_iter().enumerate() {
        let mine = format!("Scripted {n}");
        written(&mut a, &mine);
        a.execute(id, params).unwrap();
        assert!(a.dialog.is_none(), "{id}");
        assert!(draft_with(&a, &mine).is_some(), "{id}: saved first");
    }
}

/// Escape in the window, as the owner presses it.
#[test]
fn escape_asks_about_a_message_with_changes() {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    written(app, "Typed");
    h.run_steps(4);
    testing::key(&mut h, Key::Escape);
    assert_eq!(subject(h.state()).as_deref(), Some("Typed"), "still open");
    assert!(matches!(h.state().dialog, Some(Dialog::DiscardDraft { .. })));
    // Escape again answers Cancel: the message stays.
    testing::key(&mut h, Key::Escape);
    assert!(h.state().dialog.is_none());
    assert_eq!(subject(h.state()).as_deref(), Some("Typed"));
}

/// What runs after the question is the window's alone: opened by name (`ui.dialog`, which an
/// agent may run), it has nothing waiting, so the owner's answer can't run what an agent named.
#[test]
fn the_question_opened_by_name_runs_nothing_after_it() {
    for answer in ["save", "discard"] {
        let mut a = app();
        written(&mut a, "Planted");
        let outbox = |a: &MailApp| {
            let account = a.session.mailbox.default_account().unwrap().id;
            let o = a.session.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
            a.session.mailbox.messages.values().filter(|m| m.folder == o).count()
        };
        let before = outbox(&a);
        a.execute("ui.dialog", json!({"name": "discardDraft", "then": "compose.send", "params": {"id": 1}})).unwrap();
        let (then, params) = asked(&a).unwrap();
        assert_eq!((then.as_str(), &params), ("", &json!({})), "nothing waits");
        a.dialog = None;
        leave_message(&mut a, answer, &then, params);
        assert_eq!(outbox(&a), before, "{answer}: nothing was sent");
        assert!(!a.quit_requested);
        match answer {
            "save" => assert!(draft_with(&a, "Planted").is_some() && subject(&a).as_deref() == Some("Planted")),
            _ => assert!(a.session.compose.is_none() && draft_with(&a, "Planted").is_none()),
        }
    }
}
