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
//! That first build, at opening, is shared out among threads ([`kept_on`]): the chunks first,
//! each message's topic and hash; then the topics, in parts by their hash, each joined into its
//! conversations ([`Joined`]); then the index, in parts by message id
//! ([`ThreadIndex::of_joined`]). Where no thread can be started (the browser), it is all done
//! on this one.
//!
//! The check is made at the first ask after a command begins and after it ends. Within one
//! command, an ask, then a change of what groups a message, then another ask would get the first
//! answer: no command does that (each works out the conversation it acts on before it changes
//! anything), and one that would must call [`Session::threads_may_change`] between the two.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, BuildHasherDefault, Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use mailcraft_model::messages::Chunk;
use mailcraft_model::{Id, Joined, Mailbox, Message, ThreadIndex, ThreadKey};

use crate::Session;

/// The conversations kept ([`Session::threads`]).
pub(crate) type ThreadsCache = Mutex<Option<Kept>>;

/// A topic of an account ([`Message::topic_key`]), as a hash of it keyed for this run (subjects
/// are anybody's, so no sender can make them collide on purpose). Two topics may still share
/// one: the messages of a hash are put by their topic itself before they are joined.
type Topic = u64;

/// The hash of the topic `key` of `account` ([`Topic`]).
fn topic_hash(account: Id, key: &str) -> Topic {
    static KEYS: OnceLock<RandomState> = OnceLock::new();
    KEYS.get_or_init(RandomState::new).hash_one((account, key))
}

/// A hasher for what is already a keyed hash ([`Topic`]): it is only spread again, since the
/// parts of the topics each have the same top bits ([`share_of`]).
#[derive(Default)]
struct Hashed(u64);

impl Hasher for Hashed {
    fn finish(&self) -> u64 {
        self.0.wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(*b);
        }
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
}

/// How many parts the topics are kept in, each of those whose hash falls in it: the parts are
/// worked out on threads of their own at the first build.
const TOPIC_SHARES: usize = 16;

/// Which part of the topics `topic` falls in.
fn share_of(topic: Topic) -> usize {
    usize::try_from(topic >> 60).unwrap_or(0) % TOPIC_SHARES
}

/// The ids of the messages of each topic, smallest first, in [`TOPIC_SHARES`] parts.
#[derive(Default)]
struct Topics(Vec<HashMap<Topic, Vec<Id>, BuildHasherDefault<Hashed>>>);

impl Topics {
    fn get(&self, topic: &Topic) -> Option<&Vec<Id>> {
        self.0.get(share_of(*topic)).and_then(|share| share.get(topic))
    }

    /// Take the ids `gone` out of `topic` and put `came` in, keeping its ids in order and each
    /// once; a topic left without one is forgotten.
    fn change(&mut self, topic: Topic, gone: &HashSet<Id>, came: Vec<Id>) {
        if self.0.is_empty() {
            self.0 = (0..TOPIC_SHARES).map(|_| HashMap::default()).collect();
        }
        let Some(share) = self.0.get_mut(share_of(topic)) else { return };
        let ids = share.entry(topic).or_default();
        if !gone.is_empty() {
            ids.retain(|id| !gone.contains(id));
        }
        if !came.is_empty() {
            ids.extend(came);
            ids.sort_unstable();
            ids.dedup();
        }
        if ids.is_empty() {
            share.remove(&topic);
        }
    }
}

pub(crate) struct Kept {
    /// The revision and the count of messages they were last checked at; `None` once the mailbox
    /// may have changed since ([`Session::threads_may_change`]).
    at: Option<(u64, usize)>,
    threads: Arc<ThreadIndex>,
    /// Of each chunk of the map of messages, in the map's order.
    seen: Vec<SeenChunk>,
    /// The ids of the messages of each topic, smallest first: the topics a change touches are
    /// worked out again, and only those (#1897).
    topics: Topics,
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

/// A hasher for [`grouping_hash`], eight bytes at a step: it only tells a message that changed
/// from one that didn't, so it needn't be one a sender can't make collide (the default one isn't
/// keyed there either), and it is asked of every message at the first build.
#[derive(Default)]
struct Changed(u64);

impl Hasher for Changed {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for word in words {
            self.write_u64(u64::from_le_bytes(*word));
        }
        let mut last = [0u8; 8];
        if let Some(to) = last.get_mut(..rest.len()) {
            to.copy_from_slice(rest);
        }
        self.write_u64(u64::from_le_bytes(last) ^ (rest.len() as u64) << 59);
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(26) ^ n).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

/// The hash of what [`ThreadIndex::build`] reads of `m`.
fn grouping_hash(m: &Message) -> u64 {
    let mut h = Changed::default();
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

fn seen(id: Id, m: &Arc<Message>) -> Seen {
    let key = m.topic_key();
    let topic = (!key.is_empty()).then(|| topic_hash(m.account, &key));
    Seen { id, message: Arc::downgrade(m), hash: grouping_hash(m), topic }
}

/// The messages `same`, all of one [`Topic`] and in order of id, put by their topic itself (of
/// which there is one, unless two topics share the hash), each in order of id.
pub(crate) fn by_topic<'a>(same: Vec<(&'a Message, String)>, into: &mut Vec<Vec<&'a Message>>) {
    let Some((first, rest)) = same.split_first() else { return };
    if rest.iter().all(|(m, key)| m.account == first.0.account && *key == first.1) {
        into.push(same.into_iter().map(|(m, _)| m).collect());
        return;
    }
    let mut put: HashMap<(Id, String), Vec<&Message>> = HashMap::new();
    for (m, key) in same {
        put.entry((m.account, key)).or_default().push(m);
    }
    into.extend(put.into_values());
}

/// Most threads the first build goes over the chunks on.
const MOST_THREADS: usize = 16;

/// How many threads to work out `mailbox`'s conversations on: one for a small one.
fn threads_for(mailbox: &Mailbox) -> usize {
    std::thread::available_parallelism().map_or(1, |p| p.get()).min(MOST_THREADS).min(mailbox.messages.chunks().len() / 16).max(1)
}

/// One message as the first build sees it: its topic's hash, itself, and its topic.
type Entry<'a> = (Topic, &'a Message, String);

/// `work` done for each of `items`, each on a thread of its own and the last on this one, in
/// their order. An item whose thread can't be started is worked on this one (it is handed to its
/// thread only once the thread runs, so it comes back when it doesn't); `None` when a thread
/// gave no answer, its item then lost, for the caller to do it all again on one.
pub(crate) fn on_threads<T: Send, R: Send>(items: Vec<T>, work: impl Fn(T) -> R + Sync) -> Option<Vec<R>> {
    let work = &work;
    std::thread::scope(|scope| {
        let mut items = items;
        let last = items.pop();
        let started: Vec<Result<_, T>> = items
            .into_iter()
            .map(|item| {
                let (give, take) = std::sync::mpsc::sync_channel::<T>(1);
                let thread = std::thread::Builder::new().name("conversations".into()).spawn_scoped(scope, move || take.recv().ok().map(work));
                match thread {
                    Ok(thread) => give.send(item).map(|()| thread).map_err(|back| back.0),
                    Err(_) => Err(item),
                }
            })
            .collect();
        let mine = last.map(work);
        // Every thread is joined before the answer is given: one left to the scope would have its
        // panic passed on.
        let mut done = Vec::with_capacity(started.len().saturating_add(1));
        let mut lost = false;
        for started in started {
            match started {
                Ok(thread) => match thread.join().ok().flatten() {
                    Some(r) => done.push(r),
                    None => lost = true,
                },
                Err(item) => done.push(work(item)),
            }
        }
        done.extend(mine);
        (!lost).then_some(done)
    })
}

/// What is seen of the chunks `chunks`, and every message of them that has a topic, put into
/// the parts of the topics ([`TOPIC_SHARES`]) by its topic.
fn seen_chunks<'a>(chunks: &[(&Id, &'a Arc<Chunk>)]) -> (Vec<SeenChunk>, Vec<Vec<Entry<'a>>>) {
    let mut seen = Vec::with_capacity(chunks.len());
    let each = chunks.iter().map(|(_, c)| c.len()).sum::<usize>() / TOPIC_SHARES + 16;
    let mut entries: Vec<Vec<Entry>> = (0..TOPIC_SHARES).map(|_| Vec::with_capacity(each)).collect();
    for (key, chunk) in chunks {
        let mut messages = Vec::with_capacity(chunk.len());
        for (id, m) in chunk.iter() {
            let name = m.topic_key();
            let topic = (!name.is_empty()).then(|| topic_hash(m.account, &name));
            if let Some(topic) = topic
                && let Some(share) = entries.get_mut(share_of(topic))
            {
                share.push((topic, m.as_ref(), name));
            }
            messages.push(Seen { id: *id, message: Arc::downgrade(m), hash: grouping_hash(m), topic });
        }
        seen.push(SeenChunk { key: **key, chunk: Arc::downgrade(chunk), messages });
    }
    (seen, entries)
}

/// The topics of one part, of the messages `parts` (each in order of id, one part after the
/// other), and the conversations of each topic of more than one.
fn topics_of(parts: Vec<Vec<Entry>>) -> (HashMap<Topic, Vec<Id>, BuildHasherDefault<Hashed>>, Joined) {
    let mut entries = Vec::with_capacity(parts.iter().map(Vec::len).sum());
    for part in parts {
        entries.extend(part);
    }
    // By topic; within one, in order of id, as they came (a stable sort).
    entries.sort_by_key(|(topic, _, _)| *topic);
    let mut topics = HashMap::with_capacity_and_hasher(entries.len(), BuildHasherDefault::default());
    let mut lists = Vec::new();
    let mut entries = entries.into_iter().peekable();
    while let Some((topic, m, name)) = entries.next() {
        let mut same = vec![(m, name)];
        while let Some((_, m, name)) = entries.next_if(|(next, _, _)| *next == topic) {
            same.push((m, name));
        }
        topics.insert(topic, same.iter().map(|(m, _)| m.id).collect());
        if same.len() > 1 {
            by_topic(same, &mut lists);
        }
    }
    let joined = Joined::of_topics(lists.iter().map(Vec::as_slice));
    (topics, joined)
}

/// What is kept of `mailbox`, with its conversations worked out over every message.
fn kept_of(mailbox: &Mailbox, at: Option<(u64, usize)>, built: u64) -> Kept {
    kept_on(mailbox, at, built, threads_for(mailbox))
}

/// [`kept_of`], worked out on `threads` threads: the chunks shared out among them, then the
/// parts of the topics.
fn kept_on(mailbox: &Mailbox, at: Option<(u64, usize)>, built: u64, threads: usize) -> Kept {
    let threads = threads.clamp(1, TOPIC_SHARES);
    match worked_on(mailbox, threads) {
        Some((seen, topics, threads)) => Kept { at, threads: Arc::new(threads), seen, topics, rehashed: 0, built: built.saturating_add(1) },
        // A thread gave no answer: all again, on this one (where nothing is left to a thread).
        None => {
            let (seen, topics, threads) = worked_on(mailbox, 1).unwrap_or_default();
            Kept { at, threads: Arc::new(threads), seen, topics, rehashed: 0, built: built.saturating_add(1) }
        }
    }
}

/// What [`kept_on`] keeps, worked out on `threads` threads; `None` when one of them gave no
/// answer.
fn worked_on(mailbox: &Mailbox, threads: usize) -> Option<(Vec<SeenChunk>, Topics, ThreadIndex)> {
    let chunks: Vec<(&Id, &Arc<Chunk>)> = mailbox.messages.chunks().iter().collect();
    let share = chunks.len().div_ceil(threads).max(1);
    let parts = on_threads(chunks.chunks(share).collect(), seen_chunks)?;
    let mut seen = Vec::with_capacity(chunks.len());
    // Of each part of the topics, its messages of each share of the chunks.
    let mut shares: Vec<Vec<Vec<Entry>>> = (0..TOPIC_SHARES).map(|_| Vec::new()).collect();
    for (part, of_it) in parts {
        seen.extend(part);
        for (share, entries) in shares.iter_mut().zip(of_it) {
            share.push(entries);
        }
    }
    // The parts of the topics, a few to each thread.
    let mut each: Vec<Vec<(usize, Vec<Vec<Entry>>)>> = (0..threads).map(|_| Vec::new()).collect();
    for (at, share) in shares.into_iter().enumerate() {
        if let Some(to) = each.get_mut(at % threads) {
            to.push((at, share));
        }
    }
    let worked = on_threads(each, |of_it| of_it.into_iter().map(|(at, share)| (at, topics_of(share))).collect::<Vec<_>>())?;
    let mut topics: Vec<(usize, HashMap<Topic, Vec<Id>, BuildHasherDefault<Hashed>>)> = Vec::with_capacity(TOPIC_SHARES);
    let mut joined = Vec::new();
    for (at, (of_share, of_it)) in worked.into_iter().flatten() {
        topics.push((at, of_share));
        joined.push(of_it);
    }
    topics.sort_by_key(|(at, _)| *at);
    let topics = Topics(topics.into_iter().map(|(_, share)| share).collect());
    Some((seen, topics, ThreadIndex::of_joined(joined, threads)))
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
/// the conversations changed. After `None`, `kept` is dead (its `seen` is taken and left empty,
/// or only part of it is filled): the caller builds afresh and replaces it.
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
                        let one = seen(*id, now);
                        came.extend(one.topic.map(|topic| (*id, topic)));
                        messages.push(one);
                    }
                }
                None => {
                    changed = changed.saturating_add(1);
                    let one = seen(*id, now);
                    came.extend(one.topic.map(|topic| (*id, topic)));
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
    let touched: HashSet<Topic> = gone.iter().chain(&came).map(|(_, topic)| *topic).collect();
    let was: Vec<Id> = touched.iter().filter_map(|topic| kept.topics.get(topic)).flatten().copied().collect();
    // Each topic's ids gone and come, taken out and put in with one pass over its list (one at a
    // time, each would shift the rest of a large topic's list).
    let mut changes: HashMap<Topic, (HashSet<Id>, Vec<Id>)> = HashMap::new();
    for (id, topic) in gone {
        changes.entry(topic).or_default().0.insert(id);
    }
    for (id, topic) in came {
        changes.entry(topic).or_default().1.push(id);
    }
    for (topic, (gone, came)) in changes {
        kept.topics.change(topic, &gone, came);
    }
    let mut topics: Vec<Vec<&Message>> = Vec::new();
    for ids in touched.iter().filter_map(|topic| kept.topics.get(topic)) {
        by_topic(ids.iter().filter_map(|id| mailbox.message(*id)).map(|m| (m, m.topic_key())).collect(), &mut topics);
    }
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

/// The conversations of `after`, worked out on `threads` threads over `before` and then brought
/// in step with `after` (or worked out afresh over it, past [`most_changed`]).
#[cfg(test)]
pub(crate) fn threads_on(before: &Mailbox, after: &Mailbox, threads: usize) -> Arc<ThreadIndex> {
    let mut kept = kept_on(before, None, 0, threads);
    if bring_in_step(&mut kept, after).is_none() {
        kept = kept_on(after, None, 0, threads);
    }
    kept.threads
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
