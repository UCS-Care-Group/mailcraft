// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Several Exchange server accounts sent and received together, and what was done to one of them
//! sent up at once, not after the others are read.
//!
//! A Send/Receive used to run one account at a time: the second account's mail waited for the
//! first's, and a message deleted or sent meanwhile waited for the whole read. Two kinds of run
//! now go beside the one that is under way ([`MailApp::sync_rx`]):
//!
//! - **A whole run** of the next account in the round, up to [`MOST_FULL`] beside the first
//!   (so three at most together: a server that throttles one mailbox's requests is not asked by
//!   more than that, and the accounts are different mailboxes, each with its own share).
//! - **A run that only goes up** ([`EwsMailRequest::push_only`]), one at a time, for an account
//!   with something waiting to go (a message deleted, moved, marked or sent) that is not being read
//!   by another run. It reads nothing.
//!
//! One account is never in two runs: an account whose own run is under way and that has more to
//! send is called off to go up first, as before ([`MailApp::auto_sync`]), so that two runs never
//! read and change the same mailbox together. Cancel and Stop reach all of them; a mailbox that
//! takes the place of the open one calls them off and what they still bring is thrown away.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_engine::cmd::ews_mail::apply_ews_mail;
use mailcraft_engine::sync_driver::Waiting;
use mailcraft_model::sync::{EwsMailRequest, EwsMailResult, SyncPhase};
use mailcraft_model::{AccountKind, Id};
use serde_json::json;

use crate::ews_mail::EwsJob;
use crate::{MailApp, MailRun, now_ms};

/// How often the accounts are looked at for something to take up beside a busy run (milliseconds).
const LOOK_MS: f64 = 1_500.0;
/// How often a run beside the first is looked at for something the owner did meanwhile (milliseconds).
const RUN_LOOK_MS: f64 = 3_000.0;
/// Most whole runs beside the first.
const MOST_FULL: usize = 2;

/// A run of an Exchange server account beside the Send/Receive that is under way.
pub(crate) struct ExtraRun {
    pub(crate) account: Id,
    /// The mailbox opening it was started under ([`mailcraft_engine::Session::openings`]).
    opening: u64,
    pub(crate) pieces: Receiver<EwsMailResult>,
    cancel: Arc<AtomicBool>,
    /// It only takes up what was done here, and reads nothing.
    pub(crate) push_only: bool,
    /// Nobody asked for it: it is part of a round that started by itself.
    quiet: bool,
    /// How much was waiting to go when it began: more than that is the owner's doing meanwhile.
    baseline: usize,
    /// It was called off to let what the owner did go up first: it goes on after.
    called_off: bool,
    /// A piece said the server turned the password down, or a certificate asks the owner.
    refused: bool,
    untrusted: bool,
}

impl ExtraRun {
    fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

impl MailApp {
    /// Whether a whole run beside the first is under way: a Send/Receive is running.
    pub(crate) fn ews_extra_syncing(&self) -> bool {
        self.ews_extra.iter().any(|run| !run.push_only)
    }

    /// Whether a run of `account` is under way beside the first.
    pub(crate) fn ews_extra_for(&self, account: Id) -> bool {
        self.ews_extra.iter().any(|run| run.account == account)
    }

    /// Call off every run beside the first (Stop, a mailbox that takes the place of the open one).
    pub(crate) fn call_off_ews_extras(&mut self) {
        for run in self.ews_extra.drain(..) {
            run.cancel();
        }
    }

    /// Stop every run beside the first at its next safe place: what it brought stays.
    pub(crate) fn stop_ews_extras(&mut self) {
        for run in &mut self.ews_extra {
            run.cancel();
            run.called_off = false;
        }
    }

    /// Whether more than the runs now going may start: nothing stops it (offline, a question to
    /// answer, an unconfirmed mailbox) and the host runs Exchange.
    fn ews_extra_can_start(&self) -> bool {
        self.services.can_sync()
            && self.services.ews.is_some()
            && self.dialog.is_none()
            && !self.offline()
            && !self.unconfirmed
            && !self.data_removed
            && self.signing_in.is_none()
            && !self.approving()
            && self.secret_read.is_none()
    }

    /// Beside a run that is under way, start the next account of the round (a whole run, up to
    /// [`MOST_FULL`] beside the first), and a run that only goes up for an account with something
    /// waiting to go. Called every frame by [`MailApp::auto_sync`].
    pub(crate) fn ews_extra_runs(&mut self) {
        let busy = self.sync_rx.is_some() || self.graph_rx.is_some() || self.ews_extra_syncing();
        if !busy || !self.ews_extra_can_start() {
            return;
        }
        self.call_off_extra_for_changes();
        // The next account of the round, a whole run beside the others (when what is under way is
        // an Exchange account's run: the rest of the round waits its turn as it did).
        let exchange_run = matches!(self.sync_rx, Some(MailRun::Ews { .. })) || self.ews_extra_syncing();
        let full = self.ews_extra.iter().filter(|run| !run.push_only).count();
        if exchange_run
            && full < MOST_FULL
            && self.session.driver.round.inbox_queue.is_empty()
            && let Some(&(account, quiet)) = self.session.driver.round.queue.front()
            && self.session.mailbox.account(account).is_some_and(|a| a.kind == AccountKind::Exchange)
            && !self.calendar_job_for(account)
            && !self.ews_extra_for(account)
            && self.sync_run.is_none_or(|(whose, ..)| whose != account)
            && self.ews_secret(account).is_some()
            && (!quiet || self.can_sync_quietly(account))
        {
            self.session.driver.round.queue.pop_front();
            if !self.start_ews_extra(account, quiet) {
                self.session.sync_progress.leave_out(account);
            }
            return;
        }
        self.ews_push_lane();
    }

    /// A run beside the first whose account has more waiting to go than when it began is called
    /// off, which loses nothing: it goes up first, and the account's run is made again after.
    fn call_off_extra_for_changes(&mut self) {
        let now = now_ms();
        if !self.session.driver.auto_sync() || now - self.ews_extra_checked < RUN_LOOK_MS {
            return;
        }
        self.ews_extra_checked = now;
        let reading: Vec<(usize, Id)> =
            self.ews_extra.iter().enumerate().filter(|(_, run)| !run.push_only && !run.called_off).map(|(at, run)| (at, run.account)).collect();
        for (at, account) in reading {
            if self.waiting_to_go(account) > self.ews_extra[at].baseline {
                self.ews_extra[at].cancel();
                self.ews_extra[at].called_off = true;
            }
        }
    }

    /// Start a whole run of `account` beside the others; false when it can't go now.
    fn start_ews_extra(&mut self, account: Id, quiet: bool) -> bool {
        let Some(secret) = self.ews_secret(account) else { return false };
        if let Some(first) = self.session.send_hold_unanswered(account) {
            self.status(first);
        }
        let params = json!({"account": account, "background": quiet, "inboxPass": false});
        let Ok(request) = self.session.run("sync.ewsRequest", &params) else { return false };
        let Ok(mut request) = serde_json::from_value::<EwsMailRequest>(request) else { return false };
        if !request.sends.is_empty() && (self.send_gate.is_some() || self.session.driver.sending.gate_waits()) {
            return false;
        }
        if self.unconfirmed {
            request.trusted.clear();
        }
        let baseline = self.waiting_to_go(account);
        if !self.hand_extra(account, request, secret, false, quiet, baseline) {
            return false;
        }
        log::info!("ews run: this account's Send/Receive goes beside the one under way");
        // (Send/Receive Progress: under way from now, the server's folders first.)
        self.session.sync_progress.phase(account, SyncPhase::Folders);
        true
    }

    /// Hand the run to the host and keep it as one beside the first.
    fn hand_extra(&mut self, account: Id, request: EwsMailRequest, secret: String, push_only: bool, quiet: bool, baseline: usize) -> bool {
        let cancel = Arc::new(AtomicBool::new(false));
        let job = EwsJob { request, secret, cancel: cancel.clone() };
        let Ok(pieces) = self.hand_ews(account, job) else { return false };
        self.ews_extra.push(ExtraRun {
            account,
            opening: self.session.openings(),
            pieces,
            cancel,
            push_only,
            quiet,
            baseline,
            called_off: false,
            refused: false,
            untrusted: false,
        });
        true
    }

    /// Start a run that only goes up for an Exchange server account (not one being read) that has
    /// something waiting to go. Looks only every [`LOOK_MS`].
    fn ews_push_lane(&mut self) {
        if self.ews_extra.iter().any(|run| run.push_only) {
            return;
        }
        let now = now_ms();
        if !self.session.driver.auto_sync() || now - self.ews_push_checked < LOOK_MS {
            return;
        }
        self.ews_push_checked = now;
        let reading: Vec<Id> = self.sync_run.iter().map(|(account, ..)| *account).chain(self.ews_extra.iter().map(|run| run.account)).collect();
        let waiting: Vec<Waiting> = self
            .server_accounts()
            .into_iter()
            .filter(|account| !reading.contains(account))
            .filter(|account| self.session.mailbox.account(*account).is_some_and(|a| a.kind == AccountKind::Exchange))
            // (One whose password the server turned down, or whose certificate asks, is not tried by itself.)
            .filter(|account| self.can_sync_quietly(*account))
            .map(|account| Waiting { account, count: self.waiting_to_go(account) })
            .collect();
        for account in self.session.driver.round.pushes_due(&waiting, now) {
            if self.start_ews_push(account) {
                return;
            }
        }
    }

    /// Start the run of `account` that only goes up; false when it can't go now (its password
    /// isn't at hand, its folders aren't the server's yet, the mailbox couldn't be saved for a send).
    fn start_ews_push(&mut self, account: Id) -> bool {
        let Some(secret) = self.ews_secret(account) else { return false };
        // A message an earlier run handed to the server, whose answer never came back, is held for
        // the owner before any request is made (AU3-26).
        if let Some(first) = self.session.send_hold_unanswered(account) {
            self.status(first);
        }
        let params = json!({"account": account, "pushOnly": true});
        let Ok(request) = self.session.run("sync.ewsRequest", &params) else { return false };
        let Ok(mut request) = serde_json::from_value::<EwsMailRequest>(request) else { return false };
        // One mark of what is being sent at a time: a send waits while another run's marks are written.
        if !request.sends.is_empty() && (self.send_gate.is_some() || self.session.driver.sending.gate_waits()) {
            return false;
        }
        // A certificate a file trusts counts once the owner has confirmed the file (AU3-02).
        if self.unconfirmed {
            request.trusted.clear();
        }
        if !self.hand_extra(account, request, secret, true, true, 0) {
            return false;
        }
        log::info!("ews push: a run of its own for what was done here, beside the Send/Receive");
        true
    }

    /// Take in what the runs beside the first have brought, a few pieces a frame. Called by
    /// [`MailApp::poll_sync`].
    pub(crate) fn poll_ews_extras(&mut self) {
        let mut at = 0;
        while at < self.ews_extra.len() {
            if self.poll_extra(at) {
                self.ews_extra.remove(at);
            } else {
                at += 1;
            }
        }
    }

    /// Take in the pieces of the run at `at`, a few at a time; true when it is over.
    fn poll_extra(&mut self, at: usize) -> bool {
        for _ in 0..4 {
            let piece = match self.ews_extra[at].pieces.try_recv() {
                Ok(piece) => piece,
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
                    self.lost_extra(at);
                    return true;
                }
            };
            let last = !piece.partial;
            if self.ews_extra[at].opening != self.session.openings() {
                // Another mailbox took the place of the one the run was for: what it still brings is nobody's.
                self.ews_extra[at].cancel();
            } else {
                self.take_in_extra(at, piece);
            }
            if last {
                return true;
            }
        }
        false
    }

    /// The run at `at` ended without its last piece (its thread ended early): nothing more comes,
    /// and what waits to go is looked at again later, not at once.
    fn lost_extra(&mut self, at: usize) {
        let (account, push_only) = (self.ews_extra[at].account, self.ews_extra[at].push_only);
        log::warn!("ews run: a run beside the first stopped without its last piece (its thread ended early)");
        self.session.end_run_unfinished(account);
        if !push_only {
            self.logged_problems.end_run(account, "mail");
        }
        self.note_pushed(account);
    }

    /// Take in a piece of the run at `at`; the last one ends it.
    fn take_in_extra(&mut self, at: usize, piece: EwsMailResult) {
        let (account, push_only) = (self.ews_extra[at].account, self.ews_extra[at].push_only);
        let (last, whole) = (!piece.partial, piece.whole);
        self.ews_extra[at].refused |= piece.auth_refused;
        if !push_only {
            self.note_read_piece(at, &piece);
        }
        match apply_ews_mail(&mut self.session, piece) {
            Ok(said) => self.note_ews_answer(account, &said, last && !push_only),
            Err(e) => {
                // A piece that was refused is not skipped over: the run is called off.
                self.status(e.to_string());
                self.session.driver.sending.doubted();
                if !last {
                    self.ews_extra[at].cancel();
                }
            }
        }
        if last {
            self.finish_extra(at, whole);
        }
    }

    /// What a piece of a run that reads says besides its mail: more to fetch whole, how the run
    /// signed in, a certificate that asks the owner.
    fn note_read_piece(&mut self, at: usize, piece: &EwsMailResult) {
        let account = self.ews_extra[at].account;
        // New mail may be waiting to be fetched whole: the background looks again.
        if !piece.pages.is_empty() {
            self.session.driver.fetch.wake();
        }
        if !piece.signed_in_with.is_empty() {
            self.ews_signed_in.insert(account, piece.signed_in_with.clone());
        }
        if !piece.untrusted.is_empty() {
            self.ews_certificate.insert(account, piece.untrusted.clone());
            self.ews_extra[at].untrusted = true;
        }
    }

    /// The end of the run at `at`, as the first run's is ([`MailApp::poll_ews`]).
    fn finish_extra(&mut self, at: usize, whole: bool) {
        let (account, push_only, quiet, called_off, refused, untrusted) = {
            let run = &self.ews_extra[at];
            (run.account, run.push_only, run.quiet, run.called_off, run.refused, run.untrusted)
        };
        if !push_only {
            self.logged_problems.end_run(account, "mail");
        }
        // What it sent is filed, and what it didn't is known not to have gone (AU3-26).
        self.session.send_run_answered(account);
        // Called off to let what was done meanwhile go up first: it goes on after.
        if called_off {
            self.session.driver.round.queue.push_front((account, quiet));
        }
        if refused {
            self.ews_password_refused(account);
        } else if untrusted {
            // The certificate question comes first (File › Info): no round by itself tries again.
            self.jmap_refused.insert(account);
        } else if whole && !push_only {
            self.ews_certificate.remove(&account);
            self.note_sync_problems(account, &[], true);
        }
        self.note_pushed(account);
    }
}
