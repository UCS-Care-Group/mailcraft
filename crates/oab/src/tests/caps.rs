// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The caps of `Limits`, and keeping only some values.

use super::encoder::Options;
use super::packed::compressed;
use crate::test_writer::{details, spec_sample};
use crate::{Cap, Error, Limits, PropDef, Record, VALUE_SLOT, Value, crc32, read_details, read_details_keeping, tag};

#[test]
fn caps_are_checked_before_reading() {
    let file = compressed(&[7u8; 5000], 0x40000, 0, &Options::default());
    let limits = Limits { max_output: 4999, ..Limits::default() };
    assert_eq!(crate::decompress(&file, &limits), Err(Error::Cap(Cap::OutputSize)));
    let limits = Limits { max_block: 0x3FFFF, ..Limits::default() };
    assert_eq!(crate::decompress(&file, &limits), Err(Error::Cap(Cap::BlockSize)));
    let sample = spec_sample();
    assert_eq!(read_details(&sample, &Limits { max_records: 1, ..Limits::default() }), Err(Error::Cap(Cap::Records)));
    assert_eq!(read_details(&sample, &Limits { max_record: 0x8b, ..Limits::default() }), Err(Error::Cap(Cap::RecordSize)));
    assert_eq!(read_details(&sample, &Limits { max_properties: 5, ..Limits::default() }), Err(Error::Cap(Cap::Properties)));
    // Four header values and five for each of the two entries.
    assert!(read_details(&sample, &Limits { max_values: 14, ..Limits::default() }).is_ok());
    assert_eq!(read_details(&sample, &Limits { max_values: 13, ..Limits::default() }), Err(Error::Cap(Cap::Values)));
}

#[test]
fn only_the_values_asked_for_are_kept() {
    let details = read_details_keeping(&spec_sample(), &Limits::default(), |t| (t >> 16) as u16 == tag::SMTP_ADDRESS).unwrap();
    assert_eq!(details.name(), Some("\\Global Address List"));
    assert_eq!(details.records.len(), 2);
    assert!(details.records.iter().all(|r| r.values.len() == 1 && r.text(tag::SMTP_ADDRESS).is_some()));
}

/// Each of a multi-valued property's values counts against `max_values`, so a million empty
/// strings in one record can't pass as one value.
#[test]
fn each_of_many_values_counts() {
    let many = PropDef { tag: (u32::from(tag::PROXY_ADDRESSES) << 16) | 0x101F, primary_key: false, anr: false };
    let entry = Record { values: vec![(many.tag, Value::Texts(vec![String::new(); 1000]))] };
    let file = details(&[], &[many], &Record::default(), &[entry]);
    assert!(read_details(&file, &Limits { max_values: 1000, ..Limits::default() }).is_ok());
    assert_eq!(read_details(&file, &Limits { max_values: 999, ..Limits::default() }), Err(Error::Cap(Cap::Values)));
}

/// A packed block is held to the caller's `max_block` as well as to the file's own `ulBlockMax`.
#[test]
fn a_packed_block_is_held_to_the_callers_block_size() {
    // [MS-PATCH] 3's "abc", 22 bytes packed, in a file whose blocks are at most 3 bytes.
    let abc = [0x14, 0x00, 0x00, 0x30, 0x30, 0x00, 0x01, 0, 0, 0, 0x01, 0, 0, 0, 0x01, 0, 0, 0, 0x61, 0x62, 0x63, 0x00];
    let mut file = Vec::new();
    for word in [3u32, 1, 3, 3, 1, abc.len() as u32, 3, crc32(b"abc")] {
        file.extend_from_slice(&word.to_le_bytes());
    }
    file.extend_from_slice(&abc);
    assert_eq!(crate::decompress(&file, &Limits { max_block: 22, ..Limits::default() }).unwrap(), b"abc");
    assert_eq!(crate::decompress(&file, &Limits { max_block: 21, ..Limits::default() }), Err(Error::Cap(Cap::BlockSize)));
}

/// A multi-valued property with no values still counts one against `max_values`, so that a schema
/// of thousands of them can't keep them for nothing (a schema can't repeat one: each is its own).
#[test]
fn an_empty_multi_valued_property_counts_one() {
    let schema: Vec<PropDef> =
        (0..3u32).map(|i| PropDef { tag: ((u32::from(tag::PROXY_ADDRESSES) + i) << 16) | 0x101F, primary_key: false, anr: false }).collect();
    let entry = Record { values: schema.iter().map(|p| (p.tag, Value::Texts(Vec::new()))).collect() };
    let file = details(&[], &schema, &Record::default(), &[entry]);
    assert!(read_details(&file, &Limits { max_values: 3, ..Limits::default() }).is_ok());
    assert_eq!(read_details(&file, &Limits { max_values: 2, ..Limits::default() }), Err(Error::Cap(Cap::Values)));
}

/// A small packed file of large values (here sixteen certificates of 64 KB, packed by the test
/// encoder into a fraction of that) is held to `max_kept_bytes` as it is read, not after.
#[test]
fn a_small_file_of_large_values_is_held_to_the_bytes_kept() {
    let certificates = PropDef { tag: 0x3A70_1102, primary_key: false, anr: false };
    let entry = Record { values: vec![(certificates.tag, Value::Binaries(vec![vec![0x5A; 64 * 1024]]))] };
    let unpacked = details(&[], &[certificates], &Record::default(), &vec![entry; 16]);
    let file = compressed(&unpacked, 0x40000, 0, &Options::default());
    assert!(file.len() * 16 < unpacked.len(), "{} packed of {}", file.len(), unpacked.len());
    assert_eq!(crate::read(&file, &Limits::default()).unwrap().records.len(), 16);
    let limits = Limits { max_kept_bytes: 15 * 64 * 1024, ..Limits::default() };
    assert_eq!(crate::read(&file, &limits), Err(Error::Cap(Cap::KeptBytes)));
}

/// Many tiny values (each `"a"`, as the review's 500,000 proxy addresses of a record) are held
/// to `max_kept_bytes` by what they cost in memory, [`VALUE_SLOT`] each beside their byte: their
/// bytes alone would pass a cap set far below what they take.
#[test]
fn a_small_file_of_tiny_values_is_held_to_the_memory_they_take() {
    let many = PropDef { tag: (u32::from(tag::PROXY_ADDRESSES) << 16) | 0x101F, primary_key: false, anr: false };
    let entry = Record { values: vec![(many.tag, Value::Texts(vec!["a".to_owned(); 20_000]))] };
    let unpacked = details(&[], &[many], &Record::default(), &vec![entry; 8]);
    let file = compressed(&unpacked, 0x40000, 0, &Options::default());
    assert!(file.len() * 16 < unpacked.len(), "{} packed of {}", file.len(), unpacked.len());
    let values = 8 * 20_000;
    // Their bytes alone (160,000) are under a cap of a megabyte; with a slot each they are not.
    let limits = Limits { max_kept_bytes: 1 << 20, ..Limits::default() };
    assert!(values < limits.max_kept_bytes && values * (1 + VALUE_SLOT) > limits.max_kept_bytes);
    assert_eq!(crate::read(&file, &limits), Err(Error::Cap(Cap::KeptBytes)));
    let enough = Limits { max_kept_bytes: values * (1 + VALUE_SLOT), ..Limits::default() };
    assert_eq!(crate::read(&file, &enough).unwrap().records.len(), 8);
}

/// A property table that lists a property twice is refused: each record could otherwise hold the
/// same name as often as the table repeats it.
#[test]
fn a_property_listed_twice_is_refused() {
    let name = PropDef { tag: (u32::from(tag::DISPLAY_NAME) << 16) | 0x001F, primary_key: false, anr: true };
    let entry = Record { values: vec![(name.tag, Value::Text("Ada".into()))] };
    let once = details(&[], &[name], &Record::default(), std::slice::from_ref(&entry));
    assert_eq!(read_details(&once, &Limits::default()).unwrap().records.len(), 1);
    let twice = details(&[], &[name, name], &Record::default(), &[]);
    assert_eq!(read_details(&twice, &Limits::default()), Err(Error::Corrupt("a property listed twice")));
    let in_the_header = details(&[name, name], &[name], &Record::default(), &[]);
    assert_eq!(read_details(&in_the_header, &Limits::default()), Err(Error::Corrupt("a property listed twice")));
}

/// The output grows as blocks come, and never past what the file unpacks to: no doubling of a
/// large output at its last block.
#[test]
fn the_output_is_never_given_more_room_than_the_file_unpacks_to() {
    let data: Vec<u8> = (0..5 * 0x10000 + 123).map(|i| (i % 251) as u8).collect();
    let file = crate::test_writer::stored(&data, 0x10000);
    let out = crate::decompress(&file, &Limits::default()).unwrap();
    assert_eq!(out, data);
    assert_eq!(out.capacity(), out.len());
}
