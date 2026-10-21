// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dates and times in a language: the names of the months and the days, the order a date is
//! written in, the clock, and lengths of time. Every word and every pattern is a catalogue entry
//! keyed by its English (`"{weekday}, {month} {day}, {year}"`), so a translator puts the pieces in
//! their own order; what can't be said in a catalogue (whether a language pads `3/10` to `03/10`,
//! its clock and the day its weeks start on) is [`Conventions`].
//!
//! The crate stands alone, so a day comes in as plain numbers ([`Day`]): callers make one from
//! `mailcraft_time::Date`. The English output is the same as `mailcraft_time`'s own (`Date::long`,
//! `list_label`, `duration_label`…), which stays English for commands, the CLI and agents.
//!
//! Every function takes what it speaks as a [`Words`]: a [`Lang`] (there is no global here), or in
//! tests a catalogue of their own.

use crate::{Catalogue, Lang, fmt};

/// The months, January first.
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
/// The months, short (`Oct 3`).
const MONTHS_SHORT: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
/// The days of the week, Monday first.
const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
/// The days of the week, short (`Wed 3:05 PM`).
const WEEKDAYS_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// `Sunday, October 11, 2026`: full names.
const LONG: &str = "{weekday}, {month} {day}, {year}";
/// `Oct 11, 2026`: the month short (or in full: `October 11, 2026`).
const MEDIUM: &str = "{month} {day}, {year}";
/// `10/11/2026`: numbers only.
const NUMERIC: &str = "{month}/{day}/{year}";
/// `October 2026`.
const MONTH_YEAR: &str = "{month} {year}";
/// `Oct 3`: the month short (or in full: `October 3`).
const MONTH_DAY: &str = "{month} {day}";
/// `Wed, Oct 14` (both short), `Wednesday, October 14` (both full).
const WEEKDAY_MONTH_DAY: &str = "{weekday}, {month} {day}";
/// `Wed, Oct 14, 2026`: a short day and [`MEDIUM`].
const WEEKDAY_MEDIUM: &str = "{weekday}, {date}";
/// `Sat 3:05 PM`: a short day and a time.
const WEEKDAY_TIME: &str = "{weekday} {time}";
/// `Sun 10/11/2026 9:30 AM`: a short day, the date in numbers, a time.
const WEEKDAY_DATE_TIME: &str = "{weekday} {date} {time}";
/// `9:30 PM`.
const CLOCK_12: &str = "{hour}:{minute} {ampm}";
/// `21:30`.
const CLOCK_24: &str = "{hour}:{minute}";
const AM: &str = "AM";
const PM: &str = "PM";
/// `1.5 hours`, `2.5 hours`.
const HALF_HOURS: &str = "{n}.5 hours";
/// `2 h 15 min`.
const HOURS_MINUTES: &str = "{h} h {m} min";
const MINUTES: (&str, &str) = ("{n} minute", "{n} minutes");
const HOURS: (&str, &str) = ("{n} hour", "{n} hours");
const DAYS: (&str, &str) = ("{n} day", "{n} days");

/// What a language writes dates and times by that isn't a word: the default clock and first day
/// of the week (an owner who never chose their own gets these), and whether numbers in a date are
/// padded to two digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conventions {
    /// `3:05 PM` rather than `15:05`.
    pub twelve_hour: bool,
    /// The first day of the week: 0 Monday … 6 Sunday.
    pub week_start: u8,
    /// `03/10/2026` rather than `3/10/2026`.
    pub pad_numeric: bool,
}

impl Lang {
    /// How this language writes dates: English's (the program's until now) for English and pseudo.
    pub fn conventions(self) -> Conventions {
        match self {
            Lang::English | Lang::Pseudo => Conventions { twelve_hour: true, week_start: 6, pad_numeric: false },
            Lang::French | Lang::German | Lang::Spanish | Lang::Italian => Conventions { twelve_hour: false, week_start: 0, pad_numeric: true },
        }
    }
}

/// What dates are said in: a language, or (in tests) a catalogue of one's own.
pub trait Words {
    /// `s` (English), translated; English when there is no translation.
    fn t<'a>(&'a self, s: &'a str) -> &'a str;
    /// The form of `one` / `other` a count of `n` takes, placeholders unfilled.
    fn tn<'a>(&'a self, n: u64, one: &'a str, other: &'a str) -> &'a str;
    fn conventions(&self) -> Conventions;
}

impl Words for Lang {
    fn t<'a>(&'a self, s: &'a str) -> &'a str {
        Lang::t(*self, s)
    }
    fn tn<'a>(&'a self, n: u64, one: &'a str, other: &'a str) -> &'a str {
        Lang::tn(*self, n, one, other)
    }
    fn conventions(&self) -> Conventions {
        Lang::conventions(*self)
    }
}

/// A catalogue read by hand (tests, tools) with the conventions it is for.
impl Words for (&Catalogue, Conventions) {
    fn t<'a>(&'a self, s: &'a str) -> &'a str {
        self.0.get(s).unwrap_or(s)
    }
    fn tn<'a>(&'a self, n: u64, one: &'a str, other: &'a str) -> &'a str {
        let english = if n == 1 { one } else { other };
        self.0.get_plural(one, n).unwrap_or(english)
    }
    fn conventions(&self) -> Conventions {
        self.1
    }
}

/// A calendar day, as plain numbers: month 1–12, day 1–31, weekday 0 Monday … 6 Sunday
/// (`mailcraft_time::Weekday::index`). Out-of-range numbers never panic: a month or a weekday
/// that doesn't exist has an empty name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Day {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub weekday: u8,
}

/// The name of `month` (1–12); empty for any other number.
pub fn month_name<W: Words + ?Sized>(w: &W, month: u8) -> &str {
    name(w, &MONTHS, usize::from(month).wrapping_sub(1))
}

/// `Oct`.
pub fn month_short<W: Words + ?Sized>(w: &W, month: u8) -> &str {
    name(w, &MONTHS_SHORT, usize::from(month).wrapping_sub(1))
}

/// The name of a weekday (0 Monday … 6 Sunday); empty for any other number.
pub fn weekday_name<W: Words + ?Sized>(w: &W, weekday: u8) -> &str {
    name(w, &WEEKDAYS, usize::from(weekday))
}

/// `Wed`.
pub fn weekday_short<W: Words + ?Sized>(w: &W, weekday: u8) -> &str {
    name(w, &WEEKDAYS_SHORT, usize::from(weekday))
}

fn name<'a, W: Words + ?Sized>(w: &'a W, names: &'static [&'static str], i: usize) -> &'a str {
    names.get(i).map_or("", |n| w.t(n))
}

/// `Sunday, October 11, 2026`.
pub fn long<W: Words + ?Sized>(w: &W, d: Day) -> String {
    fmt(
        w.t(LONG),
        &[("weekday", weekday_name(w, d.weekday)), ("month", month_name(w, d.month)), ("day", &d.day.to_string()), ("year", &d.year.to_string())],
    )
}

/// `Oct 11, 2026`.
pub fn medium<W: Words + ?Sized>(w: &W, d: Day) -> String {
    fmt(w.t(MEDIUM), &[("month", month_short(w, d.month)), ("day", &d.day.to_string()), ("year", &d.year.to_string())])
}

/// `10/11/2026`.
pub fn numeric<W: Words + ?Sized>(w: &W, d: Day) -> String {
    let (month, day) =
        if w.conventions().pad_numeric { (format!("{:02}", d.month), format!("{:02}", d.day)) } else { (d.month.to_string(), d.day.to_string()) };
    fmt(w.t(NUMERIC), &[("month", &month), ("day", &day), ("year", &d.year.to_string())])
}

/// `October 2026`.
pub fn month_year<W: Words + ?Sized>(w: &W, year: i32, month: u8) -> String {
    fmt(w.t(MONTH_YEAR), &[("month", month_name(w, month)), ("year", &year.to_string())])
}

/// `Wed, Oct 14, 2026`.
pub fn weekday_medium<W: Words + ?Sized>(w: &W, d: Day) -> String {
    fmt(w.t(WEEKDAY_MEDIUM), &[("weekday", weekday_short(w, d.weekday)), ("date", &medium(w, d))])
}

/// `October 11, 2026`: [`medium`] with the month in full.
pub fn long_without_weekday<W: Words + ?Sized>(w: &W, d: Day) -> String {
    fmt(w.t(MEDIUM), &[("month", month_name(w, d.month)), ("day", &d.day.to_string()), ("year", &d.year.to_string())])
}

/// `October 3`: [`month_day`] with the month in full.
pub fn month_day_long<W: Words + ?Sized>(w: &W, month: u8, day: u8) -> String {
    fmt(w.t(MONTH_DAY), &[("month", month_name(w, month)), ("day", &day.to_string())])
}

/// `Oct 3`.
pub fn month_day<W: Words + ?Sized>(w: &W, month: u8, day: u8) -> String {
    fmt(w.t(MONTH_DAY), &[("month", month_short(w, month)), ("day", &day.to_string())])
}

/// `Wed, Oct 14`.
pub fn weekday_month_day<W: Words + ?Sized>(w: &W, d: Day) -> String {
    fmt(w.t(WEEKDAY_MONTH_DAY), &[("weekday", weekday_short(w, d.weekday)), ("month", month_short(w, d.month)), ("day", &d.day.to_string())])
}

/// `Wednesday, October 14`.
pub fn weekday_month_day_long<W: Words + ?Sized>(w: &W, d: Day) -> String {
    fmt(w.t(WEEKDAY_MONTH_DAY), &[("weekday", weekday_name(w, d.weekday)), ("month", month_name(w, d.month)), ("day", &d.day.to_string())])
}

/// `9:30 AM`, or `09:30` on a 24-hour clock. Out-of-range numbers wrap (hour 24 is 0).
pub fn time<W: Words + ?Sized>(w: &W, hour: u8, minute: u8, twelve_hour: bool) -> String {
    let hour = hour % 24;
    let minute = format!("{:02}", minute % 60);
    if !twelve_hour {
        return fmt(w.t(CLOCK_24), &[("hour", &format!("{hour:02}")), ("minute", &minute)]);
    }
    let h = match hour % 12 {
        0 => 12,
        h => h,
    };
    fmt(w.t(CLOCK_12), &[("hour", &h.to_string()), ("minute", &minute), ("ampm", w.t(if hour < 12 { AM } else { PM }))])
}

/// The date column of a message list for something at `at` (`hour`:`minute`), `days_ago` days
/// before today (negative: in the future): the time today, `Sat 3:05 PM` within the last week,
/// `Oct 3` this year, `10/3/2025` before that.
pub fn list_label<W: Words + ?Sized>(w: &W, at: Day, hour: u8, minute: u8, days_ago: i64, this_year: bool, twelve_hour: bool) -> String {
    if days_ago == 0 {
        time(w, hour, minute, twelve_hour)
    } else if (1..7).contains(&days_ago) {
        fmt(w.t(WEEKDAY_TIME), &[("weekday", weekday_short(w, at.weekday)), ("time", &time(w, hour, minute, twelve_hour))])
    } else if this_year {
        month_day(w, at.month, at.day)
    } else {
        numeric(w, at)
    }
}

/// The date line of an open message: `Sun 10/11/2026 9:30 AM`.
pub fn header_label<W: Words + ?Sized>(w: &W, at: Day, hour: u8, minute: u8, twelve_hour: bool) -> String {
    fmt(
        w.t(WEEKDAY_DATE_TIME),
        &[("weekday", weekday_short(w, at.weekday)), ("date", &numeric(w, at)), ("time", &time(w, hour, minute, twelve_hour))],
    )
}

/// A length of time for people: `30 minutes`, `1 hour`, `1.5 hours`, `2 h 15 min`, `2 days`.
pub fn duration<W: Words + ?Sized>(w: &W, minutes: i64) -> String {
    let m = minutes.max(0);
    let count = |n: i64, (one, other): (&'static str, &'static str)| {
        let n = u64::try_from(n).unwrap_or(0);
        fmt(w.tn(n, one, other), &[("n", &n.to_string())])
    };
    if m < 60 {
        count(m, MINUTES)
    } else if m < 24 * 60 {
        match (m / 60, m % 60) {
            (h, 0) => count(h, HOURS),
            (h, 30) => fmt(w.t(HALF_HOURS), &[("n", &h.to_string())]),
            (h, r) => fmt(w.t(HOURS_MINUTES), &[("h", &h.to_string()), ("m", &r.to_string())]),
        }
    } else {
        count(m / (24 * 60), DAYS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plural::Rule;

    const SUN: Day = Day { year: 2026, month: 10, day: 11, weekday: 6 };
    const OLD: Day = Day { year: 2025, month: 10, day: 3, weekday: 4 };

    #[test]
    fn english_is_what_the_program_always_said() {
        let en = Lang::English;
        assert_eq!(long(&en, SUN), "Sunday, October 11, 2026");
        assert_eq!(medium(&en, SUN), "Oct 11, 2026");
        assert_eq!(numeric(&en, SUN), "10/11/2026");
        assert_eq!(numeric(&en, OLD), "10/3/2025");
        assert_eq!(month_year(&en, 2026, 10), "October 2026");
        assert_eq!(month_day(&en, 10, 3), "Oct 3");
        assert_eq!(month_day_long(&en, 3, 14), "March 14");
        assert_eq!(weekday_medium(&en, SUN), "Sun, Oct 11, 2026");
        assert_eq!(long_without_weekday(&en, SUN), "October 11, 2026");
        assert_eq!(weekday_month_day(&en, SUN), "Sun, Oct 11");
        assert_eq!(weekday_month_day_long(&en, SUN), "Sunday, October 11");
        assert_eq!(time(&en, 9, 30, true), "9:30 AM");
        assert_eq!(time(&en, 0, 5, true), "12:05 AM");
        assert_eq!(time(&en, 12, 0, true), "12:00 PM");
        assert_eq!(time(&en, 21, 30, false), "21:30");
        assert_eq!(time(&en, 9, 5, false), "09:05");
        assert_eq!(list_label(&en, SUN, 15, 5, 0, true, true), "3:05 PM");
        assert_eq!(list_label(&en, SUN, 15, 5, 1, true, true), "Sun 3:05 PM");
        assert_eq!(list_label(&en, SUN, 15, 5, 9, true, true), "Oct 11");
        assert_eq!(list_label(&en, OLD, 15, 5, 400, false, true), "10/3/2025");
        assert_eq!(list_label(&en, SUN, 15, 5, -2, true, true), "Oct 11");
        assert_eq!(header_label(&en, SUN, 9, 30, true), "Sun 10/11/2026 9:30 AM");
        let durations: Vec<String> = [-5, 0, 1, 30, 60, 90, 135, 120, 1440, 2880].into_iter().map(|m| duration(&en, m)).collect();
        assert_eq!(
            durations,
            ["0 minutes", "0 minutes", "1 minute", "30 minutes", "1 hour", "1.5 hours", "2 h 15 min", "2 hours", "1 day", "2 days"]
        );
    }

    #[test]
    fn hostile_numbers_never_panic() {
        let bad = Day { year: i32::MIN, month: 200, day: 0, weekday: 9 };
        for lang in Lang::ALL {
            assert_eq!(month_name(&lang, 0), "");
            assert_eq!(weekday_short(&lang, 7), "");
            let _ = (
                long(&lang, bad),
                numeric(&lang, bad),
                header_label(&lang, bad, 255, 255, true),
                list_label(&lang, bad, 99, 99, i64::MIN, false, false),
            );
            let _ = (duration(&lang, i64::MAX), duration(&lang, i64::MIN));
        }
        assert_eq!(time(&Lang::English, 24, 60, false), "00:00");
    }

    #[test]
    fn pseudo_goes_through_the_catalogue() {
        let p = Lang::Pseudo;
        assert_eq!(long(&p, SUN), "Súndáy, Óctóbér 11, 2026");
        assert_eq!(time(&p, 9, 30, true), "9:30 ÁM");
        assert_eq!(header_label(&p, SUN, 9, 30, true), "Sún 10/11/2026 9:30 ÁM");
        assert_eq!(duration(&p, 1), "1 mínúté");
        assert_eq!(weekday_month_day_long(&p, SUN), "Súndáy, Óctóbér 11");
    }

    /// A translator's catalogue reorders the pieces, names its own months and days, and counts by
    /// its own plural rule.
    #[test]
    fn a_catalogue_puts_the_pieces_in_its_own_order() {
        let c = Catalogue::parse(concat!(
            "msgid \"\"\nmsgstr \"Plural-Forms: nplurals=2; plural=(n > 1);\\n\"\n\n",
            "msgid \"{weekday}, {month} {day}, {year}\"\nmsgstr \"{weekday} {day} {month} {year}\"\n\n",
            "msgid \"{month} {day}, {year}\"\nmsgstr \"{day} {month} {year}\"\n\n",
            "msgid \"{month}/{day}/{year}\"\nmsgstr \"{day}/{month}/{year}\"\n\n",
            "msgid \"{month} {day}\"\nmsgstr \"{day} {month}\"\n\n",
            "msgid \"{weekday}, {month} {day}\"\nmsgstr \"{weekday} {day} {month}\"\n\n",
            "msgid \"{hour}:{minute}\"\nmsgstr \"{hour} h {minute}\"\n\n",
            "msgid \"Sunday\"\nmsgstr \"dimanche\"\n\nmsgid \"Sun\"\nmsgstr \"dim.\"\n\n",
            "msgid \"October\"\nmsgstr \"octobre\"\n\nmsgid \"Oct\"\nmsgstr \"oct.\"\n\n",
            "msgid \"{n} minute\"\nmsgid_plural \"{n} minutes\"\nmsgstr[0] \"{n} minute\"\nmsgstr[1] \"{n} minutes\"\n\n",
            "msgid \"{n}.5 hours\"\nmsgstr \"{n},5 heures\"\n",
        ));
        assert_eq!(c.rule(), Rule::AboveOne);
        let fr = (&c, Lang::French.conventions());
        assert_eq!(long(&fr, SUN), "dimanche 11 octobre 2026");
        assert_eq!(medium(&fr, SUN), "11 oct. 2026");
        assert_eq!(numeric(&fr, OLD), "03/10/2025");
        assert_eq!(month_day(&fr, 10, 3), "3 oct.");
        assert_eq!(weekday_month_day(&fr, SUN), "dim. 11 oct.");
        assert_eq!(weekday_month_day_long(&fr, SUN), "dimanche 11 octobre");
        assert_eq!(time(&fr, 9, 5, false), "09 h 05");
        assert_eq!(header_label(&fr, SUN, 21, 30, fr.1.twelve_hour), "dim. 11/10/2026 21 h 30");
        // Nought is singular in French.
        assert_eq!((duration(&fr, 0), duration(&fr, 1), duration(&fr, 2)), ("0 minute".into(), "1 minute".into(), "2 minutes".into()));
        assert_eq!(duration(&fr, 90), "1,5 heures");
        // What the catalogue lacks is English.
        assert_eq!(month_name(&fr, 5), "May");
    }

    #[test]
    fn each_language_has_its_own_clock_and_week() {
        assert_eq!(Lang::English.conventions(), Conventions { twelve_hour: true, week_start: 6, pad_numeric: false });
        assert_eq!(Lang::Pseudo.conventions(), Lang::English.conventions());
        for lang in [Lang::French, Lang::German, Lang::Spanish, Lang::Italian] {
            let c = lang.conventions();
            assert_eq!((c.twelve_hour, c.week_start, c.pad_numeric), (false, 0, true), "{lang:?}");
        }
    }

    /// The shipped catalogues: each language's own order, names and numeric form.
    #[test]
    fn the_shipped_catalogues_write_dates_their_own_way() {
        let said = |lang: Lang| (long(&lang, SUN), numeric(&lang, OLD), time(&lang, 21, 5, false));
        assert_eq!(said(Lang::German), ("Sonntag, 11. Oktober 2026".into(), "03.10.2025".into(), "21:05".into()));
        assert_eq!(said(Lang::French).1, "03/10/2025");
        assert_eq!(said(Lang::Spanish).1, "03/10/2025");
        assert_eq!(said(Lang::Italian).1, "03/10/2025");
        assert!(long(&Lang::Spanish, SUN).contains("domingo") && long(&Lang::Spanish, SUN).contains("octubre"));
        assert!(long(&Lang::Italian, SUN).contains("domenica") && long(&Lang::Italian, SUN).contains("ottobre"));
        assert!(long(&Lang::French, SUN).contains("dimanche") && long(&Lang::French, SUN).contains("octobre"));
    }
}
