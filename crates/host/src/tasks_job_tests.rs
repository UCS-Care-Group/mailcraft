// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The To Do job in flight, for every front end: what it is handed, and its answer taken in.
//! The job is played here (nothing leaves this machine).

use std::sync::mpsc::{Receiver, channel};

use mailcraft_engine::Session;
use mailcraft_engine::sync_driver::Permission;
use mailcraft_i18n::Lang;
use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList, TasksDownload};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Id, OAuth};
use serde_json::json;

use crate::tasks::{TasksJob, TasksResult};
use crate::tasks_job::TasksJobs;

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn refresh(_: &mailcraft_model::Account) -> Option<String> {
    Some("refresh".into())
}

fn filled(mut o: OAuth) -> OAuth {
    o.client_id = "app".into();
    o
}

/// Start a job whose answer is `answer`, keeping what it was handed.
fn start(s: &mut Session, jobs: &mut TasksJobs, account: Id, answer: TasksResult) -> TasksJob {
    let mut handed = None;
    let started = jobs.start(s, account, refresh, filled, 1_000.0, |job| {
        let (tx, rx) = channel();
        let _ = tx.send(TasksResult { account: job.account.id, ..answer });
        handed = Some(job);
        Some(rx)
    });
    assert!(started);
    handed.unwrap()
}

#[test]
fn a_job_is_handed_the_sign_in_and_what_waits_and_the_engine_is_told() {
    let (mut s, a) = session();
    let mut jobs = TasksJobs::default();
    let job = start(&mut s, &mut jobs, a, TasksResult::default());
    assert_eq!((job.account.id, job.refresh.as_str(), job.oauth.client_id.as_str()), (a, "refresh", "app"));
    assert!(jobs.busy() && jobs.account() == Some(a));
    // One at a time.
    assert!(!jobs.start(&mut s, a, refresh, filled, 2_000.0, |_| panic!("not handed")));
}

#[test]
fn nothing_starts_without_a_sign_in_or_a_way_to_run_it() {
    let (mut s, a) = session();
    let mut jobs = TasksJobs::default();
    assert!(!jobs.start(&mut s, a, |_| None, filled, 1_000.0, |_| panic!("not handed")));
    assert!(!jobs.start(&mut s, a, |_| Some(String::new()), filled, 1_000.0, |_| panic!("not handed")));
    assert!(!jobs.start(&mut s, a, refresh, filled, 1_000.0, |_| None::<Receiver<TasksResult>>));
    assert!(!jobs.busy());
}

#[test]
fn a_task_that_exists_only_here_is_never_handed_to_a_job_unasked() {
    let (mut s, a) = session();
    s.run("tasks.new", &json!({"title": "Only here"})).unwrap();
    let mut jobs = TasksJobs::default();
    let job = start(&mut s, &mut jobs, a, TasksResult::default());
    assert!(job.upload.ops.is_empty(), "the owner hasn't said where it may go");
}

#[test]
fn the_answer_is_taken_in_and_the_newest_refresh_token_handed_back() {
    let (mut s, a) = session();
    let mut jobs = TasksJobs::default();
    let download = TasksDownload {
        account: a,
        email: "ada@example.org".into(),
        complete: true,
        lists: vec![RemoteTaskList { id: "l-1".into(), complete: true, shared: false }],
        tasks: vec![RemoteTask { id: "t-1".into(), list: "l-1".into(), title: "Buy stamps".into(), ..Default::default() }],
    };
    let tokens = Tokens { refresh: "newer".into(), ..Default::default() };
    start(&mut s, &mut jobs, a, TasksResult { tokens: Some(tokens), download: Some(download), ..Default::default() });
    let answer = loop {
        if let Some(answer) = jobs.poll(&mut s, Lang::English) {
            break answer;
        }
    };
    assert_eq!(answer.tokens.map(|t| t.refresh), Some("newer".to_string()));
    assert!(answer.errors.is_empty() && !answer.stopped_early && !jobs.busy());
    assert!(s.mailbox.tasks.values().any(|t| t.title == "Buy stamps"));
    assert_eq!(s.driver.tasks.perm.get(&a), Some(&Permission::Given));
}

#[test]
fn a_sign_in_that_does_not_cover_the_to_dos_is_noted_for_the_owner_to_be_asked() {
    let (mut s, a) = session();
    let mut jobs = TasksJobs::default();
    start(&mut s, &mut jobs, a, TasksResult { consent_needed: true, ..Default::default() });
    let answer = loop {
        if let Some(answer) = jobs.poll(&mut s, Lang::English) {
            break answer;
        }
    };
    assert!(answer.consent_needed);
    assert_eq!(s.driver.tasks.to_ask(), Some(a));
}

#[test]
fn a_job_that_ends_without_an_answer_is_said_and_counted_a_failure() {
    let (mut s, a) = session();
    let mut jobs = TasksJobs::default();
    assert!(jobs.start(&mut s, a, refresh, filled, 1_000.0, |_| Some(channel().1)));
    let answer = jobs.poll(&mut s, Lang::English).unwrap();
    assert!(answer.stopped_early && answer.tokens.is_none());
    assert_eq!(s.driver.tasks.perm.get(&a), None);
}

#[test]
fn an_answer_for_a_mailbox_that_is_no_longer_open_is_let_go_of() {
    let (mut s, a) = session();
    let mut jobs = TasksJobs::default();
    start(&mut s, &mut jobs, a, TasksResult { consent_needed: true, ..Default::default() });
    // The same file opened again: its ids may name other accounts now.
    let mailbox = s.mailbox.clone();
    s.replace_mailbox(mailbox);
    assert!(jobs.stale(&s));
    assert!(jobs.poll(&mut s, Lang::English).is_none());
    assert!(!jobs.busy());
    assert_eq!(s.driver.tasks.to_ask(), None);
}

#[test]
fn an_answer_with_somebody_elses_sign_in_is_refused_whole() {
    let (mut s, a) = session();
    let mut jobs = TasksJobs::default();
    let download = TasksDownload {
        account: a,
        email: "ada@example.org".into(),
        complete: true,
        lists: vec![RemoteTaskList { id: "l-1".into(), complete: true, shared: false }],
        tasks: vec![RemoteTask { id: "t-9".into(), list: "l-1".into(), title: "Not yours".into(), ..Default::default() }],
    };
    let theirs = Tokens { refresh: "theirs".into(), email: "mallory@example.net".into(), ..Default::default() };
    let result = TasksResult { tokens: Some(theirs), download: Some(download), consent_needed: true, ..Default::default() };
    start(&mut s, &mut jobs, a, result);
    let answer = loop {
        if let Some(answer) = jobs.poll(&mut s, Lang::English) {
            break answer;
        }
    };
    assert_eq!(answer.foreign.as_ref().map(|f| f.signed_in_as.as_str()), Some("mallory@example.net"));
    assert!(answer.tokens.is_none() && !answer.consent_needed, "nothing of it handed back");
    assert!(!format!("{answer:?}").contains("mallory"));
    assert!(!s.mailbox.tasks.values().any(|t| t.title == "Not yours"), "nothing taken in");
    // Neither given nor asked about on its word.
    assert_eq!(s.driver.tasks.perm.get(&a), None);
    assert_eq!(s.driver.tasks.to_ask(), None);
}
