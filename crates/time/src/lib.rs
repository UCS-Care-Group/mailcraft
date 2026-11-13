// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft dates and times.
//!
//! Calendar maths is our own (proleptic Gregorian, no leap seconds) so it is the same on every
//! platform and never panics on hostile input. The clock ([`now`]) and this computer's time zone
//! ([`Zone::system`], [`local_offset_minutes`]) are the only things that ask the system (or the
//! browser). Time zones ([`Zone`]) come from a database that is compiled in.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod text;
pub mod zone;

use serde::{Deserialize, Serialize};

pub use text::{format_iso_utc, format_rfc5322, iso_to_timestamp, parse_iso, parse_rfc5322};
pub use zone::{MAX_ZONE_NAME, Transition, Zone};

/// Seconds since the Unix epoch, UTC.
pub type Timestamp = i64;

pub const MINUTE: i64 = 60;
pub const HOUR: i64 = 3_600;
pub const DAY: i64 = 86_400;

/// Days since the epoch of 0001-01-01 and 9999-12-31: every [`Date`] stays inside this range.
const MIN_DAYS: i64 = -719_162;
const MAX_DAYS: i64 = 2_932_896;

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// A day of the week; weeks are counted from Monday (`index` 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    pub const ALL: [Weekday; 7] = [Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri, Weekday::Sat, Weekday::Sun];

    /// Monday = 0 … Sunday = 6.
    pub fn index(self) -> u8 {
        self as u8
    }

    /// Any integer, wrapped into the week.
    pub fn from_index(i: i64) -> Weekday {
        Self::ALL.get(i.rem_euclid(7) as usize).copied().unwrap_or(Weekday::Mon)
    }

    pub fn short(self) -> &'static str {
        match self {
            Weekday::Mon => "Mon",
            Weekday::Tue => "Tue",
            Weekday::Wed => "Wed",
            Weekday::Thu => "Thu",
            Weekday::Fri => "Fri",
            Weekday::Sat => "Sat",
            Weekday::Sun => "Sun",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Weekday::Mon => "Monday",
            Weekday::Tue => "Tuesday",
            Weekday::Wed => "Wednesday",
            Weekday::Thu => "Thursday",
            Weekday::Fri => "Friday",
            Weekday::Sat => "Saturday",
            Weekday::Sun => "Sunday",
        }
    }

    /// The two-letter iCalendar code (`MO`, `TU`…).
    pub fn ical(self) -> &'static str {
        match self {
            Weekday::Mon => "MO",
            Weekday::Tue => "TU",
            Weekday::Wed => "WE",
            Weekday::Thu => "TH",
            Weekday::Fri => "FR",
            Weekday::Sat => "SA",
            Weekday::Sun => "SU",
        }
    }

    pub fn from_ical(s: &str) -> Option<Weekday> {
        Self::ALL.into_iter().find(|d| d.ical().eq_ignore_ascii_case(s.trim()))
    }

    /// From a name or abbreviation of three or more letters (`tue`, `Tuesday`).
    pub fn parse(s: &str) -> Option<Weekday> {
        let s = s.trim().to_ascii_lowercase();
        if s.len() < 3 {
            return None;
        }
        Self::ALL.into_iter().find(|d| d.name().to_ascii_lowercase().starts_with(&s))
    }

    pub fn is_weekend(self) -> bool {
        matches!(self, Weekday::Sat | Weekday::Sun)
    }
}

/// A calendar date (proleptic Gregorian), always valid: years 1–9999.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl Default for Date {
    fn default() -> Self {
        Date { year: 1970, month: 1, day: 1 }
    }
}

pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days in `month` (1–12) of `year`; 30 for a month outside 1–12.
pub fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 30,
    }
}

/// The full English month name; empty for a month outside 1–12.
pub fn month_name(month: u8) -> &'static str {
    MONTHS.get(usize::from(month).wrapping_sub(1)).copied().unwrap_or("")
}

/// The three-letter English month abbreviation.
pub fn month_short(month: u8) -> &'static str {
    month_name(month).get(..3).unwrap_or("")
}

/// A month from its name or three-letter abbreviation.
pub fn parse_month(s: &str) -> Option<u8> {
    let s = s.trim().to_ascii_lowercase();
    if s.len() < 3 {
        return None;
    }
    MONTHS.iter().position(|m| m.to_ascii_lowercase().starts_with(&s)).map(|i| i as u8 + 1)
}

impl Date {
    /// A date, or `None` when the day doesn't exist (February 30th, month 13, year 0).
    pub fn new(year: i32, month: u8, day: u8) -> Option<Date> {
        ((1..=9999).contains(&year) && (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month)).then_some(Date {
            year,
            month,
            day,
        })
    }

    /// The date `days` after 1970-01-01 (clamped to years 1–9999).
    pub fn from_days(days: i64) -> Date {
        // Howard Hinnant's `civil_from_days`.
        let z = days.clamp(MIN_DAYS, MAX_DAYS) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        Date { year: (if m <= 2 { y + 1 } else { y }) as i32, month: m as u8, day: d as u8 }
    }

    /// Days since 1970-01-01.
    pub fn to_days(self) -> i64 {
        let (m, d) = (i64::from(self.month), i64::from(self.day));
        let y = i64::from(self.year) - i64::from(m <= 2);
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    pub fn weekday(self) -> Weekday {
        // 1970-01-01 was a Thursday.
        Weekday::from_index(self.to_days() + 3)
    }

    pub fn add_days(self, n: i64) -> Date {
        Date::from_days(self.to_days().saturating_add(n))
    }

    /// `n` months later (earlier when negative); a day the month lacks becomes its last day.
    pub fn add_months(self, n: i32) -> Date {
        let total = (i64::from(self.year) * 12 + i64::from(self.month) - 1).saturating_add(i64::from(n)).clamp(12, 9999 * 12 + 11);
        let year = total.div_euclid(12) as i32;
        let month = total.rem_euclid(12) as u8 + 1;
        Date { year, month, day: self.day.min(days_in_month(year, month)) }
    }

    pub fn add_years(self, n: i32) -> Date {
        self.add_months(n.saturating_mul(12))
    }

    pub fn start_of_month(self) -> Date {
        Date { day: 1, ..self }
    }

    pub fn end_of_month(self) -> Date {
        Date { day: days_in_month(self.year, self.month), ..self }
    }

    /// The first day of this date's week, for a week that starts on `first`.
    pub fn start_of_week(self, first: Weekday) -> Date {
        let back = (i64::from(self.weekday().index()) - i64::from(first.index())).rem_euclid(7);
        self.add_days(-back)
    }

    /// Whole days from `self` to `other` (negative when `other` is earlier).
    pub fn days_until(self, other: Date) -> i64 {
        other.to_days() - self.to_days()
    }

    /// The ISO 8601 week number (1–53).
    pub fn iso_week(self) -> u8 {
        // The week's Thursday decides which year the week belongs to.
        let thursday = self.add_days(3 - i64::from(self.weekday().index()));
        let jan1 = Date { year: thursday.year, month: 1, day: 1 };
        (jan1.days_until(thursday) / 7 + 1) as u8
    }

    /// `2026-10-11`.
    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// From `YYYY-MM-DD` (or `YYYYMMDD`).
    pub fn parse_iso(s: &str) -> Option<Date> {
        let s = s.trim();
        let b = s.as_bytes();
        let ok = match b.len() {
            8 => b.iter().all(u8::is_ascii_digit),
            10 => b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() }),
            _ => false,
        };
        if !ok {
            return None;
        }
        let digits: String = s.chars().filter(char::is_ascii_digit).collect();
        Date::new(digits.get(0..4)?.parse().ok()?, digits.get(4..6)?.parse().ok()?, digits.get(6..8)?.parse().ok()?)
    }

    /// `Sunday, October 11, 2026`.
    pub fn long(self) -> String {
        format!("{}, {} {}, {}", self.weekday().name(), month_name(self.month), self.day, self.year)
    }

    /// `Oct 11, 2026`.
    pub fn medium(self) -> String {
        format!("{} {}, {}", month_short(self.month), self.day, self.year)
    }

    /// `10/11/2026`.
    pub fn numeric(self) -> String {
        format!("{}/{}/{}", self.month, self.day, self.year)
    }

    /// `October 2026`.
    pub fn month_year(self) -> String {
        format!("{} {}", month_name(self.month), self.year)
    }
}

/// A wall-clock date and time without a zone (what a calendar shows).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DateTime {
    pub date: Date,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// A date and time, or `None` when either part is out of range.
    pub fn new(date: Date, hour: u8, minute: u8, second: u8) -> Option<DateTime> {
        (hour < 24 && minute < 60 && second < 60).then_some(DateTime { date, hour, minute, second })
    }

    pub fn at_midnight(date: Date) -> DateTime {
        DateTime { date, hour: 0, minute: 0, second: 0 }
    }

    /// Seconds since 1970-01-01T00:00:00 on the same wall clock (no zone applied).
    pub fn civil_seconds(self) -> i64 {
        self.date.to_days() * DAY + self.seconds_of_day()
    }

    pub fn from_civil_seconds(secs: i64) -> DateTime {
        let secs = secs.clamp(MIN_DAYS * DAY, MAX_DAYS * DAY + DAY - 1);
        let t = secs.rem_euclid(DAY);
        DateTime { date: Date::from_days(secs.div_euclid(DAY)), hour: (t / HOUR) as u8, minute: (t / MINUTE % 60) as u8, second: (t % 60) as u8 }
    }

    pub fn seconds_of_day(self) -> i64 {
        i64::from(self.hour) * HOUR + i64::from(self.minute) * MINUTE + i64::from(self.second)
    }

    /// Minutes since midnight.
    pub fn minutes_of_day(self) -> u32 {
        u32::from(self.hour) * 60 + u32::from(self.minute)
    }

    /// The wall clock at `ts` in a zone `offset_minutes` east of UTC.
    pub fn from_timestamp(ts: Timestamp, offset_minutes: i32) -> DateTime {
        DateTime::from_civil_seconds(ts.saturating_add(i64::from(offset_minutes) * MINUTE))
    }

    /// The instant this wall clock shows in a zone `offset_minutes` east of UTC.
    pub fn to_timestamp(self, offset_minutes: i32) -> Timestamp {
        self.civil_seconds() - i64::from(offset_minutes) * MINUTE
    }

    pub fn add_minutes(self, n: i64) -> DateTime {
        DateTime::from_civil_seconds(self.civil_seconds().saturating_add(n.saturating_mul(MINUTE)))
    }

    pub fn add_days(self, n: i64) -> DateTime {
        DateTime { date: self.date.add_days(n), ..self }
    }

    /// Whole minutes from `self` to `other`.
    pub fn minutes_until(self, other: DateTime) -> i64 {
        (other.civil_seconds() - self.civil_seconds()).div_euclid(MINUTE)
    }

    /// `2026-10-11T09:30:00`.
    pub fn iso(self) -> String {
        format!("{}T{:02}:{:02}:{:02}", self.date.iso(), self.hour, self.minute, self.second)
    }

    /// `9:30 AM`, or `09:30` on a 24-hour clock.
    pub fn time_short(self, twelve_hour: bool) -> String {
        time_short(self.hour, self.minute, twelve_hour)
    }
}

/// `9:30 AM`, or `09:30` on a 24-hour clock.
pub fn time_short(hour: u8, minute: u8, twelve_hour: bool) -> String {
    if !twelve_hour {
        return format!("{hour:02}:{minute:02}");
    }
    let h = match hour % 12 {
        0 => 12,
        h => h,
    };
    format!("{h}:{minute:02} {}", if hour % 24 < 12 { "AM" } else { "PM" })
}

/// A time of day from `9`, `9:30`, `09:30:15`, `9am`, `9:30 PM`.
pub fn parse_time(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim().to_ascii_lowercase();
    let (body, pm) = if let Some(b) = s.strip_suffix("pm") {
        (b.trim(), Some(true))
    } else if let Some(b) = s.strip_suffix("am") {
        (b.trim(), Some(false))
    } else {
        (s.as_str(), None)
    };
    let mut parts = body.split(':');
    let mut hour: u8 = parts.next()?.trim().parse().ok()?;
    let minute: u8 = parts.next().map_or(Some(0), |m| m.trim().parse().ok())?;
    let second: u8 = parts.next().map_or(Some(0), |m| m.trim().parse().ok())?;
    if parts.next().is_some() {
        return None;
    }
    match pm {
        Some(_) if hour == 0 || hour > 12 => return None,
        Some(true) if hour < 12 => hour += 12,
        Some(false) if hour == 12 => hour = 0,
        _ => {}
    }
    (hour < 24 && minute < 60 && second < 60).then_some((hour, minute, second))
}

/// Now: the system clock, or the browser's on the web.
pub fn now() -> Timestamp {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }
    // `SystemTime::now()` panics on wasm32-unknown-unknown.
    #[cfg(target_arch = "wasm32")]
    {
        let ms = js_sys::Date::now();
        if ms.is_finite() && ms > 0.0 { (ms / 1000.0) as i64 } else { 0 }
    }
}

/// Minutes east of UTC of the system's (or browser's) time zone at `ts`, daylight saving included;
/// 0 when the system can't say.
pub fn local_offset_minutes(ts: Timestamp) -> i32 {
    Zone::system().offset_at(ts)
}

/// How a message list groups by date, newest group first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DateGroup {
    Future,
    Today,
    Yesterday,
    /// Earlier this week, by day.
    ThisWeek(Weekday),
    LastWeek,
    TwoWeeksAgo,
    ThreeWeeksAgo,
    LastMonth,
    Older,
}

impl DateGroup {
    /// The group of `day` seen from `today`, for weeks that start on `first`.
    pub fn of(day: Date, today: Date, first: Weekday) -> DateGroup {
        let diff = day.days_until(today);
        if diff < 0 {
            return DateGroup::Future;
        }
        if diff == 0 {
            return DateGroup::Today;
        }
        if diff == 1 {
            return DateGroup::Yesterday;
        }
        let week = today.start_of_week(first);
        let weeks_back = week.days_until(day.start_of_week(first)) / -7;
        match weeks_back {
            0 => DateGroup::ThisWeek(day.weekday()),
            1 => DateGroup::LastWeek,
            2 => DateGroup::TwoWeeksAgo,
            3 => DateGroup::ThreeWeeksAgo,
            _ if day >= today.start_of_month().add_months(-1) => DateGroup::LastMonth,
            _ => DateGroup::Older,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DateGroup::Future => "Upcoming",
            DateGroup::Today => "Today",
            DateGroup::Yesterday => "Yesterday",
            DateGroup::ThisWeek(d) => d.name(),
            DateGroup::LastWeek => "Last Week",
            DateGroup::TwoWeeksAgo => "Two Weeks Ago",
            DateGroup::ThreeWeeksAgo => "Three Weeks Ago",
            DateGroup::LastMonth => "Last Month",
            DateGroup::Older => "Older",
        }
    }
}

/// The date column of a message list: the time today, `Sat 3:05 PM` within the last week,
/// `Oct 3` this year, `10/3/2025` before that.
pub fn list_label(ts: Timestamp, now: Timestamp, offset_minutes: i32, twelve_hour: bool) -> String {
    let (t, n) = (DateTime::from_timestamp(ts, offset_minutes), DateTime::from_timestamp(now, offset_minutes));
    let diff = t.date.days_until(n.date);
    if diff == 0 {
        t.time_short(twelve_hour)
    } else if (1..7).contains(&diff) {
        format!("{} {}", t.date.weekday().short(), t.time_short(twelve_hour))
    } else if t.date.year == n.date.year {
        format!("{} {}", month_short(t.date.month), t.date.day)
    } else {
        t.date.numeric()
    }
}

/// The date line of an open message: `Sun 10/11/2026 9:30 AM`.
pub fn header_label(ts: Timestamp, offset_minutes: i32, twelve_hour: bool) -> String {
    let t = DateTime::from_timestamp(ts, offset_minutes);
    format!("{} {} {}", t.date.weekday().short(), t.date.numeric(), t.time_short(twelve_hour))
}

/// A length of time for people: `30 minutes`, `1 hour`, `1.5 hours`, `2 days`.
pub fn duration_label(minutes: i64) -> String {
    let m = minutes.max(0);
    if m < 60 {
        format!("{m} minute{}", if m == 1 { "" } else { "s" })
    } else if m < 24 * 60 {
        match (m / 60, m % 60) {
            (1, 0) => "1 hour".into(),
            (h, 0) => format!("{h} hours"),
            (h, 30) => format!("{h}.5 hours"),
            (h, r) => format!("{h} h {r} min"),
        }
    } else {
        let d = m / (24 * 60);
        format!("{d} day{}", if d == 1 { "" } else { "s" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    #[test]
    fn civil_round_trip_and_known_days() {
        assert_eq!(d(1970, 1, 1).to_days(), 0);
        assert_eq!(d(2000, 3, 1).to_days(), 11_017);
        assert_eq!(Date::from_days(19_723), d(2024, 1, 1));
        for days in [-719_162, -1, 0, 59, 60, 365, 11_016, 11_017, 20_000, 2_932_896] {
            assert_eq!(Date::from_days(days).to_days(), days);
        }
    }

    #[test]
    fn invalid_dates_are_refused_and_hostile_days_clamped() {
        assert!(Date::new(2025, 2, 29).is_none());
        assert!(Date::new(2024, 2, 29).is_some());
        assert!(Date::new(2024, 13, 1).is_none());
        assert!(Date::new(0, 1, 1).is_none());
        assert_eq!(Date::from_days(i64::MAX), d(9999, 12, 31));
        assert_eq!(Date::from_days(i64::MIN), d(1, 1, 1));
        assert_eq!(d(9999, 12, 31).add_days(i64::MAX), d(9999, 12, 31));
        assert_eq!(d(9999, 12, 31).add_months(i32::MAX), d(9999, 12, 31));
        assert_eq!(d(1, 1, 1).add_months(i32::MIN), d(1, 1, 1));
    }

    #[test]
    fn weekdays() {
        assert_eq!(d(1970, 1, 1).weekday(), Weekday::Thu);
        assert_eq!(d(2026, 10, 11).weekday(), Weekday::Sun);
        assert_eq!(d(2024, 2, 29).weekday(), Weekday::Thu);
        assert_eq!(d(2026, 10, 14).start_of_week(Weekday::Sun), d(2026, 10, 11));
        assert_eq!(d(2026, 10, 14).start_of_week(Weekday::Mon), d(2026, 10, 12));
        assert_eq!(d(2026, 10, 11).start_of_week(Weekday::Mon), d(2026, 10, 5));
        assert_eq!(Weekday::parse("tues"), Some(Weekday::Tue));
        assert_eq!(Weekday::from_ical("fr"), Some(Weekday::Fri));
        assert_eq!(Weekday::from_index(-1), Weekday::Sun);
    }

    #[test]
    fn month_arithmetic_clamps_the_day() {
        assert_eq!(d(2026, 1, 31).add_months(1), d(2026, 2, 28));
        assert_eq!(d(2024, 1, 31).add_months(1), d(2024, 2, 29));
        assert_eq!(d(2026, 1, 15).add_months(-2), d(2025, 11, 15));
        assert_eq!(d(2024, 2, 29).add_years(1), d(2025, 2, 28));
        assert_eq!(d(2026, 2, 10).end_of_month(), d(2026, 2, 28));
    }

    #[test]
    fn iso_weeks() {
        assert_eq!(d(2026, 1, 1).iso_week(), 1);
        assert_eq!(d(2021, 1, 3).iso_week(), 53);
        assert_eq!(d(2026, 10, 11).iso_week(), 41);
    }

    #[test]
    fn date_text() {
        assert_eq!(d(2026, 10, 11).iso(), "2026-10-11");
        assert_eq!(Date::parse_iso("2026-10-11"), Some(d(2026, 10, 11)));
        assert_eq!(Date::parse_iso("20261011"), Some(d(2026, 10, 11)));
        assert_eq!(Date::parse_iso("2026-02-30"), None);
        assert_eq!(Date::parse_iso("2026/10/11"), None);
        assert_eq!(d(2026, 10, 11).long(), "Sunday, October 11, 2026");
        assert_eq!(d(2026, 10, 3).medium(), "Oct 3, 2026");
        assert_eq!(parse_month("sept"), Some(9));
    }

    #[test]
    fn timestamps_and_offsets() {
        let t = DateTime::new(d(2025, 10, 10), 7, 53, 5).unwrap();
        assert_eq!(t.to_timestamp(0), 1_760_082_785);
        assert_eq!(DateTime::from_timestamp(1_760_082_785, 0), t);
        // 09:53 in a zone two hours east is the same instant.
        assert_eq!(DateTime::new(d(2025, 10, 10), 9, 53, 5).unwrap().to_timestamp(120), 1_760_082_785);
        assert_eq!(DateTime::from_timestamp(0, -300).iso(), "1969-12-31T19:00:00");
        assert_eq!(t.add_minutes(24 * 60 + 7).iso(), "2025-10-11T08:00:05");
        assert_eq!(DateTime::from_timestamp(i64::MAX, i32::MAX).date, d(9999, 12, 31));
    }

    #[test]
    fn times_of_day() {
        assert_eq!(time_short(0, 5, true), "12:05 AM");
        assert_eq!(time_short(12, 0, true), "12:00 PM");
        assert_eq!(time_short(15, 5, true), "3:05 PM");
        assert_eq!(time_short(15, 5, false), "15:05");
        assert_eq!(parse_time("9"), Some((9, 0, 0)));
        assert_eq!(parse_time("9:30 PM"), Some((21, 30, 0)));
        assert_eq!(parse_time("12am"), Some((0, 0, 0)));
        assert_eq!(parse_time("12:15pm"), Some((12, 15, 0)));
        assert_eq!(parse_time("25:00"), None);
        assert_eq!(parse_time("13pm"), None);
        assert_eq!(parse_time(""), None);
    }

    #[test]
    fn groups_and_labels() {
        let today = d(2026, 10, 14); // Wednesday
        let g = |day| DateGroup::of(day, today, Weekday::Sun);
        assert_eq!(g(d(2026, 10, 15)), DateGroup::Future);
        assert_eq!(g(today), DateGroup::Today);
        assert_eq!(g(d(2026, 10, 13)), DateGroup::Yesterday);
        assert_eq!(g(d(2026, 10, 11)), DateGroup::ThisWeek(Weekday::Sun));
        assert_eq!(g(d(2026, 10, 10)), DateGroup::LastWeek);
        assert_eq!(g(d(2026, 9, 29)), DateGroup::TwoWeeksAgo);
        assert_eq!(g(d(2026, 9, 21)), DateGroup::ThreeWeeksAgo);
        assert_eq!(g(d(2026, 9, 2)), DateGroup::LastMonth);
        assert_eq!(g(d(2026, 7, 2)), DateGroup::Older);
        assert!(DateGroup::Today < DateGroup::Yesterday && DateGroup::LastMonth < DateGroup::Older);

        let now = DateTime::new(today, 16, 0, 0).unwrap().to_timestamp(0);
        let at = |day, h| DateTime::new(day, h, 5, 0).unwrap().to_timestamp(0);
        assert_eq!(list_label(at(today, 9), now, 0, true), "9:05 AM");
        assert_eq!(list_label(at(d(2026, 10, 10), 15), now, 0, true), "Sat 3:05 PM");
        assert_eq!(list_label(at(d(2026, 3, 3), 15), now, 0, true), "Mar 3");
        assert_eq!(list_label(at(d(2025, 3, 3), 15), now, 0, true), "3/3/2025");
        assert_eq!(header_label(at(today, 9), 0, false), "Wed 10/14/2026 09:05");
        assert_eq!(duration_label(90), "1.5 hours");
        assert_eq!(duration_label(1), "1 minute");
        assert_eq!(duration_label(3 * 24 * 60), "3 days");
    }

    #[test]
    fn the_clock_runs() {
        assert!(now() > 1_700_000_000);
        // Whatever the zone, an offset is within a day.
        assert!(local_offset_minutes(now()).abs() <= 24 * 60);
    }

    proptest::proptest! {
        #[test]
        fn any_day_count_round_trips(days in -719_162i64..=2_932_896) {
            let date = Date::from_days(days);
            proptest::prop_assert!(Date::new(date.year, date.month, date.day).is_some());
            proptest::prop_assert_eq!(date.to_days(), days);
        }

        #[test]
        fn hostile_numbers_never_panic(ts in proptest::num::i64::ANY, off in proptest::num::i32::ANY, n in proptest::num::i64::ANY) {
            let t = DateTime::from_timestamp(ts, off);
            let _ = t.add_minutes(n).iso();
            let _ = t.date.add_days(n).long();
            let _ = list_label(ts, n, off, true);
            let _ = format_rfc5322(ts, off);
            let _ = format_iso_utc(ts);
        }
    }
}
