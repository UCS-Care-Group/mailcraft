// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The chunked map of messages ([`Messages`]) does what a `BTreeMap` does, through any run of
//! changes, with copies sharing what neither changed; and [`Messages::changes`] finds exactly
//! the messages two maps differ in.

use std::collections::BTreeMap;
use std::sync::Arc;

use proptest::prelude::*;

use crate::messages::CHUNK;
use crate::{Id, Message, Messages};

fn message(id: Id, subject: &str) -> Arc<Message> {
    Arc::new(Message { id, subject: subject.into(), ..Default::default() })
}

#[derive(Clone, Debug)]
enum Op {
    Insert(Id, u8),
    Remove(Id),
    Change(Id, u8),
    Retain(u8),
    PopFirst,
    Copy,
    Clear,
}

fn id() -> impl Strategy<Value = Id> {
    prop_oneof![0..40u64, (0..8u64).prop_map(|c| c * CHUNK + 3), (CHUNK - 2..CHUNK + 2), Just(u64::MAX), Just(u64::MAX - 1)]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (id(), any::<u8>()).prop_map(|(i, s)| Op::Insert(i, s)),
        2 => id().prop_map(Op::Remove),
        2 => (id(), any::<u8>()).prop_map(|(i, s)| Op::Change(i, s)),
        1 => any::<u8>().prop_map(Op::Retain),
        1 => Just(Op::PopFirst),
        1 => Just(Op::Copy),
        1 => Just(Op::Clear),
    ]
}

/// The two say the same of everything they hold.
fn same(map: &Messages, model: &BTreeMap<Id, Arc<Message>>) {
    assert_eq!(map.len(), model.len());
    assert_eq!(map.is_empty(), model.is_empty());
    assert!(map.iter().map(|(k, v)| (*k, v.clone())).eq(model.iter().map(|(k, v)| (*k, v.clone()))));
    assert!(map.keys().rev().eq(model.keys().rev()));
    assert!(map.values().eq(model.values()));
    assert_eq!(map.first_key_value(), model.first_key_value());
    assert_eq!(map.last_key_value(), model.last_key_value());
    for probe in [0, 1, 5, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK + 3, u64::MAX] {
        assert_eq!(map.get(&probe), model.get(&probe));
        assert_eq!(map.contains_key(&probe), model.contains_key(&probe));
        let ours: Vec<Id> = map.range(probe..).map(|(k, _)| *k).collect();
        let theirs: Vec<Id> = model.range(probe..).map(|(k, _)| *k).collect();
        assert_eq!(ours, theirs, "range from {probe}");
        let ours: Vec<Id> = map.range(..=probe).rev().map(|(k, _)| *k).collect();
        let theirs: Vec<Id> = model.range(..=probe).rev().map(|(k, _)| *k).collect();
        assert_eq!(ours, theirs, "range to {probe}");
    }
    assert_eq!(*map, model.clone().into(), "equal to the map built from the same messages");
}

/// What [`Messages::changes`] finds, worked out from the two `BTreeMap`s.
fn changes_of(before: &BTreeMap<Id, Arc<Message>>, after: &BTreeMap<Id, Arc<Message>>) -> Vec<(Id, bool, bool)> {
    let mut ids: Vec<Id> = before.keys().chain(after.keys()).copied().collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter(|id| before.get(id).map(Arc::as_ptr) != after.get(id).map(Arc::as_ptr))
        .map(|id| (id, before.contains_key(&id), after.contains_key(&id)))
        .collect()
}

proptest! {
    #[test]
    fn the_chunked_map_does_what_a_btreemap_does(ops in proptest::collection::vec(op(), 0..80)) {
        let mut map = Messages::new();
        let mut model = BTreeMap::new();
        let mut copies: Vec<(Messages, BTreeMap<Id, Arc<Message>>)> = Vec::new();
        for op in ops {
            match op {
                Op::Insert(id, s) => {
                    let m = message(id, &s.to_string());
                    assert_eq!(map.insert(id, m.clone()).is_some(), model.insert(id, m).is_some());
                }
                Op::Remove(id) => assert_eq!(map.remove(&id), model.remove(&id)),
                Op::Change(id, s) => {
                    let ours = map.get_mut(&id).map(|m| Arc::make_mut(m).subject = format!("changed {s}"));
                    let theirs = model.get_mut(&id).map(|m| Arc::make_mut(m).subject = format!("changed {s}"));
                    assert_eq!(ours.is_some(), theirs.is_some());
                }
                Op::Retain(n) => {
                    let keep = |id: &Id| !id.is_multiple_of(u64::from(n.max(1)));
                    map.retain(|id, _| keep(id));
                    model.retain(|id, _| keep(id));
                }
                Op::PopFirst => assert_eq!(map.pop_first(), model.pop_first()),
                Op::Copy => copies.push((map.clone(), model.clone())),
                Op::Clear => {
                    map.clear();
                    model.clear();
                }
            }
            same(&map, &model);
        }
        // Each copy is as it was when it was taken, whatever happened to the map since, and the
        // changes from it are what differs.
        for (copy, then) in &copies {
            same(copy, then);
            let found: Vec<(Id, bool, bool)> = map.changes(copy).iter().map(|(id, a, b)| (*id, a.is_some(), b.is_some())).collect();
            prop_assert_eq!(found, changes_of(then, &model));
            prop_assert_eq!(map == *copy, *then == model);
        }
    }
}

/// A copy shares every chunk; a change makes only its own chunk the changed map's.
#[test]
fn a_copy_shares_what_neither_changed() {
    let mut map: Messages = (1..=5 * CHUNK).map(|id| (id, message(id, "x"))).collect();
    let copy = map.clone();
    assert!(map.changes(&copy).is_empty());
    if let Some(m) = map.get_mut(&7) {
        Arc::make_mut(m).read = true;
    }
    let changes = map.changes(&copy);
    assert_eq!(changes.iter().map(|c| c.0).collect::<Vec<_>>(), vec![7]);
    // The other messages of its chunk, and every other chunk, are the same allocations still.
    assert!(Arc::ptr_eq(map.get(&8).unwrap(), copy.get(&8).unwrap()));
    assert!(Arc::ptr_eq(map.get(&(3 * CHUNK)).unwrap(), copy.get(&(3 * CHUNK)).unwrap()));
    assert!(!copy.get(&7).unwrap().read, "the copy is as it was");
    // Written and read back as a map by id, as before chunks.
    let json = serde_json::to_string(&map).unwrap();
    let plain: BTreeMap<Id, Arc<Message>> = serde_json::from_str(&json).unwrap();
    assert_eq!(serde_json::to_string(&plain).unwrap(), json);
    let back: Messages = serde_json::from_str(&json).unwrap();
    assert_eq!(back, map);
}

proptest! {
    /// The messages of a list of ids, in order or not, are those `get` finds for each.
    #[test]
    fn get_sorted_finds_what_get_finds(have in proptest::collection::btree_set(id(), 0..60), mut ask in proptest::collection::vec(id(), 0..60), sort in any::<bool>()) {
        let map: Messages = have.iter().map(|id| (*id, message(*id, "x"))).collect();
        if sort {
            ask.sort_unstable();
        }
        let found: Vec<Id> = map.get_sorted(&ask).map(|m| m.id).collect();
        let each: Vec<Id> = ask.iter().filter_map(|id| map.get(id)).map(|m| m.id).collect();
        prop_assert_eq!(found, each);
    }
}

#[test]
fn a_message_put_in_place_in_copies_shares_their_chunks() {
    let mut live = Messages::new();
    for id in 1..=5 {
        live.insert(id, message(id, "full"));
    }
    let old = live.get(&3).unwrap().clone();
    // Two copies kept to undo to, sharing every chunk with each other; a third that changed
    // message 1 since.
    let mut third = live.clone();
    third.insert(1, message(1, "changed"));
    let mut copies = [live.clone(), live.clone(), third];
    // Message 3 is let go of in the live map.
    let lean = message(3, "lean");
    live.insert(3, lean.clone());
    let mut done = crate::messages::Replaced::default();
    for copy in &mut copies {
        assert!(copy.put_in_place_of(3, &old, &lean, &live, &mut done));
    }
    drop(done);
    for copy in &copies {
        assert!(Arc::ptr_eq(copy.get(&3).unwrap(), &lean));
    }
    // The two that held what the live map holds now share its chunk; the third keeps its own change.
    let chunk = |m: &Messages| m.chunks().values().next().unwrap().clone();
    assert!(Arc::ptr_eq(&chunk(&copies[0]), &chunk(&live)) && Arc::ptr_eq(&chunk(&copies[1]), &chunk(&live)));
    assert_eq!(copies[2].get(&1).unwrap().subject, "changed");
    assert_eq!(Arc::strong_count(&old), 1, "nothing holds the full message any more");
    // A copy that doesn't hold `old` itself is left as it is.
    let mut done = crate::messages::Replaced::default();
    let mut other = Messages::new();
    other.insert(3, message(3, "another"));
    assert!(!other.put_in_place_of(3, &old, &lean, &live, &mut done));
    assert!(!other.put_in_place_of(99, &old, &lean, &live, &mut done));
    assert_eq!(other.get(&3).unwrap().subject, "another");
    // One record kept across two messages of a chunk: the second is put in place of its own, and
    // the first's replacement is never taken for it.
    let mut live = Messages::new();
    for id in 1..=5 {
        live.insert(id, message(id, "full"));
    }
    let (old1, old2) = (live.get(&1).unwrap().clone(), live.get(&2).unwrap().clone());
    let (mut a, mut b) = (live.clone(), live.clone());
    let lean1 = message(1, "lean");
    let lean2 = message(2, "lean");
    let mut done = crate::messages::Replaced::default();
    assert!(a.put_in_place_of(1, &old1, &lean1, &live, &mut done));
    assert!(b.put_in_place_of(2, &old2, &lean2, &live, &mut done));
    assert!(Arc::ptr_eq(b.get(&2).unwrap(), &lean2) && Arc::ptr_eq(b.get(&1).unwrap(), &old1));
}
