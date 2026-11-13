// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendars of an account kept in step over JMAP (JMAP for Calendars, events as JSCalendar,
//! RFC 8984), in the window: read from its server by themselves, and never written there.
//!
//! The host does the talking ([`crate::Services::jmap_calendar`]): one small job lists the
//! account's calendars and reads the events of each in a stretch of days (a month back, a year
//! ahead, as a Send/Receive of a Microsoft account brings down). What it brings is taken in as a
//! Microsoft account's is (`mailcraft_engine::cmd::remote::apply_calendars`, then the events of
//! each calendar), and every calendar is one the account only reads here: a command that would
//! change an event of one is refused by the engine. A job goes with the secret the account's
//! Send/Receive goes with ([`MailApp::jmap_secret`]): none at hand, no job, and nothing here ever
//! asks the owner for one.
//!
//! **When**: right after the first Send/Receive of the account that ended without the server
//! turning its secret down, after every Send/Receive the owner asks for, and otherwise every
//! quarter of an hour while Send/Receive runs by itself; then also when the calendar comes to the
//! front and the last reading is more than two minutes old. One job at a time, never while that
//! account's Send/Receive is under way or waits its turn, nor while its contacts are being read
//! ([`crate::jmap_contacts`]), and never for an account whose secret its server turned down, until
//! the owner asks for its Send/Receive.
//!
//! **A server that keeps no calendars** for the account says so once, and is not asked again this
//! session unless the owner asks for the account's Send/Receive by hand. A job that came to
//! nothing is tried again after a while, twice as long after each one more; a server that says it
//! is busy is left alone for as long as it says.
//!
//! **Work started for one mailbox stays with it**: a job under way when another mailbox takes its
//! place is called off, and an answer is taken in only by the account it was asked for, while it
//! still has the address and the server it was asked with.
//!
//! When a job is due, the wait after one that came to nothing, and whose an answer is, are what
//! every such job shares ([`crate::jmap_side_job`]); here is what is asked for calendars, the
//! coming to the front, and how what comes is taken in.
//!
//! The log names the account's id and counts, never an address, the server's, a calendar's name
//! or anything of an event.

use mailcraft_engine::Module;
use mailcraft_engine::cmd::remote;
use mailcraft_model::sync::{JmapCalendarRequest, JmapCalendarResult};
use mailcraft_model::{Account, AccountKind, Id};

use crate::calendar_sync::{WIDE_AHEAD, WIDE_BEHIND};
use crate::jmap_side_job::{Answer, Asked, EVERY_MS, MAX_PROBLEMS, SideJob};
use crate::{MailApp, now_ms};

/// How old the last reading may be when the calendar comes to the front, before it is asked for
/// again (milliseconds).
const IN_FRONT_MS: f64 = 120_000.0;
/// Most calendars of one answer whose events are taken in.
const MAX_DOWNLOADS: usize = 64;

impl Answer for JmapCalendarResult {
    const JOB: &'static str = "jmap calendar";
    const READS: &'static str = "calendars";

    fn whose(&self) -> (Id, &str, &str) {
        (self.account, &self.email, &self.session)
    }
}

/// What the app remembers of reading JMAP accounts' calendars.
#[derive(Default)]
pub(crate) struct JmapCalendar {
    /// The job, and when the next is due ([`crate::jmap_side_job`]).
    side: SideJob<JmapCalendarResult>,
    /// The calendar was in front at the last look.
    in_front: bool,
    /// The calendar has come to the front since anything was last asked for: a reading older than
    /// [`IN_FRONT_MS`] is asked for again.
    came_front: bool,
}

impl JmapCalendar {
    /// Another mailbox took the place of the open one ([`SideJob::forget`]).
    pub(crate) fn forget(&mut self) {
        self.side.forget();
        *self = Self::default();
    }

    /// `account` is gone, or names another server now ([`SideJob::forget_account`]).
    pub(crate) fn forget_account(&mut self, account: Id) {
        self.side.forget_account(account);
    }

    /// The owner asked for a Send/Receive of `account`: its calendars follow ([`SideJob::by_hand`]).
    pub(crate) fn by_hand(&mut self, account: Id) {
        self.side.by_hand(account);
    }

    /// A Send/Receive of `account` has ended well enough ([`SideJob::after_run`]).
    pub(crate) fn after_run(&mut self, account: Id) {
        self.side.after_run(account);
    }

    /// What the job in flight brought, when it has ([`SideJob::answer`]). With a job that was for a
    /// mailbox that is no longer open, the look at the calendar goes too.
    fn answer(&mut self, opening: u64, now: f64) -> Option<(Asked, JmapCalendarResult)> {
        let stale = self.side.stale(opening);
        let got = self.side.answer(opening, now);
        if stale {
            *self = Self::default();
        }
        got
    }
}

impl MailApp {
    /// Whether a JMAP account's calendars are being read.
    pub fn jmap_calendar_syncing(&self) -> bool {
        self.jmap_calendar.side.running()
    }

    /// Take in an answer that has come, and start a job when one is due. Called every frame.
    pub(crate) fn jmap_calendar_tick(&mut self, ctx: &egui::Context) {
        let now = now_ms();
        self.poll_jmap_calendar_at(now);
        self.jmap_calendar_tick_at(now);
        if self.jmap_calendar.side.running() {
            // An answer is on its way (the host wakes the window when it is there, too).
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Whether `account`'s calendars are to be asked for at `now`: as any such job, and after its
    /// contacts; sooner than otherwise when the calendar has come to the front.
    fn jmap_calendar_due(&self, account: Id, now: f64) -> bool {
        let state = &self.jmap_calendar;
        let every = if state.came_front { IN_FRONT_MS } else { EVERY_MS };
        let free = self.jmap_side_free(account) && !self.jmap_contacts.reading(account);
        free && !state.side.held(account, now) && state.side.wanted(account, now, every, self.auto_sync)
    }

    /// [`jmap_calendar_tick`](Self::jmap_calendar_tick) at `now` (milliseconds), without looking
    /// for an answer. Whether a job was started. (Public for a host's tests, which have no window.)
    pub fn jmap_calendar_tick_at(&mut self, now: f64) -> bool {
        // The calendar has come to the front: what is shown may be older than the owner expects.
        let in_front = self.session.view.module == Module::Calendar && !self.ui.backstage;
        if in_front && !self.jmap_calendar.in_front {
            self.jmap_calendar.came_front = true;
        }
        self.jmap_calendar.in_front = in_front;
        if self.jmap_calendar.side.running() || self.services.jmap_calendar.is_none() || self.unconfirmed || self.data_removed || self.offline() {
            return false;
        }
        let theirs = |a: &&Account| a.kind == AccountKind::Jmap && !a.is_shared();
        let accounts: Vec<Id> = self.session.mailbox.accounts.iter().filter(theirs).map(|a| a.id).collect();
        // The stretch of days: a month back, a year ahead, by the session's clock.
        let today = self.session.now();
        let (from, to) = (today.saturating_sub(WIDE_BEHIND), today.saturating_add(WIDE_AHEAD));
        for account in accounts {
            if !self.jmap_calendar_due(account, now) {
                continue;
            }
            let Some(asking) = self.jmap_side_asking(account) else { continue };
            let (email, session) = (asking.email.clone(), asking.session.clone());
            let request = JmapCalendarRequest { account, email, session, bearer: asking.bearer, from, to };
            let Some(ask) = self.services.jmap_calendar.as_ref() else { return false };
            let (job, asked) = asking.job(request, self.session.openings());
            self.jmap_calendar.side.start(now, asked, ask(job));
            return true;
        }
        // Nobody to ask: the wish to ask sooner doesn't outlive the look.
        self.jmap_calendar.came_front = false;
        false
    }

    /// Take in what the job in flight brought, when it has: the calendars, their events, and what
    /// went wrong. (Public for a host's tests.)
    pub fn poll_jmap_calendar_at(&mut self, now: f64) {
        let Some((job, result)) = self.jmap_calendar.answer(self.session.openings(), now) else { return };
        let account = job.account;
        if !self.jmap_side_meant(&job, &result, |a| !a.is_shared()) {
            return;
        }
        let mut errors: Vec<String> = result.errors.into_iter().take(MAX_PROBLEMS).collect();
        if self.jmap_side_ended(|app| &mut app.jmap_calendar.side, account, result.auth_refused, result.not_offered) {
            self.note_sync_problems(account, &errors, false);
            return;
        }
        // The list first: it makes the calendars the events go into. A list that names no main
        // calendar makes nothing (the engine can't tell it from the one that is here), and events
        // are not taken in without their calendars.
        let mut taken = false;
        if let Some(list) = result.calendars.as_ref().filter(|list| list.account == account) {
            let main = list.calendars.iter().filter(|calendar| calendar.main).count();
            match remote::apply_calendars(&mut self.session, list) {
                Ok(_) if main == 1 => taken = true,
                Ok(_) => log::warn!("jmap calendar: the list names {main} main calendars; its events are left"),
                Err(e) => {
                    errors.push(crate::i18n::fmt(tl!("The list of calendars couldn't be taken in: {error}"), &[("error", &e.to_string())]));
                }
            }
        }
        if taken {
            let (mut calendars, mut events) = (0usize, 0usize);
            for download in result.events.iter().take(MAX_DOWNLOADS).filter(|download| download.account == account) {
                if self.take_in_events(account, download, &mut errors) {
                    calendars += 1;
                    events = events.saturating_add(download.events.len());
                }
            }
            log::debug!("jmap calendar: {events} event(s) of {calendars} calendar(s) taken in, {} problem(s)", errors.len());
        }
        errors.truncate(MAX_PROBLEMS);
        self.jmap_calendar.side.settle(account, now, result.busy_for, result.stopped, taken && errors.is_empty());
        if !errors.is_empty() {
            log::warn!("jmap calendar: {} problem(s)", errors.len());
        }
        self.note_sync_problems(account, &errors, false);
    }
}

#[cfg(test)]
#[path = "jmap_calendar_tests.rs"]
mod tests;
