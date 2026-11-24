// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Automatic Replies of an IMAP account in the window (#9999): kept in a Sieve script on its
//! server, read and written by the host through ManageSieve ([`crate::Services::sieve`]). The
//! dialog and File › Info are the Microsoft accounts' ([`crate::auto_replies`]); what differs is
//! here: the job, what is said when the server can't keep them, and the explicit replacement of
//! a script changed elsewhere.
//!
//! **Jobs.** The engine says what to ask (`account.automaticReplies` answers `ask.sieve`;
//! `account.setAutomaticReplies` answers the script to write) and takes what came back in
//! (`sync.applySieve`); the host does the asking on a background thread. They are read when File ›
//! Info is shown or the dialog opens, at most every [`REFRESH_MS`], never while working offline
//! or while the mailbox's servers wait for the owner's confirmation; a server that doesn't speak
//! ManageSieve is not asked again in the session, until the owner says Try Again or names another
//! Sieve server in Account Settings. What was found where the server was looked for (nothing
//! answers there, it says nothing, something else answers, no STARTTLS, a failed handshake) is
//! said as the host found it, naming the host and port, with what to do about it.
//!
//! **The sign-in** is the account's own: the password the owner entered (or the one the credential
//! store gave back), or a token that is still good, from memory. Without one nothing is asked.
//!
//! **Nothing of the owner's is written over**: that is the engine's and `mailcraft-net`'s to keep
//! (a script changed elsewhere, another active script); the window says what they found.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::sieve_replies::is_sieve_account;
use mailcraft_model::{Account, Id};
use serde_json::{Value, json};

use crate::{MailApp, now_ms};

/// How long what was read is good for before File › Info reads it again (milliseconds).
pub const REFRESH_MS: f64 = 15.0 * 60_000.0;

/// What the host is asked to do with one account's Sieve script ([`crate::Services::sieve`]).
pub struct SieveJob {
    pub account: Account,
    /// The password, or the access token of an account that signs in with OAuth.
    pub secret: String,
    /// What to write first (`account.setAutomaticReplies`' `sieve`); `None` only reads.
    pub write: Option<SieveWrite>,
    pub cancel: Arc<AtomicBool>,
}

/// A script to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SieveWrite {
    pub name: String,
    pub text: String,
    /// The digest of what may be written over; `None`: only where there is none.
    pub last_written: Option<String>,
    pub activate: bool,
}

/// What a [`SieveJob`] came to. Never an error as a whole: every trouble is said.
#[derive(Default)]
pub struct SieveResult {
    pub account: Id,
    /// What the server has after the job (`sync.applySieve`'s `state`), when it could be read.
    pub state: Option<Value>,
    /// What became of a write: `done`, `anotherActive` or `changedElsewhere`.
    pub written: Option<String>,
    /// No ManageSieve session could be had: nothing answers on its port, the server says
    /// nothing, something else answers, it offers no STARTTLS, or the handshake failed.
    pub unavailable: bool,
    /// With `unavailable`: what was found, in a few words naming the host and port (never a
    /// secret); empty when the host didn't say.
    pub why: String,
    /// With `unavailable`: what the owner can do about it.
    pub advice: String,
    /// The server refused the sign-in.
    pub sign_in_refused: bool,
    /// What went wrong otherwise, in a few words; empty when nothing did.
    pub error: String,
}

/// Why an account's Sieve script can't be had just now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SieveTrouble {
    Unavailable,
    SignIn,
    Failed(String),
}

/// What was found where an account's ManageSieve server was looked for, when no session could
/// be had ([`SieveTrouble::Unavailable`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Unreached {
    /// What was found, naming the host and port.
    pub(crate) why: String,
    /// What to do about it.
    pub(crate) advice: String,
    /// The servers it was found with ([`sieve_server_key`]): once the owner names others, the
    /// server is asked again.
    server: String,
}

/// The servers an account's Sieve scripts are looked for at, as one key: its incoming host and
/// the Sieve host and port named in Account Settings.
fn sieve_server_key(account: &Account) -> String {
    let incoming = account.incoming.as_ref().map(|s| s.host.trim().to_ascii_lowercase()).unwrap_or_default();
    format!("{incoming} {} {}", account.sieve_host.trim().to_ascii_lowercase(), account.sieve_port)
}

/// The job under way, and what the window remembers between looks. Never saved.
#[derive(Default)]
pub struct SieveSync {
    /// The job under way: its account, what it writes, and the answer to come.
    rx: Option<(Id, Option<SieveWrite>, Receiver<SieveResult>)>,
    /// When each account was last read (milliseconds).
    pub(crate) read: HashMap<Id, f64>,
    /// Accounts to read at the next chance.
    wanted: HashSet<Id>,
    /// What to write for each account, at the next chance.
    waiting: HashMap<Id, SieveWrite>,
    pub(crate) trouble: HashMap<Id, SieveTrouble>,
    /// What was found, for the accounts whose trouble is [`SieveTrouble::Unavailable`].
    pub(crate) unreached: HashMap<Id, Unreached>,
    /// The mailbox opening all of this is about ([`mailcraft_engine::Session::openings`]).
    opening: u64,
}

impl SieveSync {
    /// Whether a job for `account` is in flight, and whether it writes.
    pub(crate) fn busy_for(&self, account: Id) -> Option<bool> {
        self.rx.as_ref().filter(|(a, ..)| *a == account).map(|(_, write, _)| write.is_some())
    }
}

impl MailApp {
    /// Whether `account`'s Automatic Replies are a Sieve script here: an IMAP account of its own,
    /// with a host that reaches ManageSieve servers.
    pub(crate) fn sieve_account(&self, account: Id) -> bool {
        self.services.sieve.is_some() && self.session.mailbox.account(account).is_some_and(is_sieve_account)
    }

    /// The IMAP accounts whose Automatic Replies File › Info offers.
    pub(crate) fn sieve_accounts(&self) -> Vec<Id> {
        if self.services.sieve.is_none() {
            return Vec::new();
        }
        self.session.mailbox.accounts.iter().filter(|a| is_sieve_account(a)).map(|a| a.id).collect()
    }

    /// What the account signs in with, from memory: `None` while there is nothing.
    fn sieve_secret(&self, account: &Account) -> Option<String> {
        if account.oauth.is_some() {
            return self.tokens.get(&account.id).filter(|t| t.fresh(mailcraft_time::now() + 60)).map(|t| t.access.clone());
        }
        self.passwords.get(&account.id).filter(|p| !p.is_empty()).cloned().or_else(|| self.remembered("password", account.id))
    }

    /// `account`'s replies are to be read (`account.automaticReplies` asked for them): at once,
    /// unless they were read a moment ago.
    pub(crate) fn sieve_read_now(&mut self, account: Id) {
        self.sieve_forget_if_replaced();
        // Another Sieve server named in Account Settings since: it is asked at once.
        let server = self.session.mailbox.account(account).map(sieve_server_key).unwrap_or_default();
        if self.sieve.unreached.get(&account).is_some_and(|u| u.server != server) {
            self.sieve_forget_trouble(account);
        }
        let fresh = self.sieve.read.get(&account).is_some_and(|at| now_ms() - at < REFRESH_MS);
        // (A server that speaks no ManageSieve isn't asked again in this session, unless the owner
        // says Try Again.)
        if fresh || self.sieve.trouble.get(&account) == Some(&SieveTrouble::Unavailable) {
            return;
        }
        self.sieve.wanted.insert(account);
        self.sieve_tick_at(now_ms());
    }

    /// Try Again: `account`'s server is asked again now, whatever was found there before.
    /// (Public for a host's tests.)
    pub fn sieve_try_again(&mut self, account: Id) {
        self.sieve_forget_trouble(account);
        self.sieve.read.remove(&account);
        self.sieve_read_now(account);
    }

    /// Forget what stood in the way of `account`'s Sieve scripts.
    fn sieve_forget_trouble(&mut self, account: Id) {
        self.sieve.trouble.remove(&account);
        self.sieve.unreached.remove(&account);
    }

    /// What was found where `account`'s ManageSieve server was looked for, when the host said
    /// (with [`SieveTrouble::Unavailable`]).
    pub(crate) fn sieve_unreached(&self, account: Id) -> Option<&Unreached> {
        self.sieve.unreached.get(&account).filter(|u| !u.why.is_empty() && self.sieve.trouble.get(&account) == Some(&SieveTrouble::Unavailable))
    }

    /// The owner changed `account`'s replies: the script `account.setAutomaticReplies` said to
    /// write goes at the next chance.
    pub(crate) fn sieve_write(&mut self, account: Id, sieve: &Value) -> Result<(), String> {
        if self.services.sieve.is_none() {
            return Err(tl!("This build can't reach the server to change automatic replies. Use the desktop app.").to_string());
        }
        let text = sieve.get("text").and_then(Value::as_str).unwrap_or_default();
        let name = sieve.get("name").and_then(Value::as_str).unwrap_or_default();
        if text.is_empty() || name.is_empty() {
            return Err(tl!("Your automatic replies weren't changed").to_string());
        }
        let write = SieveWrite {
            name: name.to_string(),
            text: text.to_string(),
            last_written: sieve.get("lastWritten").and_then(Value::as_str).map(str::to_string),
            activate: sieve.get("activate").and_then(Value::as_bool).unwrap_or(false),
        };
        self.sieve.waiting.insert(account, write);
        self.sieve_tick_at(now_ms());
        Ok(())
    }

    /// Start what is waiting: a write first, then the reads that were asked for. (Public for a
    /// host's tests.)
    pub fn sieve_tick_at(&mut self, now: f64) -> bool {
        self.sieve_forget_if_replaced();
        if self.sieve.rx.is_some() || self.services.sieve.is_none() || self.offline() || self.unconfirmed {
            return false;
        }
        let next = self.sieve.waiting.keys().next().copied().map(|a| (a, true)).or_else(|| self.sieve.wanted.iter().next().map(|a| (*a, false)));
        let Some((account, writing)) = next else { return false };
        let write = if writing { self.sieve.waiting.remove(&account) } else { None };
        self.sieve.wanted.remove(&account);
        let Some(owner) = self.session.mailbox.account(account).cloned().filter(is_sieve_account) else { return false };
        let Some(secret) = self.sieve_secret(&owner) else {
            if write.is_some() {
                self.status(tl!("Sign in with Send/Receive first, then change your automatic replies."));
            }
            return false;
        };
        let job = SieveJob { account: owner, secret, write: write.clone(), cancel: Default::default() };
        let Some(host) = self.services.sieve.as_ref() else { return false };
        self.sieve.rx = Some((account, write.clone(), host(job)));
        self.sieve.read.insert(account, now);
        if write.is_some() {
            self.session.sync_status = tl!("Changing your automatic replies…").to_string();
        }
        true
    }

    pub(crate) fn sieve_tick(&mut self, ctx: &egui::Context) {
        self.sieve_tick_at(now_ms());
        if self.sieve.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// The job answered: take in what the server has, and say how a change went. (Public for a
    /// host's tests.)
    pub fn poll_sieve(&mut self) {
        self.sieve_forget_if_replaced();
        let Some((account, write, rx)) = &self.sieve.rx else { return };
        let account = *account;
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                SieveResult { account, error: tl!("Reading the automatic replies stopped unexpectedly").to_string(), ..Default::default() }
            }
        };
        let write = write.clone();
        self.sieve.rx = None;
        if write.is_some() {
            self.session.sync_status.clear();
        }
        let trouble = if result.unavailable {
            Some(SieveTrouble::Unavailable)
        } else if result.sign_in_refused {
            Some(SieveTrouble::SignIn)
        } else if !result.error.is_empty() {
            Some(SieveTrouble::Failed(result.error.clone()))
        } else {
            None
        };
        let mut said_error = None;
        if let Some(state) = &result.state {
            let mut params = json!({"account": account, "state": state});
            if let (Some(written), Some(write)) = (&result.written, &write) {
                params["written"] = json!({"result": written, "text": write.text});
            }
            if let Err(e) = self.session.run("sync.applySieve", &params) {
                said_error = Some(e.to_string());
            }
        }
        match trouble.clone().or(said_error.clone().map(SieveTrouble::Failed)) {
            Some(trouble) => {
                self.sieve.trouble.insert(account, trouble);
            }
            None => {
                self.sieve.trouble.remove(&account);
            }
        }
        if result.unavailable {
            let server = self.session.mailbox.account(account).map(sieve_server_key).unwrap_or_default();
            self.sieve.unreached.insert(account, Unreached { why: result.why.clone(), advice: result.advice.clone(), server });
        } else {
            self.sieve.unreached.remove(&account);
        }
        if write.is_none() {
            return;
        }
        // A change the owner asked for: said whichever way it went.
        let said = match (result.written.as_deref(), &trouble) {
            (Some("done"), _) => match self.auto_replies_known(account).map(|k| k.on_at(self.session.now())) {
                Some(true) => tl!("Automatic replies are on").to_string(),
                _ if self.auto_replies_known(account).is_some_and(|k| k.status == mailcraft_model::auto_replies::AutoReplyStatus::Scheduled) => {
                    tl!("Automatic replies are set for the time range you chose").to_string()
                }
                _ => tl!("Automatic replies are off").to_string(),
            },
            (Some("anotherActive"), _) => {
                tl!("Your automatic replies were saved on the server, but another Sieve script of yours runs there, so they aren't sent").to_string()
            }
            (Some(_), _) => crate::i18n::with_reason(
                tl!("Your automatic replies weren't changed"),
                tl!("MailCraft's Sieve script on the server was changed elsewhere."),
            ),
            (None, Some(SieveTrouble::Unavailable)) if !result.why.is_empty() => {
                crate::i18n::with_reason(tl!("Your automatic replies weren't changed"), &result.why)
            }
            (None, Some(SieveTrouble::Unavailable)) => {
                crate::i18n::with_reason(tl!("Your automatic replies weren't changed"), tl!("the server doesn't offer Sieve scripts (ManageSieve)."))
            }
            (None, Some(SieveTrouble::SignIn)) => {
                crate::i18n::with_reason(tl!("Your automatic replies weren't changed"), tl!("the server didn't take your sign-in."))
            }
            (None, Some(SieveTrouble::Failed(why))) => crate::i18n::with_reason(tl!("Your automatic replies weren't changed"), why),
            (None, None) => tl!("Your automatic replies weren't changed").to_string(),
        };
        self.status(said);
    }

    /// For a stand-in host (the screenshot example): `account`'s Sieve server as `how` says —
    /// `"on"` (MailCraft's script sends replies from Monday for a week), `"off"`, `"changed"`
    /// (MailCraft's script was changed elsewhere), `"other"` (the owner's own script is active),
    /// `"noVacation"` (Sieve without replies), `"none"` (no ManageSieve). A host that never
    /// answers is put in, so that nothing is asked.
    #[doc(hidden)]
    pub fn stand_in_sieve(&mut self, account: Id, how: &str) -> Result<(), String> {
        if self.services.sieve.is_none() {
            self.services.sieve = Some(Box::new(|_| std::sync::mpsc::channel().1));
        }
        self.sieve.read.insert(account, now_ms());
        let all = ["fileinto", "vacation", "date", "relational"];
        let state = |extensions: &[&str], active: Option<&str>, script: Option<&str>| {
            let scripts: Vec<Value> = active.iter().map(|name| json!({"name": name, "active": true})).collect();
            json!({"extensions": extensions, "scripts": scripts, "script": script})
        };
        let apply = |app: &mut MailApp, params: Value| app.session.run("sync.applySieve", &params).map(|_| ()).map_err(|e| e.to_string());
        let name = mailcraft_engine::sieve::SCRIPT_NAME;
        match how {
            "none" => {
                self.sieve.trouble.insert(account, SieveTrouble::Unavailable);
                Ok(())
            }
            "noVacation" => apply(self, json!({"account": account, "state": state(&["fileinto"], None, None)})),
            "changed" => {
                apply(self, json!({"account": account, "state": state(&all, Some(name), Some("require \"fileinto\";\r\nfileinto \"Lists\";\r\n"))}))
            }
            "other" => apply(self, json!({"account": account, "state": state(&all, Some("roundcube"), None)})),
            _ => {
                apply(self, json!({"account": account, "state": state(&all, None, None)}))?;
                if how == "off" {
                    return Ok(());
                }
                let start = self.session.now().saturating_add(3_600 - self.session.now().rem_euclid(3_600)).saturating_sub(86_400);
                let at = |t: i64| mailcraft_time::DateTime::from_timestamp(t, 0).iso().get(..16).unwrap_or_default().to_string();
                let change = self
                    .session
                    .run(
                        "account.setAutomaticReplies",
                        &json!({"account": account, "status": "scheduled", "start": at(start), "end": at(start + 7 * 86_400),
                            "internal": "I'm away this week. For anything urgent, ask bo@example.org.",
                            "external": "Thank you for your message. I'm out of the office this week.", "audience": "all"}),
                    )
                    .map_err(|e| e.to_string())?;
                let text = change.pointer("/sieve/text").and_then(Value::as_str).unwrap_or_default().to_string();
                apply(self, json!({"account": account, "state": state(&all, Some(name), Some(&text)), "written": {"result": "done", "text": text}}))
            }
        }
    }

    /// For a host's tests: the password the owner typed for `account`, in memory, as the
    /// password dialog keeps it.
    #[doc(hidden)]
    pub fn stand_in_password(&mut self, account: Id, password: &str) {
        self.passwords.insert(account, password.to_string());
    }

    /// Forget everything about the mailbox that was open when another has taken its place.
    pub(crate) fn sieve_forget_if_replaced(&mut self) {
        let opening = self.session.openings();
        if self.sieve.opening != opening {
            if self.sieve.rx.as_ref().is_some_and(|(_, write, _)| write.is_some())
                && self.session.sync_status == tl!("Changing your automatic replies…")
            {
                self.session.sync_status.clear();
            }
            self.sieve = SieveSync { opening, ..Default::default() };
        }
    }

    /// What File › Info says of an IMAP account's Automatic Replies. (Public for a host's tests.)
    pub fn sieve_line(&self, account: Id) -> String {
        let shown = |why: &str| crate::forms::kit::shown(why, crate::server_sync::REASON_SHOWN);
        if let Some(unreached) = self.sieve_unreached(account) {
            return crate::i18n::fmt(tl!("Automatic replies: not available ({why})"), &[("why", shown(&unreached.why).as_str())]);
        }
        match self.sieve.trouble.get(&account) {
            Some(SieveTrouble::Unavailable) => {
                return tl!("Automatic replies: not available, this server doesn't offer Sieve scripts (ManageSieve)").to_string();
            }
            Some(SieveTrouble::SignIn) => return tl!("Automatic replies: the server didn't take your sign-in").to_string(),
            Some(SieveTrouble::Failed(why)) => {
                return crate::i18n::fmt(tl!("Automatic replies: couldn't be read ({why})"), &[("why", shown(why).as_str())]);
            }
            None => {}
        }
        let Some(sieve) = self.session.sieve.get(&account) else {
            return if self.sieve.busy_for(account).is_some() {
                tl!("Automatic replies: reading…").to_string()
            } else {
                tl!("Automatic replies: not read yet").to_string()
            };
        };
        if !sieve.takes_replies() {
            return tl!("Automatic replies: not available, this server's Sieve scripts can't send them").to_string();
        }
        if sieve.changed_elsewhere {
            return tl!("Automatic replies: MailCraft's Sieve script was changed elsewhere, and is left as it is").to_string();
        }
        let line = self.auto_replies_known(account).map(|known| crate::auto_replies::summary(self, known)).unwrap_or_default();
        match sieve.active.as_deref() {
            Some(other) if other != mailcraft_engine::sieve::SCRIPT_NAME => {
                crate::i18n::fmt(tl!("{line} · your Sieve script \"{name}\" runs instead"), &[("line", &line), ("name", &shown(other))])
            }
            _ => line,
        }
    }
}
