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
    /// When the next job may start (milliseconds).
    next: f64,
    /// Jobs in a row that came to trouble, for the wait.
    failing: u32,
}

impl MailApp {
    /// The password the account has for `origin` ([`mailcraft_model::origin_of`] form), by the rules
    /// in the module's note. `None` when the owner has to be asked.
    pub(crate) fn dav_password(&self, account: Id, origin: &str) -> Option<String> {
        let a = self.session.mailbox.account(account)?;
        let server = a.dav.as_ref()?;
        // (Only for an origin the settings name: the credential store's names are by origin too.)
        if !server.home_origins().iter().any(|o| o == origin) {
            return None;
        }
        if let Some(held) = self.dav_passwords.get(&(account, origin.to_string())) {
            return Some(held.clone());
        }
        if let Some(stored) = Self::dav_secret_key(a, origin).and_then(|key| self.secret_cache.get(&key).cloned()) {
            return Some(stored);
        }
        if server.may_use_mail_password(origin) {
            return self.passwords.get(&account).cloned().or_else(|| self.remembered("password", account));
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
        let Ok(asked) = self.session.run("sync.davRequest", &json!({})) else { return };
        let Some(ask) = asked.get("ask").and_then(|a| serde_json::from_value::<DavAsk>(a.clone()).ok()) else { return };
        let mut passwords: HashMap<String, String> = HashMap::new();
        if let Some(server) = self.session.mailbox.account(ask.account).and_then(|a| a.dav.as_ref()) {
            for origin in server.home_origins() {
                if let Some(password) = self.dav_password(ask.account, &origin) {
                    passwords.insert(origin, password);
                }
            }
        }
        let account = ask.account;
        let Some(start) = &self.services.dav else { return };
        let receive = start(DavJob { ask, passwords });
        self.dav.job = Some((account, receive));
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
        // A password the server took is held for the session.
        for origin in signed_in {
            if let Some(password) = self.dav_password(account, &origin) {
                self.hold_dav_password(account, &origin, &password);
            }
        }
        let now = now_ms();
        if problem == DavProblem::None {
            self.dav.failing = 0;
            // (Another look at once when the run stopped for its limits: more is waiting.)
        } else {
            self.dav.failing = self.dav.failing.saturating_add(1);
            let wait = applied.get("wait").and_then(Value::as_f64).unwrap_or(0.0) * 1000.0;
            let backoff = (EVERY * 2f64.powi(i32::try_from(self.dav.failing).unwrap_or(10).min(10))).min(LONGEST);
            self.dav.next = now + wait.max(backoff);
        }
    }
}
