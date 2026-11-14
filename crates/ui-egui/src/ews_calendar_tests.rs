// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Exchange server account's calendar read for the days the owner goes to (#1274), with a host
//! made by hand that answers each read with one appointment in the stretch it was asked for.

use std::cell::RefCell;
use std::rc::Rc;

use mailcraft_engine::{Module, Session};
use mailcraft_model::remote::{CalendarsDownload, EventsDownload, RemoteCalendar, RemoteEvent, RemoteEventKind};
use mailcraft_model::sync::EwsDirectoryRequest;
use mailcraft_model::{Account, AccountKind, EwsServer, Id, Rights};
use serde_json::json;

use super::*;
use crate::Services;

const NOW: i64 = 1_791_993_600;
const ME: &str = "ada@example.org";
const MINUTE: f64 = 60_000.0;

type Asked = Rc<RefCell<Vec<EwsDirectoryRequest>>>;

/// What the host answers: one appointment at the start of the stretch, or the result `answer` makes.
fn host(asked: &Asked, answer: fn(&EwsDirectoryRequest) -> EwsDirectoryResult) -> Services {
    let asked = asked.clone();
    Services {
        ews_calendar: Some(Box::new(move |job: EwsCalendarJob| {
            asked.borrow_mut().push(job.request.clone());
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(answer(&job.request));
            rx
        })),
        ..Default::default()
    }
}

fn found(request: &EwsDirectoryRequest) -> EwsDirectoryResult {
    let main = RemoteCalendar { id: "cal-main".into(), main: true, rights: Rights::Edit, ..Default::default() };
    let start = request.from + 9 * 86_400;
    let event = RemoteEvent {
        id: format!("event-{}", request.from),
        title: format!("Found from {}", request.from),
        start,
        end: start + 3_600,
        kind: RemoteEventKind::Single,
        ..Default::default()
    };
    EwsDirectoryResult {
        account: request.account,
        email: request.email.clone(),
        calendars: Some(CalendarsDownload {
            account: request.account,
            email: ME.into(),
            complete: true,
            calendars: vec![main],
            ..Default::default()
        }),
        events: vec![EventsDownload {
            account: request.account,
            email: ME.into(),
            from: request.from,
            to: request.to,
            complete: true,
            events: vec![event],
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// A window with an Exchange server account whose password was given this session, the calendar
/// in front, and the host above.
fn app(answer: fn(&EwsDirectoryRequest) -> EwsDirectoryResult) -> (MailApp, Id, Asked) {
    let asked: Asked = Rc::default();
    let session = Session::at(mailcraft_model::Mailbox::default(), NOW, 0);
    let mut a = MailApp::new(session, host(&asked, answer));
    a.autosave = false;
    let server = EwsServer { url: "https://mail.example.org/EWS/Exchange.asmx".into(), ..Default::default() };
    let account = a.session.mailbox.add_account(Account { email: ME.into(), kind: AccountKind::Exchange, ews: Some(server), ..Default::default() });
    a.passwords.insert(account, "not-a-real-password".into());
    a.session.view.module = Module::Calendar;
    (a, account, asked)
}

fn titles(a: &MailApp) -> Vec<String> {
    let mut titles: Vec<String> = a.session.mailbox.events.values().map(|e| e.title.clone()).collect();
    titles.sort();
    titles
}

#[test]
fn the_days_on_screen_are_read_with_the_calendar_alone_and_nothing_sent() {
    let (mut a, account, asked) = app(found);
    assert!(a.ews_calendar_tick_at(0.0));
    let request = asked.borrow()[0].clone();
    assert_eq!(request.account, account);
    assert_eq!((request.from, request.to), a.calendar_window());
    assert!(request.calendars && !request.contacts && !request.tasks && request.background);
    assert!(request.changes.is_none() && request.contact_changes.is_none() && request.task_changes.is_none());
    // One at a time.
    assert!(!a.ews_calendar_tick_at(1.0));
    a.poll_ews_calendar(2.0);
    assert!(!a.ews_calendar_reading());
    assert_eq!(titles(&a), [format!("Found from {}", request.from)]);
    // The same days a moment later: they were just read.
    assert!(!a.ews_calendar_tick_at(MINUTE));
    assert_eq!(asked.borrow().len(), 1);
    // Five minutes on, with the calendar still in front: read again.
    assert!(a.ews_calendar_tick_at(6.0 * MINUTE));
    assert_eq!(asked.borrow().len(), 2);
}

#[test]
fn days_the_owner_goes_to_are_read_at_once() {
    let (mut a, _, asked) = app(found);
    assert!(a.ews_calendar_tick_at(0.0));
    a.poll_ews_calendar(1.0);
    a.execute("view.goToDate", json!({"date": "2028-03-06"})).unwrap();
    assert!(a.ews_calendar_tick_at(2.0));
    a.poll_ews_calendar(3.0);
    let later = asked.borrow()[1].clone();
    assert_eq!((later.from, later.to), a.calendar_window());
    assert!(later.from > NOW + 400 * 86_400);
    assert_eq!(titles(&a).len(), 2);
    // Back to where it was: read a minute ago, so not again.
    a.execute("view.goToDate", json!({"date": "2026-10-14"})).unwrap();
    assert!(!a.ews_calendar_tick_at(MINUTE));
}

#[test]
fn nothing_is_read_when_the_calendar_is_not_in_front_or_nothing_is_at_hand() {
    let (mut a, account, asked) = app(found);
    a.session.view.module = Module::Mail;
    assert!(!a.ews_calendar_tick_at(0.0));
    a.session.view.module = Module::Calendar;
    // Without the password nothing is asked for: a Send/Receive asks the owner first.
    let password = a.passwords.remove(&account).unwrap();
    assert!(!a.ews_calendar_tick_at(0.0));
    a.passwords.insert(account, password);
    // Automatic Send/Receive off: nothing by itself.
    a.auto_sync = false;
    assert!(!a.ews_calendar_tick_at(0.0));
    a.auto_sync = true;
    // A password turned down, or a certificate that needs a say, stops it until the owner acts.
    a.jmap_refused.insert(account);
    assert!(!a.ews_calendar_tick_at(0.0));
    a.jmap_refused.remove(&account);
    a.ews_certificate.insert(account, "ab".repeat(32));
    assert!(!a.ews_calendar_tick_at(0.0));
    a.ews_certificate.remove(&account);
    // A stretch a Send/Receive read whole counts as read.
    let window = a.calendar_window();
    a.ews_calendar_send_receive_read(&found(&EwsDirectoryRequest {
        account,
        email: ME.into(),
        from: window.0 - 1,
        to: window.1 + 1,
        ..Default::default()
    }));
    assert!(!a.ews_calendar_tick_at(0.0));
    assert!(asked.borrow().is_empty());
}

/// A Send/Receive may send a change the read was asked for before: the read is called off and
/// nothing of it is taken in.
#[test]
fn a_send_receive_calls_the_read_off() {
    let (mut a, _, asked) = app(found);
    assert!(a.ews_calendar_tick_at(0.0));
    a.ews_calendar_stop();
    assert!(!a.ews_calendar_reading());
    a.poll_ews_calendar(1.0);
    assert!(titles(&a).is_empty());
    assert_eq!(asked.borrow().len(), 1);
}

#[test]
fn a_password_turned_down_is_forgotten_and_a_failed_read_waits() {
    let (mut a, account, _) = app(|request| EwsDirectoryResult { account: request.account, auth_refused: true, ..Default::default() });
    assert!(a.ews_calendar_tick_at(0.0));
    a.poll_ews_calendar(1.0);
    assert!(!a.passwords.contains_key(&account));
    assert!(a.jmap_refused.contains(&account));

    let (mut a, account, asked) =
        app(|request| EwsDirectoryResult { account: request.account, errors: vec!["Couldn't reach the server".into()], ..Default::default() });
    assert!(a.ews_calendar_tick_at(0.0));
    a.poll_ews_calendar(1.0);
    assert_eq!(a.status_msg.as_ref().map(|(text, _)| text.as_str()), Some("Couldn't reach the server"));
    // Not again at once, but after a while; the password stays.
    assert!(!a.ews_calendar_tick_at(30_000.0));
    assert!(a.ews_calendar_tick_at(2.0 * MINUTE));
    assert_eq!(asked.borrow().len(), 2);
    assert!(a.passwords.contains_key(&account));
}

/// What another mailbox's read brings is nobody's.
#[test]
fn an_answer_for_a_mailbox_no_longer_open_is_dropped() {
    let (mut a, _, _) = app(found);
    assert!(a.ews_calendar_tick_at(0.0));
    let job = a.ews_calendar.job.as_mut().unwrap();
    job.opening = job.opening.wrapping_add(1);
    a.poll_ews_calendar(1.0);
    assert!(titles(&a).is_empty());
}
