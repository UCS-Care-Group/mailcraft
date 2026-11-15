// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! HTML mail to display blocks, HTML pasted into the composer, and the original under a reply or
//! forward, which must never carry what runs or embeds. What the blocks repeat of the links and
//! fonts they name stays a small multiple of the input.
#![no_main]

use mailcraft_html::{Block, BlockKind};

/// The bytes of text, links and fonts the blocks hold, counted once per span or picture.
fn held(blocks: &[Block], depth: usize) -> usize {
    let mut total = 0usize;
    for block in blocks {
        match &block.kind {
            BlockKind::Image { link, .. } => total += link.as_deref().map_or(0, str::len),
            BlockKind::Table(table) if depth < 64 => {
                for cell in table.rows.iter().flat_map(|r| &r.cells) {
                    total += held(&cell.blocks, depth + 1);
                }
            }
            _ => {}
        }
        for span in &block.spans {
            total += span.text.len() + span.link.as_deref().map_or(0, str::len) + span.font.as_deref().map_or(0, str::len);
        }
    }
    total
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let html = String::from_utf8_lossy(data);
    let blocks = mailcraft_html::parse(&html);
    let held = held(&blocks, 0);
    assert!(held <= 8 * html.len() + 64 * 1024, "{held} bytes held from {} of input", html.len());
    let _ = mailcraft_mime_model::rich::from_html(&html);
    let quoted = mailcraft_html::quotable(&html).to_ascii_lowercase();
    for tag in ["<script", "<style", "<iframe", "<object", "<embed"] {
        assert!(!quoted.contains(tag), "{tag} carried");
    }
});
