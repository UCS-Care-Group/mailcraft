// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exercise arbitrary bytes as well as edits of structured events and cards.
#![no_main]
use mailcraft_dav::document::{Component, patch};
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let data = data.get(..data.len().min(4096)).unwrap_or_default();
    let text = String::from_utf8_lossy(data);
    let card = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:stable\r\nFN:Old\r\nEND:VCARD\r\n";
    let event = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:stable\r\nSUMMARY:Old\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    for (seed, kind) in [(card, Component::Card), (event, Component::Event)] {
        let _ = patch(&text, seed, &seed.replace("Old", "New"), kind);
        let source = seed.replace("Old", &text);
        let after = source.replace("Old", "New");
        let _ = patch(&source, &source, &after, kind);
        let _ = patch(&seed.replace("Old", "Ol\r\n d"), seed, &seed.replace("Old", "New"), kind);
    }
});
