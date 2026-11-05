// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The DAV job: what it reads, what it takes in and when it is asked for.

use super::*;
use mailcraft_engine::Session;
use mailcraft_model::remote::{
    CalendarsDownload, ContactChangeKind, ContactOp, ContactOutcome, ContactsDownload, ContactsUploaded, EventResult, EventsDownload,
    RemoteAddressBook, RemoteCalendar, RemoteContact, RemoteEvent,
};
use mailcraft_model::{Account, AccountKind, Calendar, Contact, DavServer, Mailbox, Origin, Rights, Security, Server};
use serde_json::json;
use std::sync::mpsc;

fn app() -> (MailApp, Id) {
    let mut mailbox = Mailbox::with_local_account("", "");
    let id = mailbox.add_account(Account {
        email: "ada@example.org".into(),
        display_name: "Ada".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "mail.example.org".into(), port: 993, security: Security::Tls }),
        outgoing: Some(Server { host: "mail.example.org".into(), port: 587, security: Security::StartTls }),
        dav: Some(DavServer { caldav: "https://mail.example.org/caldav/".into(), carddav: "https://mail.example.org/carddav/".into() }),
        ..Default::default()
    });
    (MailApp::new(Session::at(mailbox, 1_800_000_000, 0), crate::Services::default()), id)
}

#[test]
fn the_calendars_and_every_shared_address_book_are_taken_in() {
    let (mut app, account) = app();
    let (tx, rx) = mpsc::channel();
    app.dav.rx = Some((account, app.session.openings(), rx));
    let main = "https://mail.example.org/caldav/ada@example.org/calendar/";
    let shared = "https://mail.example.org/caldav/ada@example.org/shared/";
    tx.send(DavResult {
        account,
        calendars: Some(CalendarsDownload {
            account,
            email: "ada@example.org".into(),
            complete: true,
            calendars: vec![
                RemoteCalendar { id: main.into(), name: "Calendar".into(), main: true, ..Default::default() },
                RemoteCalendar { id: shared.into(), name: "Team".into(), ..Default::default() },
            ],
            ..Default::default()
        }),
        events: vec![
            EventsDownload {
                account,
                email: "ada@example.org".into(),
                calendar: String::new(),
                from: 1_800_000_000,
                to: 1_800_003_600,
                complete: true,
                events: vec![
                    RemoteEvent {
                        id: format!("{main}e-1.ics"),
                        title: "Stand-up".into(),
                        start: 1_800_000_000,
                        end: 1_800_003_600,
                        ..Default::default()
                    },
                    RemoteEvent {
                        id: "/caldav/ada@example.org/Investigation/c3ed%2540linchange.ics".into(),
                        uid: "c3ed@linchange".into(),
                        title: "Investigation event".into(),
                        start: 1_800_003_600,
                        end: 1_800_007_200,
                        start_zone: "UTC".into(),
                        ..Default::default()
                    },
                    RemoteEvent {
                        id: "/caldav/ada@example.org/Investigation/28cd%2540linchange.ics".into(),
                        uid: "28cd@linchange".into(),
                        title: "Rome event".into(),
                        start: 1_800_010_800,
                        end: 1_800_014_400,
                        start_zone: "Europe/Rome".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            EventsDownload {
                account,
                email: "ada@example.org".into(),
                calendar: shared.into(),
                from: 1_800_000_000,
                to: 1_800_003_600,
                complete: true,
                events: vec![RemoteEvent {
                    id: format!("{shared}team.ics"),
                    title: "Team event".into(),
                    start: 1_800_000_000,
                    end: 1_800_003_600,
                    ..Default::default()
                }],
                ..Default::default()
            },
        ],
        contacts: Some(ContactsDownload {
            account,
            email: "ada@example.org".into(),
            complete: true,
            books: Vec::new(),
            contacts: vec![RemoteContact {
                id: "https://mail.example.org/carddav/ada@example.org/contacts/~book-7/one.vcf".into(),
                display_name: "Ada Lovelace".into(),
                book: "Suppliers".into(),
                ..Default::default()
            }],
        }),
        ..Default::default()
    })
    .unwrap();
    app.poll_dav();
    assert_eq!(app.dav.main.get(&account).map(String::as_str), Some(main));
    assert!(app.session.mailbox.calendars.iter().any(|c| c.name == "Calendar" && c.origin.account == account));
    assert!(app.session.mailbox.calendars.iter().any(|c| c.name == "Team" && c.origin.id == shared));
    assert!(app.session.mailbox.events.values().any(|e| e.title == "Stand-up"));
    assert!(app.session.mailbox.events.values().any(|e| e.title == "Investigation event"));
    assert!(app.session.mailbox.events.values().any(|e| e.title == "Rome event"));
    let team = app.session.mailbox.calendars.iter().find(|c| c.name == "Team").unwrap().id;
    assert!(app.session.mailbox.events.values().any(|e| e.title == "Team event" && e.calendar == team));
    let contact = app.session.mailbox.contacts.values().find(|c| c.display_name == "Ada Lovelace").unwrap();
    assert_eq!(contact.book, "Suppliers");
}

#[test]
fn a_result_from_a_replaced_mailbox_changes_nothing() {
    let (mut app, account) = app();
    let (tx, rx) = mpsc::channel();
    // A job started before the mailbox changed: its opening is no longer the session's.
    let stale = app.session.openings().wrapping_sub(1);
    app.dav.rx = Some((account, stale, rx));
    let before = app.session.mailbox.calendars.len();
    tx.send(DavResult {
        account,
        calendars: Some(CalendarsDownload {
            account,
            email: "ada@example.org".into(),
            complete: true,
            calendars: vec![RemoteCalendar {
                id: "https://mail.example.org/caldav/ada@example.org/calendar/".into(),
                name: "Stale".into(),
                main: true,
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    app.poll_dav();
    assert_eq!(app.session.mailbox.calendars.len(), before);
    assert!(!app.session.mailbox.calendars.iter().any(|calendar| calendar.name == "Stale"));
}

#[test]
fn a_read_only_owner_calendar_is_not_an_upload_target() {
    let (mut app, account) = app();
    let (tx, rx) = mpsc::channel();
    app.dav.rx = Some((account, app.session.openings(), rx));
    tx.send(DavResult {
        account,
        calendars: Some(CalendarsDownload {
            account,
            email: "ada@example.org".into(),
            complete: true,
            calendars: vec![RemoteCalendar {
                id: "https://mail.example.org/caldav/ada@example.org/calendar/".into(),
                name: "Calendar".into(),
                main: true,
                rights: Rights::Read,
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    app.poll_dav();
    assert!(!app.dav.main.contains_key(&account), "nothing made here can be sent to a calendar that only reads");
}

#[test]
fn an_event_made_here_is_asked_for_at_once() {
    let (mut app, account) = app();
    let main = app.session.mailbox.alloc_id();
    app.session.mailbox.calendars.push(Calendar {
        id: main,
        name: "Calendar".into(),
        visible: true,
        origin: Origin { account, ..Default::default() },
        ..Default::default()
    });
    app.dav.main.insert(account, "https://mail.example.org/caldav/ada@example.org/calendar/".into());
    app.passwords.insert(account, "invented".into());
    app.auto_sync = true;
    app.services.dav = Some(Box::new(move |job: DavJob| {
        assert!(job.changes.as_ref().is_some_and(|upload| upload.ops.iter().any(|op| op.calendar.is_empty())), "the event made here is in the run");
        let (tx, rx) = mpsc::channel();
        tx.send(DavResult { account: job.account.id, ..Default::default() }).unwrap();
        rx
    }));
    app.execute("calendar.new", json!({"title": "Now"})).unwrap();
    assert!(app.dav.requested.contains(&account), "the change asks for a run");
    assert!(app.dav_tick_at(1_800_000_000_000.0), "the run starts");
}

#[test]
fn a_contact_sent_is_not_taken_away_by_the_list_read_before_it() {
    let (mut app, account) = app();
    let book = "https://mail.example.org/carddav/ada@example.org/contacts/personal/";
    let id = app.session.mailbox.add_contact(Contact {
        first: "Ada".into(),
        uid: "one-uid".into(),
        book: "My contacts".into(),
        book_id: book.into(),
        origin: Origin { account, ..Default::default() },
        ..Default::default()
    });
    let (tx, rx) = mpsc::channel();
    app.dav.rx = Some((account, app.session.openings(), rx));
    let remote = format!("{book}one-uid.vcf");
    tx.send(DavResult {
        account,
        // The read ran before the send, so it cannot know the contact was just made.
        contacts: Some(ContactsDownload {
            account,
            email: "ada@example.org".into(),
            complete: true,
            books: vec![RemoteAddressBook { id: book.into(), name: "My contacts".into(), rights: Rights::Edit }],
            contacts: Vec::new(),
        }),
        uploaded_contacts: Some(ContactsUploaded {
            account,
            email: "ada@example.org".into(),
            outcomes: vec![ContactOutcome {
                op: ContactOp {
                    kind: ContactChangeKind::Create,
                    contact: id,
                    book: book.into(),
                    uid: "one-uid".into(),
                    card: String::new(),
                    ..Default::default()
                },
                result: EventResult::Done,
                remote: remote.clone(),
                status: 201,
                ..Default::default()
            }],
        }),
        ..Default::default()
    })
    .unwrap();
    app.poll_dav();
    let contact = app.session.mailbox.contact(id).expect("the just-sent contact stays");
    assert_eq!(contact.origin.id, remote);
    assert_eq!(app.session.mailbox.contact_books[0].name, "My contacts");
}
