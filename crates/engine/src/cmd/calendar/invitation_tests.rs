// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A cancellation in the mail calls a meeting off only when it comes from the meeting's organizer
//! (a local calendar) or from the account that keeps it (a server): what a message says of the
//! organizer is the sender's word.

use mailcraft_model::remote::{EventsDownload, RemoteEvent};
use mailcraft_model::{Address, Attachment, Attendee, FolderKind, Id, Message};
use mailcraft_time::{Date, DateTime, Timestamp};
use serde_json::{Value, json};

use crate::{CmdError, Session, cmd::remote};

fn clock(day: u8, hour: u8) -> DateTime {
    DateTime::new(Date::new(2026, 10, day).unwrap(), hour, 0, 0).unwrap()
}

fn now() -> Timestamp {
    clock(14, 16).to_timestamp(0)
}

fn account(s: &mut Session, email: &str) -> Id {
    let added = s.run("account.add", &json!({"email": email, "name": "Ada", "incoming": {"host": "imap.example.org"}})).unwrap();
    added["id"].as_u64().unwrap()
}

fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(now(), 0), now(), 0);
    let id = account(&mut s, "ada@example.org");
    (s, id)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// A message with a calendar part for the meeting `uid`, organized by `organizer`, sent from `from`.
fn mail(s: &mut Session, account: Id, from: &str, method: &str, uid: &str, organizer: &str) -> Id {
    let ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:{method}\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:Design review\r\n\
         DTSTART:20261015T140000Z\r\nDTEND:20261015T150000Z\r\nORGANIZER;CN=Org:mailto:{organizer}\r\n\
         ATTENDEE;CN=Ada:mailto:ada@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let message = Message {
        folder: inbox,
        message_id: format!("{method}-{uid}-{from}-{organizer}@example.org"),
        from: Address::new("Sender", from),
        to: vec![Address::new("Ada", "ada@example.org")],
        subject: "Design review".into(),
        attachments: vec![Attachment::new("invite.ics", "text/calendar", ics.into_bytes())],
        ..Default::default()
    };
    s.mailbox.add_message(message).unwrap()
}

fn removable(s: &mut Session, message: Id) -> bool {
    run(s, "calendar.invitation", json!({"message": message}))["removable"].as_bool().unwrap()
}

fn refused(s: &mut Session, message: Id) -> String {
    match s.run("calendar.removeCancelled", &json!({"message": message})) {
        Err(e @ (CmdError::Failed(_) | CmdError::Said(_))) => e.to_string(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn only_the_organizer_calls_off_a_meeting_on_a_local_calendar() {
    let (mut s, a) = session();
    let request = mail(&mut s, a, "bo@example.org", "REQUEST", "uid-bo", "bo@example.org");
    let event = run(&mut s, "calendar.respond", json!({"message": request, "response": "accept"}))["event"].as_u64().unwrap();
    // A cancellation that names another organizer.
    let other = mail(&mut s, a, "mallory@example.org", "CANCEL", "uid-bo", "mallory@example.org");
    assert!(!removable(&mut s, other));
    assert!(refused(&mut s, other).contains("organizer"));
    // One that names the real organizer but is sent from another address.
    let spoofed = mail(&mut s, a, "mallory@example.org", "CANCEL", "uid-bo", "bo@example.org");
    assert!(!removable(&mut s, spoofed));
    assert!(refused(&mut s, spoofed).contains("organizer"));
    assert!(s.mailbox.event(event).is_some());
    // The organizer's own (any case) calls it off.
    let real = mail(&mut s, a, "BO@example.org", "CANCEL", "uid-bo", "bo@example.org");
    assert!(removable(&mut s, real));
    assert_eq!(run(&mut s, "calendar.removeCancelled", json!({"message": real}))["removed"].as_u64(), Some(event));
    assert!(s.mailbox.event(event).is_none());
}

fn serve(s: &mut Session, account: Id, email: &str, events: Vec<RemoteEvent>) {
    let download = EventsDownload {
        account,
        email: email.into(),
        from: clock(1, 0).to_timestamp(0),
        to: clock(31, 0).to_timestamp(0),
        complete: true,
        events,
        ..Default::default()
    };
    remote::apply_events(s, &download).unwrap();
}

#[test]
fn a_cancellation_in_one_account_leaves_a_meeting_another_account_keeps() {
    let (mut s, a) = session();
    let b = account(&mut s, "ada@example.net");
    let start = clock(15, 14).to_timestamp(0);
    let theirs = RemoteEvent {
        id: "g-1".into(),
        change_key: "ck-1".into(),
        uid: "uid-1".into(),
        title: "Design review".into(),
        start,
        end: start + 3_600,
        organizer: Some(Address::new("Bo", "bo@example.org")),
        attendees: vec![Attendee { address: Address::new("Ada", "ada@example.org"), ..Default::default() }],
        is_organizer: Some(false),
        ..Default::default()
    };
    serve(&mut s, a, "ada@example.org", vec![theirs]);
    assert_eq!(s.mailbox.events.values().filter(|e| e.uid == "uid-1").count(), 1);
    // The cancellation arrived in the other account: shown, not offered.
    let stray = mail(&mut s, b, "bo@example.org", "CANCEL", "uid-1", "bo@example.org");
    assert!(!removable(&mut s, stray));
    assert!(refused(&mut s, stray).contains("organizer"));
    assert_eq!(s.mailbox.events.values().filter(|e| e.uid == "uid-1").count(), 1);
    // In the account that keeps it, it is.
    let own = mail(&mut s, a, "bo@example.org", "CANCEL", "uid-1", "bo@example.org");
    assert!(removable(&mut s, own));
}
