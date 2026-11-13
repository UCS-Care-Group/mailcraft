// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's side of a JMAP account's calendars, with a host that keeps every job for the test
//! to answer: the calendars and their events taken in after a Send/Receive and only read here, the
//! stretch that is asked for, an answer for another address or server let go of, a server that
//! offers none not asked again, no job without a secret, a secret turned down, a job that came to
//! nothing tried again later, the calendar coming to the front, the contacts going first, another
//! mailbox taking the place of the one a job was for, and Microsoft's calendar job never started
//! for a JMAP account.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::remote::{CalendarsDownload, EventsDownload, RemoteCalendar, RemoteEvent};
use mailcraft_model::sync::{JmapContactsResult, JmapMailResult, Tokens};
use mailcraft_model::{Mailbox, Rights};
use serde_json::json;

use super::*;
use crate::jmap_mail::{JmapCalendarJob, JmapContactsJob, JmapJob};
use crate::jmap_side_job::FIRST_RETRY_MS;
use crate::{GraphRequest, GraphResult, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const SESSION: &str = "https://jmap.example.org/jmap/session";
const MINUTE: f64 = 60_000.0;

type Kept<J, R> = Arc<Mutex<Vec<(J, mpsc::Sender<R>)>>>;
type Jobs = Kept<JmapCalendarJob, JmapCalendarResult>;
type Runs = Kept<JmapJob, JmapMailResult>;

/// A window with a JMAP account at `ada@example.org`, and a host that keeps every calendar job and
/// every Send/Receive. Nothing starts by itself unless a test says so.
fn window() -> (MailApp, Id, Jobs, Runs) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    let (jobs, runs): (Jobs, Runs) = Default::default();
    let kept = jobs.clone();
    app.services.jmap_calendar = Some(Box::new(move |job: JmapCalendarJob| {
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
fn answer(job: &JmapCalendarJob, how: JmapCalendarResult) -> JmapCalendarResult {
    JmapCalendarResult { account: job.request.account, email: job.request.email.clone(), session: job.request.session.clone(), ..how }
}

/// The account's one calendar, as the server lists it: its main one, read only.
fn list(job: &JmapCalendarJob) -> Option<CalendarsDownload> {
    let calendar = RemoteCalendar { id: "K1".into(), name: "Personal".into(), rights: Rights::Read, main: true, ..Default::default() };
    Some(CalendarsDownload {
        account: job.request.account,
        email: job.request.email.clone(),
        complete: true,
        calendars: vec![calendar],
        ..Default::default()
    })
}

/// The one event of that calendar: a review tomorrow, for an hour.
fn review(job: &JmapCalendarJob) -> Vec<EventsDownload> {
    let event = RemoteEvent { id: "E1".into(), title: "Budget review".into(), start: NOW + 86_400, end: NOW + 90_000, ..Default::default() };
    vec![EventsDownload {
        account: job.request.account,
        email: job.request.email.clone(),
        calendar: String::new(),
        from: job.request.from,
        to: job.request.to,
        complete: true,
        events: vec![event],
        ..Default::default()
    }]
}

/// The calendar and its event, as the server has them.
fn all(job: &JmapCalendarJob) -> JmapCalendarResult {
    answer(job, JmapCalendarResult { calendars: list(job), events: review(job), ..Default::default() })
}

/// The id here of the review in `account`'s calendar, when it is there.
fn the_review(app: &MailApp, account: Id) -> Option<Id> {
    let found = app.session.mailbox.events.values().find(|e| e.origin.account == account && e.origin.id == "E1" && e.title == "Budget review")?;
    Some(found.id)
}

fn has_calendar(app: &MailApp, account: Id) -> bool {
    app.session.mailbox.calendars.iter().any(|c| c.origin.account == account)
}

/// A Send/Receive of `account` the owner asked for, from its start to its end.
fn send_receive(app: &mut MailApp, account: Id, runs: &Runs) {
    app.execute("ui.sync", json!({"account": account, "password": "typed here"})).unwrap();
    let (run, tx) = one(runs);
    // Under way: the calendars wait for it.
    assert!(!app.jmap_calendar_tick_at(0.0));
    tx.send(JmapMailResult { account, email: run.request.email.clone(), whole: true, ..Default::default() }).unwrap();
    app.poll_sync();
    assert!(!app.syncing());
}

#[test]
fn the_calendars_follow_a_send_receive_and_are_only_read_here() {
    let (mut app, ada, jobs, runs) = window();
    // Nothing by itself while Send/Receive doesn't run by itself.
    app.passwords.insert(ada, "typed here".into());
    assert!(!app.jmap_calendar_tick_at(0.0) && none(&jobs));
    send_receive(&mut app, ada, &runs);
    assert!(app.jmap_calendar_tick_at(1_000.0));
    assert!(app.jmap_calendar_syncing());
    let (job, tx) = one(&jobs);
    assert_eq!((job.secret.as_str(), job.request.email.as_str(), job.request.session.as_str()), ("typed here", "ada@example.org", SESSION));
    assert!(job.request.account == ada && !job.request.bearer && !job.cancel.load(Ordering::Relaxed));
    // A month back, a year ahead, by the session's clock.
    assert_eq!((job.request.from, job.request.to), (NOW - 31 * 86_400, NOW + 366 * 86_400));
    // One at a time.
    assert!(!app.jmap_calendar_tick_at(2_000.0) && none(&jobs));
    tx.send(all(&job)).unwrap();
    app.poll_jmap_calendar_at(3_000.0);
    assert!(!app.jmap_calendar_syncing());
    assert!(!app.has_sync_problems());
    let calendar = app.session.mailbox.calendars.iter().find(|c| c.origin.account == ada).expect("the account's calendar").clone();
    assert!(!calendar.can_edit() && calendar.origin.id.is_empty());
    let event = the_review(&app, ada).expect("the event");
    assert_eq!(app.session.mailbox.events.get(&event).map(|e| e.calendar), Some(calendar.id));
    // What the server keeps is not changed here: nothing would take the change to it.
    assert!(app.execute("calendar.edit", json!({"id": event, "title": "Something else"})).is_err());
    assert!(app.execute("calendar.delete", json!({"id": event})).is_err());
    assert_eq!(the_review(&app, ada), Some(event));
    // Not again until the owner asks for a Send/Receive or, with rounds by themselves, a quarter of
    // an hour passes.
    assert!(!app.jmap_calendar_tick_at(4_000.0));
    app.auto_sync = true;
    assert!(!app.jmap_calendar_tick_at(14.0 * MINUTE));
    assert!(app.jmap_calendar_tick_at(16.0 * MINUTE));
    // The same again changes nothing, and what the server no longer has goes.
    let (job, tx) = one(&jobs);
    let mut emptied = all(&job);
    emptied.events.iter_mut().for_each(|download| download.events.clear());
    tx.send(emptied).unwrap();
    app.poll_jmap_calendar_at(17.0 * MINUTE);
    assert!(the_review(&app, ada).is_none() && has_calendar(&app, ada));
}

#[test]
fn events_are_not_taken_in_without_their_list_and_that_is_tried_again() {
    let (mut app, ada, jobs, _runs) = window();
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    assert!(app.jmap_calendar_tick_at(0.0));
    let (job, tx) = one(&jobs);
    // A list that names no main calendar makes nothing, and the events stay out.
    let mut bad = all(&job);
    bad.calendars.iter_mut().flat_map(|l| l.calendars.iter_mut()).for_each(|calendar| calendar.main = false);
    tx.send(bad).unwrap();
    app.poll_jmap_calendar_at(0.0);
    assert!(!has_calendar(&app, ada) && the_review(&app, ada).is_none());
    assert!(!app.jmap_calendar_tick_at(FIRST_RETRY_MS - 1.0));
    assert!(app.jmap_calendar_tick_at(FIRST_RETRY_MS));
    // Nor events alone, nor a download that names another account.
    let (job, tx) = one(&jobs);
    tx.send(answer(&job, JmapCalendarResult { events: review(&job), ..Default::default() })).unwrap();
    app.poll_jmap_calendar_at(FIRST_RETRY_MS);
    assert!(the_review(&app, ada).is_none());
    assert!(app.jmap_calendar_tick_at(10.0 * MINUTE));
    let (job, tx) = one(&jobs);
    let mut theirs = all(&job);
    theirs.events.iter_mut().for_each(|download| download.account = ada + 100);
    tx.send(theirs).unwrap();
    app.poll_jmap_calendar_at(10.0 * MINUTE);
    assert!(has_calendar(&app, ada) && the_review(&app, ada).is_none());
}

#[test]
fn an_answer_for_another_address_or_server_is_not_taken_in() {
    let (mut app, ada, jobs, _runs) = window();
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    // For another address.
    assert!(app.jmap_calendar_tick_at(0.0));
    let (job, tx) = one(&jobs);
    tx.send(JmapCalendarResult { email: "bo@example.org".into(), ..all(&job) }).unwrap();
    app.poll_jmap_calendar_at(1_000.0);
    assert!(!app.jmap_calendar_syncing() && !has_calendar(&app, ada));
    // For another server.
    assert!(app.jmap_calendar_tick_at(16.0 * MINUTE));
    let (job, tx) = one(&jobs);
    tx.send(JmapCalendarResult { session: "https://jmap.example.org/other/session".into(), ..all(&job) }).unwrap();
    app.poll_jmap_calendar_at(17.0 * MINUTE);
    assert!(!has_calendar(&app, ada));
    // The account names another server by the time the answer comes.
    assert!(app.jmap_calendar_tick_at(32.0 * MINUTE));
    let (job, tx) = one(&jobs);
    app.execute("account.edit", json!({"id": ada, "jmap": {"session": "https://jmap.example.org/other/session"}})).unwrap();
    tx.send(all(&job)).unwrap();
    app.poll_jmap_calendar_at(33.0 * MINUTE);
    assert!(!app.jmap_calendar_syncing() && !has_calendar(&app, ada));
    assert!(app.session.mailbox.events.values().all(|e| e.origin.account != ada));
}

#[test]
fn a_server_that_offers_no_calendars_is_not_asked_again_until_the_owner_asks() {
    let (mut app, ada, jobs, runs) = window();
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    assert!(app.jmap_calendar_tick_at(0.0));
    let (job, tx) = one(&jobs);
    tx.send(answer(&job, JmapCalendarResult { not_offered: true, ..Default::default() })).unwrap();
    app.poll_jmap_calendar_at(1_000.0);
    assert!(!app.has_sync_problems(), "nothing is wrong");
    for minutes in [1.0, 16.0, 600.0] {
        assert!(!app.jmap_calendar_tick_at(minutes * MINUTE), "{minutes}");
    }
    // Nor for the calendar coming to the front.
    app.session.view.module = Module::Calendar;
    assert!(!app.jmap_calendar_tick_at(601.0 * MINUTE));
    assert!(none(&jobs));
    // The owner's own Send/Receive asks once more.
    app.auto_sync = false;
    send_receive(&mut app, ada, &runs);
    assert!(app.jmap_calendar_tick_at(602.0 * MINUTE));
    assert_eq!(one(&jobs).0.request.account, ada);
}

#[test]
fn no_job_goes_without_a_secret_and_a_host_s_own_credential_goes_as_none() {
    let (mut app, ada, jobs, _runs) = window();
    app.auto_sync = true;
    assert!(!app.jmap_calendar_tick_at(0.0) && none(&jobs));
    assert!(app.dialog.is_none(), "and nobody is asked for one");
    app.services.jmap_host_credentials = Some(Box::new(|a: &mailcraft_model::Account| a.email == "ada@example.org"));
    assert!(app.jmap_calendar_tick_at(1_000.0));
    let (job, _tx) = one(&jobs);
    assert!(job.request.account == ada && job.secret.is_empty());
}

#[test]
fn a_secret_turned_down_is_forgotten_and_nothing_is_asked_until_the_owner_s_send_receive() {
    let (mut app, ada, jobs, _runs) = window();
    app.passwords.insert(ada, "not the one".into());
    app.auto_sync = true;
    assert!(app.jmap_calendar_tick_at(0.0));
    let (job, tx) = one(&jobs);
    tx.send(JmapCalendarResult { auth_refused: true, ..all(&job) }).unwrap();
    app.poll_jmap_calendar_at(1_000.0);
    assert!(app.jmap_refused.contains(&ada) && !app.passwords.contains_key(&ada));
    assert!(!has_calendar(&app, ada) && the_review(&app, ada).is_none());
    // Even with a secret at hand again, nothing by itself.
    app.passwords.insert(ada, "typed again".into());
    assert!(!app.jmap_calendar_tick_at(60.0 * MINUTE) && none(&jobs));
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
    assert!(app.jmap_calendar_tick_at(0.0));
    let (job, tx) = one(&jobs);
    tx.send(answer(&job, JmapCalendarResult { errors: vec!["Couldn't read the calendars".into()], ..Default::default() })).unwrap();
    app.poll_jmap_calendar_at(0.0);
    assert!(app.has_sync_problems());
    assert_eq!(app.account_problem_lines(ada).len(), 1);
    // A Send/Receive that ends meanwhile doesn't bring it forward.
    app.jmap_calendar.after_run(ada);
    assert!(!app.jmap_calendar_tick_at(FIRST_RETRY_MS - 1.0));
    assert!(app.jmap_calendar_tick_at(FIRST_RETRY_MS));
    // The task is gone without an answer: twice as long.
    drop(one(&jobs));
    app.poll_jmap_calendar_at(FIRST_RETRY_MS);
    app.jmap_calendar.after_run(ada);
    assert!(!app.jmap_calendar_tick_at(3.0 * FIRST_RETRY_MS - 1.0));
    assert!(app.jmap_calendar_tick_at(3.0 * FIRST_RETRY_MS));
    // Busy: for as long as the server says, within reason.
    let (job, tx) = one(&jobs);
    tx.send(answer(&job, JmapCalendarResult { busy_for: Some(120), ..Default::default() })).unwrap();
    let at = 3.0 * FIRST_RETRY_MS;
    app.poll_jmap_calendar_at(at);
    app.jmap_calendar.after_run(ada);
    assert!(!app.jmap_calendar_tick_at(at + 119_000.0));
    assert!(app.jmap_calendar_tick_at(at + 120_000.0));
    // A problem beside what was read: what came is kept, and the rest is tried again later.
    let (job, tx) = one(&jobs);
    tx.send(JmapCalendarResult { errors: vec!["One calendar couldn't be read".into()], ..all(&job) }).unwrap();
    let at = at + 120_000.0;
    app.poll_jmap_calendar_at(at);
    assert!(the_review(&app, ada).is_some());
    assert!(!app.jmap_calendar_tick_at(at + FIRST_RETRY_MS - 1.0));
}

#[test]
fn the_calendar_coming_to_the_front_asks_again_when_the_last_reading_is_two_minutes_old() {
    let (mut app, ada, jobs, _runs) = window();
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    assert!(app.jmap_calendar_tick_at(0.0));
    let (job, tx) = one(&jobs);
    tx.send(all(&job)).unwrap();
    app.poll_jmap_calendar_at(1_000.0);
    // In front a minute after the reading: fresh enough.
    app.session.view.module = Module::Calendar;
    assert!(!app.jmap_calendar_tick_at(MINUTE));
    // Staying in front is not coming to the front.
    assert!(!app.jmap_calendar_tick_at(3.0 * MINUTE) && none(&jobs));
    // Away and back, three minutes after.
    app.session.view.module = Module::Mail;
    assert!(!app.jmap_calendar_tick_at(3.0 * MINUTE));
    app.session.view.module = Module::Calendar;
    assert!(app.jmap_calendar_tick_at(3.0 * MINUTE + 1.0));
    assert_eq!(one(&jobs).0.request.account, ada);
}

#[test]
fn the_account_s_contacts_go_first() {
    let (mut app, ada, jobs, _runs) = window();
    let contacts: Kept<JmapContactsJob, JmapContactsResult> = Default::default();
    let kept = contacts.clone();
    app.services.jmap_contacts = Some(Box::new(move |job: JmapContactsJob| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push((job, tx));
        rx
    }));
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    assert!(app.jmap_contacts_tick_at(0.0));
    assert!(!app.jmap_calendar_tick_at(0.0) && none(&jobs));
    let (job, tx) = one(&contacts);
    tx.send(JmapContactsResult { account: ada, email: job.request.email.clone(), session: job.request.session.clone(), ..Default::default() })
        .unwrap();
    app.poll_jmap_contacts_at(1_000.0);
    assert!(app.jmap_calendar_tick_at(1_000.0));
    assert_eq!(one(&jobs).0.request.account, ada);
}

#[test]
fn a_job_under_way_when_another_mailbox_is_opened_is_called_off_and_brings_nothing_into_it() {
    let (mut app, ada, jobs, _runs) = window();
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    assert!(app.jmap_calendar_tick_at(0.0));
    let (job, tx) = one(&jobs);
    // Another mailbox takes its place, with an account at the same address on the same server.
    app.execute("file.new", json!({})).unwrap();
    let other = json!({"email": "ada@example.org", "kind": "jmap", "jmap": {"session": SESSION}});
    let other = app.execute("account.add", other).unwrap()["id"].as_u64().unwrap();
    let _ = tx.send(all(&job));
    app.poll_jmap_calendar_at(1_000.0);
    assert!(job.cancel.load(Ordering::Relaxed));
    assert!(!app.jmap_calendar_syncing());
    assert!(!has_calendar(&app, other) && app.session.mailbox.events.values().all(|e| e.origin.id != "E1"), "nothing of the old job is here");
    // The new mailbox's account goes its own way: no secret was typed for it, so nothing is asked.
    assert!(!app.jmap_calendar_tick_at(2_000.0) && none(&jobs));
    assert!(!app.jmap_refused.contains(&other));
}

#[test]
fn a_mailbox_from_a_file_has_no_calendars_read_until_its_servers_are_confirmed() {
    let (mut app, ada, jobs, _runs) = window();
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    app.session.path = Some(std::path::PathBuf::from("from-a-file.mailcraft"));
    app.judge_mailbox();
    assert!(app.services.jmap_calendar.is_none());
    assert!(!app.jmap_calendar_tick_at(0.0) && none(&jobs));
    app.execute("ui.confirmServers", json!({})).unwrap();
    assert!(app.jmap_calendar_tick_at(1_000.0));
}

#[test]
fn microsoft_s_calendar_job_never_starts_for_a_jmap_account() {
    let (mut app, ada, jobs, _runs) = window();
    let asked = Arc::new(Mutex::new(0usize));
    let count = asked.clone();
    app.services.graph = Some(Box::new(move |_request: GraphRequest| {
        *count.lock().unwrap() += 1;
        let (_tx, rx) = mpsc::channel::<GraphResult>();
        rx
    }));
    app.passwords.insert(ada, "typed here".into());
    app.auto_sync = true;
    // Its calendar is here, and Microsoft's job leaves it alone.
    assert!(app.jmap_calendar_tick_at(0.0));
    let (job, tx) = one(&jobs);
    tx.send(all(&job)).unwrap();
    app.poll_jmap_calendar_at(1_000.0);
    assert!(has_calendar(&app, ada));
    app.session.view.module = Module::Calendar;
    assert!(!app.calendar_tick_at(true, 2_000.0));
    // Even when a file says the account signs in with Microsoft and a token is at hand.
    let account = app.session.mailbox.accounts.iter_mut().find(|a| a.id == ada).unwrap();
    account.oauth = Some(mailcraft_model::OAuth::default());
    assert_eq!(account.oauth.as_ref().map(|o| o.provider), Some(mailcraft_model::OAuthProvider::Microsoft));
    app.seed_graph_token(ada, Tokens { access: "made-up".into(), refresh: "made-up too".into(), expires_at: 4_000_000_000, ..Default::default() });
    assert!(!app.calendar_tick_at(true, 20.0 * MINUTE));
    assert_eq!(*asked.lock().unwrap(), 0);
}
