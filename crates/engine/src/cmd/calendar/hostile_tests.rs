// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Hostile calendar data (a time zone that costs a great deal to use, series that count from
//! 1970, a file of thousands of events) is answered at once and never freezes the window.

use std::time::{Duration, Instant};

use mailcraft_model::{Address, Attachment, FolderKind, Id, Message};
use mailcraft_time::{DateTime, Timestamp};
use serde_json::json;

use crate::Session;

fn now() -> Timestamp {
    DateTime::new(mailcraft_time::Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(now(), 0), now(), 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "name": "Ada", "incoming": {"host": "imap.example.org"}})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn carrying(s: &mut Session, account: Id, calendar: Vec<u8>) -> Id {
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let message = Message {
        folder: inbox,
        message_id: format!("{}@example.org", calendar.len()),
        from: Address::new("Bo Marsh", "bo@example.org"),
        to: vec![Address::new("Ada", "ada@example.org")],
        subject: "Design review".into(),
        attachments: vec![Attachment::new("invite.ics", "text/calendar", calendar)],
        ..Default::default()
    };
    s.mailbox.add_message(message).unwrap()
}

fn zone_bomb() -> String {
    let parts: String = (0..250)
        .map(|i| {
            let dates: String = (0..200).map(|d| format!("RDATE:{}0101T020000\r\n", 1600 + d)).collect();
            format!("BEGIN:STANDARD\r\nDTSTART:{}0101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0100\r\n{dates}END:STANDARD\r\n", 1000 + i)
        })
        .collect();
    let exdates: Vec<String> = (0..5000).map(|i| format!("{}{:02}{:02}T100000", 1990 + i / 300, 1 + (i / 28) % 12, 1 + i % 28)).collect();
    format!(
        "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VTIMEZONE\r\nTZID:Z0\r\n{parts}END:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:bomb\r\nSUMMARY:Review\r\n\
         DTSTART;TZID=Z0:20261015T140000\r\nDTEND;TZID=Z0:20261015T150000\r\nRRULE:FREQ=DAILY\r\nEXDATE;TZID=Z0:{}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        exdates.join(",")
    )
}

#[test]
fn an_invitation_with_a_costly_time_zone_is_read_in_moments_and_the_card_stays() {
    let (mut s, account) = session();
    let message = carrying(&mut s, account, zone_bomb().into_bytes());
    let started = Instant::now();
    let said = s.run("calendar.invitation", &json!({"message": message})).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert_eq!(said["title"], "Review");
}

#[test]
fn the_card_reads_the_first_event_only() {
    let (mut s, account) = session();
    let mut text = String::from("BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\n");
    for i in 0..2_000 {
        text.push_str(&format!("BEGIN:VEVENT\r\nUID:u{i}\r\nSUMMARY:Event {i}\r\nDTSTART:20261015T140000Z\r\nEND:VEVENT\r\n"));
    }
    text.push_str("END:VCALENDAR\r\n");
    let message = carrying(&mut s, account, text.into_bytes());
    let said = s.run("calendar.invitation", &json!({"message": message})).unwrap();
    assert_eq!(said["title"], "Event 0");
}

#[test]
fn an_imported_file_of_counting_series_is_cheap_to_look_at() {
    let (mut s, _) = session();
    let mut text = String::from("BEGIN:VCALENDAR\r\n");
    for i in 0..20_000 {
        text.push_str(&format!(
            "BEGIN:VEVENT\r\nUID:c{i}\r\nSUMMARY:Series {i}\r\nDTSTART:19700101T090000Z\r\nDTEND:19700101T100000Z\r\n\
             RRULE:FREQ=DAILY;COUNT=4000000000\r\nBEGIN:VALARM\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\n"
        ));
    }
    text.push_str("END:VCALENDAR\r\n");
    let started = Instant::now();
    crate::io::import(&mut s, "bomb.ics", text.as_bytes(), None).unwrap();
    assert!(started.elapsed() < Duration::from_secs(20), "import {:?}", started.elapsed());
    let started = Instant::now();
    let local = s.local_now();
    let _ = crate::cmd::view::due_reminders(&s, local);
    s.run("calendar.view", &json!({})).ok();
    assert!(started.elapsed() < Duration::from_secs(10), "view {:?}", started.elapsed());
}
