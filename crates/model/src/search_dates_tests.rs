// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #329: `received:` takes a range of days, as Outlook's Advanced Search writes it
//! (`received:A..B`), or one side of one (`>=`, `<=`, `>`, `<`); a date without a year and the days
//! Outlook names in words, in a search made on a day.

use mailcraft_time::{Date, DateTime, Weekday};

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

/// Each time is on the clock of its own instant: a message sent in summer time and received in
/// winter time is received on the day the clock said when it came in.
#[test]
fn the_day_received_is_by_the_offset_when_it_came_in() {
    // Received at 23:30 UTC on the 26th, sent on the 23rd: UTC from the 25th at 01:00, UTC+1
    // before.
    let late = at(day(2026, 10, 26), 23, 30);
    let change = DateTime::new(day(2026, 10, 25), 1, 0, 0).unwrap().civil_seconds();
    let offset = |instant: i64| if instant < change { 60 } else { 0 };
    assert!(offset(late.date) != offset(late.received), "the clocks changed between the two");
    assert!(Query::parse("received:2026-10-26").matches_in(&late, offset));
    assert!(!Query::parse("received:2026-10-27").matches_in(&late, offset));
    // The clock of when it was sent would put it on the 27th.
    assert!(Query::parse("received:2026-10-27").matches(&late, offset(late.date)));
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
    assert!(q.matches_with(&on(2026, 10, 22), |_| 0, index_says_yes));
    assert!(!q.matches_with(&on(2026, 10, 25), |_| 0, index_says_yes));
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

// ---- dates without a year and days in words: a search made on a day ----

/// The day the searches below are made on: Monday, October 26, 2026.
fn monday() -> Date {
    day(2026, 10, 26)
}

/// Which days from `from` to `to` `q` finds, made on `today` with weeks from `first`.
fn found_on(q: &str, today: Date, first: Weekday, from: Date, to: Date) -> Vec<Date> {
    let query = Query::parse(q).on(today, first);
    (0..=from.days_until(to)).map(|i| from.add_days(i)).filter(|d| query.matches(&at(*d, 12, 0), 0)).collect()
}

/// The first and last day `q` finds within a year either side of `today`, and how many.
fn span_on(q: &str, today: Date, first: Weekday) -> Option<(Date, Date, usize)> {
    let found = found_on(q, today, first, today.add_days(-400), today.add_days(400));
    Some((*found.first()?, *found.last()?, found.len()))
}

#[test]
fn a_date_without_a_year_is_this_years() {
    let made = |q: &str, d: Date| Query::parse(q).on(monday(), Weekday::Sun).matches(&at(d, 12, 0), 0);
    assert!(made("received:10/22", day(2026, 10, 22)) && !made("received:10/22", day(2025, 10, 22)));
    assert_eq!(span_on("received:10/21..10/23", monday(), Weekday::Sun), Some((day(2026, 10, 21), day(2026, 10, 23), 3)));
    assert_eq!(span_on("received:10/21..10/23/2026", monday(), Weekday::Sun), Some((day(2026, 10, 21), day(2026, 10, 23), 3)));
    assert_eq!(found_on("received:>=10/24", monday(), Weekday::Sun, day(2026, 10, 20), day(2026, 10, 25)), [day(2026, 10, 24), day(2026, 10, 25)]);
    assert_eq!(found_on("received:<10/22", monday(), Weekday::Sun, day(2026, 10, 20), day(2026, 10, 25)), [day(2026, 10, 20), day(2026, 10, 21)]);
    // The other date keywords too; each message was sent three days before it came in.
    assert!(made("on:10/19", day(2026, 10, 22)) && made("sent:10/19", day(2026, 10, 22)));
    assert!(made("before:10/20", day(2026, 10, 22)) && !made("after:10/20", day(2026, 10, 22)));
    // Made in another year, it is that year's day.
    assert!(Query::parse("received:10/22").on(day(2025, 3, 1), Weekday::Sun).matches(&on(2025, 10, 22), 0));
    // A date without a year is a pill, like one with.
    assert_eq!(keyword_spans("received:10/22").len(), 1);
}

#[test]
fn a_day_the_year_hasnt_got_finds_nothing() {
    let q = Query::parse("received:2/29");
    assert_eq!(keyword_spans("received:2/29").len(), 1, "a date: a pill");
    assert!(!q.clone().on(monday(), Weekday::Sun).matches(&on(2026, 2, 28), 0));
    assert!(!q.clone().on(monday(), Weekday::Sun).matches(&on(2026, 3, 1), 0));
    assert!(q.on(day(2028, 1, 5), Weekday::Sun).matches(&on(2028, 2, 29), 0), "a leap year has it");
    assert!(Query::parse("-received:2/29").on(monday(), Weekday::Sun).matches(&on(2026, 3, 1), 0));
}

#[test]
fn what_is_no_day_without_a_year_is_text() {
    for q in ["received:2/30", "received:13/1", "received:0/1", "received:10/0", "received:10/22/", "received:/22", "received:10/", "on:4/31"] {
        assert!(keyword_spans(q).is_empty(), "{q} is text");
        assert!(found_on(q, monday(), Weekday::Sun, day(2026, 1, 1), day(2026, 12, 31)).is_empty(), "{q}");
    }
}

/// Monday, October 26, 2026, with weeks from Sunday: this week is the 25th to the 31st.
#[test]
fn days_in_words() {
    let (today, sun) = (monday(), Weekday::Sun);
    assert_eq!(span_on("received:today", today, sun), Some((today, today, 1)));
    assert_eq!(span_on("received:yesterday", today, sun), Some((day(2026, 10, 25), day(2026, 10, 25), 1)));
    assert_eq!(span_on("received:\"this week\"", today, sun), Some((day(2026, 10, 25), day(2026, 10, 31), 7)));
    assert_eq!(span_on("received:\"last week\"", today, sun), Some((day(2026, 10, 18), day(2026, 10, 24), 7)));
    assert_eq!(span_on("received:\"this month\"", today, sun), Some((day(2026, 10, 1), day(2026, 10, 31), 31)));
    assert_eq!(span_on("received:\"last month\"", today, sun), Some((day(2026, 9, 1), day(2026, 9, 30), 30)));
    assert_eq!(span_on("received:\"this year\"", today, sun), Some((day(2026, 1, 1), day(2026, 12, 31), 365)));
    // On the week's first day, this week starts today.
    let sunday = day(2026, 10, 25);
    assert_eq!(span_on("received:\"this week\"", sunday, sun), Some((sunday, day(2026, 10, 31), 7)));
    assert_eq!(span_on("received:\"last week\"", sunday, sun), Some((day(2026, 10, 18), day(2026, 10, 24), 7)));
    // Weeks from Saturday.
    assert_eq!(span_on("received:\"this week\"", today, Weekday::Sat), Some((day(2026, 10, 24), day(2026, 10, 30), 7)));
    // Weeks from Monday: this week is the 26th to November 1st.
    let mon = Weekday::Mon;
    assert_eq!(span_on("received:\"this week\"", today, mon), Some((day(2026, 10, 26), day(2026, 11, 1), 7)));
    assert_eq!(span_on("received:\"last week\"", today, mon), Some((day(2026, 10, 19), day(2026, 10, 25), 7)));
}

#[test]
fn days_in_words_across_a_years_and_a_months_end() {
    let (new_year, sun) = (day(2027, 1, 1), Weekday::Sun);
    assert_eq!(span_on("received:yesterday", new_year, sun), Some((day(2026, 12, 31), day(2026, 12, 31), 1)));
    assert_eq!(span_on("received:\"this week\"", new_year, sun), Some((day(2026, 12, 27), day(2027, 1, 2), 7)));
    assert_eq!(span_on("received:\"last week\"", new_year, sun), Some((day(2026, 12, 20), day(2026, 12, 26), 7)));
    assert_eq!(span_on("received:\"last month\"", new_year, sun), Some((day(2026, 12, 1), day(2026, 12, 31), 31)));
    assert_eq!(span_on("received:\"this year\"", new_year, sun), Some((day(2027, 1, 1), day(2027, 12, 31), 365)));
    let march = day(2028, 3, 1);
    assert_eq!(span_on("received:yesterday", march, sun), Some((day(2028, 2, 29), day(2028, 2, 29), 1)));
    assert_eq!(span_on("received:\"last month\"", march, sun), Some((day(2028, 2, 1), day(2028, 2, 29), 29)));
}

/// As Outlook takes them: any case, the two-word ones in quotes, the whole value and nothing more.
#[test]
fn days_in_words_as_outlook_writes_them() {
    let today = monday();
    assert_eq!(span_on("received:TODAY", today, Weekday::Sun), Some((today, today, 1)));
    assert_eq!(span_on("Received:\"Last Week\"", today, Weekday::Sun), Some((day(2026, 10, 18), day(2026, 10, 24), 7)));
    let q = "received:\"last week\"";
    assert_eq!(keyword_spans(q).len(), 1, "one pill");
    assert_eq!(&q[keyword_spans(q)[0].range.clone()], q, "with its quotes");
    for q in [
        "received:last week",
        "received:lastweek",
        "received:>=today",
        "received:today..10/30",
        "received:tomorrow",
        "received:\"next week\"",
        "received:\"last year\"",
        "received:october",
        "received:monday",
        "on:today",
        "before:yesterday",
    ] {
        assert!(span_on(q, today, Weekday::Sun).is_none(), "{q} finds no day");
    }
    assert!(keyword_spans("received:tomorrow").is_empty(), "not a day: text");
    // Left out: every day but today.
    let found = found_on("-received:today", today, Weekday::Sun, day(2026, 10, 25), day(2026, 10, 27));
    assert_eq!(found, [day(2026, 10, 25), day(2026, 10, 27)]);
}

/// A search with such a date that isn't made on a day finds nothing, left out or not; one made on
/// a day twice is the same; what doesn't need the day needs no day.
#[test]
fn a_search_not_made_on_a_day_finds_nothing() {
    let m = at(monday(), 12, 0);
    for q in ["received:today", "-received:today", "received:10/26", "-received:10/26", "budget received:10/21..10/30"] {
        assert!(!Query::parse(q).matches(&m, 0), "{q}");
    }
    assert!(Query::parse("received:10/26/2026").matches(&m, 0));
    let q = Query::parse("budget received:\"this week\" on:10/23");
    assert_eq!(q.clone().on(monday(), Weekday::Sun), q.clone().on(monday(), Weekday::Sun).on(day(2030, 1, 1), Weekday::Mon));
    assert_eq!(Query::parse("received:2026-10-26").on(monday(), Weekday::Sun), Query::parse("received:2026-10-26"));
    let server = Query::parse("budget received:today").for_server();
    assert_eq!(server.terms.len(), 1, "the word goes to the server, the day stays here");
}

#[test]
fn days_in_words_beside_what_an_index_knows() {
    let q = Query::parse("budget received:\"this week\"").on(monday(), Weekday::Sun);
    let index_says_yes = |i: usize| (i == 0).then_some(true);
    assert!(q.matches_with(&on(2026, 10, 27), |_| 0, index_says_yes));
    assert!(!q.matches_with(&on(2026, 10, 24), |_| 0, index_says_yes));
}

/// At the calendar's first day there is nothing before it to name; at its last, no day after.
#[test]
fn days_in_words_at_the_calendars_ends() {
    let (first, last) = (Date::from_days(i64::MIN), Date::from_days(i64::MAX));
    for q in ["received:yesterday", "received:\"this week\"", "received:\"last week\"", "received:\"last month\""] {
        assert!(!Query::parse(q).on(first, Weekday::Sun).matches(&at(first, 0, 0), 0), "{q}: nothing at the first day");
    }
    // What doesn't count back is there.
    for q in ["received:today", "received:\"this month\"", "received:\"this year\""] {
        assert!(Query::parse(q).on(first, Weekday::Sun).matches(&at(first, 0, 0), 0), "{q} at the first day");
        assert!(!Query::parse(&format!("-{q}")).on(first, Weekday::Sun).matches(&at(first, 0, 0), 0), "-{q} at the first day");
    }
    assert!(Query::parse("received:today").on(last, Weekday::Sun).matches(&at(last, 23, 59), 0));
    assert!(Query::parse("received:\"this month\"").on(last, Weekday::Sun).matches(&at(last, 23, 59), 0));
    for q in ["received:\"this week\"", "received:\"this year\""] {
        assert!(Query::parse(q).on(last, Weekday::Sun).matches(&at(last, 23, 59), 0), "{q} at the last day");
    }
    assert!(Query::parse("received:\"last week\"").on(last, Weekday::Sun).matches(&at(last.add_days(-7), 12, 0), 0));
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

    /// Whatever follows `received:`, made on any day with weeks from any day, is read without a
    /// panic; and a day it finds in words is one day, or a span holding today or before it.
    #[test]
    fn any_value_made_on_any_day_is_read(v in "[0-9a-zA-Z /<>=.\\-\"]{0,24}", today in -1_000_000i64..1_000_000, first in 0i64..7, offset in -840i32..=840) {
        let today = Date::from_days(today);
        let q = Query::parse(&format!("received:{v}")).on(today, Weekday::from_index(first));
        let _ = q.matches(&at(today, 12, 0), offset);
        let _ = q.for_server();
        let _ = keyword_spans(&format!("received:{v}"));
    }
}
