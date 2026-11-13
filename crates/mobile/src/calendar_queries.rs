// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar's two typed reads besides the range (`Core::calendar`): the calendars the owner
//! has, by account, and one event whole for the editor and the detail page. Both only read; an
//! event is made, changed, answered and deleted by the `calendar.*` commands through `execute`.
//! What a server or an invitation wrote is cut to size and every list is held to a count.

use mailcraft_engine::Session;
use mailcraft_engine::cmd::calendar::current_calendar;
use mailcraft_model::{Event, Recurrence};
use mailcraft_time::Weekday;

use crate::queries::{address, bad, code, my_answer, rgb};
use crate::records::{AttendeeRecord, CalendarList, CalendarRecord, EventDetail, RecurrenceRecord};
use crate::text::{self, MAX_ADDRESS, MAX_LISTED, MAX_LOCATION, MAX_NAME, MAX_TITLE};
use crate::{Core, CoreError};

/// The longest description handed over, in characters.
const MAX_DESCRIPTION: usize = 20_000;
/// Most days of the week one rule names.
const MAX_WEEKDAYS: usize = 7;

fn weekday(d: &Weekday) -> String {
    code(d).to_lowercase()
}

fn recurrence(r: &Recurrence) -> RecurrenceRecord {
    RecurrenceRecord {
        freq: code(&r.freq),
        interval: r.interval.max(1),
        by_day: text::first(&r.by_day, MAX_WEEKDAYS, weekday),
        nth: r.nth,
        count: r.count,
        until: r.until.map(|d| d.iso()),
    }
}

fn detail(s: &Session, e: &Event) -> EventDetail {
    // Whole days read as they are written: dates, the last day included.
    let (start, end) =
        if e.all_day { (e.start.date.iso(), e.end.add_minutes(-1).date.max(e.start.date).iso()) } else { (e.start.iso(), e.end.iso()) };
    let calendar = s.mailbox.calendar(e.calendar);
    EventDetail {
        rev: s.rev(),
        event: e.id,
        calendar: e.calendar,
        title: text::clip(&e.title, MAX_TITLE),
        location: text::clip(&e.location, MAX_LOCATION),
        description: text::clip(&e.description, MAX_DESCRIPTION),
        start,
        end,
        all_day: e.all_day,
        organizer: e.organizer.as_ref().map(address),
        attendees: text::first(&e.attendees, text::MAX_PEOPLE_IN_ITEM, |a| AttendeeRecord {
            name: text::clip(&a.address.name, MAX_NAME),
            address: text::clip(&a.address.email, MAX_ADDRESS),
            optional: a.optional,
            response: code(&a.response),
        }),
        recurrence: e.recurrence.as_ref().map(recurrence),
        reminder_minutes: e.reminder_minutes,
        show_as: code(&e.show_as),
        meeting: e.is_meeting(),
        invited: e.origin.guest,
        answer: my_answer(s, e),
        cancelled: e.cancelled,
        // An event in a calendar that is not there (hostile data) can't be changed.
        editable: calendar.is_some_and(|c| c.can_edit()) && !e.origin.guest,
        on_server: e.origin.account != 0,
    }
}

#[uniffi::export]
impl Core {
    /// The calendars, each with the account that keeps it (0 for one on this phone only), and
    /// the one new events go into.
    pub async fn calendars(&self) -> Result<CalendarList, CoreError> {
        self.call(|s| {
            let calendars = text::first(&s.mailbox.calendars, MAX_LISTED, |c| CalendarRecord {
                id: c.id,
                account: c.origin.account,
                name: text::clip(&c.name, MAX_NAME),
                color: rgb(c.color),
                visible: c.visible,
                editable: c.can_edit(),
            });
            Ok(CalendarList { rev: s.rev(), calendars, current: current_calendar(s) })
        })
        .await
    }

    /// One event whole (a series as the series it is). An id that is no event is a bad request.
    pub async fn event(&self, id: u64) -> Result<EventDetail, CoreError> {
        self.call(move |s| match s.mailbox.event(id) {
            Some(e) => Ok(detail(s, e)),
            None => Err(bad("there is no such event")),
        })
        .await
    }
}
