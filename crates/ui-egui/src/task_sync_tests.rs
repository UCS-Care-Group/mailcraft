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
        lists: vec![RemoteTaskList { id: "L1".into(), complete: true, shared: false }],
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
    app.execute("tasks.sendLocal", json!({"account": owner})).unwrap();
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
    let (line, again) = app.tasks_line(owner).unwrap();
    assert!(again && line.contains("waiting for your permission"), "{line}");
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

fn nothing(owner: Id) -> TasksResult {
    TasksResult { account: owner, download: Some(TasksDownload { account: owner, complete: true, ..Default::default() }), ..Default::default() }
}

/// The first look at an account with to-dos made here: the question is pending, nothing goes.
fn first_look_with_local_tasks() -> (MailApp, Id, Answers, Jobs) {
    let (mut app, owner, answers, jobs, _) = window();
    app.execute("tasks.new", json!({"title": "Write the report"})).unwrap();
    answers.lock().unwrap().push_back(nothing(owner));
    assert!(app.tasks_tick_at(0.0));
    // The permission is not known yet: nobody is asked.
    assert!(app.dialog.is_none());
    app.poll_tasks();
    assert_eq!(jobs.lock().unwrap()[0], (owner, "ref-1".to_string(), 0), "nothing local is told on the first look");
    (app, owner, answers, jobs)
}

#[test]
fn the_first_look_uploads_no_local_task_and_asks_the_owner() {
    let (mut app, owner, _, _) = first_look_with_local_tasks();
    app.tasks_tick_at(1_000.0);
    let Some(crate::dialogs::Dialog::Confirm { command, ask, .. }) = &app.dialog else { panic!("the owner is asked") };
    assert_eq!(command, "tasks.sendLocal");
    assert!(ask.title.contains("Send your 1 local task to noor@example.org"), "{}", ask.title);
    assert_eq!(ask.cancel.as_deref(), Some("Not now"), "one button that goes nowhere, not Not now and Cancel");
    assert!(ask.more.is_empty());
    assert_eq!(app.session.mailbox.local_tasks, mailcraft_model::LocalTasks::Unasked);
    // Still pending, still nothing sent.
    assert_eq!(tasks_remote_waiting(&app, owner), 0);
}

fn tasks_remote_waiting(app: &MailApp, owner: Id) -> usize {
    mailcraft_engine::cmd::tasks_remote::tasks_waiting(&app.session.mailbox, owner)
}

#[test]
fn yes_uploads_to_the_chosen_account() {
    let (mut app, owner, answers, jobs) = first_look_with_local_tasks();
    app.tasks_tick_at(1_000.0);
    let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = app.dialog.take() else { panic!("asked") };
    let with = crate::dialogs::answered(&params, &ask.with, None);
    app.run(&command, with).unwrap();
    app.tasks.offer_open = false;
    assert_eq!(tasks_remote_waiting(&app, owner), 1);
    answers.lock().unwrap().push_back(nothing(owner));
    assert!(!app.tasks_tick_at(60_000.0));
    assert!(app.tasks_tick_at(64_000.0));
    assert_eq!(jobs.lock().unwrap().last().unwrap().2, 1, "the local task goes up now");
}

#[test]
fn not_now_keeps_them_here_and_is_not_asked_on_every_sync() {
    let (mut app, owner, answers, jobs) = first_look_with_local_tasks();
    app.tasks_tick_at(1_000.0);
    assert!(matches!(&app.dialog, Some(crate::dialogs::Dialog::Confirm { ask, .. }) if ask.cancel.as_deref() == Some("Not now")));
    // Not now closes the question, which the next tick notes.
    app.dialog = None;
    app.tasks_tick_at(2_000.0);
    for n in 1..4 {
        answers.lock().unwrap().push_back(nothing(owner));
        assert!(app.tasks_tick_at(f64::from(n) * 10.0 * 60_000.0));
        app.poll_tasks();
        assert!(app.dialog.is_none(), "not asked again");
    }
    assert!(jobs.lock().unwrap().iter().all(|job| job.2 == 0), "nothing local was sent");
    // File › Info offers it again.
    assert_eq!(app.local_tasks_line().map(|l| l.1), Some(true));
    assert!(app.offer_local_tasks());
}

#[test]
fn a_question_that_goes_away_counts_as_not_now() {
    let (mut app, _, _, _) = first_look_with_local_tasks();
    app.tasks_tick_at(1_000.0);
    assert!(app.dialog.is_some());
    app.dialog = None;
    app.tasks_tick_at(2_000.0);
    assert_eq!(app.session.mailbox.local_tasks, mailcraft_model::LocalTasks::NotNow);
    assert!(app.dialog.is_none());
}

#[test]
fn a_programmatic_call_does_not_open_the_question_or_answer_it() {
    let (app, _, _, _) = first_look_with_local_tasks();
    // An agent is not let through to answer it.
    assert!(matches!(mailcraft_engine::agent::access("tasks.sendLocal"), Some(mailcraft_engine::agent::Access::Owner(_))));
    assert_eq!(app.session.mailbox.local_tasks, mailcraft_model::LocalTasks::Unasked);
}

/// Allowed on Microsoft's page, then refused for a while all the same, as a permission given a
/// moment ago can be until Microsoft has taken it in everywhere (#491): the job tries again
/// quietly, later each time, and the owner is not asked again. Refused after all of those, the
/// owner is asked.
#[test]
fn a_refusal_just_after_the_owner_allowed_tasks_is_tried_again_before_asking() {
    let (mut app, owner, answers, jobs, sign_ins) = window();
    let refused = || TasksResult { account: owner, consent_needed: true, ..Default::default() };
    answers.lock().unwrap().push_back(refused());
    assert!(app.tasks_tick_at(0.0));
    app.poll_tasks();
    app.tasks_tick_at(1.0);
    app.dialog = None;
    app.run("ui.allowTasks", json!({"account": owner, "confirmed": true})).unwrap();
    app.poll_sign_in();
    // Refused twice more, then read.
    answers.lock().unwrap().extend([refused(), refused(), nothing(owner)]);
    let mut at = 2.0;
    for _ in 0..3 {
        // A job as soon as it may run, and no question while it is tried again.
        while !app.tasks_tick_at(at) {
            assert!(app.dialog.is_none(), "asked again {at} ms after allowing");
            at += 1_000.0;
            assert!(at < 10.0 * 60_000.0, "no job was tried again");
        }
        app.poll_tasks();
        app.tasks_tick_at(at);
        assert!(app.dialog.is_none(), "asked again after a refusal right after allowing");
    }
    assert_eq!(app.tasks.perm.get(&owner), Some(&Permission::Given));
    assert_eq!(jobs.lock().unwrap().len(), 4);
    assert_eq!(sign_ins.lock().unwrap().len(), 1);
}

/// Still refused after the tries again: the owner is asked, as for any refusal.
#[test]
fn a_refusal_that_lasts_after_allowing_asks_the_owner_in_the_end() {
    let (mut app, owner, answers, _, _) = window();
    let refused = || TasksResult { account: owner, consent_needed: true, ..Default::default() };
    answers.lock().unwrap().push_back(refused());
    assert!(app.tasks_tick_at(0.0));
    app.poll_tasks();
    app.tasks_tick_at(1.0);
    app.dialog = None;
    app.run("ui.allowTasks", json!({"account": owner, "confirmed": true})).unwrap();
    app.poll_sign_in();
    answers.lock().unwrap().extend((0..10).map(|_| refused()));
    let mut at = 2.0;
    while app.dialog.is_none() && at < 30.0 * 60_000.0 {
        if app.tasks_tick_at(at) {
            app.poll_tasks();
            app.tasks_tick_at(at);
        }
        at += 1_000.0;
    }
    let Some(crate::dialogs::Dialog::Confirm { command, .. }) = &app.dialog else { panic!("the owner was never asked") };
    assert_eq!(command, "ui.allowTasks");
}

/// #520: a sign-in Microsoft no longer takes, for mail either, leaves a way back in File › Info,
/// and no job runs again until there is a new sign-in.
#[test]
fn a_refused_sign_in_for_tasks_leaves_ask_again() {
    let (mut app, owner, answers, jobs, sign_ins) = window();
    answers.lock().unwrap().push_back(TasksResult { account: owner, sign_in_refused: true, ..Default::default() });
    assert!(app.tasks_tick_at(0.0));
    app.poll_tasks();
    assert!(matches!(app.tasks.perm.get(&owner), Some(Permission::SignInOver(_))));
    let (line, again) = app.tasks_line(owner).unwrap();
    assert!(again && line.contains("no longer takes the sign-in"), "{line}");
    // Not by itself: no question, no page, no job.
    for minute in 1..60 {
        assert!(!app.tasks_tick_at(f64::from(minute) * 60_000.0));
    }
    assert!(app.dialog.is_none() && sign_ins.lock().unwrap().is_empty());
    assert_eq!(jobs.lock().unwrap().len(), 1);
    // Ask again: the question, then Microsoft's page.
    app.ask_tasks_again(owner);
    let Some(crate::dialogs::Dialog::Confirm { command, .. }) = &app.dialog else { panic!("Ask again asks") };
    assert_eq!(command, "ui.allowTasks");
    app.dialog = None;
    app.run("ui.allowTasks", json!({"account": owner, "confirmed": true})).unwrap();
    assert_eq!(*sign_ins.lock().unwrap(), [(owner, true)]);
}

/// #520: signing in again for mail is a way back too, without Ask again.
#[test]
fn a_new_sign_in_for_mail_lets_the_tasks_go_on() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(TasksResult { account: owner, sign_in_refused: true, ..Default::default() });
    assert!(app.tasks_tick_at(0.0));
    app.poll_tasks();
    assert!(!app.tasks_tick_at(10.0 * 60_000.0));
    app.tokens.insert(owner, tokens("new-access", "ref-3"));
    assert!(app.tasks_tick_at(11.0 * 60_000.0));
    assert_eq!(jobs.lock().unwrap().last().map(|j| j.1.clone()), Some("ref-3".to_string()));
    assert_eq!(app.tasks.perm.get(&owner), None);
}

/// #520: Graph refusing a token just renewed is no reason to stop: the next look tries again.
#[test]
fn a_refusal_after_the_token_was_renewed_is_tried_again() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(TasksResult {
        account: owner,
        tokens: Some(tokens("tasks-access", "ref-1")),
        sign_in_refused: true,
        ..Default::default()
    });
    assert!(app.tasks_tick_at(0.0));
    app.poll_tasks();
    assert_eq!(app.tasks.perm.get(&owner), None);
    assert!(app.tasks_tick_at(10.0 * 60_000.0));
    assert_eq!(jobs.lock().unwrap().len(), 2);
}

/// #520: a job's refusal that comes in after the owner allowed the to-dos is older than that
/// sign-in, and doesn't undo it.
#[test]
fn a_refusal_from_before_the_owner_allowed_them_is_not_taken() {
    let (mut app, owner, answers, _, _) = window();
    answers.lock().unwrap().push_back(TasksResult { account: owner, sign_in_refused: true, ..Default::default() });
    assert!(app.tasks_tick_at(0.0));
    app.run("ui.allowTasks", json!({"account": owner, "confirmed": true})).unwrap();
    app.poll_sign_in();
    app.poll_tasks();
    assert_eq!(app.tasks.perm.get(&owner), None);
}
