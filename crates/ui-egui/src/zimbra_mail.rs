// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Zimbra account kept in step through a Zimbra server's web service, in the window: its
//! Send/Receive, the password it goes with, and the code of a second step. Its messages are
//! fetched whole as a Microsoft account's are ([`crate::graph_fetch`]).
//!
//! The engine says what a run reads (`sync.zimbraRequest`) and takes in what it brings
//! (`apply_zimbra_mail`); the host does the talking in between ([`crate::Services::zimbra`]) and
//! hands the run back in pieces. A run goes in the place of an IMAP one ([`MailRun`]), so that it
//! waits its turn in a round and Cancel stops it.
//!
//! **The password** is what the owner types when the first run asks for it, kept as an Exchange
//! server account's is: in memory, and with the operating system when the owner lets it be, once
//! the server has answered it. A password the server turns down is forgotten.
//!
//! **A second step.** An account that signs in with two factors asks, once, for the code of the
//! owner's authenticator app (a dialog of its own). That sign-in asks the server to trust this
//! computer, and what it gives for that (a trusted-device token and its device id) is kept as the
//! password is, with the operating system, under the account's own name: later runs go with it and
//! ask for no code, until the server stops taking it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};

use mailcraft_engine::cmd::zimbra_mail::apply_zimbra_mail;
use mailcraft_model::Id;
use mailcraft_model::sync::{SyncPhase, ZimbraFetchRequest, ZimbraMailRequest};
use serde_json::{Value, json};

use crate::{MailApp, MailRun, dialogs};

/// What a sign-in of a Zimbra account brought to be kept: a trusted-device token and its device
/// id. Filled by the host, read once by the window when the run ends. Never shown.
pub type TrustSlot = Arc<Mutex<Option<(String, String)>>>;

/// What the host is asked to run ([`crate::Services::zimbra`]).
pub struct ZimbraJob {
    pub request: ZimbraMailRequest,
    /// The account's password. Never logged.
    pub secret: String,
    /// The code of the owner's authenticator app, when they typed one for this run.
    pub code: Option<String>,
    /// The trusted-device token and device id an earlier sign-in brought, when one is kept.
    pub trusted: Option<(String, String)>,
    /// Where the host puts what this sign-in brought to keep.
    pub trust: TrustSlot,
    /// Set when the owner stops the run (Cancel): it stops before its next request.
    pub cancel: Arc<AtomicBool>,
}

/// What the host is asked to fetch ([`crate::Services::zimbra_fetch`]).
pub struct ZimbraFetchJob {
    pub request: ZimbraFetchRequest,
    pub secret: String,
    pub trusted: Option<(String, String)>,
    pub cancel: Arc<AtomicBool>,
}

/// A trusted-device token and its device id, as they are kept: one line each.
pub(crate) fn trust_text(token: &str, device: &str) -> String {
    format!("{token}\n{device}")
}

fn trust_of(kept: &str) -> Option<(String, String)> {
    let (token, device) = kept.split_once('\n')?;
    (!token.is_empty()).then(|| (token.to_string(), device.to_string()))
}

impl MailApp {
    /// The password a job of the Zimbra account `account` goes with, when one is at hand: the one
    /// typed this session, or else the one the operating system was asked to remember.
    pub(crate) fn zimbra_secret(&self, account: Id) -> Option<String> {
        if let Some(typed) = self.passwords.get(&account) {
            return Some(typed.clone());
        }
        self.remembered("password", account)
    }

    /// The trusted-device token kept for `account`, when one is.
    pub(crate) fn zimbra_trusted(&self, account: Id) -> Option<(String, String)> {
        self.remembered("trust", account).as_deref().and_then(trust_of)
    }

    /// A Send/Receive of the Zimbra account `account`; `p` may carry the password the owner has
    /// just typed, or the code of a second step. Asks for the password the first time.
    pub(crate) fn start_zimbra_mail(&mut self, account: Id, quiet: bool, p: &Value) -> Result<Value, String> {
        if let Some(typed) = p.get("password").and_then(Value::as_str).filter(|typed| !typed.is_empty()) {
            self.passwords.insert(account, typed.to_string());
        }
        let code = p.get("code").and_then(Value::as_str).map(str::trim).filter(|code| !code.is_empty()).map(str::to_string);
        // The owner asked: a password turned down before is tried again.
        if !quiet {
            self.jmap_refused.remove(&account);
        }
        let Some(secret) = self.zimbra_secret(account) else {
            self.dialog = dialogs::Dialog::open("password", self, &json!({"account": account}));
            return Ok(json!({"pending": "password"}));
        };
        let request = self.session.run("sync.zimbraRequest", &json!({"account": account, "background": quiet})).map_err(|e| e.to_string())?;
        let mut request: ZimbraMailRequest = serde_json::from_value(request).map_err(|e| e.to_string())?;
        // A certificate a file trusts counts once the owner has confirmed the file (AU3-02).
        if self.unconfirmed {
            request.trusted.clear();
        }
        if self.services.zimbra.is_none() {
            return Err(tl!("This build can't connect to mail servers").to_string());
        }
        self.services.sync_cancel.store(false, Ordering::Relaxed);
        let trust: TrustSlot = Arc::default();
        let trusted = if code.is_some() { None } else { self.zimbra_trusted(account) };
        let job = ZimbraJob { request, secret: secret.clone(), code, trusted, trust: trust.clone(), cancel: self.services.sync_cancel.clone() };
        // What it sends is marked so, on disk, before it leaves (AU3-26).
        let pieces = self.hand_zimbra(account, job)?;
        (self.sync_refused, self.sync_password) = (false, Some(secret));
        self.zimbra_trust = Some((account, trust));
        self.sync_run = Some((account, Some(self.waiting_to_go(account)), false, quiet));
        self.sync_rx = Some(MailRun::Zimbra { account, opening: self.session.openings(), pieces });
        self.session.sync_progress.phase(account, SyncPhase::Folders);
        self.session.sync_status = tl!("Sending and receiving…").to_string();
        Ok(json!({"started": true, "account": account, "zimbra": true}))
    }

    /// Take in what the Zimbra run under way has brought so far, a few pieces a frame; the last one
    /// ends the run. Called by [`MailApp::poll_sync`].
    pub(crate) fn poll_zimbra(&mut self) {
        for _ in 0..4 {
            let Some(MailRun::Zimbra { account, opening, pieces }) = &self.sync_rx else { return };
            let (account, opening) = (*account, *opening);
            let piece = match pieces.try_recv() {
                Ok(piece) => piece,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    log::warn!("zimbra mail: the run stopped without its last piece (its thread ended early)");
                    (self.sync_rx, self.sync_refused, self.sync_password, self.zimbra_trust) = (None, false, None, None);
                    self.session.sync_status = tl!("Send/Receive stopped unexpectedly").to_string();
                    return;
                }
            };
            let (last, whole, code_needed) = (!piece.partial, piece.whole, piece.code_needed);
            // Another mailbox took the place of the one the run was for: what it brings is nobody's.
            if opening != self.session.openings() {
                self.services.sync_cancel.store(true, Ordering::Relaxed);
                if last {
                    (self.sync_rx, self.sync_run, self.sync_refused, self.sync_password, self.zimbra_trust) = (None, None, false, None, None);
                    self.session.sync_status.clear();
                    return;
                }
                continue;
            }
            self.sync_refused |= piece.auth_refused;
            if !piece.pages.is_empty() {
                self.session.driver.fetch.wake();
            }
            let untrusted = !piece.untrusted.is_empty();
            if piece.server_answered() {
                self.keep_accepted_password(account);
            }
            if untrusted {
                self.ews_certificate.insert(account, piece.untrusted.clone());
            }
            match apply_zimbra_mail(&mut self.session, piece) {
                Ok(said) => {
                    let errors = said["errors"].as_array().map(|all| all.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>());
                    self.note_sync_problems(account, &errors.unwrap_or_default(), false);
                }
                Err(e) => {
                    self.status(e.to_string());
                    if !last {
                        self.services.sync_cancel.store(true, Ordering::Relaxed);
                    }
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
            if let Some((whose, _, true, quiet)) = self.sync_run.take() {
                self.session.driver.round.queue.push_front((whose, quiet));
            }
            // What the sign-in brought to be kept: this computer is trusted from now on.
            let brought =
                self.zimbra_trust.take().filter(|(whose, _)| *whose == account).and_then(|(_, slot)| slot.lock().ok().and_then(|mut t| t.take()));
            if let Some((token, device)) = brought {
                self.remember("trust", account, &trust_text(&token, &device));
            }
            if std::mem::take(&mut self.sync_refused) {
                // Turned down: the next Send/Receive asks for it again, and no round by itself goes
                // back until then. What the system remembered is no good either.
                self.passwords.remove(&account);
                self.jmap_refused.insert(account);
                if let (Some(store), Some(a)) = (&self.services.secrets, self.session.mailbox.account(account)) {
                    for kind in ["password", "trust"] {
                        let key = Self::secret_key(kind, a);
                        store.delete(&key);
                        self.secret_cache.remove(&key);
                    }
                }
            } else if code_needed {
                // A second step: the code is asked for, once; no round by itself tries again.
                self.jmap_refused.insert(account);
                if let (Some(store), Some(a)) = (&self.services.secrets, self.session.mailbox.account(account)) {
                    let key = Self::secret_key("trust", a);
                    store.delete(&key);
                    self.secret_cache.remove(&key);
                }
                self.dialog = dialogs::Dialog::open("zimbraCode", self, &json!({"account": account}));
            } else if untrusted {
                self.jmap_refused.insert(account);
            } else if whole {
                self.keep_accepted_password(account);
                self.ews_certificate.remove(&account);
                self.note_sync_problems(account, &[], true);
            }
            self.sync_password = None;
            self.note_pushed(account);
            return;
        }
    }
}
