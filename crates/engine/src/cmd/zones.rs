// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Time zones: the zone the calendar is shown in (and a second one beside it), the zones an
//! event has of its own, and the list of zones to choose from.
//!
//! How times are kept is in `mailcraft_model::zoned`. In short: an event's `start` and `end` are
//! wall-clock times by the clock of the zone the calendar is shown in ([`Session::zone`]), which
//! the mailbox names; [`Session::settle_zone`] rewrites them when that zone changes. An event may
//! name a zone of its own for its start and for its end (`startZone`, `endZone` of
//! `calendar.new` and `calendar.edit`): it is still shown by the calendar's clock, says where it
//! is at home, and when it repeats, repeats by its own clock.

use mailcraft_model::Event;
use mailcraft_model::zoned::{instant_at, own_zones};
use mailcraft_time::{DateTime, Timestamp, Zone};
use serde_json::{Value, json};

use super::{bad, fail, remote};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Most zones `calendar.timeZones` lists, and most the owner's latest choices are remembered.
const MAX_LISTED: usize = 700;
const MAX_RECENT: usize = 6;
/// Longest label a zone is given in the time bar.
const MAX_LABEL: usize = 24;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("calendar.timeZones", "Time Zones", "Appointment › Options", time_zones)
            .pure()
            .params(r#"{"query"?: string, "limit"?: number, "id"?: number, "used"?: zone} — the time zones to choose from, the computer's own first, then the ones chosen lately, then west to east: {"zones": [{"id" (the name to give `startZone`, `endZone` and `view.timeZone`), "label", "place", "offset": "UTC-05:00" (now), "minutes", "windows"?: what Windows and Microsoft 365 call it}], "system", "shown" (the zone the calendar is shown in), "recent": [id]}; "query" narrows the list to the zones called like that (any zone of the IANA database can be found by name); with "id": that event's own zones too ({"event": {"startZone", "endZone", "ownTime"?: its times by its own clock}}); "used" notes a zone as chosen lately. An event's zones are set with `calendar.edit`"#),
        CommandSpec::new("view.timeZone", "Time Zone", "File › Options", time_zone)
            .pure()
            .params(r#"{"zone"?: zone | null (null: this computer's own, followed when it changes), "label"?: string, "second"?: zone | null (a second zone in the calendar's time bar; null: none), "secondLabel"?: string} — the time zone the calendar is shown in. Every timed event stays at its instant and is shown by the new clock; whole-day events stay on their days. Answers {"zone", "label", "follows": bool, "offset", "second": zone | null, "secondLabel", "system"}"#),
    ]
}

/// A zone as a command's answer names it.
fn zone_json(zone: Zone, now: Timestamp) -> Value {
    json!({
        "id": zone.id(), "label": zone.label(), "place": zone.place(), "offset": zone.offset_label(now),
        "minutes": zone.offset_at(now), "windows": zone.windows_name(),
    })
}

/// The zone a parameter names: `None` when it isn't given, `Some(None)` for null or an empty
/// name, an error for a name that is no zone.
fn named(v: &Value, key: &str) -> Result<Option<Option<Zone>>, CmdError> {
    match v.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(name)) if name.trim().is_empty() => Ok(Some(None)),
        Some(Value::String(name)) => match Zone::named(name) {
            Some(zone) => Ok(Some(Some(zone))),
            None => {
                let shown: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
                Err(bad(format!("`{key}`: no time zone is called \"{shown}\" (calendar.timeZones lists them)")))
            }
        },
        Some(_) => Err(bad(format!("`{key}` is the name of a time zone, or null"))),
    }
}

/// The zones to choose from in the order a picker shows them: this computer's own, the one the
/// calendar is shown in, the ones chosen lately, then the rest west to east (by where they are
/// now, then by name).
pub fn choices(s: &Session, query: &str, limit: usize) -> Vec<Zone> {
    let now = s.now();
    let mut first: Vec<Zone> = Vec::new();
    for zone in [s.system_zone(), s.zone()].into_iter().chain(s.prefs.recent_zones.iter().take(MAX_RECENT).filter_map(|name| Zone::named(name))) {
        if !zone.is_local() && !first.contains(&zone) {
            first.push(zone);
        }
    }
    let mut rest = Zone::search(query, now, limit.clamp(1, MAX_LISTED));
    if query.trim().is_empty() {
        rest.sort_by_key(|zone| (zone.offset_at(now), zone.label()));
    } else {
        // Of what was asked for, the ones at hand first, in their order.
        first.retain(|zone| rest.contains(zone));
    }
    rest.retain(|zone| !first.contains(zone));
    first.extend(rest);
    first.truncate(limit.clamp(1, MAX_LISTED));
    first
}

/// Note that the owner chose `zone`: it is listed near the top from now on.
pub fn note_used(s: &mut Session, zone: Zone) {
    if zone.is_local() {
        return;
    }
    let id = zone.id();
    s.prefs.recent_zones.retain(|have| *have != id);
    s.prefs.recent_zones.insert(0, id);
    s.prefs.recent_zones.truncate(MAX_RECENT);
}

fn time_zones(s: &mut Session, v: &Value) -> CmdResult {
    if let Some(Some(zone)) = named(v, "used")? {
        note_used(s, zone);
    }
    let now = s.now();
    let query: String = p::str(v, "query").unwrap_or("").chars().take(100).collect();
    let limit = p::u64(v, "limit").map_or(MAX_LISTED, |n| usize::try_from(n).unwrap_or(MAX_LISTED));
    let zones: Vec<Value> = choices(s, &query, limit).into_iter().map(|zone| zone_json(zone, now)).collect();
    let mut answer = json!({
        "zones": zones, "system": s.system_zone().id(), "shown": s.zone().id(),
        "recent": s.prefs.recent_zones.iter().take(MAX_RECENT).collect::<Vec<_>>(),
    });
    if let Some(id) = p::id(v, "id") {
        let event = s.mailbox.event(id).ok_or_else(|| failed!("no event with id {id}", id))?;
        if let Some(answer) = answer.as_object_mut() {
            answer.insert("event".into(), event_zones_json(s, event));
        }
    }
    Ok(answer)
}

/// What an event says of its own zones: their names, and its times by their clock when that
/// shows another time than the calendar's.
pub fn event_zones_json(s: &Session, event: &Event) -> Value {
    let here = remote::offset_of(s);
    json!({
        "startZone": event.zone().map(Zone::id), "endZone": event.zone_of_end().map(Zone::id),
        "ownTime": mailcraft_model::zoned::own_time_label(event, &here, s.prefs.twelve_hour),
    })
}

fn label(v: &Value, key: &str) -> Option<String> {
    p::str(v, key).map(|text| text.chars().filter(|c| !c.is_control()).take(MAX_LABEL).collect::<String>().trim().to_string())
}

fn time_zone(s: &mut Session, v: &Value) -> CmdResult {
    // Everything is read before anything is set: a bad name changes nothing.
    let (zone, second) = (named(v, "zone")?, named(v, "second")?);
    if let Some(zone) = zone {
        s.prefs.time_zone = zone.map(Zone::id).unwrap_or_default();
        if let Some(zone) = zone {
            note_used(s, zone);
        }
    }
    if let Some(second) = second {
        s.prefs.second_zone = second.map(Zone::id).unwrap_or_default();
        if let Some(second) = second {
            note_used(s, second);
        }
    }
    if let Some(text) = label(v, "label") {
        s.prefs.zone_label = text;
    }
    if let Some(text) = label(v, "secondLabel") {
        s.prefs.second_zone_label = text;
    }
    // (The mailbox's times are put by the new clock when the command is done: `Session::run`.)
    Ok(shown_json(s))
}

/// The second zone of the time bar, when the options name one that is known.
pub fn second_zone(s: &Session) -> Option<Zone> {
    Zone::named(&s.prefs.second_zone).filter(|zone| !zone.is_local())
}

/// What the time bar calls a zone: the label the owner gave it, else the zone's own.
pub fn bar_label(zone: Zone, given: &str) -> String {
    let given = given.trim();
    if given.is_empty() { zone.label() } else { given.chars().take(MAX_LABEL).collect() }
}

fn shown_json(s: &Session) -> Value {
    let (zone, now) = (s.zone(), s.now());
    json!({
        "zone": zone.id(), "label": bar_label(zone, &s.prefs.zone_label), "follows": Zone::named(&s.prefs.time_zone).is_none(),
        "offset": zone.offset_label(now), "second": second_zone(s).map(Zone::id), "secondLabel": second_zone(s).map(|zone| bar_label(zone, &s.prefs.second_zone_label)),
        "system": s.system_zone().id(),
    })
}

/// When an event is by the clocks of its own zones, before a command changes it: what
/// `startZone` and `endZone` are applied to ([`Times::apply`]).
pub(crate) struct Times {
    start_zone: Option<Zone>,
    end_zone: Option<Zone>,
    /// The start and the end by the clock of their zones (the calendar's, for one without).
    start: DateTime,
    end: DateTime,
    all_day: bool,
}

impl Times {
    pub(crate) fn of(s: &Session, e: &Event) -> Times {
        let here = remote::offset_of(s);
        let (start_zone, end_zone) = (e.zone(), e.zone_of_end());
        let (start, end) = if e.all_day { (0, 0) } else { e.instants(&here) };
        Times {
            start_zone,
            end_zone,
            start: start_zone.map_or(e.start, |zone| zone.wall(start)),
            end: end_zone.map_or(e.end, |zone| zone.wall(end)),
            all_day: e.all_day,
        }
    }

    /// Give `e` the zones the parameters name, after its times were set from them (`timed`:
    /// whether `start` and `end` were given).
    ///
    /// As in Outlook: a time that is typed together with a zone is a time by that zone's clock;
    /// a zone that is changed without a time keeps the time on the clock and moves the instant
    /// (three o'clock in New York becomes three o'clock in Berlin). The end's zone follows the
    /// start's unless it was another before, or is named itself. A whole-day event has no
    /// zones. A time that is given without a zone is by the calendar's clock, as ever.
    pub(crate) fn apply(&self, s: &Session, e: &mut Event, v: &Value, timed: (bool, bool)) -> Result<(), CmdError> {
        let (start_zone, end_zone) = (named(v, "startZone")?, named(v, "endZone")?);
        if e.all_day {
            (e.start_zone, e.end_zone, e.start_repeated, e.end_repeated) = (String::new(), String::new(), false, false);
            return Ok(());
        }
        // A time that was just given is a time the clock shows once (the first, of two).
        if timed.0 || self.all_day {
            (e.start_repeated, e.end_repeated) = (false, false);
        } else if timed.1 {
            e.end_repeated = false;
        }
        if start_zone.is_none() && end_zone.is_none() {
            return Ok(());
        }
        let here = remote::offset_of(s);
        let starts_in = start_zone.unwrap_or(self.start_zone);
        let ends_in = match end_zone {
            Some(named) => named.or(starts_in),
            // It follows the start's, unless the end had a zone of its own.
            None if self.end_zone == self.start_zone => starts_in,
            None => self.end_zone,
        };
        // The times on the clock: the ones just given, else the ones the event had by its own.
        let fresh = self.all_day;
        let start = if timed.0 || fresh { e.start } else { self.start };
        let end = if timed.1 || fresh {
            e.end
        } else if timed.0 {
            start.add_minutes(e.start.minutes_until(e.end).max(0))
        } else {
            self.end
        };
        let instant = |wall: DateTime, zone: Option<Zone>| zone.map_or_else(|| instant_at(wall, &here), |zone| zone.instant(wall));
        let (begins, ends) = (instant(start, starts_in), instant(end, ends_in));
        if ends < begins {
            return Err(fail("the end is before the start"));
        }
        e.set_instants(begins, ends, &here);
        let repeats = match v.get("recurrence") {
            Some(rule) => !rule.is_null(),
            None => e.recurrence.is_some(),
        };
        (e.start_zone, e.end_zone) = own_zones(starts_in, ends_in, (begins, ends), repeats, &here);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use mailcraft_model::{Freq, Mailbox, Recurrence};
    use mailcraft_time::Date;

    use super::*;

    /// Monday 2026-10-12 15:00 UTC, which is ten in the morning in Chicago.
    const NOW: i64 = 1_791_817_200;

    fn zone(name: &str) -> Zone {
        Zone::named(name).unwrap()
    }

    fn session(name: &str) -> Session {
        Session::in_zone(Mailbox::with_local_account("Ada", "ada@example.org"), NOW, zone(name))
    }

    fn at(y: i32, m: u8, d: u8, h: u8, min: u8) -> DateTime {
        DateTime::new(Date::new(y, m, d).unwrap(), h, min, 0).unwrap()
    }

    fn event(s: &Session, id: u64) -> Event {
        s.mailbox.event(id).unwrap().clone()
    }

    #[test]
    fn the_zones_to_choose_from() {
        let mut s = session("America/Chicago");
        let all = s.run("calendar.timeZones", &json!({})).unwrap();
        let zones = all["zones"].as_array().unwrap();
        assert!(zones.len() > 130);
        // The computer's own first, with where it is now.
        assert_eq!(
            zones[0],
            json!({"id": "America/Chicago", "label": "Central Time", "place": "Chicago", "offset": "UTC-05:00", "minutes": -300, "windows": "Central Standard Time"})
        );
        assert_eq!((all["system"].as_str(), all["shown"].as_str()), (Some("America/Chicago"), Some("America/Chicago")));
        // Then west to east.
        let minutes: Vec<i64> = zones.iter().skip(1).map(|zone| zone["minutes"].as_i64().unwrap()).collect();
        assert!(minutes.windows(2).all(|pair| pair[0] <= pair[1]), "{minutes:?}");
        assert_eq!((minutes.first().copied(), minutes.last().copied()), (Some(-720), Some(840)));
        // By what they are called, any zone of the database.
        let found = s.run("calendar.timeZones", &json!({"query": "kolkata"})).unwrap();
        assert_eq!(found["zones"][0]["id"], "Asia/Kolkata");
        assert_eq!(found["zones"][0]["windows"], "India Standard Time");
        let found = s.run("calendar.timeZones", &json!({"query": "detroit", "limit": 3})).unwrap();
        assert_eq!(found["zones"][0]["id"], "America/Detroit");
        assert_eq!(s.run("calendar.timeZones", &json!({"query": "no such place"})).unwrap()["zones"], json!([]));
        // What was chosen lately comes after the computer's own, the latest first.
        s.run("calendar.timeZones", &json!({"used": "Asia/Tokyo"})).unwrap();
        let all = s.run("calendar.timeZones", &json!({"used": "Europe/Berlin"})).unwrap();
        let ids: Vec<&str> = all["zones"].as_array().unwrap().iter().take(3).map(|zone| zone["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["America/Chicago", "Europe/Berlin", "Asia/Tokyo"]);
        assert_eq!(all["recent"], json!(["Europe/Berlin", "Asia/Tokyo"]));
        // Junk is said to be junk, and changes nothing.
        for params in [json!({"used": "Mars/Phobos"}), json!({"used": 7}), json!({"id": 999_999})] {
            assert!(s.run("calendar.timeZones", &params).is_err(), "{params}");
        }
        let _ = s.run("calendar.timeZones", &json!({"query": "x".repeat(1 << 20), "limit": u64::MAX}));
        assert_eq!(s.prefs.recent_zones, ["Europe/Berlin", "Asia/Tokyo"]);
    }

    #[test]
    fn an_event_in_a_zone_of_its_own() {
        let mut s = session("America/Chicago");
        // Three in the afternoon in New York is two here.
        let id = s
            .run(
                "calendar.new",
                &json!({"title": "Call", "start": "2026-10-13T15:00", "end": "2026-10-13T16:00", "startZone": "Eastern Standard Time"}),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let e = event(&s, id);
        assert_eq!(
            (e.start, e.end, e.start_zone.as_str(), e.end_zone.as_str()),
            (at(2026, 10, 13, 14, 0), at(2026, 10, 13, 15, 0), "America/New_York", "")
        );
        let got = s.run("calendar.get", &json!({"id": id})).unwrap();
        assert_eq!(
            (got["start"].as_str(), got["startZone"].as_str(), got["ownTime"].as_str()),
            (Some("2026-10-13T14:00:00"), Some("America/New_York"), Some("3:00 PM – 4:00 PM (Eastern Time)"))
        );
        // Another zone, and nothing else said: the clock time stays, the instant moves.
        s.run("calendar.edit", &json!({"id": id, "startZone": "Europe/Berlin"})).unwrap();
        let e = event(&s, id);
        assert_eq!((e.start, e.end, e.start_zone.as_str()), (at(2026, 10, 13, 8, 0), at(2026, 10, 13, 9, 0), "Europe/Berlin"));
        // A time without a zone is by the calendar's clock; the zone stays the event's.
        s.run("calendar.edit", &json!({"id": id, "start": "2026-10-13T09:00"})).unwrap();
        let e = event(&s, id);
        assert_eq!((e.start, e.end, e.start_zone.as_str()), (at(2026, 10, 13, 9, 0), at(2026, 10, 13, 10, 0), "Europe/Berlin"));
        // The end in a zone of its own: a flight that lands at half past seven in Tokyo.
        s.run(
            "calendar.edit",
            &json!({"id": id, "start": "2026-10-13T11:00", "startZone": "America/Chicago", "end": "2026-10-14T19:30", "endZone": "Asia/Tokyo"}),
        )
        .unwrap();
        let e = event(&s, id);
        assert_eq!((e.start, e.end), (at(2026, 10, 13, 11, 0), at(2026, 10, 14, 5, 30)));
        assert_eq!((e.start_zone.as_str(), e.end_zone.as_str()), ("America/Chicago", "Asia/Tokyo"));
        // Its start's zone changed: the end keeps its own.
        s.run("calendar.edit", &json!({"id": id, "startZone": "America/Denver"})).unwrap();
        let e = event(&s, id);
        assert_eq!(
            (e.start, e.end, e.start_zone.as_str(), e.end_zone.as_str()),
            (at(2026, 10, 13, 12, 0), at(2026, 10, 14, 5, 30), "America/Denver", "Asia/Tokyo")
        );
        // No zone again: by the calendar's clock, at the same time on the clock.
        s.run("calendar.edit", &json!({"id": id, "startZone": null, "endZone": null, "end": "2026-10-13T13:00"})).unwrap();
        let e = event(&s, id);
        assert_eq!((e.start, e.end, e.start_zone.as_str(), e.end_zone.as_str()), (at(2026, 10, 13, 11, 0), at(2026, 10, 13, 13, 0), "", ""));
        // The calendar's own zone is no zone of its own; a whole-day event has none at all.
        s.run("calendar.edit", &json!({"id": id, "startZone": "Central Standard Time"})).unwrap();
        assert_eq!(event(&s, id).start_zone, "");
        s.run("calendar.edit", &json!({"id": id, "startZone": "Asia/Tokyo"})).unwrap();
        s.run("calendar.edit", &json!({"id": id, "allDay": true})).unwrap();
        let e = event(&s, id);
        assert_eq!((e.all_day, e.start_zone.as_str(), e.start), (true, "", at(2026, 10, 12, 0, 0)));
        assert!(s.run("calendar.new", &json!({"title": "Day", "allDay": true, "startZone": "Asia/Tokyo"})).is_ok());
        // What is no zone is refused, and nothing changes.
        let before = s.mailbox.clone();
        for params in [
            json!({"startZone": "Nowhere/At_All"}),
            json!({"endZone": 5}),
            json!({"startZone": "x".repeat(1 << 20)}),
            json!({"startZone": "Europe/Berlin", "endZone": "UTC+99"}),
        ] {
            let mut params = params;
            params["id"] = json!(id);
            assert!(s.run("calendar.edit", &params).is_err(), "{}", params.to_string().chars().take(80).collect::<String>());
        }
        assert_eq!(s.mailbox, before);
        // An end that would be before the start in its zone is refused.
        let id = s.run("calendar.new", &json!({"title": "Short", "start": "2026-10-13T15:00", "end": "2026-10-13T15:30"})).unwrap()["id"]
            .as_u64()
            .unwrap();
        assert!(s.run("calendar.edit", &json!({"id": id, "endZone": "Asia/Tokyo"})).is_err());
    }

    #[test]
    fn a_series_made_in_another_zone_repeats_by_its_clock() {
        let mut s = session("America/Chicago");
        // Every day at nine in Berlin, from October 20: two in the morning here, three for the
        // week when Berlin has put its clocks back and Chicago has not.
        let made = s.run("calendar.new", &json!({"title": "Stand-up", "start": "2026-10-20T09:00", "end": "2026-10-20T09:15", "startZone": "Europe/Berlin", "recurrence": {"freq": "daily"}}));
        let id = made.unwrap()["id"].as_u64().unwrap();
        let e = event(&s, id);
        assert_eq!(
            (e.start, e.start_zone.as_str(), e.recurrence.clone()),
            (at(2026, 10, 20, 2, 0), "Europe/Berlin", Some(Recurrence::every(Freq::Daily)))
        );
        let listed = s.run("calendar.list", &json!({"from": "2026-10-23", "to": "2026-11-02"})).unwrap();
        let starts: Vec<&str> = listed["events"].as_array().unwrap().iter().map(|e| e["start"].as_str().unwrap()).collect();
        assert_eq!(
            starts,
            [
                "2026-10-23T02:00:00",
                "2026-10-24T02:00:00",
                "2026-10-25T03:00:00",
                "2026-10-26T03:00:00",
                "2026-10-27T03:00:00",
                "2026-10-28T03:00:00",
                "2026-10-29T03:00:00",
                "2026-10-30T03:00:00",
                "2026-10-31T03:00:00",
                "2026-11-01T02:00:00",
                "2026-11-02T02:00:00"
            ]
        );
        // One occurrence deleted, one moved an hour: each on its own day, at its own time.
        s.run("calendar.delete", &json!({"id": id, "occurrence": "2026-10-26"})).unwrap();
        let moved =
            s.run("calendar.edit", &json!({"id": id, "scope": "occurrence", "occurrence": "2026-10-27", "start": "2026-10-20T03:00"})).unwrap();
        let one = event(&s, moved["id"].as_u64().unwrap());
        assert_eq!((one.start, one.end, one.recurrence.is_none()), (at(2026, 10, 27, 4, 0), at(2026, 10, 27, 4, 15), true));
        assert_eq!(event(&s, id).exceptions, [Date::new(2026, 10, 26).unwrap(), Date::new(2026, 10, 27).unwrap()]);
        let listed = s.run("calendar.list", &json!({"from": "2026-10-25", "to": "2026-10-28"})).unwrap();
        let starts: Vec<&str> = listed["events"].as_array().unwrap().iter().map(|e| e["start"].as_str().unwrap()).collect();
        assert_eq!(starts, ["2026-10-25T03:00:00", "2026-10-27T04:00:00", "2026-10-28T03:00:00"]);
    }

    #[test]
    fn the_calendar_is_shown_in_the_zone_the_options_name() {
        let mut s = session("America/Chicago");
        let id = s.run("calendar.new", &json!({"title": "Lunch", "start": "2026-10-13T12:00", "end": "2026-10-13T13:00"})).unwrap()["id"]
            .as_u64()
            .unwrap();
        let day = s.run("calendar.new", &json!({"title": "Holiday", "start": "2026-10-14", "allDay": true})).unwrap()["id"].as_u64().unwrap();
        assert_eq!(s.mailbox.zone, "America/Chicago");
        let shown = s.run("view.timeZone", &json!({})).unwrap();
        assert_eq!(
            shown,
            json!({"zone": "America/Chicago", "label": "Central Time", "follows": true, "offset": "UTC-05:00", "second": null, "secondLabel": null, "system": "America/Chicago"})
        );
        // Shown in Berlin: lunch is at seven in the evening there, the holiday is on its day.
        let shown = s.run("view.timeZone", &json!({"zone": "W. Europe Standard Time", "label": "Office", "second": "Asia/Tokyo"})).unwrap();
        assert_eq!(
            (shown["zone"].as_str(), shown["label"].as_str(), shown["follows"].as_bool()),
            (Some("Europe/Berlin"), Some("Office"), Some(false))
        );
        assert_eq!(
            (shown["second"].as_str(), shown["secondLabel"].as_str(), shown["system"].as_str()),
            (Some("Asia/Tokyo"), Some("Tokyo Time"), Some("America/Chicago"))
        );
        assert_eq!((s.zone(), s.mailbox.zone.as_str()), (zone("Europe/Berlin"), "Europe/Berlin"));
        assert_eq!((event(&s, id).start, event(&s, id).end), (at(2026, 10, 13, 19, 0), at(2026, 10, 13, 20, 0)));
        assert_eq!((event(&s, day).start, event(&s, day).all_day), (at(2026, 10, 14, 0, 0), true));
        // "Today" and "now" are Berlin's.
        assert_eq!(s.local_now(), at(2026, 10, 12, 17, 0));
        // The computer's own again.
        let shown = s.run("view.timeZone", &json!({"zone": null, "second": null, "label": ""})).unwrap();
        assert_eq!((shown["zone"].as_str(), shown["follows"].as_bool(), shown["second"].is_null()), (Some("America/Chicago"), Some(true), true));
        assert_eq!(event(&s, id).start, at(2026, 10, 13, 12, 0));
        // A name that is no zone changes nothing.
        let before = (s.prefs.clone(), s.mailbox.clone());
        for params in [json!({"zone": "Nowhere"}), json!({"zone": "Europe/Berlin", "second": "Nowhere"}), json!({"zone": 12})] {
            assert!(s.run("view.timeZone", &params).is_err(), "{params}");
        }
        assert_eq!((s.prefs.clone(), s.mailbox.clone()), before);
        // The options are kept as the front end saves them, and a file without them reads.
        s.run("view.timeZone", &json!({"zone": "Asia/Kolkata", "second": "UTC"})).unwrap();
        let saved = serde_json::to_value(&s.prefs).unwrap();
        assert_eq!((saved["timeZone"].as_str(), saved["secondZone"].as_str()), (Some("Asia/Kolkata"), Some("UTC")));
        let read: crate::Prefs = serde_json::from_value(saved).unwrap();
        assert_eq!(read, s.prefs);
        assert_eq!(serde_json::from_value::<crate::Prefs>(json!({})).unwrap().time_zone, "");
        // A zone the options name that nobody knows (a file from elsewhere): the computer's own.
        s.prefs.time_zone = "Mars/Olympus_Mons".into();
        assert_eq!(s.zone(), zone("America/Chicago"));
    }
}
