// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading iCalendar text into events and to-dos.

use std::collections::{HashMap, HashSet};

use mailcraft_model::zoned::{instant_at, own_zones, wall_at};
use mailcraft_model::{Address, Attendee, Event, Importance, OnlineMeeting, OnlineProvider, Response, ResponseOptions, ShowAs, Task};
use mailcraft_time::{Date, DateTime, Timestamp, Zone};

use crate::IcsFile;
use crate::lines::{ContentLine, Lines, prefix};
use crate::rrule;
use crate::values::{When, parse_duration, split_list, unescape_text};
use crate::zones::{FileZones, MAX_NOTES, Source};

/// Most events and to-dos read from one text.
const MAX_COMPONENTS: usize = 20_000;
const MAX_ATTENDEES: usize = 500;
const MAX_EXDATES: usize = 5_000;
const MAX_CATEGORIES: usize = 500;
/// Most alarms looked at per event or to-do.
const MAX_ALARMS: usize = 16;
/// Deepest nesting of components followed.
const MAX_DEPTH: usize = 64;
/// Longest keyword (`VEVENT`, `CANCELLED`, a `RECURRENCE-ID`) kept, in bytes.
const MAX_KEYWORD: usize = 64;

type OffsetAt<'a> = &'a dyn Fn(Timestamp) -> i32;

/// Parse iCalendar text. Never fails: junk gives an empty file, a broken component is skipped.
///
/// `offset_at(ts)` is the offset (minutes east of UTC) at an instant of the zone the events are
/// to be shown in: every time that is an instant becomes the wall-clock time there. A `…Z` time
/// is an instant, and so is a time with a `TZID` that says when it is: by the `VTIMEZONE` the
/// text brings for it, else by the zone of that name (the IANA database, Windows' names), else
/// by the offset the name mentions. A time in a zone of which nothing is known, and a time
/// without any zone, is taken as a wall-clock time as it is written; of the first,
/// [`IcsFile::notes`] says so. An event whose zone keeps another clock than `offset_at` carries
/// it ([`Event::start_zone`]), and a series then repeats by that zone's clock. Ids are left 0
/// and `calendar` 0.
///
/// A `VEVENT` with a `RECURRENCE-ID` (a changed instance of a series) comes back as its own
/// single event with the uid `<uid>#<recurrence-id>`, and its day joins the `exceptions` of the
/// series when that is in the same text. A component that lacks its `END` is still read; an event
/// without a `DTSTART` is skipped.
///
/// Work is bounded: at most 20 000 events and to-dos are read, 500 attendees and 5 000 excluded
/// days per event, 64 time zones, and what a line holds beyond 1 MiB is dropped.
pub fn parse(text: &str, offset_at: &dyn Fn(Timestamp) -> i32) -> IcsFile {
    parse_events(text, offset_at, MAX_COMPONENTS)
}

/// As [`parse`], reading no more than `max_events` events (an invitation's card is about the
/// first one). To-dos still count towards the usual limit.
pub fn parse_events(text: &str, offset_at: &dyn Fn(Timestamp) -> i32, max_events: usize) -> IcsFile {
    let zones = FileZones::collect(text);
    let mut reader =
        Reader { max_events, offset_at, zones: &zones, file: IcsFile::default(), stack: Vec::new(), item: None, alarm: None, changed: Vec::new() };
    for raw in Lines::new(text) {
        if reader.full() {
            break;
        }
        if let Some(line) = ContentLine::parse(&raw) {
            reader.line(&line);
        }
    }
    reader.finish()
}

/// When an alarm goes off.
#[derive(Clone, Copy)]
enum Trigger {
    /// Seconds after the start (negative: before), or after the end.
    Relative {
        seconds: i64,
        from_end: bool,
    },
    Absolute(When),
}

impl Trigger {
    fn parse(line: &ContentLine) -> Option<Trigger> {
        if let Some(seconds) = parse_duration(&line.value) {
            let from_end = line.param("RELATED").is_some_and(|r| r.eq_ignore_ascii_case("END"));
            return Some(Trigger::Relative { seconds, from_end });
        }
        When::parse(&line.value).filter(|at| !matches!(at, When::Date(_))).map(Trigger::Absolute)
    }
}

/// A `VALARM` being read.
#[derive(Clone, Copy)]
struct Alarm {
    /// How deep in the reader's stack it sits.
    depth: usize,
    trigger: Option<Trigger>,
    /// `ACTION:NONE`: the placeholder some programs write to say "no alarm".
    silent: bool,
}

enum Item {
    Event(Box<EventBuilder>),
    Todo(Box<TodoBuilder>),
}

struct Reader<'a> {
    /// Most events to read.
    max_events: usize,
    offset_at: OffsetAt<'a>,
    /// The zones the text defines.
    zones: &'a FileZones,
    file: IcsFile,
    /// The names of the open components, outermost first.
    stack: Vec<String>,
    /// The open `VEVENT` or `VTODO`, and how deep in `stack` it sits.
    item: Option<(usize, Item)>,
    /// The open `VALARM` of `item`.
    alarm: Option<Alarm>,
    /// Changed instances: their index in `file.events`, the uid of their series, the time of the
    /// occurrence they replace.
    changed: Vec<(usize, String, Stamp)>,
}

impl Reader<'_> {
    fn full(&self) -> bool {
        self.file.events.len() >= self.max_events || self.file.events.len().saturating_add(self.file.tasks.len()) >= MAX_COMPONENTS
    }

    fn line(&mut self, line: &ContentLine) {
        match line.name.as_str() {
            "BEGIN" => self.begin(keyword(&line.value)),
            "END" => self.end(&keyword(&line.value)),
            _ => self.property(line),
        }
    }

    fn begin(&mut self, name: String) {
        match name.as_str() {
            // An event can't hold another: one that never ended ends here.
            "VEVENT" | "VTODO" => {
                self.close_item();
                let item = if name == "VEVENT" { Item::Event(Box::default()) } else { Item::Todo(Box::default()) };
                self.item = Some((self.stack.len(), item));
            }
            "VCALENDAR" => {
                self.close_item();
                self.stack.clear();
            }
            "VALARM" => {
                // Nor can an alarm hold another.
                if let Some(open) = self.alarm {
                    self.close_alarm();
                    self.stack.truncate(open.depth);
                }
                let depth = self.stack.len();
                if self.item.as_ref().is_some_and(|(at, _)| depth == at.saturating_add(1)) {
                    self.alarm = Some(Alarm { depth, trigger: None, silent: false });
                }
            }
            _ => {}
        }
        if self.stack.len() < MAX_DEPTH {
            self.stack.push(name);
        }
    }

    fn end(&mut self, name: &str) {
        // An END closes the innermost component of its name and all that is still open inside it.
        let Some(depth) = self.stack.iter().rposition(|open| open == name) else { return };
        if self.alarm.is_some_and(|alarm| alarm.depth >= depth) {
            self.close_alarm();
        }
        if self.item.as_ref().is_some_and(|(at, _)| *at >= depth) {
            self.close_item();
        }
        self.stack.truncate(depth);
    }

    fn property(&mut self, line: &ContentLine) {
        let depth = self.stack.len();
        if let Some(alarm) = &mut self.alarm
            && depth == alarm.depth.saturating_add(1)
        {
            match line.name.as_str() {
                "TRIGGER" if alarm.trigger.is_none() => alarm.trigger = Trigger::parse(line),
                "ACTION" => alarm.silent = keyword(&line.value) == "NONE",
                _ => {}
            }
            return;
        }
        if let Some((at, item)) = &mut self.item {
            // Deeper lines belong to a component this reader doesn't know.
            if depth == at.saturating_add(1) {
                match item {
                    Item::Event(event) => event.apply(line, self.zones),
                    Item::Todo(todo) => todo.apply(line, self.zones),
                }
            }
            return;
        }
        if line.name == "METHOD" && self.file.method.is_empty() && self.stack.last().is_none_or(|top| top == "VCALENDAR") {
            self.file.method = keyword(&line.value);
        }
    }

    fn close_alarm(&mut self) {
        let Some(Alarm { trigger: Some(trigger), silent: false, .. }) = self.alarm.take() else { return };
        let alarms = match &mut self.item {
            Some((_, Item::Event(event))) => &mut event.alarms,
            Some((_, Item::Todo(todo))) => &mut todo.alarms,
            None => return,
        };
        if alarms.len() < MAX_ALARMS {
            alarms.push(trigger);
        }
    }

    fn close_item(&mut self) {
        self.close_alarm();
        let Some((depth, item)) = self.item.take() else { return };
        self.stack.truncate(depth);
        if self.full() {
            return;
        }
        match item {
            Item::Event(event) => {
                let mut notes = Vec::new();
                if let Some((event, changed)) = event.finish(self.offset_at, &mut notes) {
                    if let Some((series, at)) = changed {
                        self.changed.push((self.file.events.len(), series, at));
                    }
                    self.file.events.push(event);
                }
                for note in notes {
                    if self.file.notes.len() < MAX_NOTES && !self.file.notes.contains(&note) {
                        self.file.notes.push(note);
                    }
                }
            }
            Item::Todo(todo) => self.file.tasks.extend(todo.finish(self.offset_at)),
        }
    }

    fn finish(mut self) -> IcsFile {
        self.close_item();
        if let Some(said) = self.zones.stopped_note() {
            self.file.notes.push(said);
        }
        if self.changed.is_empty() {
            return self.file;
        }
        // A changed instance takes its day out of the series it belongs to.
        let instances: HashSet<usize> = self.changed.iter().map(|(index, _, _)| *index).collect();
        let mut series: HashMap<&str, usize> = HashMap::new();
        for (index, event) in self.file.events.iter().enumerate() {
            if event.recurrence.is_some() && !instances.contains(&index) {
                series.entry(event.uid.as_str()).or_insert(index);
            }
        }
        let days: Vec<(usize, Date)> = self
            .changed
            .iter()
            .filter_map(|(_, uid, at)| {
                let index = *series.get(uid.as_str())?;
                Some((index, series_day(self.file.events.get(index)?, at, self.offset_at)))
            })
            .collect();
        for (index, day) in days {
            if let Some(event) = self.file.events.get_mut(index)
                && event.exceptions.len() < MAX_EXDATES
                && !event.exceptions.contains(&day)
            {
                event.exceptions.push(day);
            }
        }
        self.file
    }
}

/// A component name or an enumerated value: trimmed, upper-cased, bounded.
fn keyword(value: &str) -> String {
    prefix(value.trim(), MAX_KEYWORD).to_ascii_uppercase()
}

/// A date or a time as a property gives it, with what its zone says.
#[derive(Clone, Copy)]
struct Stamp {
    /// When it is. A time in a zone that says when it is, is that instant (as a UTC time); a
    /// time without a zone, or in a zone of which nothing is known, is the time as written.
    when: When,
    /// The time as it is written, whatever the zone.
    written: DateTime,
    /// The zone it is written in, when that is one an event can keep as its own.
    zone: Option<Zone>,
}

impl Stamp {
    fn plain(when: When) -> Stamp {
        Stamp { when, written: when.wall(), zone: None }
    }
}

/// One value of a property (`value`: all of it, or one of a list) with what the line's `TZID`
/// makes of it. What there is to say about the zone goes to `notes`.
fn stamp_of(line: &ContentLine, value: &str, zones: &FileZones, notes: &mut Vec<String>) -> Option<Stamp> {
    let when = When::parse(value)?;
    let (When::Local(written), Some(tzid)) = (when, line.param("TZID")) else {
        // (A time that ends in `Z` is UTC, whatever a parameter says.)
        let zone = matches!(when, When::Utc(_)).then_some(Zone::UTC);
        return Some(Stamp { zone, ..Stamp::plain(when) });
    };
    let source = zones.source(tzid);
    if let Source::File(definition, _) = &source
        && !zones.spend(definition.cost().saturating_mul(2))
    {
        // The file has had its share of work: the time is kept as it is written.
        return Some(Stamp::plain(when));
    }
    let instant = source.instant(written);
    let (zone, said) = source.zone_in(Some(zones), tzid, written, instant.unwrap_or(0));
    if let Some(said) = said
        && notes.len() < MAX_NOTES
        && !notes.contains(&said)
    {
        notes.push(said);
    }
    let when = match instant {
        Some(instant) => When::Utc(DateTime::from_timestamp(instant, 0)),
        None => when,
    };
    Some(Stamp { when, written, zone })
}

/// The day of `series` that the occurrence at `at` is, as the series names its days: for a
/// series that repeats in a zone of its own, by that zone.
fn series_day(series: &Event, at: &Stamp, offset_at: OffsetAt) -> Date {
    match (series.zone(), at.when) {
        (Some(_), When::Utc(_)) if !series.all_day => series.occurrence_day(at.when.timestamp(offset_at), offset_at),
        // Written as a day, or as a time without a zone: in the reckoning of the series itself.
        (Some(zone), When::Local(written)) if !series.all_day => series.occurrence_day(zone.instant(written), offset_at),
        (Some(zone), When::Date(day)) if !series.all_day => {
            let first = zone.wall(series.instants(offset_at).0);
            series.occurrence_day(zone.instant(DateTime { date: day, ..first }), offset_at)
        }
        _ => at.when.local(offset_at).date,
    }
}

/// A text property that may be given once: the first one counts.
fn once(slot: &mut Option<String>, line: &ContentLine) {
    if slot.is_none() {
        *slot = Some(unescape_text(&line.value));
    }
}

fn add_categories(list: &mut Vec<String>, value: &str) {
    for item in split_list(value, MAX_CATEGORIES) {
        let name = unescape_text(item).trim().to_string();
        if !name.is_empty() && list.len() < MAX_CATEGORIES {
            list.push(name);
        }
    }
}

/// The person of an `ORGANIZER` or `ATTENDEE` line: `CN` and the `mailto:` address (or the `EMAIL`
/// parameter when the value is some other kind of URI).
fn person(line: &ContentLine) -> Option<Address> {
    let value = line.value.trim();
    let mailto = value.get(..7).filter(|scheme| scheme.eq_ignore_ascii_case("mailto:")).and_then(|_| value.get(7..));
    let email = match mailto {
        Some(rest) => rest,
        None if value.contains(':') => "",
        None => value,
    };
    let email = if email.trim().is_empty() { line.param("EMAIL").unwrap_or("") } else { email };
    let email: String = email.chars().filter(|c| !c.is_control()).collect();
    // A name with a comma in it that its writer didn't quote reads as several values.
    let name = line.params.iter().find(|p| p.name == "CN").map(|p| p.values.join(",")).unwrap_or_default();
    let person = Address::new(name, email);
    (!person.name.is_empty() || !person.email.is_empty()).then_some(person)
}

#[derive(Default)]
struct EventBuilder {
    uid: Option<String>,
    title: Option<String>,
    location: Option<String>,
    description: Option<String>,
    start: Option<Stamp>,
    end: Option<Stamp>,
    /// `DURATION`, in seconds.
    duration: Option<i64>,
    /// `RECURRENCE-ID`: the value as written, and what it says.
    instance: Option<(String, Stamp)>,
    /// `RRULE`, as written: what its `UNTIL` means depends on the start, which may come later.
    rule: Option<String>,
    /// `EXDATE`s, which are named by the series' own days once its zone is known.
    exceptions: Vec<Stamp>,
    /// What there is to say about the zones of its times.
    notes: Vec<String>,
    organizer: Option<Address>,
    attendees: Vec<Attendee>,
    categories: Vec<String>,
    private: bool,
    cancelled: bool,
    transparent: bool,
    busy: Option<ShowAs>,
    /// `X-MICROSOFT-CDO-ALLDAYEVENT:TRUE`: midnight to midnight means all day.
    whole_days: bool,
    alarms: Vec<Trigger>,
    /// `PRIORITY`, and what Outlook says beside it (`X-MICROSOFT-CDO-IMPORTANCE`).
    priority: Option<Importance>,
    cdo_importance: Option<Importance>,
    /// `X-MICROSOFT-DISALLOW-COUNTER:TRUE`: the organizer takes no proposals of another time.
    no_counter: bool,
    /// How many of the people invited are asked to answer (`RSVP=TRUE`), and how many are told
    /// they needn't (`RSVP=FALSE`).
    rsvp: (usize, usize),
    /// The link to join an online meeting by, with what it is held with.
    join: Option<OnlineMeeting>,
}

/// What `PRIORITY` says of how much something matters: 1 to 4 much, 6 to 9 little.
fn priority(value: &str) -> Option<Importance> {
    match value.trim().parse::<u8>() {
        Ok(1..=4) => Some(Importance::High),
        Ok(5) => Some(Importance::Normal),
        Ok(6..=9) => Some(Importance::Low),
        _ => None,
    }
}

impl EventBuilder {
    fn apply(&mut self, line: &ContentLine, zones: &FileZones) {
        match line.name.as_str() {
            "UID" => once(&mut self.uid, line),
            "SUMMARY" => once(&mut self.title, line),
            "LOCATION" => once(&mut self.location, line),
            "DESCRIPTION" => once(&mut self.description, line),
            "DTSTART" => {
                if self.start.is_none() {
                    self.start = stamp_of(line, &line.value, zones, &mut self.notes);
                }
            }
            "DTEND" => {
                if self.end.is_none() {
                    self.end = stamp_of(line, &line.value, zones, &mut self.notes);
                }
            }
            "DURATION" => self.duration = self.duration.or_else(|| parse_duration(&line.value)),
            "RECURRENCE-ID" => {
                if self.instance.is_none() {
                    self.instance =
                        stamp_of(line, &line.value, zones, &mut self.notes).map(|at| (prefix(line.value.trim(), MAX_KEYWORD).to_string(), at));
                }
            }
            "RRULE" => {
                // The first rule that is one counts.
                if self.rule.is_none() && rrule::parse_rrule(&line.value).is_some() {
                    self.rule = Some(prefix(line.value.trim(), 4096).to_string());
                }
            }
            "EXDATE" => {
                let room = MAX_EXDATES.saturating_sub(self.exceptions.len());
                for value in line.value.split(',').take(room) {
                    if let Some(at) = stamp_of(line, value, zones, &mut self.notes) {
                        self.exceptions.push(at);
                    }
                }
            }
            "CATEGORIES" => add_categories(&mut self.categories, &line.value),
            "CLASS" => self.private = matches!(keyword(&line.value).as_str(), "PRIVATE" | "CONFIDENTIAL"),
            "STATUS" => self.cancelled = matches!(keyword(&line.value).as_str(), "CANCELLED" | "CANCELED"),
            "TRANSP" => self.transparent = keyword(&line.value) == "TRANSPARENT",
            "X-MICROSOFT-CDO-BUSYSTATUS" => self.busy = ShowAs::parse(&line.value).or(self.busy),
            "X-MICROSOFT-CDO-ALLDAYEVENT" => self.whole_days = keyword(&line.value) == "TRUE",
            "ORGANIZER" => self.organizer = self.organizer.take().or_else(|| person(line)),
            "ATTENDEE" => {
                if self.attendees.len() < MAX_ATTENDEES
                    && let Some(address) = person(line)
                {
                    let role = line.param("ROLE").map(keyword).unwrap_or_default();
                    let response = match line.param("PARTSTAT").map(keyword).unwrap_or_default().as_str() {
                        "ACCEPTED" => Response::Accepted,
                        "TENTATIVE" => Response::Tentative,
                        "DECLINED" => Response::Declined,
                        _ => Response::None,
                    };
                    self.attendees.push(Attendee { address, optional: matches!(role.as_str(), "OPT-PARTICIPANT" | "NON-PARTICIPANT"), response });
                    match line.param("RSVP").map(keyword).unwrap_or_default().as_str() {
                        "TRUE" => self.rsvp.0 = self.rsvp.0.saturating_add(1),
                        "FALSE" => self.rsvp.1 = self.rsvp.1.saturating_add(1),
                        _ => {}
                    }
                }
            }
            "PRIORITY" => self.priority = self.priority.or_else(|| priority(&line.value)),
            "X-MICROSOFT-CDO-IMPORTANCE" => {
                self.cdo_importance = self.cdo_importance.or(match line.value.trim() {
                    "0" => Some(Importance::Low),
                    "1" => Some(Importance::Normal),
                    "2" => Some(Importance::High),
                    _ => None,
                })
            }
            "X-MICROSOFT-DISALLOW-COUNTER" => self.no_counter = keyword(&line.value) == "TRUE",
            // The link to join by: an `https` address, or it is no link. (Whatever else a file
            // says of a meeting online, a dial-in number or a `conf:` address, is not read.)
            "X-MICROSOFT-SKYPETEAMSMEETINGURL" => {
                let link = OnlineMeeting::clean_link(&unescape_text(&line.value));
                self.join = link.map(|join_url| OnlineMeeting { provider: OnlineProvider::Teams, join_url }).or(self.join.take());
            }
            "CONFERENCE" if self.join.is_none() => {
                self.join = OnlineMeeting::clean_link(&line.value).map(|join_url| OnlineMeeting { provider: OnlineProvider::Other, join_url });
            }
            _ => {}
        }
    }

    /// The event, and for a changed instance the uid of its series and the day it replaces. `None`
    /// without a start.
    fn finish(self, offset_at: OffsetAt, notes: &mut Vec<String>) -> Option<(Event, Option<(String, Stamp)>)> {
        let first = self.start?;
        notes.extend(self.notes);
        let is_instant = |at: &Stamp| matches!(at.when, When::Utc(_));
        let mut start = first.when.local(offset_at);
        let end = match self.end {
            Some(end) => Some(end.when.local(offset_at)),
            // So long after the start: by the clock for a time as written, in seconds for an instant.
            None => self.duration.filter(|seconds| *seconds >= 0).map(|seconds| match first.when {
                When::Utc(_) => wall_at(first.when.timestamp(offset_at).saturating_add(seconds), offset_at),
                _ => DateTime::from_civil_seconds(start.civil_seconds().saturating_add(seconds)),
            }),
        };
        let midnight = |t: DateTime| t.seconds_of_day() == 0;
        // Midnight to midnight where it was written, with the flag some programs set instead of
        // a DATE value: those days, wherever it is read.
        let flagged = self.end.filter(|end| self.whole_days && midnight(first.written) && midnight(end.written) && end.written > first.written);
        let (end, all_day) = if matches!(first.when, When::Date(_)) {
            // Whole days, and at least one of them.
            (end.map(|end| DateTime::at_midnight(end.date)).filter(|end| *end > start).unwrap_or_else(|| start.add_days(1)), true)
        } else if let Some(last) = flagged {
            start = first.written;
            (last.written, true)
        } else {
            (end.filter(|end| *end >= start).unwrap_or(start), false)
        };
        let length = end.civil_seconds().saturating_sub(start.civil_seconds());
        let reminder_minutes = self.alarms.iter().find_map(|alarm| {
            let before = match *alarm {
                Trigger::Relative { seconds, from_end: false } => seconds.saturating_neg(),
                Trigger::Relative { seconds, from_end: true } => seconds.saturating_add(length).saturating_neg(),
                Trigger::Absolute(at) => start.civil_seconds().saturating_sub(at.local(offset_at).civil_seconds()),
            };
            if before >= 0 { u32::try_from(before / 60).ok() } else { None }
        });
        let uid = self.uid.unwrap_or_default().trim().to_string();
        let changed = self.instance.as_ref().map(|(_, at)| (uid.clone(), *at));
        let uid = match &self.instance {
            Some((written, _)) => format!("{uid}#{written}"),
            None => uid,
        };
        let series = changed.is_none();
        let repeats = series && self.rule.is_some();
        // When it is, for a start that is an instant; and the zone it keeps as its own: one
        // whose clock is another than the one it is read by.
        let instants = (!all_day && is_instant(&first)).then(|| {
            let begins = first.when.timestamp(offset_at);
            let ends = match (self.end, self.duration) {
                (Some(end), _) if is_instant(&end) => end.when.timestamp(offset_at),
                (None, Some(seconds)) if seconds >= 0 => begins.saturating_add(seconds),
                _ => instant_at(end, offset_at),
            };
            (begins, ends.max(begins))
        });
        let (start_zone, end_zone) = match instants {
            Some(instants) => own_zones(first.zone, self.end.and_then(|end| end.zone), instants, repeats, offset_at),
            None => (String::new(), String::new()),
        };
        let mut event = Event {
            id: 0,
            calendar: 0,
            uid,
            title: self.title.unwrap_or_default(),
            location: self.location.unwrap_or_default(),
            description: self.description.unwrap_or_default(),
            start,
            end,
            all_day,
            start_zone,
            end_zone,
            organizer: self.organizer,
            attendees: self.attendees,
            reminder_minutes,
            show_as: self.busy.unwrap_or(if self.transparent { ShowAs::Free } else { ShowAs::Busy }),
            categories: self.categories,
            private: self.private,
            cancelled: self.cancelled,
            importance: self.priority.or(self.cdo_importance).unwrap_or_default(),
            response_options: ResponseOptions {
                // Asked to answer unless everybody who is told anything is told not to.
                request_responses: !(self.rsvp.1 > 0 && self.rsvp.0 == 0),
                allow_proposals: !self.no_counter,
            },
            online: self.join,
            ..Default::default()
        };
        if let Some((begins, ends)) = instants {
            // Exactly then: also when the clock here shows that time twice.
            event.set_instants(begins, ends, offset_at);
        }
        if repeats {
            // The rule counts its days where the series repeats: an `UNTIL` in UTC is a day there.
            let there = event.zone();
            let in_zone = |instant: Timestamp| there.map_or_else(|| offset_at(instant), |zone| zone.offset_at(instant));
            event.recurrence = self.rule.as_deref().and_then(|rule| rrule::parse_with(rule, Some(&in_zone)));
        }
        if series {
            let mut days: Vec<Date> = Vec::with_capacity(self.exceptions.len());
            days.extend(self.exceptions.iter().map(|at| series_day(&event, at, offset_at)));
            event.exceptions = days;
        }
        Some((event, changed))
    }
}

#[derive(Default)]
struct TodoBuilder {
    /// Whether any property a task is made of was seen.
    seen: bool,
    uid: Option<String>,
    title: Option<String>,
    notes: Option<String>,
    start: Option<When>,
    due: Option<When>,
    completed: Option<When>,
    /// `STATUS:COMPLETED` or `PERCENT-COMPLETE:100`.
    done: bool,
    /// `DTSTAMP`, else `LAST-MODIFIED`: when a task that is only marked done was last touched.
    stamp: Option<When>,
    modified: Option<When>,
    importance: Importance,
    categories: Vec<String>,
    alarms: Vec<Trigger>,
}

impl TodoBuilder {
    fn apply(&mut self, line: &ContentLine, zones: &FileZones) {
        let mut seen = true;
        // (A to-do keeps days, not zones: nothing is said about the zone of one.)
        let when = |line: &ContentLine| stamp_of(line, &line.value, zones, &mut Vec::new()).map(|at| at.when);
        match line.name.as_str() {
            "UID" => once(&mut self.uid, line),
            "SUMMARY" => once(&mut self.title, line),
            "DESCRIPTION" => once(&mut self.notes, line),
            "DTSTART" => self.start = self.start.or_else(|| when(line)),
            "DUE" => self.due = self.due.or_else(|| when(line)),
            "COMPLETED" => self.completed = self.completed.or_else(|| When::parse(&line.value)),
            "STATUS" => self.done |= keyword(&line.value) == "COMPLETED",
            "PERCENT-COMPLETE" => self.done |= line.value.trim() == "100",
            "PRIORITY" => {
                self.importance = match line.value.trim().parse::<u8>() {
                    Ok(1..=4) => Importance::High,
                    Ok(6..=9) => Importance::Low,
                    _ => Importance::Normal,
                }
            }
            "CATEGORIES" => add_categories(&mut self.categories, &line.value),
            "DTSTAMP" => {
                self.stamp = self.stamp.or_else(|| When::parse(&line.value));
                seen = false;
            }
            "LAST-MODIFIED" => {
                self.modified = self.modified.or_else(|| When::parse(&line.value));
                seen = false;
            }
            _ => seen = false,
        }
        self.seen |= seen;
    }

    /// The task; `None` for a to-do with nothing in it.
    fn finish(self, offset_at: OffsetAt) -> Option<Task> {
        if !self.seen {
            return None;
        }
        let completed = match self.completed {
            Some(at) => Some(at.timestamp(offset_at)),
            None if self.done => Some(self.stamp.or(self.modified).map_or(0, |at| at.timestamp(offset_at))),
            None => None,
        };
        let reminder = self.alarms.iter().find_map(|alarm| match *alarm {
            Trigger::Absolute(at) => Some(at.timestamp(offset_at)),
            Trigger::Relative { seconds, from_end } => {
                let from = if from_end { self.due } else { self.start.or(self.due) };
                from.map(|from| from.timestamp(offset_at).saturating_add(seconds))
            }
        });
        Some(Task {
            id: 0,
            title: self.title.unwrap_or_default(),
            notes: self.notes.unwrap_or_default(),
            start: self.start.map(|at| at.local(offset_at).date),
            due: self.due.map(|at| at.local(offset_at).date),
            completed,
            importance: self.importance,
            categories: self.categories,
            reminder,
            message: None,
            uid: self.uid.unwrap_or_default().trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use mailcraft_model::{Freq, Recurrence};
    use mailcraft_time::Weekday;

    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    fn at(y: i32, m: u8, day: u8, h: u8, min: u8) -> DateTime {
        DateTime::new(d(y, m, day), h, min, 0).unwrap()
    }

    fn utc(text: &str) -> IcsFile {
        parse(text, &|_| 0)
    }

    /// One event from the properties of a `VEVENT`.
    fn event(properties: &str) -> Event {
        let file = utc(&format!("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n{properties}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"));
        assert_eq!(file.events.len(), 1, "{properties}");
        file.events.into_iter().next().unwrap()
    }

    /// One task from the properties of a `VTODO`.
    fn task(properties: &str) -> Task {
        let file = utc(&format!("BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\n{properties}\r\nEND:VTODO\r\nEND:VCALENDAR\r\n"));
        assert_eq!(file.tasks.len(), 1, "{properties}");
        file.tasks.into_iter().next().unwrap()
    }

    /// A meeting invitation the way the big mail programs write one.
    const INVITATION: &str = "BEGIN:VCALENDAR\r\n\
PRODID:-//Example Corp//Example Calendar 16.0//EN\r\n\
VERSION:2.0\r\n\
CALSCALE:GREGORIAN\r\n\
METHOD:request\r\n\
X-WR-TIMEZONE:Example/Riverton\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Example/Riverton\r\n\
BEGIN:STANDARD\r\n\
DTSTART:16011104T020000\r\n\
RRULE:FREQ=YEARLY;BYDAY=1SU;BYMONTH=11\r\n\
TZOFFSETFROM:-0400\r\n\
TZOFFSETTO:-0500\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:16010311T020000\r\n\
RRULE:FREQ=YEARLY;BYDAY=2SU;BYMONTH=3\r\n\
TZOFFSETFROM:-0500\r\n\
TZOFFSETTO:-0400\r\n\
END:DAYLIGHT\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
ORGANIZER;CN=\"Okafor, Nadia\":mailto:nadia.okafor@example.org\r\n\
ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN=Tomas Lind\r\n qvist:mailto:tomas.lindqvist@example.org\r\n\
ATTENDEE;ROLE=OPT-PARTICIPANT;PARTSTAT=TENTATIVE;CN=Mei Tanaka:MAILTO:mei.tanaka@example.org\r\n\
ATTENDEE;CUTYPE=ROOM;ROLE=NON-PARTICIPANT;PARTSTAT=ACCEPTED;CN=Room 4.12:mailto:room-412@example.org\r\n\
ATTENDEE;PARTSTAT=DECLINED:mailto:ibrahim@example.org\r\n\
DESCRIPTION;LANGUAGE=en-US:Agenda:\\n1. Budget\\, phase two\\n2. Hiring\\; open ro\r\n les\\n\r\n\
UID:0400AA11BB22CC33-planning@example.org\r\n\
SUMMARY;LANGUAGE=en-US:Quarterly planning\r\n\
DTSTART;TZID=Example/Riverton:20261012T090000\r\n\
DTEND;TZID=Example/Riverton:20261012T103000\r\n\
CLASS:PUBLIC\r\n\
PRIORITY:5\r\n\
DTSTAMP:20261001T141500Z\r\n\
TRANSP:OPAQUE\r\n\
STATUS:CONFIRMED\r\n\
SEQUENCE:0\r\n\
LOCATION;LANGUAGE=en-US:Room 4.12\\, North Building\r\n\
X-MICROSOFT-CDO-APPT-SEQUENCE:0\r\n\
X-MICROSOFT-CDO-BUSYSTATUS:TENTATIVE\r\n\
X-MICROSOFT-CDO-INTENDEDSTATUS:BUSY\r\n\
X-MICROSOFT-CDO-ALLDAYEVENT:FALSE\r\n\
X-MICROSOFT-CDO-IMPORTANCE:1\r\n\
X-ALT-DESC;FMTTYPE=text/html:<html><body><p>Agenda</p></body></html>\r\n\
BEGIN:VALARM\r\n\
DESCRIPTION:REMINDER\r\n\
TRIGGER;RELATED=START:-PT15M\r\n\
ACTION:DISPLAY\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn reads_a_meeting_invitation() {
        let file = utc(INVITATION);
        assert_eq!(file.method, "REQUEST");
        assert!(file.tasks.is_empty());
        // The time zone's own DTSTART and RRULE lines are not an event.
        assert_eq!(file.events.len(), 1);
        let e = &file.events[0];
        assert_eq!((e.id, e.calendar), (0, 0));
        assert_eq!(e.uid, "0400AA11BB22CC33-planning@example.org");
        assert_eq!(e.title, "Quarterly planning");
        assert_eq!(e.location, "Room 4.12, North Building");
        assert_eq!(e.description, "Agenda:\n1. Budget, phase two\n2. Hiring; open roles\n");
        // Nine o'clock in the zone the text defines (four hours behind UTC in October) is one
        // o'clock in UTC, where this is read. Nobody knows a zone of that name; it keeps the
        // clock of the zone it is like, and nothing needs saying.
        assert_eq!((e.start, e.end, e.all_day), (at(2026, 10, 12, 13, 0), at(2026, 10, 12, 14, 30), false));
        assert_eq!((e.start_zone.as_str(), e.end_zone.as_str()), ("America/New_York", ""));
        assert!(file.notes.is_empty(), "{:?}", file.notes);
        // Read where the clock is the same, it is nine o'clock, and has no zone of its own.
        let new_york = Zone::named("America/New_York").unwrap();
        let there = parse(INVITATION, &|instant| new_york.offset_at(instant));
        assert_eq!((there.events[0].start, there.events[0].start_zone.as_str()), (at(2026, 10, 12, 9, 0), ""));
        assert_eq!(e.organizer, Some(Address::new("Okafor, Nadia", "nadia.okafor@example.org")));
        let attendees: Vec<(&str, &str, bool, Response)> =
            e.attendees.iter().map(|a| (a.address.name.as_str(), a.address.email.as_str(), a.optional, a.response)).collect();
        assert_eq!(
            attendees,
            [
                ("Tomas Lindqvist", "tomas.lindqvist@example.org", false, Response::None),
                ("Mei Tanaka", "mei.tanaka@example.org", true, Response::Tentative),
                ("Room 4.12", "room-412@example.org", true, Response::Accepted),
                ("", "ibrahim@example.org", false, Response::Declined),
            ]
        );
        assert_eq!(e.reminder_minutes, Some(15));
        assert_eq!(e.show_as, ShowAs::Tentative);
        assert!(!e.private && !e.cancelled && e.recurrence.is_none() && e.exceptions.is_empty() && e.categories.is_empty());
        assert!(e.is_meeting());
    }

    #[test]
    fn utc_times_become_local_wall_clock_times() {
        let text = "BEGIN:VEVENT\nDTSTART:20261012T130000Z\nDTEND:20261012T143000Z\nEND:VEVENT\n";
        let e = parse(text, &|_| -240).events.remove(0);
        assert_eq!((e.start, e.end), (at(2026, 10, 12, 9, 0), at(2026, 10, 12, 10, 30)));
        // The offset is asked for each instant: an event across a change of offset.
        let change = at(2026, 11, 1, 6, 0).to_timestamp(0);
        let zone = |ts: Timestamp| if ts < change { -240 } else { -300 };
        let e = parse("BEGIN:VEVENT\nDTSTART:20261101T050000Z\nDTEND:20261101T070000Z\nEND:VEVENT\n", &zone).events.remove(0);
        assert_eq!((e.start, e.end), (at(2026, 11, 1, 1, 0), at(2026, 11, 1, 2, 0)));
        // Floating times are left alone, and so are times in a zone of which nothing is known,
        // which is said.
        let file = parse("BEGIN:VEVENT\nDTSTART:20261012T130000\nDTEND;TZID=Example/Riverton:20261012T143000\nEND:VEVENT\n", &|_| -240);
        let e = &file.events[0];
        assert_eq!((e.start, e.end, e.start_zone.as_str()), (at(2026, 10, 12, 13, 0), at(2026, 10, 12, 14, 30), ""));
        assert_eq!(file.notes, ["The time zone \"Example/Riverton\" isn't one that is known: its times are kept as they are written."]);
        // UTC under another name is UTC. A series written in UTC repeats by UTC's clock, and
        // the day it skips is the day of that occurrence.
        let text = "BEGIN:VEVENT\nDTSTART;TZID=UTC:20261012T130000\nDTEND;TZID=\"Etc/UTC\":20261012T143000\nRRULE:FREQ=DAILY\nEXDATE;TZID=utc:20261014T130000\nEND:VEVENT\n\
BEGIN:VTODO\nDUE;TZID=GMT:20261016T020000\nEND:VTODO\n";
        let file = parse(text, &|_| -240);
        assert_eq!((file.events[0].start, file.events[0].end), (at(2026, 10, 12, 9, 0), at(2026, 10, 12, 10, 30)));
        assert_eq!((file.events[0].start_zone.as_str(), &file.events[0].exceptions[..]), ("UTC", &[d(2026, 10, 14)][..]));
        assert_eq!(file.tasks[0].due, Some(d(2026, 10, 15)));
        assert!(file.notes.is_empty());
    }

    #[test]
    fn reads_an_all_day_event_over_several_days() {
        let text = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Trips//EN\r\nBEGIN:VEVENT\r\nUID:trip-7@example.org\r\nDTSTAMP:20260901T080000Z\r\n\
DTSTART;VALUE=DATE:20261012\r\nDTEND;VALUE=DATE:20261015\r\nSUMMARY:Conference in Riverton\r\nTRANSP:TRANSPARENT\r\nCATEGORIES:Travel,Work\\, external\r\nCATEGORIES:Conference\r\n\
CLASS:PRIVATE\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let file = utc(text);
        assert_eq!(file.method, "");
        let e = &file.events[0];
        assert!(e.all_day && e.private);
        assert_eq!((e.start, e.end), (at(2026, 10, 12, 0, 0), at(2026, 10, 15, 0, 0)));
        assert_eq!(e.duration_minutes(), 3 * 24 * 60);
        assert_eq!(e.show_as, ShowAs::Free);
        assert_eq!(e.categories, ["Travel", "Work, external", "Conference"]);
        assert_eq!(e.reminder_minutes, None);
    }

    #[test]
    fn all_day_events_last_at_least_a_day() {
        // No end, the end on the start, an end before the start, a duration.
        assert_eq!(event("DTSTART;VALUE=DATE:20261012").end, at(2026, 10, 13, 0, 0));
        assert_eq!(event("DTSTART:20261012\nDTEND:20261012").end, at(2026, 10, 13, 0, 0));
        assert_eq!(event("DTSTART:20261012\nDTEND:20261001").end, at(2026, 10, 13, 0, 0));
        assert_eq!(event("DTSTART:20261012\nDURATION:P3D").end, at(2026, 10, 15, 0, 0));
        assert_eq!(event("DTSTART:20261012\nDURATION:PT6H").end, at(2026, 10, 13, 0, 0));
        // An end with a time on it keeps its date.
        let mixed = event("DTSTART;VALUE=DATE:20261012\nDTEND:20261014T170000");
        assert_eq!((mixed.all_day, mixed.end), (true, at(2026, 10, 14, 0, 0)));
        // Midnight to midnight with the flag some programs set instead of a DATE value.
        let flagged =
            event("DTSTART;TZID=Example/Riverton:20261012T000000\nDTEND;TZID=Example/Riverton:20261013T000000\nX-MICROSOFT-CDO-ALLDAYEVENT:TRUE");
        assert!(flagged.all_day);
        assert!(!event("DTSTART:20261012T000000\nDTEND:20261013T000000").all_day);
        assert!(!event("DTSTART:20261012T090000\nDTEND:20261013T000000\nX-MICROSOFT-CDO-ALLDAYEVENT:TRUE").all_day);
    }

    #[test]
    fn timed_events_end_no_earlier_than_they_start() {
        let start = at(2026, 10, 12, 9, 0);
        assert_eq!(event("DTSTART:20261012T090000").end, start);
        assert_eq!(event("DTSTART:20261012T090000\nDTEND:20261012T080000").end, start);
        assert_eq!(event("DTSTART:20261012T090000\nDURATION:PT1H30M").end, at(2026, 10, 12, 10, 30));
        assert_eq!(event("DTSTART:20261012T090000\nDURATION:P1W").end, at(2026, 10, 19, 9, 0));
        assert_eq!(event("DTSTART:20261012T090000\nDURATION:-PT1H").end, start);
        assert_eq!(event("DTSTART:20261012T090000\nDURATION:soon").end, start);
        // DTEND wins over DURATION, and the first DTSTART over a second.
        assert_eq!(event("DTSTART:20261012T090000\nDURATION:PT1H\nDTEND:20261012T093000").end, at(2026, 10, 12, 9, 30));
        assert_eq!(event("DTSTART:20261012T090000\nDTSTART:20270101T000000").start, start);
        assert_eq!(event("DTSTART:20261012T090000\nDTEND:20261012").end, start);
    }

    /// A weekly series with two cancelled days and one moved meeting.
    const SERIES: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Series//EN\r\n\
BEGIN:VEVENT\r\nUID:standup@example.org\r\nSUMMARY:Stand-up\r\nDTSTART:20261005T140000\r\nDTEND:20261005T143000\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261231T235959\r\nEXDATE:20261019T140000,20261102T140000\r\nEXDATE;VALUE=DATE:20261130\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:standup@example.org\r\nRECURRENCE-ID:20261026T140000\r\nSUMMARY:Stand-up (moved)\r\nDTSTART:20261027T100000\r\nDTEND:20261027T103000\r\n\
RRULE:FREQ=WEEKLY\r\nEXDATE:20261027T100000\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:other@example.org\r\nRECURRENCE-ID;VALUE=DATE:20261224\r\nSUMMARY:An instance without its series\r\nDTSTART;VALUE=DATE:20261224\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn reads_a_series_with_cancelled_days_and_a_changed_instance() {
        let file = utc(SERIES);
        assert_eq!(file.events.len(), 3);
        let series = &file.events[0];
        assert_eq!(series.uid, "standup@example.org");
        assert_eq!(
            series.recurrence,
            Some(Recurrence { by_day: vec![Weekday::Mon], until: Some(d(2026, 12, 31)), ..Recurrence::every(Freq::Weekly) })
        );
        // Its own cancelled days, then the day the changed instance replaces.
        assert_eq!(series.exceptions, [d(2026, 10, 19), d(2026, 11, 2), d(2026, 11, 30), d(2026, 10, 26)]);
        let days: Vec<String> = series.occurrences(d(2026, 10, 1), d(2026, 11, 10)).into_iter().map(|o| o.start.date.iso()).collect();
        assert_eq!(days, ["2026-10-05", "2026-10-12", "2026-11-09"]);
        let moved = &file.events[1];
        assert_eq!(moved.uid, "standup@example.org#20261026T140000");
        assert_eq!((moved.title.as_str(), moved.start), ("Stand-up (moved)", at(2026, 10, 27, 10, 0)));
        assert!(moved.recurrence.is_none() && moved.exceptions.is_empty());
        let orphan = &file.events[2];
        assert_eq!(orphan.uid, "other@example.org#20261224");
        assert!(orphan.all_day);
    }

    #[test]
    fn a_changed_instance_in_utc_takes_out_its_local_day() {
        let text = "BEGIN:VEVENT\nUID:late\nRECURRENCE-ID:20261013T020000Z\nDTSTART:20261013T030000Z\nEND:VEVENT\n\
BEGIN:VEVENT\nUID:late\nDTSTART:20261006T020000Z\nRRULE:FREQ=WEEKLY;UNTIL=20261201T030000Z\nEXDATE:20261020T020000Z\nEND:VEVENT\n";
        let file = parse(text, &|_| -300);
        let series = &file.events[1];
        assert_eq!(series.start, at(2026, 10, 5, 21, 0));
        assert_eq!(series.exceptions, [d(2026, 10, 19), d(2026, 10, 12)]);
        // Written in UTC, it repeats by UTC's clock, and its rule counts UTC's days: the last
        // one is December 1 there, which is the evening of November 30 here.
        assert_eq!((series.start_zone.as_str(), series.recurrence.as_ref().and_then(|r| r.until)), ("UTC", Some(d(2026, 12, 1))));
        let last = series.occurrences_in(d(2026, 11, 20), d(2026, 12, 31), &|_| -300).pop().unwrap();
        assert_eq!((last.start, last.day), (at(2026, 11, 30, 21, 0), d(2026, 11, 30)));
        // Listed twice, taken out once.
        let twice = utc("BEGIN:VEVENT\nUID:s\nDTSTART:20261005\nRRULE:FREQ=DAILY\nEXDATE:20261007\nEND:VEVENT\n\
BEGIN:VEVENT\nUID:s\nRECURRENCE-ID:20261007\nDTSTART:20261007\nEND:VEVENT\nBEGIN:VEVENT\nUID:s\nRECURRENCE-ID:20261007\nDTSTART:20261008\nEND:VEVENT\n");
        assert_eq!(twice.events[0].exceptions, [d(2026, 10, 7)]);
    }

    #[test]
    fn reads_reminders() {
        let alarm = |trigger: &str| {
            event(&format!("DTSTART:20261012T090000\nDTEND:20261012T100000\nBEGIN:VALARM\nACTION:DISPLAY\n{trigger}\nEND:VALARM")).reminder_minutes
        };
        assert_eq!(alarm("TRIGGER:-PT15M"), Some(15));
        assert_eq!(alarm("TRIGGER:PT0S"), Some(0));
        assert_eq!(alarm("TRIGGER:-P1D"), Some(24 * 60));
        assert_eq!(alarm("TRIGGER:-PT90S"), Some(1));
        assert_eq!(alarm("TRIGGER;VALUE=DATE-TIME:20261012T083000"), Some(30));
        assert_eq!(alarm("TRIGGER;VALUE=DATE-TIME:20261012T064500Z"), Some(135));
        assert_eq!(alarm("TRIGGER;RELATED=END:-PT2H"), Some(60));
        // After the start, or junk: no reminder.
        assert_eq!(alarm("TRIGGER:PT5M"), None);
        assert_eq!(alarm("TRIGGER;RELATED=END:-PT30M"), None);
        assert_eq!(alarm("TRIGGER;VALUE=DATE-TIME:20261012T093000"), None);
        assert_eq!(alarm("TRIGGER:whenever"), None);
        assert_eq!(alarm("DESCRIPTION:no trigger"), None);
        // A placeholder that says "no alarm" with a date long past isn't a reminder.
        assert_eq!(alarm("X-EXAMPLE-DEFAULT-ALARM:TRUE\nTRIGGER;VALUE=DATE-TIME:19760401T005545Z\nACTION:NONE"), None);
        assert_eq!(alarm("ACTION:none\nTRIGGER:-PT15M"), None);
        // The first alarm that is before the start counts.
        let two = event(
            "DTSTART:20261012T090000\nBEGIN:VALARM\nTRIGGER:PT5M\nEND:VALARM\nBEGIN:VALARM\nTRIGGER:-PT10M\nTRIGGER:-PT99M\nEND:VALARM\nBEGIN:VALARM\nTRIGGER:-PT20M\nEND:VALARM",
        );
        assert_eq!(two.reminder_minutes, Some(10));
        // An alarm's own lines don't leak into the event.
        let e = event("DTSTART:20261012T090000\nSUMMARY:Review\nBEGIN:VALARM\nTRIGGER:-PT5M\nSUMMARY:Alarm text\nDESCRIPTION:Alarm text\nEND:VALARM");
        assert_eq!((e.title.as_str(), e.description.as_str()), ("Review", ""));
    }

    #[test]
    fn reads_status_class_and_free_busy() {
        assert!(event("DTSTART:20261012\nSTATUS:CANCELLED").cancelled);
        assert!(event("DTSTART:20261012\nstatus: cancelled ").cancelled);
        assert!(!event("DTSTART:20261012\nSTATUS:TENTATIVE").cancelled);
        assert!(event("DTSTART:20261012\nCLASS:CONFIDENTIAL").private);
        assert!(!event("DTSTART:20261012\nCLASS:PUBLIC").private);
        assert_eq!(event("DTSTART:20261012").show_as, ShowAs::Busy);
        assert_eq!(event("DTSTART:20261012\nTRANSP:TRANSPARENT").show_as, ShowAs::Free);
        for (code, want) in [
            ("FREE", ShowAs::Free),
            ("TENTATIVE", ShowAs::Tentative),
            ("BUSY", ShowAs::Busy),
            ("OOF", ShowAs::OutOfOffice),
            ("WORKINGELSEWHERE", ShowAs::WorkingElsewhere),
        ] {
            // Whichever line comes first.
            assert_eq!(event(&format!("DTSTART:20261012\nTRANSP:TRANSPARENT\nX-MICROSOFT-CDO-BUSYSTATUS:{code}")).show_as, want);
            assert_eq!(event(&format!("DTSTART:20261012\nX-MICROSOFT-CDO-BUSYSTATUS:{code}\nTRANSP:OPAQUE")).show_as, want);
        }
        assert_eq!(event("DTSTART:20261012\nTRANSP:TRANSPARENT\nX-MICROSOFT-CDO-BUSYSTATUS:ASLEEP").show_as, ShowAs::Free);
    }

    #[test]
    fn reads_people_in_every_spelling() {
        let organizer = |line: &str| event(&format!("DTSTART:20261012\n{line}")).organizer;
        assert_eq!(organizer("ORGANIZER;CN=Nadia Okafor:mailto:nadia@example.org"), Some(Address::new("Nadia Okafor", "nadia@example.org")));
        assert_eq!(organizer("ORGANIZER:MAILTO:nadia@example.org"), Some(Address::new("", "nadia@example.org")));
        assert_eq!(organizer("ORGANIZER:nadia@example.org"), Some(Address::new("", "nadia@example.org")));
        assert_eq!(organizer("ORGANIZER;CN=Nadia;EMAIL=nadia@example.org:urn:uuid:0000-1111"), Some(Address::new("Nadia", "nadia@example.org")));
        assert_eq!(organizer("ORGANIZER;CN=Nadia:invalid:nomail"), Some(Address::new("Nadia", "")));
        assert_eq!(
            organizer("ORGANIZER;CN=\"Nadia ^'the planner^' Okafor\":mailto:nadia@example.org").map(|a| a.name),
            Some("Nadia \"the planner\" Okafor".into())
        );
        assert_eq!(organizer("ORGANIZER:mailto:"), None);
        assert_eq!(organizer("ORGANIZER:http://example.org/people/7"), None);
        // A comma in a name its writer forgot to quote.
        assert_eq!(organizer("ORGANIZER;CN=Okafor, Nadia;X-NUM=1:mailto:nadia@example.org").map(|a| a.name), Some("Okafor, Nadia".into()));
        // The first organizer counts.
        assert_eq!(organizer("ORGANIZER:mailto:a@example.org\nORGANIZER:mailto:b@example.org").map(|a| a.email), Some("a@example.org".into()));
        let e = event(
            "DTSTART:20261012\nATTENDEE;role=opt-participant;partstat=accepted:mailto:a@example.org\nATTENDEE;PARTSTAT=DELEGATED:mailto:b@example.org\nATTENDEE:mailto:",
        );
        assert_eq!(e.attendees.len(), 2);
        assert_eq!((e.attendees[0].optional, e.attendees[0].response), (true, Response::Accepted));
        assert_eq!((e.attendees[1].optional, e.attendees[1].response), (false, Response::None));
    }

    #[test]
    fn reads_a_to_do_list() {
        let text = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Tasks 3//EN\r\n\
BEGIN:VTODO\r\nUID:task-1@example.org\r\nDTSTAMP:20261001T080000Z\r\nSUMMARY:Send the budget draft\r\nDESCRIPTION:Numbers from finance first\\; then the summary.\\nAsk Mei.\r\n\
DTSTART;VALUE=DATE:20261010\r\nDUE;VALUE=DATE:20261016\r\nPRIORITY:1\r\nCATEGORIES:Work,Finance\r\nSTATUS:NEEDS-ACTION\r\n\
BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;VALUE=DATE-TIME:20261015T160000Z\r\nEND:VALARM\r\nEND:VTODO\r\n\
BEGIN:VTODO\r\nUID:task-2@example.org\r\nDTSTAMP:20261001T080000Z\r\nSUMMARY:Book the venue\r\nDUE:20261003T170000Z\r\nCOMPLETED:20261002T101500Z\r\nSTATUS:COMPLETED\r\nPERCENT-COMPLETE:100\r\nPRIORITY:9\r\nEND:VTODO\r\n\
BEGIN:VTODO\r\nUID:task-3@example.org\r\nDTSTAMP:20261003T120000Z\r\nSUMMARY:Water the plants\r\nSTATUS:COMPLETED\r\nPRIORITY:5\r\nEND:VTODO\r\n\
BEGIN:VTODO\r\nSUMMARY:Someday\r\nEND:VTODO\r\n\
END:VCALENDAR\r\n";
        let file = utc(text);
        assert!(file.events.is_empty());
        assert_eq!(file.tasks.len(), 4);
        let stamp = |y, m, day, h, min| at(y, m, day, h, min).to_timestamp(0);
        assert_eq!(
            file.tasks[0],
            Task {
                title: "Send the budget draft".into(),
                notes: "Numbers from finance first; then the summary.\nAsk Mei.".into(),
                start: Some(d(2026, 10, 10)),
                due: Some(d(2026, 10, 16)),
                importance: Importance::High,
                categories: vec!["Work".into(), "Finance".into()],
                reminder: Some(stamp(2026, 10, 15, 16, 0)),
                uid: "task-1@example.org".into(),
                ..Default::default()
            }
        );
        let booked = &file.tasks[1];
        assert_eq!((booked.due, booked.completed, booked.importance), (Some(d(2026, 10, 3)), Some(stamp(2026, 10, 2, 10, 15)), Importance::Low));
        // Only marked done: the stamp says when.
        assert_eq!(file.tasks[2].completed, Some(stamp(2026, 10, 3, 12, 0)));
        assert_eq!(file.tasks[2].importance, Importance::Normal);
        assert_eq!(file.tasks[3], Task { title: "Someday".into(), ..Default::default() });
    }

    #[test]
    fn to_do_details() {
        assert_eq!(task("SUMMARY:x\nSTATUS:COMPLETED").completed, Some(0));
        assert_eq!(task("SUMMARY:x\nPERCENT-COMPLETE:100\nLAST-MODIFIED:19700101T000100Z").completed, Some(60));
        assert_eq!(task("SUMMARY:x\nPERCENT-COMPLETE:50").completed, None);
        assert_eq!(task("SUMMARY:x\nSTATUS:IN-PROCESS").completed, None);
        for (priority, want) in [
            ("0", Importance::Normal),
            ("1", Importance::High),
            ("4", Importance::High),
            ("5", Importance::Normal),
            ("6", Importance::Low),
            ("9", Importance::Low),
            ("10", Importance::Normal),
            ("high", Importance::Normal),
        ] {
            assert_eq!(task(&format!("SUMMARY:x\nPRIORITY:{priority}")).importance, want, "{priority}");
        }
        // A due time in UTC is a local day; a floating completion is a local instant.
        let west = parse("BEGIN:VTODO\nDUE:20261016T020000Z\nCOMPLETED:20261015T200000\nEND:VTODO\n", &|_| -300).tasks.remove(0);
        assert_eq!(west.due, Some(d(2026, 10, 15)));
        assert_eq!(west.completed, Some(at(2026, 10, 16, 1, 0).to_timestamp(0)));
        // A relative alarm counts from the start, or from the due date.
        let before_start = task("SUMMARY:x\nDTSTART:20261016T090000Z\nDUE:20261017T090000Z\nBEGIN:VALARM\nTRIGGER:-PT30M\nEND:VALARM");
        assert_eq!(before_start.reminder, Some(at(2026, 10, 16, 8, 30).to_timestamp(0)));
        let before_due = task("SUMMARY:x\nDTSTART:20261016T090000Z\nDUE:20261017T090000Z\nBEGIN:VALARM\nTRIGGER;RELATED=END:-PT30M\nEND:VALARM");
        assert_eq!(before_due.reminder, Some(at(2026, 10, 17, 8, 30).to_timestamp(0)));
        assert_eq!(task("SUMMARY:x\nBEGIN:VALARM\nTRIGGER:-PT30M\nEND:VALARM").reminder, None);
        // Nothing a task is made of: not a task.
        assert!(utc("BEGIN:VTODO\nDTSTAMP:20261001T080000Z\nX-THING:1\nEND:VTODO\n").tasks.is_empty());
    }

    #[test]
    fn junk_gives_an_empty_file() {
        for junk in [
            "",
            "\r\n\r\n",
            "hello",
            "BEGIN",
            "BEGIN:",
            "END:VEVENT",
            "BEGIN:VCALENDAR\nEND:VCALENDAR",
            "\u{0}\u{1}\u{2}\u{fffd}\u{fffd}PK\u{3}\u{4}",
            "::::;;;;====",
            "BEGIN:VEVENT\nEND:VEVENT",
            "BEGIN:VEVENT\nSUMMARY:No start\nEND:VEVENT",
            "BEGIN:VEVENT\nDTSTART:next week\nEND:VEVENT",
        ] {
            assert_eq!(utc(junk), IcsFile::default(), "{junk:?}");
        }
        let binary: String = (0u32..2000).filter_map(|i| char::from_u32(i.wrapping_mul(2_654_435_761) % 0x11_0000)).collect();
        assert_eq!(utc(&binary), IcsFile::default());
    }

    #[test]
    fn a_missing_end_loses_nothing() {
        // The file stops in the middle of the second event.
        let cut = utc("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nDTSTART:20261012\nEND:VEVENT\nBEGIN:VEVENT\nUID:b\nDTSTART:20261013\nSUMMARY:Cut he");
        assert_eq!(cut.events.iter().map(|e| e.uid.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(cut.events[1].title, "Cut he");
        // An event that never ends, then another, then a to-do.
        let run_on = utc(
            "BEGIN:VCALENDAR\nMETHOD:PUBLISH\nBEGIN:VEVENT\nUID:a\nDTSTART:20261012\nBEGIN:VEVENT\nUID:b\nDTSTART:20261013\nBEGIN:VTODO\nUID:c\nSUMMARY:t\nEND:VCALENDAR\n",
        );
        assert_eq!(run_on.events.iter().map(|e| e.uid.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(run_on.tasks.len(), 1);
        assert_eq!(run_on.method, "PUBLISH");
    }

    #[test]
    fn nested_components_without_an_end_are_contained() {
        // The alarm never ends: END:VEVENT ends both, and the next event is its own.
        let file = utc(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nDTSTART:20261012T090000\nBEGIN:VALARM\nTRIGGER:-PT10M\nEND:VEVENT\nBEGIN:VEVENT\nUID:b\nDTSTART:20261013T090000\nSUMMARY:Second\nEND:VEVENT\nEND:VCALENDAR\n",
        );
        assert_eq!(file.events.len(), 2);
        assert_eq!((file.events[0].reminder_minutes, file.events[0].title.as_str()), (Some(10), ""));
        assert_eq!((file.events[1].reminder_minutes, file.events[1].title.as_str()), (None, "Second"));
        // An alarm that never ends, then another: both are alarms of the event.
        let e = event("DTSTART:20261012T090000\nBEGIN:VALARM\nTRIGGER:PT5M\nBEGIN:VALARM\nTRIGGER:-PT10M\nEND:VALARM\nSUMMARY:After the alarms");
        assert_eq!((e.reminder_minutes, e.title.as_str()), (Some(10), "After the alarms"));
        // An unknown component inside an event keeps its lines to itself, closed or not.
        let e = event("DTSTART:20261012\nBEGIN:X-EXTRA\nSUMMARY:Inner\nBEGIN:X-DEEPER\nLOCATION:Inner\nEND:X-EXTRA\nSUMMARY:Outer");
        assert_eq!((e.title.as_str(), e.location.as_str()), ("Outer", ""));
        let open = utc("BEGIN:VEVENT\nDTSTART:20261012\nSUMMARY:Outer\nBEGIN:X-EXTRA\nSUMMARY:Inner\nEND:VEVENT\n");
        assert_eq!(open.events[0].title, "Outer");
        // Stray ENDs, a second calendar that starts before the first one ends, very deep nesting.
        let stray = utc(
            "END:VCALENDAR\nEND:VALARM\nBEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART:20261012\nEND:VTODO\nEND:STANDARD\nSUMMARY:Still here\nBEGIN:VCALENDAR\nMETHOD:CANCEL\nBEGIN:VEVENT\nDTSTART:20261013\nEND:VEVENT\n",
        );
        assert_eq!(stray.events.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Still here", ""]);
        assert_eq!(stray.method, "CANCEL");
        let deep = format!("{}BEGIN:VEVENT\nDTSTART:20261012\nEND:VEVENT\n{}", "BEGIN:X-DEEP\n".repeat(500), "END:X-DEEP\n".repeat(500));
        let _ = utc(&deep);
        // A method inside some other component isn't the file's.
        assert_eq!(utc("BEGIN:VCALENDAR\nBEGIN:VTIMEZONE\nMETHOD:REQUEST\nEND:VTIMEZONE\nEND:VCALENDAR\n").method, "");
        // Events are read without a calendar around them.
        assert_eq!(utc("METHOD:reply\nBEGIN:VEVENT\nDTSTART:20261012\nEND:VEVENT\n").method, "REPLY");
    }

    #[test]
    fn sizes_and_counts_are_capped() {
        let huge = format!("BEGIN:VEVENT\nDTSTART:20261012\nSUMMARY:{}\nDESCRIPTION:{}\nEND:VEVENT\n", "x".repeat(3 << 20), "é".repeat(1 << 20));
        let e = utc(&huge).events.remove(0);
        assert!(e.title.len() <= 1 << 20 && e.title.len() > 1_000_000);
        assert!(e.description.len() <= 1 << 20);
        let attendees = "ATTENDEE:mailto:someone@example.org\n".repeat(MAX_ATTENDEES + 50);
        let exdates = format!("EXDATE:{}\n", "20261012,".repeat(MAX_EXDATES)).repeat(2);
        let categories = format!("CATEGORIES:{}\n", "a,".repeat(MAX_CATEGORIES + 50));
        let alarms = "BEGIN:VALARM\nTRIGGER:PT5M\nEND:VALARM\n".repeat(MAX_ALARMS + 5);
        let crowded = utc(&format!(
            "BEGIN:VEVENT\nDTSTART:20261012\n{attendees}{exdates}{categories}{alarms}BEGIN:VALARM\nTRIGGER:-PT5M\nEND:VALARM\nEND:VEVENT\n"
        ))
        .events
        .remove(0);
        assert_eq!((crowded.attendees.len(), crowded.exceptions.len(), crowded.categories.len()), (MAX_ATTENDEES, MAX_EXDATES, MAX_CATEGORIES));
        assert_eq!(crowded.reminder_minutes, None);
        let many = "BEGIN:VEVENT\nDTSTART:20261012\nEND:VEVENT\nBEGIN:VTODO\nSUMMARY:t\nEND:VTODO\n".repeat(MAX_COMPONENTS / 2 + 10);
        let file = utc(&many);
        assert_eq!(file.events.len() + file.tasks.len(), MAX_COMPONENTS);
        // A series with more changed instances than it may have exceptions.
        let instances: String = (0..MAX_EXDATES + 20)
            .map(|i| format!("BEGIN:VEVENT\nUID:s\nRECURRENCE-ID:{}\nDTSTART:20261012\nEND:VEVENT\n", d(2000, 1, 1).add_days(i as i64).iso()))
            .collect();
        let series = utc(&format!("BEGIN:VEVENT\nUID:s\nDTSTART:20000101\nRRULE:FREQ=DAILY\nEND:VEVENT\n{instances}"));
        assert_eq!(series.events[0].exceptions.len(), MAX_EXDATES);
    }

    #[test]
    fn text_values_are_unescaped_and_names_are_case_blind() {
        let e = event(
            "dtstart;value=date:20261012\nSummary:Caf\u{e9} \\\"Zo\u{eb}\\\"\\, 10\\:30\nlocation:C:\\\\Rooms\\\\4\nuid:  spaced@example.org  \nDESCRIPTION:\nDESCRIPTION:second",
        );
        assert_eq!(e.title, "Caf\u{e9} \"Zo\u{eb}\", 10:30");
        assert_eq!(e.location, "C:\\Rooms\\4");
        assert_eq!(e.uid, "spaced@example.org");
        assert_eq!(e.description, "");
        assert!(e.all_day);
    }
}
