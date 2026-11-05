// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! IMAP mail fetched whole by its UID (#653): what mail to keep offline left as its headers and
//! preview here ([`super::offline`]) when the owner opens it, replies to it or forwards it, and
//! what a longer window covers again, in the background, newest first. The engine says what to
//! fetch ([`imap_fetch_request`], `sync.imapFetchRequest`) and takes in what came
//! ([`apply_imap_fetch`], `sync.applyImapFetch`); the host does the talking in between
//! (`mailcraft_net::fetch_imap`), with the password or the token the account's Send/Receive goes
//! with. A message is named by its mailbox, that mailbox's `UIDVALIDITY` and its UID there
//! ([`ImapFetchKey`]), so a copy Gmail keeps under another label is another message.

use std::collections::{HashMap, HashSet};

use mailcraft_model::sync::{GraphFetchResult, ImapFetchKey, ImapFetchRequest};
use mailcraft_model::{AccountKind, Id, Mailbox, OfflineWindow};
use mailcraft_time::Timestamp;
use serde_json::{Value, json};

use super::graph_down::FullCounts;
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.imapFetchRequest", "IMAP Fetch Request", "Send / Receive", fetch_request)
            .pure()
            .params(r#"{"account"?: number (an IMAP account; default the first), "message"?: number (fetch this one now), "most"?: number (default 10, at most 100)} — which messages of an IMAP account a host should fetch whole (#653): one the owner opened, or else the next ones within its mail to keep offline that are only headers and a preview here, newest first. Each is named "uidvalidity/uid/mailbox". The password or token is not in it: the host adds it. Answers {"account": {…}, "ids": [string], "background"?: true}"#),
        CommandSpec::new("sync.applyImapFetch", "Apply IMAP Fetch", "Send / Receive", apply_fetch)
            .params(r#"{"result": {"account": number, "email": string, "fetched"?: [{"id": string, "mime": [bytes]}], "missing"?: [string], "failed"?: [string], "errors"?: [string], "offline"?: bool}} — take in IMAP messages fetched whole: each one's text, attachments and headers from its MIME, where it is and its marks as they were. Not an undo step. Answers {"fetched", "missing", "failed", "waiting", "all", "errors", "offline"}"#),
    ]
}

/// The IMAP account a command names, or the first.
fn imap_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let id = p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Imap).map(|a| a.id))
        .ok_or_else(|| fail("no account is connected to an IMAP server"))?;
    let a = s.mailbox.account(id).ok_or_else(|| failed!("no account with id {id}", id))?;
    if a.kind != AccountKind::Imap {
        return Err(failed!("{name} isn't connected to an IMAP server", name = a.name));
    }
    Ok(id)
}

/// Which messages of `account` a host should fetch whole: `message` at once (the owner opened it,
/// or is replying to it), else the next `most` in the background.
pub fn imap_fetch_request(s: &Session, account: Id, message: Option<Id>, most: usize) -> Result<ImapFetchRequest, CmdError> {
    let a =
        s.mailbox.account(account).filter(|a| a.kind == AccountKind::Imap).ok_or_else(|| failed!("no IMAP account with id {id}", id = account))?;
    let ids = match message {
        Some(id) => {
            let m = s
                .mailbox
                .message(id)
                .filter(|m| m.account == account)
                .ok_or_else(|| failed!("no message with id {id} in {name}", id, name = a.name))?;
            s.mailbox.imap_key(m).filter(|_| m.unfetched).map(|key| vec![key.write()]).unwrap_or_default()
        }
        None => fetch_queue(&s.mailbox, account, most.clamp(1, 100), s.now(), &s.graph_unfetchable),
    };
    Ok(ImapFetchRequest { account: a.clone(), ids, background: message.is_none() })
}

/// The next `most` messages of `account` that are only their headers and preview here and within
/// its mail to keep offline at `now`, newest first, but those `passed_over` this session.
pub fn fetch_queue(mb: &Mailbox, account: Id, most: usize, now: Timestamp, passed_over: &HashSet<(Id, String)>) -> Vec<String> {
    let window = OfflineWindow::of(mb, now);
    let mut waiting: Vec<(Timestamp, String)> = mb
        .messages
        .values()
        .filter(|m| m.account == account && m.unfetched && window.within(m))
        .filter_map(|m| mb.imap_key(m).map(|key| (m.received, key.write())))
        .filter(|(_, key)| !passed_over.contains(&(account, key.clone())))
        .collect();
    waiting.sort_by_key(|(received, _)| std::cmp::Reverse(*received));
    waiting.truncate(most);
    waiting.into_iter().map(|(_, key)| key).collect()
}

/// How many messages of `account` from its server are kept in full here, fetched or waiting to
/// be, and how many are still to come: those within its mail to keep offline.
pub fn full_counts(mb: &Mailbox, account: Id, now: Timestamp) -> FullCounts {
    let window = OfflineWindow::of(mb, now);
    let mut counts = FullCounts::default();
    for m in mb.messages.values().filter(|m| m.account == account && m.uid != 0 && window.within(m)) {
        counts.all = counts.all.saturating_add(1);
        counts.waiting = counts.waiting.saturating_add(usize::from(m.unfetched));
    }
    counts
}

/// Take in IMAP messages fetched whole. Not an undo step.
pub fn apply_imap_fetch(s: &mut Session, result: GraphFetchResult) -> CmdResult {
    let account = result.account;
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind != AccountKind::Imap {
        return Err(failed!("{name} isn't connected to an IMAP server", name = a.name));
    }
    if !result.email.is_empty() && !a.email.trim().eq_ignore_ascii_case(result.email.trim()) {
        return Err(failed!("these messages were for another account, not for {name}", name = a.name));
    }
    if s.file_is_current() {
        s.note_saved();
    }
    // Only one that is still the server's message under its key, and still not whole: one moved,
    // numbered anew or fetched by other means meanwhile stays as it is.
    let by_key: HashMap<ImapFetchKey, Id> = if result.fetched.is_empty() {
        HashMap::new()
    } else {
        s.mailbox
            .messages
            .values()
            .filter(|m| m.account == account && m.unfetched)
            .filter_map(|m| s.mailbox.imap_key(m).map(|key| (key, m.id)))
            .collect()
    };
    let mut taken: Vec<Id> = Vec::new();
    for fetched in &result.fetched {
        let here = ImapFetchKey::read(&fetched.id).and_then(|key| by_key.get(&key).copied());
        if let Some(id) = here.filter(|id| !taken.contains(id))
            && let Ok(m) = s.mailbox.message_mut(id)
        {
            super::graph_down::take_mime(m, &fetched.mime);
            taken.push(id);
        }
    }
    // Older than its mail to keep offline (the owner opened it): whole until the mailbox is
    // opened again, as a Microsoft account's (#143).
    let window = OfflineWindow::of(&s.mailbox, s.now());
    let outside: Vec<Id> = taken.iter().copied().filter(|id| s.mailbox.message(*id).is_some_and(|m| !window.within(m))).collect();
    s.kept_for_now.extend(outside);
    // Refused although there, or numbered anew: not asked for again in the background this session.
    // Gone from the server: not asked for again either, until a Send/Receive takes it away here.
    s.graph_unfetchable.extend(result.failed.iter().chain(&result.missing).take(1_000).map(|id| (account, id.clone())));
    if !taken.is_empty() {
        s.mailbox_changed();
    }
    let now = s.now();
    let counts = full_counts(&s.mailbox, account, now);
    s.sync_progress.counted_as(&s.mailbox, account, counts, now);
    let FullCounts { all, waiting, large } = counts;
    Ok(
        json!({"fetched": taken.len(), "missing": result.missing.len(), "failed": result.failed.len(), "waiting": waiting, "large": large, "all": all, "errors": result.errors, "offline": result.offline}),
    )
}

fn fetch_request(s: &mut Session, v: &Value) -> CmdResult {
    let account = imap_account(s, v)?;
    let most = p::id(v, "most").and_then(|n| usize::try_from(n).ok()).unwrap_or(10);
    serde_json::to_value(imap_fetch_request(s, account, p::id(v, "message"), most)?).map_err(|e| fail(e.to_string()))
}

fn apply_fetch(s: &mut Session, v: &Value) -> CmdResult {
    let result: GraphFetchResult =
        serde_json::from_value(v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?).map_err(|e| bad(format!("`result`: {e}")))?;
    apply_imap_fetch(s, result)
}
