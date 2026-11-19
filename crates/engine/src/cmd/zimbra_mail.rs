// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Zimbra account kept in step through a Zimbra server's own web service: mail down (Z3).
//!
//! As for IMAP, Microsoft Graph, JMAP and Exchange Web Services, the engine never talks to a
//! server: [`zimbra_mail_request`] (`sync.zimbraRequest`) says what a run should do, a host that can
//! reach the server does it (`mailcraft_zimbra::sync_mail`, on a background thread), and
//! [`apply_zimbra_mail`] (`sync.applyZimbra`) takes in what it brought back, piece by piece. The
//! same goes for messages fetched whole: [`zimbra_fetch_request`] (`sync.zimbraFetchRequest`) and
//! [`apply_zimbra_fetch`] (`sync.applyZimbraFetch`). All four are the host's side of a
//! synchronisation, never an agent's, and refuse while the feature's switch is off
//! ([`Session::zimbra`], set by the host from `MAILCRAFT_ZIMBRA=1`, never by a command's params).
//!
//! **Mail down.** A run reads the server's folders (a complete listing) and the mailbox's changes
//! since the token it went on from; with no token, every folder from its start. A piece is taken
//! in only while the account is at the token the run went on from, so that a run never moves the
//! mailbox past what was lost; the token that goes on from the run is kept with its last piece, and
//! only when the run went well. Nothing goes up yet: changes made here wait, and nothing is sent.
//!
//! **What comes back is hostile** (a host may hand over anything): the model holds a piece to
//! sizes ([`mailcraft_model::sync::ZimbraMailResult::check`]), then [`super::zimbra_down::bounded`]
//! cuts its text to what is shown and passes over what has an id that is no id.

use mailcraft_model::sync::{GraphFetchResult, ZimbraFetchRequest, ZimbraMailRequest, ZimbraMailResult};
use mailcraft_model::{Account, AccountKind, FolderKind, Id, Mailbox, ZimbraServer};
use serde_json::{Value, json};

use super::graph_down::{self, Down};
use super::graph_mail::{self, Taken};
use super::{bad, fail, zimbra_down};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// What a build says while Zimbra accounts are off ([`Session::zimbra`]).
pub const SWITCHED_OFF: &str = "Zimbra accounts are being tested: start MailCraft with MAILCRAFT_ZIMBRA=1 to try them";

/// What a build says of a Zimbra account's Send/Receive where it can't run.
pub const NOT_BUILT: &str = "Zimbra accounts are sent and received through Zimbra's web service, not as an IMAP account is";

/// Why a Zimbra account is not kept in step as an IMAP account is, in words for a log or a
/// command's error.
pub fn refusal_text(s: &Session) -> &'static str {
    if s.zimbra { NOT_BUILT } else { SWITCHED_OFF }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.zimbraRequest", "Send/Receive with Zimbra", "Send / Receive", request)
            .pure()
            .params(r#"{"account"?: number, "background"?: bool} — for a Zimbra account: answers what a host should read there (its folders, and its changes since the token it goes on from). Only while Zimbra accounts are being tested (MAILCRAFT_ZIMBRA=1)"#)
            .when(available),
        CommandSpec::new("sync.applyZimbra", "Apply Send/Receive with Zimbra", "Send / Receive", apply)
            .pure()
            .params(r#"{"result": ZimbraMailResult} — takes in what a host read from a Zimbra account. Only while Zimbra accounts are being tested (MAILCRAFT_ZIMBRA=1)"#),
        CommandSpec::new("sync.zimbraFetchRequest", "Download Zimbra Messages in Full", "Send / Receive", fetch_request)
            .pure()
            .params(r#"{"account"?: number, "message"?: number (one message, at once), "most"?: number (default 10)} — answers which messages of a Zimbra account a host should fetch whole. Only while Zimbra accounts are being tested (MAILCRAFT_ZIMBRA=1)"#)
            .when(available),
        CommandSpec::new("sync.applyZimbraFetch", "Apply Downloaded Zimbra Messages", "Send / Receive", apply_fetch)
            .pure()
            .params(r#"{"result": GraphFetchResult} — takes in messages a host fetched whole from a Zimbra account. Only while Zimbra accounts are being tested (MAILCRAFT_ZIMBRA=1)"#),
    ]
}

/// Why the commands are out of reach, when they are: the switch is off, or no account is one.
fn available(s: &Session) -> Option<&'static str> {
    if !s.zimbra {
        return Some(SWITCHED_OFF);
    }
    if s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Zimbra) { None } else { Some("no account is a Zimbra account") }
}

/// The Zimbra account a command names, or the first there is.
fn named_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Zimbra).map(|a| a.id))
        .ok_or_else(|| fail("no account is a Zimbra account"))
}

/// The account `account` and its server, when it is a Zimbra account whose server settings may be
/// used ([`ZimbraServer::problem`]: a mailbox file can say anything).
fn zimbra_account(mb: &Mailbox, account: Id) -> Result<(&Account, &ZimbraServer), CmdError> {
    let a = mb.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind != AccountKind::Zimbra {
        return Err(failed!("{name} isn't a Zimbra account", name = a.name));
    }
    let server = a.zimbra.as_ref().filter(|server| server.problem().is_none());
    let server = server.ok_or_else(|| failed!("{name} has no Zimbra server address that can be used", name = a.name))?;
    Ok((a, server))
}

/// What a Zimbra run of `account` should do. The host adds the password, which is never here.
pub fn zimbra_mail_request(s: &Session, account: Id) -> Result<ZimbraMailRequest, CmdError> {
    let (a, server) = zimbra_account(&s.mailbox, account)?;
    // A token that may not be one is not gone on from: the mailbox is read afresh. The same holds
    // for where a folder's fresh read stopped: an entry that can't be one is simply not gone on
    // from, and the folder is read from its start.
    let token = if ZimbraServer::token_ok(&server.token) { server.token.clone() } else { String::new() };
    let resume = server
        .resume
        .iter()
        .filter(|e| mailcraft_model::sync::zimbra_id_ok(&e.folder) && (e.cursor.is_empty() || mailcraft_model::sync::zimbra_id_ok(&e.cursor)))
        .cloned()
        .collect();
    Ok(ZimbraMailRequest {
        account,
        email: a.email.clone(),
        url: server.url.clone(),
        username: server.username.clone(),
        trusted: server.trusted.clone(),
        token,
        resume,
        // What was done here goes up first, as a Microsoft account's does: folders, messages, and
        // what waits in the Outbox. (Drafts stay here: a Zimbra account keeps none on its server yet.)
        changes: graph_mail::folder_changes(&s.mailbox, account, mailcraft_model::sync::MAX_FOLDER_CHANGES).0,
        messages: super::graph_up::message_changes(s, account, mailcraft_model::sync::MAX_MESSAGE_CHANGES).0,
        sends: super::graph_send::sends(s, account),
        ..ZimbraMailRequest::default()
    })
}

fn request(s: &mut Session, v: &Value) -> CmdResult {
    if !s.zimbra {
        return Err(fail(SWITCHED_OFF));
    }
    let account = named_account(s, v)?;
    // A message never dated is dated now, so that it is the same text at every run.
    let now = s.now();
    super::graph_send::date_undated(&mut s.mailbox, account, now);
    let mut request = zimbra_mail_request(s, account)?;
    request.background = p::bool(v, "background").unwrap_or(false);
    // A run begins: nothing lost in an earlier one is held against it.
    s.ews_run_cut.remove(&account);
    serde_json::to_value(request).map_err(|e| fail(e.to_string()))
}

/// Which messages of `account` a host should fetch whole: `message` at once (the owner opened it),
/// else the next `most` in the background, within the account's mail to keep offline.
pub fn zimbra_fetch_request(s: &Session, account: Id, message: Option<Id>, most: usize) -> Result<ZimbraFetchRequest, CmdError> {
    let (a, server) = zimbra_account(&s.mailbox, account)?;
    let ids = match message {
        Some(id) => {
            let m = s
                .mailbox
                .message(id)
                .filter(|m| m.account == account)
                .ok_or_else(|| failed!("no message with id {id} in {name}", id, name = a.name))?;
            m.server
                .as_ref()
                .filter(|server| graph_down::still_to_fetch(&s.mailbox, m) && mailcraft_model::sync::zimbra_id_ok(&server.id))
                .map(|server| vec![server.id.clone()])
                .unwrap_or_default()
        }
        None => graph_down::fetch_queue_passing_over(&s.mailbox, account, most.clamp(1, 100), s.now(), &s.graph_unfetchable)
            .into_iter()
            .filter(|id| mailcraft_model::sync::zimbra_id_ok(id))
            .collect(),
    };
    Ok(ZimbraFetchRequest {
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
    if !s.zimbra {
        return Err(fail(SWITCHED_OFF));
    }
    let account = named_account(s, v)?;
    let most = p::id(v, "most").and_then(|n| usize::try_from(n).ok()).unwrap_or(10);
    serde_json::to_value(zimbra_fetch_request(s, account, p::id(v, "message"), most)?).map_err(|e| fail(e.to_string()))
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    if !s.zimbra {
        return Err(fail(SWITCHED_OFF));
    }
    // What comes back is hostile: read as the type says, or refused, whatever it holds.
    let result = v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?;
    let result: ZimbraMailResult = serde_json::from_value(result).map_err(|_| fail("`result` isn't a Zimbra Send/Receive result"))?;
    apply_zimbra_mail(s, result)
}

fn apply_fetch(s: &mut Session, v: &Value) -> CmdResult {
    if !s.zimbra {
        return Err(fail(SWITCHED_OFF));
    }
    let result = v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?;
    let result: GraphFetchResult = serde_json::from_value(result).map_err(|_| fail("`result` isn't a fetch result"))?;
    apply_zimbra_fetch(s, result)
}

/// The account a result is for, when it may take it in: a Zimbra account, the one the run was for,
/// at the server the run read when the result says which. Answers its name.
fn receiving(mb: &Mailbox, account: Id, email: &str, url: &str) -> Result<String, CmdError> {
    let (a, server) = zimbra_account(mb, account)?;
    if !email.is_empty() && !a.email.trim().eq_ignore_ascii_case(email.trim()) {
        return Err(failed!("this Send/Receive was for another account, not for {name}", name = a.name));
    }
    if !url.is_empty() && server.base() != url.trim().trim_end_matches('/') {
        return Err(failed!("this Send/Receive was for the server {name} named before", name = a.name));
    }
    Ok(a.name.clone())
}

/// Take in a piece of what a Zimbra run brought back (pieces come in the order the run made them,
/// each whole in itself): the folder list, then the pages, and at the end of a run that went well,
/// the token that goes on from it, and the messages the server no longer has are removed. Not an
/// undo step: the server is the source of truth, but for what was changed here since it last said.
pub fn apply_zimbra_mail(s: &mut Session, result: ZimbraMailResult) -> CmdResult {
    let account = result.account;
    let taken = take_in(s, result);
    // A piece that was refused is a piece lost: the end of its run removes nothing.
    if taken.is_err() {
        s.ews_run_cut.insert(account);
    }
    taken
}

fn take_in(s: &mut Session, result: ZimbraMailResult) -> CmdResult {
    result.check().map_err(|why| failed!("the result {why}", why = mailcraft_i18n::Arg::words(why)))?;
    let account = result.account;
    let name = receiving(&s.mailbox, account, &result.email, &result.url)?;
    // A piece of a run that went on from another token than the account's: the mailbox has moved
    // on, or back, since the run began, and the piece would carry it past what was lost.
    let at = s.mailbox.account(account).and_then(|a| a.zimbra.as_ref()).map(|z| z.token.clone()).unwrap_or_default();
    if result.from != at {
        return Err(fail("this Send/Receive went on from where the account no longer is: the next one reads again"));
    }
    let (result, mut cut) = zimbra_down::bounded(result);
    if s.file_is_current() {
        s.note_saved();
    }
    let mut taken = Taken::default();
    let mut down = Down::default();
    // What became of what went up, as for a Microsoft account: the folders, the messages (a refusal
    // is said once per kind and reason), and what was sent.
    let mut errors = result.errors.clone();
    for done in &result.changes {
        if let Some(said) = graph_mail::change_done(&mut s.mailbox, account, &done.change, &done.outcome, s.interface_language, &mut taken) {
            errors.push(said);
        }
    }
    let mut refusals = super::graph_up::Refusals::default();
    for done in &result.messages {
        if super::graph_up::message_done(&mut s.mailbox, account, done, &mut refusals) {
            match done.outcome {
                mailcraft_model::sync::MessageOutcome::Done { .. } => taken.pushed += 1,
                _ => taken.updated += 1,
            }
            s.graph_index = Default::default();
        }
    }
    let mut refused = refusals.lines();
    let mut sent = 0usize;
    for done in &result.sent {
        let sent_at = s.now();
        if super::graph_send::send_done_at(&mut s.mailbox, account, done, s.interface_language, &mut refused, Some(sent_at)) {
            sent += usize::from(matches!(done.outcome, mailcraft_model::sync::SendOutcome::Sent));
            taken.updated += 1;
            s.graph_index = Default::default();
        }
    }
    errors.extend(refused.iter().cloned());
    if let Some(listing) = &result.listing {
        down.removed = zimbra_down::take_folders(&mut s.mailbox, account, listing, &result.well_known, &mut taken);
    }
    let mut index = std::mem::take(&mut s.graph_index);
    for page in &result.pages {
        cut |= !zimbra_down::take_page(&mut s.mailbox, &mut index, account, page, &mut down);
    }
    s.graph_index = index;
    if down.left > 0 {
        s.forget_history();
    }
    if cut {
        s.ews_run_cut.insert(account);
    }
    let run_cut = s.ews_run_cut.contains(&account);
    if !result.partial {
        s.ews_run_cut.remove(&account);
    }
    // The run's end: what its reads left out goes, and the mailbox goes on from the run's token,
    // in one step, when it went well. With a folder left half-read (`resume`), nothing marked gone
    // is gone yet — the tail of that folder's read is still to come — so nothing is removed until
    // a run leaves no folder half-read.
    if result.whole && !result.partial && !run_cut {
        if result.resume.as_ref().is_none_or(Vec::is_empty) {
            graph_down::sweep(&mut s.mailbox, account, None, &mut down);
        }
        if !result.token.is_empty()
            && let Some(server) = s.mailbox.account_mut(account).ok().and_then(|a| a.zimbra.as_mut())
        {
            server.token = result.token.clone();
            if let Some(resume) = &result.resume {
                server.resume = resume.clone();
            }
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
    if result.auth_refused {
        errors.insert(0, mailcraft_i18n::fmt(lang.t("The server turned down the sign-in for {name}: sign in again"), &[("name", &name)]));
    }
    if !result.partial {
        let said = match errors.first() {
            Some(first) => first.clone(),
            None if result.stopped => lang.t("Stopped; carries on from here next time").to_string(),
            None => lang.t("Up to date").to_string(),
        };
        let t = s.local_now();
        s.sync_status = format!("{said} — {}", s.time_said(t));
    }
    let summary = errors.first().cloned().unwrap_or_else(|| "Up to date".to_string());
    Ok(json!({
        "pushed": taken.pushed,
        "sent": sent,
        "added": taken.added,
        "updated": taken.updated,
        "removed": taken.removed,
        "errors": errors,
        "busyFor": result.busy_for,
        "authRefused": result.auth_refused,
        "untrusted": result.untrusted,
        "stopped": result.stopped,
        "new": down.new,
        "changed": down.changed,
        "gone": down.gone,
        "removedMessages": down.removed,
        "summary": summary,
    }))
}

/// Take in messages of a Zimbra account fetched whole, as a Microsoft account's are
/// ([`graph_mail::apply_graph_fetch`]), once the result is held to sizes. Not an undo step. Only
/// for the account they were fetched for.
pub fn apply_zimbra_fetch(s: &mut Session, result: GraphFetchResult) -> CmdResult {
    let account = result.account;
    receiving(&s.mailbox, account, &result.email, "")?;
    let result = super::ews_down::bounded_fetch(result);
    let missing: Vec<(Id, String)> = result.missing.iter().map(|id| (account, id.clone())).collect();
    let answer = graph_mail::apply_graph_fetch(s, result)?;
    s.graph_unfetchable.extend(missing);
    Ok(answer)
}

#[cfg(test)]
#[path = "zimbra_mail_tests.rs"]
mod tests;
