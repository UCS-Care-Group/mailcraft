// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes the loose session-address normalisation (what a person types, RFC 8620 §2.2).
//!
//! Invariants: nothing panics; whatever is accepted satisfies the transport rule (https, or http to loopback).

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(address) = mailcraft_jmap::session_address(text) {
        assert!(address.uses_secure_transport());
    }
});
