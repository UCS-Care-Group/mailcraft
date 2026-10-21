// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes API response parsing (RFC 8620 §3.4, §3.6.2).
//!
//! Invariant: nothing panics, and an accepted response can be inspected.

#![no_main]

use mailcraft_jmap::{ApiResponse, Redactor};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let mut redactor = Redactor::new();
    redactor.add("secret-value");
    if let Ok(response) = ApiResponse::parse(data, &redactor) {
        let _ = response.session_state();
        let _ = response.call_ids().count();
    }
});
