// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Time zones in the mailbox: the zone an event has of its own, and the zone the mailbox's
//! wall-clock times are written in.
//!
//! **How stored times are read.** [`Event::start`] and [`Event::end`] are wall-clock times in one
//! zone for the whole mailbox: the zone the calendar is shown in, which the mailbox names
//! ([`Mailbox::zone`]). A timed event is an instant; the wall-clock time is how that instant
//! reads in that zone. A whole-day event is days, the same days in every zone.
//!
//! - A mailbox written before zones were kept names no zone. Its times are this computer's, as
//!   they always were, and it is stamped with this computer's zone the first time it is settled
//!   ([`Mailbox::rebase`] from no zone): nothing in it changes but the stamp.
//! - When the zone the calendar is shown in is no longer the one the mailbox names (the computer
//!   was carried to another zone, the owner chose another one, the file was opened on another
//!   machine), [`Mailbox::rebase`] rewrites every timed event to the clock of the new zone:
//!   the instants stay, the clock times move. Whole-day events keep their days.
//! - A series repeats by a clock: nine o'clock is nine o'clock on both sides of a daylight-saving
//!   change in *its* zone. A series without a zone of its own repeats by the clock of the zone
//!   the mailbox names. Before that zone changes, the series is given it as its own
//!   ([`Event::start_zone`]): it goes on repeating by the clock it was made by.
//!
//! **A series with a zone of its own** keeps its rule in the days of that zone ("every Monday" is
//! Monday there), and its first occurrence in `start` and `end` by the clock here, like every
//! other event. Each occurrence is at the series' clock time in its own zone on a day the rule
//! names there, and is shown at whatever that instant is here ([`Event::occurrences_in`]). The
//! days of a series that are named one by one ([`Event::exceptions`],
//! [`Origin::occurrence`](crate::Origin), [`Occurrence::day`]) are named as days *here*: the day
//! there, moved by as many days as the first occurrence starts later (or earlier) here than
//! there. That is the day an occurrence starts on here, except where a change of clocks on one
//! side carries one occurrence across midnight.
//!
//! The zone the calendar is shown in is passed as its offset at an instant ([`OffsetAt`]), which is
//! what everything that merges a server's calendar already works with.

use std::collections::HashMap;
use std::sync::Arc;

use mailcraft_time::{DAY, Date, DateTime, MINUTE, Timestamp, Zone};

use crate::{Event, Id, Mailbox, Occurrence};

/// The zone the calendar is shown in: its offset from UTC (minutes east) at an instant.
pub type OffsetAt<'a> = &'a dyn Fn(Timestamp) -> i32;

/// Longest name of a zone an event keeps.
pub const MAX_ZONE: usize = 64;

/// The wall-clock time at an instant where the offset is `offset_at`.
pub fn wall_at(instant: Timestamp, offset_at: OffsetAt) -> DateTime {
    DateTime::from_timestamp(instant, offset_at(instant))
}

/// The instant of a wall-clock time where the offset is `offset_at`, as [`Zone::instant`] reads
/// one: a time the clocks show twice (the hour repeated when they go back) is the first of the
/// two, and a time they skip is read by the offset before the change. (A wall-clock time alone
/// can't say which of two it is; everything that reads one reads it this way, so that reading a
/// time, moving it to another zone and reading it again agree.)
pub fn instant_at(wall: DateTime, offset_at: OffsetAt) -> Timestamp {
    instants_at(wall, offset_at).0
}

/// The instant of a wall-clock time where the offset is `offset_at`, and for a time the clocks
/// show twice the second instant it names.
fn instants_at(wall: DateTime, offset_at: OffsetAt) -> (Timestamp, Option<Timestamp>) {
    let civil = wall.civil_seconds();
    // The offsets a day before and a day after: around a change of clocks, the two the time can
    // be read by.
    let before = i64::from(offset_at(civil.saturating_sub(DAY))) * MINUTE;
    let after = i64::from(offset_at(civil.saturating_add(DAY))) * MINUTE;
    let (first, second) = (civil.saturating_sub(before), civil.saturating_sub(after));
    let holds = |instant: Timestamp, offset: i64| i64::from(offset_at(instant)) * MINUTE == offset;
    match (holds(first, before), holds(second, after)) {
        (true, true) if first != second => (first.min(second), Some(first.max(second))),
        (false, true) => (second, None),
        // Read by the offset before: right when that is the one that holds, and what a time
        // the clocks skip is read by.
        _ => (first, None),
    }
}

/// The instant of a wall-clock time where the offset is `offset_at`; `repeated` says that of a
/// time the clocks show twice it is the second ([`Event::start_repeated`]).
pub fn instant_of(wall: DateTime, repeated: bool, offset_at: OffsetAt) -> Timestamp {
    match instants_at(wall, offset_at) {
        (_, Some(second)) if repeated => second,
        (first, _) => first,
    }
}

/// An instant as it is kept where the offset is `offset_at`: the wall-clock time, and whether it
/// is the second time the clocks show that time. [`instant_of`] reads the instant back exactly.
pub fn wall_of(instant: Timestamp, offset_at: OffsetAt) -> (DateTime, bool) {
    let wall = wall_at(instant, offset_at);
    (wall, matches!(instants_at(wall, offset_at), (_, Some(second)) if second == instant))
}

/// The name of a zone as an event keeps it: the zone's own name when it is one that is known,
/// nothing otherwise (an event never carries a zone that can't be worked with).
pub fn zone_name(name: &str) -> String {
    if name.len() > mailcraft_time::MAX_ZONE_NAME {
        return String::new();
    }
    Zone::named(name).map(Zone::id).filter(|id| id.len() <= MAX_ZONE).unwrap_or_default()
}

/// The names an event keeps for the zones its start and its end were written in (`start`,
/// `end`), when the event is at `instants` in a calendar whose offset is `here`: what goes into
/// [`Event::start_zone`] and [`Event::end_zone`].
///
/// - A series keeps the zone it was written in, whichever it is (UTC too): it repeats by that
///   zone's clock, and two zones that keep the same time today may not keep it next year. Its
///   end is by the same clock.
/// - An event that happens once keeps a zone whose clock shows another time than the
///   calendar's at the event. (UTC is never such a zone: it is how programs write an instant,
///   not where anybody is.) Otherwise it has no zone of its own, which is what an event made
///   here has. The end's zone is named only beside a start's, and when it is another.
pub fn own_zones(start: Option<Zone>, end: Option<Zone>, instants: (Timestamp, Timestamp), repeats: bool, here: OffsetAt) -> (String, String) {
    let differs = |zone: Zone, instant: Timestamp| zone.offset_seconds_at(instant) != i64::from(here(instant)) * MINUTE;
    let name = |zone: Option<Zone>| zone.filter(|zone| !zone.is_local()).map(Zone::id).filter(|id| id.len() <= MAX_ZONE).unwrap_or_default();
    let (start, end) = (start.filter(|zone| !zone.is_local()), end.filter(|zone| !zone.is_local()));
    if repeats {
        return (name(start), String::new());
    }
    let elsewhere = |zone: Option<Zone>, instant: Timestamp| zone.is_some_and(|zone| zone != Zone::UTC && differs(zone, instant));
    if !elsewhere(start, instants.0) && !elsewhere(end.or(start), instants.1) {
        return (String::new(), String::new());
    }
    (name(start), name(end.filter(|zone| Some(*zone) != start)))
}

/// How a series that repeats in a zone of its own lies in the calendar it is shown in.
struct Anchor {
    zone: Zone,
    /// The first occurrence by the clock of its own zone.
    first: DateTime,
    /// How many days later the first occurrence starts here than there.
    shift: i64,
    /// From the start of an occurrence to its end, in seconds.
    length: i64,
}

impl Anchor {
    /// When the occurrence of the series' day `day` (named as a day here) starts.
    fn instant(&self, day: Date) -> Timestamp {
        self.zone.instant(DateTime { date: day.add_days(-self.shift), ..self.first })
    }
}

impl Event {
    /// The zone the event has of its own for its start; `None` when it has none, when it is a
    /// whole-day event, and for a name that is no zone.
    pub fn zone(&self) -> Option<Zone> {
        if self.all_day || self.start_zone.is_empty() { None } else { Zone::named(&self.start_zone) }
    }

    /// The zone the event has of its own for its end: the one that is named for it, else the
    /// start's.
    pub fn zone_of_end(&self) -> Option<Zone> {
        if self.all_day {
            return None;
        }
        Some(&self.end_zone).filter(|name| !name.is_empty()).and_then(|name| Zone::named(name)).or_else(|| self.zone())
    }

    /// When the event starts and ends as instants, for a timed event shown in a calendar whose
    /// offset is `here`. (A whole-day event has no instants: it is days.)
    pub fn instants(&self, here: OffsetAt) -> (Timestamp, Timestamp) {
        let start = instant_of(self.start, self.start_repeated, here);
        (start, instant_of(self.end, self.end_repeated, here).max(start))
    }

    /// Put a timed event at these instants, by the clock of a calendar whose offset is `here`:
    /// what [`Event::instants`] reads back, exactly.
    pub fn set_instants(&mut self, start: Timestamp, end: Timestamp, here: OffsetAt) {
        (self.start, self.start_repeated) = wall_of(start, here);
        (self.end, self.end_repeated) = wall_of(end.max(start), here);
    }

    fn anchor(&self, here: OffsetAt) -> Option<Anchor> {
        self.recurrence.as_ref()?;
        let zone = self.zone()?;
        let (start, end) = self.instants(here);
        let first = zone.wall(start);
        Some(Anchor { zone, first, shift: first.date.days_until(self.start.date).clamp(-3, 3), length: end.saturating_sub(start) })
    }

    /// Every time the event happens on the days `from`–`to` (inclusive) of a calendar whose
    /// offset is `here`, in order. As [`Event::occurrences`], but a series with a zone of its
    /// own repeats by the clock of that zone: its rule names days there, each occurrence is at
    /// the series' time of day there, and it is shown at what that instant is here. So it moves
    /// by an hour here while only one of the two zones has daylight saving.
    pub fn occurrences_in(&self, from: Date, to: Date, here: OffsetAt) -> Vec<Occurrence> {
        let (Some(anchor), Some(rule)) = (self.anchor(here), &self.recurrence) else { return self.occurrences(from, to) };
        // An occurrence that started before `from` may still be running then; and a day there is
        // not quite the same day here.
        let reach = (anchor.length / DAY).clamp(0, 400) + 2;
        let there = |day: Date| day.add_days(-anchor.shift);
        let skipped: Vec<Date> = self.exceptions.iter().map(|day| there(*day)).collect();
        let window_start = DateTime::at_midnight(from);
        let window_end = DateTime::at_midnight(to).add_days(1);
        rule.dates(anchor.first.date, there(from).add_days(-reach), there(to).add_days(1), &skipped)
            .into_iter()
            .map(|day| {
                let start = anchor.zone.instant(DateTime { date: day, ..anchor.first });
                Occurrence {
                    event: self.id,
                    start: wall_at(start, here),
                    end: wall_at(start.saturating_add(anchor.length), here),
                    day: day.add_days(anchor.shift),
                }
            })
            .filter(|o| o.start < window_end && (o.end > window_start || o.start >= window_start))
            .collect()
    }

    /// The days of the series between `from` and `to` (inclusive), whether it skips them or not,
    /// named as [`Event::exceptions`] names them. Nothing for an event that doesn't repeat.
    pub fn series_days(&self, from: Date, to: Date, here: OffsetAt) -> Vec<Date> {
        let Some(rule) = &self.recurrence else { return Vec::new() };
        match self.anchor(here) {
            Some(anchor) => rule
                .dates(anchor.first.date, from.add_days(-anchor.shift), to.add_days(-anchor.shift), &[])
                .into_iter()
                .map(|day| day.add_days(anchor.shift))
                .collect(),
            None => rule.dates(self.start.date, from, to, &[]),
        }
    }

    /// When the occurrence of the series' day `day` starts, as an instant. For a timed series.
    pub fn occurrence_instant(&self, day: Date, here: OffsetAt) -> Timestamp {
        match self.anchor(here) {
            Some(anchor) => anchor.instant(day),
            None => instant_at(DateTime { date: day, ..self.start }, here),
        }
    }

    /// When the occurrence of the series' day `day` starts and ends, by the clock here.
    pub fn occurrence_times(&self, day: Date, here: OffsetAt) -> (DateTime, DateTime) {
        match self.anchor(here) {
            Some(anchor) => {
                let start = anchor.instant(day);
                (wall_at(start, here), wall_at(start.saturating_add(anchor.length), here))
            }
            None => {
                let start = DateTime { date: day, ..self.start };
                (start, start.add_minutes(self.duration_minutes()))
            }
        }
    }

    /// The day of the series that an occurrence at `instant` is, as [`Event::exceptions`] names
    /// it. For a timed series.
    pub fn occurrence_day(&self, instant: Timestamp, here: OffsetAt) -> Date {
        match self.anchor(here) {
            Some(anchor) => anchor.zone.wall(instant).date.add_days(anchor.shift),
            None => wall_at(instant, here).date,
        }
    }

    /// The day of the series whose occurrence starts on `day` here, when there is one: what a
    /// command about "the occurrence on this day" means.
    pub fn series_day_on(&self, day: Date, here: OffsetAt) -> Option<Date> {
        self.recurrence.as_ref()?;
        let around = self.occurrences_in(day.add_days(-1), day.add_days(1), here);
        around.iter().find(|o| o.start.date == day).or_else(|| around.iter().find(|o| o.day == day)).map(|o| o.day)
    }

    /// The event as it reads when the calendar is shown in `to` instead of `from`: the same
    /// instants by the other clock. `None` when nothing of it changes (a whole-day event, a
    /// zone that shows the same time). A series is given `from` as its own zone when it had
    /// none, so that it goes on repeating by the clock it was made by, and the days it names
    /// one by one are moved by as many days as its first occurrence moves; the number of days is
    /// returned for what else names days of that series.
    fn rebased(&self, from: Zone, to: Zone) -> Option<(Event, i64)> {
        if self.all_day {
            return None;
        }
        let (start, end) = self.instants(&|instant| from.offset_at(instant));
        let mut out = self.clone();
        out.set_instants(start, end, &|instant| to.offset_at(instant));
        // The other times that people proposed for it are said by the same clock: the moments
        // stay, as the event's own do.
        for proposal in &mut out.proposals {
            let moment = |wall: DateTime| instant_at(wall, &|instant| from.offset_at(instant));
            let (starts, ends) = (moment(proposal.start), moment(proposal.end));
            proposal.start = wall_at(starts, &|instant| to.offset_at(instant));
            proposal.end = wall_at(ends.max(starts), &|instant| to.offset_at(instant));
        }
        let mut moved = 0;
        if self.recurrence.is_some() {
            let own = self.zone().unwrap_or(from);
            if self.zone().is_none() {
                out.start_zone = from.id();
                out.end_zone = String::new();
            }
            let there = own.wall(start).date;
            moved = (there.days_until(out.start.date) - there.days_until(self.start.date)).clamp(-3, 3);
            if moved != 0 {
                out.exceptions = out.exceptions.iter().map(|day| day.add_days(moved)).collect();
            }
        }
        (out != *self).then_some((out, moved))
    }
}

impl Mailbox {
    /// The zone the times of the events are written in, when the mailbox names one that is
    /// known.
    pub fn written_in(&self) -> Option<Zone> {
        Zone::named(&self.zone).filter(|zone| !zone.is_local())
    }

    /// Whether anything in the mailbox is at a time that a zone gives a meaning to.
    pub fn has_times(&self) -> bool {
        !self.events.is_empty() || !self.event_changes.is_empty() || !self.event_checks.is_empty()
    }

    /// Have the wall-clock times of the events written by the clock of `to`, the zone the
    /// calendar is shown in from now on; returns how many events read differently for it.
    ///
    /// - A mailbox that names no zone is only stamped with `to`: its times are taken as written
    ///   in it (they were this computer's, and `to` is what this computer's zone is).
    /// - A mailbox that names another zone has every timed event moved to the clock of `to`: the
    ///   instants stay. Whole-day events keep their days. A series keeps repeating by the clock
    ///   of the zone it was written in ([`Event::start_zone`]). What waits to be sent to a server
    ///   ([`Mailbox::event_changes`]) and what was sent and is still looked for there
    ///   ([`Mailbox::event_checks`]) is moved the same way, so that nothing differs from what
    ///   the server has that didn't differ before.
    /// - A zone without a name ([`Zone::LOCAL`]), on either side, changes nothing: there is no
    ///   telling what to convert from or to.
    pub fn rebase(&mut self, to: Zone) -> usize {
        if to.is_local() {
            return 0;
        }
        let Some(from) = self.written_in() else {
            // (A name nobody knows is left: a program that knows it can still read the times.)
            if self.zone.is_empty() {
                self.zone = to.id();
            }
            return 0;
        };
        if from == to {
            return 0;
        }
        // The series first: they say by how many days the days of theirs that are named
        // elsewhere move.
        let mut moved: HashMap<(Id, String), i64> = HashMap::new();
        let mut changed = 0;
        for event in self.events.values_mut() {
            if let Some((fresh, days)) = event.rebased(from, to) {
                if days != 0 && !fresh.origin.id.is_empty() {
                    moved.insert((fresh.origin.account, fresh.origin.id.clone()), days);
                }
                *Arc::make_mut(event) = fresh;
                changed += 1;
            }
        }
        let stands_for = |event: &Event| {
            let days = moved.get(&(event.origin.account, event.origin.series.clone())).copied()?;
            event.origin.occurrence.map(|day| day.add_days(days))
        };
        for event in self.events.values_mut() {
            if let Some(day) = stands_for(event) {
                Arc::make_mut(event).origin.occurrence = Some(day);
            }
        }
        // What waits to be sent, and what was sent and is yet to be seen on the server (the
        // event a deletion took away): the same moments as before.
        let waiting = self.event_changes.iter_mut().filter_map(|change| change.base.as_deref_mut());
        let sent = self.event_checks.iter_mut().filter_map(|check| check.base.as_deref_mut());
        for base in waiting.chain(sent) {
            if let Some((fresh, _)) = base.rebased(from, to) {
                *base = fresh;
            }
            if let Some(day) = stands_for(base) {
                base.origin.occurrence = Some(day);
            }
        }
        self.zone = to.id();
        changed
    }
}

/// `9:00 AM – 10:00 AM (Eastern Time)`: the times of an event by the clock of its own zone, for
/// an event whose zone has another clock than the calendar's just then; `None` otherwise.
pub fn own_time_label(event: &Event, here: OffsetAt, twelve_hour: bool) -> Option<String> {
    let end_zone = event.zone_of_end()?;
    let (start, end) = event.instants(here);
    let same = |zone: Zone, instant: Timestamp| i64::from(here(instant)) * MINUTE == zone.offset_seconds_at(instant);
    let Some(zone) = event.zone() else {
        // Only where it ends is another zone.
        return (!same(end_zone, end)).then(|| format!("ends {} ({})", end_zone.wall(end).time_short(twelve_hour), end_zone.label()));
    };
    if same(zone, start) && same(end_zone, end) {
        return None;
    }
    let (from, to) = (zone.wall(start), end_zone.wall(end));
    let day = if to.date == from.date { String::new() } else { format!("{} ", to.date.medium()) };
    Some(if end_zone == zone {
        format!("{} – {day}{} ({})", from.time_short(twelve_hour), to.time_short(twelve_hour), zone.label())
    } else {
        format!("{} ({}) – {day}{} ({})", from.time_short(twelve_hour), zone.label(), to.time_short(twelve_hour), end_zone.label())
    })
}

#[cfg(test)]
mod tests {
    use mailcraft_time::Weekday;

    use super::*;
    use crate::remote::{EventChange, EventChangeKind, EventCheck};
    use crate::{Freq, Origin, Recurrence};

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    fn at(y: i32, m: u8, day: u8, h: u8, min: u8) -> DateTime {
        DateTime::new(d(y, m, day), h, min, 0).unwrap()
    }

    fn zone(name: &str) -> Zone {
        Zone::named(name).unwrap()
    }

    fn timed(id: Id, start: DateTime, minutes: i64) -> Event {
        Event { id, calendar: 1, title: format!("event {id}"), start, end: start.add_minutes(minutes), ..Default::default() }
    }

    /// A mailbox as one written before zones were kept: a meeting, a whole-day event, a weekly
    /// series, and a change to the meeting that waits for its server.
    fn old_mailbox() -> Mailbox {
        let mut mailbox = Mailbox::with_local_account("Ada", "ada@example.org");
        let calendar = mailbox.default_calendar().unwrap();
        let mut add = |event: Event| mailbox.add_event(Event { calendar, ..event }).unwrap();
        add(timed(101, at(2026, 10, 12, 9, 0), 60));
        add(Event { all_day: true, start: at(2026, 10, 14, 0, 0), end: at(2026, 10, 16, 0, 0), ..timed(102, at(2026, 10, 14, 0, 0), 0) });
        add(Event {
            recurrence: Some(Recurrence { by_day: vec![Weekday::Mon], ..Recurrence::every(Freq::Weekly) }),
            exceptions: vec![d(2026, 10, 19)],
            origin: Origin { account: 1, id: "series-1".into(), ..Default::default() },
            ..timed(103, at(2026, 10, 5, 22, 30), 60)
        });
        add(Event {
            origin: Origin { account: 1, id: "loose-1".into(), series: "series-1".into(), occurrence: Some(d(2026, 10, 19)), ..Default::default() },
            ..timed(104, at(2026, 10, 20, 8, 0), 30)
        });
        let base = Event { origin: Origin { account: 1, id: "meeting-1".into(), ..Default::default() }, ..timed(101, at(2026, 10, 12, 8, 0), 60) };
        mailbox.event_changes.push(EventChange {
            account: 1,
            kind: EventChangeKind::Update,
            event: 101,
            remote: "meeting-1".into(),
            base: Some(Box::new(base)),
            ..Default::default()
        });
        // And a deletion the server said it took, which no download has shown yet.
        let gone = Event { origin: Origin { account: 1, id: "gone-1".into(), ..Default::default() }, ..timed(105, at(2026, 10, 13, 12, 0), 30) };
        mailbox.event_checks.push(EventCheck {
            account: 1,
            kind: EventChangeKind::Delete,
            event: 105,
            remote: "gone-1".into(),
            base: Some(Box::new(gone)),
            ..Default::default()
        });
        mailbox
    }

    #[test]
    fn a_mailbox_from_before_zones_loads_unchanged_and_is_only_stamped() {
        let old = old_mailbox();
        // What such a file holds says nothing of zones, and reads back as it was.
        let text = serde_json::to_string(&old).unwrap();
        assert!(!text.contains("zone") && !text.contains("Zone"), "{text}");
        let mut mailbox: Mailbox = serde_json::from_str(&text).unwrap();
        assert_eq!(mailbox, old);
        assert_eq!((mailbox.written_in(), mailbox.has_times()), (None, true));
        // Settled in this computer's zone: the stamp, and nothing else.
        assert_eq!(mailbox.rebase(zone("America/Chicago")), 0);
        assert_eq!(mailbox.zone, "America/Chicago");
        let mut unstamped = mailbox.clone();
        unstamped.zone.clear();
        assert_eq!(unstamped, old);
        // And again: nothing.
        let stamped = mailbox.clone();
        assert_eq!(mailbox.rebase(zone("America/Chicago")), 0);
        assert_eq!(mailbox, stamped);
        // The stamp is kept in the file, and read back.
        let back: Mailbox = serde_json::from_str(&serde_json::to_string(&mailbox).unwrap()).unwrap();
        assert_eq!(back.written_in(), Some(zone("America/Chicago")));
    }

    #[test]
    fn travelling_keeps_instants_and_whole_days() {
        let (chicago, london) = (zone("America/Chicago"), zone("Europe/London"));
        let mut mailbox = old_mailbox();
        mailbox.rebase(chicago);
        let before = mailbox.clone();
        let instants = |mailbox: &Mailbox, zone: Zone| -> Vec<(Id, Timestamp, Timestamp)> {
            mailbox.events.values().filter(|e| !e.all_day).map(|e| (e.id, zone.instant(e.start), zone.instant(e.end))).collect()
        };
        // Chicago to London, in October: six hours later on the clock.
        assert_eq!(mailbox.rebase(london), 3);
        assert_eq!(mailbox.zone, "Europe/London");
        assert_eq!(instants(&mailbox, london), instants(&before, chicago));
        let event = |id: Id| mailbox.event(id).unwrap();
        assert_eq!((event(101).start, event(101).end), (at(2026, 10, 12, 15, 0), at(2026, 10, 12, 16, 0)));
        assert_eq!(event(101).start_zone, "");
        // The whole-day event is on its days, whatever the zone.
        assert_eq!(event(102), before.event(102).unwrap());
        // The series was made in Chicago and goes on repeating by its clock: Monday 22:30 there
        // is Tuesday 4:30 here, and the day it skips is named a day later too.
        let series = event(103);
        assert_eq!((series.start, series.start_zone.as_str(), series.end_zone.as_str()), (at(2026, 10, 6, 4, 30), "America/Chicago", ""));
        assert_eq!(series.recurrence, before.event(103).unwrap().recurrence);
        assert_eq!(series.exceptions, [d(2026, 10, 20)]);
        // The occurrence that is an event of its own stands for that day.
        assert_eq!(event(104).origin.occurrence, Some(d(2026, 10, 20)));
        assert_eq!(event(104).start, at(2026, 10, 20, 14, 0));
        // What waits for the server moved with it: the same difference as before, no other.
        let base = mailbox.event_changes[0].base.as_deref().unwrap();
        assert_eq!((base.start, base.end), (at(2026, 10, 12, 14, 0), at(2026, 10, 12, 15, 0)));
        assert_eq!(london.instant(base.start), chicago.instant(before.event_changes[0].base.as_deref().unwrap().start));
        // So did what was sent and is still looked for on the server: noon in Chicago.
        let gone = mailbox.event_checks[0].base.as_deref().unwrap();
        assert_eq!((gone.start, london.instant(gone.start)), (at(2026, 10, 13, 18, 0), chicago.instant(at(2026, 10, 13, 12, 0))));
        // And home again: everything as it was, but that the series now says where it was made.
        assert_eq!(mailbox.rebase(chicago), 3);
        let mut home = mailbox.clone();
        let series = home.event_mut(103).unwrap();
        (series.start_zone, series.end_zone) = (String::new(), String::new());
        assert_eq!(home, before);
    }

    #[test]
    fn the_other_times_people_proposed_travel_with_their_meeting() {
        let (chicago, london) = (zone("America/Chicago"), zone("Europe/London"));
        let mut mailbox = Mailbox::with_local_account("Ada", "ada@example.org");
        let proposal =
            crate::Proposal { from: crate::Address::new("Bo", "bo@example.org"), start: at(2026, 10, 13, 9, 0), end: at(2026, 10, 13, 10, 0) };
        let id = mailbox.add_event(Event { proposals: vec![proposal.clone()], ..timed(0, at(2026, 10, 12, 8, 0), 60) }).unwrap();
        mailbox.rebase(chicago);
        // Chicago to London, in October: six hours later on the clock, and the same moment.
        mailbox.rebase(london);
        let there = mailbox.event(id).unwrap().proposals[0].clone();
        assert_eq!((there.start, there.end), (at(2026, 10, 13, 15, 0), at(2026, 10, 13, 16, 0)));
        assert_eq!(london.instant(there.start), chicago.instant(proposal.start));
        // And home again: as it was.
        mailbox.rebase(chicago);
        assert_eq!(mailbox.event(id).unwrap().proposals, [proposal]);
    }

    #[test]
    fn a_series_keeps_its_clock_across_daylight_saving_on_either_side() {
        let (chicago, london, sydney) = (zone("America/Chicago"), zone("Europe/London"), zone("Australia/Sydney"));
        let here = |zone: Zone| move |instant: Timestamp| zone.offset_at(instant);
        // Every Monday at nine in Chicago, made there.
        let mut mailbox = Mailbox::with_local_account("Ada", "ada@example.org");
        let id = mailbox.add_event(Event { recurrence: Some(Recurrence::every(Freq::Weekly)), ..timed(0, at(2026, 10, 5, 9, 0), 30) }).unwrap();
        mailbox.rebase(chicago);
        let starts = |mailbox: &Mailbox, zone: Zone, from: Date, to: Date| -> Vec<String> {
            mailbox.event(id).unwrap().occurrences_in(from, to, &here(zone)).into_iter().map(|o| o.start.iso()).collect()
        };
        assert_eq!(
            starts(&mailbox, chicago, d(2026, 10, 19), d(2026, 11, 9)),
            ["2026-10-19T09:00:00", "2026-10-26T09:00:00", "2026-11-02T09:00:00", "2026-11-09T09:00:00"]
        );
        // Seen from London: three in the afternoon, but for the week when London has already
        // put its clocks back (October 25) and Chicago has not yet (November 1).
        mailbox.rebase(london);
        assert_eq!(
            starts(&mailbox, london, d(2026, 10, 19), d(2026, 11, 9)),
            ["2026-10-19T15:00:00", "2026-10-26T14:00:00", "2026-11-02T15:00:00", "2026-11-09T15:00:00"]
        );
        // Each of them is nine o'clock in Chicago.
        for o in mailbox.event(id).unwrap().occurrences_in(d(2026, 1, 1), d(2027, 12, 31), &here(london)) {
            assert_eq!(chicago.wall(london.instant(o.start)).time_short(false), "09:00", "{}", o.start.iso());
            assert_eq!(o.start.minutes_until(o.end), 30);
        }
        // Seen from Sydney (the south, where summer time ends in April): Tuesday in the small
        // hours, the day named for each occurrence a day after the day there, and every one of
        // them still nine o'clock on a Monday in Chicago.
        mailbox.rebase(sydney);
        let series = mailbox.event(id).unwrap().clone();
        assert_eq!((series.start, series.start_zone.as_str()), (at(2026, 10, 6, 1, 0), "America/Chicago"));
        let seen = series.occurrences_in(d(2026, 1, 1), d(2027, 12, 31), &here(sydney));
        assert_eq!(seen.len(), 65);
        for o in &seen {
            let there = chicago.wall(sydney.instant(o.start));
            assert_eq!((there.time_short(false).as_str(), there.date.weekday()), ("09:00", Weekday::Mon), "{}", o.start.iso());
            assert_eq!(o.day, there.date.add_days(1));
            assert_eq!(series.occurrence_day(sydney.instant(o.start), &here(sydney)), o.day);
            assert_eq!(series.occurrence_times(o.day, &here(sydney)), (o.start, o.end));
            assert_eq!(series.occurrence_instant(o.day, &here(sydney)), sydney.instant(o.start));
            assert_eq!(series.series_day_on(o.start.date, &here(sydney)), Some(o.day));
        }
        let clocks: std::collections::BTreeSet<String> = seen.iter().map(|o| o.start.time_short(false)).collect();
        assert_eq!(clocks.into_iter().collect::<Vec<_>>(), ["00:00", "01:00", "02:00"]);
        // A skipped day is skipped wherever the calendar is shown.
        let first = seen[0].day;
        mailbox.event_mut(id).unwrap().exceptions.push(first);
        assert_eq!(mailbox.event(id).unwrap().occurrences_in(d(2026, 1, 1), d(2027, 12, 31), &here(sydney)).len(), 64);
        mailbox.rebase(chicago);
        let home = mailbox.event(id).unwrap();
        assert_eq!(home.exceptions, [d(2026, 10, 5)]);
        assert_eq!(
            home.occurrences_in(d(2026, 10, 1), d(2026, 10, 20), &here(chicago)).iter().map(|o| o.start.iso()).collect::<Vec<_>>(),
            ["2026-10-12T09:00:00", "2026-10-19T09:00:00"]
        );
        assert_eq!(home.series_days(d(2026, 10, 1), d(2026, 10, 20), &here(chicago)), [d(2026, 10, 5), d(2026, 10, 12), d(2026, 10, 19)]);
    }

    #[test]
    fn a_monthly_series_counts_its_days_where_it_was_made() {
        let (tokyo, new_york) = (zone("Asia/Tokyo"), zone("America/New_York"));
        let here = |instant: Timestamp| new_york.offset_at(instant);
        // The first Monday of the month at nine in Tokyo, which is Sunday evening in New York.
        let rule = Recurrence { by_day: vec![Weekday::Mon], nth: Some(1), ..Recurrence::every(Freq::Monthly) };
        let series = Event {
            start: new_york.wall(tokyo.instant(at(2026, 10, 5, 9, 0))),
            end: new_york.wall(tokyo.instant(at(2026, 10, 5, 10, 0))),
            start_zone: "Asia/Tokyo".into(),
            recurrence: Some(rule),
            ..Default::default()
        };
        assert_eq!(series.start, at(2026, 10, 4, 20, 0));
        let seen = series.occurrences_in(d(2026, 10, 1), d(2027, 3, 31), &here);
        // November 2 in Tokyo is November 1 here, and so on; in winter New York is an hour
        // further from Tokyo.
        assert_eq!(
            seen.iter().map(|o| o.start.iso()).collect::<Vec<_>>(),
            [
                "2026-10-04T20:00:00",
                "2026-11-01T19:00:00",
                "2026-12-06T19:00:00",
                "2027-01-03T19:00:00",
                "2027-01-31T19:00:00",
                "2027-02-28T19:00:00"
            ]
        );
        assert!(seen.iter().all(|o| o.day == o.start.date));
        // Without its zone it would be taken for the first Monday here, a day late.
        let plain = Event { start_zone: String::new(), ..series.clone() };
        assert_eq!(plain.occurrences_in(d(2026, 11, 1), d(2026, 11, 30), &here)[0].start.iso(), "2026-11-02T20:00:00");
    }

    #[test]
    fn zones_of_an_event_and_what_is_said_of_them() {
        let chicago = zone("America/Chicago");
        let here = |instant: Timestamp| chicago.offset_at(instant);
        let plain = timed(1, at(2026, 10, 12, 14, 0), 60);
        assert_eq!((plain.zone(), plain.zone_of_end(), own_time_label(&plain, &here, true)), (None, None, None));
        // In its own zone, an hour ahead.
        let eastern = Event { start_zone: "America/New_York".into(), ..plain.clone() };
        assert_eq!(own_time_label(&eastern, &here, true).as_deref(), Some("3:00 PM – 4:00 PM (Eastern Time)"));
        assert_eq!(own_time_label(&eastern, &here, false).as_deref(), Some("15:00 – 16:00 (Eastern Time)"));
        // A zone with the calendar's own clock says nothing; nor does a whole-day event.
        let same = Event { start_zone: "America/Mexico_City".into(), ..timed(1, at(2026, 1, 12, 14, 0), 60) };
        assert_eq!(own_time_label(&same, &here, true), None);
        let whole = Event { all_day: true, ..eastern.clone() };
        assert_eq!((whole.zone(), own_time_label(&whole, &here, true)), (None, None));
        // A flight: it leaves by one clock and lands by another, the next day.
        let flight = Event { end_zone: "Asia/Tokyo".into(), ..timed(1, at(2026, 10, 12, 11, 0), 13 * 60) };
        assert_eq!(flight.zone(), None);
        let flight = Event { start_zone: "America/Chicago".into(), ..flight };
        assert_eq!(own_time_label(&flight, &here, true).as_deref(), Some("11:00 AM (Central Time) – Oct 13, 2026 2:00 PM (Tokyo Time)"));
        // A name that is no zone is no zone, however long it is.
        let odd = Event { start_zone: "Mars/Olympus_Mons".into(), end_zone: "x".repeat(1 << 20), ..plain };
        assert_eq!((odd.zone(), odd.zone_of_end(), own_time_label(&odd, &here, true)), (None, None, None));
        assert_eq!(zone_name("w. europe standard time"), "Europe/Berlin");
        assert_eq!(zone_name("Asia/Calcutta"), "Asia/Kolkata");
        assert_eq!(zone_name("UTC+05:30"), "UTC+05:30");
        assert_eq!(zone_name("Nowhere/At_All"), "");
        assert_eq!(zone_name(&"Europe/Berlin ".repeat(100_000)), "");
    }

    #[test]
    fn rebasing_survives_what_no_calendar_has() {
        let mut mailbox = Mailbox::with_local_account("Ada", "ada@example.org");
        let hostile = DateTime { date: Date { year: i32::MAX, month: 200, day: 0 }, hour: 99, minute: 99, second: 99 };
        for (n, start) in [hostile, DateTime::default(), at(1, 1, 1, 0, 0), at(9999, 12, 31, 23, 59)].into_iter().enumerate() {
            let rule = Recurrence { interval: u32::MAX, count: Some(u32::MAX), nth: Some(i8::MIN), ..Recurrence::every(Freq::Yearly) };
            let event = Event {
                id: n as Id + 1,
                start,
                end: hostile,
                recurrence: Some(rule),
                exceptions: vec![hostile.date; 50],
                start_zone: "\u{0}".repeat(1 << 16),
                ..Default::default()
            };
            mailbox.add_event(event).unwrap();
        }
        mailbox.zone = "Pacific/Kiritimati".into();
        for name in ["Pacific/Pago_Pago", "UTC", "UTC-18:00", "Asia/Kathmandu"] {
            mailbox.rebase(zone(name));
            assert_eq!(mailbox.zone, zone(name).id());
        }
        let here = |instant: Timestamp| (instant % 100_000) as i32;
        for event in mailbox.events.values() {
            let _ = event.occurrences_in(d(1, 1, 1), d(9999, 12, 31), &here);
            let _ = event.series_days(d(1, 1, 1), d(9999, 12, 31), &here);
            let _ = (event.occurrence_times(hostile.date, &here), event.occurrence_day(i64::MIN, &here), event.series_day_on(hostile.date, &here));
            let _ = own_time_label(event, &here, true);
        }
        // A zone nobody knows, and no zone at all: nothing moves.
        let before = mailbox.clone();
        assert_eq!(mailbox.rebase(Zone::LOCAL), 0);
        assert_eq!(mailbox, before);
        mailbox.zone = "Mars/Olympus_Mons".into();
        let before = mailbox.clone();
        assert_eq!(mailbox.rebase(zone("Europe/Berlin")), 0);
        assert_eq!(mailbox, before);
    }

    #[test]
    fn a_time_the_clocks_skip_stays_put_in_its_own_zone() {
        // The case the property test found: 01:00 on the night London's clocks go forward.
        let london = zone("Europe/London");
        let mut mailbox = Mailbox::with_local_account("Ada", "ada@example.org");
        mailbox.add_event(timed(0, at(2039, 3, 27, 1, 0), 1)).unwrap();
        mailbox.rebase(london);
        let before = mailbox.clone();
        assert_eq!(mailbox.rebase(london), 0);
        assert_eq!(mailbox, before);
    }

    proptest::proptest! {
        #[test]
        fn rebasing_there_and_back_moves_no_instant(
            a in 0usize..140, b in 0usize..140,
            minutes in proptest::collection::vec((0i64..60 * 24 * 365 * 40, 1i64..3 * 24 * 60, proptest::bool::ANY), 1..12),
        ) {
            let choices = Zone::choices();
            let (first, second) = (choices[a % choices.len()], choices[b % choices.len()]);
            let mut mailbox = Mailbox::with_local_account("Ada", "ada@example.org");
            for (start, length, repeats) in minutes {
                let start = at(2000, 1, 1, 0, 0).add_minutes(start);
                let recurrence = repeats.then(|| Recurrence::every(Freq::Daily));
                mailbox.add_event(Event { recurrence, ..timed(0, start, length) }).unwrap();
            }
            mailbox.rebase(first);
            // (A time the clocks of the first zone skip is no time there: as the zone reads it.)
            let instants = |mailbox: &Mailbox, zone: Zone| -> Vec<(Timestamp, Timestamp)> {
                mailbox.events.values().map(|e| e.instants(&|instant| zone.offset_at(instant))).collect()
            };
            let before = instants(&mailbox, first);
            let starts: Vec<Timestamp> = mailbox.events.values().map(|e| first.instant(e.start)).collect();
            proptest::prop_assert_eq!(before.iter().map(|(start, _)| *start).collect::<Vec<_>>(), starts);
            mailbox.rebase(second);
            // Every instant is the same instant, also in an hour the second zone's clocks show
            // twice; and it reads as that zone's clock shows it.
            proptest::prop_assert_eq!(instants(&mailbox, second), before.clone());
            // (Into the zone it is already in nothing moves, so a time its clocks skip stays as written.)
            if first != second {
                let shown: Vec<DateTime> = before.iter().map(|(start, _)| second.wall(*start)).collect();
                proptest::prop_assert_eq!(mailbox.events.values().map(|e| e.start).collect::<Vec<_>>(), shown);
            }
            let pinned = mailbox.clone();
            for event in mailbox.events.values().filter(|event| event.recurrence.is_some()) {
                // The series still starts on the same day of its own.
                let here = |instant: Timestamp| second.offset_at(instant);
                proptest::prop_assert_eq!(event.series_days(event.start.date.add_days(-2), event.start.date.add_days(2), &here).first().copied(), Some(event.start.date));
            }
            // And back, and there again: the same mailbox as the first time there.
            mailbox.rebase(first);
            proptest::prop_assert_eq!(instants(&mailbox, first), before);
            mailbox.rebase(second);
            proptest::prop_assert_eq!(mailbox, pinned);
        }
    }
}
