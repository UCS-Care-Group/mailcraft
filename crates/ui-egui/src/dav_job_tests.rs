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
            listing: Some(DavListing {
                calendars: vec![DavCalendar { href: collection.clone(), name: "Work".into(), ..Default::default() }],
                complete: true,
                capped: false,
            }),
            collections: vec![DavRead {
                href: collection.clone(),
                state: DavReadState::Read,
                full: true,
                complete: true,
                changes: vec![DavChange { href: format!("{collection}a.ics"), etag: "\"1\"".into(), raw: Some(event.into()), gone: false }],
                present: vec![format!("{collection}a.ics")],
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
    // The password the server took is held for the session, under its origin.
    assert_eq!(a.dav_passwords.get(&(id, HOME.to_string())).map(String::as_str), Some("mail"));
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
