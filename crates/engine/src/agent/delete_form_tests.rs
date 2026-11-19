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
    use Selected::{Focus, Nothing, Set};
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

        // What the command does with it.
        let result = s.run("calendar.delete", &params);
        assert_eq!(route_taken(&result), expected, "the command took another path: {case}: {result:?}");
    }
}
