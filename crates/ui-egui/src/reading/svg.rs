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
//! - **What it would come to is weighed first ([`Weigh`]).** A reference copies or draws again
//!   what it refers to: `<use>`, and a pattern, clip, mask, marker or filter for each element
//!   that names it, by an attribute, its `style`, a rule of the drawing's style sheet (taken to
//!   name it for any element it might match) or by inheriting it: paint and markers from any
//!   ancestor, a clip, mask or filter from the parent with `inherit`. References are read as the
//!   renderer reads them (an id in any namespace, `url()` as `svgtypes` parses it), and a style
//!   with an escape, an unclosed comment or quote, or more selectors or classes than any drawing
//!   has is refused. The weighing goes the way the renderer would, every reference followed as
//!   often as it is copied, within a count of steps: the elements drawn may come to at most
//!   `MAX_SVG_COST`, nesting and references [`MAX_SVG_DEPTH`] deep (the window's thread has a
//!   small stack), a marker counted at each point a shape may have. A reference to itself is
//!   refused. The filters whose cost has no bound (`feMorphology`, `feTurbulence` of many
//!   octaves, `feConvolveMatrix` of a large kernel) are refused.
//! - **It is drawn at the size it is shown** (a drawing has no pixels of its own), within the
//!   texture caps and [`MAX_SVG_PIXELS`]; a drawing of many layers (each one a pass over its
//!   pixels) is drawn with fewer.
//!
//! Anything refused, damaged or not an SVG drawing by its bytes is a broken picture, as a damaged
//! PNG is. Parsing and drawing are somebody else's code reading hostile bytes: a panic in them
//! is a broken picture too.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

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
    let layers = Weigh::new(&document)?.of(root, 0, Vec::new())?.layers;
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

/// What an element (or a rule of the style sheet) names, by the way the renderer uses it: paint
/// (`fill`, `stroke`) and markers, which the element's descendants inherit; the rest (`clip-path`,
/// `mask`, `filter` and anything else with a `url(#…)`), which a descendant takes only by saying
/// `inherit`; and `href` links. Whether it is drawn as a layer of its own, and whether it says
/// `inherit` for one of the rest.
#[derive(Default)]
struct Effect {
    paint: Vec<String>,
    marker: Vec<String>,
    other: Vec<String>,
    link: Vec<String>,
    layer: bool,
    inherit: bool,
}

impl Effect {
    fn is_empty(&self) -> bool {
        self.paint.is_empty() && self.marker.is_empty() && self.other.is_empty() && !self.layer && !self.inherit
    }

    /// What a `property: value` names.
    fn declare(&mut self, property: &str, value: &str) {
        let property = property.trim().to_ascii_lowercase();
        let ids = func_iris(value);
        match property.as_str() {
            "fill" | "stroke" => self.paint.extend(ids),
            p if p.starts_with("marker") => self.marker.extend(ids),
            _ => self.other.extend(ids),
        }
        self.layer |= LAYER_PROPERTIES.contains(&property.as_str());
        self.inherit |= matches!(property.as_str(), "clip-path" | "mask" | "filter") && value.trim().eq_ignore_ascii_case("inherit");
    }

    /// What a list of declarations (a `style`, a rule's body; comments taken out) names.
    fn declarations(&mut self, text: &str) {
        for declaration in split_outside_quotes(text, b';') {
            if let Some((property, value)) = declaration.split_once(':') {
                self.declare(property, value);
            }
        }
    }
}

/// The properties that draw an element as a layer of its own.
const LAYER_PROPERTIES: &[&str] = &["opacity", "filter", "mask", "clip-path", "mix-blend-mode", "isolation"];

/// The elements a reference to which draws a layer of its own.
const LAYER_ELEMENTS: &[&str] = &["pattern", "mask", "clipPath", "filter"];

/// The elements that are painted, and so take the paint and markers they inherit.
const SHAPES: &[&str] = &["path", "rect", "circle", "ellipse", "line", "polyline", "polygon", "text", "tspan", "textPath", "tref"];

/// Most selectors in a drawing's style sheets, and most classes an element may have.
const MAX_SELECTORS: usize = 4_000;
const MAX_CLASSES: usize = 64;

/// Most steps weighing a drawing may take: each element looked at, each reference followed and
/// each rule of the style sheet applied to an element is one.
const MAX_STEPS: u64 = 1_000_000;

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

/// `text` cut at each `separator` that is not inside quotes.
fn split_outside_quotes(text: &str, separator: u8) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quote) = (0, None);
    for (i, b) in text.bytes().enumerate() {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == separator => {
                parts.push(text.get(start..i).unwrap_or_default());
                start = i + 1;
            }
            None => {}
        }
    }
    parts.push(text.get(start..).unwrap_or_default());
    parts
}

/// CSS with its comments made spaces, as the style sheet reader takes them. `None` for a
/// comment or a quote that is never closed, or an escape (`\`), which could make a reference of
/// what doesn't read as one: no drawing needs them.
fn uncommented(text: &str) -> Option<String> {
    if text.contains('\\') {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut start, mut quote) = (0, 0, None);
    while let Some(&b) = bytes.get(i) {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'/' && bytes.get(i + 1) == Some(&b'*') => {
                out.push_str(text.get(start..i)?);
                out.push(' ');
                let end = bytes.get(i + 2..)?.windows(2).position(|w| w == b"*/")?;
                i += 2 + end + 2;
                start = i;
                continue;
            }
            None => {}
        }
        i += 1;
    }
    if quote.is_some() {
        return None;
    }
    out.push_str(text.get(start..)?);
    Some(out)
}

/// The rules of a drawing's style sheets that name something, by the elements they might match:
/// any element, or those of a class or an id (taken from the last part of each selector; a part
/// with a pseudo-class, an attribute or no class or id is taken to match any element).
#[derive(Default)]
struct Css {
    rules: Vec<Effect>,
    any: Vec<usize>,
    by_class: HashMap<String, Vec<usize>>,
    by_id: HashMap<String, Vec<usize>>,
}

impl Css {
    /// The style sheet read; `None` for one that is refused ([`uncommented`]), is not closed or
    /// has too many selectors.
    fn read(sheet: &str) -> Option<Css> {
        let text = uncommented(sheet)?;
        let bytes = text.as_bytes();
        let mut css = Css::default();
        let mut selectors = 0usize;
        let mut open: Vec<&str> = Vec::new();
        let (mut start, mut quote) = (0, None);
        for (i, &b) in bytes.iter().enumerate() {
            match quote {
                Some(q) if b == q => quote = None,
                Some(_) => {}
                None if b == b'"' || b == b'\'' => quote = Some(b),
                None if b == b'{' => {
                    open.push(text.get(start..i)?);
                    start = i + 1;
                }
                None if b == b'}' => {
                    let prelude = open.pop()?;
                    let body = text.get(start..i)?;
                    start = i + 1;
                    if prelude.trim_start().starts_with('@') {
                        continue;
                    }
                    let parts = split_outside_quotes(prelude, b',');
                    selectors = selectors.saturating_add(parts.len());
                    if selectors > MAX_SELECTORS {
                        return None;
                    }
                    let mut effect = Effect::default();
                    effect.declarations(body);
                    if effect.is_empty() {
                        continue;
                    }
                    let index = css.rules.len();
                    css.rules.push(effect);
                    for selector in parts {
                        let last = selector.trim().rsplit(|c: char| c.is_whitespace() || matches!(c, '>' | '+' | '~')).next().unwrap_or_default();
                        if last.is_empty() || last.contains([':', '[', '*', '"', '\'']) || !last.contains(['.', '#']) {
                            css.any.push(index);
                            continue;
                        }
                        for (at, sign) in last.match_indices(['.', '#']) {
                            let name = last.get(at + 1..).unwrap_or_default().split(['.', '#']).next().unwrap_or_default();
                            if !name.is_empty() {
                                let map = if sign == "." { &mut css.by_class } else { &mut css.by_id };
                                map.entry(name.to_string()).or_default().push(index);
                            }
                        }
                    }
                }
                // A statement at the top (`@import …;`) is not part of the selectors after it.
                None if b == b';' && open.last().is_none_or(|p| p.trim_start().starts_with('@')) => start = i + 1,
                None => {}
            }
        }
        (quote.is_none() && open.is_empty()).then_some(css)
    }
}

/// How many points a shape has, at most: what a marker is drawn at. Every number has a digit,
/// and two numbers' digits are never side by side, so each run of digits, and each command
/// letter (one may stand alone, as `z` does), counts as one: never fewer than there are.
fn points(node: roxmltree::Node) -> u64 {
    let count = |name: &str| {
        let value = node.attribute(name).unwrap_or_default().as_bytes();
        let runs =
            value.iter().enumerate().filter(|&(i, b)| b.is_ascii_digit() && !(i > 0 && value.get(i - 1).is_some_and(u8::is_ascii_digit))).count();
        runs + value.iter().filter(|b| b.is_ascii_alphabetic()).count()
    };
    (count("d") + count("points")).max(2) as u64
}

/// An element as it is weighed: what it names itself, the rules of the style sheet it might
/// match, and the elements it inherits from (its parent; for what a `<use>` draws, the `<use>`
/// and the parent where it is written).
struct Frame {
    own: Effect,
    css: Vec<usize>,
    parents: Vec<Rc<Frame>>,
}

/// The weighing of a drawing: every element and every reference followed, as often as the
/// renderer would copy or draw it (nothing is taken as known from before: the steps are capped
/// instead, [`MAX_STEPS`]).
struct Weigh<'a, 'input> {
    ids: HashMap<&'a str, Vec<roxmltree::Node<'a, 'input>>>,
    css: Css,
    steps: u64,
    /// The elements being weighed, from the root down: met again, one refers to itself.
    on_path: Vec<bool>,
    /// The frame of each element as written in the document, once worked out.
    written: HashMap<usize, Rc<Frame>>,
}

impl<'a, 'input> Weigh<'a, 'input> {
    /// Ready to weigh `document`; `None` when it has a filter that is refused, or a style sheet.
    fn new(document: &'a roxmltree::Document<'input>) -> Option<Self> {
        let mut ids: HashMap<&'a str, Vec<roxmltree::Node<'a, 'input>>> = HashMap::new();
        let mut sheet = String::new();
        let mut last = 0;
        for node in document.descendants() {
            last = last.max(node.id().get_usize());
            if !node.is_element() {
                continue;
            }
            let number = |name: &str| {
                node.attribute(name).map(|v| v.split([' ', ',', '\t', '\r', '\n']).filter_map(|n| n.trim().parse::<f32>().ok()).collect::<Vec<_>>())
            };
            let refused = match node.tag_name().name() {
                "feMorphology" => true,
                "feTurbulence" => number("numOctaves").is_some_and(|n| n.iter().any(|o| !o.is_finite() || *o > MAX_OCTAVES)),
                "feConvolveMatrix" => number("order").is_some_and(|n| n.iter().any(|o| !o.is_finite() || *o > MAX_KERNEL_ORDER)),
                "style" => {
                    for text in node.children().filter_map(|t| t.text()) {
                        sheet.push_str(text);
                        sheet.push('\n');
                    }
                    sheet.len() > MAX_SHEET_BYTES
                }
                _ => false,
            };
            if refused {
                return None;
            }
            // An id in any namespace: the renderer takes one in SVG's, XLink's or XML's too.
            for attribute in node.attributes().filter(|a| a.name() == "id") {
                ids.entry(attribute.value()).or_default().push(node);
            }
        }
        Some(Weigh { ids, css: Css::read(&sheet)?, steps: 0, on_path: vec![false; last + 1], written: HashMap::new() })
    }

    fn step(&mut self, by: usize) -> Option<()> {
        self.steps = self.steps.saturating_add(by as u64);
        (self.steps <= MAX_STEPS).then_some(())
    }

    /// The frame of `node` inheriting from `parents`.
    fn frame(&mut self, node: roxmltree::Node, parents: Vec<Rc<Frame>>) -> Option<Rc<Frame>> {
        let name = node.tag_name().name();
        let mut own = Effect::default();
        let mut css = Vec::new();
        let mut classes = 0;
        for attribute in node.attributes() {
            self.step(1)?;
            let value = attribute.value();
            match attribute.name() {
                // `<a href>` is a link, never drawn.
                "href" => {
                    if name != "a" {
                        own.link.extend(iri(value));
                    }
                }
                "style" => own.declarations(&uncommented(value)?),
                "id" => css.extend(self.css.by_id.get(value).into_iter().flatten()),
                "class" => {
                    for class in value.split_whitespace() {
                        classes += 1;
                        if classes > MAX_CLASSES {
                            return None;
                        }
                        css.extend(self.css.by_class.get(class).into_iter().flatten());
                    }
                }
                property => own.declare(property, value),
            }
        }
        css.extend(&self.css.any);
        self.step(css.len())?;
        own.layer |= name.starts_with("fe");
        Some(Rc::new(Frame { own, css, parents }))
    }

    /// The frame `node`'s children inherit from where it is written: its own, inheriting from its
    /// ancestors. `None` inside when `node` is no element (the document).
    fn written(&mut self, node: roxmltree::Node) -> Option<Option<Rc<Frame>>> {
        if !node.is_element() {
            return Some(None);
        }
        let index = node.id().get_usize();
        if let Some(frame) = self.written.get(&index) {
            return Some(Some(frame.clone()));
        }
        let parents = match node.parent() {
            Some(parent) => self.written(parent)?.into_iter().collect(),
            None => Vec::new(),
        };
        let frame = self.frame(node, parents)?;
        self.written.insert(index, frame.clone());
        Some(Some(frame))
    }

    /// The ids `pick` gives of `frame` and its rules, and of every frame it inherits from too
    /// when `inherited`; each a step.
    fn gather(&mut self, frame: &Rc<Frame>, pick: fn(&Effect) -> &Vec<String>, inherited: bool) -> Option<Vec<String>> {
        let mut ids = Vec::new();
        let mut seen: HashSet<*const Frame> = HashSet::new();
        let mut todo = vec![frame.clone()];
        while let Some(f) = todo.pop() {
            if !seen.insert(Rc::as_ptr(&f)) {
                continue;
            }
            ids.extend(pick(&f.own).iter().cloned());
            for &rule in &f.css {
                ids.extend(self.css.rules.get(rule).map(pick).into_iter().flatten().cloned());
            }
            if inherited {
                todo.extend(f.parents.iter().cloned());
            }
        }
        self.step(ids.len() + seen.len())?;
        Some(ids)
    }

    /// Whether a frame says `inherit` for a clip, a mask or a filter, itself or by a rule.
    fn inherits(&self, frame: &Frame) -> bool {
        frame.own.inherit || frame.css.iter().any(|&rule| self.css.rules.get(rule).is_some_and(|e| e.inherit))
    }

    /// Whether a frame is drawn as a layer of its own.
    fn layer(&self, frame: &Frame) -> bool {
        frame.own.layer || frame.css.iter().any(|&rule| self.css.rules.get(rule).is_some_and(|e| e.layer))
    }

    /// The clips, masks, filters and the like `frame` draws with: its own, and its parent's
    /// when it says `inherit` (and theirs when they do).
    fn others(&mut self, frame: &Rc<Frame>) -> Option<Vec<String>> {
        let mut ids = self.gather(frame, |e| &e.other, false)?;
        let mut todo: Vec<Rc<Frame>> = if self.inherits(frame) { frame.parents.clone() } else { Vec::new() };
        let mut seen: HashSet<*const Frame> = HashSet::new();
        while let Some(parent) = todo.pop() {
            if !seen.insert(Rc::as_ptr(&parent)) {
                continue;
            }
            ids.extend(self.gather(&parent, |e| &e.other, false)?);
            if self.inherits(&parent) {
                todo.extend(parent.parents.iter().cloned());
            }
        }
        Some(ids)
    }

    fn targets(&self, id: &str) -> Vec<roxmltree::Node<'a, 'input>> {
        self.ids.get(id).cloned().unwrap_or_default()
    }

    /// `cost` with what each element of id `id` brings, `times` over, weighed where it is written.
    fn charge(&mut self, mut cost: Cost, id: &str, depth: usize, times: u64) -> Option<Cost> {
        for target in self.targets(id) {
            let parents = match target.parent() {
                Some(parent) => self.written(parent)?.into_iter().collect(),
                None => Vec::new(),
            };
            let mut brought = self.of(target, depth + 1, parents)?;
            brought.layers = brought.layers.saturating_add(u64::from(LAYER_ELEMENTS.contains(&target.tag_name().name())));
            cost = cost.and(brought, times)?;
        }
        Some(cost)
    }

    /// What `node` comes to, inheriting from `parents`, with its children and what it refers to;
    /// `None` past a cap, or for an element that refers to itself.
    fn of(&mut self, node: roxmltree::Node<'a, 'input>, depth: usize, parents: Vec<Rc<Frame>>) -> Option<Cost> {
        if depth > MAX_SVG_DEPTH {
            return None;
        }
        let index = node.id().get_usize();
        if *self.on_path.get(index)? {
            return None;
        }
        *self.on_path.get_mut(index)? = true;
        let cost = self.weigh(node, depth, parents);
        if let Some(on) = self.on_path.get_mut(index) {
            *on = false;
        }
        cost
    }

    fn weigh(&mut self, node: roxmltree::Node<'a, 'input>, depth: usize, parents: Vec<Rc<Frame>>) -> Option<Cost> {
        self.step(1)?;
        let frame = self.frame(node, parents)?;
        let name = node.tag_name().name();
        let mut cost = Cost { elements: 1, layers: u64::from(self.layer(&frame)), height: 0 };
        if SHAPES.contains(&name) {
            let points = points(node);
            for id in self.gather(&frame, |e| &e.paint, true)? {
                cost = self.charge(cost, &id, depth, 1)?;
            }
            for id in self.gather(&frame, |e| &e.marker, true)? {
                cost = self.charge(cost, &id, depth, points)?;
            }
        }
        for id in self.others(&frame)? {
            cost = self.charge(cost, &id, depth, 1)?;
        }
        for id in frame.own.link.clone() {
            if name != "use" {
                cost = self.charge(cost, &id, depth, 1)?;
                continue;
            }
            // What a `<use>` draws inherits from it, and (to be sure) from where it is written.
            for target in self.targets(&id) {
                let mut parents = vec![frame.clone()];
                if let Some(parent) = target.parent() {
                    parents.extend(self.written(parent)?);
                }
                cost = cost.and(self.of(target, depth + 1, parents)?, 1)?;
            }
        }
        for child in node.children().filter(roxmltree::Node::is_element) {
            cost = cost.and(self.of(child, depth + 1, vec![frame.clone()])?, 1)?;
        }
        Some(cost)
    }
}
