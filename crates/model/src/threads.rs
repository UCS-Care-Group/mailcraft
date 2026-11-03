// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conversations of a mailbox by their topic ([`Message::thread_key`]), so that the reading
//! pane's conversation ([`Mailbox::conversation`]) is looked up rather than found by a walk over
//! every message of the account, which took 70 ms at each click in a mailbox of 326,000.
//!
//! Brought in step with the mailbox by [`Threads::update`], chunk by chunk of the map of
//! messages ([`Messages`]): a chunk that is the one it was last time isn't looked into, and the
//! messages of one that changed are filed again. Each chunk is remembered weakly, so that its
//! place can't be taken by another while the index remembers it, and nothing of its messages is
//! kept alive by the index (a body let go of after a search goes).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::sync::{Arc, Weak};

use crate::messages::Chunk;
use crate::{Id, Mailbox, Message};

/// What the index knows of one chunk of the map: the chunk as it was seen, and the key of each
/// of its messages' conversation (`None` for one with no topic, a conversation of its own), in
/// order of id.
struct Seen {
    chunk: Weak<Chunk>,
    keys: Vec<(Id, Option<u64>)>,
}

/// The messages of every conversation, by a hash of their account and topic; a hash that two
/// conversations share is told apart when looked up.
#[derive(Default)]
pub struct Threads {
    seen: BTreeMap<Id, Seen>,
    by_key: HashMap<u64, Vec<Id>>,
    hasher: RandomState,
    /// The ids looked at in taking messages out of conversations, so that a test can tell the
    /// cost stays in proportion to what went.
    #[cfg(test)]
    pub(crate) work: usize,
}

/// The ids to take out of each conversation, gathered over one update and taken out with one
/// pass over the conversation: taken out one at a time, a conversation of `n` (a feed under one
/// subject, which a sender can grow as large as they like) cost `n × n`, 2.7 s for 80,000.
type Unfiled = HashMap<u64, Vec<Id>>;

impl std::fmt::Debug for Threads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Threads").field("chunks", &self.seen.len()).field("conversations", &self.by_key.len()).finish()
    }
}

impl Threads {
    fn unfile(unfiled: &mut Unfiled, key: Option<u64>, id: Id) {
        if let Some(key) = key {
            unfiled.entry(key).or_default().push(id);
        }
    }

    fn file(by_key: &mut HashMap<u64, Vec<Id>>, key: Option<u64>, id: Id) {
        if let Some(key) = key {
            by_key.entry(key).or_default().push(id);
        }
    }

    /// Takes the ids gathered out of their conversations, one pass over each. An id is never
    /// both taken out of and filed under one key in an update (it is filed again only under
    /// another key, and the ids of a chunk are of that chunk alone), so it doesn't matter that
    /// this comes after the filing.
    fn take_out(&mut self, unfiled: Unfiled) {
        for (key, gone) in unfiled {
            let Some(ids) = self.by_key.get_mut(&key) else { continue };
            #[cfg(test)]
            {
                self.work = self.work.saturating_add(ids.len());
            }
            if gone.len() <= 8 {
                ids.retain(|x| !gone.contains(x));
            } else {
                let gone: HashSet<Id> = gone.into_iter().collect();
                ids.retain(|x| !gone.contains(x));
            }
            if ids.is_empty() {
                self.by_key.remove(&key);
            }
        }
    }

    /// Brings the index in step with `mb`: the chunks of its messages that changed, came or
    /// went since the last call are filed again; the rest aren't looked at. The first call files
    /// every message.
    pub fn update(&mut self, mb: &Mailbox) {
        self.update_some(mb, usize::MAX);
    }

    /// [`Threads::update`], filing again at most `most` chunks of messages (about a millisecond
    /// for two of them), so that a window can build the index a few chunks a frame rather than
    /// in one go at the first click. Whether the index is in step with `mb` now.
    pub fn update_some(&mut self, mb: &Mailbox, most: usize) -> bool {
        let chunks = mb.messages.chunks();
        let mut unfiled = Unfiled::new();
        // Chunks gone since.
        let gone: Vec<Id> = self.seen.keys().filter(|k| !chunks.contains_key(k)).copied().collect();
        for k in gone {
            if let Some(seen) = self.seen.remove(&k) {
                for (id, key) in seen.keys {
                    Self::unfile(&mut unfiled, key, id);
                }
            }
        }
        let mut filed = 0usize;
        for (k, chunk) in chunks {
            if self.seen.get(k).is_some_and(|s| std::ptr::eq(s.chunk.as_ptr(), Arc::as_ptr(chunk))) {
                continue;
            }
            if filed >= most {
                self.take_out(unfiled);
                return false;
            }
            filed = filed.saturating_add(1);
            let keys: Vec<(Id, Option<u64>)> = chunk.iter().map(|(id, m)| (*id, key_of(&self.hasher, m))).collect();
            let was = self.seen.remove(k).map(|s| s.keys).unwrap_or_default();
            // Both in order of id: what each message was filed under and is to be now.
            let mut old = was.into_iter().peekable();
            for (id, key) in &keys {
                while let Some((gone, at)) = old.next_if(|(x, _)| x < id) {
                    Self::unfile(&mut unfiled, at, gone);
                }
                match old.next_if(|(x, _)| x == id) {
                    Some((_, at)) if at == *key => {}
                    Some((_, at)) => {
                        Self::unfile(&mut unfiled, at, *id);
                        Self::file(&mut self.by_key, *key, *id);
                    }
                    None => Self::file(&mut self.by_key, *key, *id),
                }
            }
            for (gone, at) in old {
                Self::unfile(&mut unfiled, at, gone);
            }
            self.seen.insert(*k, Seen { chunk: Arc::downgrade(chunk), keys });
        }
        self.take_out(unfiled);
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
