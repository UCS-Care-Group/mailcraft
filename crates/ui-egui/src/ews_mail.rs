// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Exchange server account kept in step through Exchange Web Services (#252, E6), in the
//! window: its Send/Receive, and the password it goes with. Its messages are fetched whole as a
//! Microsoft account's are ([`crate::graph_fetch`]).
//!
//! The engine says what a run reads (`sync.ewsRequest`) and takes in what it brings
//! ([`apply_ews_mail`]); the host does the talking in between ([`crate::Services::ews`]) and hands
//! the run back in pieces, the folder list first, then a page of a folder at a time. A run goes in
//! the place of an IMAP one ([`MailRun`]), so that it waits its turn in a round, Cancel stops it,
//! Skip and Send/Receive Progress know it, and a round that starts by itself every few minutes
//! takes it along.
//!
//! **The password** is what the owner types when the first run asks for it. It is kept in memory
//! until the app closes (`MailApp::passwords`), never in the mailbox file, a log or a command's
//! params, and goes to the server the account names now: a change of that address forgets it
//! ([`MailApp::secret_key`]). Remembering it with the operating system comes with the Add Account
//! page. A password the server turns down (a 401) is forgotten, so that the next Send/Receive asks
//! again rather than send it once more; a round by itself leaves the account out until the owner
//! asks for its Send/Receive.
//!
//! **A certificate** the system doesn't trust, or one that is not the one trusted for the account,
//! stops the run at its first request, before the password goes: the account says it needs the
//! certificate question (`EwsMailResult::untrusted`) and nothing is repeated by itself.
//!
//! **Work started for one mailbox stays with it**: a run under way when another mailbox takes its
//! place is called off, and nothing it still brings is taken in.
//!
//! **Going up**: what was changed here, saved as a draft or queued in the Outbox goes with the next run, which
//! takes it up before it reads; something made while a run is under way calls it off to go up first.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;

use mailcraft_engine::cmd::ews_mail::apply_ews_mail;
use mailcraft_model::Id;
use mailcraft_model::sync::{EwsFetchRequest, EwsMailRequest, SyncPhase};
use serde_json::{Value, json};

use crate::{MailApp, MailRun, dialogs};

/// What the host is asked to run ([`crate::Services::ews`]).
pub struct EwsJob {
    pub request: EwsMailRequest,
    /// The account's password, with the request's user name (or the account's address) the
    /// credential of a Basic sign-in over TLS. Never logged.
    pub secret: String,
    /// Set when the owner stops the run (Cancel): it stops before its next request.
    pub cancel: Arc<AtomicBool>,
}

/// What the host is asked to fetch ([`crate::Services::ews_fetch`]).
pub struct EwsFetchJob {
    pub request: EwsFetchRequest,
    /// As for a run ([`EwsJob::secret`]).
    pub secret: String,
    pub cancel: Arc<AtomicBool>,
}

impl MailApp {
    /// The password a job of the Exchange server account `account` goes with, when one is at hand:
    /// the one typed this session, or else the one the operating system was asked to remember (what
    /// the background read of the store brought, never the store itself, #613).
    pub(crate) fn ews_secret(&self, account: Id) -> Option<String> {
        if let Some(typed) = self.passwords.get(&account) {
            return Some(typed.clone());
        }
        let address = self.session.mailbox.account(account)?.email.clone();
        self.remembered("password", &address)
    }

    /// A Send/Receive of the Exchange server account `account`; `p` may carry the password the owner
    /// has just typed, and `inboxPass` asks for the Inbox alone. Asks for the password the first time.
    pub(crate) fn start_ews_mail(&mut self, account: Id, quiet: bool, p: &Value) -> Result<Value, String> {
        if let Some(typed) = p.get("password").and_then(Value::as_str).filter(|typed| !typed.is_empty()) {
            self.passwords.insert(account, typed.to_string());
        }
        // The owner asked: a password turned down before is tried again.
        if !quiet {
            self.jmap_refused.remove(&account);
        }
        let Some(secret) = self.ews_secret(account) else {
            self.dialog = dialogs::Dialog::open("password", self, &json!({"account": account}));
            return Ok(json!({"pending": "password"}));
        };
        let Some(run) = &self.services.ews else { return Err(tl!("This build can't connect to mail servers").to_string()) };
        let inbox_pass = p.get("inboxPass").and_then(Value::as_bool).unwrap_or(false);
        let params = json!({"account": account, "background": quiet, "inboxPass": inbox_pass});
        let request = self.session.run("sync.ewsRequest", &params).map_err(|e| e.to_string())?;
        let request: EwsMailRequest = serde_json::from_value(request).map_err(|e| e.to_string())?;
        self.services.sync_cancel.store(false, Ordering::Relaxed);
        let pieces = run(EwsJob { request, secret, cancel: self.services.sync_cancel.clone() });
        self.sync_refused = false;
        // (Something made here while the run is under way calls it off to go up first, as a Microsoft account's does.)
        self.sync_run = Some((account, Some(self.waiting_to_go(account)), false, quiet));
        self.sync_rx = Some(MailRun::Ews { account, opening: self.session.openings(), pieces });
        // (Send/Receive Progress: under way from now, the server's folders first.)
        self.session.sync_progress.phase(account, if inbox_pass { SyncPhase::Inbox } else { SyncPhase::Folders });
        self.session.sync_status = tl!("Sending and receiving…").to_string();
        Ok(json!({"started": true, "account": account, "ews": true}))
    }

    /// Take in what the Exchange run under way has brought so far, a few pieces a frame (each one
    /// redraws the lists, and the window should stay quick); the last one ends the run. Called by
    /// [`MailApp::poll_sync`].
    pub(crate) fn poll_ews(&mut self) {
        for _ in 0..4 {
            let Some(MailRun::Ews { account, opening, pieces }) = &self.sync_rx else { return };
            let (account, opening) = (*account, *opening);
            let piece = match pieces.try_recv() {
                Ok(piece) => piece,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    log::warn!("ews mail: the run stopped without its last piece (its thread ended early)");
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
            let untrusted = !piece.untrusted.is_empty();
            // What File › Info says: how this run signed in, and a certificate that needs a say.
            if !piece.signed_in_with.is_empty() {
                self.ews_signed_in.insert(account, piece.signed_in_with.clone());
            }
            if untrusted {
                self.ews_certificate.insert(account, piece.untrusted.clone());
            }
            match apply_ews_mail(&mut self.session, piece) {
                Ok(said) => {
                    // The folders the run could not read: File › Info names them until the next run.
                    if last {
                        let folders: Vec<String> = said["unreadableFolders"]
                            .as_array()
                            .map(|all| all.iter().filter_map(Value::as_str).map(str::to_string).collect())
                            .unwrap_or_default();
                        if folders.is_empty() {
                            self.ews_unreadable.remove(&account);
                        } else {
                            self.ews_unreadable.insert(account, folders);
                        }
                    }
                    let errors = said["errors"].as_array().map(|all| all.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>());
                    self.note_sync_problems(account, &errors.unwrap_or_default(), false);
                }
                Err(e) => {
                    self.status(e.to_string());
                    // A piece that was refused is not skipped over: the run is called off, and the
                    // pages after it (read from the state it would have kept) are left out by the
                    // engine, whichever of them still come ([`apply_ews_mail`]).
                    if !last {
                        self.services.sync_cancel.store(true, Ordering::Relaxed);
                    }
                }
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
                // Turned down: the next Send/Receive asks for it again, and no round by itself goes
                // back until then. What the system remembered is no good either.
                self.passwords.remove(&account);
                self.jmap_refused.insert(account);
                if let (Some(store), Some(a)) = (&self.services.secrets, self.session.mailbox.account(account)) {
                    let key = Self::secret_key("password", a);
                    store.delete(&key);
                    self.secret_cache.remove(&key);
                }
            } else if untrusted {
                // The certificate question comes first (File › Info): no round by itself tries again.
                self.jmap_refused.insert(account);
            } else if whole {
                // It went through: that password is worth remembering (when the owner lets it be),
                // and nothing is left of the last run's problems.
                if let Some((address, password)) =
                    self.session.mailbox.account(account).map(|a| a.email.clone()).zip(self.passwords.get(&account).cloned())
                {
                    self.remember("password", &address, &password);
                }
                self.ews_certificate.remove(&account);
                self.note_sync_problems(account, &[], true);
            }
            self.note_pushed(account);
            return;
        }
    }
}
