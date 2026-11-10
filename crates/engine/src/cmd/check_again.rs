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

use mailcraft_model::{AccountKind, Id, Mailbox, Message, ORIGINAL_TRIES, Pending};
use serde_json::{Value, json};

use super::{fail, needs_message};
use crate::{CmdResult, Session, p};

/// Why message `m` can't be checked again, or `None` when it can: it looks Digitally signed or
/// Encrypted, hasn't the bytes it came as, and a server still has it whole; or its move waits
/// for them from the copy it came from, which is asked no more ([`held_move`]).
pub(crate) fn why_not(mb: &Mailbox, m: &Message) -> Option<&'static str> {
    if held_move(mb, m) {
        return None;
    }
    if m.has_original() {
        return Some("the message has the bytes it came as already");
    }
    if m.draft {
        return Some("the message isn't Digitally signed or Encrypted");
    }
    if m.parts_removed {
        return Some("a part was removed from the message here, so the bytes it came as aren't this message any more");
    }
    if (m.original_wanted || m.refetch) && !stale(mb, m) {
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
    if m.draft || m.parts_removed || m.original_wanted || m.refetch || !on_server(kind, m) {
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

/// The original of `m`, of an account of `kind`, couldn't be read (gone from the content folder,
/// a build before #30 swept it, say): a message a server still has whole asks the server for it
/// again ([`ask`]). Either way it stays named, until a download brings the bytes it came as to
/// take its place: a look that wrongly found it gone (a drive put back later) loses nothing, and
/// a server whose copy isn't protected (one put there from its parts) never takes it away. It is
/// written from its parts meanwhile. Asked once: when what came brought none
/// ([`Message::original_asked`]), not again. Whether the server was asked.
pub(crate) fn lost(kind: Option<AccountKind>, m: &mut Message) -> bool {
    if m.draft || m.parts_removed || m.original_wanted || m.original_asked || m.refetch || given_up(m) || !on_server(kind, m) {
        log::warn!("message {}: the bytes it came as can't be read from the content folder; it is written from its parts", m.id);
        return false;
    }
    ask(kind, m);
    true
}

/// Whether the bytes `m` came as are asked for no more (#30): refused too often, too large to
/// take here ([`Message::original_tries`]), or no room for them here until there is
/// ([`Message::original_no_room`], which a caller with the mailbox looks at).
pub(crate) fn given_up(m: &Message) -> bool {
    m.original_tries >= ORIGINAL_TRIES || m.original_no_room > 0
}

/// Whether `m`'s move waits for the bytes it came as from the copy it came from, which is asked
/// for them no more (#30, [`given_up`]) or not before a while (refused, [`Message::original_retry`]):
/// Check again asks it once more, now.
pub(crate) fn held_move(mb: &Mailbox, m: &Message) -> bool {
    (given_up(m) || m.original_retry != 0) && !m.draft && !m.parts_removed && source_copy(mb, m.id).is_some()
}

/// Where the copy message `id` is being moved out of is, on a server that still has it (#451):
/// its account, and whether that is an IMAP account's. A move out of an IMAP mailbox numbered
/// anew counts while its UID there isn't known again: the copy is there still, and is asked for
/// the bytes once it is.
pub(crate) fn source_copy(mb: &Mailbox, id: Id) -> Option<(Id, bool)> {
    let m = mb.message(id)?;
    // (One a listing afresh hasn't come to yet may be there still.)
    if let Some(out) = m.moving_out.as_ref().filter(|out| (!out.left || out.listing) && !out.id.is_empty()) {
        return Some((out.account, false));
    }
    let pending = mb.pending.iter().find(|p| matches!(p, Pending::Move { message, .. } if *message == id))?;
    (pending.account() != m.account).then(|| (pending.account(), true))
}

/// The asks for the bytes a message came as (#30) that nothing will answer any more: made of
/// the copy it was being moved out of, which is gone (deleted there, its account removed, the
/// move called off), with no copy of its own on a server. Let go of, so that Check again may be
/// asked, and so that it waits for nothing.
pub(crate) fn settle(mb: &mut Mailbox) {
    let stale: Vec<Id> = mb.messages.values().filter(|m| stale(mb, m)).map(|m| m.id).collect();
    for id in stale {
        if let Ok(m) = mb.message_mut(id) {
            (m.original_wanted, m.refetch) = (false, false);
        }
    }
}

/// Whether `m` asks for the bytes it came as where nothing will answer any more ([`settle`]).
fn stale(mb: &Mailbox, m: &Message) -> bool {
    m.original_wanted && !asked_of_own(mb.account(m.account).map(|a| a.kind), m) && source_copy(mb, m.id).is_none()
}

/// Whether the ask for the bytes `m` came as goes to its own account's server (of `kind`), which
/// has it: by its UID there (IMAP), or fetched whole from there.
fn asked_of_own(kind: Option<AccountKind>, m: &Message) -> bool {
    match kind {
        Some(AccountKind::Imap) => m.refetch && m.uid != 0,
        Some(AccountKind::Microsoft | AccountKind::Jmap | AccountKind::Exchange) => {
            m.unfetched && m.server.as_ref().is_some_and(|s| !s.gone && !s.id.is_empty())
        }
        _ => false,
    }
}

/// Whether the copy message `id` is being moved out of may hold the bytes it came as (#30)
/// when nothing here does: a server has it ([`source_copy`]), and wasn't asked for them yet
/// ([`Message::original_asked`]). That copy stays until it has been.
pub(crate) fn source_may_hold(mb: &Mailbox, id: Id) -> bool {
    mb.message(id).is_some_and(|m| !m.original_asked && !m.draft && !m.parts_removed) && source_copy(mb, id).is_some()
}

/// Mark message `m`, whose original can't be read here, to have it downloaded from the copy it
/// is being moved out of (`source`, [`source_copy`]) at that account's next Send/Receive, alone
/// ([`Message::original_wanted`]): by its UID there (IMAP), or fetched whole from there
/// (Microsoft's server), keeping meanwhile all it has here. Where it goes waits for it. Whether
/// it was asked for.
pub(crate) fn ask_source(source: Option<(Id, bool)>, m: &mut Message) -> bool {
    let Some((_, imap)) = source else { return false };
    if m.draft || m.parts_removed || m.original_asked || m.original_wanted || m.refetch || given_up(m) {
        return false;
    }
    // (From Microsoft's server, fetched for them alone: `graph_down::wants_from_source`. The
    // message stays as it is here meanwhile.)
    m.original_wanted = true;
    m.refetch = imap;
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
    let held = held_move(&s.mailbox, m);
    // An ask nothing will answer any more doesn't stand in the way (`why_not` passed it over).
    settle(&mut s.mailbox);
    // Asked by the owner: whatever the server said before, it is asked again, and what comes of
    // it is said again.
    let m = s.mailbox.message_mut(id)?;
    if given_up(m) || m.original_retry != 0 {
        (m.original_tries, m.original_refused, m.original_retry, m.original_no_room) = (0, 0, 0, 0);
    }
    s.forget_told_held(id);
    if held {
        // From the copy its move waits on.
        let source = source_copy(&s.mailbox, id);
        ask_source(source, s.mailbox.message_mut(id)?);
        return Ok(json!({"id": id, "downloads": true}));
    }
    let m = s.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    let kind = s.mailbox.account(m.account).map(|a| a.kind);
    // Text read wrongly before (U+FFFD), here or on disk (read in above): the whole of what comes
    // is taken, text and all, as for #650, not the original alone.
    let garbled = kind == Some(AccountKind::Imap) && crate::cmd::imap_again::unreadable(m);
    let m = s.mailbox.message_mut(id)?;
    ask(kind, m);
    if garbled {
        m.original_wanted = false;
    }
    Ok(json!({"id": id, "downloads": true}))
}
