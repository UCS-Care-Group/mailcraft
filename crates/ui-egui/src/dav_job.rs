// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keeping an account's calendars from a CalDAV server current, by itself (#209, spec C9): the
//! window asks the engine what to read (`sync.davRequest`), hands the host one job
//! ([`Services::dav`](crate::Services::dav)), and takes in what it brought (`sync.applyDav`). One
//! job at a time, never in the mail run, after the answer or the wait the server asked for.
//!
//! **Passwords.** The job is handed the password of each origin of the account's homes, resolved
//! here and nowhere else ([`MailApp::dav_password`]): the one held for that origin in this session,
//! else the one the credential store gave for it, else, only when the owner didn't give these
//! servers a password of their own and the origin is one the settings name
//! ([`mailcraft_model::DavServer::may_use_mail_password`]), the account's mail password. The host
//! sends one only in answer to that origin's own challenge. A password that was accepted is held
//! for the session under its origin, unless a forget is pending for its name
//! ([`MailApp::dav_forget_pending`]); nothing is written to the credential store here.
//!
//! Switched off ([`mailcraft_engine::Session::dav_enabled`]) it does nothing.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use mailcraft_model::{DavAsk, DavDownload, DavProblem, Id};
use serde_json::{Value, json};

use crate::{MailApp, now_ms};

/// How long between two jobs when all is well, and the longest wait after trouble (milliseconds).
const EVERY: f64 = 60_000.0;
const LONGEST: f64 = 900_000.0;
/// How long a server that refused the passwords it was given is left alone (milliseconds), unless
/// the account has others by then.
const REFUSED_FOR: f64 = 4.0 * 3_600_000.0;

/// What the host is to do: ask the server of one account, with these passwords (by origin).
pub struct DavJob {
    pub ask: DavAsk,
    /// The passwords the account has for its origins, which the host sends only in answer to that
    /// origin's own challenge.
    pub passwords: HashMap<String, String>,
}

/// Shows nothing of the asking and no password.
impl std::fmt::Debug for DavJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DavJob").field("passwords", &self.passwords.len()).finish()
    }
}

/// What the host read.
#[derive(Debug)]
pub struct DavDone {
    pub download: DavDownload,
}

/// What the window remembers of the job.
#[derive(Default)]
pub(crate) struct DavSync {
    opening: u64,
    /// The job in flight: its account, and the answer to come.
    job: Option<(Id, Receiver<DavDone>)>,
    /// When the next job may start (milliseconds), whichever account it is for.
    pub(crate) next: f64,
    /// For each account: jobs in a row that came to trouble, and when it may be asked again
    /// (milliseconds). One account's trouble holds up that account alone.
    pub(crate) trouble: HashMap<Id, (u32, f64)>,
    /// What the job in flight was given, by origin: the password, and whether it is the account's
    /// mail password (which is looked up at every job and never held).
    sent: HashMap<String, (String, bool)>,
    /// For each account whose server refused the passwords it was given: those passwords, and when.
    /// The account is left alone (nothing is sent again, so no lockout) until it has others, or
    /// for [`REFUSED_FOR`].
    pub(crate) refused: HashMap<Id, (HashMap<String, String>, f64)>,
    /// The trouble each account was last told to the owner for, so that it is said once.
    shown: HashMap<Id, DavProblem>,
    /// Whether each account's servers have a password of their own, to see it change.
    separate: HashMap<Id, bool>,
}

impl MailApp {
    /// The password the account has for `origin` ([`mailcraft_model::origin_of`] form), by the rules
    /// in the module's note. `None` when the owner has to be asked.
    #[cfg(test)]
    pub(crate) fn dav_password(&self, account: Id, origin: &str) -> Option<String> {
        self.dav_password_from(account, origin).map(|(password, _)| password)
    }

    /// As [`MailApp::dav_password`], and whether it is the account's mail password (which is
    /// asked again at every job, under the rule as it stands, and never held).
    fn dav_password_from(&self, account: Id, origin: &str) -> Option<(String, bool)> {
        let a = self.session.mailbox.account(account)?;
        let server = a.dav.as_ref()?;
        // (Only for an origin the settings name: the credential store's names are by origin too.)
        if !server.home_origins().iter().any(|o| o == origin) {
            return None;
        }
        if let Some(held) = self.dav_passwords.get(&(account, origin.to_string())) {
            return Some((held.clone(), false));
        }
        if let Some(stored) = Self::dav_secret_key(a, origin).and_then(|key| self.secret_cache.get(&key).cloned()) {
            return Some((stored, false));
        }
        if server.may_use_mail_password(origin) {
            return self.passwords.get(&account).cloned().or_else(|| self.remembered("password", account)).map(|p| (p, true));
        }
        None
    }

    /// Holds the password that was accepted for `origin` for this session, unless a forget is
    /// pending for the name it is kept under (then it waits: the next job resolves it again).
    /// Whether it was held.
    pub(crate) fn hold_dav_password(&mut self, account: Id, origin: &str, password: &str) -> bool {
        let Some(key) = self.session.mailbox.account(account).and_then(|a| Self::dav_secret_key(a, origin)) else { return false };
        if self.dav_forget_pending(&key) || password.is_empty() {
            return false;
        }
        self.dav_passwords.insert((account, origin.to_string()), password.to_string());
        true
    }

    /// Once a frame: take in a job that came back, and start the next when it is due.
    pub(crate) fn dav_tick(&mut self) {
        if !self.session.dav_enabled {
            return;
        }
        let opening = self.session.openings();
        if self.dav.opening != opening {
            self.dav = DavSync { opening, ..Default::default() };
        }
        // The owner gave these servers a password of their own, or took it back: what was held
        // for the account is not what it is to be given now.
        let flags: Vec<(Id, bool)> =
            self.session.mailbox.accounts.iter().filter_map(|a| a.dav.as_ref().map(|d| (a.id, d.separate_password))).collect();
        for (id, flag) in flags {
            if self.dav.separate.insert(id, flag).is_some_and(|was| was != flag) {
                self.dav_passwords.retain(|(held, _), _| *held != id);
            }
        }
        if let Some((account, receive)) = &self.dav.job {
            let account = *account;
            let done = match receive.try_recv() {
                Ok(done) => done,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    DavDone { download: DavDownload { account, problem: DavProblem::Unreachable, ..Default::default() } }
                }
            };
            self.dav.job = None;
            self.take_in_dav(done);
            return;
        }
        let now = now_ms();
        if now < self.dav.next || self.data_removed || self.unconfirmed || self.opening.is_some() || self.offline() || self.services.dav.is_none() {
            return;
        }
        self.dav.next = now + EVERY;
        // Accounts to pass over: one in its back-off, or whose server refused what it would be
        // given now (left alone for hours, and until it has other passwords).
        let ids: Vec<Id> = self.session.mailbox.accounts.iter().filter(|a| a.dav.is_some()).map(|a| a.id).collect();
        let mut skip: Vec<Id> = Vec::new();
        for id in ids {
            if self.dav.trouble.get(&id).is_some_and(|(_, next)| now < *next) {
                skip.push(id);
                continue;
            }
            if let Some((refused, at)) = self.dav.refused.get(&id) {
                let current: HashMap<String, String> = self.dav_passwords_of(id).into_iter().map(|(o, (p, _))| (o, p)).collect();
                if now < *at + REFUSED_FOR && current == *refused {
                    skip.push(id);
                } else {
                    self.dav.refused.remove(&id);
                }
            }
        }
        let Ok(asked) = self.session.run("sync.davRequest", &json!({"skip": skip})) else { return };
        let Some(ask) = asked.get("ask").and_then(|a| serde_json::from_value::<DavAsk>(a.clone()).ok()) else { return };
        let account = ask.account;
        let sent = self.dav_passwords_of(account);
        let passwords: HashMap<String, String> = sent.iter().map(|(o, (p, _))| (o.clone(), p.clone())).collect();
        let Some(start) = &self.services.dav else { return };
        self.dav.sent = sent;
        let receive = start(DavJob { ask, passwords });
        self.dav.job = Some((account, receive));
    }

    /// What each origin of the account's homes would be given now, and whether it is the mail password.
    fn dav_passwords_of(&self, account: Id) -> HashMap<String, (String, bool)> {
        let mut passwords = HashMap::new();
        if let Some(server) = self.session.mailbox.account(account).and_then(|a| a.dav.as_ref()) {
            for origin in server.home_origins() {
                if let Some(found) = self.dav_password_from(account, &origin) {
                    passwords.insert(origin, found);
                }
            }
        }
        passwords
    }

    fn take_in_dav(&mut self, done: DavDone) {
        let download = done.download;
        let (account, problem, signed_in) = (download.account, download.problem, download.signed_in.clone());
        let applied = match self.session.run("sync.applyDav", &json!({"result": download})) {
            Ok(applied) => applied,
            Err(e) => {
                log::warn!("a calendar server's answer couldn't be taken in: {e}");
                Value::Null
            }
        };
        // A password the server took is held for the session (the one that was sent, not what
        // would be found now), unless it is the mail password: that one is looked up at every
        // job, by the rule as it stands.
        let sent = std::mem::take(&mut self.dav.sent);
        for origin in signed_in {
            if let Some((password, false)) = sent.get(&origin) {
                self.hold_dav_password(account, &origin, password);
            }
        }
        let now = now_ms();
        if problem == DavProblem::WrongPassword {
            // Not held, not sent again (at every back-off: a lockout) until the owner gives
            // another, or for hours (a password changed on the server, a mistaken refusal).
            self.dav_passwords.retain(|(held, _), _| *held != account);
            self.dav.refused.insert(account, (sent.into_iter().map(|(o, (p, _))| (o, p)).collect(), now));
        }
        if problem == DavProblem::None {
            self.dav.trouble.remove(&account);
            self.dav.shown.remove(&account);
            // (Another look at once when the run stopped for its limits: more is waiting.)
        } else {
            let failing = self.dav.trouble.get(&account).map_or(0, |(n, _)| *n).saturating_add(1);
            let wait = applied.get("wait").and_then(Value::as_f64).unwrap_or(0.0) * 1000.0;
            let backoff = (EVERY * 2f64.powi(i32::try_from(failing).unwrap_or(10).min(10))).min(LONGEST);
            self.dav.trouble.insert(account, (failing, now + wait.max(backoff)));
            // Said to the owner once for each kind of trouble (nothing of the server is named).
            if self.dav.shown.insert(account, problem) != Some(problem) {
                match problem {
                    DavProblem::WrongPassword | DavProblem::NeedsPassword | DavProblem::UnsupportedSignIn => {
                        self.status(tl!("A calendar server wouldn't sign in. MailCraft will try again later, or when the password changes."));
                    }
                    DavProblem::Certificate => self.status(tl!("A calendar server's certificate isn't trusted, so nothing was sent to it.")),
                    _ => {}
                }
            }
        }
    }
}
