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
//! - or, for a reply kept as a draft, by the message it answers ([`Message::answers`]), which
//!   keeps the link when a server's copy of the draft has no headers (#1922);
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
use std::hash::{BuildHasherDefault, Hasher};

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

/// A hasher for message ids, which the mailbox gives out and no sender chooses: a multiply that
/// spreads them over the table, much cheaper than the default one, which is for keys from
/// outside.
#[derive(Default)]
pub struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(u64::from(*b));
        }
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

type ById<V> = HashMap<Id, V, BuildHasherDefault<IdHasher>>;

/// How many parts an index is kept in, each of the messages whose ids fall in it ([`shard_of`]):
/// so that the parts of a large one can be filled on threads of their own
/// ([`ThreadIndex::of_joined`]).
const SHARDS: usize = 16;

/// The part of an index the message `id` falls in.
fn shard_of(id: Id) -> usize {
    usize::try_from(id.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 60).unwrap_or(0) % SHARDS
}

/// One part of an index ([`SHARDS`]).
#[derive(Clone, Debug, Default)]
struct Shard {
    /// The key of each message of a conversation of more than one; one that isn't here is
    /// its own.
    keys: ById<ThreadKey>,
    /// The messages of each conversation of more than one, by the id of its key.
    members: ById<Vec<Id>>,
}

impl Shard {
    /// The part `at` of the index of the conversations `groups` (each smallest first): its keys
    /// of the messages that fall in it, and its conversations `own`, those whose key falls in it.
    fn of(at: usize, groups: &[Vec<Vec<Id>>], own: Vec<Vec<Id>>) -> Shard {
        let ids = groups.iter().flatten().map(Vec::len).sum::<usize>();
        let mut keys = ById::with_capacity_and_hasher(ids / SHARDS + ids / (SHARDS * 8), BuildHasherDefault::default());
        for ids in groups.iter().flatten() {
            let Some(key) = ids.first().copied().map(ThreadKey) else { continue };
            for id in ids.iter().filter(|id| shard_of(**id) == at) {
                keys.insert(*id, key);
            }
        }
        let mut members = ById::with_capacity_and_hasher(own.len(), BuildHasherDefault::default());
        for ids in own {
            if let Some(key) = ids.first().copied() {
                members.insert(key, ids);
            }
        }
        Shard { keys, members }
    }
}

/// The conversations of a set of messages ([`ThreadIndex::build`]).
#[derive(Clone, Debug, Default)]
pub struct ThreadIndex {
    /// Its parts ([`SHARDS`] of them, or none while it holds nothing).
    shards: Vec<Shard>,
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
        // A reply not sent yet knows its original, though a server's copy of the draft may have lost
        // the headers that say so (#1922).
        .chain(m.answers.iter().filter(|a| !a.forward).map(|a| a.message_id.as_str()))
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

/// The conversations of more than one message of one topic ([`Joined::of`]), worked out apart
/// from an index: for the topics of a mailbox worked out on several threads, then taken in
/// ([`ThreadIndex::take_in`]).
#[derive(Debug, Default)]
pub struct Joined {
    /// The messages of each, smallest first.
    groups: Vec<Vec<Id>>,
    work: u64,
}

impl Joined {
    /// The conversations of `same`, messages all of one account and one topic, in order of id.
    pub fn of(same: &[&Message]) -> Joined {
        Joined::of_topics([same])
    }

    /// The conversations of each of `topics` ([`Joined::of`]), together.
    pub fn of_topics<'a>(topics: impl IntoIterator<Item = &'a [&'a Message]>) -> Joined {
        let mut joined = Joined::default();
        // Kept from one topic to the next, as long as they stay small.
        let mut named: HashMap<&str, usize> = HashMap::new();
        let mut threads: HashMap<String, usize> = HashMap::new();
        let mut by_set: Vec<(usize, Id)> = Vec::new();
        for same in topics {
            if same.len() < 2 {
                continue;
            }
            joined.join(same, &mut named, &mut threads, &mut by_set);
            if named.capacity() > 4096 {
                named = HashMap::new();
            }
            named.clear();
            threads.clear();
        }
        joined
    }

    fn join<'a>(
        &mut self,
        same: &[&'a Message],
        named: &mut HashMap<&'a str, usize>,
        threads: &mut HashMap<String, usize>,
        by_set: &mut Vec<(usize, Id)>,
    ) {
        let mut sets = Sets::new(same.len());
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
        let mut first = None;
        for i in 0..same.len() {
            if same.get(i).is_some_and(|m| unknown(m)) && sets.alone(i) {
                match first {
                    None => first = Some(i),
                    Some(first) => sets.union(i, first),
                }
            }
        }
        // Each message by its set; within one, in order of id, as `same` is.
        by_set.clear();
        by_set.extend(same.iter().enumerate().map(|(i, m)| (sets.find(i), m.id)));
        by_set.sort_by_key(|(set, _)| *set);
        self.work = self.work.saturating_add(sets.work);
        let mut rest = by_set.as_slice();
        while let Some(&(set, _)) = rest.first() {
            let end = rest.iter().position(|(other, _)| *other != set).unwrap_or(rest.len());
            let (group, after) = rest.split_at(end);
            if group.len() > 1 {
                let mut ids: Vec<Id> = group.iter().map(|(_, id)| *id).collect();
                ids.sort_unstable();
                self.groups.push(ids);
            }
            rest = after;
        }
    }

    /// How many messages its conversations hold.
    pub fn len(&self) -> usize {
        self.groups.iter().map(Vec::len).sum()
    }

    /// Whether it holds no conversation of more than one message.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
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

    /// Work the conversations of some of the topics out again (#1897): `was`, every id those
    /// topics had when they were last worked out, and `topics`, the messages of each now, in
    /// order of id. What it gives is what [`ThreadIndex::build`] would give of the mailbox as it
    /// is now, as long as every topic that changed is among them: a conversation is of one
    /// account and one topic, so the others stand as they are. (All of `was` goes first: a
    /// message whose subject changed was of one of them and is of another.)
    pub fn rejoin(&mut self, was: &[Id], topics: &[Vec<&Message>]) {
        for id in was {
            self.work = self.work.saturating_add(1);
            let key = self.shards.get_mut(shard_of(*id)).and_then(|shard| shard.keys.remove(id));
            if let Some(key) = key
                && let Some(shard) = self.shards.get_mut(shard_of(key.0))
            {
                shard.members.remove(&key.0);
            }
        }
        for same in topics.iter().filter(|same| same.len() > 1) {
            self.join(same);
        }
    }

    /// Link the messages `same`, all of one account and one topic, into their conversations.
    fn join(&mut self, same: &[&Message]) {
        self.take_in(Joined::of(same));
    }

    /// Take in the conversations of a topic worked out apart ([`Joined::of`]), as
    /// [`ThreadIndex::rejoin`] would work them out: after every id the topic had before is
    /// forgotten.
    pub fn take_in(&mut self, joined: Joined) {
        self.work = self.work.saturating_add(joined.work);
        for ids in joined.groups {
            self.put(ids);
        }
    }

    /// Take in one conversation, its ids smallest first.
    fn put(&mut self, ids: Vec<Id>) {
        if self.shards.is_empty() {
            self.shards = vec![Shard::default(); SHARDS];
        }
        let Some(key) = ids.first().copied().map(ThreadKey) else { return };
        for id in &ids {
            if let Some(shard) = self.shards.get_mut(shard_of(*id)) {
                shard.keys.insert(*id, key);
            }
        }
        if let Some(shard) = self.shards.get_mut(shard_of(key.0)) {
            shard.members.insert(key.0, ids);
        }
    }

    /// The index of the conversations `joined`, of topics each worked out apart
    /// ([`Joined::of`]), each topic's once: what [`ThreadIndex::build`] gives of their messages.
    /// Its parts are filled on up to `threads` threads (a thread that can't be started has its
    /// parts filled on this one).
    pub fn of_joined(joined: Vec<Joined>, threads: usize) -> ThreadIndex {
        let threads = threads.clamp(1, SHARDS);
        let work = joined.iter().fold(0u64, |work, j| work.saturating_add(j.work));
        // The conversations, by the part their key falls in.
        let mut groups: Vec<Vec<Vec<Id>>> = vec![Vec::new(); SHARDS];
        for ids in joined.into_iter().flat_map(|j| j.groups) {
            if let Some(part) = ids.first().and_then(|key| groups.get_mut(shard_of(*key))) {
                part.push(ids);
            }
        }
        if threads == 1 {
            // One thread: each conversation goes in once, not looked for by each of the parts.
            let mut index = ThreadIndex::default();
            for ids in groups.into_iter().flatten() {
                index.put(ids);
            }
            index.work = work;
            return index;
        }
        // The keys of each part are read from all of them, so each part's own conversations go
        // to it after.
        let keys = |from: usize| -> Vec<(usize, ById<ThreadKey>)> {
            (from..SHARDS).step_by(threads).map(|at| (at, Shard::of(at, &groups, Vec::new()).keys)).collect()
        };
        let mut shards: Vec<Shard> = vec![Shard::default(); SHARDS];
        let filled: Vec<(usize, ById<ThreadKey>)> = std::thread::scope(|scope| {
            let keys = &keys;
            let started: Vec<_> = (1..threads)
                .map(|from| (from, std::thread::Builder::new().name("conversations".into()).spawn_scoped(scope, move || keys(from)).ok()))
                .collect();
            let mut filled = keys(0);
            for (from, thread) in started {
                filled.extend(thread.and_then(|t| t.join().ok()).unwrap_or_else(|| keys(from)));
            }
            filled
        });
        for (at, keys) in filled {
            if let Some(shard) = shards.get_mut(at) {
                shard.keys = keys;
            }
        }
        for (shard, own) in shards.iter_mut().zip(groups) {
            shard.members = Shard::of(0, &[], own).members;
        }
        ThreadIndex { shards, work }
    }

    /// The conversation of the message `id`: its own when it is linked to no other (or isn't
    /// among the messages this was built of).
    pub fn key(&self, id: Id) -> ThreadKey {
        self.shards.get(shard_of(id)).and_then(|shard| shard.keys.get(&id)).copied().unwrap_or(ThreadKey(id))
    }

    /// The ids of the messages of the conversation of the message `id`, smallest first: `id`
    /// alone when it is linked to no other.
    pub fn members(&self, id: Id) -> Vec<Id> {
        let key = self.key(id);
        self.shards.get(shard_of(key.0)).and_then(|shard| shard.members.get(&key.0)).cloned().unwrap_or_else(|| vec![id])
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
