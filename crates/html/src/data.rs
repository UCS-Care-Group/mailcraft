// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures written into a message (#533): an `<img>` whose address is the picture itself,
//! `data:image/png;base64,…` (RFC 2397, base64 as in RFC 4648). They are part of the message, as
//! an attached (`cid:`) picture is: nothing is fetched for them and nobody is told anything.
//!
//! Only a PNG, JPEG, GIF or WebP is taken, by its declared type and then by its own bytes; any
//! other type (SVG, HTML, text) is not. The base64 is decoded within caps checked before
//! anything is decoded: [`MAX_DATA_PICTURE_BYTES`] for one picture, [`MAX_DATA_PICTURES`] and
//! [`MAX_DATA_PICTURES_BYTES`] for a message. What is refused is a placeholder for the picture.
//! A `data:` address is never a link, nor anything else of a message's.

use std::sync::Arc;

/// Largest picture written into a message that is decoded, in bytes.
pub const MAX_DATA_PICTURE_BYTES: usize = 10 * 1024 * 1024;

/// Most pictures written into one message that are decoded.
pub const MAX_DATA_PICTURES: usize = 32;

/// Most bytes of pictures written into one message that are decoded, all together.
pub const MAX_DATA_PICTURES_BYTES: usize = 40 * 1024 * 1024;

/// How much text the pictures of one message may take besides what is read of the rest
/// ([`crate::MAX_INPUT`]): their base64 for the most they may decode to, and room for the line
/// breaks a mail program puts in it.
pub(crate) const MAX_DATA_TEXT: usize = MAX_DATA_PICTURES_BYTES / 3 * 4 / 8 * 9;

/// The text of one picture past which it isn't looked at.
const MAX_PICTURE_TEXT: usize = MAX_DATA_PICTURE_BYTES / 3 * 4 / 8 * 9;

/// The bytes of a picture written into a message. Shown by its length alone when debugged, so
/// that a log or a failed test is not ten megabytes long.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture(pub Arc<Vec<u8>>);

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({} bytes)", self.0.len())
    }
}

impl std::ops::Deref for Picture {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

/// What the pictures written into a message have taken so far.
#[derive(Debug, Default)]
pub(crate) struct Budget {
    pictures: usize,
    bytes: usize,
}

/// Whether a picture's address is the picture itself (`data:`, in any letter case, with the tabs
/// and line breaks a browser leaves out left out).
pub(crate) fn is_data(src: &str) -> bool {
    let mut scheme = src.trim_start().chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).take(5);
    "data:".chars().all(|want| scheme.next().is_some_and(|c| c.eq_ignore_ascii_case(&want)))
}

/// The address kept of a picture written into the message (`data:` and its type when it is one
/// of the four, never its content), and its bytes when it is shown: decoded within `budget`
/// when `decode`, and a PNG, JPEG, GIF or WebP by its own bytes.
pub(crate) fn picture(src: &str, decode: bool, budget: &mut Budget) -> (String, Option<Picture>) {
    let Some((head, text)) = src.trim_start().split_once(',') else { return ("data:".into(), None) };
    let head: String = head.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let head = head.trim();
    let mut parts = head.get(5..).unwrap_or_default().split(';').map(str::trim);
    let kind = match parts.next().map(str::to_ascii_lowercase).as_deref() {
        Some("image/png") => "image/png",
        Some("image/jpeg" | "image/jpg" | "image/pjpeg") => "image/jpeg",
        Some("image/gif") => "image/gif",
        Some("image/webp") => "image/webp",
        _ => return ("data:".into(), None),
    };
    let kept = format!("data:{kind}");
    if !decode || !parts.any(|p| p.eq_ignore_ascii_case("base64")) || budget.pictures >= MAX_DATA_PICTURES || text.len() > MAX_PICTURE_TEXT {
        return (kept, None);
    }
    // What it would come to, from its length, before anything is decoded.
    let digits = text.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b'=').count();
    let size = digits / 4 * 3 + (digits % 4).saturating_sub(1);
    if size > MAX_DATA_PICTURE_BYTES || budget.bytes.saturating_add(size) > MAX_DATA_PICTURES_BYTES {
        return (kept, None);
    }
    budget.pictures += 1;
    budget.bytes = budget.bytes.saturating_add(size);
    let data = base64(text, size).filter(|bytes| is_picture(bytes)).map(|bytes| Picture(Arc::new(bytes)));
    (kept, data)
}

/// Base64 to bytes, the line breaks and spaces in it skipped and its padding optional; `None`
/// for anything else, or anything after the padding. `size`: what it comes to.
fn base64(text: &str, size: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(size);
    let (mut bits, mut held, mut padded) = (0u32, 0u32, false);
    for b in text.bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        if b == b'=' {
            padded = true;
            continue;
        }
        if padded {
            return None;
        }
        let value = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push(((bits >> held) & 0xff) as u8);
            bits &= (1 << held) - 1;
        }
    }
    Some(out)
}

/// Whether the bytes are a PNG, JPEG, GIF or WebP, by how each begins.
fn is_picture(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xff, 0xd8, 0xff])
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.get(..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
}
