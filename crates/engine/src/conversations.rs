// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which messages are one conversation (#795), worked out once and kept: the message list, the
//! reading pane's conversation and the commands that act on a whole conversation all ask, and the
//! answer is a pass over every message ([`ThreadIndex`]).
//!
//! What is kept is checked rather than thrown away when the mailbox changes: with it is kept, of
//! each message, a weak pointer to it and a hash of what [`ThreadIndex::build`] reads of it (its
//! account, subject, linking headers and server conversation id). When the mailbox has changed
//! since, the next ask walks the messages once: one that is the same `Arc` is the same, and only
//! one that isn't has its hash worked out again, and is pointed at as it is now, so that it isn't
//! hashed again at the next check and what it was is let go of. So a flag, a category, a move or
//! a read mark keeps the conversations (a pass of pointer comparisons), and a changed subject or
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

use mailcraft_model::{Id, Mailbox, Message, ThreadIndex, ThreadKey};

use crate::Session;

/// The conversations kept ([`Session::threads`]).
pub(crate) type ThreadsCache = Mutex<Option<Kept>>;

pub(crate) struct Kept {
    /// The revision and the count of messages they were last checked at; `None` once the mailbox
    /// may have changed since ([`Session::threads_may_change`]).
    at: Option<(u64, usize)>,
    threads: Arc<ThreadIndex>,
    /// Of each message, in the mailbox's order: its id, a weak pointer to it and the hash of what
    /// groups it.
    seen: Vec<(Id, Weak<Message>, u64)>,
    /// How many messages were hashed again in checks (for tests).
    rehashed: u64,
}

/// The hash of what [`ThreadIndex::build`] reads of `m`.
fn grouping_hash(m: &Message) -> u64 {
    let mut h = DefaultHasher::new();
    let server = m.server.as_ref().map(|s| (s.thread.as_str(), s.thread_asked));
    (m.account, &m.subject, &m.message_id, &m.in_reply_to, &m.references, server).hash(&mut h);
    h.finish()
}

/// Whether `kept` is of the messages of `mailbox`, as far as conversations go; a message found
/// the same by its hash is pointed at as it is now.
fn still_of(kept: &mut Kept, mailbox: &Mailbox) -> bool {
    if kept.seen.len() != mailbox.messages.len() {
        return false;
    }
    let mut rehashed = 0u64;
    let same = kept.seen.iter_mut().zip(&mailbox.messages).all(|((was_id, was, hash), (id, now))| {
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
    kept.rehashed = kept.rehashed.saturating_add(rehashed);
    same
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
        let seen = self.mailbox.messages.iter().map(|(id, m)| (*id, Arc::downgrade(m), grouping_hash(m))).collect();
        *cache = Some(Kept { at: Some(key), threads: threads.clone(), seen, rehashed: 0 });
        threads
    }

    /// The conversation of the message `id` ([`ThreadIndex::key`]).
    pub fn thread_of(&self, id: Id) -> ThreadKey {
        self.threads().key(id)
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
    let seen = before.messages.iter().map(|(id, m)| (*id, Arc::downgrade(m), grouping_hash(m))).collect();
    still_of(&mut Kept { at: None, threads: Arc::default(), seen, rehashed: 0 }, after)
}

#[cfg(test)]
impl Session {
    /// How many messages the checks of what is kept hashed again, and whether what is kept points
    /// at every message as it is now (holds nothing of one that was).
    pub(crate) fn threads_checked(&self) -> (u64, bool) {
        let cache = self.threads_cache.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(kept) = cache.as_ref() else { return (0, false) };
        let current = kept.seen.iter().zip(&self.mailbox.messages).all(|((_, was, _), (_, now))| std::ptr::eq(was.as_ptr(), Arc::as_ptr(now)));
        (kept.rehashed, current)
    }
}
