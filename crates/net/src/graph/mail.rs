// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's folders through Microsoft Graph (R3.1; R3 spec S4, S6–S8, S14, S21).
//!
//! [`sync_folders`] takes a [`GraphMailRequest`] (what the engine worked out from the mailbox)
//! and an access token, makes the folder changes it holds on the server, then lists every mail
//! folder the mailbox has, and answers a [`GraphMailResult`] for the engine to take in. It
//! blocks; a host calls it on a background thread.
//!
//! - **Asked of the server.** `GET /me/mailFolders/{well-known name}` for which folder is the
//!   Inbox, the Drafts… and the top of the mailbox (twenty to a `$batch`); `GET /me/mailFolders`
//!   and `GET /me/mailFolders/{id}/childFolders`, page by page, for the folders, the subfolders
//!   of a level of the tree twenty folders to a `$batch` (#276); `POST
//!   /me/mailFolders[/{id}/childFolders]` to make one, `PATCH /me/mailFolders/{id}` to rename
//!   one, `POST /me/mailFolders/{id}/move` to move one (Delete Folder is a move into Deleted
//!   Items, as Outlook does), `DELETE /me/mailFolders/{id}` to delete one for good. Every
//!   request asks for immutable ids, goes through the mailbox's gate, and is asked again or not
//!   as [`pace`](super::pace) has it: a move or a folder that is made is never made twice by a
//!   second try.
//! - **The listing** is a walk of every folder's subfolders, in full, each run: folders are few,
//!   and Graph's folder delta isn't documented to reach every depth. Hidden folders and search
//!   folders are left out with what lies under them. A folder's name is kept whole, `/` and `\`
//!   and all: the tree comes from where each folder was found, never from splitting a name.
//!   A listing that doesn't get to its end says so ([`FolderListing::complete`]), and then
//!   nothing is removed for being missing from it.
//! - **A shared mailbox** (#70) is asked at `/users/{address}/…` for all of it: its changes, its
//!   drafts (in its own Drafts) and what is sent from it (the copy in its own Sent Items). A
//!   refusal of its by Graph's code (no access, no mailbox) stops its changes, drafts and sends,
//!   which wait here; one is never taken for a folder or a message that is gone.
//! - **What comes back is hostile JSON.** Every list, id and name is capped; an entry that
//!   can't be read is passed over (and makes the listing incomplete); nothing in it is run or
//!   fetched. No folder's name or id goes into an error or a log line.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use mailcraft_model::sync::{
    ChangeOutcome, FolderChange, FolderChangeDone, FolderListing, GraphFolder, GraphMailRequest, GraphMailResult, MAX_FOLDER_CHANGES,
    MAX_LISTED_FOLDERS, SyncPhase,
};
use mailcraft_model::{FolderKind, clean_name};
use serde_json::{Value, json};

use super::pace::{Gate, Kind, Priority, Retry};
use super::{
    ASK_TIMEOUT, BATCH, BATCH_SIZE, Bearer, Doing, Graph, IMMUTABLE_IDS, MAX_BATCH, MAX_PAGE, Renew, WRITE_TIMEOUT, graph_base, nonsense, text,
};
use crate::NetError;
use crate::http::{self, Reply};

/// What is asked for of each folder: no more comes down than is used.
const FOLDER_FIELDS: &str = "id,displayName,parentFolderId,childFolderCount,isHidden,totalItemCount,unreadItemCount";
/// Folders asked for per page.
const FOLDER_PAGE: &str = "100";
/// Deepest a folder is followed (the mailbox's own limit).
const MAX_DEPTH: usize = 32;
/// Longest id taken from the server, and longest name sent to it.
const MAX_ID: usize = 1_024;
const MAX_NAME: usize = 255;
/// The folders asked for by their well-known names, with what each is here. `msgfolderroot` is
/// the top of the mailbox: what lies in it directly is at the top here.
const WELL_KNOWN: [(&str, Option<FolderKind>); 8] = [
    ("msgfolderroot", None),
    ("inbox", Some(FolderKind::Inbox)),
    ("drafts", Some(FolderKind::Drafts)),
    ("sentitems", Some(FolderKind::Sent)),
    ("deleteditems", Some(FolderKind::Deleted)),
    ("archive", Some(FolderKind::Archive)),
    ("junkemail", Some(FolderKind::Junk)),
    ("outbox", Some(FolderKind::Outbox)),
];

/// Make the folder changes of `request` on the server and list the mailbox's folders, as the
/// owner of `token` (an access token for Graph that covers `Mail.ReadWrite`).
pub fn sync_folders(token: &str, request: &GraphMailRequest) -> GraphMailResult {
    sync_folders_with(&graph_base(), token, request, Gate::for_mailbox(&request.email), Retry::STANDARD)
}

/// A whole run for a Microsoft account (R3.1, R3.2): the folders as [`sync_folders`] has them,
/// then every folder's mail ([`down`]). It comes in pieces through `emit`, each to be taken in as
/// it comes (the folders first, then a page at a time); the last has [`GraphMailResult::partial`]
/// false. `cancel` stops it after the page in hand.
pub fn sync_mail(token: &str, request: &GraphMailRequest, cancel: &AtomicBool, emit: &mut dyn FnMut(GraphMailResult)) {
    sync_mail_with(&graph_base(), token, request, Gate::for_mailbox(&request.email), Retry::STANDARD, cancel, emit);
}

/// [`sync_mail`], with `renew` to get a new token when Graph stops taking `token` part-way
/// (#306): once in the run, the request it refused asked again with the new one. A second
/// refusal ends the run as [`sync_mail`]'s would.
pub fn sync_mail_renewing(token: &str, renew: Renew<'_>, request: &GraphMailRequest, cancel: &AtomicBool, emit: &mut dyn FnMut(GraphMailResult)) {
    sync_mail_renewing_with(&graph_base(), token, Some(renew), request, Gate::for_mailbox(&request.email), Retry::STANDARD, cancel, emit);
}

/// [`sync_mail`] at `base`, through `gate`, asking again as `retry` says.
pub(crate) fn sync_mail_with(
    base: &str,
    token: &str,
    request: &GraphMailRequest,
    gate: Arc<Gate>,
    retry: Retry,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(GraphMailResult),
) {
    sync_mail_renewing_with(base, token, None, request, gate, retry, cancel, emit);
}

/// [`sync_mail_with`], renewing the token once with `renew` when there is one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_mail_renewing_with(
    base: &str,
    token: &str,
    renew: Option<Renew<'_>>,
    request: &GraphMailRequest,
    gate: Arc<Gate>,
    retry: Retry,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(GraphMailResult),
) {
    let bearer = Bearer::new(token, renew);
    // The Inbox pass (#276): the Inbox's delta alone, before anything else of the run.
    if let Some(inbox) = &request.inbox_pass {
        return inbox_pass(base, token, &bearer, request, inbox, gate, retry, cancel, emit);
    }
    let mut folders = folders_with(base, token, &bearer, request, gate.clone(), retry);
    // No listing, or a server that wants to be left alone: that is the run.
    let listed = match &folders.listing {
        Some(listing) if folders.busy_for.is_none() => listing.folders.clone(),
        _ => return emit(folders),
    };
    folders.partial = true;
    emit(folders);
    let priority = if request.background { Priority::Background } else { Priority::Owner };
    let last = match for_mail(base, token, &bearer, gate, priority, retry).and_then(|graph| graph.of_shared(shared_address(request))) {
        Ok(graph) => down::messages(&graph, request, &listed, SyncPhase::NewMail, cancel, emit),
        Err(e) => GraphMailResult {
            account: request.account,
            email: request.email.clone(),
            errors: vec![format!("Couldn't reach Microsoft for the mail: {e}")],
            ..Default::default()
        },
    };
    emit(last);
}

/// The Inbox pass of a run (#276): the delta of `inbox` from its stored link, a page at a time
/// through `emit`, and nothing else; no folder is listed and nothing is sent. Every piece is
/// partial and none says the mailbox was read whole: the run that follows is the rest, from the
/// links this one moved on.
#[allow(clippy::too_many_arguments)]
fn inbox_pass(
    base: &str,
    token: &str,
    bearer: &Bearer<'_>,
    request: &GraphMailRequest,
    inbox: &mailcraft_model::sync::GraphFolder,
    gate: Arc<Gate>,
    retry: Retry,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(GraphMailResult),
) {
    let priority = if request.background { Priority::Background } else { Priority::Owner };
    let mut last = match for_mail(base, token, bearer, gate, priority, retry).and_then(|graph| graph.of_shared(shared_address(request))) {
        Ok(graph) => down::messages(&graph, request, std::slice::from_ref(inbox), SyncPhase::Inbox, cancel, emit),
        Err(e) => GraphMailResult {
            account: request.account,
            email: request.email.clone(),
            errors: vec![format!("Couldn't reach Microsoft for the mail: {e}")],
            ..Default::default()
        },
    };
    // (Only the Inbox was read: that is not every folder of the mailbox.)
    (last.partial, last.whole, last.inbox_pass) = (true, false, true);
    emit(last);
}

/// Every piece of a run of [`sync_mail_with`] put together, in order: what a caller that takes a
/// run in at once gets.
#[cfg(test)]
pub(crate) fn whole_run(
    base: &str,
    token: &str,
    request: &GraphMailRequest,
    gate: Arc<Gate>,
    retry: Retry,
    cancel: &AtomicBool,
) -> Vec<GraphMailResult> {
    let mut pieces = Vec::new();
    sync_mail_with(base, token, request, gate, retry, cancel, &mut |piece| pieces.push(piece));
    pieces
}

/// [`sync_folders`] at `base`, through `gate`, asking again as `retry` says.
pub(crate) fn sync_folders_with(base: &str, token: &str, request: &GraphMailRequest, gate: Arc<Gate>, retry: Retry) -> GraphMailResult {
    folders_with(base, token, &Bearer::new(token, None), request, gate, retry)
}

/// Graph for a run's mail, showing the run's `bearer` (`token` is the one it began with).
fn for_mail<'a>(
    base: &str,
    token: &'a str,
    bearer: &'a Bearer<'a>,
    gate: Arc<Gate>,
    priority: Priority,
    retry: Retry,
) -> Result<Graph<'a>, NetError> {
    Graph::for_mail(base, token, gate, priority, retry).map(|graph| graph.renewing(Some(bearer)))
}

/// [`sync_folders_with`], showing the run's `bearer`.
fn folders_with(base: &str, token: &str, bearer: &Bearer<'_>, request: &GraphMailRequest, gate: Arc<Gate>, retry: Retry) -> GraphMailResult {
    let started = Instant::now();
    let mut out = GraphMailResult { account: request.account, email: request.email.clone(), ..Default::default() };
    let priority = if request.background { Priority::Background } else { Priority::Owner };
    let graph = match for_mail(base, token, bearer, gate.clone(), priority, retry).and_then(|graph| graph.of_shared(shared_address(request))) {
        Ok(graph) => graph,
        Err(e) => {
            out.errors.push(format!("Couldn't reach Microsoft for the folders: {e}"));
            return out;
        }
    };
    let mut marks = Marks::default();
    let mut out = folders_in(&graph, request, &gate, out, &mut marks);
    // What the folders' part of a run took (#276): sending what waits (changes, drafts, mail to
    // send) from the moment the special folders are known to the listing; the rest is Folders.
    let (total, requests) = (started.elapsed(), graph.requests_asked());
    let sending = match (marks.known, marks.sent) {
        (Some((from, asked)), sent) => {
            let (to, until) = sent.unwrap_or((total, requests));
            (to.saturating_sub(from), until.saturating_sub(asked))
        }
        (None, _) => (Duration::ZERO, 0),
    };
    if sending.1 > 0 {
        out.timings.add_time(SyncPhase::Sending, sending.0, sending.1);
    }
    out.timings.add_time(SyncPhase::Folders, total.saturating_sub(sending.0), requests.saturating_sub(sending.1));
    out
}

/// Where the folders' part of a run was at two moments, for its timings: how long into it and how
/// many requests had been asked.
#[derive(Default)]
struct Marks {
    /// The special folders were known: what is sent begins.
    known: Option<(Duration, u32)>,
    /// What is sent was through: the listing begins.
    sent: Option<(Duration, u32)>,
}

/// The folders' part of a run, as far as it goes: `out` has the answer so far.
fn folders_in(graph: &Graph<'_>, request: &GraphMailRequest, gate: &Arc<Gate>, mut out: GraphMailResult, marks: &mut Marks) -> GraphMailResult {
    let started = Instant::now();
    let stop = |out: &mut GraphMailResult, e: &NetError| {
        if let NetError::Busy { retry_after, .. } = e {
            out.busy_for = Some(retry_after.unwrap_or(0).max(gate.paused_for().map_or(0, |left| u32::try_from(left.as_secs()).unwrap_or(u32::MAX))));
        }
        // A shared mailbox the owner may not open (#70): said as it is, and not asked again by
        // itself.
        if let NetError::SharedRefused(why) = e {
            out.refused = Some(*why);
            out.errors.push(format!("{}.", why.said()));
            return;
        }
        out.errors.push(format!("Couldn't bring the folders in step with Microsoft: {e}"));
    };
    // Which folder is which: the top of the mailbox, and the special folders.
    let known = match well_known(graph) {
        Ok(known) => known,
        Err(e) => {
            stop(&mut out, &e);
            return out;
        }
    };
    marks.known = Some((started.elapsed(), graph.requests_asked()));
    // A shared mailbox's changes, drafts and sends go up at its own address, as the owner's do at
    // his (#70): every path below comes of `graph`, which names that mailbox.
    let (drafts, sends) = (&request.drafts[..], &request.sends[..]);
    // What was changed here goes up first: the listing then shows the server after it.
    let mut made: HashMap<u64, String> = HashMap::new();
    for change in request.changes.iter().take(MAX_FOLDER_CHANGES) {
        match make_change(graph, &known, &made, change) {
            Ok(Some(outcome)) => {
                if let (FolderChange::Create { folder, .. }, ChangeOutcome::Done { folder: there }) = (change, &outcome) {
                    made.insert(*folder, there.id.clone());
                }
                out.changes.push(FolderChangeDone { change: change.clone(), outcome });
            }
            // Not now (its parent isn't on the server yet): the next run.
            Ok(None) => {}
            Err(e) => {
                // The rest wait for the next run; the listing still says what the server has.
                log::info!("graph folders: a change got no answer; the rest wait for the next run");
                let token_refused = e.is_refused_sign_in();
                stop(&mut out, &e);
                if token_refused || matches!(e, NetError::Busy { .. } | NetError::SharedRefused(_)) {
                    return out;
                }
                break;
            }
        }
    }
    // Then the changes of messages, which may go into a folder just made (R3 spec S16).
    match up::messages(graph, &made, &request.messages, &mut out.messages) {
        Ok(0) => {}
        // A shared mailbox that refused some of them (#70): they wait here, and the owner is told.
        Ok(1) => out.errors.push("Microsoft refused a change made in this shared mailbox; it waits on this computer.".into()),
        Ok(held) => out.errors.push(format!("Microsoft refused {held} changes made in this shared mailbox; they wait on this computer.")),
        // One that won't let the owner in: said as it is, and the rest wait.
        Err(e @ NetError::SharedRefused(_)) => {
            log::info!("graph changes: the shared mailbox refused the owner; the changes wait for the next run");
            stop(&mut out, &e);
            return out;
        }
        Err(e) => {
            log::info!("graph changes: the changes of messages stopped; the rest wait for the next run");
            if let NetError::Busy { retry_after, .. } = &e {
                out.busy_for =
                    Some(retry_after.unwrap_or(0).max(gate.paused_for().map_or(0, |left| u32::try_from(left.as_secs()).unwrap_or(u32::MAX))));
            }
            out.errors.push(format!("Couldn't send the changes of messages to Microsoft: {e}"));
            if e.is_refused_sign_in() || matches!(e, NetError::Busy { .. }) {
                return out;
            }
        }
    }
    // Then what was moved or copied here from another mailbox of the same owner's (#70), which
    // may go into a folder just made too.
    match place::places(graph, &made, &request.places, &mut out.placed) {
        Ok(0) => {}
        Ok(1) => out.errors.push("Microsoft refused a message moved or copied into this shared mailbox; it waits on this computer.".into()),
        Ok(held) => {
            out.errors.push(format!("Microsoft refused {held} messages moved or copied into this shared mailbox; they wait on this computer."))
        }
        Err(e @ NetError::SharedRefused(_)) => {
            log::info!("graph places: the shared mailbox refused the owner; the messages wait for the next run");
            stop(&mut out, &e);
            return out;
        }
        Err(e) => {
            log::info!("graph places: the messages from other mailboxes stopped; the rest wait for the next run");
            if let NetError::Busy { retry_after, .. } = &e {
                out.busy_for =
                    Some(retry_after.unwrap_or(0).max(gate.paused_for().map_or(0, |left| u32::try_from(left.as_secs()).unwrap_or(u32::MAX))));
            }
            out.errors.push(format!("Couldn't put the messages moved from another mailbox on Microsoft's server: {e}"));
            if e.is_refused_sign_in() || matches!(e, NetError::Busy { .. }) {
                return out;
            }
        }
    }
    // Then the drafts saved here (R3.4.1).
    if let Err(e) = drafts::drafts(graph, drafts, &mut out.drafts) {
        log::info!("graph drafts: the drafts stopped; the rest wait for the next run");
        // A shared mailbox that won't let the owner in (#70): said as it is, and the rest wait.
        if let NetError::SharedRefused(_) = e {
            stop(&mut out, &e);
            return out;
        }
        if let NetError::Busy { retry_after, .. } = &e {
            out.busy_for = Some(retry_after.unwrap_or(0).max(gate.paused_for().map_or(0, |left| u32::try_from(left.as_secs()).unwrap_or(u32::MAX))));
        }
        out.errors.push(format!("Couldn't save the drafts on Microsoft's server: {e}"));
        if e.is_refused_sign_in() || matches!(e, NetError::Busy { .. }) {
            return out;
        }
    }
    // Then what waits in the Outbox (R3.4.2): never asked twice by itself.
    if let Err(e) = send::sends(graph, sends, &mut out.sent) {
        log::info!("graph sending: sending stopped; the rest waits for the next run");
        if let NetError::SharedRefused(_) = e {
            stop(&mut out, &e);
            return out;
        }
        if let NetError::Busy { retry_after, .. } = &e {
            out.busy_for = Some(retry_after.unwrap_or(0).max(gate.paused_for().map_or(0, |left| u32::try_from(left.as_secs()).unwrap_or(u32::MAX))));
        }
        out.errors.push(format!("Couldn't send through Microsoft: {e}"));
        if e.is_refused_sign_in() || matches!(e, NetError::Busy { .. }) {
            return out;
        }
    }
    marks.sent = Some((started.elapsed(), graph.requests_asked()));
    match listing(graph, &known) {
        Ok(listing) => {
            log::debug!("graph folders: {} listed, complete: {}", listing.folders.len(), listing.complete);
            out.listing = Some(listing);
        }
        Err((listing, e)) => {
            log::info!("graph folders: the listing was cut short ({} listed)", listing.folders.len());
            stop(&mut out, &e);
            if !e.is_refused_sign_in() {
                out.listing = Some(listing);
            }
        }
    }
    out
}

/// The shared mailbox a request is for, when it is for one (#70).
fn shared_address(request: &GraphMailRequest) -> Option<&str> {
    request.shared.then_some(request.email.as_str())
}

/// The server's ids of the top of the mailbox and of the special folders.
#[derive(Debug, Default)]
struct Known {
    root: String,
    kinds: HashMap<String, FolderKind>,
}

impl Known {
    /// A parent as the server has it (`msgfolderroot`'s id) as the mailbox has it (empty).
    fn parent_here(&self, parent: &str) -> String {
        if parent == self.root { String::new() } else { parent.to_string() }
    }
}

/// Where the folders of the mailbox are: `/v1.0/me/mailFolders`, or a shared mailbox's (#70).
fn folders(graph: &Graph<'_>) -> String {
    graph.mail_path("/mailFolders")
}

/// An id as the server writes one: visible ASCII, not too long.
fn id_of(value: &str) -> Option<String> {
    let fits = !value.is_empty() && value.len() <= MAX_ID && value.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\'));
    fits.then(|| value.to_string())
}

/// `GET /me/mailFolders/{name}` for every well-known name, twenty to a batch (one at a time
/// when the server won't take a batch). Every name has to get an answer (a folder, or "there is
/// none"): which folder is the Inbox is not guessed.
fn well_known(graph: &Graph<'_>) -> Result<Known, NetError> {
    let select = http::form(&[("$select", "id")]);
    let requests: Vec<Value> = WELL_KNOWN
        .iter()
        .enumerate()
        .map(|(n, (name, _))| json!({"id": n.to_string(), "method": "GET", "url": graph.batch_path(&format!("/mailFolders/{name}?{select}")), "headers": {"Prefer": IMMUTABLE_IDS}}))
        .collect();
    let body = json!({"requests": requests}).to_string();
    let reply = graph.call(Kind::Read, &graph.url(BATCH), "POST", &[("Content-Type", "application/json")], Some(&body), ASK_TIMEOUT, MAX_BATCH)?;
    let mut answers: Vec<Option<Option<String>>> = vec![None; WELL_KNOWN.len()];
    if reply.status == 200 {
        let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
        let said = json.get("responses").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        for one in said.iter().take(BATCH_SIZE * 2) {
            let n = one.get("id").and_then(Value::as_str).and_then(|n| n.parse::<usize>().ok());
            let Some(slot) = n.and_then(|n| answers.get_mut(n)) else { continue };
            let status = one.get("status").and_then(Value::as_u64);
            // A shared mailbox that isn't to be opened says so in every answer (#70).
            if graph.shared()
                && let Some(why) = super::shared_refusal(status.unwrap_or(0), one.get("body").unwrap_or(&Value::Null))
            {
                return Err(NetError::SharedRefused(why));
            }
            *slot = match status {
                Some(200) => one.get("body").map(|body| id_of(text(body, "id"))).filter(Option::is_some),
                // The mailbox has no such folder (an Archive, say).
                Some(404) => Some(None),
                _ => None,
            };
        }
    } else if matches!(reply.status, 401 | 403 | 429 | 503 | 504) {
        return Err(graph.refusal(Doing::Folders, &reply, &body));
    }
    // What the batch didn't settle, one at a time.
    for ((name, _), slot) in WELL_KNOWN.iter().zip(answers.iter_mut()).filter(|(_, slot)| slot.is_none()) {
        let url = graph.url(&format!("{}/{name}?{select}", folders(graph)));
        let reply = graph.call(Kind::Read, &url, "GET", &[], None, ASK_TIMEOUT, MAX_PAGE)?;
        if graph.shared()
            && let Some(why) = super::shared_refusal(u64::from(reply.status), &serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null))
        {
            return Err(NetError::SharedRefused(why));
        }
        *slot = match reply.status {
            200 => {
                let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                Some(Some(id_of(text(&json, "id")).ok_or_else(|| nonsense("Microsoft Graph named a folder without an id"))?))
            }
            404 => Some(None),
            _ => return Err(graph.refusal(Doing::Folders, &reply, "")),
        };
    }
    let mut known = Known::default();
    for ((_, kind), answer) in WELL_KNOWN.iter().zip(answers) {
        let Some(Some(id)) = answer else { continue };
        match kind {
            None => known.root = id,
            Some(kind) => {
                known.kinds.entry(id).or_insert(*kind);
            }
        }
    }
    if known.root.is_empty() {
        // For a shared mailbox, that is Graph having no mailbox there for the owner (#70).
        if graph.shared() {
            return Err(NetError::SharedRefused(mailcraft_model::sync::SharedRefusal::NoMailbox));
        }
        return Err(nonsense("Microsoft Graph didn't say where the top of the mailbox is"));
    }
    Ok(known)
}

/// A folder in an answer: its id, its name, how many folders lie in it, and whether it is one
/// that isn't shown (hidden, or a search folder). `None` for what can't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub id: String,
    pub name: String,
    pub children: u64,
    pub unseen: bool,
    /// How many messages it has, and how many are unread (0 when not said).
    pub total: u32,
    pub unread: u32,
    /// Both counts were given (#276).
    pub counted: bool,
}

/// [`Entry`] of one JSON value of a list, or of the answer to a change.
pub(crate) fn entry_of(value: &Value) -> Option<Entry> {
    let id = id_of(text(value, "id"))?;
    let name = clean_name(&text(value, "displayName").chars().take(MAX_NAME).collect::<String>())?;
    // A count that isn't given is read as "maybe": the folder is looked into.
    let children = value.get("childFolderCount").map_or(Some(1), Value::as_u64)?;
    let hidden = value.get("isHidden").and_then(Value::as_bool).unwrap_or(false);
    let search = text(value, "@odata.type").to_ascii_lowercase().contains("mailsearchfolder");
    let count = |name: &str| value.get(name).and_then(Value::as_u64).map(|n| u32::try_from(n).unwrap_or(u32::MAX));
    let (total, unread) = (count("totalItemCount"), count("unreadItemCount"));
    let counted = total.is_some() && unread.is_some();
    Some(Entry { id, name, children, unseen: hidden || search, total: total.unwrap_or(0), unread: unread.unwrap_or(0), counted })
}

/// Every folder of the mailbox, parents first, a level of the tree at a time. A listing that was
/// cut short comes back as far as it got, with why.
fn listing(graph: &Graph<'_>, known: &Known) -> Result<FolderListing, (FolderListing, NetError)> {
    let query = http::form(&[("$top", FOLDER_PAGE), ("$select", FOLDER_FIELDS)]);
    let mut out = FolderListing { folders: Vec::new(), complete: true };
    let mut seen: HashSet<String> = HashSet::from([known.root.clone()]);
    // The folders whose subfolders are asked for next (a server id; empty: the top), all of one
    // depth.
    let mut level: Vec<String> = vec![String::new()];
    let mut depth = 0usize;
    while !level.is_empty() {
        let mut next: Vec<String> = Vec::new();
        for parents in level.chunks(BATCH_SIZE) {
            let room = MAX_LISTED_FOLDERS.saturating_sub(out.folders.len()).saturating_add(1);
            let answers = match children_of(graph, parents, &query, room) {
                Ok(answers) => answers,
                Err(e) => {
                    out.complete = false;
                    return Err((out, e));
                }
            };
            for (parent, (children, failed)) in parents.iter().zip(answers) {
                match failed {
                    None => out.complete &= children.whole && children.unreadable == 0,
                    // The server is busy, can't be reached, or won't take the sign-in: the walk
                    // ends here.
                    Some(e) if matches!(e, NetError::Busy { .. }) || e.is_unreachable() || e.is_refused_sign_in() => {
                        out.complete = false;
                        return Err((out, e));
                    }
                    // This folder's list couldn't be had (it went while the walk was under way, or
                    // the server stumbled): what is under it isn't known, and the walk goes on.
                    Some(e) => {
                        log::debug!("graph folders: a list of subfolders couldn't be read: {e}");
                        out.complete = false;
                    }
                }
                for entry in children.found {
                    if out.folders.len() >= MAX_LISTED_FOLDERS {
                        out.complete = false;
                        break;
                    }
                    // A hidden folder or a search folder is not shown, nor what lies under it;
                    // one that came twice is the first one.
                    if entry.unseen || !seen.insert(entry.id.clone()) {
                        continue;
                    }
                    if entry.children > 0 {
                        if depth + 1 < MAX_DEPTH {
                            next.push(entry.id.clone());
                        } else {
                            out.complete = false;
                        }
                    }
                    let kind = known.kinds.get(&entry.id).copied().unwrap_or_default();
                    out.folders.push(GraphFolder {
                        id: entry.id,
                        parent: parent.clone(),
                        name: entry.name,
                        kind,
                        total: entry.total,
                        unread: entry.unread,
                        counted: entry.counted,
                    });
                }
            }
        }
        level = next;
        depth += 1;
    }
    Ok(out)
}

/// The subfolders one folder's list brought: what was read, how many entries couldn't be, and
/// whether the list was read to its end.
#[derive(Default)]
struct Children {
    found: Vec<Entry>,
    unreadable: usize,
    whole: bool,
}

/// Where the subfolders of `parent` (a server id; empty: the top) are listed, with `query`.
fn children_path(graph: &Graph<'_>, parent: &str, query: &str) -> String {
    if parent.is_empty() {
        format!("{}?{query}", folders(graph))
    } else {
        format!("{}/{}/childFolders?{query}", folders(graph), http::encode(parent))
    }
}

/// The subfolders of every one of `parents` (at most [`BATCH_SIZE`]), `most` of each, in one
/// `$batch` (#276): one request for the lot, where asking each by itself took one apiece. What
/// each brought, and why its list stopped short when it did. A parent whose answer in the batch
/// isn't a page of folders (refused, busy, gone) is asked by itself, so that what becomes of it
/// is what became of it before; a batch the server won't take is asked one at a time. The error
/// is a batch that couldn't be asked at all.
fn children_of(graph: &Graph<'_>, parents: &[String], query: &str, most: usize) -> Result<Vec<(Children, Option<NetError>)>, NetError> {
    let mut answers: Vec<Option<(Children, Option<NetError>)>> = (0..parents.len()).map(|_| None).collect();
    if parents.len() > 1 {
        let requests: Vec<Value> = parents
            .iter()
            .enumerate()
            .map(|(n, parent)| {
                let url = graph.batch_path(&format!("/mailFolders/{}/childFolders?{query}", http::encode(parent)));
                json!({"id": n.to_string(), "method": "GET", "url": url, "headers": {"Prefer": IMMUTABLE_IDS}})
            })
            .collect();
        let body = json!({"requests": requests}).to_string();
        let reply =
            graph.call(Kind::Read, &graph.url(BATCH), "POST", &[("Content-Type", "application/json")], Some(&body), ASK_TIMEOUT, MAX_BATCH)?;
        if reply.status == 200 {
            let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            let said = json.get("responses").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            for one in said.iter().take(BATCH_SIZE * 2) {
                let n = one.get("id").and_then(Value::as_str).and_then(|n| n.parse::<usize>().ok());
                let (Some(slot), Some(parent)) = (n.and_then(|n| answers.get_mut(n)), n.and_then(|n| parents.get(n))) else { continue };
                if slot.is_some() || one.get("status").and_then(Value::as_u64) != Some(200) {
                    continue;
                }
                let Ok(page) = super::page_from(one.get("body").cloned().unwrap_or(Value::Null)) else { continue };
                let current = graph.url(&children_path(graph, parent, query));
                // A next page that can't be followed: the folder is asked by itself.
                let next = match &page.next {
                    Some(link) => match graph.next_page(link, &current) {
                        Ok(url) => Some(url),
                        Err(_) => continue,
                    },
                    None => None,
                };
                let mut children = Children { whole: true, ..Children::default() };
                for value in &page.entries {
                    if children.found.len().saturating_add(children.unreadable) >= most {
                        children.whole = false;
                        break;
                    }
                    match entry_of(value) {
                        Some(entry) => children.found.push(entry),
                        None => children.unreadable += 1,
                    }
                }
                let mut failed = None;
                if let Some(url) = next.filter(|_| children.whole) {
                    let left = most.saturating_sub(children.found.len().saturating_add(children.unreadable));
                    let (found, unreadable) = (&mut children.found, &mut children.unreadable);
                    match graph.list(Doing::Folders, url, &[], left, &mut |value| match entry_of(value) {
                        Some(entry) => found.push(entry),
                        None => *unreadable += 1,
                    }) {
                        Ok(whole) => children.whole = whole,
                        Err(e) => {
                            children.whole = false;
                            failed = Some(e);
                        }
                    }
                }
                *slot = Some((children, failed));
            }
        } else if matches!(reply.status, 401 | 429 | 503 | 504) {
            return Err(graph.refusal(Doing::Folders, &reply, &body));
        }
    }
    // What the batch didn't settle, one at a time, as every list was asked before.
    Ok(parents
        .iter()
        .zip(answers)
        .map(|(parent, answer)| {
            answer.unwrap_or_else(|| {
                let (mut found, mut unreadable) = (Vec::new(), 0usize);
                let listed =
                    graph.list(Doing::Folders, graph.url(&children_path(graph, parent, query)), &[], most, &mut |value| match entry_of(value) {
                        Some(entry) => found.push(entry),
                        None => unreadable += 1,
                    });
                match listed {
                    Ok(whole) => (Children { found, unreadable, whole }, None),
                    Err(e) => (Children { found, unreadable, whole: false }, Some(e)),
                }
            })
        })
        .collect())
}

/// Make one change on the server. `None` when it can't be made yet (the folder it goes in isn't
/// on the server). An error is what stops the run's changes (busy, no answer, no sign-in).
fn make_change(graph: &Graph<'_>, known: &Known, made: &HashMap<u64, String>, change: &FolderChange) -> Result<Option<ChangeOutcome>, NetError> {
    // The server's id of a folder it is asked about: one it wrote, and nothing else.
    let checked = |id: &str| id_of(id).ok_or_else(|| NetError::Config("a folder change names a folder the server can't have".into()));
    let name_of = |name: &str| clean_name(&name.chars().take(MAX_NAME).collect::<String>());
    match change {
        FolderChange::Create { folder: _, name, parent, parent_folder } => {
            let Some(name) = name_of(name) else { return Ok(Some(refused("a folder needs a name"))) };
            let parent = match (parent.is_empty(), parent_folder) {
                (false, _) => checked(parent)?,
                (true, Some(local)) => match made.get(local) {
                    Some(id) => id.clone(),
                    None => return Ok(None),
                },
                (true, None) => String::new(),
            };
            let path = if parent.is_empty() { folders(graph) } else { format!("{}/{}/childFolders", folders(graph), http::encode(&parent)) };
            let body = json!({"displayName": name}).to_string();
            let reply =
                graph.call(Kind::Once, &graph.url(&path), "POST", &[("Content-Type", "application/json")], Some(&body), WRITE_TIMEOUT, MAX_PAGE)?;
            refusing(graph, &reply)?;
            match reply.status {
                200 | 201 => Ok(Some(done(known, &reply, &parent)?)),
                // There is one of that name there: an earlier try whose answer was lost, or one
                // made elsewhere. It is that one.
                409 => Ok(Some(match child_named(graph, known, &parent, &name)? {
                    Some(there) => ChangeOutcome::Done { folder: there },
                    None => refused("the server has a folder of that name there"),
                })),
                404 => Ok(Some(ChangeOutcome::Gone)),
                _ => outcome_of(graph, &reply, &body),
            }
        }
        FolderChange::Rename { folder: _, id, name } => {
            let Some(name) = name_of(name) else { return Ok(Some(refused("a folder needs a name"))) };
            let id = checked(id)?;
            let body = json!({"displayName": name}).to_string();
            let url = graph.url(&format!("{}/{}", folders(graph), http::encode(&id)));
            let reply = graph.call(Kind::Same, &url, "PATCH", &[("Content-Type", "application/json")], Some(&body), WRITE_TIMEOUT, MAX_PAGE)?;
            refusing(graph, &reply)?;
            match reply.status {
                200 | 201 => Ok(Some(done_in_place(known, &reply)?)),
                409 => Ok(Some(refused("the server has a folder of that name there"))),
                404 | 410 => Ok(Some(ChangeOutcome::Gone)),
                _ => outcome_of(graph, &reply, &body),
            }
        }
        FolderChange::Move { folder: _, id, parent } => {
            let id = checked(id)?;
            let to = if parent.is_empty() { known.root.clone() } else { checked(parent)? };
            let body = json!({"destinationId": to}).to_string();
            let url = graph.url(&format!("{}/{}/move", folders(graph), http::encode(&id)));
            let reply = graph.call(Kind::Once, &url, "POST", &[("Content-Type", "application/json")], Some(&body), WRITE_TIMEOUT, MAX_PAGE)?;
            refusing(graph, &reply)?;
            match reply.status {
                200 | 201 => Ok(Some(done(known, &reply, &to)?)),
                409 => Ok(Some(refused("the server has a folder of that name there"))),
                404 | 410 => Ok(Some(ChangeOutcome::Gone)),
                _ => outcome_of(graph, &reply, &body),
            }
        }
        FolderChange::Delete { id } => {
            let id = checked(id)?;
            let url = graph.url(&format!("{}/{}", folders(graph), http::encode(&id)));
            let reply = graph.call(Kind::Same, &url, "DELETE", &[], None, WRITE_TIMEOUT, MAX_PAGE)?;
            refusing(graph, &reply)?;
            match reply.status {
                200..=299 => Ok(Some(ChangeOutcome::Done { folder: GraphFolder { id, ..Default::default() } })),
                404 | 410 => Ok(Some(ChangeOutcome::Gone)),
                _ => outcome_of(graph, &reply, ""),
            }
        }
    }
}

/// A shared mailbox that answered a change by Graph's code with no access, no mailbox or not
/// through Graph (#70): the run's changes stop there, waiting here, and are never taken for a
/// folder that is gone (a 404 `ErrorInvalidUser` would be).
fn refusing(graph: &Graph<'_>, reply: &Reply) -> Result<(), NetError> {
    if graph.shared() && !(200..300).contains(&reply.status) {
        let body = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
        if let Some(why) = super::shared_refusal(u64::from(reply.status), &body) {
            return Err(NetError::SharedRefused(why));
        }
    }
    Ok(())
}

fn refused(why: &str) -> ChangeOutcome {
    ChangeOutcome::Refused { why: why.to_string() }
}

/// The folder in the answer to a change, which now lies in `parent` (a server id).
fn done(known: &Known, reply: &Reply, parent: &str) -> Result<ChangeOutcome, NetError> {
    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    let entry = entry_of(&json).ok_or_else(|| nonsense("Microsoft Graph's answer about a folder isn't one"))?;
    let kind = known.kinds.get(&entry.id).copied().unwrap_or_default();
    Ok(ChangeOutcome::Done { folder: GraphFolder { id: entry.id, parent: known.parent_here(parent), name: entry.name, kind, ..Default::default() } })
}

/// [`done`] for a folder that stayed where it was: the answer says where that is.
fn done_in_place(known: &Known, reply: &Reply) -> Result<ChangeOutcome, NetError> {
    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    let parent = id_of(text(&json, "parentFolderId")).unwrap_or_default();
    done(known, reply, &parent)
}

/// What a status other than the hoped-for one means for a change: a refusal for good (said in
/// plain words), or an error that stops the run's changes (busy, no sign-in).
///
/// In a shared mailbox (#70) a refusal never puts back what the owner changed: one that is by
/// Graph's code no access (or no mailbox) stops the run's changes as [`NetError::SharedRefused`],
/// and any other 403 stops them too, the change waiting here for the next run.
fn outcome_of(graph: &Graph<'_>, reply: &Reply, sent: &str) -> Result<Option<ChangeOutcome>, NetError> {
    refusing(graph, reply)?;
    if graph.shared() && reply.status == 403 {
        return Err(graph.refusal(Doing::Folders, reply, sent));
    }
    match reply.status {
        400 => Ok(Some(refused("the server says the change isn't one it can make"))),
        403 => Ok(Some(refused("the server says this account may not make that change"))),
        _ => Err(graph.refusal(Doing::Folders, reply, sent)),
    }
}

/// The folder called `name` (in any case) in `parent` (a server id; empty: the top).
fn child_named(graph: &Graph<'_>, known: &Known, parent: &str, name: &str) -> Result<Option<GraphFolder>, NetError> {
    let query = http::form(&[("$top", FOLDER_PAGE), ("$select", FOLDER_FIELDS)]);
    let path = if parent.is_empty() {
        format!("{}?{query}", folders(graph))
    } else {
        format!("{}/{}/childFolders?{query}", folders(graph), http::encode(parent))
    };
    let mut found = None;
    let wanted = name.to_lowercase();
    graph.list(Doing::Folders, graph.url(&path), &[], MAX_LISTED_FOLDERS, &mut |value| {
        if let Some(entry) = entry_of(value).filter(|entry| found.is_none() && entry.name.to_lowercase() == wanted) {
            let kind = known.kinds.get(&entry.id).copied().unwrap_or_default();
            found = Some(GraphFolder { id: entry.id, parent: parent.to_string(), name: entry.name, kind, ..Default::default() });
        }
    })?;
    Ok(found)
}

mod down;
mod drafts;
mod fetch;
mod place;
mod search;
mod send;
mod up;

pub use fetch::fetch_messages;
pub use search::search_messages;

#[cfg(test)]
mod down_tests;
#[cfg(test)]
mod renew_tests;
#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod shared_tests;
#[cfg(test)]
mod skip_tests;
#[cfg(test)]
mod tests;
