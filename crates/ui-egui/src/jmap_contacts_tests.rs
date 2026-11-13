// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's side of a JMAP account's contacts, with a host that keeps every job for the test
//! to answer: the contacts taken in after a Send/Receive, an answer for another address or server
//! let go of, a server that offers none not asked again, no job without a secret, a secret turned
//! down, a job that came to nothing tried again later, and another mailbox taking the place of
//! the one a job was for.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::remote::{ContactsDownload, RemoteContact};
use mailcraft_model::sync::JmapMailResult;
use mailcraft_model::{Labeled, Mailbox};
use serde_json::json;

use super::*;
use crate::Services;
use crate::jmap_mail::{JmapContactsJob, JmapJob};
use crate::jmap_side_job::FIRST_RETRY_MS;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const SESSION: &str = "https://jmap.example.org/jmap/session";
const MINUTE: f64 = 60_000.0;

type Kept<J, R> = Arc<Mutex<Vec<(J, mpsc::Sender<R>)>>>;
type Jobs = Kept<JmapContactsJob, JmapContactsResult>;
type Runs = Kept<JmapJob, JmapMailResult>;

/// A window with a JMAP account at `ada@example.org`, and a host that keeps every contacts job and
/// every Send/Receive. Nothing starts by itself unless a test says so.
fn window() -> (MailApp, Id, Jobs, Runs) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    let (jobs, runs): (Jobs, Runs) = Default::default();
    let kept = jobs.clone();
    app.services.jmap_contacts = Some(Box::new(move |job: JmapContactsJob| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push((job, tx));
        rx
    }));
    let kept = runs.clone();
    app.services.jmap = Some(Box::new(move |job: JmapJob| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push((job, tx));
        rx
    }));
    let added = app.execute("account.add", json!({"email": "ada@example.org", "kind": "jmap", "jmap": {"session": SESSION}})).unwrap();
    let ada = added["id"].as_u64().unwrap();
    (app, ada, jobs, runs)
}

/// The one job the host was handed since the last look.
fn one<J, R>(kept: &Kept<J, R>) -> (J, mpsc::Sender<R>) {
    let mut all = std::mem::take(&mut *kept.lock().unwrap());
    assert_eq!(all.len(), 1);
    all.pop().unwrap()
}

fn none<J, R>(kept: &Kept<J, R>) -> bool {
    kept.lock().unwrap().is_empty()
}

/// An answer to `job`, as the host makes it.
fn answer(job: &JmapContactsJob, how: JmapContactsResult) -> JmapContactsResult {
    JmapContactsResult { account: job.request.account, email: job.request.email.clone(), session: job.request.session.clone(), ..how }
}

/// Zoe, as the server has her.
fn zoe(job: &JmapContactsJob) -> Option<ContactsDownload> {
    let contact = RemoteContact {
        id: "C1".into(),
        display_name: "Zoe Example".into(),
        first: "Zoe".into(),
        last: "Example".into(),
        emails: vec![Labeled::new("work", "zoe@example.org")],
        ..Default::default()
    };
    Some(ContactsDownload { account: job.request.account, email: job.request.email.clone(), complete: true, contacts: vec![contact] })
}

fn has_zoe(app: &MailApp, account: Id) -> bool {
    app.session.mailbox.contacts.values().any(|c| c.origin.account == account && c.origin.id == "C1" && c.display_name == "Zoe Example")
}

/// A Send/Receive of `account` the owner asked for, from its start to its end.
fn send_receive(app: &mut MailApp, account: Id, runs: &Runs) {
    app.execute("ui.sync", json!({"account": account, "password": "typed here"})).unwrap();
    let (run, tx) = one(runs);
    // Under way: the contacts wait for it.
    assert!(!app.jmap_contacts_tick_at(0.0));
    tx.send(JmapMailResult { account, email: run.request.email.clone(), whole: true, ..Default::default() }).unwrap();
    app.poll_sync();
    assert!(!app.syncing());
}

#[test]
fn the_contacts_follow_a_send_receive_and_land_in_the_mailbox_as_the_account_s() {
    let (mut app, ada, jobs, runs) = window();
    // Nothing by itself while Send/Receive doesn't run by itself.
    app.passwords.insert(ada, "typed here".into());
    assert!(!app.jmap_contacts_tick_at(0.0) && none(&jobs));
    send_receive(&mut app, ada, &runs);
    assert!(app.jmap_contacts_tick_at(1_000.0));
    assert!(app.jmap_contacts_syncing());
    let (job, tx) = one(&jobs);
    assert_eq!((job.secret.as_str(), job.request.email.as_str(), job.request.session.as_str()), ("typed here", "ada@example.org", SESSION));
    assert!(job.request.account == ada && !job.request.bearer && !job.cancel.load(Ordering::Relaxed));
    // One at a time.
    assert!(!app.jmap_contacts_tick_at(2_000.0) && none(&jobs));
    tx.send(answer(&job, JmapContactsResult { contacts: zoe(&job), ..Default::default() })).unwrap();
    app.poll_jmap_contacts_at(3_000.0);
    assert!(!app.jmap_contacts_syncing());
    assert!(has_zoe(&app, ada));
    assert!(!app.has_sync_problems());
    // Not again until the owner asks for a Send/Receive or, with rounds by themselves, a quarter of
    // an hour passes.
    assert!(!app.jmap_contacts_tick_at(4_000.0));
    app.auto_sync = true;
    assert!(!app.jmap_contacts_tick_at(14.0 * MINUTE));
    assert!(app.jmap_contacts_tick_at(16.0 * MINUTE));
}

#[test]
fn an_answer_for_another_address_or_server_is_not_taken_in() {
    let (mut app, ada, jobs, _runs) = window();
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    // For another address.
    assert!(app.jmap_contacts_tick_at(0.0));
    let (job, tx) = one(&jobs);
    tx.send(JmapContactsResult { email: "bo@example.org".into(), contacts: zoe(&job), ..answer(&job, Default::default()) }).unwrap();
    app.poll_jmap_contacts_at(1_000.0);
    assert!(!app.jmap_contacts_syncing() && !has_zoe(&app, ada));
    // For another server.
    assert!(app.jmap_contacts_tick_at(16.0 * MINUTE));
    let (job, tx) = one(&jobs);
    let other = "https://jmap.example.org/other/session".to_string();
    tx.send(JmapContactsResult { session: other, contacts: zoe(&job), ..answer(&job, Default::default()) }).unwrap();
    app.poll_jmap_contacts_at(17.0 * MINUTE);
    assert!(!has_zoe(&app, ada));
    // The account names another server by the time the answer comes.
    assert!(app.jmap_contacts_tick_at(32.0 * MINUTE));
    let (job, tx) = one(&jobs);
    app.execute("account.edit", json!({"id": ada, "jmap": {"session": "https://jmap.example.org/other/session"}})).unwrap();
    tx.send(answer(&job, JmapContactsResult { contacts: zoe(&job), ..Default::default() })).unwrap();
    app.poll_jmap_contacts_at(33.0 * MINUTE);
    assert!(!app.jmap_contacts_syncing() && !has_zoe(&app, ada));
    assert!(app.session.mailbox.contacts.values().all(|c| c.origin.account != ada));
}

#[test]
fn a_server_that_offers_no_contacts_is_not_asked_again_until_the_owner_asks() {
    let (mut app, ada, jobs, runs) = window();
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    assert!(app.jmap_contacts_tick_at(0.0));
    let (job, tx) = one(&jobs);
    tx.send(answer(&job, JmapContactsResult { not_offered: true, ..Default::default() })).unwrap();
    app.poll_jmap_contacts_at(1_000.0);
    assert!(!app.has_sync_problems(), "nothing is wrong");
    for minutes in [1.0, 16.0, 600.0] {
        assert!(!app.jmap_contacts_tick_at(minutes * MINUTE), "{minutes}");
    }
    assert!(none(&jobs));
    // The owner's own Send/Receive asks once more.
    app.auto_sync = false;
    send_receive(&mut app, ada, &runs);
    assert!(app.jmap_contacts_tick_at(601.0 * MINUTE));
    assert_eq!(one(&jobs).0.request.account, ada);
}

#[test]
fn no_job_goes_without_a_secret_and_a_host_s_own_credential_goes_as_none() {
    let (mut app, ada, jobs, _runs) = window();
    app.auto_sync = true;
    assert!(!app.jmap_contacts_tick_at(0.0) && none(&jobs));
    assert!(app.dialog.is_none(), "and nobody is asked for one");
    app.services.jmap_host_credentials = Some(Box::new(|a: &mailcraft_model::Account| a.email == "ada@example.org"));
    assert!(app.jmap_contacts_tick_at(1_000.0));
    let (job, _tx) = one(&jobs);
    assert!(job.request.account == ada && job.secret.is_empty());
}

#[test]
fn a_secret_turned_down_is_forgotten_and_nothing_is_asked_until_the_owner_s_send_receive() {
    let (mut app, ada, jobs, _runs) = window();
    app.passwords.insert(ada, "not the one".into());
    app.auto_sync = true;
    assert!(app.jmap_contacts_tick_at(0.0));
    let (job, tx) = one(&jobs);
    tx.send(answer(&job, JmapContactsResult { auth_refused: true, contacts: zoe(&job), ..Default::default() })).unwrap();
    app.poll_jmap_contacts_at(1_000.0);
    assert!(app.jmap_refused.contains(&ada) && !app.passwords.contains_key(&ada));
    assert!(!has_zoe(&app, ada));
    // Even with a secret at hand again, nothing by itself.
    app.passwords.insert(ada, "typed again".into());
    assert!(!app.jmap_contacts_tick_at(60.0 * MINUTE) && none(&jobs));
    // The owner's own Send/Receive asks again.
    app.passwords.remove(&ada);
    app.auto_sync = false;
    assert_eq!(app.execute("ui.sync", json!({"account": ada})).unwrap()["pending"], "password");
}

#[test]
fn a_job_that_came_to_nothing_is_told_and_tried_again_later_and_a_busy_server_is_left_alone() {
    let (mut app, ada, jobs, _runs) = window();
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    assert!(app.jmap_contacts_tick_at(0.0));
    let (job, tx) = one(&jobs);
    tx.send(answer(&job, JmapContactsResult { errors: vec!["Couldn't read the contacts".into()], ..Default::default() })).unwrap();
    app.poll_jmap_contacts_at(0.0);
    assert!(app.has_sync_problems());
    assert_eq!(app.account_problem_lines(ada).len(), 1);
    // A Send/Receive that ends meanwhile doesn't bring it forward.
    app.jmap_contacts.after_run(ada);
    assert!(!app.jmap_contacts_tick_at(FIRST_RETRY_MS - 1.0));
    assert!(app.jmap_contacts_tick_at(FIRST_RETRY_MS));
    // The task is gone without an answer: twice as long.
    drop(one(&jobs));
    app.poll_jmap_contacts_at(FIRST_RETRY_MS);
    app.jmap_contacts.after_run(ada);
    assert!(!app.jmap_contacts_tick_at(3.0 * FIRST_RETRY_MS - 1.0));
    assert!(app.jmap_contacts_tick_at(3.0 * FIRST_RETRY_MS));
    // Busy: for as long as the server says, within reason.
    let (job, tx) = one(&jobs);
    tx.send(answer(&job, JmapContactsResult { busy_for: Some(120), ..Default::default() })).unwrap();
    let at = 3.0 * FIRST_RETRY_MS;
    app.poll_jmap_contacts_at(at);
    app.jmap_contacts.after_run(ada);
    assert!(!app.jmap_contacts_tick_at(at + 119_000.0));
    assert!(app.jmap_contacts_tick_at(at + 120_000.0));
}

#[test]
fn a_job_under_way_when_another_mailbox_is_opened_is_called_off_and_brings_nothing_into_it() {
    let (mut app, ada, jobs, _runs) = window();
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    assert!(app.jmap_contacts_tick_at(0.0));
    let (job, tx) = one(&jobs);
    // Another mailbox takes its place, with an account at the same address on the same server.
    app.execute("file.new", json!({})).unwrap();
    let other = json!({"email": "ada@example.org", "kind": "jmap", "jmap": {"session": SESSION}});
    let other = app.execute("account.add", other).unwrap()["id"].as_u64().unwrap();
    let _ = tx.send(answer(&job, JmapContactsResult { contacts: zoe(&job), ..Default::default() }));
    app.poll_jmap_contacts_at(1_000.0);
    assert!(job.cancel.load(Ordering::Relaxed));
    assert!(!app.jmap_contacts_syncing());
    assert!(app.session.mailbox.contacts.values().all(|c| c.origin.id != "C1"), "nothing of the old job is here");
    // The new mailbox's account goes its own way: no secret was typed for it, so nothing is asked.
    assert!(!app.jmap_contacts_tick_at(2_000.0) && none(&jobs));
    assert!(!app.jmap_refused.contains(&other));
}

#[test]
fn a_mailbox_from_a_file_has_no_contacts_read_until_its_servers_are_confirmed() {
    let (mut app, ada, jobs, _runs) = window();
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    app.session.path = Some(std::path::PathBuf::from("from-a-file.mailcraft"));
    app.judge_mailbox();
    assert!(app.services.jmap_contacts.is_none());
    assert!(!app.jmap_contacts_tick_at(0.0) && none(&jobs));
    app.execute("ui.confirmServers", json!({})).unwrap();
    assert!(app.jmap_contacts_tick_at(1_000.0));
}
