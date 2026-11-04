// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A damaged page or block loses what it holds and no more: the rest of the file reads, and
//! each loss is named (in a walk's, list's or message's `left_out`, and a page of the file's
//! own trees in [`Pst::damage`]).

use std::io::Cursor;

use super::sample;
use super::writer::{Layout, build};
use crate::header::{HEADER_READ, Header};
use crate::ndb::Ndb;
use crate::source::Source;
use crate::{Encryption, Error, Format, Limits, MessageRef, Nid, Pst};

fn base() -> Vec<u8> {
    build(&sample(Layout::tiny(Format::Unicode, Encryption::Permute), None))
}

/// The leaf pages of one tree (0x81 the node tree, 0x80 the block tree): offset and first key.
fn leaves(bytes: &[u8], ptype: u8) -> Vec<(usize, u64)> {
    (0x4400..bytes.len().saturating_sub(511))
        .step_by(512)
        .filter(|&ib| bytes[ib + 496] == ptype && bytes[ib + 497] == ptype && bytes[ib + 491] == 0)
        .map(|ib| (ib, u64::from_le_bytes(bytes[ib..ib + 8].try_into().unwrap())))
        .collect()
}

fn ndb(bytes: &[u8]) -> Ndb<Cursor<&[u8]>> {
    let limits = Limits::default();
    let mut src = Source::new(Cursor::new(bytes), &limits).unwrap();
    let header = Header::parse(&src.read_at(0, HEADER_READ).unwrap()).unwrap();
    Ndb::new(src, header, limits)
}

/// Every item of every folder: read, or the reason it wasn't.
fn read_all(pst: &mut Pst<Cursor<&[u8]>>) -> (usize, Vec<(Nid, Error)>) {
    let tree = pst.walk_folders().unwrap();
    let (mut read, mut lost) = (0, Vec::new());
    for f in &tree.folders {
        let Ok(list) = pst.items(f.folder.id) else { continue };
        for item in list.items {
            match pst.message(&MessageRef::item(item.id)) {
                Ok(_) => read += 1,
                Err(e) => lost.push((item.id, e)),
            }
        }
    }
    (read, lost)
}

#[test]
fn a_damaged_leaf_page_of_the_node_tree_loses_its_nodes_only() {
    let mut bytes = base();
    // The leaf with the highest ids holds messages alone.
    let &(ib, first) = leaves(&bytes, 0x81).iter().max_by_key(|l| l.1).unwrap();
    assert_eq!(first & 0x1F, 4, "a leaf of items");
    bytes[ib + 20] ^= 0xFF;
    let mut pst = Pst::open(Cursor::new(&bytes[..])).unwrap();
    let tree = pst.walk_folders().unwrap();
    assert_eq!(tree.folders.len(), 7, "every folder still read");
    let (read, lost) = read_all(&mut pst);
    assert!(read >= 1 && !lost.is_empty(), "{read} read, {lost:?}");
    assert!(lost.iter().all(|(_, e)| *e == Error::BadChecksum("a B-tree page")));
    let mut nodes = 0;
    pst.for_each_node(|_| {
        nodes += 1;
        true
    })
    .unwrap();
    assert!(nodes > 0);
    assert_eq!(pst.damage().len(), 1);
    assert_eq!((pst.damage()[0].offset, pst.damage()[0].what), (ib as u64, "a page of the node tree"));
}

#[test]
fn a_damaged_leaf_page_of_the_block_tree_loses_the_data_it_finds_only() {
    let mut bytes = base();
    let all = leaves(&bytes, 0x80);
    // The last leaf: blocks written last, of the search root's own tables.
    let &(ib, _) = all.iter().max_by_key(|l| l.1).unwrap();
    bytes[ib + 20] ^= 0xFF;
    let mut pst = Pst::open(Cursor::new(&bytes[..])).unwrap();
    let tree = pst.walk_folders().unwrap();
    assert!(tree.folders.len() >= 5, "{}", tree.folders.len());
    let (read, _) = read_all(&mut pst);
    assert!(read >= 3, "{read}");
    let lost_folders = tree.left_out.iter().filter(|l| l.reason == Error::BadChecksum("a B-tree page")).count();
    assert!(lost_folders >= 1 || pst.damage().iter().any(|d| d.offset == ib as u64), "{:?}", tree.left_out);
    assert!(pst.damage().iter().all(|d| d.what == "a page of the block tree" && d.offset == ib as u64), "{:?}", pst.damage());
    assert_eq!(pst.damage().len(), 1);
}

#[test]
fn a_damaged_block_of_one_message_loses_that_message_only() {
    let clean = base();
    let mut pst = Pst::open(Cursor::new(&clean[..])).unwrap();
    let item = super::first_item(&mut pst);
    let mut n = ndb(&clean);
    let entry = n.node(item.0).unwrap();
    // Its top block: its one data block, or the block of its data tree.
    let ib = n.block_entry(entry.data).unwrap().ib as usize;
    let mut bytes = clean.clone();
    bytes[ib + 1] ^= 0xFF;
    let mut pst = Pst::open(Cursor::new(&bytes[..])).unwrap();
    let tree = pst.walk_folders().unwrap();
    assert!(tree.left_out.is_empty());
    let (read, lost) = read_all(&mut pst);
    assert_eq!(lost, vec![(item, Error::BadChecksum("a block"))]);
    assert_eq!(read, 3, "the file's other items");
    assert!(pst.damage().is_empty(), "a block of a message is the message's loss, not the file's");
    // Its other parts are still there to be read.
    let list = pst.items(tree.folders[2].folder.id).unwrap();
    assert_eq!(list.items[0].properties.text(0x0037).as_deref(), Some("Quarterly figures"));
}

#[test]
fn a_damaged_folder_is_left_out_and_the_rest_read() {
    let clean = base();
    let mut pst = Pst::open(Cursor::new(&clean[..])).unwrap();
    let calendar = pst.walk_folders().unwrap().folders.iter().find(|f| f.folder.name == "Calendar").unwrap().folder.id;
    let mut n = ndb(&clean);
    let entry = n.node(calendar.0).unwrap();
    let mut bid = entry.data;
    if crate::ndb::is_internal(bid) {
        // The first data block under the data tree's top block.
        let top = n.block(bid).unwrap();
        bid = u64::from_le_bytes(top[8..16].try_into().unwrap());
    }
    let ib = n.block_entry(bid).unwrap().ib as usize;
    let mut bytes = clean.clone();
    bytes[ib + 1] ^= 0xFF;
    let mut pst = Pst::open(Cursor::new(&bytes[..])).unwrap();
    let tree = pst.walk_folders().unwrap();
    let names: Vec<&str> = tree.folders.iter().map(|f| f.folder.name.as_str()).collect();
    assert_eq!(names, vec!["", "Top of Personal Folders", "Inbox", "Projects", "Contacts", "Search Root"]);
    assert_eq!(tree.left_out.len(), 1);
    assert_eq!((tree.left_out[0].id, &tree.left_out[0].reason), (calendar, &Error::BadChecksum("a block")));
    let (read, lost) = read_all(&mut pst);
    assert_eq!((read, lost.len()), (3, 0), "the items of the other folders");
}
