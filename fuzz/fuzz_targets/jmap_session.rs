// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes session object parsing (RFC 8620 §2).
//!
//! Invariants: nothing panics; an accepted session's API address satisfies the transport rule.

#![no_main]

use mailcraft_jmap::{Address, Session};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(base) = Address::parse("https://jmap.example.com/.well-known/jmap") else {
        return;
    };
    if let Ok(session) = Session::parse(data, &base) {
        assert!(session.api_address().uses_secure_transport());
    }
});
