// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The caps of `Limits`, and keeping only some values.

use super::encoder::Options;
use super::packed::compressed;
use crate::test_writer::{details, spec_sample};
use crate::{Cap, Error, Limits, PropDef, Record, Value, crc32, read_details, read_details_keeping, tag};

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
