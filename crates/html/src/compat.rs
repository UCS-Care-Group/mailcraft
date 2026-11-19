// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether the HTML of a signature will come out as written in the mail programs it is sent to.
//!
//! [`check`] reads the HTML as the owner wrote it and gives a [`Finding`] for each thing that a
//! common mail program is known to show differently or not at all: which [`Rule`] it breaks, how
//! much that matters ([`Severity`]), the byte range in the checked text that it is about, and,
//! where one exact edit puts it right, that edit ([`Fix`]). Nothing here is prose for a person:
//! a window says what a rule means, in the owner's language, by its [`Rule::id`].
//!
//! The picture and element rules read the start tags the tokenizer gives, and the frames it skips
//! whole; the CSS rules read every `style` attribute and the declarations of the `<style>` sheets,
//! walked as the writer of an authored signature walks them (every `@media` block, no other
//! at-rule). Comments, conditional comments and scripts are not checked, and nothing named by the
//! document is fetched or opened.
//!
//! The rules:
//!
//! - `OL-IMG-001`: a picture whose source is SVG, by its path's extension or a `data:` type.
//! - `OL-IMG-003`: a picture whose source is WebP, AVIF or HEIC/HEIF, the same way.
//! - `OL-IMG-005`: a picture sized by its inline `width`/`height` style alone, without the
//!   matching `width`/`height` attribute. When every such size is a whole number of pixels, the
//!   fix adds the missing attributes right after the tag's name and keeps the style as it is.
//! - `OL-IMG-004`: a picture whose source is written into the HTML as a `data:` address, whatever
//!   its type. It has no fix: the picture is attached to the signature instead.
//! - `OL-EL-002`, `OL-EL-003`, `OL-EL-004`: a form or form control (`<form>`, `<input>`,
//!   `<button>`, `<select>`, `<textarea>`), embedded content (`<iframe>`, `<object>`, `<embed>`)
//!   or a video or audio player (`<video>`, `<audio>`), found on its start tag. The fix takes the
//!   whole element out, start tag to end tag (a void element: its tag alone), when its end tag is
//!   found; a `<form>` has none, since it holds the owner's own content. A frame is found although
//!   the reader skips it whole, by reading what was skipped the way the reader does.
//! - `OL-SEL-003`: a `<style>` rule for `::before` or `::after` that adds text with `content`,
//!   found on the whole rule. It has no fix.
//! - `OL-DEC-003`: a CSS custom property's definition (`--name:`) and each `var()`, in a `style`
//!   attribute or a `<style>` rule. A `var()` in a `style` attribute whose variable is defined
//!   once, at the root or in that same attribute, with a plain value of at most
//!   [`MAX_FIX_BYTES`] bytes, is fixed by writing that value in its place; the definition itself
//!   has no fix.
//! - `OL-COL-002`: a colour function (`rgb()`, `rgba()`, `hsl()`, `hsla()`) whose numbers are
//!   separated by spaces. `rgb()` of three plain whole numbers is fixed by separating them with
//!   commas.
//! - Warnings, which don't stop a signature being kept, each without a fix: `OL-LAY-001`
//!   (`display: flex`, `inline-flex`), `OL-LAY-002` (`display: grid`, `inline-grid`), `OL-LAY-003`
//!   (`float`), `OL-LAY-004` (`position: absolute`, `fixed`, `relative`), `OL-LAY-005`
//!   (`max-width`, `min-width`, but not for a table), `OL-BOX-007` (`border-radius` that isn't
//!   zero), `OL-BOX-008` (`box-shadow`), `OL-BG-001` (a background picture), `OL-BG-004` (a
//!   gradient) and `OL-FNT-002` (a `font-family` list that doesn't end with `serif`, `sans-serif`
//!   or `monospace`), on the whole declaration; `OL-UNIT-001` (`rem`) and `OL-UNIT-002` (`vw`,
//!   `vh`, `vmin`, `vmax`), on the size itself.
//! - `MC-CSS-001`: a `<style>` sheet that the writer of an authored signature refuses
//!   ([`crate::authored_fragment_with_rebound_cids`]: an `@import`, a `url(`, a selector it can't
//!   scope, …), so that the signature could not be sent with it. The finding is the whole
//!   `<style>` element the writer first refuses the text up to the end of, or the first sheet's
//!   start tag when no one element explains the refusal (what the writer would write of the
//!   document is longer than it gives, say). It has no fix. Nothing is said of a document without a
//!   style sheet.
//! - `MC-CSS-002`: a `style` attribute that the writer leaves out whole, because something in it
//!   could fetch or run something (a `url(`, an `expression(`, a `behavior`, …: what makes a quote
//!   leave a style out), so that every declaration in it would be lost, a colour or padding beside
//!   a background picture included. Found on the attribute's value, with no fix.
//!
//! A picture attached to the message (`cid:`) is not judged by its name: what it is comes from
//! its own bytes, and the signature importer already takes only PNG, JPEG and GIF pictures by
//! their content.
//!
//! The checker fails closed: a document longer than is read ([`crate::MAX_INPUT`]), one cut off
//! inside a start tag, a tag with more attributes than are kept, or more findings than are given
//! is refused with a [`CheckError`], never checked in part. What it gives is bounded too: at most
//! [`MAX_FINDINGS`] findings, and no fix longer than [`MAX_FIX_BYTES`].

use std::collections::HashMap;
use std::ops::Range;

use crate::entities::unescape;
use crate::sheet::{block_end, media_prelude, scan_to, skip_trivia, split_commas};
use crate::style::{declarations, strip_important};
use crate::tokenizer::{Tag, Token, Tokenizer, span_of};

/// Most findings one document gives; one that has more is refused ([`CheckError::TooManyFindings`]).
pub const MAX_FINDINGS: usize = 1024;

/// Longest replacement a [`Fix`] writes, in bytes. A `var()` whose variable's value is longer is
/// found without a fix, so that a long value used in many calls is not copied into each of their
/// findings: the fixes of one check hold at most [`MAX_FINDINGS`] times this (256 KB), however
/// long the document. Every other fix is far shorter by its making.
pub const MAX_FIX_BYTES: usize = 256;

/// How much a finding matters ([`Rule::severity`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    /// Something the owner must put right before the signature is kept.
    Error,
    /// Something a mail program shows otherwise, which doesn't stop the signature being kept.
    Warning,
}

/// A compatibility rule. Each has a stable [`Rule::id`] that a window translates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rule {
    /// `OL-IMG-001`: the picture's source is SVG.
    SvgImage,
    /// `OL-IMG-003`: the picture's source is WebP, AVIF or HEIC/HEIF.
    UnsupportedImageFormat,
    /// `OL-IMG-005`: the picture is sized by its inline style alone, without the matching
    /// `width`/`height` attribute.
    CssOnlyImageSize,
    /// `MC-CSS-001`: a style sheet that can't be sent safely with the signature.
    StylesheetNotSendable,
    /// `MC-CSS-002`: a `style` attribute that can't be sent safely, so that the writer leaves it
    /// out whole.
    StyleNotSendable,
    /// `OL-IMG-004`: the picture's source is written into the HTML as a `data:` address.
    DataUriImage,
    /// `OL-EL-002`: a form or a form control (`<form>`, `<input>`, `<button>`, `<select>`,
    /// `<textarea>`).
    FormControl,
    /// `OL-EL-003`: embedded content (`<iframe>`, `<object>`, `<embed>`).
    EmbeddedContent,
    /// `OL-EL-004`: a video or audio player (`<video>`, `<audio>`).
    MediaPlayer,
    /// `OL-SEL-003`: a style rule for `::before` or `::after` that adds `content`.
    GeneratedContent,
    /// `OL-DEC-003`: a CSS custom property (`--name:`) or a `var()`.
    CssVariable,
    /// `OL-COL-002`: a colour function whose numbers are separated by spaces (`rgb(0 128 0)`).
    SpaceSeparatedColour,
    /// `OL-LAY-001`: `display: flex` or `inline-flex`.
    Flexbox,
    /// `OL-LAY-002`: `display: grid` or `inline-grid`.
    Grid,
    /// `OL-LAY-003`: `float`.
    Float,
    /// `OL-LAY-004`: `position: absolute`, `fixed` or `relative`.
    Positioned,
    /// `OL-LAY-005`: `max-width` or `min-width` on anything but a table.
    MinMaxWidth,
    /// `OL-BOX-007`: rounded corners (`border-radius`).
    RoundedCorners,
    /// `OL-BOX-008`: `box-shadow`.
    BoxShadow,
    /// `OL-BG-001`: a background picture.
    BackgroundImage,
    /// `OL-BG-004`: a gradient.
    Gradient,
    /// `OL-UNIT-001`: a size in `rem`.
    RemUnit,
    /// `OL-UNIT-002`: a size in `vw`, `vh`, `vmin` or `vmax`.
    ViewportUnit,
    /// `OL-FNT-002`: a `font-family` list that doesn't end with a generic family.
    NoGenericFontFamily,
}

impl Rule {
    /// Every rule: the rulebook's, in the order of their ids, then MailCraft's own.
    pub const ALL: [Rule; 24] = [
        Rule::BackgroundImage,
        Rule::Gradient,
        Rule::RoundedCorners,
        Rule::BoxShadow,
        Rule::SpaceSeparatedColour,
        Rule::CssVariable,
        Rule::FormControl,
        Rule::EmbeddedContent,
        Rule::MediaPlayer,
        Rule::NoGenericFontFamily,
        Rule::SvgImage,
        Rule::UnsupportedImageFormat,
        Rule::DataUriImage,
        Rule::CssOnlyImageSize,
        Rule::Flexbox,
        Rule::Grid,
        Rule::Float,
        Rule::Positioned,
        Rule::MinMaxWidth,
        Rule::GeneratedContent,
        Rule::RemUnit,
        Rule::ViewportUnit,
        Rule::StylesheetNotSendable,
        Rule::StyleNotSendable,
    ];

    /// The rule's stable id, which never changes once given.
    pub fn id(self) -> &'static str {
        match self {
            Rule::SvgImage => "OL-IMG-001",
            Rule::UnsupportedImageFormat => "OL-IMG-003",
            Rule::CssOnlyImageSize => "OL-IMG-005",
            Rule::StylesheetNotSendable => "MC-CSS-001",
            Rule::StyleNotSendable => "MC-CSS-002",
            Rule::DataUriImage => "OL-IMG-004",
            Rule::FormControl => "OL-EL-002",
            Rule::EmbeddedContent => "OL-EL-003",
            Rule::MediaPlayer => "OL-EL-004",
            Rule::GeneratedContent => "OL-SEL-003",
            Rule::CssVariable => "OL-DEC-003",
            Rule::SpaceSeparatedColour => "OL-COL-002",
            Rule::Flexbox => "OL-LAY-001",
            Rule::Grid => "OL-LAY-002",
            Rule::Float => "OL-LAY-003",
            Rule::Positioned => "OL-LAY-004",
            Rule::MinMaxWidth => "OL-LAY-005",
            Rule::RoundedCorners => "OL-BOX-007",
            Rule::BoxShadow => "OL-BOX-008",
            Rule::BackgroundImage => "OL-BG-001",
            Rule::Gradient => "OL-BG-004",
            Rule::RemUnit => "OL-UNIT-001",
            Rule::ViewportUnit => "OL-UNIT-002",
            Rule::NoGenericFontFamily => "OL-FNT-002",
        }
    }

    /// How much a finding of this rule matters.
    pub fn severity(self) -> Severity {
        match self {
            Rule::SvgImage
            | Rule::UnsupportedImageFormat
            | Rule::CssOnlyImageSize
            | Rule::StylesheetNotSendable
            | Rule::StyleNotSendable
            | Rule::DataUriImage
            | Rule::FormControl
            | Rule::EmbeddedContent
            | Rule::MediaPlayer
            | Rule::GeneratedContent
            | Rule::CssVariable
            | Rule::SpaceSeparatedColour => Severity::Error,
            Rule::Flexbox
            | Rule::Grid
            | Rule::Float
            | Rule::Positioned
            | Rule::MinMaxWidth
            | Rule::RoundedCorners
            | Rule::BoxShadow
            | Rule::BackgroundImage
            | Rule::Gradient
            | Rule::RemUnit
            | Rule::ViewportUnit
            | Rule::NoGenericFontFamily => Severity::Warning,
        }
    }
}

/// One exact edit of the checked text: replace the bytes of `range` (empty for an insertion) with
/// `replacement`. No byte outside `range` changes. `range` falls on character boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    /// The bytes replaced, in the checked text.
    pub range: Range<usize>,
    /// What they are replaced with.
    pub replacement: String,
}

/// Something in the checked text that a mail program will not show as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The rule it breaks.
    pub rule: Rule,
    /// How much it matters: always its rule's [`Rule::severity`].
    pub severity: Severity,
    /// The bytes it is about in the checked text, on character boundaries.
    pub range: Range<usize>,
    /// The one edit that puts it right, where there is one.
    pub fix: Option<Fix>,
}

/// Why a document was not checked. A caller treats it like an error finding: the document is
/// not known to be clean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckError {
    /// The document is longer than [`crate::MAX_INPUT`] bytes, far more than a signature holds:
    /// what the checker keeps of a document grows with it.
    OverInput,
    /// A start tag is cut off by the end of the document.
    UnclosedTag,
    /// A tag has more attributes than are read, so one that was dropped (a `style`, a picture's
    /// source or size) may matter.
    AttributeOverflow,
    /// A part of a tag is not where the tag it was read from says it is.
    BadRange,
    /// The document gives more than [`MAX_FINDINGS`] findings.
    TooManyFindings,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            CheckError::OverInput => "the document is longer than is checked",
            CheckError::UnclosedTag => "a start tag does not end",
            CheckError::AttributeOverflow => "a tag has more attributes than are read",
            CheckError::BadRange => "a part of a tag is not where its tag says it is",
            CheckError::TooManyFindings => "the document gives more findings than are listed",
        };
        f.write_str(text)
    }
}

impl std::error::Error for CheckError {}

/// The compatibility findings of `html`, in the order of their ranges' starts. Refused, rather
/// than checked in part, as [`CheckError`] says. `html` is only read.
pub fn check(html: &str) -> Result<Vec<Finding>, CheckError> {
    // The reader's own cap, not the room it leaves for pictures written into a message: each
    // declaration read is kept, so what is held is some forty times the text.
    if html.len() > crate::MAX_INPUT {
        return Err(CheckError::OverInput);
    }
    let mut found = Vec::new();
    let mut elements = Elements::default();
    let mut styles = Styles::default();
    // How many start tags were read before this one.
    let mut element = 0usize;
    // The raw-text element just opened: its text is what the reader gives next.
    let mut raw: Option<&'static str> = None;
    let mut tokens = Tokenizer::new(html);
    loop {
        let from = tokens.pos();
        let token = tokens.next();
        let to = if token.is_some() { tokens.pos() } else { html.len() };
        // What the reader skipped before this token, from past a raw-text element's text.
        let skipped = match raw.take() {
            Some(name) => find_end_tag(html.as_bytes(), from, name).unwrap_or(html.len()),
            None => from,
        };
        frames(html, skipped, to, &mut found)?;
        let Some(token) = token else { break };
        match &token {
            Token::Open(tag) => {
                // A start tag the end of the document cut off: what follows it is not known.
                if !tokens.tag_closed() {
                    return Err(CheckError::UnclosedTag);
                }
                // An attribute that was dropped may be a `style` that matters.
                if tag.attrs_overflowed() {
                    return Err(CheckError::AttributeOverflow);
                }
                let within = tokens.tag_start()..tokens.pos();
                elements.open(tag, &within, &mut found);
                styles.inline(html, tag, &within, element)?;
                dropped_style(html, tag, &within, &mut found)?;
                element = element.saturating_add(1);
                raw = RAW_TEXT.iter().copied().find(|name| *name == tag.name);
                if tag.name == "img" {
                    let name_end = image_name_end(html, &within)?;
                    source(html, tag, &within, &mut found)?;
                    size(html, tag, &within, name_end, &mut found)?;
                }
            }
            Token::Close(name) => elements.close(name, tokens.pos(), &mut found),
            Token::Text(_) => {}
        }
        if found.len() > MAX_FINDINGS {
            return Err(CheckError::TooManyFindings);
        }
    }
    elements.finish(&mut found);
    let sheets = style_elements(html);
    for sheet in &sheets {
        styles.sheet(html, sheet.css.clone(), 0);
    }
    generated_content(&styles, &mut found)?;
    variables(&styles, &mut found)?;
    spaced_colours(&styles, &mut found)?;
    declaration_warnings(&styles, &mut found)?;
    units(&styles, &mut found)?;
    if let Some(sheet) = stylesheet(html, &sheets) {
        note(&mut found, sheet)?;
    }
    if found.len() > MAX_FINDINGS {
        return Err(CheckError::TooManyFindings);
    }
    found.sort_by_key(|f| (f.range.start, f.range.end));
    // The same finding twice (two calls in a declaration that can only be found whole) is one.
    found.dedup();
    Ok(found)
}

/// MC-CSS-002: the `style` attribute of `tag` (spanning `within`), when the writer leaves it out
/// whole: read as the writer reads it, with its character references decoded
/// ([`crate::quote::risky_style`]).
fn dropped_style(html: &str, tag: &Tag<'_>, within: &Range<usize>, found: &mut Vec<Finding>) -> Result<(), CheckError> {
    let Some(raw) = tag.raw_attr("style") else { return Ok(()) };
    if !crate::quote::risky_style(&unescape(raw)) {
        return Ok(());
    }
    note(found, finding(Rule::StyleNotSendable, range_in(html, raw, within)?, None))
}

fn finding(rule: Rule, range: Range<usize>, fix: Option<Fix>) -> Finding {
    Finding { rule, severity: rule.severity(), range, fix }
}

/// `f` added to `found`, refused as soon as there are more findings than are given, so that a
/// pass doesn't go on reading for findings that would be refused anyway.
fn note(found: &mut Vec<Finding>, f: Finding) -> Result<(), CheckError> {
    found.push(f);
    if found.len() > MAX_FINDINGS { Err(CheckError::TooManyFindings) } else { Ok(()) }
}

/// What an element is that a mail program won't show or let be used: OL-EL-002, OL-EL-003 or
/// OL-EL-004, by its (lower-case) name.
fn element_rule(name: &str) -> Option<Rule> {
    match name {
        "form" | "input" | "button" | "select" | "textarea" => Some(Rule::FormControl),
        "iframe" | "object" | "embed" => Some(Rule::EmbeddedContent),
        "video" | "audio" => Some(Rule::MediaPlayer),
        _ => None,
    }
}

/// Most elements of those kinds waiting for their end tags at once; one more is found without a
/// fix, since its end is not looked for.
const MAX_OPEN_ELEMENTS: usize = 256;

/// One element of those kinds whose end tag has not been read yet.
struct OpenElement {
    rule: Rule,
    name: String,
    /// Its start tag.
    tag: Range<usize>,
    /// How many elements of its name are open from it on, itself included.
    depth: u32,
    /// Whether taking it out whole is its fix: not for a form, which holds the owner's own content.
    fixable: bool,
}

/// OL-EL-002, OL-EL-003 and OL-EL-004 for the elements the reader gives as tokens. Each is found on
/// its start tag; its fix takes the whole element out, start tag to the end tag's `>` (a void
/// element: its tag alone), when that end is known.
#[derive(Default)]
struct Elements {
    open: Vec<OpenElement>,
}

impl Elements {
    fn open(&mut self, tag: &Tag<'_>, within: &Range<usize>, found: &mut Vec<Finding>) {
        for e in self.open.iter_mut().filter(|e| e.name == tag.name) {
            e.depth = e.depth.saturating_add(1);
        }
        let Some(rule) = element_rule(&tag.name) else { return };
        if matches!(tag.name.as_str(), "input" | "embed") {
            found.push(finding(rule, within.clone(), Some(Fix { range: within.clone(), replacement: String::new() })));
        } else if self.open.len() < MAX_OPEN_ELEMENTS {
            self.open.push(OpenElement { rule, name: tag.name.clone(), tag: within.clone(), depth: 1, fixable: tag.name != "form" });
        } else {
            found.push(finding(rule, within.clone(), None));
        }
    }

    /// The end tag `name`, whose `>` ends at `end`.
    fn close(&mut self, name: &str, end: usize, found: &mut Vec<Finding>) {
        let mut i = 0;
        while let Some(e) = self.open.get_mut(i) {
            if e.name == name {
                e.depth = e.depth.saturating_sub(1);
                if e.depth == 0 {
                    let e = self.open.remove(i);
                    let fix = e.fixable.then(|| Fix { range: e.tag.start..end, replacement: String::new() });
                    found.push(finding(e.rule, e.tag, fix));
                    continue;
                }
            }
            i += 1;
        }
    }

    /// The document ended: those still open are found without a fix, their end not being known.
    fn finish(self, found: &mut Vec<Finding>) {
        found.extend(self.open.into_iter().map(|e| finding(e.rule, e.tag, None)));
    }
}

/// The elements the reader skips whole, start tag, content and end tag, without giving a token.
const SKIPPED: [&str; 6] = ["script", "style", "title", "iframe", "noembed", "noframes"];

/// The elements whose content the reader gives as text, never as markup.
const RAW_TEXT: [&str; 2] = ["textarea", "xmp"];

/// OL-EL-003 for `<iframe>`, which the reader skips whole and never gives as a token: the frames
/// in `from..to`, the stretch it went over before giving its next token, read the way it reads
/// them. Each is found on its start tag; its fix takes it out to its end tag's `>`, when it has one.
fn frames(html: &str, from: usize, to: usize, found: &mut Vec<Finding>) -> Result<(), CheckError> {
    let b = html.as_bytes();
    let mut i = from;
    while i < to && b.get(i) == Some(&b'<') {
        match b.get(i + 1) {
            Some(b'!' | b'?') => i = declaration_end(b, i),
            Some(b'/') if b.get(i + 2) == Some(&b'>') => i += 3,
            Some(c) if c.is_ascii_alphabetic() => {
                let name_end =
                    (i + 1..b.len()).find(|&j| b.get(j).is_some_and(|c| c.is_ascii_whitespace() || matches!(*c, b'/' | b'>'))).unwrap_or(b.len());
                let name = html.get(i + 1..name_end).unwrap_or("").to_ascii_lowercase();
                if !SKIPPED.contains(&name.as_str()) {
                    break;
                }
                let (tag_end, closed) = start_tag_end(b, name_end);
                let end_tag = find_end_tag(b, tag_end, &name);
                let element_end = end_tag.map_or(b.len(), |at| find_byte(b, at, b'>').map_or(b.len(), |j| j + 1));
                // Cut off by the end of the document, as a start tag the reader gives would be.
                if !closed {
                    return Err(CheckError::UnclosedTag);
                }
                if name == "iframe" {
                    let fix = end_tag.map(|_| Fix { range: i..element_end, replacement: String::new() });
                    found.push(finding(Rule::EmbeddedContent, i..tag_end, fix));
                }
                i = element_end;
            }
            _ => break,
        }
    }
    Ok(())
}

/// Where the comment, doctype or processing instruction at `at` ends, as the reader reads it.
fn declaration_end(b: &[u8], at: usize) -> usize {
    let end = if b.get(at..at + 4) == Some(b"<!--") {
        let body = at + 4;
        if b.get(body) == Some(&b'>') {
            body + 1
        } else if b.get(body..body + 2) == Some(b"->") {
            body + 2
        } else {
            find_sub(b, body, b"-->").map_or(b.len(), |e| e + 3)
        }
    } else {
        find_byte(b, at, b'>').map_or(b.len(), |e| e + 1)
    };
    end.min(b.len())
}

/// Where the start tag whose name ends at `i` ends, as the reader reads its attributes, and
/// whether its own `>` ends it (rather than the end of the document).
fn start_tag_end(b: &[u8], mut i: usize) -> (usize, bool) {
    loop {
        while matches!(b.get(i), Some(c) if c.is_ascii_whitespace() || *c == b'/') {
            i += 1;
        }
        match b.get(i) {
            None => return (b.len(), false),
            Some(b'>') => return (i + 1, true),
            Some(_) => {}
        }
        i += 1;
        while let Some(c) = b.get(i) {
            if c.is_ascii_whitespace() || matches!(*c, b'/' | b'>' | b'=') {
                break;
            }
            i += 1;
        }
        let mut j = i;
        while matches!(b.get(j), Some(c) if c.is_ascii_whitespace()) {
            j += 1;
        }
        if b.get(j) != Some(&b'=') {
            i = j;
            continue;
        }
        j += 1;
        while matches!(b.get(j), Some(c) if c.is_ascii_whitespace()) {
            j += 1;
        }
        i = match b.get(j) {
            Some(&q) if q == b'"' || q == b'\'' => find_byte(b, j + 1, q).map_or(b.len(), |e| e + 1),
            _ => (j..b.len()).find(|&e| b.get(e).is_some_and(|c| c.is_ascii_whitespace() || *c == b'>')).unwrap_or(b.len()),
        };
    }
}

fn find_byte(b: &[u8], from: usize, byte: u8) -> Option<usize> {
    b.get(from..)?.iter().position(|c| *c == byte).map(|i| i + from)
}

fn find_sub(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    b.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

/// Where `</name` starts (any letter case, then white space, `/`, `>` or the end), at or after
/// `from`, as the reader looks for a skipped or raw-text element's end.
fn find_end_tag(b: &[u8], from: usize, name: &str) -> Option<usize> {
    let mut i = from;
    while let Some(lt) = find_byte(b, i, b'<') {
        let after = lt + 2 + name.len();
        let named = b.get(lt + 1) == Some(&b'/') && b.get(lt + 2..after).is_some_and(|n| n.eq_ignore_ascii_case(name.as_bytes()));
        if named && b.get(after).is_none_or(|c| c.is_ascii_whitespace() || matches!(*c, b'/' | b'>')) {
            return Some(lt);
        }
        i = lt + 1;
    }
    None
}

/// Where the name of the `<img>` start tag spanning `within` ends, checked against the text.
fn image_name_end(html: &str, within: &Range<usize>) -> Result<usize, CheckError> {
    let end = within.start.checked_add(4).ok_or(CheckError::BadRange)?;
    let written = html.get(within.start..end).ok_or(CheckError::BadRange)?;
    let named = written.strip_prefix('<').is_some_and(|name| name.eq_ignore_ascii_case("img"));
    if named && end < within.end { Ok(end) } else { Err(CheckError::BadRange) }
}

/// OL-IMG-001 and OL-IMG-003: what the picture's source says it is; OL-IMG-004: a source
/// written into the HTML as a `data:` address, whatever it says it is.
fn source(html: &str, tag: &Tag<'_>, within: &Range<usize>, found: &mut Vec<Finding>) -> Result<(), CheckError> {
    let Some(raw) = tag.raw_attr("src") else { return Ok(()) };
    let decoded = unescape(raw);
    let src = decoded.trim_matches(|c: char| c.is_ascii_whitespace());
    let kind = source_rule(src);
    let written_in = strip_prefix_ci(src, "data:").is_some();
    if kind.is_none() && !written_in {
        return Ok(());
    }
    let range = range_in(html, raw, within)?;
    if let Some(rule) = kind {
        found.push(finding(rule, range.clone(), None));
    }
    if written_in {
        found.push(finding(Rule::DataUriImage, range, None));
    }
    Ok(())
}

/// The rule a picture's (decoded, trimmed) source breaks, if any.
fn source_rule(src: &str) -> Option<Rule> {
    if strip_prefix_ci(src, "cid:").is_some() {
        return None;
    }
    if let Some(rest) = strip_prefix_ci(src, "data:") {
        let kind = rest.split([';', ',']).next().unwrap_or("").trim();
        return kind_rule(strip_prefix_ci(kind, "image/")?);
    }
    let path = src.split(['?', '#']).next().unwrap_or("");
    let last = path.rsplit('/').next().unwrap_or("");
    let (_, extension) = last.rsplit_once('.')?;
    kind_rule(extension)
}

/// The rule a picture type (an extension or the part of a media type after `image/`) breaks.
fn kind_rule(kind: &str) -> Option<Rule> {
    let is = |name: &str| kind.eq_ignore_ascii_case(name);
    if is("svg") || is("svg+xml") {
        Some(Rule::SvgImage)
    } else if is("webp") || is("avif") || is("heic") || is("heif") {
        Some(Rule::UnsupportedImageFormat)
    } else {
        None
    }
}

/// `s` without `prefix`, matched in any ASCII letter case.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) { s.get(prefix.len()..) } else { None }
}

/// A size the inline style gives: where its declaration is, and its whole pixels when it is
/// written as plain pixels.
struct StyleSize {
    range: Range<usize>,
    px: Option<u16>,
}

/// OL-IMG-005: a size in the inline style with no matching attribute.
fn size(html: &str, tag: &Tag<'_>, within: &Range<usize>, name_end: usize, found: &mut Vec<Finding>) -> Result<(), CheckError> {
    let Some(raw) = tag.raw_attr("style") else { return Ok(()) };
    // A style with a character reference in it is read decoded, as the writer writes it; its
    // parts can't be told apart by their bytes then, so a size is found on the whole value.
    let decoded: String;
    let (text, whole) = if raw.contains('&') {
        decoded = unescape(raw);
        (decoded.as_str(), Some(range_in(html, raw, within)?))
    } else {
        (raw, None)
    };
    // Each size with whether it is `!important`.
    let mut width: Option<(bool, Option<StyleSize>)> = None;
    let mut height: Option<(bool, Option<StyleSize>)> = None;
    // `text` is decoded once already (or has no reference to decode): `&amp;#49;` is `&#49;`, as
    // the writer writes it, and is not read again.
    for (property, value) in declarations(text) {
        let name = after_comments(property);
        let slot = if name.eq_ignore_ascii_case("width") {
            &mut width
        } else if name.eq_ignore_ascii_case("height") {
            &mut height
        } else {
            continue;
        };
        let (value_text, important) = strip_important(value);
        // No CSS value holds a `&` (one that is left once decoded was written twice): CSS drops
        // such a declaration, and the one before it stands.
        if value_text.contains('&') {
            continue;
        }
        let range = match &whole {
            Some(whole) => whole.clone(),
            None => range_in(html, property, within)?.start..range_in(html, value, within)?.end,
        };
        // As in CSS: an `!important` declaration wins over every one that isn't, and among
        // those alike the last does; a keyword (`auto`) sets no size.
        if slot.as_ref().is_none_or(|(was, _)| important || !*was) {
            *slot = Some((important, style_px(value_text).map(|px| StyleSize { range, px })));
        }
    }
    let missing: Vec<(&str, StyleSize)> = [("width", width), ("height", height)]
        .into_iter()
        .filter_map(|(name, size)| Some((name, size?.1?)))
        .filter(|(name, _)| !tag.attr(name).is_some_and(|v| !v.trim().is_empty()))
        .collect();
    let (Some(start), Some(end)) = (missing.iter().map(|(_, s)| s.range.start).min(), missing.iter().map(|(_, s)| s.range.end).max()) else {
        return Ok(());
    };
    let mut replacement = String::new();
    let mut fixable = true;
    for (name, size) in &missing {
        match size.px {
            Some(px) => replacement.push_str(&format!(" {name}=\"{px}\"")),
            None => fixable = false,
        }
    }
    let fix = fixable.then_some(Fix { range: name_end..name_end, replacement });
    found.push(finding(Rule::CssOnlyImageSize, start..end, fix));
    Ok(())
}

/// What a `width` or `height` value sets: `None` for a keyword or nothing, `Some(Some(px))` for
/// plain whole pixels, `Some(None)` for any other size (a percentage, another unit, a fraction,
/// a function).
fn style_px(value: &str) -> Option<Option<u16>> {
    let value = value.trim();
    if value.is_empty() || value.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
        return None;
    }
    let px = value
        .len()
        .checked_sub(2)
        .filter(|&at| value.get(at..).is_some_and(|unit| unit.eq_ignore_ascii_case("px")))
        .and_then(|at| value.get(..at))
        .filter(|digits| (1..=5).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|digits| digits.parse::<u16>().ok());
    Some(px)
}

/// Where `part`, a slice the tokenizer gave of `html`, sits in it, checked to lie inside the
/// start tag spanning `within`, before its `>`.
fn range_in(html: &str, part: &str, within: &Range<usize>) -> Result<Range<usize>, CheckError> {
    let range = span_of(html, part).ok_or(CheckError::BadRange)?;
    if range.start <= within.start || range.end >= within.end {
        return Err(CheckError::BadRange);
    }
    Ok(range)
}

/// `property`, a declaration's name as the declarations were split, from past the CSS white space
/// and comments it starts with, as a CSS reader reads it: still a slice of the text it came from.
fn after_comments(property: &str) -> &str {
    let b = property.as_bytes();
    let mut i = 0;
    loop {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if b.get(i) == Some(&b'/') && b.get(i + 1) == Some(&b'*') {
            i = find_sub(b, i + 2, b"*/").map_or(b.len(), |e| e + 2);
        } else {
            break;
        }
    }
    property.get(i..).unwrap_or("")
}

/// One CSS declaration the checker reads, from a `style` attribute or a `<style>` rule.
struct Decl {
    /// Its property's name, as written.
    property: String,
    /// Its value, `!important` and all.
    value: String,
    /// Its bytes in the checked text, its name to its value's end; for a `style` attribute with a
    /// character reference in it, whose parts can't be told apart by their bytes, the whole value
    /// of the attribute.
    range: Range<usize>,
    /// Where `value` starts in the checked text, when it is written there just as it is here.
    value_at: Option<usize>,
    place: Place,
}

/// Where a declaration is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// In the `style` attribute of the `element`th start tag of the text, which is a `<table>`'s
    /// or not.
    Inline { element: usize, table: bool },
    /// In the `n`th rule of the style sheets ([`Styles::rules`]).
    Sheet { rule: usize },
}

/// One rule of a `<style>` sheet, with what the passes ask of its selectors worked out once
/// when it is read, however many declarations it has.
struct StyleRule {
    /// The whole rule, its selectors to its closing brace.
    range: Range<usize>,
    /// Whether each of its selectors ends with a `table` ([`tables_only`]).
    tables_only: bool,
    /// Whether one of its selectors is for `::before` or `::after` (or the old `:before`,
    /// `:after`).
    generated: bool,
    /// Whether it is the root of the document (`:root`, `html`, `body` or `*`) outside `@media`,
    /// and so holds for every window.
    root: bool,
}

/// The CSS of the checked text: every declaration, with the rules of the sheets they are in.
#[derive(Default)]
struct Styles {
    decls: Vec<Decl>,
    rules: Vec<StyleRule>,
}

/// Most `@media` blocks inside one another whose rules are read.
const MAX_SHEET_LEVELS: usize = 8;

impl Styles {
    /// The declarations of the `style` attribute of `tag`, the `element`th start tag of `html`,
    /// which spans `within`.
    fn inline(&mut self, html: &str, tag: &Tag<'_>, within: &Range<usize>, element: usize) -> Result<(), CheckError> {
        let Some(raw) = tag.raw_attr("style").filter(|s| !s.trim().is_empty()) else { return Ok(()) };
        let place = Place::Inline { element, table: tag.name == "table" };
        if raw.contains('&') {
            let whole = range_in(html, raw, within)?;
            let decoded = unescape(raw);
            for (property, value) in declarations(&decoded) {
                let property = after_comments(property);
                self.decls.push(Decl { property: property.to_string(), value: value.to_string(), range: whole.clone(), value_at: None, place });
            }
            return Ok(());
        }
        for (property, value) in declarations(raw) {
            let property = after_comments(property);
            let (p, v) = (range_in(html, property, within)?, range_in(html, value, within)?);
            self.decls.push(Decl { property: property.to_string(), value: value.to_string(), range: p.start..v.end, value_at: Some(v.start), place });
        }
        Ok(())
    }

    /// The rules and declarations of the sheet `html[css]`, `level` `@media` blocks deep (from 0),
    /// walked as the writer of an authored signature walks them: every `@media` block, whatever
    /// its media say; no other at-rule.
    fn sheet(&mut self, html: &str, css: Range<usize>, level: usize) {
        let Some(text) = html.get(css.clone()) else { return };
        let b = text.as_bytes();
        let mut i = 0;
        loop {
            i = skip_trivia(b, i);
            match b.get(i) {
                None => return,
                // A `}` with no block to close.
                Some(b'}') => i += 1,
                Some(b'@') => {
                    let Some(end) = scan_to(b, i, b";{") else { return };
                    if b.get(end) == Some(&b';') {
                        i = end + 1;
                        continue;
                    }
                    let close = block_end(b, end);
                    if media_prelude(text.get(i..end).unwrap_or("")).is_some() && level < MAX_SHEET_LEVELS {
                        self.sheet(html, css.start + end + 1..css.start + close, level + 1);
                    }
                    i = close.saturating_add(1);
                }
                Some(_) => {
                    let Some(open) = scan_to(b, i, b"{") else { return };
                    let close = block_end(b, open);
                    let rule = self.rules.len();
                    let prelude = text.get(i..open).unwrap_or("").trim();
                    let lower = prelude.to_ascii_lowercase();
                    self.rules.push(StyleRule {
                        range: css.start + i..css.start + close.saturating_add(1).min(b.len()),
                        tables_only: tables_only(prelude),
                        generated: lower.contains(":before") || lower.contains(":after"),
                        root: level == 0 && matches!(lower.as_str(), ":root" | "html" | "body" | "*"),
                    });
                    for (property, value) in declarations(text.get(open + 1..close).unwrap_or("")) {
                        let property = after_comments(property);
                        let (Some(p), Some(v)) = (span_of(html, property), span_of(html, value)) else { continue };
                        self.decls.push(Decl {
                            property: property.to_string(),
                            value: value.to_string(),
                            range: p.start..v.end,
                            value_at: Some(v.start),
                            place: Place::Sheet { rule },
                        });
                    }
                    i = close.saturating_add(1);
                }
            }
        }
    }
}

/// OL-SEL-003: a rule for `::before` or `::after` (or the old `:before`, `:after`) that adds
/// text with `content`: once for each such rule, on the whole rule.
fn generated_content(styles: &Styles, found: &mut Vec<Finding>) -> Result<(), CheckError> {
    let mut last = None;
    for d in &styles.decls {
        let Place::Sheet { rule } = d.place else { continue };
        if last == Some(rule) || !d.property.eq_ignore_ascii_case("content") {
            continue;
        }
        let (value, _) = strip_important(&d.value);
        if matches!(value.to_ascii_lowercase().as_str(), "none" | "normal" | "\"\"" | "''") {
            continue;
        }
        let Some(r) = styles.rules.get(rule) else { continue };
        if r.generated {
            note(found, finding(Rule::GeneratedContent, r.range.clone(), None))?;
            last = Some(rule);
        }
    }
    Ok(())
}

/// A call of a CSS function in a value: its bytes in the value, its name as written, and what is
/// between its parentheses.
struct Call<'a> {
    range: Range<usize>,
    args: &'a str,
    /// Whether its `)` was found; when not, it runs to the end of the value.
    closed: bool,
}

/// The calls in `value` of any of the functions `names` (in any letter case), outside strings
/// and comments. The calls inside a call of another function are looked for too; those inside
/// one of `names` are not.
fn calls<'a>(value: &'a str, names: &[&str]) -> Vec<Call<'a>> {
    let b = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c == b'"' || c == b'\'' {
            i = find_byte(b, i + 1, c).map_or(b.len(), |e| e + 1);
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = find_sub(b, i + 2, b"*/").map_or(b.len(), |e| e + 2);
        } else if is_name_start(c) {
            let start = i;
            while b.get(i).is_some_and(|c| is_name_byte(*c)) {
                i += 1;
            }
            let name = value.get(start..i).unwrap_or("");
            if b.get(i) == Some(&b'(') && names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                let (end, closed) = matching_paren(b, i).map_or((b.len(), false), |e| (e + 1, true));
                let args = value.get(i + 1..if closed { end - 1 } else { end }).unwrap_or("");
                out.push(Call { range: start..end, args, closed });
                i = end;
            }
        } else {
            i += 1;
        }
    }
    out
}

fn is_name_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'-' || c == b'_'
}

fn is_name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

/// The `)` that closes the `(` at `open`, outside strings.
fn matching_paren(b: &[u8], open: usize) -> Option<usize> {
    let (mut depth, mut i) = (0u32, open);
    while let Some(&c) = b.get(i) {
        match c {
            b'"' | b'\'' => i = find_byte(b, i + 1, c)?,
            b'(' => depth = depth.saturating_add(1),
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Where a part of a declaration's value is in the checked text: `part`, its bytes in the value,
/// when the value is written there as it is; else the whole declaration, and no fix can be had.
fn part_range(d: &Decl, part: &Range<usize>) -> (Range<usize>, bool) {
    match d.value_at {
        Some(at) => (at + part.start..at + part.end, true),
        None => (d.range.clone(), false),
    }
}

/// OL-DEC-003: each custom property's definition, and each `var()`. A `var()` in a `style`
/// attribute is fixed by putting its variable's value in its place when that value is known for
/// certain ([`variable_value`]); nothing else has a fix.
fn variables(styles: &Styles, found: &mut Vec<Finding>) -> Result<(), CheckError> {
    let defined = definitions(styles)?;
    for d in &styles.decls {
        if d.property.starts_with("--") {
            note(found, finding(Rule::CssVariable, d.range.clone(), None))?;
        }
        for call in calls(&d.value, &["var"]) {
            let (range, exact) = part_range(d, &call.range);
            let fix =
                exact.then(|| variable_value(styles, &defined, d, &call)).flatten().map(|value| Fix { range: range.clone(), replacement: value });
            note(found, finding(Rule::CssVariable, range, fix))?;
        }
    }
    Ok(())
}

/// OL-COL-002: each call of `rgb()`, `rgba()`, `hsl()` or `hsla()` whose numbers are separated by
/// spaces (or a `/`) rather than commas. `rgb()` of three plain whole numbers is fixed by
/// separating them with commas, its name kept as written; nothing else has a fix.
fn spaced_colours(styles: &Styles, found: &mut Vec<Finding>) -> Result<(), CheckError> {
    for d in &styles.decls {
        for call in calls(&d.value, &["rgb", "rgba", "hsl", "hsla"]) {
            let args = call.args.trim();
            if args.is_empty() || args.contains(',') || !(args.contains(|c: char| c.is_ascii_whitespace()) || args.contains('/')) {
                continue;
            }
            let (range, exact) = part_range(d, &call.range);
            let fix = exact.then(|| with_commas(&d.value, &call)).flatten().map(|text| Fix { range: range.clone(), replacement: text });
            note(found, finding(Rule::SpaceSeparatedColour, range, fix))?;
        }
    }
    Ok(())
}

/// The call `call` of `value`, when it is `rgb()` of three plain whole numbers, written with
/// commas between them and its name as written.
fn with_commas(value: &str, call: &Call<'_>) -> Option<String> {
    let name = value.get(call.range.clone())?.split('(').next()?;
    let numbers: Vec<&str> = call.args.split_ascii_whitespace().collect();
    let plain = numbers.len() == 3 && numbers.iter().all(|n| (1..=3).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()));
    (call.closed && name.eq_ignore_ascii_case("rgb") && plain).then(|| format!("{name}({})", numbers.join(",")))
}

/// The gradient functions of OL-BG-004.
const GRADIENTS: [&str; 6] =
    ["linear-gradient", "radial-gradient", "conic-gradient", "repeating-linear-gradient", "repeating-radial-gradient", "repeating-conic-gradient"];

/// The warnings a declaration gives by its property and value alone, each on the whole
/// declaration and without a fix: OL-LAY-001 to OL-LAY-005, OL-BOX-007, OL-BOX-008, OL-BG-001,
/// OL-BG-004 and OL-FNT-002.
fn declaration_warnings(styles: &Styles, found: &mut Vec<Finding>) -> Result<(), CheckError> {
    for d in &styles.decls {
        let property = d.property.to_ascii_lowercase();
        let (value, _) = strip_important(&d.value);
        let value = value.to_ascii_lowercase();
        let mut warn = |rule: Rule| note(found, finding(rule, d.range.clone(), None));
        match property.as_str() {
            "display" if matches!(value.as_str(), "flex" | "inline-flex") => warn(Rule::Flexbox)?,
            "display" if matches!(value.as_str(), "grid" | "inline-grid") => warn(Rule::Grid)?,
            "float" if value != "none" => warn(Rule::Float)?,
            "position" if matches!(value.as_str(), "absolute" | "fixed" | "relative") => warn(Rule::Positioned)?,
            "max-width" | "min-width" if value != "none" && !on_table(styles, d) => warn(Rule::MinMaxWidth)?,
            "border-radius" | "border-top-left-radius" | "border-top-right-radius" | "border-bottom-left-radius" | "border-bottom-right-radius"
                if !all_zero(&value) =>
            {
                warn(Rule::RoundedCorners)?
            }
            "box-shadow" if value != "none" => warn(Rule::BoxShadow)?,
            "background" | "background-image" => {
                let gradient = !calls(&value, &GRADIENTS).is_empty();
                let picture = !calls(&value, &["url"]).is_empty() || (property == "background-image" && !gradient && value != "none");
                if picture {
                    warn(Rule::BackgroundImage)?;
                }
                if gradient {
                    warn(Rule::Gradient)?;
                }
            }
            "font-family" if !ends_generic(&value) => warn(Rule::NoGenericFontFamily)?,
            _ => {}
        }
    }
    Ok(())
}

/// Whether the declaration `d` is for tables alone: in a `<table>`'s `style`, or in a rule each of
/// whose selectors ends with a `table`.
fn on_table(styles: &Styles, d: &Decl) -> bool {
    match d.place {
        Place::Inline { table, .. } => table,
        Place::Sheet { rule } => styles.rules.get(rule).is_some_and(|r| r.tables_only),
    }
}

/// Whether each selector of the list `prelude` ends with a `table`.
fn tables_only(prelude: &str) -> bool {
    split_commas(prelude).iter().all(|selector| {
        let last = selector.rsplit(|c: char| c.is_ascii_whitespace() || matches!(c, '>' | '+' | '~')).next().unwrap_or("");
        let name = last.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).next().unwrap_or("");
        name.eq_ignore_ascii_case("table")
    })
}

/// Whether every length of `value` (lower case) is a zero: square corners.
fn all_zero(value: &str) -> bool {
    let mut lengths = value.split(|c: char| c.is_ascii_whitespace() || c == '/').filter(|t| !t.is_empty()).peekable();
    lengths.peek().is_some()
        && lengths.all(|t| t.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%').parse::<f64>().is_ok_and(|n| n == 0.0))
}

/// Whether the `font-family` list `value` (lower case) ends with a generic family, or is a
/// keyword that names no font.
fn ends_generic(value: &str) -> bool {
    if matches!(value, "inherit" | "initial" | "unset" | "revert" | "revert-layer") {
        return true;
    }
    let last = split_commas(value).last().map_or("", |f| f.trim());
    matches!(last, "serif" | "sans-serif" | "monospace")
}

/// OL-UNIT-001 and OL-UNIT-002: each size in `rem`, or in `vw`, `vh`, `vmin` or `vmax`, on the
/// number and its unit, without a fix.
fn units(styles: &Styles, found: &mut Vec<Finding>) -> Result<(), CheckError> {
    for d in &styles.decls {
        for (size, unit) in dimensions(&d.value) {
            let rule = if unit.eq_ignore_ascii_case("rem") {
                Rule::RemUnit
            } else if ["vw", "vh", "vmin", "vmax"].iter().any(|u| unit.eq_ignore_ascii_case(u)) {
                Rule::ViewportUnit
            } else {
                continue;
            };
            note(found, finding(rule, part_range(d, &size).0, None))?;
        }
    }
    Ok(())
}

/// The sizes in a CSS value (a number with a unit), outside strings, names and `url()`: each
/// one's bytes in the value, and its unit.
fn dimensions(value: &str) -> Vec<(Range<usize>, &str)> {
    let b = value.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let number =
            digit(i) || (c == b'.' && digit(i + 1)) || (matches!(c, b'-' | b'+') && (digit(i + 1) || (b.get(i + 1) == Some(&b'.') && digit(i + 2))));
        if c == b'"' || c == b'\'' {
            i = find_byte(b, i + 1, c).map_or(b.len(), |e| e + 1);
        } else if number {
            let start = i;
            i += 1;
            while b.get(i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
                i += 1;
            }
            let unit = i;
            while b.get(i).is_some_and(u8::is_ascii_alphabetic) {
                i += 1;
            }
            if i > unit {
                out.push((start..i, value.get(unit..i).unwrap_or("")));
            }
        } else if is_name_start(c) || c == b'#' {
            let start = i;
            i += 1;
            while b.get(i).is_some_and(|c| is_name_byte(*c)) {
                i += 1;
            }
            if value.get(start..i).is_some_and(|name| name.eq_ignore_ascii_case("url")) && b.get(i) == Some(&b'(') {
                i = matching_paren(b, i).map_or(b.len(), |e| e + 1);
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The value a `var()` of the declaration `used` stands for, when it is known for certain and can
/// be written in its place as it is: the call is in a `style` attribute, its variable is defined
/// once in the text, at the root of the document (`:root`, `html`, `body` or `*`, outside
/// `@media`) or in the same `style` attribute, and that definition is a plain value (no `var()`,
/// no `!important`, no quote, character reference, `<`, `>`, `\`, `;` or brace, and no CSS-wide
/// keyword such as `initial`: a `var()` of one stands for its fallback, never for the keyword)
/// of at most [`MAX_FIX_BYTES`] bytes.
fn variable_value(styles: &Styles, defined: &HashMap<&str, Definition<'_>>, used: &Decl, call: &Call<'_>) -> Option<String> {
    let Place::Inline { element, .. } = used.place else { return None };
    let name = call.args.split(',').next()?.trim();
    if !call.closed || !name.starts_with("--") {
        return None;
    }
    let definition = defined.get(name).filter(|d| d.count == 1)?;
    let everywhere = match definition.decl.place {
        Place::Inline { element: at, .. } => at == element,
        Place::Sheet { rule } => styles.rules.get(rule).is_some_and(|r| r.root),
    };
    let value = definition.plain?;
    everywhere.then(|| value.to_string())
}

/// A custom property of the checked text: how many times it is defined, its first definition,
/// and that definition's value when it is a plain one ([`plain_value`]).
struct Definition<'s> {
    count: usize,
    decl: &'s Decl,
    plain: Option<&'s str>,
}

/// Every custom property of `styles` by its name as written, read once for all the `var()` calls
/// that may name it. Each definition is a finding of its own ([`variables`]), so more than
/// [`MAX_FINDINGS`] of them are refused here, as they would be there, before the index holds them.
fn definitions(styles: &Styles) -> Result<HashMap<&str, Definition<'_>>, CheckError> {
    let mut out: HashMap<&str, Definition<'_>> = HashMap::new();
    for (n, d) in styles.decls.iter().filter(|d| d.property.starts_with("--")).enumerate() {
        if n >= MAX_FINDINGS {
            return Err(CheckError::TooManyFindings);
        }
        out.entry(d.property.as_str()).and_modify(|seen| seen.count = seen.count.saturating_add(1)).or_insert_with(|| Definition {
            count: 1,
            decl: d,
            plain: plain_value(d),
        });
    }
    Ok(out)
}

/// The CSS-wide keywords: as a custom property's value, each makes a `var()` of it stand for its
/// fallback or for nothing, not for the keyword.
const CSS_WIDE: [&str; 5] = ["initial", "inherit", "unset", "revert", "revert-layer"];

/// The value of the definition `d` when it can be written in a `var()`'s place as it is: written
/// in the checked text just as it is here, plain ([`variable_value`]), and short enough to be
/// copied into each call's fix ([`MAX_FIX_BYTES`]).
fn plain_value(d: &Decl) -> Option<&str> {
    d.value_at?;
    let (value, important) = strip_important(&d.value);
    let plain = !value.is_empty()
        && value.len() <= MAX_FIX_BYTES
        && !important
        && !value.to_ascii_lowercase().contains("var(")
        && !value.contains(['"', '\'', '&', '<', '>', '\\', ';', '{', '}'])
        && !CSS_WIDE.iter().any(|k| value.trim().eq_ignore_ascii_case(k));
    plain.then_some(value)
}

/// One `<style>` sheet of the checked text, as the writer of an authored signature reads them.
struct StyleElement {
    /// The sheet's own text.
    css: Range<usize>,
    /// The whole element, start tag to the end tag's `>`; from the sheet's first byte when its
    /// start tag can't be found again.
    element: Range<usize>,
    /// Its start tag; the sheet's own text when that can't be found again.
    start_tag: Range<usize>,
}

/// Most scope classes tried before the checker gives up looking for one the text doesn't hold.
const MAX_SCOPE_TRIES: usize = 32;

/// MC-CSS-001: the signature's style sheets, when the writer that puts a signature in a message
/// with them ([`crate::authored_fragment_with_rebound_cids`]) refuses the signature. Said once,
/// on the first sheet that the writer refuses the text up to the end of; on the first sheet's
/// start tag when no one sheet explains it. Nothing is said when the text has no sheet.
fn stylesheet(html: &str, sheets: &[StyleElement]) -> Option<Finding> {
    let first = sheets.first()?;
    let scope = free_scope(html);
    let refused = |text: &str| match &scope {
        Some(scope) => crate::authored_fragment_with_rebound_cids(text, scope, &mut |cid: &str| Some(cid.to_string())).is_err(),
        // Every class tried is in the text already: not known to be sendable.
        None => true,
    };
    if !refused(html) {
        return None;
    }
    let refused_to = |i: usize| sheets.get(i).and_then(|s| html.get(..s.element.end)).is_none_or(refused);
    // The first sheet whose text, with everything before it, is refused; the writer reads the
    // sheets in order, so the answers go from no to yes once.
    let (mut low, mut high) = (0usize, sheets.len());
    while low < high {
        let mid = low + (high - low) / 2;
        if refused_to(mid) { high = mid } else { low = mid + 1 }
    }
    let range = match sheets.get(low) {
        Some(sheet) => sheet.element.clone(),
        None => first.start_tag.clone(),
    };
    Some(finding(Rule::StylesheetNotSendable, range, None))
}

/// A scope class for the writer that the text doesn't hold.
fn free_scope(html: &str) -> Option<crate::AuthoredScope> {
    let lower = html.to_ascii_lowercase();
    (0..MAX_SCOPE_TRIES)
        .map(|n| if n == 0 { "mc-check".to_string() } else { format!("mc-check-{n}") })
        .find(|class| !lower.contains(class.as_str()))
        .and_then(|class| crate::AuthoredScope::new(&class).ok())
}

/// The style sheets of `html` that the writer reads (none in a `<template>`), in order.
fn style_elements(html: &str) -> Vec<StyleElement> {
    let mut out = Vec::new();
    let mut template = 0usize;
    let mut tokens = Tokenizer::new(html);
    loop {
        let token = tokens.next();
        for (_, css) in std::mem::take(&mut tokens.styles) {
            if template == 0
                && let Some(sheet) = style_element(html, css)
            {
                out.push(sheet);
            }
        }
        match &token {
            None => break,
            Some(Token::Open(tag)) if tag.name == "template" => template = template.saturating_add(1),
            Some(Token::Close(name)) if name == "template" => template = template.saturating_sub(1),
            _ => {}
        }
    }
    out
}

/// Where the sheet `css`, a slice the tokenizer gave of `html`, and its element are.
fn style_element(html: &str, css: &str) -> Option<StyleElement> {
    let Range { start, end } = span_of(html, css)?;
    // The element ends where the tokenizer goes on from: after the `>` that follows the sheet.
    let close = html.get(end..)?.find('>').map_or(html.len(), |at| end + at + 1);
    Some(match style_start(html, css, start) {
        Some(tag) => StyleElement { css: start..end, element: tag..close, start_tag: tag..start },
        None => StyleElement { css: start..end, element: start..close, start_tag: start..end },
    })
}

/// Most `<` looked back at for the start tag of a sheet.
const MAX_START_TRIES: usize = 16;

/// Where the start tag of the sheet `css`, which begins at `at` in `html`, begins: the nearest
/// `<style` before it from which the tokenizer reads that very sheet.
fn style_start(html: &str, css: &str, at: usize) -> Option<usize> {
    let mut before = at;
    for _ in 0..MAX_START_TRIES {
        let p = html.get(..before)?.rfind('<')?;
        if html.get(p + 1..p + 6).is_some_and(|name| name.eq_ignore_ascii_case("style")) {
            let mut again = Tokenizer::new(html.get(p..)?);
            let _ = again.next();
            if again.styles.first().is_some_and(|(_, read)| read.as_ptr() == css.as_ptr() && read.len() == css.len()) {
                return Some(p);
            }
        }
        before = p;
    }
    None
}
