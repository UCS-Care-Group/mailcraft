// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's pictures that are SVG drawings (#565): an `<img>` of one attached to the message
//! (`cid:`), written into it (`data:`) or, when the owner asked for its pictures, on the web.
//! An `<svg>` element in the message itself is not drawn.
//!
//! A drawing is a stranger's program for a renderer that was not made for strangers. So only the
//! plainest drawings are shown, the kind flat logos and
//! icons are, by an allowlist: what is not on it makes the drawing a broken picture, as a damaged
//! PNG is.
//!
//! - **Nothing is loaded from outside it.** The resolvers usvg would load a picture or another
//!   drawing with answer nothing ([`options`]), no folder is given to look in, and the renderer is
//!   built without text, so no font is looked for. No element or attribute that refers to
//!   anything but a gradient is allowed.
//! - **Before it is read:** at most [`MAX_SVG_BYTES`] of UTF-8 (`svgz` is not unpacked), nesting
//!   no deeper than [`MAX_SVG_DEPTH`] (by a scan with no stack, before the XML reader, which
//!   recurses, sees it), at most [`MAX_SVG_NODES`] nodes, no entity and no internal subset of the
//!   document type, an `<svg>` root in SVG's namespace.
//! - **Only what is allowed ([`plain`]):** the elements of [`ELEMENTS`] in SVG's namespace or in
//!   none (and elements of other namespaces, which the renderer skips, but no `<style>` of any
//!   namespace, which usvg reads all the same), the attributes of [`ATTRIBUTES`] (and `data-…`,
//!   `aria-…`, `xml:space`, `xml:lang` and editors' own namespaces', which it ignores), a `style` of those properties
//!   alone with nothing quoted, escaped or commented, and `opacity` on a shape alone, so that no
//!   layer is drawn inside another. So there is no `<use>`, style sheet, `href` (gradients don't
//!   refer to each other), clip, mask, marker, pattern, filter, dash, blend mode or picture, and
//!   nothing the renderer copies but a gradient's stops for each shape painted with it: the
//!   elements times the stops are capped. At most [`MAX_SHAPES`] shapes.
//! - **The tree usvg made is checked** ([`counted`]): no clip, mask or filter in it, no layer in
//!   a layer, at most `MAX_SVG_COST` nodes and `MAX_SEGMENTS` segments of paths (a stroke's weighing
//!   more, a round one most).
//! - **It is drawn at the size it is shown** (a drawing has no pixels of its own), within the
//!   texture caps and [`MAX_SVG_PIXELS`], and with fewer pixels the more shapes, layers and segments it
//!   has, so that drawing it is a bounded amount of work; and it is drawn on a thread of its own,
//!   one drawing at a time, never on the window's (`Pictures::draw_apart`; the browser build,
//!   which has no threads, shows none). Reading it ([`probe`]: the parse, usvg's tree and the
//!   stroking for bounds) is done on the window's thread, which is why the caps above come first.
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

/// Most shapes a drawing may have.
pub const MAX_SHAPES: u64 = 5_000;

/// Most nodes the tree usvg makes of a drawing may have.
const MAX_SVG_COST: u64 = 50_000;

/// Most copies of gradients' stops (the elements times the stops and gradients) a drawing may
/// come to: each shape painted with a gradient, its own or one a group gives it, takes a copy.
const MAX_GRADIENT_COPIES: u64 = 2_000_000;

/// Longest side a drawing is taken to have, in pixels: one that says it is larger is shown as
/// if it said this, its shape kept.
pub const MAX_SVG_SIDE: f32 = 4096.0;

/// Most pixels a drawing is drawn with, and the fewest a drawing of very many shapes is.
pub const MAX_SVG_PIXELS: u64 = 4_000_000;
const MIN_SVG_PIXELS: u64 = 256 * 256;

/// The pixels drawing one drawing may touch all together, each shape and each layer taken to
/// cover all of it: the pixels it is drawn with are this over their count.
const DRAWING_WORK: u64 = 600_000_000;

/// The elements of SVG's namespace a drawing that is shown may have.
pub const ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "linearGradient",
    "radialGradient",
    "stop",
    "title",
    "desc",
    "metadata",
];

/// The shapes among them.
const SHAPES: &[&str] = &["path", "rect", "circle", "ellipse", "line", "polyline", "polygon"];

/// The attributes they may have (and the properties a `style` may set, those of them that are).
pub const ATTRIBUTES: &[&str] = &[
    // What each element is.
    "id",
    "class",
    "version",
    "baseProfile",
    "viewBox",
    "preserveAspectRatio",
    "x",
    "y",
    "width",
    "height",
    "transform",
    "d",
    "points",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "x1",
    "y1",
    "x2",
    "y2",
    "fx",
    "fy",
    "fr",
    "offset",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "enable-background",
    // How it is painted.
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-opacity",
    "color",
    "stop-color",
    "stop-opacity",
    "opacity",
    "display",
    "visibility",
    "shape-rendering",
    // Said of the shape, and nothing that is drawn here: the rule of a clip (there is none),
    // whether it overflows (only a nested drawing could), and for screen readers.
    "clip-rule",
    "overflow",
    "role",
    "focusable",
    "style",
];

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

/// What a drawing is, read and checked; `None` when it is not one that is shown.
pub fn probe(data: &[u8]) -> Option<Drawing> {
    std::panic::catch_unwind(|| read(data).and_then(|(tree, work)| drawing(&tree, work))).ok().flatten()
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
        let (tree, work) = read(data)?;
        let [w, h] = kept_size(&drawing(&tree, work)?, max_side, max_width);
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

/// Its size and the pixels it may take, from the tree and how many passes (shapes and layers)
/// drawing it takes.
fn drawing(tree: &usvg::Tree, passes: u64) -> Option<Drawing> {
    let (w, h) = (tree.size().width(), tree.size().height());
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let scale = (MAX_SVG_SIDE / w).min(MAX_SVG_SIDE / h).min(1.0);
    let size = [(w * scale).round().max(1.0) as u32, (h * scale).round().max(1.0) as u32];
    let max_pixels = (DRAWING_WORK / passes.max(1)).clamp(MIN_SVG_PIXELS, MAX_SVG_PIXELS);
    Some(Drawing { size, max_pixels })
}

/// How a drawing is read: nothing is loaded from anywhere.
fn options() -> usvg::Options<'static> {
    let mut options = usvg::Options { resources_dir: None, style_sheet: None, ..usvg::Options::default() };
    // The ones usvg comes with read files from disk, and draw a drawing written into another.
    options.image_href_resolver = usvg::ImageHrefResolver { resolve_data: Box::new(|_, _, _| None), resolve_string: Box::new(|_, _| None) };
    options
}

/// The drawing read within the caps, and the passes drawing it takes; `None` for anything
/// refused.
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
    if root.tag_name().name() != "svg" || root.tag_name().namespace() != Some(SVG) {
        return None;
    }
    let shapes = plain(&document)?;
    let tree = usvg::Tree::from_xmltree(&document, &options()).ok()?;
    let layers = counted(&tree)?;
    Some((tree, shapes.saturating_add(layers).saturating_add(1)))
}

const SVG: &str = "http://www.w3.org/2000/svg";
const XLINK: &str = "http://www.w3.org/1999/xlink";

/// Whether a `style` sets only what is allowed: the properties of [`ATTRIBUTES`], with nothing
/// quoted, escaped, commented or in braces, so that it reads the same to anybody; `opacity` only
/// on a shape.
fn plain_style(style: &str, shape: bool) -> bool {
    if style.contains(['"', '\'', '\\', '{', '}', '<', '>', '@']) || style.contains("/*") {
        return false;
    }
    style.split(';').filter(|d| !d.trim().is_empty()).all(|declaration| {
        let Some((property, _)) = declaration.split_once(':') else { return false };
        let property = property.trim();
        ATTRIBUTES.contains(&property) && !matches!(property, "style" | "id" | "class") && (shape || property != "opacity")
    })
}

/// The shapes `document` has, when it has only what is allowed: the elements, attributes and
/// styles above, and within the caps of shapes and gradient copies.
fn plain(document: &roxmltree::Document) -> Option<u64> {
    let (mut elements, mut shapes, mut stops) = (0u64, 0u64, 0u64);
    // For a drawing with strokes, which usvg strokes as it reads (for their bounds): the most
    // lines its paths could be, from their text, and whether a stroke or a round one is anywhere.
    let (mut path_text, mut stroked, mut round) = (0u64, false, false);
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        let name = node.tag_name().name();
        match node.tag_name().namespace() {
            // An element of no namespace (`xmlns=""`, which the XML reader gives as an empty
            // one) is held to the allowlist as SVG's is: usvg may read it as one.
            None | Some("" | SVG) => {}
            // usvg takes a style sheet from a `<style>` of any namespace: none is allowed.
            Some(_) if name == "style" => return None,
            // An element of another namespace (an editor's own) is skipped by the renderer.
            Some(_) => continue,
        }
        // Only the root is an `<svg>`: one inside it is a drawing of its own.
        let root = node.parent().is_some_and(|p| !p.is_element());
        if !ELEMENTS.contains(&name) || (name == "svg" && !root) {
            return None;
        }
        let shape = SHAPES.contains(&name);
        for attribute in node.attributes() {
            let local = attribute.name();
            let allowed = match attribute.namespace() {
                None | Some(SVG) => {
                    ATTRIBUTES.contains(&local) && (shape || local != "opacity") || local.starts_with("data-") || local.starts_with("aria-")
                }
                // usvg reads XLink's and XML's attributes by their own names (`xml:href` is an
                // `href`): only `xml:space` and `xml:lang`, which say nothing that is drawn.
                Some(XLINK) => false,
                Some("http://www.w3.org/XML/1998/namespace") => matches!(local, "space" | "lang"),
                // Another namespace's (an editor's own): never read by the renderer.
                Some(_) => true,
            };
            let value = attribute.value();
            if !allowed || (local == "style" && !plain_style(value, shape)) {
                return None;
            }
            if matches!(attribute.namespace(), None | Some(SVG)) {
                let lower = value.to_ascii_lowercase();
                match local {
                    "d" | "points" => path_text = path_text.saturating_add(text_lines(value)),
                    "stroke" => stroked |= lower.trim() != "none",
                    "stroke-linejoin" | "stroke-linecap" => round |= lower.contains("round"),
                    "style" => {
                        stroked |= lower.split(';').any(|d| d.split_once(':').is_some_and(|(p, v)| p.trim() == "stroke" && v.trim() != "none"));
                        round |= lower.contains("round");
                    }
                    _ => {}
                }
            }
        }
        elements += 1;
        shapes += u64::from(shape);
        stops += u64::from(matches!(name, "stop" | "linearGradient" | "radialGradient"));
    }
    // usvg strokes every stroked path as it reads it, on the window's thread, for its bounds: a
    // stroked drawing's segments (a shape that isn't a path is a few), each weighted for what the
    // stroker makes of it, are bounded before usvg sees it.
    let segments = path_text.saturating_add(shapes.saturating_mul(8));
    let weight = if round {
        1 + READ_ROUND_WEIGHT
    } else if stroked {
        1 + READ_STROKE_WEIGHT
    } else {
        0
    };
    let strokes_fit = segments.saturating_mul(weight) <= MAX_READ_STROKES;
    (shapes <= MAX_SHAPES && elements.saturating_mul(stops) <= MAX_GRADIENT_COPIES && strokes_fit).then_some(shapes)
}

/// The most segments path text (`d`, `points`) could be: every number has a digit and two
/// numbers' digits never touch, so each run of digits and each command letter is at most one.
fn text_lines(text: &str) -> u64 {
    let bytes = text.as_bytes();
    let runs = bytes.iter().enumerate().filter(|&(i, b)| b.is_ascii_digit() && !(i > 0 && bytes.get(i - 1).is_some_and(u8::is_ascii_digit))).count();
    let letters = bytes.iter().filter(|b| b.is_ascii_alphabetic()).count();
    (runs + letters) as u64
}

/// What stroking a segment takes, for its bounds as usvg reads the drawing: a few curves for its
/// sides (8), and with round joins or caps up to three conics of 16 curves each (48 more), at
/// most [`MAX_READ_STROKES`] for a drawing, so that reading one is a bounded amount of work on
/// the window's thread.
const READ_STROKE_WEIGHT: u64 = 8;
const READ_ROUND_WEIGHT: u64 = 48;
const MAX_READ_STROKES: u64 = 2_000_000;

/// The passes drawing the tree usvg made takes besides one a shape: a layer for each shape drawn
/// apart for its opacity, and one for every [`SEGMENTS_A_PASS`] lines of its paths (filled
/// once; stroked [`STROKE_WEIGHT`] times more, [`ROUND_WEIGHT`] with round joins or caps, which
/// the stroker draws as many small segments). `None` when it has what is refused: a clip, mask or
/// filter, a layer in a layer, more than `MAX_SVG_COST` nodes or [`MAX_SEGMENTS`] weighted
/// lines, each curve counted as the lines it is drawn with ([`QUAD_LINES`], [`CUBIC_LINES`]).
fn counted(tree: &usvg::Tree) -> Option<u64> {
    let (mut nodes, mut layers, mut segments) = (0u64, 0u64, 0u64);
    let mut groups = vec![(tree.root(), false)];
    while let Some((group, in_layer)) = groups.pop() {
        nodes += 1;
        let dashed = group.children().iter().any(|c| matches!(c, usvg::Node::Path(p) if p.stroke().is_some_and(|s| s.dasharray().is_some())));
        if group.clip_path().is_some() || group.mask().is_some() || !group.filters().is_empty() || dashed {
            return None;
        }
        let layer = group.should_isolate();
        if layer && in_layer {
            return None;
        }
        layers += u64::from(layer);
        for child in group.children() {
            nodes += 1;
            match child {
                usvg::Node::Group(child) => groups.push((child, in_layer || layer)),
                usvg::Node::Path(path) => {
                    let weight = match path.stroke() {
                        None => 1,
                        Some(stroke) if stroke.linejoin() == usvg::LineJoin::Round || stroke.linecap() == usvg::LineCap::Round => 1 + ROUND_WEIGHT,
                        Some(_) => 1 + STROKE_WEIGHT,
                    };
                    // Each curve as the lines it is drawn with (usvg has made every arc curves).
                    let lines: u64 = path
                        .data()
                        .segments()
                        .map(|segment| match segment {
                            tiny_skia::PathSegment::QuadTo(..) => QUAD_LINES,
                            tiny_skia::PathSegment::CubicTo(..) => CUBIC_LINES,
                            _ => 1,
                        })
                        .fold(0, u64::saturating_add);
                    segments = segments.saturating_add(lines.saturating_mul(weight));
                }
                _ => {}
            }
        }
        if nodes > MAX_SVG_COST || segments > MAX_SEGMENTS {
            return None;
        }
    }
    Some(layers.saturating_add(segments / SEGMENTS_A_PASS))
}

/// Most lines a drawing's paths may come to, each curve counted as the lines it is drawn with and
/// each line weighted for what stroking it takes. Drawing a path costs its lines and the pixels it
/// covers, so at worst a drawing is 2,000,000 lines and its passes over at most `max_pixels`,
/// drawn on a thread of its own. The counts are close to the most tiny-skia draws, not exact.
const MAX_SEGMENTS: u64 = 2_000_000;
/// The lines tiny-skia draws a quadratic and a cubic curve with, at most: it cuts a quad at its
/// extremes into up to 2 pieces and a cubic into up to 3, and each piece into up to
/// 2^`MAX_COEFF_SHIFT` (6) lines.
const QUAD_LINES: u64 = 128;
const CUBIC_LINES: u64 = 192;
/// How many times more a stroke's lines weigh than a fill's, and a round stroke's (its joins and
/// caps up to three conics of 16 curves each).
const STROKE_WEIGHT: u64 = 8;
const ROUND_WEIGHT: u64 = 64;
/// The weighted segments that cost drawing as much as a pass over the pixels.
const SEGMENTS_A_PASS: u64 = 256;

/// How deep the elements of XML text nest, at most, read a byte at a time with no stack: tags
/// counted in and out; comments, character data, processing instructions, the document type and
/// what is quoted in a tag or the document type skipped, each to where XML ends it. `None` for
/// text that ends inside one of them, which is no drawing, and for a document type with an
/// internal subset. (It may count a little deeper than the XML is, never less.)
fn nesting(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let find = |from: usize, end: &[u8]| bytes.get(from..)?.windows(end.len()).position(|w| w == end).map(|at| from + at + end.len());
    // From `from`, past what is quoted, to the first `>` that isn't; `None` at a `[` (an internal
    // subset) when `subset` is refused.
    let close = |from: usize, refuse_subset: bool| -> Option<usize> {
        let (mut j, mut quote) = (from, None);
        loop {
            let b = *bytes.get(j)?;
            match quote {
                Some(q) if b == q => quote = None,
                Some(_) => {}
                None if b == b'"' || b == b'\'' => quote = Some(b),
                None if b == b'[' && refuse_subset => return None,
                None if b == b'>' => return Some(j),
                None => {}
            }
            j += 1;
        }
    };
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
            // A document type: its literals may hold a `>`.
            close(at + 2, true)? + 1
        } else {
            let j = close(at + 1, false)?;
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
