// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar: appointments, meetings, recurrence, calendars.

use mailcraft_model::{Address, Attendee, Calendar, Event, Freq, Id, Recurrence, Response, Rgb, ShowAs, clean_name};
use mailcraft_time::{Date, DateTime, Weekday};
use serde_json::{Value, json};

use super::{bad, done, fail, needs_message, remote};
use crate::{CmdError, CmdResult, CommandSpec, Module, Session, p};

#[cfg(test)]
mod hostile_tests;
mod invitation;
#[cfg(test)]
mod invitation_tests;
pub(crate) use invitation::{same_organizer, tidy_imported};
mod meeting;

pub use meeting::teams_refusal;

pub fn specs() -> Vec<CommandSpec> {
    let mut specs = own_specs();
    specs.extend(invitation::specs());
    specs.extend(meeting::specs());
    specs
}

fn own_specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("calendar.freeBusy", "Free and Busy Times", "Meeting › Attendees", free_busy)
            .pure()
            .params(r#"{"people": ["a@x", …] | "a@x; b@y", "from": "YYYY-MM-DDTHH:MM", "to": same (62 days at most), "result"?: FreeBusy} — without "result": what a host is to ask the account's server ({"ask": {"people", "from": seconds, "to": seconds}}, for `mailcraft-net`'s `graph::schedule`); with what the host brought back: when each of them is busy, as stretches that don't overlap ({"from", "to", "people": [{"email", "busy": [{"start", "end", "showAs": "tentative"|"busy"|"outOfOffice"|"workingElsewhere"}], "error"?: string}]}), in the times of this computer"#),
        CommandSpec::new("calendar.new", "New Appointment", "Home › New", new).key("Mod+Shift+A").params(EVENT),
        CommandSpec::new("calendar.newMeeting", "New Meeting", "Home › New", new_meeting).key("Mod+Shift+Q").params(EVENT),
        CommandSpec::new("calendar.edit", "Edit Event", "Appointment", edit)
            .params(r#"{"id"?: number, …the fields of calendar.new; "recurrence": null ends a series; for an event that repeats: "scope"?: "series" (default) | "occurrence" with "occurrence"?: "YYYY-MM-DD" (default: the selected day) — one occurrence becomes an event of its own, changed by as much as the fields change the series (a start an hour later is that occurrence an hour later); "send"?: true is required when the change makes the account's server write to the people invited. "calendar" moves it: an event of this computer's into any calendar, and an appointment that an account's server keeps into another calendar of the same account (made there, then deleted where it was: the answer says "moved": true); a meeting, a series and an item with files attached stay where they are, and the refusal says why}"#)
            .selection(needs_event),
        CommandSpec::new("calendar.move", "Move Event", "Appointment", move_event)
            .params(r#"{"start": "YYYY-MM-DDTHH:MM", "end"?: same (default: keep the length), "id"?: number, "scope"?: "series" | "occurrence", "occurrence"?: "YYYY-MM-DD" — as for calendar.edit: with "occurrence", `start` is where the series would start, and that one occurrence moves by as much, "send"?: true (required for a meeting the owner organizes on a server: the people invited are sent the update)}"#)
            .selection(needs_event),
        CommandSpec::new("calendar.delete", "Delete", "Appointment › Actions", delete)
            .params(r#"{"id"?: number, "scope"?: "series" | "occurrence", "occurrence"?: "YYYY-MM-DD" (only that day of a series), "send"?: true (required for a meeting the owner organizes on a server: the people invited are sent a cancellation), "comment"?: string (a note for them), "decline"?: true (somebody else's meeting: decline it rather than only delete the owner's copy; the organizer is told when "send" is true)}"#)
            .selection(needs_event),
        CommandSpec::new("calendar.select", "Select Event", "Calendar", select).pure().params(r#"{"id": number | null, "day"?: "YYYY-MM-DD"}"#),
        CommandSpec::new("calendar.get", "Get Event", "Calendar", get).pure().params(r#"{"id"?: number}"#),
        CommandSpec::new("calendar.list", "List Events", "Calendar", list)
            .pure()
            .params(r#"{"from"?: "YYYY-MM-DD", "to"?: "YYYY-MM-DD"} (default: the range in view)"#),
        CommandSpec::new("calendar.respond", "Respond", "Meeting › Respond", respond)
            .params(r#"{"response": "accept"|"tentative"|"decline", "id"?: number, "send"?: bool (tell the organizer by mail; default false: the answer is kept in the calendar and nobody is written to), "comment"?: string (goes with the answer when it is sent), "scope"?: "series" | "occurrence", "occurrence"?: "YYYY-MM-DD"} — a declined meeting is taken out of the calendar; the answer says {"id", "response", "sent": bool}"#)
            .selection(needs_event),
        CommandSpec::new("calendar.showAs", "Show As", "Appointment › Options", show_as)
            .params(r#"{"value": "free"|"tentative"|"busy"|"outOfOffice"|"workingElsewhere", "id"?: number}"#)
            .selection(needs_event),
        CommandSpec::new("calendar.reminder", "Reminder", "Appointment › Options", reminder)
            .params(r#"{"minutes": number | null, "id"?: number}"#)
            .selection(needs_event),
        CommandSpec::new("calendar.private", "Private", "Appointment › Tags", private)
            .params(r#"{"value"?: bool, "id"?: number}"#)
            .selection(needs_event),
        CommandSpec::new("calendar.categorize", "Categorize", "Appointment › Tags", categorize)
            .params(r#"{"category": string, "value"?: bool, "id"?: number}"#)
            .selection(needs_event),
        CommandSpec::new("calendar.fromMessage", "Reply with Meeting", "Home › Respond", from_message).key("Mod+Alt+R").selection(needs_message),
        CommandSpec::new("calendar.addCalendar", "Add Calendar", "Home › Manage Calendars", add_calendar)
            .params(r##"{"name": string, "color"?: "#RRGGBB"}"##),
        CommandSpec::new("calendar.removeCalendar", "Delete Calendar", "Folder › Actions", remove_calendar).params(r#"{"id": number}"#),
        CommandSpec::new("calendar.showCalendar", "Show Calendar", "Home › Manage Calendars", show_calendar)
            .params(r#"{"id": number, "value"?: bool (default: toggle)}"#),
        CommandSpec::new("calendar.moveToAccount", "Move to the Account's Calendar", "Home › Manage Calendars", move_to_account)
            .params(r#"{"calendar"?: number (default: every calendar on this computer), "send"?: bool} — puts what is only on this computer into the main calendar of the account's server, so that it is in the owner's Outlook calendar too: an appointment simply moves; a meeting with people invited moves only with "send": true, which makes the server send them the invitation (with "send": false it stays where it is; with neither, and meetings among them, nothing is moved and the command says so); one that the account's calendar has already (the same title at the same time) is not made a second time there: the copy on this computer goes; what can't go says why. A calendar on this computer that is empty afterwards goes away. Answers {"moved": number, "already": number, "kept": [{"id", "title", "why"}], "calendar": number, "removed": number}"#),
        CommandSpec::new("calendar.setCurrent", "Work in Calendar", "Home › Manage Calendars", set_current)
            .pure()
            .params(r#"{"id": number | null} — the calendar new events go into (null: the account's server calendar when there is one, else the mailbox's own)"#),
    ]
}

const EVENT: &str = r#"{"title": string, "start"?: "YYYY-MM-DDTHH:MM" (default: the next half hour on the day in view), "end"?: same (default: 30 minutes), "startZone"?: zone | null, "endZone"?: zone | null (the time zones the event has of its own, by a name `calendar.timeZones` lists: `Europe/Berlin`, `Eastern Standard Time`, `UTC+05:30`; a time given together with its zone is a time by that zone's clock, a zone given alone keeps the event's time on the clock and moves it; times given without a zone, and all times in answers, are by the clock of the zone the calendar is shown in, `view.timeZone`), "allDay"?: bool, "location"?: string, "description"?: string, "attendees"?: "a@x; b@y", "optional"?: "c@z", "recurrence"?: {"freq": "daily"|"weekly"|"monthly"|"yearly", "interval"?: n, "byDay"?: ["mon",…], "nth"?: 1-5|-1, "count"?: n, "until"?: "YYYY-MM-DD"} | null, "reminder"?: minutes | null, "showAs"?: string, "categories"?: [string], "private"?: bool, "importance"?: "high"|"normal"|"low", "requestResponses"?: bool, "allowProposals"?: bool (what the people invited to a meeting are asked for; both true unless said), "online"?: bool (a Teams meeting, which the server of the calendar makes: refused for a calendar on this computer, and for an account that can't make one), "calendar"?: number (default: the calendar being worked in, calendar.setCurrent), "send"?: bool} — a meeting with people invited that goes into a calendar an account's server keeps makes that server send the invitations by mail, so it has to say which is meant: "send": true invites them, "send": false keeps the meeting in a calendar on this computer and invites nobody (the answer then says "keptHere": true); without either the command refuses"#;

/// How long before its first day a new whole-day event reminds, when reminders are on: 18
/// hours, which is six in the evening of the day before.
pub const ALL_DAY_REMINDER: u32 = 18 * 60;

fn needs_event(s: &Session) -> Option<&'static str> {
    if s.sel.event.is_none() { Some("select an appointment first") } else { None }
}

fn target(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let id = p::id(v, "id").or(s.sel.event).ok_or_else(|| fail("select an appointment first"))?;
    s.mailbox.event(id).map(|e| e.id).ok_or_else(|| failed!("no event with id {id}", id))
}

/// [`target`] for a command that changes or deletes the event: the event as it is before, which
/// is what tells its server what changed ([`remote::note_changed`]). Refused for an event in a
/// calendar the account may only read.
fn changing(s: &Session, v: &Value) -> Result<Event, CmdError> {
    let id = target(s, v)?;
    let e = s.mailbox.event(id).cloned().ok_or_else(|| fail("the event is gone"))?;
    if kept_by(s, &e).is_some() {
        writable(s, e.calendar)?;
    }
    Ok(e)
}

/// The change to an event that was `before` is made: its server is told, when it has one.
fn changed(s: &mut Session, before: &Event) -> Result<(), CmdError> {
    let offset_at = remote::offset_of(s);
    remote::note_changed(&mut s.mailbox, before, &offset_at)
}

/// Whose server keeps an event, in words; `None` for one of the mailbox's own.
fn kept_by(s: &Session, e: &Event) -> Option<String> {
    remote::keeper(&s.mailbox, &e.origin).map(|a| remote::who(a).to_string())
}

/// Whether a calendar is one that an account's server keeps.
fn served(s: &Session, calendar: Id) -> bool {
    s.mailbox.calendar(calendar).is_some_and(|c| remote::keeper(&s.mailbox, &c.origin).is_some())
}

/// The first calendar that is the mailbox's own.
fn own_calendar(s: &Session) -> Id {
    let own = s.mailbox.calendars.iter().find(|c| remote::keeper(&s.mailbox, &c.origin).is_none());
    own.map_or(0, |c| c.id)
}

/// Which part of an event that repeats a command is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    /// All of it (and an event that doesn't repeat is all there is of it).
    Series,
    /// The occurrence of this day.
    One(Date),
}

/// What of an event a command is about: `"scope": "occurrence"` is one occurrence, the one on
/// the day `"occurrence"` names or else on the day that is selected; `"scope": "series"`, or
/// neither, is all of it.
fn part(s: &Session, e: &Event, v: &Value) -> Result<Part, CmdError> {
    let day = p::date(v, "occurrence")?;
    let one = match p::str(v, "scope") {
        None => day.is_some(),
        Some("occurrence") => true,
        Some("series") => false,
        Some(_) => return Err(bad("`scope` is \"occurrence\" or \"series\"")),
    };
    if !one || e.recurrence.is_none() {
        return Ok(Part::Series);
    }
    let day = day.or(s.sel.event_day.filter(|_| s.sel.event == Some(e.id)));
    let day = day.ok_or_else(|| bad("`occurrence` (a day, YYYY-MM-DD) says which occurrence is meant"))?;
    // The day is the one the occurrence starts on here. That is the day the series names it
    // by, but for a series that repeats in another zone on a night when the clocks change.
    Ok(Part::One(e.series_day_on(day, &remote::offset_of(s)).unwrap_or(day)))
}

/// Whether a series has an occurrence of its day `day` (and doesn't skip it), in the calendar
/// of `s`.
fn happens_on(s: &Session, e: &Event, day: Date) -> bool {
    e.occurrences_in(day.add_days(-1), day.add_days(1), &remote::offset_of(s)).iter().any(|o| o.day == day)
}

/// How many people an account's server would write to when the event is made, changed or
/// deleted there: those invited to a meeting the owner organizes. None for an appointment, for
/// somebody else's meeting (it is the owner's copy that changes) and for anything kept here.
fn invited(s: &Session, e: &Event) -> usize {
    if kept_by(s, e).is_some() && !e.origin.guest { e.attendees.len() } else { 0 }
}

/// What a command is refused with when it would make a server write to `people` and wasn't
/// told to (`"send": true`): `doing` says what would be sent.
fn unsent(people: usize, doing: &str) -> CmdError {
    let who = if people == 1 { "1 person is".to_string() } else { format!("{people} people are") };
    fail(format!("{who} invited to this meeting, and {doing} Nothing was changed. Say that this is meant with \"send\": true."))
}

const TELLS_UPDATE: &str = "changing it makes the account's server send them an update by mail.";
const TELLS_CANCEL: &str = "deleting it makes the account's server send them a cancellation by mail.";
const TELLS_INVITE: &str = "putting it into a calendar that an account's server keeps makes that server send them the invitation by mail.";

/// Whether the owner said yes to the mail that reaching `people` takes; an error when there
/// are people and no yes. No people, no mail.
fn may_send(v: &Value, people: usize, doing: &str) -> Result<bool, CmdError> {
    match (people, p::bool(v, "send")) {
        (0, _) => Ok(false),
        (_, Some(true)) => Ok(true),
        _ => Err(unsent(people, doing)),
    }
}

/// The note that goes with a cancellation or an answer: lines of text, not too long.
fn comment(v: &Value) -> String {
    let words = p::str(v, "comment").unwrap_or("");
    words.chars().filter(|c| *c == '\n' || !c.is_control()).take(mailcraft_model::remote::MAX_COMMENT).collect::<String>().trim().to_string()
}

/// The calendar the owner works in, which is where an event made here goes when none is named:
/// the one that was chosen (`calendar.setCurrent`), for as long as the mailbox has it; otherwise
/// the main calendar of the account's server when a download has brought one, so that the event
/// shows wherever the account is read; otherwise the first calendar of the mailbox's own.
pub fn current_calendar(s: &Session) -> Id {
    if s.mailbox.calendar(s.view.in_calendar).is_some() {
        return s.view.in_calendar;
    }
    match remote::server_calendar(&s.mailbox) {
        Some(c) => c.id,
        None => own_calendar(s),
    }
}

/// A calendar of the mailbox's own, for what is not to be on a server (a meeting nobody is to be
/// invited to yet): the first there is, or a new one that says where it lives.
fn own_calendar_or_new(s: &mut Session) -> Result<Id, CmdError> {
    match own_calendar(s) {
        0 => {
            if s.mailbox.calendars.len() >= 200 {
                return Err(fail("there are too many calendars"));
            }
            let (id, color) = (s.mailbox.alloc_id(), next_color(&s.mailbox));
            s.mailbox.calendars.push(Calendar { id, name: remote::LOCAL_CALENDAR.into(), color, visible: true, ..Default::default() });
            Ok(id)
        }
        id => Ok(id),
    }
}

/// Refuse to put into a calendar that a server keeps what would not arrive there as it is here:
/// a meeting that repeats and skips days (each skipped day would be a cancellation in the mail of
/// everyone invited), and anything into a calendar the account may only read.
fn fits_server(s: &Session, e: &Event) -> Result<(), CmdError> {
    if !e.attendees.is_empty() && e.recurrence.is_some() && !e.exceptions.is_empty() {
        return Err(fail(
            "A meeting that repeats and skips some days can't be put on the server's calendar from here: everyone invited would get a cancellation for each skipped day. Keep it in a calendar on this computer, or make it again without the skipped days.",
        ));
    }
    writable(s, e.calendar)
}

/// Refuse to change what is in a calendar the account may only read.
fn writable(s: &Session, calendar: Id) -> Result<(), CmdError> {
    remote::writable(&s.mailbox, calendar)
}

/// Our own palette for calendars, cycled.
const COLORS: [Rgb; 6] =
    [Rgb(0x2E, 0x8B, 0x57), Rgb(0xE0, 0x7B, 0x1F), Rgb(0x7E, 0x57, 0xC2), Rgb(0xD1, 0x3B, 0x3B), Rgb(0x1A, 0x9B, 0xA6), Rgb(0xD9, 0xA4, 0x0C)];

/// The colour the mailbox's next calendar gets.
pub(crate) fn next_color(mailbox: &mailcraft_model::Mailbox) -> Rgb {
    COLORS.get(mailbox.calendars.len() % COLORS.len()).copied().unwrap_or_default()
}

fn parse_recurrence(v: &Value) -> Result<Recurrence, CmdError> {
    let freq = Freq::parse(p::req_str(v, "freq")?).ok_or_else(|| bad("`freq` is daily, weekly, monthly or yearly"))?;
    let mut by_day = Vec::new();
    for d in p::strings(v, "byDay").unwrap_or_default() {
        by_day.push(Weekday::parse(&d).or_else(|| Weekday::from_ical(&d)).ok_or_else(|| bad(format!("`byDay`: \"{d}\" isn't a day of the week")))?);
    }
    by_day.sort();
    by_day.dedup();
    let nth = match p::i64(v, "nth") {
        None | Some(0) => None,
        Some(n @ (1..=5 | -1)) => Some(n as i8),
        Some(_) => return Err(bad("`nth` is 1 to 5, or -1 for the last")),
    };
    Ok(Recurrence {
        freq,
        interval: p::u64(v, "interval").unwrap_or(1).clamp(1, 999) as u32,
        by_day,
        nth,
        count: p::u64(v, "count").filter(|c| *c > 0).map(|c| c.min(9999) as u32),
        until: p::date(v, "until")?,
    })
}

fn parse_attendees(text: &str, optional: bool) -> Result<Vec<Attendee>, CmdError> {
    Address::parse_list(text)
        .into_iter()
        .take(500)
        .map(|address| {
            if address.is_valid() {
                Ok(Attendee { address, optional, response: Response::None })
            } else {
                Err(failed!("\"{address}\" isn't an e-mail address", address = address.email))
            }
        })
        .collect()
}

/// Apply the fields present in `v` to `e`.
fn apply(s: &Session, e: &mut Event, v: &Value) -> Result<(), CmdError> {
    if let Some(t) = p::str(v, "title") {
        e.title = t.replace(['\r', '\n'], " ").trim().chars().take(500).collect();
    }
    if let Some(t) = p::str(v, "location") {
        e.location = t.replace(['\r', '\n'], " ").trim().chars().take(500).collect();
    }
    if let Some(t) = p::str(v, "description") {
        e.description = t.to_string();
    }
    let length = e.duration_minutes();
    let was_all_day = e.all_day;
    // (Its times by the clocks of its own zones, for `startZone` and `endZone` below.)
    let own_times = super::zones::Times::of(s, e);
    if let Some(all_day) = p::bool(v, "allDay") {
        e.all_day = all_day;
    }
    let (start, end) = (p::datetime(v, "start")?, p::datetime(v, "end")?);
    if e.all_day {
        // Whole days, midnight to midnight; `end` names the last day.
        let first = start.map_or(e.start.date, |t| t.date);
        let last = match end {
            Some(t) => t.date,
            // An all-day event keeps its number of days when it moves.
            None if was_all_day => first.add_days((length / (24 * 60)).max(1) - 1),
            None if start.is_some() => first,
            // A timed event made all-day covers the days it touched.
            None => e.end.add_minutes(-1).date,
        };
        if last < first {
            return Err(fail("the end is before the start"));
        }
        e.start = DateTime::at_midnight(first);
        e.end = DateTime::at_midnight(last.add_days(1));
    } else {
        match start {
            // Moving the start keeps the length unless an end comes too.
            Some(t) => {
                e.start = t;
                e.end = t.add_minutes(if was_all_day { 30 } else { length });
            }
            // No longer all day: a half-hour slot at the start of the working day.
            None if was_all_day => {
                e.start = DateTime { hour: s.prefs.work_start.min(23), ..e.start };
                e.end = e.start.add_minutes(30);
            }
            None => {}
        }
        if let Some(t) = end {
            e.end = t;
        }
        if e.end < e.start {
            return Err(fail("the end is before the start"));
        }
    }
    own_times.apply(s, e, v, (start.is_some(), end.is_some()))?;
    if v.get("attendees").is_some() || v.get("optional").is_some() {
        let mut list = parse_attendees(p::str(v, "attendees").unwrap_or(""), false)?;
        list.extend(parse_attendees(p::str(v, "optional").unwrap_or(""), true)?);
        // People already invited keep their answers.
        for a in &mut list {
            if let Some(old) = e.attendees.iter().find(|o| o.address.same_mailbox(&a.address)) {
                a.response = old.response;
            }
        }
        e.attendees = list;
        if !e.attendees.is_empty() && e.organizer.is_none() {
            e.organizer = s.mailbox.default_account().map(|a| a.address());
        }
    }
    match v.get("recurrence") {
        Some(Value::Null) => {
            e.recurrence = None;
            e.exceptions.clear();
        }
        Some(r) => e.recurrence = Some(parse_recurrence(r)?),
        None => {}
    }
    match v.get("reminder") {
        Some(Value::Null) => e.reminder_minutes = None,
        Some(_) => {
            e.reminder_minutes = Some(p::u64(v, "reminder").ok_or_else(|| bad("`reminder` is a number of minutes, or null"))?.min(40_320) as u32)
        }
        None => {}
    }
    if let Some(x) = p::str(v, "showAs") {
        e.show_as = ShowAs::parse(x).ok_or_else(|| bad("`showAs` is free, tentative, busy, outOfOffice or workingElsewhere"))?;
    }
    if let Some(c) = p::strings(v, "categories") {
        e.categories = c;
    }
    if let Some(b) = p::bool(v, "private") {
        e.private = b;
    }
    if let Some(c) = p::id(v, "calendar") {
        let calendar = s.mailbox.calendar(c).ok_or_else(|| failed!("no calendar with id {id}", id = c))?;
        e.calendar = calendar.id;
    }
    meeting::apply(e, v)
}

/// The start a new appointment gets: the next half hour today, or the start of the working day
/// on another day in view.
fn default_start(s: &Session) -> DateTime {
    let now = s.local_now();
    if s.view.date == now.date {
        let minutes = (i64::from(now.minutes_of_day()) / 30 + 1) * 30;
        DateTime::at_midnight(now.date).add_minutes(minutes)
    } else {
        DateTime { hour: s.prefs.work_start.min(23), ..DateTime::at_midnight(s.view.date) }
    }
}

fn create(s: &mut Session, v: &Value, meeting: bool) -> CmdResult {
    let start = default_start(s);
    let mut e = Event { start, end: start.add_minutes(30), reminder_minutes: s.prefs.default_reminder, ..Default::default() };
    if p::bool(v, "allDay") == Some(true) {
        // As Outlook has it: a whole-day event doesn't take the day up (it shows as Free), and
        // it reminds the afternoon before. `showAs` and `reminder` say otherwise when given.
        e.reminder_minutes = s.prefs.default_reminder.map(|_| ALL_DAY_REMINDER);
        e.show_as = ShowAs::Free;
    }
    apply(s, &mut e, v)?;
    if e.title.is_empty() {
        return Err(bad("`title` is required"));
    }
    if meeting && e.attendees.is_empty() {
        return Err(bad("a meeting needs `attendees`"));
    }
    if e.calendar == 0 {
        e.calendar = current_calendar(s);
    }
    // A meeting in a calendar that a server keeps is one the server sends invitations for: only
    // when that is said to be meant. Said not to be, it is kept on this computer, and the answer
    // says so; with nothing said, nothing is made.
    let people = if served(s, e.calendar) { e.attendees.len() } else { 0 };
    let kept_here = people > 0 && p::bool(v, "send") == Some(false);
    if kept_here {
        e.calendar = own_calendar_or_new(s)?;
    } else if people > 0 && p::bool(v, "send") != Some(true) {
        let who = if people == 1 { "1 person is".to_string() } else { format!("{people} people are") };
        return Err(fail(format!(
            "{who} invited to this meeting, and {TELLS_INVITE} Nothing was made. Say \"send\": true to invite them, or \"send\": false to keep the meeting on this computer and invite nobody."
        )));
    }
    if served(s, e.calendar) {
        fits_server(s, &e)?;
        // The account whose calendar it is organizes it.
        let owner = s.mailbox.calendar(e.calendar).and_then(|c| remote::keeper(&s.mailbox, &c.origin)).map(|a| a.address());
        if !e.attendees.is_empty() {
            e.organizer = owner.or(e.organizer);
        }
    }
    // A Teams meeting is made by an account's server: one that is kept here has none.
    if kept_here {
        e.online = None;
    }
    meeting::online_fits(s, &e, None)?;
    let id = s.mailbox.add_event(e)?;
    // In a calendar that a server keeps: the server is to have it too.
    remote::note_created(s, id)?;
    if people > 0 && !kept_here {
        let account = s.mailbox.event(id).map_or(0, |e| e.origin.account);
        remote::approve_mail(&mut s.mailbox, account, id, "", "");
    }
    s.sel.event = Some(id);
    s.sel.event_day = s.mailbox.event(id).map(|e| e.start.date);
    Ok(if kept_here { json!({"id": id, "keptHere": true}) } else { json!({"id": id}) })
}

fn new(s: &mut Session, v: &Value) -> CmdResult {
    create(s, v, false)
}

fn new_meeting(s: &mut Session, v: &Value) -> CmdResult {
    create(s, v, true)
}

/// A change to an event that a server keeps (`before` as it was, `e` as it would be) is checked
/// before it is made: what is the organizer's to say is not changed in the owner's copy, and
/// what the people invited would be written to about is only done when that is said to be meant.
/// Whether the owner said yes to that mail.
fn may_change(s: &Session, v: &Value, before: &Event, e: &Event) -> Result<bool, CmdError> {
    let Some(who) = kept_by(s, before) else { return Ok(false) };
    writable(s, before.calendar)?;
    let fields = remote::changed_fields(before, e, &remote::offset_of(s));
    if before.origin.guest {
        let why = if fields.attendees.is_some() {
            mailcraft_i18n::Arg::words("Who is invited is for its organizer to say.")
        } else if fields.repeat.is_some() {
            mailcraft_i18n::Arg::words("How it repeats is for its organizer to say.")
        } else if fields.online.is_some() {
            mailcraft_i18n::Arg::words("Whether it is held online is for its organizer to say.")
        } else if fields.request_responses.is_some() || fields.allow_proposals.is_some() {
            mailcraft_i18n::Arg::words("What the people invited are asked for is for its organizer to say.")
        } else {
            return Ok(false);
        };
        return Err(failed!("This meeting is kept by the server of {who}, and somebody else organizes it. {why}", who, why));
    }
    // Those invited before and those invited now: whoever is dropped hears of it too.
    let people = if remote::seen_by_others(&fields) { before.attendees.len().max(e.attendees.len()) } else { 0 };
    may_send(v, people, TELLS_UPDATE)
}

/// One occurrence of a series (`series`, on `day`) is changed on its own: the series skips that
/// day from now on, and what happens on it is an event of its own. `changed` is the series as
/// the change would leave it; the occurrence is changed by as much (an hour later, another
/// title), on its own day.
fn change_one(s: &mut Session, v: &Value, series: &Event, day: Date, changed: Event) -> CmdResult {
    if !happens_on(s, series, day) {
        return Err(failed!("the series doesn't happen on {date}", date = day.iso()));
    }
    if series.exceptions.len() >= 5000 {
        return Err(fail("this series has too many changed days"));
    }
    let kept = kept_by(s, series).is_some();
    if kept && series.origin.id.is_empty() {
        return Err(fail("This series is still on its way to the account's server. Change one of its days once it is there (in a moment)."));
    }
    let apart = series.start.date.days_until(day);
    let origin = if kept {
        mailcraft_model::Origin {
            account: series.origin.account,
            series: series.origin.id.clone(),
            guest: series.origin.guest,
            occurrence: Some(day),
            ..Default::default()
        }
    } else {
        mailcraft_model::Origin::default()
    };
    // An event of its own has a name of its own (the server gives one it keeps the series' own).
    let uid = String::new();
    let here = remote::offset_of(s);
    let was = Event { origin: origin.clone(), uid: uid.clone(), ..remote::occurrence_in(series, day, &here) };
    let mut one = Event {
        id: 0,
        start: changed.start.add_days(apart),
        end: changed.end.add_days(apart),
        recurrence: None,
        exceptions: Vec::new(),
        origin,
        uid,
        ..changed
    };
    if series.zone().is_some() && !series.all_day && !one.all_day {
        // A series by another zone's clock has this occurrence wherever that clock puts it
        // here: it is changed by as long as the series was (an hour later is an hour later).
        let instant = |wall: DateTime| mailcraft_model::zoned::instant_at(wall.add_days(-apart), &here);
        let (from, to, at) = (series.instants(&here), (instant(one.start), instant(one.end)), was.instants(&here));
        one.set_instants(at.0.saturating_add(to.0.saturating_sub(from.0)), at.1.saturating_add(to.1.saturating_sub(from.1)), &here);
    }
    if one == was {
        return Ok(json!({"id": series.id}));
    }
    let send = if kept { may_change(s, v, &was, &one)? } else { false };
    s.mailbox.event_mut(series.id)?.exceptions.push(day);
    let id = s.mailbox.add_event(one)?;
    if kept {
        remote::note_occurrence_changed(s, series, day, id, Event { id, ..was }, send)?;
    }
    s.sel.event = Some(id);
    s.sel.event_day = s.mailbox.event(id).map(|e| e.start.date);
    Ok(json!({"id": id, "series": series.id, "occurrence": day.iso()}))
}

/// An event as `calendar.edit` with `v` would leave it: for one occurrence, the series as the
/// fields change it, less what one occurrence can't have (a rule of its own, another calendar).
fn edited(s: &Session, before: &Event, v: &Value, one: Part) -> Result<Event, CmdError> {
    let mut e = before.clone();
    match (one, v.as_object()) {
        (Part::One(_), Some(fields)) => {
            let mut fields = fields.clone();
            fields.remove("recurrence");
            fields.remove("calendar");
            apply(s, &mut e, &Value::Object(fields))?;
        }
        _ => apply(s, &mut e, v)?,
    }
    Ok(e)
}

/// The mail a calendar command would make an account's server send to other people.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mail {
    /// An invitation, to this many people.
    Invitation(usize),
    /// An update of a meeting they are invited to.
    Update(usize),
    /// A cancellation.
    Cancellation(usize),
}

/// What mail the command `id` with `v` would make an account's server send, were it told to
/// (`"send": true`); `None` when it would write to nobody (or would fail anyway). For whoever
/// asks the owner first: the command itself refuses to send unasked.
pub fn mail_of(s: &Session, id: &str, v: &Value) -> Option<Mail> {
    if matches!(id, "calendar.new" | "calendar.newMeeting") {
        let mut e = Event::default();
        apply(s, &mut e, v).ok()?;
        let calendar = if e.calendar == 0 { current_calendar(s) } else { e.calendar };
        return (served(s, calendar) && !e.attendees.is_empty()).then_some(Mail::Invitation(e.attendees.len()));
    }
    let before = s.mailbox.event(p::id(v, "id").or(s.sel.event)?)?.clone();
    let one = part(s, &before, v).ok()?;
    if id == "calendar.delete" {
        // (One that isn't on the server yet has invited nobody yet.)
        let people = if before.origin.id.is_empty() || p::bool(v, "decline") == Some(true) { 0 } else { invited(s, &before) };
        return (people > 0).then_some(Mail::Cancellation(people));
    }
    let e = match id {
        "calendar.private" => Event { private: p::bool(v, "value").unwrap_or(!before.private), ..before.clone() },
        "calendar.move" => Event { start: p::datetime(v, "start").ok()??, ..before.clone() },
        "calendar.edit" => edited(s, &before, v, one).ok()?,
        "calendar.importance" | "calendar.responseOptions" | "calendar.teamsMeeting" => meeting::as_changed(&before, id, v).ok()?,
        _ => return None,
    };
    if kept_by(s, &before).is_none() {
        // One of the mailbox's own that goes into a server's calendar.
        let arrives = e.calendar != before.calendar && served(s, e.calendar) && !e.attendees.is_empty();
        return arrives.then_some(Mail::Invitation(e.attendees.len()));
    }
    if before.origin.guest {
        return None;
    }
    let people = before.attendees.len().max(e.attendees.len());
    let seen = remote::seen_by_others(&remote::changed_fields(&before, &e, &remote::offset_of(s)));
    (people > 0 && seen).then_some(Mail::Update(people))
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let before = changing(s, v)?;
    let id = before.id;
    let one = part(s, &before, v)?;
    let mut e = edited(s, &before, v, one)?;
    if e.title.is_empty() {
        return Err(fail("an event needs a title"));
    }
    if let Part::One(day) = one {
        return change_one(s, v, &before, day, e);
    }
    let kept = kept_by(s, &before);
    let moved = e.calendar != before.calendar;
    let mut send = false;
    meeting::online_fits(s, &e, Some(&before))?;
    if let Some(who) = &kept {
        // Into another calendar of the same account: its server has no way of moving an event,
        // so an appointment is made again there and the original deleted once that went through
        // ([`remote::note_moved`]). Anything else stays where it is, and is told why.
        if moved {
            meeting::movable(s, &before, &e, who)?;
            *s.mailbox.event_mut(id)? = e;
            remote::note_moved(s, id, &before)?;
            return Ok(json!({"id": id, "moved": true}));
        }
        send = may_change(s, v, &before, &e)?;
    } else if moved && served(s, e.calendar) {
        // One of the mailbox's own that goes into a server's calendar: a meeting is then one
        // the server sends invitations for.
        fits_server(s, &e)?;
        send = may_send(v, e.attendees.len(), TELLS_INVITE)?;
        if send {
            e.organizer = s.mailbox.calendar(e.calendar).and_then(|c| remote::keeper(&s.mailbox, &c.origin)).map(|a| a.address()).or(e.organizer);
        }
    }
    *s.mailbox.event_mut(id)? = e;
    match kept {
        Some(_) => changed(s, &before)?,
        // One of the mailbox's own that was put into a server's calendar is that server's now.
        None if moved => remote::note_created(s, id)?,
        None => {}
    }
    if send {
        let account = s.mailbox.event(id).map_or(0, |e| e.origin.account);
        remote::approve_mail(&mut s.mailbox, account, id, &before.origin.id, "");
    }
    Ok(json!({"id": id}))
}

fn move_event(s: &mut Session, v: &Value) -> CmdResult {
    let before = changing(s, v)?;
    let id = before.id;
    let start = p::datetime(v, "start")?.ok_or_else(|| bad("`start` is required"))?;
    let mut e = before.clone();
    let length = e.duration_minutes();
    // (A time that is given is a time the clock shows once.)
    (e.start_repeated, e.end_repeated) = (false, false);
    e.start = if e.all_day { DateTime::at_midnight(start.date) } else { start };
    e.end = match p::datetime(v, "end")? {
        // For whole days, `end` names the last day.
        Some(end) if e.all_day && end.date >= e.start.date => DateTime::at_midnight(end.date.add_days(1)),
        Some(end) if !e.all_day && end >= e.start => end,
        Some(_) => return Err(fail("the end is before the start")),
        None => e.start.add_minutes(length),
    };
    if let Part::One(day) = part(s, &before, v)? {
        return change_one(s, v, &before, day, e);
    }
    let send = may_change(s, v, &before, &e)?;
    *s.mailbox.event_mut(id)? = e;
    changed(s, &before)?;
    if send {
        remote::approve_mail(&mut s.mailbox, before.origin.account, id, &before.origin.id, "");
    }
    s.sel.event_day = Some(start.date);
    Ok(json!({"id": id}))
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let before = changing(s, v)?;
    let id = before.id;
    if p::date(v, "occurrence")?.is_some() && before.recurrence.is_none() {
        return Err(fail("that event doesn't repeat"));
    }
    let kept = kept_by(s, &before).is_some();
    if kept {
        writable(s, before.calendar)?;
    }
    // Somebody else's meeting can be declined rather than only taken out of the calendar.
    let decline = p::bool(v, "decline") == Some(true) && kept && before.origin.guest && !before.origin.id.is_empty();
    let (tell, note) = (p::bool(v, "send") == Some(true), comment(v));
    if let Part::One(day) = part(s, &before, v)? {
        if before.exceptions.contains(&day) {
            return Ok(json!({"id": id, "occurrence": day.iso()}));
        }
        if !happens_on(s, &before, day) {
            return Err(failed!("the series doesn't happen on {date}", date = day.iso()));
        }
        if before.exceptions.len() >= 5000 {
            return Err(fail("this series has too many cancelled days"));
        }
        // (A series that isn't on the server yet has invited nobody yet.)
        let people = if before.origin.id.is_empty() { 0 } else { invited(s, &before) };
        let send = if decline { tell } else { may_send(v, people, TELLS_CANCEL)? };
        s.mailbox.event_mut(id)?.exceptions.push(day);
        // Its server is to delete (or decline) that occurrence too; a series that isn't there
        // yet arrives without it.
        if kept && !before.origin.id.is_empty() {
            let response = if decline { Response::Declined } else { Response::None };
            remote::note_occurrence_gone(s, &before, day, response, send, &note)?;
        }
        return Ok(json!({"id": id, "occurrence": day.iso()}));
    }
    // (A meeting that isn't on the server yet has invited nobody yet: it just doesn't go.)
    let people = if before.origin.id.is_empty() { 0 } else { invited(s, &before) };
    let send = if decline { tell } else { may_send(v, people, TELLS_CANCEL)? };
    s.mailbox.remove_event(id)?;
    if decline {
        remote::note_responded(&mut s.mailbox, &before, Response::Declined, send, &note)?;
    } else {
        // Its server is to delete it too.
        remote::note_deleted(&mut s.mailbox, &before)?;
        if send {
            remote::approve_mail(&mut s.mailbox, before.origin.account, id, &before.origin.id, &note);
        }
    }
    if s.sel.event == Some(id) {
        s.sel.event = None;
        s.sel.event_day = None;
    }
    Ok(json!({"deleted": id}))
}

fn select(s: &mut Session, v: &Value) -> CmdResult {
    match p::id(v, "id") {
        Some(id) => {
            let e = s.mailbox.event(id).ok_or_else(|| failed!("no event with id {id}", id))?;
            s.sel.event_day = p::date(v, "day")?.or(Some(e.start.date));
            s.sel.event = Some(id);
        }
        None => {
            s.sel.event = None;
            s.sel.event_day = None;
        }
    }
    done()
}

fn event_json(s: &Session, e: &Event) -> Value {
    // Whole days read as they are written: dates, the last day included.
    let (start, end) =
        if e.all_day { (e.start.date.iso(), e.end.add_minutes(-1).date.max(e.start.date).iso()) } else { (e.start.iso(), e.end.iso()) };
    json!({
        "id": e.id, "calendar": e.calendar, "title": e.title, "location": e.location, "description": e.description,
        "start": start, "end": end, "allDay": e.all_day,
        "organizer": e.organizer.as_ref().map(Address::full),
        "attendees": e.attendees.iter().map(|a| json!({"address": a.address.full(), "optional": a.optional, "response": a.response})).collect::<Vec<_>>(),
        "recurrence": e.recurrence.as_ref().map(|r| json!({"rule": r, "text": r.describe(e.start.date)})),
        "exceptions": e.exceptions.iter().map(|d| d.iso()).collect::<Vec<_>>(),
        "reminder": e.reminder_minutes, "showAs": e.show_as, "categories": e.categories, "private": e.private, "cancelled": e.cancelled,
        "time": e.time_label(s.prefs.twelve_hour),
        // The zones it has of its own (`start` and `end` are by the calendar's clock either
        // way), and its times by their clock when that shows another time.
        "startZone": e.zone().map(mailcraft_time::Zone::id), "endZone": e.zone_of_end().map(mailcraft_time::Zone::id),
        "ownTime": mailcraft_model::zoned::own_time_label(e, &remote::offset_of(s), s.prefs.twelve_hour),
        // An account's server keeps it; somebody else organizes it; it is one occurrence of a
        // series that was changed on its own (the day it stands for).
        "onServer": kept_by(s, e).is_some(), "guest": e.origin.guest, "occurrenceOf": e.origin.occurrence.map(|day| day.iso()),
        // How much it matters; what the people invited are asked for; whether it is held online,
        // with the link to join by (a link from outside: data, to be followed only on the
        // owner's word); the other times people proposed.
        "importance": e.importance, "requestResponses": e.response_options.request_responses, "allowProposals": e.response_options.allow_proposals,
        "online": e.online.as_ref().map(|online| json!({"provider": online.provider, "joinUrl": online.join_url})),
        "proposals": e.proposals.iter().map(|x| json!({"from": x.from.full(), "start": x.start.iso(), "end": x.end.iso()})).collect::<Vec<_>>(),
        "hasAttachments": e.has_attachments,
    })
}

fn get(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let e = s.mailbox.event(id).ok_or_else(|| fail("the event is gone"))?;
    Ok(event_json(s, e))
}

/// What happens on the days `from`–`to` in the calendars that are shown, in order.
pub fn occurrences(s: &Session, from: Date, to: Date) -> Vec<mailcraft_model::Occurrence> {
    let shown: Vec<Id> = s.mailbox.calendars.iter().filter(|c| c.visible).map(|c| c.id).collect();
    // (A series by another zone's clock is wherever that clock puts each occurrence here.)
    let here = remote::offset_of(s);
    let mut out: Vec<mailcraft_model::Occurrence> =
        s.mailbox.events.values().filter(|e| shown.contains(&e.calendar)).flat_map(|e| e.occurrences_in(from, to, &here)).collect();
    out.sort_by_key(|o| (o.start, o.end, o.event));
    out
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let (view_from, view_to) = super::view::calendar_range(s);
    let from = p::date(v, "from")?.unwrap_or(view_from);
    let to = p::date(v, "to")?.unwrap_or(view_to).max(from);
    if from.days_until(to) > 3700 {
        return Err(bad("the range is longer than ten years"));
    }
    let items: Vec<Value> = occurrences(s, from, to)
        .into_iter()
        .filter_map(|o| {
            let e = s.mailbox.event(o.event)?;
            Some(json!({"id": e.id, "title": e.title, "start": o.start.iso(), "end": o.end.iso(), "allDay": e.all_day, "location": e.location, "repeats": e.recurrence.is_some(), "showAs": e.show_as, "categories": e.categories}))
        })
        .collect();
    Ok(json!({"from": from.iso(), "to": to.iso(), "events": items}))
}

fn respond(s: &mut Session, v: &Value) -> CmdResult {
    // An invitation in the mail is answered through what it says (`calendar.invitation`).
    if p::id(v, "message").is_some() {
        return invitation::respond(s, v);
    }
    let before = changing(s, v)?;
    let id = before.id;
    let response = Response::parse(p::req_str(v, "response")?)
        .filter(|r| *r != Response::None)
        .ok_or_else(|| bad("`response` is accept, tentative or decline"))?;
    let mine: Vec<String> = s.mailbox.accounts.iter().map(|a| a.email.to_lowercase()).collect();
    // A meeting that an account's server keeps is answered there: the server puts the answer
    // into the calendar, and tells the organizer when that is asked for.
    if let Some(who) = kept_by(s, &before) {
        if !before.origin.guest {
            return Err(fail("You organize this meeting, so there is nobody to send an answer to."));
        }
        if before.origin.id.is_empty() {
            return Err(failed!("This meeting hasn't reached the server of {who} yet.", who));
        }
        writable(s, before.calendar)?;
        let (send, note) = (p::bool(v, "send") == Some(true), comment(v));
        // Another time proposed with the answer (`calendar.propose`): mail to the organizer.
        let proposal = meeting::proposed(s, v, before.response_options.allow_proposals, send, response)?;
        match part(s, &before, v)? {
            Part::One(day) => {
                if !happens_on(s, &before, day) {
                    return Err(failed!("the series doesn't happen on {date}", date = day.iso()));
                }
                // Declined, that day is out of the calendar; otherwise the server makes the
                // occurrence an event of its own with the answer, which the next download brings.
                if response == Response::Declined {
                    s.mailbox.event_mut(id)?.exceptions.push(day);
                }
                remote::note_occurrence_gone(s, &before, day, response, send, &note)?;
                if let Some((_, _, time)) = proposal {
                    remote::propose_with_answer(&mut s.mailbox, time);
                }
                return Ok(json!({"id": id, "occurrence": day.iso(), "response": response, "sent": send}));
            }
            Part::Series if response == Response::Declined => {
                // As Outlook has it: a meeting that was declined is taken out of the calendar.
                s.mailbox.remove_event(id)?;
                if s.sel.event == Some(id) {
                    (s.sel.event, s.sel.event_day) = (None, None);
                }
            }
            Part::Series => {
                let e = s.mailbox.event_mut(id)?;
                for a in e.attendees.iter_mut().filter(|a| mine.contains(&a.address.email.to_lowercase())) {
                    a.response = response;
                }
                e.show_as = if response == Response::Tentative { ShowAs::Tentative } else { ShowAs::Busy };
            }
        }
        remote::note_responded(&mut s.mailbox, &before, response, send, &note)?;
        if let Some((_, _, time)) = proposal {
            remote::propose_with_answer(&mut s.mailbox, time);
        }
        return Ok(json!({"id": id, "response": response, "sent": send}));
    }
    // (An answer given in a calendar on this computer writes to nobody: there is nothing for
    // another time to go with. The invitation in the mail can carry one.)
    if v.get("propose").is_some_and(|asked| !asked.is_null()) {
        return Err(fail("Another time can be proposed for a meeting that the account's server keeps, or from the invitation in the mail."));
    }
    let e = s.mailbox.event_mut(id)?;
    for a in e.attendees.iter_mut().filter(|a| mine.contains(&a.address.email.to_lowercase())) {
        a.response = response;
    }
    // The calendar shows the answer: a declined meeting frees the time, a tentative one hatches it.
    e.show_as = match response {
        Response::Declined => ShowAs::Free,
        Response::Tentative => ShowAs::Tentative,
        _ => ShowAs::Busy,
    };
    Ok(json!({"id": id, "response": response}))
}

fn show_as(s: &mut Session, v: &Value) -> CmdResult {
    let before = changing(s, v)?;
    let id = before.id;
    let value = ShowAs::parse(p::req_str(v, "value")?).ok_or_else(|| bad("`value` is free, tentative, busy, outOfOffice or workingElsewhere"))?;
    s.mailbox.event_mut(id)?.show_as = value;
    changed(s, &before)?;
    Ok(json!({"id": id, "showAs": value}))
}

fn reminder(s: &mut Session, v: &Value) -> CmdResult {
    let before = changing(s, v)?;
    let id = before.id;
    let minutes = match v.get("minutes") {
        Some(Value::Null) => None,
        _ => Some(p::u64(v, "minutes").ok_or_else(|| bad("`minutes` is a number, or null for no reminder"))?.min(40_320) as u32),
    };
    s.mailbox.event_mut(id)?.reminder_minutes = minutes;
    changed(s, &before)?;
    Ok(json!({"id": id, "reminder": minutes}))
}

fn private(s: &mut Session, v: &Value) -> CmdResult {
    let before = changing(s, v)?;
    let id = before.id;
    let private = p::bool(v, "value").unwrap_or(!before.private);
    // A meeting the owner organizes is private for everyone invited: they are told.
    let send = may_change(s, v, &before, &Event { private, ..before.clone() })?;
    s.mailbox.event_mut(id)?.private = private;
    changed(s, &before)?;
    if send {
        remote::approve_mail(&mut s.mailbox, before.origin.account, id, &before.origin.id, "");
    }
    Ok(json!({"id": id, "private": private}))
}

fn categorize(s: &mut Session, v: &Value) -> CmdResult {
    let before = changing(s, v)?;
    let id = before.id;
    let name = clean_name(p::req_str(v, "category")?).ok_or_else(|| bad("`category` is empty"))?;
    if kept_by(s, &before).is_some() {
        writable(s, before.calendar)?;
    }
    let e = s.mailbox.event_mut(id)?;
    let has = e.categories.iter().any(|c| c.eq_ignore_ascii_case(&name));
    let on = p::bool(v, "value").unwrap_or(!has);
    e.categories.retain(|c| !c.eq_ignore_ascii_case(&name));
    if on {
        e.categories.push(name);
    }
    let categories = e.categories.clone();
    // The account's server keeps an event's categories with it (and tells nobody else).
    changed(s, &before)?;
    Ok(json!({"id": id, "categories": categories}))
}

fn from_message(s: &mut Session, v: &Value) -> CmdResult {
    let mid = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    if s.mailbox.message(mid).is_none() {
        return Err(failed!("no message with id {id}", id = mid));
    }
    // Its text goes into the meeting: from where the mailbox keeps it when it isn't in memory.
    let m = s.loaded_message_where(mid, |_| false)?;
    let mine: Vec<String> = s.mailbox.accounts.iter().map(|a| a.email.to_lowercase()).collect();
    // Everyone on the message but me.
    let mut people: Vec<Address> = Vec::new();
    for a in std::iter::once(&m.from).chain(&m.to).chain(&m.cc) {
        if a.is_valid() && !mine.contains(&a.email.to_lowercase()) && !people.iter().any(|x| x.same_mailbox(a)) {
            people.push(a.clone());
        }
    }
    let start = default_start(s);
    // A meeting: it stays on this computer (nothing made here sends invitations yet).
    let calendar = own_calendar_or_new(s)?;
    let e = Event {
        calendar,
        title: m.topic(),
        description: m.body.text.clone(),
        start,
        end: start.add_minutes(30),
        organizer: s.mailbox.account(m.account).map(|a| a.address()),
        attendees: people.into_iter().take(500).map(|address| Attendee { address, optional: false, response: Response::None }).collect(),
        reminder_minutes: s.prefs.default_reminder,
        ..Default::default()
    };
    let id = s.mailbox.add_event(e)?;
    s.sel.event = Some(id);
    s.sel.event_day = Some(start.date);
    s.view.module = Module::Calendar;
    s.view.date = start.date;
    Ok(json!({"id": id}))
}

fn add_calendar(s: &mut Session, v: &Value) -> CmdResult {
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    if s.mailbox.calendars.iter().any(|c| c.name.eq_ignore_ascii_case(&name)) {
        return Err(failed!("a calendar named \"{name}\" already exists", name));
    }
    if s.mailbox.calendars.len() >= 200 {
        return Err(fail("there are too many calendars"));
    }
    let color = match p::str(v, "color") {
        Some(c) => Rgb::parse(c).ok_or_else(|| bad("`color` looks like #2E8B57"))?,
        None => next_color(&s.mailbox),
    };
    let id = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar { id, name, color, visible: true, ..Default::default() });
    Ok(json!({"id": id}))
}

fn remove_calendar(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let Some(calendar) = s.mailbox.calendar(id) else { return Err(failed!("no calendar with id {id}", id)) };
    // A calendar that a server keeps would be back with the next download, its events with it.
    super::remote::changeable(&s.mailbox, &calendar.origin, "calendar")?;
    // One calendar of the mailbox's own stays, for new events to go in.
    let own = |c: &Calendar| super::remote::keeper(&s.mailbox, &c.origin).is_none();
    if !s.mailbox.calendars.iter().any(|c| c.id != id && own(c)) {
        return Err(fail("the last calendar can't be deleted"));
    }
    s.mailbox.calendars.retain(|c| c.id != id);
    let events: Vec<Id> = s.mailbox.events.values().filter(|e| e.calendar == id).map(|e| e.id).collect();
    for e in &events {
        s.mailbox.remove_event(*e)?;
    }
    Ok(json!({"deleted": id, "events": events.len()}))
}

/// What is only on this computer goes into the main calendar of the account's server
/// (`calendar.moveToAccount`): an appointment simply moves, and is made on the server; a meeting
/// moves only when its invitations are to be sent; what can't go stays and says why. A calendar
/// of the mailbox's own that is left empty goes away, unless the owner named it.
fn move_to_account(s: &mut Session, v: &Value) -> CmdResult {
    let Some((target, account)) = remote::server_calendar(&s.mailbox).map(|calendar| (calendar.id, calendar.origin.account)) else {
        return Err(fail("No account's server keeps a calendar here yet, so there is nowhere to move these to."));
    };
    writable(s, target)?;
    let own = |s: &Session, calendar: Id| s.mailbox.calendar(calendar).is_some_and(|c| remote::keeper(&s.mailbox, &c.origin).is_none());
    let from: Vec<Id> = match p::id(v, "calendar") {
        Some(calendar) if own(s, calendar) => vec![calendar],
        Some(calendar) if s.mailbox.calendar(calendar).is_some() => return Err(fail("That calendar is kept by an account's server already.")),
        Some(calendar) => return Err(failed!("no calendar with id {id}", id = calendar)),
        None => s.mailbox.calendars.iter().filter(|c| remote::keeper(&s.mailbox, &c.origin).is_none()).map(|c| c.id).collect(),
    };
    let ids: Vec<Id> = s.mailbox.events.values().filter(|e| from.contains(&e.calendar)).map(|e| e.id).take(10_000).collect();
    let meetings = ids.iter().filter(|id| s.mailbox.event(**id).is_some_and(|e| !e.attendees.is_empty())).count();
    let send = p::bool(v, "send");
    if meetings > 0 && send.is_none() {
        let which = if meetings == 1 { "1 of these is a meeting".to_string() } else { format!("{meetings} of these are meetings") };
        return Err(fail(format!(
            "{which} with people invited, and {TELLS_INVITE} Nothing was moved. Say \"send\": true to move them too and invite the people, or \"send\": false to move only the appointments."
        )));
    }
    let owner = s.mailbox.account(account).map(|a| a.address());
    let (mut moved, mut already, mut kept) = (0usize, 0usize, Vec::new());
    // What the account's calendars have already: the same title at the same time, repeating
    // the same way. Such an item is on the server; a second one there would help nobody.
    let twin = |s: &Session, e: &Event| {
        let same = |other: &Event| {
            other.origin.account == account
                && other.title.trim() == e.title.trim()
                && (other.start, other.end, other.all_day) == (e.start, e.end, e.all_day)
                && other.recurrence == e.recurrence
        };
        s.mailbox.events.values().any(|other| same(other))
    };
    for id in ids {
        let Some(before) = s.mailbox.event(id).cloned() else { continue };
        if twin(s, &before) {
            s.mailbox.remove_event(id)?;
            already += 1;
            continue;
        }
        let mut e = Event { calendar: target, ..before.clone() };
        let why = if !e.attendees.is_empty() && send != Some(true) {
            Some("People are invited to it, and no invitations were to be sent.".to_string())
        } else {
            fits_server(s, &e).and_then(|()| remote::room_for_change(&s.mailbox)).err().map(|e| e.to_string())
        };
        if let Some(why) = why {
            kept.push(json!({"id": id, "title": before.title, "why": why}));
            continue;
        }
        // The account whose calendar it is organizes it from now on.
        if !e.attendees.is_empty() {
            e.organizer = owner.clone().or(e.organizer);
        }
        let invites = !e.attendees.is_empty();
        *s.mailbox.event_mut(id)? = e;
        // It is that server's now, and the server is to have it too.
        remote::note_created(s, id)?;
        if invites {
            remote::approve_mail(&mut s.mailbox, account, id, "", "");
        }
        moved += 1;
    }
    // A calendar on this computer with nothing left in it goes away: the one the mailbox was
    // made with, under either of the names it goes by. One the owner named stays.
    let in_use: std::collections::HashSet<Id> = s.mailbox.events.values().map(|e| e.calendar).collect();
    let default_name = |name: &str| name.trim().eq_ignore_ascii_case("Calendar") || name.trim().eq_ignore_ascii_case(remote::LOCAL_CALENDAR);
    let calendars = s.mailbox.calendars.len();
    s.mailbox.calendars.retain(|c| !(from.contains(&c.id) && !in_use.contains(&c.id) && default_name(&c.name)));
    let removed = calendars - s.mailbox.calendars.len();
    if s.mailbox.calendar(s.view.in_calendar).is_none() {
        s.view.in_calendar = 0;
    }
    Ok(json!({"moved": moved, "already": already, "kept": kept, "calendar": target, "removed": removed}))
}

fn set_current(s: &mut Session, v: &Value) -> CmdResult {
    s.view.in_calendar = match v.get("id") {
        None | Some(Value::Null) => 0,
        Some(_) => {
            let id = p::req_id(v, "id")?;
            s.mailbox.calendar(id).map(|c| c.id).ok_or_else(|| failed!("no calendar with id {id}", id))?
        }
    };
    Ok(json!({"id": current_calendar(s)}))
}

fn show_calendar(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let value = p::bool(v, "value");
    let c = s.mailbox.calendars.iter_mut().find(|c| c.id == id).ok_or_else(|| failed!("no calendar with id {id}", id))?;
    c.visible = value.unwrap_or(!c.visible);
    Ok(json!({"id": id, "visible": c.visible}))
}

/// How much a way of showing time counts when two overlap: the stronger one is what shows.
fn weight(show_as: ShowAs) -> u8 {
    match show_as {
        ShowAs::Free => 0,
        ShowAs::WorkingElsewhere => 1,
        ShowAs::Tentative => 2,
        ShowAs::Busy => 3,
        ShowAs::OutOfOffice => 4,
    }
}

/// Somebody's time within `from`..`to` as stretches that don't overlap, in order: where several
/// things are on at once the strongest shows (out of office over busy over tentative), stretches
/// that touch and show alike are one, and free time is left out.
pub fn merge_busy(times: &[mailcraft_model::remote::BusyTime], from: i64, to: i64) -> Vec<mailcraft_model::remote::BusyTime> {
    use mailcraft_model::remote::{BusyTime, MAX_BUSY};
    let clipped: Vec<BusyTime> = times
        .iter()
        .take(MAX_BUSY)
        .map(|time| BusyTime { start: time.start.max(from), end: time.end.min(to), show_as: time.show_as })
        .filter(|time| time.end > time.start && time.show_as != ShowAs::Free)
        .collect();
    let mut edges: Vec<i64> = clipped.iter().flat_map(|time| [time.start, time.end]).collect();
    edges.sort_unstable();
    edges.dedup();
    let mut merged: Vec<BusyTime> = Vec::new();
    for pair in edges.windows(2) {
        let (Some(start), Some(end)) = (pair.first().copied(), pair.get(1).copied()) else { continue };
        let over =
            clipped.iter().filter(|time| time.start <= start && time.end >= end).map(|time| time.show_as).max_by_key(|show_as| weight(*show_as));
        let Some(show_as) = over else { continue };
        match merged.last_mut() {
            Some(last) if last.end == start && last.show_as == show_as => last.end = end,
            _ => merged.push(BusyTime { start, end, show_as }),
        }
    }
    merged
}

fn free_busy(s: &mut Session, v: &Value) -> CmdResult {
    use mailcraft_model::remote::{FreeBusy, FreeBusyAsk, MAX_SCHEDULE_STRETCH, MAX_SCHEDULES};
    let offset_at = remote::offset_of(s);
    let instant = |wall: DateTime| wall.to_timestamp(offset_at(wall.to_timestamp(offset_at(wall.to_timestamp(0)))));
    let from = p::datetime(v, "from")?.ok_or_else(|| bad("`from` is required"))?;
    let to = p::datetime(v, "to")?.ok_or_else(|| bad("`to` is required"))?;
    let (from, to) = (instant(from), instant(to));
    if to <= from || to.saturating_sub(from) > MAX_SCHEDULE_STRETCH {
        return Err(bad("`to` is after `from`, and no more than 62 days after"));
    }
    let named: Vec<String> = match v.get("people") {
        Some(Value::String(line)) => Address::parse_list(line).into_iter().map(|a| a.email).collect(),
        Some(_) => p::strings(v, "people").unwrap_or_default(),
        None => Vec::new(),
    };
    let asked = named.len();
    let mut ask = FreeBusyAsk { people: named, from, to };
    ask.tidy();
    if ask.people.is_empty() {
        return Err(bad("`people` names nobody with an e-mail address"));
    }
    if asked > MAX_SCHEDULES {
        return Err(bad(format!("`people` names more than {MAX_SCHEDULES}, which is the most that can be asked about at once")));
    }
    let Some(result) = v.get("result").filter(|result| !result.is_null()) else {
        return Ok(json!({"ask": ask}));
    };
    let mut answer: FreeBusy = serde_json::from_value(result.clone())
        .map_err(|e| bad(format!("`result`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(200).collect::<String>())))?;
    answer.tidy();
    let local = |instant: i64| DateTime::from_timestamp(instant, offset_at(instant)).iso();
    // Everybody who was asked about, in that order; of whom nothing came back, nothing is known.
    let people: Vec<Value> = ask
        .people
        .iter()
        .map(|email| {
            let said = answer.people.iter().find(|person| person.email.eq_ignore_ascii_case(email));
            let busy: Vec<Value> = merge_busy(said.map_or(&[][..], |person| &person.busy), from, to)
                .into_iter()
                .map(|time| json!({"start": local(time.start), "end": local(time.end), "showAs": time.show_as}))
                .collect();
            let error = match said {
                Some(person) => person.error.clone(),
                None => "the server said nothing about them".to_string(),
            };
            if error.is_empty() { json!({"email": email, "busy": busy}) } else { json!({"email": email, "busy": busy, "error": error}) }
        })
        .collect();
    Ok(json!({"from": local(from), "to": local(to), "people": people}))
}
