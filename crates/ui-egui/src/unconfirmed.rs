// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The **unconfirmed mailbox** (C1 PR-c): a mailbox opened from a file (File > Open, a double-click,
//! a drop) names accounts, servers and queued mail of its own choosing, so nothing of it reaches
//! a server until the owner has seen every account's servers and confirmed them. The
//! confirmation is kept for that file and a fingerprint of its accounts' addresses and servers:
//! change either and the owner is asked again. The owner's own mailbox and one named with
//! `--mailbox` are never unconfirmed.
//!
//! While a mailbox is unconfirmed the jobs that reach servers (Send/Receive, Microsoft Graph, the
//! sign-in, fetching and searching on a server) are held back, so the window does not start any of
//! them: no automatic Send/Receive, no quiet push, nothing in the Outbox sent, no changes sent, no
//! rules run on arriving mail. A manual Send/Receive asks the owner to confirm instead.

use std::sync::mpsc::Receiver;

use egui::Ui;
use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens};
use mailcraft_model::{Account, Mailbox};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::dialogs::Question;
use crate::{GraphRequest, GraphResult, MailApp, SignInRequest};

/// The most files a confirmation is kept for.
const MAX_CONFIRMED: usize = 200;

/// A file whose accounts' servers the owner confirmed (kept in `ui.json`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConfirmedMailbox {
    pub path: String,
    pub fingerprint: String,
}

/// The jobs that reach servers, put aside while a mailbox is unconfirmed.
#[derive(Default)]
pub(crate) struct Held {
    sync: Option<Box<dyn Fn(SyncRequest) -> Receiver<SyncResult>>>,
    sign_in: Option<Box<dyn Fn(SignInRequest) -> Receiver<Result<Tokens, String>>>>,
    graph: Option<Box<dyn Fn(GraphRequest) -> Receiver<GraphResult>>>,
    graph_fetch: Option<Box<dyn Fn(crate::graph_fetch::GraphFetchJob) -> Receiver<mailcraft_model::sync::GraphFetchResult>>>,
    graph_search: Option<Box<dyn Fn(crate::server_search::GraphSearchJob) -> Receiver<mailcraft_model::sync::GraphSearchResult>>>,
}

/// A hash of what a mailbox's accounts connect to: each account's address, kind and servers (a JMAP
/// account's session address too, and how its secret goes), sorted, so the order in the file does
/// not matter. SHA-256, so that a file cannot be made to match one the owner confirmed with other
/// servers.
pub fn fingerprint(mailbox: &Mailbox) -> String {
    let server = |s: &Option<mailcraft_model::Server>| match s {
        Some(s) => format!("{}:{}:{:?}", s.host.trim().to_ascii_lowercase(), s.port, s.security),
        None => "-".to_string(),
    };
    // (Only for an account that has one: what every other account's line was stays as it was.)
    let jmap = |a: &Account| a.jmap.as_ref().map(|j| format!("\u{1}{}\u{1}{}", j.session.trim(), j.bearer)).unwrap_or_default();
    let mut lines: Vec<String> = mailbox
        .accounts
        .iter()
        .map(|a| {
            format!("{}\u{1}{:?}\u{1}{}\u{1}{}{}", a.email.trim().to_ascii_lowercase(), a.kind, server(&a.incoming), server(&a.outgoing), jmap(a))
        })
        .collect();
    lines.sort();
    // Each line behind its length: no two different lists make the same bytes.
    let bytes: Vec<u8> = lines.iter().flat_map(|l| format!("{}:{l}", l.len()).into_bytes()).collect();
    digest(&bytes)
}

/// SHA-256 in hex.
#[cfg(not(target_arch = "wasm32"))]
fn digest(bytes: &[u8]) -> String {
    mailcraft_engine::store::Digest::of(bytes).hex()
}

/// The browser build reaches no server, so nothing there is confirmed by a fingerprint: the text itself.
#[cfg(target_arch = "wasm32")]
fn digest(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A path as it is kept: the file's real name when it can be had.
pub(crate) fn kept_path(path: &str) -> String {
    std::fs::canonicalize(path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string())
}

/// What an account connects to, in a line.
fn line(a: &Account) -> String {
    format!("{} ({})", a.email.chars().take(80).collect::<String>(), mailcraft_engine::cmd::account::servers_text(a))
}

impl MailApp {
    /// Whether the open mailbox came from a file whose servers nobody has confirmed.
    pub fn is_unconfirmed(&self) -> bool {
        self.unconfirmed
    }

    /// Where each account of the open mailbox connects to, one line per account.
    pub fn account_servers(&self) -> Vec<String> {
        self.session.mailbox.accounts.iter().filter(|a| a.kind.has_server()).map(line).collect()
    }

    /// A mailbox has just taken the place of the one before: it is unconfirmed when it names
    /// servers and is not the owner's own, nor a file whose servers were confirmed as they are.
    pub(crate) fn judge_mailbox(&mut self) {
        self.unconfirmed = false;
        self.release_services();
        if !self.session.mailbox.accounts.iter().any(|a| a.kind.has_server()) {
            return;
        }
        if let Some(path) = &self.session.path {
            let path = kept_path(&path.to_string_lossy());
            if self.own_mailbox.as_deref() == Some(path.as_str()) {
                return;
            }
            let print = fingerprint(&self.session.mailbox);
            if self.ui.confirmed_mailboxes.iter().any(|c| c.path == path && c.fingerprint == print) {
                return;
            }
        }
        self.unconfirmed = true;
        self.hold_services();
    }

    fn hold_services(&mut self) {
        let s = &mut self.services;
        self.held = Held {
            sync: s.sync.take(),
            sign_in: s.sign_in.take(),
            graph: s.graph.take(),
            graph_fetch: s.graph_fetch.take(),
            graph_search: s.graph_search.take(),
        };
    }

    fn release_services(&mut self) {
        let held = std::mem::take(&mut self.held);
        let s = &mut self.services;
        s.sync = held.sync.or(s.sync.take());
        s.sign_in = held.sign_in.or(s.sign_in.take());
        s.graph = held.graph.or(s.graph.take());
        s.graph_fetch = held.graph_fetch.or(s.graph_fetch.take());
        s.graph_search = held.graph_search.or(s.graph_search.take());
    }

    /// `ui.confirmServers`: the owner has seen where every account connects and says yes. Kept
    /// for this file as its accounts are now.
    pub(crate) fn confirm_servers(&mut self) -> Result<Value, String> {
        if !self.unconfirmed {
            return Ok(json!({"confirmed": false}));
        }
        if let Some(path) = &self.session.path {
            let entry = ConfirmedMailbox { path: kept_path(&path.to_string_lossy()), fingerprint: fingerprint(&self.session.mailbox) };
            self.ui.confirmed_mailboxes.retain(|c| c.path != entry.path);
            self.ui.confirmed_mailboxes.push(entry);
            let over = self.ui.confirmed_mailboxes.len().saturating_sub(MAX_CONFIRMED);
            self.ui.confirmed_mailboxes.drain(..over);
        }
        self.unconfirmed = false;
        self.release_services();
        // A Send/Receive is due now rather than at the next interval.
        self.last_auto_sync = 0.0;
        Ok(json!({"confirmed": true, "accounts": self.account_servers()}))
    }

    /// What the owner is asked before the servers are confirmed.
    pub(crate) fn confirm_servers_question(&self) -> Question {
        let list = self.account_servers().join("\n");
        let text = format!(
            "{}\n\n{list}\n\n{}",
            tl!("This mailbox was opened from a file. Its accounts connect to:"),
            tl!("Only confirm if you know these servers.")
        );
        Question::new(tl!("Confirm Servers"), text, tl!("Confirm servers"), Value::Null)
    }

    /// A manual Send/Receive in an unconfirmed mailbox: ask to confirm the servers instead.
    pub(crate) fn ask_to_confirm_servers(&mut self) -> Result<Value, String> {
        if self.dialog.is_none() {
            let ask = self.confirm_servers_question();
            self.dialog = Some(crate::dialogs::Dialog::Confirm { command: "ui.confirmServers".into(), params: json!({}), ask });
        }
        Ok(json!({"pending": "confirmServers"}))
    }
}

/// The line across the window that says the mailbox came from a file, and where its accounts
/// connect to, with the button that confirms.
pub(crate) fn bar(app: &mut MailApp, ui: &mut Ui) {
    if !app.unconfirmed {
        return;
    }
    let servers: String = app.account_servers().join("; ");
    let servers = if servers.chars().count() > 110 { format!("{}…", servers.chars().take(110).collect::<String>()) } else { servers };
    let said = format!("{} {servers}", tl!("This mailbox was opened from a file. Its accounts connect to:"));
    crate::updates::bar(app, ui, "unconfirmed_servers", &said, &[("confirmServers", tl!("Confirm servers"), true)], |app, _| {
        let _ = app.run("ui.confirmServers", json!({}));
    });
}

#[cfg(test)]
#[path = "unconfirmed_tests.rs"]
mod tests;
