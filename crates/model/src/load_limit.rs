// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A bound on what reading a mailbox file may cost, enforced while it is read.
//!
//! In a JSON mailbox an item can be as short as `"1":{}` and still become a message of about a
//! kilobyte in memory, and a list element as short as `{}` an attachment of a few hundred bytes, so
//! the size of the file says little about what reading it costs. The maps and lists of a
//! [`Mailbox`] count their items as they are read, and every list element, map entry and compound
//! value at any depth is charged what it takes in memory ([`Counting`]), against one [`Budget`];
//! the read fails, having built no more than the budget, when the file holds more. Every form of
//! mailbox file is read this way: one file read whole or as a stream, the records of the form with
//! a log of records, and a mailbox handed over as text.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::de::{Deserialize, DeserializeOwned, DeserializeSeed, Deserializer, Error, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::{Id, Mailbox};

mod counting;

pub use counting::Counting;

/// What reading a mailbox from JSON costs per item, at the least, in memory (bytes).
pub const MIN_ITEM_BYTES: u64 = 1024;

/// The bytes of parsed values [`from_json`] allows: 1 GiB.
pub const DEFAULT_BYTES: u64 = 1 << 30;

/// Why a read stopped for too many items.
pub const TOO_MANY_ITEMS: &str = "it holds more items than this computer has the memory to open";

/// Why a read stopped for values that would cost too much memory.
pub const TOO_MUCH: &str = "it holds more than this computer has the memory to open";

/// What a mailbox file may cost while it is read: a number of items (messages, contacts, events,
/// tasks, accounts, folders, categories and the entries of a few lists, each at least
/// [`MIN_ITEM_BYTES`]) and bytes of parsed values. One budget may be shared by the threads that
/// read the parts of one file.
#[derive(Debug)]
pub struct Budget {
    /// Items and bytes left. (Threads take bytes a chunk at a time, so they seldom meet here.)
    left: Mutex<(usize, u64)>,
    /// A charge was refused: 0 none, [`ITEMS`] an item, [`BYTES`] bytes.
    spent: AtomicU8,
}

impl Budget {
    /// A budget of `items` items and `bytes` bytes of parsed values.
    pub fn new(items: usize, bytes: u64) -> Arc<Budget> {
        Arc::new(Budget { left: Mutex::new((items, bytes)), spent: AtomicU8::new(0) })
    }

    /// Take one item; `false` (and nothing taken) when none is left.
    pub fn take_item(&self) -> bool {
        let mut left = self.left();
        let taken = match left.0.checked_sub(1) {
            Some(n) => {
                left.0 = n;
                true
            }
            None => false,
        };
        if !taken {
            self.spent.store(ITEMS, Ordering::Relaxed);
        }
        taken
    }

    /// Take `n` bytes; `false` (and nothing taken) when fewer are left.
    pub fn take(&self, n: u64) -> bool {
        let taken = self.try_take(n);
        if !taken {
            self.spent.store(BYTES, Ordering::Relaxed);
        }
        taken
    }

    fn try_take(&self, n: u64) -> bool {
        let mut left = self.left();
        match left.1.checked_sub(n) {
            Some(rest) => {
                left.1 = rest;
                true
            }
            None => false,
        }
    }

    fn left(&self) -> MutexGuard<'_, (usize, u64)> {
        // (Nothing is ever left half-changed under the lock.)
        self.left.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Bytes not yet taken.
    pub fn bytes_left(&self) -> u64 {
        self.left().1
    }

    /// Why a charge was refused ([`TOO_MANY_ITEMS`] or [`TOO_MUCH`]), when one was: what was being
    /// read holds more than the budget.
    pub fn spent(&self) -> Option<&'static str> {
        match self.spent.load(Ordering::Relaxed) {
            ITEMS => Some(TOO_MANY_ITEMS),
            BYTES => Some(TOO_MUCH),
            _ => None,
        }
    }

    /// Give back `n` bytes taken and let go of (or not used).
    pub fn give_back(&self, n: u64) {
        if n > 0 {
            let mut left = self.left();
            left.1 = left.1.saturating_add(n);
        }
    }
}

/// [`Budget::spent`]: an item was refused.
const ITEMS: u8 = 1;
/// [`Budget::spent`]: bytes were refused.
const BYTES: u8 = 2;

thread_local! {
    /// The budget what is read on this thread is charged to; `None`: none (a mailbox that doesn't
    /// come from a file).
    static POOL: RefCell<Option<Arc<Budget>>> = const { RefCell::new(None) };
    /// Bytes taken from the budget ahead and not yet charged, so that threads sharing one budget
    /// don't meet at every value.
    static CREDIT: Cell<u64> = const { Cell::new(0) };
}

/// Bytes taken from a shared budget at a time.
const CHUNK: u64 = 256 << 10;

/// Run `f` with what it reads on this thread charged to `budget` (a budget in force before is set
/// back after).
pub fn within<T>(budget: &Arc<Budget>, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Arc<Budget>>, u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            let credit = CREDIT.with(|c| c.replace(self.1));
            if let Some(mine) = POOL.with(|p| p.replace(self.0.take())) {
                mine.give_back(credit);
            }
        }
    }
    let before = POOL.with(|p| p.replace(Some(budget.clone())));
    let _restore = Restore(before, CREDIT.with(|c| c.replace(0)));
    f()
}

/// Take one item from the budget in force.
fn charge<E: Error>() -> Result<(), E> {
    POOL.with(|p| match p.borrow().as_deref() {
        None => Ok(()),
        Some(budget) if budget.take_item() => Ok(()),
        Some(_) => Err(E::custom(TOO_MANY_ITEMS)),
    })
}

/// Charge `n` bytes of parsed values to the budget in force.
fn charge_bytes<E: Error>(n: u64) -> Result<(), E> {
    let credit = CREDIT.with(Cell::get);
    if let Some(left) = credit.checked_sub(n) {
        CREDIT.with(|c| c.set(left));
        return Ok(());
    }
    POOL.with(|p| {
        let pool = p.borrow();
        let Some(budget) = pool.as_deref() else { return Ok(()) };
        // What is wanted past the credit, and a chunk more ahead when there is room for it.
        let want = n.saturating_sub(credit);
        let ahead = want.saturating_add(CHUNK);
        if budget.try_take(ahead) {
            CREDIT.with(|c| c.set(CHUNK));
            Ok(())
        } else if budget.take(want) {
            CREDIT.with(|c| c.set(0));
            Ok(())
        } else {
            Err(E::custom(TOO_MUCH))
        }
    })
}

/// Read a mailbox from JSON holding at most `max_items` items (messages, contacts, events, tasks,
/// accounts, folders and categories together) and [`DEFAULT_BYTES`] of parsed values. A file with
/// more is an error.
pub fn from_json(bytes: &[u8], max_items: usize) -> Result<Mailbox, serde_json::Error> {
    read_json(bytes, &Budget::new(max_items, DEFAULT_BYTES))
}

/// Read a `T` (a mailbox, or one item of a mailbox file) from JSON within `budget`: past it, an
/// error, having built no more than the budget allows.
pub fn read_json<T: DeserializeOwned>(bytes: &[u8], budget: &Arc<Budget>) -> Result<T, serde_json::Error> {
    within(budget, || {
        let mut de = serde_json::Deserializer::from_slice(bytes);
        let value = T::deserialize(Counting(&mut de))?;
        de.end()?;
        // A charge refused inside a value read leniently (one that leaves out what it can't
        // read) is still a refusal: nothing is left out of a mailbox for want of memory, to be
        // saved without it.
        refused_within(budget)?;
        Ok(value)
    })
}

/// An error when a charge to `budget` was refused, even where the reader went on.
pub fn refused_within(budget: &Budget) -> Result<(), serde_json::Error> {
    match budget.spent() {
        Some(why) => Err(serde_json::Error::custom(why)),
        None => Ok(()),
    }
}

/// A `T` read with what it holds charged to the budget in force ([`within`]), for a reader that
/// takes values one at a time (`MapAccess::next_value_seed`).
pub struct Counted<T>(PhantomData<T>);

impl<T> Default for Counted<T> {
    fn default() -> Self {
        Counted(PhantomData)
    }
}

impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for Counted<T> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<T, D::Error> {
        T::deserialize(Counting(deserializer))
    }
}

/// A `T` read from a JSON value already held, charged as if it were read from the file.
pub(crate) fn from_value<T: DeserializeOwned>(value: &Value) -> Result<T, serde_json::Error> {
    T::deserialize(Counting(value))
}

/// `deserialize_with` for a list.
pub(crate) fn capped_vec<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Vec<T>, D::Error> {
    struct V<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a list")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut out = Vec::new();
            while let Some(item) = seq.next_element()? {
                charge()?;
                out.push(item);
            }
            Ok(out)
        }
    }
    deserializer.deserialize_seq(V(PhantomData))
}

/// `deserialize_with` for the messages of a mailbox ([`crate::Messages`]).
pub(crate) fn capped_messages<'de, D: Deserializer<'de>>(deserializer: D) -> Result<crate::Messages, D::Error> {
    capped_map::<D, std::sync::Arc<crate::Message>>(deserializer).map(crate::Messages::read)
}

/// `deserialize_with` for a list of ids a file may have got wrong (the owner's order of the
/// accounts, #702): every entry counts as it is read; an entry that isn't an id, and a value that
/// isn't a list, is passed over without being held, and never costs the owner the whole file.
pub(crate) fn capped_ids<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Id>, D::Error> {
    /// One entry: an id, or `None` for anything else (read past, not kept).
    struct Entry(Option<Id>);
    impl<'de> Deserialize<'de> for Entry {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_any(Ids(false)).map(|ids| Entry(ids.first().copied()))
        }
    }
    /// `true`: the list itself; `false`: one of its entries.
    struct Ids(bool);
    impl<'de> Visitor<'de> for Ids {
        type Value = Vec<Id>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a list of ids")
        }
        fn visit_u64<E: Error>(self, n: u64) -> Result<Vec<Id>, E> {
            Ok(if self.0 { Vec::new() } else { vec![n] })
        }
        fn visit_i64<E: Error>(self, n: i64) -> Result<Vec<Id>, E> {
            Ok(if self.0 { Vec::new() } else { u64::try_from(n).ok().into_iter().collect() })
        }
        fn visit_f64<E: Error>(self, _: f64) -> Result<Vec<Id>, E> {
            Ok(Vec::new())
        }
        fn visit_bool<E: Error>(self, _: bool) -> Result<Vec<Id>, E> {
            Ok(Vec::new())
        }
        fn visit_str<E: Error>(self, _: &str) -> Result<Vec<Id>, E> {
            Ok(Vec::new())
        }
        fn visit_unit<E: Error>(self) -> Result<Vec<Id>, E> {
            Ok(Vec::new())
        }
        fn visit_none<E: Error>(self) -> Result<Vec<Id>, E> {
            Ok(Vec::new())
        }
        fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Vec<Id>, D::Error> {
            deserializer.deserialize_any(self)
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Vec<Id>, A::Error> {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {
                charge()?;
            }
            Ok(Vec::new())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<Id>, A::Error> {
            let mut out = Vec::new();
            if self.0 {
                while let Some(Entry(id)) = seq.next_element::<Entry>()? {
                    charge()?;
                    out.extend(id);
                }
            } else {
                while seq.next_element::<IgnoredAny>()?.is_some() {
                    charge()?;
                }
            }
            Ok(out)
        }
    }
    deserializer.deserialize_any(Ids(true))
}

/// `deserialize_with` for lists by id: the items of each list count as well as the lists, and what
/// isn't such a list or such an item is left out, which never costs the owner the whole file.
pub(crate) fn capped_lists<'de, D: Deserializer<'de>, T: DeserializeOwned>(deserializer: D) -> Result<BTreeMap<Id, Vec<T>>, D::Error> {
    let mut out = BTreeMap::new();
    let Value::Object(lists) = Value::deserialize(deserializer)? else { return Ok(out) };
    for (id, list) in lists {
        charge()?;
        let (Ok(id), Value::Array(items)) = (id.parse::<Id>(), list) else { continue };
        let mut kept = Vec::new();
        for item in items {
            charge()?;
            kept.extend(from_value::<T>(&item).ok());
        }
        out.insert(id, kept);
    }
    Ok(out)
}

/// `deserialize_with` for a map by id.
pub(crate) fn capped_map<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<BTreeMap<Id, T>, D::Error> {
    struct V<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
        type Value = BTreeMap<Id, T>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a map")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<BTreeMap<Id, T>, A::Error> {
            let mut out = BTreeMap::new();
            while let Some((id, item)) = map.next_entry::<Id, T>()? {
                charge()?;
                out.insert(id, item);
            }
            Ok(out)
        }
    }
    deserializer.deserialize_map(V(PhantomData))
}
