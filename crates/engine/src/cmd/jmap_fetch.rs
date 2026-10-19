//! Messages of a JMAP account fetched whole (RFC 8621 §4.1: an email's raw octets are its blob).
//!
//! As for a Microsoft account's ([`super::graph_mail`]), the engine fetches nothing:
//! [`jmap_fetch_request`] (`sync.jmapFetchRequest`) says which messages a host should fetch, the host
//! downloads them, and [`apply_jmap_fetch`] (`sync.applyJmapFetch`) takes in what it brought. What it
//! brings is the same [`GraphFetchResult`] a Graph fetch brings, so that the window takes in both alike;
//! what is asked is a [`JmapFetchRequest`] of its own, which also says where the server is.
//!
//! - **At once**, for the message the owner opens or replies to: that one message, whatever its size (the host's
//!   own limit aside), ahead of the background.
//! - **In the background**, for the Inbox's newest [`PREFETCH_NEWEST`] messages and no others: the
//!   whole mailbox is in memory in a browser, so the rest is fetched when it is opened. Only what is
//!   within the account's Download messages in full, not gone, and not passed over this session.
//! - **What the server no longer has** (a message another client moved or deleted: some servers name a
//!   message by where it is) is passed over by the background for the session, so that it is not asked
//!   for again and again until the next run says what became of it. Opening it still asks.

use std::cmp::Reverse;

use mailcraft_model::sync::{GraphFetchResult, JmapFetchRequest};
use mailcraft_model::{AccountKind, FolderKind, Id, Message};
use serde_json::{Value, json};

use super::graph_mail::apply_graph_fetch;
use super::jmap_down::plausible_id;
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// How many of the Inbox's newest messages the background fetches whole.
pub const PREFETCH_NEWEST: usize = 40;
/// The most messages one request names.
const MOST: usize = 100;
/// The most messages one answer has the background pass over for the session.
const MAX_PASSED_OVER: usize = 1_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.jmapFetchRequest", "Download Messages in Full with JMAP", "Send / Receive", fetch_request)
            .pure()
            .params(r#"{"account"?: number, "message"?: number (one message, at once), "most"?: number (default 10)} — for an account kept in step with a JMAP server: answers which messages a host should fetch whole, and from where: the one asked for, or the next ones in the background (the Inbox's newest, not fetched, within the account's Download messages in full)"#)
            .when(has_jmap_account),
        CommandSpec::new("sync.applyJmapFetch", "Apply Downloaded Messages with JMAP", "Send / Receive", apply_fetch)
            .pure()
            .params(r#"{"result": GraphFetchResult}"#),
    ]
}

fn has_jmap_account(s: &Session) -> Option<&'static str> {
    if s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Jmap) { None } else { Some("no account is kept in step with a JMAP server") }
}

/// Which messages of `account` a host should fetch whole: `message` at once (the owner opened it, or is
/// replying to it), else the next `most` in the background. The host adds the secret.
pub fn jmap_fetch_request(s: &Session, account: Id, message: Option<Id>, most: usize) -> Result<JmapFetchRequest, CmdError> {
    let a = s.mailbox.account(account).ok_or_else(|| fail(format!("no account with id {account}")))?;
    if a.kind != AccountKind::Jmap {
        return Err(fail(format!("{} isn't kept in step with a JMAP server", a.name)));
    }
    let server = a.jmap.as_ref().ok_or_else(|| fail(format!("{} has no JMAP server", a.name)))?;
    // The mailbox file may say anything: the address is looked at again here.
    if let Some(why) = server.problem() {
        return Err(fail(format!("{}'s JMAP server can't be reached: {why}", a.name)));
    }
    let ids = match message {
        Some(id) => {
            let m = s.mailbox.message(id).filter(|m| m.account == account).ok_or_else(|| fail(format!("no message with id {id} in {}", a.name)))?;
            // One whose id this account doesn't have yet (a moved message waiting to be found again) can't be asked for.
            m.server.as_ref().filter(|server| m.unfetched && plausible_id(&server.id)).map(|server| vec![server.id.clone()]).unwrap_or_default()
        }
        None => queue(s, account, most.clamp(1, MOST)),
    };
    Ok(JmapFetchRequest {
        account,
        email: a.email.clone(),
        session: server.session.clone(),
        bearer: server.bearer,
        ids,
        background: message.is_none(),
    })
}

/// Where a message the background looks after stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// It is here whole.
    Whole,
    /// It waits to be fetched in the background.
    Waiting,
    /// It is larger than the account's Download messages in full: it waits until it is opened.
    Large,
}

/// The messages the background looks after: the Inbox's newest [`PREFETCH_NEWEST`] that the server still
/// has and that something can still be done for, newest first, each with its server id and where it
/// stands. A message the server wouldn't give this session, or whose copy was gone before it could be
/// fetched, is left out: nothing fetches it by itself.
fn scope(s: &Session, account: Id) -> Vec<(Standing, &str)> {
    let mb = &s.mailbox;
    let (Some(inbox), Some(a)) = (mb.folder_of_kind(account, FolderKind::Inbox), mb.account(account)) else { return Vec::new() };
    let limit = a.download_in_full.limit();
    let mut newest: Vec<&Message> = mb.messages_in(inbox).map(|m| m.as_ref()).collect();
    newest.sort_unstable_by_key(|m| (Reverse(m.received), m.id));
    newest.truncate(PREFETCH_NEWEST);
    newest
        .into_iter()
        .filter_map(|m| {
            let server = m.server.as_ref().filter(|server| !server.gone && plausible_id(&server.id))?;
            if !m.unfetched {
                return Some((Standing::Whole, server.id.as_str()));
            }
            if m.incomplete || s.graph_unfetchable.contains(&(account, server.id.clone())) {
                return None;
            }
            let large = limit.is_some_and(|limit| server.size > limit);
            Some((if large { Standing::Large } else { Standing::Waiting }, server.id.as_str()))
        })
        .collect()
}

/// The next `most` messages to fetch in the background: those of [`scope`] that wait, newest first.
fn queue(s: &Session, account: Id, most: usize) -> Vec<String> {
    scope(s, account).into_iter().filter(|(standing, _)| *standing == Standing::Waiting).take(most).map(|(_, id)| id.to_string()).collect()
}

/// Take in messages a host fetched whole, with the intake a Microsoft account's have
/// ([`apply_graph_fetch`]) and what is a JMAP server's own: the messages it no longer has are passed over
/// by the background for the session (opening one still asks), and the counts in the answer are those of
/// what the background looks after (the Inbox's newest), not of every message the account has. Not an
/// undo step.
pub fn apply_jmap_fetch(s: &mut Session, result: GraphFetchResult) -> CmdResult {
    let account = result.account;
    let a = s.mailbox.account(account).ok_or_else(|| fail(format!("no account with id {account}")))?;
    if a.kind != AccountKind::Jmap {
        return Err(fail(format!("{} isn't kept in step with a JMAP server", a.name)));
    }
    let gone: Vec<String> = result.missing.iter().filter(|id| plausible_id(id)).take(MAX_PASSED_OVER).cloned().collect();
    let mut said = apply_graph_fetch(s, result)?;
    // (Only an answer that was taken in: one for another account's address was refused above.)
    s.graph_unfetchable.extend(gone.into_iter().map(|id| (account, id)));
    let standings = scope(s, account);
    let count = |wanted: &[Standing]| standings.iter().filter(|(standing, _)| wanted.contains(standing)).count();
    let (waiting, large) = (count(&[Standing::Waiting, Standing::Large]), count(&[Standing::Large]));
    if let Some(counts) = said.as_object_mut() {
        counts.insert("waiting".into(), json!(waiting));
        counts.insert("large".into(), json!(large));
        counts.insert("all".into(), json!(standings.len()));
    }
    Ok(said)
}

fn fetch_request(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Jmap).map(|a| a.id))
        .ok_or_else(|| fail("no account is kept in step with a JMAP server"))?;
    let most = p::id(v, "most").and_then(|n| usize::try_from(n).ok()).unwrap_or(10);
    serde_json::to_value(jmap_fetch_request(s, account, p::id(v, "message"), most)?).map_err(|e| fail(e.to_string()))
}

fn apply_fetch(s: &mut Session, v: &Value) -> CmdResult {
    let result: GraphFetchResult =
        serde_json::from_value(v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?).map_err(|e| bad(format!("`result`: {e}")))?;
    apply_jmap_fetch(s, result)
}

#[cfg(test)]
#[path = "jmap_fetch_tests.rs"]
mod tests;
