//! New mail in other accounts' Inboxes while one account's Send/Receive is busy for long.
//!
//! A run reads one account (its own Inbox now and then, `mailcraft-net`); every other account,
//! and the shared mailboxes among them, waits for it to end. So while a run has been at it for
//! [`AFTER_MS`], the Inboxes of the others are looked at in turn beside it, one at a time and
//! each at most every [`EVERY_MS`]: a look reads the Inbox alone
//! (`mailcraft_engine::cmd::account::inbox_request`), sends nothing and pushes nothing. Only an
//! account whose sign-in is at hand is looked at: a look never asks anybody to sign in.
//!
//! A look is also how word of new mail between two runs is answered ([`crate::inbox_watch`]):
//! the Inbox alone, rather than a run over every folder of the account, which the timed round
//! does ([`MailApp::want_inbox_look`]).

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_model::Id;
use mailcraft_model::sync::SyncResult;

use crate::MailApp;

/// How long a run goes before the other Inboxes are looked at beside it, and how often each is.
pub(crate) const AFTER_MS: f64 = 60_000.0;
pub(crate) const EVERY_MS: f64 = 60_000.0;
/// Most pieces of a look taken in a frame.
const PIECES_A_FRAME: usize = 16;

/// The looks under way and done.
#[derive(Default)]
pub struct InboxLooks {
    /// The look under way: whose, and its pieces to come.
    current: Option<(Id, Receiver<SyncResult>)>,
    /// When each account's Inbox was last looked at (milliseconds).
    looked: HashMap<Id, f64>,
    /// When the run in flight began (milliseconds), while there is one.
    began: Option<f64>,
    /// Inboxes to look at as soon as a look can start, in the order the word came.
    wanted: VecDeque<Id>,
}

impl InboxLooks {
    /// Whether a look is under way.
    #[cfg(test)]
    pub(crate) fn looking(&self) -> Option<Id> {
        self.current.as_ref().map(|(id, _)| *id)
    }
}

impl MailApp {
    /// Every frame, at `now` (milliseconds): take in what a look brought, and begin the next one
    /// when a run has been busy for long.
    pub(crate) fn poll_inbox_looks(&mut self, now: f64) {
        self.take_in_inbox_look();
        let running = self.sync_run.map(|(id, ..)| id).filter(|_| self.sync_rx.is_some());
        // An Inbox that was heard to change is looked at first, run or no run.
        while self.inbox_looks.current.is_none() && self.services.sync.is_some() {
            let Some(account) = self.inbox_looks.wanted.pop_front() else { break };
            // A run of the account in flight or waiting its turn brings its new mail itself.
            if running == Some(account) || self.sync_queue.iter().any(|job| job.account == account) {
                continue;
            }
            self.inbox_looks.looked.insert(account, now);
            let Some(request) = self.inbox_request(account) else { continue };
            if let Some(start) = &self.services.sync {
                self.inbox_looks.current = Some((account, start(request)));
            }
        }
        let Some(running) = running else {
            self.inbox_looks.began = None;
            return;
        };
        let began = *self.inbox_looks.began.get_or_insert(now);
        if self.inbox_looks.current.is_some() || now - began < AFTER_MS || self.services.sync.is_none() {
            return;
        }
        // The account looked at longest ago (never, first), but the one the run is for.
        let due = self
            .server_accounts()
            .into_iter()
            .filter(|id| *id != running)
            .filter(|id| self.inbox_looks.looked.get(id).is_none_or(|at| now - at >= EVERY_MS || now < *at))
            .min_by(|a, b| {
                let at = |id: &Id| self.inbox_looks.looked.get(id).copied().unwrap_or(f64::MIN);
                at(a).total_cmp(&at(b))
            });
        let Some(account) = due else { return };
        // Looked at, or found it can't be now: either way not again for a while.
        self.inbox_looks.looked.insert(account, now);
        let Some(request) = self.inbox_request(account) else { return };
        if let Some(start) = &self.services.sync {
            self.inbox_looks.current = Some((account, start(request)));
        }
    }

    /// Word came that the Inbox of `account` changed ([`crate::inbox_watch`]): it is looked at
    /// as soon as a look can start, unless a run of the account is at it or waiting to be.
    pub(crate) fn want_inbox_look(&mut self, account: Id) {
        if !self.inbox_looks.wanted.contains(&account) {
            self.inbox_looks.wanted.push_back(account);
        }
    }

    /// What a look at the Inbox of `account` asks, with the sign-in that is at hand; `None` when
    /// there is none (nobody is asked to sign in for a look).
    fn inbox_request(&self, account: Id) -> Option<mailcraft_model::sync::SyncRequest> {
        let mut request = mailcraft_engine::cmd::account::inbox_request(&self.session, account).ok()?;
        if request.account.oauth.is_some() {
            // A shared mailbox goes with its owner's sign-in, the way the owner's mail is read.
            let signer = mailcraft_engine::cmd::shared::signer(&self.session, account);
            let access = if self.mail_through_graph(account) {
                Some(self.graph_access(signer, false)).filter(|access| !access.is_empty())
            } else {
                self.tokens.get(&signer).filter(|t| t.fresh(mailcraft_time::now())).map(|t| t.access.clone())
            };
            request.access_token = access?;
        } else {
            request.password = self.passwords.get(&account)?.clone();
        }
        Some(request)
    }

    /// Take in the pieces of the look under way as they come.
    fn take_in_inbox_look(&mut self) {
        let Some((account, rx)) = self.inbox_looks.current.take() else { return };
        for _ in 0..PIECES_A_FRAME {
            match rx.try_recv() {
                Ok(piece) => {
                    let last = !piece.partial;
                    if !piece.errors.is_empty() {
                        // (A look is nobody's to hear about: the account's own run says what is wrong.)
                        log::debug!("a look at an Inbox: {} problem(s)", piece.errors.len());
                    }
                    if let Err(e) = mailcraft_engine::cmd::account::apply_sync(&mut self.session, piece) {
                        log::debug!("a look at an Inbox couldn't be taken in: {e}");
                    }
                    if last {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        self.inbox_looks.current = Some((account, rx));
    }
}

#[cfg(test)]
mod tests;
