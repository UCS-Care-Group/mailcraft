//! The Scheduling Assistant's arithmetic: when everybody is free over several days, and the next
//! time that suits all of them (Outlook's AutoPick).
//!
//! `calendar.scheduling` looks at the owner's own calendars and, when a host hands in what a
//! server said of the other people (`result`, as `calendar.freeBusy` takes it), at theirs: it
//! suggests the times of the meeting's length, on the half hour, within the working hours of the
//! working days, when nobody is busy, tentative or out of office. Without `result` it says what
//! to ask the server (`ask`) and suggests by the owner's calendar alone. It reads; nothing is
//! written and nobody is told. [`next_free`] is the same search for the window's AutoPick.

use mailcraft_model::{Address, Id, ShowAs};
use mailcraft_time::{DateTime, Weekday};
use serde_json::{Value, json};

use super::super::{bad, remote};
use crate::{CmdResult, CommandSpec, Session, p};

/// The step suggestions are on, the most days looked at, and the most suggestions given.
const STEP: i64 = 30;
const MAX_DAYS: i64 = 14;
const MAX_SUGGESTIONS: usize = 20;

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("calendar.scheduling", "Scheduling Assistant", "Meeting › Show", scheduling).pure().params(
            r#"{"id"?: number (a meeting: its people, length and start; default the selected one), "people"?: "a@x; b@y" | [string] (instead of a meeting's), "from"?: "YYYY-MM-DDTHH:MM" (default: the meeting's start, else now), "days"?: 1-14 (default 7), "minutes"?: number (default: the meeting's length, else 30), "count"?: 1-20 (default 5), "result"?: what a server said of the people (as calendar.freeBusy takes it)} — the next times of that length, on the half hour and within the working hours of the working days, when the owner's calendars and everybody the server said something of are free (not busy, tentative or out of office): {"from", "to", "minutes", "people": [string], "known": [string] (who the server said something of), "suggestions": [{"start", "end"}], "ask": {…}|null (what to ask a server about the people, as calendar.freeBusy says it, when no result was handed in)}. It reads; nothing is written and nobody is told"#,
        ),
    ]
}

/// The working week as the owner has it: the days, and the hours from and to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkWeek {
    pub days: Vec<Weekday>,
    pub start: u8,
    pub end: u8,
}

impl WorkWeek {
    pub fn of(s: &Session) -> WorkWeek {
        WorkWeek { days: s.prefs.work_days.clone(), start: s.prefs.work_start.min(24), end: s.prefs.work_end.min(24) }
    }
}

/// The first time at or after `after`, on the half hour, when a meeting of `minutes` fits within
/// the working hours of a working day of `week` and overlaps none of `taken` (by the calendar's
/// clock), up to `until`. `None` when there is none. A meeting longer than the working day is
/// fitted from the start of a working day. Bounded: at most [`MAX_DAYS`] days are walked.
pub fn next_free(taken: &[(DateTime, DateTime)], after: DateTime, minutes: i64, week: &WorkWeek, until: DateTime) -> Option<DateTime> {
    let minutes = minutes.clamp(1, 14 * 24 * 60);
    let first = {
        let midnight = DateTime::at_midnight(after.date);
        let into = midnight.minutes_until(after).max(0);
        midnight.add_minutes(into.div_euclid(STEP) * STEP + if into % STEP == 0 && after.second == 0 { 0 } else { STEP })
    };
    let last = until.min(after.add_days(MAX_DAYS));
    let (open, close) = (i64::from(week.start.min(week.end)) * 60, i64::from(week.end.max(week.start)) * 60);
    let long = minutes > close - open;
    let mut at = first;
    // (At most fourteen days of half hours.)
    for _ in 0..(MAX_DAYS * 24 * 60 / STEP + 1) {
        if at >= last {
            return None;
        }
        let into = DateTime::at_midnight(at.date).minutes_until(at);
        let end = at.add_minutes(minutes);
        let working = week.days.contains(&at.date.weekday());
        let fits = if long { into == open } else { into >= open && into + minutes <= close };
        if working && fits && !taken.iter().any(|(from, to)| *from < end && *to > at) {
            return Some(at);
        }
        at = at.add_minutes(STEP);
    }
    None
}

/// Whether a way of showing time takes it: busy, tentative, out of office (working elsewhere is
/// working).
fn takes(show_as: ShowAs) -> bool {
    matches!(show_as, ShowAs::Busy | ShowAs::Tentative | ShowAs::OutOfOffice)
}

fn scheduling(s: &mut Session, v: &Value) -> CmdResult {
    // The meeting named, or (when no people are named instead) the one selected.
    let named_people = v.get("people").is_some_and(|people| !people.is_null());
    let meeting = match p::id(v, "id").or(s.sel.event.filter(|_| !named_people)) {
        Some(id) => s.mailbox.event(id).cloned(),
        None => None,
    };
    let named: Vec<String> = match v.get("people") {
        Some(Value::String(line)) => Address::parse_list(line).into_iter().map(|a| a.email).collect(),
        Some(Value::Array(_)) => p::strings(v, "people").unwrap_or_default(),
        _ => meeting.as_ref().map(|e| e.attendees.iter().map(|a| a.address.email.clone()).collect()).unwrap_or_default(),
    };
    let mine = |email: &str| s.mailbox.accounts.iter().any(|a| a.email.trim().eq_ignore_ascii_case(email.trim()));
    let mut people: Vec<String> = Vec::new();
    for email in named.iter().map(|e| e.trim().to_lowercase()) {
        if Address::new("", &email).is_valid() && !mine(&email) && !people.contains(&email) && people.len() < mailcraft_model::remote::MAX_SCHEDULES {
            people.push(email);
        }
    }
    let now = s.local_now();
    let from = p::datetime(v, "from")?.or(meeting.as_ref().map(|e| e.start)).unwrap_or(now);
    let days = p::u64(v, "days").map_or(7, |n| i64::try_from(n).unwrap_or(MAX_DAYS)).clamp(1, MAX_DAYS);
    let length = meeting.as_ref().map(|e| e.duration_minutes()).filter(|m| *m > 0).unwrap_or(30);
    let minutes = p::u64(v, "minutes").map_or(length, |n| i64::try_from(n).unwrap_or(length)).clamp(STEP / 2, 24 * 60);
    let count = p::u64(v, "count").map_or(5, |n| usize::try_from(n).unwrap_or(MAX_SUGGESTIONS)).clamp(1, MAX_SUGGESTIONS);
    let (start, until) = (DateTime::at_midnight(from.date), DateTime::at_midnight(from.date.add_days(days)));
    // The owner's own calendars, without the meeting itself.
    let without: Option<Id> = meeting.as_ref().map(|e| e.id);
    let here = remote::offset_of(s);
    let mut taken: Vec<(DateTime, DateTime)> = s
        .mailbox
        .events
        .values()
        .filter(|e| Some(e.id) != without && !e.cancelled && takes(e.show_as))
        .flat_map(|e| e.occurrences_in(start.date, until.date, &here))
        .take(20_000)
        .map(|o| (o.start, o.end))
        .collect();
    // Everybody else's, as far as a server said.
    let mut known: Vec<String> = Vec::new();
    let mut ask = Value::Null;
    if !people.is_empty() {
        let window = json!({"people": people, "from": start.iso(), "to": until.iso()});
        match v.get("result").filter(|result| !result.is_null()) {
            Some(result) => {
                let mut with = window.clone();
                with["result"] = result.clone();
                let said = super::free_busy(s, &with)?;
                for person in said.get("people").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default().iter().take(people.len()) {
                    if person.get("error").is_some() {
                        continue;
                    }
                    if let Some(email) = person.get("email").and_then(Value::as_str) {
                        known.push(email.to_string());
                    }
                    for time in person.get("busy").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default().iter().take(5_000) {
                        let at = |key: &str| time.get(key).and_then(Value::as_str).and_then(mailcraft_time::parse_iso).map(|(at, _)| at);
                        let show_as = time.get("showAs").and_then(Value::as_str).and_then(ShowAs::parse).unwrap_or(ShowAs::Busy);
                        if let (Some(from), Some(to), true) = (at("start"), at("end"), takes(show_as)) {
                            taken.push((from, to));
                        }
                    }
                }
            }
            None => ask = super::free_busy(s, &window)?.get("ask").cloned().unwrap_or(Value::Null),
        }
    }
    taken.sort();
    let week = WorkWeek::of(s);
    if week.days.is_empty() {
        return Err(bad("there are no working days to suggest a time on (File › Options › Calendar)"));
    }
    let mut suggestions: Vec<Value> = Vec::new();
    let mut after = from;
    while suggestions.len() < count {
        let Some(at) = next_free(&taken, after, minutes, &week, until) else { break };
        suggestions.push(json!({"start": at.iso(), "end": at.add_minutes(minutes).iso()}));
        after = at.add_minutes(STEP);
    }
    Ok(json!({
        "from": start.iso(), "to": until.iso(), "minutes": minutes, "people": people, "known": known,
        "suggestions": suggestions, "ask": ask,
    }))
}

#[cfg(test)]
mod tests {
    use mailcraft_time::Date;

    use super::*;

    fn at(day: u8, hour: u8, minute: u8) -> DateTime {
        DateTime::new(Date::new(2026, 10, day).unwrap(), hour, minute, 0).unwrap()
    }

    fn week() -> WorkWeek {
        WorkWeek { days: vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri], start: 9, end: 17 }
    }

    #[test]
    fn the_next_free_time_is_on_the_half_hour_within_working_hours_and_clear_of_everyone() {
        let until = at(31, 0, 0);
        // Thursday 15th from 10:10: the next half hour.
        assert_eq!(next_free(&[], at(15, 10, 10), 60, &week(), until), Some(at(15, 10, 30)));
        assert_eq!(next_free(&[], at(15, 10, 0), 60, &week(), until), Some(at(15, 10, 0)));
        // Before the working day: its start; after it: the next working day's (Friday), and
        // after Friday's: Monday's.
        assert_eq!(next_free(&[], at(15, 6, 0), 60, &week(), until), Some(at(15, 9, 0)));
        assert_eq!(next_free(&[], at(15, 16, 30), 60, &week(), until), Some(at(16, 9, 0)));
        assert_eq!(next_free(&[], at(16, 16, 30), 60, &week(), until), Some(at(19, 9, 0)));
        // Around what is taken.
        let taken = [(at(15, 9, 0), at(15, 10, 0)), (at(15, 10, 30), at(15, 12, 0))];
        assert_eq!(next_free(&taken, at(15, 9, 0), 60, &week(), until), Some(at(15, 12, 0)));
        assert_eq!(next_free(&taken, at(15, 9, 0), 30, &week(), until), Some(at(15, 10, 0)));
        // Longer than the working day: from the start of one.
        assert_eq!(next_free(&[], at(15, 9, 30), 10 * 60, &week(), until), Some(at(16, 9, 0)));
        // Nothing before `until`, and no working days at all.
        assert_eq!(next_free(&[], at(15, 16, 30), 60, &week(), at(16, 0, 0)), None);
        assert_eq!(next_free(&[], at(15, 9, 0), 60, &WorkWeek { days: Vec::new(), ..week() }, until), None);
        // A step past the day's end of the walk can't run away either.
        assert_eq!(
            next_free(&[], at(31, 23, 59), 60, &week(), DateTime::at_midnight(Date::new(2027, 1, 1).unwrap())),
            Some(DateTime::new(Date::new(2026, 11, 2).unwrap(), 9, 0, 0).unwrap())
        );
        // Everything taken: none, after a bounded walk.
        assert_eq!(next_free(&[(at(1, 0, 0), at(31, 0, 0))], at(1, 0, 0), 30, &week(), DateTime::at_midnight(Date::new(2030, 1, 1).unwrap())), None);
    }

    #[test]
    fn suggestions_take_the_owners_calendar_and_what_the_server_said_of_the_others() {
        let now = at(14, 16, 0).to_timestamp(0);
        let mut s = Session::at(crate::sample::mailbox(now, 0), now, 0);
        // A Monday far from the sample's own appointments, and an appointment of the owner's.
        s.run("calendar.new", &json!({"title": "Dentist", "start": "2026-12-07T09:00", "end": "2026-12-07T10:00"})).unwrap();
        let alone =
            s.run("calendar.scheduling", &json!({"people": "bo@example.org", "from": "2026-12-07T08:00", "minutes": 60, "count": 2})).unwrap();
        // (Working hours are 8 to 5 unless the owner said otherwise.)
        assert_eq!(
            alone["suggestions"],
            json!([{"start": "2026-12-07T08:00:00", "end": "2026-12-07T09:00:00"}, {"start": "2026-12-07T10:00:00", "end": "2026-12-07T11:00:00"}])
        );
        // Without what a server said, it says what to ask.
        assert_eq!(alone["ask"]["people"], json!(["bo@example.org"]));
        // Bo is busy until noon, and tentative from one to two; working elsewhere is free.
        let busy = |from: u8, to: u8, show_as: &str| {
            let day = DateTime::new(Date::new(2026, 12, 7).unwrap(), from, 0, 0).unwrap().to_timestamp(0);
            json!({"start": day, "end": day + i64::from(to - from) * 3600, "showAs": show_as})
        };
        let result = json!({"people": [{"email": "bo@example.org", "busy": [busy(10, 12, "busy"), busy(13, 14, "tentative"), busy(14, 17, "workingElsewhere")]}]});
        let both = s
            .run("calendar.scheduling", &json!({"people": "bo@example.org", "from": "2026-12-07T08:00", "minutes": 60, "count": 3, "result": result}))
            .unwrap();
        assert_eq!(both["known"], json!(["bo@example.org"]));
        let starts: Vec<&str> = both["suggestions"].as_array().unwrap().iter().map(|x| x["start"].as_str().unwrap()).collect();
        assert_eq!(starts, ["2026-12-07T08:00:00", "2026-12-07T12:00:00", "2026-12-07T14:00:00"]);
        assert!(both["ask"].is_null());
        // Hostile numbers are cut to size.
        let wild = s.run("calendar.scheduling", &json!({"from": "2026-12-07T08:00", "days": 99999, "minutes": 999999, "count": 999})).unwrap();
        assert!(wild["suggestions"].as_array().unwrap().len() <= MAX_SUGGESTIONS);
    }
}
