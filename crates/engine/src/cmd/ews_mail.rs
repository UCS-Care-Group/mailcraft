// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Exchange server account kept in step through Exchange Web Services (#252), E1 and E6: the
//! switch, and mail down.
//!
//! As for IMAP, Microsoft Graph and JMAP, the engine never talks to a server: [`ews_mail_request`]
//! (`sync.ewsRequest`) says what a run should read, a host that can reach the server does it
//! (`mailcraft_ews::sync_mail`, on a background thread), and [`apply_ews_mail`]
//! (`sync.applyEws`) takes in what it brought back, piece by piece. The same goes for messages
//! fetched whole: [`ews_fetch_request`] (`sync.ewsFetchRequest`) and [`apply_ews_fetch`]
//! (`sync.applyEwsFetch`). All four are the host's side of a synchronisation, never an agent's.
//!
//! **The switch.** While the feature's switch is off ([`Session::ews`], set by the host from
//! `MAILCRAFT_EWS=1`, never by a command's params) every one of them refuses with
//! [`SWITCHED_OFF`].
//!
//! **Mail down, and changes up.** A run first takes up what was done here (folder changes, message
//! changes, drafts, what waits in the Outbox: the same pieces as a Microsoft account's, worked out
//! from the difference with what the server last said, `graph_up`, `graph_drafts`, `graph_send`),
//! then reads the server's folders (a delta from the sync state the server gave) and each folder's
//! messages as headers and a preview from the folder's sync state. Sending is through Exchange
//! Web Services alone ([`can_queue_mail`]); a send whose answer was lost is held, as a Microsoft
//! account's is. Undo is a reverse change, which the next run sends.
//!
//! **What comes back is hostile** (a host may hand over anything): the model holds a piece to
//! sizes ([`mailcraft_model::sync::EwsMailResult::check`]), then [`super::ews_down::bounded`] cuts
//! its text to what is shown and passes over what has an id that is no id. A piece that had to be
//! cut removes nothing, as a run that went wrong removes nothing.
//!
//! **Taking in** goes the way of a Graph run, by server id: the folder list makes and updates the
//! folders; each page is a page of a folder's changes ([`super::ews_down::take_page`]), taken in with
//! the sync state that goes on from it; a folder read afresh (the server lost its state) marks its
//! messages gone first, and at the end of a run that went well, what the fresh read didn't bring is
//! removed ([`super::graph_down::sweep`]). Messages are matched by the server's item id, which stays
//! with a message while it is where it is. A mark the server changed comes down, but for one
//! changed here since the server last said. No server id and no sync state is logged.

use mailcraft_model::sync::{
    EwsFetchRequest, EwsFolderState, EwsMailRequest, EwsMailResult, EwsScope, GraphFetchResult, MAX_DRAFTS, MAX_EWS_STATE_BYTES, MAX_EWS_STATES,
    MAX_FOLDER_CHANGES, MAX_MESSAGE_CHANGES, MessageOutcome, SendOutcome, SyncPhase,
};
use mailcraft_model::{Account, AccountKind, EwsServer, FolderKind, Id, Mailbox};
use serde_json::{Value, json};

use super::graph_down::{self, Down};
use super::graph_mail::{self, Taken};
use super::graph_up::{self, Refusals};
use super::sync_progress::{self, Piece};
use super::{bad, ews_down, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// What a build says while Exchange server accounts are off ([`Session::ews`]).
pub const SWITCHED_OFF: &str = "Exchange server accounts are being tested: start MailCraft with MAILCRAFT_EWS=1 to try them";

/// What a piece is refused with when the account would keep more sync state than is taken in.
/// Most folders a run names as unreadable in its lines (the rest are counted in the log).
const MAX_BLOCKED_SHOWN: usize = 8;
const STATES_TOO_LARGE: &str = "the result would make this account keep more sync state than is taken in";

/// Whether a message may be queued in the Outbox of an account of this kind, to be sent by the
/// next Send/Receive. The one place that says so, for a message being written, an answer to a
/// meeting and a forwarded meeting alike: an account whose kind sends no mail, or whose sending is
/// off, has nowhere for the message, and an Outbox would keep it for ever.
pub(crate) fn can_queue_mail(s: &Session, kind: AccountKind) -> bool {
    kind.sends_mail() && (!kind.needs_switch() || s.ews)
}

/// Why a message can't be queued for an account of this kind ([`can_queue_mail`]), in words that
/// follow "…: send it from another account". For a kind that has a server; the same words as ever
/// for a JMAP account.
pub(crate) fn not_sending_text(_s: &Session, kind: AccountKind) -> &'static str {
    match kind {
        AccountKind::Exchange => SWITCHED_OFF,
        _ => mailcraft_model::NOT_SENDING_YET,
    }
}

/// Why an Exchange server account is not sent and received as an IMAP account is, in words for a
/// log or a command's error.
pub fn refusal_text(s: &Session) -> &'static str {
    if s.ews { "an Exchange server account is sent and received through Exchange Web Services, not as an IMAP account is" } else { SWITCHED_OFF }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.ewsRequest", "Send/Receive with Exchange Web Services", "Send / Receive", request)
            .pure()
            .params(r#"{"account"?: number, "light"?: bool (the Inbox and the folder on screen only), "inboxPass"?: bool (the Inbox alone), "background"?: bool} — for an Exchange server account: answers what a host should read there. Only while Exchange server accounts are being tested (MAILCRAFT_EWS=1)"#)
            .when(available),
        CommandSpec::new("sync.applyEws", "Apply Send/Receive with Exchange Web Services", "Send / Receive", apply)
            .pure()
            .params(r#"{"result": EwsMailResult} — takes in what a host read from an Exchange server account. Only while Exchange server accounts are being tested (MAILCRAFT_EWS=1)"#),
        CommandSpec::new("sync.ewsFetchRequest", "Download Exchange Messages in Full", "Send / Receive", fetch_request)
            .pure()
            .params(r#"{"account"?: number, "message"?: number (one message, at once), "most"?: number (default 10)} — answers which messages of an Exchange server account a host should fetch whole: the one asked for, or the next ones in the background (within the account's mail to keep offline and its Download messages in full). Only while Exchange server accounts are being tested (MAILCRAFT_EWS=1)"#)
            .when(available),
        CommandSpec::new("sync.applyEwsFetch", "Apply Downloaded Exchange Messages", "Send / Receive", apply_fetch)
            .pure()
            .params(r#"{"result": GraphFetchResult} — takes in messages a host fetched whole from an Exchange server account. Only while Exchange server accounts are being tested (MAILCRAFT_EWS=1)"#),
    ]
}

/// Why the commands are out of reach, when they are: the switch is off, or no account is one.
fn available(s: &Session) -> Option<&'static str> {
    if !s.ews {
        return Some(SWITCHED_OFF);
    }
    if s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Exchange) { None } else { Some("no account is an Exchange server account") }
}

/// The Exchange server account a command names, or the first there is.
fn named_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Exchange).map(|a| a.id))
        .ok_or_else(|| fail("no account is an Exchange server account"))
}

/// The account `account` and its server, when it is an Exchange server account whose server
/// settings may be used ([`EwsServer::problem`]: a mailbox file can say anything).
fn ews_account(mb: &Mailbox, account: Id) -> Result<(&Account, &EwsServer), CmdError> {
    let a = mb.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind != AccountKind::Exchange {
        return Err(failed!("{name} isn't an Exchange server account", name = a.name));
    }
    let server = a.ews.as_ref().filter(|server| server.problem().is_none());
    let server = server.ok_or_else(|| failed!("{name} has no Exchange server address that can be used", name = a.name))?;
    Ok((a, server))
}

/// The request for `account` with what every run says: where the server is, whose mailbox it is.
fn base(s: &Session, account: Id) -> Result<EwsMailRequest, CmdError> {
    let (a, server) = ews_account(&s.mailbox, account)?;
    Ok(EwsMailRequest {
        account,
        email: a.email.clone(),
        url: server.url.clone(),
        username: server.username.clone(),
        trusted: server.trusted.clone(),
        ..Default::default()
    })
}

/// What an Exchange run of `account` should read. The host adds the password, which is never here.
///
/// - **Scope.** `light`: the Inbox and the folder on screen, for a run nobody waits on; otherwise
///   every folder ([`EwsScope::All`]).
/// - **The folder list** is read in a run that reads every folder, and in the first run of the
///   account (none of its folders is the server's yet): from the state the server gave, so that
///   only what changed comes.
/// - **States.** Where each folder goes on from: its sync state, empty for a folder not read yet.
/// - **What goes up** (the same in a light run): the changes of folders and of messages made here
///   since the server said, the drafts saved here, and what waits in the Outbox, as for a Microsoft
///   account ([`graph_mail::folder_changes`], [`graph_up::message_changes`]). The Inbox pass
///   ([`ews_inbox_request`]) sends and changes nothing.
pub fn ews_mail_request(s: &Session, account: Id, light: bool) -> Result<EwsMailRequest, CmdError> {
    let mut request = base(s, account)?;
    let (_, server) = ews_account(&s.mailbox, account)?;
    let mb = &s.mailbox;
    let listed = mb.folders.iter().any(|f| f.account == account && f.server.is_some());
    let server_id = |folder: Id| mb.folder(folder).filter(|f| f.account == account).and_then(|f| f.server.as_ref()).map(|server| server.id.clone());
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).and_then(server_id);
    request.scope = if light {
        // (The Inbox is read anyway.)
        let on_screen = server_id(s.view.folder).filter(|id| Some(id) != inbox.as_ref());
        EwsScope::Light { also: on_screen.into_iter().collect() }
    } else {
        EwsScope::All
    };
    request.changes = graph_mail::folder_changes(mb, account, MAX_FOLDER_CHANGES).0;
    request.messages = graph_up::message_changes(s, account, MAX_MESSAGE_CHANGES).0;
    request.drafts = super::graph_drafts::draft_changes(s, account, MAX_DRAFTS).0;
    request.sends = super::graph_send::sends(s, account);
    request.list_folders = !light || !listed;
    request.hierarchy = server.hierarchy.clone();
    let wanted = |id: &str| match &request.scope {
        EwsScope::All => true,
        EwsScope::Light { also } => inbox.as_deref() == Some(id) || also.iter().any(|a| a == id),
    };
    request.states = mb
        .folders
        .iter()
        .filter(|f| f.account == account)
        .filter_map(|f| f.server.as_ref())
        .filter(|server| wanted(&server.id))
        .take(MAX_EWS_STATES)
        .map(|server| EwsFolderState { folder: server.id.clone(), state: server.sync_state.clone() })
        .collect();
    Ok(request)
}

/// The run that reads the Inbox alone, from its sync state: the first thing a Send/Receive does for
/// each account, so that new mail shows before the long read of the rest. None until the account's
/// Inbox is the server's.
pub fn ews_inbox_request(s: &Session, account: Id) -> Option<EwsMailRequest> {
    let mb = &s.mailbox;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).and_then(|f| mb.folder(f)).and_then(|f| f.server.as_ref())?;
    let mut request = base(s, account).ok()?;
    request.inbox_pass = true;
    request.scope = EwsScope::Light { also: Vec::new() };
    request.states = vec![EwsFolderState { folder: inbox.id.clone(), state: inbox.sync_state.clone() }];
    Some(request)
}

fn request(s: &mut Session, v: &Value) -> CmdResult {
    if !s.ews {
        return Err(fail(SWITCHED_OFF));
    }
    let account = named_account(s, v)?;
    let mut request = if p::bool(v, "inboxPass").unwrap_or(false) {
        ews_inbox_request(s, account).ok_or_else(|| fail("the Inbox of this account hasn't been read yet"))?
    } else {
        ews_mail_request(s, account, p::bool(v, "light").unwrap_or(false))?
    };
    request.background = p::bool(v, "background").unwrap_or(false);
    // A run begins: nothing lost in an earlier one is held against it.
    if !request.inbox_pass {
        s.ews_run_cut.remove(&account);
    }
    serde_json::to_value(request).map_err(|e| fail(e.to_string()))
}

/// Which messages of `account` a host should fetch whole: `message` at once (the owner opened it,
/// or is replying to it), else the next `most` in the background: those that aren't here whole,
/// within the account's mail to keep offline and its Download messages in full
/// ([`graph_down::fetch_queue_passing_over`]).
pub fn ews_fetch_request(s: &Session, account: Id, message: Option<Id>, most: usize) -> Result<EwsFetchRequest, CmdError> {
    let (a, server) = ews_account(&s.mailbox, account)?;
    let ids = match message {
        Some(id) => {
            let m = s
                .mailbox
                .message(id)
                .filter(|m| m.account == account)
                .ok_or_else(|| failed!("no message with id {id} in {name}", id, name = a.name))?;
            m.server.as_ref().filter(|server| m.unfetched && ews_down::is_id(&server.id)).map(|server| vec![server.id.clone()]).unwrap_or_default()
        }
        None => graph_down::fetch_queue_passing_over(&s.mailbox, account, most.clamp(1, 100), s.now(), &s.graph_unfetchable)
            .into_iter()
            .filter(|id| ews_down::is_id(id))
            .collect(),
    };
    Ok(EwsFetchRequest {
        account,
        email: a.email.clone(),
        url: server.url.clone(),
        username: server.username.clone(),
        trusted: server.trusted.clone(),
        ids,
        background: message.is_none(),
    })
}

fn fetch_request(s: &mut Session, v: &Value) -> CmdResult {
    if !s.ews {
        return Err(fail(SWITCHED_OFF));
    }
    let account = named_account(s, v)?;
    let most = p::id(v, "most").and_then(|n| usize::try_from(n).ok()).unwrap_or(10);
    serde_json::to_value(ews_fetch_request(s, account, p::id(v, "message"), most)?).map_err(|e| fail(e.to_string()))
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    if !s.ews {
        return Err(fail(SWITCHED_OFF));
    }
    // What comes back is hostile: read as the type says, or refused, whatever it holds.
    let result = v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?;
    let result: EwsMailResult = serde_json::from_value(result).map_err(|_| fail("`result` isn't an Exchange Send/Receive result"))?;
    apply_ews_mail(s, result)
}

fn apply_fetch(s: &mut Session, v: &Value) -> CmdResult {
    if !s.ews {
        return Err(fail(SWITCHED_OFF));
    }
    let result = v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?;
    let result: GraphFetchResult = serde_json::from_value(result).map_err(|_| fail("`result` isn't a fetch result"))?;
    apply_ews_fetch(s, result)
}

/// The account a result is for, when it may take it in: an Exchange server account, the one the
/// run was for, and at the server the run read when the result says which (`url`). (An answer for
/// another account, the mailbox having been replaced while the run was under way, would tie this
/// one's folders to a stranger's server; so would an answer from the server an account named
/// before its address was changed.)
fn receiving(mb: &Mailbox, account: Id, email: &str, url: &str) -> Result<String, CmdError> {
    let (a, server) = ews_account(mb, account)?;
    if !email.is_empty() && !a.email.trim().eq_ignore_ascii_case(email.trim()) {
        return Err(failed!("this Send/Receive was for another account, not for {name}", name = a.name));
    }
    if !url.is_empty() && server.url.trim() != url.trim() {
        return Err(failed!("this Send/Receive was for the server {name} named before", name = a.name));
    }
    Ok(a.name.clone())
}

/// Take in a piece of what an Exchange run brought back (pieces come in the order the run made
/// them, each whole in itself): the folder list, then the pages of the folders, and at the end of a
/// run that went well, the messages that a fresh read left out are removed. Not an undo step: the
/// server is the source of truth, but for what was changed here since it last said.
///
/// The piece is held to sizes first ([`mailcraft_model::sync::EwsMailResult::check`], then
/// [`ews_down::bounded`]); a piece that had to be cut, a run that went wrong, was stopped, or was
/// not a read of every folder removes nothing.
///
/// Answers what it changed, with the keys a Graph run's answer has (`added`, `updated` and
/// `removed` folders; `new`, `changed`, `gone` and `removedMessages` messages; `errors`,
/// `busyFor`, `summary`), `authRefused` when the server turned the password down and `untrusted`
/// (the certificate's fingerprint) when the run stopped at a certificate the owner is to be asked about.
pub fn apply_ews_mail(s: &mut Session, result: EwsMailResult) -> CmdResult {
    let account = result.account;
    let taken = take_in(s, result);
    // A piece that was refused is a piece lost: the end of its run removes nothing.
    if taken.is_err() && s.ews {
        s.ews_run_cut.insert(account);
    }
    taken
}

fn take_in(s: &mut Session, result: EwsMailResult) -> CmdResult {
    if !s.ews {
        return Err(fail(SWITCHED_OFF));
    }
    result.check().map_err(|why| failed!("the result {why}", why = mailcraft_i18n::Arg::words(why)))?;
    let account = result.account;
    let name = receiving(&s.mailbox, account, &result.email, &result.url)?;
    let (result, cut) = ews_down::bounded(result);
    if ews_down::state_bytes_after(&s.mailbox, account, &result) > MAX_EWS_STATE_BYTES {
        // (The sync states kept for one account are held to a total, as one is held to a size: a
        // piece that would go over is refused whole, and what it was for is read again.)
        return Err(fail(STATES_TOO_LARGE));
    }
    if s.file_is_current() {
        s.note_saved();
    }
    let mut taken = Taken::default();
    let mut down = Down::default();
    // What became of what went up, as for a Microsoft account: the folders, the messages (a
    // refusal is said once per kind and reason), the drafts, and what was sent.
    let mut errors = result.errors.clone();
    for done in &result.changes {
        if let Some(said) = graph_mail::change_done(&mut s.mailbox, account, &done.change, &done.outcome, &mut taken) {
            errors.push(said);
        }
    }
    let mut refusals = Refusals::default();
    for done in &result.messages {
        if graph_up::message_done(&mut s.mailbox, account, done, &mut refusals) {
            match done.outcome {
                MessageOutcome::Done { .. } => taken.pushed += 1,
                _ => taken.updated += 1,
            }
            // (A moved message has an id of its own in its new folder.)
            s.graph_index = Default::default();
        }
    }
    let mut refused = refusals.lines();
    for done in &result.drafts {
        if super::graph_drafts::draft_done(&mut s.mailbox, account, done, s.interface_language, &mut refused) {
            taken.updated += 1;
            s.graph_index = Default::default();
        }
    }
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
    if let Some(deltas) = &result.folders {
        down.removed = ews_down::take_folders(&mut s.mailbox, account, deltas, &mut taken);
        // The folder list goes on from here (with the folders it brought, in one step).
        if !result.hierarchy.is_empty()
            && let Some(server) = s.mailbox.account_mut(account).ok().and_then(|a| a.ews.as_mut())
        {
            server.hierarchy = result.hierarchy.clone();
        }
    }
    let mut index = std::mem::take(&mut s.graph_index);
    // A page that doesn't follow the folder's state is left out ([`ews_down::take_page`]): what was
    // lost before it is not skipped over, and a run that left a page out removes nothing.
    let mut left_out = 0usize;
    for page in &result.pages {
        if !ews_down::take_page(&mut s.mailbox, &mut index, account, page, &mut down) {
            left_out = left_out.saturating_add(1);
        }
    }
    s.graph_index = index;
    // A rule moved new mail out of the account (#567): there is no going back past it
    // ([`super::graph_mail::apply_graph_mail`] says why).
    if down.left > 0 {
        s.forget_history();
    }
    // The run's end: what its fresh reads left out goes, when every folder was read and it went
    // well (the host says `whole` of such a run only).
    // A page lost anywhere in the run, not in this piece alone, keeps the end from sweeping.
    if cut || left_out > 0 || !result.blocked.is_empty() {
        s.ews_run_cut.insert(account);
    }
    let run_cut = s.ews_run_cut.contains(&account);
    if !result.partial {
        s.ews_run_cut.remove(&account);
    }
    if result.whole && !result.partial && !run_cut {
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
    let lang = s.interface_language;
    if result.auth_refused {
        errors.insert(0, mailcraft_i18n::fmt(lang.t("The server turned down the sign-in for {name}: sign in again"), &[("name", &name)]));
    }
    if !result.untrusted.is_empty() {
        errors.insert(0, mailcraft_i18n::fmt(lang.t("The certificate of the server of {name} needs your say"), &[("name", &name)]));
    }
    // The folders that hold a change that can't be taken in stay where they were, and are named as
    // the owner knows them (no id, no state): one line each, up to a few.
    let blocked: Vec<String> = result
        .blocked
        .iter()
        .filter_map(|id| s.mailbox.folder_by_server(account, id).and_then(|f| s.mailbox.folder(f)).map(|f| sync_progress::shown(&f.name)))
        .take(MAX_BLOCKED_SHOWN)
        .collect();
    for folder in &blocked {
        errors.push(mailcraft_i18n::fmt(lang.t("{folder} couldn't be read from the server"), &[("folder", folder)]));
    }
    // Send/Receive Progress: where the run is, what it brought, what went wrong.
    let step = match (&result.progress, &result.folders) {
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
        sent,
        problems: &errors,
        last: !result.partial,
        stopped: result.stopped,
        timings: None,
    };
    s.sync_progress.take(account, piece);
    let summary = errors.first().cloned().unwrap_or_else(|| "Up to date".to_string());
    if !result.partial {
        // What the server refused in this run, said once, ahead of anything else.
        let refused = std::mem::take(&mut s.graph_said);
        let fresh: Vec<String> = refused.into_iter().filter(|line| !errors.contains(line)).collect();
        errors.splice(0..0, fresh);
    }
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
        "pushed": taken.pushed,
        "added": taken.added,
        "updated": taken.updated,
        "removed": taken.removed,
        "errors": errors,
        "busyFor": result.busy_for,
        "authRefused": result.auth_refused,
        "untrusted": result.untrusted,
        "unreadableFolders": blocked,
        "stopped": result.stopped,
        "new": down.new,
        "changed": down.changed,
        "gone": down.gone,
        "removedMessages": down.removed,
        "summary": summary,
    }))
}

/// Take in messages of an Exchange server account fetched whole, as a Microsoft account's are
/// ([`graph_mail::apply_graph_fetch`]), once the result is held to sizes ([`ews_down::bounded_fetch`]).
/// Not an undo step. Only for the account they were fetched for.
///
/// A message the server said it no longer has stays as it is (the next read says whether it is
/// gone), and the background doesn't ask for it again this session; nor for one the server
/// wouldn't give, as for Graph.
pub fn apply_ews_fetch(s: &mut Session, result: GraphFetchResult) -> CmdResult {
    if !s.ews {
        return Err(fail(SWITCHED_OFF));
    }
    let account = result.account;
    receiving(&s.mailbox, account, &result.email, "")?;
    let result = ews_down::bounded_fetch(result);
    let missing: Vec<(Id, String)> = result.missing.iter().map(|id| (account, id.clone())).collect();
    let answer = graph_mail::apply_graph_fetch(s, result)?;
    s.graph_unfetchable.extend(missing);
    Ok(answer)
}

#[cfg(test)]
#[path = "ews_mail_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ews_up_tests.rs"]
mod up_tests;
