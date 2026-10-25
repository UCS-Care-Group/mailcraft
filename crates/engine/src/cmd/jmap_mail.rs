// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account kept in step over JMAP (RFC 8620, RFC 8621), M6.
//!
//! As for IMAP and Microsoft Graph, the engine never talks to a server: [`jmap_mail_request`]
//! (`sync.jmapRequest`) says what a run should read, a host that can reach the server does it
//! (`mailcraft_jmap::sync_mail`, over `fetch` in the browser), and [`apply_jmap_mail`]
//! (`sync.applyJmap`) takes in what it brought back, piece by piece. The same goes for messages
//! fetched whole: [`jmap_fetch_request`] (`sync.jmapFetchRequest`) and [`apply_jmap_fetch`]
//! (`sync.applyJmapFetch`). All four are the host's side of a synchronisation, never an agent's.
//!
//! **Reading only, for now.** A run lists the server's mailboxes and reads a window of each, its
//! newest messages; nothing made here goes up yet, and nothing is sent from such an account
//! ([`AccountKind::sends_mail`]). A mark made here stays here; mail the server has isn't moved or
//! deleted here until those go up ([`mailcraft_model::Mailbox::stays_on_server`]).
//!
//! **What comes back is hostile** (a host may hand over anything): every list and string of a
//! result is held to a size before it is taken in, ids are held to the JMAP alphabet (RFC 8620
//! §1.2), and what only a Graph delta says is passed over (see [`super::jmap_down`]). A result cut
//! that way removes nothing, as a run that went wrong removes nothing.
//!
//! **Taking in** goes the way of a Graph run, by server id, with the same pieces: the listing
//! makes and updates the folders ([`super::graph_mail`]); each window is a page of a folder's
//! messages ([`super::graph_down::take_page`]), and a window that holds all of its mailbox is
//! a complete listing of it ([`mailcraft_model::sync::MessagePage::afresh`]); at the end of a run
//! that went well, what the complete windows of that run left out is gone ([`jmap_down::sweep`]).
//! A mark the server changed comes down, but for one changed here since the server last said
//! (three ways, as for Graph). A folder whose mailbox the server no longer lists goes with its
//! mail, but not a standard folder, nor one holding mail only this computer has
//! ([`jmap_down::keep_what_is_only_here`]).

use std::collections::HashSet;

use mailcraft_model::sync::{GraphFetchResult, JmapFetchRequest, JmapMailRequest, JmapMailResult, JmapScope, MAX_JMAP_WINDOW, SyncPhase};
use mailcraft_model::{AccountKind, FolderKind, Id, JmapServer, Mailbox};
use serde_json::{Value, json};

use super::graph_down::{self, Down};
use super::graph_mail::{self, Taken};
use super::jmap_down;
use super::sync_progress::{self, Piece};
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.jmapRequest", "Send/Receive with JMAP", "Send / Receive", request)
            .pure()
            .params(r#"{"account"?: number, "light"?: bool (the Inbox and the folder on screen only), "background"?: bool} — for an account kept in step over JMAP: answers what a host should read there"#)
            .when(has_jmap_account),
        CommandSpec::new("sync.applyJmap", "Apply Send/Receive with JMAP", "Send / Receive", apply).pure().params(r#"{"result": JmapMailResult}"#),
        CommandSpec::new("sync.jmapFetchRequest", "Download JMAP Messages in Full", "Send / Receive", fetch_request)
            .pure()
            .params(r#"{"account"?: number, "message"?: number (one message, at once), "most"?: number (default 10)} — answers which messages of an account kept in step over JMAP a host should fetch whole: the one asked for, or the next ones in the background (the Inbox's newest that aren't here whole, within the account's Download messages in full)"#)
            .when(has_jmap_account),
        CommandSpec::new("sync.applyJmapFetch", "Apply Downloaded JMAP Messages", "Send / Receive", apply_fetch)
            .pure()
            .params(r#"{"result": GraphFetchResult}"#),
    ]
}

fn has_jmap_account(s: &Session) -> Option<&'static str> {
    if s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Jmap) { None } else { Some("no account is kept in step over JMAP") }
}

/// The JMAP account a command names, or the first there is.
fn named_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Jmap).map(|a| a.id))
        .ok_or_else(|| fail("no account is kept in step over JMAP"))
}

/// The account `account` and its server, when it is a JMAP account whose session address may be
/// used ([`JmapServer::check_session`]: a mailbox file can say anything).
fn jmap_account(mb: &Mailbox, account: Id) -> Result<(&mailcraft_model::Account, &JmapServer), CmdError> {
    let a = mb.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind != AccountKind::Jmap {
        return Err(failed!("{name} isn't kept in step over JMAP", name = a.name));
    }
    let server = a.jmap.as_ref().filter(|server| server.problem().is_none());
    let server = server.ok_or_else(|| failed!("{name} has no JMAP server address that can be used", name = a.name))?;
    Ok((a, server))
}

/// How many of a mailbox's newest messages a run reads: the owner's Messages to download per
/// folder, from 1 to [`MAX_JMAP_WINDOW`] (0, "all", is the most there is).
fn window_of(s: &Session) -> u32 {
    match s.prefs.sync_limit {
        0 => MAX_JMAP_WINDOW,
        n => n.min(MAX_JMAP_WINDOW),
    }
}

/// What a JMAP run of `account` should read. The host adds the secret (a password, or a bearer
/// token), which is never here.
///
/// - **Scope.** `light`: the Inbox and the folder on screen, for a run nobody waits on;
///   otherwise every mailbox ([`JmapScope::All`]).
/// - **The listing** of the server's mailboxes comes with a run that reads every mailbox, with
///   the first run of the account (none of its folders is the server's yet), and with any run
///   after a listing that was cut short.
/// - **Known.** For each mailbox read, the server ids of the messages here that the server last
///   said lie there, the newest first and no more than a window: those come with their marks
///   alone, the others whole.
/// - Nothing goes up: no change of a message and nothing to send.
pub fn jmap_mail_request(s: &Session, account: Id, light: bool) -> Result<JmapMailRequest, CmdError> {
    let (a, server) = jmap_account(&s.mailbox, account)?;
    let mb = &s.mailbox;
    let listed = mb.folders.iter().any(|f| f.account == account && f.server.is_some());
    let server_id = |folder: Id| mb.folder(folder).filter(|f| f.account == account).and_then(|f| f.server.as_ref()).map(|server| server.id.clone());
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).and_then(server_id);
    let scope = if light {
        // (The Inbox is read anyway.)
        let on_screen = server_id(s.view.folder).filter(|id| Some(id) != inbox.as_ref());
        JmapScope::Light { also: on_screen.into_iter().collect() }
    } else {
        JmapScope::All
    };
    let read: Option<Vec<String>> = match &scope {
        JmapScope::Light { also } => Some(inbox.iter().chain(also).cloned().collect()),
        JmapScope::All => None,
    };
    let window = window_of(s);
    Ok(JmapMailRequest {
        account,
        email: a.email.clone(),
        session: server.session.clone(),
        bearer: server.bearer,
        window,
        list_folders: !light || !listed || s.graph_listed_short.contains(&account),
        scope,
        known: jmap_down::known(mb, account, read.as_deref(), usize::try_from(window).unwrap_or(usize::MAX)),
        ..Default::default()
    })
}

fn request(s: &mut Session, v: &Value) -> CmdResult {
    let account = named_account(s, v)?;
    let mut request = jmap_mail_request(s, account, p::bool(v, "light").unwrap_or(false))?;
    // A run begins: what one that never came to its end read is no part of it.
    s.jmap_read.remove(&account);
    request.background = p::bool(v, "background").unwrap_or(false);
    serde_json::to_value(request).map_err(|e| fail(e.to_string()))
}

/// Which messages of `account` a host should fetch whole: `message` at once (the owner opened it,
/// or is replying to it), else the next `most` in the background: of the Inbox's newest, those
/// that aren't here whole, within the account's Download messages in full ([`jmap_down::fetch_queue`]).
pub fn jmap_fetch_request(s: &Session, account: Id, message: Option<Id>, most: usize) -> Result<JmapFetchRequest, CmdError> {
    let (a, server) = jmap_account(&s.mailbox, account)?;
    let ids = match message {
        Some(id) => {
            let m = s
                .mailbox
                .message(id)
                .filter(|m| m.account == account)
                .ok_or_else(|| failed!("no message with id {id} in {name}", id, name = a.name))?;
            m.server.as_ref().filter(|server| m.unfetched && jmap_down::is_id(&server.id)).map(|server| vec![server.id.clone()]).unwrap_or_default()
        }
        None => jmap_down::fetch_queue(&s.mailbox, account, most.clamp(1, 100), &s.graph_unfetchable),
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

fn fetch_request(s: &mut Session, v: &Value) -> CmdResult {
    let account = named_account(s, v)?;
    let most = p::id(v, "most").and_then(|n| usize::try_from(n).ok()).unwrap_or(10);
    serde_json::to_value(jmap_fetch_request(s, account, p::id(v, "message"), most)?).map_err(|e| fail(e.to_string()))
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    let result: JmapMailResult =
        serde_json::from_value(v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?).map_err(|e| bad(format!("`result`: {e}")))?;
    apply_jmap_mail(s, result)
}

fn apply_fetch(s: &mut Session, v: &Value) -> CmdResult {
    let result: GraphFetchResult =
        serde_json::from_value(v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?).map_err(|e| bad(format!("`result`: {e}")))?;
    apply_jmap_fetch(s, result)
}

/// The account a result is for, when it may take it in: a JMAP account, and the one the run was
/// for. (An answer for another account, the mailbox having been replaced while the run was under
/// way, would tie this one's folders to a stranger's server.)
fn receiving(mb: &Mailbox, account: Id, email: &str) -> Result<String, CmdError> {
    let a = mb.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind != AccountKind::Jmap {
        return Err(failed!("{name} isn't kept in step over JMAP", name = a.name));
    }
    if !email.is_empty() && !a.email.trim().eq_ignore_ascii_case(email.trim()) {
        return Err(failed!("this Send/Receive was for another account, not for {name}", name = a.name));
    }
    Ok(a.name.clone())
}

/// Take in a piece of what a JMAP run brought back (pieces come in the order the run made them,
/// each whole in itself): the listing, then the windows, and at the end of a run that went well,
/// the messages that complete windows left out are removed. Not an undo step: the server is the
/// source of truth, but for what was changed here since it last said.
///
/// The piece is held to sizes first ([`jmap_down::bounded`]); a piece that had to be cut, a run
/// that went wrong, was stopped, or follows a listing that was cut short, removes nothing.
///
/// Answers what it changed, with the keys a Graph run's answer has (`added`, `updated` and
/// `removed` folders; `new`, `changed`, `gone` and `removedMessages` messages; `errors`,
/// `busyFor`, `complete`, `summary`), and `authRefused` when the server turned the secret down.
pub fn apply_jmap_mail(s: &mut Session, result: JmapMailResult) -> CmdResult {
    let account = result.account;
    let name = receiving(&s.mailbox, account, &result.email)?;
    let result = jmap_down::bounded(result);
    let mut taken = Taken::default();
    let mut down = Down::default();
    if let Some(listing) = &result.listing {
        down.removed = jmap_down::keep_what_is_only_here(&mut s.mailbox, account, listing);
        graph_mail::take_listing(&mut s.mailbox, account, listing, &mut taken);
        if listing.complete {
            s.graph_listed_short.remove(&account);
        } else {
            s.graph_listed_short.insert(account);
        }
    }
    let mut index = std::mem::take(&mut s.graph_index);
    for page in &result.pages {
        // A mailbox that turns up twice in a run is read in part: neither of its windows is taken
        // as all of it, so that nothing in it goes for what one of them lacks.
        let read = s.jmap_read.entry(account).or_default();
        let first = !read.contains_key(&page.folder);
        read.insert(page.folder.clone(), first && page.afresh);
        jmap_down::take_window(&mut s.mailbox, &mut index, account, page, first, &mut down);
    }
    s.graph_index = index;
    // The run's end: what its complete windows left out goes, when it went well (and not after a
    // listing cut short: what left a mailbox may be in one it missed). Either way the next run
    // begins afresh.
    if !result.partial {
        let read: HashSet<String> =
            s.jmap_read.remove(&account).unwrap_or_default().into_iter().filter_map(|(mailbox, whole)| whole.then_some(mailbox)).collect();
        if result.whole && !s.graph_listed_short.contains(&account) {
            jmap_down::sweep(&mut s.mailbox, account, &read, &mut down);
        }
    }
    if taken != Taken::default() || down != Down::default() || !result.pages.is_empty() {
        s.mailbox_changed();
    }
    // The view stays on a folder that is there.
    if s.mailbox.folder(s.view.folder).is_none() {
        s.view.folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap_or(0);
        s.sel.messages.clear();
    }
    let lang = s.interface_language;
    let mut errors = result.errors.clone();
    if result.auth_refused {
        errors.insert(0, mailcraft_i18n::fmt(lang.t("The server turned down the sign-in for {name}: sign in again"), &[("name", &name)]));
    }
    // Send/Receive Progress: where the run is, what it brought, what went wrong.
    let step = match (&result.progress, &result.listing) {
        (Some(progress), _) => {
            let name = s.mailbox.folder_by_server(account, &progress.folder).and_then(|f| s.mailbox.folder(f)).map(|f| f.name.as_str());
            Some((SyncPhase::NewMail, sync_progress::shown(name.unwrap_or_default()), progress.read.min(progress.total), progress.total))
        }
        (None, Some(_)) => Some((SyncPhase::Folders, String::new(), 0, 0)),
        (None, None) => None,
    };
    let piece = Piece {
        step,
        folders: result.progress.as_ref().map(|p| (p.number, p.folders)),
        new: down.new,
        sent: 0,
        problems: &errors,
        last: !result.partial,
        stopped: result.stopped,
        timings: None,
    };
    s.sync_progress.take(account, piece);
    let summary = errors.first().cloned().unwrap_or_else(|| "Up to date".to_string());
    if result.partial {
        // Under way: how far it has got.
        if let Some(progress) = &result.progress {
            s.sync_status = graph_down::progress_words(&s.mailbox, account, progress, lang);
        }
    } else {
        let said = match errors.first() {
            Some(first) => first.clone(),
            None if result.stopped => lang.t("Stopped; carries on from here next time").to_string(),
            None => lang.t("Up to date").to_string(),
        };
        let t = s.local_now();
        s.sync_status = format!("{said} — {}", s.time_said(t));
    }
    Ok(json!({
        "pushed": 0,
        "added": taken.added,
        "updated": taken.updated,
        "removed": taken.removed,
        "errors": errors,
        "busyFor": result.busy_for,
        "authRefused": result.auth_refused,
        "stopped": result.stopped,
        "complete": result.listing.as_ref().map(|l| l.complete),
        "new": down.new,
        "changed": down.changed,
        "gone": down.gone,
        "removedMessages": down.removed,
        "summary": summary,
    }))
}

/// Take in messages of a JMAP account fetched whole, as a Microsoft account's are
/// ([`graph_mail::apply_graph_fetch`]), once the result is held to sizes ([`jmap_down::bounded_fetch`]).
/// Not an undo step.
///
/// A message the server said it no longer has stays as it is (its window says whether it is gone,
/// and a run that reads it whole removes it), and the background doesn't ask for it again this
/// session; nor for one the server wouldn't give, as for Graph.
pub fn apply_jmap_fetch(s: &mut Session, result: GraphFetchResult) -> CmdResult {
    let account = result.account;
    receiving(&s.mailbox, account, &result.email)?;
    let result = jmap_down::bounded_fetch(result);
    let missing: Vec<(Id, String)> = result.missing.iter().map(|id| (account, id.clone())).collect();
    let answer = graph_mail::apply_graph_fetch(s, result)?;
    s.graph_unfetchable.extend(missing);
    Ok(answer)
}

#[cfg(test)]
#[path = "jmap_mail_tests.rs"]
mod tests;
