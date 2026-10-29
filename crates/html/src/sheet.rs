// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The style sheets of a message.
//!
//! A message's `<style>` blocks (in its head or its body) are read for the same whitelist of
//! presentational facts as a `style` attribute ([`crate::style`]); a style sheet is one more
//! source of the same values, never a new kind of presentation. Nothing in a sheet is run or
//! fetched: `@import`, `@font-face`, `@media` (for now), every other at-rule, `url()` (but in a
//! background, which the whitelist reads only for its colour), `content` and everything else the
//! whitelist doesn't name are ignored.
//!
//! What is read is CSS as a browser reads it, from our own code (no CSS crate):
//!
//! * selectors: a tag, `*`, a class, an id, an attribute (`[a]`, `=`, `~=`, `|=`, `^=`, `$=`,
//!   `*=`, with `i`), the descendant, child (`>`), adjacent (`+`) and general sibling (`~`)
//!   combinators, `:first-child`, `:last-child`, `:only-child`, `:nth-child()`, `:nth-of-type()`,
//!   `:first-of-type`, `:root`, `:link` and `:not()` of simple selectors. `:visited`, `:hover`,
//!   `:focus`, `:active`, `::before`, `::after` and the like never match, and a selector this
//!   doesn't know is dropped;
//! * the cascade: specificity, then source order, with `!important` over all. The order of what
//!   an element gets is the sheet's normal declarations, its own `style` attribute's normal ones,
//!   the sheet's `!important` ones, and the `style` attribute's `!important` ones last, as in
//!   CSS. Inheritance is the builder's, as for a `style` attribute.
//!
//! Everything is hostile and bounded: [`MAX_SHEET`] bytes of sheet text per message, [`MAX_RULES`]
//! rules, [`MAX_PARTS`] compound selectors in a selector (a longer one is dropped), a count of
//! matching steps per element ([`MAX_STEPS_PER_ELEMENT`]; a rule past it skips that element) and
//! per message ([`MAX_STEPS`]; the elements after it are not styled), [`MAX_ELEMENTS`] elements
//! and [`MAX_DEPTH`] levels of them. The look is worked out once, while the message is turned
//! into blocks, and lives in the blocks.

use std::collections::HashMap;
use std::rc::Rc;

use crate::media::{self, Env};
use crate::style::Css;

use crate::tokenizer::{Tag, Token, Tokenizer};
use crate::truncate;

/// Most bytes of style sheet read from one message, all its `<style>` blocks together.
pub(crate) const MAX_SHEET: usize = 64 * 1024;

/// Most rules (a selector with its declarations; `a, b {}` is two) kept from one message.
pub(crate) const MAX_RULES: usize = 4_000;

/// Most compound selectors (the parts between combinators) in a selector; a longer one is dropped.
pub(crate) const MAX_PARTS: usize = 16;

/// Most matching steps spent on one element.
pub(crate) const MAX_STEPS_PER_ELEMENT: u32 = 600;

/// Most matching steps spent on a whole message, however long.
pub(crate) const MAX_STEPS: u32 = 20_000_000;

/// Matching steps a message gets for each byte of it, and the least it gets: a step is about 30
/// nanoseconds, so a megabyte of mail spends at most about 45 milliseconds of its look.
const STEPS_PER_BYTE_NUM: u64 = 6;
const STEPS_PER_BYTE_DEN: u64 = 1;
const MIN_STEPS: u32 = 50_000;

/// What working out a look (reading its declarations) costs, in steps, at least and for each two
/// bytes of it: about ten microseconds for what a long look says.
const MIN_BUILD_STEPS: u64 = 200;
/// What building a look costs more for each declaration it considers.
const BUILD_STEPS_PER_DECL: u64 = 4;
/// And for each declaration the look says (each is parsed again, and kept once per property).
const BUILD_STEPS_PER_OUTPUT: u64 = 20;

/// The steps a message of `len` bytes gets.
pub(crate) fn steps_for(len: usize) -> u32 {
    let steps = (len as u64).saturating_mul(STEPS_PER_BYTE_NUM) / STEPS_PER_BYTE_DEN;
    u32::try_from(steps).unwrap_or(MAX_STEPS).clamp(MIN_STEPS, MAX_STEPS)
}

/// Most elements a message's tree keeps for matching; the ones after are not styled.
const MAX_ELEMENTS: usize = 60_000;

/// Deepest nesting the tree keeps; deeper elements are leaves.
const MAX_DEPTH: usize = 512;

/// Most simple selectors in one compound selector, and most levels of `:not()`.
const MAX_SIMPLES: usize = 32;
const MAX_NOT_DEPTH: usize = 2;

/// Most selectors in a `:not()`; a longer list drops the rule.
const MAX_NOT_LIST: usize = 32;

/// Most different sets of rules whose look is kept for the elements that match the same ones.
const MAX_CACHED: usize = 2_048;

/// Most declarations, and most bytes of them, one element takes from the sheet on each side of
/// its `style` attribute; and most declarations looked at to find them.
const MAX_DECLARATIONS: usize = 40;
const MAX_APPLIED: usize = 1_500;
const MAX_CONSIDERED: usize = 600;

/// Longest value of a declaration kept, and of an attribute kept for matching.
const MAX_VALUE: usize = 1_024;
const MAX_ATTR_VALUE: usize = 128;
const MAX_ELEMENT_ATTR_VALUE: usize = 256;

/// How many of an element's classes a step looks through, and how many bytes of an attribute's
/// value it compares.
const CLASSES_PER_STEP: u32 = 2;
const ATTR_BYTES_PER_STEP: u32 = 16;

/// Most different element names kept apart among one element's children.
const MAX_TYPES: usize = 48;

/// Most classes kept for an element.
const MAX_CLASSES: usize = 16;

/// Elements without content or an end tag.
fn is_void(name: &str) -> bool {
    matches!(name, "area" | "base" | "br" | "col" | "embed" | "hr" | "img" | "input" | "link" | "meta" | "param" | "source" | "track" | "wbr")
}

/// Elements that close an open `<p>` as they start.
fn closes_p(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "ul"
            | "ol"
            | "table"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "blockquote"
            | "pre"
            | "hr"
            | "form"
            | "dl"
            | "address"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "nav"
            | "aside"
            | "center"
            | "fieldset"
            | "figure"
            | "main"
            | "menu"
    )
}

const NONE: u32 = u32::MAX;

// ---- the tree ----

/// One element of the message, as far as matching needs it.
#[derive(Debug)]
struct El {
    name: String,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, String)>,
    /// The values of `attrs` in lower case, when a rule asks for a match that ignores case
    /// (`[a=b i]`); else empty.
    lower: Vec<String>,
    /// Index in the tree; 0 is the document itself, which nothing matches.
    parent: u32,
    /// The element before it among its parent's children.
    prev: u32,
    /// Its place among its parent's children, from 1, and among those of its name.
    index: u32,
    type_index: u32,
    /// The last of its own children so far.
    last_child: u32,
}

impl El {
    fn document() -> El {
        El {
            name: String::new(),
            id: None,
            classes: Vec::new(),
            attrs: Vec::new(),
            lower: Vec::new(),
            parent: NONE,
            prev: NONE,
            index: 0,
            type_index: 0,
            last_child: NONE,
        }
    }

    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// What the parent's ancestors have (the parent's own names are added by the caller).
    fn parent_bloom(&self, blooms: &[Bloom]) -> Bloom {
        let mut bloom = Bloom::default();
        if self.parent != NONE
            && self.parent != 0
            && let Some(b) = blooms.get(self.parent as usize)
        {
            bloom.or(b);
        }
        bloom
    }

    /// An attribute's value, in lower case when `fold`.
    fn attr_value(&self, name: &str, fold: bool) -> Option<&str> {
        let at = self.attrs.iter().position(|(n, _)| n == name)?;
        if fold { self.lower.get(at).map(String::as_str) } else { self.attrs.get(at).map(|(_, v)| v.as_str()) }
    }
}

struct Open {
    el: u32,
    children: u32,
    /// How many of its children there are of each name (the first [`MAX_TYPES`] names).
    by_type: Vec<(String, u32)>,
}

struct TreeBuilder {
    els: Vec<El>,
    stack: Vec<Open>,
    /// Where in the stack the open elements of each name are, outermost first.
    positions: HashMap<String, Vec<usize>>,
    /// Names looked up while closing, for the tests.
    walked: u64,
}

impl TreeBuilder {
    fn new() -> TreeBuilder {
        TreeBuilder { els: vec![El::document()], stack: vec![Open { el: 0, children: 0, by_type: Vec::new() }], positions: HashMap::new(), walked: 0 }
    }

    /// Close the nearest open element named in `names`, and what is inside it, unless one of
    /// `barriers` is nearer.
    fn close_nearest(&mut self, names: &[&str], barriers: &[&str]) {
        let nearest = |this: &TreeBuilder, list: &[&str]| list.iter().filter_map(|n| this.positions.get(*n)?.last().copied()).max();
        // Nothing of that name is open: nothing to look for (and a stray end tag costs nothing).
        let Some(found) = nearest(self, names) else { return };
        self.walked += (names.len() + barriers.len()) as u64;
        if nearest(self, barriers).is_some_and(|barrier| barrier > found) {
            return;
        }
        self.truncate_stack(found);
    }

    fn truncate_stack(&mut self, at: usize) {
        for open in self.stack.drain(at..) {
            if let Some(list) = self.els.get(open.el as usize).and_then(|el| self.positions.get_mut(el.name.as_str())) {
                list.pop();
            }
        }
    }

    fn open(&mut self, tag: &Tag<'_>) {
        // Elements past the cap are not recorded, so the order of the ones before is kept.
        if self.els.len() > MAX_ELEMENTS {
            return;
        }
        let name = tag.name.as_str();
        if name == "body" {
            self.close_nearest(&["head"], &[]);
        }
        match name {
            "li" => self.close_nearest(&["li"], &["ul", "ol", "table", "td", "th"]),
            "dt" | "dd" => self.close_nearest(&["dt", "dd"], &["dl", "table"]),
            "td" | "th" => self.close_nearest(&["td", "th"], &["tr", "table"]),
            "tr" => self.close_nearest(&["tr"], &["table"]),
            "thead" | "tbody" | "tfoot" => self.close_nearest(&["thead", "tbody", "tfoot"], &["table"]),
            _ => {}
        }
        if closes_p(name) {
            self.close_nearest(&["p"], &["table", "td", "th", "button", "caption", "object", "template"]);
        }
        let Some(parent) = self.stack.last_mut() else { return };
        parent.children = parent.children.saturating_add(1);
        let index = parent.children;
        let type_index = if let Some(at) = parent.by_type.iter().position(|(n, _)| n == name) {
            parent.by_type.get_mut(at).map_or(0, |(_, c)| {
                *c = c.saturating_add(1);
                *c
            })
        } else if parent.by_type.len() < MAX_TYPES {
            parent.by_type.push((name.to_string(), 1));
            1
        } else {
            // Past that many different names among the children, `:nth-of-type` can't tell.
            0
        };
        let parent_at = parent.el;
        let id = u32::try_from(self.els.len()).unwrap_or(NONE);
        let prev = self.els.get(parent_at as usize).map_or(NONE, |p| p.last_child);
        if let Some(p) = self.els.get_mut(parent_at as usize) {
            p.last_child = id;
        }
        let mut el = El {
            name: tag.name.clone(),
            id: None,
            classes: Vec::new(),
            attrs: Vec::new(),
            lower: Vec::new(),
            parent: parent_at,
            prev,
            index,
            type_index,
            last_child: NONE,
        };
        // The first of an attribute counts, as in a browser.
        let (mut seen_id, mut seen_class) = (false, false);
        for (attr, value) in tag.attributes() {
            let attr = attr.to_ascii_lowercase();
            match attr.as_str() {
                "id" if !seen_id => {
                    seen_id = true;
                    el.id = Some(value.trim().to_ascii_lowercase()).filter(|v| !v.is_empty());
                }
                "class" if !seen_class => {
                    seen_class = true;
                    el.classes = value.split_ascii_whitespace().take(MAX_CLASSES).map(str::to_ascii_lowercase).collect();
                }
                "id" | "class" | "style" => {}
                _ if value.len() <= MAX_ELEMENT_ATTR_VALUE && attr.len() <= 32 && !el.attrs.iter().any(|(n, _)| *n == attr) => {
                    el.attrs.push((attr, value));
                }
                _ => {}
            }
        }
        self.els.push(el);
        if !is_void(name) && self.stack.len() < MAX_DEPTH {
            match self.positions.get_mut(name) {
                Some(list) => list.push(self.stack.len()),
                None => {
                    self.positions.insert(name.to_string(), vec![self.stack.len()]);
                }
            }
            self.stack.push(Open { el: id, children: 0, by_type: Vec::new() });
        }
    }

    fn close(&mut self, name: &str) {
        if is_void(name) || matches!(name, "html" | "body") {
            return;
        }
        let crossing: &[&str] = if matches!(name, "td" | "th" | "tr" | "table" | "tbody" | "thead" | "tfoot") { &[] } else { &["td", "th", "table"] };
        self.close_nearest(&[name], crossing);
    }
}

// ---- selectors ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Combinator {
    Descendant,
    Child,
    Next,
    Later,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttrOp {
    Exists,
    Equals,
    Word,
    Dash,
    Prefix,
    Suffix,
    Contains,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AttrSel {
    name: String,
    op: AttrOp,
    value: String,
    fold: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Simple {
    Tag(String),
    Id(String),
    Class(String),
    Attr(AttrSel),
    FirstChild,
    LastChild,
    OnlyChild,
    /// `a n + b` of the element's place among its siblings, or among those of its name.
    Nth {
        a: i64,
        b: i64,
        of_type: bool,
    },
    Not(Vec<Vec<Simple>>),
    Root,
    Link,
    /// A pseudo-class or element that has no meaning for mail on a page that is not interacted
    /// with: `:visited`, `:hover`, `::before`...
    Never,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct Compound {
    simples: Vec<Simple>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Selector {
    /// Left to right.
    compounds: Vec<Compound>,
    /// `combinators[i]` joins `compounds[i]` and `compounds[i + 1]`.
    combinators: Vec<Combinator>,
    specificity: u32,
}

enum Key {
    Id(String),
    Class(String),
    Tag(String),
    Any,
}

impl Selector {
    /// What the rightmost compound can be found by.
    fn key(&self) -> Key {
        let Some(last) = self.compounds.last() else { return Key::Any };
        if let Some(id) = last.simples.iter().find_map(|s| if let Simple::Id(id) = s { Some(id) } else { None }) {
            return Key::Id(id.clone());
        }
        if let Some(class) = last.simples.iter().find_map(|s| if let Simple::Class(c) = s { Some(c) } else { None }) {
            return Key::Class(class.clone());
        }
        if let Some(tag) = last.simples.iter().find_map(|s| if let Simple::Tag(t) = s { Some(t) } else { None }) {
            return Key::Tag(tag.clone());
        }
        Key::Any
    }
}

/// A selector's text being read.
struct Cursor<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Cursor<'a> {
    fn rest(&self) -> &'a str {
        self.text.get(self.at..).unwrap_or("")
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.at += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn skip_space(&mut self) -> bool {
        let before = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.at += 1;
        }
        self.at > before
    }

    fn done(&self) -> bool {
        self.at >= self.text.len()
    }

    /// A name: letters, digits, `-`, `_`, non-ASCII, and escaped characters. Lower case.
    fn ident(&mut self) -> Option<String> {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c == '\\' {
                self.at += 1;
                let Some(next) = self.bump() else { break };
                if next.is_ascii_hexdigit() {
                    let mut hex = String::from(next);
                    while hex.len() < 6 && self.peek().is_some_and(|h| h.is_ascii_hexdigit()) {
                        hex.push(self.bump().unwrap_or('0'));
                    }
                    if self.peek().is_some_and(|w| w.is_ascii_whitespace()) {
                        self.at += 1;
                    }
                    out.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).filter(|c| *c != '\0').unwrap_or('\u{fffd}'));
                } else if next != '\n' {
                    out.push(next);
                }
            } else if c.is_ascii_alphanumeric() || c == '-' || c == '_' || !c.is_ascii() {
                self.at += c.len_utf8();
                out.push(c);
            } else {
                break;
            }
            if out.len() > 128 {
                return None;
            }
        }
        (!out.is_empty()).then(|| out.to_lowercase())
    }

    /// The text inside parentheses, the opening one already read; the closing one is read too.
    fn parenthesised(&mut self) -> Option<&'a str> {
        let start = self.at;
        let end = close_at(self.text.as_bytes(), start, b')')?;
        self.at = end + 1;
        self.text.get(start..end)
    }
}

/// Where the `close` that ends what starts at `from` is, skipping strings and nested brackets.
fn close_at(b: &[u8], from: usize, close: u8) -> Option<usize> {
    let mut depth = 0u32;
    let mut quote = 0u8;
    let mut i = from;
    while let Some(&c) = b.get(i) {
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote {
                quote = 0;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = c,
                b'\\' => i += 1,
                b'(' | b'[' => depth = depth.saturating_add(1),
                b')' | b']' => {
                    if depth == 0 {
                        return (c == close).then_some(i);
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Split at the commas outside parentheses, brackets and strings.
fn split_commas(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let (mut out, mut start, mut depth, mut quote) = (Vec::new(), 0usize, 0u32, 0u8);
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote {
                quote = 0;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = c,
                b'\\' => i += 1,
                b'(' | b'[' => depth = depth.saturating_add(1),
                b')' | b']' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    out.push(text.get(start..i).unwrap_or(""));
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    out.push(text.get(start..).unwrap_or(""));
    out
}

/// Pseudo-classes and elements that never match here.
const NEVER: [&str; 32] = [
    "visited",
    "hover",
    "focus",
    "active",
    "focus-within",
    "focus-visible",
    "target",
    "checked",
    "disabled",
    "enabled",
    "before",
    "after",
    "first-line",
    "first-letter",
    "selection",
    "placeholder",
    "marker",
    "backdrop",
    "file-selector-button",
    "indeterminate",
    "valid",
    "invalid",
    "required",
    "optional",
    "read-only",
    "read-write",
    "default",
    "in-range",
    "out-of-range",
    "autofill",
    "-webkit-autofill",
    "user-invalid",
];

/// The `a`, `b` of `an+b`.
fn nth(text: &str) -> Option<(i64, i64)> {
    let t: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect::<String>().to_ascii_lowercase();
    let clamp = |n: i64| n.clamp(-1_000_000, 1_000_000);
    match t.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        "" => return None,
        _ => {}
    }
    let number = |s: &str| s.trim_start_matches('+').parse::<i64>().ok().map(clamp);
    match t.split_once('n') {
        None => number(&t).map(|b| (0, b)),
        Some((a, b)) => {
            let a = match a {
                "" | "+" => 1,
                "-" => -1,
                other => number(other)?,
            };
            let b = if b.is_empty() { 0 } else { number(b)? };
            Some((a, b))
        }
    }
}

fn simple_specificity(s: &Simple) -> (u32, u32, u32) {
    match s {
        Simple::Id(_) => (1, 0, 0),
        Simple::Tag(_) => (0, 0, 1),
        Simple::Not(list) => list.iter().map(|c| compound_specificity(c)).max().unwrap_or((0, 0, 0)),
        _ => (0, 1, 0),
    }
}

fn compound_specificity(simples: &[Simple]) -> (u32, u32, u32) {
    simples.iter().map(simple_specificity).fold((0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2))
}

fn pack(a: u32, b: u32, c: u32) -> u32 {
    (a.min(255) << 16) | (b.min(255) << 8) | c.min(255)
}

/// One simple selector at the cursor: `Err` when it isn't understood, `Ok(None)` when there is
/// none (a combinator, the end).
fn simple(cur: &mut Cursor<'_>, depth: usize) -> Result<Option<Simple>, ()> {
    let Some(first) = cur.peek() else { return Ok(None) };
    match first {
        '#' => {
            cur.bump();
            Ok(Some(Simple::Id(cur.ident().ok_or(())?)))
        }
        '.' => {
            cur.bump();
            Ok(Some(Simple::Class(cur.ident().ok_or(())?)))
        }
        '[' => {
            cur.bump();
            let end = close_at(cur.text.as_bytes(), cur.at, b']').ok_or(())?;
            let inner = cur.text.get(cur.at..end).ok_or(())?;
            cur.at = end + 1;
            Ok(Some(Simple::Attr(attribute(inner).ok_or(())?)))
        }
        ':' => {
            cur.bump();
            let element = cur.eat(':');
            let name = cur.ident().ok_or(())?;
            let args = if cur.eat('(') { Some(cur.parenthesised().ok_or(())?) } else { None };
            if element || NEVER.contains(&name.as_str()) {
                return Ok(Some(Simple::Never));
            }
            let found = match (name.as_str(), args) {
                ("first-child", None) => Simple::FirstChild,
                ("last-child", None) => Simple::LastChild,
                ("only-child", None) => Simple::OnlyChild,
                ("first-of-type", None) => Simple::Nth { a: 0, b: 1, of_type: true },
                ("root", None) => Simple::Root,
                ("link" | "any-link", None) => Simple::Link,
                ("nth-child", Some(a)) => nth(a).map(|(a, b)| Simple::Nth { a, b, of_type: false }).ok_or(())?,
                ("nth-of-type", Some(a)) => nth(a).map(|(a, b)| Simple::Nth { a, b, of_type: true }).ok_or(())?,
                ("not", Some(a)) if depth < MAX_NOT_DEPTH => {
                    let mut list = Vec::new();
                    let parts = split_commas(a);
                    if parts.len() > MAX_NOT_LIST {
                        return Err(());
                    }
                    for part in parts {
                        let mut inner = Cursor { text: part.trim(), at: 0 };
                        let one = compound(&mut inner, depth + 1).ok_or(())?;
                        if !inner.done() {
                            return Err(());
                        }
                        list.push(one.simples);
                    }
                    Simple::Not(list)
                }
                _ => return Err(()),
            };
            Ok(Some(found))
        }
        _ => Ok(None),
    }
}

fn attribute(inner: &str) -> Option<AttrSel> {
    let mut cur = Cursor { text: inner, at: 0 };
    cur.skip_space();
    let name = cur.ident()?;
    cur.skip_space();
    if cur.done() {
        return Some(AttrSel { name, op: AttrOp::Exists, value: String::new(), fold: false });
    }
    let op = match cur.bump()? {
        '=' => AttrOp::Equals,
        c @ ('~' | '|' | '^' | '$' | '*') => {
            if !cur.eat('=') {
                return None;
            }
            match c {
                '~' => AttrOp::Word,
                '|' => AttrOp::Dash,
                '^' => AttrOp::Prefix,
                '$' => AttrOp::Suffix,
                _ => AttrOp::Contains,
            }
        }
        _ => return None,
    };
    cur.skip_space();
    let value = match cur.peek()? {
        q @ ('"' | '\'') => {
            cur.bump();
            let rest = cur.rest();
            let end = rest.find(q)?;
            let v = rest.get(..end)?.to_string();
            cur.at += end + 1;
            v
        }
        _ => cur.ident()?,
    };
    cur.skip_space();
    let fold = match cur.peek() {
        Some('i' | 'I') => {
            cur.bump();
            true
        }
        Some('s' | 'S') => {
            cur.bump();
            false
        }
        _ => false,
    };
    cur.skip_space();
    if !cur.done() || value.len() > MAX_ATTR_VALUE * 2 {
        return None;
    }
    let value = if fold { value.to_ascii_lowercase() } else { value };
    Some(AttrSel { name, op, value, fold })
}

/// A compound selector at the cursor. `None` when it reads nothing or something in it isn't
/// understood.
fn compound(cur: &mut Cursor<'_>, depth: usize) -> Option<Compound> {
    let start = cur.at;
    let mut simples = Vec::new();
    if cur.eat('*') {
        // Matches anything; no namespace is understood.
        if cur.peek() == Some('|') {
            return None;
        }
    } else if cur.peek().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '\\' || !c.is_ascii()) {
        let name = cur.ident()?;
        if cur.peek() == Some('|') {
            return None;
        }
        simples.push(Simple::Tag(name));
    }
    while let Some(found) = simple(cur, depth).ok()? {
        simples.push(found);
        if simples.len() > MAX_SIMPLES {
            return None;
        }
    }
    (cur.at > start).then_some(Compound { simples })
}

/// One selector of a list.
fn selector(text: &str) -> Option<Selector> {
    let mut cur = Cursor { text: text.trim(), at: 0 };
    let (mut compounds, mut combinators) = (Vec::new(), Vec::new());
    let (mut ids, mut classes, mut tags) = (0u32, 0u32, 0u32);
    loop {
        let one = compound(&mut cur, 0)?;
        let (a, b, c) = compound_specificity(&one.simples);
        ids += a;
        classes += b;
        tags += c;
        compounds.push(one);
        if compounds.len() > MAX_PARTS {
            return None;
        }
        let space = cur.skip_space();
        if cur.done() {
            break;
        }
        let combinator = match cur.peek()? {
            '>' => Combinator::Child,
            '+' => Combinator::Next,
            '~' => Combinator::Later,
            _ if space => {
                combinators.push(Combinator::Descendant);
                continue;
            }
            _ => return None,
        };
        cur.bump();
        cur.skip_space();
        if cur.done() {
            return None;
        }
        combinators.push(combinator);
    }
    Some(Selector { compounds, combinators, specificity: pack(ids, classes, tags) })
}

// ---- rules ----

#[derive(Debug, PartialEq, Eq)]
struct Decl {
    property: String,
    value: String,
    important: bool,
}

#[derive(Debug)]
pub(crate) struct Rule {
    selector: Selector,
    decls: Rc<[Decl]>,
    /// What the ancestors that the selector asks for (to the left of descendant and child
    /// combinators) have to have among their ids, classes and tags, hashed: an element none of
    /// whose ancestors could have them cannot match, and is not looked at further.
    ancestors: Vec<u64>,
    /// It is in a block for dark windows (`@media (prefers-color-scheme: dark)`, or a `<style>`
    /// whose `media` says so) that holds for this window.
    dark: bool,
}

fn hash_key(kind: u8, name: &str) -> u64 {
    // FNV-1a.
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ u64::from(kind);
    h = h.wrapping_mul(0x0100_0000_01b3);
    for b in name.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn compound_key(c: &Compound) -> Option<u64> {
    c.simples
        .iter()
        .find_map(|s| if let Simple::Id(id) = s { Some(hash_key(b'#', id)) } else { None })
        .or_else(|| c.simples.iter().find_map(|s| if let Simple::Class(k) = s { Some(hash_key(b'.', k)) } else { None }))
        .or_else(|| c.simples.iter().find_map(|s| if let Simple::Tag(t) = s { Some(hash_key(b't', t)) } else { None }))
}

fn ancestor_keys(sel: &Selector) -> Vec<u64> {
    let mut out = Vec::new();
    let mut k = sel.compounds.len().saturating_sub(1);
    while k > 0 {
        match sel.combinators.get(k - 1) {
            Some(Combinator::Descendant | Combinator::Child) => {
                if let Some(h) = sel.compounds.get(k - 1).and_then(compound_key) {
                    out.push(h);
                }
                k -= 1;
            }
            _ => break,
        }
    }
    out
}

/// A set of hashed names, 256 bits, two bits to a name.
#[derive(Clone, Copy, Default)]
struct Bloom([u64; 4]);

impl Bloom {
    fn bits(h: u64) -> [usize; 2] {
        [(h % 256) as usize, ((h >> 24) % 256) as usize]
    }

    fn add(&mut self, h: u64) {
        for b in Bloom::bits(h) {
            if let Some(w) = self.0.get_mut(b / 64) {
                *w |= 1 << (b % 64);
            }
        }
    }

    fn has(&self, h: u64) -> bool {
        Bloom::bits(h).iter().all(|b| self.0.get(b / 64).is_some_and(|w| w & (1 << (b % 64)) != 0))
    }

    fn or(&mut self, other: &Bloom) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a |= b;
        }
    }
}

/// Whether a property is one the whitelist ([`crate::style`]) can use.
fn kept(property: &str) -> bool {
    let sided = |base: &str| {
        property
            .strip_prefix(base)
            .is_some_and(|rest| rest.is_empty() || matches!(rest, "-top" | "-right" | "-bottom" | "-left" | "-block-start" | "-block-end"))
    };
    matches!(
        property,
        "color"
            | "background"
            | "background-color"
            | "font"
            | "font-weight"
            | "font-style"
            | "font-size"
            | "font-family"
            | "text-decoration"
            | "text-decoration-line"
            | "text-align"
            | "display"
            | "visibility"
            | "opacity"
            | "max-height"
            | "max-width"
            | "overflow"
            | "overflow-x"
            | "overflow-y"
            | "height"
            | "width"
            | "line-height"
            | "vertical-align"
            | "border-spacing"
            | "border-width"
            | "border-style"
            | "border-color"
            | "border-radius"
    ) || sided("margin")
        || sided("padding")
        || sided("border")
        || ["top", "right", "bottom", "left"].iter().any(|side| {
            property.strip_prefix("border-").and_then(|r| r.strip_prefix(side)).is_some_and(|r| matches!(r, "-width" | "-style" | "-color"))
        })
}

/// Skip white space, comments and the `<!--` and `-->` that old sheets are wrapped in.
fn skip_trivia(b: &[u8], mut i: usize) -> usize {
    loop {
        match b.get(i) {
            Some(c) if c.is_ascii_whitespace() => i += 1,
            Some(b'/') if b.get(i + 1) == Some(&b'*') => {
                i = find(b, i + 2, b"*/").map_or(b.len(), |e| e + 2);
            }
            Some(b'<') if b.get(i..i + 4) == Some(b"<!--") => i += 4,
            Some(b'-') if b.get(i..i + 3) == Some(b"-->") => i += 3,
            _ => return i,
        }
    }
}

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    b.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// The first of `stops` outside strings, comments and brackets, at or after `from`.
fn scan_to(b: &[u8], from: usize, stops: &[u8]) -> Option<usize> {
    let (mut i, mut depth, mut quote) = (from, 0u32, 0u8);
    while let Some(&c) = b.get(i) {
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote || c == b'\n' {
                quote = 0;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = find(b, i + 2, b"*/").map_or(b.len(), |e| e + 1);
        } else {
            match c {
                b'"' | b'\'' => quote = c,
                b'\\' => i += 1,
                b'(' | b'[' => depth = depth.saturating_add(1),
                b')' | b']' => depth = depth.saturating_sub(1),
                _ if depth == 0 && stops.contains(&c) => return Some(i),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// The `}` that closes the block whose `{` is at `open`, or the end.
fn block_end(b: &[u8], open: usize) -> usize {
    let (mut i, mut depth, mut quote) = (open + 1, 1u32, 0u8);
    while let Some(&c) = b.get(i) {
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote || c == b'\n' {
                quote = 0;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = find(b, i + 2, b"*/").map_or(b.len(), |e| e + 1);
        } else {
            match c {
                b'"' | b'\'' => quote = c,
                b'\\' => i += 1,
                b'{' => depth = depth.saturating_add(1),
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    b.len()
}

/// The declarations of a block that the whitelist can use.
fn declarations(block: &str) -> Vec<Decl> {
    let b = block.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        i = skip_trivia(b, i);
        let Some(end) = scan_to(b, i, b";{") else {
            read_declaration(block.get(i..).unwrap_or(""), &mut out);
            break;
        };
        if b.get(end) == Some(&b'{') {
            // A nested rule: not understood, and skipped whole.
            i = block_end(b, end) + 1;
            continue;
        }
        read_declaration(block.get(i..end).unwrap_or(""), &mut out);
        i = end + 1;
    }
    out
}

fn read_declaration(text: &str, out: &mut Vec<Decl>) {
    let Some((property, value)) = text.split_once(':') else { return };
    let property = property.trim().to_ascii_lowercase();
    if property.len() > 32 || !kept(&property) {
        return;
    }
    let (value, important) = crate::style::strip_important(value);
    let lower = value.to_ascii_lowercase();
    let background = property.starts_with("background");
    if value.is_empty()
        || value.len() > MAX_VALUE
        || (!background && lower.contains("url("))
        || lower.contains("expression(")
        || lower.contains("image-set(")
        || value.contains(['{', '}', '\0'])
    {
        return;
    }
    out.push(Decl { property, value: value.to_string(), important });
}

/// What reading the media queries of a sheet came to.
#[derive(Debug, Default)]
pub(crate) struct Media {
    /// Media lists tested so far, against [`media::MAX_LISTS`].
    lists: usize,
}

/// The rules of a style sheet for a wide pane in the light theme, at most [`MAX_RULES`], from at
/// most [`MAX_SHEET`] bytes.
pub(crate) fn rules(text: &str) -> Vec<Rule> {
    rules_in(text, &Env::default(), &mut Media::default())
}

/// The rules of a style sheet that hold for `env`: those outside `@media`, and those inside a
/// block whose media queries are true for it. At most [`MAX_RULES`], from at most [`MAX_SHEET`] bytes.
pub(crate) fn rules_in(text: &str, env: &Env, media: &mut Media) -> Vec<Rule> {
    let mut out = Vec::new();
    read_rules(truncate(text, MAX_SHEET), env, media, 1, &mut out);
    out
}

/// The text after `@media`, when the at-rule's prelude is one.
fn media_prelude(prelude: &str) -> Option<&str> {
    let rest = prelude.get(..6).filter(|name| name.eq_ignore_ascii_case("@media")).and_then(|_| prelude.get(6..))?;
    // `@media(…)` and `@media screen`, not `@mediaquery`.
    match rest.bytes().next() {
        Some(c) if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' => None,
        _ => Some(rest),
    }
}

/// The media lists of a message's style sheets, each once: the `media` of each `<style>` that has
/// one, and the lists of its `@media` blocks, nested ones too (to [`media::MAX_NESTING`] levels),
/// at most [`media::MAX_LISTS`]. Read the way [`Sheet::read_in`] finds the sheets, so that a
/// message has a list here exactly when its look can depend on the pane or the theme.
pub(crate) fn media_lists(html: &str) -> Vec<String> {
    let mut lists = Lists::default();
    if !has_style(html.as_bytes()) {
        return lists.out;
    }
    let mut tokens = Tokenizer::new(html);
    let mut template = 0usize;
    let mut size = 0usize;
    let mut take = |tokens: &mut Tokenizer<'_>, template: usize, lists: &mut Lists| {
        take_styles(tokens, template, &mut size, |media, css| {
            lists.add(media);
            collect_lists(css, 1, lists);
        });
    };
    while let Some(token) = tokens.next() {
        take(&mut tokens, template, &mut lists);
        match token {
            Token::Open(tag) if tag.name == "template" => template = template.saturating_add(1),
            Token::Close(name) if name == "template" => template = template.saturating_sub(1),
            _ => {}
        }
    }
    take(&mut tokens, template, &mut lists);
    lists.out
}

/// The `<style>` sheets the tokenizer has found since it was last asked, in the one way every
/// reader of a message's sheets takes them (the cascade and the list of media it depends on, so
/// that the two never disagree about which are read): none in a `<template>` (`template` is the
/// depth of those open), and each cut to what is left of [`MAX_SHEET`] bytes for the whole
/// message, `size` being what the sheets before it took, whatever their `media` says. `each` gets
/// a sheet's `media` attribute and its text.
fn take_styles(tokens: &mut Tokenizer<'_>, template: usize, size: &mut usize, mut each: impl FnMut(&str, &str)) {
    for (media, css) in std::mem::take(&mut tokens.styles) {
        if template > 0 {
            continue;
        }
        let css = truncate(css, MAX_SHEET.saturating_sub(*size));
        *size = size.saturating_add(css.len());
        each(&media, css);
    }
}

#[derive(Default)]
struct Lists {
    out: Vec<String>,
    seen: std::collections::HashSet<String>,
}

impl Lists {
    fn add(&mut self, list: &str) {
        let list = list.trim();
        if !list.is_empty() && self.out.len() < media::MAX_LISTS && self.seen.insert(list.to_string()) {
            self.out.push(list.to_string());
        }
    }
}

/// Add the media lists of the `@media` blocks of `text` (all of them, whatever they say).
fn collect_lists(text: &str, level: usize, lists: &mut Lists) {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() && lists.out.len() < media::MAX_LISTS {
        i = skip_trivia(b, i);
        match b.get(i) {
            None => break,
            Some(b'}') => i += 1,
            Some(b'@') => match scan_to(b, i, b";{") {
                None => break,
                Some(end) if b.get(end) == Some(&b'{') => {
                    let close = block_end(b, end);
                    if let Some(list) = media_prelude(text.get(i..end).unwrap_or(""))
                        && level <= media::MAX_NESTING
                    {
                        lists.add(list);
                        collect_lists(text.get(end + 1..close).unwrap_or(""), level + 1, lists);
                    }
                    i = close + 1;
                }
                Some(end) => i = end + 1,
            },
            Some(_) => match scan_to(b, i, b"{") {
                Some(open) => i = block_end(b, open) + 1,
                None => break,
            },
        }
    }
}

/// Read the rules in `text` into `out`; `level` is how many `@media` blocks deep this text is, from 1.
fn read_rules(text: &str, env: &Env, media: &mut Media, level: usize, out: &mut Vec<Rule>) {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() && out.len() < MAX_RULES {
        i = skip_trivia(b, i);
        match b.get(i) {
            None => break,
            Some(b'}') => i += 1,
            Some(b'@') => {
                // An at-rule: `@import`, `@charset` and the rest end at a `;`; `@font-face`,
                // `@keyframes` and `@supports` have a block, which is skipped. `@media` has one
                // whose rules count when its queries hold for `env`.
                match scan_to(b, i, b";{") {
                    None => break,
                    Some(end) if b.get(end) == Some(&b'{') => {
                        let close = block_end(b, end);
                        if let Some(list) = media_prelude(text.get(i..end).unwrap_or(""))
                            && level <= media::MAX_NESTING
                            && media.lists < media::MAX_LISTS
                        {
                            media.lists += 1;
                            let verdict = media::test(list, env);
                            if verdict.holds {
                                let before = out.len();
                                read_rules(text.get(end + 1..close).unwrap_or(""), env, media, level + 1, out);
                                if verdict.dark {
                                    out.iter_mut().skip(before).for_each(|r| r.dark = true);
                                }
                            }
                        }
                        i = close + 1;
                    }
                    Some(end) => i = end + 1,
                }
            }
            Some(_) => {
                let Some(open) = scan_to(b, i, b"{") else { break };
                let close = block_end(b, open);
                let prelude = text.get(i..open).unwrap_or("");
                let block = text.get(open + 1..close).unwrap_or("");
                i = close + 1;
                let decls = declarations(block);
                if decls.is_empty() {
                    continue;
                }
                let decls: Rc<[Decl]> = decls.into();
                // One selector in the list that isn't understood drops the whole rule, as in CSS.
                let parsed: Option<Vec<Selector>> = split_commas(prelude).into_iter().take(MAX_RULES + 1).map(selector).collect();
                for selector in parsed.unwrap_or_default() {
                    if out.len() >= MAX_RULES {
                        break;
                    }
                    let ancestors = ancestor_keys(&selector);
                    out.push(Rule { selector, decls: decls.clone(), ancestors, dark: false });
                }
            }
        }
    }
}

// ---- matching ----

fn parent(dom: &[El], el: usize) -> Option<usize> {
    let p = dom.get(el)?.parent;
    // (The document, 0, is nothing a selector matches.)
    (p != NONE && p != 0).then_some(p as usize)
}

fn previous(dom: &[El], el: usize) -> Option<usize> {
    let p = dom.get(el)?.prev;
    (p != NONE).then_some(p as usize)
}

/// Whether the simple selector matches. Each costs a step (an attribute test four), and with
/// none left nothing matches.
fn simple_matches(dom: &[El], s: &Simple, at: usize, steps: &mut u32) -> bool {
    let Some(el) = dom.get(at) else { return false };
    // What a test costs follows what it does: a class is looked for among the element's classes,
    // an attribute's value is compared along its length.
    let cost = match s {
        Simple::Class(_) => 1 + el.classes.len() as u32 / CLASSES_PER_STEP,
        Simple::Attr(sel) => 4 + el.attrs.iter().find(|(n, _)| *n == sel.name).map_or(0, |(_, v)| v.len() as u32 / ATTR_BYTES_PER_STEP),
        _ => 1,
    };
    if *steps < cost {
        *steps = 0;
        return false;
    }
    *steps -= cost;
    match s {
        Simple::Tag(name) => el.name == *name,
        Simple::Id(id) => el.id.as_deref() == Some(id.as_str()),
        Simple::Class(class) => el.classes.iter().any(|c| c == class),
        Simple::Attr(sel) => el.attr_value(&sel.name, sel.fold).is_some_and(|v| attribute_matches(sel, v)),
        Simple::FirstChild => el.index == 1,
        Simple::LastChild => dom.get(el.parent as usize).is_some_and(|p| p.last_child as usize == at),
        Simple::OnlyChild => el.index == 1 && dom.get(el.parent as usize).is_some_and(|p| p.last_child as usize == at),
        Simple::Nth { a, b, of_type } => {
            let position = i64::from(if *of_type { el.type_index } else { el.index });
            let offset = position - b;
            if *a == 0 { offset == 0 } else { offset % a == 0 && offset / a >= 0 }
        }
        Simple::Not(list) => {
            for compound in list {
                let mut all = true;
                for inner in compound {
                    if !simple_matches(dom, inner, at, steps) {
                        all = false;
                        break;
                    }
                }
                if *steps == 0 {
                    return false;
                }
                if all {
                    return false;
                }
            }
            true
        }
        Simple::Root => el.parent == 0 && el.name == "html",
        Simple::Link => matches!(el.name.as_str(), "a" | "area") && el.attr("href").is_some(),
        Simple::Never => false,
    }
}

/// `value` is in lower case already when the selector ignores case.
fn attribute_matches(sel: &AttrSel, value: &str) -> bool {
    let want = sel.value.as_str();
    match sel.op {
        AttrOp::Exists => true,
        AttrOp::Equals => value == want,
        AttrOp::Word => !want.is_empty() && !want.contains(char::is_whitespace) && value.split_ascii_whitespace().any(|w| w == want),
        AttrOp::Dash => value == want || value.strip_prefix(want).is_some_and(|rest| rest.starts_with('-')),
        AttrOp::Prefix => !want.is_empty() && value.starts_with(want),
        AttrOp::Suffix => !want.is_empty() && value.ends_with(want),
        AttrOp::Contains => !want.is_empty() && value.contains(want),
    }
}

/// Whether compound `k` of the selector matches `at`, and the compounds to its left match what
/// their combinators say. Each compound tried costs a step; with none left nothing matches.
fn matches_from(dom: &[El], sel: &Selector, k: usize, at: usize, steps: &mut u32) -> bool {
    if *steps == 0 {
        return false;
    }
    *steps -= 1;
    let Some(compound) = sel.compounds.get(k) else { return false };
    for simple in &compound.simples {
        if !simple_matches(dom, simple, at, steps) {
            return false;
        }
    }
    let Some(left) = k.checked_sub(1) else { return true };
    match sel.combinators.get(left) {
        Some(Combinator::Child) => parent(dom, at).is_some_and(|p| matches_from(dom, sel, left, p, steps)),
        Some(Combinator::Next) => previous(dom, at).is_some_and(|p| matches_from(dom, sel, left, p, steps)),
        Some(Combinator::Descendant) => {
            let mut up = parent(dom, at);
            while let Some(p) = up {
                if matches_from(dom, sel, left, p, steps) {
                    return true;
                }
                if *steps == 0 {
                    return false;
                }
                up = parent(dom, p);
            }
            false
        }
        Some(Combinator::Later) => {
            let mut before = previous(dom, at);
            while let Some(p) = before {
                if matches_from(dom, sel, left, p, steps) {
                    return true;
                }
                if *steps == 0 {
                    return false;
                }
                before = previous(dom, p);
            }
            false
        }
        None => false,
    }
}

// ---- the sheet of a message ----

/// The sheet of one message, with the elements it is matched against.
pub(crate) struct Sheet {
    rules: Vec<Rule>,
    by_id: HashMap<String, Vec<u32>>,
    by_class: HashMap<String, Vec<u32>>,
    by_tag: HashMap<String, Vec<u32>>,
    other: Vec<u32>,
    dom: Vec<El>,
    /// Per element, what its ancestors have (empty when no rule asks for ancestors).
    blooms: Vec<Bloom>,
    steps_left: u32,
    /// A rule from a block for dark windows gave a colour or a background to an element that is
    /// shown: the sender designed this for a dark window, and its colours are its own.
    pub dark_design: bool,
    /// Some rule is from a block for dark windows.
    has_dark: bool,
    /// The last element looked at got a colour or a background from a rule for dark windows, and
    /// is shown itself.
    dark_hit: bool,
    /// Per property (colour, background) of the last element looked at: whether the declaration
    /// that wins is from a rule for dark windows, and is `!important`.
    dark_slots: [Option<(bool, bool)>; 2],
    /// What the message was given to spend.
    #[cfg(test)]
    budget: u32,
    /// Looks worked out, for the tests.
    #[cfg(test)]
    builds: u32,
    /// What elements that match the same rules get, worked out once for them all.
    cache: HashMap<Vec<u32>, Option<Rc<Applied>>>,
}

/// What a sheet adds to one element: declarations to put before its `style` attribute, those
/// marked `!important`, to put after it, and what the two make when the element has no `style`
/// of its own.
#[derive(Debug, PartialEq)]
pub(crate) struct Applied {
    before: String,
    after: String,
    pub css: Css,
}

/// What an element's `style` attribute and its sheet's rules ask for, in the order of the
/// cascade: the rules, the attribute's declarations, the rules marked `!important`, the
/// attribute's marked `!important`.
pub(crate) fn css_with(applied: &Applied, inline: Option<&str>) -> Css {
    let Some(inline) = inline else { return applied.css.clone() };
    let inline = truncate(inline, crate::style::MAX_STYLE);
    let (normal, important) = split_important(inline);
    Css::parse_limited(
        &format!("{};{};{};{}", applied.before, normal, applied.after, important),
        2 * MAX_APPLIED + crate::style::MAX_STYLE + 16,
        2 * MAX_DECLARATIONS + crate::style::MAX_DECLARATIONS,
    )
}

/// The declarations of a `style` attribute that are not marked `!important`, and those that are:
/// the first [`crate::style::MAX_DECLARATIONS`] of them, counted as [`Css::parse`] counts them.
fn split_important(style: &str) -> (String, String) {
    let (mut normal, mut important) = (String::new(), String::new());
    for (property, value) in crate::style::declarations(style).take(crate::style::MAX_DECLARATIONS) {
        let to = if crate::style::strip_important(value).1 { &mut important } else { &mut normal };
        to.push_str(property);
        to.push(':');
        to.push_str(value);
        to.push(';');
    }
    (normal, important)
}

fn selector_folds(sel: &Selector) -> bool {
    fn simple_folds(s: &Simple) -> bool {
        match s {
            Simple::Attr(a) => a.fold,
            Simple::Not(list) => list.iter().flatten().any(simple_folds),
            _ => false,
        }
    }
    sel.compounds.iter().flat_map(|c| &c.simples).any(simple_folds)
}

impl Sheet {
    /// The sheet of a message, when it has any rules. Reads `html` once to find its `<style>`
    /// blocks and to keep the shape of its elements.
    pub fn read(html: &str) -> Option<Sheet> {
        Sheet::read_in(html, &Env::default())
    }

    /// The same for a pane and a theme: the rules in `@media` blocks, and in a `<style media>`,
    /// that are true for `env` count, and the rest are left out.
    pub fn read_in(html: &str, env: &Env) -> Option<Sheet> {
        if !has_style(html.as_bytes()) {
            return None;
        }
        // The style sheets, each with whether it is for dark windows (its `media` says so).
        let mut sheets: Vec<(String, bool)> = Vec::new();
        let mut size = 0usize;
        let mut tree = TreeBuilder::new();
        let mut tokens = Tokenizer::new(html);
        // A `<style>` in a `<template>` is never shown, nor read.
        let mut template = 0usize;
        let mut capped = false;
        let mut take = |tokens: &mut Tokenizer<'_>, template: usize| {
            take_styles(tokens, template, &mut size, |media, css| {
                // (A `<style media>` that is not for this pane is not read.)
                let verdict = media::test(media, env);
                if verdict.holds {
                    sheets.push((css.to_string(), verdict.dark));
                }
            });
        };
        while let Some(token) = tokens.next() {
            take(&mut tokens, template);
            match token {
                Token::Open(tag) => {
                    if tag.name == "template" {
                        template = template.saturating_add(1);
                    }
                    if !capped {
                        tree.open(&tag);
                    }
                }
                Token::Close(name) => {
                    if name == "template" {
                        template = template.saturating_sub(1);
                    }
                    if !capped {
                        tree.close(&name);
                    }
                }
                Token::Text(_) => {}
            }
            // Past the cap the tree grows no more (the elements after it are not styled), but the
            // style sheets are still collected.
            capped = capped || tree.els.len() > MAX_ELEMENTS;
        }
        // (A style at the very end has no token after it.)
        take(&mut tokens, template);
        let mut queried = Media::default();
        let mut rules = Vec::new();
        for (css, dark) in &sheets {
            let before = rules.len();
            read_rules(css, env, &mut queried, 1, &mut rules);
            if *dark {
                rules.iter_mut().skip(before).for_each(|r| r.dark = true);
            }
        }
        if rules.is_empty() {
            return None;
        }
        let mut dom = tree.els;
        if rules.iter().any(|r| selector_folds(&r.selector)) {
            for el in &mut dom {
                el.lower = el.attrs.iter().map(|(_, v)| v.to_ascii_lowercase()).collect();
            }
        }
        let mut sheet = Sheet {
            rules: Vec::new(),
            by_id: HashMap::new(),
            by_class: HashMap::new(),
            by_tag: HashMap::new(),
            other: Vec::new(),
            dom,
            blooms: Vec::new(),
            steps_left: steps_for(html.len()),
            dark_design: false,
            has_dark: rules.iter().any(|r| r.dark),
            dark_hit: false,
            dark_slots: [None; 2],
            #[cfg(test)]
            budget: steps_for(html.len()),
            #[cfg(test)]
            builds: 0,
            cache: HashMap::new(),
        };
        for (n, rule) in rules.iter().enumerate() {
            let n = u32::try_from(n).unwrap_or(NONE);
            match rule.selector.key() {
                Key::Id(k) => sheet.by_id.entry(k).or_default().push(n),
                Key::Class(k) => sheet.by_class.entry(k).or_default().push(n),
                Key::Tag(k) => sheet.by_tag.entry(k).or_default().push(n),
                Key::Any => sheet.other.push(n),
            }
        }
        if rules.iter().any(|r| !r.ancestors.is_empty()) {
            // Parents come before their children: each element has its parent's ancestors, and the parent.
            let mut blooms: Vec<Bloom> = Vec::with_capacity(sheet.dom.len());
            for el in &sheet.dom {
                let mut bloom = el.parent_bloom(&blooms);
                if let Some(parent) = sheet.dom.get(el.parent as usize).filter(|_| el.parent != NONE && el.parent != 0) {
                    bloom.add(hash_key(b't', &parent.name));
                    if let Some(id) = &parent.id {
                        bloom.add(hash_key(b'#', id));
                    }
                    for class in &parent.classes {
                        bloom.add(hash_key(b'.', class));
                    }
                }
                blooms.push(bloom);
            }
            sheet.blooms = blooms;
        }
        sheet.rules = rules;
        Some(sheet)
    }

    /// Matching steps spent so far.
    #[cfg(test)]
    pub fn work(&self) -> u32 {
        self.budget - self.steps_left
    }

    /// Put what the sheet says about the `ordinal`th start tag of the message on `tag`.
    pub fn apply(&mut self, ordinal: usize, tag: &mut Tag<'_>) {
        tag.sheet = self.style_for(ordinal, tag.has("style"));
        if !self.dark_hit {
            return;
        }
        // The element's own `style` beats a sheet's plain declaration, and a plain one of its own
        // does not beat a sheet's `!important`.
        let inline = tag.attr("style");
        for (property, value) in crate::style::declarations(inline.as_deref().unwrap_or("")).take(crate::style::MAX_DECLARATIONS) {
            let slot = if property.eq_ignore_ascii_case("color") {
                0
            } else if property.eq_ignore_ascii_case("background") || property.eq_ignore_ascii_case("background-color") {
                1
            } else {
                continue;
            };
            let own_important = crate::style::strip_important(value).1;
            if let Some(Some((dark, important))) = self.dark_slots.get_mut(slot)
                && (own_important || !*important)
            {
                *dark = false;
            }
        }
        // And an element that is not shown has no design (an ancestor's hiding is for the builder).
        let hidden = crate::blocks::left_out(tag, &crate::blocks::css_of(tag));
        self.dark_hit = !hidden && self.dark_slots.iter().flatten().any(|(dark, _)| *dark);
    }

    /// Whether the element last looked at ([`Sheet::apply`]) got a colour or a background from a
    /// rule for dark windows that wins over the rest. Whether it is shown, with its ancestors, is
    /// for whoever builds the blocks to say, who then calls [`Sheet::designed_for_dark`].
    pub fn dark_hit(&self) -> bool {
        self.dark_hit
    }

    /// The sender designed this message for dark windows: a shown element got its colours from
    /// a rule for them.
    pub fn designed_for_dark(&mut self) {
        self.dark_design = true;
    }

    /// What the sheet says about the `ordinal`th start tag of the message, counted from 0, for
    /// an element that has a `style` attribute of its own or not.
    pub fn style_for(&mut self, ordinal: usize, inline: bool) -> Option<Rc<Applied>> {
        // (Nothing is left of the element before, whichever way this returns.)
        self.dark_hit = false;
        self.dark_slots = [None; 2];
        let at = ordinal.checked_add(1)?;
        let el = self.dom.get(at)?;
        if self.steps_left == 0 {
            return None;
        }
        let mut steps = MAX_STEPS_PER_ELEMENT.min(self.steps_left);
        let start = steps;
        let mut lists: Vec<&[u32]> = Vec::new();
        if let Some(list) = el.id.as_ref().and_then(|id| self.by_id.get(id)) {
            lists.push(list);
        }
        for class in &el.classes {
            if let Some(list) = self.by_class.get(class) {
                lists.push(list);
            }
        }
        if let Some(list) = self.by_tag.get(&el.name) {
            lists.push(list);
        }
        lists.push(&self.other);
        let mut found: Vec<(u32, u32)> = Vec::new();
        'candidates: for list in lists {
            for n in list {
                if steps == 0 {
                    break 'candidates;
                }
                steps -= 1;
                let Some(rule) = self.rules.get(*n as usize) else { continue };
                // An ancestor the selector asks for that none of this element's can be: no match.
                if let Some(bloom) = self.blooms.get(at)
                    && rule.ancestors.iter().any(|h| !bloom.has(*h))
                {
                    continue;
                }
                if matches_from(&self.dom, &rule.selector, rule.selector.compounds.len().saturating_sub(1), at, &mut steps) {
                    found.push((rule.selector.specificity, *n));
                }
            }
        }
        found.sort_unstable();
        let mut spent = u64::from(start - steps) + found.len() as u64;
        let key: Vec<u32> = found.iter().map(|(_, n)| *n).collect();
        if self.has_dark && !self.dark_design {
            self.dark_slots = self.dark_wins(&found);
            spent += found.len() as u64;
        }
        let applied = if let Some(hit) = self.cache.get(&key) {
            hit.clone()
        } else {
            let (made, considered) = self.build(&found);
            #[cfg(test)]
            {
                self.builds += u32::from(made.is_some());
            }
            // Working out a look costs about as much as reading what it says.
            // (It reads, sorts and de-duplicates every declaration it considers.)
            spent += made.as_ref().map_or(0, |a| {
                let output = [&a.before, &a.after].iter().map(|p| if p.is_empty() { 0 } else { p.matches(';').count() + 1 }).sum::<usize>();
                MIN_BUILD_STEPS
                    + BUILD_STEPS_PER_DECL * considered as u64
                    + BUILD_STEPS_PER_OUTPUT * output as u64
                    + (a.before.len() + a.after.len()) as u64 * 3 / 2
            });
            if self.cache.len() < MAX_CACHED {
                self.cache.insert(key, made.clone());
            }
            made
        };
        // (Whether the element is shown is for [`Sheet::apply`] and whoever builds the blocks.)
        self.dark_hit = self.dark_slots.iter().flatten().any(|(dark, _)| *dark);
        if inline && let Some(a) = &applied {
            // Read again with the element's own style, which a hit saves nothing of.
            spent += (a.before.len() + a.after.len()) as u64 / 4;
        }
        self.steps_left = self.steps_left.saturating_sub(u32::try_from(spent).unwrap_or(u32::MAX));
        applied
    }

    /// Whether a colour or a background the rules `found` (by specificity and order) give is the
    /// one of a rule for dark windows: the declaration that wins for the property is from one
    /// (`!important` over the rest, then the last).
    fn dark_wins(&self, found: &[(u32, u32)]) -> [Option<(bool, bool)>; 2] {
        // The colour, and the background (`background` and `background-color` are one here): who
        // won among the `!important` declarations, and among the others.
        let mut won: [(Option<bool>, Option<bool>); 2] = [(None, None); 2];
        let mut considered = 0usize;
        'rules: for (_, n) in found.iter().rev() {
            let Some(rule) = self.rules.get(*n as usize) else { continue };
            for decl in rule.decls.iter().rev() {
                if considered >= MAX_CONSIDERED {
                    break 'rules;
                }
                considered += 1;
                let at = match decl.property.as_str() {
                    "color" => 0,
                    "background" | "background-color" => 1,
                    _ => continue,
                };
                if let Some((important, normal)) = won.get_mut(at) {
                    let slot = if decl.important { important } else { normal };
                    slot.get_or_insert(rule.dark);
                }
            }
        }
        won.map(|(important, normal)| match (important, normal) {
            (Some(dark), _) => Some((dark, true)),
            (None, Some(dark)) => Some((dark, false)),
            _ => None,
        })
    }

    /// The look the rules `found` (by specificity and order) give together.
    fn build(&self, found: &[(u32, u32)]) -> (Option<Rc<Applied>>, usize) {
        let (mut normal, mut important): (Vec<&Decl>, Vec<&Decl>) = (Vec::new(), Vec::new());
        let mut considered = 0usize;
        // The last rules count the most: when there are too many, the first are left.
        'rules: for (_, n) in found.iter().rev() {
            let Some(rule) = self.rules.get(*n as usize) else { continue };
            for decl in rule.decls.iter().rev() {
                if considered >= MAX_CONSIDERED {
                    break 'rules;
                }
                considered += 1;
                if decl.important {
                    important.push(decl);
                } else {
                    normal.push(decl);
                }
            }
        }
        let (before, after) = (flatten(&normal), flatten(&important));
        if before.is_empty() && after.is_empty() {
            return (None, considered);
        }
        let css = Css::parse_limited(&format!("{before};{after}"), 2 * MAX_APPLIED + 16, 2 * MAX_DECLARATIONS);
        (Some(Rc::new(Applied { before, after, css })), considered)
    }
}

/// Declarations given last-first, as `property:value;` in cascade order, each property once, at
/// most [`MAX_DECLARATIONS`] of them and [`MAX_APPLIED`] bytes: the last, which count the most,
/// are the ones kept, and none is cut through.
fn flatten(last_first: &[&Decl]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    let mut bytes = 0usize;
    for decl in last_first {
        if seen.contains(&decl.property.as_str()) {
            continue;
        }
        let part = format!("{}:{}", decl.property, decl.value);
        if bytes + part.len() + 1 > MAX_APPLIED || parts.len() >= MAX_DECLARATIONS {
            break;
        }
        seen.push(&decl.property);
        bytes += part.len() + 1;
        parts.push(part);
    }
    parts.reverse();
    parts.join(";")
}

/// Whether `<style` (in any letter case) is in the text at all.
fn has_style(b: &[u8]) -> bool {
    b.windows(6).any(|w| w.first() == Some(&b'<') && w.get(1..6).is_some_and(|s| s.eq_ignore_ascii_case(b"style")))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod media_tests;
