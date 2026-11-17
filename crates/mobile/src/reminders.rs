// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reminders a phone has the system raise while the app is closed: the next calendar and task
//! reminders, each with the instant it is due and a key that stays the same from one ask to the
//! next (so the app replaces an alarm instead of adding a second, and remembers what it showed).
//! What is due is the engine's answer (`view::upcoming_reminders`); the app asks again when
//! [`crate::Event::ReminderScheduleChanged`] comes, at every open, after a background round, and
//! when the phone restarts or its clock or zone changes. Nothing here is sent anywhere.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::view::{Upcoming, UpcomingFor, upcoming_reminders};

use crate::text::{self, MAX_LOCATION, MAX_TITLE};
use crate::{Core, CoreError};

/// The most reminders scheduled at once: the plan's number for iPhone, where the system keeps 64
/// local notifications an app, kept on Android too so that both phones remind of the same ones
/// (and far inside Android's 500 alarms an app). Another ask after a sync moves the window on.
pub const MAX_SCHEDULED: u32 = 64;

/// The most keys of reminders already shown that one ask leaves out (the app remembers 400).
pub const MAX_SKIPPED: usize = 1_000;

/// How old the schedule a change is told against may be: time alone moves the four weeks the
/// schedule looks at, so before a job it is looked at afresh when older than this (and not at
/// every job, which would work the reminders out at every read).
const STALE_AFTER_SECONDS: i64 = 60;

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
    /// Soonest first, at most `count` (and [`MAX_SCHEDULED`]), none of those asked to be left out.
    pub reminders: Vec<ScheduledReminder>,
    /// The keys asked to be left out that are still to come (so the app can tell a reminder it
    /// showed from one whose event went away).
    pub kept: Vec<u64>,
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

/// Every reminder still to come that one ask can need: those it gives and those it leaves out.
fn all(s: &Session) -> Vec<ScheduledReminder> {
    let most = usize::try_from(MAX_SCHEDULED).unwrap_or(0).saturating_add(MAX_SKIPPED);
    upcoming_reminders(s, s.now(), most).iter().map(record).collect()
}

/// The next `count` reminders (at most [`MAX_SCHEDULED`]) that aren't in `skip`, and the keys of
/// `skip` that are still to come. The cap is taken after the skipped are left out, so reminders
/// already shown never keep a later one from its alarm.
pub(crate) fn schedule(s: &Session, count: u32, skip: &HashSet<u64>) -> (Vec<ScheduledReminder>, Vec<u64>) {
    let count = usize::try_from(count.min(MAX_SCHEDULED)).unwrap_or(0);
    let (kept, given): (Vec<ScheduledReminder>, Vec<ScheduledReminder>) = all(s).into_iter().partition(|r| skip.contains(&r.key));
    (given.into_iter().take(count).collect(), kept.into_iter().map(|r| r.key).collect())
}

/// Says when the schedule has changed since it was last looked at, on the session thread after
/// a job moved the revision (as the desktop works out its reminders again at a change).
#[derive(Default)]
pub(crate) struct ScheduleWatch {
    last: Option<u64>,
    /// When `last` was worked out (the session's clock).
    taken: i64,
}

impl ScheduleWatch {
    /// True when the schedule differs from the one seen last; the first look only notes it (the
    /// app asks for the schedule at every open).
    pub(crate) fn changed(&mut self, s: &Session) -> bool {
        let mut h = DefaultHasher::new();
        all(s).hash(&mut h);
        let now = h.finish();
        self.taken = s.now();
        let before = self.last.replace(now);
        before.is_some_and(|b| b != now)
    }

    /// Before a job: the schedule is noted again when time alone may have moved it since (an
    /// event came into the four weeks), so that the job's own change is told against it.
    pub(crate) fn before_job(&mut self, s: &Session) {
        if self.last.is_none() || s.now().saturating_sub(self.taken) >= STALE_AFTER_SECONDS {
            let _ = self.changed(s);
        }
    }
}

#[uniffi::export]
impl Core {
    /// The next reminders of events and tasks for the system to raise, soonest first, at most
    /// `count` and [`MAX_SCHEDULED`], leaving out those in `skip` (the ones the app showed
    /// already; at most [`MAX_SKIPPED`] of them are read).
    pub async fn reminder_schedule(&self, count: u32, skip: Vec<u64>) -> Result<ReminderSchedule, CoreError> {
        let skip: HashSet<u64> = skip.into_iter().take(MAX_SKIPPED).collect();
        let held = skip.len().saturating_mul(std::mem::size_of::<u64>());
        self.call_holding(held, move |s| {
            let (reminders, kept) = schedule(s, count, &skip);
            Ok(ReminderSchedule { rev: s.rev(), reminders, kept })
        })
        .await
    }
}
