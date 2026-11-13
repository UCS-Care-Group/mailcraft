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
use mailcraft_model::remote::{ContactField, ContactOpKind, ContactOutcome, ContactResult, ContactsDownload, RemoteContact};
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

/// The first reading of `account`'s contacts, with rounds by themselves on: Zoe is here as the
/// server has her, and nothing waits. The time it ended at.
fn zoe_read(app: &mut MailApp, account: Id, jobs: &Jobs) -> f64 {
    app.passwords.insert(account, "typed here".into());
    app.auto_sync = true;
    assert!(app.jmap_contacts_tick_at(0.0));
    let (job, tx) = one(jobs);
    assert!(job.request.changes.is_empty(), "nothing was changed here yet");
    tx.send(answer(&job, JmapContactsResult { contacts: zoe(&job), ..Default::default() })).unwrap();
    app.poll_jmap_contacts_at(1_000.0);
    assert!(has_zoe(app, account) && !app.jmap_contacts_tick_at(1_500.0));
    1_500.0
}

fn zoe_here(app: &MailApp, account: Id) -> mailcraft_model::Contact {
    app.session.mailbox.contacts.values().find(|c| c.origin.account == account && c.origin.id == "C1").map(|c| c.as_ref().clone()).unwrap()
}

fn waiting(app: &MailApp, account: Id) -> usize {
    contacts_remote::contacts_waiting(&app.session.mailbox, account)
}

/// `job`'s changes, each answered with `result`; a creation that is done is given the id `made`.
fn outcomes(job: &JmapContactsJob, result: ContactResult, made: &str) -> Vec<ContactOutcome> {
    let of = |op: &mailcraft_model::remote::ContactOp| ContactOutcome {
        kind: op.kind,
        contact: op.contact,
        id: if op.kind == ContactOpKind::Create && result == ContactResult::Done { made.to_string() } else { op.id.clone() },
        result,
        changed: if result == ContactResult::Done { op.changed.clone() } else { Vec::new() },
        error: String::new(),
    };
    job.request.changes.iter().map(of).collect()
}

#[test]
fn a_change_made_here_goes_soon_and_what_the_server_said_is_taken_in_before_what_it_has() {
    let (mut app, ada, jobs, _runs) = window();
    let t = zoe_read(&mut app, ada, &jobs);
    let zoe_id = zoe_here(&app, ada).id;
    app.execute("people.edit", json!({"id": zoe_id, "notes": "Met in May."})).unwrap();
    let made = app.execute("people.new", json!({"first": "Dee", "email": "dee@example.org", "account": ada})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(waiting(&app, ada), 2);
    // A moment of quiet first, then it goes: not at the next quarter of an hour.
    assert!(!app.jmap_contacts_tick_at(t) && !app.jmap_contacts_tick_at(t + QUIET_MS - 1.0));
    assert!(app.jmap_contacts_tick_at(t + QUIET_MS));
    let (job, tx) = one(&jobs);
    let kinds: Vec<(ContactOpKind, Id, &str)> = job.request.changes.iter().map(|op| (op.kind, op.contact, op.id.as_str())).collect();
    assert_eq!(kinds, [(ContactOpKind::Update, zoe_id, "C1"), (ContactOpKind::Create, made, "")]);
    assert_eq!((&job.request.changes[0].changed, job.request.changes[0].card.notes.as_str()), (&vec![ContactField::Notes], "Met in May."));
    assert_eq!(job.request.changes[1].card.first, "Dee");
    // One at a time, whatever is changed meanwhile.
    assert!(!app.jmap_contacts_tick_at(t + 10.0 * QUIET_MS) && none(&jobs));
    // The server took both, and what it has now was read after: Dee under the id it gave her,
    // with a nickname of the server's own. Were the contacts taken in first, she would be here twice.
    let mut download = zoe(&job).unwrap();
    download.contacts[0].notes = "Met in May.".into();
    download.contacts.push(RemoteContact {
        id: "C2".into(),
        first: "Dee".into(),
        nickname: "Dee from the server".into(),
        emails: vec![Labeled::new("work", "dee@example.org")],
        ..Default::default()
    });
    let done = outcomes(&job, ContactResult::Done, "C2");
    tx.send(answer(&job, JmapContactsResult { contacts: Some(download), changed: done, ..Default::default() })).unwrap();
    let at = t + 11.0 * QUIET_MS;
    app.poll_jmap_contacts_at(at);
    let dee = app.session.mailbox.contact(made).unwrap();
    assert_eq!((dee.origin.account, dee.origin.id.as_str(), dee.nickname.as_str()), (ada, "C2", "Dee from the server"));
    assert_eq!(app.session.mailbox.contacts.values().filter(|c| c.origin.account == ada).count(), 2, "nobody twice");
    assert_eq!((zoe_here(&app, ada).notes.as_str(), waiting(&app, ada)), ("Met in May.", 0));
    assert!(!app.has_sync_problems());
    // Nothing is left to tell: nothing goes until the next quarter of an hour.
    assert!(!app.jmap_contacts_tick_at(at + 10.0 * QUIET_MS) && none(&jobs));
}

#[test]
fn a_deletion_goes_with_the_job_and_can_no_longer_be_undone() {
    let (mut app, ada, jobs, _runs) = window();
    let t = zoe_read(&mut app, ada, &jobs);
    app.execute("people.delete", json!({"id": zoe_here(&app, ada).id})).unwrap();
    assert!(app.session.can_undo());
    assert!(!app.jmap_contacts_tick_at(t));
    assert!(app.jmap_contacts_tick_at(t + QUIET_MS));
    let (job, tx) = one(&jobs);
    assert_eq!((job.request.changes.len(), job.request.changes[0].kind, job.request.changes[0].id.as_str()), (1, ContactOpKind::Delete, "C1"));
    assert!(!app.session.can_undo(), "the server is being told: the contact would come back here alone");
    // The server deleted her, and no longer shows her.
    let empty = ContactsDownload { contacts: Vec::new(), ..zoe(&job).unwrap() };
    let done = outcomes(&job, ContactResult::Done, "");
    tx.send(answer(&job, JmapContactsResult { contacts: Some(empty), changed: done, ..Default::default() })).unwrap();
    app.poll_jmap_contacts_at(t + 2.0 * QUIET_MS);
    assert!(!has_zoe(&app, ada) && app.session.mailbox.contact_gone.is_empty() && waiting(&app, ada) == 0);
}

#[test]
fn what_the_server_refuses_is_said_to_the_owner_put_back_and_not_sent_again() {
    let (mut app, ada, jobs, _runs) = window();
    let t = zoe_read(&mut app, ada, &jobs);
    app.execute("people.edit", json!({"id": zoe_here(&app, ada).id, "notes": "Too much for it."})).unwrap();
    assert!(!app.jmap_contacts_tick_at(t));
    assert!(app.jmap_contacts_tick_at(t + QUIET_MS));
    let (job, tx) = one(&jobs);
    let mut refused = outcomes(&job, ContactResult::Refused, "");
    refused[0].error = "invalidProperties".into();
    tx.send(answer(&job, JmapContactsResult { contacts: zoe(&job), changed: refused, ..Default::default() })).unwrap();
    let at = t + 2.0 * QUIET_MS;
    app.poll_jmap_contacts_at(at);
    assert!(app.has_sync_problems());
    let lines = app.account_problem_lines(ada);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("didn't take a change to the contact \"Zoe Example\"") && lines[0].contains("invalidProperties"), "{lines:?}");
    // As the server has it again, and nothing more to tell: no job by itself, and no failure
    // either (the reading itself went well).
    assert!(zoe_here(&app, ada).notes.is_empty() && waiting(&app, ada) == 0);
    assert!(!app.jmap_contacts_tick_at(at + 20.0 * QUIET_MS) && !app.jmap_contacts_tick_at(at + 10.0 * MINUTE));
    assert!(none(&jobs));
}

#[test]
fn a_change_goes_by_itself_only_with_rounds_on_a_secret_and_a_server_that_takes_it() {
    let (mut app, ada, jobs, runs) = window();
    let t = zoe_read(&mut app, ada, &jobs);
    app.execute("people.edit", json!({"id": zoe_here(&app, ada).id, "notes": "Changed here."})).unwrap();
    let later = t + 100.0 * QUIET_MS;
    // Send/Receive doesn't run by itself: neither does this.
    app.auto_sync = false;
    assert!(!app.jmap_contacts_tick_at(later) && !app.jmap_contacts_tick_at(later + QUIET_MS) && none(&jobs));
    app.auto_sync = true;
    // No secret at hand: no job, and nobody is asked for one.
    app.passwords.remove(&ada);
    assert!(!app.jmap_contacts_tick_at(later) && !app.jmap_contacts_tick_at(later + 2.0 * QUIET_MS) && none(&jobs));
    assert!(app.dialog.is_none());
    // The server turned the secret down: nothing by itself, with a secret at hand again too.
    app.passwords.insert(ada, "typed again".into());
    app.jmap_refused.insert(ada);
    assert!(!app.jmap_contacts_tick_at(later + 3.0 * QUIET_MS) && none(&jobs));
    // The owner's own Send/Receive: the change goes with the contacts job that follows it, rounds
    // by themselves or not.
    app.auto_sync = false;
    app.execute("ui.sync", json!({"account": ada})).unwrap();
    let (run, tx) = one(&runs);
    assert!(!app.jmap_contacts_tick_at(later + 4.0 * QUIET_MS), "its Send/Receive goes first");
    tx.send(JmapMailResult { account: ada, email: run.request.email.clone(), whole: true, ..Default::default() }).unwrap();
    app.poll_sync();
    assert!(!app.jmap_refused.contains(&ada));
    assert!(app.jmap_contacts_tick_at(later + 5.0 * QUIET_MS));
    let (job, _tx) = one(&jobs);
    assert_eq!((job.request.changes.len(), job.request.changes[0].card.notes.as_str()), (1, "Changed here."));
}

#[test]
fn a_job_that_gets_no_change_through_is_tried_later_each_time_and_a_failed_one_waits_its_turn() {
    let (mut app, ada, jobs, _runs) = window();
    let t = zoe_read(&mut app, ada, &jobs);
    app.execute("people.edit", json!({"id": zoe_here(&app, ada).id, "notes": "Changed here."})).unwrap();
    assert!(!app.jmap_contacts_tick_at(t));
    assert!(app.jmap_contacts_tick_at(t + QUIET_MS));
    // The answer says nothing of the change (the call for it failed): it is worked out again,
    // and goes again, but not at once.
    let (job, tx) = one(&jobs);
    tx.send(answer(&job, JmapContactsResult { contacts: zoe(&job), ..Default::default() })).unwrap();
    let at = t + 2.0 * QUIET_MS;
    app.poll_jmap_contacts_at(at);
    assert_eq!((zoe_here(&app, ada).notes.as_str(), waiting(&app, ada)), ("Changed here.", 1), "the download didn't take it back");
    assert!(!app.jmap_contacts_tick_at(at + 2.0 * QUIET_MS - 1.0));
    assert!(app.jmap_contacts_tick_at(at + 2.0 * QUIET_MS));
    let (job, tx) = one(&jobs);
    assert_eq!(job.request.changes.len(), 1);
    // It couldn't be asked: nothing changes, and twice as long again.
    let failed = outcomes(&job, ContactResult::Failed, "");
    tx.send(answer(&job, JmapContactsResult { contacts: zoe(&job), changed: failed, ..Default::default() })).unwrap();
    let at = at + 3.0 * QUIET_MS;
    app.poll_jmap_contacts_at(at);
    assert_eq!(waiting(&app, ada), 1);
    assert!(!app.jmap_contacts_tick_at(at + 4.0 * QUIET_MS - 1.0));
    assert!(app.jmap_contacts_tick_at(at + 4.0 * QUIET_MS));
    // A job that came to nothing at all leaves the server alone as ever, changes waiting or not.
    let (job, tx) = one(&jobs);
    tx.send(answer(&job, JmapContactsResult { errors: vec!["Couldn't reach the server".into()], ..Default::default() })).unwrap();
    let at = at + 5.0 * QUIET_MS;
    app.poll_jmap_contacts_at(at);
    assert!(!app.jmap_contacts_tick_at(at + FIRST_RETRY_MS - 1.0));
    assert!(app.jmap_contacts_tick_at(at + FIRST_RETRY_MS));
    // Through at last: the next change goes after the usual moment again.
    let (job, tx) = one(&jobs);
    let mut download = zoe(&job).unwrap();
    download.contacts[0].notes = "Changed here.".into();
    let done = outcomes(&job, ContactResult::Done, "");
    tx.send(answer(&job, JmapContactsResult { contacts: Some(download), changed: done, ..Default::default() })).unwrap();
    let at = at + FIRST_RETRY_MS + 1_000.0;
    app.poll_jmap_contacts_at(at);
    assert_eq!(waiting(&app, ada), 0);
    app.execute("people.edit", json!({"id": zoe_here(&app, ada).id, "notes": "Changed again."})).unwrap();
    assert!(!app.jmap_contacts_tick_at(at + 1.0));
    assert!(app.jmap_contacts_tick_at(at + 1.0 + QUIET_MS));
}

#[test]
fn a_contact_of_an_account_that_writes_is_changed_in_people_and_its_deletion_is_asked_about() {
    let (mut app, ada, jobs, _runs) = window();
    zoe_read(&mut app, ada, &jobs);
    let zoe = zoe_here(&app, ada);
    assert!(mailcraft_engine::cmd::remote::keeper(&app.session.mailbox, &zoe.origin).is_some_and(|a| a.writes_contacts()));
    // Deleting one asks first, as for any contact: nothing goes before the owner says yes.
    app.execute("people.select", json!({"id": zoe.id})).unwrap();
    app.run("people.delete", json!({"id": zoe.id})).unwrap();
    assert!(has_zoe(&app, ada) && app.dialog.is_some(), "asked about first");
    assert!(app.session.mailbox.contact_gone.is_empty());
}
