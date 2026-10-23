// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A file's own time zone definitions cost bounded work, whatever they hold and however many
//! times use them: a hostile invitation can't freeze the window.

use std::time::{Duration, Instant};

use mailcraft_time::Zone;

use crate::{IcsFile, parse};

fn part(i: usize, dates: usize) -> String {
    let rdates: String = (0..dates).map(|d| format!("RDATE:{}{:02}01T020000\r\n", 1900 + (d % 100), 1 + (i % 12))).collect();
    format!("BEGIN:STANDARD\r\nDTSTART:19{:02}0101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0100\r\n{rdates}END:STANDARD\r\n", i % 100)
}

fn hostile(parts: usize, dates: usize, exdates: usize) -> String {
    let body: String = (0..parts).map(|i| part(i, dates)).collect();
    let ex: Vec<String> = (0..exdates).map(|i| format!("{}{:02}{:02}T100000", 2000 + i / 300, 1 + (i / 28) % 12, 1 + i % 28)).collect();
    format!(
        "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Z0\r\n{body}END:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:x\r\n\
         DTSTART;TZID=Z0:20261012T100000\r\nRRULE:FREQ=DAILY\r\nEXDATE;TZID=Z0:{}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        ex.join(",")
    )
}

fn timed(text: &str) -> (IcsFile, Duration) {
    let start = Instant::now();
    let file = parse(text, &|instant| Zone::UTC.offset_at(instant));
    (file, start.elapsed())
}

#[test]
fn many_parts_and_many_exdates_are_fast() {
    let (file, took) = timed(&hostile(400, 0, 5000));
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert_eq!(file.events.len(), 1);
}

#[test]
fn many_parts_with_many_dates_are_fast() {
    let (file, took) = timed(&hostile(250, 400, 5000));
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert_eq!(file.events.len(), 1);
}

#[test]
fn over_budget_keeps_the_event_and_says_so() {
    let (file, _) = timed(&hostile(64, 64, 5000));
    assert_eq!(file.events.len(), 1);
    assert!(file.notes.iter().any(|note| note.starts_with("Time zone work stopped after")), "{:?}", file.notes);
}

#[test]
fn a_realistic_zone_is_not_cut_short() {
    let (file, _) = timed(&hostile(20, 2, 500));
    assert_eq!(file.events.len(), 1);
    assert!(!file.notes.iter().any(|note| note.starts_with("Time zone work stopped")), "{:?}", file.notes);
}
