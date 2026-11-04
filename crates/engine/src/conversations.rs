// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which messages are one conversation (#795), worked out once per change of the mailbox and
//! kept until the next: the message list, the reading pane's conversation and the commands that
//! act on a whole conversation all ask, and the answer is a pass over every message
//! ([`ThreadIndex`]).

use std::sync::Arc;

use mailcraft_model::{Id, ThreadIndex, ThreadKey};

use crate::Session;

impl Session {
    /// The conversations of the mailbox as it is now: every account's, of every folder (a
    /// message deleted still links the rest of its thread, as its headers do).
    pub fn threads(&mut self) -> Arc<ThreadIndex> {
        let key = (self.rev(), self.mailbox.messages.len());
        if let Some((at, threads)) = &self.threads_cache
            && *at == key
        {
            return threads.clone();
        }
        let threads = Arc::new(ThreadIndex::build(self.mailbox.messages.values().map(|m| m.as_ref())));
        self.threads_cache = Some((key, threads.clone()));
        threads
    }

    /// The conversations, when they are of the mailbox as it is now ([`Session::threads`] was
    /// asked since it last changed).
    pub fn fresh_threads(&self) -> Option<&ThreadIndex> {
        let key = (self.rev(), self.mailbox.messages.len());
        self.threads_cache.as_ref().filter(|(at, _)| *at == key).map(|(_, threads)| threads.as_ref())
    }

    /// The conversations, for a caller that can't keep them ([`Session::threads`]): those kept
    /// when they are of the mailbox as it is now, else worked out afresh.
    pub fn threads_now(&self) -> std::borrow::Cow<'_, ThreadIndex> {
        match self.fresh_threads() {
            Some(threads) => std::borrow::Cow::Borrowed(threads),
            None => std::borrow::Cow::Owned(ThreadIndex::build(self.mailbox.messages.values().map(|m| m.as_ref()))),
        }
    }

    /// The conversation of the message `id` ([`ThreadIndex::key`]).
    pub fn thread_of(&mut self, id: Id) -> ThreadKey {
        self.threads().key(id)
    }
}
