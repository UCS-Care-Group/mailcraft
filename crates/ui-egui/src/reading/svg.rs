// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's pictures that are SVG drawings (#565): an `<img>` of one attached to the message
//! (`cid:`), written into it (`data:`) or, when the owner asked for its pictures, on the web.
//! An `<svg>` element in the message itself is not drawn.
//!
//! A drawing is a stranger's program for a renderer, so it is read in steps that each refuse
//! what could take the window down or reach outside the drawing, and drawn with `resvg`:
//!
//! - **Nothing is loaded from outside it.** Every picture, drawing, style sheet or font a drawing
//!   names, on the web, on this computer or written into it, is left out: the resolvers that
//!   would load them answer nothing ([`options`]), no folder is given to look in, `<use>` and
//!   paint only ever refer to the drawing's own elements, and the renderer is built without
//!   text, so no font is looked for (a drawing's words are not drawn). Scripts, animation and
//!   links do nothing: resvg has none of them.
//! - **It is read within caps, before the renderer sees it:** [`MAX_SVG_BYTES`] of UTF-8, not
//!   packed (an `svgz` is not unpacked, so it is no SVG here), nesting no deeper than
//!   [`MAX_SVG_DEPTH`] by a scan with no stack before the XML reader (which recurses) sees it,
//!   at most [`MAX_SVG_NODES`] nodes, and no entity declared (none is then expanded: an entity
//!   bomb is refused unread).
//! - **What could be copied depending on where it is drawn is refused, and the rest weighed
//!   ([`Weigh`]).** The renderer copies what a reference names for each element that refers to
//!   it, and an element can be given a reference by its ancestors (paint and markers inherit; a
//!   clip, mask or filter with `inherit`) or by any rule of a style sheet that matches it. So a
//!   drawing with a `<pattern>` or a `<marker>`, a clip, mask or filter that says `inherit`, a
//!   style with a comment, an escape or `inherit` in it, or a style sheet that refers to anything
//!   but a gradient is refused (a gradient's copies are its stops, bounded for the drawing as a
//!   whole). What is left is copied the same wherever it is drawn: each element is weighed once,
//!   with its children and what its own attributes refer to (`<use>`, a clip, mask, filter or
//!   gradient), read as the renderer reads them (an id in any namespace, `url()` as `svgtypes`
//!   parses it). The elements drawn may come to at most `MAX_SVG_COST`, nesting and references
//!   [`MAX_SVG_DEPTH`] deep (the window's thread has a small stack). A reference to itself is
//!   refused. The filters whose cost has no bound (`feMorphology`, `feTurbulence` of many
//!   octaves, `feConvolveMatrix` of a large kernel) are refused.
//! - **It is drawn at the size it is shown** (a drawing has no pixels of its own), within the
//!   texture caps and [`MAX_SVG_PIXELS`]; a drawing of many layers (each one a pass over its
//!   pixels) is drawn with fewer.
//!
//! Anything refused, damaged or not an SVG drawing by its bytes is a broken picture, as a damaged
//! PNG is. Parsing and drawing are somebody else's code reading hostile bytes: a panic in them
//! is a broken picture too.

use std::collections::HashMap;

use resvg::{tiny_skia, usvg, usvg::roxmltree};

/// Largest drawing that is read, in bytes.
pub const MAX_SVG_BYTES: usize = 2 * 1024 * 1024;

/// Most nodes (elements, text, comments) of a drawing.
pub const MAX_SVG_NODES: u32 = 20_000;

/// Deepest a drawing's elements may nest, references followed counting as a step each.
pub const MAX_SVG_DEPTH: usize = 40;

/// Most elements a drawing may come to with every reference counted with what it brings.
const MAX_SVG_COST: u64 = 100_000;

/// Longest side a drawing is taken to have, in pixels: one that says it is larger is shown as
/// if it said this, its shape kept.
pub const MAX_SVG_SIDE: f32 = 4096.0;

/// Most pixels a drawing is drawn with, and the fewest a drawing of very many layers is.
pub const MAX_SVG_PIXELS: u64 = 4_000_000;
const MIN_SVG_PIXELS: u64 = 256 * 256;

/// The pixels the layers of one drawing may take all together: the passes of its group
/// opacities, clips, masks, patterns and filter steps over the pixels it is drawn with.
const LAYER_WORK: u64 = 400_000_000;

/// Most text of a drawing's style sheets.
const MAX_SHEET_BYTES: usize = 64 * 1024;

/// The largest `numOctaves` of `feTurbulence`, and kernel of `feConvolveMatrix`, that is drawn.
const MAX_OCTAVES: f32 = 8.0;
const MAX_KERNEL_ORDER: f32 = 5.0;

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

/// What a drawing is, read and weighed; `None` when it is not one that is shown.
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
    let layers = Weigh::new(&document)?.total(root)?.layers;
    let tree = usvg::Tree::from_xmltree(&document, &options()).ok()?;
    Some((tree, layers))
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

/// What one element comes to: the elements drawn, the layers among them, and how many steps
/// deep it goes, through what it holds and what it refers to (the renderer follows each).
#[derive(Clone, Copy, Debug, Default)]
struct Cost {
    elements: u64,
    layers: u64,
    height: usize,
}

impl Cost {
    /// `other`, held or referred to `times` over, added; `None` past the cap.
    fn and(self, other: Cost, times: u64) -> Option<Cost> {
        let elements = self.elements.saturating_add(other.elements.saturating_mul(times));
        let layers = self.layers.saturating_add(other.layers.saturating_mul(times));
        let height = self.height.max(other.height.saturating_add(1));
        (elements <= MAX_SVG_COST && height <= MAX_SVG_DEPTH).then_some(Cost { elements, layers, height })
    }
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// The ids a value refers to with `url(#id)`, read as the renderer reads one (`svgtypes`'
/// `FuncIRI`: spaces skipped, a quote or none, `#`, then to the quote, or to a space or `)`),
/// wherever in the value one begins, in any letter case: never fewer than the renderer finds.
fn func_iris(value: &str) -> Vec<String> {
    let bytes = value.as_bytes();
    let lower = value.to_ascii_lowercase();
    let mut ids = Vec::new();
    for (at, _) in lower.match_indices("url(") {
        let mut j = at + 4;
        while bytes.get(j).copied().is_some_and(is_space) {
            j += 1;
        }
        let quote = bytes.get(j).copied().filter(|b| *b == b'\'' || *b == b'"');
        if quote.is_some() {
            j += 1;
            while bytes.get(j).copied().is_some_and(is_space) {
                j += 1;
            }
        }
        if bytes.get(j) != Some(&b'#') {
            continue;
        }
        j += 1;
        let rest = bytes.get(j..).unwrap_or_default();
        let end = match quote {
            Some(q) => rest.iter().position(|b| *b == q),
            None => rest.iter().position(|b| *b == b' ' || *b == b')'),
        }
        .map_or(bytes.len(), |n| j + n);
        let id = value.get(j..end).unwrap_or_default();
        let id = if quote.is_some() { id.trim_end() } else { id };
        if !id.is_empty() {
            ids.push(id.to_string());
        }
    }
    ids
}

/// The id an `href` refers to, read as the renderer reads one (`svgtypes`' IRI: spaces
/// skipped, `#`, then to a space).
fn iri(value: &str) -> Option<String> {
    let rest = value.trim_start_matches([' ', '\t', '\n', '\r']).strip_prefix('#')?;
    let id = rest.split(' ').next().unwrap_or_default();
    (!id.is_empty()).then(|| id.to_string())
}

/// The properties that draw an element as a layer of its own.
const LAYER_PROPERTIES: &[&str] = &["opacity", "filter", "mask", "clip-path", "mix-blend-mode", "isolation"];

/// The elements a reference to which draws a layer of its own.
const LAYER_ELEMENTS: &[&str] = &["mask", "clipPath", "filter"];

/// The paint a drawing's style sheet may refer to: gradients, whose copies are their stops.
const GRADIENTS: &[&str] = &["linearGradient", "radialGradient"];

/// The elements a drawing that is shown may not have. A pattern is painted by any shape that
/// inherits it, and a marker drawn at every point of any shape that inherits it: what they
/// would come to depends on where they are inherited, which is not weighed; and `feMorphology`
/// costs without bound.
const REFUSED_ELEMENTS: &[&str] = &["pattern", "marker", "feMorphology"];

/// Most stops of gradients a drawing may come to, each element taking a copy of each gradient
/// (as many as a style sheet or an inherited paint could give it).
const MAX_GRADIENT_COPIES: u64 = 2_000_000;

/// Whether the text of a style (a sheet or a `style` attribute) is refused: a comment or an
/// escape, which could make a reference of what doesn't read as one, or `inherit`, which could
/// give an element a clip, mask or filter its parent has. No drawing needs them.
fn style_refused(text: &str) -> bool {
    text.contains("/*") || text.contains('\\') || text.to_ascii_lowercase().contains("inherit")
}

#[derive(Clone, Copy)]
enum State {
    New,
    /// Being weighed: met again, it refers to itself.
    Busy,
    Done(Cost),
}

/// The weighing of a drawing. With patterns, markers and `inherit` refused, and a style sheet
/// that refers only to gradients, what an element comes to doesn't depend on where it is drawn:
/// its children and what its own attributes refer to (`href`, `url(#…)` in any attribute and its
/// `style`), each element weighed once and taken as known after. Copies of gradients, which an
/// element may take from its ancestors or a style sheet, are bounded for the drawing as a whole.
struct Weigh<'a, 'input> {
    ids: HashMap<&'a str, Vec<roxmltree::Node<'a, 'input>>>,
    states: Vec<State>,
    /// Layers the style sheet may give: as many as there are elements, if it names a property
    /// that makes one.
    sheet_layers: u64,
}

impl<'a, 'input> Weigh<'a, 'input> {
    /// Ready to weigh `document`; `None` when it has what is refused.
    fn new(document: &'a roxmltree::Document<'input>) -> Option<Self> {
        let mut ids: HashMap<&'a str, Vec<roxmltree::Node<'a, 'input>>> = HashMap::new();
        let mut sheet = String::new();
        let (mut last, mut elements, mut stops) = (0, 0u64, 0u64);
        for node in document.descendants() {
            last = last.max(node.id().get_usize());
            if !node.is_element() {
                continue;
            }
            elements += 1;
            let name = node.tag_name().name();
            let number = |name: &str| {
                node.attribute(name).map(|v| v.split([' ', ',', '\t', '\r', '\n']).filter_map(|n| n.trim().parse::<f32>().ok()).collect::<Vec<_>>())
            };
            let refused = REFUSED_ELEMENTS.contains(&name)
                || match name {
                    "feTurbulence" => number("numOctaves").is_some_and(|n| n.iter().any(|o| !o.is_finite() || *o > MAX_OCTAVES)),
                    "feConvolveMatrix" => number("order").is_some_and(|n| n.iter().any(|o| !o.is_finite() || *o > MAX_KERNEL_ORDER)),
                    _ => false,
                };
            if refused {
                return None;
            }
            if name == "style" {
                for text in node.children().filter_map(|t| t.text()) {
                    sheet.push_str(text);
                    sheet.push('\n');
                }
                if sheet.len() > MAX_SHEET_BYTES {
                    return None;
                }
            }
            if name == "stop" || GRADIENTS.contains(&name) {
                stops += 1;
            }
            for attribute in node.attributes() {
                let value = attribute.value();
                match attribute.name() {
                    // An id in any namespace: the renderer takes one in SVG's, XLink's or XML's too.
                    "id" => ids.entry(value).or_default().push(node),
                    "style" if style_refused(value) => return None,
                    "clip-path" | "mask" | "filter" if value.trim().eq_ignore_ascii_case("inherit") => return None,
                    _ => {}
                }
            }
        }
        if style_refused(&sheet) || elements.saturating_mul(stops) > MAX_GRADIENT_COPIES {
            return None;
        }
        // Whatever the sheet's rules match, each element could take a copy of what it names:
        // only a gradient's is small and bounded.
        for id in func_iris(&sheet) {
            if ids.get(id.as_str()).into_iter().flatten().any(|target| !GRADIENTS.contains(&target.tag_name().name())) {
                return None;
            }
        }
        let lower = sheet.to_ascii_lowercase();
        let sheet_layers = if LAYER_PROPERTIES.iter().any(|p| lower.contains(p)) { elements } else { 0 };
        Some(Weigh { ids, states: vec![State::New; last + 1], sheet_layers })
    }

    /// What the drawing comes to: its root weighed, and the layers its style sheet may add.
    fn total(mut self, root: roxmltree::Node<'a, 'input>) -> Option<Cost> {
        let mut cost = self.of(root, 0)?;
        cost.layers = cost.layers.saturating_add(self.sheet_layers);
        Some(cost)
    }

    /// The ids `node`'s own attributes refer to, and whether they draw it as a layer of its own.
    fn refs(node: roxmltree::Node) -> (Vec<String>, bool) {
        let name = node.tag_name().name();
        let (mut refs, mut layer) = (Vec::new(), name.starts_with("fe"));
        for attribute in node.attributes() {
            let value = attribute.value();
            match attribute.name() {
                // `<a href>` is a link, never drawn.
                "href" => {
                    if name != "a" {
                        refs.extend(iri(value));
                    }
                }
                "id" | "class" => {}
                property => {
                    refs.extend(func_iris(value));
                    let lower = value.to_ascii_lowercase();
                    layer |= LAYER_PROPERTIES.contains(&property) || (property == "style" && LAYER_PROPERTIES.iter().any(|p| lower.contains(p)));
                }
            }
        }
        (refs, layer)
    }

    /// What `node` comes to, with its children and what it refers to; `None` past a cap, or for
    /// an element that refers to itself.
    fn of(&mut self, node: roxmltree::Node<'a, 'input>, depth: usize) -> Option<Cost> {
        if depth > MAX_SVG_DEPTH {
            return None;
        }
        let index = node.id().get_usize();
        match *self.states.get(index)? {
            // Weighed already, from elsewhere: as deep below here as it was below there.
            State::Done(cost) => return (depth.saturating_add(cost.height) <= MAX_SVG_DEPTH).then_some(cost),
            State::Busy => return None,
            State::New => {}
        }
        *self.states.get_mut(index)? = State::Busy;
        let (refs, layer) = Self::refs(node);
        let mut cost = Cost { elements: 1, layers: u64::from(layer), height: 0 };
        for child in node.children().filter(roxmltree::Node::is_element) {
            cost = cost.and(self.of(child, depth + 1)?, 1)?;
        }
        for id in refs {
            for target in self.ids.get(id.as_str()).cloned().unwrap_or_default() {
                let mut brought = self.of(target, depth + 1)?;
                brought.layers = brought.layers.saturating_add(u64::from(LAYER_ELEMENTS.contains(&target.tag_name().name())));
                cost = cost.and(brought, 1)?;
            }
        }
        *self.states.get_mut(index)? = State::Done(cost);
        Some(cost)
    }
}
