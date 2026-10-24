// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Hostile DAV XML is bounded and never panics.
#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Ok(root) = mailcraft_dav::xml::parse(data) {
        let _ = mailcraft_dav::xml::properties(&root).count();
    }
});
