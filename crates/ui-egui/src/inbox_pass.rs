// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Inbox pass of a Send/Receive (#276): before anything else of a round (token renewal
//! aside), every account kept in step through Microsoft Graph, the owner's and each shared
//! mailbox, brings its Inbox's delta from the link kept for it, so that new mail is on screen
//! within seconds. Then the round goes on account by account as it always did: sending, folders,
//! new mail.
//!
//! - An account whose Inbox has no delta link yet has no pass; a mail server's account has none.
//! - The pass is its own small Graph job (the Inbox's delta alone, no folder changes, nothing
//!   sent), its pieces taken in as they come. The run that follows is built after it, from the
//!   links the pass moved on, so that the Inbox is not read twice.
//! - Nobody is asked anything for it: an account that would have to sign in has no pass, and its
//!   run asks as it always did.
//! - Stop ends the round, whichever pass it falls in. Skip leaves an account's other phases out;
//!   its pass is never left out.
//!
//! **The Inbox checked by itself.** Between rounds, every minute while the window is open, each
//! of those accounts has its Inbox pass on its own ([`MailApp::inbox_check_tick`]), one account
//! after the other, so that new mail is on screen within a minute while a round comes every few.
//!
//! - Only while Send/Receive runs by itself (`syncMinutes` above 0): an owner who turned that off
//!   is not checked behind their back either.
//! - Never while a round, a sign-in or another check is under way, nor for an account whose
//!   calendar's job is at work: that tick is left out, never kept for later. A round reads the
//!   Inbox itself, and the next check is a minute after it.
//! - Quiet: Send/Receive Progress and the status line stay as they were, nothing is asked and
//!   nothing is shown; what went wrong goes to the log as a round's does, and the next tick tries
//!   again; a check that keeps failing says so once, until one gets through. A run the owner
//!   asks for meanwhile goes ahead: the check gives way, and whatever sign-in it renewed is kept
//!   all the same.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_model::{AccountKind, Id};
use serde_json::{Value, json};

use crate::{GraphRequest, GraphResult, MailApp, Tokens};

/// How often the Inbox is checked by itself, between rounds (seconds).
pub(crate) const INBOX_CHECK_SECONDS: i64 = 60;

/// The Inbox checked by itself every minute (#276).
#[derive(Default)]
pub(crate) struct InboxCheck {
    /// The account whose Inbox is being checked.
    running: Option<Id>,
    /// Its job's own Stop (a round's Stop is not for it, nor this one for a round).
    cancel: Option<Arc<AtomicBool>>,
    /// The accounts of this tick still to be checked.
    queue: VecDeque<Id>,
    /// When the last tick was, or a round last kept it from coming (the session's clock, in
    /// seconds; 0 before the first).
    last: i64,
    /// Checks that gave way to a run: only their answer's sign-in is still waited for.
    gave_way: Vec<(Id, Receiver<GraphResult>)>,
    /// Accounts whose last check went wrong and said so: said once, until one gets through.
    failing: HashSet<Id>,
}

impl InboxCheck {
    /// Whether a check is under way.
    pub(crate) fn under_way(&self) -> bool {
        self.running.is_some()
    }
}

impl MailApp {
    /// Every frame: the Inbox of every account kept in step through Graph is checked by itself
    /// once a minute, between rounds, by the session's clock. The account whose check was
    /// started, if one was.
    pub(crate) fn inbox_check_tick(&mut self) -> Option<Id> {
        self.take_in_given_way();
        let now = self.session.now();
        if !self.auto_sync || self.session.prefs.sync_minutes == 0 || self.services.graph.is_none() {
            self.inbox_check.queue.clear();
            return None;
        }
        // One at a time, and never two ticks on top of each other.
        if self.inbox_check.under_way() {
            return None;
        }
        let round = self.sync_rx.is_some()
            || self.graph_rx.is_some()
            || !self.sync_queue.is_empty()
            || !self.inbox_queue.is_empty()
            || self.signing_in.is_some()
            || self.approving()
            || self.graph_switch_copy.is_some();
        // (A clock put back starts the minute again.)
        if round || self.inbox_check.last == 0 || now < self.inbox_check.last {
            self.inbox_check.queue.clear();
            self.inbox_check.last = now;
            return None;
        }
        if self.inbox_check.queue.is_empty() {
            if now.saturating_sub(self.inbox_check.last) < INBOX_CHECK_SECONDS {
                return None;
            }
            self.inbox_check.last = now;
            let graph = |a: &&mailcraft_model::Account| a.kind == AccountKind::Microsoft;
            let accounts: Vec<Id> = self.session.mailbox.accounts.iter().filter(graph).map(|a| a.id).collect();
            self.inbox_check.queue = accounts
                .into_iter()
                .filter(|account| {
                    mailcraft_engine::cmd::graph_mail::graph_inbox_request(&self.session, *account).is_some_and(|r| !r.links.is_empty())
                })
                .collect();
        }
        while let Some(account) = self.inbox_check.queue.pop_front() {
            // (The calendar's job is at work for it: its check is left out of this tick.)
            if !self.calendar_job_for(account) && self.start_inbox_pass(account, true, true).is_some() {
                return Some(account);
            }
        }
        None
    }

    /// A run is about to start: the Inbox check under way, if any, gives way to it. Its job is
    /// told to stop and its mail is no longer listened to; what it already brought stays, and the
    /// run reads the Inbox from there. A sign-in it renewed is kept, now if its answer is in (the
    /// run then goes with it), else when the answer comes.
    pub(crate) fn give_up_inbox_check(&mut self) {
        self.inbox_check.queue.clear();
        if self.inbox_check.running.take().is_none() {
            return;
        }
        if let Some(cancel) = self.inbox_check.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some((account, _, _, job)) = self.graph_rx.take() {
            self.inbox_check.gave_way.push((account, job));
            self.take_in_given_way();
        }
        self.graph_pieces = None;
        self.inbox_run = None;
        self.graph_pieces_taken = 0;
        self.graph_problems.clear();
    }

    /// A round of `accounts` is about to begin: those with an Inbox pass line up for it, in the
    /// round's order.
    pub(crate) fn queue_inbox_passes(&mut self, accounts: &[Id], quiet: bool) {
        self.inbox_queue = accounts
            .iter()
            .copied()
            .filter(|account| mailcraft_engine::cmd::graph_mail::graph_inbox_request(&self.session, *account).is_some())
            .map(|account| (account, quiet))
            .collect();
    }

    /// Start the next Inbox pass in line; an account that can't have one now (it would have to
    /// sign in, or Microsoft won't open it) goes without. What was started, if anything.
    pub(crate) fn start_next_inbox_pass(&mut self) -> Option<Value> {
        while let Some((account, quiet)) = self.inbox_queue.pop_front() {
            if let Some(started) = self.start_inbox_pass(account, quiet, false) {
                return Some(started);
            }
        }
        None
    }

    /// The Inbox pass of `account`, as a Graph job of its own: a round's, or the Inbox checked by
    /// itself (`check`).
    fn start_inbox_pass(&mut self, account: Id, quiet: bool, check: bool) -> Option<Value> {
        self.services.graph.as_ref()?;
        if self.sync_rx.is_some() || self.graph_rx.is_some() || (quiet && !self.can_sync_quietly(account)) {
            return None;
        }
        let mailbox = self.session.mailbox.account(account).cloned()?;
        // A shared mailbox is read with its owner's sign-in.
        let owner = if mailbox.is_shared() { self.session.mailbox.owner_of(account).cloned()? } else { mailbox.clone() };
        if mailbox.is_shared() && quiet && self.shared_refused.contains_key(&account) {
            return None;
        }
        let oauth = owner.oauth.clone()?;
        let kept = self.tokens.get(&owner.id).or(self.graph_tokens.get(&owner.id)).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", &owner.email)).unwrap_or_default();
        if refresh.is_empty() {
            return None;
        }
        let mail = mailcraft_engine::cmd::graph_mail::graph_inbox_request(&self.session, account)?;
        // A check every minute goes with the token it had a minute ago while that is still good,
        // as the calendar's job does: one request, not two.
        let access = if check {
            self.graph_tokens.get(&account).filter(|t| t.fresh(mailcraft_time::now() + 60)).map(|t| t.access.clone()).unwrap_or_default()
        } else {
            String::new()
        };
        let cancel = if check { Arc::new(AtomicBool::new(false)) } else { self.services.sync_cancel.clone() };
        let (pieces, taken) = std::sync::mpsc::sync_channel(8);
        let ask = GraphRequest {
            account: mailbox,
            oauth: self.effective_oauth(oauth),
            refresh,
            // (A round's is renewed for the job, as the run's own is.)
            access,
            outbox: Vec::new(),
            directory: false,
            calendar: None,
            changes: None,
            asks: Vec::new(),
            settings: false,
            favorites: false,
            favorites_scopes: false,
            mail: Some(mail),
            mail_pieces: Some(pieces),
            cancel: cancel.clone(),
        };
        let graph = self.services.graph.as_ref()?;
        if !check {
            self.services.sync_cancel.store(false, Ordering::Relaxed);
        }
        let job = graph(ask);
        self.graph_pieces = Some(taken);
        self.graph_problems.clear();
        self.graph_pieces_taken = 0;
        self.inbox_run = Some(account);
        if check {
            // Quiet: no row of Send/Receive Progress, no word in the status line.
            (self.inbox_check.running, self.inbox_check.cancel) = (Some(account), Some(cancel));
            self.graph_rx = Some((account, true, false, job));
            return Some(json!({"started": true, "account": account, "graph": true, "inbox": true, "check": true}));
        }
        // (No run of its own to call off for something made here: the round's runs do that.)
        self.sync_run = None;
        self.session.sync_progress.phase(account, mailcraft_model::sync::SyncPhase::Inbox);
        self.graph_rx = Some((account, quiet, false, job));
        self.session.sync_status = tl!("Sending and receiving…").to_string();
        Some(json!({"started": true, "account": account, "graph": true, "inbox": true}))
    }

    /// The answers of checks that gave way, as they come: their sign-in is kept, nothing else.
    fn take_in_given_way(&mut self) {
        let mut waiting = Vec::new();
        for (account, job) in std::mem::take(&mut self.inbox_check.gave_way) {
            match job.try_recv() {
                Ok(result) => self.keep_inbox_tokens(account, &result),
                Err(TryRecvError::Empty) => waiting.push((account, job)),
                Err(TryRecvError::Disconnected) => {}
            }
        }
        self.inbox_check.gave_way = waiting;
    }

    /// What an Inbox pass's answer says of its sign-in, kept as for any job: renewed tokens (the
    /// refresh token for the owner, in the system's store when remembered), or a token refused.
    fn keep_inbox_tokens(&mut self, account: Id, result: &GraphResult) {
        let owner = self.session.mailbox.owner_of(account).map(|o| (o.id, o.email.clone()));
        if result.token_refused {
            self.graph_tokens.remove(&account);
        }
        if let Some(tokens) = result.tokens.clone()
            && (owner.is_some() || !self.signed_in_as_other(account, &tokens))
        {
            let (owner, address) =
                owner.unwrap_or_else(|| (account, self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default()));
            if !tokens.refresh.is_empty() {
                self.remember("refresh", &address, &tokens.refresh);
                if let Some(kept) = self.tokens.get_mut(&owner) {
                    kept.refresh = tokens.refresh.clone();
                }
            }
            let now = mailcraft_time::now();
            self.graph_tokens.insert(account, Tokens { expires_at: tokens.expires_at.clamp(now, now.saturating_add(86_400)), ..tokens });
        }
    }

    /// The Inbox pass's job came back (its pieces are in already): keep the tokens it got and
    /// let the account wait for the rest of its run. Whatever went wrong is the run's to meet
    /// again, and to ask about.
    pub(crate) fn finish_inbox_pass(&mut self, account: Id, result: GraphResult) {
        let check = self.inbox_check.running.take() == Some(account);
        self.inbox_check.cancel = None;
        self.keep_inbox_tokens(account, &result);
        let problems: Vec<String> = std::mem::take(&mut self.graph_problems).into_iter().chain(result.errors).collect();
        if check && problems.is_empty() {
            self.inbox_check.failing.remove(&account);
        }
        // A check that keeps failing (the network is down) says so once, until one gets through.
        let said_already = check && !problems.is_empty() && !self.inbox_check.failing.insert(account);
        if !said_already {
            let what = if check { "inbox check" } else { "inbox pass" };
            for problem in &problems {
                log::warn!("send/receive, account {account}: {what}: {problem}");
            }
        }
        // The check is quiet: it leaves the round's Progress and the status alone, and the next
        // tick tries again whatever went wrong.
        if check {
            return;
        }
        // Stop ends the round: nothing is left in line.
        if self.services.sync_cancel.load(Ordering::Relaxed) && self.sync_queue.is_empty() {
            let now = self.session.now();
            self.session.sync_progress.count_full_if_due(&self.session.mailbox, account, now);
            self.session.sync_progress.end(account, true);
            self.session.sync_status.clear();
        } else {
            self.session.sync_progress.rest(account);
        }
        if self.session.sync_status == tl!("Sending and receiving…") {
            self.session.sync_status.clear();
        }
    }
}

#[cfg(test)]
#[path = "inbox_pass_tests.rs"]
mod tests;
