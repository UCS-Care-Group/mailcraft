// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The CRC of [MS-OXOAB] 2.9.1: the reflected IEEE 802.3 polynomial `0xEDB88320`, seeded with
//! `0xFFFFFFFF` and with no final inversion (so the bitwise complement of the usual CRC-32; the
//! specification's sample file in 3.3 checks out only this way). It guards the unpacked details
//! file (`ulSerial`) and every unpacked block (`LZX_BLK.ulCRC`).

const TABLE: [u32; 256] = table();

#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)] // a constant, every index below 256
const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

/// The CRC of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        let at = ((crc ^ u32::from(b)) & 0xFF) as usize;
        crc = TABLE.get(at).copied().unwrap_or(0) ^ (crc >> 8);
    }
    crc
}
