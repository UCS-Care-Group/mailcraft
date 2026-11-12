// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message (and whether it is Digitally signed or Encrypted), an mbox file, a `mailto:` link and
//! encoded words, from any bytes.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = mailcraft_mime::parse(data);
    let _ = mailcraft_mime::parse_fetched(data);
    let _ = mailcraft_mime::parse_mbox(data);
    let text = String::from_utf8_lossy(data);
    let _ = mailcraft_mime::parse_mailto(&text);
    let _ = mailcraft_mime::decode_words(&text);
});
