//! An account kept in step with a JMAP server (RFC 8620, RFC 8621).
//!
//! As for IMAP and Graph, the engine never talks to a server: [`jmap_mail_request`]
//! (`sync.jmapRequest`) says what a run should do, a host does it (in a browser, over `fetch`),
//! and [`apply_jmap_mail`] (`sync.applyJmap`) takes in what it brought back. The request and the
//! answer are JMAP's own (`mailcraft_model::sync::JmapMailRequest`), beside the IMAP and Graph
//! ones rather than bent into them.
//!
//! **What a run reads.** A JMAP server says which messages a mailbox holds, newest first. This
//! client does not ask it what changed since the last run (`Email/changes`, which a server may also
//! refuse): a run reads each mailbox's newest messages, up to the window (File › Options, "Messages
//! to download per folder", 500 at most). A window that holds all of a mailbox is complete, and the
//! mailbox here is made the same as the server's; a window cut short adds and updates only
//! ([`super::jmap_down`]). The ids this mailbox already has go with the request, so that the host
//! asks the server only for the keywords of those, and for the headers of the others.
//!
//! **Folders** come down by server id as a Graph account's do ([`super::graph_mail`]): the
//! account's standard folders are tied to the server's by role, the others are made here, and a
//! complete listing removes what the server no longer has.

use std::collections::HashMap;

use mailcraft_model::sync::{JmapKnown, JmapMailRequest, JmapMailResult, JmapScope, MAX_JMAP_WINDOW, SyncPhase};
use mailcraft_model::time::Timestamp;
use mailcraft_model::{AccountKind, FolderKind, Id, Mailbox};
use serde_json::{Value, json};

use super::graph_down::{self, Down};
use super::graph_mail::{Taken, take_listing};
use super::jmap_down::{self, plausible_id};
use super::sync_progress::{self, Piece};
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// The most mailboxes whose known ids a request names, and the most ids it names in all.
const MAX_KNOWN_MAILBOXES: usize = 500;
const MAX_KNOWN_IDS: usize = 50_000;
/// The most mailboxes a light run reads besides the Inbox (the folder on screen, mostly).
const MAX_ALSO: usize = 8;
/// The most windows and error lines an answer is read for: what is past them waits for the next run.
const MAX_PAGES: usize = 1_000;
const MAX_ERRORS: usize = 50;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.jmapRequest", "Send/Receive with JMAP", "Send / Receive", request)
            .pure()
            .params(r#"{"account"?: number, "full"?: bool (every mailbox and the folder list; else the Inbox and "also"), "also"?: [string] (mailbox ids to read besides the Inbox), "background"?: bool} — for an account kept in step with a JMAP server: answers what a host should read and change there"#)
            .when(has_jmap_account),
        CommandSpec::new("sync.applyJmap", "Apply Send/Receive with JMAP", "Send / Receive", apply)
            .pure()
            .params(r#"{"result": JmapMailResult}"#),
    ]
}

fn has_jmap_account(s: &Session) -> Option<&'static str> {
    if s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Jmap) { None } else { Some("no account is kept in step with a JMAP server") }
}

/// What a run of `account` should do. `full`: every mailbox, and the folder list; else the Inbox
/// and the mailboxes named in `also` (the folder on screen). The folder list is also read when no
/// folder of the account is tied to the server yet. The host adds the credential.
pub fn jmap_mail_request(s: &Session, account: Id, full: bool, also: &[String], background: bool) -> Result<JmapMailRequest, CmdError> {
    let a = s.mailbox.account(account).ok_or_else(|| fail(format!("no account with id {account}")))?;
    if a.kind != AccountKind::Jmap {
        return Err(fail(format!("{} isn't kept in step with a JMAP server", a.name)));
    }
    let server = a.jmap.as_ref().ok_or_else(|| fail(format!("{} has no JMAP server", a.name)))?;
    // The mailbox file may say anything: the address is looked at again here.
    if let Some(why) = server.problem() {
        return Err(fail(format!("{}'s JMAP server can't be reached: {why}", a.name)));
    }
    let window = match s.prefs.sync_limit {
        0 => MAX_JMAP_WINDOW,
        n => n.min(MAX_JMAP_WINDOW),
    };
    let tied = s.mailbox.folders.iter().any(|f| f.account == account && f.server.is_some());
    let scope = if full || !tied {
        JmapScope::All
    } else {
        let mut also: Vec<String> = also.iter().filter(|id| plausible_id(id)).take(MAX_ALSO).cloned().collect();
        also.dedup();
        JmapScope::Light { also }
    };
    Ok(JmapMailRequest {
        account,
        email: a.email.clone(),
        session: server.session.clone(),
        bearer: server.bearer,
        window,
        list_folders: full || !tied,
        scope,
        known: known(&s.mailbox, account, window),
        // What was changed here goes up from a later step on; nothing is sent from here yet.
        changes: Vec::new(),
        sends: Vec::new(),
        background,
    })
}

/// The server ids this mailbox has of `account`'s messages, by mailbox, newest first: at most
/// `window` a mailbox, so that the host asks the server for the keywords alone of those.
fn known(mb: &Mailbox, account: Id, window: u32) -> Vec<JmapKnown> {
    let mut by_mailbox: HashMap<&str, Vec<(Timestamp, &str)>> = HashMap::new();
    for m in mb.messages.values().filter(|m| m.account == account) {
        if let Some(s) = m.server.as_ref().filter(|s| !s.gone && plausible_id(&s.id) && plausible_id(&s.folder)) {
            by_mailbox.entry(s.folder.as_str()).or_default().push((m.received, s.id.as_str()));
        }
    }
    let mut mailboxes: Vec<(&str, Vec<(Timestamp, &str)>)> = by_mailbox.into_iter().collect();
    mailboxes.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let per = usize::try_from(window).unwrap_or(usize::MAX);
    let mut named = 0usize;
    let mut out = Vec::new();
    for (mailbox, mut ids) in mailboxes.into_iter().take(MAX_KNOWN_MAILBOXES) {
        ids.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        let room = MAX_KNOWN_IDS.saturating_sub(named).min(per);
        let ids: Vec<String> = ids.into_iter().take(room).map(|(_, id)| id.to_string()).collect();
        named = named.saturating_add(ids.len());
        if !ids.is_empty() {
            out.push(JmapKnown { mailbox: mailbox.to_string(), ids });
        }
    }
    out
}

/// How many changes made in `account` its server hasn't heard of yet. None go up from here yet.
pub fn unpushed(_s: &Session, _account: Id) -> usize {
    0
}

fn request(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Jmap).map(|a| a.id))
        .ok_or_else(|| fail("no account is kept in step with a JMAP server"))?;
    let also: Vec<String> = match v.get("also") {
        Some(Value::Array(ids)) => ids.iter().take(MAX_ALSO).filter_map(|id| id.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    let full = v.get("full").and_then(Value::as_bool).unwrap_or(false);
    let background = v.get("background").and_then(Value::as_bool).unwrap_or(false);
    serde_json::to_value(jmap_mail_request(s, account, full, &also, background)?).map_err(|e| fail(e.to_string()))
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    let result: JmapMailResult =
        serde_json::from_value(v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?).map_err(|e| bad(format!("`result`: {e}")))?;
    apply_jmap_mail(s, result)
}

/// A line a server sent, as it is shown: one line, no control characters, not too long.
fn said(line: &str) -> String {
    line.chars().map(|c| if c.is_control() { ' ' } else { c }).take(300).collect::<String>().trim().to_string()
}

/// Take in what a JMAP run brought back: the folder list, then each mailbox's window. Not an undo
/// step: the server is the source of truth, but for what was changed here and hasn't reached it.
pub fn apply_jmap_mail(s: &mut Session, result: JmapMailResult) -> CmdResult {
    let account = result.account;
    let a = s.mailbox.account(account).ok_or_else(|| fail(format!("no account with id {account}")))?;
    if a.kind != AccountKind::Jmap {
        return Err(fail(format!("{} isn't kept in step with a JMAP server", a.name)));
    }
    // An answer for another account (the mailbox was replaced while the run was under way) would
    // tie this one's folders to a stranger's server.
    if !result.email.is_empty() && !a.email.trim().eq_ignore_ascii_case(result.email.trim()) {
        return Err(fail(format!("this Send/Receive was for another account, not for {}", a.name)));
    }
    if s.file_is_current() {
        s.note_saved();
    }
    let mut taken = Taken::default();
    let mut errors: Vec<String> = result.errors.iter().take(MAX_ERRORS).map(|e| said(e)).filter(|e| !e.is_empty()).collect();
    if result.auth_refused && !result.partial {
        errors.push("The server refused the sign-in".to_string());
    }
    if let Some(listing) = &result.listing {
        take_listing(&mut s.mailbox, account, listing, &mut taken);
    }
    let mut down = Down::default();
    let mut index = std::mem::take(&mut s.graph_index);
    for page in result.pages.iter().take(MAX_PAGES) {
        jmap_down::take_window(&mut s.mailbox, &mut index, account, page, &mut down);
    }
    s.graph_index = index;
    // Every window was read to its end: what is still gone is gone here too.
    if result.whole {
        graph_down::sweep(&mut s.mailbox, account, &mut down);
    }
    if taken != Taken::default() || down != Down::default() || !result.pages.is_empty() {
        s.mailbox_changed();
    }
    // The view stays on a folder that is there.
    if s.mailbox.folder(s.view.folder).is_none() {
        s.view.folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap_or(0);
        s.sel.messages.clear();
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
    let folders = result.progress.as_ref().map(|p| (p.number, p.folders));
    let piece = Piece { step, folders, new: down.new, sent: 0, problems: &errors, last: !result.partial, stopped: result.stopped };
    s.sync_progress.take(account, piece);
    let summary = match (errors.first(), taken.pushed + taken.added + taken.updated + taken.removed + down.new + down.changed + down.removed) {
        (Some(e), _) => e.clone(),
        (None, 0) => "Up to date".to_string(),
        (None, _) => "Folders up to date".to_string(),
    };
    match (&result.progress, result.partial) {
        // Under way: how far it has got.
        (Some(progress), true) => s.sync_status = graph_down::progress_words(&s.mailbox, account, progress),
        (_, true) => {}
        (_, false) => {
            let t = s.local_now();
            let said = if result.stopped && errors.is_empty() { "Stopped; carries on from here next time".to_string() } else { summary.clone() };
            s.sync_status = format!("{said} — {}", t.time_short(s.prefs.twelve_hour));
        }
    }
    Ok(json!({
        "pushed": taken.pushed,
        "added": taken.added,
        "updated": taken.updated,
        "removed": taken.removed,
        "errors": errors,
        "busyFor": result.busy_for,
        "complete": result.listing.as_ref().map(|l| l.complete),
        "new": down.new,
        "changed": down.changed,
        "gone": down.gone,
        "removedMessages": down.removed,
        "summary": summary,
        "authRefused": result.auth_refused,
    }))
}

#[cfg(test)]
#[path = "jmap_mail_tests.rs"]
mod tests;
