// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the window's small jobs beside a JMAP account's Send/Receive share: one kind of the
//! account's data read from its server by itself ([`crate::jmap_contacts`],
//! [`crate::jmap_calendar`]), one job at a time for each kind.
//!
//! [`SideJob`] is what the app remembers of one kind: the job in flight, when each account was
//! last asked, which are asked as soon as may be, and which are left alone and until when.
//!
//! **When a job is due.** Never for an account whose secret its server turned down, nor while its
//! Send/Receive is under way or waits its turn ([`MailApp::jmap_side_free`]); never for one whose
//! server offers none of the kind, nor before a wait is over ([`SideJob::held`]); then when it is
//! asked for as soon as may be, or when Send/Receive runs by itself and the last asking is old
//! enough ([`SideJob::wanted`]).
//!
//! **A job that came to nothing** is tried again after [`FIRST_RETRY_MS`], twice as long after each
//! one more and never longer than half an hour; a server that says it is busy is left alone for as
//! long as it says, an hour at the most ([`SideJob::settle`]).
//!
//! **Work started for one mailbox stays with it**: a job under way when another mailbox takes its
//! place is called off ([`SideJob::forget`]), and an answer is taken in only by the account it was
//! asked for, while it still has the address and the server it was asked with
//! ([`MailApp::jmap_side_meant`]).
//!
//! The log names the account's id and the kind, never an address or the server's.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_model::{Account, AccountKind, Id};

use crate::jmap_mail::JmapSideJob;
use crate::{MailApp, MailRun};

/// How often an account's data of one kind is asked for by itself (milliseconds).
pub(crate) const EVERY_MS: f64 = 900_000.0;
/// How long the server is left alone after a job that came to nothing: this long after the first,
/// twice as long after each one more, and never longer than the second.
pub(crate) const FIRST_RETRY_MS: f64 = 60_000.0;
const LONGEST_RETRY_MS: f64 = 1_800_000.0;
/// The longest a server that says it is busy is believed (seconds).
const LONGEST_BUSY: u32 = 3_600;
/// Most problems of one job that are kept for the owner.
pub(crate) const MAX_PROBLEMS: usize = 20;

/// What a job's answer says of itself, whatever it brings.
pub(crate) trait Answer {
    /// The job, as the log names it (`jmap contacts`), and what it reads (`contacts`).
    const JOB: &'static str;
    const READS: &'static str;

    /// The account it is for, with the address and the session's address it was asked with (either
    /// may be empty in an answer that does not say).
    fn whose(&self) -> (Id, &str, &str);
}

/// What a job of an account goes with: the secret, and where it goes.
pub(crate) struct Asking {
    account: Id,
    secret: String,
    pub(crate) email: String,
    pub(crate) session: String,
    pub(crate) bearer: bool,
}

impl Asking {
    /// The job the host is handed for `request`, and what is kept of it here until its answer
    /// comes; `opening` is the opening of a mailbox it is started for.
    pub(crate) fn job<Q>(self, request: Q, opening: u64) -> (JmapSideJob<Q>, Asked) {
        let cancel = Arc::new(AtomicBool::new(false));
        let asked = Asked { account: self.account, opening, email: self.email, session: self.session, cancel: cancel.clone() };
        (JmapSideJob { request, secret: self.secret, cancel }, asked)
    }
}

/// What is kept of a job that was handed to the host.
pub(crate) struct Asked {
    pub(crate) account: Id,
    /// Which opening of a mailbox it was started for.
    opening: u64,
    /// The address and the session's address it was asked with.
    pub(crate) email: String,
    session: String,
    cancel: Arc<AtomicBool>,
}

impl Asked {
    /// Whether `result` is an answer to what was asked: for that account, at that address, on that
    /// server.
    fn answered_by<R: Answer>(&self, result: &R) -> bool {
        let (account, email, session) = result.whose();
        account == self.account
            && (email.is_empty() || email.trim().eq_ignore_ascii_case(self.email.trim()))
            && (session.is_empty() || session.trim() == self.session.trim())
    }
}

/// What the app remembers of reading one kind of JMAP accounts' data.
pub(crate) struct SideJob<R> {
    /// The job in flight, and where its answer comes.
    job: Option<(Asked, Receiver<R>)>,
    /// When each account was last asked (milliseconds).
    asked: HashMap<Id, f64>,
    /// How many jobs in a row came to nothing for an account.
    failures: HashMap<Id, u32>,
    /// The accounts whose server is left alone until then (milliseconds).
    not_before: HashMap<Id, f64>,
    /// The accounts whose server offers none of the kind.
    not_offered: HashSet<Id>,
    /// The accounts that are asked as soon as may be: a Send/Receive of theirs has just ended, or
    /// the owner asked for one.
    due: HashSet<Id>,
}

impl<R> Default for SideJob<R> {
    fn default() -> Self {
        Self {
            job: None,
            asked: HashMap::new(),
            failures: HashMap::new(),
            not_before: HashMap::new(),
            not_offered: HashSet::new(),
            due: HashSet::new(),
        }
    }
}

impl<R: Answer> SideJob<R> {
    /// Another mailbox took the place of the open one: the job in flight is called off, and what
    /// was known of its accounts goes with it (their ids may name others now).
    pub(crate) fn forget(&mut self) {
        if let Some((job, _)) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        *self = Self::default();
    }

    /// `account` is gone, or names another server now: what was known of it was the old one's.
    pub(crate) fn forget_account(&mut self, account: Id) {
        self.asked.remove(&account);
        self.failures.remove(&account);
        self.not_before.remove(&account);
        self.not_offered.remove(&account);
        self.due.remove(&account);
    }

    /// The owner asked for a Send/Receive of `account`: a job follows, whatever came of the jobs
    /// before.
    pub(crate) fn by_hand(&mut self, account: Id) {
        self.forget_account(account);
        self.due.insert(account);
    }

    /// A Send/Receive of `account` has ended well enough: a job follows when none was asked for
    /// yet. After that the jobs keep their own pace, however often mail is read.
    pub(crate) fn after_run(&mut self, account: Id) {
        if !self.asked.contains_key(&account) {
            self.due.insert(account);
        }
    }

    /// Whether a job is in flight.
    pub(crate) fn running(&self) -> bool {
        self.job.is_some()
    }

    /// Whether the job in flight is `account`'s.
    pub(crate) fn reading(&self, account: Id) -> bool {
        self.job.as_ref().is_some_and(|(job, _)| job.account == account)
    }

    /// Whether the job in flight was started for another opening of a mailbox than `opening`: its
    /// answer is nobody's (the accounts are other accounts, whatever their ids and addresses).
    pub(crate) fn stale(&self, opening: u64) -> bool {
        self.job.as_ref().is_some_and(|(job, _)| job.opening != opening)
    }

    /// Whether `account` is left alone at `now`: its server offers none of the kind, or a wait is
    /// not over.
    pub(crate) fn held(&self, account: Id, now: f64) -> bool {
        self.not_offered.contains(&account) || self.not_before.get(&account).is_some_and(|until| now < *until)
    }

    /// Whether a job of `account` is wanted at `now`: it is asked for as soon as may be, or jobs go
    /// by themselves (`auto`) and the last was asked `every` milliseconds ago or more.
    pub(crate) fn wanted(&self, account: Id, now: f64, every: f64, auto: bool) -> bool {
        self.due.contains(&account) || (auto && self.asked.get(&account).is_none_or(|at| now - at >= every))
    }

    /// The host was handed a job at `now`: `job` is what is kept of it, and its answer comes
    /// through `rx`.
    pub(crate) fn start(&mut self, now: f64, job: Asked, rx: Receiver<R>) {
        let account = job.account;
        self.job = Some((job, rx));
        self.asked.insert(account, now);
        self.due.remove(&account);
        log::debug!("{}: asked for", R::JOB);
    }

    /// What the job in flight brought, when it has, with what was kept of the job. A job for a
    /// mailbox that is no longer open ([`Self::stale`]) is let go of with everything known; one
    /// that ended without an answer came to nothing.
    pub(crate) fn answer(&mut self, opening: u64, now: f64) -> Option<(Asked, R)> {
        if self.stale(opening) {
            log::info!("{}: the answer is for a mailbox that is no longer open; let go of", R::JOB);
            self.forget();
            return None;
        }
        let result = match self.job.as_ref()?.1.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => None,
        };
        let (job, _) = self.job.take()?;
        let Some(result) = result else {
            log::warn!("{}: the job ended without an answer (its task ended early)", R::JOB);
            self.failed(job.account, now);
            return None;
        };
        Some((job, result))
    }

    /// A job of `account` has been taken in at `now`: the server said it is busy for so many
    /// seconds (`busy_for`), or the job was called off (`stopped`), which is neither a failure nor
    /// an answer, or it went `well`, or it came to nothing.
    pub(crate) fn settle(&mut self, account: Id, now: f64, busy_for: Option<u32>, stopped: bool, well: bool) {
        if let Some(seconds) = busy_for {
            log::info!("{}: the server is busy", R::JOB);
            self.not_before.insert(account, now + f64::from(seconds.min(LONGEST_BUSY)) * 1_000.0);
            self.due.insert(account);
        } else if stopped {
            // Called off.
        } else if well {
            self.failures.remove(&account);
            self.not_before.remove(&account);
        } else {
            self.failed(account, now);
        }
    }

    /// A job of `account` came to nothing at `now`: the next waits.
    fn failed(&mut self, account: Id, now: f64) {
        let count = self.failures.get(&account).copied().unwrap_or(0).saturating_add(1);
        self.failures.insert(account, count);
        let doublings = i32::try_from(count.saturating_sub(1).min(16)).unwrap_or(16);
        let wait = (FIRST_RETRY_MS * 2f64.powi(doublings)).min(LONGEST_RETRY_MS);
        self.not_before.insert(account, now + wait);
        // Tried again as soon as the wait is over, not at the next quarter of an hour.
        self.due.insert(account);
    }
}

impl MailApp {
    /// Whether a job beside the Send/Receive of the JMAP account `account` may go: its server has
    /// not turned its secret down, and its Send/Receive, which goes first, is neither under way nor
    /// waiting its turn.
    pub(crate) fn jmap_side_free(&self, account: Id) -> bool {
        let running = matches!(&self.sync_rx, Some(MailRun::Jmap { account: busy, .. }) if *busy == account);
        !self.jmap_refused.contains(&account) && !running && !self.sync_queue.iter().any(|(waits, _)| *waits == account)
    }

    /// What a job of the JMAP account `account` goes with: the secret its Send/Receive goes with
    /// (empty when the host holds the credential itself), its address and its server's. None when
    /// no secret is at hand (the owner types it for a Send/Receive, and nothing here asks), or the
    /// account names no server.
    pub(crate) fn jmap_side_asking(&self, account: Id) -> Option<Asking> {
        let secret = self.jmap_secret(account)?;
        let a = self.session.mailbox.account(account)?;
        let server = a.jmap.as_ref()?;
        Some(Asking { account, secret, email: a.email.clone(), session: server.session.clone(), bearer: server.bearer })
    }

    /// Whether `result` is for the account `job` was asked for as it is now: an answer to what was
    /// asked, and the account still a JMAP account that `keeps` such data, at that address, on that
    /// server.
    pub(crate) fn jmap_side_meant<R: Answer>(&self, job: &Asked, result: &R, keeps: impl Fn(&Account) -> bool) -> bool {
        let (email, session) = (job.email.trim(), job.session.trim());
        let still = self.session.mailbox.account(job.account).is_some_and(|a| {
            a.kind == AccountKind::Jmap
                && keeps(a)
                && a.email.trim().eq_ignore_ascii_case(email)
                && a.jmap.as_ref().map(|server| server.session.trim()) == Some(session)
        });
        let meant = job.answered_by(result) && still;
        if !meant {
            log::info!("{}: the answer is not for the account as it is now; let go of", R::JOB);
        }
        meant
    }

    /// Whether an answer for `account` ends with one of two words of its server, and what follows
    /// from the word. The sign-in was `refused`, as in a Send/Receive: a typed secret is forgotten,
    /// the next Send/Receive the owner asks for asks again, and nothing by itself goes back until
    /// then. Or the server offers none of the kind (`not_offered`): `side`'s jobs do not ask it
    /// again.
    pub(crate) fn jmap_side_ended<R: Answer>(
        &mut self,
        side: impl FnOnce(&mut Self) -> &mut SideJob<R>,
        account: Id,
        refused: bool,
        not_offered: bool,
    ) -> bool {
        if refused {
            log::warn!("{}: the server turned the sign-in down", R::JOB);
            self.passwords.remove(&account);
            self.jmap_refused.insert(account);
        } else if not_offered {
            log::info!("{}: the server offers no {}; not asked again", R::JOB, R::READS);
            let side = side(self);
            side.not_offered.insert(account);
            side.failures.remove(&account);
        }
        refused || not_offered
    }
}
