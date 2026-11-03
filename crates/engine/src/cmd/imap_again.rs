// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! IMAP mail kept with characters that couldn't be read (#650), fetched again once. Mail stored
//! before character sets beyond UTF-8 and Windows-1252 were read (#178) has U+FFFD where č, š
//! or ž were, and the stored text is all there is: so, once, the messages of IMAP accounts that
//! hold one are marked ([`Message::refetch`]), a synchronisation asks the server for them again
//! by UID (`UID FETCH … BODY.PEEK[]`, which never sets `\Seen`, [`FolderState::again`]), and what
//! comes ([`FolderDelta::refetched`]) is read anew and its subject, names and text written over
//! the old ones. Microsoft accounts have their own way ([`super::graph_down::refetch_unreadable`]).
//!
//! [`FolderState::again`]: mailcraft_model::sync::FolderState::again

use std::collections::HashMap;

use mailcraft_model::sync::FolderDelta;
use mailcraft_model::{AccountKind, Address, Id, Mailbox, Message};

/// Most messages one synchronisation request asks to have downloaded again, over all folders.
pub const MAX_AGAIN: usize = 200;

/// Whether `m` holds a character that couldn't be read, in its subject, names or text.
fn unreadable(m: &Message) -> bool {
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
pub fn wanted(mb: &Mailbox, account: Id) -> HashMap<Id, Vec<u32>> {
    let mut found: HashMap<Id, Vec<u32>> = HashMap::new();
    // In folder and UID order, so that which are asked for first doesn't change from run to run.
    let mut marked: Vec<(Id, u32)> =
        mb.messages.values().filter(|m| m.refetch && m.uid != 0 && m.account == account).map(|m| (m.folder, m.uid)).collect();
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
pub fn take_again(mb: &mut Mailbox, id: Id, read: Message) {
    let Ok(m) = mb.message_mut(id) else { return };
    (m.refetch, m.refetched) = (false, true);
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
}

/// Take in what a folder's delta says of messages downloaded again. `by_uid` finds a message of
/// the folder by its UID.
pub fn take_in(mb: &mut Mailbox, delta: &mut FolderDelta, by_uid: impl Fn(u32) -> Option<Id>) {
    for read in std::mem::take(&mut delta.refetched) {
        if let Some(id) = by_uid(read.uid) {
            take_again(mb, id, read);
        }
    }
    for uid in std::mem::take(&mut delta.refetch_done) {
        if let Some(id) = by_uid(uid)
            && let Ok(m) = mb.message_mut(id)
        {
            (m.refetch, m.refetched) = (false, true);
        }
    }
}
