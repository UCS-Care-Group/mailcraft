// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail moved or copied into a Microsoft account from elsewhere: between two mailboxes of one
//! owner's (#70: a shared mailbox and its owner's, or two shared mailboxes of his), and from an
//! IMAP account or a folder on this computer (#451).
//!
//! Microsoft moves and copies a message only within its own mailbox. What goes to another is
//! made there by that mailbox's own run, at its own address, from the whole message here
//! ([`PlaceUp`]): the message waits here, in its new folder, until it is
//! ([`Queued::PlaceMessage`]). Of a move, the original stays on its own server, **being moved**
//! ([`mailcraft_model::sync::MovingOut`] in a Microsoft account, a [`Pending::Move`] out of an
//! IMAP account), and goes from there only once the copy is confirmed on the destination's
//! server ([`safe_here`]: `graph_up`'s moved-out changes, the IMAP run's deletions): a copy that
//! fails never loses the message.
//!
//! - **Made**: the message here takes the copy's server id, and is that server message from then on.
//! - **Refused** (403, 400, or a folder that is gone): a move goes back to where its original
//!   still is, as that server message, or to the folder on this computer it came from; a copy is
//!   taken away here again (its source is where it was). Said once per run and kind, as other
//!   refusals are (R3 spec S20). A move whose original was gone already is kept in a folder on
//!   this computer: it is all there is of it.
//! - **No answer**, or a shared mailbox's 403 that isn't "no access": it waits for the next run,
//!   where the copy is looked for by its marker before it is made again.
//! - Moved on, deleted or undone meanwhile: a copy made all the same is deleted there again.

use std::collections::HashSet;

use mailcraft_model::sync::{FlagUp, MAX_PLACES, MessageOutcome, PLACE_BYTES, PlaceDone, PlaceUp, ServerMessage};
use mailcraft_model::{FlagStatus, FolderKind, Id, Mailbox, Message, Pending, Queued, RETURNED_LISTINGS};
use mailcraft_time::{DateTime, Timestamp};

use super::graph_up::{Kind, Refusals, Why, destination, folder_name};
use crate::Session;

/// The messages waiting to be made in `account`'s folders: at most `limit` spelled out (each
/// whole, its body and files read in when they are kept on disk), and how many there are.
pub fn place_changes(s: &Session, account: Id, limit: usize) -> (Vec<PlaceUp>, usize) {
    let mb = &s.mailbox;
    let today = s.zone().instant(DateTime::at_midnight(s.today()));
    let now = s.now();
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox);
    let (mut out, mut count, mut spent) = (Vec::new(), 0usize, 0u64);
    for queued in &mb.queued {
        let Queued::PlaceMessage { account: a, message, marker, copy, .. } = queued else { continue };
        if *a != account {
            continue;
        }
        // (Not one whose text couldn't be read here: the original is all there is of it.)
        let readable = |m: &Message| !m.kept.as_deref().is_some_and(|k| k.failed);
        let Some(m) = mb.message(*message).filter(|m| m.account == account && m.server.is_none() && Some(m.folder) != outbox && readable(m)) else {
            continue;
        };
        let Some((to, to_folder)) = destination(mb, account, m.folder) else { continue };
        count += 1;
        if out.len() >= limit || (!out.is_empty() && spent.saturating_add(m.size()) > PLACE_BYTES) {
            continue;
        }
        spent = spent.saturating_add(m.size());
        match s.loaded_message(m.id) {
            Ok(m) => out.push(PlaceUp::of(&m, to, to_folder, marker.clone(), !copy, flag_of(&m, today, now))),
            Err(e) => log::warn!("message {} waits to go to another mailbox: {e}", m.id),
        }
    }
    (out, count)
}

/// What waits to be put on `account`'s server but can't be read here, as far as a run looks
/// ([`MAX_PLACES`], [`PLACE_BYTES`]): its content went missing after it was moved there, so no
/// run takes it (#451). Each is said in the status bar after the run, rather than in the log
/// alone: its original stays where it was, and moving it back puts it there again.
pub(crate) fn unreadable_waiting(s: &Session, account: Id) -> Vec<String> {
    let mb = &s.mailbox;
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox);
    let (mut said, mut taken, mut spent) = (Vec::new(), 0usize, 0u64);
    for queued in &mb.queued {
        let Queued::PlaceMessage { account: a, message, .. } = queued else { continue };
        let Some(m) = mb.message(*message).filter(|m| *a == account && m.account == account && m.uid == 0 && m.server.is_none()) else {
            continue;
        };
        if Some(m.folder) == outbox || m.unfetched || m.incomplete || m.not_downloaded > 0 {
            continue;
        }
        if taken >= MAX_PLACES || (taken > 0 && spent.saturating_add(m.size()) > PLACE_BYTES) {
            break;
        }
        spent = spent.saturating_add(m.size());
        if m.kept.as_deref().is_some_and(|k| k.failed) || !s.readable_whole(m) {
            let subject: String = m.subject.chars().filter(|c| !c.is_control()).take(80).collect();
            said.push(format!("Couldn't put \"{subject}\" on the server: it couldn't be read on this computer"));
        } else {
            taken += 1;
        }
    }
    said
}

/// Whether `m`, moved here out of an account whose server still has the original, is safe where
/// it is now, so that the original may go (R3 spec S19, #70, #451): made on the server of the
/// Microsoft account it is in; on the server of the IMAP account it is in, under its UID there;
/// in an account whose server is given no copy, whole here (every part of it read back, its
/// attachments too) and kept in the mailbox file (`saved`: the file holds what is here now).
/// Only when `read` are its parts read back, which is done for a change that goes into a request:
/// a count of what waits to go, which a host asks for every few seconds, reads no file, and
/// counts one that can't be read as it does a change the server refused.
pub(crate) fn safe_here(s: &Session, m: &Message, saved: bool, read: bool) -> bool {
    if s.mailbox.is_graph(m.account) {
        m.server.as_ref().is_some_and(|server| !server.gone && !server.id.is_empty())
    } else if s.mailbox.puts_on_server(m.account) {
        m.uid != 0
    } else {
        let whole = !m.unfetched && !m.incomplete && m.not_downloaded == 0 && !m.kept.as_deref().is_some_and(|k| k.failed);
        whole && (saved || s.saved_whole.contains(&m.id)) && (!read || s.readable_whole(m))
    }
}

/// The flag of `m` as it goes with the copy.
fn flag_of(m: &Message, today: Timestamp, now: Timestamp) -> Option<FlagUp> {
    match m.flag {
        FlagStatus::None => None,
        FlagStatus::Flagged => {
            let start = m.flag_due.map_or(today, |due| today.min(due));
            Some(FlagUp { status: FlagStatus::Flagged, start: Some(start), due: m.flag_due, completed: None })
        }
        FlagStatus::Complete => Some(FlagUp { status: FlagStatus::Complete, completed: Some(now), ..Default::default() }),
    }
}

/// Take in what became of one message made in `account` from another mailbox. Whether anything
/// changed here.
pub fn place_done(mb: &mut Mailbox, account: Id, done: &PlaceDone, refusals: &mut Refusals) -> bool {
    let change = &done.change;
    let id = change.message;
    let entry = mb.queued.iter().position(
        |q| matches!(q, Queued::PlaceMessage { account: a, message, marker, .. } if *a == account && *message == id && *marker == change.marker),
    );
    let made = match &done.outcome {
        MessageOutcome::Done { id: made, .. } => Some(made.clone()).filter(|made| !made.is_empty()),
        _ => None,
    };
    let Some(entry) = entry else {
        // Called off meanwhile (moved on, deleted, undone): a copy made all the same goes again.
        if let Some(made) = made {
            mb.queued.push(Queued::DeleteMessage { account, id: made, kept: None });
            return true;
        }
        return false;
    };
    if matches!(done.outcome, MessageOutcome::Done { .. }) && made.is_none() {
        // Made, but which one isn't said: found by its marker next time.
        return false;
    }
    let (copy, from) = match mb.queued.get(entry) {
        Some(Queued::PlaceMessage { copy, from, .. }) => (*copy, *from),
        _ => (false, None),
    };
    mb.queued.remove(entry);
    let to_name = folder_name(mb, account, &change.to, change.to_folder);
    match (&done.outcome, made) {
        (MessageOutcome::Done { folder, .. }, Some(made)) => {
            let folder = if folder.is_empty() { change.to.clone() } else { folder.clone() };
            // Come down meanwhile (its make's answer was lost, and the delta brought it): this is it.
            let twins: Vec<Id> = mb
                .messages
                .values()
                .filter(|o| o.account == account && o.id != id && o.server.as_ref().is_some_and(|s| s.id == made))
                .map(|o| o.id)
                .collect();
            for twin in twins {
                let _ = mb.forget_message(twin);
            }
            let Ok(m) = mb.message_mut(id) else { return true };
            let flag = change.flag.clone().unwrap_or_default();
            m.server = Some(ServerMessage {
                id: made,
                folder,
                read: change.read,
                flag: flag.status,
                flag_start: flag.start,
                flag_due: flag.due,
                flag_completed: flag.completed,
                categories: change.categories.clone(),
                importance: change.importance,
                ..Default::default()
            });
            true
        }
        (outcome, _) => {
            // (A copy isn't "gone": what the server no longer has is the folder it was to go to.)
            let why = if matches!(outcome, MessageOutcome::Refused { status: 403 }) { Why::Denied } else { Why::Invalid };
            let Some(m) = mb.message(id) else { return true };
            let subject = m.subject.clone();
            refusals.note(if copy { Kind::Copy } else { Kind::Move }, why, &subject, to_name);
            put_back(mb, id, copy, from);
            true
        }
    }
}

/// Mail moved in that a server wasn't given yet, which is about to go with its account or its
/// folder (`going`, of the account it waits for and the message): it has no copy there, so it
/// goes back to where it came from (#451).
pub(crate) fn put_back_waiting(mb: &mut Mailbox, going: impl Fn(&Mailbox, Id, Id) -> bool) {
    let waiting: Vec<(Id, bool, Option<Id>)> = mb
        .queued
        .iter()
        .filter_map(|q| match q {
            Queued::PlaceMessage { account, message, copy, from, .. } if going(mb, *account, *message) => Some((*message, *copy, *from)),
            _ => None,
        })
        .collect();
    for (message, copy, from) in waiting {
        mb.queued.retain(|q| !matches!(q, Queued::PlaceMessage { message: other, .. } if *other == message));
        put_back(mb, message, copy, from);
    }
}

/// Mail moved into accounts that are about to go (`going`) whose original another account's
/// server still has, waiting for the copy to be safe where it went (#451): once it is on the
/// account's server, the original goes as it would have; until then, or when the account is one
/// on this computer, whose copy goes with it, the message goes back to its original. Either way
/// the original isn't left on its server, shown nowhere, nor taken away with the account.
pub(crate) fn settle_moves_into(s: &mut Session, going: &HashSet<Id>) {
    let mb = &s.mailbox;
    let moving: HashSet<Id> = mb
        .pending
        .iter()
        .filter_map(|p| match p {
            Pending::Move { account, message, .. } if !going.contains(account) => Some(*message),
            _ => None,
        })
        .collect();
    let elsewhere = |m: &Message| {
        m.moving_out.as_ref().is_some_and(|out| !out.left && !out.id.is_empty() && !going.contains(&out.account)) || moving.contains(&m.id)
    };
    // Safe only on a server, which keeps the copy: one on this computer goes with its account.
    let kept = |m: &Message| mb.is_graph(m.account) || mb.puts_on_server(m.account);
    let waiting: Vec<(Id, bool)> = mb
        .messages
        .values()
        .filter(|m| going.contains(&m.account) && elsewhere(m))
        .map(|m| (m.id, kept(m) && safe_here(s, m, false, true)))
        .collect();
    for (id, safe) in waiting {
        if !safe {
            put_back(&mut s.mailbox, id, false, None);
            continue;
        }
        let original = s.mailbox.message(id).and_then(|m| m.moving_out.clone()).filter(|out| !out.left && !going.contains(&out.account));
        if let Some(out) = original {
            s.mailbox.queued.push(Queued::DeleteMessage { account: out.account, id: out.id, kept: None });
        }
        super::account::original_goes(&mut s.mailbox, id);
    }
}

/// A message the destination wouldn't take: a move back to where its original still is, as that
/// server message, or to the folder on this computer it came from (`from`); a copy taken away;
/// a move whose original is gone kept on this computer. In an Outbox again it is held, and
/// anywhere else it no longer is.
pub(crate) fn put_back(mb: &mut Mailbox, id: Id, copy: bool, from: Option<Id>) {
    goes_back(mb, id, copy, from);
    mb.hold_if_in_outbox(id);
}

fn goes_back(mb: &mut Mailbox, id: Id, copy: bool, from: Option<Id>) {
    let Some(m) = mb.message(id) else { return };
    let out = m.moving_out.clone().filter(|out| mb.is_graph(out.account) && !out.id.is_empty() && !out.left);
    match out {
        Some(out) => {
            let folder = mb.folder_by_server(out.account, &out.folder).or_else(|| mb.folder_of_kind(out.account, FolderKind::Inbox));
            let Some(folder) = folder else { return };
            let Ok(m) = mb.message_mut(id) else { return };
            let server = ServerMessage {
                id: out.id,
                folder: out.folder,
                read: m.read,
                flag: m.flag,
                flag_due: m.flag_due,
                categories: m.categories.clone(),
                importance: m.importance,
                ..Default::default()
            };
            (m.server, m.moving_out, m.returned) = (Some(server), None, RETURNED_LISTINGS);
            m.lies_in(folder, out.account, 0, None);
        }
        None if copy => {
            let _ = mb.forget_message(id);
        }
        None => {
            if back_on_imap(mb, id) {
                return;
            }
            let folder = match from.filter(|f| mb.folder(*f).is_some()) {
                Some(folder) => Ok(folder),
                None => mb.imported_folder(false),
            };
            if let Ok(folder) = folder
                && let Some(account) = mb.folder(folder).map(|f| f.account)
                && let Ok(m) = mb.message_mut(id)
            {
                m.lies_in(folder, account, 0, None);
            }
        }
    }
}

/// A move out of an IMAP account that was refused (#451): the message goes back to the folder
/// that follows the mailbox its original is still in, as that server message, and nothing is
/// deleted there. When that folder is gone here, it goes to the account's Inbox, as a move there.
/// Whether it was one.
fn back_on_imap(mb: &mut Mailbox, id: Id) -> bool {
    let Some(account) = mb.message(id).map(|m| m.account) else { return false };
    let at = mb.pending.iter().position(|p| matches!(p, Pending::Move { message, .. } if *message == id));
    let Some(Pending::Move { account: owner, from, uid, synced, .. }) = at.and_then(|at| mb.pending.get(at)).cloned() else { return false };
    if owner == account {
        return false;
    }
    let there = mb.folders.iter().find(|f| f.account == owner && !f.remote.is_empty() && f.remote == from).map(|f| f.id);
    let (folder, uid, synced) = match there.or_else(|| mb.folder_of_kind(owner, FolderKind::Inbox)) {
        Some(folder) if there == Some(folder) => {
            if let Some(at) = at {
                mb.pending.remove(at);
            }
            (folder, uid, synced)
        }
        Some(inbox) => {
            if let Some(Pending::Move { to, .. }) = at.and_then(|at| mb.pending.get_mut(at)) {
                *to = inbox;
            }
            (inbox, 0, None)
        }
        None => {
            // Nowhere in its account to go: the original stays where it is, untouched.
            if let Some(at) = at {
                mb.pending.remove(at);
            }
            return false;
        }
    };
    let Ok(m) = mb.message_mut(id) else { return true };
    m.lies_in(folder, owner, uid, synced);
    m.returned = if uid != 0 { RETURNED_LISTINGS } else { 0 };
    true
}

/// A message put back as its server's message after a refused copy ([`Message::returned`]) that
/// the server says is gone since (deleted elsewhere meanwhile): kept on this computer, in
/// Imported, as all there is of it (#451). Whether it was one.
pub(crate) fn kept_instead(mb: &mut Mailbox, id: Id) -> bool {
    if !mb.message(id).is_some_and(|m| m.returned > 0) {
        return false;
    }
    let Ok(folder) = mb.imported_folder(false) else { return false };
    let Some(account) = mb.folder(folder).map(|f| f.account) else { return false };
    let Ok(m) = mb.message_mut(id) else { return false };
    m.lies_in(folder, account, 0, None);
    (m.server, m.moving_out, m.returned) = (None, None, 0);
    true
}
