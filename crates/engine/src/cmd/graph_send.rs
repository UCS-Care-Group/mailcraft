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
use mailcraft_model::{FolderKind, Id, Mailbox};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mail.sendAgain", "Send Again", "Mail", send_again)
            .params(r#"{"id"?: number} (default: the selected message) — a held message in the Outbox goes with the next Send/Receive"#)
            .selection(super::needs_message),
        CommandSpec::new("sync.holdSends", "Hold Unsent Mail", "Send / Receive", hold_sends)
            .params(r#"{"account": number (a shared mailbox), "why": "declined"|"approval"|"organisation"} — what waits in a shared mailbox's Outbox is held, with that reason: sending from shared mailboxes was not allowed on Microsoft's page ("declined"), waits on an administrator's approval ("approval"), or the organisation doesn't allow it ("organisation"). Nothing is sent from another address. Send Again puts a held message back. Answers {"held": [ids]}"#),
    ]
}

/// What `account` has waiting to be sent: every message of its Outbox that isn't held, whole.
/// What a message keeps on disk is read for it (R6.1); one whose content can't be read waits in
/// the Outbox for the next run, with a line in the log, rather than going without its files.
pub fn sends(s: &Session, account: Id) -> Vec<SendUp> {
    let mb = &s.mailbox;
    let Some(outbox) = mb.folder_of_kind(account, FolderKind::Outbox) else { return Vec::new() };
    let mut waiting: Vec<_> = mb.messages_in(outbox).filter(|m| m.held.is_empty()).map(|m| m.id).collect();
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
        .map(|m| match m.server_draft.as_ref().and(super::graph_drafts::draft_up(&m)) {
            // Sent as it is here: whatever was changed on the server meanwhile is the owner's to lose.
            Some(draft) => SendUp { message: m.id, draft: Some(mailcraft_model::sync::DraftUp { change_key: String::new(), ..draft }), mime: None },
            None => SendUp { message: m.id, draft: None, mime: Some(Box::new(m.as_ref().clone())) },
        })
        .collect()
}

/// How many messages of `account` wait in its Outbox to be sent (held ones aren't).
pub fn waiting(mb: &Mailbox, account: Id) -> usize {
    mb.folder_of_kind(account, FolderKind::Outbox).map_or(0, |outbox| mb.messages_in(outbox).filter(|m| m.held.is_empty()).count())
}

/// Take in what became of one message sent. Whether anything changed; a line for the owner when
/// it is held.
pub fn send_done(mb: &mut Mailbox, account: Id, done: &SendDone, lang: mailcraft_i18n::Lang, said: &mut Vec<String>) -> bool {
    let mut changed = false;
    if let Some(draft) = &done.draft {
        changed |= super::graph_drafts::draft_done(mb, account, draft, lang, said);
    }
    let Some(m) = mb.message(done.message).filter(|m| m.account == account) else { return changed };
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
                "It may have been sent: the answer from Microsoft was lost. Look in Sent Items before you send it again.".to_string()
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
        let done = SendDone { message: *id, outcome: SendOutcome::Held { lost: false, why: why.to_string() }, draft: None };
        send_done(&mut s.mailbox, account, &done, s.interface_language, &mut said);
    }
    if let Some(first) = said.first() {
        s.sync_status = first.clone();
        s.mailbox_changed();
    }
    Ok(json!({"held": waiting}))
}

fn send_again(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| bad("select a message first"))?;
    let m = s.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    if m.held.is_empty() {
        return Err(fail("that message isn't held"));
    }
    s.mailbox.message_mut(id)?.held.clear();
    s.mailbox_changed();
    Ok(json!({"id": id, "held": false}))
}
