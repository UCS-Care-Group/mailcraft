// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reminders a phone has the system raise while the app is closed: the next calendar and task
//! reminders, each with the instant it is due and a key that stays the same from one ask to the
//! next (so the app replaces an alarm instead of adding a second, and remembers what it showed).
//! What is due is the engine's answer (`view::upcoming_reminders`); the app asks again when
//! [`crate::Event::ReminderScheduleChanged`] comes, at every open, after a background round, and
//! when the phone restarts or its clock or zone changes. Nothing here is sent anywhere.

use std::hash::{DefaultHasher, Hash, Hasher};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::view::{Upcoming, UpcomingFor, upcoming_reminders};

use crate::text::{self, MAX_LOCATION, MAX_TITLE};
use crate::{Core, CoreError};

/// The most reminders scheduled at once: the plan's number for iPhone, where the system keeps 64
/// local notifications an app, kept on Android too so that both phones remind of the same ones
/// (and far inside Android's 500 alarms an app). Another ask after a sync moves the window on.
pub const MAX_SCHEDULED: u32 = 64;

/// What a reminder is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ReminderKind {
    Event,
    Task,
}

/// One reminder for the system to raise at `at`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ScheduledReminder {
    /// The same for the same reminder at every ask, and another when its event or task moved: an
    /// alarm is known by it, and a reminder already shown or dismissed is remembered by it.
    pub key: u64,
    pub kind: ReminderKind,
    /// When it is due (seconds since 1970, UTC); before now when it was due while the phone was
    /// off and its event hasn't started yet.
    pub at: i64,
    /// When the event starts, or the task's reminder time (seconds since 1970, UTC).
    pub starts: i64,
    pub all_day: bool,
    /// The event's or the task's title, cut to size: shown in the notification, never on a
    /// locked screen that hides private content.
    pub title: String,
    pub location: String,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ReminderSchedule {
    pub rev: u64,
    /// Soonest first, at most `count` (and [`MAX_SCHEDULED`]).
    pub reminders: Vec<ScheduledReminder>,
}

/// FNV-1a: a hash that is the same in every run of the program (the standard library's isn't
/// promised to be), because a key outlives the process in the system's alarms.
fn stable(parts: &[&[u8]]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    parts.iter().fold(OFFSET, |h, part| {
        // (A separator, so that two parts never read as one.)
        part.iter().chain([0xff_u8].iter()).fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(PRIME))
    })
}

fn key_of(u: &Upcoming) -> u64 {
    let at = u.at.to_le_bytes();
    match &u.what {
        UpcomingFor::Event { event, start, .. } => stable(&[b"event", &event.to_le_bytes(), start.iso().as_bytes(), &at]),
        UpcomingFor::Task { task } => stable(&[b"task", &task.to_le_bytes(), &at]),
    }
}

fn record(u: &Upcoming) -> ScheduledReminder {
    let (kind, all_day, location) = match &u.what {
        UpcomingFor::Event { all_day, location, .. } => (ReminderKind::Event, *all_day, text::clip(location, MAX_LOCATION)),
        UpcomingFor::Task { .. } => (ReminderKind::Task, false, String::new()),
    };
    ScheduledReminder { key: key_of(u), kind, at: u.at, starts: u.starts, all_day, title: text::clip(&u.title, MAX_TITLE), location }
}

pub(crate) fn schedule(s: &Session, count: u32) -> Vec<ScheduledReminder> {
    let count = usize::try_from(count.min(MAX_SCHEDULED)).unwrap_or(0);
    upcoming_reminders(s, s.now(), count).iter().map(record).collect()
}

/// Says when the schedule has changed since it was last looked at, on the session thread after
/// a job moved the revision (as the desktop works out its reminders again at a change).
#[derive(Default)]
pub(crate) struct ScheduleWatch {
    last: Option<u64>,
}

impl ScheduleWatch {
    /// True when the schedule differs from the one seen last; the first look only notes it (the
    /// app asks for the schedule at every open).
    pub(crate) fn changed(&mut self, s: &Session) -> bool {
        let mut h = DefaultHasher::new();
        schedule(s, MAX_SCHEDULED).hash(&mut h);
        let now = h.finish();
        let before = self.last.replace(now);
        before.is_some_and(|b| b != now)
    }
}

#[uniffi::export]
impl Core {
    /// The next reminders of events and tasks for the system to raise, soonest first, at most
    /// `count` and [`MAX_SCHEDULED`].
    pub async fn reminder_schedule(&self, count: u32) -> Result<ReminderSchedule, CoreError> {
        self.call(move |s| Ok(ReminderSchedule { rev: s.rev(), reminders: schedule(s, count) })).await
    }
}
