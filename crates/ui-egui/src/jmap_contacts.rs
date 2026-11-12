// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contacts of an account kept in step over JMAP (RFC 9610), in the window: kept in step
//! with its server by themselves, both ways.
//!
//! The host does the talking ([`crate::Services::jmap_contacts`]): one small job tells the server
//! what was made, changed or deleted here (`contacts_remote::contact_upload`), then reads every
//! contact of every address book of the account. What the server made of the changes is taken in
//! first (`contacts_remote::apply_contact_changes`), then what it has is merged with what is here
//! (`mailcraft_engine::cmd::remote::apply_contacts`); what it refused is said to the owner where
//! a Send/Receive's problems are. A job goes with the secret the account's Send/Receive goes with
//! ([`MailApp::jmap_secret`]): none at hand, no job, and nothing here ever asks the owner for one.
//!
//! **When**: right after the first Send/Receive of the account that ended without the server
//! turning its secret down, after every Send/Receive the owner asks for, and otherwise every
//! quarter of an hour while Send/Receive runs by itself. While it does, a change made here goes
//! a few seconds after the last one ([`QUIET_MS`]), without waiting for the quarter of an hour;
//! one that a job left waiting without getting any of it through is tried again later each time.
//! One job at a time, never while that account's Send/Receive is under way or waits its turn, and
//! never for an account whose secret its server turned down, until the owner asks for its
//! Send/Receive.
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
//! every such job shares ([`crate::jmap_side_job`]); here is what is asked for contacts, when a
//! change made here goes, and how what comes is taken in.
//!
//! The log names the account's id and counts, never an address, the server's or anybody's name.

use std::collections::HashMap;

use mailcraft_engine::cmd::{contacts_remote, remote};
use mailcraft_model::remote::{ContactsUpload, ContactsUploaded};
use mailcraft_model::sync::{JmapContactsRequest, JmapContactsResult};
use mailcraft_model::{Account, AccountKind, Id};

use crate::jmap_side_job::{Answer, Asked, EVERY_MS, MAX_PROBLEMS, SideJob};
use crate::{MailApp, now_ms};

/// How long a change made here is left to settle before it goes (milliseconds): a second change
/// to the same contact goes with the first, and one taken back with Undo doesn't go at all.
const QUIET_MS: f64 = 3_000.0;

impl Answer for JmapContactsResult {
    const JOB: &'static str = "jmap contacts";
    const READS: &'static str = "contacts";

    fn whose(&self) -> (Id, &str, &str) {
        (self.account, &self.email, &self.session)
    }
}

/// What the app remembers of reading JMAP accounts' contacts, and of telling their servers what
/// was changed here.
#[derive(Default)]
pub(crate) struct JmapContacts {
    /// The job, and when the next is due ([`crate::jmap_side_job`]).
    side: SideJob<JmapContactsResult>,
    /// What the job in flight carries: the changes as they were worked out (what an answer is read
    /// against), and how many changes waited when it started.
    carried: Option<(ContactsUpload, usize)>,
    /// How many changes wait for each account's server, as of a revision of the mailbox: counted
    /// once for each, not at every frame.
    ready: Option<(u64, HashMap<Id, usize>)>,
    /// How many waited when that was last looked at, and since when it has been that many.
    waiting: HashMap<Id, (usize, f64)>,
    /// How many jobs in a row carried changes for an account and got none of them through.
    stuck: HashMap<Id, u32>,
}

impl JmapContacts {
    /// Another mailbox took the place of the open one ([`SideJob::forget`]): what waited for its
    /// accounts' servers goes with it.
    pub(crate) fn forget(&mut self) {
        self.side.forget();
        *self = Self::default();
    }

    /// `account` is gone, or names another server now ([`SideJob::forget_account`]).
    pub(crate) fn forget_account(&mut self, account: Id) {
        self.side.forget_account(account);
        self.waiting.remove(&account);
        self.stuck.remove(&account);
        self.ready = None;
    }

    /// The owner asked for a Send/Receive of `account`: its contacts follow, whatever came of the
    /// jobs before ([`SideJob::by_hand`]).
    pub(crate) fn by_hand(&mut self, account: Id) {
        self.forget_account(account);
        self.side.by_hand(account);
    }

    /// A Send/Receive of `account` has ended well enough ([`SideJob::after_run`]).
    pub(crate) fn after_run(&mut self, account: Id) {
        self.side.after_run(account);
    }

    /// What the job in flight brought, when it has ([`SideJob::answer`]), with what it carried.
    fn answer(&mut self, opening: u64, now: f64) -> Option<(Asked, JmapContactsResult, (ContactsUpload, usize))> {
        let stale = self.side.stale(opening);
        let got = self.side.answer(opening, now);
        if stale {
            *self = Self::default();
        }
        if self.side.running() {
            return None;
        }
        let carried = self.carried.take().unwrap_or_default();
        got.map(|(job, result)| (job, result, carried))
    }
}

impl MailApp {
    /// Whether a JMAP account's contacts are being read.
    pub fn jmap_contacts_syncing(&self) -> bool {
        self.jmap_contacts.side.running()
    }

    /// Take in an answer that has come, and start a job when one is due. Called every frame.
    pub(crate) fn jmap_contacts_tick(&mut self, ctx: &egui::Context) {
        let now = now_ms();
        self.poll_jmap_contacts_at(now);
        self.jmap_contacts_tick_at(now);
        if self.jmap_contacts.side.running() {
            // An answer is on its way (the host wakes the window when it is there, too).
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if self.auto_sync && self.jmap_contacts.waiting.values().any(|(waiting, _)| *waiting > 0) {
            // A change waits out its quiet moment: looked at again when it is over.
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    /// How many changes to `account`'s contacts wait for its server, counted once for a revision
    /// of the mailbox.
    fn jmap_contacts_waiting(&mut self, account: Id) -> usize {
        let rev = self.session.rev();
        let state = &mut self.jmap_contacts;
        if state.ready.as_ref().is_none_or(|(at, _)| *at != rev) {
            state.ready = Some((rev, HashMap::new()));
        }
        if let Some(n) = state.ready.as_ref().and_then(|(_, counted)| counted.get(&account).copied()) {
            return n;
        }
        let n = contacts_remote::contacts_waiting(&self.session.mailbox, account);
        if let Some((_, counted)) = &mut self.jmap_contacts.ready {
            counted.insert(account, n);
        }
        n
    }

    /// Whether changes made here to `account`'s contacts are to go at `now`: some wait, and it has
    /// been quiet since the last of them, for longer each time a job got none of them through.
    fn jmap_contacts_push(&mut self, account: Id, now: f64) -> bool {
        let ready = self.jmap_contacts_waiting(account);
        let state = &mut self.jmap_contacts;
        let since = match state.waiting.get(&account) {
            Some((seen, since)) if *seen == ready => *since,
            _ => {
                state.waiting.insert(account, (ready, now));
                now
            }
        };
        let doublings = i32::try_from(state.stuck.get(&account).copied().unwrap_or(0).min(16)).unwrap_or(16);
        ready > 0 && now - since >= (QUIET_MS * 2f64.powi(doublings)).min(EVERY_MS)
    }

    /// A job of `account` that carried `sent` changes, of the `waited` that waited, has been
    /// taken in at `now`: whether it got any of them through decides how soon the rest goes.
    fn jmap_contacts_settle(&mut self, account: Id, sent: usize, waited: usize, now: f64) {
        self.jmap_contacts.ready = None;
        let left = self.jmap_contacts_waiting(account);
        let state = &mut self.jmap_contacts;
        if sent > 0 && left >= waited {
            let count = state.stuck.get(&account).copied().unwrap_or(0).saturating_add(1);
            state.stuck.insert(account, count);
        } else {
            state.stuck.remove(&account);
        }
        // What is left waits its quiet moment from now.
        state.waiting.insert(account, (left, now));
    }

    /// Whether `account`'s contacts are to be asked for at `now`; `push`: changes made here are
    /// to go ([`Self::jmap_contacts_push`]).
    fn jmap_contacts_due(&self, account: Id, now: f64, push: bool) -> bool {
        let state = &self.jmap_contacts;
        let free = self.jmap_side_free(account) && !state.side.held(account, now);
        free && (push || state.side.wanted(account, now, EVERY_MS, self.auto_sync))
    }

    /// [`jmap_contacts_tick`](Self::jmap_contacts_tick) at `now` (milliseconds), without looking
    /// for an answer. Whether a job was started. (Public for a host's tests, which have no window.)
    pub fn jmap_contacts_tick_at(&mut self, now: f64) -> bool {
        if self.jmap_contacts.side.running() || self.services.jmap_contacts.is_none() || self.unconfirmed || self.data_removed || self.offline() {
            return false;
        }
        let theirs = |a: &&Account| a.kind == AccountKind::Jmap && a.keeps_contacts();
        let accounts: Vec<Id> = self.session.mailbox.accounts.iter().filter(theirs).map(|a| a.id).collect();
        for account in accounts {
            // (Nothing by itself while Send/Receive doesn't run by itself: what was changed
            // here goes with the next the owner asks for.)
            let push = self.auto_sync && self.jmap_contacts_push(account, now);
            if !self.jmap_contacts_due(account, now, push) {
                continue;
            }
            let Some(asking) = self.jmap_side_asking(account) else { continue };
            // What was made, changed or deleted here goes first; the server is then read.
            let sent = contacts_remote::contact_upload(&self.session.mailbox, account).unwrap_or_default();
            let (email, session, changes) = (asking.email.clone(), asking.session.clone(), sent.ops.clone());
            let request = JmapContactsRequest { account, email, session, bearer: asking.bearer, changes };
            let waited = self.jmap_contacts_waiting(account);
            let Some(ask) = self.services.jmap_contacts.as_ref() else { return false };
            let (job, asked) = asking.job(request, self.session.openings());
            let rx = ask(job);
            // A deletion that goes out can't be undone: the contact would come back here, gone there.
            contacts_remote::mark_sent(&mut self.session, contacts_remote::carries_delete(&sent));
            log::debug!("jmap contacts: {} change(s) go with it", sent.ops.len());
            self.jmap_contacts.carried = Some((sent, waited));
            self.jmap_contacts.side.start(now, asked, rx);
            return true;
        }
        false
    }

    /// Take in what the job in flight brought, when it has: the contacts, and what went wrong.
    /// (Public for a host's tests.)
    pub fn poll_jmap_contacts_at(&mut self, now: f64) {
        let Some((job, result, (sent, waited))) = self.jmap_contacts.answer(self.session.openings(), now) else { return };
        let account = job.account;
        if !self.jmap_side_meant(&job, &result, Account::keeps_contacts) {
            return;
        }
        let mut errors: Vec<String> = result.errors.into_iter().take(MAX_PROBLEMS).collect();
        // What the server made of the changes, before anything it was read for: the contacts in
        // the same answer were read after them. Whatever else the answer says, what was done is done.
        let carried = sent.ops.len();
        let mut said: Vec<String> = Vec::new();
        if !result.changed.is_empty() {
            let uploaded = ContactsUploaded { account, email: job.email.clone(), outcomes: result.changed };
            match contacts_remote::apply_contact_changes(&mut self.session, &uploaded, Some(&sent)) {
                Ok(merged) => {
                    log::debug!(
                        "jmap contacts: of {carried} change(s), {} taken, {} not, {} refused",
                        merged.added + merged.updated + merged.removed,
                        merged.skipped,
                        merged.put_back
                    );
                    said = merged.messages;
                }
                Err(e) => errors.push(crate::i18n::fmt(
                    tl!("What the server said of the changed contacts couldn't be taken in: {error}"),
                    &[("error", &e.to_string())],
                )),
            }
        }
        // What the engine says of refusals is for the owner, beside what went wrong.
        let told = |errors: &[String]| errors.iter().chain(&said).take(MAX_PROBLEMS).cloned().collect::<Vec<String>>();
        if self.jmap_side_ended(|app| &mut app.jmap_contacts.side, account, result.auth_refused, result.not_offered) {
            self.note_sync_problems(account, &told(&errors), false);
            self.jmap_contacts_settle(account, carried, waited, now);
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
        self.jmap_contacts.side.settle(account, now, result.busy_for, result.stopped, taken && errors.is_empty());
        if !errors.is_empty() {
            log::warn!("jmap contacts: {} problem(s)", errors.len());
        }
        self.note_sync_problems(account, &told(&errors), false);
        self.jmap_contacts_settle(account, carried, waited, now);
    }
}

#[cfg(test)]
#[path = "jmap_contacts_tests.rs"]
mod tests;
