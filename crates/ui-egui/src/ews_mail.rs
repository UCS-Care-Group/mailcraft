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
//! ([`MailApp::secret_key`]). It is remembered with the operating system, when the owner lets it
//! be, by the Add Account page and by a run as soon as the server has answered it with its
//! folders or its mail (#802). A password the server turns down (a 401) is forgotten, so that the
//! next Send/Receive asks again rather than send it once more; a round by itself leaves the
//! account out until the owner asks for its Send/Receive. The window asks for it again at once
//! when the run was the owner's (the first one after Add Account among them), and the status bar
//! offers Sign In Again until a password is typed (#971).
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
        self.remembered("password", account)
    }

    /// Whether the server of the Exchange server account `account` turned its password down and no
    /// other has been typed since (#971): the password dialog says so, and the status bar offers
    /// Sign In Again. (A run that stopped at a certificate keeps its password, so it isn't one.)
    pub(crate) fn ews_turned_down(&self, account: Id) -> bool {
        self.jmap_refused.contains(&account)
            && self.session.mailbox.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Exchange)
            && self.ews_secret(account).is_none()
    }

    /// Ask for the password of the account a Send/Receive the owner asked for was turned down for
    /// while another question was open (`ews_ask_again`), now that none is: `true` when
    /// it asked. Called by the round when nothing else is under way. An account that has a password
    /// again, or is gone, is not asked about.
    pub(crate) fn ask_ews_again(&mut self) -> bool {
        if self.dialog.is_some() {
            return false;
        }
        let Some(account) = self.ews_ask_again.take().filter(|account| self.ews_turned_down(*account)) else { return false };
        self.dialog = dialogs::Dialog::open("password", self, &json!({"account": account}));
        self.dialog.is_some()
    }

    /// The first Exchange server account, in the folder pane's order, whose server turned its
    /// password down ([`MailApp::ews_turned_down`]), and its address: the one the status bar's Sign
    /// In Again is for.
    pub(crate) fn ews_to_sign_in(&self) -> Option<(Id, String)> {
        if self.jmap_refused.is_empty() {
            return None;
        }
        self.session.mailbox.accounts_in_order().into_iter().find(|a| self.ews_turned_down(a.id)).map(|a| (a.id, a.email.clone()))
    }

    /// A Send/Receive of the Exchange server account `account`; `p` may carry the password the owner
    /// has just typed, and `inboxPass` asks for the Inbox alone. Asks for the password the first time.
    pub(crate) fn start_ews_mail(&mut self, account: Id, quiet: bool, p: &Value) -> Result<Value, String> {
        if let Some(typed) = p.get("password").and_then(Value::as_str).filter(|typed| !typed.is_empty()) {
            self.passwords.insert(account, typed.to_string());
        }
        // (An account its server turned down stays so until a password is typed for it: the
        // dialog says why it asks, and the status bar keeps Sign In Again if it is closed.)
        let Some(secret) = self.ews_secret(account) else {
            self.dialog = dialogs::Dialog::open("password", self, &json!({"account": account}));
            return Ok(json!({"pending": "password"}));
        };
        // The owner asked: a password turned down before is tried again.
        if !quiet {
            self.jmap_refused.remove(&account);
        }
        let inbox_pass = p.get("inboxPass").and_then(Value::as_bool).unwrap_or(false);
        let params = json!({"account": account, "background": quiet, "inboxPass": inbox_pass});
        let request = self.session.run("sync.ewsRequest", &params).map_err(|e| e.to_string())?;
        let mut request: EwsMailRequest = serde_json::from_value(request).map_err(|e| e.to_string())?;
        // A certificate a file trusts counts once the owner has confirmed the file (AU3-02).
        if self.unconfirmed {
            request.trusted.clear();
        }
        if self.services.ews.is_none() {
            return Err(tl!("This build can't connect to mail servers").to_string());
        }
        self.services.sync_cancel.store(false, Ordering::Relaxed);
        let sent = Some(secret.clone());
        // What it sends is marked so, on disk, before it leaves (AU3-26).
        let pieces = self.hand_ews(account, EwsJob { request, secret, cancel: self.services.sync_cancel.clone() })?;
        (self.sync_refused, self.sync_password) = (false, sent);
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
                    (self.sync_rx, self.sync_refused, self.sync_password) = (None, false, None);
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
                    (self.sync_rx, self.sync_run, self.sync_refused, self.sync_password) = (None, None, false, None);
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
            let untrusted = !piece.untrusted.is_empty();
            // The server answered with its folders or its mail: it took the password (#802).
            if piece.server_answered() {
                self.keep_accepted_password(account);
            }
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
                    self.session.driver.sending.doubted();
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
            self.logged_problems.end_run(account, "mail");
            self.sync_rx = None;
            // What it sent is filed, and what it didn't is known not to have gone (AU3-26).
            self.session.send_run_answered(account);
            // (Whether the owner asked for this run, or it came by itself.)
            let asked = matches!(self.sync_run, Some((_, _, _, false)));
            // Called off for something else (a shared mailbox's first read): it goes on after.
            if let Some((whose, _, true, quiet)) = self.sync_run.take() {
                self.session.driver.round.queue.push_front((whose, quiet));
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
                // A run the owner asked for asks for the password again at once, rather than only
                // say it was turned down (#971); a question already open (Add Account's last page,
                // say) is left as it is, and the password is asked for once it is closed. A round
                // by itself asks nothing: the status bar offers Sign In Again.
                if asked && self.dialog.is_none() {
                    self.dialog = dialogs::Dialog::open("password", self, &json!({"account": account}));
                } else if asked {
                    self.ews_ask_again = Some(account);
                }
            } else if untrusted {
                // The certificate question comes first (File › Info): no round by itself tries again.
                self.jmap_refused.insert(account);
            } else if whole {
                // It went through: that password is worth remembering (when the owner lets it be,
                // and no piece said so before), and nothing is left of the last run's problems.
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
