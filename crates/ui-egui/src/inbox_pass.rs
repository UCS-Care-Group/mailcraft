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

use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::{GraphRequest, GraphResult, MailApp, Tokens};

impl MailApp {
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
            if let Some(started) = self.start_inbox_pass(account, quiet) {
                return Some(started);
            }
        }
        None
    }

    /// The Inbox pass of `account`, as a Graph job of its own.
    fn start_inbox_pass(&mut self, account: Id, quiet: bool) -> Option<Value> {
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
        let (pieces, taken) = std::sync::mpsc::sync_channel(8);
        let ask = GraphRequest {
            account: mailbox,
            oauth: self.effective_oauth(oauth),
            refresh,
            // (Renewed for the job, as the run's own is.)
            access: String::new(),
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
            cancel: self.services.sync_cancel.clone(),
        };
        let graph = self.services.graph.as_ref()?;
        self.services.sync_cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        self.graph_pieces = Some(taken);
        self.graph_problems.clear();
        self.graph_pieces_taken = 0;
        // (No run of its own to call off for something made here: the round's runs do that.)
        self.sync_run = None;
        self.inbox_run = Some(account);
        self.session.sync_progress.phase(account, mailcraft_model::sync::SyncPhase::Inbox);
        self.graph_rx = Some((account, quiet, false, graph(ask)));
        self.session.sync_status = tl!("Sending and receiving…").to_string();
        Some(json!({"started": true, "account": account, "graph": true, "inbox": true}))
    }

    /// The Inbox pass's job came back (its pieces are in already): keep the tokens it got and
    /// let the account wait for the rest of its run. Whatever went wrong is the run's to meet
    /// again, and to ask about.
    pub(crate) fn finish_inbox_pass(&mut self, account: Id, result: GraphResult) {
        let owner = self.session.mailbox.owner_of(account).map(|o| (o.id, o.email.clone()));
        if result.token_refused {
            self.graph_tokens.remove(&account);
        }
        if let Some(tokens) = result.tokens
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
        for problem in std::mem::take(&mut self.graph_problems).iter().chain(&result.errors) {
            log::warn!("send/receive, account {account}: inbox pass: {problem}");
        }
        // Stop ends the round: nothing is left in line.
        if self.services.sync_cancel.load(std::sync::atomic::Ordering::Relaxed) && self.sync_queue.is_empty() {
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
