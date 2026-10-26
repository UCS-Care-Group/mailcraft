// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing events and to-dos as iCalendar text.

use std::collections::BTreeMap;

use mailcraft_model::zoned::instant_of;
use mailcraft_model::{Address, Event, Importance, OnlineMeeting, OnlineProvider, Response, ShowAs, Task};
use mailcraft_time::{Date, DateTime, Timestamp, Zone};

use crate::WriteOptions;
use crate::lines::{fold, param_value};
use crate::rrule;
use crate::values::{escape_text, format_date, format_local, format_utc, valid_time};
use crate::zones::{tzid, write_zone};

/// Write a `VCALENDAR` (`VERSION:2.0`, `PRODID:-//MailCraft//MailCraft//EN`) with CRLF line endings
/// and lines folded at 75 octets on char boundaries.
///
/// Times are written floating (no zone), just as the events have them; all-day events as dates.
/// ([`write_in`] writes them in their zones, which is what a file for another program wants.) An
/// event without a uid gets `mailcraft-<id>@mailcraft.local`, a to-do
/// `mailcraft-task-<id>@mailcraft.local`. Text is escaped; control characters other than line
/// breaks and tabs are dropped. A to-do's link to a message isn't written.
pub fn write(events: &[&Event], tasks: &[&Task], opts: &WriteOptions) -> String {
    write_in(events, tasks, opts, None)
}

/// How far past its start a series without an end has its zone written out, in years, when the
/// zone can only be written change by change.
const SERIES_YEARS: i32 = 10;

/// [`write()`], with every time in its zone: `zone` is the zone the calendar is shown in, which
/// is the one the events' wall-clock times are by ([`Mailbox::zone`](mailcraft_model::Mailbox)).
///
/// A timed event is written by the clock of its own zone when it has one
/// ([`Event::start_zone`]), else of `zone`, as `DTSTART;TZID=…`, and the text carries a
/// `VTIMEZONE` for each zone it names, so that a program which knows nothing of the zone's name
/// still reads the instant. A time in UTC is written as UTC (`…Z`). A series is written with the
/// rule in its zone's days, its `UNTIL` in UTC and its skipped days as times in the zone, as
/// RFC 5545 asks. All-day events are dates, which have no zone. Without a `zone` (or with one
/// the system couldn't name) the times are floating, as [`write()`] writes them.
pub fn write_in(events: &[&Event], tasks: &[&Task], opts: &WriteOptions, zone: Option<Zone>) -> String {
    let zone = zone.filter(|zone| !zone.is_local());
    let mut out = String::new();
    fold(&mut out, "BEGIN:VCALENDAR");
    fold(&mut out, "VERSION:2.0");
    fold(&mut out, "PRODID:-//MailCraft//MailCraft//EN");
    let method: String = opts.method.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').map(|c| c.to_ascii_uppercase()).collect();
    if !method.is_empty() {
        fold(&mut out, &format!("METHOD:{method}"));
    }
    if let Some(here) = zone {
        // The zones the events name, each with the years its times fall in.
        let mut named: BTreeMap<String, (Zone, i32, i32)> = BTreeMap::new();
        for event in events.iter().filter(|event| !event.all_day) {
            let times = Zoned::of(event, here);
            let last = match &event.recurrence {
                Some(rule) => rule.until.map_or(times.first.date.year.saturating_add(SERIES_YEARS), |until| until.year),
                None => times.last.date.year,
            };
            for (zone, from, to) in [(times.start_zone, times.first.date.year, last), (times.end_zone, times.last.date.year, times.last.date.year)] {
                if zone != Zone::UTC {
                    let span = named.entry(tzid(zone)).or_insert((zone, from, to));
                    (span.1, span.2) = (span.1.min(from), span.2.max(to).max(from));
                }
            }
        }
        for (zone, from, to) in named.into_values() {
            write_zone(&mut out, zone, from, to);
        }
    }
    let stamp = format_utc(opts.now);
    for event in events {
        write_event(&mut out, event, &stamp, zone);
    }
    for task in tasks {
        write_task(&mut out, task, &stamp);
    }
    fold(&mut out, "END:VCALENDAR");
    out
}

/// A timed event as it is written in its zones.
struct Zoned {
    start_zone: Zone,
    end_zone: Zone,
    /// When it starts and ends.
    start: Timestamp,
    end: Timestamp,
    /// The same by the clocks of its zones.
    first: DateTime,
    last: DateTime,
}

impl Zoned {
    /// `event`, whose wall-clock times are by the clock of `here`.
    fn of(event: &Event, here: Zone) -> Zoned {
        let offset_at = |instant: Timestamp| here.offset_at(instant);
        let start = instant_of(valid_time(event.start), event.start_repeated, &offset_at);
        let end = instant_of(valid_time(event.end), event.end_repeated, &offset_at).max(start);
        let start_zone = event.zone().filter(|zone| !zone.is_local()).unwrap_or(here);
        // A series ends by the clock it starts by.
        let end_zone = event.zone_of_end().filter(|zone| !zone.is_local() && event.recurrence.is_none()).unwrap_or(start_zone);
        Zoned { start_zone, end_zone, start, end, first: start_zone.wall(start), last: end_zone.wall(end) }
    }
}

/// `NAME;TZID=Europe/Berlin:20261012T090000`, or `NAME:20261012T070000Z` for UTC.
fn zoned_time(name: &str, instant: Timestamp, zone: Zone) -> String {
    if zone == Zone::UTC {
        format!("{name}:{}", format_utc(instant))
    } else {
        format!("{name};TZID={}:{}", param_value(&tzid(zone)), format_local(zone.wall(instant)))
    }
}

/// The last second of `day` by the clock of `zone`: where a series that ends that day ends.
/// Not "23:59:59 that day", which the clocks may skip (America/Nuuk springs from 23:00 to
/// midnight) and would then be an instant of the next day, nor the first of two 23:59:59s
/// where they show the last hour twice.
fn last_instant_of(zone: Zone, day: Date) -> Timestamp {
    let next = DateTime::at_midnight(day.add_days(1));
    let last = zone.instant(next).saturating_sub(1);
    // A midnight the clocks skip is taken after the change: step back to before it.
    let past = zone.wall(last).civil_seconds().saturating_sub(next.civil_seconds());
    if past >= 0 { last.saturating_sub(past.saturating_add(1)) } else { last }
}

/// `NAME:escaped text`.
fn text(out: &mut String, name: &str, value: &str) {
    fold(out, &format!("{name}:{}", escape_text(value)));
}

fn categories(out: &mut String, list: &[String]) {
    let names: Vec<String> = list.iter().map(|name| escape_text(name.trim())).filter(|name| !name.is_empty()).collect();
    if !names.is_empty() {
        fold(out, &format!("CATEGORIES:{}", names.join(",")));
    }
}

/// `;CN=Name:mailto:address`, the tail of an `ORGANIZER` or `ATTENDEE` line.
fn person(address: &Address) -> String {
    let name = address.name.trim();
    let email: String = address.email.trim().chars().filter(|c| !c.is_control()).collect();
    if name.is_empty() { format!(":mailto:{email}") } else { format!(";CN={}:mailto:{email}", param_value(name)) }
}

fn is_blank(address: &Address) -> bool {
    address.name.trim().is_empty() && address.email.trim().is_empty()
}

fn write_event(out: &mut String, e: &Event, stamp: &str, zone: Option<Zone>) {
    fold(out, "BEGIN:VEVENT");
    let uid = e.uid.trim();
    if uid.is_empty() {
        text(out, "UID", &format!("mailcraft-{}@mailcraft.local", e.id));
    } else {
        text(out, "UID", uid);
    }
    fold(out, &format!("DTSTAMP:{stamp}"));
    let start = valid_time(e.start);
    let end = valid_time(e.end).max(start);
    // In its zones, when the zone the calendar is shown in is known; the times as they are
    // otherwise.
    let zoned = zone.filter(|_| !e.all_day).map(|here| (here, Zoned::of(e, here)));
    if e.all_day {
        // The end is exclusive: a time left on it still counts its day in.
        let mut last = if end.seconds_of_day() > 0 { end.date.add_days(1) } else { end.date };
        if last <= start.date {
            last = start.date.add_days(1);
        }
        fold(out, &format!("DTSTART;VALUE=DATE:{}", format_date(start.date)));
        fold(out, &format!("DTEND;VALUE=DATE:{}", format_date(last)));
    } else if let Some((_, times)) = &zoned {
        // A time in the second of an hour that comes twice (clocks going back) can't be written
        // by the clock: read back, it would be the first. An event that happens once is then
        // written in UTC, and an end as the time it comes after the start.
        let exact = |instant: Timestamp, zone: Zone| zone.instant(zone.wall(instant)) == instant;
        if !exact(times.start, times.start_zone) && e.recurrence.is_none() {
            fold(out, &zoned_time("DTSTART", times.start, Zone::UTC));
            fold(out, &zoned_time("DTEND", times.end, Zone::UTC));
        } else {
            fold(out, &zoned_time("DTSTART", times.start, times.start_zone));
            if exact(times.end, times.end_zone) {
                fold(out, &zoned_time("DTEND", times.end, times.end_zone));
            } else {
                fold(out, &format!("DURATION:PT{}S", times.end.saturating_sub(times.start).max(0)));
            }
        }
    } else {
        fold(out, &format!("DTSTART:{}", format_local(start)));
        fold(out, &format!("DTEND:{}", format_local(end)));
    }
    if let Some(recurrence) = &e.recurrence {
        match &zoned {
            // The rule counts days in the series' zone; where it ends is an instant.
            Some((_, times)) => {
                let until = recurrence.until.map(|day| last_instant_of(times.start_zone, day));
                fold(out, &format!("RRULE:{}", rrule::format_zoned(recurrence, times.first, until)));
            }
            None => fold(out, &format!("RRULE:{}", rrule::format_for(recurrence, start, e.all_day))),
        }
    }
    for day in &e.exceptions {
        // The same value type as DTSTART, so other programs match the day to an occurrence.
        if e.all_day {
            fold(out, &format!("EXDATE;VALUE=DATE:{}", format_date(*day)));
        } else if let Some((here, times)) = &zoned {
            let offset_at = |instant: Timestamp| here.offset_at(instant);
            fold(out, &zoned_time("EXDATE", e.occurrence_instant(*day, &offset_at), times.start_zone));
        } else {
            fold(out, &format!("EXDATE:{}", format_local(DateTime { date: *day, ..start })));
        }
    }
    text(out, "SUMMARY", &e.title);
    if !e.location.is_empty() {
        text(out, "LOCATION", &e.location);
    }
    if !e.description.is_empty() {
        text(out, "DESCRIPTION", &e.description);
    }
    categories(out, &e.categories);
    if e.private {
        fold(out, "CLASS:PRIVATE");
    }
    if e.cancelled {
        fold(out, "STATUS:CANCELLED");
    }
    fold(out, if e.show_as == ShowAs::Free { "TRANSP:TRANSPARENT" } else { "TRANSP:OPAQUE" });
    let busy = match e.show_as {
        ShowAs::Free => "FREE",
        ShowAs::Tentative => "TENTATIVE",
        ShowAs::Busy => "BUSY",
        ShowAs::OutOfOffice => "OOF",
        ShowAs::WorkingElsewhere => "WORKINGELSEWHERE",
    };
    fold(out, &format!("X-MICROSOFT-CDO-BUSYSTATUS:{busy}"));
    // How much it matters, as for a to-do; nothing for the usual.
    match e.importance {
        Importance::High => fold(out, "PRIORITY:1"),
        Importance::Low => fold(out, "PRIORITY:9"),
        Importance::Normal => {}
    }
    if !e.response_options.allow_proposals {
        fold(out, "X-MICROSOFT-DISALLOW-COUNTER:TRUE");
    }
    // The link to join an online meeting by: where Outlook looks for a Teams meeting's, and
    // RFC 7986's `CONFERENCE` for any other. Only an `https` address is one.
    if let Some(online) = &e.online
        && let Some(link) = OnlineMeeting::clean_link(&online.join_url)
    {
        match online.provider {
            OnlineProvider::Teams => text(out, "X-MICROSOFT-SKYPETEAMSMEETINGURL", &link),
            _ => fold(out, &format!("CONFERENCE;VALUE=URI:{link}")),
        }
    }
    if let Some(organizer) = e.organizer.as_ref().filter(|o| !is_blank(o)) {
        fold(out, &format!("ORGANIZER{}", person(organizer)));
    }
    for attendee in e.attendees.iter().filter(|a| !is_blank(&a.address)) {
        let role = if attendee.optional { "OPT-PARTICIPANT" } else { "REQ-PARTICIPANT" };
        let status = match attendee.response {
            Response::None if !e.response_options.request_responses => "NEEDS-ACTION;RSVP=FALSE",
            Response::None => "NEEDS-ACTION;RSVP=TRUE",
            Response::Accepted => "ACCEPTED",
            Response::Tentative => "TENTATIVE",
            Response::Declined => "DECLINED",
        };
        fold(out, &format!("ATTENDEE;ROLE={role};PARTSTAT={status}{}", person(&attendee.address)));
    }
    if let Some(minutes) = e.reminder_minutes {
        fold(out, "BEGIN:VALARM");
        fold(out, "ACTION:DISPLAY");
        fold(out, "DESCRIPTION:Reminder");
        fold(out, &format!("TRIGGER:-PT{minutes}M"));
        fold(out, "END:VALARM");
    }
    fold(out, "END:VEVENT");
}

fn write_task(out: &mut String, t: &Task, stamp: &str) {
    fold(out, "BEGIN:VTODO");
    let uid = t.uid.trim();
    if uid.is_empty() {
        text(out, "UID", &format!("mailcraft-task-{}@mailcraft.local", t.id));
    } else {
        text(out, "UID", uid);
    }
    fold(out, &format!("DTSTAMP:{stamp}"));
    text(out, "SUMMARY", &t.title);
    if !t.notes.is_empty() {
        text(out, "DESCRIPTION", &t.notes);
    }
    if let Some(start) = t.start {
        fold(out, &format!("DTSTART;VALUE=DATE:{}", format_date(start)));
    }
    if let Some(due) = t.due {
        fold(out, &format!("DUE;VALUE=DATE:{}", format_date(due)));
    }
    match t.completed {
        Some(at) => {
            fold(out, &format!("COMPLETED:{}", format_utc(at)));
            fold(out, "PERCENT-COMPLETE:100");
            fold(out, "STATUS:COMPLETED");
        }
        None => fold(out, "STATUS:NEEDS-ACTION"),
    }
    match t.importance {
        Importance::High => fold(out, "PRIORITY:1"),
        Importance::Low => fold(out, "PRIORITY:9"),
        Importance::Normal => {}
    }
    categories(out, &t.categories);
    if let Some(at) = t.reminder {
        fold(out, "BEGIN:VALARM");
        fold(out, "ACTION:DISPLAY");
        fold(out, "DESCRIPTION:Reminder");
        fold(out, &format!("TRIGGER;VALUE=DATE-TIME:{}", format_utc(at)));
        fold(out, "END:VALARM");
    }
    fold(out, "END:VTODO");
}

#[cfg(test)]
mod tests {
    use mailcraft_model::{Attendee, Freq, Recurrence};
    use mailcraft_time::{Date, Weekday};

    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    fn at(y: i32, m: u8, day: u8, h: u8, min: u8) -> DateTime {
        DateTime::new(d(y, m, day), h, min, 0).unwrap()
    }

    fn options() -> WriteOptions {
        WriteOptions { method: String::new(), now: at(2026, 10, 11, 12, 0).to_timestamp(0) }
    }

    /// The unfolded lines of what `write` gives for one event.
    fn event_lines(e: &Event) -> Vec<String> {
        crate::lines::Lines::new(&write(&[e], &[], &options())).collect()
    }

    fn task_lines(t: &Task) -> Vec<String> {
        crate::lines::Lines::new(&write(&[], &[t], &options())).collect()
    }

    fn meeting() -> Event {
        Event {
            id: 42,
            calendar: 3,
            uid: "planning-2026@example.org".into(),
            title: "Quarterly planning; budget, hiring".into(),
            location: "Room 4.12, North Building".into(),
            description: "Agenda:\n1. Budget\n2. Hiring \\ open roles".into(),
            start: at(2026, 10, 12, 9, 0),
            end: at(2026, 10, 12, 10, 30),
            organizer: Some(Address::new("Okafor, Nadia", "nadia.okafor@example.org")),
            attendees: vec![
                Attendee { address: Address::new("Tomas Lindqvist", "tomas.lindqvist@example.org"), optional: false, response: Response::None },
                Attendee { address: Address::new("Mei \"MT\" Tanaka", "mei.tanaka@example.org"), optional: true, response: Response::Tentative },
                Attendee { address: Address::new("", "ibrahim@example.org"), optional: false, response: Response::Declined },
                Attendee { address: Address::new("Zoë: ops", "zoe@example.org"), optional: false, response: Response::Accepted },
                Attendee::default(),
            ],
            recurrence: Some(Recurrence { interval: 2, by_day: vec![Weekday::Mon, Weekday::Wed], count: Some(8), ..Recurrence::every(Freq::Weekly) }),
            exceptions: vec![d(2026, 10, 26)],
            reminder_minutes: Some(15),
            show_as: ShowAs::OutOfOffice,
            categories: vec!["Work".into(), " Planning, 2026 ".into(), "  ".into()],
            private: true,
            cancelled: true,
            ..Default::default()
        }
    }

    #[test]
    fn writes_a_whole_event() {
        assert_eq!(
            event_lines(&meeting()),
            [
                "BEGIN:VCALENDAR",
                "VERSION:2.0",
                "PRODID:-//MailCraft//MailCraft//EN",
                "BEGIN:VEVENT",
                "UID:planning-2026@example.org",
                "DTSTAMP:20261011T120000Z",
                "DTSTART:20261012T090000",
                "DTEND:20261012T103000",
                "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=8",
                "EXDATE:20261026T090000",
                "SUMMARY:Quarterly planning\\; budget\\, hiring",
                "LOCATION:Room 4.12\\, North Building",
                "DESCRIPTION:Agenda:\\n1. Budget\\n2. Hiring \\\\ open roles",
                "CATEGORIES:Work,Planning\\, 2026",
                "CLASS:PRIVATE",
                "STATUS:CANCELLED",
                "TRANSP:OPAQUE",
                "X-MICROSOFT-CDO-BUSYSTATUS:OOF",
                "ORGANIZER;CN=\"Okafor, Nadia\":mailto:nadia.okafor@example.org",
                "ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN=Tomas Lindqvist:mailto:tomas.lindqvist@example.org",
                "ATTENDEE;ROLE=OPT-PARTICIPANT;PARTSTAT=TENTATIVE;CN=Mei ^'MT^' Tanaka:mailto:mei.tanaka@example.org",
                "ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=DECLINED:mailto:ibrahim@example.org",
                "ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=\"Zoë: ops\":mailto:zoe@example.org",
                "BEGIN:VALARM",
                "ACTION:DISPLAY",
                "DESCRIPTION:Reminder",
                "TRIGGER:-PT15M",
                "END:VALARM",
                "END:VEVENT",
                "END:VCALENDAR",
            ]
        );
    }

    #[test]
    fn lines_end_in_crlf_and_stay_within_75_octets() {
        let mut e = meeting();
        e.description = "Ünïcödé — 東京 🎉 ".repeat(60);
        let out = write(&[&e], &[], &options());
        assert!(out.ends_with("END:VCALENDAR\r\n"));
        assert!(!out.replace("\r\n", "").contains(['\r', '\n']));
        assert!(out.split("\r\n").all(|line| line.len() <= 75), "{out}");
        assert!(out.contains("\r\n "));
    }

    #[test]
    fn a_plain_event_writes_only_what_it_has() {
        let e = Event { id: 7, start: at(2026, 10, 12, 9, 0), end: at(2026, 10, 12, 9, 0), ..Default::default() };
        assert_eq!(
            event_lines(&e).get(3..12).unwrap(),
            [
                "BEGIN:VEVENT",
                "UID:mailcraft-7@mailcraft.local",
                "DTSTAMP:20261011T120000Z",
                "DTSTART:20261012T090000",
                "DTEND:20261012T090000",
                "SUMMARY:",
                "TRANSP:OPAQUE",
                "X-MICROSOFT-CDO-BUSYSTATUS:BUSY",
                "END:VEVENT"
            ]
        );
        let free = Event { show_as: ShowAs::Free, uid: "   ".into(), ..e.clone() };
        let lines = event_lines(&free);
        assert!(lines.contains(&"TRANSP:TRANSPARENT".to_string()) && lines.contains(&"X-MICROSOFT-CDO-BUSYSTATUS:FREE".to_string()));
        assert!(lines.contains(&"UID:mailcraft-7@mailcraft.local".to_string()));
        // An organizer with nothing in it isn't one.
        let blank = Event { organizer: Some(Address::default()), ..e };
        assert!(!event_lines(&blank).iter().any(|l| l.starts_with("ORGANIZER")));
    }

    #[test]
    fn all_day_events_are_dates() {
        let e = Event {
            all_day: true,
            start: at(2026, 10, 12, 0, 0),
            end: at(2026, 10, 15, 0, 0),
            recurrence: Some(Recurrence { until: Some(d(2030, 10, 12)), ..Recurrence::every(Freq::Yearly) }),
            exceptions: vec![d(2027, 10, 12), d(2028, 10, 12)],
            ..Default::default()
        };
        let lines = event_lines(&e);
        for want in [
            "DTSTART;VALUE=DATE:20261012",
            "DTEND;VALUE=DATE:20261015",
            "RRULE:FREQ=YEARLY;UNTIL=20301012",
            "EXDATE;VALUE=DATE:20271012",
            "EXDATE;VALUE=DATE:20281012",
        ] {
            assert!(lines.contains(&want.to_string()), "{want} in {lines:?}");
        }
        // An end that isn't after the start, or has a time left on it, still gives whole days.
        let same_day = Event { end: at(2026, 10, 12, 0, 0), recurrence: None, ..e.clone() };
        assert!(event_lines(&same_day).contains(&"DTEND;VALUE=DATE:20261013".to_string()));
        let with_time = Event { end: at(2026, 10, 13, 17, 0), ..e.clone() };
        assert!(event_lines(&with_time).contains(&"DTEND;VALUE=DATE:20261014".to_string()));
        let backwards = Event { end: at(2026, 10, 1, 0, 0), ..e };
        assert!(event_lines(&backwards).contains(&"DTEND;VALUE=DATE:20261013".to_string()));
    }

    #[test]
    fn a_timed_series_ends_with_its_last_day_and_spells_out_its_weekday() {
        let e = Event {
            start: at(2026, 10, 13, 9, 30),
            end: at(2026, 10, 13, 8, 0),
            recurrence: Some(Recurrence { nth: Some(2), until: Some(d(2027, 6, 30)), ..Recurrence::every(Freq::Monthly) }),
            ..Default::default()
        };
        let lines = event_lines(&e);
        assert!(lines.contains(&"RRULE:FREQ=MONTHLY;BYDAY=2TU;UNTIL=20270630T235959".to_string()), "{lines:?}");
        // An end before the start is the start.
        assert!(lines.contains(&"DTEND:20261013T093000".to_string()));
    }

    #[test]
    fn the_method_is_a_clean_token() {
        let with = |method: &str| write(&[], &[], &WriteOptions { method: method.into(), now: 0 });
        assert_eq!(with("request"), "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//MailCraft//MailCraft//EN\r\nMETHOD:REQUEST\r\nEND:VCALENDAR\r\n");
        assert!(with(" REPLY\r\nBEGIN:VEVENT ").contains("METHOD:REPLYBEGINVEVENT\r\n"));
        assert!(!with("").contains("METHOD"));
        assert!(!with(" \r\n:;").contains("METHOD"));
    }

    #[test]
    fn text_cannot_inject_lines() {
        let e = Event {
            uid: "id\r\nATTENDEE:mailto:evil@example.org".into(),
            title: "Hello\r\nEND:VEVENT\r\nBEGIN:VEVENT".into(),
            location: "x\rEND:VCALENDAR".into(),
            organizer: Some(Address { name: "A\r\nX-INJECTED:1".into(), email: "a@example.org\r\nX-INJECTED:2".into() }),
            categories: vec!["a\nb".into()],
            ..Default::default()
        };
        let lines = event_lines(&e);
        assert_eq!(lines.iter().filter(|l| l.starts_with("BEGIN:") || l.starts_with("END:")).count(), 4, "{lines:?}");
        assert!(!lines.iter().any(|l| l.starts_with("ATTENDEE") || l.starts_with("X-INJECTED")));
        assert!(lines.contains(&"SUMMARY:Hello\\nEND:VEVENT\\nBEGIN:VEVENT".to_string()));
        assert!(lines.contains(&"ORGANIZER;CN=\"A^nX-INJECTED:1\":mailto:a@example.orgX-INJECTED:2".to_string()), "{lines:?}");
    }

    #[test]
    fn hostile_dates_are_written_as_real_ones() {
        let hostile = DateTime { date: Date { year: -5, month: 200, day: 0 }, hour: 77, minute: 88, second: 99 };
        let e = Event {
            start: hostile,
            end: hostile,
            all_day: true,
            exceptions: vec![Date { year: i32::MAX, month: 0, day: 255 }],
            recurrence: Some(Recurrence { until: Some(hostile.date), ..Default::default() }),
            ..Default::default()
        };
        let back = crate::parse(&write(&[&e, &Event { all_day: false, ..e.clone() }], &[], &options()), &|_| 0);
        assert_eq!(back.events.len(), 2);
        assert!(back.events.iter().all(|e| e.exceptions.len() == 1 && e.recurrence.as_ref().is_some_and(|r| r.until.is_some())));
        let t =
            Task { start: Some(hostile.date), due: Some(hostile.date), completed: Some(i64::MIN), reminder: Some(i64::MAX), ..Default::default() };
        let back = crate::parse(&write(&[], &[&t], &options()), &|_| 0);
        assert!(
            back.tasks[0].start.is_some() && back.tasks[0].due.is_some() && back.tasks[0].completed.is_some() && back.tasks[0].reminder.is_some()
        );
    }

    #[test]
    fn writes_tasks() {
        let done = Task {
            id: 9,
            title: "Send the budget draft".into(),
            notes: "Numbers first; then the summary.\nAsk Mei.".into(),
            start: Some(d(2026, 10, 10)),
            due: Some(d(2026, 10, 16)),
            completed: Some(at(2026, 10, 15, 17, 45).to_timestamp(0)),
            importance: Importance::High,
            categories: vec!["Work".into(), "Finance".into()],
            reminder: Some(at(2026, 10, 15, 16, 0).to_timestamp(0)),
            message: Some(77),
            uid: "task-1@example.org".into(),
            ..Default::default()
        };
        assert_eq!(
            task_lines(&done).get(3..19).unwrap(),
            [
                "BEGIN:VTODO",
                "UID:task-1@example.org",
                "DTSTAMP:20261011T120000Z",
                "SUMMARY:Send the budget draft",
                "DESCRIPTION:Numbers first\\; then the summary.\\nAsk Mei.",
                "DTSTART;VALUE=DATE:20261010",
                "DUE;VALUE=DATE:20261016",
                "COMPLETED:20261015T174500Z",
                "PERCENT-COMPLETE:100",
                "STATUS:COMPLETED",
                "PRIORITY:1",
                "CATEGORIES:Work,Finance",
                "BEGIN:VALARM",
                "ACTION:DISPLAY",
                "DESCRIPTION:Reminder",
                "TRIGGER;VALUE=DATE-TIME:20261015T160000Z",
            ]
        );
        let plain = Task { id: 5, importance: Importance::Low, ..Default::default() };
        assert_eq!(
            task_lines(&plain).get(3..9).unwrap(),
            ["BEGIN:VTODO", "UID:mailcraft-task-5@mailcraft.local", "DTSTAMP:20261011T120000Z", "SUMMARY:", "STATUS:NEEDS-ACTION", "PRIORITY:9"]
        );
    }

    #[test]
    fn events_come_before_tasks_in_one_calendar() {
        let (e, t) = (meeting(), Task { title: "t".into(), ..Default::default() });
        let out = write(&[&e, &e], &[&t], &WriteOptions { method: "PUBLISH".into(), now: 0 });
        assert_eq!(out.matches("BEGIN:VCALENDAR").count(), 1);
        assert_eq!((out.matches("BEGIN:VEVENT").count(), out.matches("BEGIN:VTODO").count()), (2, 1));
        assert!(out.find("BEGIN:VTODO").unwrap() > out.rfind("END:VEVENT").unwrap());
        assert!(out.contains("DTSTAMP:19700101T000000Z"));
    }
}
