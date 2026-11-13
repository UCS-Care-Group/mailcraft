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
    /// How many changes wait per account, worked out for the mailbox as of this revision (every
    /// look asks; the work is done again only when the mailbox changed).
    ready: Option<(u64, HashMap<Id, usize>)>,
}

impl TasksDriver {
    /// Whether the permission is a question being asked, or one that went away unanswered, or no.
    fn answered_by_owner(&self, account: Id) -> bool {
        matches!(self.perm.get(&account), Some(Permission::Asking | Permission::Declined | Permission::Refused(_)))
    }

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
        if still {
            return;
        }
        let asking: Vec<Id> = self.perm.iter().filter(|(_, p)| **p == Permission::Asking).map(|(a, _)| *a).collect();
        for account in asking {
            self.perm.insert(account, Permission::Declined);
        }
    }

    /// The account the owner is next to be asked about, the lowest first.
    pub fn to_ask(&self) -> Option<Id> {
        self.perm.iter().filter(|(_, p)| **p == Permission::Needed).map(|(a, _)| *a).min()
    }
}

/// What the host says of itself when it asks whether the to-dos are due.
#[derive(Clone, Copy, Debug)]
pub struct TasksFacts {
    /// Automatic checking is on and the host has a way to reach the to-dos' server.
    pub enabled: bool,
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
        if !facts.enabled || facts.running || facts.busy.blocks(Start::Tasks) {
            return due;
        }
        for account in tasks_remote::task_accounts(&self.mailbox) {
            if self.tasks_blocked(account, refresh(account).as_deref()) {
                continue;
            }
            if matches!(self.driver.tasks.perm.get(&account), Some(Permission::SignInOver(_))) {
                // Signed in again since (for mail, say): a job finds out where the to-dos stand.
                self.driver.tasks.perm.remove(&account);
            }
            let ready = self.tasks_ready(account);
            if self.driver.tasks.book.due(&Pacing::TASKS, account, ready, now) {
                due.push(account);
            }
        }
        due
    }

    /// How many changes wait for `account`, as of this revision of the mailbox.
    fn tasks_ready(&mut self, account: Id) -> usize {
        let rev = self.rev();
        let driver = &mut self.driver.tasks;
        if driver.ready.as_ref().is_none_or(|(at, _)| *at != rev) {
            driver.ready = Some((rev, HashMap::new()));
        }
        if let Some(n) = driver.ready.as_ref().and_then(|(_, m)| m.get(&account).copied()) {
            return n;
        }
        let n = tasks_remote::tasks_waiting(&self.mailbox, account);
        if let Some((_, m)) = &mut self.driver.tasks.ready {
            m.insert(account, n);
        }
        n
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
        let mut errors = Vec::new();
        let settling = self.driver.tasks.settling.get(&account).copied().unwrap_or(0);
        if !came.consent_needed {
            self.driver.tasks.settling.remove(&account);
        }
        if came.consent_needed && settling > 0 {
            log::info!("tasks, account {account}: refused just after the owner allowed them; trying again ({settling} more)");
            self.driver.tasks.settling.insert(account, settling - 1);
            self.driver.tasks.book.failed(account, started);
            self.driver.tasks.book.forget_checked(account);
        } else if came.consent_needed {
            log::info!("tasks, account {account}: the sign-in doesn't cover to-dos; the owner is to be asked");
            if !self.driver.tasks.answered_by_owner(account) {
                self.driver.tasks.needed(account);
            }
        } else if came.denied {
            let said = said!("This account may not use its to-dos through Microsoft Graph");
            self.driver.tasks.refused(account, said.in_lang(lang));
        } else if came.sign_in_refused && !came.new_tokens && settling == 0 {
            // The refresh itself was refused, for mail too: never "not asked yet" with no way back
            // (#520). (Not when the owner allowed the to-dos since the job began: that sign-in is
            // newer than this answer.)
            log::warn!("tasks: the sign-in was refused; waiting for a new one");
            if !self.driver.tasks.answered_by_owner(account) {
                self.driver.tasks.perm.insert(account, Permission::SignInOver(given));
            }
        } else if came.sign_in_refused {
            // Graph refused a token just renewed: the next look tries again.
            log::warn!("tasks: the server refused the sign-in; trying again later");
        } else if came.download.is_some() || came.uploaded.is_some() {
            self.driver.tasks.perm.insert(account, Permission::Given);
        }
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
        if worked && errors.is_empty() && !came.job_errors {
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
