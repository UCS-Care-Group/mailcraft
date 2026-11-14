// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The samples the specifications print.

use super::writer::spec_sample;
use crate::lzxd::{base_position, decode, footer_bits, position_slots};
use crate::{Limits, Value, crc32, read_details, tag};

/// [MS-PATCH] 3: "abc" in an uncompressed block.
#[test]
fn the_patch_sample_unpacks_to_abc() {
    let stream = [0x14, 0x00, 0x00, 0x30, 0x30, 0x00, 0x01, 0, 0, 0, 0x01, 0, 0, 0, 0x01, 0, 0, 0, 0x61, 0x62, 0x63, 0x00];
    assert_eq!(decode(&stream, &[], 17, 3).unwrap(), b"abc");
}

/// [MS-PATCH] 2.1.6 and 2.6.2: the slots of each window and the table's rows.
#[test]
fn position_slots_are_the_specifications() {
    let slots: Vec<usize> = (17..=25).map(|b| position_slots(b).unwrap()).collect();
    assert_eq!(slots, [34, 36, 38, 42, 50, 66, 98, 162, 290]);
    assert_eq!(position_slots(16), None);
    assert_eq!(position_slots(26), None);
    for (slot, base, footer) in [
        (0, 0, 0),
        (3, 3, 0),
        (4, 4, 1),
        (6, 8, 2),
        (12, 64, 5),
        (24, 4096, 11),
        (35, 196_608, 16),
        (36, 262_144, 17),
        (42, 1_048_576, 17),
        (289, 33_423_360, 17),
    ] {
        assert_eq!((base_position(slot), footer_bits(slot)), (Some(base), footer), "slot {slot}");
    }
    assert_eq!(base_position(290), None);
}

/// [MS-OXOAB] 2.9.1: seeded with all ones and not inverted at the end, so the complement of the
/// usual CRC-32 (whose check value for "123456789" is 0xCBF43926).
#[test]
fn the_crc_is_the_complement_of_crc32() {
    assert_eq!(crc32(b"123456789"), !0xCBF4_3926);
    assert_eq!(crc32(b""), 0xFFFF_FFFF);
}

/// [MS-OXOAB] 3.3: the sample Full Details file, its CRC included.
#[test]
fn the_sample_details_file_reads() {
    let details = read_details(&spec_sample(), &Limits::default()).unwrap();
    assert_eq!(details.name(), Some("\\Global Address List"));
    assert_eq!(details.sequence(), Some(6));
    assert_eq!(details.header.text(tag::OAB_CONTAINER_GUID), Some("d4f244a8-a8ec-442a-87a3-5236f82cabdc"));
    assert_eq!(details.header.text(tag::OAB_DN), Some("/"));
    assert_eq!(details.header_schema.len(), 4);
    let tags: Vec<u32> = details.schema.iter().map(|p| p.tag).collect();
    assert_eq!(tags, [0x3003_001E, 0x39FE_001F, 0x3001_001F, 0x0FFE_0003, 0x3900_0003, 0x6805_1003]);
    let keys: Vec<(bool, bool)> = details.schema.iter().map(|p| (p.primary_key, p.anr)).collect();
    assert_eq!(keys, [(true, false), (true, false), (false, true), (false, false), (false, false), (false, false)]);
    assert_eq!(details.records.len(), 2);
    let lisa = &details.records[0];
    assert_eq!(lisa.text(tag::DISPLAY_NAME), Some("Lisa Miller"));
    assert_eq!(lisa.text(tag::SMTP_ADDRESS), Some("LisaM@example.com"));
    assert!(lisa.text(tag::EMAIL_ADDRESS).unwrap().ends_with("/cn=Recipients/cn=Lisa Miller"));
    assert_eq!(lisa.integer(tag::OBJECT_TYPE), Some(6));
    assert_eq!(lisa.integer(tag::DISPLAY_TYPE), Some(0));
    assert_eq!(lisa.get(tag::TRUNCATED_PROPERTIES), None);
    assert_eq!(details.records[1].get(tag::SMTP_ADDRESS), Some(&Value::Text("Administrator@example.com".into())));
}
