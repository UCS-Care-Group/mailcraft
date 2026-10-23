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

use serde::de::{Deserialize, Deserializer, Error, MapAccess, SeqAccess, Visitor};

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
