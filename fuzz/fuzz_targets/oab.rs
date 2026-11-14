// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Offline Address Book (#1393): the bytes as an LZXD stream in every window, with
//! themselves as reference data; as a compressed file; and as a details file, once as they are
//! and once with its CRC put right, so that the reader of records is reached.
#![no_main]

use mailcraft_oab::{Limits, crc32, decompress, lzxd, read, read_details};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    // Small caps, so that one input stays quick.
    let limits = Limits { max_output: 1 << 20, max_block: 1 << 18, max_records: 10_000, max_record: 1 << 16, max_properties: 512, max_values: 200_000 };
    let (len, window) = match data {
        [a, b, c, ..] => (usize::from(u16::from_le_bytes([*a, *b])), 17 + u32::from(*c % 9)),
        _ => (0, 17),
    };
    let stream = data.get(3..).unwrap_or_default();
    let _ = lzxd::decode(stream, &[], window, len);
    let _ = lzxd::decode(stream, data, window, len);
    let _ = decompress(data, &limits);
    let _ = read(data, &limits);
    let _ = read_details(data, &limits);
    if data.len() >= 12 {
        let mut fixed = data.to_vec();
        let crc = crc32(&fixed[12..]);
        fixed[4..8].copy_from_slice(&crc.to_le_bytes());
        let _ = read_details(&fixed, &limits);
    }
});
