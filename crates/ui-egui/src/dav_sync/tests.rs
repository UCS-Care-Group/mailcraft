// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The DAV job: what it reads, what it takes in and when it is asked for.

use super::*;
use mailcraft_engine::Session;
use mailcraft_model::remote::{CalendarsDownload, ContactsDownload, EventsDownload, RemoteCalendar, RemoteContact, RemoteEvent};
use mailcraft_model::{Account, AccountKind, Calendar, DavServer, Mailbox, Origin, Security, Server};
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
        dav: Some(DavServer {
            caldav: "https://mail.example.org/caldav/".into(),
            carddav: "https://mail.example.org/carddav/".into(),
            ..Default::default()
        }),
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
                        id: "/caldav/ada@example.org/Investigation/c3ed%2540example.org.ics".into(),
                        uid: "c3ed@example.org".into(),
                        title: "Investigation event".into(),
                        start: 1_800_003_600,
                        end: 1_800_007_200,
                        start_zone: "UTC".into(),
                        ..Default::default()
                    },
                    RemoteEvent {
                        id: "/caldav/ada@example.org/Investigation/28cd%2540example.org.ics".into(),
                        uid: "28cd@example.org".into(),
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

/// An account with calendars and address books alone, on another provider than any mail.
fn calendar_app() -> (MailApp, Id) {
    let mut mailbox = Mailbox::with_local_account("", "");
    let id = mailbox.add_account(Account {
        email: "ada@example.org".into(),
        display_name: "Ada".into(),
        kind: AccountKind::Dav,
        dav: Some(DavServer {
            caldav: "https://cloud.example.org/remote.php/dav/".into(),
            carddav: "https://cloud.example.org/remote.php/dav/".into(),
            username: "ada-cloud".into(),
        }),
        ..Default::default()
    });
    (MailApp::new(Session::at(mailbox, 1_800_000_000, 0), crate::Services::default()), id)
}

#[test]
fn a_calendar_only_account_signs_in_with_its_own_name_and_password() {
    let (mut app, account) = calendar_app();
    app.passwords.insert(account, "mail password".into());
    app.adopt_dav_password(account, "calendar password".into());
    app.auto_sync = true;
    app.services.dav = Some(Box::new(move |job: DavJob| {
        assert_eq!(job.username, "ada-cloud");
        assert_eq!(job.password, "calendar password");
        let (tx, rx) = mpsc::channel();
        tx.send(DavResult { account: job.account.id, ..Default::default() }).unwrap();
        rx
    }));
    assert!(app.dav_tick_at(1_800_000_000_000.0), "a calendar-only account is read too");
}

#[test]
fn calendars_at_another_provider_never_get_the_mail_password() {
    let (mut app, account) = app();
    app.session.mailbox.account_mut(account).unwrap().dav = Some(DavServer {
        caldav: "https://cloud.example.org/caldav/".into(),
        carddav: "https://cloud.example.org/carddav/".into(),
        ..Default::default()
    });
    app.adopt_password(account, "the mail account's password".into());
    app.auto_sync = true;
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = asked.clone();
    app.services.dav = Some(Box::new(move |job: DavJob| {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        tx.send(DavResult { account: job.account.id, ..Default::default() }).unwrap();
        rx
    }));
    // Another host gets a sign-in of its own or nothing: the mail password is never sent there.
    assert!(!app.dav_tick_at(1_800_000_000_000.0));
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 0);
    // With the calendars' own password, the read goes ahead.
    app.adopt_dav_password(account, "the calendars' password".into());
    assert!(app.dav_tick_at(1_800_000_000_100.0));
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn a_mail_account_whose_calendars_share_its_sign_in_uses_its_password() {
    let (mut app, account) = app();
    app.adopt_password(account, "the account's password".into());
    app.auto_sync = true;
    app.services.dav = Some(Box::new(move |job: DavJob| {
        assert_eq!(job.username, "ada@example.org");
        assert_eq!(job.password, "the account's password");
        let (tx, rx) = mpsc::channel();
        tx.send(DavResult { account: job.account.id, ..Default::default() }).unwrap();
        rx
    }));
    assert!(app.dav_tick_at(1_800_000_000_000.0));
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
fn days_that_come_into_view_are_read_at_once() {
    use std::sync::{Arc, Mutex};
    let (mut app, account) = app();
    app.passwords.insert(account, "invented".into());
    app.auto_sync = true;
    let asked: Arc<Mutex<Vec<(Timestamp, Timestamp)>>> = Arc::default();
    let seen = asked.clone();
    app.services.dav = Some(Box::new(move |job: DavJob| {
        seen.lock().unwrap().push((job.from, job.to));
        let (tx, rx) = mpsc::channel();
        tx.send(DavResult { account: job.account.id, ..Default::default() }).unwrap();
        rx
    }));
    // The first look reads the days on screen.
    assert!(app.dav_tick_at(1_800_000_000_000.0), "the first run starts");
    app.poll_dav();
    // Shortly after, with the view where it was, nothing is asked for again.
    assert!(!app.dav_tick_at(1_800_000_000_100.0));
    // A week on: the days in view were never read, so the run starts without waiting for
    // Send/Receive or the five-minute tick.
    app.session.view.date = app.session.view.date.add_days(7);
    assert!(app.dav_tick_at(1_800_000_000_200.0), "the new days are asked for at once");
    app.poll_dav();
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 2, "two runs were asked for");
    assert!(asked[1].0 > asked[0].0, "the second run asks for later days");
    // And not once more: those days were just read.
    assert!(!app.dav_tick_at(1_800_000_000_300.0));
}
