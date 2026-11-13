// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes the reader of the autoconfig document (#1112): what a domain publishes for mail
//! programs, or Mozilla's ISP database lists for it, as a hostile server would write it.
//!
//! Invariants: nothing panics; a server that is taken has a host name and a port and is
//! encrypted; the user name is no longer than a field may be.

#![no_main]

use mailcraft_model::Security;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Some((incoming, outgoing, username)) = mailcraft_net::read_autoconfig_for_fuzzing(data, "example.org") else { return };
    assert!(incoming.is_some() || outgoing.is_some());
    for server in incoming.iter().chain(&outgoing) {
        assert!(!server.host.is_empty() && server.host.len() <= 253, "{}", server.host);
        assert!(server.host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_'), "{}", server.host);
        assert!(server.port != 0);
        assert!(server.security != Security::None);
    }
    assert!(username.len() <= 512);
    assert!(incoming.is_some() || username.is_empty());
});
