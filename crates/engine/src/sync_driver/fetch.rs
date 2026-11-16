// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fetching messages whole (Microsoft Graph, JMAP, Exchange servers), the decisions and the state,
//! with no I/O: which commands need a message whole before they run and which messages a forward
//! takes, which of them are still to fetch, what a batch brought of each, how long an account
//! whose batches stop is left alone, how far the background has got, and how a result is taken
//! in. A front end hands the host a fetch, and feeds what came back to [`apply_fetch`] and
//! [`FetchDriver::background_came`].

use std::collections::HashSet;

use mailcraft_model::sync::GraphFetchResult;
use mailcraft_model::{AccountKind, Id};
use serde_json::Value;

use super::{FETCH_IDLE_MS, fetch_backoff_ms};
use crate::Session;
use crate::cmd::{compose, ews_mail, graph_mail, jmap_mail};

/// Most messages one fetch asks for (and one forward takes at a time).
pub const BATCH: usize = 10;

/// What the driver remembers of fetching messages whole.
#[derive(Debug, Default)]
pub struct FetchDriver {
    /// The background looks again at this time (milliseconds), not before.
    pub idle_until: f64,
    /// The last try couldn't reach the server.
    pub offline: bool,
    /// Accounts whose background batches stopped with nothing fetched, one after another: how
    /// many times, and until when (milliseconds) the account is left alone. Each wait is twice
    /// the one before, and the other accounts go on meanwhile.
    pub stuck: std::collections::HashMap<Id, (u32, f64)>,
    /// The command that waits for messages to be fetched whole ([`super::fetch_flow`]).
    pub then: Option<super::fetch_flow::Waiting>,
    /// A command whose messages came while a form with changes that aren't saved was open: it
    /// goes on once that form is closed (#513).
    pub held: Option<super::fetch_flow::Waiting>,
    /// While a forward of several runs again once messages came: those fetched for it already,
    /// not asked for again.
    pub tried: Option<Vec<Id>>,
    /// The message last asked for when it was opened (not asked again while it stays open).
    pub opened: Option<Id>,
}

/// How far the background fetch has got, as the engine said after the last batch taken in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FetchProgress {
    /// How many messages are fetched.
    pub done: u64,
    /// Of how many.
    pub all: u64,
    /// How many of those still to come are too large to fetch in the background.
    pub large: u64,
    /// How many are still to come, large ones included.
    pub waiting: u64,
}

impl FetchProgress {
    /// Read from what `apply_fetch` said.
    pub fn of(said: &Value) -> FetchProgress {
        let get = |key: &str| said.get(key).and_then(Value::as_u64).unwrap_or(0);
        let (waiting, all) = (get("waiting"), get("all"));
        FetchProgress { done: all.saturating_sub(waiting), all, large: get("large"), waiting }
    }

    /// How many of those still to come can be fetched in the background.
    pub fn background(&self) -> u64 {
        self.waiting.saturating_sub(self.large)
    }
}

/// What the driver says of a background batch that stopped, for the log.
#[derive(Clone, Debug, PartialEq)]
pub struct Stopped {
    /// How long the account is left alone now (milliseconds).
    pub wait_ms: f64,
    /// Whether this is the first time in a row: said once, not every time it tries again.
    pub first: bool,
}

impl FetchDriver {
    /// Something new may be waiting: the background looks again at once.
    pub fn wake(&mut self) {
        self.idle_until = 0.0;
    }

    /// A Send/Receive started: the background may go on, and the waits start afresh.
    pub fn resume(&mut self) {
        self.idle_until = 0.0;
        self.stuck.clear();
    }

    /// Another mailbox took the place of the one these were for.
    pub fn forget(&mut self) {
        self.stuck.clear();
        (self.idle_until, self.offline) = (0.0, false);
        (self.then, self.held, self.tried, self.opened) = (None, None, None, None);
    }

    /// Whether the background is to wait at `now`: it found nothing to fetch a moment ago.
    pub fn resting(&self, now: f64) -> bool {
        now < self.idle_until
    }

    /// Whether `account` is left alone at `now`, its batches having stopped.
    pub fn account_waits(&self, account: Id, now: f64) -> bool {
        self.stuck.get(&account).is_some_and(|(_, until)| now < *until)
    }

    /// No account had anything to fetch at `now`: the background rests a while.
    pub fn found_nothing(&mut self, now: f64) {
        self.idle_until = now + FETCH_IDLE_MS;
    }

    /// A background batch of `account` came back at `now`. `paused`: the owner stopped the
    /// background (no server's doing: nothing to wait out). Whether it stopped, and for how long
    /// the account is left alone if so.
    pub fn background_came(&mut self, account: Id, paused: bool, result: &GraphFetchResult, now: f64) -> Option<Stopped> {
        self.offline = result.offline;
        let stuck = !paused && (result.offline || result.busy_for.is_some() || (result.fetched.is_empty() && !result.errors.is_empty()));
        if stuck {
            // Not now: this account is tried again in a while, longer each time it stops again
            // (a Send/Receive renews the token too, and starts the waits afresh); the others go on.
            let times = self.stuck.get(&account).map_or(0, |(times, _)| *times);
            let wait_ms = fetch_backoff_ms(times).max(f64::from(result.busy_for.unwrap_or(0)) * 1_000.0);
            self.stuck.insert(account, (times.saturating_add(1), now + wait_ms));
            return Some(Stopped { wait_ms, first: times == 0 });
        }
        if !result.fetched.is_empty() || !result.missing.is_empty() || !result.failed.is_empty() {
            self.stuck.remove(&account);
        }
        None
    }
}

/// The commands other than a forward that need a message whole, and the message they are about.
pub fn needs_whole(s: &Session, id: &str, params: &Value) -> Option<Id> {
    if !matches!(
        id,
        "mail.reply"
            | "mail.replyAll"
            | "mail.replyAndDelete"
            | "mail.editDraft"
            | "mail.resend"
            | "file.exportMessage"
            | "mail.saveAttachment"
            | "mail.saveAllAttachments"
            | "mail.openAttachment"
            | "mail.viewSource"
            | "compose.attachItem"
    ) {
        return None;
    }
    let selected = match s.sel.messages.as_slice() {
        [one] => Some(*one),
        _ => None,
    };
    // Read as the command reads it (#553): `7.0` is message 7, and a message named wrongly is
    // refused by the command, so nothing is fetched for it.
    use crate::p;
    let named = match id {
        "mail.reply" | "mail.replyAll" | "mail.replyAndDelete" | "mail.editDraft" | "mail.resend" => p::message_strict(params).ok()?,
        "mail.viewSource" | "mail.saveAllAttachments" | "compose.attachItem" => p::id_strict(params, "id").ok()?,
        _ => p::message_strict(params).ok()?,
    };
    to_fetch(s, id, named.or(selected)?)
}

/// `message`, when command `id` waits for it to be fetched whole first.
pub fn to_fetch(s: &Session, id: &str, message: Id) -> Option<Id> {
    // One whose server's copy went before it could be fetched never will be: Forward as Attachment
    // says so itself (the others have no word of their own for it, and wait as they did).
    let never = |m: &mailcraft_model::Message| m.incomplete && id == "mail.forwardAsAttachment";
    s.mailbox.message(message).filter(|m| m.unfetched && !never(m) && m.server.is_some()).map(|m| m.id)
}

/// The messages a forward takes, as the engine takes them, and the one in focus. `None` for
/// another command, and for what the engine refuses (it says why once the command runs).
pub fn forwarded(s: &mut Session, id: &str, params: &Value) -> Option<(Vec<Id>, Id)> {
    if !matches!(id, "mail.forward" | "mail.forwardAsAttachment") {
        return None;
    }
    compose::forwarded(s, params).ok()
}

/// What a forward that waits for its messages does, as [`forwarded`] named it: the messages it
/// takes, the one in focus, and whether what is being written is let go. (With several, Forward
/// and Forward as Attachment do the same.) `None` for another command.
pub fn forward_of(command: &str, params: &Value) -> Option<(Vec<Id>, Option<Id>, bool)> {
    if !matches!(command, "mail.forward" | "mail.forwardAsAttachment") {
        return None;
    }
    let mut messages: Vec<Id> = params.get("ids")?.as_array()?.iter().filter_map(Value::as_u64).collect();
    let focus = messages.last().copied();
    messages.sort_unstable();
    Some((messages, focus, params.get("discard").and_then(Value::as_bool) == Some(true)))
}

/// Of `ids`, those still to fetch first, but `tried`: of the first one's account, at most
/// [`BATCH`]. A Microsoft account's server doesn't say how large its messages are before they come
/// (a delta gives no sizes), so a forward of more than 64 MB together is found out after at most
/// that many more have come.
pub fn still_to_fetch(s: &Session, ids: &[Id], tried: &[Id]) -> Vec<Id> {
    let waiting = |m: &&mailcraft_model::Message| m.unfetched && m.server.is_some() && !tried.contains(&m.id);
    let mut waiting = ids.iter().filter_map(|id| s.mailbox.message(*id)).filter(waiting);
    let Some(first) = waiting.next() else { return Vec::new() };
    let account = first.account;
    std::iter::once(first.id).chain(waiting.filter(|m| m.account == account).map(|m| m.id)).take(BATCH).collect()
}

/// Take in messages fetched whole, as their account's kind is taken in (a JMAP account's are held
/// to sizes first).
pub fn apply_fetch(s: &mut Session, result: GraphFetchResult) -> Result<Value, crate::CmdError> {
    let kind = s.mailbox.account(result.account).map(|a| a.kind);
    if kind == Some(AccountKind::Jmap) {
        jmap_mail::apply_jmap_fetch(s, result)
    } else if kind == Some(AccountKind::Exchange) {
        ews_mail::apply_ews_fetch(s, result)
    } else {
        graph_mail::apply_graph_fetch(s, result)
    }
}

/// What a fetch of messages for the owner brought of each of them: those it answered for (whole,
/// gone or refused), those it sent whole, the first of them, in the order asked, that it didn't
/// send (`true`: gone from the server), and the server's own word on why it stopped, if it said
/// one. One it didn't answer for (the server busy, or a host that stops at a batch's size) is
/// asked for again; one no longer here (deleted meanwhile) is nothing to wait for, and the engine
/// says so when the command runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Came {
    /// Those answered for.
    pub answered: Vec<Id>,
    /// Those sent whole.
    pub sent: Vec<Id>,
    /// The first that wasn't sent, and whether it is gone from the server.
    pub not_sent: Option<(Id, bool)>,
    /// The server's own word on why it stopped.
    pub why: Option<String>,
}

impl Came {
    /// What `result` brought of `messages`.
    pub fn of(s: &Session, messages: &[Id], result: &GraphFetchResult) -> Came {
        let fetched: HashSet<&str> = result.fetched.iter().map(|f| f.id.as_str()).collect();
        let missing: HashSet<&str> = result.missing.iter().map(String::as_str).collect();
        let failed: HashSet<&str> = result.failed.iter().map(String::as_str).collect();
        let mut came = Came { why: result.errors.first().cloned(), ..Default::default() };
        for m in messages {
            let Some(on_server) = s.mailbox.message(*m).and_then(|m| m.server.as_ref()).map(|s| s.id.as_str()) else {
                came.answered.push(*m);
                continue;
            };
            if missing.contains(on_server) || failed.contains(on_server) {
                came.not_sent = came.not_sent.or(Some((*m, missing.contains(on_server))));
            } else if fetched.contains(on_server) {
                came.sent.push(*m);
            } else {
                continue;
            }
            came.answered.push(*m);
        }
        came
    }
}
