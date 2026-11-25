// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether a `calendar.delete` is a set or the one event: the agent gate reads every payload as
//! the command does (rust-review-01 F5). The owner's yes to an agent is bound to what the gate
//! classified, so a payload the gate took for one event while the command deleted a set (or the
//! other way round) would reach what the owner wasn't asked about.
//!
//! Each row is a payload over one selection state, with the form the command must take it in:
//! the command is run on a fresh session and its answer says which path it took; the gate's
//! classification ([`super::is_batch`], its reader [`super::batch`] and the binding's
//! [`super::bind::shape`]) must say the same. Synthetic appointments only, all on this computer.

use serde_json::{Value, json};

use super::bind::Shape;
use crate::Session;
use crate::cmd::calendar::{DeleteForm, delete_form};
use crate::time::{Date, DateTime};

const NOW: i64 = 1_791_993_600;

/// What the selection holds when the payload is run.
#[derive(Clone, Copy, Debug)]
enum Selected {
    /// Nothing of the calendar.
    Nothing,
    /// A typed set of two appointments, `a` and `b`.
    Set,
    /// The older single focus on `c`, with no typed set.
    Focus,
    /// A typed set of one, `a`, which has the focus: what a plain click selects.
    One,
}

/// How the command took the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    Batch,
    One,
    Refused,
}

/// A session with three appointments of this computer's (`a`, `b`, `c`) and `selected`.
fn session(selected: Selected) -> (Session, [u64; 3]) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let calendar = s.mailbox.default_calendar().expect("the sample has a calendar");
    let day = Date::new(2026, 10, 14).expect("a date");
    let mut ids = [0u64; 3];
    for (n, id) in ids.iter_mut().enumerate() {
        let start = DateTime::new(day, 9 + n as u8, 0, 0).expect("a time");
        let event = mailcraft_model::Event { calendar, title: format!("Item {n}"), start, end: start.add_minutes(30), ..Default::default() };
        *id = s.mailbox.add_event(event).expect("the calendar takes an appointment");
    }
    s.sel.event = None;
    s.sel.event_day = None;
    s.sel.events.clear();
    match selected {
        Selected::Nothing => {}
        Selected::Set => {
            s.run("calendar.select", &json!({"items": [{"id": ids[0]}, {"id": ids[1]}]})).expect("the set is selected");
        }
        Selected::Focus => {
            s.run("calendar.select", &json!({"id": ids[2]})).expect("the focus is selected");
        }
        Selected::One => {
            s.run("calendar.select", &json!({"items": [{"id": ids[0]}]})).expect("the set of one is selected");
        }
    }
    (s, ids)
}

/// Which path the command took, from what it answered.
fn route_taken(result: &Result<Value, crate::CmdError>) -> Route {
    match result {
        Ok(answer) if answer.get("items").is_some() => Route::Batch,
        Ok(_) => Route::One,
        Err(e) => {
            let said = e.to_string();
            if said.contains("not by `ids`") || said.contains("not both") {
                Route::Refused
            } else if ["name each day in its item", "`items` is a list", "the parameters are an object"].iter().any(|batch| said.contains(batch)) {
                // (Refusals only the set's reader says.)
                Route::Batch
            } else {
                Route::One
            }
        }
    }
}

#[test]
fn the_agent_gate_reads_every_delete_payload_as_the_command_does() {
    use Selected::{Focus, Nothing, One, Set};
    // `C` stands for the third appointment's id, filled in per row.
    let rows: Vec<(Selected, Value, Route)> = vec![
        (Set, json!({}), Route::Batch),
        (Set, Value::Null, Route::Batch),
        (Set, json!({"scope": "occurrence"}), Route::Batch),
        (Set, json!({"scope": "series", "send": true}), Route::Batch),
        (Set, json!({"calendar": null}), Route::Batch),
        (Set, json!({"occurrence": "2026-10-14"}), Route::Batch),
        (Set, json!({"day": "2026-10-14"}), Route::Batch),
        (Set, json!({"items": "C"}), Route::Batch),
        (Set, json!({"items": [{"id": "C"}]}), Route::Batch),
        (Set, json!([]), Route::Batch),
        (Set, json!("text"), Route::Batch),
        (Nothing, json!({"items": [{"id": "C"}]}), Route::Batch),
        (Focus, json!({"items": [{"id": "C"}], "scope": "occurrence"}), Route::Batch),
        (Set, json!({"id": "C"}), Route::One),
        (Set, json!({"id": null}), Route::One),
        (Set, json!({"id": 0}), Route::One),
        (Set, json!({"id": "twelve"}), Route::One),
        (Set, json!({"id": "C", "occurrence": "2026-10-14"}), Route::One),
        (Nothing, json!({}), Route::One),
        (Nothing, json!([]), Route::One),
        (Nothing, json!({"id": "C"}), Route::One),
        (Focus, json!({}), Route::One),
        (Focus, json!({"occurrence": "2026-10-14"}), Route::One),
        (Focus, json!({"scope": "series"}), Route::One),
        // A set of one that has the focus is that one event, as it was before sets (benandez on
        // #1665): a day of a series is then asked about, or told by `scope`, as Delete always was.
        (One, json!({}), Route::One),
        (One, json!({"scope": "occurrence"}), Route::One),
        (One, json!({"scope": "series"}), Route::One),
        (One, json!({"id": "C"}), Route::One),
        (One, json!({"items": [{"id": "C"}]}), Route::Batch),
        (One, json!({"ids": ["C"]}), Route::Refused),
        (Set, json!({"ids": ["C"]}), Route::Refused),
        (Nothing, json!({"ids": ["C"]}), Route::Refused),
        (Focus, json!({"ids": ["C"]}), Route::Refused),
        (Set, json!({"items": [{"id": "C"}], "id": "C"}), Route::Refused),
        (Nothing, json!({"items": [{"id": "C"}], "id": "C"}), Route::Refused),
        (Focus, json!({"items": [{"id": "C"}], "ids": ["C"]}), Route::Refused),
    ];
    for (selected, raw, expected) in rows {
        let (mut s, ids) = session(selected);
        // (The placeholder "C" is the third appointment's id wherever a row names one.)
        let params: Value = serde_json::from_str(&raw.to_string().replace("\"C\"", &ids[2].to_string())).expect("the row is JSON");
        let case = format!("{selected:?} {params}");

        // The one classifier.
        let form = delete_form(&s.sel, &params);
        let classified = match form {
            Ok(DeleteForm::Batch) => Route::Batch,
            Ok(DeleteForm::One) => Route::One,
            Err(_) => Route::Refused,
        };
        assert_eq!(classified, expected, "delete_form: {case}");

        // The agent gate and the binding of the owner's yes, before anything runs.
        let gate_batch = super::is_batch(&s, "calendar.delete", &params);
        assert_eq!(gate_batch, expected != Route::One, "the gate's is_batch: {case}");
        let shape = super::bind::shape(&s, "calendar.delete", &params);
        let wanted = if expected == Route::One { Shape::Legacy } else { Shape::Batch };
        assert_eq!(shape, Some(wanted), "the binding's shape: {case}");
        if expected == Route::Refused {
            assert!(super::batch(&s, "calendar.delete", &params).is_err(), "the gate refuses what the command refuses: {case}");
        }

        // What the command does with it. An `id` that names no appointment (null, 0, text) is
        // refused and deletes nothing, never the one in focus: `route_taken` alone can't tell the
        // two apart (CodeRabbit on #1665, delete_form_tests.rs:149).
        let before = s.mailbox.clone();
        let result = s.run("calendar.delete", &params);
        assert_eq!(route_taken(&result), expected, "the command took another path: {case}: {result:?}");
        if params.get("id").is_some_and(|id| id.as_u64().is_none_or(|n| n == 0)) {
            assert!(result.is_err(), "an id that names nothing is refused: {case}: {result:?}");
            assert!(s.mailbox == before, "and nothing is deleted: {case}");
        }
    }
}

/// A typed set of one that has the focus, as a plain click on one day of a series leaves it, is
/// deleted as the one event in focus (benandez on #1665): the command, the agent gate and the
/// binding of the owner's yes read it as the older Delete, so the window asks which of the series
/// is meant and `scope` says it. A set of one beside another focus is still the set it names.
#[test]
fn a_set_of_one_in_focus_is_deleted_as_the_one_event_in_focus() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let series = s.mailbox.events.values().find(|e| e.title == "Team stand-up").map(|e| e.id).expect("the sample has its stand-up");
    let other = s.mailbox.events.values().find(|e| e.recurrence.is_none() && !e.cancelled).map(|e| e.id).expect("an appointment that doesn't repeat");
    let select = |s: &mut Session, day: Date| {
        s.run("calendar.select", &json!({"items": [{"id": series, "day": day.iso()}]})).expect("one day of the series is selected");
        assert_eq!(s.sel.events.len(), 1, "fixture: a set of one");
        assert_eq!((s.sel.event, s.sel.event_day), (Some(series), Some(day)), "fixture: it has the focus");
    };
    let (tuesday, wednesday) = (Date::new(2026, 10, 13).expect("a date"), Date::new(2026, 10, 14).expect("a date"));
    select(&mut s, tuesday);
    for params in [json!({}), json!({"scope": "occurrence"}), json!({"scope": "series"})] {
        assert_eq!(delete_form(&s.sel, &params).ok(), Some(DeleteForm::One), "the command: {params}");
        assert!(!super::is_batch(&s, "calendar.delete", &params), "the gate: {params}");
        assert_eq!(super::bind::shape(&s, "calendar.delete", &params), Some(Shape::Legacy), "the binding: {params}");
    }
    // A set of one whose focus is another item is the set it names.
    let mut beside = s.sel.clone();
    beside.events = vec![crate::SelectedEvent { id: other, day: None }];
    assert_eq!(delete_form(&beside, &json!({})).ok(), Some(DeleteForm::Batch), "a set of one beside another focus");

    // "Just This One": that day alone goes.
    let answer = s.run("calendar.delete", &json!({"scope": "occurrence"})).expect("the day goes");
    assert_eq!(answer, json!({"id": series, "occurrence": tuesday.iso()}));
    assert_eq!(s.mailbox.event(series).map(|e| e.exceptions.clone()), Some(vec![tuesday]), "just Tuesday's");
    // Unscoped it is the older Delete of the focus, which the window asks about first: the
    // entire series.
    select(&mut s, wednesday);
    let answer = s.run("calendar.delete", &json!({})).expect("the series goes");
    assert_eq!(answer, json!({"deleted": series}));
    assert!(s.mailbox.event(series).is_none(), "the entire series");
}
