// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contacts of an account kept in step over JMAP (RFC 9610), in the window: read from its
//! server by themselves, and never written there.
//!
//! The host does the talking ([`crate::Services::jmap_contacts`]): one small job reads every
//! contact of every address book of the account, and what it brings is merged as a Microsoft
//! account's contacts are (`mailcraft_engine::cmd::remote::apply_contacts`). A job goes with the
//! secret the account's Send/Receive goes with ([`MailApp::jmap_secret`]): none at hand, no job,
//! and nothing here ever asks the owner for one.
//!
//! **When**: right after the first Send/Receive of the account that ended without the server
//! turning its secret down, after every Send/Receive the owner asks for, and otherwise every
//! quarter of an hour while Send/Receive runs by itself. One job
//! at a time, never while that account's Send/Receive is under way or waits its turn, and never
//! for an account whose secret its server turned down, until the owner asks for its Send/Receive.
//!
//! **A server that keeps no contacts** for the account says so once, and is not asked again this
//! session unless the owner asks for the account's Send/Receive by hand. A job that came to
//! nothing is tried again after a while, twice as long after each one more; a server that says it
//! is busy is left alone for as long as it says.
//!
//! **Work started for one mailbox stays with it**: a job under way when another mailbox takes its
//! place is called off, and an answer is taken in only by the account it was asked for, while it
//! still has the address and the server it was asked with.
//!
//! When a job is due, the wait after one that came to nothing, and whose an answer is, are what
//! every such job shares ([`crate::jmap_side_job`]); here is what is asked for contacts, and how
//! what comes is taken in.
//!
//! The log names the account's id and counts, never an address, the server's or anybody's name.

use mailcraft_engine::cmd::remote;
use mailcraft_model::sync::{JmapContactsRequest, JmapContactsResult};
use mailcraft_model::{Account, AccountKind, Id};

use crate::jmap_side_job::{Answer, EVERY_MS, MAX_PROBLEMS, SideJob};
use crate::{MailApp, now_ms};

impl Answer for JmapContactsResult {
    const JOB: &'static str = "jmap contacts";
    const READS: &'static str = "contacts";

    fn whose(&self) -> (Id, &str, &str) {
        (self.account, &self.email, &self.session)
    }
}

/// What the app remembers of reading JMAP accounts' contacts.
pub(crate) type JmapContacts = SideJob<JmapContactsResult>;

impl MailApp {
    /// Whether a JMAP account's contacts are being read.
    pub fn jmap_contacts_syncing(&self) -> bool {
        self.jmap_contacts.running()
    }

    /// Take in an answer that has come, and start a job when one is due. Called every frame.
    pub(crate) fn jmap_contacts_tick(&mut self, ctx: &egui::Context) {
        let now = now_ms();
        self.poll_jmap_contacts_at(now);
        self.jmap_contacts_tick_at(now);
        if self.jmap_contacts.running() {
            // An answer is on its way (the host wakes the window when it is there, too).
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Whether `account`'s contacts are to be asked for at `now`.
    fn jmap_contacts_due(&self, account: Id, now: f64) -> bool {
        let state = &self.jmap_contacts;
        self.jmap_side_free(account) && !state.held(account, now) && state.wanted(account, now, EVERY_MS, self.auto_sync)
    }

    /// [`jmap_contacts_tick`](Self::jmap_contacts_tick) at `now` (milliseconds), without looking
    /// for an answer. Whether a job was started. (Public for a host's tests, which have no window.)
    pub fn jmap_contacts_tick_at(&mut self, now: f64) -> bool {
        if self.jmap_contacts.running() || self.services.jmap_contacts.is_none() || self.unconfirmed || self.data_removed || self.offline() {
            return false;
        }
        let theirs = |a: &&Account| a.kind == AccountKind::Jmap && a.keeps_contacts();
        let accounts: Vec<Id> = self.session.mailbox.accounts.iter().filter(theirs).map(|a| a.id).collect();
        for account in accounts {
            if !self.jmap_contacts_due(account, now) {
                continue;
            }
            let Some(asking) = self.jmap_side_asking(account) else { continue };
            let request = JmapContactsRequest { account, email: asking.email.clone(), session: asking.session.clone(), bearer: asking.bearer };
            let Some(ask) = self.services.jmap_contacts.as_ref() else { return false };
            let (job, asked) = asking.job(request, self.session.openings());
            self.jmap_contacts.start(now, asked, ask(job));
            return true;
        }
        false
    }

    /// Take in what the job in flight brought, when it has: the contacts, and what went wrong.
    /// (Public for a host's tests.)
    pub fn poll_jmap_contacts_at(&mut self, now: f64) {
        let Some((job, result)) = self.jmap_contacts.answer(self.session.openings(), now) else { return };
        let account = job.account;
        if !self.jmap_side_meant(&job, &result, Account::keeps_contacts) {
            return;
        }
        let mut errors: Vec<String> = result.errors.into_iter().take(MAX_PROBLEMS).collect();
        if self.jmap_side_ended(|app| &mut app.jmap_contacts, account, result.auth_refused, result.not_offered) {
            self.note_sync_problems(account, &errors, false);
            return;
        }
        let mut taken = false;
        if let Some(download) = &result.contacts {
            match remote::apply_contacts(&mut self.session, download) {
                Ok(_) => {
                    taken = true;
                    log::debug!(
                        "jmap contacts: {} taken in (all of them: {}), {} problem(s)",
                        download.contacts.len(),
                        download.complete,
                        errors.len()
                    );
                }
                Err(e) => errors.push(crate::i18n::fmt(tl!("The contacts couldn't be taken in: {error}"), &[("error", &e.to_string())])),
            }
        }
        self.jmap_contacts.settle(account, now, result.busy_for, result.stopped, taken && errors.is_empty());
        if !errors.is_empty() {
            log::warn!("jmap contacts: {} problem(s)", errors.len());
        }
        self.note_sync_problems(account, &errors, false);
    }
}

#[cfg(test)]
#[path = "jmap_contacts_tests.rs"]
mod tests;
