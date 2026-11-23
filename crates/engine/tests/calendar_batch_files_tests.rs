// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1664 for a batch: an item with files attached keeps them on its server only, so a copy made
//! from here would be without them. Copy Event refuses it, and a batch paste of copies refuses it
//! the same way, whole, before anything is made. A cut keeps the item itself, files and all.
//!
//! Synthetic events on `example.org` only, with a fixed clock; no files, servers or accounts with
//! a real server. These are integration tests of the public engine API.

use mailcraft_engine::Session;
use mailcraft_engine::time::{Date, DateTime};
use mailcraft_model::{Calendar, Event, Origin, Rgb};
use serde_json::{Value, json};

/// Wednesday 2026-10-14 16:00 UTC: the fixed clock the engine's calendar tests use.
const NOW: i64 = 1_791_993_600;

const WHY: &str = "This item has files attached, which only the server keeps and which a copy would be without: it can't be copied from here.";

fn at(month: u8, day: u8, hour: u8) -> DateTime {
    DateTime::new(Date::new(2026, month, day).expect("a date that exists"), hour, 0, 0).expect("a time of day that exists")
}

/// The sample mailbox with a second account whose server keeps a calendar holding two
/// appointments: "Contract", with files attached, and "Eye test", without.
/// Returns the session, the server's calendar, the local calendar, and the two events.
fn session() -> (Session, u64, u64, u64, u64) {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let local = s.mailbox.default_calendar().expect("the sample mailbox has a local calendar");
    let account = s
        .run("account.add", &json!({"email": "zoe@example.org", "name": "Zoe", "incoming": {"host": "imap.example.org"}}))
        .expect("an account is added")["id"]
        .as_u64()
        .expect("an account id");
    let calendar = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar {
        id: calendar,
        name: "The account's".into(),
        color: Rgb(0x2E, 0x8B, 0x57),
        visible: true,
        origin: Origin { account, ..Default::default() },
        ..Default::default()
    });
    let kept = |s: &mut Session, title: &str, server_id: &str, start: DateTime, files: bool| {
        s.mailbox
            .add_event(Event {
                calendar,
                title: title.into(),
                start,
                end: start.add_minutes(60),
                has_attachments: files,
                origin: Origin { account, id: server_id.into(), ..Default::default() },
                ..Default::default()
            })
            .expect("a server calendar takes an appointment")
    };
    let contract = kept(&mut s, "Contract", "srv-contract", at(11, 16, 9), true);
    let eye_test = kept(&mut s, "Eye test", "srv-eye", at(11, 13, 9), false);
    (s, calendar, local, contract, eye_test)
}

fn refusal(s: &mut Session, command: &str, params: &Value) -> String {
    let mailbox = s.mailbox.clone();
    let error = s.run(command, params).expect_err("the copy is refused").to_string();
    assert!(s.mailbox == mailbox, "{command} {params}: nothing is made or noted for the server");
    error
}

#[test]
fn a_batch_copy_of_an_item_with_files_attached_is_refused_whole() {
    let (mut s, calendar, local, contract, eye_test) = session();
    for params in [
        // The item alone, into its own calendar, into the account's own, and into one on this computer.
        json!({"items": [{"id": contract}], "start": "2026-11-20T09:00"}),
        json!({"items": [{"id": contract}], "start": "2026-11-20T09:00", "calendar": calendar}),
        json!({"items": [{"id": contract}], "start": "2026-11-20T09:00", "calendar": local}),
        // With an item that could be copied, before it and after it: neither is made.
        json!({"items": [{"id": eye_test}, {"id": contract}], "start": "2026-11-20T09:00"}),
        json!({"items": [{"id": contract}, {"id": eye_test}], "start": "2026-11-20T09:00", "calendar": local}),
        json!({"items": [{"id": eye_test}, {"id": contract}], "start": "2026-11-20T09:00", "cut": false}),
    ] {
        assert!(refusal(&mut s, "calendar.paste", &params).contains(WHY), "{params}");
    }
}

#[test]
fn a_batch_copy_says_what_copy_event_says() {
    let (mut s, _, local, contract, _) = session();
    let one = refusal(&mut s, "calendar.copy", &json!({"id": contract, "start": "2026-11-20T09:00", "calendar": local}));
    let batch = refusal(&mut s, "calendar.paste", &json!({"items": [{"id": contract}], "start": "2026-11-20T09:00", "calendar": local}));
    assert_eq!(one, batch);
    assert!(one.contains(WHY), "{one}");
}

#[test]
fn items_without_files_are_still_copied_and_a_cut_keeps_the_item_and_its_files() {
    let (mut s, calendar, _, contract, eye_test) = session();
    let made = s.run("calendar.paste", &json!({"items": [{"id": eye_test}], "start": "2026-11-20T09:00"})).expect("an item without files is copied");
    let copy = made["ids"][0].as_u64().expect("the copy's id");
    assert!(s.mailbox.event(copy).is_some_and(|e| e.calendar == calendar && e.title == "Eye test" && e.start == at(11, 20, 9)));

    // A cut in its own calendar moves the item itself, so its files stay with it on the server.
    let moved = s
        .run("calendar.paste", &json!({"items": [{"id": contract}], "start": "2026-11-23T09:00", "cut": true}))
        .expect("a cut keeps the item, files and all");
    assert_eq!(moved["ids"], json!([contract]));
    assert!(s.mailbox.event(contract).is_some_and(|e| e.has_attachments && e.start == at(11, 23, 9) && e.calendar == calendar));
}

#[test]
fn a_batch_cut_of_an_item_with_files_into_another_calendar_is_refused_as_a_move_is() {
    // (The rule a move into another calendar has had since before #1664: the files would be lost.)
    let (mut s, _, _, contract, eye_test) = session();
    let account = s.mailbox.event(contract).map(|e| e.origin.account).expect("the fixture's event");
    let side = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar {
        id: side,
        name: "The account's other".into(),
        color: Rgb(0x8B, 0x2E, 0x57),
        visible: true,
        origin: Origin { account, id: "srv-side".into(), ..Default::default() },
        ..Default::default()
    });
    let why = refusal(
        &mut s,
        "calendar.paste",
        &json!({"items": [{"id": eye_test}, {"id": contract}], "start": "2026-11-20T09:00", "cut": true, "calendar": side}),
    );
    assert!(why.contains("can't be moved to another calendar from here"), "{why}");
}
