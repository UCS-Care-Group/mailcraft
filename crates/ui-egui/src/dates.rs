// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dates and times as the window shows them: in the interface language
//! ([`mailcraft_i18n::date`]), from `mailcraft_time`'s days and clocks. `mailcraft_time`'s own
//! text (`Date::long`, `list_label`…) stays English for commands, the CLI and agents; the window
//! says dates through here.
//!
//! A time the owner types into a field is the exception: a field is filled with
//! `mailcraft_time::time_short`, which `mailcraft_time::parse_time` reads back.

use mailcraft_i18n::date::{self, Day};
use mailcraft_time::{Date, DateTime, Timestamp, Weekday};

use crate::i18n::{Lang, current};

/// The window's language, for as long as the program runs (so that a name it gives is
/// `&'static`).
fn lang() -> &'static Lang {
    match current() {
        Lang::English => &Lang::English,
        Lang::French => &Lang::French,
        Lang::German => &Lang::German,
        Lang::Spanish => &Lang::Spanish,
        Lang::Italian => &Lang::Italian,
        Lang::Pseudo => &Lang::Pseudo,
    }
}

/// The day as the i18n crate takes it.
pub fn day(d: Date) -> Day {
    Day { year: d.year, month: d.month, day: d.day, weekday: d.weekday().index() }
}

/// `Sunday, October 11, 2026`.
pub fn long(d: Date) -> String {
    date::long(lang(), day(d))
}

/// `Oct 11, 2026`.
pub fn medium(d: Date) -> String {
    date::medium(lang(), day(d))
}

/// `Wed, Oct 14, 2026`.
pub fn weekday_medium(d: Date) -> String {
    date::weekday_medium(lang(), day(d))
}

/// `October 11, 2026`.
pub fn long_without_weekday(d: Date) -> String {
    date::long_without_weekday(lang(), day(d))
}

/// `October 3`.
pub fn month_day_long(d: Date) -> String {
    date::month_day_long(lang(), d.month, d.day)
}

/// `October 2026`.
pub fn month_year(d: Date) -> String {
    date::month_year(lang(), d.year, d.month)
}

/// `Oct 3`.
pub fn month_day(d: Date) -> String {
    date::month_day(lang(), d.month, d.day)
}

/// `Wed, Oct 14`.
pub fn weekday_month_day(d: Date) -> String {
    date::weekday_month_day(lang(), day(d))
}

/// `Wednesday, October 14`.
pub fn weekday_month_day_long(d: Date) -> String {
    date::weekday_month_day_long(lang(), day(d))
}

/// `October`.
pub fn month_name(month: u8) -> &'static str {
    date::month_name(lang(), month)
}

/// `Oct`.
pub fn month_short(month: u8) -> &'static str {
    date::month_short(lang(), month)
}

/// `Wednesday`.
pub fn weekday_name(d: Weekday) -> &'static str {
    date::weekday_name(lang(), d.index())
}

/// `Wed`.
pub fn weekday_short(d: Weekday) -> &'static str {
    date::weekday_short(lang(), d.index())
}

/// `9:30 AM`, or `09:30` on a 24-hour clock.
pub fn time(hour: u8, minute: u8, twelve_hour: bool) -> String {
    date::time(lang(), hour, minute, twelve_hour)
}

/// The time of `t`.
pub fn clock(t: DateTime, twelve_hour: bool) -> String {
    time(t.hour, t.minute, twelve_hour)
}

/// `Oct 11, 2026 9:30 AM`: a day and a time.
pub fn medium_time(t: DateTime, twelve_hour: bool) -> String {
    format!("{} {}", medium(t.date), clock(t, twelve_hour))
}

/// The date column of a message list: the time today, `Sat 3:05 PM` within the last week,
/// `Oct 3` this year, `10/3/2025` before that.
pub fn list_label(ts: Timestamp, now: Timestamp, offset_minutes: i32, twelve_hour: bool) -> String {
    let (t, n) = (DateTime::from_timestamp(ts, offset_minutes), DateTime::from_timestamp(now, offset_minutes));
    let days_ago = t.date.days_until(n.date);
    date::list_label(lang(), day(t.date), t.hour, t.minute, days_ago, t.date.year == n.date.year, twelve_hour)
}

/// The date line of an open message: `Sun 10/11/2026 9:30 AM`.
pub fn header_label(ts: Timestamp, offset_minutes: i32, twelve_hour: bool) -> String {
    let t = DateTime::from_timestamp(ts, offset_minutes);
    date::header_label(lang(), day(t.date), t.hour, t.minute, twelve_hour)
}

/// A length of time: `30 minutes`, `1 hour`, `1.5 hours`, `2 days`.
pub fn duration(minutes: i64) -> String {
    date::duration(lang(), minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In English the window says dates exactly as `mailcraft_time` does (and always did).
    #[test]
    fn english_is_mailcraft_time_s_own() {
        // (The window's language is shared by the tests running alongside: only English here.)
        let now = 1_791_993_600; // Wednesday 2026-10-14 16:00 UTC.
        let mut ts: i64 = 1_600_000_000;
        while ts < now + 40 * 86_400 {
            let t = DateTime::from_timestamp(ts, 120);
            assert_eq!(long(t.date), t.date.long());
            assert_eq!(medium(t.date), t.date.medium());
            assert_eq!(month_year(t.date), t.date.month_year());
            for twelve in [true, false] {
                assert_eq!(clock(t, twelve), t.time_short(twelve));
                assert_eq!(list_label(ts, now, 120, twelve), mailcraft_time::list_label(ts, now, 120, twelve), "{ts}");
                assert_eq!(header_label(ts, 120, twelve), mailcraft_time::header_label(ts, 120, twelve));
            }
            ts += 3 * 86_400 + 3_917;
        }
        for m in [-1, 0, 1, 2, 45, 60, 90, 95, 120, 600, 1439, 1440, 2880, 100_000] {
            assert_eq!(duration(m), mailcraft_time::duration_label(m));
        }
        for d in Weekday::ALL {
            assert_eq!((weekday_name(d), weekday_short(d)), (d.name(), d.short()));
        }
        for m in 0..=13 {
            assert_eq!((month_name(m), month_short(m)), (mailcraft_time::month_name(m), mailcraft_time::month_short(m)));
        }
    }
}
