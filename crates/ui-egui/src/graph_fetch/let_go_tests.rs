// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A command that waits for its message to download runs again once it is here (#513). A reply
//! opens beside the message being written, which stays open as it is (#286): a message opened or
//! changed while it waited is never thrown away nor asked about. Nor does what it asks take the
//! place of a form the owner is changing: it waits for the form to be closed.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
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
    (app.autosave, app.auto_sync) = (false, false);
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

/// The message being written with `subject` is still open, beside the current one, and saved nowhere.
fn kept_open(app: &MailApp, subject: &str) -> bool {
    app.session.composing.iter().any(|c| c.subject == subject && c.dirty) && !draft_with(app, subject)
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
        app.run("mail.new", json!({})).unwrap();
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

#[test]
fn the_message_it_was_asked_to_let_go_is_let_go() {
    let (mut app, waiting, jobs) = window();
    owner_writes(&mut app, "Let go");
    app.run("mail.reply", json!({"id": waiting, "discard": true})).unwrap();
    downloaded(&mut app, &jobs);
    assert!(app.dialog.is_none(), "as asked: no question");
    assert!(subject(&app).is_some_and(|s| s.contains("Waiting")), "{:?}", subject(&app));
    assert!(!draft_with(&app, "Let go"));
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
fn what_it_was_asked_to_let_go_is_let_go_once_the_form_is_closed() {
    let (mut w, waiting, jobs) = drawn();
    owner_writes(w.state_mut(), "Mine");
    w.state_mut().run("mail.reply", json!({"id": waiting, "discard": true})).unwrap();
    owner_fills(&mut w, "contact", "Ada");
    answered(&jobs);
    w.run_steps(5);
    assert!(matches!(w.state().dialog, Some(Dialog::Form(_))));
    assert_eq!(subject(w.state()).as_deref(), Some("Mine"), "waits");
    w.state_mut().execute("ui.closeDialog", json!({})).unwrap();
    w.run_steps(3);
    assert!(w.state().dialog.is_none(), "as asked: no question");
    assert!(subject(w.state()).is_some_and(|s| s.contains("Waiting")), "{:?}", subject(w.state()));
    assert!(!draft_with(w.state(), "Mine"));
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
