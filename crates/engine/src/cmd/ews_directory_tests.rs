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
    let request = ews_directory_request(&s, account, Some(1_000), Some(2_000), false).unwrap();
    assert_eq!((request.from, request.to), (1_000, 2_000));
    assert!(request.calendars && request.contacts && request.tasks);
    assert!(request.changes.is_none());
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
fn a_result_for_another_account_is_refused_whole() {
    let (mut s, account) = session();
    let result = EwsDirectoryResult { account, email: "eve@example.org".into(), ..Default::default() };
    assert!(apply_ews_directory(&mut s, &result).is_err());
}
