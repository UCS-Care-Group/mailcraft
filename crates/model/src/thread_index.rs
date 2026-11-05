// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which messages are one conversation (#795).
//!
//! Two messages of one account are of one conversation when their topics are the same (the
//! subject without `Re:` and `Fw:`, letter case aside, [`Message::topic_key`]) **and** they are
//! linked, directly or through others of the same topic:
//!
//! - by their headers: one's `Message-ID` is the other's `In-Reply-To` or among its
//!   `References`, or both name the same message there (a parent that isn't here still links
//!   its children), as RFC 5256's REFERENCES does, without its step that joins threads by
//!   subject;
//! - or by the conversation id their server gave them ([`crate::sync::ServerMessage::thread`]:
//!   Graph's `conversationId`, EWS's `ConversationId`, JMAP's `threadId`).
//!
//! A message without a subject is a conversation of its own, and so is one linked to nothing.
//! A thread moved to an account of another kind stays one: its headers go with it.
//!
//! A message **not known** yet ([`unknown`]) is one that nothing links and nothing could: a
//! server's message that its server hasn't given a conversation id and hasn't yet been asked
//! for one ([`crate::sync::ServerMessage::thread_asked`]: once it has, and the server gave none,
//! it links by its headers alone, as an IMAP server's mail does), and whose headers, all that is here of it, name no other; or one with no
//! `Message-ID` at all. Those of one topic that nothing else links to either are one
//! conversation among themselves, as every conversation was before, until a Send/Receive brings
//! their ids; one that a known message links to is of that message's conversation alone.
//!
//! Everything here is read from mail and from servers, so it is hostile: of each message only
//! its `Message-ID`, its `In-Reply-To` and its last [`MAX_REFERENCES`] `References` of at most
//! [`MAX_LINK_ID`] bytes are looked at, and a conversation id only as [`thread_id`] keeps it. The
//! work is linear in the messages and their links: messages are put by topic, and those of one
//! topic joined by a union-find over a map from each id to the first message that named it.

use std::collections::HashMap;

use crate::sync::thread_id;
use crate::{Id, Message};

/// Most `References` of a message that link it: the last ones, its nearest ancestors.
pub const MAX_REFERENCES: usize = 32;
/// The longest message id that links (RFC 5322's line), in bytes; a longer one links nothing.
pub const MAX_LINK_ID: usize = 998;

/// A conversation as the message list, the reading pane and the commands that act on a whole
/// conversation know it ([`ThreadIndex::key`]): the smallest id of its messages, which stands
/// for it while it keeps that message. Within one mailbox, two conversations never share one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThreadKey(pub Id);

/// The conversations of a set of messages ([`ThreadIndex::build`]).
#[derive(Clone, Debug, Default)]
pub struct ThreadIndex {
    /// The key of each message of a conversation of more than one; one that isn't here is
    /// its own.
    keys: HashMap<Id, ThreadKey>,
    /// The messages of each conversation of more than one, by id.
    members: HashMap<ThreadKey, Vec<Id>>,
    /// Steps taken building it: for the tests that hold it to linear work.
    work: u64,
}

/// Whether nothing links `m` to the rest of its conversation, nor could (see the module's
/// documentation).
pub fn unknown(m: &Message) -> bool {
    if !m.in_reply_to.is_empty() || !m.references.is_empty() {
        return false;
    }
    match &m.server {
        // Asked of its server, which gave none (or refused, as often as it is asked): linked by
        // its headers alone, as mail of an IMAP server is.
        Some(server) if !server.thread_asked => thread_id(&server.thread).is_empty(),
        _ => m.message_id.trim().is_empty(),
    }
}

/// The message ids of `m` that link it: its own, what it answers, and its last references.
fn links(m: &Message) -> impl Iterator<Item = &str> {
    let refs = m.references.len().saturating_sub(MAX_REFERENCES);
    let refs = m.references.get(refs..).unwrap_or(&[]);
    std::iter::once(m.message_id.as_str())
        .chain(std::iter::once(m.in_reply_to.as_str()))
        .chain(refs.iter().map(String::as_str))
        .map(|id| id.trim().trim_start_matches('<').trim_end_matches('>'))
        .filter(|id| !id.is_empty() && id.len() <= MAX_LINK_ID)
}

/// A union-find over the messages of one topic.
struct Sets {
    parent: Vec<usize>,
    size: Vec<usize>,
    work: u64,
}

impl Sets {
    fn new(n: usize) -> Sets {
        Sets { parent: (0..n).collect(), size: vec![1; n], work: 0 }
    }

    fn find(&mut self, mut x: usize) -> usize {
        let mut root = x;
        while let Some(&up) = self.parent.get(root) {
            self.work = self.work.saturating_add(1);
            if up == root {
                break;
            }
            root = up;
        }
        // Every one on the way now points at the root.
        while let Some(up) = self.parent.get_mut(x) {
            if *up == root {
                break;
            }
            let next = *up;
            *up = root;
            x = next;
        }
        root
    }

    /// Whether `x` is in a set of its own.
    fn alone(&mut self, x: usize) -> bool {
        let root = self.find(x);
        self.size.get(root).copied().unwrap_or(1) <= 1
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a == b {
            return;
        }
        let (sa, sb) = (self.size.get(a).copied().unwrap_or(1), self.size.get(b).copied().unwrap_or(1));
        let (big, small) = if sa >= sb { (a, b) } else { (b, a) };
        if let Some(up) = self.parent.get_mut(small) {
            *up = big;
        }
        if let Some(size) = self.size.get_mut(big) {
            *size = sa.saturating_add(sb);
        }
    }
}

impl ThreadIndex {
    /// The conversations of `messages`: those of one account and one topic, linked as the
    /// module's documentation says.
    pub fn build<'a>(messages: impl IntoIterator<Item = &'a Message>) -> ThreadIndex {
        let mut index = ThreadIndex::default();
        let mut topics: HashMap<(Id, String), Vec<&Message>> = HashMap::new();
        for m in messages {
            index.work = index.work.saturating_add(1);
            let topic = m.topic_key();
            if !topic.is_empty() {
                topics.entry((m.account, topic)).or_default().push(m);
            }
        }
        for same in topics.into_values().filter(|same| same.len() > 1) {
            index.join(&same);
        }
        index
    }

    /// Link the messages `same`, all of one account and one topic, into their conversations.
    fn join(&mut self, same: &[&Message]) {
        let mut sets = Sets::new(same.len());
        let mut named: HashMap<&str, usize> = HashMap::new();
        let mut threads: HashMap<String, usize> = HashMap::new();
        for (i, m) in same.iter().enumerate() {
            for id in links(m) {
                self.work = self.work.saturating_add(1);
                match named.get(id) {
                    Some(j) => sets.union(i, *j),
                    None => {
                        named.insert(id, i);
                    }
                }
            }
            let thread = m.server.as_ref().map(|s| thread_id(&s.thread)).unwrap_or_default();
            if !thread.is_empty() {
                self.work = self.work.saturating_add(1);
                match threads.get(&thread) {
                    Some(j) => sets.union(i, *j),
                    None => {
                        threads.insert(thread, i);
                    }
                }
            }
        }
        // Then the unknowns, among themselves, and only those still linked to nothing: one that a
        // reply names is of that reply's conversation, and joining it to the other unknowns would
        // join two threads through it (#795).
        let alone: Vec<usize> = (0..same.len()).filter(|&i| same.get(i).is_some_and(|m| unknown(m)) && sets.alone(i)).collect();
        if let Some((&first, rest)) = alone.split_first() {
            for &i in rest {
                sets.union(i, first);
            }
        }
        let mut groups: HashMap<usize, Vec<Id>> = HashMap::new();
        for (i, m) in same.iter().enumerate() {
            let root = sets.find(i);
            groups.entry(root).or_default().push(m.id);
        }
        self.work = self.work.saturating_add(sets.work);
        for mut ids in groups.into_values().filter(|ids| ids.len() > 1) {
            ids.sort_unstable();
            let Some(key) = ids.first().copied().map(ThreadKey) else { continue };
            for id in &ids {
                self.keys.insert(*id, key);
            }
            self.members.insert(key, ids);
        }
    }

    /// The conversation of the message `id`: its own when it is linked to no other (or isn't
    /// among the messages this was built of).
    pub fn key(&self, id: Id) -> ThreadKey {
        self.keys.get(&id).copied().unwrap_or(ThreadKey(id))
    }

    /// The ids of the messages of the conversation of the message `id`, smallest first: `id`
    /// alone when it is linked to no other.
    pub fn members(&self, id: Id) -> Vec<Id> {
        self.members.get(&self.key(id)).cloned().unwrap_or_else(|| vec![id])
    }

    /// Whether the messages `a` and `b` are of one conversation.
    pub fn same(&self, a: Id, b: Id) -> bool {
        self.key(a) == self.key(b)
    }

    /// How many steps building it took: linear in the messages and their links.
    pub fn work(&self) -> u64 {
        self.work
    }
}

#[cfg(test)]
#[path = "thread_index_tests.rs"]
mod thread_index_tests;
