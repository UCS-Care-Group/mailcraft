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

/// A meeting Bo organizes that Ada attends, kept by account `a`'s server.
fn served_guest_meeting(s: &mut Session, a: Id) -> Id {
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
    serve(s, a, "ada@example.org", vec![theirs]);
    s.mailbox.events.values().find(|e| e.uid == "uid-1").map(|e| e.id).unwrap()
}

/// AU3-13: a meeting a server keeps is held to the same organizer checks as a local one. A
/// stranger's cancellation or invitation with its `UID`, naming herself or the real organizer,
/// is shown and changes nothing; the organizer's own still does.
#[test]
fn a_stranger_neither_calls_off_nor_answers_a_meeting_a_server_keeps() {
    for organizer in ["mallory@example.org", "bo@example.org"] {
        let (mut s, a) = session();
        let event = served_guest_meeting(&mut s, a);
        let cancel = mail(&mut s, a, "mallory@example.org", "CANCEL", "uid-1", organizer);
        assert!(!removable(&mut s, cancel), "{organizer}");
        assert!(refused(&mut s, cancel).contains("organizer"));
        assert!(s.mailbox.event(event).is_some() && s.mailbox.event_changes.is_empty());

        let request = mail(&mut s, a, "mallory@example.org", "REQUEST", "uid-1", organizer);
        let info = run(&mut s, "calendar.invitation", json!({"message": request}));
        assert_eq!(info["blocked"], "mismatch", "{organizer}: {info}");
        let answer = s.run("calendar.respond", &json!({"message": request, "response": "decline", "send": true}));
        assert!(matches!(answer, Err(CmdError::Failed(_) | CmdError::Said(_))), "{organizer}: {answer:?}");
        assert!(s.mailbox.event(event).is_some() && s.mailbox.event_changes.is_empty(), "{organizer}");
    }
    // The organizer's own word does what it says.
    let (mut s, a) = session();
    let event = served_guest_meeting(&mut s, a);
    let request = mail(&mut s, a, "bo@example.org", "REQUEST", "uid-1", "bo@example.org");
    assert!(run(&mut s, "calendar.invitation", json!({"message": request}))["blocked"].is_null());
    let cancel = mail(&mut s, a, "bo@example.org", "CANCEL", "uid-1", "bo@example.org");
    assert!(removable(&mut s, cancel));
    assert_eq!(run(&mut s, "calendar.removeCancelled", json!({"message": cancel}))["removed"].as_u64(), Some(event));
}

/// #755: a session without the window's settings (`mailcraft-cli run`, headless `mcp`) keeps an
/// answered request where it is; a saved choice, on or off, is read back as it was, and
/// preferences saved without the option leave it off. (A new install's window turns it on:
/// `ui-egui`'s `reading/tests/invitation.rs`.)
#[test]
fn a_session_without_the_windows_settings_keeps_an_answered_request_and_a_saved_choice_is_kept() {
    let (mut s, account) = session();
    assert!(!s.prefs.delete_answered_requests && !crate::Prefs::default().delete_answered_requests);
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let request = mail(&mut s, account, "bo@example.org", "REQUEST", "uid-1", "bo@example.org");
    let answer = run(&mut s, "calendar.respond", json!({"message": request, "response": "accept"}));
    assert_eq!((answer["deleted"].as_bool(), s.mailbox.message(request).map(|m| m.folder)), (Some(false), Some(inbox)));

    let saved = |on: bool| serde_json::to_value(crate::Prefs { delete_answered_requests: on, ..Default::default() }).unwrap();
    assert!(!serde_json::from_value::<crate::Prefs>(saved(false)).unwrap().delete_answered_requests);
    assert!(serde_json::from_value::<crate::Prefs>(saved(true)).unwrap().delete_answered_requests);
    assert!(!serde_json::from_value::<crate::Prefs>(json!({"weekNumbers": true})).unwrap().delete_answered_requests);

    // An owner who turned it on: the answered request leaves the Inbox for Deleted Items.
    let (mut s, account) = session();
    s.prefs = serde_json::from_value(saved(true)).unwrap();
    let bin = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let request = mail(&mut s, account, "bo@example.org", "REQUEST", "uid-1", "bo@example.org");
    let answer = run(&mut s, "calendar.respond", json!({"message": request, "response": "accept"}));
    assert_eq!((answer["deleted"].as_bool(), s.mailbox.message(request).map(|m| m.folder)), (Some(true), Some(bin)));
}

/// #755: only the Inbox is cleared, as the option says and as Outlook does: a request in another
/// folder (one a rule filed it in, or a folder under the Inbox) stays where it is, answered.
#[test]
fn an_answered_request_outside_the_inbox_stays_where_it_is() {
    let (mut s, account) = session();
    s.prefs.delete_answered_requests = true;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let filed = s.mailbox.add_folder(account, None, "Meetings").unwrap();
    let under = s.mailbox.add_folder(account, Some(inbox), "Invitations").unwrap();
    for (n, folder) in [filed, under].into_iter().enumerate() {
        let request = mail(&mut s, account, "bo@example.org", "REQUEST", &format!("uid-{n}"), "bo@example.org");
        s.mailbox.message_mut(request).unwrap().folder = folder;
        s.mailbox_changed();
        let answer = run(&mut s, "calendar.respond", json!({"message": request, "response": "accept"}));
        assert_eq!((answer["deleted"].as_bool(), s.mailbox.message(request).map(|m| m.folder)), (Some(false), Some(folder)));
        assert!(s.mailbox.message(request).unwrap().answered);
    }
    // A cancellation from the organizer outside the Inbox likewise: the meeting goes, the mail stays.
    let cancel = mail(&mut s, account, "bo@example.org", "CANCEL", "uid-0", "bo@example.org");
    s.mailbox.message_mut(cancel).unwrap().folder = filed;
    s.mailbox_changed();
    let removed = run(&mut s, "calendar.removeCancelled", json!({"message": cancel}));
    assert_eq!((removed["deleted"].as_bool(), s.mailbox.message(cancel).map(|m| m.folder)), (Some(false), Some(filed)));
}
