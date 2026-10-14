//! A [`Document`] as a PDF (1.7): text in the program's own fonts, embedded as subsets
//! (TrueType outlines, `Identity-H`, with a `ToUnicode` map so the text can be searched and
//! copied), lines, boxes and pictures. Streams are compressed. The same document gives the same
//! bytes every time: nothing of the time or the machine is written.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write as _;

use crate::font::{self, FontKind};
use crate::picture::Pixels;
use crate::{Document, Item, Rgb};

struct Writer {
    buf: Vec<u8>,
    /// Where each object starts, by number (0 is the free head).
    offsets: Vec<usize>,
}

impl Writer {
    fn new() -> Writer {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
        Writer { buf, offsets: vec![0] }
    }

    fn reserve(&mut self) -> usize {
        self.offsets.push(0);
        self.offsets.len() - 1
    }

    fn obj(&mut self, n: usize, body: &str) {
        self.start(n);
        self.buf.extend_from_slice(body.as_bytes());
        self.buf.extend_from_slice(b"\nendobj\n");
    }

    fn start(&mut self, n: usize) {
        if let Some(o) = self.offsets.get_mut(n) {
            *o = self.buf.len();
        }
        self.buf.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
    }

    /// A stream object; `dict` is what goes in its dictionary besides the length and filter.
    fn stream(&mut self, n: usize, dict: &str, data: &[u8], compress: bool) {
        let packed = if compress { deflate(data) } else { None };
        let (bytes, filter) = match &packed {
            Some(p) => (p.as_slice(), " /Filter /FlateDecode"),
            None => (data, ""),
        };
        self.start(n);
        self.buf.extend_from_slice(format!("<< {dict} /Length {}{filter} >>\nstream\n", bytes.len()).as_bytes());
        self.buf.extend_from_slice(bytes);
        self.buf.extend_from_slice(b"\nendstream\nendobj\n");
    }

    fn finish(mut self, root: usize, info: usize) -> Vec<u8> {
        let xref = self.buf.len();
        let mut table = format!("xref\n0 {}\n0000000000 65535 f \n", self.offsets.len());
        for o in self.offsets.iter().skip(1) {
            let _ = writeln!(table, "{o:010} 00000 n ");
        }
        let _ = write!(table, "trailer\n<< /Size {} /Root {root} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n", self.offsets.len());
        self.buf.extend_from_slice(table.as_bytes());
        self.buf
    }
}

fn deflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::with_capacity(data.len() / 2), flate2::Compression::default());
    e.write_all(data).ok()?;
    e.finish().ok()
}

/// A number as PDF writes it: at most two decimals, no exponent, never NaN.
fn num(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let v = v.clamp(-100_000.0, 100_000.0);
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.to_string() }
}

fn color(c: Rgb) -> String {
    format!("{} {} {}", num(f32::from(c.0) / 255.0), num(f32::from(c.1) / 255.0), num(f32::from(c.2) / 255.0))
}

/// Text as a PDF text string: UTF-16 with its byte-order mark, in hex.
fn text_string(s: &str) -> String {
    let mut out = String::from("<FEFF");
    for unit in s.encode_utf16().take(2000) {
        let _ = write!(out, "{unit:04X}");
    }
    out.push('>');
    out
}

/// Six capital letters naming a subset, from what is in it.
fn subset_tag(kind: FontKind, glyphs: &BTreeSet<u16>) -> String {
    let mut h: u32 = 2_166_136_261;
    for g in glyphs.iter().copied().chain(std::iter::once(kind as u16)) {
        for b in g.to_be_bytes() {
            h ^= u32::from(b);
            h = h.wrapping_mul(16_777_619);
        }
    }
    (0..6)
        .map(|i| {
            let letter = (h >> (i * 5)) % 26;
            char::from(b'A' + letter as u8)
        })
        .collect()
}

fn to_unicode(map: &BTreeMap<u16, char>) -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let entries: Vec<(&u16, &char)> = map.iter().collect();
    for block in entries.chunks(100) {
        let _ = writeln!(s, "{} beginbfchar", block.len());
        for (g, c) in block {
            let mut units = [0u16; 2];
            let hex: String = c.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect();
            let _ = writeln!(s, "<{g:04X}> <{hex}>");
        }
        s.push_str("endbfchar\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    s
}

/// The document as PDF bytes.
pub fn write(doc: &Document) -> Vec<u8> {
    let mut w = Writer::new();
    let root = w.reserve();
    let pages_root = w.reserve();
    let info = w.reserve();
    let resources = w.reserve();

    // The glyphs each font prints, and the character each stands for.
    let mut used: BTreeMap<FontKind, BTreeMap<u16, char>> = BTreeMap::new();
    for page in &doc.pages {
        for item in &page.items {
            if let Item::Text(t) = item
                && let Some(f) = font::get(t.font)
            {
                let map = used.entry(t.font).or_default();
                for c in t.text.chars() {
                    map.entry(f.glyph(c)).or_insert(c);
                }
            }
        }
    }

    // Fonts.
    let mut font_names: BTreeMap<FontKind, String> = BTreeMap::new();
    let mut font_refs = String::new();
    for (index, (kind, map)) in used.iter().enumerate() {
        let Some(f) = font::get(*kind) else { continue };
        let glyphs: BTreeSet<u16> = map.keys().copied().collect();
        let name = format!("{}+{}", subset_tag(*kind, &glyphs), kind.postscript_name());
        let (type0, cid, descriptor, file, unicode) = (w.reserve(), w.reserve(), w.reserve(), w.reserve(), w.reserve());
        let program = crate::subset::subset(f.data, &glyphs).unwrap_or_else(|| f.data.to_vec());
        w.stream(file, &format!("/Length1 {}", program.len()), &program, true);
        let mut widths = String::new();
        for g in &glyphs {
            let _ = write!(widths, "{g} [{}] ", num(f.glyph_width(*g)));
        }
        let b = f.bbox();
        let flags = if *kind == FontKind::Mono { 32 | 1 } else { 32 };
        w.obj(
            descriptor,
            &format!(
                "<< /Type /FontDescriptor /FontName /{name} /Flags {flags} /FontBBox [{} {} {} {}] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV 80 /FontFile2 {file} 0 R >>",
                num(b[0]),
                num(b[1]),
                num(b[2]),
                num(b[3]),
                num(f.ascent()),
                num(f.descent()),
                num(f.cap_height())
            ),
        );
        w.obj(
            cid,
            &format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {descriptor} 0 R /DW 0 /W [{widths}] /CIDToGIDMap /Identity >>"
            ),
        );
        w.stream(unicode, "", to_unicode(map).as_bytes(), true);
        w.obj(
            type0,
            &format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H /DescendantFonts [{cid} 0 R] /ToUnicode {unicode} 0 R >>"
            ),
        );
        let resource = format!("F{index}");
        let _ = write!(font_refs, "/{resource} {type0} 0 R ");
        font_names.insert(*kind, resource);
    }

    // Pictures.
    let mut image_refs = String::new();
    for (index, p) in doc.pictures.iter().enumerate() {
        let n = w.reserve();
        match &p.pixels {
            Pixels::Jpeg { data, components } => {
                let space = if *components == 1 { "/DeviceGray" } else { "/DeviceRGB" };
                w.stream(
                    n,
                    &format!(
                        "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {space} /BitsPerComponent 8 /Filter /DCTDecode",
                        p.width, p.height
                    ),
                    data,
                    false,
                );
            }
            Pixels::Raw { samples, gray, alpha } => {
                let mask = match alpha {
                    Some(a) => {
                        let m = w.reserve();
                        w.stream(
                            m,
                            &format!(
                                "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8",
                                p.width, p.height
                            ),
                            a,
                            true,
                        );
                        format!(" /SMask {m} 0 R")
                    }
                    None => String::new(),
                };
                let space = if *gray { "/DeviceGray" } else { "/DeviceRGB" };
                w.stream(
                    n,
                    &format!("/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {space} /BitsPerComponent 8{mask}", p.width, p.height),
                    samples,
                    true,
                );
            }
        }
        let _ = write!(image_refs, "/Im{index} {n} 0 R ");
    }
    w.obj(resources, &format!("<< /Font << {font_refs}>> /XObject << {image_refs}>> >>"));

    // Pages.
    let (pw, ph) = (doc.paper.width, doc.paper.height);
    let mut kids = Vec::new();
    let empty = [crate::Page::default()];
    let pages = if doc.pages.is_empty() { &empty[..] } else { &doc.pages[..] };
    for page in pages {
        let (page_obj, content) = (w.reserve(), w.reserve());
        let mut s = String::new();
        for item in &page.items {
            match item {
                Item::Text(t) => {
                    let (Some(f), Some(name)) = (font::get(t.font), font_names.get(&t.font)) else { continue };
                    if t.text.is_empty() {
                        continue;
                    }
                    let mut hex = String::with_capacity(t.text.len() * 4);
                    for c in t.text.chars() {
                        let _ = write!(hex, "{:04X}", f.glyph(c));
                    }
                    let skew = if t.italic { "0.2" } else { "0" };
                    let _ = writeln!(
                        s,
                        "BT /{name} {} Tf {} rg 1 0 {skew} 1 {} {} Tm <{hex}> Tj ET",
                        num(t.size),
                        color(t.color),
                        num(t.x),
                        num(ph - t.y)
                    );
                }
                Item::Rect { x, y, w: rw, h, fill, stroke, width } => {
                    let rect = format!("{} {} {} {} re", num(*x), num(ph - y - h), num(*rw), num(*h));
                    if let Some(c) = fill {
                        let _ = writeln!(s, "{} rg {rect} f", color(*c));
                    }
                    if let Some(c) = stroke {
                        let _ = writeln!(s, "{} RG {} w {rect} S", color(*c), num(*width));
                    }
                }
                Item::Line { x1, y1, x2, y2, width, color: c } => {
                    let _ = writeln!(s, "{} RG {} w {} {} m {} {} l S", color(*c), num(*width), num(*x1), num(ph - y1), num(*x2), num(ph - y2));
                }
                Item::Image { x, y, w: iw, h, picture } => {
                    if *picture < doc.pictures.len() {
                        let _ = writeln!(s, "q {} 0 0 {} {} {} cm /Im{picture} Do Q", num(*iw), num(*h), num(*x), num(ph - y - h));
                    }
                }
            }
        }
        w.stream(content, "", s.as_bytes(), true);
        w.obj(
            page_obj,
            &format!(
                "<< /Type /Page /Parent {pages_root} 0 R /MediaBox [0 0 {} {}] /Resources {resources} 0 R /Contents {content} 0 R >>",
                num(pw),
                num(ph)
            ),
        );
        kids.push(format!("{page_obj} 0 R"));
    }
    w.obj(pages_root, &format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), kids.len()));
    w.obj(root, &format!("<< /Type /Catalog /Pages {pages_root} 0 R >>"));
    w.obj(info, &format!("<< /Title {} /Producer (MailCraft) /Creator (MailCraft) >>", text_string(&doc.title)));
    w.finish(root, info)
}
