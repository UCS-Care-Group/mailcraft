// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account kept in step over JMAP (M6), in the window: its Send/Receive, and the secret it goes
//! with. Its messages are fetched whole as a Microsoft account's are ([`crate::graph_fetch`]).
//!
//! The engine says what a run takes up, sends and reads (`sync.jmapRequest`) and takes in what it
//! brings ([`apply_jmap_mail`]); the host does the talking in between ([`crate::Services::jmap`])
//! and hands the run back in pieces: what became of the changes and of the mail sent, the listing,
//! then a window of a mailbox at a time. Each piece is taken in as it comes, one that says only
//! what became of what went up as any other. A run
//! goes in the place of an IMAP one ([`MailRun`]), so that it waits its turn in a round, Cancel
//! stops it, Skip and Send/Receive Progress know it, and a round that starts by itself every few
//! minutes takes it along.
//!
//! **The secret** is the password (or the bearer token, when the account's server says so) that
//! the owner types when the first run asks for it. It is kept in memory until the app closes, never
//! by the system's credential store, a file or the browser's storage, and it goes to the server
//! the account names now: a change of that address forgets it ([`MailApp::secret_key`]). A host
//! that holds an account's credential itself ([`crate::Services::jmap_host_credentials`]) is
//! asked with none. A typed secret the server turns down is forgotten, so that the next
//! Send/Receive asks again rather than send it once more; and after any refusal, the host's
//! credential too, a round by itself leaves the account out until the owner asks for its
//! Send/Receive.
//!
//! **Work started for one mailbox stays with it**: a run under way when another mailbox takes its
//! place is called off, and nothing it still brings is taken in.
//!
//! **Going up**: what was changed here (a mark, a move between the server's folders, a deletion
//! for good) or queued in the Outbox goes with the next run, which takes it up before it reads,
//! and that run is due within seconds, as a Microsoft account's is ([`MailApp::waiting_to_go`]);
//! something made while a run is under way calls it off to go up first. What a run sends is marked
//! as being sent, on disk, before it leaves ([`MailApp::hand_jmap`]): a message whose answer never
//! came is held at the next run, never sent again by itself.
//!
//! **Send asks for the secret**: a run by itself never asks anybody anything, so mail put in the
//! Outbox of an account whose secret isn't at hand would wait there unseen. When the owner presses
//! Send in such an account, the secret is asked for once ([`MailApp::jmap_send_pressed`]); a no
//! leaves the message in the Outbox for the next Send/Receive the owner asks for.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;

use mailcraft_engine::cmd::jmap_mail::apply_jmap_mail;
use mailcraft_model::Id;
use mailcraft_model::sync::{JmapFetchRequest, JmapMailRequest, SyncPhase};
use serde_json::{Value, json};

use crate::{MailApp, MailRun, dialogs};

/// What the host is asked to run ([`crate::Services::jmap`]).
pub struct JmapJob {
    pub request: JmapMailRequest,
    /// The password, or the bearer token when [`JmapMailRequest::bearer`] says so; empty when the
    /// host holds the credential itself.
    pub secret: String,
    /// Set when the owner stops the run (Cancel): it stops before its next request.
    pub cancel: Arc<AtomicBool>,
}

/// What the host is asked to fetch ([`crate::Services::jmap_fetch`]).
pub struct JmapFetchJob {
    pub request: JmapFetchRequest,
    /// As for a run ([`JmapJob::secret`]).
    pub secret: String,
    pub cancel: Arc<AtomicBool>,
}

impl MailApp {
    /// The secret a job of the JMAP account `account` goes with, when one is at hand: the one
    /// typed this session, or none (empty) when the host holds the credential.
    pub(crate) fn jmap_secret(&self, account: Id) -> Option<String> {
        let held = self.services.jmap_host_credentials.as_ref().zip(self.session.mailbox.account(account)).is_some_and(|(holds, a)| holds(a));
        if held {
            return Some(String::new());
        }
        self.passwords.get(&account).cloned()
    }

    /// A Send/Receive of the JMAP account `account`; `p` may carry the password the owner has
    /// just typed. Asks for it the first time, unless the host holds the credential.
    pub(crate) fn start_jmap_mail(&mut self, account: Id, quiet: bool, p: &Value) -> Result<Value, String> {
        if let Some(typed) = p.get("password").and_then(Value::as_str).filter(|typed| !typed.is_empty()) {
            self.passwords.insert(account, typed.to_string());
        }
        // The owner asked: a credential turned down before is tried again.
        if !quiet {
            self.jmap_refused.remove(&account);
        }
        let Some(secret) = self.jmap_secret(account) else {
            self.dialog = dialogs::Dialog::open("password", self, &json!({"account": account}));
            return Ok(json!({"pending": "password"}));
        };
        // (Through the command: a run begins afresh, whatever one that never ended had read.)
        let request = self.session.run("sync.jmapRequest", &json!({"account": account, "background": quiet})).map_err(|e| e.to_string())?;
        let request: JmapMailRequest = serde_json::from_value(request).map_err(|e| e.to_string())?;
        if self.services.jmap.is_none() {
            return Err(tl!("This build can't connect to mail servers").to_string());
        }
        self.services.sync_cancel.store(false, Ordering::Relaxed);
        // What it sends is marked so, on disk, before it leaves (AU3-26).
        let pieces = self.hand_jmap(account, JmapJob { request, secret, cancel: self.services.sync_cancel.clone() })?;
        self.sync_refused = false;
        // (Something made here while the run is under way calls it off to go up first, as a Microsoft account's does.)
        self.sync_run = Some((account, Some(self.waiting_to_go(account)), false, quiet));
        self.jmap_made = 0;
        self.sync_rx = Some(MailRun::Jmap { account, opening: self.session.openings(), pieces });
        // (Send/Receive Progress: under way from now, the server's mailboxes first.)
        self.session.sync_progress.phase(account, SyncPhase::Folders);
        self.session.sync_status = tl!("Sending and receiving…").to_string();
        Ok(json!({"started": true, "account": account, "jmap": true}))
    }

    /// The owner pressed Send in the window and the message `sent` waits in an Outbox: when that
    /// is a JMAP account's, and its secret isn't at hand, it is asked for as soon as nothing else
    /// is under way ([`MailApp::ask_for_jmap_send`]). An account whose secret is at hand needs
    /// nothing: its run is due anyway.
    pub(crate) fn jmap_send_pressed(&mut self, sent: Id) {
        let mb = &self.session.mailbox;
        let Some(account) = mb.message(sent).map(|m| m.account) else { return };
        let jmap = mb.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Jmap);
        if jmap && self.services.jmap.is_some() && !self.can_send_jmap_quietly(account) {
            self.jmap_send_asks.insert(account);
        }
    }

    /// Whether a run of the JMAP account `account` can go by itself: its secret is at hand, and
    /// its server didn't turn it down.
    fn can_send_jmap_quietly(&self, account: Id) -> bool {
        !self.jmap_refused.contains(&account) && self.jmap_secret(account).is_some()
    }

    /// Nothing is under way and no question is open: a JMAP account the owner pressed Send in,
    /// whose mail still waits and whose secret still isn't at hand, has its Send/Receive started
    /// as the owner's own, which asks for the secret. Once per Send: an owner who closes the
    /// question isn't asked again until the next one. Whether a Send/Receive was started.
    pub(crate) fn ask_for_jmap_send(&mut self) -> bool {
        let Some(account) = self.jmap_send_asks.iter().next().copied() else { return false };
        self.jmap_send_asks.remove(&account);
        let waits = mailcraft_engine::cmd::graph_send::waiting(&self.session.mailbox, account) > 0;
        waits && !self.can_send_jmap_quietly(account) && self.start_sync(&json!({"account": account})).is_ok()
    }

    /// Take in what the JMAP run under way has brought so far, a few pieces a frame (each one
    /// redraws the lists, and the window should stay quick); the last one ends the run. Called by
    /// [`MailApp::poll_sync`].
    pub(crate) fn poll_jmap(&mut self) {
        for _ in 0..4 {
            let Some(MailRun::Jmap { account, opening, pieces }) = &self.sync_rx else { return };
            let (account, opening) = (*account, *opening);
            let piece = match pieces.try_recv() {
                Ok(piece) => piece,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    log::warn!("jmap mail: the run stopped without its last piece (its task ended early)");
                    (self.sync_rx, self.sync_refused) = (None, false);
                    self.session.sync_status = tl!("Send/Receive stopped unexpectedly").to_string();
                    return;
                }
            };
            let (last, mut whole) = (!piece.partial, piece.whole);
            // Another mailbox took the place of the one the run was for: its accounts are other
            // accounts, whatever their ids and addresses, and what the run brings is nobody's. The
            // run is called off, and what comes of it until its end is thrown away.
            if opening != self.session.openings() {
                self.services.sync_cancel.store(true, Ordering::Relaxed);
                if last {
                    (self.sync_rx, self.sync_run, self.sync_refused) = (None, None, false);
                    self.session.sync_status.clear();
                    return;
                }
                continue;
            }
            self.sync_refused |= piece.auth_refused;
            // New mail may be waiting to be fetched whole: the background looks again.
            if !piece.pages.is_empty() {
                self.session.driver.fetch.wake();
            }
            match apply_jmap_mail(&mut self.session, piece) {
                Ok(said) => {
                    let errors = said["errors"].as_array().map(|all| all.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>());
                    self.note_sync_problems(account, &errors.unwrap_or_default(), false);
                    // What the server wouldn't do in a run that went well otherwise (a change it
                    // refused, a message it wouldn't send) stays to be read in its problems.
                    whole &= said["refused"].as_u64().unwrap_or(0) == 0;
                    // What a run made: changes taken up, and messages sent (a run sends only so many,
                    // and what is left of a full Outbox goes with the next at once).
                    let made = said["pushed"].as_u64().unwrap_or(0).saturating_add(said["sent"].as_u64().unwrap_or(0));
                    self.jmap_made = self.jmap_made.saturating_add(usize::try_from(made).unwrap_or(usize::MAX));
                }
                Err(e) => {
                    self.status(e.to_string());
                    self.session.driver.sending.doubted();
                }
            }
            if !last {
                continue;
            }
            // The run is over.
            self.logged_problems.end_run(account, "mail");
            self.sync_rx = None;
            // What it sent is filed, and what it didn't is known not to have gone (AU3-26).
            self.session.send_run_answered(account);
            // Called off for something else (a shared mailbox's first read): it goes on after.
            if let Some((whose, _, true, quiet)) = self.sync_run.take() {
                self.session.driver.round.queue.push_front((whose, quiet));
            }
            if std::mem::take(&mut self.sync_refused) {
                // Turned down: the next Send/Receive asks for it again (the host for its own), and
                // no round by itself goes back until then.
                self.passwords.remove(&account);
                self.jmap_refused.insert(account);
            } else if whole {
                // And nothing is left of the last run's problems.
                self.note_sync_problems(account, &[], true);
            }
            self.note_pushed(account);
            // What still waits after a run whose changes were all made never went with it (a
            // mark made while it ran on a message whose other mark was going, or what came after
            // as many as a run takes): it isn't something the server turned down, so it goes
            // with a run of its own at once, not in a few minutes. A run that made nothing
            // doesn't ask for another, so this ends.
            if whole && std::mem::take(&mut self.jmap_made) > 0 {
                self.push_again_soon(account);
            }
            return;
        }
    }
}

#[cfg(test)]
#[path = "jmap_mail_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "jmap_going_up_tests.rs"]
mod going_up_tests;
