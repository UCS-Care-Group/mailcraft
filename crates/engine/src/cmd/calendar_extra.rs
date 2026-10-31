// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendar, the commands the grid needs beyond `calendar.rs`: copying an event (a drag with Ctrl
//! held, Copy and Paste), the number of days in view, and how the calendar is drawn (its time
//! scale, week numbers).

use mailcraft_model::Event;
use mailcraft_time::DateTime;
use serde_json::{Value, json};

use super::{bad, fail, remote};
use crate::{CalView, CmdResult, CommandSpec, Module, Session, p};

/// The time scales the calendar's hours can be drawn at: the minutes one row stands for.
pub const TIME_SCALES: [u8; 6] = [60, 30, 15, 10, 6, 5];
/// The scale a calendar starts with, and falls back to.
pub const DEFAULT_TIME_SCALE: u8 = 30;
/// Most days the Day view shows side by side.
pub const MAX_DAYS: u8 = 14;

const DAY_MINUTES: i64 = 24 * 60;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("calendar.copy", "Copy Event", "Appointment", copy)
            .params(r#"{"start": "YYYY-MM-DDTHH:MM" (a date alone for whole days), "end"?: same (default: keep the length; for whole days it names the last day), "allDay"?: bool (default: as the original), "calendar"?: number (default: the original's), "id"?: number} — the copy happens once, whatever the original does, and nobody is invited to it"#)
            .selection(needs_event),
        CommandSpec::new("view.days", "Number of Days", "Home › Arrange", days)
            .pure()
            .params(r#"{"count": 1-14, "start"?: "YYYY-MM-DD" (default: the day in view)} — the Day view with that many days side by side"#),
        CommandSpec::new("view.next7Days", "Next 7 Days", "Home › Go To", next_7_days).pure(),
        CommandSpec::new("view.timeScale", "Time Scale", "View › Arrangement", time_scale_set)
            .pure()
            .params(r#"{"minutes": 60|30|15|10|6|5} — what one row of the calendar's hours stands for"#),
        CommandSpec::new("view.weekNumbers", "Week Numbers", "File › Options", week_numbers)
            .pure()
            .params(r#"{"value"?: bool} (default: toggle) — the week of the year beside each week of the date navigator and the Month view"#),
    ]
}

fn needs_event(s: &Session) -> Option<&'static str> {
    if s.sel.event.is_none() { Some("select an appointment first") } else { None }
}

/// The time scale in effect: the preference, when it is one of [`TIME_SCALES`].
pub fn time_scale(s: &Session) -> u8 {
    if TIME_SCALES.contains(&s.prefs.time_scale) { s.prefs.time_scale } else { DEFAULT_TIME_SCALE }
}

/// How many days the Day view shows: 1 to [`MAX_DAYS`].
pub fn days_shown(s: &Session) -> i64 {
    i64::from(s.view.days.clamp(1, MAX_DAYS))
}

/// A copy of an event (of one occurrence, for a series) at another time: an appointment of its
/// own, which doesn't repeat and to which nobody is invited, in the original's calendar unless
/// another is named. In a calendar that a server keeps, the server is to have it too.
fn copy(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::id_strict(v, "id")?.or(s.sel.event).ok_or_else(|| fail("select an appointment first"))?;
    let source = s.mailbox.event(id).cloned().ok_or_else(|| failed!("no event with id {id}", id))?;
    let start = p::datetime(v, "start")?.ok_or_else(|| bad("`start` is required"))?;
    let end = p::datetime(v, "end")?;
    let all_day = p::bool(v, "allDay").unwrap_or(source.all_day);
    let length = source.duration_minutes();
    let (start, end) = if all_day {
        // Whole days, midnight to midnight; `end` names the last day.
        let first = start.date;
        let last = match end {
            Some(end) if end.date >= first => end.date,
            Some(_) => return Err(fail("the end is before the start")),
            // Whole days keep their number; a timed event made all-day is the one day.
            None if source.all_day => first.add_days((length / DAY_MINUTES).clamp(1, 36_600) - 1),
            None => first,
        };
        (DateTime::at_midnight(first), DateTime::at_midnight(last.add_days(1)))
    } else {
        let end = match end {
            Some(end) if end >= start => end,
            Some(_) => return Err(fail("the end is before the start")),
            // Whole days copied into the hours are a half-hour appointment there.
            None => start.add_minutes(if source.all_day { 30 } else { length }),
        };
        (start, end)
    };
    let calendar = match p::id(v, "calendar") {
        Some(c) => s.mailbox.calendar(c).map(|calendar| calendar.id).ok_or_else(|| failed!("no calendar with id {id}", id = c))?,
        None => source.calendar,
    };
    let copy = Event {
        calendar,
        title: source.title.clone(),
        location: source.location.clone(),
        description: source.description.clone(),
        start,
        end,
        all_day,
        reminder_minutes: source.reminder_minutes,
        show_as: source.show_as,
        categories: source.categories.clone(),
        private: source.private,
        importance: source.importance,
        ..Default::default()
    };
    let made = s.mailbox.add_event(copy)?;
    remote::note_created(s, made)?;
    s.sel.event = Some(made);
    s.sel.event_day = Some(start.date);
    Ok(json!({"id": made, "copyOf": id}))
}

fn days(s: &mut Session, v: &Value) -> CmdResult {
    let count = p::u64(v, "count").filter(|n| (1..=u64::from(MAX_DAYS)).contains(n)).ok_or_else(|| bad("`count` is 1 to 14"))?;
    if let Some(start) = p::date(v, "start")? {
        s.view.date = start;
    }
    s.view.calendar = CalView::Day;
    s.view.days = u8::try_from(count).unwrap_or(1);
    s.view.module = Module::Calendar;
    Ok(range(s))
}

fn next_7_days(s: &mut Session, _: &Value) -> CmdResult {
    s.view.date = s.today();
    s.view.calendar = CalView::Day;
    s.view.days = 7;
    s.view.module = Module::Calendar;
    Ok(range(s))
}

fn range(s: &Session) -> Value {
    let (from, to) = super::view::calendar_range(s);
    json!({"view": s.view.calendar, "days": days_shown(s), "range": {"from": from.iso(), "to": to.iso(), "date": s.view.date.iso()}})
}

fn time_scale_set(s: &mut Session, v: &Value) -> CmdResult {
    let wrong = || bad("`minutes` is 60, 30, 15, 10, 6 or 5");
    let minutes = p::u64(v, "minutes").and_then(|m| u8::try_from(m).ok()).filter(|m| TIME_SCALES.contains(m)).ok_or_else(wrong)?;
    s.prefs.time_scale = minutes;
    Ok(json!({"minutes": minutes}))
}

fn week_numbers(s: &mut Session, v: &Value) -> CmdResult {
    s.prefs.week_numbers = p::bool(v, "value").unwrap_or(!s.prefs.week_numbers);
    Ok(json!({"value": s.prefs.week_numbers}))
}

#[cfg(test)]
mod tests {
    use mailcraft_model::{Address, Attendee, Freq, Recurrence, ShowAs};
    use mailcraft_time::Date;

    use super::*;

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;

    fn session() -> Session {
        Session::at(crate::sample::mailbox(NOW, 0), NOW, 0)
    }

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    fn at(day: Date, h: u8, m: u8) -> DateTime {
        DateTime::new(day, h, m, 0).unwrap()
    }

    fn event_id(s: &Session, title: &str) -> u64 {
        s.mailbox.events.values().find(|e| e.title.starts_with(title)).map(|e| e.id).unwrap()
    }

    #[test]
    fn a_copy_is_an_appointment_of_its_own_at_the_new_time() {
        let mut s = session();
        let review = event_id(&s, "Design review");
        let before = s.mailbox.event(review).cloned().unwrap();
        assert!(before.is_meeting());
        let made = s.run("calendar.copy", &json!({"id": review, "start": "2026-10-16T09:15"})).unwrap();
        assert_eq!(made["copyOf"], review);
        let copy = s.mailbox.event(made["id"].as_u64().unwrap()).cloned().unwrap();
        // The same thing at another time, as long as the original; nobody is invited to it.
        assert_eq!(
            (copy.title.as_str(), copy.location.as_str(), copy.description.as_str()),
            (before.title.as_str(), before.location.as_str(), before.description.as_str())
        );
        assert_eq!((copy.start, copy.end), (at(d(2026, 10, 16), 9, 15), at(d(2026, 10, 16), 9, 15).add_minutes(before.duration_minutes())));
        assert_eq!(
            (copy.show_as, copy.reminder_minutes, &copy.categories, copy.private),
            (before.show_as, before.reminder_minutes, &before.categories, before.private)
        );
        assert!(!copy.is_meeting() && copy.organizer.is_none() && copy.calendar == before.calendar && copy.id != before.id && copy.uid.is_empty());
        // The original is as it was, the copy is selected, and it is one step to undo.
        assert_eq!(s.mailbox.event(review), Some(&before));
        assert_eq!((s.sel.event, s.sel.event_day), (Some(copy.id), Some(d(2026, 10, 16))));
        assert_eq!(s.undo_label(), Some("Copy Event"));
        assert!(s.undo() && s.mailbox.event(copy.id).is_none());

        // The selection is copied when nothing is named; an end and another calendar when they are.
        s.run("calendar.addCalendar", &json!({"name": "Side"})).unwrap();
        let side = s.mailbox.calendars.iter().find(|c| c.name == "Side").map(|c| c.id).unwrap();
        s.run("calendar.select", &json!({"id": review})).unwrap();
        let made = s.run("calendar.copy", &json!({"start": "2026-10-16T09:00", "end": "2026-10-16T09:20", "calendar": side})).unwrap();
        let copy = s.mailbox.event(made["id"].as_u64().unwrap()).unwrap();
        assert_eq!((copy.duration_minutes(), copy.calendar), (20, side));
    }

    #[test]
    fn a_copy_of_a_series_happens_once_and_whole_days_stay_whole() {
        let mut s = session();
        let standup = event_id(&s, "Team stand-up");
        assert!(s.mailbox.event(standup).unwrap().recurrence.is_some());
        let made = s.run("calendar.copy", &json!({"id": standup, "start": "2026-10-17T10:00"})).unwrap();
        let copy = s.mailbox.event(made["id"].as_u64().unwrap()).cloned().unwrap();
        assert!(copy.recurrence.is_none() && copy.exceptions.is_empty());
        assert_eq!(copy.occurrences(d(2026, 10, 1), d(2026, 10, 31)).len(), 1);

        // Two whole days stay two whole days; `end` names the last one.
        let planning = event_id(&s, "Planning days");
        let made = s.run("calendar.copy", &json!({"id": planning, "start": "2026-10-28"})).unwrap();
        let copy = s.mailbox.event(made["id"].as_u64().unwrap()).cloned().unwrap();
        assert_eq!((copy.all_day, copy.start, copy.end), (true, at(d(2026, 10, 28), 0, 0), at(d(2026, 10, 30), 0, 0)));
        let made = s.run("calendar.copy", &json!({"id": planning, "start": "2026-10-28T13:45", "end": "2026-10-30"})).unwrap();
        let copy = s.mailbox.event(made["id"].as_u64().unwrap()).cloned().unwrap();
        assert_eq!((copy.all_day, copy.start, copy.end), (true, at(d(2026, 10, 28), 0, 0), at(d(2026, 10, 31), 0, 0)));
        // Into the hours it is half an hour; a timed one into whole days is the one day.
        let made = s.run("calendar.copy", &json!({"id": planning, "start": "2026-10-28T13:30", "allDay": false})).unwrap();
        let copy = s.mailbox.event(made["id"].as_u64().unwrap()).cloned().unwrap();
        assert_eq!((copy.all_day, copy.start, copy.end), (false, at(d(2026, 10, 28), 13, 30), at(d(2026, 10, 28), 14, 0)));
        let made = s.run("calendar.copy", &json!({"id": standup, "start": "2026-10-28T13:30", "allDay": true})).unwrap();
        let copy = s.mailbox.event(made["id"].as_u64().unwrap()).cloned().unwrap();
        assert_eq!((copy.all_day, copy.start, copy.end), (true, at(d(2026, 10, 28), 0, 0), at(d(2026, 10, 29), 0, 0)));
    }

    #[test]
    fn what_cannot_be_copied_is_refused_and_nothing_changes() {
        let mut s = session();
        let review = event_id(&s, "Design review");
        let before = s.mailbox.clone();
        for params in [
            json!({}),
            json!({"id": review}),
            json!({"id": review, "start": "soon"}),
            json!({"id": review, "start": "2026-10-16T09:00", "end": "2026-10-16T08:00"}),
            json!({"id": review, "start": "2026-10-16", "end": "2026-10-15", "allDay": true}),
            json!({"id": review, "start": "2026-10-16T09:00", "calendar": 999_999}),
            json!({"id": 999_999, "start": "2026-10-16T09:00"}),
            json!({"id": review, "start": 7}),
        ] {
            assert!(s.run("calendar.copy", &params).is_err(), "{params}");
        }
        assert!(s.mailbox == before);
        // The ends of the calendar, and an event as long as the calendar.
        let _ = s.mailbox.add_event(Event {
            title: "Forever".into(),
            start: at(d(1, 1, 1), 0, 0),
            end: at(d(9999, 12, 31), 23, 59),
            all_day: true,
            attendees: vec![Attendee { address: Address::new("Bo", "bo@example.org"), ..Default::default() }],
            recurrence: Some(Recurrence::every(Freq::Daily)),
            show_as: ShowAs::OutOfOffice,
            ..Default::default()
        });
        let forever = event_id(&s, "Forever");
        for start in ["9999-12-31T23:59", "0001-01-01", "9999-12-31"] {
            for all_day in [true, false] {
                let _ = s.run("calendar.copy", &json!({"id": forever, "start": start, "allDay": all_day}));
            }
        }
    }

    #[test]
    fn the_day_view_shows_as_many_days_as_asked_for() {
        let mut s = session();
        let range = |s: &Session| {
            let (from, to) = crate::cmd::view::calendar_range(s);
            (from.iso(), to.iso())
        };
        let shown = s.run("view.days", &json!({"count": 3, "start": "2026-10-20"})).unwrap();
        assert_eq!(
            (shown["days"].as_i64(), shown["range"]["from"].as_str(), shown["range"]["to"].as_str()),
            (Some(3), Some("2026-10-20"), Some("2026-10-22"))
        );
        assert_eq!((s.view.calendar, s.view.module, s.view.days), (CalView::Day, Module::Calendar, 3));
        // Forward and Back go by as many days as are shown.
        s.run("view.next", &json!({})).unwrap();
        assert_eq!(range(&s), ("2026-10-23".to_string(), "2026-10-25".to_string()));
        s.run("view.previous", &json!({})).unwrap();
        s.run("view.previous", &json!({})).unwrap();
        assert_eq!(range(&s), ("2026-10-17".to_string(), "2026-10-19".to_string()));
        // Without a start, from the day in view; Today starts the days at today.
        s.run("view.days", &json!({"count": 10})).unwrap();
        assert_eq!(range(&s), ("2026-10-17".to_string(), "2026-10-26".to_string()));
        s.run("view.today", &json!({})).unwrap();
        assert_eq!(range(&s), ("2026-10-14".to_string(), "2026-10-23".to_string()));
        // The view's own buttons show the view as it is: Day is one day again.
        s.run("view.day", &json!({})).unwrap();
        assert_eq!((range(&s), s.view.days), (("2026-10-14".to_string(), "2026-10-14".to_string()), 0));
        s.run("view.days", &json!({"count": 5})).unwrap();
        s.run("view.week", &json!({})).unwrap();
        assert_eq!((range(&s), s.view.days), (("2026-10-11".to_string(), "2026-10-17".to_string()), 0));
        // The next seven days start today, wherever the calendar was.
        s.run("view.goToDate", &json!({"date": "2027-03-01"})).unwrap();
        s.run("view.next7Days", &json!({})).unwrap();
        assert_eq!((range(&s), s.view.calendar), (("2026-10-14".to_string(), "2026-10-20".to_string()), CalView::Day));
        // None of it is a change to the mailbox.
        assert!(!s.dirty && s.undo_label().is_none());
        for params in [json!({}), json!({"count": 0}), json!({"count": 15}), json!({"count": "3"}), json!({"count": 3, "start": "someday"})] {
            assert!(s.run("view.days", &params).is_err(), "{params}");
        }
        // A number nobody could have set still shows a day, and no more than there may be.
        s.view.calendar = CalView::Day;
        s.view.days = u8::MAX;
        assert_eq!(days_shown(&s), i64::from(MAX_DAYS));
        s.view.date = d(9999, 12, 31);
        let _ = range(&s);
        s.run("view.next", &json!({})).unwrap();
    }

    /// Settings that were saved before these were added (the owner's `ui.json`) load as they
    /// were, with the new ones at what they start as.
    #[test]
    fn settings_saved_before_these_were_added_still_load() {
        let prefs: crate::Prefs = serde_json::from_value(json!({"twelveHour": false, "workStart": 9, "syncMinutes": 15})).unwrap();
        assert_eq!((prefs.time_scale, prefs.week_numbers), (DEFAULT_TIME_SCALE, false));
        assert_eq!((prefs.twelve_hour, prefs.work_start, prefs.sync_minutes), (false, 9, 15));
        let view: crate::ViewState = serde_json::from_value(json!({"calendar": "month", "folderPane": false, "inCalendar": 7})).unwrap();
        assert_eq!((view.days, view.calendar, view.folder_pane, view.in_calendar), (0, CalView::Month, false, 7));
        assert_eq!(serde_json::from_value::<crate::Prefs>(json!({})).unwrap(), crate::Prefs::default());
        assert_eq!(serde_json::from_value::<crate::ViewState>(json!({})).unwrap(), crate::ViewState::default());
        // What they are saved as, and that they come back.
        let saved = serde_json::to_value(crate::Prefs { time_scale: 15, week_numbers: true, ..Default::default() }).unwrap();
        assert_eq!((saved["timeScale"].as_u64(), saved["weekNumbers"].as_bool()), (Some(15), Some(true)));
        assert_eq!(serde_json::from_value::<crate::Prefs>(saved).unwrap().time_scale, 15);
        let saved = serde_json::to_value(crate::ViewState { days: 3, ..Default::default() }).unwrap();
        assert_eq!(saved["days"], 3);
        assert_eq!(serde_json::from_value::<crate::ViewState>(saved).unwrap().days, 3);
        // Nonsense in their place is refused by the file's reader rather than taken in; a
        // number nobody offers is the usual scale when it is used.
        assert!(serde_json::from_value::<crate::Prefs>(json!({"timeScale": "fast"})).is_err());
        let odd: crate::Prefs = serde_json::from_value(json!({"timeScale": 7})).unwrap();
        let mut s = session();
        s.prefs = odd;
        assert_eq!(time_scale(&s), DEFAULT_TIME_SCALE);
    }

    #[test]
    fn the_time_scale_and_week_numbers_are_preferences() {
        let mut s = session();
        assert_eq!((time_scale(&s), s.prefs.week_numbers), (30, false));
        for minutes in TIME_SCALES {
            assert_eq!(s.run("view.timeScale", &json!({"minutes": minutes})).unwrap()["minutes"], minutes);
            assert_eq!(time_scale(&s), minutes);
        }
        for params in
            [json!({}), json!({"minutes": 0}), json!({"minutes": 7}), json!({"minutes": 300}), json!({"minutes": "15"}), json!({"minutes": -5})]
        {
            assert!(s.run("view.timeScale", &params).is_err(), "{params}");
        }
        assert_eq!(time_scale(&s), 5);
        // A scale nobody could have set is the usual one.
        s.prefs.time_scale = 0;
        assert_eq!(time_scale(&s), DEFAULT_TIME_SCALE);
        assert_eq!(s.run("view.weekNumbers", &json!({})).unwrap()["value"], true);
        assert_eq!(s.run("view.weekNumbers", &json!({"value": true})).unwrap()["value"], true);
        assert_eq!(s.run("view.weekNumbers", &json!({})).unwrap()["value"], false);
        assert!(!s.dirty);
    }
}
