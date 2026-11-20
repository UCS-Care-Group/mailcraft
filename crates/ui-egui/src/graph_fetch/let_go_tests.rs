// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A command that waits for its message to download runs again once it is here (#513). A reply
//! opens beside the message being written, which stays open and is saved to Drafts first (#286):
//! a message opened or changed while it waited is never thrown away nor asked about, `discard` or
//! not. Nor does what it asks take the place of a form the owner is changing: it waits for the
//! form to be closed.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::{Module, Session};
use mailcraft_model::sync::{GraphFetchResult, GraphFetched, ServerMessage, Tokens};
use mailcraft_model::{Body, FolderKind, Message};

use super::*;
use crate::Services;
use crate::dialogs::Dialog;
use crate::people_view::testing;

type Jobs = Arc<Mutex<Vec<(GraphFetchJob, mpsc::Sender<GraphFetchResult>)>>>;

const NOW: i64 = 1_791_993_600;

/// A window with a Microsoft account whose Inbox holds one message of which only the headers are
/// here, and a host that keeps every job for the test to answer.
fn window() -> (MailApp, Id, Jobs) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::default(), NOW, 0), Services::default());
    let (waiting, jobs) = microsoft(&mut app);
    (app, waiting, jobs)
}

/// `app` with the account and the message of [`window`].
fn microsoft(app: &mut MailApp) -> (Id, Jobs) {
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let account = app.execute("account.add", json!({"email": "one@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let message = Message {
        account,
        folder: inbox,
        subject: "Waiting".into(),
        date: NOW - 3_600,
        received: NOW - 3_600,
        unfetched: true,
        body: Body { text: "A preview".into(), html: None },
        server: Some(ServerMessage { id: "AAM0".into(), folder: "inbox".into(), size: 2_000, ..Default::default() }),
        ..Default::default()
    };
    let waiting = app.session.mailbox.add_message(message).unwrap();
    let tokens = Tokens { access: "stand-in".into(), refresh: "r".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
    app.seed_graph_token(account, tokens);
    app.session.view.folder = inbox;
    app.session.sync_progress.full_paused = true;
    let jobs: Jobs = Default::default();
    let kept = jobs.clone();
    app.services.graph_fetch = Some(Box::new(move |job: GraphFetchJob| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push((job, tx));
        rx
    }));
    (waiting, jobs)
}

/// The message being written with `subject` is still open, beside the current one, and what is
/// written in it is in Drafts: the reply saved it first.
fn kept_open(app: &MailApp, subject: &str) -> bool {
    app.session.composing.iter().any(|c| c.subject == subject) && draft_with(app, subject)
}

/// Every message asked for comes whole, and the window takes it in.
fn downloaded(app: &mut MailApp, jobs: &Jobs) {
    answered(jobs);
    app.drive_fetch();
}

/// Every message asked for comes whole, for the window to take in.
fn answered(jobs: &Jobs) {
    for (job, tx) in std::mem::take(&mut *jobs.lock().unwrap()) {
        let mut result = GraphFetchResult { account: job.request.account, email: job.request.email.clone(), ..Default::default() };
        for id in &job.request.ids {
            let mime = format!("From: someone@example.org\r\nSubject: Waiting\r\nMessage-ID: <{id}@example.org>\r\n\r\nAll of {id}.\r\n");
            result.fetched.push(GraphFetched { id: id.clone(), mime: mime.into_bytes() });
        }
        let _ = tx.send(result);
    }
}

/// The owner writes `subject` in the message being written, or in a new one.
fn owner_writes(app: &mut MailApp, subject: &str) {
    if app.session.compose.is_none() {
        // (In the reading pane, as a script starts it: the owner's New Email has a window, #286.)
        app.execute("mail.new", json!({})).unwrap();
    }
    app.execute("compose.set", json!({"subject": subject, "body": "Not saved yet"})).unwrap();
}

fn subject(app: &MailApp) -> Option<String> {
    app.session.compose.as_ref().map(|c| c.subject.clone())
}

fn draft_with(app: &MailApp, subject: &str) -> bool {
    app.session.mailbox.messages.values().any(|m| m.draft && m.subject == subject)
}

#[test]
fn a_message_opened_while_it_waited_is_kept_beside_the_reply() {
    let (mut app, waiting, jobs) = window();
    let r = app.run("mail.reply", json!({"id": waiting, "discard": true})).unwrap();
    assert_eq!(r["pending"], "download");
    owner_writes(&mut app, "Started meanwhile");
    downloaded(&mut app, &jobs);
    assert!(app.dialog.is_none(), "nothing to ask about");
    assert!(subject(&app).is_some_and(|s| s.contains("Waiting")), "{:?}", subject(&app));
    assert!(kept_open(&app, "Started meanwhile"), "not thrown away");
}

#[test]
fn a_message_changed_while_it_waited_is_kept_beside_the_reply() {
    let (mut app, waiting, jobs) = window();
    owner_writes(&mut app, "Let go");
    app.run("mail.reply", json!({"id": waiting, "discard": true})).unwrap();
    owner_writes(&mut app, "Let go, but written on");
    downloaded(&mut app, &jobs);
    assert!(app.dialog.is_none());
    assert!(subject(&app).is_some_and(|s| s.contains("Waiting")), "{:?}", subject(&app));
    assert!(kept_open(&app, "Let go, but written on"));
}

/// A window, frames and all, with the account and the message of [`window`].
fn drawn() -> (testing::Window, Id, Jobs) {
    let mut w = testing::window_on(mailcraft_model::Mailbox::default(), "view.mail");
    let (waiting, jobs) = microsoft(w.state_mut());
    w.run_steps(3);
    (w, waiting, jobs)
}

/// The owner opens the dialog `name` and types `typed` in its first box.
fn owner_fills(w: &mut testing::Window, name: &str, typed: &str) {
    w.state_mut().execute("ui.dialog", json!({"name": name})).unwrap();
    w.run_steps(3);
    testing::text(w, typed);
    assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if f.dirty()), "{name}");
}

#[test]
fn a_form_being_changed_when_it_came_stays_until_it_is_closed() {
    for name in ["contact", "task", "event"] {
        let (mut w, waiting, jobs) = drawn();
        assert_eq!(w.state_mut().run("mail.reply", json!({"id": waiting})).unwrap()["pending"], "download");
        owner_writes(w.state_mut(), "Mine");
        owner_fills(&mut w, name, "Ada");
        answered(&jobs);
        w.run_steps(5);
        // Unsaved Message would have taken the form's place, and what was typed in it with it.
        assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if f.dirty()), "{name}: {:?}", w.state().dialog.as_ref().map(|d| d.name()));
        assert_eq!(subject(w.state()).as_deref(), Some("Mine"), "{name}");
        // Closed, the reply goes on, beside the message being written.
        w.state_mut().execute("ui.closeDialog", json!({})).unwrap();
        w.run_steps(3);
        assert!(w.state().dialog.is_none(), "{name}");
        assert!(subject(w.state()).is_some_and(|s| s.contains("Waiting")), "{name}: {:?}", subject(w.state()));
        assert!(kept_open(w.state(), "Mine"), "{name}");
    }
}

#[test]
fn one_that_waits_for_the_form_gives_way_to_the_next_and_says_so() {
    let (mut w, first, jobs) = drawn();
    let app = w.state_mut();
    let mut second = app.session.mailbox.message(first).unwrap().clone();
    second.id = 0;
    second.subject = "Second".into();
    second.server = Some(ServerMessage { id: "AAM1".into(), folder: "inbox".into(), size: 2_000, ..Default::default() });
    let second = app.session.mailbox.add_message(second).unwrap();
    app.run("mail.reply", json!({"id": first})).unwrap();
    owner_fills(&mut w, "contact", "Ada");
    answered(&jobs);
    w.run_steps(5);
    // The owner asks for another reply meanwhile (from a message's window of its own).
    assert_eq!(w.state_mut().run("mail.reply", json!({"id": second})).unwrap()["pending"], "download");
    let said = w.state().status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("was called off"), "{said}");
    answered(&jobs);
    w.run_steps(5);
    w.state_mut().execute("ui.closeDialog", json!({})).unwrap();
    w.run_steps(3);
    // The second one alone.
    assert_eq!(w.state().session.compose.as_ref().and_then(|c| c.source), Some(second));
}

#[test]
fn a_recurrence_being_filled_in_holds_it_too() {
    use egui_kittest::kittest::Queryable as _;
    let (mut w, waiting, jobs) = drawn();
    assert_eq!(w.state_mut().run("mail.reply", json!({"id": waiting})).unwrap()["pending"], "download");
    owner_writes(w.state_mut(), "Mine");
    w.state_mut().execute("ui.dialog", json!({"name": "event"})).unwrap();
    w.run_steps(3);
    // Make it repeat, ending after a number of times: in the Appointment Recurrence dialog only.
    for label in ["Make it repeat", "After"] {
        let at = w.query_all_by_label_contains(label).last().unwrap_or_else(|| panic!("{label}")).rect();
        testing::click(&mut w, at.center(), egui::PointerButton::Primary, 1);
        w.run_steps(3);
    }
    assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if !f.dirty() && f.changed()));
    answered(&jobs);
    w.run_steps(5);
    assert!(matches!(&w.state().dialog, Some(Dialog::Form(f)) if f.changed()), "{:?}", w.state().dialog.as_ref().map(|d| d.name()));
    assert_eq!(subject(w.state()).as_deref(), Some("Mine"));
}

/// What the form a drop opened starts with: its command and fields.
fn dropped_form(app: &MailApp) -> Option<(String, Value)> {
    match &app.dialog {
        Some(Dialog::Form(crate::forms::Form::Task(f))) => Some(f.command()).map(|(c, v)| (c.to_string(), v)),
        Some(Dialog::Form(crate::forms::Form::Contact(f))) => Some(f.command()).map(|(c, v)| (c.to_string(), v)),
        _ => None,
    }
}

#[test]
fn a_message_dropped_on_a_module_waits_for_the_whole_message() {
    let (mut w, waiting, jobs) = drawn();
    // Only the server's short preview is here: nothing opens with it. The message is asked for,
    // as for Reply, and the status bar says so.
    crate::nav::drop_on_module(w.state_mut(), Module::Tasks, &[waiting]);
    assert!(dropped_form(w.state()).is_none(), "no form with the preview");
    assert_eq!(jobs.lock().unwrap().len(), 1, "the message is asked for");
    let said = w.state().status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("Downloading the message first"), "{said}");
    answered(&jobs);
    w.run_steps(5);
    // Whole, it runs again: the form has the text of the message, not the preview.
    let (command, fields) = dropped_form(w.state()).expect("the form opens once the message is here");
    assert_eq!(command, "tasks.new");
    let notes = fields["notes"].as_str().unwrap_or_default();
    assert!(notes.contains("All of AAM0") && !notes.contains("A preview"), "{fields}");
}

#[test]
fn a_message_dropped_that_cannot_be_downloaded_says_so_and_opens_nothing() {
    let (mut w, waiting, jobs) = drawn();
    // No sign-in that is good: it can't come.
    w.state_mut().graph_tokens.clear();
    crate::nav::drop_on_module(w.state_mut(), Module::People, &[waiting]);
    assert!(dropped_form(w.state()).is_none());
    assert!(jobs.lock().unwrap().is_empty());
    let said = w.state().status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("isn't on this computer yet"), "{said}");
}

#[test]
fn a_drop_naming_a_great_many_messages_looks_at_the_first_ones_only() {
    let (mut w, waiting, jobs) = drawn();
    // The message that is here comes after more ids than are looked at: none is found.
    let mut ids: Vec<Id> = (1_000_000..1_002_000).collect();
    ids.push(waiting);
    crate::nav::drop_on_module(w.state_mut(), Module::Tasks, &ids);
    assert!(dropped_form(w.state()).is_none() && jobs.lock().unwrap().is_empty());
    let said = w.state().status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("None of those messages"), "{said}");
    // Among the first ones, it is found.
    ids.insert(0, waiting);
    crate::nav::drop_on_module(w.state_mut(), Module::Tasks, &ids);
    assert_eq!(jobs.lock().unwrap().len(), 1);
}
