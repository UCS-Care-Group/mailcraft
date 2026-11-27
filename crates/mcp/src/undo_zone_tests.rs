// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent's undo is classed by what it does to the mailbox it runs on (#1674): when the
//! computer has been carried to another time zone since the last command, the zone is settled
//! before the gate reads the session, as `Session::run` settles it before the undo.

use mailcraft_engine::Session;
use mailcraft_engine::model::Id;
use mailcraft_engine::model::remote::{EventsDownload, RemoteEvent};
use mailcraft_engine::time::{Date, DateTime, Timestamp, Zone};
use serde_json::{Value, json};

use crate::Headless;
use crate::backend::Backend;

fn at(day: u8, hour: u8) -> Timestamp {
    DateTime::new(Date::new(2026, 10, day).unwrap(), hour, 0, 0).unwrap().to_timestamp(0)
}

fn event(id: &str, title: &str, start: Timestamp) -> RemoteEvent {
    RemoteEvent { id: id.into(), change_key: format!("ck-{id}-1"), title: title.into(), start, end: start + 3600, ..Default::default() }
}

/// All of October 2026 for `account`, complete, as a host hands it over.
fn october(account: Id, events: Vec<RemoteEvent>) -> Value {
    let download =
        EventsDownload { account, email: "ada@example.org".into(), from: at(1, 0), to: at(31, 0), complete: true, events, ..Default::default() };
    json!({ "download": download })
}

/// The sample mailbox with this computer in New York and a Microsoft account whose calendar
/// came down; the owner renamed "Budget review", and then a download brought "Standup". Answers
/// the session and the id of "Standup".
fn edited_then_brought() -> (Session, Id) {
    let now = at(14, 16);
    let mut s = Session::in_zone(mailcraft_engine::sample::mailbox(now, -240), now, Zone::named("America/New_York").unwrap());
    s.settle_zone();
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap();
    let account = added["id"].as_u64().unwrap();
    s.run("sync.applyEvents", &october(account, vec![event("e-1", "Budget review", at(12, 13))])).unwrap();
    s.forget_history();
    let first = s.mailbox.events.values().find(|e| e.origin.account == account && e.origin.id == "e-1").map(|e| e.id).unwrap();
    s.run("calendar.edit", &json!({"id": first, "title": "Mine"})).unwrap();
    s.run("sync.applyEvents", &october(account, vec![event("e-1", "Budget review", at(12, 13)), event("e-3", "Standup", at(15, 13))])).unwrap();
    let brought = s.mailbox.events.values().find(|e| e.origin.account == account && e.origin.id == "e-3").map(|e| e.id).unwrap();
    (s, brought)
}

#[test]
fn headless_an_undo_after_an_unsettled_zone_move_needs_delete_to_take_an_event_away() {
    // In its zone, the undo keeps "Standup", and an agent allowed nothing may run it.
    let (s, brought) = edited_then_brought();
    let mut h = Headless::new(s, Vec::new());
    h.call("engine.execute", json!({"command": "edit.undo"})).unwrap();
    assert!(h.session.mailbox.event(brought).is_some());

    // Carried to London with no command since: the undo would put every event back as it was
    // before the edit, by that clock, and "Standup" would go.
    let (mut s, brought) = edited_then_brought();
    s.clock.zone = Zone::named("Europe/London");
    let mut h = Headless::new(s, Vec::new());
    let refused = h.call("engine.execute", json!({"command": "edit.undo"})).unwrap_err();
    assert!(refused.contains("--allow delete"), "{refused}");
    assert!(h.session.mailbox.event(brought).is_some());
    assert_eq!(h.session.undo_label(), Some("Edit Event"));
    // The zone was settled before the gate read the session, as the command would have it.
    assert_eq!(h.session.mailbox.zone, "Europe/London");

    // Allowed to delete, it runs, and takes "Standup" away until the next download.
    h.allow = vec![mailcraft_engine::agent::Class::Delete];
    h.call("engine.execute", json!({"command": "edit.undo"})).unwrap();
    assert!(h.session.mailbox.event(brought).is_none());
}
