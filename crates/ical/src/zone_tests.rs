// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Time zones through the whole crate: a time in any zone lands at its instant wherever it is
//! read, what is written in zones reads back the same, and no zone, however odd, panics or
//! silently moves an event.

use mailcraft_model::{Event, Freq, Recurrence};
use mailcraft_time::{Date, DateTime, Timestamp, Weekday, Zone};
use proptest::prelude::*;

use crate::{IcsFile, WriteOptions, parse, write, write_in};

fn d(y: i32, m: u8, day: u8) -> Date {
    Date::new(y, m, day).unwrap()
}

fn at(y: i32, m: u8, day: u8, h: u8, min: u8) -> DateTime {
    DateTime::new(d(y, m, day), h, min, 0).unwrap()
}

fn zone(name: &str) -> Zone {
    Zone::named(name).unwrap()
}

/// Read `text` where the calendar is shown in `zone`.
fn read_in(text: &str, zone: Zone) -> IcsFile {
    parse(text, &|instant| zone.offset_at(instant))
}

/// One event from the properties of a `VEVENT`, read in `zone`.
fn event_in(properties: &str, zone: Zone) -> Event {
    let file = read_in(&format!("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n{properties}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"), zone);
    assert_eq!(file.events.len(), 1, "{properties}");
    assert!(file.notes.is_empty(), "{:?}", file.notes);
    file.events.into_iter().next().unwrap()
}

fn options() -> WriteOptions {
    WriteOptions { method: "REQUEST".into(), now: 1_760_000_000 }
}

#[test]
fn an_invitation_from_another_zone_lands_at_the_right_time_here() {
    let chicago = zone("America/Chicago");
    // By the names the big programs write: IANA, Windows', with a prefix, in quotes.
    for (tzid, written, here) in [
        ("Europe/Berlin", "20261012T160000", at(2026, 10, 12, 9, 0)),
        ("W. Europe Standard Time", "20261012T160000", at(2026, 10, 12, 9, 0)),
        ("\"W. Europe Standard Time\"", "20261012T160000", at(2026, 10, 12, 9, 0)),
        ("/example.org/20250101_1/Europe/Berlin", "20261012T160000", at(2026, 10, 12, 9, 0)),
        // The week Europe has put its clocks back and America has not.
        ("Europe/Berlin", "20261027T160000", at(2026, 10, 27, 10, 0)),
        // The south, in its summer and in its winter; and across the date line.
        ("Australia/Sydney", "20260115T090000", at(2026, 1, 14, 16, 0)),
        ("AUS Eastern Standard Time", "20260715T090000", at(2026, 7, 14, 18, 0)),
        ("Pacific/Kiritimati", "20261012T000000", at(2026, 10, 11, 5, 0)),
        // Half and three quarters of an hour.
        ("Asia/Kolkata", "20261012T193000", at(2026, 10, 12, 9, 0)),
        ("India Standard Time", "20261012T193000", at(2026, 10, 12, 9, 0)),
        ("Asia/Kathmandu", "20261012T194500", at(2026, 10, 12, 9, 0)),
        ("Australia/Eucla", "20261012T224500", at(2026, 10, 12, 9, 0)),
        // Its own zone: as written.
        ("America/Chicago", "20261012T090000", at(2026, 10, 12, 9, 0)),
        ("Central Standard Time", "20261012T090000", at(2026, 10, 12, 9, 0)),
        // An offset is a zone too.
        ("\"UTC+05:30\"", "20261012T193000", at(2026, 10, 12, 9, 0)),
    ] {
        let e = event_in(&format!("DTSTART;TZID={tzid}:{written}\r\nDURATION:PT1H"), chicago);
        assert_eq!((e.start, e.end), (here, here.add_minutes(60)), "{tzid} {written}");
        // It says where it was made, unless that is where it is read.
        let own = Zone::named(tzid).unwrap();
        assert_eq!(e.start_zone, if own.same_clock(chicago, 1_790_000_000) { String::new() } else { own.id() }, "{tzid}");
    }
    // An end in another zone: a flight.
    let flight = event_in("DTSTART;TZID=America/Chicago:20261012T110000\r\nDTEND;TZID=Asia/Tokyo:20261013T140000", chicago);
    assert_eq!((flight.start, flight.end), (at(2026, 10, 12, 11, 0), at(2026, 10, 13, 0, 0)));
    assert_eq!((flight.start_zone.as_str(), flight.end_zone.as_str()), ("America/Chicago", "Asia/Tokyo"));
    // A long time ago, and a long time from now: the rule of the day.
    let old = event_in("DTSTART;TZID=America/New_York:20060320T090000", zone("Europe/London"));
    assert_eq!(old.start, at(2006, 3, 20, 14, 0));
    let new = event_in("DTSTART;TZID=America/New_York:20070320T090000", zone("Europe/London"));
    assert_eq!(new.start, at(2007, 3, 20, 13, 0));
    let far = event_in("DTSTART;TZID=America/New_York:23000704T090000", zone("Asia/Kolkata"));
    assert_eq!(far.start, at(2300, 7, 4, 18, 30));
    let past = event_in("DTSTART;TZID=Europe/Berlin:18500601T120000", Zone::UTC);
    assert_eq!(past.start.iso(), "1850-06-01T11:06:32");
}

/// The zone as a mail program defines it in an invitation: today's rule only, from 1601.
const BERLIN: &str = "BEGIN:VTIMEZONE\r\nTZID:W. Europe Standard Time\r\nBEGIN:STANDARD\r\nDTSTART:16010101T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=10\r\nEND:STANDARD\r\nBEGIN:DAYLIGHT\r\nDTSTART:16010101T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=3\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\n";

#[test]
fn a_series_from_another_zone_keeps_its_clock_there() {
    let chicago = zone("America/Chicago");
    let here = |instant: Timestamp| chicago.offset_at(instant);
    // Mondays at four in the afternoon in Berlin, with one skipped and one moved, until the
    // end of November.
    let text = format!(
        "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\n{BERLIN}BEGIN:VEVENT\r\nUID:weekly@example.org\r\nSUMMARY:Weekly\r\n\
DTSTART;TZID=W. Europe Standard Time:20261005T160000\r\nDTEND;TZID=W. Europe Standard Time:20261005T170000\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T150000Z\r\nEXDATE;TZID=W. Europe Standard Time:20261019T160000\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:weekly@example.org\r\nRECURRENCE-ID;TZID=W. Europe Standard Time:20261109T160000\r\nSUMMARY:Weekly (moved)\r\n\
DTSTART;TZID=W. Europe Standard Time:20261110T100000\r\nDTEND;TZID=W. Europe Standard Time:20261110T110000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let file = read_in(&text, chicago);
    assert!(file.notes.is_empty(), "{:?}", file.notes);
    let series = &file.events[0];
    assert_eq!((series.start, series.end, series.start_zone.as_str()), (at(2026, 10, 5, 9, 0), at(2026, 10, 5, 10, 0), "Europe/Berlin"));
    assert_eq!(series.recurrence, Some(Recurrence { by_day: vec![Weekday::Mon], until: Some(d(2026, 11, 30)), ..Recurrence::every(Freq::Weekly) }));
    assert_eq!(series.exceptions, [d(2026, 10, 19), d(2026, 11, 9)]);
    // Nine o'clock here, but for the week when only Berlin has changed its clocks.
    let seen: Vec<String> = series.occurrences_in(d(2026, 10, 1), d(2026, 12, 31), &here).into_iter().map(|o| o.start.iso()).collect();
    assert_eq!(
        seen,
        [
            "2026-10-05T09:00:00",
            "2026-10-12T09:00:00",
            "2026-10-26T10:00:00",
            "2026-11-02T09:00:00",
            "2026-11-16T09:00:00",
            "2026-11-23T09:00:00",
            "2026-11-30T09:00:00"
        ]
    );
    let moved = &file.events[1];
    assert_eq!((moved.start, moved.start_zone.as_str(), moved.recurrence.is_none()), (at(2026, 11, 10, 3, 0), "Europe/Berlin", true));

    // Late in the evening in Tokyo is the morning of the same day here; early in the morning
    // there is the day before here, and the days are named as they fall here.
    let late = event_in("DTSTART;TZID=Asia/Tokyo:20261005T080000\r\nRRULE:FREQ=WEEKLY;BYDAY=MO\r\nEXDATE;TZID=Asia/Tokyo:20261012T080000", chicago);
    assert_eq!((late.start, late.start_zone.as_str()), (at(2026, 10, 4, 18, 0), "Asia/Tokyo"));
    assert_eq!(late.recurrence.as_ref().map(|rule| rule.by_day.clone()), Some(vec![Weekday::Mon]));
    assert_eq!(late.exceptions, [d(2026, 10, 11)]);
    let seen: Vec<String> = late.occurrences_in(d(2026, 10, 1), d(2026, 11, 10), &here).into_iter().map(|o| o.start.iso()).collect();
    assert_eq!(seen, ["2026-10-04T18:00:00", "2026-10-18T18:00:00", "2026-10-25T18:00:00", "2026-11-01T17:00:00", "2026-11-08T17:00:00"]);

    // Read in Berlin itself, it is at four, and still says by whose clock it repeats (the
    // changed occurrence, which happens once where the clock is the same, has nothing to say).
    let home = read_in(&text, zone("Europe/Berlin"));
    assert_eq!((home.events[0].start, home.events[0].start_zone.as_str()), (at(2026, 10, 5, 16, 0), "Europe/Berlin"));
    assert_eq!(home.events[0].exceptions, [d(2026, 10, 19), d(2026, 11, 9)]);
    assert_eq!((home.events[1].start, home.events[1].start_zone.as_str()), (at(2026, 11, 10, 10, 0), ""));
}

#[test]
fn zones_nobody_knows_are_said_and_nothing_is_guessed() {
    let chicago = zone("America/Chicago");
    let read = |properties: &str| read_in(&format!("BEGIN:VEVENT\r\n{properties}\r\nEND:VEVENT\r\n"), chicago);
    // Nothing is known: as written, and it says so.
    let file = read("DTSTART;TZID=Customized Time Zone:20261012T160000\r\nDTEND;TZID=Customized Time Zone:20261012T170000\r\nRRULE:FREQ=DAILY");
    assert_eq!(
        (file.events[0].start, file.events[0].end, file.events[0].start_zone.as_str()),
        (at(2026, 10, 12, 16, 0), at(2026, 10, 12, 17, 0), "")
    );
    assert_eq!(file.notes, ["The time zone \"Customized Time Zone\" isn't one that is known: its times are kept as they are written."]);
    // The name gives an offset: kept at that offset, which is said.
    let file = read("DTSTART;TZID=\"(UTC+01:00) Somewhere in Europe\":20261012T160000\r\nRRULE:FREQ=DAILY");
    assert_eq!((file.events[0].start, file.events[0].start_zone.as_str()), (at(2026, 10, 12, 10, 0), "UTC+01:00"));
    assert_eq!(file.notes.len(), 1);
    assert!(file.notes[0].contains("its times are kept at UTC+01:00"), "{:?}", file.notes);
    // A hundred thousand letters of name, control characters in it, a name that is only quotes.
    for tzid in ["Z".repeat(100_000), "\"A\u{7}B\"".into(), "\"\"".into(), "Europe/Berlin/".repeat(1000), "🕰".repeat(5_000)] {
        let file = read(&format!("DTSTART;TZID={tzid}:20261012T160000"));
        assert_eq!(file.events[0].start, at(2026, 10, 12, 16, 0), "{}", tzid.len());
        assert!(file.notes.len() <= 1 && file.notes.iter().all(|note| note.len() < 400 && !note.contains('\u{7}')));
    }
    // A megabyte of it is more than a line may hold: there is no start left to read, and so no
    // event; what follows is read.
    let huge = format!(
        "BEGIN:VEVENT\r\nDTSTART;TZID={}:20261012T160000\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART:20261013T160000\r\nEND:VEVENT\r\n",
        "Z".repeat(1 << 20)
    );
    let file = read_in(&huge, chicago);
    assert_eq!(file.events.iter().map(|e| e.start).collect::<Vec<_>>(), [at(2026, 10, 13, 16, 0)]);
    // A definition that contradicts its name: the file's own times, and a word about it.
    let wrong = BERLIN.replace("+0200", "-0700").replace("+0100", "-0800");
    let file = read_in(&format!("{wrong}BEGIN:VEVENT\r\nDTSTART;TZID=W. Europe Standard Time:20261012T160000\r\nEND:VEVENT\r\n"), chicago);
    assert_eq!(file.events[0].start, at(2026, 10, 12, 18, 0));
    assert!(file.notes[0].contains("is defined in the file otherwise than the zone of that name"), "{:?}", file.notes);
    // Offsets of days are no definition: the name is all there is, and it is a good one.
    let days = BERLIN.replace("+0200", "+9900").replace("+0100", "-9900");
    let file = read_in(&format!("{days}BEGIN:VEVENT\r\nDTSTART;TZID=W. Europe Standard Time:20261012T160000\r\nEND:VEVENT\r\n"), chicago);
    assert_eq!((file.events[0].start, file.notes.len()), (at(2026, 10, 12, 9, 0), 0));
    // Many zones nobody knows: a few notes, not thousands.
    let many: String = (0..500).map(|n| format!("BEGIN:VEVENT\r\nDTSTART;TZID=Nowhere {n}:20261012T160000\r\nEND:VEVENT\r\n")).collect();
    let file = read_in(&many, chicago);
    assert_eq!((file.events.len(), file.notes.len()), (500, 20));
}

#[test]
fn whole_days_are_days_in_every_zone() {
    for name in ["Pacific/Kiritimati", "Pacific/Pago_Pago", "Europe/Berlin"] {
        let e = event_in("DTSTART;VALUE=DATE:20261012\r\nDTEND;VALUE=DATE:20261014", zone(name));
        assert_eq!((e.all_day, e.start, e.end, e.start_zone.as_str()), (true, at(2026, 10, 12, 0, 0), at(2026, 10, 14, 0, 0), ""), "{name}");
        // Midnight to midnight in a zone, with the flag that says it is whole days.
        let flagged = event_in(
            "DTSTART;TZID=Asia/Tokyo:20261012T000000\r\nDTEND;TZID=Asia/Tokyo:20261013T000000\r\nX-MICROSOFT-CDO-ALLDAYEVENT:TRUE",
            zone(name),
        );
        assert_eq!(
            (flagged.all_day, flagged.start, flagged.end, flagged.start_zone.as_str()),
            (true, at(2026, 10, 12, 0, 0), at(2026, 10, 13, 0, 0), ""),
            "{name}"
        );
    }
}

#[test]
fn what_is_written_in_zones_is_what_other_programs_read() {
    let chicago = zone("America/Chicago");
    let meeting = Event { id: 7, title: "Review".into(), start: at(2026, 10, 12, 9, 0), end: at(2026, 10, 12, 10, 0), ..Default::default() };
    let series = Event {
        id: 8,
        uid: "weekly@example.org".into(),
        start: at(2026, 10, 5, 9, 0),
        end: at(2026, 10, 5, 10, 0),
        start_zone: "Europe/Berlin".into(),
        recurrence: Some(Recurrence { by_day: vec![Weekday::Mon], until: Some(d(2026, 11, 30)), ..Recurrence::every(Freq::Weekly) }),
        exceptions: vec![d(2026, 10, 19)],
        ..Default::default()
    };
    let utc = Event { id: 9, start_zone: "UTC".into(), ..meeting.clone() };
    let whole = Event { id: 10, all_day: true, start: at(2026, 10, 12, 0, 0), end: at(2026, 10, 13, 0, 0), ..Default::default() };
    let text = write_in(&[&meeting, &series, &utc, &whole], &[], &options(), Some(chicago));
    let lines: Vec<&str> = text.split("\r\n").collect();
    for want in [
        // Each zone is defined once, before the events.
        "TZID:America/Chicago",
        "TZID:Europe/Berlin",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU",
        "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU",
        "DTSTART;TZID=America/Chicago:20261012T090000",
        "DTEND;TZID=America/Chicago:20261012T100000",
        // The series by its own clock, its end as an instant, its skipped day as a time there.
        "DTSTART;TZID=Europe/Berlin:20261005T160000",
        "DTEND;TZID=Europe/Berlin:20261005T170000",
        "RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T225959Z",
        "EXDATE;TZID=Europe/Berlin:20261019T160000",
        "DTSTART:20261012T140000Z",
        "DTSTART;VALUE=DATE:20261012",
    ] {
        assert!(lines.contains(&want), "{want} in\n{text}");
    }
    assert_eq!(text.matches("BEGIN:VTIMEZONE").count(), 2);
    assert!(text.find("END:VTIMEZONE").unwrap() < text.find("BEGIN:VEVENT").unwrap());
    // Read back here: the same events. Read elsewhere: the same instants.
    let uid = |e: &Event| if e.uid.is_empty() { format!("mailcraft-{}@mailcraft.local", e.id) } else { e.uid.clone() };
    let here = read_in(&text, chicago);
    let want: Vec<Event> = [&meeting, &series, &utc, &whole].into_iter().map(|e| Event { id: 0, uid: uid(e), ..e.clone() }).collect();
    // (An event that happens once doesn't keep UTC as its zone: it is how an instant is written.)
    let want: Vec<Event> = want.into_iter().map(|e| if e.start_zone == "UTC" { Event { start_zone: String::new(), ..e } } else { e }).collect();
    assert_eq!(here.events, want);
    let tokyo = zone("Asia/Tokyo");
    let there = read_in(&text, tokyo);
    assert_eq!(tokyo.instant(there.events[0].start), chicago.instant(meeting.start));
    assert_eq!((there.events[0].start_zone.as_str(), there.events[1].start_zone.as_str()), ("America/Chicago", "Europe/Berlin"));
    assert_eq!(there.events[1].exceptions, [d(2026, 10, 19)]);
    assert_eq!(there.events[3], want[3]);
    // Without a zone to write in, the times are as they are (and no zone is defined).
    let floating = write(&[&meeting], &[], &options());
    assert!(floating.contains("DTSTART:20261012T090000\r\n") && !floating.contains("VTIMEZONE"));
    assert_eq!(write_in(&[&meeting], &[], &options(), None), floating);
    assert_eq!(write_in(&[&meeting], &[], &options(), Some(Zone::LOCAL)), floating);
}

#[test]
fn what_the_clock_cannot_say_is_written_exactly() {
    // Berlin puts its clocks back on October 25, 2026 at 3:00: 2:30 comes twice. Seen from
    // Chicago those are 19:30 and 20:30 on the evening before.
    let (chicago, berlin) = (zone("America/Chicago"), zone("Europe/Berlin"));
    let event = |start: DateTime, minutes: i64| Event {
        uid: "u".into(),
        start,
        end: start.add_minutes(minutes),
        start_zone: "Europe/Berlin".into(),
        ..Default::default()
    };
    // It ends in the second 2:30: the end is written as how long it lasts.
    let ends = event(at(2026, 10, 24, 19, 0), 90);
    let text = write_in(&[&ends], &[], &options(), Some(chicago));
    assert!(text.contains("DTSTART;TZID=Europe/Berlin:20261025T020000\r\nDURATION:PT5400S\r\n"), "{text}");
    assert_eq!(read_in(&text, chicago).events, std::slice::from_ref(&ends));
    // It starts in the second 2:30: in UTC, which keeps when it is (and not where).
    let starts = event(at(2026, 10, 24, 20, 30), 60);
    let text = write_in(&[&starts], &[], &options(), Some(chicago));
    assert!(text.contains("DTSTART:20261025T013000Z\r\nDTEND:20261025T023000Z\r\n"), "{text}");
    let back = read_in(&text, chicago).events.remove(0);
    assert_eq!((back.start, back.end, back.start_zone.as_str()), (starts.start, starts.end, ""));
    // The first 2:30 is 2:30.
    let first = event(at(2026, 10, 24, 19, 30), 30);
    let text = write_in(&[&first], &[], &options(), Some(chicago));
    assert!(text.contains("DTSTART;TZID=Europe/Berlin:20261025T023000\r\n"), "{text}");
    assert_eq!(read_in(&text, chicago).events, [first]);
    // A time the clocks skip (2:30 on March 29, 2026 in Berlin) is read as RFC 5545 says: as
    // much after the change as it is written after it, which is 3:30.
    let skipped = event_in("DTSTART;TZID=Europe/Berlin:20260329T023000", Zone::UTC);
    assert_eq!(skipped.start, at(2026, 3, 29, 1, 30));
    assert_eq!(berlin.wall(Zone::UTC.instant(skipped.start)), at(2026, 3, 29, 3, 30));
}

fn any_zone() -> impl Strategy<Value = Zone> {
    (0usize..200).prop_map(|index| {
        let choices = Zone::choices();
        choices[index % choices.len()]
    })
}

fn rule() -> impl Strategy<Value = Option<Recurrence>> {
    let freq = proptest::sample::select(vec![Freq::Daily, Freq::Weekly, Freq::Monthly, Freq::Yearly]);
    let until = proptest::option::of(0i64..800);
    proptest::option::of((freq, 1u32..4, until, proptest::option::of(1u32..30)))
        .prop_map(|rule| rule.map(|(freq, interval, until, count)| (Recurrence { freq, interval, count, ..Default::default() }, until)))
        .prop_map(|rule| {
            rule.map(|(rule, until)| Recurrence {
                until: until.map(|days| d(2026, 1, 1).add_days(days)),
                count: rule.count.filter(|_| until.is_none()),
                ..rule
            })
        })
}

proptest! {
    /// Written in zones and read back where it was written: the same event. Read anywhere
    /// else: the same instants, the same days of the series.
    #[test]
    fn events_in_zones_round_trip(
        here in any_zone(), own in proptest::option::of(any_zone()), elsewhere in any_zone(),
        minutes in 0i64..40 * 366 * 24 * 60, length in 0i64..3 * 24 * 60, rule in rule(), skipped in proptest::collection::vec(0usize..12, 0..3),
    ) {
        let offset_at = |instant: Timestamp| here.offset_at(instant);
        // A time the clocks of `here` show, once.
        let start = here.wall(here.instant(at(2000, 1, 1, 0, 0).add_minutes(minutes)));
        let end = here.wall(here.instant(start) + length * 60);
        prop_assume!(here.wall(here.instant(start)) == start && here.wall(here.instant(end)) == end && end >= start);
        let mut event = Event { uid: "u@example.org".into(), start, end, recurrence: rule, ..Default::default() };
        let instants = (here.instant(start), here.instant(end));
        (event.start_zone, event.end_zone) = mailcraft_model::zoned::own_zones(own, None, instants, event.recurrence.is_some(), &offset_at);
        // (A start in the second of an hour that comes twice in its own zone is written in UTC,
        // which keeps the instant and not the zone: `what_the_clock_cannot_say_is_written_exactly`.)
        let there = event.zone().unwrap_or(here);
        prop_assume!(there.instant(there.wall(instants.0)) == instants.0);
        // Some of its own days are skipped.
        let days = event.series_days(start.date.add_days(-2), start.date.add_days(4000), &offset_at);
        event.exceptions = skipped.iter().filter_map(|n| days.get(*n).copied()).collect();
        event.exceptions.sort();
        event.exceptions.dedup();

        let text = write_in(&[&event], &[], &options(), Some(here));
        let back = read_in(&text, here);
        prop_assert!(back.notes.is_empty(), "{:?}\n{}", back.notes, text);
        // (A series that had no zone of its own comes back with the one it was written in.)
        let pinned = event.recurrence.is_some() && event.start_zone.is_empty();
        let want = Event { start_zone: if pinned { here.id() } else { event.start_zone.clone() }, ..event.clone() };
        prop_assert_eq!(&back.events, &vec![want], "{}", text);

        let there = read_in(&text, elsewhere);
        let other = |instant: Timestamp| elsewhere.offset_at(instant);
        let seen = &there.events[0];
        // The same instants, as the clock there shows them (also where it shows a time twice).
        prop_assert_eq!((seen.start, seen.end), (elsewhere.wall(instants.0), elsewhere.wall(instants.1)), "{}", text);
        prop_assert_eq!(seen.instants(&other), instants, "{}", text);
        // What happens in the next two years happens at the same instants there. (The edges
        // of a stretch of days are other instants in another zone: they are left out.)
        let (from, to) = (start.date.add_days(-2), start.date.add_days(730));
        let (first, last) = (DateTime::at_midnight(from).to_timestamp(0) + 3 * 86_400, DateTime::at_midnight(to).to_timestamp(0) - 3 * 86_400);
        let when = |e: &Event, offset_at: &dyn Fn(Timestamp) -> i32| -> Vec<Timestamp> {
            let all = e.occurrences_in(from.add_days(-2), to.add_days(2), offset_at).into_iter().map(|o| e.occurrence_instant(o.day, offset_at));
            all.filter(|instant| (first..last).contains(instant)).collect()
        };
        if event.recurrence.is_some() {
            prop_assert_eq!(when(&event, &offset_at), when(seen, &other), "{}", text);
        }
    }

    #[test]
    fn any_tzid_and_any_definition_are_survived(tzid in any::<String>(), offsets in proptest::collection::vec("[-+][0-9]{4,6}", 2), junk in "\\PC{0,40}", zone in any_zone()) {
        let text = format!(
            "BEGIN:VTIMEZONE\nTZID:{tzid}\nBEGIN:STANDARD\nDTSTART:19700101T000000\nTZOFFSETFROM:{}\nTZOFFSETTO:{}\nRRULE:{junk}\nEND:STANDARD\nEND:VTIMEZONE\n\
BEGIN:VEVENT\nDTSTART;TZID={tzid}:20261012T090000\nDTEND;TZID={junk}:20261012T100000\nRRULE:FREQ=DAILY;UNTIL=20261231T000000Z\nEXDATE;TZID={tzid}:20261013T090000\nEND:VEVENT\n",
            offsets[0], offsets[1]
        );
        let file = read_in(&text, zone);
        for event in &file.events {
            let _ = event.occurrences_in(d(2026, 1, 1), d(2027, 1, 1), &|instant| zone.offset_at(instant));
            // Whatever was made of the zone, the event is writable and reads back as itself.
            let again = read_in(&write_in(&[event], &[], &options(), Some(zone)), zone);
            prop_assert_eq!(again.events.len(), 1);
            prop_assert_eq!(again.events[0].instants(&|instant| zone.offset_at(instant)), event.instants(&|instant| zone.offset_at(instant)));
        }
    }
}

/// Found by `events_in_zones_round_trip`: a daily series in America/Nuuk that ends on
/// 27 March 2027, the day the clocks there spring from 23:00 to midnight. Its `UNTIL` was
/// "23:59:59 on the 27th", a time the clocks skip, which is an instant of the 28th: read back,
/// the series ended a day later. It is the day's last second, wherever the calendar is shown.
#[test]
fn a_series_that_ends_on_the_day_the_clocks_skip_its_last_hour_ends_that_day() {
    let nuuk = zone("America/Nuuk");
    for here in Zone::choices().iter().copied() {
        let offset_at = |instant: Timestamp| here.offset_at(instant);
        let start = here.wall(here.instant(at(2000, 1, 1, 0, 0)));
        let rule = Recurrence { until: Some(d(2027, 3, 27)), ..Recurrence::every(Freq::Daily) };
        let mut event = Event { uid: "u@example.org".into(), start, end: start, recurrence: Some(rule), ..Default::default() };
        let instants = (here.instant(start), here.instant(start));
        (event.start_zone, event.end_zone) = mailcraft_model::zoned::own_zones(Some(nuuk), None, instants, true, &offset_at);
        assert_eq!(event.start_zone, "America/Nuuk");
        let text = write_in(&[&event], &[], &options(), Some(here));
        // 23:00 on the 27th at -02:00 is when the 28th begins: a second before that.
        assert!(text.contains("UNTIL=20270328T005959Z"), "{}\n{text}", here.id());
        let back = read_in(&text, here);
        assert!(back.notes.is_empty(), "{:?}", back.notes);
        assert_eq!(back.events, vec![event.clone()], "{}\n{text}", here.id());
        // The 27th's occurrence is still the last.
        let last = |e: &Event| e.series_days(d(2027, 3, 20), d(2027, 4, 5), &offset_at).last().copied();
        assert_eq!(last(&back.events[0]), last(&event), "{}", here.id());
    }
}

/// Found by `events_in_zones_round_trip`: a series at midnight in Pacific/Apia, shown in a
/// calendar at Etc/GMT+12, every two months on the 30th, skipping the occurrence of
/// 30 December 2011. Apia skipped that day (its clocks went from the end of the 29th at -10:00
/// to the 31st at +14:00), so the occurrence the rule names "midnight on the 30th" is at the
/// instant the clock there shows as midnight on the 31st. Its `EXDATE` was written as the 31st
/// and read back as the occurrence of the 31st: a skipped day that wasn't the one skipped. It
/// is written and read as the rule names it.
#[test]
fn a_skipped_occurrence_on_a_day_the_zone_skipped_is_that_occurrence() {
    let here = zone("Etc/GMT+12");
    let offset_at = |instant: Timestamp| here.offset_at(instant);
    let start = at(2010, 6, 29, 23, 0);
    let rule = Recurrence { interval: 2, ..Recurrence::every(Freq::Monthly) };
    let mut event = Event { uid: "u@example.org".into(), start, end: start, recurrence: Some(rule), ..Default::default() };
    let instants = (here.instant(start), here.instant(start));
    (event.start_zone, event.end_zone) = mailcraft_model::zoned::own_zones(Some(zone("Pacific/Apia")), None, instants, true, &offset_at);
    event.exceptions = vec![d(2011, 12, 29)];
    let text = write_in(&[&event], &[], &options(), Some(here));
    assert!(text.contains("EXDATE;TZID=Pacific/Apia:20111230T000000\r\n"), "{text}");
    let back = read_in(&text, here);
    assert!(back.notes.is_empty(), "{:?}", back.notes);
    assert_eq!(back.events, vec![event.clone()], "{text}");
    // Another program (and MailCraft before) writes the time the clock shows: the same instant,
    // and the day the rule names at it is the one skipped.
    let shown = text.replace("EXDATE;TZID=Pacific/Apia:20111230T000000", "EXDATE;TZID=Pacific/Apia:20111231T000000");
    assert_eq!(read_in(&shown, here).events, vec![event.clone()], "{shown}");
    // A daily series names both the 30th and the 31st there, at the same instant: the time the
    // clock shows is the 31st's occurrence, which is the 30th here.
    let daily = Event { recurrence: Some(Recurrence::every(Freq::Daily)), ..event.clone() };
    let shown = write_in(&[&daily], &[], &options(), Some(here))
        .replace("EXDATE;TZID=Pacific/Apia:20111230T000000", "EXDATE;TZID=Pacific/Apia:20111231T000000");
    assert_eq!(read_in(&shown, here).events[0].exceptions, [d(2011, 12, 30)], "{shown}");
    // America/Nuuk springs from 23:00 to midnight on the last Saturday of March: a weekly series
    // at 23:30 on Saturdays names 23:30 on 27 March 2027, which the clock shows as 0:30 on the
    // 28th. Either way it is the 27th that is skipped.
    let nuuk = zone("America/Nuuk");
    let at_nuuk = |instant: Timestamp| nuuk.offset_at(instant);
    let start = at(2027, 3, 6, 23, 30);
    let rule = Recurrence { by_day: vec![Weekday::Sat], ..Recurrence::every(Freq::Weekly) };
    let mut weekly =
        Event { uid: "u@example.org".into(), start, end: start, recurrence: Some(rule), exceptions: vec![d(2027, 3, 27)], ..Default::default() };
    let instants = (nuuk.instant(start), nuuk.instant(start));
    (weekly.start_zone, weekly.end_zone) = mailcraft_model::zoned::own_zones(Some(nuuk), None, instants, true, &at_nuuk);
    let text = write_in(&[&weekly], &[], &options(), Some(nuuk));
    assert!(text.contains("EXDATE;TZID=America/Nuuk:20270327T233000\r\n"), "{text}");
    assert_eq!(read_in(&text, nuuk).events, vec![weekly.clone()], "{text}");
    let shown = text.replace("EXDATE;TZID=America/Nuuk:20270327T233000", "EXDATE;TZID=America/Nuuk:20270328T003000");
    assert_eq!(read_in(&shown, nuuk).events, vec![weekly], "{shown}");
}

/// The same for the zones that skipped a day when they moved across the date line, and for
/// zones whose clocks skip the hour before or after midnight, for series of each kind at a time
/// the clocks skip that skip each of their days around the change, wherever the calendar is
/// shown.
#[test]
fn series_across_a_skipped_day_skip_their_own_days() {
    // The zone, the day the clocks skip a time, and an hour of the day they skip.
    for (own, change, skipped) in [
        ("Pacific/Apia", d(2011, 12, 30), 0),
        ("Pacific/Fakaofo", d(2011, 12, 30), 0),
        ("Pacific/Kiritimati", d(1994, 12, 31), 0),
        ("Pacific/Kanton", d(1994, 12, 31), 0),
        ("Pacific/Kwajalein", d(1993, 8, 21), 0),
        ("America/Nuuk", d(2027, 3, 27), 23),
        ("America/Santiago", d(2026, 9, 6), 0),
        ("America/Havana", d(2026, 3, 8), 0),
    ] {
        let own = zone(own);
        for here in Zone::choices() {
            let offset_at = |instant: Timestamp| here.offset_at(instant);
            for (freq, hour) in [(Freq::Daily, skipped), (Freq::Daily, 9), (Freq::Weekly, skipped), (Freq::Monthly, skipped)] {
                // A series by the clock of its zone that begins some weeks before the change and
                // has an occurrence on the day of it.
                let begins = if freq == Freq::Monthly { change.add_months(-1) } else { change.add_days(-28) };
                if begins.day != change.day {
                    continue;
                }
                let first = DateTime::new(begins, hour, 30, 0).unwrap();
                let start = here.wall(own.instant(first));
                if here.wall(here.instant(start)) != start || own.instant(own.wall(here.instant(start))) != here.instant(start) {
                    continue;
                }
                let rule = Recurrence { count: Some(70), ..Recurrence::every(freq) };
                let mut event =
                    Event { uid: "u@example.org".into(), start, end: start.add_minutes(30), recurrence: Some(rule), ..Default::default() };
                let instants = (here.instant(start), here.instant(event.end));
                (event.start_zone, event.end_zone) = mailcraft_model::zoned::own_zones(Some(own), None, instants, true, &offset_at);
                event.exceptions = event.series_days(change.add_days(-5), change.add_days(5), &offset_at);
                let text = write_in(&[&event], &[], &options(), Some(here));
                let back = read_in(&text, here);
                assert!(back.notes.is_empty(), "{} {:?}", here.id(), back.notes);
                assert_eq!(back.events, vec![event.clone()], "{} in {}\n{text}", own.id(), here.id());
            }
        }
    }
}
