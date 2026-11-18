// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! TNEF streams (`winmail.dat`, [MS-OXTNEF]) from arbitrary bytes: as they come, and after the
//! signature so the reader gets past its first check.
#![no_main]

use mailcraft_tnef::{Limits, SIGNATURE, read};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    if let Ok(t) = read(data, &limits) {
        assert!(t.files.iter().map(|f| f.data.len()).sum::<usize>() <= data.len());
    }
    let mut signed = SIGNATURE.to_vec();
    signed.extend_from_slice(data);
    if let Ok(t) = read(&signed, &limits) {
        assert!(t.files.iter().map(|f| f.data.len()).sum::<usize>() <= signed.len());
    }
});
