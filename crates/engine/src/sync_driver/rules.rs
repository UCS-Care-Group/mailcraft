// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rules kept on the server (#566), the decisions and the state, with no I/O:
//! [`Session::rules_due`] says which accounts' rules jobs are to start now and whether they carry
//! changes; the host starts the first it can ([`Session::rules_started`]) and, when it answers,
//! hands what came to [`Session::rules_answered`], which keeps the permission's state, takes in
//! what the server made of the changes and what it has, and says what went wrong. Where the
//! permission stands ([`Permission`]) and when to ask the owner for it are here; the asking is
//! the window's.

use std::collections::{HashMap, HashSet};

use mailcraft_i18n::Text;
use mailcraft_model::Id;
use mailcraft_model::remote_rules::{RulesDownload, RulesUploaded};

use super::{Busy, JobBook, Pacing, Permission, SETTLING_TRIES, Start};
use crate::Session;
use crate::cmd::rules_remote;

/// What the driver remembers of the rules' jobs.
#[derive(Debug, Default)]
pub struct RulesDriver {
    /// When each account's rules were last asked for, the failures since the last answer, and
    /// the changes waiting.
    pub book: JobBook,
    /// Where each account's permission to change its rules on the server stands.
    pub perm: HashMap<Id, Permission>,
    /// The owner allowed changing the rules on Microsoft's page: how many refusals are still
    /// taken for Microsoft not having taken it in yet, rather than for a no (#491).
    pub settling: HashMap<Id, u32>,
    /// The accounts whose server's rules have been read, and those whose can't be.
    pub read: HashSet<Id>,
    pub unreadable: HashSet<Id>,
    /// How many changes wait per account.
    ready: super::WaitingCache,
}

impl RulesDriver {
    /// The owner is being asked about `account`.
    pub fn asking(&mut self, account: Id) {
        self.perm.insert(account, Permission::Asking);
    }

    /// The owner is to be asked about `account`.
    pub fn needed(&mut self, account: Id) {
        self.perm.insert(account, Permission::Needed);
    }

    /// The owner said no, or the question couldn't be put (`why`).
    pub fn refused(&mut self, account: Id, why: String) {
        self.perm.insert(account, Permission::Refused(why));
    }

    /// The question went away unanswered, or there was no way to ask: not asked again by itself.
    pub fn declined(&mut self, account: Id) {
        self.perm.insert(account, Permission::Declined);
    }

    /// The owner allowed changing the mailbox's settings on Microsoft's page, for the rules or
    /// for the automatic replies: one permission for both, so the rules that waited for it go
    /// with the next job.
    pub fn allowed(&mut self, account: Id) {
        self.perm.remove(&account);
        self.settling.insert(account, SETTLING_TRIES);
        self.book.forget_failures(account);
        self.book.forget_checked(account);
    }

    /// Every account that keeps rules on its server has them working, as if a job had changed them.
    pub fn working_all(&mut self, accounts: &[Id]) {
        for account in accounts {
            self.perm.insert(*account, Permission::Given);
            self.read.insert(*account);
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

    /// Account removal: nothing of these accounts' permission or rules is kept.
    pub fn forget_accounts(&mut self, gone: &HashSet<Id>) {
        self.book.forget_accounts(gone);
        self.perm.retain(|account, _| !gone.contains(account));
        self.settling.retain(|account, _| !gone.contains(account));
        self.read.retain(|account| !gone.contains(account));
        self.unreadable.retain(|account| !gone.contains(account));
    }
}

/// What the host says of itself when it asks whether the rules are due.
#[derive(Clone, Copy, Debug)]
pub struct RulesFacts {
    /// The host has a way to reach the rules' server.
    pub reachable: bool,
    /// A rules job is in flight already.
    pub running: bool,
    /// What else is at work.
    pub busy: Busy,
}

/// One rules job that is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RulesPlan {
    pub account: Id,
    /// It carries the changes that wait (else it only reads: the owner hasn't allowed changing
    /// the rules on the server).
    pub writes: bool,
}

/// What a rules job brought, as the engine takes it in (borrowed from the host's answer).
#[derive(Clone, Copy, Debug, Default)]
pub struct RulesCame<'a> {
    /// There were changes to send and the sign-in doesn't cover changing the rules.
    pub consent_needed: bool,
    /// The sign-in itself is no longer taken.
    pub sign_in_refused: bool,
    /// The job got a refresh token (the account's newest).
    pub new_tokens: bool,
    /// The server said this account may not change its rules.
    pub denied: bool,
    /// The server's rules can't be read with this sign-in.
    pub unreadable: bool,
    /// The host's own job put something among its errors: the answer is not counted a success.
    pub job_errors: bool,
    pub uploaded: Option<&'a RulesUploaded>,
    pub download: Option<&'a RulesDownload>,
}

impl Session {
    /// Whether changes to the rules of `account` wait for the owner's permission. The server's
    /// rules are read all the same.
    pub fn rules_held(&self, account: Id) -> bool {
        matches!(self.driver.rules.perm.get(&account), Some(Permission::Needed | Permission::Asking | Permission::Declined | Permission::Refused(_)))
    }

    /// Whether the sign-in the last job had is over, and there is no newer one (`refresh` is the
    /// account's refresh token now).
    pub fn rules_signed_out(&self, account: Id, refresh: Option<&str>) -> bool {
        match self.driver.rules.perm.get(&account) {
            Some(Permission::SignInOver(over)) => !refresh.is_some_and(|r| !r.is_empty() && super::fingerprint(r) != *over),
            _ => false,
        }
    }

    /// The accounts whose rules job is due at `now`, in the order they are tried; the host starts
    /// the first it can ([`Session::rules_started`]).
    pub fn rules_due(&mut self, facts: &RulesFacts, refresh: impl Fn(Id) -> Option<String>, now: f64) -> Vec<RulesPlan> {
        let mut due = Vec::new();
        // One thing at a time: the calendar's job, the to-dos', the automatic replies', a
        // Send/Receive and a sign-in go first (the jobs renew the same sign-in).
        if !facts.reachable || !self.driver.auto_sync() || facts.running || facts.busy.blocks(Start::Rules) {
            return due;
        }
        // Working offline: the books go on, nothing is started.
        let offline = self.prefs.offline;
        for account in rules_remote::rule_accounts(&self.mailbox) {
            if self.rules_signed_out(account, refresh(account).as_deref()) {
                continue;
            }
            if matches!(self.driver.rules.perm.get(&account), Some(Permission::SignInOver(_))) {
                self.driver.rules.perm.remove(&account);
            }
            let writes = !self.rules_held(account);
            // What waits for a permission not given doesn't make a job: the reads come as ever.
            let ready = if writes { self.rules_ready(account) } else { 0 };
            let book = &mut self.driver.rules.book;
            let pushes = book.quiet_over(&Pacing::RULES, account, ready, now);
            let asked = book.checked(account);
            if !(pushes || asked.is_none_or(|at| now - at >= Pacing::RULES.refresh_ms)) || !book.patient(&Pacing::RULES, account, now) || offline {
                continue;
            }
            due.push(RulesPlan { account, writes });
        }
        due
    }

    /// How many changes wait for `account`, as of this revision of the mailbox.
    fn rules_ready(&mut self, account: Id) -> usize {
        let (rev, mailbox) = (self.rev(), &self.mailbox);
        self.driver.rules.ready.get(rev, account, || rules_remote::rules_waiting(mailbox, account))
    }

    /// The host handed a rules job for `account` to its server at `now`.
    pub fn rules_started(&mut self, account: Id, now: f64) {
        self.driver.rules.book.started(account, now);
    }

    /// A rules job for `account` answered. `started`: when it began; `given`: the
    /// [`fingerprint`](super::fingerprint) of the refresh token it was given; `lang`: the words a
    /// refusal that is kept is said in. Say what went wrong, in order.
    pub fn rules_answered(&mut self, account: Id, started: f64, given: u64, lang: mailcraft_i18n::Lang, came: &RulesCame<'_>) -> Vec<Text> {
        // An answer for an account removed meanwhile changes nothing (else its question would be
        // left standing first in the queue, and nobody could answer it).
        if self.mailbox.account(account).is_none() {
            return Vec::new();
        }
        let mut errors = Vec::new();
        let rules = &mut self.driver.rules;
        let standing = super::Standing {
            consent_needed: came.consent_needed,
            denied: came.denied,
            sign_in_refused: came.sign_in_refused,
            new_tokens: came.new_tokens,
        };
        let said = || said!("This account may not change its rules on the server").in_lang(lang);
        let noted = super::standing_in(
            "rules",
            &mut rules.book,
            &mut rules.perm,
            &mut rules.settling,
            account,
            (started, given),
            standing,
            (said, came.uploaded.is_some()),
        );
        if came.unreadable {
            self.driver.rules.unreadable.insert(account);
        } else if came.download.is_some() {
            self.driver.rules.unreadable.remove(&account);
            self.driver.rules.read.insert(account);
        }
        let mut worked = false;
        // (The host names the account it was for, address and all, which the engine checks.)
        if let Some(uploaded) = came.uploaded {
            match rules_remote::apply_rule_changes(self, uploaded) {
                Ok(merged) => {
                    errors.extend(merged.messages.into_iter().map(Text::verbatim));
                    worked = true;
                }
                Err(e) => errors.push(said!("The rules sent couldn't be noted: {error}", error = e.to_string())),
            }
        }
        if let Some(download) = came.download {
            match rules_remote::apply_rules(self, download) {
                Ok(merged) => {
                    errors.extend(merged.messages.into_iter().map(Text::verbatim));
                    worked = true;
                }
                Err(e) => errors.push(said!("The rules on the server couldn't be taken in: {error}", error = e.to_string())),
            }
        }
        if noted {
            // (A failure is noted for this answer already: a success doesn't undo it.)
        } else if worked && errors.is_empty() && !came.job_errors {
            self.driver.rules.book.forget_failures(account);
        } else if !came.consent_needed && !came.sign_in_refused && !came.unreadable {
            self.driver.rules.book.failed(account, started);
        }
        errors
    }
}
