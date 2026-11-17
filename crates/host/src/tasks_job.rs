// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The To Do job between Send/Receive runs, for every front end (the desktop's window and the
//! phone's core alike): what a job is handed, the job in flight, and its answer taken in.
//!
//! When the to-dos are due is the engine's to say ([`mailcraft_engine::Session::tasks_due`]); the
//! front end asks, then hands each account it may run to [`TasksJobs::start`], which makes the job
//! (what waits to go up, `sync.taskChanges`), has the front end start it its own way (a thread of
//! the host's, the desktop's `Services`), marks a delete that goes out as gone
//! ([`tasks_remote::mark_sent`], #706) and tells the engine. [`TasksJobs::poll`] takes the answer
//! in through the engine ([`mailcraft_engine::Session::tasks_answered`]) and hands back what the
//! front end keeps itself: the account's newest refresh token, and the lines for the owner.
//!
//! Nothing of a task is logged, and no refresh token appears in a `Debug` output.

use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::tasks_remote;
use mailcraft_engine::sync_driver::{TasksCame, fingerprint};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Account, Id, OAuth};

use crate::job_sign_in::{ForeignSignIn, foreign_sign_in};
use crate::tasks::{TasksJob, TasksResult};

/// The job under way: for which account, since when, the fingerprint of the refresh token it was
/// given, the opening of the mailbox it was started under, the account as it was (whose address
/// the answer is taken in for, AU3-08), and the answer to come.
struct Running {
    account: Id,
    started: f64,
    given: u64,
    opening: u64,
    ran_for: Account,
    rx: Receiver<TasksResult>,
}

/// The to-dos' job in flight, at most one.
#[derive(Default)]
pub struct TasksJobs {
    running: Option<Running>,
}

impl std::fmt::Debug for TasksJobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TasksJobs").field("running", &self.running.as_ref().map(|r| r.account)).finish()
    }
}

/// What a to-do job came to, once the engine has taken it in: what the front end keeps itself.
#[derive(Default)]
pub struct TasksAnswer {
    pub account: Id,
    /// The tokens the job got: its refresh token is the account's newest (the front end keeps it,
    /// unless they are somebody else's). Never in `Debug` output.
    pub tokens: Option<Tokens>,
    /// The lines for the owner: the job's own, then what the engine said of the answer.
    pub errors: Vec<String>,
    /// The job ended without an answer (its thread ended early): the front end says so, in its words.
    pub stopped_early: bool,
    /// The sign-in doesn't cover the to-dos: the owner is to be asked (the engine noted it).
    pub consent_needed: bool,
    /// The answer came with somebody else's sign-in: it was refused whole (nothing of it taken in,
    /// no tokens handed back), and the front end tells the owner.
    pub foreign: Option<ForeignSignIn>,
}

impl std::fmt::Debug for TasksAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TasksAnswer")
            .field("account", &self.account)
            .field("tokens", &self.tokens.is_some())
            .field("errors", &self.errors.len())
            .field("stopped_early", &self.stopped_early)
            .field("consent_needed", &self.consent_needed)
            .field("foreign", &self.foreign.is_some())
            .finish()
    }
}

impl TasksJobs {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// The account of the job in flight.
    pub fn account(&self) -> Option<Id> {
        self.running.as_ref().map(|r| r.account)
    }

    /// Let go of the job in flight (another mailbox took the place of the open one).
    pub fn forget(&mut self) {
        self.running = None;
    }

    /// Whether the job in flight was started under another opening of the mailbox than the one
    /// open now: its answer is nobody's ([`poll`](Self::poll) lets it go).
    pub fn stale(&self, session: &Session) -> bool {
        self.running.as_ref().is_some_and(|r| r.opening != session.openings())
    }

    /// Start the to-dos' job for `account` at `now` (milliseconds): `refresh` gives the account's
    /// refresh token (the session's or the credential store's), `oauth` fills the application id
    /// into its sign-in settings, and `launch` starts the job its front end's way, `None` when it
    /// can't (no host for it, Work Offline). Whether it was started: not without a sign-in, nor
    /// with an upload the mailbox couldn't make, nor while one is in flight.
    pub fn start(
        &mut self,
        session: &mut Session,
        account: Id,
        refresh: impl FnOnce(&Account) -> Option<String>,
        oauth: impl FnOnce(OAuth) -> OAuth,
        now: f64,
        launch: impl FnOnce(TasksJob) -> Option<Receiver<TasksResult>>,
    ) -> bool {
        if self.running.is_some() {
            return false;
        }
        let Some(owner) = session.mailbox.account(account).cloned() else { return false };
        let Some(settings) = owner.oauth.clone() else { return false };
        let refresh = refresh(&owner).unwrap_or_default();
        if refresh.is_empty() {
            // (Nothing to renew with: Send/Receive signs in first.)
            return false;
        }
        let Ok(upload) = tasks_remote::task_upload(&session.mailbox, account) else { return false };
        let given = fingerprint(&refresh);
        let deletes = tasks_remote::carries_delete(&upload);
        let zone = session.zone();
        let opening = session.openings();
        let job = TasksJob { account: owner.clone(), oauth: oauth(settings), refresh, upload, zone, cancel: Default::default() };
        let Some(rx) = launch(job) else { return false };
        // A delete that goes out can't be undone: the task would come back here, gone there (#706).
        tasks_remote::mark_sent(session, deletes);
        self.running = Some(Running { account, started: now, given, opening, ran_for: owner, rx });
        session.tasks_started(account, now);
        true
    }

    /// Take the answer in when it has come: what the server made of the changes and what it has
    /// (the engine's), with the lines the owner is told in `lang`. `None` while it hasn't come, or
    /// when it is for a mailbox that is no longer open (then it is let go of, AU3-08).
    pub fn poll(&mut self, session: &mut Session, lang: mailcraft_i18n::Lang) -> Option<TasksAnswer> {
        let running = self.running.as_ref()?;
        if running.opening != session.openings() {
            log::info!("tasks: the answer is for a mailbox that is no longer open; let go of");
            self.running = None;
            return None;
        }
        let (result, stopped_early) = match running.rx.try_recv() {
            Ok(result) => (result, false),
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                log::warn!("tasks: the job ended without an answer (its thread ended early)");
                (TasksResult { account: running.account, ..Default::default() }, true)
            }
        };
        let running = self.running.take()?;
        // Somebody else's sign-in: nothing of what it brought is this account's, and its token is
        // not kept; the engine takes it as a job that failed. (Logged without naming anybody.)
        let foreign = foreign_sign_in(&running.ran_for, result.tokens.as_ref());
        let result = if foreign.is_some() {
            log::warn!("tasks: the answer came with another account's sign-in; refused whole");
            TasksResult { account: running.account, ..Default::default() }
        } else {
            result
        };
        let mut errors = result.errors;
        let came = TasksCame {
            job_errors: !errors.is_empty() || stopped_early || foreign.is_some(),
            consent_needed: result.consent_needed,
            sign_in_refused: result.sign_in_refused,
            new_tokens: result.tokens.is_some(),
            denied: result.denied,
            uploaded: result.uploaded.as_ref(),
            download: result.download.as_ref(),
        };
        let said = session.tasks_answered(running.account, running.started, running.given, &running.ran_for, lang, &came);
        errors.extend(said.iter().map(|e| e.in_lang(lang)));
        Some(TasksAnswer { account: running.account, tokens: result.tokens, errors, stopped_early, consent_needed: result.consent_needed, foreign })
    }
}
