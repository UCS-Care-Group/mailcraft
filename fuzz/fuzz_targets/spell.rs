// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Arbitrary text against the bundled US English dictionary: ranges stay inside the
//! string, and suggestions come back.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let Some(dict) = mailcraft_spell::en_us() else { return };
    let ranges = dict.check_text(&text, 32);
    let mut last = 0usize;
    for range in ranges {
        assert!(range.start <= range.end && range.end <= text.len());
        assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        assert!(range.start >= last);
        last = range.end;
    }
    let word: String = text.chars().take(80).collect();
    let _ = dict.suggest(&word, &[]);
});
