// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Selection lifecycle and the owner's unsaved-series protection, without a window or server.

use mailcraft_engine::{SelectedEvent, Session, agent::Class};
use mailcraft_model::{Event, Freq, Recurrence};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

const NOW: i64 = 1_791_993_600;

fn session() -> Session {
    Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
}

fn add(s: &mut Session, repeats: bool) -> u64 {
    let day = Date::new(2026, 11, 4).unwrap();
    let start = DateTime::new(day, 9, 0, 0).unwrap();
    let calendar = s.mailbox.default_calendar().unwrap();
    s.mailbox
        .add_event(Event {
            calendar,
            title: "Synthetic selection lifecycle".into(),
            start,
            end: start.add_minutes(30),
            recurrence: repeats.then(|| Recurrence::every(Freq::Weekly)),
            ..Default::default()
        })
        .unwrap()
}

#[test]
fn normalize_prunes_calendar_items_whose_events_are_gone() {
    let mut s = session();
    let first = add(&mut s, false);
    let last = add(&mut s, false);
    s.sel.events = vec![SelectedEvent { id: first, day: None }, SelectedEvent { id: last, day: None }];
    s.sel.event = Some(last);
    s.sel.event_day = s.mailbox.event(last).map(|e| e.start.date);
    s.mailbox.remove_event(first).unwrap();
    s.normalize();
    assert_eq!(s.sel.events, vec![SelectedEvent { id: last, day: None }]);
    assert_eq!(s.sel.event, Some(last));
}

#[test]
fn an_agent_cannot_cancel_an_occurrence_of_a_series_the_owner_is_editing() {
    let mut s = session();
    let series = add(&mut s, true);
    let day = Date::new(2026, 11, 11).unwrap();
    s.owner_changing = vec![series];
    s.agent = Some(Class::ALL.to_vec());
    let before = s.mailbox.clone();
    let selection = s.sel.clone();
    let steps = s.undo_steps_moved();
    let result = s.run("calendar.delete", &json!({"id": series, "scope": "occurrence", "occurrence": day.iso()}));
    let error = result.expect_err("the series is still present, but cancelling its day must protect the owner's changes");
    assert!(error.to_string().contains("the owner is changing"), "{error}");
    assert!(s.mailbox == before, "the refused cancellation changes nothing");
    assert_eq!(s.sel, selection);
    assert_eq!(s.undo_steps_moved(), steps);
}

#[test]
fn explicit_calendar_items_reach_dispatch_without_a_legacy_focus() {
    let mut s = session();
    let event = add(&mut s, false);
    s.sel.event = None;
    s.sel.event_day = None;
    s.sel.events.clear();
    let result = s.run("calendar.delete", &json!({"items": [{"id": event}]}));
    assert!(!matches!(&result, Err(mailcraft_engine::CmdError::Disabled(_))), "explicit items must reach the handler: {result:?}");
}

#[test]
fn rebasing_the_calendar_never_retargets_a_selected_occurrence_to_the_master() {
    // A pinned offset deliberately overrides the preference. Pin the computer's named zone
    // instead, so this fixture actually changes coordinate systems through view.timeZone.
    let mut s = Session::in_zone(mailcraft_engine::sample::mailbox(NOW, 0), NOW, mailcraft_time::Zone::UTC);
    s.run("view.timeZone", &json!({"zone": "UTC"})).unwrap();
    assert_eq!(s.zone(), mailcraft_time::Zone::UTC);
    let series = add(&mut s, true);
    let day = Date::new(2026, 11, 11).unwrap();
    s.sel.events = vec![SelectedEvent { id: series, day: Some(day) }];
    s.sel.event = Some(series);
    s.sel.event_day = Some(day);
    s.run("view.timeZone", &json!({"zone": "Asia/Tokyo"})).unwrap();
    assert_eq!(s.zone(), mailcraft_time::Zone::named("Asia/Tokyo").unwrap(), "the fixture must really rebase");
    assert!(s.sel.events.is_empty(), "intrinsic keys must not survive a change of coordinate system");
    assert_eq!((s.sel.event, s.sel.event_day), (None, None), "require re-selection rather than silently targeting the first occurrence");
    assert!(s.mailbox.event(series).is_some(), "rebasing selection must not delete the series");
}

#[test]
fn a_fixed_offset_clock_does_not_rebase_or_drop_selection_for_a_zone_preference() {
    let mut s = session();
    let series = add(&mut s, true);
    let day = Date::new(2026, 11, 11).unwrap();
    s.sel.events = vec![SelectedEvent { id: series, day: Some(day) }];
    s.sel.event = Some(series);
    s.sel.event_day = Some(day);
    let before = (s.zone(), s.sel.clone(), s.mailbox.event(series).unwrap().clone());
    s.run("view.timeZone", &json!({"zone": "Asia/Tokyo"})).unwrap();
    assert_eq!(s.zone(), before.0, "Session::at pins the offset ahead of preferences");
    assert_eq!(s.sel, before.1, "there was no coordinate-system change");
    assert_eq!(s.mailbox.event(series).unwrap(), &before.2);
}

#[test]
fn rebasing_clears_typed_keys_but_keeps_an_ordinary_focus_at_its_new_start() {
    let mut s = Session::in_zone(mailcraft_engine::sample::mailbox(NOW, 0), NOW, mailcraft_time::Zone::UTC);
    let made = s.run("calendar.new", &json!({"title": "Ordinary zone focus", "start": "2026-11-04T23:30", "end": "2026-11-05T00:15"})).unwrap();
    let id = made["id"].as_u64().unwrap();
    let before = s.mailbox.event(id).unwrap().clone();
    s.run("calendar.select", &json!({"items": [{"id": id}]})).unwrap();
    assert_eq!(s.sel.events, vec![SelectedEvent { id, day: None }]);
    s.run("view.timeZone", &json!({"zone": "Asia/Tokyo"})).unwrap();
    assert_eq!(s.zone(), mailcraft_time::Zone::named("Asia/Tokyo").unwrap(), "fixture: a real named-zone rebase occurred");
    let after = s.mailbox.event(id).unwrap();
    assert_ne!(after.start.date, before.start.date, "fixture: the ordinary focus crosses midnight");
    assert!(s.sel.events.is_empty(), "typed coordinate keys are reset");
    assert_eq!(
        (s.sel.event, s.sel.event_day),
        (Some(id), Some(after.start.date)),
        "an ordinary event has stable identity; keep its legacy focus at the rebased start (contract E-7)"
    );
}
