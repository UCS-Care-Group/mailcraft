// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar of an Exchange server account, read for the days the owner goes to (#1274).
//!
//! A Send/Receive reads the account's calendars a month back and a year ahead, with its mail
//! (`sync.ewsRequest`, [`crate::ews_mail`]). Days the owner goes to beyond that, and the days on
//! screen when the last look at them is a few minutes old, are read by a small job of their own
//! while the calendar is in front: the account's calendars (and those it opened by address) alone,
//! for the days on screen and a week either side, through the host
//! ([`crate::Services::ews_calendar`]). Microsoft accounts' calendars are kept the same way
//! ([`crate::calendar_sync`]).
//!
//! **It reads and sends nothing.** What was changed in the calendar here goes up with the next
//! Send/Receive, as before, and the merge leaves an event that waits to go as it is here. So that
//! an answer asked for before a Send/Receive sent a change is never taken in after it, the job is
//! called off when a Send/Receive of an Exchange server account starts, and none starts while one
//! is under way.
//!
//! **It asks for nothing.** The password is the one a Send/Receive was given this session, or the
//! one the operating system remembers; without one, nothing is read until a Send/Receive asks for
//! it. A password the server turns down is forgotten as a Send/Receive forgets it, and a
//! certificate that needs a say stops the account's reads until the owner has given it
//! (File › Info).
//!
//! One job at a time, with Automatic Send/Receive on and the window online; a server that didn't
//! answer is given longer each time.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_engine::Module;
use mailcraft_engine::cmd::ews_directory::{Part, apply_ews_directory, ews_directory_request};
use mailcraft_model::sync::{EwsDirectoryRequest, EwsDirectoryResult};
use mailcraft_model::{AccountKind, Id};
use mailcraft_time::Timestamp;

use crate::{MailApp, MailRun};

/// How long a stretch that was read counts as current while the calendar is in front
/// (milliseconds): after that it is read again.
const FRESH_MS: f64 = 300_000.0;
/// How long the server is left alone after a read that came to nothing: this long after the
/// first, twice as long after each one more, and never longer than the second (milliseconds).
const FIRST_RETRY_MS: f64 = 60_000.0;
const LONGEST_RETRY_MS: f64 = 600_000.0;
/// Most stretches remembered for an account.
const MAX_READ: usize = 16;

/// What the host is asked to read ([`crate::Services::ews_calendar`]).
pub struct EwsCalendarJob {
    /// The calendars alone (`Part::Calendar`): nothing in it goes up.
    pub request: EwsDirectoryRequest,
    /// As for a run ([`crate::ews_mail::EwsJob::secret`]). Never logged.
    pub secret: String,
    /// Set when the job is called off: it stops before its next request.
    pub cancel: Arc<AtomicBool>,
}

/// A job in flight.
struct Job {
    account: Id,
    /// The mailbox opening it started under: an answer for another mailbox is nobody's.
    opening: u64,
    stretch: (Timestamp, Timestamp),
    cancel: Arc<AtomicBool>,
    rx: Receiver<EwsDirectoryResult>,
}

/// The job in flight, the stretches each account's calendar was read for and when, and the reads
/// that came to nothing.
#[derive(Default)]
pub(crate) struct EwsCalendar {
    job: Option<Job>,
    read: HashMap<Id, Vec<(Timestamp, Timestamp, f64)>>,
    failures: HashMap<Id, (u32, f64)>,
}

impl MailApp {
    /// Whether a read of an Exchange server account's calendar is in flight.
    pub fn ews_calendar_reading(&self) -> bool {
        self.ews_calendar.job.is_some()
    }

    /// Take in what came, and start a read when one is due. Called every frame.
    pub(crate) fn ews_calendar_tick(&mut self, ctx: &egui::Context) {
        let now = crate::now_ms();
        self.poll_ews_calendar(now);
        self.ews_calendar_tick_at(now);
        if self.ews_calendar.job.is_some() {
            // (The host wakes the window when the answer is there, too.)
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if self.session.view.module == Module::Calendar && self.ews_calendar_accounts().next().is_some() {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
    }

    /// The Exchange server accounts whose calendar may be read now.
    fn ews_calendar_accounts(&self) -> impl Iterator<Item = Id> + '_ {
        self.session
            .mailbox
            .accounts
            .iter()
            .filter(|a| a.kind == AccountKind::Exchange)
            .map(|a| a.id)
            .filter(|id| !self.jmap_refused.contains(id) && !self.ews_certificate.contains_key(id))
    }

    /// [`ews_calendar_tick`](Self::ews_calendar_tick) at `now` (milliseconds). Whether a job was started.
    pub(crate) fn ews_calendar_tick_at(&mut self, now: f64) -> bool {
        if !self.auto_sync || self.services.ews_calendar.is_none() || self.ews_calendar.job.is_some() || self.offline() {
            return false;
        }
        if self.session.view.module != Module::Calendar || self.ui.backstage {
            return false;
        }
        // A Send/Receive of an Exchange server account goes first, and may be sending a change.
        if matches!(self.sync_rx, Some(MailRun::Ews { .. })) {
            return false;
        }
        let window = self.calendar_window();
        let accounts: Vec<Id> = self.ews_calendar_accounts().collect();
        for account in accounts {
            // About to have its Send/Receive: that reads the calendar.
            if self.sync_queue.iter().any(|(queued, _)| *queued == account) {
                continue;
            }
            if self.ews_calendar_read_lately(account, window, now) {
                continue;
            }
            let (failures, at) = self.ews_calendar.failures.get(&account).copied().unwrap_or((0, 0.0));
            let patience = (FIRST_RETRY_MS * f64::from(1u32 << failures.saturating_sub(1).min(8))).min(LONGEST_RETRY_MS);
            if failures > 0 && now - at < patience {
                continue;
            }
            // Nothing is asked for: without a password at hand, a Send/Receive asks for it first.
            let Some(secret) = self.ews_secret(account) else { continue };
            let Ok(mut request) = ews_directory_request(&self.session, account, Some(window.0), Some(window.1), Part::Calendar, true) else {
                continue;
            };
            // A certificate a file trusts counts once the owner has confirmed the file (AU3-02).
            if self.unconfirmed {
                request.trusted.clear();
            }
            let Some(run) = self.services.ews_calendar.as_ref() else { return false };
            let cancel = Arc::new(AtomicBool::new(false));
            let stretch = (request.from, request.to);
            let rx = run(EwsCalendarJob { request, secret, cancel: cancel.clone() });
            self.ews_calendar.job = Some(Job { account, opening: self.session.openings(), stretch, cancel, rx });
            return true;
        }
        false
    }

    /// Whether `window` of `account`'s calendar was read in the last few minutes.
    fn ews_calendar_read_lately(&self, account: Id, window: (Timestamp, Timestamp), now: f64) -> bool {
        self.ews_calendar
            .read
            .get(&account)
            .is_some_and(|read| read.iter().any(|(from, to, at)| *from <= window.0 && window.1 <= *to && now - at < FRESH_MS))
    }

    /// `account`'s calendar was read for `from..to` at `now` (a Send/Receive's stretch, or a job's).
    pub(crate) fn ews_calendar_was_read(&mut self, account: Id, from: Timestamp, to: Timestamp, now: f64) {
        let read = self.ews_calendar.read.entry(account).or_default();
        read.retain(|(_, _, at)| now - at < FRESH_MS);
        read.push((from, to, now));
        if read.len() > MAX_READ {
            read.remove(0);
        }
    }

    /// The stretch a Send/Receive's calendar part brought, when it came whole: it counts as read.
    pub(crate) fn ews_calendar_send_receive_read(&mut self, result: &EwsDirectoryResult) {
        if !result.errors.is_empty() || result.auth_refused || !result.untrusted.is_empty() || result.busy_for.is_some() {
            return;
        }
        let from = result.events.iter().map(|download| download.from).min();
        let to = result.events.iter().map(|download| download.to).max();
        if let (Some(from), Some(to)) = (from, to) {
            self.ews_calendar_was_read(result.account, from, to, crate::now_ms());
        }
    }

    /// A Send/Receive of an Exchange server account starts: a read in flight is called off, and
    /// nothing of it is taken in (it may have been asked for before a change went up).
    pub(crate) fn ews_calendar_stop(&mut self) {
        if let Some(job) = self.ews_calendar.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Take in the answer of the job in flight, once it is there.
    pub(crate) fn poll_ews_calendar(&mut self, now: f64) {
        let Some(job) = self.ews_calendar.job.as_ref() else { return };
        let result = match job.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                log::warn!("ews calendar: the read stopped without an answer (its thread ended early)");
                if let Some(job) = self.ews_calendar.job.take() {
                    self.ews_calendar_failed(job.account, now);
                }
                return;
            }
        };
        let Some(job) = self.ews_calendar.job.take() else { return };
        // Another mailbox took the place of the one the read was for: what it brings is nobody's.
        if job.opening != self.session.openings() || result.account != job.account {
            return;
        }
        let account = job.account;
        if result.auth_refused {
            // As a Send/Receive does: the next one asks for the password again.
            self.ews_password_refused(account);
            return;
        }
        if !result.untrusted.is_empty() {
            // The certificate question comes first (File › Info).
            self.ews_certificate.insert(account, result.untrusted.clone());
            self.jmap_refused.insert(account);
            return;
        }
        if !result.signed_in_with.is_empty() {
            self.ews_signed_in.insert(account, result.signed_in_with.clone());
        }
        let came_whole = result.errors.is_empty() && result.busy_for.is_none();
        match apply_ews_directory(&mut self.session, &result) {
            Ok(said) => {
                let first = said["errors"].as_array().and_then(|all| all.iter().find_map(serde_json::Value::as_str)).map(str::to_string);
                if let Some(first) = first {
                    log::warn!("ews calendar: the read of the days on screen said {} problem(s)", result.errors.len().max(1));
                    self.status(first);
                }
            }
            Err(e) => {
                self.status(e.to_string());
                self.ews_calendar_failed(account, now);
                return;
            }
        }
        if came_whole {
            self.ews_calendar.failures.remove(&account);
            self.ews_calendar_was_read(account, job.stretch.0, job.stretch.1, now);
        } else {
            self.ews_calendar_failed(account, now);
        }
    }

    fn ews_calendar_failed(&mut self, account: Id, now: f64) {
        let failures = self.ews_calendar.failures.get(&account).map_or(0, |(n, _)| *n);
        self.ews_calendar.failures.insert(account, (failures.saturating_add(1), now));
    }
}

#[cfg(test)]
#[path = "ews_calendar_tests.rs"]
mod tests;
