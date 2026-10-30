// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account kept in step through Microsoft Graph (R3; ADR 0002, ADR 0003).
//!
//! As for IMAP, the engine never talks to a server: [`graph_mail_request`] (`sync.graphRequest`)
//! says what a run should do, a host with sockets does it (`mailcraft_net::graph::sync_folders`,
//! on a background thread), and [`apply_graph_mail`] (`sync.applyGraph`) takes in what it brought
//! back. The two are Graph's own (`mailcraft_model::sync::GraphMailRequest`), beside the IMAP ones
//! rather than bent into them.
//!
//! **Folders (R3.1).** A folder the server has is known by its server id ([`ServerFolder`]),
//! which stays the same when it is renamed or moved. What is changed here is the difference
//! between a folder and what the server last said of it: a name that differs is a rename, a
//! parent that differs a move (Delete Folder is a move into Deleted Items, as Outlook does), a
//! folder with no server id one to make. Only a folder deleted for good waits in a queue
//! ([`mailcraft_model::Queued`]). What comes back is taken in by server id: the server's name and
//! place win, but for what was changed here and hasn't reached it yet; a folder that a complete
//! listing doesn't have is removed with its mail, which the server no longer has either, and a
//! listing that was cut short removes nothing (R3 spec S6–S8).

use std::collections::{HashMap, HashSet};

use mailcraft_model::sync::{
    ChangeOutcome, FolderChange, FolderListing, GraphFetchRequest, GraphFetchResult, GraphFolder, GraphLink, GraphMailRequest, GraphMailResult,
    MAX_DRAFTS, MAX_FOLDER_CHANGES, MAX_MESSAGE_CHANGES, MAX_PLACES, MessageOutcome, SendOutcome, SyncPhase,
};
use mailcraft_model::{AccountKind, Folder, FolderKind, Id, Mailbox, Queued, ServerFolder, clean_name};
use serde_json::{Value, json};

use super::graph_down::{self, Down};
use super::graph_up::{self, Refusals};
use super::sync_progress::{self, Piece};
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.graphRequest", "Send/Receive with Microsoft Graph", "Send / Receive", request)
            .pure()
            .params(r#"{"account"?: number} — for an account kept in step through Microsoft Graph: answers what a host should change and read there"#)
            .when(has_graph_account),
        CommandSpec::new("sync.applyGraph", "Apply Send/Receive with Microsoft Graph", "Send / Receive", apply)
            .pure()
            .params(r#"{"result": GraphMailResult}"#),
        CommandSpec::new("sync.graphFetchRequest", "Download Messages in Full", "Send / Receive", fetch_request)
            .pure()
            .params(r#"{"account"?: number, "message"?: number (one message, at once), "most"?: number (default 10)} — answers which messages a host should fetch whole: the one asked for, or the next ones in the background (not fetched, within the account's Download messages in full, newest first)"#)
            .when(has_graph_account),
        CommandSpec::new("sync.applyGraphFetch", "Apply Downloaded Messages", "Send / Receive", apply_fetch)
            .pure()
            .params(r#"{"result": GraphFetchResult}"#),
    ]
}

fn has_graph_account(s: &Session) -> Option<&'static str> {
    if s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Microsoft) {
        None
    } else {
        Some("no account is kept in step through Microsoft Graph")
    }
}

/// What a Graph run of `account` should do: the folder changes made here go up, and every folder
/// is listed. The host adds the token.
pub fn graph_mail_request(s: &Session, account: Id) -> Result<GraphMailRequest, CmdError> {
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind != AccountKind::Microsoft {
        return Err(failed!("{name} isn't kept in step through Microsoft Graph", name = a.name));
    }
    let shared = shared_mailbox(s, account)?;
    // A shared mailbox's changes, drafts and sends go up as its owner's do, at its own address
    // with its own ids (#70: the host asks `/users/{address}/…`, never `/me`): its drafts in its
    // own Drafts, what is sent from it through its own address, the copy in its Sent Items.
    let (drafts, sends) = (super::graph_drafts::draft_changes(s, account, MAX_DRAFTS).0, super::graph_send::sends(s, account));
    Ok(GraphMailRequest {
        account,
        email: a.email.clone(),
        shared,
        changes: folder_changes(&s.mailbox, account, MAX_FOLDER_CHANGES).0,
        links: links(&s.mailbox, account),
        background: false,
        messages: graph_up::message_changes(s, account, MAX_MESSAGE_CHANGES).0,
        drafts,
        sends,
        // Mail moved or copied here from another mailbox of the same owner's (#70).
        places: super::graph_place::place_changes(s, account, MAX_PLACES).0,
        inbox_pass: None,
        full: full_sweep_due(s, account),
    })
}

/// How long after a full sweep of an account's folders the next one is due (#276).
pub const FULL_SWEEP_EVERY: i64 = 60 * 60;

/// Whether the next Graph run of `account` reads every folder, passing none over for having the
/// counts it had (#276): the first run after the mailbox is opened, one an hour after the last
/// whole one, and one while a message the server said left its folder waits for a whole run (a
/// folder passed over might be where it went). Counts that match can hide a change (one message
/// in and one out, one read and another unread, a flag or a category): the sweep is what brings
/// those, late but never lost.
fn full_sweep_due(s: &Session, account: Id) -> bool {
    let due = s.graph_swept.get(&account).is_none_or(|at| s.now().saturating_sub(*at) >= FULL_SWEEP_EVERY);
    due || s.mailbox.messages.values().any(|m| m.account == account && m.server.as_ref().is_some_and(|server| server.gone))
}

/// The Inbox pass of `account` (#276): only its Inbox's delta, from the link kept for it, to be
/// run before anything else of a Send/Receive. `None` for an account with no Inbox pass: not one
/// kept in step through Graph, one that can't be read, or one whose Inbox is part-way through a
/// round (its link is the next page of it: the run itself brings it in). An Inbox whose id is
/// known but that has never been read (no link yet) is read from its start. One the server's id
/// isn't known for (never listed) has none: the run lists the folders first.
pub fn graph_inbox_request(s: &Session, account: Id) -> Option<GraphMailRequest> {
    let a = s.mailbox.account(account).filter(|a| a.kind == AccountKind::Microsoft)?;
    let shared = shared_mailbox(s, account).ok()?;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).and_then(|id| s.mailbox.folder(id))?;
    let (id, links) = match inbox.server.as_ref()? {
        server if server.id.is_empty() => return None,
        server if server.delta.is_empty() => (server.id.clone(), Vec::new()),
        server if server.caught_up => {
            (server.id.clone(), vec![GraphLink { folder: server.id.clone(), link: server.delta.clone(), caught_up: true, counts: None }])
        }
        _ => return None,
    };
    Some(GraphMailRequest {
        account,
        email: a.email.clone(),
        shared,
        links,
        inbox_pass: Some(GraphFolder { id, name: inbox.name.clone(), kind: FolderKind::Inbox, ..Default::default() }),
        ..Default::default()
    })
}

/// Whether `account` is a shared mailbox (#70), read with its owner's sign-in at
/// `/users/{address}/…`; refused when it is one that can't be read (its owner is gone, or its
/// address isn't one a request may name).
fn shared_mailbox(s: &Session, account: Id) -> Result<bool, CmdError> {
    let mb = &s.mailbox;
    let Some(a) = mb.account(account).filter(|a| a.is_shared()) else { return Ok(false) };
    if mb.owner_of(account).is_none() {
        return Err(failed!("{name} is a shared mailbox whose account is gone: remove it and add it again", name = a.name));
    }
    if mailcraft_model::mailbox_address(&a.email).is_none() {
        return Err(failed!("{name} isn't an address a shared mailbox can be opened at", name = a.name));
    }
    Ok(true)
}

/// Where each folder of `account` that the server has goes on from (R3 spec S10).
fn links(mb: &Mailbox, account: Id) -> Vec<GraphLink> {
    mb.folders
        .iter()
        .filter(|f| f.account == account)
        .filter_map(|f| f.server.as_ref())
        .filter(|s| !s.delta.is_empty())
        .map(|s| GraphLink { folder: s.id.clone(), link: s.delta.clone(), caught_up: s.caught_up, counts: s.counts.filter(|_| s.caught_up) })
        .collect()
}

/// How many changes made in `account` its server hasn't heard of yet ([`super::account::unpushed`]
/// for an account kept in step through Graph).
pub fn unpushed(s: &Session, account: Id) -> usize {
    folder_changes(&s.mailbox, account, 0).1
        + graph_up::message_changes(s, account, 0).1
        + super::graph_drafts::pending(&s.mailbox, account)
        + super::graph_place::place_changes(s, account, 0).1
}

/// The server id of the folder a folder of `account` lies in, as the server would have it:
/// empty for the top; `None` while that folder isn't on the server (the change waits).
fn parent_on_server(mb: &Mailbox, folder: &Folder) -> Option<String> {
    match folder.parent {
        None => Some(String::new()),
        Some(parent) => mb.folder(parent).filter(|p| p.account == folder.account).and_then(|p| p.server.as_ref()).map(|s| s.id.clone()),
    }
}

/// The folder changes made in `account` that its server hasn't heard of, parents before what lies
/// in them: at most `limit` spelled out, and how many there are.
fn folder_changes(mb: &Mailbox, account: Id, limit: usize) -> (Vec<FolderChange>, usize) {
    let (mut changes, mut count) = (Vec::new(), 0usize);
    let mut note = |change: &dyn Fn() -> FolderChange| {
        if changes.len() < limit {
            changes.push(change());
        }
        count += 1;
    };
    let mut folders: Vec<&Folder> = mb.folders.iter().filter(|f| f.account == account).collect();
    // How many folders down (a name may hold a `/`: the parents are counted, not the path).
    let depth = |f: &Folder| std::iter::successors(f.parent, |p| mb.folder(*p).and_then(|p| p.parent)).take(64).count();
    folders.sort_by_cached_key(|f| (depth(f), f.id));
    for f in folders {
        match &f.server {
            // The special folders are the server's: they are found in its listing, never made;
            // so is a folder that was on the account's IMAP server before it switched (R3.7).
            None if f.kind != FolderKind::Custom || f.awaiting_listing => {}
            None => {
                let parent = mb.folder(f.parent.unwrap_or(0)).filter(|p| p.account == account);
                let (server_parent, local_parent) = match parent {
                    None => (String::new(), None),
                    Some(p) => match &p.server {
                        Some(s) => (s.id.clone(), None),
                        None => (String::new(), Some(p.id)),
                    },
                };
                note(&|| FolderChange::Create { folder: f.id, name: f.name.clone(), parent: server_parent.clone(), parent_folder: local_parent });
            }
            Some(server) => {
                if f.kind == FolderKind::Custom && f.name != server.name {
                    note(&|| FolderChange::Rename { folder: f.id, id: server.id.clone(), name: f.name.clone() });
                }
                if let Some(parent) = parent_on_server(mb, f).filter(|parent| f.kind == FolderKind::Custom && *parent != server.parent) {
                    note(&|| FolderChange::Move { folder: f.id, id: server.id.clone(), parent: parent.clone() });
                }
            }
        }
    }
    for queued in mb.queued.iter().filter(|q| q.account() == account) {
        match queued {
            Queued::DeleteFolder { id, .. } => note(&|| FolderChange::Delete { id: id.clone() }),
            // Changes of messages: sent with the message changes (R3.3.2).
            Queued::CopyMessage { .. } | Queued::DeleteMessage { .. } | Queued::PlaceMessage { .. } => {}
        }
    }
    (changes, count)
}

fn request(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Microsoft).map(|a| a.id))
        .ok_or_else(|| fail("no account is kept in step through Microsoft Graph"))?;
    serde_json::to_value(graph_mail_request(s, account)?).map_err(|e| fail(e.to_string()))
}

/// Which messages of `account` a host should fetch whole: `message` at once (the owner opened it,
/// or is replying to it), else the next `most` in the background (R3 spec S12).
pub fn graph_fetch_request(s: &Session, account: Id, message: Option<Id>, most: usize) -> Result<GraphFetchRequest, CmdError> {
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind != AccountKind::Microsoft {
        return Err(failed!("{name} isn't kept in step through Microsoft Graph", name = a.name));
    }
    let ids = match message {
        Some(id) => {
            let m = s
                .mailbox
                .message(id)
                .filter(|m| m.account == account)
                .ok_or_else(|| failed!("no message with id {id} in {name}", id, name = a.name))?;
            m.server.as_ref().filter(|_| m.unfetched).map(|server| vec![server.id.clone()]).unwrap_or_default()
        }
        None => graph_down::fetch_queue_passing_over(&s.mailbox, account, most.clamp(1, 100), s.now(), &s.graph_unfetchable),
    };
    let shared = shared_mailbox(s, account)?;
    Ok(GraphFetchRequest { account, email: a.email.clone(), shared, ids, background: message.is_none() })
}

/// Take in messages fetched whole. Not an undo step.
pub fn apply_graph_fetch(s: &mut Session, result: GraphFetchResult) -> CmdResult {
    let account = result.account;
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if !result.email.is_empty() && !a.email.trim().eq_ignore_ascii_case(result.email.trim()) {
        return Err(failed!("these messages were for another account, not for {name}", name = a.name));
    }
    if s.file_is_current() {
        s.note_saved();
    }
    let mut index = std::mem::take(&mut s.graph_index);
    let taken: Vec<Id> =
        result.fetched.iter().filter_map(|f| graph_down::take_fetched_as(&mut s.mailbox, &mut index, account, &f.id, &f.mime)).collect();
    s.graph_index = index;
    // Older than its mail to keep offline (the owner opened it): whole until the mailbox is
    // opened again (#143).
    let window = mailcraft_model::OfflineWindow::of(&s.mailbox, s.now());
    let outside: Vec<Id> = taken.iter().copied().filter(|id| s.mailbox.message(*id).is_some_and(|m| !window.within(m))).collect();
    s.kept_for_now.extend(outside);
    let taken = taken.len();
    // Gone before it could be fetched: a message being moved out of the account stays as it is.
    let incomplete = graph_up::missing(&mut s.mailbox, account, &result.missing);
    // Refused although there: not asked for again in the background this session.
    s.graph_unfetchable.extend(result.failed.iter().take(1_000).map(|id| (account, id.clone())));
    if taken + incomplete > 0 {
        s.mailbox_changed();
    }
    let now = s.now();
    let counts = graph_down::full_counts(&s.mailbox, account, now);
    s.sync_progress.counted_as(&s.mailbox, account, counts, now);
    let graph_down::FullCounts { all, waiting, large } = counts;
    Ok(
        json!({"fetched": taken, "missing": result.missing.len(), "failed": result.failed.len(), "waiting": waiting, "large": large, "all": all, "errors": result.errors, "offline": result.offline, "busyFor": result.busy_for}),
    )
}

fn fetch_request(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Microsoft).map(|a| a.id))
        .ok_or_else(|| fail("no account is kept in step through Microsoft Graph"))?;
    let most = p::id(v, "most").and_then(|n| usize::try_from(n).ok()).unwrap_or(10);
    serde_json::to_value(graph_fetch_request(s, account, p::id(v, "message"), most)?).map_err(|e| fail(e.to_string()))
}

fn apply_fetch(s: &mut Session, v: &Value) -> CmdResult {
    let result: GraphFetchResult =
        serde_json::from_value(v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?).map_err(|e| bad(format!("`result`: {e}")))?;
    apply_graph_fetch(s, result)
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    let result: GraphMailResult =
        serde_json::from_value(v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?).map_err(|e| bad(format!("`result`: {e}")))?;
    apply_graph_mail(s, result)
}

/// What taking in a Graph run changed (a JMAP run's listing too, [`super::jmap_mail`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Taken {
    pub(super) pushed: usize,
    pub(super) added: usize,
    pub(super) updated: usize,
    pub(super) removed: usize,
}

/// Take in what a Graph run brought back: what became of the changes sent, then the listing.
/// Not an undo step: the server is the source of truth, but for what was changed here and hasn't
/// reached it yet. The undo history stays: undoing a change that reached the server is another
/// change, which the next run sends (R3.5, [`crate::undo`]).
pub fn apply_graph_mail(s: &mut Session, result: GraphMailResult) -> CmdResult {
    let account = result.account;
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind != AccountKind::Microsoft {
        return Err(failed!("{name} isn't kept in step through Microsoft Graph", name = a.name));
    }
    // An answer for another account (the mailbox was replaced while the run was under way)
    // would tie this one's folders to a stranger's server.
    if !result.email.is_empty() && !a.email.trim().eq_ignore_ascii_case(result.email.trim()) {
        return Err(failed!("this Send/Receive was for another account, not for {name}", name = a.name));
    }
    if s.file_is_current() {
        s.note_saved();
    }
    let mut taken = Taken::default();
    let mut errors = result.errors.clone();
    for done in &result.changes {
        if let Some(said) = change_done(&mut s.mailbox, account, &done.change, &done.outcome, &mut taken) {
            errors.push(said);
        }
    }
    // The changes of messages (R3.3): a refusal is said once per kind of change and reason, and
    // the status says it at the end of the run.
    let mut refusals = Refusals::default();
    for done in &result.messages {
        if graph_up::message_done(&mut s.mailbox, account, done, &mut refusals) {
            match done.outcome {
                MessageOutcome::Done { .. } => taken.pushed += 1,
                _ => taken.updated += 1,
            }
        }
    }
    // What was made here from another mailbox of the same owner's (#70).
    for done in &result.placed {
        if super::graph_place::place_done(&mut s.mailbox, account, done, &mut refusals) {
            match done.outcome {
                MessageOutcome::Done { .. } => taken.pushed += 1,
                _ => taken.updated += 1,
            }
            // A message's server id is new or gone: where each message is, is looked at afresh.
            s.graph_index = Default::default();
        }
    }
    let mut refused = refusals.lines();
    // The drafts (R3.4.1).
    for done in &result.drafts {
        if super::graph_drafts::draft_done(&mut s.mailbox, account, done, s.interface_language, &mut refused) {
            taken.updated += 1;
            // A draft's server id is new or gone: where each message is, is looked at afresh.
            s.graph_index = Default::default();
        }
    }
    // What was sent (R3.4.2): gone from the Outbox, or held there with what happened.
    let mut sent = 0;
    for done in &result.sent {
        if super::graph_send::send_done(&mut s.mailbox, account, done, s.interface_language, &mut refused) {
            sent += usize::from(matches!(done.outcome, SendOutcome::Sent));
            taken.updated += 1;
            s.graph_index = Default::default();
        }
    }
    errors.extend(refused.iter().cloned());
    s.graph_said.extend(refused);
    if let Some(listing) = &result.listing {
        take_listing(&mut s.mailbox, account, listing, &mut taken);
        if listing.complete {
            s.graph_listed_short.remove(&account);
        } else {
            s.graph_listed_short.insert(account);
        }
    }
    // Mail down (R3.2): each page with its link; what is still gone once every folder is done.
    let mut down = Down::default();
    let mut index = std::mem::take(&mut s.graph_index);
    for page in &result.pages {
        graph_down::take_page(&mut s.mailbox, &mut index, account, page, &mut down);
    }
    s.graph_index = index;
    // A rule moved new mail out of the account (#567). An undo of a step from before goes back to
    // a mailbox that never had it, and keeps only the Microsoft accounts' mail as it is now: once
    // the server's copy is removed, the mail would be gone for good. Until an undo leaves alone
    // what arrived after its step (#605), there is no going back past it.
    if down.left > 0 {
        s.forget_history();
    }
    // (Not after a listing cut short: what left a folder may be in one it missed, #305.)
    if result.whole && !result.inbox_pass && !s.graph_listed_short.contains(&account) {
        graph_down::sweep(&mut s.mailbox, account, &mut down);
        // Every folder read and none passed over: the full sweep is done for the hour (#276).
        if !result.partial && result.timings.skipped == 0 {
            let now = s.now();
            s.graph_swept.insert(account, now);
        }
    }
    // A shared mailbox is named as its server names it, once its Sent Items say so (#70).
    let named = s.mailbox.name_shared_from_sent(account);
    if taken != Taken::default() || down != Down::default() || !result.pages.is_empty() || named {
        s.mailbox_changed();
    }
    // The view stays on a folder that is there.
    if s.mailbox.folder(s.view.folder).is_none() {
        s.view.folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap_or(0);
        s.sel.messages.clear();
    }
    // Send/Receive Progress: where the run is, what it brought, what went wrong.
    let step = match (&result.progress, &result.listing, result.sent.is_empty()) {
        (Some(progress), ..) => {
            let name = s.mailbox.folder_by_server(account, &progress.folder).and_then(|f| s.mailbox.folder(f)).map(|f| f.name.as_str());
            let phase = if result.inbox_pass { SyncPhase::Inbox } else { SyncPhase::NewMail };
            Some((phase, sync_progress::shown(name.unwrap_or_default()), progress.read.min(progress.total), progress.total))
        }
        (None, Some(_), _) => Some((SyncPhase::Folders, String::new(), 0, 0)),
        (None, None, false) => Some((SyncPhase::Sending, String::new(), 0, 0)),
        (None, None, true) => result.inbox_pass.then(|| (SyncPhase::Inbox, String::new(), 0, 0)),
    };
    let folders = result.progress.as_ref().map(|p| (p.number, p.folders));
    // (The Inbox pass is never the end of the account's part of the run.)
    let last = !result.partial && !result.inbox_pass;
    if last {
        errors.extend(super::graph_place::unreadable_waiting(s, account));
    }
    let piece = Piece { step, folders, new: down.new, sent, problems: &errors, last, stopped: result.stopped, timings: Some(&result.timings) };
    s.sync_progress.take(account, piece);
    // Full messages: counting them goes through every message, so a run's pages are counted at
    // most once a second while it goes on, and exactly when the account's part of it ends.
    let now = s.now();
    if result.whole || (last && !result.pages.is_empty()) {
        s.sync_progress.count_full(&s.mailbox, account, now);
    } else if !result.pages.is_empty() {
        s.sync_progress.count_full_now_and_then(&s.mailbox, account, now);
    } else if last {
        s.sync_progress.count_full_if_due(&s.mailbox, account, now);
    }
    let summary = match (errors.first(), taken.pushed + taken.added + taken.updated + taken.removed) {
        (Some(e), _) => e.clone(),
        (None, 0) => "Up to date".to_string(),
        (None, _) => "Folders up to date".to_string(),
    };
    match (&result.progress, result.partial) {
        // Under way: how far it has got.
        (Some(progress), true) => s.sync_status = graph_down::progress_words(&s.mailbox, account, progress, s.interface_language),
        (_, true) => {}
        (_, false) => {
            let t = s.local_now();
            // What the server refused in this run, said once, ahead of anything else.
            let refused = std::mem::take(&mut s.graph_said);
            let lang = s.interface_language;
            let said = if !refused.is_empty() {
                refused.join(" ")
            } else if result.stopped && errors.is_empty() {
                lang.t("Stopped; carries on from here next time").to_string()
            } else if errors.is_empty() {
                (if summary == "Up to date" { lang.t("Up to date") } else { lang.t("Folders up to date") }).to_string()
            } else {
                summary.clone()
            };
            s.sync_status = format!("{said} — {}", s.time_said(t));
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
    }))
}

/// A folder's name as the owner reads it, for a line about it.
fn shown(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(80).collect()
}

/// Take in what became of one change. Answers what to tell the owner, when there is something.
fn change_done(mb: &mut Mailbox, account: Id, change: &FolderChange, outcome: &ChangeOutcome, taken: &mut Taken) -> Option<String> {
    let mine = |mb: &Mailbox, id: Id| mb.folder(id).is_some_and(|f| f.account == account);
    match (change, outcome) {
        (FolderChange::Delete { id }, ChangeOutcome::Done { .. } | ChangeOutcome::Gone) => {
            mb.queued.retain(|q| !matches!(q, Queued::DeleteFolder { account: a, id: queued } if *a == account && queued == id));
            taken.pushed += 1;
            None
        }
        (FolderChange::Delete { id }, ChangeOutcome::Refused { why }) => {
            // The server keeps it: the listing brings it back here.
            mb.queued.retain(|q| !matches!(q, Queued::DeleteFolder { account: a, id: queued } if *a == account && queued == id));
            Some(format!("Couldn't delete a folder on the server: {why}"))
        }
        (FolderChange::Create { folder, name, .. }, ChangeOutcome::Done { folder: there }) => {
            // Made already (an earlier answer that is taken in twice), or one this mailbox has.
            if !mine(mb, *folder) || mb.folder_by_server(account, &there.id).is_some() {
                return None;
            }
            let f = mb.folder_mut(*folder).ok()?;
            if f.server.is_some() {
                return None;
            }
            // Named as the server named it (a folder it had already, in another case), unless it
            // was renamed here meanwhile: that goes up next time.
            if f.name == *name {
                f.name = there.name.clone();
            }
            f.server = Some(ServerFolder {
                id: there.id.clone(),
                name: there.name.clone(),
                parent: there.parent.clone(),
                delta: String::new(),
                caught_up: false,
                round_ended: false,
                counts: None,
                sync_state: String::new(),
            });
            taken.pushed += 1;
            None
        }
        (FolderChange::Create { name, .. }, ChangeOutcome::Refused { why }) => {
            Some(format!("Couldn't make the folder \"{}\" on the server: {why}", shown(name)))
        }
        (FolderChange::Rename { folder, .. } | FolderChange::Move { folder, .. }, ChangeOutcome::Done { folder: there }) => {
            let f = mb.folder_mut(*folder).ok().filter(|f| f.account == account)?;
            let server = f.server.as_mut().filter(|server| server.id == there.id)?;
            match change {
                FolderChange::Rename { .. } => server.name = there.name.clone(),
                _ => server.parent = there.parent.clone(),
            }
            taken.pushed += 1;
            None
        }
        (FolderChange::Rename { folder, .. } | FolderChange::Move { folder, .. }, ChangeOutcome::Refused { why }) => {
            // Put back to how the server has it, and said once.
            let server = mb.folder(*folder).filter(|f| f.account == account)?.server.clone()?;
            let back = if server.parent.is_empty() { None } else { mb.folder_by_server(account, &server.parent) };
            let f = mb.folder_mut(*folder).ok()?;
            let was = shown(&f.name);
            let said = match change {
                FolderChange::Rename { .. } => {
                    f.name = server.name.clone();
                    format!("Couldn't rename the folder \"{was}\" on the server: {why}")
                }
                _ => {
                    f.parent = back;
                    format!("Couldn't move the folder \"{was}\" on the server: {why}")
                }
            };
            taken.updated += 1;
            Some(said)
        }
        // Gone from the server: the listing takes it away here.
        (_, ChangeOutcome::Gone) => None,
    }
}

/// Take in every folder the server listed, by server id (a JMAP server's too: its mailboxes are
/// known by their ids as Graph's folders are).
pub(super) fn take_listing(mb: &mut Mailbox, account: Id, listing: &FolderListing, taken: &mut Taken) {
    release_outbox(mb, account, listing);
    let mut listed: HashSet<&str> = HashSet::new();
    // The server's folders as this mailbox has them, by server id, as they are found or made.
    let mut here: HashMap<String, Id> =
        mb.folders.iter().filter(|f| f.account == account).filter_map(|f| f.server.as_ref().map(|s| (s.id.clone(), f.id))).collect();
    // Parents come first; one whose parent isn't known (yet) goes at the top.
    for there in &listing.folders {
        if there.id.is_empty() || !listed.insert(there.id.as_str()) {
            continue;
        }
        let Some(name) = clean_name(&there.name) else { continue };
        let parent_here = if there.parent.is_empty() { None } else { here.get(&there.parent).copied() };
        let found = here.get(&there.id).copied().or_else(|| adopt(mb, account, there, parent_here, &name));
        match found {
            Some(id) => {
                if update(mb, id, there, &name, parent_here) {
                    taken.updated += 1;
                }
                here.insert(there.id.clone(), id);
            }
            None => {
                let id = mb.alloc_id();
                mb.folders.push(Folder {
                    id,
                    account,
                    parent: parent_here,
                    name: name.clone(),
                    kind: there.kind,
                    server: Some(ServerFolder { id: there.id.clone(), name, parent: there.parent.clone(), ..Default::default() }),
                    ..Default::default()
                });
                here.insert(there.id.clone(), id);
                taken.added += 1;
            }
        }
    }
    // What a complete listing doesn't have, the server doesn't: gone here too, with its mail. A
    // listing that was cut short removes nothing.
    if listing.complete {
        let gone: Vec<Id> = mb
            .folders
            .iter()
            .filter(|f| f.account == account && f.server.as_ref().is_some_and(|s| !listed.contains(s.id.as_str())))
            .map(|f| f.id)
            .collect();
        taken.removed += gone.len();
        // Mail moved in that the server wasn't given yet isn't in the folder there: it goes back
        // to where it came from, not with the folder (#451).
        super::graph_place::put_back_waiting(mb, |mb, a, message| a == account && mb.message(message).is_some_and(|m| gone.contains(&m.folder)));
        mb.remove_folders(&gone);
        taken.removed += super::graph_switch::sweep_unlisted(mb, account);
    }
}

/// The Outbox here is the queue mail is sent from; it follows no folder of the server (AU2-V2).
/// 0.8.0 and before bound it to the server's Outbox, whose items were then sent from here with
/// the owner's rights. A listing that now has that folder as a folder like another lets go of the
/// bond; what the server delivered into it is forgotten here (it comes down again into the
/// server's Outbox as a folder of its own) and is not sent.
fn release_outbox(mb: &mut Mailbox, account: Id, listing: &FolderListing) {
    let bound: Vec<Id> = mb
        .folders
        .iter()
        .filter(|f| f.account == account && f.kind == FolderKind::Outbox)
        .filter(|f| f.server.as_ref().is_some_and(|s| listing.folders.iter().any(|l| l.id == s.id && l.kind != FolderKind::Outbox)))
        .map(|f| f.id)
        .collect();
    for folder in bound {
        let delivered: Vec<Id> = mb.messages_in(folder).filter(|m| super::graph_send::from_server(m)).map(|m| m.id).collect();
        for id in delivered {
            let _ = mb.forget_message(id);
        }
        if let Ok(f) = mb.folder_mut(folder) {
            f.server = None;
        }
    }
}

/// A folder of this mailbox that a listed one is, though it has no server id yet: the special
/// folder of its kind, or a folder made here of the same name in the same place (made on the
/// server by a run whose answer was lost, or made in both places).
fn adopt(mb: &Mailbox, account: Id, there: &GraphFolder, parent_here: Option<Id>, name: &str) -> Option<Id> {
    let unsynced = |f: &&Folder| f.account == account && f.server.is_none();
    if there.kind != FolderKind::Custom {
        return mb.folders.iter().filter(unsynced).find(|f| f.kind == there.kind).map(|f| f.id);
    }
    let name = name.to_lowercase();
    mb.folders
        .iter()
        .filter(unsynced)
        .find(|f| f.kind == FolderKind::Custom && f.parent == parent_here && f.name.to_lowercase() == name)
        .map(|f| f.id)
}

/// Bring the folder `id` in step with what the server listed of it: its name and its place, but
/// for what was changed here since the server last said (that goes up with the next run). Whether
/// anything changed here.
fn update(mb: &mut Mailbox, id: Id, there: &GraphFolder, name: &str, parent_here: Option<Id>) -> bool {
    let Some(f) = mb.folder(id) else { return false };
    let before = f.clone();
    let last = f.server.clone().unwrap_or_default();
    // What the folder's parent is on the server, as this mailbox has it now.
    let parent_now = parent_on_server(mb, f);
    let renamed_here = f.server.is_some() && f.name != last.name;
    let moved_here = f.server.is_some() && parent_now.as_deref() != Some(last.parent.as_str());
    // A place it can be: known here (or the top), and not inside itself (a move made here may
    // not have reached the server yet).
    let can_go = (there.parent.is_empty() || parent_here.is_some()) && !parent_here.is_some_and(|p| mb.subtree(id).contains(&p));
    let Ok(f) = mb.folder_mut(id) else { return false };
    if !renamed_here {
        f.name = name.to_string();
    }
    if !moved_here && can_go {
        f.parent = parent_here;
    }
    if f.kind == FolderKind::Custom && there.kind != FolderKind::Custom {
        f.kind = there.kind;
    }
    // Found: the server's folder from now on (R3.7).
    f.awaiting_listing = false;
    f.server = Some(ServerFolder {
        id: there.id.clone(),
        name: name.to_string(),
        parent: there.parent.clone(),
        delta: last.delta,
        caught_up: last.caught_up,
        round_ended: last.round_ended,
        counts: last.counts,
        sync_state: String::new(),
    });
    *f != before
}

#[cfg(test)]
#[path = "graph_mail_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "graph_sweep_tests.rs"]
mod sweep_tests;

#[cfg(test)]
#[path = "graph_inbox_first_tests.rs"]
mod inbox_first_tests;
