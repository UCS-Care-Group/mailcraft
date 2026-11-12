// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures as a PDF takes them: a JPEG as it is, anything else decoded and compressed.

use std::collections::HashMap;
use std::io::{Cursor, Write};

use image::{ImageFormat, ImageReader, Limits};

/// The most pixels of one picture.
const MAX_PIXELS: u64 = 25_000_000;
/// The most pixels of all the pictures of a document.
const MAX_TOTAL_PIXELS: u64 = 150_000_000;
/// The most pictures a document prints.
const MAX_PICTURES: usize = 500;

/// A picture ready for the PDF.
pub(crate) struct Picture {
    pub width: u32,
    pub height: u32,
    pub data: Data,
}

pub(crate) enum Data {
    /// A baseline or progressive JPEG, grey or colour, passed through.
    Jpeg { bytes: Vec<u8>, gray: bool },
    /// Compressed RGB samples, and compressed alpha when the picture has any.
    Raw { rgb: Vec<u8>, alpha: Option<Vec<u8>> },
}

/// The pictures of a document, each decoded once.
#[derive(Default)]
pub(crate) struct Pictures {
    pub list: Vec<Picture>,
    by_hash: HashMap<u64, Option<usize>>,
    pixels: u64,
}

impl Pictures {
    /// The picture of `bytes`, decoded and kept, with its size in pixels; `None` when it isn't a
    /// picture this can read, or when the caps are reached.
    pub fn add(&mut self, bytes: &[u8]) -> Option<(usize, u32, u32)> {
        let key = hash(bytes);
        if let Some(known) = self.by_hash.get(&key) {
            return known.and_then(|i| self.list.get(i).map(|p| (i, p.width, p.height)));
        }
        let made = if self.list.len() >= MAX_PICTURES { None } else { self.decode(bytes) };
        let index = made.map(|p| {
            self.pixels = self.pixels.saturating_add(u64::from(p.width) * u64::from(p.height));
            self.list.push(p);
            self.list.len() - 1
        });
        self.by_hash.insert(key, index);
        index.and_then(|i| self.list.get(i).map(|p| (i, p.width, p.height)))
    }

    fn decode(&self, bytes: &[u8]) -> Option<Picture> {
        let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
        let format = reader.format()?;
        let (width, height) = reader.into_dimensions().ok()?;
        let pixels = u64::from(width) * u64::from(height);
        if width == 0 || height == 0 || pixels > MAX_PIXELS || self.pixels.saturating_add(pixels) > MAX_TOTAL_PIXELS {
            return None;
        }
        let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
        let mut limits = Limits::default();
        limits.max_image_width = Some(width);
        limits.max_image_height = Some(height);
        limits.max_alloc = Some(pixels.saturating_mul(8).saturating_add(1 << 20));
        reader.limits(limits);
        let image = reader.decode().ok()?;
        if format == ImageFormat::Jpeg {
            use image::ColorType;
            match image.color() {
                ColorType::L8 => return Some(Picture { width, height, data: Data::Jpeg { bytes: bytes.to_vec(), gray: true } }),
                ColorType::Rgb8 => return Some(Picture { width, height, data: Data::Jpeg { bytes: bytes.to_vec(), gray: false } }),
                _ => {}
            }
        }
        let rgba = image.to_rgba8();
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
}

/// `data` compressed for a `FlateDecode` stream.
pub(crate) fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    if encoder.write_all(data).is_err() {
        return Vec::new();
    }
    encoder.finish().unwrap_or_default()
}

fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}
