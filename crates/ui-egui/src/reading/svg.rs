// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's pictures that are SVG drawings (#565): an `<img>` of one attached to the message
//! (`cid:`), written into it (`data:`) or, when the owner asked for its pictures, on the web.
//! An `<svg>` element in the message itself is not drawn.
//!
//! A drawing is a stranger's program for a renderer, so only plain drawings are shown, the kind
//! logos and icons are: shapes, groups, gradients and clips. Everything else is a broken
//! picture, as a damaged PNG is:
//!
//! - **Nothing is loaded from outside it.** The resolvers usvg would load a picture or another
//!   drawing with answer nothing ([`options`]), no folder is given to look in, and the renderer is
//!   built without text, so no font is looked for (a drawing's words are not drawn). Scripts,
//!   animation and links do nothing: resvg has none of them.
//! - **Refused before it is read:** more than [`MAX_SVG_BYTES`], not UTF-8, packed (`svgz` is not
//!   unpacked), nesting deeper than [`MAX_SVG_DEPTH`] (by a scan with no stack, before the XML
//!   reader, which recurses, sees it), more than [`MAX_SVG_NODES`] nodes, an entity or an
//!   internal subset of the document type, or a root that is no `<svg>` in SVG's namespace.
//! - **Refused for what it has ([`REFUSED`]):** the elements whose content the renderer copies
//!   for each element that uses them, or that inherits them from a group (`<use>`, `<pattern>`,
//!   `<marker>`, `<mask>`), style sheets (`<style>`, which could give any element anything), and
//!   filters, pictures and foreign content; `inherit` in any value; a comment or an escape in a
//!   `style`; and a clip inside a clip.
//! - **What is left is bounded before it is read:** a clip's content is copied for each element
//!   it clips, and a gradient's stops for each element painted with it, so the elements clipped
//!   times the elements in clips, and the elements (with those copies) times the gradients'
//!   stops, are capped.
//! - **The tree usvg made is counted** ([`counted`]): at most `MAX_SVG_COST` nodes, and its
//!   layers (groups drawn apart, for opacity or a clip) set the pixels it is drawn with.
//! - **It is drawn at the size it is shown** (a drawing has no pixels of its own), within the
//!   texture caps and [`MAX_SVG_PIXELS`]; a drawing of many layers is drawn with fewer.
//!
//! Parsing and drawing are somebody else's code reading hostile bytes: a panic in them is a
//! broken picture too.

use resvg::{tiny_skia, usvg, usvg::roxmltree};

/// Largest drawing that is read, in bytes.
pub const MAX_SVG_BYTES: usize = 256 * 1024;

/// Most nodes (elements, text, comments) of a drawing.
pub const MAX_SVG_NODES: u32 = 20_000;

/// Deepest a drawing's elements may nest.
pub const MAX_SVG_DEPTH: usize = 40;

/// Most nodes the tree usvg makes of a drawing may have, its clips' and the copies of them in.
const MAX_SVG_COST: u64 = 100_000;

/// Most copies of what is in clips (the elements clipped times the elements in clips), and of
/// gradients' stops (the elements times the stops and gradients), a drawing may come to.
const MAX_CLIP_COPIES: u64 = 100_000;
const MAX_GRADIENT_COPIES: u64 = 2_000_000;

/// Longest side a drawing is taken to have, in pixels: one that says it is larger is shown as
/// if it said this, its shape kept.
pub const MAX_SVG_SIDE: f32 = 4096.0;

/// Most pixels a drawing is drawn with, and the fewest a drawing of very many layers is.
pub const MAX_SVG_PIXELS: u64 = 4_000_000;
const MIN_SVG_PIXELS: u64 = 256 * 256;

/// The pixels the layers of one drawing may take all together: the passes of its group
/// opacities and clips over the pixels it is drawn with.
const LAYER_WORK: u64 = 400_000_000;

/// The elements a drawing that is shown may not have (any element whose name begins `fe`, a
/// filter's step, is refused too).
pub const REFUSED: &[&str] =
    &["use", "pattern", "marker", "mask", "style", "filter", "image", "foreignObject", "symbol", "tref", "textPath", "feImage"];

/// What is known of a drawing that can be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drawing {
    /// Its size, in pixels ([`MAX_SVG_SIDE`] at most a side).
    pub size: [u32; 2],
    /// Most pixels it is drawn with.
    pub max_pixels: u64,
}

/// Whether bytes are an SVG drawing by how they begin ([`mailcraft_html::looks_like_svg`]).
pub fn is_svg(data: &[u8]) -> bool {
    mailcraft_html::looks_like_svg(data)
}

/// What a drawing is, read and counted; `None` when it is not one that is shown.
pub fn probe(data: &[u8]) -> Option<Drawing> {
    std::panic::catch_unwind(|| read(data).and_then(|(tree, layers)| drawing(&tree, layers))).ok().flatten()
}

/// The size a drawing is drawn at, shown `max_width` pixels wide: as wide as that, larger than
/// its own size too, within `max_side` a side and its [`Drawing::max_pixels`].
pub fn kept_size(drawing: &Drawing, max_side: u32, max_width: u32) -> [u32; 2] {
    let [w, h] = drawing.size.map(|v| f64::from(v.max(1)));
    let max_side = f64::from(max_side.clamp(1, super::images::MAX_TEXTURE_SIDE));
    let pixels = drawing.max_pixels.clamp(1, MAX_SVG_PIXELS) as f64;
    let scale = (f64::from(max_width.max(1)) / w).min(max_side / w).min(max_side / h).min((pixels / (w * h)).sqrt());
    // A hair over, so that a scale worked out to the pixel isn't a pixel short.
    [(w * scale + 1e-6).floor().max(1.0) as u32, (h * scale + 1e-6).floor().max(1.0) as u32]
}

/// Draw a drawing for a texture shown `max_width` pixels wide ([`kept_size`]). `None` when it is
/// not one that is shown.
pub fn decode(data: &[u8], max_side: u32, max_width: u32) -> Option<egui::ColorImage> {
    std::panic::catch_unwind(|| {
        let (tree, layers) = read(data)?;
        let [w, h] = kept_size(&drawing(&tree, layers)?, max_side, max_width);
        let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
        let size = tree.size();
        let transform = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        let size = [usize::try_from(w).ok()?, usize::try_from(h).ok()?];
        (pixmap.data().len() == size[0].checked_mul(size[1])?.checked_mul(4)?).then(|| egui::ColorImage::from_rgba_premultiplied(size, pixmap.data()))
    })
    .ok()
    .flatten()
}

/// Its size and the pixels it may take, from the tree and how many layers it has.
fn drawing(tree: &usvg::Tree, layers: u64) -> Option<Drawing> {
    let (w, h) = (tree.size().width(), tree.size().height());
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let scale = (MAX_SVG_SIDE / w).min(MAX_SVG_SIDE / h).min(1.0);
    let size = [(w * scale).round().max(1.0) as u32, (h * scale).round().max(1.0) as u32];
    let max_pixels = (LAYER_WORK / layers.saturating_add(1)).clamp(MIN_SVG_PIXELS, MAX_SVG_PIXELS);
    Some(Drawing { size, max_pixels })
}

/// How a drawing is read: nothing is loaded from anywhere.
fn options() -> usvg::Options<'static> {
    let mut options = usvg::Options { resources_dir: None, style_sheet: None, ..usvg::Options::default() };
    // The ones usvg comes with read files from disk, and draw a drawing written into another.
    options.image_href_resolver = usvg::ImageHrefResolver { resolve_data: Box::new(|_, _, _| None), resolve_string: Box::new(|_, _| None) };
    options
}

/// The drawing read within the caps, and how many layers it has; `None` for anything refused.
fn read(data: &[u8]) -> Option<(usvg::Tree, u64)> {
    if data.len() > MAX_SVG_BYTES || !is_svg(data) {
        return None;
    }
    let text = std::str::from_utf8(data).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // An entity, declared in the document type, is the one thing the XML reader expands.
    if text.as_bytes().windows(8).any(|w| w.eq_ignore_ascii_case(b"<!entity")) {
        return None;
    }
    // The XML reader goes a step down its stack for each element in another: how deep they nest
    // is looked at first, without it.
    if nesting(text)? > MAX_SVG_DEPTH + 1 {
        return None;
    }
    let xml = roxmltree::ParsingOptions { allow_dtd: true, nodes_limit: MAX_SVG_NODES };
    let document = roxmltree::Document::parse_with_options(text, xml).ok()?;
    let root = document.root_element();
    // As a browser shows an `<img>` of one: an `<svg>` in SVG's namespace, or no drawing.
    if root.tag_name().name() != "svg" || root.tag_name().namespace() != Some("http://www.w3.org/2000/svg") {
        return None;
    }
    plain(&document)?;
    let tree = usvg::Tree::from_xmltree(&document, &options()).ok()?;
    let layers = counted(&tree)?;
    Some((tree, layers))
}

/// `Some` when `document` is a plain drawing: none of what is [`REFUSED`], and what the renderer
/// copies of clips and gradients within bounds.
fn plain(document: &roxmltree::Document) -> Option<()> {
    let (mut elements, mut clipped, mut in_clips, mut stops) = (0u64, 0u64, 0u64, 0u64);
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        let name = node.tag_name().name();
        if REFUSED.contains(&name) || name.starts_with("fe") {
            return None;
        }
        elements += 1;
        let in_clip = node.ancestors().any(|a| a.tag_name().name() == "clipPath");
        let mut clips = false;
        for attribute in node.attributes() {
            let value = attribute.value();
            let lower = value.to_ascii_lowercase();
            if lower.contains("inherit") {
                return None;
            }
            match attribute.name() {
                "style" if value.contains("/*") || value.contains('\\') => return None,
                "style" => clips |= lower.contains("clip-path"),
                "clip-path" => clips = true,
                _ => {}
            }
        }
        // A clip's content (or a clip itself) clipped again would be copied for each copy.
        if clips && in_clip {
            return None;
        }
        clipped += u64::from(clips);
        if in_clip && name != "clipPath" {
            in_clips += 1;
        }
        if matches!(name, "stop" | "linearGradient" | "radialGradient") {
            stops += 1;
        }
    }
    let clip_copies = clipped.saturating_mul(in_clips);
    let drawn = elements.saturating_add(clip_copies);
    (clip_copies <= MAX_CLIP_COPIES && drawn.saturating_mul(stops) <= MAX_GRADIENT_COPIES).then_some(())
}

/// The layers of the tree usvg made (groups drawn apart, for an opacity or a clip, and the
/// clips themselves), counting every node it has, its clips' content included; `None` for one of
/// more than `MAX_SVG_COST` nodes.
fn counted(tree: &usvg::Tree) -> Option<u64> {
    let (mut nodes, mut layers) = (0u64, 0u64);
    let mut groups = vec![tree.root()];
    while let Some(group) = groups.pop() {
        nodes += 1;
        layers += u64::from(group.should_isolate());
        let mut clip = group.clip_path();
        while let Some(path) = clip {
            nodes += 1;
            layers += 1;
            groups.push(path.root());
            clip = path.clip_path();
            if nodes > MAX_SVG_COST {
                return None;
            }
        }
        for child in group.children() {
            nodes += 1;
            if let usvg::Node::Group(child) = child {
                groups.push(child);
            }
        }
        if nodes > MAX_SVG_COST {
            return None;
        }
    }
    Some(layers)
}

/// How deep the elements of XML text nest, at most, read a byte at a time with no stack: tags
/// counted in and out, comments, character data, processing instructions, the document type and
/// quoted attribute values skipped. `None` for text that ends inside one of them or a tag, which
/// is no drawing. (It may count a little deeper than the XML is, never less.)
fn nesting(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let find = |from: usize, end: &[u8]| bytes.get(from..)?.windows(end.len()).position(|w| w == end).map(|at| from + at + end.len());
    let (mut depth, mut deepest, mut i) = (0usize, 0usize, 0usize);
    while let Some(at) = bytes.get(i..).and_then(|rest| rest.iter().position(|&b| b == b'<')).map(|at| i + at) {
        let rest = bytes.get(at..)?;
        i = if rest.starts_with(b"<!--") {
            find(at + 4, b"-->")?
        } else if rest.starts_with(b"<![CDATA[") {
            find(at + 9, b"]]>")?
        } else if rest.starts_with(b"<?") {
            find(at + 2, b"?>")?
        } else if rest.starts_with(b"<!") {
            // A document type. One with an internal subset (in brackets), which no drawing needs
            // and which could say things about the rest, is refused.
            let open = rest.iter().position(|&b| b == b'[' || b == b'>')?;
            if rest.get(open) == Some(&b'[') {
                return None;
            }
            at + open + 1
        } else {
            // A tag: to its `>`, past what is quoted.
            let (mut j, mut quote) = (at + 1, None);
            loop {
                let b = *bytes.get(j)?;
                match quote {
                    Some(q) if b == q => quote = None,
                    Some(_) => {}
                    None if b == b'"' || b == b'\'' => quote = Some(b),
                    None if b == b'>' => break,
                    None => {}
                }
                j += 1;
            }
            if rest.starts_with(b"</") {
                depth = depth.saturating_sub(1);
            } else if bytes.get(j - 1) != Some(&b'/') {
                depth += 1;
                deepest = deepest.max(depth);
            }
            j + 1
        };
    }
    Some(deepest)
}
