// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the test encoder and writer make reads back as it was.

use proptest::prelude::*;

use super::encoder::{Kind, Options, encode, encode_with_reference};
use super::writer::{compressed, compressed_with, details};
use crate::lzxd::decode;
use crate::{Limits, PropDef, Record, Value, read, tag};

/// Text that repeats itself, as an address list does, with a counter so that it isn't all the
/// same.
fn sample(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut i = 0u32;
    while out.len() < len {
        out.extend_from_slice(format!("/o=Example/ou=Exchange/cn=Recipients/cn=person{i}\0person{i}@example.org\0Person {i}\0").as_bytes());
        i = i.wrapping_mul(7).wrapping_add(13) % 5000;
    }
    out.truncate(len);
    out
}

/// Bytes that don't compress.
fn noise(len: usize, mut seed: u32) -> Vec<u8> {
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect()
}

fn round_trip(data: &[u8], opts: &Options) {
    let packed = encode(data, opts);
    let unpacked = decode(&packed, &[], opts.window_bits, data.len()).unwrap();
    assert!(unpacked == data, "{opts:?}: {} bytes differ", data.iter().zip(&unpacked).filter(|(a, b)| a != b).count());
}

#[test]
fn every_kind_of_block_round_trips_over_several_chunks() {
    let data = sample(100_000);
    for kind in [Kind::Verbatim, Kind::Aligned, Kind::Uncompressed] {
        for runs in [true, false] {
            for repeats in [true, false] {
                round_trip(&data, &Options { blocks: vec![(kind, 1 << 20)], runs, repeats, ..Options::default() });
            }
        }
    }
}

#[test]
fn blocks_of_every_kind_follow_each_other_across_chunks() {
    let mut data = sample(70_000);
    data.extend(noise(30_001, 7));
    data.extend(sample(40_000));
    let blocks =
        vec![(Kind::Verbatim, 20_001), (Kind::Uncompressed, 33_333), (Kind::Aligned, 12_345), (Kind::Uncompressed, 1), (Kind::Verbatim, 50_000)];
    round_trip(&data, &Options { blocks: blocks.clone(), ..Options::default() });
    round_trip(&data, &Options { blocks, window_bits: 18, ..Options::default() });
}

/// An uncompressed block that ends on a chunk with an odd length, and one that is all one chunk.
#[test]
fn uncompressed_blocks_meet_chunks_at_any_byte() {
    let data = noise(3 * 32 * 1024 + 5, 3);
    for first in [32 * 1024 - 1, 32 * 1024, 32 * 1024 + 1, 1] {
        round_trip(&data, &Options { blocks: vec![(Kind::Uncompressed, first), (Kind::Uncompressed, 32 * 1024 + 3)], ..Options::default() });
        round_trip(&data, &Options { blocks: vec![(Kind::Verbatim, first), (Kind::Uncompressed, 777)], ..Options::default() });
    }
}

/// Long runs: matches of 257 bytes and more, with every form of the extra length.
#[test]
fn long_matches_round_trip() {
    let mut data = vec![b'a'; 32 * 1024 * 3];
    data.extend(sample(10_000));
    data.extend(vec![0u8; 300]);
    data.extend(vec![1u8; 600]);
    data.extend(vec![2u8; 2_000]);
    data.extend(vec![3u8; 9_000]);
    round_trip(&data, &Options::default());
    round_trip(&data, &Options { blocks: vec![(Kind::Aligned, 1 << 20)], ..Options::default() });
}

/// Matches far back: every window, with the aligned offset tree and without.
#[test]
fn far_matches_round_trip_in_every_window() {
    let unit = noise(70_000, 11);
    let mut data = unit.clone();
    data.extend(sample(5_000));
    data.extend(&unit[..40_000]);
    for window_bits in [17, 18, 20, 25] {
        for kind in [Kind::Verbatim, Kind::Aligned] {
            round_trip(&data, &Options { window_bits, blocks: vec![(kind, 1 << 20)], ..Options::default() });
        }
    }
}

/// E8 call translation, on bytes full of 0xE8.
#[test]
fn e8_translation_is_undone() {
    let mut data = Vec::new();
    for i in 0..20_000u32 {
        data.push(0xE8);
        data.extend_from_slice(&(i.wrapping_mul(2_654_435_761) as i32 >> (i % 20)).to_le_bytes());
        data.extend_from_slice(b"\x90\x90");
    }
    for size in [0u32, 1000, 12_000_000, u32::MAX] {
        round_trip(&data, &Options { e8: Some(size), ..Options::default() });
        round_trip(&data, &Options { e8: Some(size), blocks: vec![(Kind::Uncompressed, 40_000), (Kind::Aligned, 1 << 20)], ..Options::default() });
    }
}

/// Reference data before the output, as a patch has it.
#[test]
fn matches_reach_into_the_reference() {
    let reference = sample(50_000);
    let mut data = reference[10_000..30_000].to_vec();
    data.extend(noise(1000, 5));
    let opts = Options::default();
    let packed = encode_with_reference(&reference, &data, &opts);
    assert!(packed.len() < 3_000, "{} bytes: the matches didn't reach the reference", packed.len());
    assert_eq!(decode(&packed, &reference, 17, data.len()).unwrap(), data);
}

/// A whole address list, written, packed in blocks of 256 KB (one in three stored) and read.
#[test]
fn a_compressed_details_file_reads_back() {
    let def = |tag: u32| PropDef { tag, primary_key: false, anr: false };
    let header_schema = [def(0x6800_001F), def(0x6804_001E), def(0x6801_0003), def(0x6802_001E)];
    let schema = [
        PropDef { tag: 0x3003_001E, primary_key: true, anr: false },
        PropDef { tag: 0x39FE_001F, primary_key: true, anr: false },
        PropDef { tag: 0x3001_001F, primary_key: false, anr: true },
        def(0x3900_0003),
        def(0x3A08_001F),
        def(0x800F_101F),
        def(0x8C6A_1102),
        def(0x3A40_000B),
        def(0x6805_1003),
        def(0x8C6D_0102),
    ];
    let header = Record {
        values: vec![
            (0x6800_001F, Value::Text("Address list".into())),
            (0x6804_001E, Value::Text("/".into())),
            (0x6801_0003, Value::Integer(4242)),
            (0x6802_001E, Value::Text("00000000-0000-0000-0000-000000000001".into())),
        ],
    };
    let records: Vec<Record> = (0..6_000u32)
        .map(|i| {
            let mut values = vec![
                (0x3003_001E, Value::Text(format!("/o=Example/cn=Recipients/cn=p{i}"))),
                (0x39FE_001F, Value::Text(format!("p{i}@example.org"))),
                (0x3001_001F, Value::Text(format!("Persön {i}"))),
                (0x3900_0003, Value::Integer(i % 7)),
            ];
            if i % 3 == 0 {
                values.push((0x3A08_001F, Value::Text(format!("+1 555 0100 {i}"))));
            }
            values.push((0x800F_101F, Value::Texts(vec![format!("SMTP:p{i}@example.org"), format!("smtp:alias{i}@example.org")])));
            if i % 50 == 0 {
                values.push((0x8C6A_1102, Value::Binaries(vec![vec![0x30, 0x82, i as u8], vec![]])));
            }
            values.push((0x3A40_000B, Value::Boolean(i % 2 == 0)));
            if i % 10 == 0 {
                values.push((0x6805_1003, Value::Integers(vec![0x8C9E_0102, 300, 70_000, 0x0100_0000])));
            }
            values.push((0x8C6D_0102, Value::Binary(i.to_le_bytes().to_vec())));
            Record { values }
        })
        .collect();
    let file = details(&header_schema, &schema, &header, &records);
    assert!(file.len() > 3 * 0x40000, "{} bytes: want several blocks", file.len());
    let packed = compressed(&file, 0x40000, 3, &Options { window_bits: 18, ..Options::default() });
    assert!(packed.len() < file.len() / 2);
    let read_back = read(&packed, &Limits::default()).unwrap();
    assert_eq!(read_back.name(), Some("Address list"));
    assert_eq!(read_back.sequence(), Some(4242));
    assert_eq!(read_back.records, records);
    assert_eq!(read_back.records[3].text(tag::BUSINESS_PHONE), Some("+1 555 0100 3"));
    assert_eq!(read_back.records[3].texts(tag::PROXY_ADDRESSES)[1], "smtp:alias3@example.org");
}

/// A server's file packs a block with the window that holds the block; one packed with the window
/// that holds the largest block reads too, and a block too large for 128 KB gets a larger one.
#[test]
fn blocks_read_with_whichever_window_they_were_packed_with() {
    let data = sample(700_000);
    for block_max in [0x20000, 0x40000, 0x80000] {
        for largest in [false, true] {
            let file = compressed_with(&data, block_max, 0, &Options::default(), largest);
            assert_eq!(crate::decompress(&file, &Limits::default()).unwrap(), data, "{block_max:#x}, largest {largest}");
        }
    }
    // The last block, of 77 KB in a file whose blocks may be 256 KB, as the server's file was.
    let file = compressed(&data[..78_885], 0x40000, 0, &Options::default());
    assert_eq!(crate::decompress(&file, &Limits::default()).unwrap(), &data[..78_885]);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Any bytes, in any blocks, with any options.
    #[test]
    fn anything_round_trips(
        seed in any::<u32>(),
        len in 0usize..80_000,
        alphabet in 1u8..=255,
        kinds in proptest::collection::vec((0usize..3, 1usize..50_000), 1..6),
        runs in any::<bool>(),
        repeats in any::<bool>(),
        e8 in proptest::option::of(any::<u32>()),
        window_bits in 17u32..=19,
    ) {
        let data: Vec<u8> = noise(len, seed | 1).into_iter().map(|b| b % alphabet).collect();
        let blocks = kinds.into_iter().map(|(k, n)| ([Kind::Verbatim, Kind::Aligned, Kind::Uncompressed][k], n)).collect();
        let opts = Options { window_bits, e8, blocks, runs, repeats };
        let packed = encode(&data, &opts);
        prop_assert_eq!(decode(&packed, &[], window_bits, data.len()).unwrap(), data);
    }
}
