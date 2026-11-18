// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A vCard file, in any character set; what is read (keys among it) is written and read again.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let contacts = mailcraft_vcard::parse_bytes(data);
    let _ = mailcraft_vcard::parse(&mailcraft_vcard::write(&contacts.iter().collect::<Vec<_>>()));
});
