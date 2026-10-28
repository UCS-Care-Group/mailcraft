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
//! * the cascade: specificity, then source order, with `!important` over all; a message's own
//!   `style` attribute comes after the sheet's normal declarations and before its `!important`
//!   ones. Inheritance is the builder's, as for a `style` attribute.
//!
//! Everything is hostile and bounded: [`MAX_SHEET`] bytes of sheet text per message, [`MAX_RULES`]
//! rules, [`MAX_PARTS`] compound selectors in a selector (a longer one is dropped), a count of
//! matching steps per element ([`MAX_STEPS_PER_ELEMENT`]; a rule past it skips that element) and
//! per message ([`MAX_STEPS`]; the elements after it are not styled), [`MAX_ELEMENTS`] elements
//! and [`MAX_DEPTH`] levels of them. The look is worked out once, while the message is turned
//! into blocks, and lives in the blocks.

use std::collections::HashMap;
use std::rc::Rc;

use crate::tokenizer::{Tag, Token, Tokenizer};
use crate::truncate;

/// Most bytes of style sheet read from one message, all its `<style>` blocks together.
pub(crate) const MAX_SHEET: usize = 64 * 1024;

/// Most rules (a selector with its declarations; `a, b {}` is two) kept from one message.
pub(crate) const MAX_RULES: usize = 4_000;

/// Most compound selectors (the parts between combinators) in a selector; a longer one is dropped.
pub(crate) const MAX_PARTS: usize = 16;

/// Most matching steps spent on one element.
pub(crate) const MAX_STEPS_PER_ELEMENT: u32 = 400;

/// Most matching steps spent on a whole message.
pub(crate) const MAX_STEPS: u32 = 2_000_000;

/// Most elements a message's tree keeps for matching; the ones after are not styled.
const MAX_ELEMENTS: usize = 150_000;

/// Deepest nesting the tree keeps; deeper elements are leaves.
const MAX_DEPTH: usize = 512;

/// Most simple selectors in one compound selector, and most levels of `:not()`.
const MAX_SIMPLES: usize = 32;
const MAX_NOT_DEPTH: usize = 2;

/// Most declarations, and most bytes of them, one element takes from the sheet on each side of
/// its `style` attribute; and most declarations looked at to find them.
const MAX_DECLARATIONS: usize = 40;
const MAX_APPLIED: usize = 1_500;
const MAX_CONSIDERED: usize = 600;

/// Longest value of a declaration kept, and of an attribute kept for matching.
const MAX_VALUE: usize = 1_024;
const MAX_ATTR_VALUE: usize = 256;

/// Most classes kept for an element.
const MAX_CLASSES: usize = 16;

/// Elements without content or an end tag.
const VOID: [&str; 14] = ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];

/// Elements that close an open `<p>` as they start.
const CLOSES_P: [&str; 28] = [
    "p",
    "div",
    "ul",
    "ol",
    "table",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "pre",
    "hr",
    "form",
    "dl",
    "address",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "aside",
    "center",
    "fieldset",
    "figure",
    "main",
    "menu",
];

const NONE: u32 = u32::MAX;

// ---- the tree ----

/// One element of the message, as far as matching needs it.
#[derive(Debug)]
struct El {
    name: String,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, String)>,
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
}

struct Open {
    el: u32,
    name: String,
    children: u32,
    by_type: HashMap<String, u32>,
}

struct TreeBuilder {
    els: Vec<El>,
    stack: Vec<Open>,
}

impl TreeBuilder {
    fn new() -> TreeBuilder {
        TreeBuilder { els: vec![El::document()], stack: vec![Open { el: 0, name: String::new(), children: 0, by_type: HashMap::new() }] }
    }

    /// Close the nearest open element named in `names`, and what is inside it, unless one of
    /// `barriers` is met first.
    fn close_nearest(&mut self, names: &[&str], barriers: &[&str]) {
        let mut at = self.stack.len();
        while at > 1 {
            at -= 1;
            let Some(open) = self.stack.get(at) else { return };
            if names.contains(&open.name.as_str()) {
                self.stack.truncate(at);
                return;
            }
            if barriers.contains(&open.name.as_str()) {
                return;
            }
        }
    }

    fn open(&mut self, tag: &Tag<'_>) {
        // Elements past the cap are not recorded, so the order of the ones before is kept.
        if self.els.len() > MAX_ELEMENTS {
            return;
        }
        let name = tag.name.as_str();
        match name {
            "li" => self.close_nearest(&["li"], &["ul", "ol", "table", "td", "th"]),
            "dt" | "dd" => self.close_nearest(&["dt", "dd"], &["dl", "table"]),
            "td" | "th" => self.close_nearest(&["td", "th"], &["tr", "table"]),
            "tr" => self.close_nearest(&["tr"], &["table"]),
            "thead" | "tbody" | "tfoot" => self.close_nearest(&["thead", "tbody", "tfoot"], &["table"]),
            _ => {}
        }
        if CLOSES_P.contains(&name) {
            self.close_nearest(&["p"], &["table", "td", "th", "button", "caption", "object", "template"]);
        }
        let Some(parent) = self.stack.last_mut() else { return };
        parent.children = parent.children.saturating_add(1);
        let index = parent.children;
        let slot = parent.by_type.entry(name.to_string()).or_insert(0);
        *slot = slot.saturating_add(1);
        let type_index = *slot;
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
            parent: parent_at,
            prev,
            index,
            type_index,
            last_child: NONE,
        };
        for (attr, value) in tag.attributes() {
            let attr = attr.to_ascii_lowercase();
            match attr.as_str() {
                "id" => el.id = Some(value.trim().to_ascii_lowercase()).filter(|v| !v.is_empty()),
                "class" => {
                    el.classes = value.split_ascii_whitespace().take(MAX_CLASSES).map(str::to_ascii_lowercase).collect();
                }
                "style" => {}
                _ if value.len() <= MAX_ATTR_VALUE && attr.len() <= 32 => el.attrs.push((attr, value)),
                _ => {}
            }
        }
        self.els.push(el);
        if !VOID.contains(&name) && self.stack.len() < MAX_DEPTH {
            self.stack.push(Open { el: id, name: tag.name.clone(), children: 0, by_type: HashMap::new() });
        }
    }

    fn close(&mut self, name: &str) {
        if VOID.contains(&name) || matches!(name, "html" | "body" | "head") {
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
                ("only-child", None) => Simple::Nth { a: 0, b: 1, of_type: false },
                ("first-of-type", None) => Simple::Nth { a: 0, b: 1, of_type: true },
                ("root", None) => Simple::Root,
                ("link" | "any-link", None) => Simple::Link,
                ("nth-child", Some(a)) => nth(a).map(|(a, b)| Simple::Nth { a, b, of_type: false }).ok_or(())?,
                ("nth-of-type", Some(a)) => nth(a).map(|(a, b)| Simple::Nth { a, b, of_type: true }).ok_or(())?,
                ("not", Some(a)) if depth < MAX_NOT_DEPTH => {
                    let mut list = Vec::new();
                    for part in split_commas(a) {
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
    let value = if fold { value.to_lowercase() } else { value };
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
}

/// Whether a property is one the whitelist ([`crate::style`]) can use.
fn kept(property: &str) -> bool {
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
    ) || property.starts_with("margin")
        || property.starts_with("padding")
        || property == "border"
        || property.starts_with("border-")
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
    let mut value = value.trim();
    let mut important = false;
    let lower = value.to_ascii_lowercase();
    if let Some(head) = lower.strip_suffix("important").map(str::trim_end).and_then(|h| h.strip_suffix('!')) {
        value = value.get(..head.len()).unwrap_or(value).trim_end();
        important = true;
    }
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

/// The rules of a style sheet, at most [`MAX_RULES`], from at most [`MAX_SHEET`] bytes.
pub(crate) fn rules(text: &str) -> Vec<Rule> {
    let text = truncate(text, MAX_SHEET);
    let b = text.as_bytes();
    let mut out: Vec<Rule> = Vec::new();
    let mut i = 0;
    while i < b.len() && out.len() < MAX_RULES {
        i = skip_trivia(b, i);
        match b.get(i) {
            None => break,
            Some(b'}') => i += 1,
            Some(b'@') => {
                // An at-rule: `@import`, `@charset` and the rest end at a `;`; `@media`,
                // `@font-face`, `@keyframes`, `@supports` have a block. None is read here.
                match scan_to(b, i, b";{") {
                    None => break,
                    Some(end) if b.get(end) == Some(&b'{') => i = block_end(b, end) + 1,
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
                for part in split_commas(prelude) {
                    if out.len() >= MAX_RULES {
                        break;
                    }
                    if let Some(selector) = selector(part) {
                        out.push(Rule { selector, decls: decls.clone() });
                    }
                }
            }
        }
    }
    out
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

fn simple_matches(dom: &[El], s: &Simple, at: usize) -> bool {
    let Some(el) = dom.get(at) else { return false };
    match s {
        Simple::Tag(name) => el.name == *name,
        Simple::Id(id) => el.id.as_deref() == Some(id.as_str()),
        Simple::Class(class) => el.classes.iter().any(|c| c == class),
        Simple::Attr(sel) => el.attr(&sel.name).is_some_and(|v| attribute_matches(sel, v)),
        Simple::FirstChild => el.index == 1,
        Simple::LastChild => dom.get(el.parent as usize).is_some_and(|p| p.last_child as usize == at),
        Simple::Nth { a, b, of_type } => {
            let position = i64::from(if *of_type { el.type_index } else { el.index });
            let offset = position - b;
            if *a == 0 { offset == 0 } else { offset % a == 0 && offset / a >= 0 }
        }
        Simple::Not(list) => !list.iter().any(|compound| compound.iter().all(|s| simple_matches(dom, s, at))),
        Simple::Root => el.parent == 0 && el.name == "html",
        Simple::Link => matches!(el.name.as_str(), "a" | "area") && el.attr("href").is_some(),
        Simple::Never => false,
    }
}

fn attribute_matches(sel: &AttrSel, value: &str) -> bool {
    let lowered;
    let value = if sel.fold {
        lowered = value.to_lowercase();
        lowered.as_str()
    } else {
        value
    };
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
    if !compound.simples.iter().all(|s| simple_matches(dom, s, at)) {
        return false;
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
    steps_left: u32,
}

/// What a sheet adds to one element: declarations to put before its `style` attribute, and
/// those marked `!important`, to put after.
pub(crate) type Applied = (String, String);

impl Sheet {
    /// The sheet of a message, when it has any rules. Reads `html` once to find its `<style>`
    /// blocks and to keep the shape of its elements.
    pub fn read(html: &str) -> Option<Sheet> {
        if !has_style(html.as_bytes()) {
            return None;
        }
        let mut text = String::new();
        let mut tree = TreeBuilder::new();
        let mut tokens = Tokenizer::new(html);
        let mut take = |tokens: &mut Tokenizer<'_>| {
            for css in std::mem::take(&mut tokens.styles) {
                let room = MAX_SHEET.saturating_sub(text.len());
                text.push_str(truncate(css, room));
                text.push('\n');
            }
        };
        while let Some(token) = tokens.next() {
            take(&mut tokens);
            match token {
                Token::Open(tag) => tree.open(&tag),
                Token::Close(name) => tree.close(&name),
                Token::Text(_) => {}
            }
        }
        // (A style at the very end has no token after it.)
        take(&mut tokens);
        let rules = rules(&text);
        if rules.is_empty() {
            return None;
        }
        let mut sheet = Sheet {
            rules: Vec::new(),
            by_id: HashMap::new(),
            by_class: HashMap::new(),
            by_tag: HashMap::new(),
            other: Vec::new(),
            dom: tree.els,
            steps_left: MAX_STEPS,
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
        sheet.rules = rules;
        Some(sheet)
    }

    /// Matching steps spent so far.
    #[cfg(test)]
    pub fn work(&self) -> u32 {
        MAX_STEPS - self.steps_left
    }

    /// Put what the sheet says about the `ordinal`th start tag of the message on `tag`.
    pub fn apply(&mut self, ordinal: usize, tag: &mut Tag<'_>) {
        tag.sheeted = true;
        tag.sheet = self.style_for(ordinal);
    }

    /// What the sheet says about the `ordinal`th start tag of the message, counted from 0.
    pub fn style_for(&mut self, ordinal: usize) -> Option<Applied> {
        let at = ordinal.checked_add(1)?;
        let el = self.dom.get(at)?;
        if self.steps_left == 0 {
            return None;
        }
        let mut steps = MAX_STEPS_PER_ELEMENT.min(self.steps_left);
        let start = steps;
        let mut candidates: Vec<u32> = Vec::new();
        if let Some(list) = el.id.as_ref().and_then(|id| self.by_id.get(id)) {
            candidates.extend(list);
        }
        for class in &el.classes {
            if let Some(list) = self.by_class.get(class) {
                candidates.extend(list);
            }
        }
        if let Some(list) = self.by_tag.get(&el.name) {
            candidates.extend(list);
        }
        candidates.extend(&self.other);
        let mut found: Vec<(u32, u32)> = Vec::new();
        for n in candidates {
            if steps == 0 {
                break;
            }
            steps -= 1;
            let Some(rule) = self.rules.get(n as usize) else { continue };
            if matches_from(&self.dom, &rule.selector, rule.selector.compounds.len().saturating_sub(1), at, &mut steps) {
                found.push((rule.selector.specificity, n));
            }
        }
        found.sort_unstable();
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
        self.steps_left = self.steps_left.saturating_sub(start - steps).saturating_sub(u32::try_from(considered).unwrap_or(0));
        let (normal, important) = (flatten(&normal), flatten(&important));
        (!normal.is_empty() || !important.is_empty()).then_some((normal, important))
    }
}

/// Declarations given last-first, as `property:value;` in cascade order, each property once.
fn flatten(last_first: &[&Decl]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    for decl in last_first {
        if seen.contains(&decl.property.as_str()) {
            continue;
        }
        seen.push(&decl.property);
        parts.push(format!("{}:{}", decl.property, decl.value));
        if parts.len() >= MAX_DECLARATIONS {
            break;
        }
    }
    parts.reverse();
    truncate(&parts.join(";"), MAX_APPLIED).to_string()
}

/// Whether `<style` (in any letter case) is in the text at all.
fn has_style(b: &[u8]) -> bool {
    b.windows(6).any(|w| w.first() == Some(&b'<') && w.get(1..6).is_some_and(|s| s.eq_ignore_ascii_case(b"style")))
}

#[cfg(test)]
mod tests;
