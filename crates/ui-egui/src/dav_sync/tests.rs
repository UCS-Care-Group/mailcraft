// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The DAV job: what it reads, what it takes in and when it is asked for.

use super::*;
use mailcraft_engine::Session;
use mailcraft_model::remote::{CalendarsDownload, ContactsDownload, EventsDownload, RemoteCalendar, RemoteContact, RemoteEvent};
use mailcraft_model::{Account, AccountKind, DavServer, Mailbox, Security, Server};
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
