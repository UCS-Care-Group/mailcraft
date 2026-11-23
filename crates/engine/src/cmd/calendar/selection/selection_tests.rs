// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `calendar.paste` checks a whole batch before it changes any of it, on its own: these call the
//! command's function directly, without `Session::run`, whose rollback would hide a refusal that
//! came halfway through.

use super::paste;
use crate::Session;
use crate::time::{Date, DateTime};
use mailcraft_model::{Calendar, Event, Id, Origin, Rgb};
use serde_json::{Value, json};

/// Wednesday 2026-10-14 16:00 UTC: the fixed clock the engine's calendar tests use.
const NOW: i64 = 1_791_993_600;

fn at(month: u8, day: u8, hour: u8) -> DateTime {
    DateTime::new(Date::new(2026, month, day).unwrap(), hour, 0, 0).unwrap()
}

/// An account whose server keeps two calendars; in the first, "Eye test" (nothing attached) and
/// "Contract" (files attached). Returns the session, the second calendar and the two events.
fn session() -> (Session, Id, Id, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let account = s.run("account.add", &json!({"email": "zoe@example.org", "name": "Zoe", "incoming": {"host": "imap.example.org"}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    let calendar = |s: &mut Session, server_id: &str| {
        let id = s.mailbox.alloc_id();
        s.mailbox.calendars.push(Calendar {
            id,
            name: server_id.into(),
            color: Rgb(0x2E, 0x8B, 0x57),
            visible: true,
            origin: Origin { account, id: server_id.into(), ..Default::default() },
            ..Default::default()
        });
        id
    };
    let (first, second) = (calendar(&mut s, "srv-first"), calendar(&mut s, "srv-second"));
    let mut kept = |title: &str, server_id: &str, start: DateTime, files: bool| {
        s.mailbox
            .add_event(Event {
                calendar: first,
                title: title.into(),
                start,
                end: start.add_minutes(60),
                has_attachments: files,
                origin: Origin { account, id: server_id.into(), ..Default::default() },
                ..Default::default()
            })
            .unwrap()
    };
    let eye_test = kept("Eye test", "srv-eye", at(11, 13, 9), false);
    let contract = kept("Contract", "srv-contract", at(11, 16, 9), true);
    (s, second, eye_test, contract)
}

fn refused_before_anything_changes(s: &mut Session, params: &Value) {
    let before = s.mailbox.clone();
    assert!(paste(s, params).is_err(), "{params}: refused");
    assert!(s.mailbox == before, "{params}: refused before the item ahead of the one that can't go was made or moved");
}

#[test]
fn a_copy_with_an_item_that_cant_be_copied_makes_none_of_the_others() {
    let (mut s, _, eye_test, contract) = session();
    refused_before_anything_changes(&mut s, &json!({"items": [{"id": eye_test}, {"id": contract}], "start": "2026-11-20T09:00"}));
}

#[test]
fn a_cut_with_an_item_that_cant_go_to_that_calendar_moves_none_of_the_others() {
    let (mut s, second, eye_test, contract) = session();
    refused_before_anything_changes(
        &mut s,
        &json!({"items": [{"id": eye_test}, {"id": contract}], "start": "2026-11-20T09:00", "cut": true, "calendar": second}),
    );
}
