// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Time zones in iCalendar text: what a `TZID` stands for, the `VTIMEZONE` definitions a file
//! brings with it (RFC 5545 §3.6.5), and the definitions that are written.
//!
//! Reading, a time with a `TZID` is an instant, found in this order:
//! 1. the name is one of UTC's;
//! 2. the file defines the zone (`VTIMEZONE`): its own definition says when its times are, since
//!    whoever wrote the file reckoned with it;
//! 3. the name is one that is known (the IANA database, Windows' names): that zone;
//! 4. the name mentions an offset (`(UTC-05:00) Somewhere`): that offset;
//! 5. otherwise the time is kept as it is written, like a time without a zone.
//!
//! The zone an event keeps as its own ([`Event::start_zone`](mailcraft_model::Event)) is a zone of
//! the database: the one the name says when the file's definition agrees with it at the event's
//! time, else one that keeps the same clock as the definition around that time, else the offset
//! the definition gives. Cases 4 and 5, and a definition that contradicts its name, are said
//! ([`IcsFile::notes`](crate::IcsFile)).
//!
//! Definitions are hostile text: offsets of more than eighteen hours are not offsets, and the
//! zones, their parts and their dates are all capped.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use mailcraft_time::{DAY, Date, DateTime, Timestamp, Weekday, Zone, days_in_month};

use crate::lines::{ContentLine, Lines, fold, prefix};
use crate::values::{When, format_local};

/// Most zones one file defines, most parts (`STANDARD`, `DAYLIGHT`) of one zone, and most dates
/// (`RDATE`) of one part.
const MAX_ZONES: usize = 64;
const MAX_PARTS: usize = 64;
const MAX_DATES: usize = 64;
/// The work one parse may spend on finding the instants of times in zones the file defines, in
/// units of one look at one part or one date of a definition. Past it, times are kept as written.
const WORK_BUDGET: u64 = 20_000_000;
/// How many weeks `Source::zone` compares, both sides, times two and one (the work of one miss).
const SAMPLES: u64 = 121;
/// The furthest from UTC an offset may be, in seconds.
const MAX_OFFSET: i32 = 18 * 3600;
/// Longest `TZID` looked at, and how much of one is shown in a note.
const MAX_TZID: usize = 128;
const SHOWN: usize = 60;
/// Most notes one file gives.
pub(crate) const MAX_NOTES: usize = 20;
/// How many weeks around an event a definition is compared with a zone of the database.
const WEEKS: i64 = 60;

/// On which day of each year a part of a zone begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Yearly {
    /// The `nth` (1–5, or negative from the end) `weekday` of `month`.
    Nth { month: u8, nth: i8, weekday: Weekday },
    /// Day `day` of `month`.
    Day { month: u8, day: u8 },
}

impl Yearly {
    fn date(self, year: i32) -> Option<Date> {
        match self {
            Yearly::Day { month, day } => Date::new(year, month, day),
            Yearly::Nth { month, nth, weekday } => {
                let first = Date::new(year, month, 1)?;
                if nth > 0 {
                    let offset = (i64::from(weekday.index()) - i64::from(first.weekday().index())).rem_euclid(7);
                    Date::new(year, month, u8::try_from(1 + offset + 7 * (i64::from(nth) - 1)).ok()?)
                } else {
                    let last = first.end_of_month();
                    let back = (i64::from(last.weekday().index()) - i64::from(weekday.index())).rem_euclid(7);
                    let day = last.add_days(-back - 7 * (i64::from(nth.saturating_neg()) - 1));
                    (day.month == month).then_some(day)
                }
            }
        }
    }
}

/// One `STANDARD` or `DAYLIGHT` part of a definition: from `start` on (a wall-clock time by the
/// offset `from`), and then again every year or on the listed dates, the offset is `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Part {
    start: DateTime,
    /// Seconds east of UTC before and after.
    from: i32,
    to: i32,
    yearly: Option<Yearly>,
    /// The rule holds up to this instant, and for this many beginnings.
    until: Option<Timestamp>,
    count: Option<u32>,
    dates: Vec<DateTime>,
}

impl Part {
    /// The wall-clock second from which `to` holds for a beginning written as `start`: a time
    /// the clocks skip is still by the offset before, and so is the first of an hour that comes
    /// twice (RFC 5545 §3.3.5).
    fn wall_from(&self, start: DateTime) -> i64 {
        start.civil_seconds().saturating_add(i64::from((self.to - self.from).max(0)))
    }

    fn instant_of(&self, start: DateTime) -> Timestamp {
        start.civil_seconds().saturating_sub(i64::from(self.from))
    }

    /// The beginning of the rule's year `year`, when the rule has one then.
    fn in_year(&self, year: i32) -> Option<DateTime> {
        let rule = self.yearly?;
        let first = self.start.date.year;
        if year < first || self.count.is_some_and(|count| i64::from(year) - i64::from(first) >= i64::from(count)) {
            return None;
        }
        let begins = DateTime { date: rule.date(year)?, ..self.start };
        (begins >= self.start && self.until.is_none_or(|until| self.instant_of(begins) <= until)).then_some(begins)
    }

    /// The last beginning at or before `limit`, as `key` measures beginnings.
    fn latest(&self, limit: i64, year: i32, key: &dyn Fn(&Part, DateTime) -> i64) -> Option<i64> {
        let mut best: Option<i64> = None;
        let mut take = |begins: DateTime| {
            let at = key(self, begins);
            if at <= limit && best.is_none_or(|have| at > have) {
                best = Some(at);
            }
        };
        take(self.start);
        self.dates.iter().copied().for_each(&mut take);
        // The year of the time asked about and the years beside it; and where the rule ends.
        let last = self.until.map(|until| DateTime::from_timestamp(until, 0).date.year);
        let counted = self.count.map(|count| self.start.date.year.saturating_add(i32::try_from(count).unwrap_or(i32::MAX)).saturating_sub(1));
        for year in [Some(year.saturating_add(1)), Some(year), Some(year.saturating_sub(1)), last, last.map(|y| y.saturating_sub(1)), counted]
            .into_iter()
            .flatten()
        {
            if let Some(begins) = self.in_year(year) {
                take(begins);
            }
        }
        best
    }
}

/// A zone as a file defines it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileZone {
    parts: Vec<Part>,
}

impl FileZone {
    /// What one look at the definition costs, in units of `WORK_BUDGET`.
    pub(crate) fn cost(&self) -> u64 {
        self.parts.iter().fold(0u64, |sum, part| sum.saturating_add(8).saturating_add(part.dates.len() as u64))
    }

    fn offset_by(&self, limit: i64, year: i32, key: &dyn Fn(&Part, DateTime) -> i64) -> i32 {
        let mut best: Option<(i64, i32)> = None;
        for part in &self.parts {
            if let Some(at) = part.latest(limit, year, key)
                && best.is_none_or(|(have, _)| at >= have)
            {
                best = Some((at, part.to));
            }
        }
        match best {
            Some((_, to)) => to,
            // Before the first beginning: the offset the first one changes from.
            None => self.parts.iter().min_by_key(|part| part.start).map_or(0, |part| part.from),
        }
    }

    /// Seconds east of UTC for a wall-clock time written in the zone.
    pub(crate) fn offset_for_wall(&self, wall: DateTime) -> i32 {
        self.offset_by(wall.civil_seconds(), wall.date.year, &|part, begins| part.wall_from(begins))
    }

    /// Seconds east of UTC at an instant.
    pub(crate) fn offset_at(&self, instant: Timestamp) -> i32 {
        self.offset_by(instant, DateTime::from_timestamp(instant, 0).date.year, &|part, begins| part.instant_of(begins))
    }

    /// The instant of a wall-clock time written in the zone.
    pub(crate) fn instant(&self, wall: DateTime) -> Timestamp {
        wall.civil_seconds().saturating_sub(i64::from(self.offset_for_wall(wall)))
    }
}

/// `+0100`, `-0500`, `+053000`: seconds east of UTC; `None` for anything else and for more than
/// an offset can be.
fn utc_offset(text: &str) -> Option<i32> {
    let text = text.trim();
    let (sign, digits) = match (text.strip_prefix('+'), text.strip_prefix('-')) {
        (Some(rest), _) => (1, rest),
        (None, Some(rest)) => (-1, rest),
        (None, None) => return None,
    };
    let digits: String = digits.chars().filter(|c| *c != ':').collect();
    if !matches!(digits.len(), 4 | 6) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let part = |at: usize| digits.get(at..at.saturating_add(2)).and_then(|d| d.parse::<i32>().ok());
    let (hours, minutes, seconds) = (part(0)?, part(2)?, if digits.len() == 6 { part(4)? } else { 0 });
    let total = hours * 3600 + minutes * 60 + seconds;
    (minutes < 60 && seconds < 60 && total <= MAX_OFFSET).then_some(sign * total)
}

/// The rule of a part: `FREQ=YEARLY;BYMONTH=3;BYDAY=2SU`, with the day of the month or the
/// part's own day when no weekday is named. `None` for a rule that isn't yearly.
fn yearly(rule: &str, start: DateTime) -> Option<(Yearly, Option<Timestamp>, Option<u32>)> {
    let (mut yearly, mut month, mut by_day, mut month_day) = (false, None, None, None);
    let (mut until, mut count) = (None, None);
    for part in prefix(rule, 512).split(';').take(16) {
        let Some((key, value)) = part.split_once('=') else { continue };
        let value = value.trim();
        match key.trim().to_ascii_uppercase().as_str() {
            "FREQ" => yearly = value.eq_ignore_ascii_case("YEARLY"),
            "INTERVAL" if value != "1" => return None,
            "BYMONTH" => month = value.parse::<u8>().ok().filter(|m| (1..=12).contains(m)),
            "BYMONTHDAY" => month_day = value.parse::<u8>().ok().filter(|d| (1..=31).contains(d)),
            "BYDAY" => {
                let split = value.len().checked_sub(2)?;
                let weekday = Weekday::from_ical(value.get(split..)?)?;
                let nth = match value.get(..split)? {
                    "" => return None,
                    nth => nth.parse::<i8>().ok().filter(|n| (1..=5).contains(n) || (-5..=-1).contains(n))?,
                };
                by_day = Some((nth, weekday));
            }
            "UNTIL" => until = When::parse(value).map(|at| at.wall().to_timestamp(0)),
            "COUNT" => count = value.parse::<u32>().ok(),
            _ => {}
        }
    }
    if !yearly {
        return None;
    }
    let month = month.unwrap_or(start.date.month);
    let rule = match (by_day, month_day) {
        (Some((nth, weekday)), _) => Yearly::Nth { month, nth, weekday },
        (None, day) => Yearly::Day { month, day: day.unwrap_or(start.date.day) },
    };
    Some((rule, until, count))
}

/// A `STANDARD` or `DAYLIGHT` part being read.
#[derive(Default)]
struct PartBuilder {
    start: Option<DateTime>,
    from: Option<i32>,
    to: Option<i32>,
    rule: Option<String>,
    dates: Vec<DateTime>,
}

impl PartBuilder {
    fn finish(self) -> Option<Part> {
        let (start, from, to) = (self.start?, self.from?, self.to?);
        let (yearly, until, count) = match self.rule.as_deref().and_then(|rule| yearly(rule, start)) {
            Some((rule, until, count)) => (Some(rule), until, count),
            None => (None, None, None),
        };
        Some(Part { start, from, to, yearly, until, count, dates: self.dates })
    }
}

/// How a `TZID` is read.
#[derive(Clone, Copy)]
pub(crate) enum Source<'a> {
    Utc,
    /// As the file defines it; and the zone of the database its name (or its `X-LIC-LOCATION`)
    /// says it is, when it says one.
    File(&'a FileZone, Option<Zone>),
    Known(Zone),
    /// Minutes east of UTC, from a name that mentions an offset.
    Offset(i32),
    Unknown,
}

/// The zones a file defines.
#[derive(Default)]
pub(crate) struct FileZones {
    /// The `TZID`, the definition, and the place it says it is (`X-LIC-LOCATION`).
    zones: Vec<(String, FileZone, String)>,
    /// Work spent so far (see `WORK_BUDGET`), times resolved so far, and whether the budget ran out.
    spent: Cell<u64>,
    resolved: Cell<u32>,
    stopped: Cell<bool>,
    /// What `Source::zone` found, per `TZID` and offset: a definition is compared with the
    /// database once, not once per property.
    found: RefCell<HashMap<(String, i64), (Option<Zone>, Option<String>)>>,
}

impl FileZones {
    /// Read the `VTIMEZONE`s of a text.
    pub(crate) fn collect(text: &str) -> FileZones {
        let mut out = FileZones::default();
        // Nothing to look for in most texts.
        let named = text.as_bytes().windows(9).any(|bytes| bytes.eq_ignore_ascii_case(b"VTIMEZONE"));
        if !named {
            return out;
        }
        let mut zone: Option<(String, FileZone, String)> = None;
        let mut part: Option<PartBuilder> = None;
        for raw in Lines::new(text) {
            let Some(line) = ContentLine::parse(&raw) else { continue };
            let value = prefix(line.value.trim(), MAX_TZID);
            match (line.name.as_str(), value.to_ascii_uppercase().as_str()) {
                ("BEGIN", "VTIMEZONE") => {
                    if out.zones.len() >= MAX_ZONES {
                        break;
                    }
                    zone = Some(Default::default());
                    part = None;
                }
                ("END", "VTIMEZONE") => {
                    part = None;
                    if let Some((name, definition, place)) = zone.take()
                        && !name.is_empty()
                        && !definition.parts.is_empty()
                        && !out.zones.iter().any(|(have, _, _)| *have == name)
                    {
                        out.zones.push((name, definition, place));
                    }
                }
                ("BEGIN", "STANDARD" | "DAYLIGHT") if zone.is_some() => part = Some(PartBuilder::default()),
                ("END", "STANDARD" | "DAYLIGHT") => {
                    if let (Some((_, definition, _)), Some(built)) = (&mut zone, part.take().and_then(PartBuilder::finish))
                        && definition.parts.len() < MAX_PARTS
                    {
                        definition.parts.push(built);
                    }
                }
                // Anything else that begins inside a zone is not a part of it.
                ("BEGIN", _) => part = None,
                (name, _) => match (&mut zone, &mut part) {
                    (Some(_), Some(part)) => match name {
                        "DTSTART" => part.start = part.start.or_else(|| When::parse(&line.value).map(When::wall)),
                        "TZOFFSETFROM" => part.from = part.from.or_else(|| utc_offset(&line.value)),
                        "TZOFFSETTO" => part.to = part.to.or_else(|| utc_offset(&line.value)),
                        "RRULE" if part.rule.is_none() => part.rule = Some(prefix(&line.value, 512).to_string()),
                        "RDATE" => {
                            let room = MAX_DATES.saturating_sub(part.dates.len());
                            part.dates.extend(line.value.split(',').take(room).filter_map(When::parse).map(When::wall));
                        }
                        _ => {}
                    },
                    (Some((tzid, _, place)), None) => match name {
                        "TZID" if tzid.is_empty() => *tzid = value.to_string(),
                        "X-LIC-LOCATION" if place.is_empty() => *place = value.to_string(),
                        _ => {}
                    },
                    _ => {}
                },
            }
        }
        out
    }

    /// Spend `units` of the file's work budget; `false` (and the budget is out for good) when
    /// there is not enough left.
    pub(crate) fn spend(&self, units: u64) -> bool {
        let total = self.spent.get().saturating_add(units);
        if self.stopped.get() || total > WORK_BUDGET {
            self.stopped.set(true);
            return false;
        }
        self.spent.set(total);
        self.resolved.set(self.resolved.get().saturating_add(1));
        true
    }

    /// What is said when the budget ran out.
    pub(crate) fn stopped_note(&self) -> Option<String> {
        self.stopped.get().then(|| {
            format!(
                "Time zone work stopped after {} times: the rest of the times in the zones this file defines are kept as they are written.",
                self.resolved.get()
            )
        })
    }

    /// How times with this `TZID` are read.
    pub(crate) fn source(&self, tzid: &str) -> Source<'_> {
        let tzid = prefix(tzid.trim(), MAX_TZID);
        let known = Zone::named(tzid);
        if known == Some(Zone::UTC) {
            return Source::Utc;
        }
        if let Some((_, definition, place)) =
            self.zones.iter().find(|(name, _, _)| name == tzid).or_else(|| self.zones.iter().find(|(name, _, _)| name.eq_ignore_ascii_case(tzid)))
        {
            return Source::File(definition, known.or_else(|| Zone::named(place)));
        }
        match (known, Zone::offset_in_name(tzid)) {
            (Some(zone), _) => Source::Known(zone),
            (None, Some(minutes)) => Source::Offset(minutes),
            (None, None) => Source::Unknown,
        }
    }
}

/// What is said about a zone that couldn't be taken at its word.
pub(crate) fn note(tzid: &str, what: &str) -> String {
    let shown: String = prefix(tzid.trim(), SHOWN).chars().filter(|c| !c.is_control()).collect();
    format!("The time zone \"{shown}\" {what}")
}

impl Source<'_> {
    /// The instant of a wall-clock time written in the zone; `None` when nothing says (the time
    /// is then kept as it is written).
    pub(crate) fn instant(&self, wall: DateTime) -> Option<Timestamp> {
        match self {
            Source::Utc => Some(wall.to_timestamp(0)),
            Source::File(definition, _) => Some(definition.instant(wall)),
            Source::Known(zone) => Some(zone.instant(wall)),
            Source::Offset(minutes) => Some(wall.to_timestamp(*minutes)),
            Source::Unknown => None,
        }
    }

    /// The zone of the database an event written in this zone at `wall` (which is `instant`)
    /// keeps as its own, and what there is to say about it.
    #[cfg(test)]
    pub(crate) fn zone(&self, tzid: &str, wall: DateTime, instant: Timestamp) -> (Option<Zone>, Option<String>) {
        self.zone_in(None, tzid, wall, instant)
    }

    /// As [`zone`](Self::zone), remembering in `zones` what a definition came to, per offset.
    pub(crate) fn zone_in(&self, zones: Option<&FileZones>, tzid: &str, wall: DateTime, instant: Timestamp) -> (Option<Zone>, Option<String>) {
        match self {
            Source::Utc => (Some(Zone::UTC), None),
            Source::Known(zone) => (Some(*zone), None),
            Source::Offset(minutes) => {
                let zone = Zone::fixed(*minutes);
                let label = zone.map(Zone::id).unwrap_or_default();
                (zone, Some(note(tzid, &format!("isn't one that is known: its times are kept at {label}, which is all its name says."))))
            }
            Source::Unknown => (None, Some(note(tzid, "isn't one that is known: its times are kept as they are written."))),
            Source::File(definition, named) => {
                let offset = i64::from(definition.offset_for_wall(wall));
                if let Some(zone) = named.filter(|zone| zone.offset_seconds_at(instant) == offset) {
                    return (Some(zone), None);
                }
                let key = (tzid.to_string(), offset);
                if let Some(found) = zones.and_then(|zones| zones.found.borrow().get(&key).cloned()) {
                    return found;
                }
                let affordable = zones.is_none_or(|zones| zones.spend(SAMPLES.saturating_mul(definition.cost())));
                let contradicts = named
                    .map(|_| note(tzid, "is defined in the file otherwise than the zone of that name: its times are read as the file defines them."));
                // A zone of the database that keeps the same clock around then.
                let same = |zone: &Zone| {
                    (-WEEKS..=WEEKS).all(|week| {
                        let at = instant.saturating_add(week * 7 * DAY);
                        zone.offset_seconds_at(at) == i64::from(definition.offset_at(at))
                    })
                };
                let chosen =
                    if affordable { Zone::choices().into_iter().filter(|zone| zone.offset_seconds_at(instant) == offset).find(same) } else { None };
                if let Some(zone) = chosen {
                    let result = (Some(zone), contradicts);
                    if let Some(zones) = zones {
                        zones.found.borrow_mut().insert(key, result.clone());
                    }
                    return result;
                }
                let fixed = i32::try_from(offset.div_euclid(60)).ok().and_then(Zone::fixed);
                let label = fixed.map(Zone::id).unwrap_or_default();
                let said = contradicts
                    .unwrap_or_else(|| note(tzid, &format!("is defined in the file and is like none that is known: its times are kept at {label}.")));
                let result = (fixed, Some(said));
                if affordable && let Some(zones) = zones {
                    zones.found.borrow_mut().insert(key, result.clone());
                }
                result
            }
        }
    }
}

// ---- writing ----

/// What a zone is called in a file that is written: the name of the database, or for an offset
/// a name that says it.
pub(crate) fn tzid(zone: Zone) -> String {
    zone.id()
}

/// The offset as `+0100`, `-0930`, `+045602`.
fn format_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let a = seconds.unsigned_abs();
    if a.is_multiple_of(60) {
        format!("{sign}{:02}{:02}", a / 3600, a / 60 % 60)
    } else {
        format!("{sign}{:02}{:02}{:02}", a / 3600, a / 60 % 60, a % 60)
    }
}

/// The rule a change of clocks follows when it falls on the same weekday of the same month each
/// year at the same time of day: the `nth` weekday (the last, when it is in the month's last week
/// in each of the years).
fn yearly_rule(changes: &[(DateTime, i32, i32)]) -> Option<Yearly> {
    let (first, _, _) = changes.first()?;
    let alike = changes.iter().all(|(at, from, to)| {
        let (_, first_from, first_to) = changes.first().copied().unwrap_or((*at, *from, *to));
        at.date.month == first.date.month
            && at.date.weekday() == first.date.weekday()
            && at.seconds_of_day() == first.seconds_of_day()
            && (*from, *to) == (first_from, first_to)
    });
    if !alike {
        return None;
    }
    let nth = |at: &DateTime| (at.date.day - 1) / 7 + 1;
    let last = |at: &DateTime| u16::from(at.date.day) + 7 > u16::from(days_in_month(at.date.year, at.date.month));
    let (month, weekday) = (first.date.month, first.date.weekday());
    if changes.iter().all(|(at, _, _)| nth(at) == nth(first)) && nth(first) < 5 && !changes.iter().all(|(at, _, _)| last(at)) {
        Some(Yearly::Nth { month, nth: nth(first) as i8, weekday })
    } else if changes.iter().all(|(at, _, _)| last(at)) {
        Some(Yearly::Nth { month, nth: -1, weekday })
    } else {
        None
    }
}

fn write_part(out: &mut String, kind: &str, start: DateTime, from: i32, to: i32, rule: Option<Yearly>) {
    fold(out, &format!("BEGIN:{kind}"));
    fold(out, &format!("DTSTART:{}", format_local(start)));
    fold(out, &format!("TZOFFSETFROM:{}", format_offset(from)));
    fold(out, &format!("TZOFFSETTO:{}", format_offset(to)));
    if let Some(Yearly::Nth { month, nth, weekday }) = rule {
        fold(out, &format!("RRULE:FREQ=YEARLY;BYMONTH={month};BYDAY={nth}{}", weekday.ical()));
    }
    fold(out, &format!("END:{kind}"));
}

/// Write the definition of `zone` for times in the years `from`–`to`.
///
/// A zone that changes its clocks twice a year by one rule is written as most programs write it,
/// and all of them read it: one `STANDARD` and one `DAYLIGHT` part, each with its yearly rule. A
/// zone that doesn't change is one `STANDARD` part. Any other (a year in which the rule changed,
/// a zone that follows the moon) is written out change by change for those years.
pub(crate) fn write_zone(out: &mut String, zone: Zone, from: i32, to: i32) {
    let (from, to) = (from.clamp(2, 9_990), to.clamp(2, 9_990).max(from.clamp(2, 9_990)));
    // A few hundred changes at most are looked at.
    let to = to.min(from.saturating_add(120));
    let instant = |year: i32| Date::new(year, 1, 1).map_or(0, |day| DateTime::at_midnight(day).to_timestamp(0));
    let before = i32::try_from(zone.offset_seconds_at(instant(from - 1))).unwrap_or(0);
    // Each change as it is written: the wall-clock time by the offset before it.
    let changes: Vec<(DateTime, i32, i32)> = zone
        .transitions(instant(from - 1), instant(to + 1))
        .into_iter()
        .map(|change| (DateTime::from_civil_seconds(change.at.saturating_add(i64::from(change.before))), change.before, change.after))
        .collect();
    fold(out, "BEGIN:VTIMEZONE");
    fold(out, &format!("TZID:{}", tzid(zone)));
    if changes.is_empty() {
        write_part(out, "STANDARD", DateTime::at_midnight(Date::new(1601, 1, 1).unwrap_or_default()), before, before, None);
        fold(out, "END:VTIMEZONE");
        return;
    }
    // Clocks forward is the beginning of daylight time, clocks back the end of it.
    let (forward, back): (Vec<_>, Vec<_>) = changes.iter().copied().partition(|(_, from, to)| to > from);
    let years = usize::try_from(i64::from(to) - i64::from(from) + 2).unwrap_or(0);
    let regular = forward.len() == years && back.len() == years;
    match (yearly_rule(&forward).filter(|_| regular), yearly_rule(&back).filter(|_| regular)) {
        (Some(spring), Some(autumn)) => {
            for (kind, list, rule) in [("DAYLIGHT", &forward, spring), ("STANDARD", &back, autumn)] {
                if let Some((start, from, to)) = list.first().copied() {
                    write_part(out, kind, start, from, to, Some(rule));
                }
            }
        }
        _ => {
            // What holds before the first change, then each change.
            let first = changes.first().map_or(DateTime::default(), |(at, _, _)| *at);
            write_part(
                out,
                "STANDARD",
                DateTime::at_midnight(Date::new(first.date.year.saturating_sub(1).max(1), 1, 1).unwrap_or_default()),
                before,
                before,
                None,
            );
            for (start, from, to) in changes {
                write_part(out, if to > from { "DAYLIGHT" } else { "STANDARD" }, start, from, to, None);
            }
        }
    }
    fold(out, "END:VTIMEZONE");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    fn at(y: i32, m: u8, day: u8, h: u8, min: u8) -> DateTime {
        DateTime::new(d(y, m, day), h, min, 0).unwrap()
    }

    fn zone(name: &str) -> Zone {
        Zone::named(name).unwrap()
    }

    /// The zone as a big mail program writes it: today's rule, from the year 1601.
    const EASTERN: &str = "BEGIN:VTIMEZONE\r\nTZID:Eastern Standard Time\r\nBEGIN:STANDARD\r\nDTSTART:16011104T020000\r\nRRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=11\r\n\
TZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\nEND:STANDARD\r\nBEGIN:DAYLIGHT\r\nDTSTART:16010311T020000\r\nRRULE:FREQ=YEARLY;BYDAY=2SU;BYMONTH=3\r\n\
TZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\n";

    fn only(text: &str) -> FileZone {
        let zones = FileZones::collect(text);
        assert_eq!(zones.zones.len(), 1, "{text}");
        zones.zones.into_iter().next().unwrap().1
    }

    #[test]
    fn a_definition_gives_each_time_its_offset() {
        let eastern = only(EASTERN);
        assert_eq!(eastern.offset_for_wall(at(2026, 1, 15, 9, 0)), -5 * 3600);
        assert_eq!(eastern.offset_for_wall(at(2026, 7, 15, 9, 0)), -4 * 3600);
        // March 8, 2026: 1:59 is winter time, 3:00 summer time, and 2:30 (which no clock shows)
        // is still read by the offset before.
        assert_eq!(eastern.offset_for_wall(at(2026, 3, 8, 1, 59)), -5 * 3600);
        assert_eq!(eastern.offset_for_wall(at(2026, 3, 8, 2, 30)), -5 * 3600);
        assert_eq!(eastern.offset_for_wall(at(2026, 3, 8, 3, 0)), -4 * 3600);
        // November 1: 1:30 comes twice, and is the first of the two.
        assert_eq!(eastern.offset_for_wall(at(2026, 11, 1, 1, 30)), -4 * 3600);
        assert_eq!(eastern.offset_for_wall(at(2026, 11, 1, 2, 0)), -5 * 3600);
        // The same by instants, and every hour of two years agrees with the database.
        let new_york = zone("America/New_York");
        let mut wall = at(2026, 1, 1, 0, 0);
        while wall < at(2028, 1, 1, 0, 0) {
            assert_eq!(eastern.instant(wall), new_york.instant(wall), "{}", wall.iso());
            let instant = wall.to_timestamp(0);
            assert_eq!(i64::from(eastern.offset_at(instant)), new_york.offset_seconds_at(instant), "{}", wall.iso());
            wall = wall.add_minutes(30);
        }
        // Before there were any rules: what the first change changes from.
        assert_eq!(eastern.offset_for_wall(at(1500, 6, 1, 0, 0)), -5 * 3600);
    }

    #[test]
    fn definitions_of_every_shape_are_read() {
        // Listed dates, a rule that ends, a rule by the day of the month, a part with no rule.
        let text = "BEGIN:VTIMEZONE\nTZID:Example/Odd\nX-LIC-LOCATION:Europe/Berlin\n\
BEGIN:STANDARD\nDTSTART:19700101T000000\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\nEND:STANDARD\n\
BEGIN:DAYLIGHT\nDTSTART:20200329T020000\nRDATE:20210328T020000,20220327T020000\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0200\nEND:DAYLIGHT\n\
BEGIN:STANDARD\nDTSTART:20201025T030000\nRRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU;UNTIL=20221030T010000Z\nTZOFFSETFROM:+0200\nTZOFFSETTO:+0100\nEND:STANDARD\n\
BEGIN:DAYLIGHT\nDTSTART:20240401T000000\nRRULE:FREQ=YEARLY;BYMONTHDAY=1;BYMONTH=4;COUNT=2\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0130\nEND:DAYLIGHT\n\
BEGIN:STANDARD\nDTSTART:20241001T000000\nRRULE:FREQ=YEARLY\nTZOFFSETFROM:+0130\nTZOFFSETTO:+0100\nEND:STANDARD\n\
END:VTIMEZONE\n";
        let odd = only(text);
        let offset = |wall: DateTime| odd.offset_for_wall(wall) / 60;
        assert_eq!(offset(at(2019, 7, 1, 12, 0)), 60);
        assert_eq!(offset(at(2020, 7, 1, 12, 0)), 120);
        assert_eq!(offset(at(2021, 1, 1, 12, 0)), 60);
        assert_eq!(offset(at(2022, 7, 1, 12, 0)), 120);
        assert_eq!(offset(at(2023, 7, 1, 12, 0)), 60);
        // Two Aprils of half an hour, and no third.
        assert_eq!(offset(at(2024, 7, 1, 12, 0)), 90);
        assert_eq!(offset(at(2025, 7, 1, 12, 0)), 90);
        assert_eq!(offset(at(2025, 12, 1, 12, 0)), 60);
        assert_eq!(offset(at(2026, 7, 1, 12, 0)), 60);
        // It says where it is.
        let zones = FileZones::collect(text);
        assert!(matches!(zones.source("Example/Odd"), Source::File(_, Some(named)) if named == zone("Europe/Berlin")));
        assert!(matches!(zones.source("example/odd"), Source::File(..)));
    }

    #[test]
    fn nonsense_definitions_are_no_definitions() {
        for text in [
            "BEGIN:VTIMEZONE\nEND:VTIMEZONE\n",
            "BEGIN:VTIMEZONE\nTZID:Empty\nEND:VTIMEZONE\n",
            "BEGIN:VTIMEZONE\nBEGIN:STANDARD\nDTSTART:19700101T000000\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\nEND:STANDARD\nEND:VTIMEZONE\n",
            // Offsets of days, and no offsets at all.
            "BEGIN:VTIMEZONE\nTZID:Far\nBEGIN:STANDARD\nDTSTART:19700101T000000\nTZOFFSETFROM:+9900\nTZOFFSETTO:-7200\nEND:STANDARD\nEND:VTIMEZONE\n",
            "BEGIN:VTIMEZONE\nTZID:Far\nBEGIN:STANDARD\nDTSTART:19700101T000000\nTZOFFSETFROM:+1801\nTZOFFSETTO:+0100\nEND:STANDARD\nEND:VTIMEZONE\n",
            "BEGIN:VTIMEZONE\nTZID:None\nBEGIN:STANDARD\nDTSTART:19700101T000000\nTZOFFSETFROM:soon\nTZOFFSETTO:later\nEND:STANDARD\nEND:VTIMEZONE\n",
            "BEGIN:VTIMEZONE\nTZID:NoStart\nBEGIN:STANDARD\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\nEND:STANDARD\nEND:VTIMEZONE\n",
            // A part outside a zone, and a zone that never ends.
            "BEGIN:STANDARD\nDTSTART:19700101T000000\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\nEND:STANDARD\n",
            "BEGIN:VTIMEZONE\nTZID:Open\nBEGIN:STANDARD\nDTSTART:19700101T000000\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\nEND:STANDARD\n",
        ] {
            assert!(FileZones::collect(text).zones.is_empty(), "{text}");
        }
        assert_eq!(utc_offset("+053000"), Some(5 * 3600 + 30 * 60));
        assert_eq!(utc_offset("-00:30"), Some(-30 * 60));
        for bad in ["", "0100", "+1", "+01", "+0160", "+010060", "+2400", "+١٢٠٠", "++0100"] {
            assert_eq!(utc_offset(bad), None, "{bad}");
        }
        // A megabyte of name, hundreds of zones, thousands of parts and dates: all bounded.
        let long = format!(
            "BEGIN:VTIMEZONE\nTZID:{}\nBEGIN:STANDARD\nDTSTART:19700101T000000\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\nEND:STANDARD\nEND:VTIMEZONE\n",
            "Z".repeat(1 << 20)
        );
        let zones = FileZones::collect(&long);
        assert!(zones.zones[0].0.len() <= MAX_TZID);
        assert!(matches!(zones.source(&"Z".repeat(1 << 20)), Source::File(..)));
        let many: String = (0..500).map(|n| format!("BEGIN:VTIMEZONE\nTZID:Zone{n}\nBEGIN:STANDARD\nDTSTART:19700101T000000\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\nEND:STANDARD\nEND:VTIMEZONE\n")).collect();
        assert_eq!(FileZones::collect(&many).zones.len(), MAX_ZONES);
        let part = format!(
            "BEGIN:STANDARD\nDTSTART:19700101T000000\nRDATE:{}\nTZOFFSETFROM:+0100\nTZOFFSETTO:+0100\nEND:STANDARD\n",
            "19800101T000000,".repeat(5_000)
        );
        let crowded = only(&format!("BEGIN:VTIMEZONE\nTZID:Crowded\n{}END:VTIMEZONE\n", part.repeat(1_000)));
        assert_eq!((crowded.parts.len(), crowded.parts[0].dates.len()), (MAX_PARTS, MAX_DATES));
        let _ = crowded.offset_for_wall(at(2026, 1, 1, 0, 0));
        // Hostile times ask nothing that panics.
        let eastern = only(EASTERN);
        for wall in [DateTime { date: Date { year: i32::MAX, month: 99, day: 99 }, hour: 99, minute: 99, second: 99 }, DateTime::default()] {
            let _ = (eastern.offset_for_wall(wall), eastern.instant(wall));
        }
        let _ = (eastern.offset_at(i64::MIN), eastern.offset_at(i64::MAX));
    }

    #[test]
    fn a_tzid_is_read_by_what_is_known_of_it() {
        let none = FileZones::default();
        assert!(matches!(none.source("UTC"), Source::Utc));
        assert!(matches!(none.source("Etc/UTC"), Source::Utc));
        assert!(matches!(none.source("Europe/Berlin"), Source::Known(z) if z == zone("Europe/Berlin")));
        assert!(matches!(none.source("W. Europe Standard Time"), Source::Known(z) if z == zone("Europe/Berlin")));
        assert!(matches!(none.source("(UTC-05:00) Somewhere Else"), Source::Offset(-300)));
        assert!(matches!(none.source("Customized Time Zone"), Source::Unknown));
        assert!(matches!(none.source(&"x".repeat(1 << 20)), Source::Unknown));
        let wall = at(2026, 10, 12, 9, 0);
        // What an event keeps as its zone, and what is said.
        let known = none.source("Europe/Berlin");
        assert_eq!(known.zone("Europe/Berlin", wall, known.instant(wall).unwrap()), (Some(zone("Europe/Berlin")), None));
        let offset = none.source("(UTC-05:00) Somewhere Else");
        let (kept, said) = offset.zone("(UTC-05:00) Somewhere Else", wall, offset.instant(wall).unwrap());
        assert_eq!((kept, offset.instant(wall)), (Zone::fixed(-300), Some(wall.to_timestamp(-300))));
        assert_eq!(
            said.as_deref(),
            Some(
                "The time zone \"(UTC-05:00) Somewhere Else\" isn't one that is known: its times are kept at UTC-05:00, which is all its name says."
            )
        );
        let unknown = none.source("Customized Time Zone");
        assert_eq!(unknown.instant(wall), None);
        let (kept, said) = unknown.zone("Customized\r\nTime Zone", wall, 0);
        assert!(kept.is_none() && said.unwrap().contains("\"CustomizedTime Zone\" isn't one that is known"));

        // Defined in the file as it is: the zone of that name.
        let zones = FileZones::collect(EASTERN);
        let file = zones.source("Eastern Standard Time");
        let instant = file.instant(wall).unwrap();
        assert_eq!(instant, zone("America/New_York").instant(wall));
        assert_eq!(file.zone("Eastern Standard Time", wall, instant), (Some(zone("America/New_York")), None));
        // The same definition under a name nobody knows is the zone with that clock.
        let renamed = EASTERN.replace("Eastern Standard Time", "Customized Time Zone");
        let zones = FileZones::collect(&renamed);
        let file = zones.source("Customized Time Zone");
        assert_eq!(file.zone("Customized Time Zone", wall, instant), (Some(zone("America/New_York")), None));
        // A definition that contradicts its name: the file's times, the zone with the file's
        // clock, and a word about it.
        let wrong = EASTERN.replace("Eastern Standard Time", "Pacific Standard Time");
        let zones = FileZones::collect(&wrong);
        let file = zones.source("Pacific Standard Time");
        assert_eq!(file.instant(wall), Some(instant));
        let (kept, said) = file.zone("Pacific Standard Time", wall, instant);
        assert_eq!(kept, Some(zone("America/New_York")));
        assert!(said.unwrap().contains("is defined in the file otherwise than the zone of that name"));
        // A clock no zone keeps: the offset it has then.
        let alone = "BEGIN:VTIMEZONE\nTZID:Example/Alone\nBEGIN:STANDARD\nDTSTART:19700101T000000\nTZOFFSETFROM:+0317\nTZOFFSETTO:+0317\nEND:STANDARD\nEND:VTIMEZONE\n";
        let zones = FileZones::collect(alone);
        let file = zones.source("Example/Alone");
        let (kept, said) = file.zone("Example/Alone", wall, file.instant(wall).unwrap());
        assert_eq!(kept, Zone::fixed(197));
        assert!(said.unwrap().contains("is like none that is known: its times are kept at UTC+03:17."));
    }

    /// What a written definition says, read back.
    fn written(zone: Zone, from: i32, to: i32) -> (String, FileZone) {
        let mut out = String::new();
        write_zone(&mut out, zone, from, to);
        let read = only(&out);
        (out, read)
    }

    #[test]
    fn written_definitions_are_the_usual_two_rules_where_a_zone_has_them() {
        let (text, _) = written(zone("America/New_York"), 2026, 2026);
        assert_eq!(
            text.replace("\r\n", "\n"),
            "BEGIN:VTIMEZONE\nTZID:America/New_York\n\
BEGIN:DAYLIGHT\nDTSTART:20250309T020000\nTZOFFSETFROM:-0500\nTZOFFSETTO:-0400\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU\nEND:DAYLIGHT\n\
BEGIN:STANDARD\nDTSTART:20251102T020000\nTZOFFSETFROM:-0400\nTZOFFSETTO:-0500\nRRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU\nEND:STANDARD\n\
END:VTIMEZONE\n"
        );
        let (text, _) = written(zone("Europe/Berlin"), 2026, 2030);
        assert!(text.contains("RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU") && text.contains("RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU"), "{text}");
        assert!(text.contains("DTSTART:20250330T020000") && text.contains("DTSTART:20251026T030000"), "{text}");
        // The south: summer time begins in October.
        let (text, _) = written(zone("Australia/Sydney"), 2026, 2026);
        assert!(text.contains("RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=1SU") && text.contains("RRULE:FREQ=YEARLY;BYMONTH=4;BYDAY=1SU"), "{text}");
        // No changes: one part. An offset is a zone too.
        let (text, _) = written(zone("Asia/Kolkata"), 2026, 2026);
        assert_eq!(
            text.replace("\r\n", "\n"),
            "BEGIN:VTIMEZONE\nTZID:Asia/Kolkata\nBEGIN:STANDARD\nDTSTART:16010101T000000\nTZOFFSETFROM:+0530\nTZOFFSETTO:+0530\nEND:STANDARD\nEND:VTIMEZONE\n"
        );
        let (text, _) = written(Zone::fixed(-570).unwrap(), 2026, 2026);
        assert!(text.contains("TZID:UTC-09:30") && text.contains("TZOFFSETTO:-0930"), "{text}");
        // The year the United States changed its rule: written out change by change.
        let (text, read) = written(zone("America/New_York"), 2006, 2007);
        assert!(!text.contains("RRULE") && text.contains("DTSTART:20060402T020000") && text.contains("DTSTART:20070311T020000"), "{text}");
        assert_eq!(read.instant(at(2006, 3, 20, 9, 0)), zone("America/New_York").instant(at(2006, 3, 20, 9, 0)));
        // Hostile years are years.
        for (from, to) in [(i32::MIN, i32::MAX), (9_999, 1), (0, 0)] {
            let _ = written(zone("Europe/Berlin"), from, to);
        }
    }

    proptest::proptest! {
        #[test]
        fn a_written_definition_reads_as_the_zone_it_was_written_for(index in 0usize..200, year in 1971i32..2090, span in 0i32..4, minutes in 0i64..365 * 24 * 60) {
            let choices = Zone::choices();
            let zone = choices[index % choices.len()];
            let (text, read) = written(zone, year, year + span);
            // Any time of those years is the instant the database says.
            let wall = at(year, 1, 1, 0, 0).add_minutes(minutes).add_days(i64::from(span) * 365);
            proptest::prop_assert_eq!(read.instant(wall), zone.instant(wall), "{} at {}\n{}", zone.id(), wall.iso(), text);
            let instant = wall.to_timestamp(0);
            proptest::prop_assert_eq!(i64::from(read.offset_at(instant)), zone.offset_seconds_at(instant), "{} at {}\n{}", zone.id(), wall.iso(), text);
        }

        #[test]
        fn any_text_is_definitions_or_none(text in proptest::prelude::any::<String>(), parts in proptest::collection::vec("(BEGIN|END|DTSTART|TZOFFSETFROM|TZOFFSETTO|RRULE|RDATE|TZID):[-+A-Z0-9=;,]{0,24}", 0..40), wall in proptest::num::i64::ANY) {
            let _ = FileZones::collect(&text);
            let zones = FileZones::collect(&format!("BEGIN:VTIMEZONE\nTZID:X\n{}\nEND:VTIMEZONE\n", parts.join("\n")));
            let wall = DateTime::from_civil_seconds(wall);
            if let Source::File(definition, _) = zones.source("X") {
                let _ = (definition.offset_for_wall(wall), definition.offset_at(wall.to_timestamp(0)), definition.instant(wall));
            }
            let source = zones.source(&text);
            let _ = source.zone(&text, wall, source.instant(wall).unwrap_or(0));
        }
    }
}
