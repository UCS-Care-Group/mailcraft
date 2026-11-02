// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conversations of a mailbox by their topic ([`Message::thread_key`]), so that the reading
//! pane's conversation ([`Mailbox::conversation`]) is looked up rather than found by a walk over
//! every message of the account, which took 70 ms at each click in a mailbox of 326,000.
//!
//! Brought in step with the mailbox by [`Threads::update`]: one walk in order of id that touches
//! only the messages changed, come or gone since the last time (a message is the same while it is
//! the same allocation: each one is held weakly, so that its place can't be taken by another one
//! while the index remembers it, and nothing of its content is kept alive).

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::sync::{Arc, Weak};

use crate::{Id, Mailbox, Message};

/// What the index knows of one message: the message as it was seen, and its conversation's
/// key (`None` for one with no topic, which is a conversation of its own).
struct Seen {
    id: Id,
    message: Weak<Message>,
    key: Option<u64>,
}

/// The messages of every conversation, by a hash of their account and topic; a hash that two
/// conversations share is told apart when looked up.
#[derive(Default)]
pub struct Threads {
    seen: Vec<Seen>,
    /// The last walk's list, empty, kept for the next walk to write into without asking for
    /// memory again.
    spare: Vec<Seen>,
    by_key: HashMap<u64, Vec<Id>>,
    hasher: RandomState,
}

impl std::fmt::Debug for Threads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Threads").field("messages", &self.seen.len()).field("conversations", &self.by_key.len()).finish()
    }
}

impl Threads {
    fn unfile(by_key: &mut HashMap<u64, Vec<Id>>, key: Option<u64>, id: Id) {
        let Some(key) = key else { return };
        if let Some(ids) = by_key.get_mut(&key) {
            ids.retain(|x| *x != id);
            if ids.is_empty() {
                by_key.remove(&key);
            }
        }
    }

    /// Brings the index in step with `mb`: what changed, came or went since the last call is
    /// filed again; the rest isn't looked at. The first call files every message.
    pub fn update(&mut self, mb: &Mailbox) {
        // The same messages as before, as is usual (read marks, flags, a move): compared where
        // they lie, and only those changed are written.
        if self.seen.len() == mb.messages.len() && self.update_in_place(mb) {
            return;
        }
        let mut was = std::mem::take(&mut self.seen);
        let mut seen = std::mem::take(&mut self.spare);
        seen.clear();
        seen.reserve(mb.messages.len());
        let mut by_key = std::mem::take(&mut self.by_key);
        let mut old = was.drain(..).peekable();
        for (id, m) in &mb.messages {
            // Gone since: those of a lower id that weren't met.
            while let Some(gone) = old.next_if(|s| s.id < *id) {
                Self::unfile(&mut by_key, gone.key, gone.id);
            }
            match old.next_if(|s| s.id == *id) {
                Some(same) if std::ptr::eq(same.message.as_ptr(), Arc::as_ptr(m)) => seen.push(same),
                was => {
                    let key = key_of(&self.hasher, m);
                    if let Some(was) = &was {
                        if was.key == key {
                            seen.push(Seen { id: *id, message: Arc::downgrade(m), key });
                            continue;
                        }
                        Self::unfile(&mut by_key, was.key, *id);
                    }
                    if let Some(key) = key {
                        by_key.entry(key).or_default().push(*id);
                    }
                    seen.push(Seen { id: *id, message: Arc::downgrade(m), key });
                }
            }
        }
        for gone in old {
            Self::unfile(&mut by_key, gone.key, gone.id);
        }
        self.seen = seen;
        self.spare = was;
        self.by_key = by_key;
    }

    /// [`Threads::update`] when `mb` has the messages the index knows, by id, in one walk:
    /// `false` when it hasn't, with those before the first that differs brought in step (which
    /// the walk of [`Threads::update`] then finds so).
    fn update_in_place(&mut self, mb: &Mailbox) -> bool {
        for (entry, (id, m)) in self.seen.iter_mut().zip(&mb.messages) {
            if entry.id != *id {
                return false;
            }
            if std::ptr::eq(entry.message.as_ptr(), Arc::as_ptr(m)) {
                continue;
            }
            let key = key_of(&self.hasher, m);
            entry.message = Arc::downgrade(m);
            let was = std::mem::replace(&mut entry.key, key);
            if was != key {
                Self::unfile(&mut self.by_key, was, entry.id);
                if let Some(key) = key {
                    self.by_key.entry(key).or_default().push(entry.id);
                }
            }
        }
        true
    }

    /// Brings the index in step with `mb` where only the messages `ids` changed, each of them
    /// still there (read marks): without the walk. `false`, and nothing changed, when one of
    /// them isn't known to the index or is no longer in `mb`: [`Threads::update`] is for that.
    pub fn renew(&mut self, mb: &Mailbox, ids: &[Id]) -> bool {
        let at = |seen: &[Seen], id: Id| seen.binary_search_by_key(&id, |s| s.id).ok();
        if !ids.iter().all(|id| at(&self.seen, *id).is_some() && mb.messages.contains_key(id)) {
            return false;
        }
        for id in ids {
            let (Some(i), Some(m)) = (at(&self.seen, *id), mb.messages.get(id)) else { continue };
            let key = key_of(&self.hasher, m);
            let Some(entry) = self.seen.get_mut(i) else { continue };
            let was = std::mem::replace(&mut entry.key, key);
            entry.message = Arc::downgrade(m);
            if was != key {
                Self::unfile(&mut self.by_key, was, *id);
                if let Some(key) = key {
                    self.by_key.entry(key).or_default().push(*id);
                }
            }
        }
        true
    }

    /// [`Mailbox::conversation`] of `id`, from the index as the last [`Threads::update`] left
    /// it, which must have been given `mb` as it is now.
    pub fn conversation(&self, mb: &Mailbox, id: Id) -> Vec<Id> {
        let Some(m) = mb.message(id) else { return Vec::new() };
        let Some(ids) = key_of(&self.hasher, m).and_then(|key| self.by_key.get(&key)) else { return vec![m.id] };
        let skips = mb.conversation_skips(m.account, m.folder);
        mb.conversation_among(m, ids.iter().filter_map(|i| mb.message(*i)).filter(|x| x.account == m.account && !skips.contains(&x.folder)))
    }
}

/// A hash of the message's account and [`Message::thread_key`], worked out without copying
/// the subject when it is ASCII, as most are (the first walk over a large mailbox works out
/// every message's). `None` for one with no topic.
fn key_of(hasher: &RandomState, m: &Message) -> Option<u64> {
    let topic = crate::message::topic_of(&m.subject);
    if topic.is_empty() {
        return None;
    }
    let mut h = hasher.build_hasher();
    h.write_u64(m.account);
    // The bytes of the key, case-folded, in pieces of the same size whichever way they came,
    // so that a key reached both ways (U+212A KELVIN SIGN folds to an ASCII `k`) hashes alike.
    let mut feed = |bytes: &[u8]| {
        let mut piece = [0u8; 64];
        for chunk in bytes.chunks(piece.len()) {
            let Some(into) = piece.get_mut(..chunk.len()) else { continue };
            for (to, from) in into.iter_mut().zip(chunk) {
                *to = from.to_ascii_lowercase();
            }
            h.write(into);
        }
    };
    if topic.is_ascii() {
        feed(topic.as_bytes());
    } else {
        feed(topic.to_lowercase().as_bytes());
    }
    Some(h.finish())
}
