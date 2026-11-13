// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What was done to a JMAP account's mail here goes with its run (M6), with a host that keeps
//! every job for the test to answer: the request carries the change, the answers are taken in as
//! they come, a run is due by itself once something waits, and a run under way gives way to it.
//! (The window's side of reading: `jmap_mail_tests.rs`.)

use std::sync::atomic::Ordering;

use mailcraft_model::sync::{
    FolderListing, GraphFolder, GraphMessage, JmapChange, JmapChangeDone, JmapMailResult, JmapOutcome, MessagePage, ServerMessage,
};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use super::tests::{last, taken, window};
use super::*;

/// The owner's Send/Receive of `account` with the password typed, answered by the host with the
/// server's Inbox (M1), Sent (M2) and Trash (M4), and `One` (E1) unread in the Inbox: the message.
fn with_mail(app: &mut MailApp, account: Id, jobs: &super::tests::Jobs) -> Id {
    app.execute("ui.sync", json!({"account": account, "password": "typed here"})).unwrap();
    let [(job, tx)]: [_; 1] = taken(jobs).try_into().ok().unwrap();
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders = vec![folder("M1", "Inbox", FolderKind::Inbox), folder("M2", "Sent", FolderKind::Sent), folder("M4", "Trash", FolderKind::Deleted)];
    let one = GraphMessage {
        server: ServerMessage { id: "E1".into(), size: 900, ..Default::default() },
        message_id: "e1@example.org".into(),
        subject: "One".into(),
        received: 100,
        ..Default::default()
    };
    let listing = Some(FolderListing { folders, complete: true });
    tx.send(last(&job, JmapMailResult { partial: true, listing, ..Default::default() })).unwrap();
    let pages = vec![MessagePage { folder: "M1".into(), messages: vec![one], afresh: true, ..Default::default() }];
    tx.send(last(&job, JmapMailResult { pages, whole: true, ..Default::default() })).unwrap();
    app.poll_sync();
    assert!(!app.syncing());
    app.session.mailbox.messages.values().find(|m| m.subject == "One").unwrap().id
}

#[test]
fn a_change_goes_with_a_run_that_is_due_by_itself_and_the_answer_is_taken_in() {
    let (mut app, accounts, jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    let one = with_mail(&mut app, ada, &jobs);
    assert_eq!(app.waiting_to_go(ada), 0);
    // A mark made here waits to go.
    app.execute("mail.markRead", json!({"ids": [one]})).unwrap();
    assert_eq!(app.waiting_to_go(ada), 1);
    // A run is due for it by itself, with no round of every account waited for: the first frame
    // lines it up, the next starts it, nobody asked.
    app.session.driver.set_auto_sync(true);
    app.session.prefs.sync_minutes = 0;
    app.auto_sync();
    app.auto_sync();
    let [(job, tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    assert!(job.request.background && job.secret == "typed here" && app.dialog.is_none());
    let change = JmapChange::Keywords { message: one, id: "E1".into(), read: Some(true), flagged: None, answered: None, forwarded: None };
    assert_eq!(job.request.changes, std::slice::from_ref(&change));
    assert!(job.request.sends.is_empty());
    // The first piece says only what became of what went up: taken in as it comes.
    let changes = vec![JmapChangeDone { change, outcome: JmapOutcome::Done { id: String::new() } }];
    tx.send(last(&job, JmapMailResult { partial: true, changes, ..Default::default() })).unwrap();
    app.poll_sync();
    assert!(app.syncing());
    assert!(app.session.mailbox.message(one).and_then(|m| m.server.as_ref()).is_some_and(|server| server.read));
    tx.send(last(&job, JmapMailResult { whole: true, ..Default::default() })).unwrap();
    app.poll_sync();
    assert!(!app.syncing() && !app.has_sync_problems());
    assert!(app.session.sync_status.starts_with("Up to date"), "{}", app.session.sync_status);
    // Nothing waits any more, and nothing more starts.
    assert_eq!(app.waiting_to_go(ada), 0);
    app.auto_sync();
    app.auto_sync();
    assert!(taken(&jobs).is_empty());
}

#[test]
fn something_made_while_a_run_is_under_way_calls_it_off_to_go_up_first() {
    let (mut app, accounts, jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    let one = with_mail(&mut app, ada, &jobs);
    app.execute("ui.sync", json!({"account": ada})).unwrap();
    let [(job, tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    assert!(job.request.changes.is_empty() && !job.cancel.load(Ordering::Relaxed));
    // The owner deletes the message while the run reads.
    app.execute("mail.delete", json!({"ids": [one]})).unwrap();
    app.session.driver.set_auto_sync(true);
    app.session.prefs.sync_minutes = 0;
    app.auto_sync();
    assert!(job.cancel.load(Ordering::Relaxed));
    assert_eq!(app.session.sync_status, "Sending your changes…");
    // The run ends where it was, and the account's next run takes the move up.
    tx.send(last(&job, JmapMailResult { stopped: true, ..Default::default() })).unwrap();
    app.poll_sync();
    app.auto_sync();
    let [(job, _tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    assert_eq!(job.request.changes, [JmapChange::Move { message: one, id: "E1".into(), from: "M1".into(), to: "M4".into() }]);
}

#[test]
fn a_refused_change_is_told_once_and_starts_no_run_after_run() {
    let (mut app, accounts, jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    let one = with_mail(&mut app, ada, &jobs);
    app.execute("mail.deletePermanently", json!({"ids": [one]})).unwrap();
    app.execute("ui.sync", json!({"account": ada})).unwrap();
    let [(job, tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    let change = JmapChange::Destroy { id: "E1".into() };
    assert_eq!(job.request.changes, std::slice::from_ref(&change));
    let changes = vec![JmapChangeDone { change, outcome: JmapOutcome::Refused { why: "the mailbox is read-only".into() } }];
    tx.send(last(&job, JmapMailResult { partial: true, changes, ..Default::default() })).unwrap();
    tx.send(last(&job, JmapMailResult { whole: true, ..Default::default() })).unwrap();
    app.poll_sync();
    // Back where the server has it, and said: in the status and in the problems of the run.
    assert!(app.session.mailbox.messages.values().any(|m| m.subject == "One"));
    let said = "Couldn't permanently delete 'One': the mailbox is read-only.";
    assert!(app.session.sync_status.starts_with(said), "{}", app.session.sync_status);
    assert!(app.has_sync_problems());
    // Nothing is left to go, so no run follows by itself.
    assert_eq!(app.waiting_to_go(ada), 0);
    app.session.driver.set_auto_sync(true);
    app.session.prefs.sync_minutes = 0;
    app.auto_sync();
    app.auto_sync();
    assert!(taken(&jobs).is_empty());
}

#[test]
fn emptying_a_folder_says_the_server_is_told_only_when_the_server_has_some_of_it() {
    let (mut app, accounts, jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    let one = with_mail(&mut app, ada, &jobs);
    let trash = app.session.mailbox.folder_of_kind(ada, FolderKind::Deleted).unwrap();
    let asks = |app: &MailApp| crate::dialogs::confirmation(app, "folder.empty", &json!({"id": trash})).expect("asks first").text;
    // Only what this computer has: nothing is removed anywhere else.
    app.session.mailbox.add_message(mailcraft_model::Message { folder: trash, subject: "Mine".into(), ..Default::default() }).unwrap();
    assert!(!asks(&app).contains("mail server"), "{}", asks(&app));
    // With one the server has among it, the server is told too.
    app.execute("mail.delete", json!({"ids": [one]})).unwrap();
    assert_eq!(app.session.mailbox.message(one).map(|m| m.folder), Some(trash));
    assert!(asks(&app).contains("They are removed from the mail server too."), "{}", asks(&app));
}

#[test]
fn a_mark_made_while_another_mark_of_the_message_goes_up_has_a_run_of_its_own_at_once() {
    let (mut app, accounts, jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    let one = with_mail(&mut app, ada, &jobs);
    app.execute("mail.markRead", json!({"ids": [one]})).unwrap();
    app.session.driver.set_auto_sync(true);
    app.session.prefs.sync_minutes = 0;
    app.auto_sync();
    app.auto_sync();
    let [(job, tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    let change = JmapChange::Keywords { message: one, id: "E1".into(), read: Some(true), flagged: None, answered: None, forwarded: None };
    assert_eq!(job.request.changes, std::slice::from_ref(&change));
    // Flagged while the run is under way: still one change of the message, so nothing calls the
    // run off, and the flag is no part of what the run took up.
    app.execute("mail.flag", json!({"ids": [one]})).unwrap();
    assert_eq!(app.waiting_to_go(ada), 1);
    let changes = vec![JmapChangeDone { change, outcome: JmapOutcome::Done { id: String::new() } }];
    tx.send(last(&job, JmapMailResult { whole: true, changes, ..Default::default() })).unwrap();
    app.poll_sync();
    assert!(!app.syncing());
    assert_eq!(app.waiting_to_go(ada), 1, "the flag still waits");
    // It is not something the server turned down: the next look starts a run for it.
    app.push_checked = 0.0;
    app.auto_sync();
    app.auto_sync();
    let [(job, tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    let flag = JmapChange::Keywords { message: one, id: "E1".into(), read: None, flagged: Some(true), answered: None, forwarded: None };
    assert_eq!(job.request.changes, std::slice::from_ref(&flag));
    // A change that gets no answer makes nothing: no further run is started for it by itself.
    tx.send(last(&job, JmapMailResult { whole: true, ..Default::default() })).unwrap();
    app.poll_sync();
    assert_eq!(app.waiting_to_go(ada), 1);
    app.push_checked = 0.0;
    app.auto_sync();
    app.auto_sync();
    assert!(taken(&jobs).is_empty());
}
