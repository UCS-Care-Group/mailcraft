// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A calendar or To Do job's answer that comes back with somebody else's sign-in is refused whole
//! in the window: nothing of it reaches the mailbox, its token is not kept, and the owner is told
//! who to sign in as (#103). Hosts are fakes; every address is invented.

use std::sync::mpsc;

use mailcraft_engine::Session;
use mailcraft_model::Id;
use mailcraft_model::remote::{EventsDownload, RemoteEvent};
use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList, TasksDownload};
use mailcraft_model::sync::Tokens;
use serde_json::json;

use crate::task_sync::{TasksJob, TasksResult};
use crate::{GraphRequest, GraphResult, MailApp, Services};

const OWNER: &str = "noor@example.org";
const STRANGER: &str = "mallory@example.net";
const NOW: i64 = 1_791_993_600;
const SAID: &str = "You signed in as mallory@example.net, but this account is noor@example.org. Sign in again as noor@example.org.";

fn theirs() -> Tokens {
    Tokens {
        access: "their-access".into(),
        refresh: "their-refresh".into(),
        expires_at: mailcraft_time::now() + 3_600,
        email: STRANGER.into(),
        ..Default::default()
    }
}

fn window() -> (MailApp, Id) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let owner = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let own = Tokens { access: "own-access".into(), refresh: "ref-1".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
    app.tokens.insert(owner, own.clone());
    app.graph_tokens.insert(owner, own);
    (app, owner)
}

/// Neither the session nor the Graph tokens hold anything of the stranger's.
fn nothing_of_theirs_kept(app: &MailApp, owner: Id) -> bool {
    app.tokens.get(&owner).is_some_and(|t| t.refresh == "ref-1" && t.access == "own-access")
        && app.graph_tokens.get(&owner).is_some_and(|t| t.access == "own-access" && t.refresh == "ref-1")
}

#[test]
fn a_calendar_answer_with_somebody_elses_sign_in_is_refused_whole() {
    let (mut app, owner) = window();
    app.services.graph = Some(Box::new(|req: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        let (from, to) = req.calendar.unwrap_or_default();
        let events = EventsDownload {
            account: req.account.id,
            email: req.account.email.clone(),
            from,
            to,
            complete: true,
            events: vec![RemoteEvent { id: "e-9".into(), title: "Not yours".into(), start: NOW + 3_600, end: NOW + 7_200, ..Default::default() }],
            ..Default::default()
        };
        tx.send(GraphResult { account: req.account.id, events: Some(events), tokens: Some(theirs()), ..Default::default() }).unwrap();
        rx
    }));
    assert!(app.calendar_tick_at(true, 0.0), "a calendar job started");
    app.poll_calendar();
    assert!(!app.calendar_syncing());
    assert!(!app.session.mailbox.events.values().any(|e| e.title == "Not yours"), "nothing taken in");
    assert!(nothing_of_theirs_kept(&app, owner));
    assert_eq!(app.status_msg.as_ref().map(|(m, _)| m.as_str()), Some(SAID));
    let problems = app.sync_problems.iter().find(|(a, ..)| *a == owner).map(|(.., lines)| lines.len()).unwrap_or(0);
    assert!(problems >= 1, "in the account's problems too");
}

#[test]
fn a_to_do_answer_with_somebody_elses_sign_in_is_refused_whole() {
    let (mut app, owner) = window();
    app.services.graph_tasks = Some(Box::new(|job: TasksJob| {
        let (tx, rx) = mpsc::channel();
        let download = TasksDownload {
            account: job.account.id,
            email: job.account.email.clone(),
            complete: true,
            lists: vec![RemoteTaskList { id: "L1".into(), complete: true, shared: false }],
            tasks: vec![RemoteTask { id: "t9".into(), list: "L1".into(), title: "Not yours".into(), ..Default::default() }],
        };
        tx.send(TasksResult { account: job.account.id, download: Some(download), tokens: Some(theirs()), ..Default::default() }).unwrap();
        rx
    }));
    assert!(app.tasks_tick_at(0.0), "a to-do job started");
    app.poll_tasks();
    assert!(!app.tasks.busy());
    assert!(!app.session.mailbox.tasks.values().any(|t| t.title == "Not yours"), "nothing taken in");
    assert!(nothing_of_theirs_kept(&app, owner));
    assert_eq!(app.session.driver.tasks.perm.get(&owner), None, "nothing learnt from it");
    assert_eq!(app.status_msg.as_ref().map(|(m, _)| m.as_str()), Some(SAID));
}

#[test]
fn a_background_jobs_refusal_leaves_the_send_receive_words_alone() {
    // What Send/Receive says (here, that it waits for a sign-in) stays when a calendar or To Do
    // job running beside it is refused for its sign-in: the refusal is said in the status bar.
    let (mut app, owner) = window();
    let waiting = "Waiting for you to sign in…".to_string();
    app.session.sync_status = waiting.clone();
    app.services.graph_tasks = Some(Box::new(|job: TasksJob| {
        let (tx, rx) = mpsc::channel();
        tx.send(TasksResult { account: job.account.id, tokens: Some(theirs()), ..Default::default() }).unwrap();
        rx
    }));
    assert!(app.tasks_tick_at(0.0), "a to-do job started");
    app.poll_tasks();
    assert_eq!(app.status_msg.as_ref().map(|(m, _)| m.as_str()), Some(SAID));
    assert_eq!(app.session.sync_status, waiting, "a background job's refusal leaves it");
    let said = app.say_signed_in_as_other(owner, STRANGER);
    assert_eq!(said, SAID);
    assert_eq!(app.session.sync_status, waiting, "saying it alone leaves it too");
}

#[test]
fn a_sign_in_of_somebody_elses_still_ends_the_sign_ins_own_words() {
    let (mut app, owner) = window();
    app.session.sync_status = "Signing in…".into();
    assert!(app.signed_in_as_other(owner, &theirs()));
    assert!(app.session.sync_status.is_empty(), "the sign-in's progress words go");
    assert_eq!(app.status_msg.as_ref().map(|(m, _)| m.as_str()), Some(SAID));
}
