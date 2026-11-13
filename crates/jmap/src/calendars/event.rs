// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One JSCalendar event (RFC 8984), as a server sent it for a stretch of a calendar, made into the event the
//! engine takes in.
//!
//! The server was asked to expand what repeats, so every object is one happening: an event of its own, or one
//! occurrence of a series under an id the server made for it. Each becomes a plain event; nothing here knows
//! of rules.
//!
//! Everything is read by name and by type: a property that is not what the standard says it is, is as good
//! as absent. Every map is read for its first few entries only and in the order of its keys, so that the
//! outcome does not depend on the order a server wrote them in; every string is cut before it is kept; a
//! duration is read within bounds, and every sum of times saturates.
//!
//! **When.** The instants the server worked out (`utcStart`, `utcEnd`) when it sent them; else the event's
//! own wall-clock `start` in its `timeZone` (an event that floats, with no zone, is read as UTC) and its
//! `duration`, whose weeks and days count on the wall clock and whose hours, minutes and seconds count in
//! real time (RFC 8984 §1.4.6). An event in a zone nobody knows, with no instant from the server, has no time
//! that can be told: it is not read. An event shown without a time is a whole-day one: from midnight (UTC) of
//! the day its own `start` names to midnight of the day after its last.

use mailcraft_model::remote::{RemoteEvent, RemoteEventKind};
use mailcraft_model::{Address, Attendee, Response, ShowAs};
use mailcraft_time::{Date, DateTime, Timestamp, Zone};
use serde_json::{Map, Value};

use crate::reader::cut;

/// Longest title, place, name or id of an event kept, in bytes: the model cuts again.
const MAX_LINE: usize = 1_024;
/// Longest description kept.
const MAX_TEXT: usize = 32 << 10;
/// Longest name of a category kept.
const MAX_KEYWORD: usize = 255;
/// Longest address of a participant, and longest `mailto:` address it is read from.
const MAX_ADDRESS: usize = 320;
const MAX_URI: usize = 1_024;
/// Longest text that is read as a time, and as a duration.
const MAX_TIME: usize = 40;
const MAX_DURATION: usize = 32;
/// Most entries of a map that are looked at (participants, alerts, locations, keywords), and most
/// categories kept.
const MAX_SCAN: usize = 200;
const MAX_KEYWORDS: usize = 50;
/// The most days and the most seconds a duration may say: more is no duration.
const MAX_DAYS: u64 = 40_000;
const MAX_SECONDS: u64 = MAX_DAYS * DAY_U;
const DAY_U: u64 = 86_400;
const DAY: i64 = 86_400;
/// The instants an event may be at: the years 1 to 9999.
const FIRST: Timestamp = -62_135_596_800;
const LAST: Timestamp = 253_402_300_799;

/// What an object of an answer came to.
pub(crate) enum Mapped {
    /// An event.
    Event(Box<RemoteEvent>),
    /// What is no event (a task, a group), or no object: answered for, and nothing to keep.
    Other,
    /// An event whose time can not be told.
    Unreadable,
}

/// A duration (RFC 8984 §1.4.6 and §1.4.7): its sign, its weeks and days as days, and the rest as seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) negative: bool,
    pub(crate) days: i64,
    pub(crate) seconds: i64,
}

impl Span {
    /// All of it in seconds, a day taken for 24 hours; without the sign.
    pub(crate) fn total(self) -> i64 {
        self.days.saturating_mul(DAY).saturating_add(self.seconds)
    }
}

/// The duration `text` says: `P` and then weeks, days and, after a `T`, hours, minutes and seconds, each a
/// number and its letter, in that order and each once at the most (`PT1H30M`, `P1D`, `-PT15M`, `P1W2DT3H`),
/// behind a sign or none. A fraction of a second is passed over. None for anything else, and for more than
/// [`MAX_DAYS`] days in all.
pub(crate) fn duration(text: &str) -> Option<Span> {
    if text.len() > MAX_DURATION {
        return None;
    }
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let rest = rest.strip_prefix('P')?;
    let (mut days, mut seconds) = (0u64, 0u64);
    let mut number: Option<u64> = None;
    // After the `T`; after the point of a fraction; the place of the last unit read; whether there was one.
    let (mut time, mut fraction, mut rank, mut any) = (false, false, 0u8, false);
    for byte in rest.bytes() {
        match byte {
            b'0'..=b'9' if fraction => {}
            b'0'..=b'9' => number = Some(number.unwrap_or(0).checked_mul(10)?.checked_add(u64::from(byte.wrapping_sub(b'0')))?),
            b'T' if !time && number.is_none() => time = true,
            b'.' if time && number.is_some() && !fraction => fraction = true,
            unit => {
                let count = number.take()?;
                let (place, in_days, in_seconds) = match (unit, time) {
                    (b'W', false) => (1u8, 7u64, 0u64),
                    (b'D', false) => (2, 1, 0),
                    (b'H', true) => (3, 0, 3_600),
                    (b'M', true) => (4, 0, 60),
                    (b'S', true) => (5, 0, 1),
                    _ => return None,
                };
                if place <= rank || (fraction && unit != b'S') {
                    return None;
                }
                (rank, fraction, any) = (place, false, true);
                days = days.checked_add(count.checked_mul(in_days)?)?;
                seconds = seconds.checked_add(count.checked_mul(in_seconds)?)?;
            }
        }
    }
    if number.is_some() || fraction || !any || days > MAX_DAYS || seconds > MAX_SECONDS {
        return None;
    }
    Some(Span { negative, days: i64::try_from(days).ok()?, seconds: i64::try_from(seconds).ok()? })
}

/// The object `value`, which the server calls `id`, as an event of the account whose address is `owner`.
pub(crate) fn event(id: &str, value: &Value, owner: &str) -> Mapped {
    let Some(event) = value.as_object() else { return Mapped::Other };
    if event.get("@type").is_some_and(|kind| kind.as_str() != Some("Event")) {
        return Mapped::Other;
    }
    let Some((start, end, all_day)) = times(event) else { return Mapped::Unreadable };
    let zone = event
        .get("timeZone")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|zone| zone.len() <= mailcraft_model::zoned::MAX_ZONE)
        .unwrap_or_default();
    let mut out = RemoteEvent {
        id: id.to_string(),
        uid: line(event.get("uid")),
        title: line(event.get("title")),
        location: location(event),
        description: event.get("description").and_then(Value::as_str).map(|text| cut(text, MAX_TEXT).to_string()).unwrap_or_default(),
        start,
        end,
        all_day,
        start_zone: zone.to_string(),
        end_zone: zone.to_string(),
        show_as: show_as(event),
        private: matches!(event.get("privacy").and_then(Value::as_str), Some("private" | "secret")),
        cancelled: event.get("status").and_then(Value::as_str) == Some("cancelled"),
        kind: RemoteEventKind::Single,
        reminder_minutes: reminder(event),
        modified: event.get("updated").and_then(instant).unwrap_or(0),
        categories: categories(event),
        ..RemoteEvent::default()
    };
    people(event, owner, &mut out);
    Mapped::Event(Box::new(out))
}

/// A property that is text, cut to a line's length; empty when it is anything else.
fn line(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).map(|text| cut(text, MAX_LINE).to_string()).unwrap_or_default()
}

/// The entries of a map of objects (an `Id[...]` of the standard) in the order of their keys. Of a map with
/// more than [`MAX_SCAN`] entries the rest is not looked at.
fn entries(value: Option<&Value>) -> Vec<(&str, &Map<String, Value>)> {
    let Some(map) = value.and_then(Value::as_object) else { return Vec::new() };
    let mut list: Vec<(&str, &Map<String, Value>)> =
        map.iter().take(MAX_SCAN).filter_map(|(key, entry)| entry.as_object().map(|entry| (key.as_str(), entry))).collect();
    list.sort_by(|a, b| a.0.cmp(b.0));
    list
}

/// Whether the set `name` of an entry (a participant's `roles`) has `member`.
fn has(entry: &Map<String, Value>, name: &str, member: &str) -> bool {
    entry.get(name).and_then(|set| set.get(member)).and_then(Value::as_bool) == Some(true)
}

/// The instant a text names (a `UTCDateTime` of the standard); None for what is no text, or no time, or a
/// time outside the years that are counted.
fn instant(value: &Value) -> Option<Timestamp> {
    let text = value.as_str().filter(|text| text.len() <= MAX_TIME)?;
    mailcraft_time::iso_to_timestamp(text, 0).filter(|at| (FIRST..=LAST).contains(at))
}

/// When the event starts and ends, and whether it is a whole-day one (see the module documentation). None
/// when its start can not be told.
fn times(event: &Map<String, Value>) -> Option<(Timestamp, Timestamp, bool)> {
    let local = event.get("start").and_then(Value::as_str).filter(|text| text.len() <= MAX_TIME).and_then(mailcraft_time::parse_iso);
    let span = event.get("duration").and_then(Value::as_str).and_then(duration).filter(|span| !span.negative).unwrap_or_default();
    let utc_start = event.get("utcStart").and_then(instant);
    let utc_end = event.get("utcEnd").and_then(instant);
    if event.get("showWithoutTime").and_then(Value::as_bool) == Some(true) {
        // The days as the event itself names them, not as they fall in UTC.
        let day: Date = local.map(|(wall, _)| wall.date).or_else(|| utc_start.map(|at| DateTime::from_timestamp(at, 0).date))?;
        let start = DateTime::at_midnight(day).civil_seconds();
        // Every day it touches: a day and a half is two.
        let days = span.total().saturating_add(DAY.saturating_sub(1)).checked_div(DAY).unwrap_or(1).max(1);
        let end = start.saturating_add(days.saturating_mul(DAY));
        return within(start, end).map(|(start, end)| (start, end, true));
    }
    // The event's own clock: a wall-clock time that names an offset is an instant already; one in a zone that
    // is known is read there; one with no zone floats, and is read as UTC.
    let zone = match event.get("timeZone") {
        None | Some(Value::Null) => Some(Zone::UTC),
        Some(name) => name.as_str().and_then(Zone::named),
    };
    let own = |wall: DateTime, offset: Option<i32>| match offset {
        Some(minutes) => Some(wall.civil_seconds().saturating_sub(i64::from(minutes).saturating_mul(60))),
        None => zone.map(|zone| zone.instant(wall)),
    };
    let start = utc_start.or_else(|| local.and_then(|(wall, offset)| own(wall, offset)))?;
    let end = utc_end
        // Weeks and days on the wall clock, the rest in real time.
        .or_else(|| {
            let (wall, offset) = local?;
            let later = DateTime::from_civil_seconds(wall.civil_seconds().saturating_add(span.days.saturating_mul(DAY)));
            own(later, offset).map(|at| at.saturating_add(span.seconds))
        })
        .unwrap_or_else(|| start.saturating_add(span.total()));
    within(start, end).map(|(start, end)| (start, end, false))
}

/// The two instants when they are ones an event may be at, the end no earlier than the start.
fn within(start: Timestamp, end: Timestamp) -> Option<(Timestamp, Timestamp)> {
    ((FIRST..=LAST).contains(&start) && (FIRST..=LAST).contains(&end)).then_some((start, end.max(start)))
}

/// The place: of the event's locations in the order of their keys, the first that has a name.
fn location(event: &Map<String, Value>) -> String {
    entries(event.get("locations")).into_iter().map(|(_, place)| line(place.get("name"))).find(|name| !name.is_empty()).unwrap_or_default()
}

/// How the time shows to others: free when the event says it takes none, tentative when it is not settled,
/// busy otherwise.
fn show_as(event: &Map<String, Value>) -> ShowAs {
    if event.get("freeBusyStatus").and_then(Value::as_str) == Some("free") {
        ShowAs::Free
    } else if event.get("status").and_then(Value::as_str) == Some("tentative") {
        ShowAs::Tentative
    } else {
        ShowAs::Busy
    }
}

/// The categories: the keywords that are set, in the order of their names, [`MAX_KEYWORDS`] at the most.
fn categories(event: &Map<String, Value>) -> Vec<String> {
    let Some(keywords) = event.get("keywords").and_then(Value::as_object) else { return Vec::new() };
    let mut out: Vec<String> = keywords
        .iter()
        .take(MAX_SCAN)
        .filter(|(_, set)| set.as_bool() == Some(true))
        .map(|(keyword, _)| cut(keyword, MAX_KEYWORD).to_string())
        .filter(|keyword| !keyword.is_empty())
        .collect();
    out.sort_unstable();
    out.dedup();
    out.truncate(MAX_KEYWORDS);
    out
}

/// How many minutes before the start the event reminds: of its alerts in the order of their keys, the first
/// that goes off a time before the start, or at it.
fn reminder(event: &Map<String, Value>) -> Option<u32> {
    entries(event.get("alerts")).into_iter().find_map(|(_, alert)| {
        let trigger = alert.get("trigger")?.as_object()?;
        if !matches!(trigger.get("@type").map(Value::as_str), None | Some(Some("OffsetTrigger"))) {
            return None;
        }
        if !matches!(trigger.get("relativeTo").map(Value::as_str), None | Some(Some("start"))) {
            return None;
        }
        let offset = trigger.get("offset").and_then(Value::as_str).and_then(duration)?;
        let seconds = offset.total();
        (offset.negative || seconds == 0).then(|| u32::try_from(seconds.checked_div(60).unwrap_or(0)).unwrap_or(u32::MAX))
    })
}

/// A mail address as a participant's is kept: one word with an `@` inside it, of a length an address has.
fn address(text: &str) -> Option<String> {
    let text = text.trim();
    let inside = text.find('@').is_some_and(|at| at > 0 && at.saturating_add(1) < text.len());
    let plain = !text.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';' | '"'));
    (inside && plain && text.len() <= MAX_ADDRESS).then(|| text.to_string())
}

/// The mail address a `mailto:` address names (RFC 6068), without what follows a `?`.
fn mailto(uri: &str) -> Option<String> {
    let uri = uri.trim();
    if uri.len() > MAX_URI {
        return None;
    }
    let rest = uri.get(..7).filter(|scheme| scheme.eq_ignore_ascii_case("mailto:")).and_then(|_| uri.get(7..))?;
    let to = rest.split('?').next().unwrap_or_default();
    if to.contains('%') { address(&crate::percent::decode(to).ok()?) } else { address(to) }
}

/// A participant's mail address: where invitations to them go by mail, else the address the event has for
/// them. Empty when there is none.
fn mail_of(participant: &Map<String, Value>) -> String {
    let text = |name: &str| participant.get(name).and_then(Value::as_str);
    let by_mail = participant.get("sendTo").and_then(|ways| ways.get("imip")).and_then(Value::as_str);
    by_mail.and_then(mailto).or_else(|| text("calendarAddress").and_then(mailto)).or_else(|| text("email").and_then(address)).unwrap_or_default()
}

/// What a participant answered.
fn answer(participant: &Map<String, Value>) -> Response {
    match participant.get("participationStatus").and_then(Value::as_str) {
        Some("accepted") => Response::Accepted,
        Some("declined") => Response::Declined,
        Some("tentative") => Response::Tentative,
        // `needs-action`, `delegated`, and nothing said.
        _ => Response::None,
    }
}

/// The people of the event, in the order of their keys: the first whose role is `owner` is its organizer,
/// those whose role is `attendee` are invited (the organizer is not listed among them again), and the one
/// whose address is the account's says what the account's owner answered. Whether the account's owner
/// organizes it is told only when the event names an organizer by an address.
fn people(event: &Map<String, Value>, owner: &str, out: &mut RemoteEvent) {
    let owner = owner.trim();
    let mut named = false;
    for (_, participant) in entries(event.get("participants")) {
        let person = Address { name: line(participant.get("name")), email: mail_of(participant) };
        let own = !owner.is_empty() && person.email.eq_ignore_ascii_case(owner);
        if own {
            out.response = answer(participant);
        }
        if has(participant, "roles", "owner") && !named {
            named = true;
            if !person.email.is_empty() {
                out.is_organizer = Some(own);
            }
            if !person.email.is_empty() || !person.name.is_empty() {
                out.organizer = Some(person);
            }
        } else if has(participant, "roles", "attendee") && !person.email.is_empty() {
            out.attendees.push(Attendee { address: person, optional: has(participant, "roles", "optional"), response: answer(participant) });
        }
    }
}
