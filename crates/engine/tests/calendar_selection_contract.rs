// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar item identity must include which visible occurrence was selected.

use mailcraft_engine::Selection;
use mailcraft_time::Date;
use serde_json::json;

#[test]
fn selection_distinguishes_two_occurrences_of_the_same_series() {
    // Selection's stored Date representation, not calendar.select's ISO-text command parameter.
    let first = Date::new(2026, 11, 13).unwrap();
    let second = Date::new(2026, 11, 20).unwrap();
    let items = json!([
        {"id": 7, "day": first},
        {"id": 7, "day": second},
        {"id": 9}
    ]);
    let selection: Selection = serde_json::from_value(json!({
        "event": 7,
        "eventDay": second,
        "events": items
    }))
    .unwrap();
    let saved = serde_json::to_value(selection).unwrap();
    assert_eq!(saved.get("events"), Some(&items));
}

#[test]
fn older_single_event_selection_still_round_trips() {
    let selection: Selection = serde_json::from_value(json!({"event": 9})).unwrap();
    let saved = serde_json::to_value(selection).unwrap();
    assert_eq!(saved.get("event"), Some(&json!(9)));
    assert!(saved.get("events").is_none_or(|items| items.as_array().is_some_and(Vec::is_empty)));
}
