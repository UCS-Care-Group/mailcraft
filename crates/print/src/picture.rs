//! Pictures a message carries itself (`cid:`), for printing: PNG and JPEG only, and only within
//! limits. Anything else (another format, a picture too large, bytes that don't decode) is
//! printed as a placeholder. Nothing is ever fetched.

use std::io::Cursor;
use std::sync::Arc;

/// The largest picture file printed.
pub const MAX_BYTES: usize = 8 << 20;
/// The most pixels a printed picture has.
pub const MAX_PIXELS: u64 = 4096 * 4096;
/// The most pictures one print shows; the rest are placeholders.
pub const MAX_PICTURES: usize = 40;

/// A picture's pixels as the PDF takes them.
#[derive(Clone, Debug, PartialEq)]
pub enum Pixels {
    /// A JPEG file as it is (`DCTDecode`), with 1 (grey) or 3 (colour) components.
    Jpeg { data: Arc<Vec<u8>>, components: u8 },
    /// Eight bits per sample: grey (1) or RGB (3) samples per pixel, and an alpha plane.
    Raw { samples: Vec<u8>, gray: bool, alpha: Option<Vec<u8>> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
}

/// The picture in these bytes, if they are a PNG or JPEG that can be printed.
pub fn decode(bytes: &[u8]) -> Option<Picture> {
    if bytes.len() > MAX_BYTES {
        return None;
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return jpeg(bytes);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png(bytes);
    }
    None
}

fn fits(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_PIXELS
}

/// A JPEG's size and components from its frame header; the file itself goes into the PDF.
fn jpeg(bytes: &[u8]) -> Option<Picture> {
    let mut at = 2usize;
    for _ in 0..10_000 {
        // Fill bytes between segments.
        while bytes.get(at) == Some(&0xFF) && bytes.get(at + 1) == Some(&0xFF) {
            at += 1;
        }
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        at += 2;
        if matches!(marker, 0xD0..=0xD9 | 0x01) {
            continue;
        }
        let len = usize::from(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]));
        if len < 2 {
            return None;
        }
        match marker {
            // Baseline, extended and progressive Huffman frames: what PDF readers decode.
            0xC0..=0xC2 => {
                let height = u32::from(u16::from_be_bytes([*bytes.get(at + 3)?, *bytes.get(at + 4)?]));
                let width = u32::from(u16::from_be_bytes([*bytes.get(at + 5)?, *bytes.get(at + 6)?]));
                let components = *bytes.get(at + 7)?;
                if !fits(width, height) || !matches!(components, 1 | 3) {
                    return None;
                }
                return Some(Picture { width, height, pixels: Pixels::Jpeg { data: Arc::new(bytes.to_vec()), components } });
            }
            // Other frame kinds (arithmetic, lossless, hierarchical): not printed.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            // Start of scan before a frame: not a JPEG this reads.
            0xDA => return None,
            _ => at = at.checked_add(len)?,
        }
    }
    None
}

fn png(bytes: &[u8]) -> Option<Picture> {
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: 80 << 20 });
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let (width, height) = reader.info().size();
    if !fits(width, height) {
        return None;
    }
    let size = reader.output_buffer_size()?;
    if size > (MAX_PIXELS as usize).saturating_mul(4) {
        return None;
    }
    let mut buf = vec![0; size];
    let frame = reader.next_frame(&mut buf).ok()?;
    let data = buf.get(..frame.buffer_size())?;
    let pixels = (width as usize).checked_mul(height as usize)?;
    let (samples, gray, alpha) = match frame.color_type {
        png::ColorType::Grayscale => (data.get(..pixels)?.to_vec(), true, None),
        png::ColorType::Rgb => (data.get(..pixels * 3)?.to_vec(), false, None),
        png::ColorType::GrayscaleAlpha => {
            let d = data.get(..pixels * 2)?;
            (d.iter().step_by(2).copied().collect(), true, Some(d.iter().skip(1).step_by(2).copied().collect::<Vec<u8>>()))
        }
        png::ColorType::Rgba => {
            let d = data.get(..pixels * 4)?;
            let mut rgb = Vec::with_capacity(pixels * 3);
            let mut a = Vec::with_capacity(pixels);
            for [r, g, b, alpha] in d.as_chunks::<4>().0 {
                rgb.extend_from_slice(&[*r, *g, *b]);
                a.push(*alpha);
            }
            (rgb, false, Some(a))
        }
        png::ColorType::Indexed => return None,
    };
    // An alpha plane that is opaque everywhere is no alpha plane.
    let alpha = alpha.filter(|a: &Vec<u8>| a.iter().any(|v| *v != 255));
    Some(Picture { width, height, pixels: Pixels::Raw { samples, gray, alpha } })
}
