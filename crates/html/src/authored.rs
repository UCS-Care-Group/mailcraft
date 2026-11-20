// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An authored signature written out to go in a message, with its own style sheets.
//!
//! A received message's quote ([`crate::quotable`]) loses its `<style>` blocks: they would style
//! the whole message they go into. A signature the owner wrote keeps them here, scoped: its body
//! is written by the same element, attribute and inline-style lists as a quote, inside
//! `<div class="SCOPE">`, and each rule of its sheets is written again for the head with every
//! selector under that class (`.SCOPE x`; `.SCOPE.c` too where the signature's own `<body>` could
//! have been the `.c`; `body` as `div.SCOPE`), its declarations copied as written. Every selector
//! so gains one class and nothing else, so the sheet's own order of precedence stays.
//!
//! What can't be scoped that way, or could fetch or run something in the recipient's program,
//! is refused whole, with nothing written: any at-rule but `@media`, `@media` nested deeper than
//! the reader reads, a selector the reader drops (`:is()`, `:where()`), `html` and `:root`, a
//! sibling of the wrapper, a selector, `@media` prelude, media list or declaration block with an
//! escape, a `<`, a `url(` or what a risky `style` attribute has, any style sheet text with a
//! carriage return, a form feed or a NUL (a CSS reader turns the first two into line breaks, which
//! end a string, and replaces the last; this reader would read on past them), a `<style>` without
//! its end, and more than the reader reads.
//!
//! The selectors and `@media` preludes are written back as they came, so they must mean to a
//! recipient's CSS reader what they meant to this one: a comment in a selector or an `@media`
//! prelude is refused, and so is a `(`, `[` or `{` a CSS reader would pair otherwise (left open,
//! or closed by another kind; comments dropped, strings read to their quote) in a selector, an
//! `@media` prelude or block, or a declaration block, and a `(` in a selector but for the
//! arguments of `:not()`, `:nth-child()` and `:nth-of-type()`, the only ones the reader reads.
//! Then what is written is read again, as a CSS reader reads it ([`crate::authored_css`]), and
//! refused unless it is nothing but `@media` blocks and rules whose every selector starts with
//! the scope class.

use crate::authored_css;
use crate::entities::escape;
use crate::quote::{Quoter, ReboundCid, attribute_kept, risky_style};
use crate::sheet::{self, MAX_RULES, MAX_SHEET, block_end, media_prelude, scan_to, skip_trivia, split_commas};
use crate::tokenizer::{Tag, Token, Tokenizer, span_of};
use crate::{MAX_INPUT, media};

/// Longest scope class, in bytes.
const MAX_SCOPE: usize = 64;

/// The most `<style>` texts the tokenizer holds between two tokens (`tokenizer::MAX_STYLES`): a
/// drain that full may have lost some.
const MAX_STYLES: usize = 256;

/// The class an authored signature's rules are scoped to: an ASCII letter or `_`, then letters,
/// digits, `_` and `-`, 1 to 64 bytes in all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoredScope(String);

impl AuthoredScope {
    /// The scope `class`, which the writer puts on the signature's wrapper and in front of every
    /// selector of its sheets.
    ///
    /// # Errors
    ///
    /// [`AuthoredError::InvalidScope`] when `class` is not an ASCII letter or `_` followed by
    /// ASCII letters, digits, `_` and `-`, 1 to 64 bytes in all.
    pub fn new(class: &str) -> Result<AuthoredScope, AuthoredError> {
        let mut chars = class.chars();
        let first = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        if !first || class.len() > MAX_SCOPE || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(AuthoredError::InvalidScope);
        }
        Ok(AuthoredScope(class.to_string()))
    }

    /// The class.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An authored signature as it goes into a message: `head` is zero or more whole `<style>`
/// elements for the document's head, `body` the signature in `<div class="SCOPE">`, with no style
/// sheet in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoredFragment {
    /// Zero or more whole `<style>` elements, for the document's head.
    pub head: String,
    /// The signature in its `<div class="SCOPE">`, with no style sheet in it.
    pub body: String,
}

/// Why an authored signature can't be written with its style sheets. Nothing is written then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthoredError {
    /// The input is longer than [`MAX_INPUT`], ends inside a tag, or has more style sheet than the
    /// reader reads ([`crate::MAX_SHEET_BYTES`]).
    Truncated,
    /// A `<style>`, a rule or an `@media` block has no end.
    Unclosed,
    /// Something in a style sheet that isn't scoped this way.
    Unsupported,
    /// A selector, `@media` prelude, media list or declaration block that could fetch or run
    /// something, or hides what it says; or a style sheet with a carriage return, form feed or NUL.
    Risky,
    /// What would be written is more than the reader reads.
    TooLarge,
    /// The scope class is already in the signature.
    ScopeCollides,
    /// The scope is no class this writes.
    InvalidScope,
}

impl std::fmt::Display for AuthoredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            AuthoredError::Truncated => "the signature is longer than is read, ends inside a tag or has more style sheet than is read",
            AuthoredError::Unclosed => "a style sheet, a rule or an @media block does not end",
            AuthoredError::Unsupported => "a style sheet holds something that can't be scoped to the signature",
            AuthoredError::Risky => "a style sheet holds something that could fetch or run something, or hides what it says",
            AuthoredError::TooLarge => "the signature with its scoped style sheets would be longer than is read",
            AuthoredError::ScopeCollides => "the scope class is already in the signature",
            AuthoredError::InvalidScope => "the scope is not a class the writer puts on a signature",
        };
        f.write_str(text)
    }
}

impl std::error::Error for AuthoredError {}

use AuthoredError::*;

/// The authored signature `html` written to go in a message, its style sheets scoped to `scope`,
/// and the pictures it holds (`cid:`) given to `rebound` as [`crate::quotable_with_rebound_cids`]
/// gives them. The body is written as that function writes a quote, but for two things: what the
/// signature's own sheets hide (a copy for narrow windows, or for dark ones) is kept, and its
/// `<body>`'s attributes that a quote would keep on a `div` go on the wrapper.
pub fn authored_fragment_with_rebound_cids(
    html: &str,
    scope: &AuthoredScope,
    rebound: &mut ReboundCid<'_>,
) -> Result<AuthoredFragment, AuthoredError> {
    if html.len() > MAX_INPUT {
        return Err(Truncated);
    }
    if html.to_ascii_lowercase().contains(&scope.0.to_ascii_lowercase()) {
        return Err(ScopeCollides);
    }
    let mut writer = Writer { scope: &scope.0, head: String::new(), size: 0, rules: 0, lists: 0 };
    let mut quoter = Quoter::default();
    quoter.rebind = Some(rebound);
    let mut wrapper = None;
    let mut template = 0usize;
    let mut tokens = Tokenizer::new(html);
    while let Some(token) = tokens.next() {
        writer.styles(html, &mut tokens, template)?;
        match &token {
            Token::Open(_) if !tokens.tag_closed() => return Err(Truncated),
            Token::Open(tag) if tag.name == "template" => template = template.saturating_add(1),
            Token::Close(name) if name == "template" => template = template.saturating_sub(1),
            // A `<body>` inside any template, whatever holds it, is inert: it gives the wrapper
            // nothing, as it ends nothing in the quoter.
            Token::Open(tag) if tag.name == "body" && wrapper.is_none() && template == 0 && !quoter.template_body(&token) => {
                wrapper = Some(wrapper_tag(&scope.0, Some(tag)));
            }
            _ => {}
        }
        quoter.authored_token(token);
    }
    writer.styles(html, &mut tokens, template)?;
    let inner = quoter.finish();
    let mut body = wrapper.unwrap_or_else(|| wrapper_tag(&scope.0, None));
    body.push_str(&inner);
    body.push_str("</div>");
    if body.len() > MAX_INPUT || writer.head.len() > MAX_SHEET || writer.rules > MAX_RULES {
        return Err(TooLarge);
    }
    Ok(AuthoredFragment { head: writer.head, body })
}

/// The wrapper's start tag: the scope class, then the classes of the signature's `<body>`, and
/// that body's other attributes a quote keeps on a `div`.
fn wrapper_tag(scope: &str, body: Option<&Tag<'_>>) -> String {
    let mut out = format!("<div class=\"{scope}");
    let mut rest = String::new();
    let mut seen: Vec<String> = Vec::new();
    for (attr, value) in body.into_iter().flat_map(|tag| tag.attributes()) {
        let attr = attr.to_ascii_lowercase();
        // The first of a name counts, as in browsers.
        if seen.contains(&attr) {
            continue;
        }
        seen.push(attr.clone());
        if attr == "class" {
            // HTML's own spaces round the classes, and no other: a no-break space is part of one.
            let classes = value.trim_matches(|c: char| c.is_ascii_whitespace());
            if !classes.is_empty() {
                out.push(' ');
                out.push_str(&escape(classes));
            }
        } else if attribute_kept("div", &attr, &value) {
            rest.push(' ');
            rest.push_str(&attr);
            rest.push_str("=\"");
            rest.push_str(&escape(&value));
            rest.push('"');
        }
    }
    out.push('"');
    out.push_str(&rest);
    out.push('>');
    out
}

struct Writer<'s> {
    scope: &'s str,
    head: String,
    /// Bytes of style sheet read, against [`MAX_SHEET`].
    size: usize,
    /// Selectors written, against [`MAX_RULES`].
    rules: usize,
    /// `@media` blocks met, against [`media::MAX_LISTS`].
    lists: usize,
}

impl Writer<'_> {
    /// Write the `<style>` sheets the tokenizer has passed since it was last asked, as every
    /// reader of the sheets takes them: none in a `<template>`.
    fn styles(&mut self, html: &str, tokens: &mut Tokenizer<'_>, template: usize) -> Result<(), AuthoredError> {
        let found = std::mem::take(&mut tokens.styles);
        if found.len() >= MAX_STYLES {
            return Err(Truncated);
        }
        for crate::tokenizer::Style { media: list, css, overflowed } in found {
            if template > 0 {
                continue;
            }
            // Its start tag had more attributes than are kept: a `media` among those dropped
            // would be lost, and the sheet sent for every window.
            if overflowed {
                return Err(Unsupported);
            }
            // The sheet ends at its end tag, and that tag at its `>`.
            let end = span_of(html, css).ok_or(Unclosed)?.end;
            let after = html.get(end..).unwrap_or("");
            if !after.get(..7).is_some_and(|t| t.eq_ignore_ascii_case("</style")) {
                return Err(Unclosed);
            }
            if !after.contains('>') {
                return Err(Truncated);
            }
            self.size = self.size.saturating_add(css.len());
            if self.size > MAX_SHEET {
                return Err(Truncated);
            }
            // Line breaks this reader doesn't read as a CSS reader does, anywhere in the sheet.
            unread_breaks(css)?;
            unread_breaks(&list)?;
            // Trimmed as it is tested: a condition of spaces that aren't CSS white space is kept,
            // for no window, rather than dropped and the sheet sent for every window.
            let list = media::trim(&list);
            if list.len() > media::MAX_QUERY_BYTES {
                return Err(Unsupported);
            }
            guard(list)?;
            let mut out = String::new();
            self.sheet(css, 1, &mut out)?;
            if out.is_empty() {
                continue;
            }
            if out.len() > MAX_SHEET {
                return Err(TooLarge);
            }
            // What was written, read again as a CSS reader reads it: every rule under the scope.
            if !authored_css::scoped(&out, self.scope) {
                return Err(Unsupported);
            }
            self.head.push_str("<style");
            if !list.is_empty() {
                self.head.push_str(" media=\"");
                self.head.push_str(&escape(list));
                self.head.push('"');
            }
            self.head.push('>');
            self.head.push_str(&out);
            self.head.push_str("</style>");
        }
        Ok(())
    }

    /// The rules of `text` scoped, into `out`; `level` is how many `@media` blocks deep it is, from 1.
    /// Every block is walked, whatever its media say.
    fn sheet(&mut self, text: &str, level: usize, out: &mut String) -> Result<(), AuthoredError> {
        let b = text.as_bytes();
        let mut i = 0;
        loop {
            i = skip_trivia(b, i);
            match b.get(i) {
                None => return Ok(()),
                // A `}` with no block to close: a browser reads it into the next selector.
                Some(b'}') => return Err(Unsupported),
                Some(b'@') => {
                    let end = scan_to(b, i, b";{").ok_or(Unclosed)?;
                    let prelude = text.get(i..end).unwrap_or("");
                    let list = media_prelude(prelude).filter(|_| b.get(end) == Some(&b'{')).ok_or(Unsupported)?;
                    self.lists = self.lists.saturating_add(1);
                    if level > media::MAX_NESTING {
                        return Err(Unsupported);
                    }
                    if self.lists > media::MAX_LISTS {
                        return Err(TooLarge);
                    }
                    // The complete, CSS-trimmed query must fit the reader, not just its prefix.
                    if list.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')).len() > media::MAX_QUERY_BYTES {
                        return Err(Unsupported);
                    }
                    guard(prelude)?;
                    guard(list)?;
                    // Written as it came: no comment, and brackets a CSS reader pairs as this one.
                    no_comment(prelude)?;
                    if !authored_css::balanced(prelude) {
                        return Err(Unsupported);
                    }
                    let close = block_end(b, end);
                    if close >= b.len() {
                        return Err(Unclosed);
                    }
                    let body = text.get(end + 1..close).unwrap_or("");
                    if !authored_css::balanced(body) {
                        return Err(Unsupported);
                    }
                    out.push_str(prelude);
                    out.push('{');
                    self.sheet(body, level + 1, out)?;
                    out.push('}');
                    i = close + 1;
                }
                Some(_) => {
                    let open = scan_to(b, i, b"{").ok_or(Unclosed)?;
                    let close = block_end(b, open);
                    if close >= b.len() {
                        return Err(Unclosed);
                    }
                    self.rule(text.get(i..open).unwrap_or(""), text.get(open..=close).unwrap_or(""), out)?;
                    i = close + 1;
                }
            }
        }
    }

    /// One rule, its selectors scoped and its declaration `block` (braces and all) as written.
    fn rule(&mut self, prelude: &str, block: &str, out: &mut String) -> Result<(), AuthoredError> {
        let inner = block.get(1..block.len().saturating_sub(1)).unwrap_or("");
        if inner.contains('{') {
            return Err(Unsupported);
        }
        guard(inner)?;
        // An unclosed `(` or `[` would keep a CSS reader in the block past its `}`.
        if !authored_css::balanced(inner) {
            return Err(Unsupported);
        }
        // The selectors are written as they came, so they are held to the same guard, and read
        // as a CSS reader reads them: no comment, brackets paired as here, and arguments only
        // where the reader reads them.
        guard(prelude)?;
        no_comment(prelude)?;
        if !authored_css::balanced(prelude) || !authored_css::functions_known(prelude) {
            return Err(Unsupported);
        }
        let scope = self.scope;
        let mut selectors = Vec::new();
        for part in split_commas(prelude) {
            let text = sheet::css_trim(part);
            let shape = sheet::shape(text).ok_or(Unsupported)?;
            if shape.outside {
                return Err(Unsupported);
            }
            // The wrapper stands where the signature's body stood, but not in its place among
            // its siblings, and its attributes are not the body's (its class has the scope in
            // it): a structural test of it, a sibling of it or an attribute test of it would mean
            // other things.
            let wrapper = |rest: &str, out: &mut Vec<String>| -> Result<(), AuthoredError> {
                if shape.structural || shape.sibling || shape.attribute {
                    return Err(Unsupported);
                }
                out.push(format!(".{scope}{rest}"));
                Ok(())
            };
            match &shape.tag {
                Some((end, name)) if name == "body" => {
                    let rest = text.get(*end..).unwrap_or("");
                    // The wrapper's own attributes are not the body's (its class has the scope
                    // in it): an attribute test of it would not match as it did.
                    if shape.structural || shape.sibling || shape.attribute {
                        return Err(Unsupported);
                    }
                    selectors.push(format!("div.{scope}{rest}"));
                }
                Some((end, name)) if name == "*" => {
                    selectors.push(format!(".{scope} {text}"));
                    if !shape.sibling {
                        wrapper(text.get(*end..).unwrap_or(""), &mut selectors)?;
                    }
                }
                Some(_) => selectors.push(format!(".{scope} {text}")),
                None => {
                    selectors.push(format!(".{scope} {text}"));
                    if !shape.sibling {
                        wrapper(text, &mut selectors)?;
                    }
                }
            }
        }
        self.rules = self.rules.saturating_add(selectors.len());
        if self.rules > MAX_RULES {
            return Err(TooLarge);
        }
        out.push_str(&selectors.join(", "));
        out.push(' ');
        out.push_str(block);
        out.push('\n');
        Ok(())
    }
}

/// Refuse CSS text that could fetch or run something in a recipient's program, or that hides what
/// it says from a plain reading: an escape, a `<`, a `url(`, what makes a `style` attribute risky,
/// or a character [`unread_breaks`] refuses.
fn guard(css: &str) -> Result<(), AuthoredError> {
    unread_breaks(css)?;
    if css.contains(['\\', '<']) || css.to_ascii_lowercase().contains("url(") || risky_style(css) {
        return Err(Risky);
    }
    Ok(())
}

/// Refuse a comment in text that is written as it came: this reader and a CSS reader may count
/// the brackets in it differently.
fn no_comment(css: &str) -> Result<(), AuthoredError> {
    if css.contains("/*") {
        return Err(Risky);
    }
    Ok(())
}

/// Refuse a carriage return, a form feed or a NUL: a CSS reader turns the first two into a line
/// break, which ends a string where this reader would read on, and replaces the last.
fn unread_breaks(css: &str) -> Result<(), AuthoredError> {
    if css.contains(['\r', '\u{c}', '\0']) {
        return Err(Risky);
    }
    Ok(())
}
