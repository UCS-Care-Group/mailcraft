// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! IMAP mail kept with characters that couldn't be read (#650), fetched again once. Mail stored
//! before character sets beyond UTF-8 and Windows-1252 were read (#178) has U+FFFD where č, š
//! or ž were, and the stored text is all there is: so, once, the messages of IMAP accounts that
//! hold one are marked ([`Message::refetch`]), a synchronisation asks the server for them again
//! by UID (`UID FETCH … BODY.PEEK[]`, which never sets `\Seen`, [`FolderState::again`]), and what
//! comes ([`FolderDelta::refetched`]) is read anew and its subject, names and text written over
//! the old ones. Microsoft accounts have their own way ([`super::graph_down::refetch_unreadable`]).
//! The same mark asks for a Digitally signed or Encrypted message read before the bytes it came
//! as were kept (`mail.checkAgain`, #30), which then keeps them ([`Message::original`]).
//!
//! [`FolderState::again`]: mailcraft_model::sync::FolderState::again

use std::collections::HashMap;

use mailcraft_model::sync::FolderDelta;
use mailcraft_model::{AccountKind, Address, Id, Mailbox, Message, ORIGINAL_PATIENCE, ORIGINAL_TRIES};
use mailcraft_time::Timestamp;

/// Most messages one synchronisation request asks to have downloaded again, over all folders.
pub const MAX_AGAIN: usize = 200;

/// Whether `m` holds a character that couldn't be read, in its subject, names or text.
pub(crate) fn unreadable(m: &Message) -> bool {
    const MARK: char = char::REPLACEMENT_CHARACTER;
    let names = std::iter::once(&m.from).chain(&m.to).chain(&m.cc).chain(&m.reply_to).map(|a| a.name.as_str());
    let texts = [m.subject.as_str(), m.body.text.as_str(), m.body.html.as_deref().unwrap_or("")];
    names.chain(texts).any(|text| text.contains(MARK))
}

/// The messages of IMAP accounts that the server has (they have a UID) and whose subject, names
/// or text hold U+FFFD are marked to be downloaded again, each at most once
/// ([`Message::refetched`]). Drafts are the owner's own and left alone, as is mail that was never
/// downloaded whole. Only what is in memory is looked at, so a host calls this when the mailbox
/// opens and again once the bodies an opening left for later have come in (a body not in memory
/// yet reads as empty, and is looked at then). A body kept on disk beyond the memory budget is
/// not looked at (it is not read for this). How many were marked.
pub fn mark_unreadable(mb: &mut Mailbox) -> usize {
    if !mb.accounts.iter().any(|a| a.kind == AccountKind::Imap) {
        return 0;
    }
    let imap: Vec<Id> = mb.accounts.iter().filter(|a| a.kind == AccountKind::Imap).map(|a| a.id).collect();
    let mut marked = 0usize;
    for m in mb.messages.values_mut() {
        let candidate = m.uid != 0
            && imap.contains(&m.account)
            && !m.refetch
            && !m.refetched
            && !m.draft
            && !m.unfetched
            && !m.incomplete
            && m.not_downloaded == 0
            && m.moving_out.is_none();
        if candidate && unreadable(m) {
            std::sync::Arc::make_mut(m).refetch = true;
            marked = marked.saturating_add(1);
        }
    }
    marked
}

/// The UIDs to have downloaded again, per folder, at most [`MAX_AGAIN`] in all (the rest are
/// asked for by the runs after).
pub fn wanted(mb: &Mailbox, account: Id, now: Timestamp) -> HashMap<Id, Vec<u32>> {
    let mut found: HashMap<Id, Vec<u32>> = HashMap::new();
    // (Not one whose server refused the bytes it came as a while ago, #30: asked again later. A
    // time further off than the longest wait was set by a clock that was ahead: due now.)
    let due = |m: &Message| m.original_retry <= now || m.original_retry > now.saturating_add(LONGEST_WAIT);
    // In folder and UID order, so that which are asked for first doesn't change from run to run.
    let mut marked: Vec<(Id, u32)> =
        mb.messages.values().filter(|m| m.refetch && m.uid != 0 && m.account == account && due(m)).map(|m| (m.folder, m.uid)).collect();
    // And the bytes a message being moved into another account came as (#30), from the copy it
    // came from, which waits here to go ([`crate::cmd::mail::check_again::ask_source`]).
    marked.extend(moved_out(mb, account).into_iter().filter(|(_, id)| mb.message(*id).is_some_and(due)).map(|(at, _)| at));
    marked.sort_unstable();
    marked.truncate(MAX_AGAIN);
    for (folder, uid) in marked {
        found.entry(folder).or_default().push(uid);
    }
    found
}

/// Take in a message downloaded again: its subject, names and text written over the old ones,
/// the mark gone. Where it is, its flags, categories and everything else stay. A draft is the
/// owner's own and keeps its text; a name list of another length or of other addresses is not
/// the same list and stays.
pub fn take_again(mb: &mut Mailbox, id: Id, mut read: Message) {
    // What came brought no original (#30): its server's copy isn't protected either.
    let brought = read.original.is_some();
    // Within what the mailbox keeps of originals in all (#30), counting the one it replaces.
    let replaced = mb.message(id).and_then(|m| m.original.as_ref()).map_or(0, mailcraft_model::Original::size);
    // (No room for it: how much room it needs, which there must be before it is asked again.)
    let mut no_room = 0;
    let needs = read.original.as_ref().map_or(0, |o| o.size().saturating_sub(replaced));
    if read.original.is_some() && !mb.room_for_original(needs) {
        read.original = None;
        no_room = needs.max(1);
    }
    let Ok(m) = mb.message_mut(id) else { return };
    (m.refetch, m.refetched) = (false, true);
    let wanted = std::mem::take(&mut m.original_wanted);
    m.original_asked |= wanted && !brought;
    if wanted && no_room > 0 {
        m.original_no_room = no_room;
    }
    if wanted {
        // Answered: what was refused before counts no more.
        (m.original_tries, m.original_refused, m.original_retry) = (0, 0, 0);
    }
    // Check again asked for the bytes it came as alone (#30): nothing else of it changes, but
    // for text read wrongly before (U+FFFD), which is taken from the same download too, as it
    // would have been had it been marked for that. (One being moved into another account, asked
    // for from the copy it came from, alike.)
    if wanted && (m.draft || !unreadable(m)) {
        if !m.parts_removed && read.original.is_some() {
            m.original = read.original;
        }
        return;
    }
    if m.draft {
        return;
    }
    if !read.subject.is_empty() {
        m.subject = read.subject;
    }
    if !read.from.email.is_empty() && read.from.email.eq_ignore_ascii_case(&m.from.email) {
        m.from.name = read.from.name;
    }
    let names = |old: &mut Vec<Address>, new: Vec<Address>| {
        if old.len() == new.len() && old.iter().zip(&new).all(|(a, b)| a.email.eq_ignore_ascii_case(&b.email)) {
            for (a, b) in old.iter_mut().zip(new) {
                a.name = b.name;
            }
        }
    };
    names(&mut m.to, read.to);
    names(&mut m.cc, read.cc);
    names(&mut m.reply_to, read.reply_to);
    m.body = read.body;
    m.hidden_text = read.hidden_text;
    // What it is now is all of it: nothing the mailbox kept on disk of its preview is its body.
    m.kept = None;
    // Digitally signed or Encrypted: the bytes it came as (#30), unless a part was removed here:
    // its files aren't taken from this download, so its original would bring a removed one back.
    if read.original.is_some() && !m.parts_removed {
        m.original = read.original;
    }
}

/// The messages moved out of `account` whose original is asked for from the copy they came from
/// ([`crate::cmd::mail::check_again::ask_source`]), by that copy's folder here and UID.
pub(crate) fn moved_out(mb: &Mailbox, account: Id) -> HashMap<(Id, u32), Id> {
    mb.pending
        .iter()
        .filter(|p| p.account() == account)
        .filter_map(|p| match p {
            mailcraft_model::Pending::Move { message, .. } => {
                let (from, uid) = p.source();
                let m = mb.message(*message).filter(|m| m.refetch && m.original_wanted && m.account != account && uid != 0)?;
                let folder = mb.folders.iter().find(|f| f.account == account && f.remote == from)?;
                Some(((folder.id, uid), m.id))
            }
            _ => None,
        })
        .collect()
}

/// The server didn't give the bytes `m` came as when asked (#30), at `now`: too large to take
/// here (`too_large`), they are given up on; refused, they are asked for again after a wait that
/// grows (a quarter of an hour, doubled each time up to four hours), and given up on once refused
/// [`ORIGINAL_TRIES`] times over at least [`ORIGINAL_PATIENCE`].
pub(crate) fn refused(m: &mut Message, too_large: bool, now: Timestamp) {
    // (A first refusal later than now was seen by a clock that was ahead: counted from now.)
    if m.original_refused == 0 || m.original_refused > now {
        m.original_refused = now.max(1);
    }
    let tries = m.original_tries.saturating_add(1);
    if too_large || (tries >= ORIGINAL_TRIES && now.saturating_sub(m.original_refused) >= ORIGINAL_PATIENCE) {
        m.original_tries = ORIGINAL_TRIES;
        (m.refetch, m.original_wanted, m.original_retry) = (false, false, 0);
        return;
    }
    m.original_tries = tries.min(ORIGINAL_TRIES - 1);
    let wait: Timestamp = 15 * 60 * (1 << tries.saturating_sub(1).min(4));
    m.original_retry = now.saturating_add(wait.min(LONGEST_WAIT));
}

/// The longest wait before the bytes a message came as are asked for again after a refusal (#30).
const LONGEST_WAIT: Timestamp = 4 * 60 * 60;

/// Take in what a folder's delta says of messages downloaded again. `by_uid` finds a message of
/// the folder by its UID.
pub fn take_in(mb: &mut Mailbox, delta: &mut FolderDelta, now: Timestamp, by_uid: impl Fn(u32) -> Option<Id>) {
    for read in std::mem::take(&mut delta.refetched) {
        if let Some(id) = by_uid(read.uid) {
            take_again(mb, id, read);
        }
    }
    let too_large = std::mem::take(&mut delta.refetch_too_large);
    delta.refetch_refused.clear();
    for uid in std::mem::take(&mut delta.refetch_done) {
        if let Some(id) = by_uid(uid)
            && let Ok(m) = mb.message_mut(id)
        {
            // The bytes it came as asked for (#30), and nothing came: never taken for a copy
            // without them, which would let a copy that may hold them go. Refused, they are
            // asked for again, ever less often, and given up on only once refused so many times
            // over a day; too large to take here, at once. What waits for them goes on waiting
            // either way, until Check again asks once more.
            if m.original_wanted {
                refused(m, too_large.contains(&uid), now);
                continue;
            }
            (m.refetch, m.refetched) = (false, true);
        }
    }
}
