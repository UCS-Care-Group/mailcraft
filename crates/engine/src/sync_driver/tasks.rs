// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tasks kept in step with Microsoft To Do (#248), the decisions and the state, with no I/O:
//! [`Session::tasks_due`] says which accounts' to-do jobs are to start now; the host starts the
//! first it can ([`Session::tasks_started`]) and, when it answers, hands what came to
//! [`Session::tasks_answered`], which keeps the permission's state, takes in what the server made
//! of the changes and what it has, and says what went wrong. Where the permission stands
//! ([`Permission`]) and when to ask the owner for it are here; the asking is the window's.

use std::collections::HashMap;

use mailcraft_i18n::Text;
use mailcraft_model::remote_tasks::{TasksDownload, TasksUploaded};
use mailcraft_model::{Account, Id};

use super::{Busy, JobBook, Pacing, Permission, SETTLING_TRIES, Start};
use crate::Session;
use crate::cmd::tasks_remote;

/// What the driver remembers of the to-dos' jobs.
#[derive(Debug, Default)]
pub struct TasksDriver {
    /// When each account's to-dos were last asked for, the failures since the last answer, and
    /// the changes waiting.
    pub book: JobBook,
    /// Where each account's permission for to-dos stands.
    pub perm: HashMap<Id, Permission>,
    /// The owner allowed the to-dos on Microsoft's page: how many refusals are still taken for
    /// Microsoft not having taken it in yet, rather than for a no.
    pub settling: HashMap<Id, u32>,
    /// How many changes wait per account.
    ready: super::WaitingCache,
}

impl TasksDriver {
    /// The owner is being asked about `account`.
    pub fn asking(&mut self, account: Id) {
        self.perm.insert(account, Permission::Asking);
    }

    /// The owner is to be asked about `account`.
    pub fn needed(&mut self, account: Id) {
        self.perm.insert(account, Permission::Needed);
    }

    /// The owner said no, or the question couldn't be put (`say`).
    pub fn refused(&mut self, account: Id, why: String) {
        self.perm.insert(account, Permission::Refused(why));
    }

    /// The question went away unanswered, or there was no way to ask: not asked again by itself.
    pub fn declined(&mut self, account: Id) {
        self.perm.insert(account, Permission::Declined);
    }

    /// The owner allowed the to-dos on Microsoft's page, as far as is known: the next job finds
    /// out, and refusals right after are taken for Microsoft not having caught up.
    pub fn allowed(&mut self, account: Id) {
        self.perm.remove(&account);
        self.settling.insert(account, SETTLING_TRIES);
        self.book.worked(account);
        self.book.forget_checked(account);
    }

    /// Every account that keeps to-dos has its To Do working, as if a job had read it.
    pub fn working_all(&mut self, accounts: &[Id]) {
        for account in accounts {
            self.perm.insert(*account, Permission::Given);
        }
    }

    /// A question that went away unanswered is not asked again by itself: when none is on screen
    /// (`still`), every account being asked about is `Declined`.
    pub fn unanswered(&mut self, still: bool) {
        super::unanswered(&mut self.perm, still);
    }

    /// The account the owner is next to be asked about, the lowest first.
    pub fn to_ask(&self) -> Option<Id> {
        super::to_ask(&self.perm)
    }

    /// Account removal: nothing of these accounts' permission is kept.
    pub fn forget_accounts(&mut self, gone: &std::collections::HashSet<Id>) {
        self.book.forget_accounts(gone);
        self.perm.retain(|account, _| !gone.contains(account));
        self.settling.retain(|account, _| !gone.contains(account));
    }
}

/// What the host says of itself when it asks whether the to-dos are due.
#[derive(Clone, Copy, Debug)]
pub struct TasksFacts {
    /// The host has a way to reach the to-dos' server.
    pub reachable: bool,
    /// A to-do job is in flight already.
    pub running: bool,
    /// What else is at work.
    pub busy: Busy,
}

/// What a to-do job brought, as the engine takes it in (borrowed from the host's answer).
#[derive(Clone, Copy, Debug, Default)]
pub struct TasksCame<'a> {
    /// The sign-in doesn't cover to-dos: the owner is to be asked.
    pub consent_needed: bool,
    /// The sign-in itself is no longer taken.
    pub sign_in_refused: bool,
    /// The job got a refresh token (the account's newest).
    pub new_tokens: bool,
    /// The server said this account may not use its to-dos.
    pub denied: bool,
    /// The host's own job put something among its errors (a part it couldn't bring down, say):
    /// the answer is not counted a success, whatever else came.
    pub job_errors: bool,
    pub uploaded: Option<&'a TasksUploaded>,
    pub download: Option<&'a TasksDownload>,
}

impl Session {
    /// Whether the to-dos of `account` wait for the owner's permission, or for a new sign-in:
    /// `refresh` is the account's refresh token now, if it has one.
    pub fn tasks_blocked(&self, account: Id, refresh: Option<&str>) -> bool {
        Permission::blocks(self.driver.tasks.perm.get(&account), refresh)
    }

    /// The accounts whose to-do job is due at `now`, in the order they are tried; the host starts
    /// the first it can ([`Session::tasks_started`]). `refresh`: an account's refresh token now.
    pub fn tasks_due(&mut self, facts: &TasksFacts, refresh: impl Fn(Id) -> Option<String>, now: f64) -> Vec<Id> {
        let mut due = Vec::new();
        // One thing at a time: the calendar's job, a Send/Receive and a sign-in go first.
        if !facts.reachable || !self.driver.auto_sync() || facts.running || facts.busy.blocks(Start::Tasks) {
            return due;
        }
        // Working offline: the books go on (a change waits its quiet time as ever), nothing is started.
        let offline = self.prefs.offline;
        for account in tasks_remote::task_accounts(&self.mailbox) {
            if self.tasks_blocked(account, refresh(account).as_deref()) {
                continue;
            }
            if matches!(self.driver.tasks.perm.get(&account), Some(Permission::SignInOver(_))) {
                // Signed in again since (for mail, say): a job finds out where the to-dos stand.
                self.driver.tasks.perm.remove(&account);
            }
            let ready = self.tasks_ready(account);
            if self.driver.tasks.book.due(&Pacing::TASKS, account, ready, now) && !offline {
                due.push(account);
            }
        }
        due
    }

    /// How many changes wait for `account`, as of this revision of the mailbox.
    fn tasks_ready(&mut self, account: Id) -> usize {
        let (rev, mailbox) = (self.rev(), &self.mailbox);
        self.driver.tasks.ready.get(rev, account, || tasks_remote::tasks_waiting(mailbox, account))
    }

    /// The host handed a to-do job for `account` to its server at `now`.
    pub fn tasks_started(&mut self, account: Id, now: f64) {
        self.driver.tasks.book.started(account, now);
    }

    /// A to-do job for `account` answered. `started`: when it began; `given`: the
    /// [`fingerprint`](super::fingerprint) of the refresh token it was given; `ran_for`: the
    /// account as the job had it; `lang`: the words a refusal that is kept is said in. Say what went wrong, in order.
    pub fn tasks_answered(
        &mut self,
        account: Id,
        started: f64,
        given: u64,
        ran_for: &Account,
        lang: mailcraft_i18n::Lang,
        came: &TasksCame<'_>,
    ) -> Vec<Text> {
        // An answer for an account removed meanwhile changes nothing (else its question would be
        // left standing first in the queue, and nobody could answer it).
        if self.mailbox.account(account).is_none() {
            return Vec::new();
        }
        let mut errors = Vec::new();
        let tasks = &mut self.driver.tasks;
        let standing = super::Standing {
            consent_needed: came.consent_needed,
            denied: came.denied,
            sign_in_refused: came.sign_in_refused,
            new_tokens: came.new_tokens,
        };
        let said = || said!("This account may not use its to-dos through Microsoft Graph").in_lang(lang);
        let read = came.download.is_some() || came.uploaded.is_some();
        let noted =
            super::standing_in("tasks", &mut tasks.book, &mut tasks.perm, &mut tasks.settling, account, (started, given), standing, (said, read));
        let mut worked = false;
        if let Some(uploaded) = came.uploaded {
            // (Taken in for the account as the job had it, never relabelled with what is here now.)
            let uploaded = uploaded.clone().for_account(ran_for);
            match tasks_remote::apply_task_changes(self, &uploaded) {
                Ok(merged) => {
                    errors.extend(merged.messages.into_iter().map(Text::verbatim));
                    worked = true;
                }
                Err(e) => errors.push(said!("The to-dos sent couldn't be noted: {error}", error = e.to_string())),
            }
        }
        if let Some(download) = came.download {
            match tasks_remote::apply_tasks(self, download) {
                Ok(_) => worked = true,
                Err(e) => errors.push(said!("The to-dos couldn't be taken in: {error}", error = e.to_string())),
            }
        }
        if noted {
            // (A failure is noted for this answer already: a success doesn't undo it.)
        } else if worked && errors.is_empty() && !came.job_errors {
            self.driver.tasks.book.worked(account);
        } else if !came.consent_needed && !came.sign_in_refused {
            self.driver.tasks.book.failed(account, started);
        }
        errors
    }

    /// The accounts whose To Do is working (a job read it with the permission).
    pub fn tasks_working(&self) -> Vec<Id> {
        tasks_remote::task_accounts(&self.mailbox).into_iter().filter(|a| self.driver.tasks.perm.get(a) == Some(&Permission::Given)).collect()
    }

    /// Whether a to-do put in `account` waits on this computer because a job found that To Do
    /// isn't allowed there (nothing is known of an account no job has looked at yet).
    pub fn tasks_held(&self, account: Id) -> bool {
        !matches!(self.driver.tasks.perm.get(&account), None | Some(Permission::Given))
    }
}
