// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account kept in step over JMAP (M6), in the window: its Send/Receive, and the secret it goes
//! with. Its messages are fetched whole as a Microsoft account's are ([`crate::graph_fetch`]).
//!
//! The engine says what a run reads (`sync.jmapRequest`) and takes in what it brings
//! ([`apply_jmap_mail`]); the host does the talking in between ([`crate::Services::jmap`]) and
//! hands the run back in pieces, the listing first, then a window of a mailbox at a time. A run
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
//! **Reading only**: nothing made here goes up yet, so nothing made here calls a run off to go up
//! first, as it calls off an IMAP or a Graph run.

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
        let Some(run) = &self.services.jmap else { return Err(tl!("This build can't connect to mail servers").to_string()) };
        self.services.sync_cancel.store(false, Ordering::Relaxed);
        let pieces = run(JmapJob { request, secret, cancel: self.services.sync_cancel.clone() });
        self.sync_refused = false;
        self.sync_run = Some((account, None, false, quiet));
        self.sync_rx = Some(MailRun::Jmap { account, opening: self.session.openings(), pieces });
        // (Send/Receive Progress: under way from now, the server's mailboxes first.)
        self.session.sync_progress.phase(account, SyncPhase::Folders);
        self.session.sync_status = tl!("Sending and receiving…").to_string();
        Ok(json!({"started": true, "account": account, "jmap": true}))
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
            let (last, whole) = (!piece.partial, piece.whole);
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
                self.fetching.wake();
            }
            match apply_jmap_mail(&mut self.session, piece) {
                Ok(said) => {
                    let errors = said["errors"].as_array().map(|all| all.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>());
                    self.note_sync_problems(account, &errors.unwrap_or_default(), false);
                }
                Err(e) => self.status(e.to_string()),
            }
            if !last {
                continue;
            }
            // The run is over.
            self.sync_rx = None;
            // Called off for something else (a shared mailbox's first read): it goes on after.
            if let Some((whose, _, true, quiet)) = self.sync_run.take() {
                self.sync_queue.push_front((whose, quiet));
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
            return;
        }
    }
}

#[cfg(test)]
#[path = "jmap_mail_tests.rs"]
mod tests;
