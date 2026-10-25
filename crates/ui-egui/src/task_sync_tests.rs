// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tasks kept in step with Microsoft To Do in the window (#248): a job when they are due, the
//! owner's permission asked for once and by itself, and what the host brings taken in. Hosts are
//! fakes; every address is invented.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList, TasksDownload};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Id, Task};
use serde_json::json;

use crate::task_sync::{Permission, TasksJob, TasksResult};
use crate::{MailApp, Services, SignInRequest};

const OWNER: &str = "noor@example.org";
const NOW: i64 = 1_791_993_600;

type Jobs = Arc<Mutex<Vec<(Id, String, usize)>>>;
type Answers = Arc<Mutex<VecDeque<TasksResult>>>;
type SignIns = Arc<Mutex<Vec<(Id, bool)>>>;

fn tokens(access: &str, refresh: &str) -> Tokens {
    Tokens { access: access.into(), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

fn window() -> (MailApp, Id, Answers, Jobs, SignIns) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let owner = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.tokens.insert(owner, tokens("own-access", "ref-1"));
    let (answers, jobs, sign_ins): (Answers, Jobs, SignIns) = Default::default();
    let (to_answer, log) = (answers.clone(), jobs.clone());
    app.services.graph_tasks = Some(Box::new(move |job: TasksJob| {
        log.lock().unwrap().push((job.account.id, job.refresh.clone(), job.upload.ops.len()));
        let (tx, rx) = mpsc::channel();
        tx.send(to_answer.lock().unwrap().pop_front().unwrap_or_default()).unwrap();
        rx
    }));
    let log = sign_ins.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        log.lock().unwrap().push((req.account, req.tasks));
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(tokens("tasks-access", "ref-2"))).unwrap();
        rx
    }));
    (app, owner, answers, jobs, sign_ins)
}

fn remote(id: &str, title: &str) -> RemoteTask {
    RemoteTask { id: id.into(), list: "L1".into(), title: title.into(), ..Default::default() }
}

#[test]
fn a_job_runs_when_the_tasks_are_due_and_brings_them_in() {
    let (mut app, owner, answers, jobs, _) = window();
    let download = TasksDownload {
        account: owner,
        email: OWNER.into(),
        complete: true,
        lists: vec![RemoteTaskList { id: "L1".into(), complete: true }],
        tasks: vec![remote("t1", "Call Bo")],
    };
    answers.lock().unwrap().push_back(TasksResult { account: owner, download: Some(download), ..Default::default() });
    assert!(app.tasks_tick_at(0.0));
    // One at a time.
    assert!(!app.tasks_tick_at(1.0));
    app.poll_tasks();
    let t: Vec<&Task> = app.session.mailbox.tasks.values().map(|t| &**t).filter(|t| t.title == "Call Bo").collect();
    assert_eq!((t.len(), t[0].origin.account, t[0].origin.id.as_str()), (1, owner, "t1"));
    assert_eq!(app.tasks.perm.get(&owner), Some(&Permission::Given));
    assert_eq!(jobs.lock().unwrap()[0], (owner, "ref-1".to_string(), 0));
    // Not again until a few minutes have gone by.
    assert!(!app.tasks_tick_at(60_000.0));
    assert!(app.tasks_tick_at(6.0 * 60_000.0));
}

#[test]
fn a_change_made_here_goes_after_a_quiet_moment() {
    let (mut app, owner, answers, jobs, _) = window();
    let nothing = || TasksResult {
        account: owner,
        download: Some(TasksDownload { account: owner, complete: true, ..Default::default() }),
        ..Default::default()
    };
    answers.lock().unwrap().push_back(nothing());
    answers.lock().unwrap().push_back(nothing());
    assert!(app.tasks_tick_at(0.0));
    app.poll_tasks();
    app.execute("tasks.new", json!({"title": "Write the report"})).unwrap();
    assert!(!app.tasks_tick_at(1_000.0));
    assert!(app.tasks_tick_at(10_000.0));
    let sent = jobs.lock().unwrap().last().cloned().unwrap();
    assert_eq!((sent.0, sent.2 >= 1), (owner, true));
}

#[test]
fn the_owner_is_asked_once_by_itself_when_the_sign_in_does_not_cover_tasks() {
    let (mut app, owner, answers, jobs, sign_ins) = window();
    answers.lock().unwrap().push_back(TasksResult { account: owner, consent_needed: true, ..Default::default() });
    assert!(app.tasks_tick_at(0.0));
    app.poll_tasks();
    // Found missing: nothing is sent or read; the window asks.
    assert_eq!(app.tasks.perm.get(&owner), Some(&Permission::Needed));
    assert!(!app.tasks_tick_at(10.0 * 60_000.0) || jobs.lock().unwrap().len() == 1);
    let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = &app.dialog else { panic!("the owner is asked first") };
    assert_eq!((command.as_str(), params["account"].as_u64()), ("ui.allowTasks", Some(owner)));
    assert!(ask.text.contains(OWNER) && ask.text.contains("stay on this computer"), "{}", ask.text);
    assert!(sign_ins.lock().unwrap().is_empty(), "no page yet");
    assert_eq!(jobs.lock().unwrap().len(), 1);
    // Not meant without a yes; nothing for an account that is not there.
    assert!(app.run("ui.allowTasks", json!({"account": owner})).is_err());
    assert!(app.run("ui.allowTasks", json!({"account": 9_999, "confirmed": true})).is_err());
    app.dialog = None;
    assert_eq!(app.run("ui.allowTasks", json!({"account": owner, "confirmed": true})).unwrap()["account"], owner);
    assert_eq!(*sign_ins.lock().unwrap(), [(owner, true)]);
    app.poll_sign_in();
    // Allowed: the next look runs a job again, and nobody is asked again.
    assert_eq!(app.tasks.perm.get(&owner), None);
    assert!(app.tasks_tick_at(11.0 * 60_000.0));
    assert_eq!(jobs.lock().unwrap().len(), 2);
    assert!(app.dialog.is_none());
}

#[test]
fn a_question_that_goes_away_is_not_asked_again_by_itself() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(TasksResult { account: owner, consent_needed: true, ..Default::default() });
    assert!(app.tasks_tick_at(0.0));
    app.poll_tasks();
    app.tasks_tick_at(1.0);
    assert!(app.dialog.is_some());
    app.dialog = None;
    assert!(!app.tasks_tick_at(10.0 * 60_000.0));
    assert!(app.dialog.is_none());
    assert_eq!(app.tasks.perm.get(&owner), Some(&Permission::Declined));
    assert_eq!(jobs.lock().unwrap().len(), 1);
    let (line, again) = app.tasks_line(owner).unwrap();
    assert!(again && line.contains("this computer only"), "{line}");
}

#[test]
fn a_sign_in_the_server_refuses_to_read_for_makes_no_changes_and_says_so() {
    let (mut app, owner, answers, _, _) = window();
    answers.lock().unwrap().push_back(TasksResult { account: owner, denied: true, ..Default::default() });
    assert!(app.tasks_tick_at(0.0));
    app.poll_tasks();
    assert!(matches!(app.tasks.perm.get(&owner), Some(Permission::Refused(_))));
    assert!(!app.tasks_tick_at(10.0 * 60_000.0));
}
