// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's header block (HS), from any bytes: read as the content file it is kept in, and its
//! lines put back among the message's other header lines. Never a panic, and never more than the
//! caps. (A small block stays in a message's JSON, which `mailbox_file` reads.)
#![no_main]

use mailcraft_model::headers::{HeaderBlock, MAX_BLOCK_LINES, MAX_HEADER_BLOCK};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Some(block) = HeaderBlock::read(data) {
        assert!(block.data.len() <= MAX_HEADER_BLOCK);
        let lines = block.lines();
        assert!(lines.len() <= MAX_BLOCK_LINES);
        let working = vec![("List-Id".to_string(), "<list.example.org>".to_string())];
        let all = mailcraft_model::headers::merge(&working, &lines);
        assert_eq!(all.len(), lines.len() + 1);
    }
});
