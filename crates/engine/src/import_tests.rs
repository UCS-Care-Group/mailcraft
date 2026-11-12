// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a file brought in may and may not do: replace a meeting only for its organizer, and
//! never put mail in a send queue.

use mailcraft_model::{FolderKind, Id};
use mailcraft_time::{Date, DateTime, Timestamp};
use serde_json::json;

use crate::Session;

fn now() -> Timestamp {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(now(), 0), now(), 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "name": "Ada", "incoming": {"host": "imap.example.org"}})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn ics(uid: &str, title: &str, organizer: &str) -> Vec<u8> {
    let organizer = if organizer.is_empty() { String::new() } else { format!("ORGANIZER;CN=X:mailto:{organizer}\r\n") };
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{title}\r\nDTSTART:20261015T140000Z\r\nDTEND:20261015T150000Z\r\n{organizer}END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    .into_bytes()
}

#[test]
fn an_ics_file_replaces_a_meeting_only_for_the_same_organizer() {
    let (mut s, _) = session();
    crate::io::import(&mut s, "a.ics", &ics("uid-1", "Design review", "bo@example.org"), None).unwrap();
    let first: Vec<_> = s.mailbox.events.values().filter(|e| e.uid == "uid-1").map(|e| (e.id, e.title.clone())).collect();
    assert_eq!(first.len(), 1);
    // Another organizer, same UID: the meeting stays, the file's event is added beside it.
    let said = crate::io::import(&mut s, "b.ics", &ics("uid-1", "Pay the invoice", "mallory@example.org"), None).unwrap();
    assert_eq!(said["updated"], 0);
    let titles: Vec<_> = s.mailbox.events.values().filter(|e| e.uid == "uid-1").map(|e| e.title.clone()).collect();
    assert_eq!(titles.len(), 2, "{titles:?}");
    assert_eq!(s.mailbox.event(first[0].0).unwrap().title, "Design review");
    // No organizer in the file does not replace one that has an organizer either.
    crate::io::import(&mut s, "c.ics", &ics("uid-1", "Nobody's", ""), None).unwrap();
    assert_eq!(s.mailbox.event(first[0].0).unwrap().title, "Design review");
    // The same organizer (any case) is an update.
    let said = crate::io::import(&mut s, "d.ics", &ics("uid-1", "Design review 2", "BO@example.org"), None).unwrap();
    assert_eq!(said["updated"], 1);
    assert_eq!(s.mailbox.event(first[0].0).unwrap().title, "Design review 2");
    // The owner's own appointment (no organizer), exported and imported again, is updated.
    crate::io::import(&mut s, "e.ics", &ics("uid-own", "Dentist", ""), None).unwrap();
    let said = crate::io::import(&mut s, "e.ics", &ics("uid-own", "Dentist", ""), None).unwrap();
    assert_eq!(said["updated"], 1);
    // ... but an organizer's meeting does not take it over.
    let said = crate::io::import(&mut s, "f.ics", &ics("uid-own", "Pay", "mallory@example.org"), None).unwrap();
    assert_eq!(said["updated"], 0);
}

#[test]
fn an_ics_file_is_cut_to_size_like_an_invitation() {
    let (mut s, _) = session();
    let long = "x".repeat(5_000);
    crate::io::import(&mut s, "a.ics", &ics("uid-long", &long, "bo@example.org"), None).unwrap();
    let event = s.mailbox.events.values().find(|e| e.uid == "uid-long").unwrap();
    assert!(event.title.chars().count() < 1_000, "{}", event.title.len());
}

#[test]
fn an_import_never_puts_mail_in_a_send_queue() {
    let (mut s, account) = session();
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let drafts = s.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
    let eml = b"From: eve@example.net\r\nTo: ada@example.org\r\nSubject: hello\r\nMessage-ID: <a@example.net>\r\n\r\nbody\r\n";
    // From the view: the account's Drafts.
    s.view.folder = outbox;
    let said = crate::io::import(&mut s, "m.eml", eml, None).unwrap();
    assert_eq!(said["folder"].as_u64(), Some(drafts));
    assert_eq!(s.mailbox.messages_in(outbox).count(), 0);
    let id = said["ids"][0].as_u64().unwrap();
    assert!(s.mailbox.message(id).unwrap().draft);
    // Asked for by name: refused, and nothing is added.
    let before = s.mailbox.messages.len();
    assert!(crate::io::import(&mut s, "m.eml", eml, Some(outbox)).is_err());
    assert_eq!(s.mailbox.messages.len(), before);
    // A folder under an Outbox is one too.
    if let Ok(sub) = s.mailbox.add_folder(account, Some(outbox), "Queue") {
        assert!(crate::io::import(&mut s, "m.eml", eml, Some(sub)).is_err());
        s.view.folder = sub;
        let said = crate::io::import(&mut s, "m.eml", eml, None).unwrap();
        assert_eq!(said["folder"].as_u64(), Some(drafts));
    }
    // Nothing is waiting to be sent.
    assert_eq!(s.mailbox.messages_in(outbox).count(), 0);
}

/// A mailbox with an account that signs in with Microsoft and whose calendar has come down once:
/// the empty calendar the mailbox was made with is gone, and the server's is the only one.
fn served() -> (Session, Id, Id) {
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), now(), 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap();
    let account = added["id"].as_u64().unwrap();
    let start = now().saturating_add(3600);
    let download = json!({"download": {"account": account, "email": "ada@example.org", "from": now(), "to": now().saturating_add(86_400 * 7), "complete": true,
        "events": [{"id": "e-1", "changeKey": "ck-1", "title": "Stand-up", "start": start, "end": start.saturating_add(900)}]}});
    s.run("sync.applyEvents", &download).unwrap();
    let server = crate::cmd::remote::server_calendar_id(&s.mailbox).unwrap();
    assert_eq!(s.mailbox.calendars.iter().map(|c| c.id).collect::<Vec<_>>(), [server], "only the server's calendar is left");
    (s, account, server)
}

/// The calendar `event` is in is kept on this computer, and says so.
fn kept_here(s: &Session, event: Id) -> bool {
    let calendar = s.mailbox.event(event).and_then(|e| s.mailbox.calendar(e.calendar)).unwrap();
    crate::cmd::remote::keeper(&s.mailbox, &calendar.origin).is_none() && crate::cmd::remote::stays_here(&s.mailbox, event)
}

#[test]
fn an_ics_file_beside_a_server_calendar_goes_to_this_computer_and_can_be_moved() {
    // #1050: with the server's calendar the only one, an imported appointment went into it as
    // the owner's own: shown there, never sent, and with nothing offering to send it.
    let (mut s, account, server) = served();
    let said = crate::io::import(&mut s, "delivery.ics", &ics("uid-delivery", "Grocery delivery", ""), None).unwrap();
    assert_eq!(said["events"], 1);
    let id = s.sel.event.unwrap();
    assert!(kept_here(&s, id));
    let home = s.mailbox.event(id).unwrap().calendar;
    assert_ne!(home, server);
    assert_eq!(s.mailbox.calendar(home).unwrap().name, crate::cmd::remote::LOCAL_CALENDAR);
    assert!(s.mailbox.event_changes.is_empty(), "nothing is sent unasked");
    // So the line over the calendar counts it, and the menu's Move to Calendar has the server's.
    assert_eq!(crate::cmd::remote::only_here(&s.mailbox), [(home, 1)]);
    // The same file again replaces it where it is.
    let again = crate::io::import(&mut s, "delivery.ics", &ics("uid-delivery", "Grocery delivery", ""), None).unwrap();
    assert_eq!((again["updated"].as_u64(), s.mailbox.event(id).map(|e| e.calendar)), (Some(1), Some(home)));
    // Moved to the account, it is made on the server.
    s.run("calendar.moveToAccount", &json!({})).unwrap();
    let moved = s.mailbox.event(id).unwrap();
    assert_eq!((moved.calendar, moved.origin.account), (server, account));
    assert_eq!(s.mailbox.event_changes.len(), 1);
    assert!(crate::cmd::remote::only_here(&s.mailbox).is_empty());
}

#[test]
fn an_appointment_an_earlier_import_left_in_a_server_calendar_comes_back_to_this_computer() {
    // What a version before #1050 left: the owner's own appointment in the server's calendar.
    let (mut s, account, server) = served();
    let start = s.local_now();
    let stray = s
        .mailbox
        .add_event(mailcraft_model::Event {
            title: "Grocery delivery".into(),
            calendar: server,
            start,
            end: start.add_minutes(60),
            ..Default::default()
        })
        .unwrap();
    assert!(!crate::cmd::remote::stays_here(&s.mailbox, stray) && crate::cmd::remote::only_here(&s.mailbox).is_empty());
    // The next download puts it in a calendar on this computer, from where it can be moved.
    let download = json!({"download": {"account": account, "email": "ada@example.org", "from": now(), "to": now().saturating_add(86_400), "complete": false, "events": []}});
    s.run("sync.applyEvents", &download).unwrap();
    assert!(kept_here(&s, stray));
    assert_eq!(s.mailbox.calendar(s.mailbox.event(stray).unwrap().calendar).unwrap().name, crate::cmd::remote::LOCAL_CALENDAR);
    assert!(s.mailbox.event_changes.is_empty());
    assert!(s.mailbox.clone().repair().is_empty());
}
