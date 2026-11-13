// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures as a PDF takes them: a grey or colour JPEG as it is, anything else decoded and
//! compressed. Only what the reading pane shows is printed: a PNG, JPEG, GIF or WebP by its own
//! bytes, no larger than [`MAX_PICTURE_BYTES`].

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::io::{Cursor, Write};

use image::{ImageFormat, ImageReader, Limits};

/// The largest picture file that is looked at, as in the reading pane.
pub const MAX_PICTURE_BYTES: usize = 24 * 1024 * 1024;
/// The most pixels of one picture.
const MAX_PIXELS: u64 = 25_000_000;
/// The most pixels of the pictures of one document (a message), and of a whole print.
const MAX_DOC_PIXELS: u64 = 150_000_000;
const MAX_JOB_PIXELS: u64 = 600_000_000;
/// The most pictures one document prints, and a whole print.
const MAX_DOC_PICTURES: usize = 500;
const MAX_JOB_PICTURES: usize = 2_000;
/// The kinds printed, by what their bytes are.
const KINDS: &[ImageFormat] = &[ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Gif, ImageFormat::WebP];

/// A picture ready for the PDF.
pub(crate) struct Picture {
    pub width: u32,
    pub height: u32,
    pub data: Data,
}

pub(crate) enum Data {
    /// A baseline or progressive JPEG of one (grey) or three (colour) components, passed through.
    Jpeg { bytes: Vec<u8>, gray: bool },
    /// Compressed RGB samples, and compressed alpha when the picture has any.
    Raw { rgb: Vec<u8>, alpha: Option<Vec<u8>> },
}

/// The pictures of a print, each decoded once. A picture is known again by where its bytes are
/// (one attachment, however a message spells its address, is one set of bytes), and else by a
/// hash of them with a key of this print's own, which a message can't aim at.
pub(crate) struct Pictures {
    pub list: Vec<Picture>,
    by_place: HashMap<(usize, usize), Option<usize>>,
    by_hash: HashMap<u64, Option<usize>>,
    key: RandomState,
    /// What this print has taken, and what the document being laid out has.
    job: (usize, u64),
    doc: (usize, u64),
}

impl Default for Pictures {
    fn default() -> Pictures {
        Pictures { list: Vec::new(), by_place: HashMap::new(), by_hash: HashMap::new(), key: RandomState::new(), job: (0, 0), doc: (0, 0) }
    }
}

impl Pictures {
    /// A new document starts: its pictures have caps of their own, so that one message can't
    /// use up the others'.
    pub fn start_doc(&mut self) {
        self.doc = (0, 0);
    }

    /// The picture of `bytes`, decoded and kept, with its size in pixels; `None` when it isn't a
    /// picture this prints, or when the caps are reached.
    pub fn add(&mut self, bytes: &[u8]) -> Option<(usize, u32, u32)> {
        let place = (bytes.as_ptr() as usize, bytes.len());
        let known = match self.by_place.get(&place) {
            Some(known) => *known,
            None => {
                let index = if bytes.len() > MAX_PICTURE_BYTES {
                    None
                } else {
                    let key = self.key.hash_one(bytes);
                    match self.by_hash.get(&key) {
                        Some(known) => *known,
                        None => {
                            let index = self.decode_new(bytes);
                            self.by_hash.insert(key, index);
                            index
                        }
                    }
                };
                self.by_place.insert(place, index);
                index
            }
        };
        known.and_then(|i| self.list.get(i).map(|p| (i, p.width, p.height)))
    }

    fn decode_new(&mut self, bytes: &[u8]) -> Option<usize> {
        if self.list.len() >= MAX_JOB_PICTURES || self.doc.0 >= MAX_DOC_PICTURES {
            return None;
        }
        let room = MAX_DOC_PIXELS.saturating_sub(self.doc.1).min(MAX_JOB_PIXELS.saturating_sub(self.job.1));
        let p = decode(bytes, room)?;
        let pixels = u64::from(p.width) * u64::from(p.height);
        self.doc = (self.doc.0 + 1, self.doc.1.saturating_add(pixels));
        self.job = (self.job.0 + 1, self.job.1.saturating_add(pixels));
        self.list.push(p);
        Some(self.list.len() - 1)
    }
}

/// `bytes` as a picture, when it is one of [`KINDS`] of at most `room` pixels.
fn decode(bytes: &[u8], room: u64) -> Option<Picture> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let format = reader.format()?;
    if !KINDS.contains(&format) {
        return None;
    }
    let (width, height) = reader.into_dimensions().ok()?;
    let pixels = u64::from(width) * u64::from(height);
    if width == 0 || height == 0 || pixels > MAX_PIXELS || pixels > room {
        return None;
    }
    if format == ImageFormat::Jpeg
        && let Some(gray) = passes_through(bytes)
    {
        return Some(Picture { width, height, data: Data::Jpeg { bytes: bytes.to_vec(), gray } });
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    limits.max_alloc = Some(pixels.saturating_mul(8).saturating_add(1 << 20));
    reader.limits(limits);
    let rgba = reader.decode().ok()?.to_rgba8();
    let mut rgb = Vec::with_capacity(usize::try_from(pixels.saturating_mul(3)).unwrap_or(0));
    let mut alpha = Vec::with_capacity(usize::try_from(pixels).unwrap_or(0));
    for px in rgba.pixels() {
        let [r, g, b, a] = px.0;
        rgb.extend_from_slice(&[r, g, b]);
        alpha.push(a);
    }
    let alpha = alpha.iter().any(|a| *a != 255).then(|| deflate(&alpha));
    Some(Picture { width, height, data: Data::Raw { rgb: deflate(&rgb), alpha } })
}

/// Whether a JPEG can go into the PDF as it is, and whether it is grey: its frame has one
/// component, or three that are YCbCr (not an Adobe JPEG that says its three are RGB, which a
/// PDF reader would turn as if they were YCbCr). CMYK and YCCK JPEGs, with four, are decoded and
/// written again, for `DeviceRGB` would take their bytes for something else.
pub(crate) fn passes_through(bytes: &[u8]) -> Option<bool> {
    let (components, adobe_transform) = jpeg_frame(bytes)?;
    match (components, adobe_transform) {
        (1, _) => Some(true),
        (3, Some(0)) => None,
        (3, _) => Some(false),
        _ => None,
    }
}

/// The number of components in a JPEG's frame header, and the colour transform its Adobe marker
/// names, if it has one; `None` when the markers can't be read to a frame header.
fn jpeg_frame(bytes: &[u8]) -> Option<(u8, Option<u8>)> {
    if bytes.get(..2) != Some(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2usize;
    let mut transform = None;
    loop {
        if *bytes.get(i)? != 0xFF {
            return None;
        }
        while bytes.get(i) == Some(&0xFF) {
            i = i.checked_add(1)?;
        }
        let marker = *bytes.get(i)?;
        i = i.checked_add(1)?;
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]));
        if len < 2 {
            return None;
        }
        let body = bytes.get(i + 2..i.checked_add(len)?)?;
        let sof = matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF);
        if sof {
            return Some((*body.get(5)?, transform));
        }
        if marker == 0xEE && body.starts_with(b"Adobe") {
            transform = body.get(11).copied();
        }
        i = i.checked_add(len)?;
    }
}

/// `data` compressed for a `FlateDecode` stream.
pub(crate) fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    if encoder.write_all(data).is_err() {
        return Vec::new();
    }
    encoder.finish().unwrap_or_default()
}
