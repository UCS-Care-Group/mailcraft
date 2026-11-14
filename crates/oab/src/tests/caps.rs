// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The caps of `Limits`, and keeping only some values.

use super::encoder::Options;
use super::writer::{compressed, spec_sample};
use crate::{Cap, Error, Limits, read_details, read_details_keeping, tag};

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
