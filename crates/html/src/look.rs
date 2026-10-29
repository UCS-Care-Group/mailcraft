// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether an element that asks to be a box on a line (`display: inline-block`) is one.
//!
//! A row of buttons, a navigation bar or a pair of columns holds nothing but its boxes. A badge
//! in a sentence, or a link with a bar after it, is text with a highlight, and drawing it as a
//! box would break the sentence in two. So before an element that asks for it is made a box, the
//! element that holds it is read to its end: the box stands only if everything else in the
//! holder is white space (a space, a no-break space, a zero-width non-joiner), a comment or
//! something hidden, besides other boxes. Otherwise the element is drawn in the line, as it
//! would be without `display: inline-block`.
//!
//! The look is bounded: it reads at most [`MAX_TOKENS`] tokens for one holder, the looks of a
//! message together read at most [`MAX_LOOKED`], and the verdict for a holder is kept, so a page
//! of many boxes is read once. Past a bound the answer is no.

use std::collections::HashMap;

use crate::blocks::{block_name, canon, css_of, has_box_look, left_out};
use crate::entities::unescape;
use crate::is_blank;
use crate::media::Env;
use crate::sheet::Sheet;
use crate::style::Css;
use crate::tokenizer::{Tag, Token, Tokenizer};

/// Most tokens one look reads.
const MAX_TOKENS: usize = 8_192;

/// Most that the looks of a message read together, in tokens and in sixteens of bytes of text.
const MAX_LOOKED: usize = 1 << 20;

/// Deepest nesting a look follows.
const MAX_NESTING: usize = 64;

/// The element that holds the one about to open: where its start tag ended, in bytes and in
/// start tags counted, and its name. The message itself, before any element, is `at: (0, 0)`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Holder {
    pub id: usize,
    pub at: (usize, usize),
    pub name: &'static str,
}

/// The message's style sheets, and the verdicts so far.
pub(crate) struct Look<'a> {
    html: &'a str,
    sheet: Option<Sheet>,
    verdicts: HashMap<usize, bool>,
    left: usize,
}

impl<'a> Look<'a> {
    /// `env`: the pane and theme the `@media` of the sheets are tested against, for the look as
    /// for the build.
    pub fn new(html: &'a str, env: &Env) -> Look<'a> {
        Look { html, sheet: Sheet::read_in(html, env), verdicts: HashMap::new(), left: MAX_LOOKED }
    }

    /// Put what the sheet says about the `ordinal`th start tag on `tag`. Whether a rule for dark
    /// windows gave it colours (`Sheet::dark_hit`), to be asked before anything else is.
    pub fn apply(&mut self, ordinal: usize, tag: &mut Tag<'_>) -> bool {
        let Some(sheet) = self.sheet.as_mut() else { return false };
        sheet.apply(ordinal, tag);
        sheet.dark_hit()
    }

    /// The sender designed the message for dark windows (`Sheet::designed_for_dark`).
    pub fn designed_for_dark(&mut self) {
        if let Some(sheet) = self.sheet.as_mut() {
            sheet.designed_for_dark();
        }
    }

    pub fn dark_design(&self) -> bool {
        self.sheet.as_ref().is_some_and(|s| s.dark_design)
    }

    /// How many answers of the sheet are kept.
    #[cfg(test)]
    pub fn kept(&self) -> usize {
        self.sheet.as_ref().map_or(0, Sheet::kept)
    }

    /// Whether the holder holds nothing but boxes and white space.
    pub fn only_boxes(&mut self, holder: &Holder) -> bool {
        if let Some(verdict) = self.verdicts.get(&holder.id) {
            return *verdict;
        }
        let verdict = self.read(holder);
        self.verdicts.insert(holder.id, verdict);
        verdict
    }

    fn read(&mut self, holder: &Holder) -> bool {
        if let Some(sheet) = self.sheet.as_mut() {
            sheet.keep_answers();
        }
        let Some(rest) = self.html.get(holder.at.0..) else { return false };
        let tokens = Tokenizer::new(rest);
        let mut ordinal = holder.at.1;
        // The elements open since the holder began, and the height of that stack at which what
        // is inside is not read (a box, something hidden).
        let mut open: Vec<String> = Vec::new();
        let mut skipping: Option<usize> = None;
        let mut read = 0usize;
        for mut token in tokens {
            read += 1;
            let cost = 1 + match &token {
                Token::Text(text) => text.len() / 16,
                _ => 0,
            };
            if read > MAX_TOKENS || self.left < cost {
                return false;
            }
            self.left -= cost;
            match &mut token {
                Token::Text(text) => {
                    if skipping.is_none() && !is_white_space(text) {
                        return false;
                    }
                }
                Token::Open(tag) => {
                    // (The same sheet and pane as the build's; its answers are kept by the sheet.)
                    if let Some(sheet) = self.sheet.as_mut() {
                        tag.sheet = sheet.style_for(ordinal, tag.has("style"));
                    }
                    ordinal = ordinal.saturating_add(1);
                    let name = tag.name.as_str();
                    // What the build ends at this tag it ends here, in a box or out of one: a
                    // `<p>` at a block, a link at a link.
                    // (A heading does not nest in a heading: the build ends one that is the last open.)
                    let heading = open.len().checked_sub(1).filter(|i| canon(name) == "h1" && open.get(*i).is_some_and(|n| n == "h1"));
                    let paragraph = if ends_holder("p", name) { open_at(&open, "p") } else { None };
                    let link = if name == "a" { open_at(&open, "a") } else { None };
                    let ends = [heading, paragraph, link].into_iter().flatten().min();
                    if let Some(index) = ends {
                        open.truncate(index);
                        if skipping.is_some_and(|level| index <= level) {
                            skipping = None;
                        }
                    }
                    if skipping.is_none() {
                        if ends_holder(holder.name, name) {
                            return true;
                        }
                        let css = css_of(tag);
                        let hidden = left_out(tag, &css);
                        if !hidden && holds_more(name) {
                            return false;
                        }
                        // What is inside a box, something hidden or a block of its own is not
                        // in the line.
                        if !is_void(name) && (hidden || is_block(name) || is_box(tag, &css)) {
                            skipping = Some(open.len());
                        }
                    }
                    if !is_void(name) {
                        if open.len() >= MAX_NESTING {
                            return false;
                        }
                        open.push(canon(&tag.name).to_string());
                    }
                }
                Token::Close(name) => {
                    let name = canon(name);
                    if let Some(index) = open.iter().rposition(|n| n == name) {
                        open.truncate(index);
                        if skipping.is_some_and(|level| index <= level) {
                            skipping = None;
                        }
                    } else if skipping.is_none() && closes_holder(holder.name, name) {
                        return true;
                    }
                }
            }
        }
        // The input ended: so did the holder.
        true
    }
}

/// Where the nearest open element of this name is, short of a table: where the build finds it too.
fn open_at(open: &[String], name: &str) -> Option<usize> {
    for (index, open) in open.iter().enumerate().rev() {
        if open == name {
            return Some(index);
        }
        if matches!(open.as_str(), "td" | "th" | "tr" | "table") {
            return None;
        }
    }
    None
}

/// Text that is nothing to see: white space, no-break spaces and the zero-width characters.
fn is_white_space(text: &str) -> bool {
    if text.contains('&') { unescape(text).chars().all(is_blank) } else { text.chars().all(is_blank) }
}

/// Whether an element with this style is a box on a line: what the build makes of one that has
/// `display: inline-block` and something to show of its own.
pub(crate) fn is_box(tag: &Tag<'_>, css: &Css) -> bool {
    if !css.inline_block || left_out(tag, css) {
        return false;
    }
    match tag.name.as_str() {
        "table" => true,
        "body" | "html" | "head" | "tr" | "td" | "th" => false,
        name => (block_name(name).is_some() || is_inline(name)) && has_box_look(css),
    }
}

/// The elements that are read as runs of text.
fn is_inline(name: &str) -> bool {
    matches!(
        name,
        "b" | "strong"
            | "i"
            | "em"
            | "cite"
            | "var"
            | "dfn"
            | "u"
            | "ins"
            | "s"
            | "strike"
            | "del"
            | "code"
            | "tt"
            | "kbd"
            | "samp"
            | "small"
            | "big"
            | "sub"
            | "sup"
            | "mark"
            | "font"
            | "a"
            | "span"
            | "label"
            | "abbr"
            | "acronym"
            | "q"
            | "time"
            | "bdi"
            | "bdo"
            | "data"
            | "nobr"
    )
}

/// Elements that have no end tag.
fn is_void(name: &str) -> bool {
    matches!(name, "br" | "img" | "hr" | "input" | "meta" | "link" | "area" | "base" | "col" | "embed" | "param" | "source" | "track" | "wbr")
}

/// Elements that are more than white space in a line of boxes: pictures, line breaks, controls,
/// and the parts of a table that have no table.
fn holds_more(name: &str) -> bool {
    matches!(
        name,
        "img"
            | "br"
            | "tr"
            | "td"
            | "th"
            | "caption"
            | "thead"
            | "tbody"
            | "tfoot"
            | "li"
            | "input"
            | "button"
            | "select"
            | "textarea"
            | "svg"
            | "video"
            | "audio"
            | "iframe"
            | "object"
            | "embed"
            | "canvas"
    )
}

/// Elements that make blocks of their own: what is in them is not in the line of the holder.
fn is_block(name: &str) -> bool {
    block_name(name).is_some()
        || matches!(
            name,
            "table" | "ul" | "ol" | "menu" | "dir" | "blockquote" | "pre" | "xmp" | "listing" | "hr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
        )
}

/// Whether a start tag ends the holder, as it ends it in a browser: a `<p>` by any block, a
/// table cell by the next cell or row.
fn ends_holder(holder: &str, name: &str) -> bool {
    match holder {
        "p" => is_block(name) || name == "li",
        "td" | "th" => matches!(name, "td" | "th" | "tr" | "tbody" | "thead" | "tfoot" | "caption"),
        _ => false,
    }
}

/// Whether an end tag that closes nothing opened inside ends the holder.
fn closes_holder(holder: &str, name: &str) -> bool {
    name == holder || matches!(name, "body" | "html") || (matches!(holder, "td" | "th") && matches!(name, "td" | "th" | "tr" | "table" | "tbody"))
}
