// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sending through Microsoft Graph (R3.4.2; R3 spec S27, S28).
//!
//! What waits in a Microsoft account's Outbox goes with its run: a message saved as a draft is
//! sent as that draft (brought up to date first), one never saved with `sendMail`. A message that
//! went leaves the Outbox; the server's copy in Sent Items comes down by delta, and nothing is
//! uploaded here. A **held** message ([`Message::held`]) is one whose answer was lost (it may have
//! gone) or that the server refused: it stays in the Outbox, says what happened and what to check,
//! and isn't sent again until the owner says so (Send Again).
//!
//! A shared mailbox's Outbox (#70) goes the same way, through its own address: the message is
//! from it, and the copy is in its Sent Items. When Exchange won't let the owner send as it nor
//! on its behalf, or the owner doesn't allow sending from shared mailboxes (`Mail.Send.Shared`,
//! asked for at the first send: `sync.holdSends`), what waits is held with that reason. It is
//! never sent from the owner's own address instead.

use mailcraft_model::sync::{MAX_SENDS, SEND_AS_DENIED, SendDone, SendOutcome, SendUp};
use mailcraft_model::{FolderKind, Id, Mailbox, Queued};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mail.sendAgain", "Send Again", "Mail", send_again)
            .params(r#"{"id"?: number} (default: the selected message; an `id` that is null or isn't an id is refused) — a held message in the Outbox goes with the next Send/Receive"#)
            .selection(super::needs_message),
        CommandSpec::new("sync.holdSends", "Hold Unsent Mail", "Send / Receive", hold_sends)
            .params(r#"{"account": number (a shared mailbox), "why": "declined"|"approval"|"organisation"} — what waits in a shared mailbox's Outbox is held, with that reason: sending from shared mailboxes was not allowed on Microsoft's page ("declined"), waits on an administrator's approval ("approval"), or the organisation doesn't allow it ("organisation"). Nothing is sent from another address. Send Again puts a held message back. Answers {"held": [ids]}"#),
    ]
}

/// Whether `m` was delivered by the server (into a folder 0.8.0 and before followed as the
/// Outbox) rather than queued here: it has a server copy and is not a draft saved or sent from
/// here (those carry MailCraft's marker, or a change of the owner's waiting to go up, which only
/// the owner's own saves and sends make: a draft begun elsewhere and sent here before it had a
/// marker of its own). Only what the owner queued here is ever sent (AU2-V2).
pub fn from_server(m: &mailcraft_model::Message) -> bool {
    m.server.is_some() && m.server_draft.as_ref().is_none_or(|sd| sd.marker.is_empty() && !sd.pending)
}

/// What `account` has waiting to be sent: every message of its Outbox that isn't held, whole.
/// What a message keeps on disk is read for it (R6.1); one whose content can't be read waits in
/// the Outbox for the next run, with a line in the log, rather than going without its files.
pub fn sends(s: &Session, account: Id) -> Vec<SendUp> {
    let mb = &s.mailbox;
    let Some(outbox) = mb.folder_of_kind(account, FolderKind::Outbox) else { return Vec::new() };
    let exchange = mb.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Exchange);
    let mut waiting: Vec<_> = mb.messages_in(outbox).filter(|m| m.held.is_empty() && !from_server(m)).map(|m| m.id).collect();
    waiting.sort_unstable();
    waiting
        .into_iter()
        .take(MAX_SENDS)
        .filter_map(|id| match s.loaded_message(id) {
            Ok(m) => Some(m),
            Err(e) => {
                log::warn!("message {id} stays in the Outbox this time: {e}");
                None
            }
        })
        .map(|m| match m.server_draft.as_ref().and(super::graph_drafts::draft_up_whole(&m)) {
            // An Exchange server account's message always goes as its MIME, with the server filing
            // the copy in Sent Items (not every server files one when a saved draft is sent); the
            // draft it was saved as is named so that the run takes it away once the send is clear
            // (E8).
            Some(draft) if exchange => SendUp { message: m.id, draft: Some(draft), mime: Some(Box::new(m.as_ref().clone())) },
            // Sent as it is here, every field and attachment written over what the server has:
            // whatever was changed on the server meanwhile is the owner's to lose (AU2-14).
            Some(draft) => SendUp { message: m.id, draft: Some(mailcraft_model::sync::DraftUp { change_key: String::new(), ..draft }), mime: None },
            None => SendUp { message: m.id, draft: None, mime: Some(Box::new(m.as_ref().clone())) },
        })
        .collect()
}

/// How many messages of `account` wait in its Outbox to be sent (held ones aren't).
pub fn waiting(mb: &Mailbox, account: Id) -> usize {
    mb.folder_of_kind(account, FolderKind::Outbox).map_or(0, |outbox| mb.messages_in(outbox).filter(|m| m.held.is_empty() && !from_server(m)).count())
}

/// Take in what became of one message sent. Whether anything changed; a line for the owner when
/// it is held.
pub fn send_done(mb: &mut Mailbox, account: Id, done: &SendDone, lang: mailcraft_i18n::Lang, said: &mut Vec<String>) -> bool {
    let mut changed = false;
    // Whether the owner queued it here, as it stood before the answer: `draft_done` may take away
    // the pending change that alone says so of a draft begun elsewhere without a marker (#738).
    // Not every answer is for a message the owner queued (`sync.holdSends` holds what waits in a
    // shared mailbox's Outbox, the server's own mail too).
    let queued_here = mb.message(done.message).is_some_and(|m| m.account == account && !from_server(m));
    if let Some(draft) = &done.draft {
        changed |= super::graph_drafts::draft_done(mb, account, draft, lang, said);
    }
    // The answer accounts for it (AU3-26): whatever became of it, it is no longer in doubt.
    if mb.message(done.message).is_some_and(|m| m.account == account && m.sending)
        && let Ok(m) = mb.message_mut(done.message)
    {
        m.sending = false;
        changed = true;
    }
    // Queued here by the owner: one without MailCraft's marker (a draft begun elsewhere that an
    // earlier version queued) gets it now (#738), as `compose.send` gives it. Its change may have
    // just gone up, and without the marker it would then be taken for mail the server put in the
    // Outbox: never sent again, even after Send Again. After `draft_done`, which matches the
    // answer by the marker the draft went up with.
    if queued_here
        && let Ok(m) = mb.message_mut(done.message)
        && let Some(sd) = m.server_draft.as_mut().filter(|sd| sd.marker.is_empty())
    {
        sd.marker = super::graph_drafts::marker(done.message, u64::from(sd.saves));
        changed = true;
    }
    let Some(m) = mb.message(done.message).filter(|m| m.account == account) else { return changed };
    let server = if mb.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Exchange) { "the server" } else { "Microsoft" };
    let subject: String = m.subject.chars().filter(|c| !c.is_control()).take(60).collect();
    let subject = if subject.trim().is_empty() { lang.t("(no subject)").to_string() } else { subject };
    let couldnt = |why: &str| {
        mailcraft_i18n::fmt(lang.t("Couldn't send '{subject}': {why}. It is held in the Outbox."), &[("subject", &subject), ("why", why)])
    };
    match &done.outcome {
        SendOutcome::Waits => changed,
        // Gone: the server files it in Sent Items, and that copy comes down. What it answers or
        // forwards is marked so now (#154).
        SendOutcome::Sent => {
            // An Exchange server account's draft the run could not take away after the send: the
            // takeaway goes up with the next run as any other delete, and the send is never made
            // again (E8).
            if !done.draft_left.is_empty() {
                let queued = Queued::DeleteMessage { account, id: done.draft_left.clone(), kept: None };
                if !mb.queued.contains(&queued) {
                    mb.queued.push(queued);
                }
                changed = true;
            }
            mb.mark_answered(done.message);
            mb.forget_message(done.message).is_ok() || changed
        }
        // Exchange won't let the owner send as the shared mailbox nor on its behalf (#70): said
        // with its address where the message is read, and never sent from another.
        SendOutcome::Held { lost: false, why } if why == SEND_AS_DENIED => {
            let address = mb.account(account).map(|a| a.email.trim().to_string()).unwrap_or_default();
            said.push(couldnt(why));
            if let Ok(m) = mb.message_mut(done.message) {
                m.held = format!(
                    "It wasn't sent: you don't have permission to send as {address}. Nothing was sent from another address. Your organisation's administrator gives Send As or Send on Behalf permission; then send it again."
                );
            }
            true
        }
        SendOutcome::Held { lost, why } => {
            let held = if *lost {
                format!("It may have been sent: the answer from {server} was lost. Look in Sent Items before you send it again.")
            } else {
                format!("It wasn't sent: {why}. Change it, or send it again.")
            };
            said.push(if *lost {
                mailcraft_i18n::fmt(lang.t("'{subject}' may have been sent: look in Sent Items. It is held in the Outbox."), &[("subject", &subject)])
            } else {
                couldnt(why)
            });
            if let Ok(m) = mb.message_mut(done.message) {
                m.held = held;
            }
            true
        }
    }
}

/// What a message held for having been handed to a server without an answer coming back says.
pub const MAY_HAVE_GONE: &str = "It may have been sent: it was handed to the server, and the answer never came back (another mailbox was opened, or MailCraft stopped). Look in Sent Items before you send it again.";

/// Mark the messages `ids` of `account`'s Outbox as being sent (AU3-26), before the run that
/// sends them is handed to the host. Whether any mark is new (the mailbox is to be saved before
/// the send leaves).
pub fn mark_sending(mb: &mut Mailbox, account: Id, ids: &[Id]) -> bool {
    let Some(outbox) = mb.folder_of_kind(account, FolderKind::Outbox) else { return false };
    let mut changed = false;
    for id in ids {
        let marks = mb.message(*id).is_some_and(|m| m.account == account && m.folder == outbox && m.held.is_empty() && !m.sending);
        if marks && let Ok(m) = mb.message_mut(*id) {
            m.sending = true;
            changed = true;
        }
    }
    changed
}

/// The answer of a run of `account` was taken in: what it sent is gone, and what it didn't send
/// is known not to have gone, so no mark is left on its messages. Whether anything changed.
pub fn sending_answered(mb: &mut Mailbox, account: Id) -> bool {
    let marked: Vec<Id> = mb.messages.values().filter(|m| m.account == account && m.sending).map(|m| m.id).collect();
    for id in &marked {
        if let Ok(m) = mb.message_mut(*id) {
            m.sending = false;
        }
    }
    !marked.is_empty()
}

/// A run of `account` begins: a message still marked as being sent was handed to a server by a
/// run whose answer never came back, so it may have gone. It is held (the owner decides with Send
/// Again), never sent again by itself. A line for the owner for each one; whether anything changed.
/// Only what is still in the Outbox is held: a mark anywhere else (filed in Sent Items, moved
/// away) is a stray, and is only taken off.
pub fn hold_unanswered(mb: &mut Mailbox, account: Id, lang: mailcraft_i18n::Lang, said: &mut Vec<String>) -> bool {
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox);
    let mut marked: Vec<Id> = mb.messages.values().filter(|m| m.account == account && m.sending).map(|m| m.id).collect();
    marked.sort_unstable();
    for id in &marked {
        let Ok(m) = mb.message_mut(*id) else { continue };
        m.sending = false;
        if !m.held.is_empty() || Some(m.folder) != outbox {
            continue;
        }
        m.held = MAY_HAVE_GONE.to_string();
        let subject: String = m.subject.chars().filter(|c| !c.is_control()).take(60).collect();
        let subject = if subject.trim().is_empty() { lang.t("(no subject)").to_string() } else { subject };
        said.push(mailcraft_i18n::fmt(
            lang.t("'{subject}' may have been sent: look in Sent Items. It is held in the Outbox."),
            &[("subject", &subject)],
        ));
    }
    !marked.is_empty()
}

/// Messages `ids` of `account` that were handed to `server` (in the owner's words: "Microsoft",
/// "the server") whose answer was lost (a timeout, a connection that broke, an answer that doesn't
/// say): they may have gone, so each still in the Outbox is held for the owner, never sent again
/// by itself (AU3-26). A line for the owner for each; whether anything changed.
pub fn hold_lost(mb: &mut Mailbox, account: Id, ids: &[Id], server: &str, lang: mailcraft_i18n::Lang, said: &mut Vec<String>) -> bool {
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox);
    let mut changed = false;
    for id in ids {
        let Ok(m) = mb.message_mut(*id) else { continue };
        if m.account != account || Some(m.folder) != outbox {
            continue;
        }
        m.sending = false;
        changed = true;
        if !m.held.is_empty() {
            continue;
        }
        m.held = format!("It may have been sent: the answer from {server} was lost. Look in Sent Items before you send it again.");
        let subject: String = m.subject.chars().filter(|c| !c.is_control()).take(60).collect();
        let subject = if subject.trim().is_empty() { lang.t("(no subject)").to_string() } else { subject };
        said.push(mailcraft_i18n::fmt(
            lang.t("'{subject}' may have been sent: look in Sent Items. It is held in the Outbox."),
            &[("subject", &subject)],
        ));
    }
    changed
}

/// Why a shared mailbox's mail wasn't let go at the first send (#70), in plain words.
fn hold_reason(code: &str) -> Option<&'static str> {
    match code {
        "declined" => Some("sending from shared mailboxes wasn't allowed on Microsoft's page"),
        "approval" => Some("your organisation's administrator has to approve MailCraft sending from shared mailboxes first"),
        "organisation" => Some("your organisation doesn't let MailCraft send from shared mailboxes"),
        _ => None,
    }
}

fn hold_sends(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::id(v, "account").ok_or_else(|| bad("`account` is required"))?;
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if !a.is_shared() {
        return Err(failed!("{name} isn't a shared mailbox", name = a.name));
    }
    let why = p::str(v, "why").and_then(hold_reason).ok_or_else(|| bad("`why` is declined, approval or organisation"))?;
    let Some(outbox) = s.mailbox.folder_of_kind(account, FolderKind::Outbox) else { return Ok(json!({"held": []})) };
    let waiting: Vec<Id> = s.mailbox.messages_in(outbox).filter(|m| m.held.is_empty()).map(|m| m.id).take(MAX_SENDS).collect();
    let mut said = Vec::new();
    for id in &waiting {
        let done =
            SendDone { message: *id, outcome: SendOutcome::Held { lost: false, why: why.to_string() }, draft: None, draft_left: String::new() };
        send_done(&mut s.mailbox, account, &done, s.interface_language, &mut said);
    }
    if let Some(first) = said.first() {
        s.sync_status = first.clone();
        s.mailbox_changed();
    }
    Ok(json!({"held": waiting}))
}

fn send_again(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::message_strict(v)?.or(s.sel.focus()).ok_or_else(|| bad("select a message first"))?;
    let m = s.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    if m.held.is_empty() {
        return Err(fail("that message isn't held"));
    }
    let m = s.mailbox.message_mut(id)?;
    m.held.clear();
    m.sending = false;
    s.mailbox_changed();
    Ok(json!({"id": id, "held": false}))
}

#[cfg(test)]
#[path = "graph_send_tests.rs"]
mod tests;
