// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which messages are one conversation (#795), worked out once and kept: the message list, the
//! reading pane's conversation and the commands that act on a whole conversation all ask, and the
//! answer is a pass over every message ([`ThreadIndex`]).
//!
//! What is kept is checked rather than thrown away when the mailbox changes: with it is kept, of
//! each chunk of the map of messages ([`Messages`](mailcraft_model::Messages): 1,024 consecutive
//! ids, shared between the copies of the map until one changes it), a weak pointer to the chunk,
//! and of each of its messages a weak pointer to it and a hash of what [`ThreadIndex::build`]
//! reads of it (its account, subject, linking headers and server conversation id). When the
//! mailbox has changed since, the next ask walks the chunks once: one that is the same `Arc` holds
//! the same messages and isn't looked into; in one that isn't, a message that is the same `Arc`
//! is the same, and only one that isn't has its hash worked out again, and is pointed at as it is
//! now, so that it isn't hashed again at the next check and what it was is let go of. So a flag,
//! a category, a move or a read mark keeps the conversations (a pass of pointer comparisons over
//! a few hundred chunks, and over the messages of the few that changed), and a changed subject or
//! header, a server's answer about its conversation, or a message more or less, has them worked
//! out again.
//!
//! The check is made at the first ask after a command begins and after it ends. Within one
//! command, an ask, then a change of what groups a message, then another ask would get the first
//! answer: no command does that (each works out the conversation it acts on before it changes
//! anything), and one that would must call [`Session::threads_may_change`] between the two.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use mailcraft_model::messages::Chunk;
use mailcraft_model::{Id, Mailbox, Message, ThreadIndex, ThreadKey};

use crate::Session;

/// The conversations kept ([`Session::threads`]).
pub(crate) type ThreadsCache = Mutex<Option<Kept>>;

pub(crate) struct Kept {
    /// The revision and the count of messages they were last checked at; `None` once the mailbox
    /// may have changed since ([`Session::threads_may_change`]).
    at: Option<(u64, usize)>,
    threads: Arc<ThreadIndex>,
    /// Of each chunk of the map of messages, in the map's order.
    seen: Vec<SeenChunk>,
    /// How many messages were hashed again in checks (for tests).
    rehashed: u64,
}

/// What is kept of one chunk of the map of messages: the chunk as it was seen, and of each of
/// its messages, in order of id, its id, a weak pointer to it and the hash of what groups it.
struct SeenChunk {
    key: Id,
    chunk: Weak<Chunk>,
    messages: Vec<(Id, Weak<Message>, u64)>,
}

impl SeenChunk {
    fn of(key: Id, chunk: &Arc<Chunk>) -> SeenChunk {
        let messages = chunk.iter().map(|(id, m)| (*id, Arc::downgrade(m), grouping_hash(m))).collect();
        SeenChunk { key, chunk: Arc::downgrade(chunk), messages }
    }
}

/// The hash of what [`ThreadIndex::build`] reads of `m`.
fn grouping_hash(m: &Message) -> u64 {
    let mut h = DefaultHasher::new();
    let server = m.server.as_ref().map(|s| (s.thread.as_str(), s.thread_asked));
    (m.account, &m.subject, &m.message_id, &m.in_reply_to, &m.references, server).hash(&mut h);
    h.finish()
}

/// Whether `a` and `b` (a message as it was and as it is) group the same, as far as
/// [`ThreadIndex::build`] reads them: for a list that would stand through the change otherwise
/// ([`Session::list_stands`]).
pub(crate) fn same_grouping(a: &Message, b: &Message) -> bool {
    let server = |m: &Message| m.server.as_ref().map(|s| (s.thread.clone(), s.thread_asked));
    a.account == b.account
        && a.subject == b.subject
        && a.message_id == b.message_id
        && a.in_reply_to == b.in_reply_to
        && a.references == b.references
        && server(a) == server(b)
}

/// Whether `kept` is of the messages of `mailbox`, as far as conversations go; a chunk or a
/// message found the same is pointed at as it is now.
fn still_of(kept: &mut Kept, mailbox: &Mailbox) -> bool {
    let chunks = mailbox.messages.chunks();
    if kept.seen.len() != chunks.len() {
        return false;
    }
    let mut rehashed = 0u64;
    let same = kept.seen.iter_mut().zip(chunks).all(|(seen, (key, chunk))| {
        if seen.key != *key {
            return false;
        }
        if std::ptr::eq(seen.chunk.as_ptr(), Arc::as_ptr(chunk)) {
            return true;
        }
        if seen.messages.len() != chunk.len() {
            return false;
        }
        let same = seen.messages.iter_mut().zip(chunk.iter()).all(|((was_id, was, hash), (id, now))| {
            if was_id != id {
                return false;
            }
            if std::ptr::eq(was.as_ptr(), Arc::as_ptr(now)) {
                return true;
            }
            rehashed = rehashed.saturating_add(1);
            let same = grouping_hash(now) == *hash;
            if same {
                *was = Arc::downgrade(now);
            }
            same
        });
        if same {
            seen.chunk = Arc::downgrade(chunk);
        }
        same
    });
    kept.rehashed = kept.rehashed.saturating_add(rehashed);
    same
}

fn seen_of(mailbox: &Mailbox) -> Vec<SeenChunk> {
    mailbox.messages.chunks().iter().map(|(key, chunk)| SeenChunk::of(*key, chunk)).collect()
}

impl Session {
    fn threads_key(&self) -> (u64, usize) {
        (self.rev(), self.mailbox.messages.len())
    }

    /// The conversations of the mailbox as it is now: every account's, of every folder (a
    /// message deleted still links the rest of its thread, as its headers do). Kept for the next
    /// ask, so a caller that can only read the session keeps them too.
    pub fn threads(&self) -> Arc<ThreadIndex> {
        let key = self.threads_key();
        let mut cache = self.threads_cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(kept) = cache.as_mut() {
            if kept.at == Some(key) {
                return kept.threads.clone();
            }
            if still_of(kept, &self.mailbox) {
                kept.at = Some(key);
                return kept.threads.clone();
            }
        }
        let threads = Arc::new(ThreadIndex::build(self.mailbox.messages.values().map(|m| m.as_ref())));
        *cache = Some(Kept { at: Some(key), threads: threads.clone(), seen: seen_of(&self.mailbox), rehashed: 0 });
        threads
    }

    /// The conversation of the message `id` ([`ThreadIndex::key`]).
    pub fn thread_of(&self, id: Id) -> ThreadKey {
        self.threads().key(id)
    }

    /// Has the conversations worked out now, when they aren't yet in step with the mailbox: for
    /// a window to have them built right after the mailbox opens, rather than at the first click
    /// on a message or the first list in conversation view. Whether they were worked out again.
    pub fn prepare_conversations(&self) -> bool {
        let key = self.threads_key();
        let in_step = {
            let cache = self.threads_cache.lock().unwrap_or_else(PoisonError::into_inner);
            cache.as_ref().is_some_and(|kept| kept.at == Some(key))
        };
        if in_step {
            return false;
        }
        let _ = self.threads();
        true
    }

    /// The mailbox is about to change under the same revision (a command runs): what is kept is
    /// checked at the next ask, even before the revision moves on.
    pub(crate) fn threads_may_change(&mut self) {
        if let Some(kept) = self.threads_cache.get_mut().unwrap_or_else(PoisonError::into_inner).as_mut() {
            kept.at = None;
        }
    }
}

/// Whether the conversations of `before` are those of `after`, as [`Session::threads`] checks.
#[cfg(test)]
pub(crate) fn grouping_kept(before: &Mailbox, after: &Mailbox) -> bool {
    still_of(&mut Kept { at: None, threads: Arc::default(), seen: seen_of(before), rehashed: 0 }, after)
}

#[cfg(test)]
impl Session {
    /// How many messages the checks of what is kept hashed again, and whether what is kept points
    /// at every chunk and every message as it is now (holds nothing of one that was).
    pub(crate) fn threads_checked(&self) -> (u64, bool) {
        let cache = self.threads_cache.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(kept) = cache.as_ref() else { return (0, false) };
        let chunks = self.mailbox.messages.chunks();
        let current = kept.seen.len() == chunks.len()
            && kept.seen.iter().zip(chunks).all(|(seen, (_, chunk))| {
                std::ptr::eq(seen.chunk.as_ptr(), Arc::as_ptr(chunk))
                    && seen.messages.len() == chunk.len()
                    && seen.messages.iter().zip(chunk.iter()).all(|((_, was, _), (_, now))| std::ptr::eq(was.as_ptr(), Arc::as_ptr(now)))
            });
        (kept.rehashed, current)
    }
}
