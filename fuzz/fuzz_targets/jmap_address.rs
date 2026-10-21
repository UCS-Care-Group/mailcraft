// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes absolute address parsing (RFC 3986) and reference resolution (RFC 3986 §5.2).
//!
//! Invariants: nothing panics; an accepted address re-parses to itself; its debug form shows the origin only.

#![no_main]

use mailcraft_jmap::Address;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(address) = Address::parse(text) {
        assert_eq!(Address::parse(address.as_str()).as_ref(), Ok(&address));
        assert_eq!(format!("{address:?}"), format!("Address({})", address.origin()));
    }
    if let Ok(base) = Address::parse("https://jmap.example.com/a/b/c?q")
        && let Ok(resolved) = base.resolve(text)
    {
        assert_eq!(Address::parse(resolved.as_str()).as_ref(), Ok(&resolved));
    }
});
