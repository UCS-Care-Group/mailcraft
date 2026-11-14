// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Exchange server account's calendar, contacts and to-dos as the engine works them out and
//! takes them in, with results made by hand.

use mailcraft_model::Rights;
use mailcraft_model::remote::{CalendarsDownload, ContactsDownload, EventsDownload, RemoteCalendar, RemoteContact, RemoteEvent, RemoteEventKind};
use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList, TasksDownload};
use serde_json::json;

use super::*;

const ME: &str = "ada@example.org";
const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": URL}})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

#[test]
fn a_request_names_the_calendar_window_and_what_waits_to_go() {
    let (s, account) = session();
    let request = ews_directory_request(&s, account, Some(1_000), Some(2_000), Part::Whole, false).unwrap();
    assert_eq!((request.from, request.to), (1_000, 2_000));
    assert!(request.calendars && request.contacts && request.tasks);
    assert!(request.changes.is_none());
}

const NOW: i64 = 1_791_993_600;
const DAY: i64 = 86_400;

/// An Exchange server account with an appointment made here (waiting to go), a contact book
/// opened by address and a calendar opened by address, at [`NOW`].
fn busy_session() -> (Session, Id) {
    let mut s = Session::at(mailcraft_model::Mailbox::default(), NOW, 0);
    let added = s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": URL}})).unwrap();
    let account = added["id"].as_u64().unwrap();
    let a = s.mailbox.account_mut(account).unwrap();
    a.opened_calendars.push(mailcraft_model::OpenedCalendar { address: "bo@example.org".into(), ..Default::default() });
    a.opened_address_books.push(mailcraft_model::OpenedAddressBook { address: "cy@example.org".into(), ..Default::default() });
    let main = RemoteCalendar { id: "cal-main".into(), main: true, rights: Rights::Edit, ..Default::default() };
    crate::cmd::remote::apply_calendars(
        &mut s,
        &CalendarsDownload { account, email: ME.into(), complete: true, calendars: vec![main], ..Default::default() },
    )
    .unwrap();
    s.run("calendar.new", &json!({"title": "Planning", "start": "2026-10-20T10:00"})).unwrap();
    (s, account)
}

/// The calendar alone, for the days on screen (#1274): nothing goes up with it, and no address
/// book or to-do list is read.
#[test]
fn a_calendar_read_reads_the_calendars_alone_and_sends_nothing() {
    let (mut s, account) = busy_session();
    let whole = ews_directory_request(&s, account, None, None, Part::Whole, false).unwrap();
    assert!(whole.changes.is_some(), "the appointment made here waits to go with a Send/Receive");
    assert_eq!(whole.shared.len(), 2);
    let calendar = ews_directory_request(&s, account, None, None, Part::Calendar, true).unwrap();
    assert!(calendar.calendars && !calendar.contacts && !calendar.tasks);
    assert!(calendar.changes.is_none() && calendar.contact_changes.is_none() && calendar.task_changes.is_none());
    assert_eq!(calendar.shared.len(), 1, "the calendar opened by address, not the address book");
    assert!(calendar.shared[0].calendar && !calendar.shared[0].contacts);
    assert!(calendar.background);
    // The days on screen and a margin either side, as a Send/Receive used to read.
    assert_eq!((calendar.from, calendar.to), (whole.from, whole.to));
    assert!(calendar.from < NOW && calendar.to > NOW);
    assert!(calendar.to - calendar.from < 60 * DAY);
    // The command says the same.
    let v = s.run("sync.ewsDirectoryRequest", &json!({"account": account, "calendarOnly": true})).unwrap();
    assert_eq!(v["contacts"], json!(null));
    assert_eq!(v["calendars"], json!(true));
}

/// A Send/Receive reads a month back and a year ahead (#1274), with the days on screen when they
/// are not too far off, and never more than two years at once.
#[test]
fn a_send_receive_reads_a_month_back_and_a_year_ahead() {
    let (mut s, account) = busy_session();
    let (from, to) = send_receive_stretch(&s);
    assert!(from <= NOW - 31 * DAY && from > NOW - 60 * DAY, "{}", (NOW - from) / DAY);
    assert!((NOW + 366 * DAY..NOW + 400 * DAY).contains(&to), "{}", (to - NOW) / DAY);
    let request = crate::cmd::ews_mail::ews_mail_request(&s, account, false).unwrap();
    let directory = request.directory.expect("a full run reads the directory");
    assert_eq!((directory.from, directory.to), (from, to));
    // The owner looks at the days a year and a half ahead: they come with it.
    s.run("view.goToDate", &json!({"date": "2028-04-10"})).unwrap();
    let (_, later) = send_receive_stretch(&s);
    assert!(later > NOW + 540 * DAY, "{}", (later - NOW) / DAY);
    // Ten years ahead: the wide stretch alone, and the days on screen are read when they are shown.
    s.run("view.goToDate", &json!({"date": "2036-04-10"})).unwrap();
    assert_eq!(send_receive_stretch(&s), (from, to));
    // A stretch asked for is never longer than two years.
    let far = ews_directory_request(&s, account, Some(NOW), Some(NOW + 3_000 * DAY), Part::Calendar, true).unwrap();
    assert_eq!(far.to - far.from, 730 * DAY);
}

#[test]
fn a_download_is_taken_in_exactly_as_a_microsoft_accounts_is() {
    let (mut s, account) = session();
    let calendars = CalendarsDownload {
        account,
        email: ME.into(),
        complete: true,
        calendars: vec![RemoteCalendar { id: String::new(), name: String::new(), main: true, rights: Rights::Edit, ..Default::default() }],
        ..Default::default()
    };
    let event = RemoteEvent {
        id: "event-1".into(),
        title: "Budget review".into(),
        start: 1_000,
        end: 1_600,
        kind: RemoteEventKind::Single,
        ..Default::default()
    };
    let events = EventsDownload { account, email: ME.into(), from: 0, to: 2_000, complete: true, events: vec![event], ..Default::default() };
    let contacts = ContactsDownload {
        account,
        email: ME.into(),
        complete: true,
        contacts: vec![RemoteContact { id: "c-1".into(), display_name: "Grace Hopper".into(), ..Default::default() }],
        ..Default::default()
    };
    let tasks = TasksDownload {
        account,
        email: ME.into(),
        complete: true,
        lists: vec![RemoteTaskList { id: "tasks".into(), complete: true, shared: false }],
        tasks: vec![RemoteTask { id: "t-1".into(), list: "tasks".into(), title: "Send it".into(), ..Default::default() }],
    };
    let result = EwsDirectoryResult {
        account,
        email: ME.into(),
        calendars: Some(calendars),
        events: vec![events],
        contacts: Some(contacts),
        tasks: Some(tasks),
        ..Default::default()
    };
    let applied = apply_ews_directory(&mut s, &result).unwrap();
    assert_eq!(applied["events"], 1);
    assert_eq!(s.mailbox.events.len(), 1);
    assert_eq!(s.mailbox.contacts.len(), 1);
    assert_eq!(s.mailbox.tasks.len(), 1);
    assert!(s.mailbox.calendars.iter().any(|calendar| calendar.origin.account == account && calendar.origin.id.is_empty()));
}

#[test]
fn a_second_directory_run_keeps_the_same_event_and_reminder() {
    let (mut s, account) = session();
    let calendars = CalendarsDownload {
        account,
        email: ME.into(),
        complete: true,
        calendars: vec![RemoteCalendar { id: String::new(), name: String::new(), main: true, rights: Rights::Edit, ..Default::default() }],
        ..Default::default()
    };
    let event = RemoteEvent {
        id: "event-1".into(),
        change_key: "ck-1".into(),
        title: "Budget review".into(),
        start: 1_000,
        end: 1_600,
        kind: RemoteEventKind::Single,
        reminder_minutes: Some(15),
        ..Default::default()
    };
    let events = EventsDownload { account, email: ME.into(), from: 0, to: 2_000, complete: true, events: vec![event], ..Default::default() };
    let result = EwsDirectoryResult { account, email: ME.into(), calendars: Some(calendars), events: vec![events], ..Default::default() };
    apply_ews_directory(&mut s, &result).unwrap();
    let first = s.mailbox.events.values().next().map(|event| (event.id, event.start, event.reminder_minutes)).unwrap();
    apply_ews_directory(&mut s, &result).unwrap();
    let second = s.mailbox.events.values().next().map(|event| (event.id, event.start, event.reminder_minutes)).unwrap();
    assert_eq!(first, second, "a second read is the same event, at the same reminder time");
}

#[test]
fn a_result_for_another_account_is_refused_whole() {
    let (mut s, account) = session();
    let result = EwsDirectoryResult { account, email: "eve@example.org".into(), ..Default::default() };
    assert!(apply_ews_directory(&mut s, &result).is_err());
}
