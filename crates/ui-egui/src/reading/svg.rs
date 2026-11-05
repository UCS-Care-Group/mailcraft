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
//!   that names it (by an attribute, its `style` or a rule of the drawing's style sheet, which
//!   is taken to name it for any element it might match). The elements drawn, each reference
//!   counted with all it brings, may come to at most `MAX_SVG_COST`, nesting and references
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

/// Most text of a drawing's style sheets, and most rules in them.
const MAX_SHEET_BYTES: usize = 64 * 1024;
const MAX_RULES: usize = 2_000;

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
    let layers = Weigh::new(&document)?.of(root, 0)?.layers;
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
            // A document type, its internal subset (in brackets) and all.
            let open = rest.iter().position(|&b| b == b'[' || b == b'>')?;
            if rest.get(open) == Some(&b'[') { find(find(at + open, b"]")?, b">")? } else { at + open + 1 }
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

#[derive(Clone, Copy)]
enum State {
    New,
    /// Being weighed: met again, it refers to itself.
    Busy,
    Done(Cost),
}

/// What an element names: the ids it refers to (and whether as a marker, drawn at each of its
/// points), and whether it is drawn as a layer of its own.
#[derive(Default)]
struct Effect {
    refs: Vec<(String, bool)>,
    layer: bool,
}

impl Effect {
    fn is_empty(&self) -> bool {
        self.refs.is_empty() && !self.layer
    }

    /// What a `property: value` names.
    fn declare(&mut self, property: &str, value: &str) {
        let property = property.trim().to_ascii_lowercase();
        let marker = property.starts_with("marker");
        self.refs.extend(local_urls(value).map(|id| (id.to_string(), marker)));
        self.layer |= LAYER_PROPERTIES.contains(&property.as_str());
    }

    /// What a list of declarations (a `style`, a rule's body) names.
    fn declarations(&mut self, text: &str) {
        for declaration in text.split(';') {
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

/// The ids a value refers to with `url(#id)`, in any letter case, quoted or not.
fn local_urls(value: &str) -> impl Iterator<Item = &str> {
    let lower = value.to_ascii_lowercase();
    let starts: Vec<usize> = lower.match_indices("url(").map(|(at, _)| at + 4).collect();
    starts.into_iter().filter_map(move |at| {
        let rest = value.get(at..)?.trim_start();
        let rest = rest.strip_prefix(['\'', '"']).unwrap_or(rest).strip_prefix('#')?;
        let end = rest.find(|c: char| matches!(c, '\'' | '"' | ')') || c.is_whitespace()).unwrap_or(rest.len());
        rest.get(..end).filter(|id| !id.is_empty())
    })
}

/// A rule of a drawing's style sheet that names something, and the elements it might match:
/// any, or those with one of its classes or ids (taken from the last part of each selector; a
/// part with a pseudo-class, an attribute or no class or id is taken to match any element).
struct Rule {
    any: bool,
    classes: Vec<String>,
    ids: Vec<String>,
    effect: Effect,
}

impl Rule {
    fn matches(&self, classes: &[&str], id: Option<&str>) -> bool {
        self.any || self.classes.iter().any(|c| classes.contains(&c.as_str())) || id.is_some_and(|id| self.ids.iter().any(|i| i == id))
    }
}

/// The rules of a style sheet that name something; `None` for one with too many rules.
fn rules(sheet: &str) -> Option<Vec<Rule>> {
    let mut text = String::with_capacity(sheet.len());
    let mut rest = sheet;
    while let Some(at) = rest.find("/*") {
        text.push_str(rest.get(..at).unwrap_or_default());
        rest = rest.get(at + 2..).and_then(|after| after.find("*/").and_then(|end| after.get(end + 2..))).unwrap_or_default();
    }
    text.push_str(rest);
    let mut rules = Vec::new();
    for chunk in text.split('}') {
        let Some((selectors, body)) = chunk.rsplit_once('{') else { continue };
        let mut effect = Effect::default();
        effect.declarations(body);
        if effect.is_empty() {
            continue;
        }
        // What comes before the selectors (`@media … {`) is not part of them.
        let selectors = selectors.rsplit('{').next().unwrap_or_default();
        let mut rule = Rule { any: false, classes: Vec::new(), ids: Vec::new(), effect };
        for selector in selectors.split(',') {
            let last = selector.trim().rsplit(|c: char| c.is_whitespace() || matches!(c, '>' | '+' | '~')).next().unwrap_or_default();
            if last.is_empty() || last.contains([':', '[', '*', '\\']) || !last.contains(['.', '#']) {
                rule.any = true;
                continue;
            }
            for (at, sign) in last.match_indices(['.', '#']) {
                let name = last.get(at + 1..).unwrap_or_default();
                let name = name.split(['.', '#']).next().unwrap_or_default();
                if !name.is_empty() {
                    if sign == "." { rule.classes.push(name.to_string()) } else { rule.ids.push(name.to_string()) }
                }
            }
        }
        rules.push(rule);
        if rules.len() > MAX_RULES {
            return None;
        }
    }
    Some(rules)
}

/// How many points a shape has, at most (each takes two numbers): what a marker is drawn at.
fn points(node: roxmltree::Node) -> u64 {
    let numbers = |name: &str| {
        let value = node.attribute(name).unwrap_or_default().as_bytes();
        value
            .iter()
            .enumerate()
            .filter(|&(i, b)| b.is_ascii_digit() && (i == 0 || !value.get(i - 1).is_some_and(|p| p.is_ascii_digit() || *p == b'.')))
            .count()
    };
    (numbers("d") + numbers("points")).max(2) as u64
}

/// The weighing of a drawing, element by element, each once.
struct Weigh<'a, 'input> {
    ids: HashMap<&'a str, Vec<roxmltree::Node<'a, 'input>>>,
    rules: Vec<Rule>,
    states: Vec<State>,
}

impl<'a, 'input> Weigh<'a, 'input> {
    /// Ready to weigh `document`; `None` when it has a filter that is refused, or too much style.
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
                    sheet.extend(node.children().filter_map(|t| t.text()));
                    sheet.len() > MAX_SHEET_BYTES
                }
                _ => false,
            };
            if refused {
                return None;
            }
            if let Some(id) = node.attribute("id") {
                ids.entry(id).or_default().push(node);
            }
        }
        Some(Weigh { ids, rules: rules(&sheet)?, states: vec![State::New; last + 1] })
    }

    /// What `node` names, by its attributes, its `style` and the style sheet.
    fn effect(&self, node: roxmltree::Node) -> Effect {
        let mut effect = Effect::default();
        let name = node.tag_name().name();
        for attribute in node.attributes() {
            let value = attribute.value();
            match attribute.name() {
                // `<a href>` is a link, never drawn.
                "href" if name != "a" => {
                    if let Some(id) = value.trim().strip_prefix('#').filter(|id| !id.is_empty()) {
                        effect.refs.push((id.to_string(), false));
                    }
                }
                "style" => effect.declarations(value),
                property => effect.declare(property, value),
            }
        }
        let classes: Vec<&str> = node.attribute("class").unwrap_or_default().split_whitespace().collect();
        for rule in self.rules.iter().filter(|r| r.matches(&classes, node.attribute("id"))) {
            effect.refs.extend(rule.effect.refs.iter().cloned());
            effect.layer |= rule.effect.layer;
        }
        effect.layer |= name.starts_with("fe");
        effect
    }

    /// What `node` comes to, with its children and what it refers to; `None` past a cap.
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
        let effect = self.effect(node);
        let mut cost = Cost { elements: 1, layers: u64::from(effect.layer), height: 0 };
        for child in node.children().filter(roxmltree::Node::is_element) {
            cost = cost.and(self.of(child, depth + 1)?, 1)?;
        }
        let points = points(node);
        for (id, marker) in effect.refs {
            let targets = self.ids.get(id.as_str()).cloned().unwrap_or_default();
            for target in targets {
                let mut brought = self.of(target, depth + 1)?;
                brought.layers = brought.layers.saturating_add(u64::from(LAYER_ELEMENTS.contains(&target.tag_name().name())));
                cost = cost.and(brought, if marker { points } else { 1 })?;
            }
        }
        *self.states.get_mut(index)? = State::Done(cost);
        Some(cost)
    }
}
