// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An IMAP account's message that its mail to keep offline left as its headers and preview
//! (#653) is fetched whole by its UID when it is opened, and before a reply, with the password
//! its Send/Receive goes with; without one nothing is asked of the server.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::sync::{GraphFetchResult, GraphFetched, ImapFetchKey};
use mailcraft_model::{Body, FolderKind, Message};

use super::*;
use crate::Services;

type Jobs = Arc<Mutex<Vec<(ImapFetchJob, mpsc::Sender<GraphFetchResult>)>>>;

const NOW: i64 = 1_791_993_600;

/// A window with an IMAP account keeping a month, whose Inbox holds a message of a year ago of
/// which only the headers and a preview are here, and a host that keeps every job for the test
/// to answer. The password is known when `password` is.
fn window(password: bool) -> (MailApp, Id, Jobs) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::default(), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let account =
        app.execute("account.add", json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    app.execute("account.edit", json!({"id": account, "keepOffline": "1m"})).unwrap();
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let f = app.session.mailbox.folder_mut(inbox).unwrap();
    (f.remote, f.uid_validity) = ("INBOX".into(), 7);
    let message = Message {
        account,
        folder: inbox,
        uid: 42,
        subject: "A year ago".into(),
        received: NOW - 365 * 86_400,
        unfetched: true,
        body: Body { text: "A preview".into(), html: None },
        ..Default::default()
    };
    let waiting = app.session.mailbox.add_message(message).unwrap();
    if password {
        app.passwords.insert(account, "hunter2".into());
    }
    app.session.view.folder = inbox;
    app.session.sync_progress.full_paused = true;
    let jobs: Jobs = Default::default();
    let kept = jobs.clone();
    app.services.imap_fetch = Some(Box::new(move |job: ImapFetchJob| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push((job, tx));
        rx
    }));
    (app, waiting, jobs)
}

fn key() -> String {
    ImapFetchKey { remote: "INBOX".into(), uid_validity: 7, uid: 42 }.write()
}

/// Every message asked for comes whole.
fn answer(jobs: &Jobs) {
    for (job, tx) in std::mem::take(&mut *jobs.lock().unwrap()) {
        let mut result = GraphFetchResult { account: job.request.account.id, email: job.request.account.email.clone(), ..Default::default() };
        for id in &job.request.ids {
            let mime = b"From: Bob <bob@example.org>\r\nSubject: A year ago\r\n\r\nAll of it, from the server.\r\n".to_vec();
            result.fetched.push(GraphFetched { id: id.clone(), mime });
        }
        tx.send(result).unwrap();
    }
}

#[test]
fn opening_a_preview_only_imap_message_fetches_it_by_its_uid() {
    let (mut app, waiting, jobs) = window(true);
    app.session.sel.messages = vec![waiting];
    app.drive_fetch();
    {
        let asked = jobs.lock().unwrap();
        assert_eq!(asked.len(), 1);
        let (job, _) = &asked[0];
        assert_eq!(job.request.ids, vec![key()]);
        assert_eq!(job.secret, "hunter2");
        assert!(!job.request.background);
    }
    assert!(app.fetching.fetching(waiting));
    answer(&jobs);
    app.drive_fetch();
    let m = app.session.mailbox.message(waiting).unwrap();
    assert!(!m.unfetched && m.body.text.contains("All of it"), "{:?}", m.body.text);
    // Older than the window: whole for the rest of the session, not thinned again meanwhile.
    assert_eq!(app.session.run("account.thinOffline", &json!({})).unwrap()["thinned"], 0);
}

#[test]
fn a_reply_waits_for_the_whole_message() {
    let (mut app, waiting, jobs) = window(true);
    let said = app.run("mail.reply", json!({"id": waiting})).unwrap();
    assert_eq!(said["pending"], "download");
    assert_eq!(jobs.lock().unwrap().len(), 1);
    answer(&jobs);
    app.drive_fetch();
    assert!(!app.session.mailbox.message(waiting).unwrap().unfetched);
    let subject = app.session.compose.as_ref().map(|c| c.subject.clone());
    assert!(subject.as_deref().is_some_and(|s| s.contains("A year ago")), "{subject:?}");
}

#[test]
fn without_the_password_nothing_is_asked_of_the_server() {
    let (mut app, waiting, jobs) = window(false);
    app.session.sel.messages = vec![waiting];
    app.drive_fetch();
    assert!(jobs.lock().unwrap().is_empty());
    assert!(app.session.mailbox.message(waiting).unwrap().unfetched);
    assert!(app.session.driver.fetch.offline);
}
