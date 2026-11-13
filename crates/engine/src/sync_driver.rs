// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The sync driver's decisions, with no I/O: when an account's job is due, how long to wait after
//! a failure, whether a change has settled, and where an account's permission for a job stands.
//! A front end hands in the time (milliseconds, from any clock it likes) and asks; the host runs
//! what is decided. Nothing here opens a socket, reads a clock or starts a thread.

use std::collections::HashMap;

pub mod calendar;
#[cfg(test)]
mod calendar_tests;
pub mod offline;
#[cfg(test)]
mod offline_tests;

pub use calendar::{CalendarDriver, MARGIN_DAYS, MAX_ASKED, WIDE_AHEAD, WIDE_BEHIND, consent_wait};
pub use offline::{DAY_MS, THIN_BATCH, Thinned, Thinning};

use mailcraft_model::Id;

/// How one kind of job is paced (all times in milliseconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pacing {
    /// How often an account is read when nothing is waiting to go.
    pub refresh_ms: f64,
    /// How long a change is left to settle before it goes.
    pub quiet_ms: f64,
    /// The wait after a first failure; it doubles with each further one.
    pub first_retry_ms: f64,
    /// The longest wait after failures.
    pub longest_retry_ms: f64,
}

impl Pacing {
    /// The To Do job's: read every few minutes, a change goes after three quiet seconds.
    pub const TASKS: Pacing = Pacing { refresh_ms: 5.0 * 60_000.0, quiet_ms: 3_000.0, first_retry_ms: 30_000.0, longest_retry_ms: 15.0 * 60_000.0 };

    /// The calendar's: a change goes at once (it is on the server as soon as it is in Outlook), a
    /// look every five minutes while the calendar is behind ([`calendar_every_ms`]), and after a
    /// failure this long, twice as long after each one more, never more than ten minutes.
    pub const CALENDAR: Pacing = Pacing { refresh_ms: 300_000.0, quiet_ms: 0.0, first_retry_ms: 30_000.0, longest_retry_ms: 600_000.0 };

    /// How long to wait after `failures` failures in a row: the first wait, doubled for each
    /// further failure (at most eight doublings), and never more than the longest.
    pub fn patience_ms(&self, failures: u32) -> f64 {
        (self.first_retry_ms * f64::from(1u32 << failures.saturating_sub(1).min(8))).min(self.longest_retry_ms)
    }
}

/// How long the fetch of whole messages waits before looking again when there was nothing to fetch.
pub const FETCH_IDLE_MS: f64 = 20_000.0;
/// The longest it waits after batches that brought nothing, one after another.
pub const FETCH_MOST_IDLE_MS: f64 = 15.0 * 60_000.0;

/// How long the fetch of whole messages waits after `stuck` batches one after another brought
/// nothing: [`FETCH_IDLE_MS`], doubling each time, at most [`FETCH_MOST_IDLE_MS`].
pub fn fetch_backoff_ms(stuck: u32) -> f64 {
    (FETCH_IDLE_MS * 2f64.powi(i32::try_from(stuck.min(10)).unwrap_or(10))).min(FETCH_MOST_IDLE_MS)
}

/// How often the calendar is asked for while it is in front of the owner (milliseconds).
pub const CALENDAR_IN_FRONT_MS: f64 = 30_000.0;
/// How often it is asked for while a change that was just sent is yet to be seen on the server.
pub const CALENDAR_RECHECK_MS: f64 = 10_000.0;
/// How often the wide stretch of the calendar and the contacts are brought down.
pub const CALENDAR_DIRECTORY_MS: f64 = 900_000.0;
/// How often the working hours the mailbox has set are asked for: they change when the owner
/// changes them in the mailbox's settings, which is seldom.
pub const CALENDAR_SETTINGS_MS: f64 = 3_600_000.0;

/// How often an account's calendar is asked for: soon after a change that isn't seen on the
/// server yet (`unseen`), else more often while the calendar is in front of the owner.
pub fn calendar_every_ms(unseen: bool, in_front: bool) -> f64 {
    match (unseen, in_front) {
        (true, _) => CALENDAR_RECHECK_MS,
        (false, true) => CALENDAR_IN_FRONT_MS,
        (false, false) => Pacing::CALENDAR.refresh_ms,
    }
}

/// How often an account's Favorites are read when nothing asks for them sooner (milliseconds).
pub const FAVORITES_EVERY_MS: f64 = 900_000.0;
/// How long an account whose server said the sign-in doesn't cover the Favorites is left alone.
pub const FAVORITES_REFUSED_MS: f64 = 86_400_000.0;

/// Whether an account's Favorites are due at `now`: the owner asked (`owner`, from a click, or
/// `asked_for`, a Send/Receive), or a quarter of an hour has gone by since `last` (or the clock
/// went back), and the server didn't say, within the day before (`refused_at`), that the sign-in
/// doesn't cover them: only the owner's own ask overrides that.
pub fn favorites_due(owner: bool, asked_for: bool, last: Option<f64>, refused_at: Option<f64>, now: f64) -> bool {
    let refused = refused_at.is_some_and(|at| (0.0..FAVORITES_REFUSED_MS).contains(&(now - at)));
    let due = owner || asked_for || last.is_none_or(|at| now - at >= FAVORITES_EVERY_MS || now < at);
    due && (owner || !refused)
}

/// How many times a job tries again, quietly, when the owner has just allowed what it needs and
/// the provider hasn't taken that in everywhere yet.
pub const SETTLING_TRIES: u32 = 3;

/// The bookkeeping of one kind of job, per account: when it was last asked for, the failures
/// since the last answer, and how many changes were waiting and since when.
#[derive(Debug, Default)]
pub struct JobBook {
    checked: HashMap<Id, f64>,
    failures: HashMap<Id, (u32, f64)>,
    waiting: HashMap<Id, (usize, f64)>,
}

impl JobBook {
    /// Note that `account` has `ready` changes waiting at `now`, and say whether the account's job
    /// is due: changes that have waited out the quiet time, or nothing asked for since the refresh
    /// time, and not within the wait a failure asks for.
    pub fn due(&mut self, pacing: &Pacing, account: Id, ready: usize, now: f64) -> bool {
        let pushes = self.quiet_over(pacing, account, ready, now);
        if !(pushes || self.checked.get(&account).is_none_or(|at| now - at >= pacing.refresh_ms)) {
            return false;
        }
        self.patient(pacing, account, now)
    }

    /// Note that `account` has `ready` changes waiting at `now`, and say since when that many
    /// have waited (`now` when the number is new).
    pub fn waiting_since(&mut self, account: Id, ready: usize, now: f64) -> f64 {
        match self.waiting.get(&account) {
            Some((seen, since)) if *seen == ready => *since,
            _ => {
                self.waiting.insert(account, (ready, now));
                now
            }
        }
    }

    /// Note that `account` has `ready` changes waiting at `now`, and say whether there are some
    /// and they have waited out the quiet time.
    pub fn quiet_over(&mut self, pacing: &Pacing, account: Id, ready: usize, now: f64) -> bool {
        let since = self.waiting_since(account, ready, now);
        ready > 0 && now - since >= pacing.quiet_ms
    }

    /// Whether any account had changes waiting at its last look.
    pub fn any_waiting(&self) -> bool {
        self.waiting.values().any(|(n, _)| *n > 0)
    }

    /// Whether `account` is not within the wait that its failures in a row ask for at `now`.
    pub fn patient(&self, pacing: &Pacing, account: Id, now: f64) -> bool {
        let (failures, at) = self.failures.get(&account).copied().unwrap_or((0, 0.0));
        !(failures > 0 && now - at < pacing.patience_ms(failures))
    }

    /// When `account` was last asked for.
    pub fn checked(&self, account: Id) -> Option<f64> {
        self.checked.get(&account).copied()
    }

    /// Forget when every account was last asked for.
    pub fn forget_all_checked(&mut self) {
        self.checked.clear();
    }

    /// Forget the failures of `account`: the next look is not made to wait for them.
    pub fn forget_failures(&mut self, account: Id) {
        self.failures.remove(&account);
    }

    /// Forget every account's failures.
    pub fn forget_all_failures(&mut self) {
        self.failures.clear();
    }

    /// A job for `account` was started at `now`.
    pub fn started(&mut self, account: Id, now: f64) {
        self.checked.insert(account, now);
    }

    /// Forget when `account` was last asked for: the next look asks again.
    pub fn forget_checked(&mut self, account: Id) {
        self.checked.remove(&account);
    }

    /// The job that started at `started` failed: one more failure in a row.
    pub fn failed(&mut self, account: Id, started: f64) {
        let (n, _) = self.failures.get(&account).copied().unwrap_or((0, 0.0));
        self.failures.insert(account, (n.saturating_add(1), started));
    }

    /// The job worked: no failures in a row.
    pub fn worked(&mut self, account: Id) {
        self.forget_failures(account);
    }

    /// How many failures in a row `account` has.
    pub fn failures(&self, account: Id) -> u32 {
        self.failures.get(&account).map_or(0, |(n, _)| *n)
    }
}

/// Where an account's permission for a kind of job stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Permission {
    /// A job read it with the permission.
    Given,
    /// A job found the sign-in doesn't cover it: the owner is to be asked.
    Needed,
    /// The owner is being asked: the question, or the provider's page.
    Asking,
    /// The question went away unanswered: not asked again by itself.
    Declined,
    /// Asked, and no: why, in a few words.
    Refused(String),
    /// The provider no longer took the sign-in the job had (its refresh token's [`fingerprint`]),
    /// for mail either. Asking again is one way back; a new sign-in for mail is the other, and the
    /// next look runs a job again by itself.
    SignInOver(u64),
}

impl Permission {
    /// Whether a job for the account waits for the owner's permission, or for a new sign-in:
    /// `refresh` is the account's refresh token now, if it has one.
    pub fn blocks(this: Option<&Permission>, refresh: Option<&str>) -> bool {
        match this {
            Some(Permission::Needed | Permission::Asking | Permission::Declined | Permission::Refused(_)) => true,
            Some(Permission::SignInOver(over)) => !refresh.is_some_and(|r| !r.is_empty() && fingerprint(r) != *over),
            _ => false,
        }
    }
}

/// A refresh token, told apart from another without being kept: whether the account has signed
/// in again since a job's was refused.
pub fn fingerprint(refresh: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    refresh.hash(&mut hasher);
    hasher.finish()
}

/// The sync driver's state, in the session: what the front end's jobs have to remember between
/// looks. In memory only, never saved, and with no I/O; a front end reads and writes it through
/// the functions here and on [`Session`](crate::Session).
#[derive(Debug, Default)]
pub struct Driver {
    /// Mail to keep offline: when it was last taken back.
    pub thinning: Thinning,
    /// The calendar kept in step with each account's server.
    pub calendar: CalendarDriver,
}
