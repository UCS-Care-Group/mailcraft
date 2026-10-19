//! A JMAP account's Send/Receive in the window (RFC 8620, 8621).
//!
//! The engine says what a run should do ([`jmap_mail_request`], `sync.jmapRequest`) and takes in
//! what it brought back ([`apply_jmap_mail`], `sync.applyJmap`). The host does the talking in
//! between, as a task of its own ([`crate::Services::jmap`]): `fetch` in the browser, a thread
//! where there are sockets. This file is the window's part: the job the host is handed
//! ([`JmapJob`]), starting a run ([`MailApp::start_jmap_mail`]) and taking in its pieces as they
//! come ([`MailApp::poll_jmap`]).
//!
//! **The pieces come through a plain channel**, not a bounded one as Microsoft Graph's do. In a
//! browser the host's task runs on the window's own thread, so a `send` that waited until the
//! window had taken a piece would wait for ever. The host wakes the window for each piece, and the
//! window takes a few a frame. Nothing piles up without limit: a run reads so many mailboxes, each
//! up to the window (500 messages at most).
//!
//! **The secret** that opens the server (a password, or a bearer token) is the host's when it holds
//! the credential itself ([`crate::Services::jmap_host_credentials`]): then no job carries one and
//! the owner is never asked for a password. Otherwise it is asked for as an IMAP account's is, kept
//! for the session, and a password (never a bearer token, which is an access token) is remembered
//! by the system's credential store once a run has gone through with it. It is in the job and
//! nowhere else: not in the request, not in a debug line.
//!
//! **A server that turns the secret down** ends the run with a plain line, and no run starts by
//! itself for that account until the owner asks for one: asking again would only be refused again.
//!
//! **Messages fetched whole** are asked for by [`crate::graph_fetch`], which dispatches by the kind
//! of account: the message the owner opens and the Inbox's newest in the background come as a
//! [`JmapFetchJob`] to [`crate::Services::jmap_fetch`], with the secret of the run (or none, when the
//! host holds the credential), and what comes back is the same
//! [`GraphFetchResult`](mailcraft_model::sync::GraphFetchResult) a Microsoft account's fetch gives.
//! A fetch never asks for a password and never reads the system's credential store: without a
//! secret at hand (nobody has signed in this session) it is not started, and the reading pane says
//! the message is not on this computer yet.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;

use mailcraft_engine::CmdResult;
use mailcraft_engine::cmd::graph_mail::apply_graph_fetch;
use mailcraft_engine::cmd::jmap_fetch::apply_jmap_fetch;
use mailcraft_engine::cmd::jmap_mail::{apply_jmap_mail, jmap_mail_request};
use mailcraft_model::sync::{GraphFetchResult, JmapFetchRequest, JmapMailRequest, JmapMailResult};
use mailcraft_model::{AccountKind, FolderKind, Id};
use serde_json::{Value, json};

use crate::MailApp;

/// Pieces of a run taken in at most in a frame: each one redraws the lists, and the window should
/// stay quick.
const PIECES_A_FRAME: usize = 4;

/// Most problem lines kept for a run: what is past them is of the same kind and is left out.
const MOST_PROBLEMS: usize = 200;

/// What the host is asked to do for one run of a JMAP account ([`crate::Services::jmap`]).
pub struct JmapJob {
    /// What the run reads, sends and changes: the engine's `sync.jmapRequest`, with the address of
    /// the server's session resource.
    pub request: JmapMailRequest,
    /// The secret that opens the server: the account's password, or a bearer token when
    /// `request.bearer` is set. `None` when the host holds the credential itself
    /// ([`crate::Services::jmap_host_credentials`]).
    pub secret: Option<String>,
    /// Set when the owner stops the run (Cancel): the host's run stops at the next safe place and
    /// says so in its last piece (`stopped`). It is [`crate::Services::sync_cancel`], which the
    /// window clears when it starts a run.
    pub cancel: Arc<AtomicBool>,
}

/// Counts and flags only: never the secret, nor the server's address, nor the account's.
impl std::fmt::Debug for JmapJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JmapJob")
            .field("request", &self.request)
            .field("secret", &self.secret.is_some())
            .field("cancel", &self.cancel.load(Ordering::Relaxed))
            .finish()
    }
}

/// What the host is asked to fetch ([`crate::Services::jmap_fetch`]): messages of a JMAP account, whole.
pub struct JmapFetchJob {
    /// Which messages, and from where: the engine's `sync.jmapFetchRequest`, with the address of the
    /// server's session resource.
    pub request: JmapFetchRequest,
    /// The secret that opens the server: the account's password, or a bearer token when
    /// `request.bearer` is set. `None` when the host holds the credential itself
    /// ([`crate::Services::jmap_host_credentials`]).
    pub secret: Option<String>,
    /// Set when the owner stops the downloads (Stop): the host's task stops before the next message
    /// and answers with what it has. It is the one the window keeps for the background's batch
    /// (`graph_fetch`); a message the owner opened is not stopped by it.
    pub cancel: Arc<AtomicBool>,
}

/// Counts and flags only: never the secret, nor the server's address, nor the account's, nor a message's id.
impl std::fmt::Debug for JmapFetchJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JmapFetchJob")
            .field("request", &self.request)
            .field("secret", &self.secret.is_some())
            .field("cancel", &self.cancel.load(Ordering::Relaxed))
            .finish()
    }
}

impl MailApp {
    /// Whether `account` is kept in step with a JMAP server.
    pub(crate) fn is_jmap(&self, account: Id) -> bool {
        self.session.mailbox.account(account).is_some_and(|a| a.kind == AccountKind::Jmap)
    }

    /// Take in what a fetch of messages brought: a JMAP account's through its own intake
    /// ([`apply_jmap_fetch`]: it also passes over what the server no longer has), any other account's as a
    /// Microsoft account's is ([`apply_graph_fetch`]).
    pub(crate) fn apply_fetched(&mut self, result: GraphFetchResult) -> CmdResult {
        if self.is_jmap(result.account) { apply_jmap_fetch(&mut self.session, result) } else { apply_graph_fetch(&mut self.session, result) }
    }

    /// What a fetch of messages of the JMAP account `account` is opened with, when it can be opened now:
    /// `Some(None)` when the host holds the credential (the job carries no secret), `Some(Some(secret))`
    /// with the one of this session otherwise, and `None` when there is none at hand (nobody has signed in
    /// since the program started: a fetch asks for no password and doesn't read the system's store) or the
    /// server turned it down since the last run. A lookup in memory, nothing more.
    pub(crate) fn jmap_fetch_secret(&self, account: Id) -> Option<Option<String>> {
        if self.jmap_refused.contains(&account) {
            return None;
        }
        if self.services.jmap_host_credentials {
            return Some(None);
        }
        self.passwords.get(&account).cloned().map(Some)
    }

    /// A Send/Receive of an account kept in step with a JMAP server: hands the host a job with
    /// what the engine says the run should do, and takes its pieces in from then on
    /// ([`MailApp::poll_jmap`]).
    ///
    /// The owner's own Send/Receive and the first run of an account read every mailbox; a run that
    /// started by itself (`quiet`) reads the Inbox and the folder on screen, and asks for the share
    /// of requests that work nobody waits for gets. Without a secret at hand (and with a host that
    /// doesn't hold the credential) the owner is asked for a password, as for an IMAP account; a
    /// run that started by itself asks nobody.
    pub(crate) fn start_jmap_mail(&mut self, account: Id, quiet: bool) -> Result<Value, String> {
        let owner = self
            .session
            .mailbox
            .account(account)
            .filter(|a| a.kind == AccountKind::Jmap)
            .cloned()
            .ok_or_else(|| tl!("This account isn't kept in step with a JMAP server").to_string())?;
        if self.services.jmap.is_none() {
            return Err(tl!("This build can't connect to JMAP servers").to_string());
        }
        let bearer = owner.jmap.as_ref().is_some_and(|server| server.bearer);
        let secret = if self.services.jmap_host_credentials {
            None
        } else if let Some(secret) = self.jmap_secret(account, &owner.email, bearer) {
            Some(secret)
        } else if quiet {
            // Nobody asked for this run: no dialog is put in front of the owner.
            return Err(format!("{} {}", tl!("Use Send/Receive to sign in to"), owner.email));
        } else {
            self.dialog = crate::dialogs::Dialog::open("password", self, &json!({"account": account}));
            return Ok(json!({"pending": "password"}));
        };
        // The folder on screen is read besides the Inbox, when it is one of this account's that the
        // server has.
        let on_screen = self
            .session
            .mailbox
            .folder(self.session.view.folder)
            .filter(|f| f.account == account && f.kind != FolderKind::Inbox)
            .and_then(|f| f.server.as_ref())
            .map(|server| server.id.clone());
        let also: Vec<String> = on_screen.into_iter().collect();
        let request = jmap_mail_request(&self.session, account, !quiet, &also, quiet).map_err(|e| e.to_string())?;
        let Some(start) = &self.services.jmap else { return Err(tl!("This build can't connect to JMAP servers").to_string()) };
        self.services.sync_cancel.store(false, Ordering::Relaxed);
        let job = JmapJob { request, secret, cancel: self.services.sync_cancel.clone() };
        self.jmap_problems.clear();
        // A run is a fresh try: what the server said of the secret before is not held against it.
        self.jmap_refused.remove(&account);
        self.jmap_rx = Some((account, quiet, start(job)));
        self.session.sync_status = tl!("Sending and receiving…").to_string();
        Ok(json!({"started": true, "account": account, "jmap": true}))
    }

    /// The secret a JMAP account's run is opened with, when one is at hand: the one typed this
    /// session, else, for a password, the one the system remembers. A bearer token is only ever in
    /// memory.
    fn jmap_secret(&mut self, account: Id, address: &str, bearer: bool) -> Option<String> {
        if !bearer
            && !self.passwords.contains_key(&account)
            && let Some(password) = self.remembered("password", address)
        {
            self.passwords.insert(account, password);
        }
        self.passwords.get(&account).cloned()
    }

    /// Take in what the JMAP run under way has brought so far, a few pieces a frame. The pieces
    /// arrive in the order things were done, each whole in itself, so mail shows up while the rest
    /// is still on its way; the piece that isn't `partial` ends the run. A sender that is dropped
    /// before that piece is a run that stopped unexpectedly. (Public for a host's tests, which
    /// have no window.)
    pub fn poll_jmap(&mut self) {
        let Some((account, _, rx)) = &self.jmap_rx else { return };
        let account = *account;
        let mut pieces = Vec::new();
        let mut dropped = false;
        while pieces.len() < PIECES_A_FRAME {
            match rx.try_recv() {
                Ok(piece) => {
                    let last = !piece.partial;
                    pieces.push(piece);
                    if last {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    dropped = true;
                    break;
                }
            }
        }
        for piece in pieces {
            self.take_jmap_piece(account, piece);
            // The last piece ended the run: whatever the host sends after it is not read.
            if self.jmap_rx.is_none() {
                return;
            }
        }
        if dropped {
            self.jmap_run_stopped_early(account);
        }
    }

    /// One piece of the run of `account`.
    fn take_jmap_piece(&mut self, account: Id, mut piece: JmapMailResult) {
        // The pieces of a run are for the run's account, whatever they name; the engine still
        // checks the address they carry against the account's.
        piece.account = account;
        let (last, stopped) = (!piece.partial, piece.stopped);
        if piece.auth_refused {
            self.jmap_refused.insert(account);
        }
        // New mail may be waiting to be fetched whole: the background looks again at once.
        if !piece.pages.is_empty() {
            self.fetching.wake();
        }
        match apply_jmap_mail(&mut self.session, piece) {
            Ok(said) => {
                let errors = said["errors"].as_array().map(|all| all.iter().filter_map(Value::as_str).map(str::to_string));
                let room = MOST_PROBLEMS.saturating_sub(self.jmap_problems.len());
                self.jmap_problems.extend(errors.into_iter().flatten().take(room));
            }
            Err(e) if self.jmap_problems.len() < MOST_PROBLEMS => self.jmap_problems.push(e.to_string()),
            Err(_) => {}
        }
        if last {
            self.finish_jmap_run(account, stopped);
        }
    }

    /// The run of `account` is over: its last piece is in. What it said is kept for the problems
    /// report, a secret the server turned down is forgotten, one that worked is remembered, and
    /// what still waits to go to the server doesn't start another run every few seconds.
    fn finish_jmap_run(&mut self, account: Id, stopped: bool) {
        self.jmap_rx = None;
        // Its part of the run is over, however its last piece went in.
        self.session.sync_progress.end(account, stopped);
        let mut errors = std::mem::take(&mut self.jmap_problems);
        errors.dedup();
        let (address, bearer) = match self.session.mailbox.account(account) {
            Some(a) => (a.email.clone(), a.jmap.as_ref().is_some_and(|server| server.bearer)),
            None => (String::new(), false),
        };
        if self.jmap_refused.contains(&account) {
            // The password is forgotten, so that the next try asks for it again; a sign-in that
            // the host holds is the host's to renew. No run starts by itself for this account
            // until the owner asks for one (`can_sync_quietly`).
            self.passwords.remove(&account);
            if !bearer && let Some(store) = &self.services.secrets {
                store.delete(&Self::secret_key("password", &address));
            }
            let said = if self.services.jmap_host_credentials {
                tl!("The server refused the sign-in. Sign in again, then use Send/Receive.")
            } else {
                tl!("The server refused the sign-in. Use Send/Receive to enter it again.")
            };
            self.session.sync_status = said.to_string();
            if errors.is_empty() {
                errors.push(said.to_string());
            }
        } else {
            if let Some(first) = errors.first() {
                self.session.sync_status = first.clone();
            } else if self.session.sync_status == tl!("Sending and receiving…") {
                // The run is over: it never says it is still going.
                self.session.sync_status.clear();
            }
            // It went through: this password is worth remembering.
            if errors.is_empty()
                && !bearer
                && let Some(password) = self.passwords.get(&account)
            {
                self.remember("password", &address, password);
            }
        }
        self.note_sync_problems(account, &errors, true);
        self.note_pushed(account);
    }

    /// The host's sender was dropped before the last piece of the run of `account`: whatever came
    /// is in, and the run is over without having finished.
    fn jmap_run_stopped_early(&mut self, account: Id) {
        log::warn!("jmap, account {account}: the run stopped without its last piece (its task ended early)");
        self.jmap_rx = None;
        self.session.sync_progress.end(account, true);
        let said = tl!("Send/Receive stopped unexpectedly").to_string();
        let mut errors = std::mem::take(&mut self.jmap_problems);
        errors.push(said.clone());
        self.session.sync_status = said;
        self.note_sync_problems(account, &errors, false);
        self.note_pushed(account);
    }
}

#[cfg(test)]
#[path = "jmap_job_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "jmap_fetch_tests.rs"]
mod fetch_tests;
