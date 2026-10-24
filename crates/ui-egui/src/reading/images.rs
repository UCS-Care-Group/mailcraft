// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures inside a message: the ones attached to it (`cid:`) and, when the owner asked for
//! them, the ones it refers to on the web; decoded and kept as textures.
//!
//! Nothing here goes and gets anything: a picture is the bytes of an attachment of the message
//! being read, or bytes the host downloaded because the owner asked ([`super::remote`]) and
//! that are kept here by their address for as long as the program runs, or it is a placeholder.
//! Either way the bytes are hostile, so everything is capped: how large a file is looked at, how
//! large a picture is decoded, how much memory decoding may take, how large a texture is made,
//! how many are kept. A picture that is too large, damaged or of a kind that isn't decoded (PNG,
//! JPEG, GIF and WebP are; one that moves shows its first frame) is a placeholder, not an error.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mailcraft_model::{Attachment, Message};

/// Largest attached picture that is looked at, in bytes.
pub const MAX_PICTURE_BYTES: usize = 24 * 1024 * 1024;

/// Longest side of a picture that is decoded, in pixels.
pub const MAX_PICTURE_SIDE: u32 = 16_384;

/// Most pixels of a picture that is decoded.
pub const MAX_PICTURE_PIXELS: u64 = 48_000_000;

/// Most memory decoding one picture may take, in bytes.
const MAX_DECODE_BYTES: u64 = 320 * 1024 * 1024;

/// Longest side of a texture, in pixels (less where the graphics card allows less).
pub const MAX_TEXTURE_SIDE: u32 = 8_192;

/// Most pixels of a texture; a larger picture is scaled down to this.
pub const MAX_TEXTURE_PIXELS: u64 = 8_000_000;

/// Most pictures kept, and the most memory their textures may take, in bytes. The least recently
/// shown go first.
pub const CACHE_PICTURES: usize = 64;
pub const CACHE_BYTES: usize = 160 * 1024 * 1024;

/// Most different pictures of one message that are shown; the rest are placeholders.
pub const MAX_PICTURES_PER_MESSAGE: usize = 200;

/// Most pictures from the web that are kept, and the most their bytes (as they came) may take.
/// The least recently shown go first. Nothing of them is written to disk from here (the host
/// may keep what it downloaded, for the next start: `Services::picture_cache`).
pub const REMOTE_PICTURES: usize = 300;
pub const REMOTE_BYTES: usize = 96 * 1024 * 1024;

/// The frame the window is at: the main window's count, whichever window is being laid out, so
/// that what was shown last can be told in all of them alike.
pub fn frame(ctx: &egui::Context) -> u64 {
    ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT)
}

/// Whether a picture's address is one on the web (`http://` or `https://`): the only kind that
/// is ever asked for.
pub fn is_remote(src: &str) -> bool {
    let src = src.trim_start();
    ["http://", "https://"].iter().any(|scheme| src.get(..scheme.len()).is_some_and(|start| start.eq_ignore_ascii_case(scheme)))
}

/// The attachment a `cid:` address refers to. The address is compared with each part's
/// `Content-ID` as written, then with `%xx` decoded, then without regard to letter case.
pub fn attachment<'a>(message: &'a Message, src: &str) -> Option<&'a Attachment> {
    let src = src.trim();
    let id = src.get(..4).filter(|scheme| scheme.eq_ignore_ascii_case("cid:")).and_then(|_| src.get(4..))?;
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    if id.is_empty() {
        return None;
    }
    let decoded = percent_decode(id);
    let parts = || message.attachments.iter().filter(|a| !a.content_id.is_empty());
    parts()
        .find(|a| a.content_id == id)
        .or_else(|| parts().find(|a| a.content_id == decoded))
        .or_else(|| parts().find(|a| a.content_id.eq_ignore_ascii_case(id) || a.content_id.eq_ignore_ascii_case(&decoded)))
}

/// `%40` and the like decoded; anything that isn't a valid escape, or doesn't give UTF-8, is left.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let escape = (b == b'%').then(|| text.get(i + 1..i + 3)).flatten().and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escape {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// The kinds of picture shown, by what their bytes are: in a message (and in a signature), and
/// for a sender, whose picture may be a web site's icon as well ([`crate::sender_images`]).
pub(crate) const KINDS: &[image::ImageFormat] =
    &[image::ImageFormat::Png, image::ImageFormat::Jpeg, image::ImageFormat::Gif, image::ImageFormat::WebP];
const ICON_KINDS: &[image::ImageFormat] =
    &[image::ImageFormat::Png, image::ImageFormat::Jpeg, image::ImageFormat::Gif, image::ImageFormat::WebP, image::ImageFormat::Ico];

/// The reader of a picture's bytes, when they are one of `kinds` and within the caps.
fn reader<'a>(data: &'a [u8], kinds: &[image::ImageFormat]) -> Option<image::ImageReader<std::io::Cursor<&'a [u8]>>> {
    if data.len() > MAX_PICTURE_BYTES {
        return None;
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format().ok()?;
    // By what the bytes are, never by what their name or their server says they are.
    if !kinds.contains(&reader.format()?) {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PICTURE_SIDE);
    limits.max_image_height = Some(MAX_PICTURE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    Some(reader)
}

/// A picture's size in pixels, from its header alone. `None` for bytes that are not a picture
/// that is decoded here, and for one over the caps.
pub fn probe(data: &[u8]) -> Option<[u32; 2]> {
    probe_as(data, KINDS)
}

/// [`probe`], for a sender's picture: a web site's icon (ICO) too.
pub fn probe_icon(data: &[u8]) -> Option<[u32; 2]> {
    probe_as(data, ICON_KINDS)
}

fn probe_as(data: &[u8], kinds: &[image::ImageFormat]) -> Option<[u32; 2]> {
    // A decoder is somebody else's code reading hostile bytes: should it panic, that is a damaged picture.
    let size = std::panic::catch_unwind(|| reader(data, kinds).and_then(|r| r.into_dimensions().ok())).ok().flatten()?;
    let (w, h) = size;
    let within = w >= 1 && h >= 1 && w <= MAX_PICTURE_SIDE && h <= MAX_PICTURE_SIDE && u64::from(w) * u64::from(h) <= MAX_PICTURE_PIXELS;
    within.then_some([w, h])
}

/// The size a picture of `size` pixels is kept at, when no side may pass `max_side` and it is
/// shown at most `max_width` pixels wide.
fn kept_size(size: [u32; 2], max_side: u32, max_width: u32) -> [u32; 2] {
    let [w, h] = size.map(|v| f64::from(v.max(1)));
    let max_side = f64::from(max_side.clamp(1, MAX_TEXTURE_SIDE));
    let scale = (f64::from(max_width.max(1)) / w).min(max_side / w).min(max_side / h).min((MAX_TEXTURE_PIXELS as f64 / (w * h)).sqrt()).min(1.0);
    [(w * scale).round().max(1.0) as u32, (h * scale).round().max(1.0) as u32]
}

/// Decode a picture, scaled down to fit a texture. `None` when it cannot be decoded.
pub fn decode(data: &[u8], max_side: u32, max_width: u32) -> Option<egui::ColorImage> {
    decode_as(data, max_side, max_width, KINDS)
}

/// [`decode`], for a sender's picture: a web site's icon (ICO) too.
pub fn decode_icon(data: &[u8], max_side: u32) -> Option<egui::ColorImage> {
    decode_as(data, max_side, max_side, ICON_KINDS)
}

fn decode_as(data: &[u8], max_side: u32, max_width: u32, kinds: &[image::ImageFormat]) -> Option<egui::ColorImage> {
    let size = probe_as(data, kinds)?;
    let decoded = std::panic::catch_unwind(|| reader(data, kinds).and_then(|r| r.decode().ok())).ok().flatten()?;
    let rgba = decoded.into_rgba8();
    let [w, h] = kept_size(size, max_side, max_width);
    let rgba = if [w, h] == [rgba.width(), rgba.height()] { rgba } else { image::imageops::thumbnail(&rgba, w, h) };
    let size = [usize::try_from(rgba.width()).ok()?, usize::try_from(rgba.height()).ok()?];
    (rgba.as_raw().len() == size[0].checked_mul(size[1])?.checked_mul(4)?).then(|| egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// What is known of one attachment as a picture.
struct Entry {
    /// The bytes. Kept so that the address they are at stays theirs while the entry lives.
    data: Arc<Vec<u8>>,
    /// Its size in pixels; `None` when it is not a picture that can be shown.
    size: Option<[u32; 2]>,
    texture: Option<egui::TextureHandle>,
    /// It was tried and could not be decoded.
    failed: bool,
    /// The frame it was last asked for.
    used: u64,
}

impl Entry {
    fn bytes(&self) -> usize {
        self.texture.as_ref().map_or(0, |t| t.size().iter().product::<usize>().saturating_mul(4))
    }
}

/// A picture, as far as it has come.
pub enum Picture {
    Ready(egui::TextureHandle),
    /// Not decoded yet: at most one picture is decoded in a frame, so that a message full of them
    /// opens at once and fills in.
    Pending,
    /// It cannot be shown.
    Failed,
}

/// What became of a picture from the web that was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Remote {
    /// Its bytes, as the host brought them.
    Ready(Arc<Vec<u8>>),
    /// It could not be had, or what came was not a picture: why, in words that read on after
    /// "couldn't be downloaded:" ([`reason`] makes them fit to show). It is not asked for again
    /// unless the owner says "Try again".
    Failed(Arc<str>),
}

struct RemoteEntry {
    state: Remote,
    /// The frame it was last asked about by a message on screen.
    used: u64,
}

impl RemoteEntry {
    fn bytes(&self) -> usize {
        match &self.state {
            Remote::Ready(data) => data.len(),
            Remote::Failed(why) => why.len(),
        }
    }
}

/// Longest reason kept for a picture that failed, in characters.
const MAX_REASON: usize = 160;

/// A reason for a failure as it is kept and shown: one line, no control characters, not long.
/// The words may be a server's (the host hands on what it was told), so they are as hostile as
/// anything else that comes from outside.
pub fn reason(text: &str) -> Arc<str> {
    let words: Vec<&str> = text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    let line = words.join(" ");
    let line = line.trim_end_matches(['.', ':', ' ']);
    if line.is_empty() {
        return Arc::from(tl!("the download failed"));
    }
    if line.chars().count() <= MAX_REASON { Arc::from(line) } else { line.chars().take(MAX_REASON).chain(['…']).collect::<String>().into() }
}

/// Why bytes that came are not shown as a picture: what they look like instead. For the owner,
/// who asked for a picture and sees a stand-in.
pub fn not_a_picture(data: &[u8]) -> &'static str {
    let starts = |magic: &[u8]| data.starts_with(magic);
    let text = || {
        let head = String::from_utf8_lossy(data.get(..data.len().min(600)).unwrap_or(&[])).to_ascii_lowercase();
        (head.trim_start().starts_with('<') || head.contains("<html"), head.contains("<svg"))
    };
    if data.is_empty() {
        tl!("the server sent nothing")
    } else if data.len() > MAX_PICTURE_BYTES {
        tl!("it is too large to show")
    } else if data.get(4..8) == Some(&b"ftyp"[..]) {
        tl!("it is a kind of picture that can't be shown here (AVIF or HEIC)")
    } else if starts(b"\x89PNG") || starts(b"\xff\xd8\xff") || starts(b"GIF8") || (starts(b"RIFF") && data.get(8..12) == Some(&b"WEBP"[..])) {
        tl!("it is damaged, or larger than can be shown")
    } else if starts(b"BM") || starts(b"\0\0\x01\0") || starts(b"II*\0") || starts(b"MM\0*") {
        tl!("it is a kind of picture that can't be shown here")
    } else {
        match text() {
            (_, true) => tl!("it is a drawing of a kind that isn't shown (SVG)"),
            (true, false) => tl!("the server sent a web page instead of a picture"),
            _ => tl!("what the server sent is not a picture"),
        }
    }
}

/// The pictures of the messages read lately, decoded.
#[derive(Default)]
pub struct Pictures {
    entries: Vec<Entry>,
    /// The frame in which a picture was last decoded.
    decoded_in: Option<u64>,
    /// Pictures from the web, by their address as the message wrote it.
    remote: HashMap<Arc<str>, RemoteEntry>,
}

impl Pictures {
    /// The window's pictures.
    pub fn of(ctx: &egui::Context) -> Arc<Mutex<Pictures>> {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<Mutex<Pictures>>>(egui::Id::unique("mailcraft-inline-pictures")).clone())
    }

    fn entry(&mut self, data: &Arc<Vec<u8>>, frame: u64) -> Option<&mut Entry> {
        let index = match self.entries.iter().position(|e| Arc::ptr_eq(&e.data, data)) {
            Some(index) => index,
            None => {
                self.entries.push(Entry { data: data.clone(), size: probe(data), texture: None, failed: false, used: frame });
                self.entries.len() - 1
            }
        };
        let entry = self.entries.get_mut(index)?;
        entry.used = frame;
        Some(entry)
    }

    /// The size in pixels of an attachment that is a picture; `None` when it cannot be shown.
    pub fn size(&mut self, data: &Arc<Vec<u8>>, frame: u64) -> Option<[u32; 2]> {
        let size = self.entry(data, frame).and_then(|e| e.size);
        self.trim(frame);
        size
    }

    /// What became of the picture at `url`, when it was asked for; a message on screen is asking.
    pub fn remote(&mut self, url: &str, frame: u64) -> Option<Remote> {
        let entry = self.remote.get_mut(url)?;
        entry.used = frame;
        Some(entry.state.clone())
    }

    /// The same, for the curious: it does not count as being shown.
    pub fn remote_peek(&self, url: &str) -> Option<&Remote> {
        self.remote.get(url).map(|entry| &entry.state)
    }

    /// Forget what became of the picture at `url` when it failed, so that it is asked for
    /// again; whether there was such a failure to forget.
    pub fn forget_failure(&mut self, url: &str) -> bool {
        let failed = matches!(self.remote.get(url), Some(RemoteEntry { state: Remote::Failed(_), .. }));
        if failed {
            self.remote.remove(url);
        }
        failed
    }

    /// Forget every picture from the web that is kept: they are asked for again when wanted.
    pub fn forget_remote(&mut self) {
        self.remote.clear();
    }

    /// Keep what became of the picture at `url`.
    pub fn set_remote(&mut self, url: Arc<str>, state: Remote, frame: u64) {
        self.remote.insert(url, RemoteEntry { state, used: frame });
        // Forget the ones asked about longest ago while there are too many or they take too much.
        loop {
            let bytes: usize = self.remote.values().map(RemoteEntry::bytes).sum();
            if self.remote.len() <= REMOTE_PICTURES && bytes <= REMOTE_BYTES {
                return;
            }
            let oldest = self.remote.iter().filter(|(_, e)| e.used < frame).min_by_key(|(_, e)| e.used).map(|(url, _)| url.clone());
            match oldest {
                Some(url) => drop(self.remote.remove(&url)),
                // Everything left is on screen now.
                None => return,
            }
        }
    }

    /// The texture of a picture, decoding it when no other picture was decoded in this frame.
    pub fn texture(&mut self, ctx: &egui::Context, data: &Arc<Vec<u8>>) -> Picture {
        let frame = frame(ctx);
        let busy = self.decoded_in == Some(frame);
        let Some(entry) = self.entry(data, frame) else { return Picture::Failed };
        if let Some(texture) = &entry.texture {
            return Picture::Ready(texture.clone());
        }
        if entry.failed || entry.size.is_none() {
            return Picture::Failed;
        }
        if busy {
            return Picture::Pending;
        }
        let max_side = u32::try_from(ctx.input(|i| i.max_texture_side)).unwrap_or(MAX_TEXTURE_SIDE);
        // Never wider than the widest the reading pane shows a picture, in physical pixels.
        let max_width = (super::MAX_BODY_WIDTH * ctx.pixels_per_point()).ceil().clamp(1.0, MAX_TEXTURE_SIDE as f32) as u32;
        let picture = match decode(&entry.data, max_side, max_width) {
            Some(image) => {
                let texture = ctx.load_texture("mailcraft-inline-picture", image, egui::TextureOptions::LINEAR);
                entry.texture = Some(texture.clone());
                Picture::Ready(texture)
            }
            None => {
                entry.failed = true;
                Picture::Failed
            }
        };
        self.decoded_in = Some(frame);
        self.trim(frame);
        picture
    }

    /// Forget the pictures shown longest ago while there are too many or they take too much.
    fn trim(&mut self, frame: u64) {
        loop {
            let bytes: usize = self.entries.iter().map(Entry::bytes).sum();
            if self.entries.len() <= CACHE_PICTURES && bytes <= CACHE_BYTES {
                return;
            }
            let oldest = self.entries.iter().enumerate().filter(|(_, e)| e.used < frame).min_by_key(|(_, e)| e.used).map(|(i, _)| i);
            match oldest {
                Some(index) => drop(self.entries.swap_remove(index)),
                // Everything left is on screen now.
                None => return,
            }
        }
    }

    /// How many pictures are kept, and how many of them are decoded.
    #[cfg(test)]
    pub fn counts(&self) -> (usize, usize) {
        (self.entries.len(), self.entries.iter().filter(|e| e.texture.is_some()).count())
    }

    /// How many pictures from the web are kept, and how many bytes of them.
    #[cfg(test)]
    pub fn remote_counts(&self) -> (usize, usize) {
        (self.remote.len(), self.remote.values().map(RemoteEntry::bytes).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reading::tests::{gif, jpeg, png, webp};

    #[test]
    fn pictures_are_found_by_their_content_id() {
        let part = |id: &str| Attachment { content_id: id.into(), inline: true, ..Attachment::new("x.png", "image/png", vec![1]) };
        let m = Message {
            attachments: vec![Attachment::new("plain.txt", "text/plain", vec![2]), part("logo@example.org"), part("Image 2@X")],
            ..Message::default()
        };
        let found = |src: &str| attachment(&m, src).map(|a| a.content_id.as_str());
        assert_eq!(found("cid:logo@example.org"), Some("logo@example.org"));
        assert_eq!(found(" CID:logo@example.org "), Some("logo@example.org"));
        assert_eq!(found("cid:logo%40example.org"), Some("logo@example.org"));
        assert_eq!(found("cid:<logo@example.org>"), Some("logo@example.org"));
        assert_eq!(found("cid:Image%202@x"), Some("Image 2@X"));
        // Anything that isn't `cid:` is nothing of this message's, whatever it looks like.
        for remote in
            ["https://example.org/logo@example.org", "logo@example.org", "cid:", "cid:other", "", "file:logo@example.org", "cid:%ZZ", "cid:%"]
        {
            assert_eq!(found(remote), None, "{remote:?}");
        }
        // A part without a Content-ID is never matched by an empty address.
        assert_eq!(attachment(&m, "cid:"), None);
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%ff%fe"), "%ff%fe");
    }

    #[test]
    fn png_jpeg_gif_and_webp_are_decoded() {
        for (name, bytes) in [("png", png(64, 40)), ("jpeg", jpeg(64, 40)), ("gif", gif(64, 40)), ("webp", webp(64, 40))] {
            assert_eq!(probe(&bytes), Some([64, 40]), "{name}");
            let image = decode(&bytes, 2048, 2048).unwrap_or_else(|| panic!("{name} was not decoded"));
            assert_eq!(image.size, [64, 40], "{name}");
            // The colours are the picture's own (a point in the drawing's title bar), for the
            // kinds that keep every pixel as it was.
            if matches!(name, "png" | "webp") {
                assert_eq!(image.pixels.get(3 * 64 + 20).map(|c| c.to_array()), Some([0x1f, 0x5f, 0x8b, 0xff]), "{name}");
            }
        }
        // A picture is told by what its bytes are, whatever it was called on the way.
        assert!(webp(8, 8).starts_with(b"RIFF") && webp(8, 8).get(8..12) == Some(&b"WEBP"[..]));
    }

    #[test]
    fn what_is_said_of_bytes_that_are_no_picture() {
        let said = |bytes: &[u8]| not_a_picture(bytes);
        assert_eq!(said(b""), "the server sent nothing");
        assert_eq!(said(b"<!DOCTYPE html><html><body>Not found</body></html>"), "the server sent a web page instead of a picture");
        assert_eq!(said(b"\r\n  <HTML><head>"), "the server sent a web page instead of a picture");
        assert_eq!(said(b"<?xml version='1.0'?><svg xmlns='http://www.w3.org/2000/svg'/>"), "it is a drawing of a kind that isn't shown (SVG)");
        assert_eq!(said(b"<svg viewBox='0 0 1 1'></svg>"), "it is a drawing of a kind that isn't shown (SVG)");
        assert_eq!(said(b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf"), "it is a kind of picture that can't be shown here (AVIF or HEIC)");
        assert_eq!(said(b"BM\x46\0\0\0\0\0\0\0\x36\0\0\0"), "it is a kind of picture that can't be shown here");
        assert_eq!(said(b"\0\0\x01\0\x01\0\x10\x10"), "it is a kind of picture that can't be shown here");
        assert_eq!(said(b"\x89PNG\r\n\x1a\n"), "it is damaged, or larger than can be shown");
        assert_eq!(said(b"RIFF\x04\0\0\0WEBP"), "it is damaged, or larger than can be shown");
        assert_eq!(said(b"{\"error\":\"not found\"}"), "what the server sent is not a picture");
        assert_eq!(said(&[0xff; 9]), "what the server sent is not a picture");
        // A reason is kept as one line of no great length, whoever wrote it.
        assert_eq!(&*reason("the server answered 404 instead of a picture"), "the server answered 404 instead of a picture");
        assert_eq!(&*reason("  it is\r\n no longer\tthere (404). \u{7}"), "it is no longer there (404)");
        assert_eq!(&*reason(" \n\u{0} "), "the download failed");
        let long = reason(&"é ".repeat(500));
        assert_eq!(long.chars().count(), MAX_REASON + 1);
        assert!(long.ends_with('…') && !long.contains('\n'));
    }

    #[test]
    fn large_pictures_are_scaled_to_fit_a_texture() {
        assert_eq!(kept_size([4000, 3000], 8192, 1720), [1720, 1290]);
        assert_eq!(kept_size([1200, 9000], 2048, 1720), [273, 2048]);
        assert_eq!(kept_size([100, 50], 2048, 1720), [100, 50]);
        assert_eq!(kept_size([16_000, 16_000], 8192, 8192), [2828, 2828]);
        assert_eq!(kept_size([1, 16_000], 0, 0), [1, 1]);
        assert_eq!(kept_size([0, 0], 2048, 2048), [1, 1]);
        let image = decode(&png(600, 300), 2048, 200).unwrap();
        assert_eq!(image.size, [200, 100]);
        let image = decode(&jpeg(300, 900), 256, 4000).unwrap();
        assert_eq!(image.size, [85, 256]);
    }

    #[test]
    fn what_is_not_a_picture_is_not_decoded() {
        let png = png(16, 16);
        let truncated = png.get(..png.len() / 2).unwrap();
        for (what, bytes) in [
            ("nothing", &b""[..]),
            ("text", b"just some text, not a picture at all"),
            ("a signature and nothing else", b"\x89PNG\r\n\x1a\n"),
            ("svg", b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>"),
            ("html", b"<html><img src=https://example.org/x.png></html>"),
            ("bmp", b"BM\x46\0\0\0\0\0\0\0\x36\0\0\0\x28\0\0\0\x02\0\0\0\x02\0\0\0\x01\0\x18\0\0\0\0\0\x10\0\0\0"),
            ("avif", b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf\0\0\0\xeameta"),
            ("ico", b"\0\0\x01\0\x01\0\x10\x10\0\0\x01\0\x20\0\x68\x04\0\0\x16\0\0\0"),
            ("a webp signature and nothing else", b"RIFF\x04\0\0\0WEBP"),
        ] {
            assert!(decode(bytes, 2048, 2048).is_none(), "{what} was decoded");
        }
        // The header of a cut-off picture still says how large it is; decoding it fails quietly.
        assert_eq!(probe(truncated), Some([16, 16]));
        assert!(decode(truncated, 2048, 2048).is_none());
    }

    /// A PNG header that claims a size, with nothing behind it.
    fn png_claiming(width: u32, height: u32) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        out.extend(width.to_be_bytes());
        out.extend(height.to_be_bytes());
        out.extend([8, 6, 0, 0, 0]);
        // The CRC of the header chunk, then an empty data chunk.
        let crc = crc32(out.get(12..).unwrap());
        out.extend(crc.to_be_bytes());
        out.extend(b"\0\0\0\0IDAT\x35\xaf\x06\x1e\0\0\0\0IEND\xae\x42\x60\x82");
        out
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for b in bytes {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    #[test]
    fn absurd_sizes_are_refused_before_anything_is_allocated() {
        assert_eq!(probe(&png_claiming(64, 64)), Some([64, 64]));
        for (w, h) in
            [(60_000, 60_000), (u32::MAX, u32::MAX), (MAX_PICTURE_SIDE + 1, 1), (1, MAX_PICTURE_SIDE + 1), (16_000, 16_000), (0, 10), (10, 0)]
        {
            let bytes = png_claiming(w, h);
            assert_eq!(probe(&bytes), None, "{w}x{h}");
            assert!(decode(&bytes, 8192, 8192).is_none(), "{w}x{h}");
        }
        // A size within the caps with no pixels behind it: refused when decoded, not before.
        let empty = png_claiming(6000, 6000);
        assert_eq!(probe(&empty), Some([6000, 6000]));
        assert!(decode(&empty, 8192, 8192).is_none());
        // Too many bytes to look at.
        let mut huge = png(8, 8);
        huge.resize(MAX_PICTURE_BYTES + 1, 0);
        assert_eq!(probe(&huge), None);
    }

    /// A small generator of the same numbers every run (xorshift): enough to throw junk at a decoder.
    struct Junk(u64);

    impl Junk {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        let mut junk = Junk(0x9e37_79b9_7f4a_7c15);
        for round in 0..400 {
            // Junk behind each kind's signature gets furthest into its decoder.
            let mut data = match round % 6 {
                0 => b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec(),
                1 => b"\xff\xd8\xff\xe0\0\x10JFIF\0".to_vec(),
                2 => b"GIF89a".to_vec(),
                3 => b"RIFF\xf0\x03\0\0WEBPVP8L".to_vec(),
                4 => b"RIFF\xf0\x03\0\0WEBPVP8 ".to_vec(),
                _ => Vec::new(),
            };
            let len = junk.below(600);
            data.extend((0..len).map(|_| junk.next() as u8));
            let _ = probe(&data);
            if let Some(image) = decode(&data, 512, 512) {
                assert!(image.size[0] <= 512 && image.size[1] <= 512);
            }
        }
    }

    #[test]
    fn damaged_pictures_never_panic() {
        let mut junk = Junk(0x2545_f491_4f6c_dd1d);
        let originals = [png(24, 16), jpeg(24, 16), gif(24, 16), webp(24, 16)];
        for round in 0..800 {
            let mut data = originals[round % 4].clone();
            // A few bytes changed, and sometimes the end cut off.
            for _ in 0..1 + junk.below(4) {
                let at = junk.below(data.len());
                data[at] = junk.next() as u8;
            }
            if round % 2 == 0 {
                let keep = junk.below(data.len());
                data.truncate(keep);
            }
            let _ = probe(&data);
            if let Some(image) = decode(&data, 512, 512) {
                assert!(image.size[0] <= 512 && image.size[1] <= 512);
            }
        }
    }
}
