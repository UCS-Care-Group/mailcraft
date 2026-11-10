// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An S/MIME Digital signature (#30): the bytes as a CMS SignedData, opaque (holding what was
//! signed) and detached (over a fixed signed part), checked against RFC 9216's example
//! certification authority, with the bytes also offered as a trust anchor and as a certificate
//! kept for the sender. The seeds in `fuzz/seeds/smime/` are signatures by RFC 9216's Alice
//! (DER and BER).
#![no_main]

use mailcraft_model::digital_signature::Coverage;
use mailcraft_smime::{Anchors, Context, Signed, check};

const CA: &[u8] = include_bytes!("../../crates/smime/testdata/ca.rsa.der");

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let anchors = Anchors::new([CA, data]);
    let known: [&[u8]; 1] = [data];
    let context = Context { anchors: &anchors, from: "alice@smime.example", now: 1_794_182_400, known: &known, coverage: Coverage::Whole };
    let _ = check(Signed::Opaque(data), &context);
    let _ = check(Signed::Detached { content: b"Content-Type: text/plain\r\n\r\nHello\r\n", signature: data }, &context);
});
