// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A bound on how many items a mailbox file may hold, enforced while it is read.
//!
//! In a JSON mailbox an item can be as short as `"1":{}` and still become a message of about a
//! kilobyte in memory, so the size of the file says little about what reading it costs. The maps and
//! lists of a [`Mailbox`] count their items as they are read against a budget set by [`from_json`],
//! and the read fails, having built no more than the budget, when the file holds more.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::marker::PhantomData;

use serde::de::{Deserialize, DeserializeOwned, Deserializer, Error, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::{Id, Mailbox};

thread_local! {
    /// Items that may still be read; `None`: no budget (a mailbox that doesn't come from a file).
    static LEFT: Cell<Option<usize>> = const { Cell::new(None) };
}

/// What reading a mailbox from JSON costs per item, at the least, in memory (bytes).
pub const MIN_ITEM_BYTES: u64 = 1024;

/// Read a mailbox from JSON holding at most `max_items` items (messages, contacts, events, tasks,
/// accounts, folders and categories together). A file with more is an error.
pub fn from_json(bytes: &[u8], max_items: usize) -> Result<Mailbox, serde_json::Error> {
    struct Reset(Option<usize>);
    impl Drop for Reset {
        fn drop(&mut self) {
            LEFT.with(|left| left.set(self.0));
        }
    }
    let _reset = Reset(LEFT.with(|left| left.replace(Some(max_items))));
    serde_json::from_slice(bytes)
}

/// Take one item from the budget.
fn charge<E: Error>() -> Result<(), E> {
    LEFT.with(|left| match left.get() {
        None => Ok(()),
        Some(0) => Err(E::custom("it holds more items than this computer has the memory to open")),
        Some(n) => {
            left.set(Some(n - 1));
            Ok(())
        }
    })
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
            kept.extend(T::deserialize(item).ok());
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
