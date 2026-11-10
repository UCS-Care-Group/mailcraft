// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures in the text of a message being written (#547): the checks a picture passes to be put
//! in, and the list that holds them beside the text.
//!
//! A picture stands in the text as a picture run ([`crate::rich::Run::picture`]); its bytes,
//! width and description are a [`Picture`] in the list the message keeps. Message pictures take
//! ids from [`FIRST_ID`] up, so they never meet the ids of a signature's pictures, which sit in
//! the same text. The caps are a message's own, well above a signature's (a signature is added to
//! every message, a picture in the text is one the owner chose): [`MAX_PICTURE_BYTES`], [`MAX_PICTURES_BYTES`]
//! and [`MAX_PICTURES`]. The kind of a picture is read from its own bytes, never its name.

use std::collections::HashSet;
use std::sync::Arc;

use crate::ModelError;
use crate::signature::{MAX_ALT, Picture};

/// Most bytes one picture in a message may have.
pub const MAX_PICTURE_BYTES: usize = 5 * 1024 * 1024;
/// Most bytes a message's pictures may have together.
pub const MAX_PICTURES_BYTES: usize = 20 * 1024 * 1024;
/// Most pictures one message holds.
pub const MAX_PICTURES: usize = 30;
/// The widest a picture is made or shown, in pixels: a wider one is scaled down to this.
pub const MAX_SHOWN_WIDTH: u16 = 1920;
/// Pictures above this many bytes together make the message noticeably heavy: the answer of
/// `insert.picture` says so.
pub const HEAVY_BYTES: usize = 2 * 1024 * 1024;

/// The first id of a picture in a message's text.
pub const FIRST_ID: u32 = 1 << 20;

/// The text a plain-text part has where a picture without a description stood.
pub const PLACEHOLDER: &str = "picture";

/// The kind of picture `bytes` are, by their own first bytes: PNG, JPEG, GIF or WebP only.
pub fn picture_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(&b"WEBP"[..]) { Some("image/webp") } else { crate::signature::picture_mime(bytes) }
}

/// A description as kept: one line, no control characters, at most [`MAX_ALT`] characters.
pub fn clean_alt(alt: &str) -> String {
    alt.chars().filter(|c| !c.is_control()).take(MAX_ALT).collect::<String>().trim().to_string()
}

/// A file name as kept: no folders, no control characters, at most 255 characters.
pub fn clean_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    name.rsplit(['/', '\\']).next().unwrap_or("").chars().take(255).collect()
}

/// Whether `data` may join `kept`: a picture of a kind that is sent, within every cap.
fn check(kept: &[Picture], data: &[u8]) -> Result<&'static str, ModelError> {
    let mime = picture_mime(data).ok_or_else(|| ModelError::Invalid("a picture in a message is a PNG, JPEG, GIF or WebP file".into()))?;
    if data.len() > MAX_PICTURE_BYTES {
        return Err(ModelError::Invalid(format!("a picture in a message is at most {} MB", MAX_PICTURE_BYTES >> 20)));
    }
    if kept.len() >= MAX_PICTURES {
        return Err(ModelError::Invalid(format!("a message holds at most {MAX_PICTURES} pictures")));
    }
    let total = kept.iter().map(|p| p.data.len()).fold(data.len(), usize::saturating_add);
    if total > MAX_PICTURES_BYTES {
        return Err(ModelError::Invalid(format!("a message's pictures are at most {} MB together", MAX_PICTURES_BYTES >> 20)));
    }
    Ok(mime)
}

/// Add a picture to `pictures`; `held` is the ids the text holds now. Pictures the text no longer
/// holds are kept (undo can bring them back) until they stand in the way of a cap. `seq` counts
/// the ids given out, so that an id is never given twice. The new picture's id.
pub fn add(
    pictures: &mut Vec<Picture>,
    seq: &mut u32,
    held: &HashSet<u32>,
    name: &str,
    data: Vec<u8>,
    alt: Option<&str>,
    width: Option<u16>,
) -> Result<u32, ModelError> {
    if check(pictures, &data).is_err() {
        pictures.retain(|p| held.contains(&p.id));
    }
    let mime = check(pictures, &data)?;
    let id = FIRST_ID.checked_add(*seq).ok_or_else(|| ModelError::Invalid("this message has had as many pictures as it can".into()))?;
    *seq = seq.saturating_add(1);
    pictures.push(Picture {
        id,
        name: clean_name(name),
        mime: mime.to_string(),
        data: Arc::new(data),
        width: width.map(|w| w.clamp(1, MAX_SHOWN_WIDTH)),
        alt: alt.map(clean_alt).unwrap_or_default(),
    });
    Ok(id)
}

/// A picture as a draft hands it back (any id, any bytes): `None` unless it is one that may be
/// kept, given the pictures already `kept`.
pub fn take(kept: &[Picture], id: u32, name: &str, data: Vec<u8>, alt: &str, width: Option<u16>) -> Option<Picture> {
    if id < FIRST_ID || kept.iter().any(|p| p.id == id) {
        return None;
    }
    let mime = check(kept, &data).ok()?;
    Some(Picture {
        id,
        name: clean_name(name),
        mime: mime.to_string(),
        data: Arc::new(data),
        width: width.map(|w| w.clamp(1, MAX_SHOWN_WIDTH)),
        alt: clean_alt(alt),
    })
}
