// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check again (#30): a Digitally signed or Encrypted message read before the bytes it came as
//! were kept ([`Message::original`]) has them downloaded again from its server at its account's
//! next Send/Receive. The engine only marks it: an IMAP message is asked for again by its UID
//! ([`Message::refetch`], as for #650), the message of a Microsoft, Exchange server or JMAP
//! account is fetched whole again ([`Message::unfetched`], as for #185), keeping meanwhile the
//! text, attachments and everything else it has here. Nothing changes of it but the bytes it came
//! as, which it then keeps.

use mailcraft_model::{AccountKind, Mailbox, Message};
use serde_json::{Value, json};

use super::{fail, needs_message};
use crate::{CmdResult, Session, p};

/// Why message `m` can't be checked again, or `None` when it can: it looks Digitally signed or
/// Encrypted, hasn't the bytes it came as, and a server still has it whole.
pub(crate) fn why_not(mb: &Mailbox, m: &Message) -> Option<&'static str> {
    if m.has_original() {
        return Some("the message has the bytes it came as already");
    }
    if m.draft {
        return Some("the message isn't Digitally signed or Encrypted");
    }
    // (Where it is first, and what it holds last: this is asked every frame a menu shows it.)
    let kind = mb.account(m.account).map(|a| a.kind);
    let on_server = match kind {
        Some(AccountKind::Imap) => m.uid != 0 && m.not_downloaded == 0 && !m.unfetched,
        Some(AccountKind::Microsoft | AccountKind::Jmap | AccountKind::Exchange) => {
            m.server.as_ref().is_some_and(|s| !s.gone && !s.id.is_empty()) && m.server_draft.is_none()
        }
        _ => false,
    };
    if !on_server || m.incomplete || m.moving_out.is_some() {
        return Some("no server has this message to download it from");
    }
    if !m.looks_protected() {
        return Some("the message isn't Digitally signed or Encrypted");
    }
    None
}

/// Enabled for a message selected that can be checked again ([`why_not`]).
pub(crate) fn needs_checkable(s: &Session) -> Option<&'static str> {
    needs_message(s).or_else(|| match s.sel.focus().and_then(|id| s.mailbox.message(id)) {
        Some(m) => why_not(&s.mailbox, m),
        None => Some("select a message first"),
    })
}

/// `mail.checkAgain`: mark the message to have the bytes it came as downloaded again.
pub(crate) fn check_again(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::message_strict(v)?.or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    // Its text in memory, for armour at the start of a line to be seen.
    let _ = s.load_where(id, |_| false);
    let m = s.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    if let Some(why) = why_not(&s.mailbox, m) {
        return Err(fail(why));
    }
    let imap = s.mailbox.account(m.account).is_some_and(|a| a.kind == AccountKind::Imap);
    let m = s.mailbox.message_mut(id)?;
    if imap {
        m.refetch = true;
    } else if !m.unfetched {
        // Its text and files stay until the new copy comes and is taken in.
        m.attachments_on_server = m.has_attachments();
        m.unfetched = true;
    }
    Ok(json!({"id": id, "downloads": true}))
}
