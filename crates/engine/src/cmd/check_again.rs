// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check again (#30): a Digitally signed or Encrypted message read before the bytes it came as
//! were kept ([`Message::original`]) has them downloaded again from its server at its account's
//! next Send/Receive. The engine only marks it: an IMAP message is asked for again by its UID
//! ([`Message::refetch`], as for #650), the message of a Microsoft, Exchange server or JMAP
//! account is fetched whole again ([`Message::unfetched`], as for #185), keeping meanwhile the
//! text, attachments and everything else it has here. What comes is taken for the bytes it came
//! as alone ([`Message::original_wanted`]): nothing else of it changes. A message a part was
//! removed from here ([`Message::parts_removed`]) is never checked again.

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
    if m.parts_removed {
        return Some("a part was removed from the message here, so the bytes it came as aren't this message any more");
    }
    if m.original_wanted {
        return Some("the bytes it came as are asked for already");
    }
    // (Where it is first, and what it holds last: this is asked every frame a menu shows it.)
    if !on_server(mb.account(m.account).map(|a| a.kind), m) {
        return Some("no server has this message to download it from");
    }
    if !m.looks_protected() {
        return Some("the message isn't Digitally signed or Encrypted");
    }
    None
}

/// Whether a server still has message `m`, of an account of `kind`, whole, to download again.
fn on_server(kind: Option<AccountKind>, m: &Message) -> bool {
    let there = match kind {
        Some(AccountKind::Imap) => m.uid != 0 && m.not_downloaded == 0,
        Some(AccountKind::Microsoft | AccountKind::Jmap | AccountKind::Exchange) => {
            m.server.as_ref().is_some_and(|s| !s.gone && !s.id.is_empty()) && m.server_draft.is_none()
        }
        _ => false,
    };
    // Only a message whose body is here: one that is only its headers and preview is fetched
    // whole, original and all, when it is opened, and what Check again brings is taken for the
    // original alone, which would leave the preview as its body.
    there && !m.unfetched && !m.incomplete && m.moving_out.is_none()
}

/// Mark `m`, of an account of `kind`, to have the bytes it came as downloaded at the next
/// Send/Receive, alone ([`Message::original_wanted`]): by its UID (IMAP) or fetched whole again,
/// keeping meanwhile all it has here. Nothing when no server has it whole, or a part was removed.
pub(crate) fn ask(kind: Option<AccountKind>, m: &mut Message) {
    if m.draft || m.parts_removed || m.original_wanted || !on_server(kind, m) {
        return;
    }
    m.original_wanted = true;
    if kind == Some(AccountKind::Imap) {
        m.refetch = true;
    } else {
        // Its text and files stay until the new copy comes and is taken in.
        m.attachments_on_server = m.has_attachments();
        m.unfetched = true;
    }
}

/// The original of `m`, of an account of `kind`, is surely gone from the content folder (a
/// build before #30 swept it, say): a message a server still has whole asks the server for it
/// again ([`ask`]). Either way it stays named, until a download brings the bytes it came as to
/// take its place: a look that wrongly found it gone (a drive put back later) loses nothing, and
/// a server whose copy isn't protected (one put there from its parts) never takes it away. It is
/// written from its parts meanwhile. Whether the server was asked.
// ponytail: asked again at each opening while the file stays gone and the server's copy brings
// none; a mark of "asked, none came" if that ever costs.
pub(crate) fn lost(kind: Option<AccountKind>, m: &mut Message) -> bool {
    if m.draft || m.parts_removed || m.original_wanted || !on_server(kind, m) {
        log::warn!("message {}: the bytes it came as are missing from the content folder; it is written from its parts", m.id);
        return false;
    }
    ask(kind, m);
    true
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
    let kind = s.mailbox.account(m.account).map(|a| a.kind);
    ask(kind, s.mailbox.message_mut(id)?);
    Ok(json!({"id": id, "downloads": true}))
}
