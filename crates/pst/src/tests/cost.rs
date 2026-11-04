// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Files made to cost far more to read than their size: one value or block named many times,
//! loads that were not counted, attached messages that hold themselves or multiply. Each is
//! held to a budget for the whole call, or refused.

use std::io::Cursor;

use super::writer::{self, AttachSpec, Data, FolderSpec, Layout, Node, Spill, SubNode, binary, hnid_ref, int32, nodes, pc, raw, tc, unicode};
use super::{first_item, note, pattern, sample};
use crate::header::{HEADER_READ, Header};
use crate::ndb::Ndb;
use crate::prop::types::BINARY;
use crate::source::Source;
use crate::{AttachmentContent, Cap, Encryption, Error, Format, Limits, MessageRef, Nid, Part, PropName, PropValue, Pst};

fn unicode_layout() -> Layout {
    Layout::new(Format::Unicode, Encryption::Permute)
}

fn open(bytes: &[u8], limits: Limits) -> Pst<Cursor<&[u8]>> {
    Pst::open_with(Cursor::new(bytes), limits).unwrap()
}

/// The first spilled value of a fresh [`Spill`] (and of a table's): its subnode id.
const FIRST_SPILL: u32 = (0x101 << 5) | 0x1F;

/// Bytes kept in binary values, to measure what a walk or a listing holds.
fn binary_bytes<'a>(props: impl Iterator<Item = &'a crate::Property>) -> usize {
    props.map(|p| if let PropValue::Binary(b) = &p.value { b.len() } else { 0 }).sum()
}

fn inbox(pst: &mut Pst<Cursor<&[u8]>>) -> Nid {
    pst.walk_folders().unwrap().folders.iter().find(|f| f.folder.name == "Inbox").unwrap().folder.id
}

#[test]
fn named_property_names_are_capped_one_by_one_and_in_all() {
    // One 2,048-byte name, then 5,000 entries that all name the same 1,000-byte string: a
    // stream of 3 KB that would come out as 5 MB of names.
    let mut strings = Vec::new();
    for len in [2048usize, 1000] {
        strings.extend_from_slice(&(len as u32).to_le_bytes());
        strings.extend(std::iter::repeat_n(b'a', len).enumerate().map(|(i, c)| if i % 2 == 0 { c } else { 0 }));
    }
    let mut entries = Vec::new();
    for i in 0..5001u32 {
        let at: u32 = if i == 0 { 0 } else { 2052 };
        entries.extend_from_slice(&at.to_le_bytes());
        entries.extend_from_slice(&((2u16 << 1) | 1).to_le_bytes());
        entries.extend_from_slice(&(i as u16).to_le_bytes());
    }
    let spec = sample(unicode_layout(), None);
    let mut all = nodes(&spec);
    let mut spill = Spill::new();
    let blocks = pc(&spec.layout, &[(0x0001, int32(251)), (0x0002, binary(&[])), (0x0003, binary(&entries)), (0x0004, binary(&strings))], &mut spill);
    let map = all.iter_mut().find(|n| n.nid == 0x61).unwrap();
    map.data = Data::Blocks(blocks);
    map.subnodes = spill.subnodes;
    let bytes = raw(&spec.layout, &all);
    let mut pst = open(&bytes, Limits::default());
    let named = pst.named_properties().unwrap();
    assert_eq!(named.left_out()[0], (0x8000, Error::Cap(Cap::NamedProperties)), "a name over 1 KB is left out");
    assert!(named.left_out().iter().all(|(_, e)| *e == Error::Cap(Cap::NamedProperties)));
    assert_eq!(named.len() + named.left_out().len(), 5001, "every entry is read or named as left out");
    let kept: usize = named.iter().map(|p| if let PropName::Name(n) = &p.name { n.len() * 2 } else { 0 }).sum();
    assert!(kept <= crate::named::MAX_NAMES, "{kept}");
    assert!(named.len() >= 4000, "names within the total are kept: {}", named.len());
}

/// The file's folders under Contacts, each with 64 binary properties that all name one value of
/// `size` bytes.
fn aliased_folders(size: usize, count: usize) -> (Layout, Vec<u8>) {
    let mut spec = sample(unicode_layout(), None);
    spec.folders[2].children = (0..count).map(|i| FolderSpec { name: format!("Box {i}"), ..Default::default() }).collect();
    let mut all = nodes(&spec);
    for n in all.iter_mut().filter(|n| n.nid & 0x1F == 2 && n.nid != 0x122 && n.nid != writer::IPM_SUBTREE) {
        let mut spill = Spill::new();
        let blob = spill.put(0x1F, pattern(size, 3));
        assert_eq!(blob, FIRST_SPILL);
        let mut props = vec![(0x3001, unicode("Aliased"))];
        props.extend((0..64u16).map(|i| (0x6000 + i, hnid_ref(BINARY, blob))));
        n.data = Data::Blocks(pc(&spec.layout, &props, &mut spill));
        n.subnodes = spill.subnodes;
    }
    let bytes = raw(&spec.layout, &all);
    (spec.layout, bytes)
}

#[test]
fn a_folder_walk_reads_within_one_budget() {
    // 25 folders × 64 names of one 60 KB value: 96 MB from a file of under 2 MB.
    let (_, bytes) = aliased_folders(60_000, 20);
    assert!(bytes.len() < 2_000_000);
    let limit = 4 * 1024 * 1024;
    let mut pst = open(&bytes, Limits { max_listing_bytes: limit, ..Default::default() });
    let tree = pst.walk_folders().unwrap();
    let kept: usize = tree.folders.iter().map(|f| binary_bytes(f.folder.properties.iter())).sum();
    assert!(kept as u64 <= limit, "{kept}");
    assert!(tree.left_out.iter().any(|l| l.reason == Error::Cap(Cap::ListingSize)), "{:?}", tree.left_out);
}

#[test]
fn a_folder_walk_leaves_large_values_for_later() {
    let (_, bytes) = aliased_folders(100_000, 1);
    let mut pst = open(&bytes, Limits::default());
    let tree = pst.walk_folders().unwrap();
    let boxed = tree.folders.iter().find(|f| f.folder.name == "Aliased").unwrap().folder.clone();
    assert_eq!(boxed.properties.get(0x6000), Some(&PropValue::Large { size: 100_000 }), "past the summary cap");
    let whole = pst.folder(boxed.id).unwrap();
    assert_eq!(whole.properties.get(0x6000), Some(&PropValue::Binary(pattern(100_000, 3))), "read whole on its own");
}

#[test]
fn a_listing_of_items_reads_within_one_budget() {
    // 200 rows whose binary cell names the first row's 50 KB value: 10 MB from 60 KB.
    let spec = sample(unicode_layout(), None);
    let mut all = nodes(&spec);
    let tag = 0x6100_0102u32;
    let rows: Vec<(u32, Vec<(u32, writer::Value)>)> = (0..200u32)
        .map(|i| {
            let v = if i == 0 { binary(&pattern(50_000, 9)) } else { hnid_ref(BINARY, FIRST_SPILL) };
            (((0x20000 + i) << 5) | 4, vec![(tag, v)])
        })
        .collect();
    let (blocks, subs) = tc(&spec.layout, &[tag], &rows);
    let inbox_contents = (0x403 << 5) | 0x0E;
    let table = all.iter_mut().find(|n| n.nid == inbox_contents).unwrap();
    table.data = Data::Blocks(blocks);
    table.subnodes = subs;
    let bytes = raw(&spec.layout, &all);
    assert!(bytes.len() < 200_000, "{}", bytes.len());
    let limit = 1024 * 1024;
    let mut pst = open(&bytes, Limits { max_listing_bytes: limit, ..Default::default() });
    let folder = inbox(&mut pst);
    let list = pst.items(folder).unwrap();
    assert_eq!(list.items.len(), 200, "every item is listed, read or not");
    let kept: usize = list.items.iter().map(|i| binary_bytes(i.properties.iter())).sum();
    assert!(kept as u64 <= limit, "{kept}");
    let short: Vec<&crate::LeftOut> = list.left_out.iter().filter(|l| l.reason == Error::Cap(Cap::ListingSize)).collect();
    assert!(!short.is_empty() && short.len() < 200, "{}", short.len());
    assert!(short.iter().all(|l| list.items.iter().any(|i| i.id == l.id && i.properties.is_empty())));
}

#[test]
fn a_data_tree_naming_one_block_twice_is_refused() {
    for levels in [1u8, 2] {
        let layout = Layout::new(Format::Unicode, Encryption::None);
        let store = Node { nid: 0x21, parent: 0, data: Data::Repeated { piece: vec![0xEC; 8000], times: 32, levels }, subnodes: vec![] };
        let bytes = raw(&layout, &[store]);
        assert!(bytes.len() < 30_000);
        assert_eq!(Pst::open(Cursor::new(&bytes[..])).err(), Some(Error::Cycle("a data tree")), "levels {levels}");
    }
}

#[test]
fn every_load_of_a_message_is_charged_to_its_budget() {
    // 30 attachments whose data (3,000 bytes each) is kept in their own heaps: each heap is
    // loaded, and counts, though the data itself is left for later.
    let mut spec = sample(unicode_layout(), None);
    let mut m = note("Many small files");
    m.attachments =
        (0..30).map(|i| AttachSpec::File { name: format!("f{i}.bin"), mime: "application/octet-stream".into(), data: pattern(3000, i) }).collect();
    spec.folders[0].messages = vec![m];
    let bytes = build_spec(&spec);
    let mut pst = open(&bytes, Limits::default());
    let item = first_item(&mut pst);
    assert_eq!(pst.message(&MessageRef::item(item)).unwrap().attachments.len(), 30);
    let mut pst = open(&bytes, Limits { max_item_bytes: 60_000, ..Default::default() });
    let item = first_item(&mut pst);
    assert_eq!(pst.message(&MessageRef::item(item)).err(), Some(Error::Cap(Cap::ItemSize)));
}

fn build_spec(spec: &writer::FileSpec) -> Vec<u8> {
    writer::build(spec)
}

#[test]
fn all_that_is_read_is_held_to_the_file_size() {
    let bytes = build_spec(&sample(unicode_layout(), None));
    let mut pst = open(&bytes, Limits { read_multiple: 3, read_floor: 0, ..Default::default() });
    let mut stopped = None;
    for _ in 0..100 {
        // A walk names what it could not read in its left_out, or fails as a whole.
        let reason = match pst.walk_folders() {
            Err(e) => Some(e),
            Ok(tree) => tree.left_out.first().map(|l| l.reason.clone()),
        };
        if reason.is_some() {
            stopped = reason;
            break;
        }
    }
    assert_eq!(stopped, Some(Error::Cap(Cap::TotalRead)));
    let mut pst = open(&bytes, Limits::default());
    for _ in 0..20 {
        pst.walk_folders().unwrap();
    }
}

/// A file whose first Inbox item has one attachment: a message whose object is `object`.
fn with_object(object: SubNode) -> Vec<u8> {
    let mut spec = sample(unicode_layout(), None);
    let mut m = note("Holds itself");
    m.attachments = vec![AttachSpec::Object(object)];
    spec.folders[0].messages = vec![m];
    build_spec(&spec)
}

const OBJECT: u32 = (0x300 << 5) | 0x04;
const ATTACHMENT: u32 = (0x200 << 5) | 0x05;

/// The data and subnode tree of a node, read from the file's node tree.
fn node_bids(bytes: &[u8], nid: u32) -> (u64, u64) {
    let limits = Limits::default();
    let mut src = Source::new(Cursor::new(bytes), &limits).unwrap();
    let header = Header::parse(&src.read_at(0, HEADER_READ).unwrap()).unwrap();
    let e = Ndb::new(src, header, limits).node(nid).unwrap();
    (e.data, e.sub)
}

#[test]
fn an_attached_message_that_is_the_message_it_is_attached_to_is_a_cycle() {
    // First with a stand-in for the object, to learn where the message's own data and subnode
    // tree are; then with the object naming them. Nothing else moves between the two.
    let first = with_object(SubNode::with_sub_bid(OBJECT, 4, 6));
    let mut pst = open(&first, Limits::default());
    let item = first_item(&mut pst);
    let (data, sub) = node_bids(&first, item.0);
    let bytes = with_object(SubNode::with_sub_bid(OBJECT, data, sub));
    assert_eq!(node_bids(&bytes, item.0), (data, sub));
    let mut pst = open(&bytes, Limits::default());
    let m = pst.message(&MessageRef::item(item)).unwrap();
    assert!(m.attachments.is_empty());
    assert_eq!(m.left_out[0].part, Part::Attachment { id: Nid(ATTACHMENT) });
    assert_eq!(m.left_out[0].reason, Error::Cycle("an attached message"));
    let inner = MessageRef { node: item, path: vec![Nid(ATTACHMENT), Nid(OBJECT)] };
    assert_eq!(pst.message(&inner).err(), Some(Error::Cycle("an attached message")));
}

#[test]
fn an_attachment_listed_twice_is_read_once() {
    let mut spec = sample(unicode_layout(), None);
    let mut m = note("Twice");
    m.attachments = vec![
        AttachSpec::File { name: "a.txt".into(), mime: "text/plain".into(), data: b"one".to_vec() },
        AttachSpec::File { name: "b.txt".into(), mime: "text/plain".into(), data: b"two".to_vec() },
    ];
    spec.folders[0].messages = vec![m];
    let mut all = nodes(&spec);
    let message = all.iter_mut().find(|n| n.nid & 0x1F == 4).unwrap();
    let rows = vec![(ATTACHMENT, vec![(0x3705_0003, int32(1))]), (ATTACHMENT, vec![(0x3705_0003, int32(1))])];
    let (blocks, subs) = tc(&spec.layout, &[0x3705_0003], &rows);
    let table = message.subnodes.iter_mut().find(|s| s.nid == 0x671).unwrap();
    table.data = Data::Blocks(blocks);
    table.subnodes = subs;
    let bytes = raw(&spec.layout, &all);
    let mut pst = open(&bytes, Limits::default());
    let item = first_item(&mut pst);
    let m = pst.message(&MessageRef::item(item)).unwrap();
    assert_eq!(m.attachments.len(), 1);
    assert_eq!(m.left_out[0].part, Part::Attachment { id: Nid(ATTACHMENT) });
    assert_eq!(m.left_out[0].reason, Error::Cycle("an attachment listed twice"));
}

#[test]
fn attached_messages_are_counted_per_item() {
    // Three attached messages, each with three of its own: twelve under the item.
    let mut spec = sample(unicode_layout(), None);
    let leaf = note("Leaf");
    let mut middle = note("Middle");
    middle.attachments = vec![AttachSpec::Message(Box::new(leaf)); 3];
    let mut top = note("Top");
    top.attachments = vec![AttachSpec::Message(Box::new(middle)); 3];
    spec.folders[0].messages = vec![top.clone(), top];
    let bytes = build_spec(&spec);
    let mut pst = open(&bytes, Limits { max_embedded_messages: 5, ..Default::default() });
    let folder = inbox(&mut pst);
    let items = pst.items(folder).unwrap().items;
    for item in &items {
        let (mut read, mut refused) = (0, 0);
        let mut queue = vec![MessageRef::item(item.id)];
        while let Some(m) = queue.pop() {
            match pst.message(&m) {
                Ok(msg) => {
                    read += usize::from(!m.path.is_empty());
                    for a in msg.attachments {
                        if let AttachmentContent::Message(inner) = a.content {
                            queue.push(inner);
                        }
                    }
                }
                Err(e) => {
                    assert_eq!(e, Error::Cap(Cap::EmbeddedMessages));
                    refused += 1;
                }
            }
        }
        assert_eq!((read, refused), (5, 4), "counted again for each item");
    }
}
