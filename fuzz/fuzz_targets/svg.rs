// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The SVG drawings of a message (#565): arbitrary bytes, and bytes that begin as a drawing, read
//! as the reading pane reads them (the caps and the allowlist, then usvg's tree) and drawn at
//! the size they are shown. Never a panic that is not caught, and never long: the runner's time
//! limit is the check on the reader's own.
#![no_main]

// The reader is the reading pane's own file, so that the fuzz workspace builds it without
// the program's window (the one thing it takes from its neighbours is a texture's largest side).
#[path = "../../crates/ui-egui/src/reading/svg.rs"]
#[allow(dead_code)]
mod svg;

mod images {
    pub const MAX_TEXTURE_SIDE: u32 = 8_192;
}

const START: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"20\">";

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = svg::probe(data);
    let _ = svg::decode(data, 512, 256);
    // As the start of a drawing, so that the fuzzer gets past the check of how it begins.
    let mut drawing = START.to_vec();
    drawing.extend_from_slice(data);
    drawing.extend_from_slice(b"</svg>");
    if svg::probe(&drawing).is_some() {
        let _ = svg::decode(&drawing, 512, 256);
    }
});
