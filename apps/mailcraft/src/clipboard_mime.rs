// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the Wayland clipboard hands over as bytes, checked before anything uses it (#1217).
//!
//! Under Wayland in the Flatpak, `arboard` has neither the compositor's data-control protocol nor
//! an X11 to fall back on, so HTML and pictures are read through winit's own `wl_data_device`
//! (`clipboard::wayland`), as the text already is. What comes through it was written by whatever
//! program the owner copied from: the types asked for, the most read and the shape a picture must
//! have are decided here, with no I/O, so that they can be tested on any system.

use mailcraft_ui_egui::MAX_PICTURE_SIDE;

/// HTML, in the order asked for.
pub const HTML_MIMES: [&str; 2] = ["text/html;charset=utf-8", "text/html"];

/// A picture, as PNG (what screenshot tools and "Copy Image" put there).
pub const IMAGE_MIMES: [&str; 1] = ["image/png"];

/// The most bytes read for HTML: twice the engine's 4 MB limit, since a program may send UTF-16.
pub const MAX_HTML_BYTES: usize = 8 << 20;

/// The most bytes read for a picture.
pub const MAX_PICTURE_BYTES: usize = 32 << 20;

/// HTML text out of the bytes a clipboard gave: UTF-8, or UTF-16 when it starts with a byte
/// order mark (some browsers send it so). Anything undecodable is repaired, never refused.
pub fn decode_html(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], big: bool| {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|p| if big { u16::from_be_bytes(*p) } else { u16::from_le_bytes(*p) }).collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Whether `bytes` are a PNG whose header says it is no wider or taller than a picture may be
/// (`MAX_PICTURE_SIDE`), read from the header alone: nothing is decoded here.
pub fn png_fits(bytes: &[u8]) -> bool {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() > MAX_PICTURE_BYTES || !bytes.starts_with(SIGNATURE) {
        return false;
    }
    // The signature, then the IHDR chunk: length (4), "IHDR" (4), width (4), height (4).
    let (Some(kind), Some(width), Some(height)) = (bytes.get(12..16), bytes.get(16..20), bytes.get(20..24)) else {
        return false;
    };
    let side = |b: &[u8]| b.try_into().map(u32::from_be_bytes).ok().and_then(|n| usize::try_from(n).ok());
    kind == b"IHDR"
        && matches!((side(width), side(height)), (Some(w), Some(h)) if (1..=MAX_PICTURE_SIDE).contains(&w) && (1..=MAX_PICTURE_SIDE).contains(&h))
}

#[cfg(test)]
#[path = "clipboard_mime_tests.rs"]
mod clipboard_mime_tests;
