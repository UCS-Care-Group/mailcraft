// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Encrypted S/MIME message (#30 part 6): the bytes as a CMS EnvelopedData or
//! AuthEnvelopedData, opened with a key holder that hands back a key of each size in turn
//! (so that the ciphers and the padding checks are reached, not only the recipient matching),
//! with the bytes also offered as the owner's certificate.
#![no_main]

use mailcraft_smime::{Unwrapped, decrypt, is_enveloped};
use zeroize::Zeroizing;

const CA: &[u8] = include_bytes!("../../crates/smime/testdata/ca.rsa.der");

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = is_enveloped(data);
    let certificates: [&[u8]; 2] = [CA, data];
    let mut asked = 0usize;
    let _ = decrypt::<()>(data, &certificates, |_, _, wrapped| {
        asked += 1;
        // A key of every size the ciphers take (and one they don't), by how often it was asked.
        match (asked + wrapped.len()) % 5 {
            0 => Unwrapped::Refused,
            1 => Unwrapped::Key(Zeroizing::new(vec![0x11; 16])),
            2 => Unwrapped::Key(Zeroizing::new(vec![0x22; 24])),
            3 => Unwrapped::Key(Zeroizing::new(vec![0x33; 32])),
            _ => Unwrapped::Stop(()),
        }
    });
});
