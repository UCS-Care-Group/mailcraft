// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the forms share: date and time fields, time menus, what changed, text cut for labels.

use super::*;

#[test]
fn date_and_time_fields_read_what_people_type() {
    assert!(date_ok("2026-10-16", true) && date_ok(" 20261016 ", true));
    assert!(!date_ok("2026-02-30", true) && !date_ok("16/10/2026", false) && !date_ok("tomorrow", false));
    // An empty field is fine only where a date may be left out.
    assert!(date_ok("  ", false) && !date_ok("", true));
    assert_eq!(parse_clock("9"), Some((9, 0)));
    assert_eq!(parse_clock("9:30"), Some((9, 30)));
    assert_eq!(parse_clock("9am"), Some((9, 0)));
    assert_eq!(parse_clock("14:15"), Some((14, 15)));
    assert_eq!(parse_clock(" 9:30 PM "), Some((21, 30)));
    assert_eq!(parse_clock("12am"), Some((0, 0)));
    for wrong in ["", "25:00", "9:75", "13pm", "noon", "9.5", "-1"] {
        assert_eq!(parse_clock(wrong), None, "{wrong}");
    }
    assert!(time_ok("", false) && !time_ok("", true) && time_ok("7pm", true) && !time_ok("7 o'clock", false));
    assert_eq!((clock_text(870, false).as_str(), clock_text(870, true).as_str()), ("14:30", "2:30 PM"));
    let _ = clock_text(u32::MAX, true);
}

#[test]
fn fields_become_command_values() {
    assert_eq!(stamp("2026-10-16", "3pm"), "2026-10-16T15:00");
    assert_eq!(stamp(" 2026-10-16 ", "09:05"), "2026-10-16T09:05");
    // Text that isn't a date or a time goes as typed, for the command to refuse.
    assert_eq!(stamp(" soon ", "late "), "soonTlate");
    assert_eq!(date_value("2026-10-16"), json!("2026-10-16"));
    assert_eq!(date_value("20261016"), json!("2026-10-16"));
    assert_eq!(date_value("   "), Value::Null);
    assert_eq!(date_value(" 2026-1-1 "), json!("2026-1-1"));
}

#[test]
fn time_menus_list_half_hours() {
    let all = slots(None, true);
    assert_eq!(all.len(), 48);
    assert_eq!(all.first(), Some(&(0, "12:00 AM".to_string())));
    assert_eq!(all.last(), Some(&(1410, "11:30 PM".to_string())));
    assert_eq!(slots(None, false).get(29), Some(&(870, "14:30".to_string())));
    // After a start, later times that day with how long the event would be.
    let after = slots(Some(15 * 60), true);
    assert_eq!(after.first(), Some(&(930, "3:30 PM (30 minutes)".to_string())));
    assert_eq!(after.get(1), Some(&(960, "4:00 PM (1 hour)".to_string())));
    assert_eq!(after.get(2).map(|s| s.1.as_str()), Some("4:30 PM (1.5 hours)"));
    assert_eq!(after.last().map(|s| s.0), Some(1410));
    // Nothing is left of the day after 11:30 PM: the whole list again.
    assert_eq!(slots(Some(23 * 60 + 45), true).len(), 48);
    assert_eq!(slots(Some(u32::MAX), true).len(), 48);
}

#[test]
fn only_what_changed_is_sent() {
    let before = object(json!({"title": "A", "start": "s", "end": "e", "allDay": false, "private": false}));
    let same = changes(&before, &before, &[&["start", "end", "allDay"]]);
    assert!(same.is_empty());
    let now = object(json!({"title": "B", "start": "s2", "end": "e", "allDay": false, "private": false, "calendar": 3}));
    let got = changes(&before, &now, &[&["start", "end", "allDay"]]);
    // A new start brings the end and All day along; a new field counts as changed.
    assert_eq!(Value::Object(got), json!({"title": "B", "start": "s2", "end": "e", "allDay": false, "calendar": 3}));
    assert_eq!(object(json!([1, 2])), Map::new());
}

#[test]
fn hostile_text_is_cut_for_labels() {
    assert_eq!(shown("a\tb\nc\u{7}", 10), "a b c");
    assert_eq!(shown("abcdef", 3), "ab…");
    // Cut by characters, never inside one; what shows nothing is gone.
    let long = "é\u{202e}\u{0}".repeat(10_000);
    assert_eq!(shown(&long, 400), format!("{}…", "é".repeat(399)));
    assert_eq!(shown("", 5), "");
}
