// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bytes nobody meant: never a panic, and what is wrong named.

use proptest::prelude::*;

use super::encoder::{Kind, Options, encode};
use super::packed::compressed;
use crate::lzxd::decode;
use crate::test_writer::spec_sample;
use crate::{Error, Limits, crc32, decompress, read, read_details};

fn small() -> Limits {
    Limits { max_output: 1 << 20, max_block: 1 << 18, max_records: 10_000, max_record: 1 << 16, max_properties: 256, max_values: 100_000 }
}

fn text(len: usize) -> Vec<u8> {
    (0..len).map(|i| b"Exchange address list "[i % 22] ^ (i / 997) as u8).collect()
}

/// The stream stops early: an error, at every length.
#[test]
fn a_cut_stream_is_an_error() {
    let data = text(40_000);
    for kind in [Kind::Verbatim, Kind::Aligned, Kind::Uncompressed] {
        let packed = encode(&data, &Options { blocks: vec![(kind, 1 << 20)], ..Options::default() });
        for cut in (0..packed.len()).step_by(97) {
            assert!(decode(&packed[..cut], &[], 17, data.len()).is_err(), "{kind:?} cut at {cut}");
        }
    }
}

/// A match before the first byte.
#[test]
fn a_match_before_the_start_is_refused() {
    let data = text(1000);
    let packed = encode(&data, &Options::default());
    // The same stream, told the output starts later than it does, still decodes; with less
    // reference than none it can't: a stream that matches 22 bytes back from the third byte.
    assert!(decode(&packed, &[], 17, data.len()).is_ok());
    let mut stream = vec![0u8; 0];
    // Verbatim block of 30 bytes whose first element is a match: built by hand from the encoder
    // would need a reference, so use one and then take it away.
    let reference = text(100);
    let tail = text(130)[100..].to_vec();
    let packed = super::encoder::encode_with_reference(&reference, &tail, &Options::default());
    stream.extend_from_slice(&packed);
    assert_eq!(decode(&stream, &reference, 17, 30).unwrap(), tail);
    assert_eq!(decode(&stream, &[], 17, 30), Err(Error::Corrupt("an LZX match before the start")));
}

/// A chunk whose size says otherwise than where it ends.
#[test]
fn a_chunk_of_the_wrong_size_is_refused() {
    let data = text(70_000);
    let mut packed = encode(&data, &Options::default());
    let size = u16::from_le_bytes([packed[0], packed[1]]);
    packed[..2].copy_from_slice(&(size - 2).to_le_bytes());
    assert_eq!(decode(&packed, &[], 17, data.len()), Err(Error::Corrupt("an LZX chunk of another size than it says")));
}

/// A block's CRC and the details file's.
#[test]
fn checksums_are_checked() {
    let data = text(5000);
    let mut file = compressed(&data, 0x40000, 0, &Options::default());
    assert_eq!(decompress(&file, &small()).unwrap(), data);
    file[16 + 12] ^= 1;
    assert_eq!(decompress(&file, &small()), Err(Error::BadChecksum("LZX_BLK")));
    let mut details = spec_sample();
    details[40] ^= 1;
    assert_eq!(read_details(&details, &small()), Err(Error::BadChecksum("OAB_HDR")));
}

/// Version fields that say it is another file.
#[test]
fn other_files_are_not_read() {
    let mut file = compressed(&text(100), 0x40000, 0, &Options::default());
    file[4] = 2;
    assert_eq!(decompress(&file, &small()), Err(Error::NotOab("LZX_HDR version")));
    let mut details = spec_sample();
    details[0] = 7;
    assert_eq!(read_details(&details, &small()), Err(Error::NotOab("OAB_HDR version")));
}

/// A details file whose CRC has been put right after a change, as a writer that means harm
/// would.
fn with_crc(mut file: Vec<u8>) -> Vec<u8> {
    let crc = crc32(&file[12..]);
    file[4..8].copy_from_slice(&crc.to_le_bytes());
    file
}

#[test]
fn records_that_overrun_are_refused() {
    let mut file = spec_sample();
    // The second entry says it is longer than the file.
    let at = file.len() - 0x8c;
    file[at..at + 4].copy_from_slice(&0x1000u32.to_le_bytes());
    assert_eq!(read_details(&with_crc(file), &small()), Err(Error::Truncated("OAB_V4_REC")));
    // More entries than there are.
    let mut file = spec_sample();
    file[8] = 3;
    assert_eq!(read_details(&with_crc(file), &small()), Err(Error::Truncated("OAB_V4_REC")));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn any_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096), window_bits in 15u32..27, len in 0usize..70_000) {
        let _ = decode(&bytes, &[], window_bits, len);
        let _ = decode(&bytes, &bytes, window_bits, len);
        let _ = decompress(&bytes, &small());
        let _ = read(&bytes, &small());
        let _ = read_details(&bytes, &small());
        if bytes.len() >= 12 {
            let _ = read_details(&with_crc(bytes.clone()), &small());
        }
    }

    /// A good stream with bytes changed: an error, or exactly as much output as asked for.
    #[test]
    fn a_changed_stream_never_panics(changes in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..8), kind in 0usize..3) {
        let data = text(50_000);
        let kind = [Kind::Verbatim, Kind::Aligned, Kind::Uncompressed][kind];
        let mut packed = encode(&data, &Options { blocks: vec![(kind, 20_000), (Kind::Aligned, 1 << 20)], ..Options::default() });
        for (at, b) in changes {
            let at = at % packed.len();
            packed[at] = b;
        }
        if let Ok(out) = decode(&packed, &[], 17, data.len()) {
            prop_assert_eq!(out.len(), data.len());
        }
    }

    /// The sample details file with bytes changed and its CRC put right.
    #[test]
    fn a_changed_details_file_never_panics(changes in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..8)) {
        let mut file = spec_sample();
        for (at, b) in changes {
            let at = 12 + at % (file.len() - 12);
            file[at] = b;
        }
        let _ = read_details(&with_crc(file), &small());
    }
}
