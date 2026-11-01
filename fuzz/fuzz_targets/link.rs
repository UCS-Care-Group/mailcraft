// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Any text as a link: a web address read, written out and read again, and Punycode of any label.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Some(link) = mailcraft_link::WebLink::parse(&text) {
        assert!(!link.host().is_empty());
        // Reading what was written changes nothing.
        let again = mailcraft_link::WebLink::parse(link.as_str());
        assert_eq!(again.as_ref().map(mailcraft_link::WebLink::as_str), Some(link.as_str()));
    }
    let _ = mailcraft_link::encode_opaque(&text);
    let _ = mailcraft_link::encode_component(&text);
    let chars: Vec<char> = text.chars().take(200).collect();
    let _ = mailcraft_link::punycode::encode(&chars);
});
