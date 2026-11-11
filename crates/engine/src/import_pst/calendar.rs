// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Appointments, meetings and tasks of a .pst file ([MS-OXOCAL], [MS-OXOTASK]) as events and
//! to-dos of the mailbox (#815). Nothing is ever sent for them: attendees and their answers are
//! kept as data. What the file holds is hostile: lengths are capped and a series that can't be
//! read comes in as its first occurrence, named.

use std::collections::HashMap;
use std::io::{Read, Seek};

use mailcraft_model::{Address, Attendee, Event, Freq, Importance, Recurrence, Response, ShowAs, Task};
use mailcraft_pst::{Error as PstError, NamedMap, PropName, PropValue, Pst, Recur, recur};
use mailcraft_time::{Date, DateTime, Timestamp, Weekday, Zone};

use super::contact::value;
use super::item::{self, Named, PSETID_TASK, Text, filetime};

pub const PSETID_APPOINTMENT: [u8; 16] = [0x02, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46];
pub const PSETID_COMMON: [u8; 16] = [0x08, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46];
pub const PSETID_MEETING: [u8; 16] = [0x90, 0xDA, 0xD8, 0x6E, 0x0B, 0x45, 0x1B, 0x10, 0x98, 0xDA, 0x00, 0xAA, 0x00, 0x3F, 0x13, 0x05];

const BUSY_STATUS: u32 = 0x8205;
const LOCATION: u32 = 0x8208;
const START_WHOLE: u32 = 0x820D;
const END_WHOLE: u32 = 0x820E;
const ALL_DAY: u32 = 0x8215;
const RECUR: u32 = 0x8216;
const STATE_FLAGS: u32 = 0x8217;
const RECURRING: u32 = 0x8223;
const ZONE_START: u32 = 0x825E;
const ZONE_END: u32 = 0x825F;
const ZONE_RECUR: u32 = 0x8260;
const REMINDER_DELTA: u32 = 0x8501;
const REMINDER_SET: u32 = 0x8503;
const COMMON_START: u32 = 0x8516;
const COMMON_END: u32 = 0x8517;
const GLOBAL_ID: u32 = 0x0003;
const TASK_START: u32 = 0x8104;
const TASK_DUE: u32 = 0x8105;
const TASK_DONE_ON: u32 = 0x810F;
const TASK_RECURS: u32 = 0x8126;
const TASK_COMPLETE: u32 = 0x811C;
const REMINDER_TIME: u32 = 0x8502;

const WANTED: [([u8; 16], u32); 19] = [
    (PSETID_APPOINTMENT, BUSY_STATUS),
    (PSETID_APPOINTMENT, LOCATION),
    (PSETID_APPOINTMENT, START_WHOLE),
    (PSETID_APPOINTMENT, END_WHOLE),
    (PSETID_APPOINTMENT, ALL_DAY),
    (PSETID_APPOINTMENT, RECUR),
    (PSETID_APPOINTMENT, STATE_FLAGS),
    (PSETID_APPOINTMENT, RECURRING),
    (PSETID_APPOINTMENT, ZONE_START),
    (PSETID_APPOINTMENT, ZONE_END),
    (PSETID_APPOINTMENT, ZONE_RECUR),
    (PSETID_COMMON, REMINDER_DELTA),
    (PSETID_COMMON, REMINDER_SET),
    (PSETID_COMMON, COMMON_START),
    (PSETID_COMMON, COMMON_END),
    (PSETID_COMMON, REMINDER_TIME),
    (PSETID_MEETING, GLOBAL_ID),
    (PSETID_TASK, TASK_START),
    (PSETID_TASK, TASK_DUE),
];
const WANTED_TASK: [u32; 3] = [TASK_DONE_ON, TASK_RECURS, TASK_COMPLETE];

/// Most bytes of a recurrence or a time zone read from the file.
const MAX_BLOB: u64 = 1 << 20;
/// Most characters of an event's notes or a to-do's.
const MAX_NOTES: usize = 1 << 20;
/// Most characters of a title or a place.
const MAX_SHORT: usize = 4096;
/// Most people of one meeting.
const MAX_ATTENDEES: usize = 1000;
/// Minutes between 1601 and 1970.
const MINUTES_TO_1970: i64 = 194_074_560;
/// Longest an occurrence of a series may last, in minutes (five years).
const MAX_LENGTH: u32 = 5 * 366 * 1440;
/// Most days of a series named as not happening.
const MAX_DAYS: usize = 5_000;
/// A reminder no further ahead than this (four weeks), as an invitation's is.
const MAX_REMINDER: u32 = 40_320;

/// The clock of the calendar, for the reader's thread.
#[derive(Clone, Copy, Debug)]
pub struct Here {
    pub offset: Option<i32>,
    pub zone: Zone,
}

impl Here {
    pub fn at(&self, ts: Timestamp) -> i32 {
        self.offset.unwrap_or_else(|| self.zone.offset_at(ts))
    }
}

/// The ids this file gives the named properties of calendar items and tasks.
#[derive(Debug, Default, Clone)]
pub struct CalendarIds(HashMap<u32, u16>);

impl CalendarIds {
    pub fn find(map: &NamedMap) -> CalendarIds {
        let task = WANTED_TASK.iter().map(|lid| (PSETID_TASK, *lid));
        let ids = WANTED.iter().copied().chain(task).filter_map(|(set, lid)| map.find(&set, &PropName::Id(lid)).map(|id| (lid, id))).collect();
        CalendarIds(ids)
    }

    fn id(&self, lid: u32) -> Option<u16> {
        self.0.get(&lid).copied()
    }
}

/// What an item is, when it is one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Appointment,
    Task,
}

pub fn kind(class: &str) -> Option<Kind> {
    let c = class.to_ascii_lowercase();
    let family = |prefix: &str| c == prefix || c.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.'));
    if family("ipm.appointment") {
        Some(Kind::Appointment)
    } else if family("ipm.task") {
        Some(Kind::Task)
    } else {
        None
    }
}

/// One appointment: the event, the occurrences of a series that were changed, and what of it
/// couldn't be brought.
pub struct BuiltEvent {
    pub event: Event,
    pub changed: Vec<Event>,
    pub notes: Vec<String>,
}

fn short(s: String) -> String {
    s.trim_end_matches('\0').chars().take(MAX_SHORT).collect()
}

/// A wall-clock time from minutes since 1601 by a clock.
fn wall(minutes: u32) -> DateTime {
    DateTime::from_timestamp((i64::from(minutes).saturating_sub(MINUTES_TO_1970)).saturating_mul(60), 0)
}

fn weekdays(mask: u32) -> Vec<Weekday> {
    let days = [Weekday::Sun, Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri, Weekday::Sat];
    days.into_iter().enumerate().filter(|(bit, _)| mask >> bit & 1 == 1).map(|(_, d)| d).collect()
}

/// The first day a series happens: PatternStartDate is only a lower bound, and a month's pattern
/// names its own day ([MS-OXOCAL] 2.2.1.44.1), which the mailbox reads from the first occurrence.
fn first_day(r: &Recur, start_date: Date) -> Date {
    if r.pattern == recur::PATTERN_MONTH && matches!(r.day_or_nth, 1..=31) {
        let wanted = u8::try_from(r.day_or_nth).unwrap_or(1);
        let mut month = start_date.start_of_month();
        // (A month must have the day and be on or after the start: at most a year or so on.)
        for _ in 0..14 {
            if month.end_of_month().day >= wanted {
                let on = Date { day: wanted, ..month };
                if on >= start_date {
                    return on;
                }
            }
            month = month.add_months(1);
        }
    }
    start_date
}

/// The rule of a series whose first day is `first`, or why the mailbox can't keep it.
pub fn rule(r: &Recur, first: Date) -> Result<Recurrence, &'static str> {
    if r.calendar > 1 || r.pattern >= 0x0A {
        return Err("it repeats by a calendar other than the Gregorian");
    }
    // (An occurrence starts within its day and lasts at most a few years.)
    if r.start_offset >= 1440
        || r.end_offset < r.start_offset
        || r.end_offset - r.start_offset > MAX_LENGTH
        || i64::from(r.start_date) < MINUTES_TO_1970
    {
        return Err("its times can't be read");
    }
    let every = |n: u32| if n == 0 || n > 1000 { Err("its interval can't be read") } else { Ok(n) };
    let months = |freq: Freq| {
        let per = if freq == Freq::Yearly { 12 } else { 1 };
        if !r.period.is_multiple_of(per) { Err("its interval can't be read") } else { every(r.period / per) }
    };
    let freq = match r.frequency {
        recur::FREQ_DAILY => Freq::Daily,
        recur::FREQ_WEEKLY => Freq::Weekly,
        recur::FREQ_MONTHLY => Freq::Monthly,
        recur::FREQ_YEARLY => Freq::Yearly,
        _ => return Err("its frequency isn't known"),
    };
    let mut out = match (r.pattern, freq) {
        (recur::PATTERN_DAY, _) => Recurrence { freq: Freq::Daily, interval: every(r.period / 1440)?, ..Default::default() },
        (recur::PATTERN_WEEK, _) if r.weekdays & 0x7F != 0 => {
            Recurrence { freq: Freq::Weekly, interval: every(r.period)?, by_day: weekdays(r.weekdays), ..Default::default() }
        }
        (recur::PATTERN_MONTH, Freq::Monthly | Freq::Yearly) => {
            // (A month without that day falls on its last, as Outlook has it.)
            Recurrence { freq, interval: months(freq)?, ..Default::default() }
        }
        (recur::PATTERN_MONTH_END, Freq::Monthly | Freq::Yearly) if first.day == first.end_of_month().day && first.day == 31 => {
            Recurrence { freq, interval: months(freq)?, ..Default::default() }
        }
        (recur::PATTERN_MONTH_NTH, Freq::Monthly | Freq::Yearly) => {
            let days = weekdays(r.weekdays);
            let nth = match r.day_or_nth {
                n @ 1..=4 => i8::try_from(n).unwrap_or(1),
                5 => -1,
                _ => return Err("which of the month's days it repeats on can't be read"),
            };
            match days.len() {
                1 => Recurrence { freq, interval: months(freq)?, by_day: days, nth: Some(nth), ..Default::default() },
                // Every day of the week, counted: the nth day of the month.
                7 if nth > 0 => Recurrence { freq, interval: months(freq)?, ..Default::default() },
                _ => return Err("it repeats on a counted day of the month that this calendar can't keep"),
            }
        }
        _ => return Err("it repeats in a way that this calendar can't keep"),
    };
    if matches!(r.end_type, recur::END_DATE | recur::END_COUNT) {
        // The last day, whichever way the end was asked for: a count may or may not include
        // days deleted since.
        out.until = Some(wall(r.end_date).date);
    }
    Ok(out)
}

fn zone_of<R: Read + Seek>(pst: &mut Pst<R>, m: &mailcraft_pst::Message, id: Option<u16>, notes: &mut Vec<String>) -> Result<Option<Zone>, PstError> {
    let Some(id) = id else { return Ok(None) };
    let Some(PropValue::Binary(b)) = value(pst, m, id, MAX_BLOB, "time zone", notes)? else { return Ok(None) };
    Ok(mailcraft_pst::time_zone_key(&b).ok().and_then(|key| Zone::named(&key)))
}

fn show_as(status: Option<i64>) -> ShowAs {
    match status {
        Some(0) => ShowAs::Free,
        Some(1) => ShowAs::Tentative,
        Some(3) => ShowAs::OutOfOffice,
        Some(4) => ShowAs::WorkingElsewhere,
        _ => ShowAs::Busy,
    }
}

fn importance(p: &mailcraft_pst::Properties) -> Importance {
    match p.int(0x0017) {
        Some(0) => Importance::Low,
        Some(2) => Importance::High,
        _ => Importance::Normal,
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().take(256).map(|x| format!("{x:02X}")).collect()
}

/// The people of a meeting, and its organizer when a row of them is.
fn people(m: &mailcraft_pst::Message, text: &Text, notes: &mut Vec<String>) -> (Vec<Attendee>, Option<Address>) {
    let (mut list, mut organizer) = (Vec::new(), None);
    for r in &m.recipients {
        let p = &r.properties;
        let Some(address) = item::person(text, p, [0x3001, 0x39FE, 0x3003]) else { continue };
        // recipOrganizer ([MS-OXOCAL] 2.2.4.10.1).
        if p.int(0x5FFD).is_some_and(|f| f & 2 != 0) {
            organizer = Some(address);
            continue;
        }
        if list.len() >= MAX_ATTENDEES {
            notes.push(format!("it has more people than the {MAX_ATTENDEES} a meeting keeps"));
            break;
        }
        let response = match p.int(0x5FFF) {
            Some(2) => Response::Tentative,
            Some(3) => Response::Accepted,
            Some(4) => Response::Declined,
            _ => Response::None,
        };
        // (PidTagRecipientType: 1 required, 2 optional, 3 a resource.)
        let optional = p.int(0x0C15) == Some(2);
        list.push(Attendee { address, optional, response });
    }
    (list, organizer)
}

/// The appointment `m` as an event; `None` when it has no time to be at.
pub fn build<R: Read + Seek>(pst: &mut Pst<R>, m: &mailcraft_pst::Message, named: &Named, here: Here) -> Result<Option<BuiltEvent>, PstError> {
    let p = &m.properties;
    let ids = &named.calendar;
    let text = Text::of(p, named.charset);
    let offset_at = |t: Timestamp| here.at(t);
    let time = |lid: u32| ids.id(lid).and_then(|id| p.time(id)).and_then(filetime);
    let named_text = |lid: u32| ids.id(lid).and_then(|id| text.short(p, id));
    let Some(start) = time(START_WHOLE).or_else(|| time(COMMON_START)) else { return Ok(None) };
    let end = time(END_WHOLE).or_else(|| time(COMMON_END)).unwrap_or(start).max(start);
    let (description, mut notes) = item::plain_body(pst, m, named, MAX_NOTES)?;
    let all_day = ids.id(ALL_DAY).and_then(|id| p.bool(id)).unwrap_or(false);
    let state = ids.id(STATE_FLAGS).and_then(|id| p.int(id)).unwrap_or(0);
    let (attendees, mut organizer) = people(m, &text, &mut notes);
    if !attendees.is_empty() && organizer.is_none() {
        organizer = item::person(&text, p, [0x0042, 0x5D02, 0x0065]);
    }
    let reminder = ids.id(REMINDER_SET).and_then(|id| p.bool(id)).unwrap_or(false);
    let delta = ids.id(REMINDER_DELTA).and_then(|id| p.int(id)).and_then(|d| u32::try_from(d).ok()).map(|d| d.min(MAX_REMINDER));
    let uid = ids.id(GLOBAL_ID).and_then(|id| p.binary(id)).map(hex).unwrap_or_default();
    let files = m.attachments.iter().filter(|a| !a.properties.bool(0x7FFE).unwrap_or(false)).count();
    if files > 0 {
        notes.push(format!("{files} attached files are left out: events here don't keep files"));
    }
    let mut e = Event {
        uid,
        title: text.short(p, 0x0037).map(short).unwrap_or_default(),
        location: named_text(LOCATION).map(short).unwrap_or_default(),
        description,
        all_day,
        organizer,
        attendees,
        reminder_minutes: reminder.then_some(delta).flatten(),
        show_as: show_as(ids.id(BUSY_STATUS).and_then(|id| p.int(id))),
        categories: item::own_categories(p, named, &text),
        private: p.int(0x0036) == Some(2),
        cancelled: state & 4 != 0,
        importance: importance(p),
        ..Default::default()
    };

    let recurring = ids.id(RECURRING).and_then(|id| p.bool(id)).unwrap_or(false);
    let blob = match ids.id(RECUR).filter(|_| recurring) {
        Some(id) => value(pst, m, id, MAX_BLOB, "recurrence", &mut notes)?,
        None => None,
    };
    let series = match &blob {
        Some(PropValue::Binary(b)) => match mailcraft_pst::appointment_recurrence(b) {
            Ok(r) => match rule(&r, first_day(&r, wall(r.start_date).date)) {
                Ok(rule) => Some((r, rule)),
                Err(why) => {
                    notes.push(format!("{why}: only its first occurrence is brought in"));
                    None
                }
            },
            Err(err) => {
                notes.push(format!("its recurrence could not be read ({err}): only its first occurrence is brought in"));
                None
            }
        },
        _ => {
            if recurring {
                notes.push("its recurrence is missing: only its first occurrence is brought in".to_string());
            }
            None
        }
    };

    let Some((r, rule)) = series else {
        let (zs, ze) = (zone_of(pst, m, ids.id(ZONE_START), &mut notes)?, zone_of(pst, m, ids.id(ZONE_END), &mut notes)?);
        place_once(&mut e, start, end, zs, ze, here);
        return Ok(Some(BuiltEvent { event: e, changed: Vec::new(), notes }));
    };

    let zone = zone_of(pst, m, ids.id(ZONE_RECUR), &mut notes)?;
    // (Weekly and counted patterns: the first day the rule itself names.)
    let from = first_day(&r, wall(r.start_date).date);
    let first =
        if r.pattern == recur::PATTERN_MONTH { from } else { rule.dates(from, from, from.add_days(800), &[]).first().copied().unwrap_or(from) };
    let clock = DateTime::at_midnight(first).add_minutes(i64::from(r.start_offset));
    let length = i64::from(r.end_offset.saturating_sub(r.start_offset));
    let instant = |w: DateTime| match zone {
        Some(z) => z.instant(w),
        None => mailcraft_model::zoned::instant_at(w, &offset_at),
    };
    e.recurrence = Some(rule);
    if all_day {
        e.start = DateTime::at_midnight(first);
        e.end = e.start.add_minutes(length.max(1440));
    } else {
        let begins = instant(clock);
        let ends = begins.saturating_add(length.saturating_mul(60));
        e.set_instants(begins, ends, &offset_at);
        (e.start_zone, e.end_zone) = mailcraft_model::zoned::own_zones(zone, None, (begins, ends), true, &offset_at);
    }
    let day_here = |e: &Event, minutes: u32| {
        let day = wall(minutes).date;
        if e.all_day { day } else { e.occurrence_day(instant(DateTime { date: day, ..clock }), &offset_at) }
    };
    let mut days: Vec<Date> = r.deleted.iter().map(|m| day_here(&e, *m)).collect();
    days.sort();
    days.dedup();
    if days.len() > MAX_DAYS {
        notes.push(format!("it has more deleted days than the {MAX_DAYS} a series keeps: the latest happen again here"));
        days.truncate(MAX_DAYS);
    }
    e.exceptions = days;

    let mut changed = Vec::new();
    let mut past_budget = 0usize;
    for x in &r.exceptions {
        let mut one = Event { recurrence: None, exceptions: Vec::new(), start_zone: String::new(), end_zone: String::new(), ..e.clone() };
        let begins = wall(x.start);
        one.uid = if e.uid.is_empty() { String::new() } else { format!("{}-{}", e.uid, wall(x.original).date.iso()) };
        if let Some(t) = x.subject.clone().or_else(|| x.subject_8bit.as_deref().map(|b| text.decode(b))) {
            one.title = short(t);
        }
        if let Some(t) = x.location.clone().or_else(|| x.location_8bit.as_deref().map(|b| text.decode(b))) {
            one.location = short(t);
        }
        if let Some(b) = x.busy {
            one.show_as = show_as(Some(i64::from(b)));
        }
        if let Some(on) = x.reminder_set {
            one.reminder_minutes = if on { x.reminder_delta.or(delta).map(|d| d.min(MAX_REMINDER)) } else { None };
        } else if let (Some(d), true) = (x.reminder_delta, one.reminder_minutes.is_some()) {
            one.reminder_minutes = Some(d.min(MAX_REMINDER));
        }
        one.all_day = x.all_day.unwrap_or(e.all_day);
        if one.all_day {
            one.start = DateTime::at_midnight(begins.date);
            one.end = DateTime::at_midnight(wall(x.end).date).max(one.start.add_days(1));
        } else {
            let at = instant(begins);
            let until = instant(wall(x.end)).max(at);
            one.set_instants(at, until, &offset_at);
            (one.start_zone, one.end_zone) = mailcraft_model::zoned::own_zones(zone, None, (at, until), false, &offset_at);
        }
        // Each is a copy of the series: a few bytes of the file can ask for many.
        let (cost, room) = (held(&one), named.rtf_room.get());
        if cost > room {
            past_budget = past_budget.saturating_add(1);
            continue;
        }
        named.rtf_room.set(room.saturating_sub(cost));
        changed.push(one);
    }
    if past_budget > 0 {
        notes.push(format!("{past_budget} changed occurrences are left out: this import has made as many as one import may"));
    }
    if !changed.is_empty() {
        notes.push(format!("{} changed occurrences come in as events of their own, with the notes and people of the series", changed.len()));
    }
    Ok(Some(BuiltEvent { event: e, changed, notes }))
}

/// An appointment that happens once.
fn place_once(e: &mut Event, start: Timestamp, end: Timestamp, zs: Option<Zone>, ze: Option<Zone>, here: Here) {
    let offset_at = |t: Timestamp| here.at(t);
    if e.all_day {
        // Whole days are kept as the days they were in the zone they were made in.
        let day = |t: Timestamp, z: Option<Zone>| z.map_or_else(|| mailcraft_model::zoned::wall_at(t, &offset_at).date, |z| z.wall(t).date);
        e.start = DateTime::at_midnight(day(start, zs));
        e.end = DateTime::at_midnight(day(end, ze.or(zs))).max(e.start.add_days(1));
    } else {
        e.set_instants(start, end, &offset_at);
        (e.start_zone, e.end_zone) = mailcraft_model::zoned::own_zones(zs, ze, (start, end), false, &offset_at);
    }
}

/// About how much memory an event holds, for the import's batches.
pub fn held(e: &Event) -> u64 {
    let people =
        e.attendees.iter().map(|a| a.address.name.len().saturating_add(a.address.email.len()).saturating_add(64)).fold(0usize, usize::saturating_add);
    let days = e.exceptions.len().saturating_mul(16);
    (e.title
        .len()
        .saturating_add(e.location.len())
        .saturating_add(e.description.len())
        .saturating_add(people)
        .saturating_add(days)
        .saturating_add(512)) as u64
}

/// A task of the file as a to-do on this computer.
pub fn build_task<R: Read + Seek>(pst: &mut Pst<R>, m: &mailcraft_pst::Message, named: &Named) -> Result<(Task, Vec<String>), PstError> {
    let p = &m.properties;
    let ids = &named.calendar;
    let text = Text::of(p, named.charset);
    let time = |lid: u32| ids.id(lid).and_then(|id| p.time(id)).and_then(filetime);
    // A task's dates are days, written as midnight UTC ([MS-OXOTASK] 2.2.2.2.4, 2.2.2.2.5).
    let day = |lid: u32| time(lid).map(|t| DateTime::from_timestamp(t, 0).date);
    let (notes_text, mut notes) = item::plain_body(pst, m, named, MAX_NOTES)?;
    let done = ids.id(TASK_COMPLETE).and_then(|id| p.bool(id)).unwrap_or(false);
    let completed = done.then(|| time(TASK_DONE_ON).or_else(|| p.time(0x3008).and_then(filetime)).unwrap_or(0));
    let reminder = ids.id(REMINDER_SET).and_then(|id| p.bool(id)).unwrap_or(false);
    if ids.id(TASK_RECURS).and_then(|id| p.bool(id)).unwrap_or(false) {
        notes.push("it repeats, which to-dos here don't: only this occurrence is brought in".to_string());
    }
    let files = m.attachments.iter().filter(|a| !a.properties.bool(0x7FFE).unwrap_or(false)).count();
    if files > 0 {
        notes.push(format!("{files} attached files are left out: to-dos here don't keep files"));
    }
    let task = Task {
        title: text.short(p, 0x0037).map(short).unwrap_or_default(),
        notes: notes_text,
        start: day(TASK_START),
        due: day(TASK_DUE),
        completed,
        importance: importance(p),
        categories: item::own_categories(p, named, &text),
        reminder: reminder.then(|| time(REMINDER_TIME)).flatten(),
        // Kept on this computer: an answer the owner gave once about the to-dos they had then
        // doesn't send an archive's to a server.
        local_only: true,
        kept_here: true,
        ..Default::default()
    };
    Ok((task, notes))
}
