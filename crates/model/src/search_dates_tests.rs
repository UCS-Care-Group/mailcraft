// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #329: `received:` takes a range of days, as Outlook's Advanced Search writes it
//! (`received:A..B`), or one side of one (`>=`, `<=`, `>`, `<`).

use mailcraft_time::{Date, DateTime};

use crate::Message;
use crate::search::{Query, keyword_pills, keyword_spans};

fn day(y: i32, m: u8, d: u8) -> Date {
    Date::new(y, m, d).unwrap()
}

/// A message received at `h:mm` UTC on that day, sent three days before.
fn at(date: Date, h: u8, mi: u8) -> Message {
    let received = DateTime::new(date, h, mi, 0).unwrap().civil_seconds();
    Message { date: received.saturating_sub(3 * 86_400), received, subject: "Budget".into(), ..Default::default() }
}

fn on(y: i32, m: u8, d: u8) -> Message {
    at(day(y, m, d), 12, 0)
}

fn finds(q: &str, m: &Message) -> bool {
    Query::parse(q).matches(m, 0)
}

/// Which of the 20th to the 25th of October 2026 `q` finds.
fn days_found(q: &str) -> Vec<u8> {
    (20..=25).filter(|&d| finds(q, &on(2026, 10, d))).collect()
}

#[test]
fn a_range_holds_both_its_days() {
    assert_eq!(days_found("received:2026-10-21..2026-10-23"), [21, 22, 23]);
    assert_eq!(days_found("received:10/21/2026..10/23/2026"), [21, 22, 23], "in the form Outlook writes");
    assert_eq!(days_found("received:2026-10-21..10/23/2026"), [21, 22, 23], "each side in either form");
    assert_eq!(days_found("received:2026-10-22..2026-10-22"), [22], "one day");
    assert_eq!(days_found("RECEIVED:2026-10-21..2026-10-23"), [21, 22, 23]);
    assert_eq!(days_found("received: 2026-10-21..2026-10-23"), [21, 22, 23], "a space after the colon");
}

/// `received:` looks at the day a message came in, not the day it was sent; one that doesn't say
/// when it came in, at the day it was sent.
#[test]
fn received_is_the_day_it_came_in() {
    let m = on(2026, 10, 22);
    assert!(finds("received:2026-10-22", &m) && !finds("received:2026-10-19", &m));
    assert!(finds("on:2026-10-19", &m), "on: stays the day it was sent");
    let unsaid = Message { received: 0, ..m };
    assert!(finds("received:2026-10-19", &unsaid) && finds("received:>=2026-10-19", &unsaid));
}

#[test]
fn one_side_of_a_range() {
    assert_eq!(days_found("received:>=2026-10-23"), [23, 24, 25]);
    assert_eq!(days_found("received:<=2026-10-22"), [20, 21, 22]);
    assert_eq!(days_found("received:>2026-10-23"), [24, 25]);
    assert_eq!(days_found("received:<2026-10-22"), [20, 21]);
    assert_eq!(days_found("received:2026-10-22"), [22], "a day alone, as before");
    // `>=` has no end: mail dated in the future is in it.
    assert!(finds("received:>=2026-10-23", &on(2031, 1, 1)));
}

#[test]
fn a_range_written_backwards_finds_nothing() {
    assert_eq!(days_found("received:2026-10-23..2026-10-21"), Vec::<u8>::new());
}

#[test]
fn a_range_excluded_and_with_other_terms() {
    assert_eq!(days_found("-received:2026-10-21..2026-10-23"), [20, 24, 25]);
    assert_eq!(days_found("budget received:>=2026-10-24"), [24, 25]);
    assert_eq!(days_found("lunch received:>=2026-10-24"), Vec::<u8>::new());
}

/// A day is the owner's day: in UTC+1 a message at 23:30 UTC on the 20th was received on the 21st.
#[test]
fn a_range_counts_days_where_the_owner_is() {
    let late = at(day(2026, 10, 20), 23, 30);
    let q = Query::parse("received:2026-10-21..2026-10-22");
    assert!(q.matches(&late, 60) && !q.matches(&late, 0));
    let q = Query::parse("received:<=2026-10-20");
    assert!(!q.matches(&late, 60) && q.matches(&late, 0));
}

/// What isn't a range or a date is text, as a date that can't be read is today: it finds no day
/// by its date, and is drawn as text.
#[test]
fn what_is_not_a_range_is_text() {
    for q in [
        "received:2026-10-21..",
        "received:..2026-10-21",
        "received:2026-10-21..2026-10-22..2026-10-23",
        "received:>=",
        "received:>=junk",
        "received:>=2026-10-21..2026-10-22",
        "received:10/21/2026/1..10/22/2026",
        "received:2026-02-30..2026-03-01",
    ] {
        assert!(days_found(q).is_empty(), "{q} is not a range");
        assert!(keyword_spans(q).is_empty(), "{q} is drawn as text, not a pill");
    }
    assert!(!finds("received:10/22/2026/junk", &on(2026, 10, 22)), "a date has three parts");
}

/// The other date keywords keep what they took: one day, no operators.
#[test]
fn on_sent_before_and_after_take_no_ranges() {
    for q in ["on:>=2026-10-21", "sent:>=2026-10-21", "on:2026-10-21..2026-10-23", "sent:2026-10-21..2026-10-23", "before:<=2026-10-21"] {
        assert!(keyword_spans(q).is_empty(), "{q}");
    }
    // Each message of `days_found` was sent three days before it came in.
    assert_eq!(days_found("sent:2026-10-19"), [22]);
    assert_eq!(days_found("on:2026-10-19"), [22]);
}

#[test]
fn a_range_is_one_pill_and_stays_off_the_server() {
    let q = "budget received:2026-10-21..2026-10-23";
    let spans = keyword_spans(q);
    assert_eq!(spans.len(), 1);
    assert_eq!(&q[spans[0].range.clone()], "received:2026-10-21..2026-10-23");
    // While its last day is still being typed it keeps its pill.
    let typing = "received:2026-10-21..";
    assert_eq!(keyword_pills(typing, Some(typing.len())), vec![0..9_usize], "the keyword and its colon");
    let server = Query::parse(q).for_server();
    assert_eq!(server.terms.len(), 1, "the words go to the server");
    assert_eq!(server.local, Query::parse("received:2026-10-21..2026-10-23"), "the range is looked at here");
}

/// What a word index knows of a message's words stands in for looking at them; the range beside
/// them is still looked at.
#[test]
fn a_range_beside_what_an_index_knows() {
    let q = Query::parse("budget received:2026-10-21..2026-10-23");
    let index_says_yes = |i: usize| (i == 0).then_some(true);
    assert!(q.matches_with(&on(2026, 10, 22), 0, index_says_yes));
    assert!(!q.matches_with(&on(2026, 10, 25), 0, index_says_yes));
}

/// The first and last days there are: no day falls off either end.
#[test]
fn the_calendars_ends() {
    let (first, last) = (Date::from_days(i64::MIN), Date::from_days(i64::MAX));
    for (q, m) in [
        (format!("received:>={}", first.iso()), at(first, 0, 0)),
        (format!("received:<={}", last.iso()), at(last, 23, 59)),
        (format!("received:{}..{}", first.iso(), last.iso()), on(2026, 10, 22)),
    ] {
        assert!(Query::parse(&q).matches(&m, 0), "{q}");
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

    /// Whatever follows `received:` is read without a panic.
    #[test]
    fn any_value_after_received_is_read(v in "[0-9/<>=.\\-]{0,24}", d in 0i64..100_000, offset in -840i32..=840) {
        let m = Message { date: d * 86_400, ..Default::default() };
        let q = Query::parse(&format!("received:{v}"));
        let _ = q.matches(&m, offset);
        let _ = q.for_server();
        let _ = keyword_spans(&format!("received:{v}"));
    }
}
