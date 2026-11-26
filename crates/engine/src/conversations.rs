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
//! a few hundred chunks, and over the messages of the few that changed). A changed subject or
//! header, a server's answer about its conversation, or a message more or less has only its
//! topics worked out again (#1897): a conversation is of one account and one topic
//! ([`ThreadIndex::rejoin`]), and with each message is kept its topic, and with each topic its
//! messages. So a new account's first download, a piece at a time, doesn't go over every message
//! of the mailbox at each piece. A change past [`most_changed`] (another mailbox opened in its
//! place) has them worked out over every message.
//!
//! The check is made at the first ask after a command begins and after it ends. Within one
//! command, an ask, then a change of what groups a message, then another ask would get the first
//! answer: no command does that (each works out the conversation it acts on before it changes
//! anything), and one that would must call [`Session::threads_may_change`] between the two.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use mailcraft_model::messages::Chunk;
use mailcraft_model::{Id, Mailbox, Message, ThreadIndex, ThreadKey};

use crate::Session;

/// The conversations kept ([`Session::threads`]).
pub(crate) type ThreadsCache = Mutex<Option<Kept>>;

/// A topic of an account ([`Message::topic_key`]), shared by the messages of it that are kept.
type Topic = Arc<(Id, String)>;

pub(crate) struct Kept {
    /// The revision and the count of messages they were last checked at; `None` once the mailbox
    /// may have changed since ([`Session::threads_may_change`]).
    at: Option<(u64, usize)>,
    threads: Arc<ThreadIndex>,
    /// Of each chunk of the map of messages, in the map's order.
    seen: Vec<SeenChunk>,
    /// The ids of the messages of each topic, smallest first: the topics a change touches are
    /// worked out again, and only those (#1897).
    topics: HashMap<Topic, Vec<Id>>,
    /// How many messages the checks hashed again (for tests).
    rehashed: u64,
    /// How many times they were worked out over every message (for tests).
    built: u64,
}

/// What is kept of one chunk of the map of messages: the chunk as it was seen, and of each of
/// its messages, in order of id, what was seen of it.
struct SeenChunk {
    key: Id,
    chunk: Weak<Chunk>,
    messages: Vec<Seen>,
}

/// What was seen of one message: a weak pointer to it, the hash of what groups it, and its topic
/// (none, without a subject: a conversation of its own).
struct Seen {
    id: Id,
    message: Weak<Message>,
    hash: u64,
    topic: Option<Topic>,
}

/// The hash of what [`ThreadIndex::build`] reads of `m`.
fn grouping_hash(m: &Message) -> u64 {
    let mut h = DefaultHasher::new();
    let server = m.server.as_ref().map(|s| (s.thread.as_str(), s.thread_asked));
    (m.account, &m.subject, &m.message_id, &m.in_reply_to, &m.references, server).hash(&mut h);
    m.answers.as_ref().map(|a| (a.forward, &a.message_id)).hash(&mut h);
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
        && a.answers.as_ref().map(|x| (x.forward, &x.message_id)) == b.answers.as_ref().map(|x| (x.forward, &x.message_id))
        && server(a) == server(b)
}

/// The topic of `m`, the one kept when there is one (so each is kept once).
fn topic_of(topics: &HashMap<Topic, Vec<Id>>, m: &Message) -> Option<Topic> {
    let key = m.topic_key();
    if key.is_empty() {
        return None;
    }
    let key = (m.account, key);
    Some(topics.get_key_value(&key).map_or_else(|| Arc::new(key), |(topic, _)| topic.clone()))
}

fn seen(topics: &HashMap<Topic, Vec<Id>>, id: Id, m: &Arc<Message>) -> Seen {
    Seen { id, message: Arc::downgrade(m), hash: grouping_hash(m), topic: topic_of(topics, m) }
}

/// What is kept of `mailbox`, with its conversations worked out over every message.
fn kept_of(mailbox: &Mailbox, at: Option<(u64, usize)>, built: u64) -> Kept {
    // Each topic with its messages, each message's topic found once.
    let mut by_topic: HashMap<(Id, String), (Topic, Vec<Id>, Vec<&Message>)> = HashMap::new();
    let mut seen_chunks = Vec::with_capacity(mailbox.messages.chunks().len());
    for (key, chunk) in mailbox.messages.chunks() {
        let mut messages = Vec::with_capacity(chunk.len());
        for (id, m) in chunk.iter() {
            let name = m.topic_key();
            let topic = (!name.is_empty()).then(|| {
                let (topic, ids, of_it) = by_topic.entry((m.account, name)).or_insert_with_key(|k| (Arc::new(k.clone()), Vec::new(), Vec::new()));
                ids.push(*id);
                of_it.push(m.as_ref());
                topic.clone()
            });
            messages.push(Seen { id: *id, message: Arc::downgrade(m), hash: grouping_hash(m), topic });
        }
        seen_chunks.push(SeenChunk { key: *key, chunk: Arc::downgrade(chunk), messages });
    }
    let mut topics = HashMap::with_capacity(by_topic.len());
    let mut lists = Vec::with_capacity(by_topic.len());
    for (topic, ids, of_it) in by_topic.into_values() {
        topics.insert(topic, ids);
        lists.push(of_it);
    }
    let mut threads = ThreadIndex::default();
    threads.rejoin(&[], &lists);
    Kept { at, threads: Arc::new(threads), seen: seen_chunks, topics, rehashed: 0, built: built.saturating_add(1) }
}

/// Most messages a change may touch for its topics alone to be worked out again: past it (a
/// mailbox opened in place of another, a whole folder brought in at once) working them all out
/// is no slower.
fn most_changed(mailbox: &Mailbox) -> usize {
    (mailbox.messages.len() / 4).max(1024)
}

/// Bring `kept` in step with `mailbox`: a chunk that is the same `Arc` holds the same messages
/// and isn't looked into; in one that isn't, a message that is the same `Arc` is the same, and
/// only one that isn't has its hash worked out again, and is pointed at as it is now. The topics
/// of the messages that came, went or group otherwise are worked out again. Whether it could be
/// (`false` past [`most_changed`]: `kept` is then to be built afresh), and whether anything about
/// the conversations changed.
fn bring_in_step(kept: &mut Kept, mailbox: &Mailbox) -> Option<bool> {
    let most = most_changed(mailbox);
    let mut gone: Vec<(Id, Topic)> = Vec::new();
    let mut came: Vec<(Id, Topic)> = Vec::new();
    let mut changed = 0usize;
    let mut rehashed = 0u64;
    let mut old = std::mem::take(&mut kept.seen).into_iter().peekable();
    let mut now_seen = Vec::with_capacity(mailbox.messages.chunks().len());
    let went = |was: Seen, gone: &mut Vec<(Id, Topic)>, changed: &mut usize| {
        *changed = changed.saturating_add(1);
        if let Some(topic) = was.topic {
            gone.push((was.id, topic));
        }
    };
    for (key, chunk) in mailbox.messages.chunks() {
        while let Some(before) = old.next_if(|c| c.key < *key) {
            for was in before.messages {
                went(was, &mut gone, &mut changed);
            }
        }
        let before = old.next_if(|c| c.key == *key);
        let before = match before {
            Some(same) if std::ptr::eq(same.chunk.as_ptr(), Arc::as_ptr(chunk)) => {
                now_seen.push(same);
                continue;
            }
            other => other,
        };
        let mut was = before.map(|c| c.messages).unwrap_or_default().into_iter().peekable();
        let mut messages = Vec::with_capacity(chunk.len());
        for (id, now) in chunk.iter() {
            while let Some(left) = was.next_if(|w| w.id < *id) {
                went(left, &mut gone, &mut changed);
            }
            match was.next_if(|w| w.id == *id) {
                Some(same) if std::ptr::eq(same.message.as_ptr(), Arc::as_ptr(now)) => messages.push(same),
                Some(same) => {
                    rehashed = rehashed.saturating_add(1);
                    let hash = grouping_hash(now);
                    if hash == same.hash {
                        messages.push(Seen { message: Arc::downgrade(now), ..same });
                    } else {
                        went(same, &mut gone, &mut changed);
                        let one = seen(&kept.topics, *id, now);
                        came.extend(one.topic.clone().map(|topic| (*id, topic)));
                        messages.push(one);
                    }
                }
                None => {
                    changed = changed.saturating_add(1);
                    let one = seen(&kept.topics, *id, now);
                    came.extend(one.topic.clone().map(|topic| (*id, topic)));
                    messages.push(one);
                }
            }
            if changed > most {
                return None;
            }
        }
        for left in was {
            went(left, &mut gone, &mut changed);
        }
        now_seen.push(SeenChunk { key: *key, chunk: Arc::downgrade(chunk), messages });
    }
    for before in old {
        for was in before.messages {
            went(was, &mut gone, &mut changed);
        }
    }
    if changed > most {
        return None;
    }
    kept.seen = now_seen;
    kept.rehashed = kept.rehashed.saturating_add(rehashed);
    if gone.is_empty() && came.is_empty() {
        return Some(false);
    }
    // The topics touched, and every id they had.
    let touched: HashSet<Topic> = gone.iter().chain(&came).map(|(_, topic)| topic.clone()).collect();
    let was: Vec<Id> = touched.iter().filter_map(|topic| kept.topics.get(topic)).flatten().copied().collect();
    for (id, topic) in gone {
        if let Some(ids) = kept.topics.get_mut(&topic)
            && let Ok(at) = ids.binary_search(&id)
        {
            ids.remove(at);
        }
    }
    for (id, topic) in came {
        let ids = kept.topics.entry(topic).or_default();
        if let Err(at) = ids.binary_search(&id) {
            ids.insert(at, id);
        }
    }
    kept.topics.retain(|_, ids| !ids.is_empty());
    let topics: Vec<Vec<&Message>> =
        touched.iter().filter_map(|topic| kept.topics.get(topic)).map(|ids| ids.iter().filter_map(|id| mailbox.message(*id)).collect()).collect();
    Arc::make_mut(&mut kept.threads).rejoin(&was, &topics);
    Some(true)
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
        let mut built = 0;
        if let Some(kept) = cache.as_mut() {
            if kept.at == Some(key) {
                return kept.threads.clone();
            }
            if bring_in_step(kept, &self.mailbox).is_some() {
                kept.at = Some(key);
                return kept.threads.clone();
            }
            built = kept.built;
        }
        let kept = kept_of(&self.mailbox, Some(key), built);
        let threads = kept.threads.clone();
        *cache = Some(kept);
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
    bring_in_step(&mut kept_of(before, None, 0), after) == Some(false)
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
                    && seen.messages.iter().zip(chunk.iter()).all(|(was, (_, now))| std::ptr::eq(was.message.as_ptr(), Arc::as_ptr(now)))
            });
        (kept.rehashed, current)
    }

    /// How many times the conversations were worked out over every message (#1897).
    pub(crate) fn threads_built(&self) -> u64 {
        self.threads_cache.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map_or(0, |kept| kept.built)
    }
}
