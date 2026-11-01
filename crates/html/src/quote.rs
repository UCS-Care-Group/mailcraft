// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's HTML made fit to quote under a reply or forward.
//!
//! The original goes out to other people, who read it in programs that run what we never would,
//! so [`quotable`] writes it anew from its tokens and keeps only its look: the elements and
//! attributes that lay out and dress text, tables and pictures, by list. Nothing that runs, embeds
//! another document, sends a form or sets a style for the whole message is carried, nor anything
//! the sender hid, nor a picture too small to see. Pictures in the message (`cid:`) and ordinary
//! pictures on the web stay: the recipient's program decides whether to fetch those.

use crate::blocks::{css_of, picture_url};
use crate::entities::{escape, unescape};
use crate::sheet::Sheet;
use crate::style::{self, Css};
use crate::tokenizer::{Tag, Token, Tokenizer};
use crate::{MAX_INPUT, is_embedded, is_safe_url, truncate};

/// Deepest elements may nest in what is written; tags opened deeper are left out, their text kept.
const MAX_DEPTH: usize = 128;

/// The elements carried. Anything else loses its tags and keeps its text, unless [`DROPPED`]
/// takes the text too.
const KEPT: [&str; 69] = [
    "a",
    "abbr",
    "address",
    "article",
    "aside",
    "b",
    "bdi",
    "bdo",
    "big",
    "blockquote",
    "br",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "font",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "main",
    "mark",
    "nobr",
    "ol",
    "p",
    "pre",
    "q",
    "s",
    "samp",
    "section",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "tt",
    "u",
    "ul",
    "var",
    "wbr",
];

/// Elements left out with everything inside them: what runs, embeds or asks, and the head.
const DROPPED: [&str; 18] = [
    "script", "style", "iframe", "frameset", "object", "applet", "button", "select", "textarea", "option", "optgroup", "datalist", "svg", "math",
    "template", "noscript", "title", "head",
];

/// Elements that never have content or an end tag.
const VOID: [&str; 20] = [
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "image", "img", "input", "isindex", "keygen", "link", "meta",
    "param", "source", "track", "wbr",
];

/// Elements that close an open paragraph.
const BLOCKS: [&str; 33] = [
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
];

/// Attributes carried on any kept element (`href` and `src` have rules of their own).
const ATTRIBUTES: [&str; 37] = [
    "abbr",
    "align",
    "alt",
    "bgcolor",
    "border",
    "cellpadding",
    "cellspacing",
    "class",
    "clear",
    "color",
    "colspan",
    "dir",
    "face",
    "frame",
    "headers",
    "height",
    "hspace",
    "id",
    "lang",
    "noshade",
    "nowrap",
    "reversed",
    "role",
    "rowspan",
    "rules",
    "scope",
    "size",
    "span",
    "start",
    "style",
    "summary",
    "title",
    "type",
    "valign",
    "value",
    "vspace",
    "width",
];

/// What, once CSS escapes, comments and spaces are gone, makes a `style` attribute fetch or run
/// something; a style holding one of these is left out whole.
const RISKY_STYLE: [&str; 9] = ["url(", "image(", "image-set(", "expression(", "@import", "behavior", "-moz-binding", "javascript:", "vbscript:"];

/// `html` reduced to what may go out under a reply or forward: the inside of its `<body>` (all of
/// it when it has none), written anew with the elements and attributes that give it its look.
///
/// Left out with what is inside them: scripts, style sheets, frames, objects, form controls, SVG
/// and MathML, templates, `noscript`, the head, comments, and every element hidden inline
/// (`display:none` and the rest `style.rs` reads as hidden, or the `hidden` attribute). Left out
/// alone: every other element not on the list, every attribute not on the list (`on…` handlers,
/// `srcset`, `background`, `ping`, `formaction` among them), a link to anything but `http`,
/// `https`, `mailto`, `tel` or `cid`, a picture from anywhere but the message itself or the web,
/// a picture on the web 2 pixels or less across or high, and a `style` that loads or runs
/// something. Text is escaped again, tags left open are closed at the end, and nesting deeper
/// than a limit keeps its text and loses its tags. Takes any string, never panics, reads at most
/// [`MAX_INPUT`] bytes and is linear in them.
pub fn quotable(html: &str) -> String {
    let mut q = Quoter::default();
    let html = truncate(html, MAX_INPUT);
    // The message's style sheets are read to see what the sender hid with them; they are not
    // carried.
    let mut sheet = Sheet::read(html);
    let mut opened = 0usize;
    for mut token in Tokenizer::new(html) {
        if let Token::Open(tag) = &mut token {
            if let Some(sheet) = &mut sheet {
                sheet.apply(opened, tag);
            }
            opened = opened.saturating_add(1);
        }
        q.token(token);
    }
    q.finish()
}

#[derive(Default)]
struct Quoter {
    out: String,
    /// Kept elements open in what is written, outermost first.
    open: Vec<&'static str>,
    /// The element being left out with its content.
    skip: Option<Skip>,
    /// The `<body>` has been seen: what came before it is gone.
    body: bool,
    /// The next text is raw text (inside `xmp`): written as it is, not decoded first.
    raw: bool,
}

struct Skip {
    name: String,
    /// Elements of the same name opened inside it and not yet closed.
    nested: usize,
}

impl Quoter {
    fn token(&mut self, token: Token<'_>) {
        if let Token::Open(tag) = &token
            && tag.name == "body"
        {
            // What is before the body is the head, or out of place: only the body is carried.
            if !self.body {
                *self = Quoter { body: true, ..Quoter::default() };
            }
            return;
        }
        if self.skipping(&token) {
            return;
        }
        let raw = std::mem::take(&mut self.raw);
        match token {
            Token::Text(text) if raw => self.out.push_str(&escape(text)),
            Token::Text(text) => self.out.push_str(&escape(&unescape(text))),
            Token::Open(tag) => self.open_tag(&tag),
            Token::Close(name) => self.close_tag(&name),
        }
    }

    /// Whether `token` is inside an element being left out (and so goes too).
    fn skipping(&mut self, token: &Token<'_>) -> bool {
        let Some(skip) = &mut self.skip else { return false };
        let ends_itself = ends_itself(&skip.name);
        match token {
            Token::Text(_) => true,
            Token::Open(tag) => {
                if skip.nested == 0 && ends_itself && closed_by(&skip.name, &tag.name) {
                    // `<p hidden>a<p>b`: the second paragraph ends the first.
                    self.skip = None;
                    return false;
                }
                if tag.name == skip.name && !VOID.contains(&tag.name.as_str()) {
                    skip.nested = skip.nested.saturating_add(1);
                }
                true
            }
            Token::Close(name) => {
                if *name == skip.name {
                    match skip.nested.checked_sub(1) {
                        Some(n) => skip.nested = n,
                        None => self.skip = None,
                    }
                    return true;
                }
                if skip.nested == 0 && ends_itself && self.open.iter().any(|o| o == name) {
                    // `<tr><td hidden>x</tr>`: the row's end ends the cell.
                    self.skip = None;
                    return false;
                }
                true
            }
        }
    }

    fn open_tag(&mut self, tag: &Tag<'_>) {
        let name = tag.name.as_str();
        if name == "xmp" {
            self.raw = true;
            return;
        }
        if DROPPED.contains(&name) {
            self.skip = Some(Skip { name: name.to_string(), nested: 0 });
            return;
        }
        let void = VOID.contains(&name);
        let kept = KEPT.iter().copied().find(|k| *k == name);
        if hidden(tag) {
            if !void {
                self.skip = Some(Skip { name: name.to_string(), nested: 0 });
            }
            return;
        }
        let Some(kept) = kept else { return };
        let picture = if kept == "img" {
            match picture_kept(tag) {
                Some(src) => Some(src),
                None => return,
            }
        } else {
            None
        };
        self.close_implied(kept);
        if !void && self.open.len() >= MAX_DEPTH {
            return;
        }
        self.out.push('<');
        self.out.push_str(kept);
        let mut seen: Vec<String> = Vec::new();
        for (attr, value) in tag.attributes() {
            let attr = attr.to_ascii_lowercase();
            // The first of a name counts, as in browsers.
            if seen.contains(&attr) {
                continue;
            }
            seen.push(attr.clone());
            let value = match (&picture, attr.as_str()) {
                (Some(src), "src") => src.clone(),
                _ if attribute_kept(kept, &attr, &value) => value,
                _ => continue,
            };
            self.out.push(' ');
            self.out.push_str(&attr);
            self.out.push_str("=\"");
            self.out.push_str(&escape(&value));
            self.out.push('"');
        }
        self.out.push('>');
        if !void {
            self.open.push(kept);
        }
    }

    fn close_tag(&mut self, name: &str) {
        if VOID.contains(&name) {
            return;
        }
        // An end tag does not reach out of the table (or cell) it is in, as in browsers.
        let bounds: &[&str] = match name {
            "table" => &[],
            "td" | "th" | "tr" | "tbody" | "thead" | "tfoot" | "caption" | "colgroup" => &["table"],
            _ => &["table", "td", "th", "caption"],
        };
        if let Some(at) = self.find_open(&[name], bounds) {
            self.close_to(at);
        }
    }

    /// Close what opening `name` closes by itself: the paragraph before a block, the item before
    /// an item, the cell before a cell or a row, and so on.
    fn close_implied(&mut self, name: &str) {
        let (targets, bounds): (&[&str], &[&str]) = match name {
            "li" => (&["li"], &["ul", "ol", "menu", "table", "td", "th", "caption"]),
            "dt" | "dd" => (&["dt", "dd"], &["dl", "table", "td", "th", "caption"]),
            "td" | "th" => (&["td", "th"], &["tr", "table"]),
            "tr" => (&["tr"], &["table"]),
            "thead" | "tbody" | "tfoot" => (&["thead", "tbody", "tfoot"], &["table"]),
            n if BLOCKS.contains(&n) => (&["p"], &["table", "td", "th", "caption"]),
            _ => return,
        };
        if let Some(at) = self.find_open(targets, bounds) {
            self.close_to(at);
        }
    }

    /// Where the innermost open element named in `targets` is, looking outwards no further than
    /// an element named in `bounds`.
    fn find_open(&self, targets: &[&str], bounds: &[&str]) -> Option<usize> {
        for (i, open) in self.open.iter().enumerate().rev() {
            if targets.contains(open) {
                return Some(i);
            }
            if bounds.contains(open) {
                return None;
            }
        }
        None
    }

    /// Close every open element from the innermost out to the one at `at`, that one included.
    fn close_to(&mut self, at: usize) {
        while self.open.len() > at {
            let Some(name) = self.open.pop() else { break };
            self.out.push_str("</");
            self.out.push_str(name);
            self.out.push('>');
        }
    }

    fn finish(mut self) -> String {
        self.close_to(0);
        self.out
    }
}

/// Whether the element ends where another of its kind (or its row, list…) starts, so that its end
/// tag is often left out.
fn ends_itself(name: &str) -> bool {
    matches!(name, "p" | "li" | "dt" | "dd" | "td" | "th" | "tr" | "thead" | "tbody" | "tfoot" | "option" | "optgroup")
}

/// Whether opening `opening` ends an open `name` whose end tag is left out.
fn closed_by(name: &str, opening: &str) -> bool {
    match name {
        "p" => BLOCKS.contains(&opening),
        "li" => opening == "li",
        "dt" | "dd" => matches!(opening, "dt" | "dd"),
        "td" | "th" => matches!(opening, "td" | "th" | "tr" | "thead" | "tbody" | "tfoot"),
        "tr" => matches!(opening, "tr" | "thead" | "tbody" | "tfoot"),
        "thead" | "tbody" | "tfoot" => matches!(opening, "thead" | "tbody" | "tfoot"),
        "option" | "optgroup" => matches!(opening, "option" | "optgroup"),
        _ => false,
    }
}

/// Whether the sender hid the element: the way the reading pane reads it.
fn hidden(tag: &Tag<'_>) -> bool {
    tag.has("hidden") || css_of(tag).hidden
}

/// A picture's address as it is carried, when it may be: one in the message, or one on the web
/// (read the way the reading pane reads it: without line breaks, `https:` before a bare `//host`).
fn picture_src(raw: Option<String>) -> Option<String> {
    picture_url(raw).filter(|url| is_embedded(url) || web(url))
}

fn web(url: &str) -> bool {
    ["http:", "https:"].iter().any(|s| url.get(..s.len()).is_some_and(|p| p.eq_ignore_ascii_case(s)))
}

/// A picture's address, when the picture is carried at all: it has an address it may have, and
/// is not on the web and too small to see (a tracking pixel).
fn picture_kept(tag: &Tag<'_>) -> Option<String> {
    let src = picture_src(tag.attr("src"))?;
    if is_embedded(&src) {
        return Some(src);
    }
    let css = tag.attr("style").map(|s| Css::parse(&s)).unwrap_or_default();
    let px = |name: &str| tag.attr(name).and_then(|v| style::attr_px(&v));
    let tiny = |v: Option<f32>| v.is_some_and(|v| v <= 2.0);
    (!(tiny(px("width")) || tiny(px("height")) || tiny(css.width) || tiny(css.height))).then_some(src)
}

fn attribute_kept(element: &str, attr: &str, value: &str) -> bool {
    match attr {
        "href" => element == "a" && is_safe_url(value),
        "style" => !risky_style(value),
        _ => ATTRIBUTES.contains(&attr),
    }
}

/// Whether a `style` attribute could fetch or run something, read the way a CSS reader would:
/// with its escapes (`\75 rl(`) decoded and its comments and white space gone.
fn risky_style(style: &str) -> bool {
    let mut plain = String::with_capacity(style.len());
    let mut chars = style.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let mut code: u32 = 0;
                let mut digits = 0;
                while digits < 6 {
                    let Some(d) = chars.peek().and_then(|h| h.to_digit(16)) else { break };
                    code = code.saturating_mul(16).saturating_add(d);
                    digits += 1;
                    chars.next();
                }
                if digits == 0 {
                    if let Some(next) = chars.next() {
                        plain.push(next.to_ascii_lowercase());
                    }
                } else {
                    plain.push(char::from_u32(code).unwrap_or('\u{fffd}').to_ascii_lowercase());
                    if chars.peek().is_some_and(|c| c.is_whitespace()) {
                        chars.next();
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut star = false;
                for c in chars.by_ref() {
                    if star && c == '/' {
                        break;
                    }
                    star = c == '*';
                }
            }
            c if c.is_whitespace() || c.is_control() => {}
            c => plain.push(c.to_ascii_lowercase()),
        }
    }
    RISKY_STYLE.iter().any(|r| plain.contains(r))
}

#[cfg(test)]
mod tests;
