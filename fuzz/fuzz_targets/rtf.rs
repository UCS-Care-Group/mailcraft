// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Compressed rich text ([MS-OXRTFCP]) and the text of RTF, from arbitrary bytes: as a whole
//! stream, with its CRC put right so the decoder runs past the check, and as RTF directly.
#![no_main]

use mailcraft_pst::{crc32, decompress_rtf, rtf_html, rtf_text};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = decompress_rtf(data, 1 << 20);
    // The header's COMPSIZE and CRC made right for what follows it, as a writer would.
    let mut fixed = data.to_vec();
    if fixed.len() >= 16 {
        let content_crc = crc32(&fixed[16..]);
        let comp = u32::try_from(fixed.len() - 4).unwrap_or(0);
        fixed[0..4].copy_from_slice(&comp.to_le_bytes());
        fixed[8..12].copy_from_slice(b"LZFu");
        fixed[12..16].copy_from_slice(&content_crc.to_le_bytes());
        if let Ok(rtf) = decompress_rtf(&fixed, 1 << 20) {
            let _ = rtf_text(&rtf, 1 << 16);
        }
    }
    let _ = rtf_text(data, 1 << 16);
    let mut with_start = b"{\\rtf1 ".to_vec();
    with_start.extend_from_slice(data);
    let _ = rtf_text(&with_start, 1 << 16);
    let mut encapsulated = b"{\\rtf1\\fromhtml1 ".to_vec();
    encapsulated.extend_from_slice(data);
    let _ = rtf_html(&encapsulated, 1 << 16);
});
