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
    // (In the words of what it would do: a day of the series goes, it is not taken away whole.)
    assert_eq!(
        error.to_string(),
        "`calendar.delete` would change which days a series has (cancel one of them, or move one out of it as an appointment of its own) while that series is open with changes the owner hasn't saved, and the owner wasn't asked about that; nothing was changed. Try again once the owner has saved or closed its window",
        "{error}"
    );
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

/// Appointments a typed set can be made of, each at its own time on 2026-11-04.
fn several(s: &mut Session, n: usize) -> Vec<u64> {
    let calendar = s.mailbox.default_calendar().unwrap();
    let day = Date::new(2026, 11, 4).unwrap();
    (0..n)
        .map(|i| {
            let start = DateTime::new(day, 0, 0, 0).unwrap().add_minutes(i64::try_from(i).unwrap());
            s.mailbox
                .add_event(Event { calendar, title: format!("Synthetic member {i}"), start, end: start.add_minutes(1), ..Default::default() })
                .unwrap()
        })
        .collect()
}

/// A server that moves the focused member of a typed set to another day (outside a command, as
/// a synchronisation does) moves its focus with it: the set the owner chose stays selected, and
/// the date in focus is the one it is shown on now (CodeRabbit on #1665, session.rs:3205). A
/// legacy producer that focuses another event still replaces the set.
#[test]
fn a_sync_that_moves_the_focused_member_to_another_day_keeps_the_typed_set() {
    let mut s = session();
    let first = add(&mut s, false);
    let last = add(&mut s, false);
    s.run("calendar.select", &json!({"items": [{"id": first}, {"id": last}]})).unwrap();
    let event = s.mailbox.event_mut(last).unwrap();
    event.start = event.start.add_minutes(24 * 60);
    event.end = event.end.add_minutes(24 * 60);
    let moved_to = event.start.date;
    s.mailbox_changed();
    assert_eq!(s.sel.events, vec![SelectedEvent { id: first, day: None }, SelectedEvent { id: last, day: None }], "the typed set stays");
    assert_eq!((s.sel.event, s.sel.event_day), (Some(last), Some(moved_to)), "and its focus is on the day it is shown on now");

    // A focus put on another event the older way is still a new selection.
    s.sel.event = Some(first);
    s.sel.event_day = s.mailbox.event(first).map(|e| e.start.date);
    s.normalize();
    assert!(s.sel.events.is_empty(), "a legacy focus elsewhere replaces the typed set");
}

/// A member a server marks cancelled leaves the typed set, as one that went does, so that the
/// rest stays selected and usable: Delete of the set and a Command click on another item work
/// (CodeRabbit on #1665, session.rs:3214).
#[test]
fn a_member_cancelled_by_a_sync_leaves_the_set_and_the_rest_stays_usable() {
    let mut s = session();
    let ids = several(&mut s, 3);
    s.run("calendar.select", &json!({"items": [{"id": ids[0]}, {"id": ids[1]}]})).unwrap();
    s.mailbox.event_mut(ids[1]).unwrap().cancelled = true;
    s.mailbox_changed();
    assert_eq!(s.sel.events, vec![SelectedEvent { id: ids[0], day: None }], "the cancelled member left the set");
    assert_eq!(s.sel.event, Some(ids[0]), "and the focus is on what is left");
    s.run("calendar.select", &json!({"items": [{"id": ids[2]}], "extend": true})).expect("a Command click adds to what is left");
    assert_eq!(s.sel.events.len(), 2);
}

/// A toggle that adds one item and takes another away ends at the cap whichever order the two are
/// named in; adding one more than the cap allows is still refused (CodeRabbit on #1665,
/// selection.rs:284).
#[test]
fn an_extend_that_ends_at_the_cap_is_taken_in_either_order() {
    let cap = mailcraft_engine::MAX_SELECTED_EVENTS;
    let mut s = session();
    let ids = several(&mut s, cap + 1);
    let full: Vec<serde_json::Value> = ids.iter().take(cap).map(|id| json!({"id": id})).collect();
    let (new, old) = (ids[cap], ids[0]);
    for order in [[new, old], [old, new]] {
        s.run("calendar.select", &json!({"items": full})).unwrap();
        let toggles: Vec<serde_json::Value> = order.iter().map(|id| json!({"id": id})).collect();
        s.run("calendar.select", &json!({"items": toggles, "extend": true})).unwrap_or_else(|e| panic!("{order:?}: {e}"));
        assert_eq!(s.sel.events.len(), cap, "{order:?}");
        assert!(s.sel.events.contains(&SelectedEvent { id: new, day: None }) && !s.sel.events.contains(&SelectedEvent { id: old, day: None }));
    }
    s.run("calendar.select", &json!({"items": full})).unwrap();
    let before = s.sel.clone();
    assert!(s.run("calendar.select", &json!({"items": [{"id": new}], "extend": true})).is_err(), "one more than the cap is refused");
    assert_eq!(s.sel, before, "and changes nothing");
}

/// `{"id": null}` selects nothing, as it did before typed sets, also with the older `day` beside
/// it; a top-level `day` beside a typed list is still refused (CodeRabbit on #1665,
/// selection.rs:95).
#[test]
fn a_null_id_with_a_day_still_clears_the_selection() {
    let mut s = session();
    let first = add(&mut s, false);
    s.run("calendar.select", &json!({"items": [{"id": first}]})).unwrap();
    s.run("calendar.select", &json!({"id": null, "day": "2026-10-14"})).expect("null selects nothing, whatever day is beside it");
    assert!(s.sel.events.is_empty() && s.sel.event.is_none() && s.sel.event_day.is_none());
    assert!(s.run("calendar.select", &json!({"items": [{"id": first}], "day": "2026-10-14"})).is_err(), "a day beside a typed list is refused");
}

/// A day of a series that a server cancels (outside a command, as a synchronisation does) leaves
/// the typed set, and another day of the same series stays selected, with the focus on it
/// (review of #1665: the cancelled days `days_gone` reads).
#[test]
fn a_day_of_a_series_cancelled_by_a_sync_leaves_the_set_and_its_other_day_stays() {
    let mut s = session();
    let series = add(&mut s, true);
    let (first, second) = (Date::new(2026, 11, 11).unwrap(), Date::new(2026, 11, 18).unwrap());
    s.run("calendar.select", &json!({"items": [{"id": series, "day": first.iso()}, {"id": series, "day": second.iso()}]})).unwrap();
    assert_eq!(s.sel.events.len(), 2, "fixture: two days of one series");
    s.mailbox.event_mut(series).unwrap().exceptions.push(first);
    s.mailbox_changed();
    assert_eq!(s.sel.events, vec![SelectedEvent { id: series, day: Some(second) }], "the cancelled day left the set, the other stayed");
    assert_eq!((s.sel.event, s.sel.event_day), (Some(series), Some(second)), "and the focus is on the day that stayed");
}
