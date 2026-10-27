// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Making a picture put in a message fit (#547): one wider than [`MAX_SHOWN_WIDTH`] is scaled
//! down to it, and one still over the cap is saved again smaller (PNG; JPEG as well when it has no
//! transparency and PNG is too big), at a smaller width each try, until it fits.
//!
//! A picture that moves (an animated GIF or WebP) is never flattened: it is kept as it is, and
//! the caps refuse it when it is over them. A picture that is no PNG, JPEG, GIF or WebP by its
//! bytes, or that the decoder won't read within its limits, is returned as it is, for the caps
//! to refuse in words.

use std::io::Cursor;

use image::{ImageDecoder as _, ImageEncoder as _, ImageFormat};
use mailcraft_model::pictures::{MAX_PICTURE_BYTES, MAX_SHOWN_WIDTH};

/// Most bytes of a picture that are read to be made smaller; more is left for the caps to refuse.
const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
/// Most width and height a picture may decode from, and most memory it may take doing so.
const MAX_SIDE: u32 = 16_384;
const MAX_ALLOC: u64 = 256 * 1024 * 1024;
/// Most tries at a smaller size.
const TRIES: usize = 8;

/// `bytes` as they go in the message, and the extension that names them (`None`: unchanged).
pub fn fit(bytes: Vec<u8>) -> (Vec<u8>, Option<&'static str>) {
    match shrink(&bytes) {
        Some(done) => (done.0, Some(done.1)),
        None => (bytes, None),
    }
}

/// The smaller picture, when it needed to be and could be.
fn shrink(bytes: &[u8]) -> Option<(Vec<u8>, &'static str)> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let format = reader.format()?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP) {
        return None;
    }
    let mut decoder = reader.into_decoder().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    decoder.set_limits(limits).ok()?;
    let (w, h) = decoder.dimensions();
    if bytes.len() <= MAX_PICTURE_BYTES && w <= u32::from(MAX_SHOWN_WIDTH) {
        return None;
    }
    if moves(bytes, format) {
        return None;
    }
    let img = image::DynamicImage::from_decoder(decoder).ok()?;
    let opaque = !img.color().has_alpha();
    let mut width = w.min(u32::from(MAX_SHOWN_WIDTH));
    for _ in 0..TRIES {
        if width < 16 {
            return None;
        }
        let height = u32::try_from(u64::from(h) * u64::from(width) / u64::from(w.max(1))).ok()?.max(1);
        let small = if width == w { img.clone() } else { img.resize_exact(width, height, image::imageops::FilterType::Triangle) };
        let mut png = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut png);
        if opaque {
            encoder.write_image(small.to_rgb8().as_raw(), width, height, image::ExtendedColorType::Rgb8).ok()?;
        } else {
            encoder.write_image(small.to_rgba8().as_raw(), width, height, image::ExtendedColorType::Rgba8).ok()?;
        }
        if png.len() <= MAX_PICTURE_BYTES {
            return Some((png, "png"));
        }
        if opaque {
            let mut jpeg = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
                .write_image(small.to_rgb8().as_raw(), width, height, image::ExtendedColorType::Rgb8)
                .ok()?;
            if jpeg.len() <= MAX_PICTURE_BYTES {
                return Some((jpeg, "jpg"));
            }
        }
        width = width * 2 / 3;
    }
    None
}

/// Whether the picture has more than one frame.
fn moves(bytes: &[u8], format: ImageFormat) -> bool {
    use image::AnimationDecoder as _;
    match format {
        ImageFormat::Gif => image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).is_ok_and(|d| d.into_frames().take(2).count() > 1),
        ImageFormat::WebP => image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).is_ok_and(|d| d.has_animation()),
        _ => false,
    }
}
