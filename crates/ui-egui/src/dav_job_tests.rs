// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading calendars from a calendar server by itself (#209): whose password goes where, and what
//! the window does with a job's answer.

use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::{DavCalendar, DavChange, DavDownload, DavListing, DavRead, DavReadState, Id};
use serde_json::json;

use crate::{DavDone, DavJob, MailApp, Services};

const NOW: i64 = 1_791_993_600;
const HOME: &str = "https://dav.example.org";
const OTHER: &str = "https://cards.example.org";

fn app(separate: bool) -> (MailApp, Id) {
    let session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let mut a = MailApp::new(session, Services::default());
    a.autosave = false;
    a.session.dav_enabled = true;
    let id = a.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    a.execute("account.davSet", json!({"id": id, "calendarHomes": [format!("{HOME}/cal/ada/")], "separatePassword": separate})).unwrap();
    (a, id)
}

#[test]
fn the_mail_password_goes_only_to_a_home_of_the_settings_and_only_when_the_servers_share_it() {
    let (mut a, id) = app(false);
    assert_eq!(a.dav_password(id, HOME), None, "nothing is known yet");
    a.passwords.insert(id, "mail".into());
    assert_eq!(a.dav_password(id, HOME).as_deref(), Some("mail"));
    assert_eq!(a.dav_password(id, OTHER), None, "an origin the settings don't name");
    assert_eq!(a.dav_password(id, "https://evil.example.net"), None);
    // One held for the origin wins.
    a.dav_passwords.insert((id, HOME.into()), "own".into());
    assert_eq!(a.dav_password(id, HOME).as_deref(), Some("own"));
    // A held password for an origin that is no home is never given either.
    a.dav_passwords.insert((id, OTHER.into()), "x".into());
    assert_eq!(a.dav_password(id, OTHER), None);
    // The owner gave these servers a password of their own: the mail one never goes.
    let (mut a, id) = app(true);
    a.passwords.insert(id, "mail".into());
    assert_eq!(a.dav_password(id, HOME), None);
    a.dav_passwords.insert((id, HOME.into()), "own".into());
    assert_eq!(a.dav_password(id, HOME).as_deref(), Some("own"));
}

#[test]
fn a_password_is_held_only_while_no_forget_is_pending_for_its_name() {
    let (mut a, id) = app(true);
    let key = MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), HOME).unwrap();
    a.dav_forget_queue.push(key.clone());
    assert!(a.dav_forget_pending(&key));
    assert!(!a.hold_dav_password(id, HOME, "pw"), "it waits");
    assert!(a.dav_passwords.is_empty());
    a.dav_forget_queue.clear();
    assert!(a.hold_dav_password(id, HOME, "pw"));
    assert_eq!(a.dav_passwords.get(&(id, HOME.to_string())).map(String::as_str), Some("pw"));
    assert!(!a.hold_dav_password(id, HOME, ""), "nothing empty");
    assert!(!a.hold_dav_password(id + 99, HOME, "pw"), "no account, no name");
}

#[test]
fn a_job_is_started_with_the_homes_passwords_and_its_answer_is_taken_in_once() {
    let (mut a, id) = app(false);
    a.passwords.insert(id, "mail".into());
    let jobs: Arc<Mutex<Vec<(String, Vec<String>)>>> = Arc::default();
    let seen = jobs.clone();
    a.services.dav = Some(Box::new(move |job: DavJob| {
        seen.lock().unwrap().push((format!("{job:?}"), job.passwords.keys().cloned().collect()));
        let (send, receive) = std::sync::mpsc::channel();
        let collection = format!("{HOME}/cal/ada/work/");
        let event = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261201T100000Z\r\nDTEND:20261201T110000Z\r\nSUMMARY:Planning\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let download = DavDownload {
            account: job.ask.account,
            email: job.ask.email.clone(),
            server: job.ask.server.clone(),
            listing: Some(DavListing {
                calendars: vec![DavCalendar { href: collection.clone(), name: "Work".into(), ..Default::default() }],
                complete: true,
                capped: false,
            }),
            collections: vec![DavRead {
                href: collection.clone(),
                state: DavReadState::Read,
                complete: true,
                changes: vec![DavChange { href: format!("{collection}a.ics"), etag: "\"1\"".into(), raw: Some(event.into()), gone: false }],
                members: Some(vec![format!("{collection}a.ics")]),
                ..Default::default()
            }],
            signed_in: vec![HOME.into()],
            ..Default::default()
        };
        send.send(DavDone { download }).unwrap();
        receive
    }));
    a.dav_tick();
    a.dav_tick();
    {
        let jobs = jobs.lock().unwrap();
        assert_eq!(jobs.len(), 1, "one at a time, and not again before it is time");
        assert_eq!(jobs[0].1, vec![HOME.to_string()]);
        assert!(!jobs[0].0.contains("mail"), "{}", jobs[0].0);
    }
    assert!(a.session.mailbox.events.values().any(|e| e.title == "Planning" && e.origin.account == id));
    // The account's mail password is not held under the origin: it is looked up at every job, by
    // the rule as it stands.
    assert!(a.dav_passwords.is_empty());
}

#[test]
fn nothing_starts_while_the_feature_is_off_or_without_a_host() {
    let (mut a, _) = app(false);
    let started = Arc::new(Mutex::new(0usize));
    let count = started.clone();
    a.services.dav = Some(Box::new(move |_| {
        *count.lock().unwrap() += 1;
        std::sync::mpsc::channel().1
    }));
    a.session.dav_enabled = false;
    a.dav_tick();
    assert_eq!(*started.lock().unwrap(), 0);
    a.session.dav_enabled = true;
    a.services.dav = None;
    a.dav_tick();
    assert_eq!(*started.lock().unwrap(), 0);
}

#[test]
fn a_job_that_ended_without_an_answer_is_trouble_not_a_hang() {
    let (mut a, _) = app(false);
    a.services.dav = Some(Box::new(|_| std::sync::mpsc::channel().1));
    a.dav_tick();
    a.dav_tick();
    assert!(a.session.mailbox.events.values().all(|e| e.origin.account == 0));
}

/// A job that answers with `problem` and counts the jobs started.
fn failing_job(a: &mut MailApp, problem: mailcraft_model::DavProblem) -> Arc<Mutex<Vec<std::collections::HashMap<String, String>>>> {
    let jobs: Arc<Mutex<Vec<std::collections::HashMap<String, String>>>> = Arc::default();
    let seen = jobs.clone();
    a.services.dav = Some(Box::new(move |job: DavJob| {
        seen.lock().unwrap().push(job.passwords.clone());
        let (send, receive) = std::sync::mpsc::channel();
        let download = DavDownload {
            account: job.ask.account,
            email: job.ask.email.clone(),
            server: job.ask.server.clone(),
            generation: job.ask.generation,
            problem,
            ..Default::default()
        };
        send.send(DavDone { download }).unwrap();
        receive
    }));
    jobs
}

#[test]
fn a_password_the_server_refused_is_not_sent_again_until_the_owner_gives_another() {
    let (mut a, id) = app(false);
    a.passwords.insert(id, "mail".into());
    let jobs = failing_job(&mut a, mailcraft_model::DavProblem::WrongPassword);
    a.dav_passwords.insert((id, HOME.into()), "held".into());
    a.dav_tick();
    a.dav_tick();
    assert_eq!(jobs.lock().unwrap().len(), 1);
    assert!(a.dav_passwords.is_empty(), "what was refused is not held");
    // (The mail password is another one, and was not refused: it is tried, once.)
    a.dav.next = 0.0;
    a.dav.trouble.clear();
    a.dav_tick();
    a.dav_tick();
    assert_eq!(jobs.lock().unwrap().len(), 2);
    // Every back-off would send it again, and lock the account out: it isn't.
    for _ in 0..3 {
        a.dav.next = 0.0;
        a.dav.trouble.clear();
        a.dav_tick();
        a.dav_tick();
    }
    assert_eq!(jobs.lock().unwrap().len(), 2, "nothing was sent again");
    // The owner gives another: it goes.
    a.dav_passwords.insert((id, HOME.into()), "new".into());
    a.dav.next = 0.0;
    a.dav.trouble.clear();
    a.dav_tick();
    assert_eq!(jobs.lock().unwrap().len(), 3);
    assert_eq!(jobs.lock().unwrap()[2].get(HOME).map(String::as_str), Some("new"));
}

#[test]
fn a_password_of_their_own_given_or_taken_back_forgets_what_was_held() {
    let (mut a, id) = app(false);
    a.dav_tick();
    a.dav_passwords.insert((id, HOME.into()), "held".into());
    a.passwords.insert(id, "mail".into());
    a.dav_tick();
    assert!(!a.dav_passwords.is_empty(), "nothing changed");
    a.execute("account.davSet", json!({"id": id, "separatePassword": true})).unwrap();
    a.dav_tick();
    assert!(a.dav_passwords.is_empty(), "held for the shared password: not the one of their own");
    assert_eq!(a.dav_password(id, HOME), None, "and the mail password no longer goes");
    a.dav_passwords.insert((id, HOME.into()), "own".into());
    a.execute("account.davSet", json!({"id": id, "separatePassword": false})).unwrap();
    a.dav_tick();
    assert!(a.dav_passwords.is_empty());
    assert_eq!(a.dav_password(id, HOME).as_deref(), Some("mail"));
}

#[test]
fn a_server_that_refused_a_password_is_tried_again_after_hours_and_the_owner_is_told() {
    let (mut a, id) = app(false);
    a.passwords.insert(id, "mail".into());
    let jobs = failing_job(&mut a, mailcraft_model::DavProblem::WrongPassword);
    a.dav_tick();
    a.dav_tick();
    assert_eq!(jobs.lock().unwrap().len(), 1);
    let said = a.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert!(said.contains("calendar server") && !said.contains("dav.example.org"), "the owner is told, and nothing of the server: {said:?}");
    // Not at once, whatever the back-off.
    a.dav.next = 0.0;
    a.dav.trouble.clear();
    a.dav_tick();
    assert_eq!(jobs.lock().unwrap().len(), 1);
    // After hours, with the same password: once more.
    a.dav.refused.get_mut(&id).unwrap().1 = 0.0;
    a.dav.next = 0.0;
    a.dav_tick();
    assert_eq!(jobs.lock().unwrap().len(), 2);
}

#[test]
fn one_account_in_trouble_does_not_hold_up_another() {
    let (mut a, first) = app(false);
    let second =
        a.execute("account.add", json!({"email": "grace@example.org", "incoming": {"host": "imap2.example.org"}})).unwrap()["id"].as_u64().unwrap();
    a.execute("account.davSet", json!({"id": second, "calendarHomes": ["https://dav2.example.org/cal/grace/"]})).unwrap();
    let seen: Arc<Mutex<Vec<Id>>> = Arc::default();
    let log = seen.clone();
    a.services.dav = Some(Box::new(move |job: DavJob| {
        log.lock().unwrap().push(job.ask.account);
        let (send, receive) = std::sync::mpsc::channel();
        let problem = if job.ask.account == first { mailcraft_model::DavProblem::Unreachable } else { mailcraft_model::DavProblem::None };
        let download = DavDownload {
            account: job.ask.account,
            email: job.ask.email.clone(),
            server: job.ask.server.clone(),
            generation: job.ask.generation,
            problem,
            ..Default::default()
        };
        send.send(DavDone { download }).unwrap();
        receive
    }));
    for _ in 0..4 {
        a.dav.next = 0.0;
        a.dav_tick();
        a.dav_tick();
    }
    let order = seen.lock().unwrap().clone();
    assert_eq!(order[0], first);
    assert!(order[1..].iter().all(|id| *id == second), "the one that is unreachable waits alone, and takes none of the others' turns: {order:?}");
    assert_eq!(order.len(), 4);
}

#[test]
fn what_is_held_is_the_password_that_was_sent_not_what_would_be_found_now() {
    let (mut a, id) = app(false);
    a.dav_passwords.insert((id, HOME.into()), "own".into());
    a.services.dav = Some(Box::new(move |job: DavJob| {
        let (send, receive) = std::sync::mpsc::channel();
        let download = DavDownload {
            account: job.ask.account,
            email: job.ask.email.clone(),
            server: job.ask.server.clone(),
            signed_in: vec![HOME.into()],
            ..Default::default()
        };
        send.send(DavDone { download }).unwrap();
        receive
    }));
    a.dav_tick();
    // While the job was out, something else was put in its place.
    a.dav_passwords.insert((id, HOME.into()), "changed".into());
    a.dav_tick();
    assert_eq!(a.dav_passwords.get(&(id, HOME.to_string())).map(String::as_str), Some("own"));
}

// ---- the invariants, held by design (round 5) -------------------------------------------------

fn say(a: &MailApp) -> String {
    a.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
}

#[test]
fn i2_a_run_that_stopped_for_its_limits_is_not_trouble() {
    let (mut a, id) = app(false);
    a.passwords.insert(id, "mail".into());
    let _ = failing_job(&mut a, mailcraft_model::DavProblem::Limits);
    a.dav_tick();
    a.dav_tick();
    assert!(!a.dav.trouble.contains_key(&id), "no back-off: more is waiting, and it goes on at the next run");
}

#[test]
fn i5_the_owner_is_told_which_account_by_its_own_name_and_again_for_another_account() {
    let (mut a, first) = app(false);
    a.execute("account.edit", json!({"id": first, "name": "Work mail"})).unwrap();
    let second =
        a.execute("account.add", json!({"email": "grace@example.org", "incoming": {"host": "imap2.example.org"}})).unwrap()["id"].as_u64().unwrap();
    a.execute("account.edit", json!({"id": second, "name": "Home mail"})).unwrap();
    a.execute("account.davSet", json!({"id": second, "calendarHomes": ["https://dav2.example.org/cal/grace/"]})).unwrap();
    a.passwords.insert(first, "m1".into());
    a.passwords.insert(second, "m2".into());
    let _ = failing_job(&mut a, mailcraft_model::DavProblem::WrongPassword);
    a.dav_tick();
    a.dav_tick();
    let said = say(&a);
    assert!(said.contains("Work mail") && !said.contains("example.org"), "{said:?}");
    a.status_msg = None;
    a.dav.next = 0.0;
    a.dav_tick();
    a.dav_tick();
    let said = say(&a);
    assert!(said.contains("Home mail") && !said.contains("example.org"), "the same kind of trouble, for another account: {said:?}");
}

#[test]
fn i5_a_refusal_is_forgotten_when_the_user_name_is_put_right() {
    let (mut a, id) = app(false);
    a.passwords.insert(id, "mail".into());
    let jobs = failing_job(&mut a, mailcraft_model::DavProblem::WrongPassword);
    a.dav_tick();
    a.dav_tick();
    assert_eq!(jobs.lock().unwrap().len(), 1);
    a.execute("account.davSet", json!({"id": id, "username": "ada.lovelace"})).unwrap();
    a.dav.next = 0.0;
    a.dav.trouble.clear();
    a.dav_tick();
    assert_eq!(jobs.lock().unwrap().len(), 2, "the same password under another name is another try");
}

#[test]
fn r_a_job_whose_thread_died_is_trouble_and_waits_its_back_off() {
    let (mut a, id) = app(false);
    a.passwords.insert(id, "mail".into());
    a.services.dav = Some(Box::new(|_| std::sync::mpsc::channel().1));
    a.dav_tick();
    a.dav_tick();
    assert!(a.dav.trouble.contains_key(&id), "not tried again every minute");
}

#[test]
fn no_job_starts_while_the_credential_store_is_still_being_read() {
    // The first job of a session came before the store's answer, ended in `NeedsPassword` and
    // told the owner so, for a password that was remembered all along (#209).
    let (mut a, id) = app(false);
    a.ui.remember_sign_in = true;
    let jobs = failing_job(&mut a, mailcraft_model::DavProblem::None);
    let (send, receive) = std::sync::mpsc::channel();
    a.secret_read = Some((a.session.openings(), receive));
    a.dav_tick();
    assert!(jobs.lock().unwrap().is_empty(), "it waits for the answer");
    let key = MailApp::secret_key("password", a.session.mailbox.account(id).unwrap());
    let answers = vec![(key, Some("mail".to_string()))];
    send.send(crate::secret_read::SecretRead { answers, moved: Vec::new(), migrated: false, trouble: None }).unwrap();
    a.poll_secret_read();
    a.dav_tick();
    let jobs = jobs.lock().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].get(HOME).map(String::as_str), Some("mail"), "with the remembered password");
}

// ---- found in review, round 6 ------------------------------------------------------------------

#[test]
fn r6_a_job_out_while_the_server_was_put_away_and_set_again_is_not_taken_in() {
    let (mut a, id) = app(false);
    a.passwords.insert(id, "mail".into());
    let _ = failing_job(&mut a, mailcraft_model::DavProblem::Unreachable);
    a.dav_tick();
    // While the job is out: off, and on again with the same settings.
    a.execute("account.davOff", json!({"id": id})).unwrap();
    a.execute("account.davSet", json!({"id": id, "calendarHomes": [format!("{HOME}/cal/ada/")]})).unwrap();
    a.dav_tick();
    assert!(!a.dav.trouble.contains_key(&id), "its answer says nothing of the account as it is now");
    assert!(say(&a).is_empty());
}

#[test]
fn r6_an_account_that_is_full_is_said_once_while_it_stays_so_and_neither_it_nor_errors_hold_the_account_up() {
    let (mut a, id) = app(false);
    a.passwords.insert(id, "mail".into());
    let run = |a: &mut MailApp, problem: mailcraft_model::DavProblem| {
        a.status_msg = None;
        let _ = failing_job(a, problem);
        a.dav.next = 0.0;
        a.dav_tick();
        a.dav_tick();
        assert!(!a.dav.trouble.contains_key(&id), "{problem:?}: the next run comes at the usual time");
        say(a)
    };
    use mailcraft_model::DavProblem as P;
    assert!(run(&mut a, P::Full).contains("as much"));
    assert!(run(&mut a, P::Full).is_empty(), "said once while every run finds it so");
    assert!(run(&mut a, P::None).is_empty());
    assert!(run(&mut a, P::Full).contains("as much"), "full again after a run that wasn't");
    // Put away and set again: said again.
    a.execute("account.davOff", json!({"id": id})).unwrap();
    a.execute("account.davSet", json!({"id": id, "calendarHomes": [format!("{HOME}/cal/ada/")]})).unwrap();
    assert!(run(&mut a, P::Full).contains("as much"));
    assert!(run(&mut a, P::ServerErrors).contains("errors"));
    assert!(run(&mut a, P::ServerErrors).is_empty(), "said once");
}
