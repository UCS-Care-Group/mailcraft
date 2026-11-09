// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The messages of a mailbox by id ([`Messages`]), as a map that is cheap to copy and to
//! compare with a copy: the ids are kept in chunks of [`CHUNK`] consecutive ids, each chunk
//! shared between the copies of the map until one of them changes it.
//!
//! Every command that changes the mailbox keeps a copy of it to undo to and compares the two
//! afterwards. With one map of a few hundred thousand messages, the copy, the comparisons and
//! letting go of the copy took 80 to 100 ms at each change (a flag, a move, what a
//! synchronisation brought); with chunks a copy is a few hundred shared chunks, and a
//! comparison looks only into the chunks that aren't shared.

use std::collections::BTreeMap;
use std::collections::btree_map;
use std::ops::{Bound, RangeBounds};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Id, Message};

/// How many consecutive ids a chunk holds, as a power of two.
const SHIFT: u32 = 10;
/// How many consecutive ids a chunk holds.
pub const CHUNK: u64 = 1 << SHIFT;

/// The messages of one chunk of consecutive ids ([`Messages::chunks`]).
pub type Chunk = BTreeMap<Id, Arc<Message>>;

fn chunk_of(id: Id) -> Id {
    id >> SHIFT
}

/// The messages of a mailbox by id: a `BTreeMap<Id, Arc<Message>>` in what it does and how it
/// is written to a file, in chunks shared between copies. No chunk is ever empty, so two maps
/// with the same messages have the same chunks.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Messages {
    chunks: BTreeMap<Id, Arc<Chunk>>,
    len: usize,
    originals: Originals,
}

/// The bytes of originals (#30) the map holds, kept as a running total ([`Messages::originals_size`]):
/// each chunk's share, and the chunks changed since (any way of changing one marks it), whose
/// shares are worked out again when the total is next asked for. A total is thus at most a
/// chunk's work, however many messages the map holds. Not part of what the map is: two maps
/// with the same messages are equal whatever each has worked out.
#[derive(Clone, Default)]
struct Originals {
    per_chunk: BTreeMap<Id, u64>,
    stale: std::collections::BTreeSet<Id>,
    total: u64,
}

impl PartialEq for Originals {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Originals {}

/// A message that differs between two maps ([`Messages::changes`]): as it was, and as it is.
/// `None` for one that isn't there.
pub type Change<'a> = (Id, Option<&'a Arc<Message>>, Option<&'a Arc<Message>>);

impl Messages {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// How many bytes of originals (`Message::original`, #30) the messages hold, in memory or
    /// kept outside it: a running total, brought up to date for the chunks changed since it was
    /// last asked for.
    pub fn originals_size(&mut self) -> u64 {
        let o = &mut self.originals;
        for key in std::mem::take(&mut o.stale) {
            let share = self
                .chunks
                .get(&key)
                .map_or(0, |c| c.values().filter_map(|m| m.original.as_ref()).map(crate::Original::size).fold(0u64, u64::saturating_add));
            let was = if share == 0 { o.per_chunk.remove(&key) } else { o.per_chunk.insert(key, share) };
            o.total = o.total.saturating_sub(was.unwrap_or(0)).saturating_add(share);
        }
        o.total
    }

    /// The chunks, in order: for an index of the messages that follows the map chunk by chunk
    /// (the engine's conversations, the known correspondents of the sender warnings): one that is the same
    /// allocation as before holds the same messages, and no chunk is ever empty.
    pub fn chunks(&self) -> &BTreeMap<Id, Arc<Chunk>> {
        &self.chunks
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, id: &Id) -> Option<&Arc<Message>> {
        self.chunks.get(&chunk_of(*id)).and_then(|c| c.get(id))
    }

    pub fn contains_key(&self, id: &Id) -> bool {
        self.get(id).is_some()
    }

    /// The message `id` to change: its chunk is this map's own from then on (a copy, while
    /// another map shares it).
    pub fn get_mut(&mut self, id: &Id) -> Option<&mut Arc<Message>> {
        let chunk = self.chunks.get_mut(&chunk_of(*id))?;
        if !chunk.contains_key(id) {
            return None;
        }
        self.originals.stale.insert(chunk_of(*id));
        Arc::make_mut(chunk).get_mut(id)
    }

    pub fn insert(&mut self, id: Id, message: Arc<Message>) -> Option<Arc<Message>> {
        self.originals.stale.insert(chunk_of(id));
        let chunk = Arc::make_mut(self.chunks.entry(chunk_of(id)).or_default());
        let was = chunk.insert(id, message);
        if was.is_none() {
            self.len = self.len.saturating_add(1);
        }
        was
    }

    pub fn remove(&mut self, id: &Id) -> Option<Arc<Message>> {
        let key = chunk_of(*id);
        let chunk = self.chunks.get_mut(&key)?;
        if !chunk.contains_key(id) {
            return None;
        }
        self.originals.stale.insert(key);
        let shared = Arc::make_mut(chunk);
        let was = shared.remove(id);
        if shared.is_empty() {
            self.chunks.remove(&key);
        }
        if was.is_some() {
            self.len = self.len.saturating_sub(1);
        }
        was
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.len = 0;
        self.originals = Originals::default();
    }

    pub fn iter(&self) -> Iter<'_> {
        self.into_iter()
    }

    pub fn keys(&self) -> Keys<'_> {
        fn each(c: &Arc<Chunk>) -> btree_map::Keys<'_, Id, Arc<Message>> {
            c.keys()
        }
        self.chunks.values().flat_map(each)
    }

    pub fn values(&self) -> Values<'_> {
        fn each(c: &Arc<Chunk>) -> btree_map::Values<'_, Id, Arc<Message>> {
            c.values()
        }
        self.chunks.values().flat_map(each)
    }

    /// Every message, to change: every chunk becomes this map's own (a copy of those shared).
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Arc<Message>> {
        self.originals.stale.extend(self.chunks.keys().copied());
        self.chunks.values_mut().flat_map(|c| Arc::make_mut(c).values_mut())
    }

    /// [`Messages::values_mut`] with the ids.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&Id, &mut Arc<Message>)> {
        self.originals.stale.extend(self.chunks.keys().copied());
        self.chunks.values_mut().flat_map(|c| Arc::make_mut(c).iter_mut())
    }

    /// Keeps the messages `keep` says yes to. Only the chunks that lose one become this map's
    /// own.
    pub fn retain(&mut self, mut keep: impl FnMut(&Id, &Arc<Message>) -> bool) {
        let mut emptied = Vec::new();
        for (key, chunk) in &mut self.chunks {
            let gone: Vec<Id> = chunk.iter().filter(|(id, m)| !keep(id, m)).map(|(id, _)| *id).collect();
            if gone.is_empty() {
                continue;
            }
            self.originals.stale.insert(*key);
            let own = Arc::make_mut(chunk);
            for id in &gone {
                own.remove(id);
            }
            self.len = self.len.saturating_sub(gone.len());
            if own.is_empty() {
                emptied.push(*key);
            }
        }
        for key in emptied {
            self.chunks.remove(&key);
        }
    }

    /// The messages whose ids are in `range`, in order of id.
    pub fn range<R: RangeBounds<Id> + Clone>(&self, range: R) -> impl DoubleEndedIterator<Item = (&Id, &Arc<Message>)> {
        let low = match range.start_bound() {
            Bound::Included(id) | Bound::Excluded(id) => Bound::Included(chunk_of(*id)),
            Bound::Unbounded => Bound::Unbounded,
        };
        let high = match range.end_bound() {
            Bound::Included(id) | Bound::Excluded(id) => Bound::Included(chunk_of(*id)),
            Bound::Unbounded => Bound::Unbounded,
        };
        // (An empty range of chunks for bounds the wrong way round, as `BTreeMap::range` would
        // panic on them.)
        let ordered = match (low, high) {
            (Bound::Included(a), Bound::Included(b)) => a <= b,
            _ => true,
        };
        let chunks = if ordered { Some(self.chunks.range((low, high))) } else { None };
        chunks.into_iter().flatten().flat_map(move |(_, c)| in_range(c, range.clone()))
    }

    /// The messages of `ids`, which are in order of id (a folder's list), as [`Messages::get`]
    /// finds each, in that order: one walk through the chunks and through each of them, rather
    /// than a search from the top for each one. Ids out of order are looked up one by one.
    pub fn get_sorted<'a, 'b>(&'a self, ids: &'b [Id]) -> impl Iterator<Item = &'a Arc<Message>> + use<'a, 'b> {
        let mut chunk: Option<(Id, std::iter::Peekable<btree_map::Iter<'a, Id, Arc<Message>>>)> = None;
        let mut last: Option<Id> = None;
        ids.iter().filter_map(move |id| {
            if last.is_some_and(|l| l >= *id) {
                last = Some(*id);
                chunk = None;
                return self.get(id);
            }
            last = Some(*id);
            let key = chunk_of(*id);
            if chunk.as_ref().is_none_or(|(k, _)| *k != key) {
                chunk = self.chunks.get(&key).map(|c| (key, c.iter().peekable()));
            }
            let (_, it) = chunk.as_mut()?;
            while it.next_if(|(x, _)| *x < id).is_some() {}
            it.next_if(|(x, _)| *x == id).map(|(_, m)| m)
        })
    }

    pub fn first_key_value(&self) -> Option<(&Id, &Arc<Message>)> {
        self.chunks.values().next().and_then(|c| c.first_key_value())
    }

    pub fn last_key_value(&self) -> Option<(&Id, &Arc<Message>)> {
        self.chunks.values().next_back().and_then(|c| c.last_key_value())
    }

    pub fn pop_first(&mut self) -> Option<(Id, Arc<Message>)> {
        let id = *self.first_key_value()?.0;
        self.remove(&id).map(|m| (id, m))
    }

    /// The messages that differ between `before` and this map, in order of id: in only one of
    /// them, or not the same allocation in both (which may hold the same message: a caller
    /// that cares compares them). The chunks the two share aren't looked into.
    pub fn changes<'a>(&'a self, before: &'a Messages) -> Vec<Change<'a>> {
        let mut out = Vec::new();
        let mut was = before.chunks.iter().peekable();
        let mut is = self.chunks.iter().peekable();
        loop {
            match (was.peek(), is.peek()) {
                (None, None) => break,
                (Some((a, then)), Some((b, now))) if a == b => {
                    if !Arc::ptr_eq(then, now) {
                        diff_chunks(then, now, &mut out);
                    }
                    was.next();
                    is.next();
                }
                (Some((a, then)), Some((b, _))) if a < b => {
                    out.extend(then.iter().map(|(id, m)| (*id, Some(m), None)));
                    was.next();
                }
                (Some((_, then)), None) => {
                    out.extend(then.iter().map(|(id, m)| (*id, Some(m), None)));
                    was.next();
                }
                (_, Some((_, now))) => {
                    out.extend(now.iter().map(|(id, m)| (*id, None, Some(m))));
                    is.next();
                }
            }
        }
        out
    }
}

/// The messages of `chunk` in `range`: none for bounds the wrong way round, as
/// `BTreeMap::range` would panic on them.
fn in_range<R: RangeBounds<Id>>(chunk: &Chunk, range: R) -> impl DoubleEndedIterator<Item = (&Id, &Arc<Message>)> {
    let ordered = match (range.start_bound(), range.end_bound()) {
        (Bound::Included(a), Bound::Included(b)) => a <= b,
        (Bound::Included(a) | Bound::Excluded(a), Bound::Excluded(b)) | (Bound::Excluded(a), Bound::Included(b)) => a < b,
        _ => true,
    };
    ordered.then(|| chunk.range(range)).into_iter().flatten()
}

/// The messages that differ between two chunks of the same ids, in order of id.
fn diff_chunks<'a>(then: &'a Chunk, now: &'a Chunk, out: &mut Vec<Change<'a>>) {
    let mut was = then.iter().peekable();
    let mut is = now.iter().peekable();
    loop {
        match (was.peek(), is.peek()) {
            (None, None) => break,
            (Some((a, x)), Some((b, y))) if a == b => {
                if !Arc::ptr_eq(x, y) {
                    out.push((**a, Some(*x), Some(*y)));
                }
                was.next();
                is.next();
            }
            (Some((a, x)), Some((b, _))) if a < b => {
                out.push((**a, Some(*x), None));
                was.next();
            }
            (Some((a, x)), None) => {
                out.push((**a, Some(*x), None));
                was.next();
            }
            (_, Some((b, y))) => {
                out.push((**b, None, Some(*y)));
                is.next();
            }
        }
    }
}

impl std::fmt::Debug for Messages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl FromIterator<(Id, Arc<Message>)> for Messages {
    fn from_iter<I: IntoIterator<Item = (Id, Arc<Message>)>>(iter: I) -> Self {
        let mut out = Messages::new();
        out.extend(iter);
        out
    }
}

impl Extend<(Id, Arc<Message>)> for Messages {
    fn extend<I: IntoIterator<Item = (Id, Arc<Message>)>>(&mut self, iter: I) {
        for (id, m) in iter {
            self.insert(id, m);
        }
    }
}

impl From<BTreeMap<Id, Arc<Message>>> for Messages {
    fn from(map: BTreeMap<Id, Arc<Message>>) -> Self {
        map.into_iter().collect()
    }
}

/// The messages of a map in order of id, with their ids ([`Messages::iter`]).
pub type Iter<'a> = std::iter::FlatMap<
    btree_map::Values<'a, Id, Arc<Chunk>>,
    btree_map::Iter<'a, Id, Arc<Message>>,
    fn(&'a Arc<Chunk>) -> btree_map::Iter<'a, Id, Arc<Message>>,
>;
/// The ids of a map in order ([`Messages::keys`]).
pub type Keys<'a> = std::iter::FlatMap<
    btree_map::Values<'a, Id, Arc<Chunk>>,
    btree_map::Keys<'a, Id, Arc<Message>>,
    fn(&'a Arc<Chunk>) -> btree_map::Keys<'a, Id, Arc<Message>>,
>;
/// The messages of a map in order of id ([`Messages::values`]).
pub type Values<'a> = std::iter::FlatMap<
    btree_map::Values<'a, Id, Arc<Chunk>>,
    btree_map::Values<'a, Id, Arc<Message>>,
    fn(&'a Arc<Chunk>) -> btree_map::Values<'a, Id, Arc<Message>>,
>;

impl<'a> IntoIterator for &'a Messages {
    type Item = (&'a Id, &'a Arc<Message>);
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        fn each(c: &Arc<Chunk>) -> btree_map::Iter<'_, Id, Arc<Message>> {
            c.iter()
        }
        self.chunks.values().flat_map(each)
    }
}

impl IntoIterator for Messages {
    type Item = (Id, Arc<Message>);
    type IntoIter = std::iter::FlatMap<
        btree_map::IntoValues<Id, Arc<Chunk>>,
        btree_map::IntoIter<Id, Arc<Message>>,
        fn(Arc<Chunk>) -> btree_map::IntoIter<Id, Arc<Message>>,
    >;
    fn into_iter(self) -> Self::IntoIter {
        fn each(c: Arc<Chunk>) -> btree_map::IntoIter<Id, Arc<Message>> {
            Arc::unwrap_or_clone(c).into_iter()
        }
        self.chunks.into_values().flat_map(each as fn(Arc<Chunk>) -> btree_map::IntoIter<Id, Arc<Message>>)
    }
}

impl Serialize for Messages {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.iter())
    }
}

impl<'de> Deserialize<'de> for Messages {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        BTreeMap::<Id, Arc<Message>>::deserialize(deserializer).map(Messages::from)
    }
}
