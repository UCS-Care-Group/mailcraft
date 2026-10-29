// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Hostile HTML → display blocks.
//!
//! This is not a tree builder. Open elements live on one small stack, each with the style in
//! force inside it (the whitelist of `style.rs`, inherited the way a browser inherits it), so an
//! end tag can close what it belongs to and nothing else. Blocks are collected in frames: the
//! message, and inside it every table cell and every block with a background of its own.

use std::rc::Rc;

use crate::data::{self, Budget, MAX_DATA_TEXT};
use crate::entities::unescape;
use crate::style::{self, Css, MsoList, Size};
use crate::tokenizer::{Tag, Token, Tokenizer};
use crate::{
    Align, Block, BlockKind, Cell, Insets, Length, Line, LineHeight, MAX_BLOCKS, MAX_BOX_PX, MAX_CELLS, MAX_INPUT, MAX_ROW_SPAN, MAX_TABLE_COLUMNS,
    MAX_TABLE_DEPTH, MAX_TABLE_ROWS, Marker, Rgb, Row, Sides, Span, Table, VAlign, is_blank, is_safe_url, truncate,
};

/// Deepest nesting of elements that is tracked; what is nested deeper keeps its text and loses its
/// structure and style.
const MAX_DEPTH: usize = 128;

/// Longest written-out list marker read from an Outlook list paragraph, in bytes.
const MAX_MARKER: usize = 24;

/// Most blank lines kept between two blocks.
const MAX_SPACE: u8 = 3;

/// The largest picture size a message can ask for, in CSS pixels.
const MAX_IMAGE_PX: f32 = 8192.0;

/// Most text a cell of a data table holds, in bytes; a table with more in a cell is laying things out.
const DATA_CELL_TEXT: usize = 240;

/// Hostile HTML mail as display blocks.
///
/// Scripts, styles, the head, comments and unknown markup are dropped (the text inside unknown
/// elements is kept). Character references are decoded and white space collapses as in a browser,
/// except in `<pre>`. A `<br>` is a `\n` in the span text. What a message hides (`display:none`
/// and the other tricks `style.rs` lists, the `hidden` attribute, one-pixel pictures) is not in
/// the result; text at a font size too small to read is not either, though what is inside it at
/// a size of its own is. Links and pictures are kept only when [`is_safe_url`] accepts their
/// address (a picture's that leaves its scheme out, `//host/x.png`, is read as `https`); a
/// picture without a safe address leaves its alt text.
///
/// A table becomes a [`Table`] with its cells' blocks inside, marked as data or layout; a table
/// that only wraps one thing is that thing. Lines that follow each other directly (`<div>` after
/// `<div>`, Outlook's `MsoNormal` paragraphs) have `space` 0, ordinary paragraphs 1, and a blank
/// paragraph is a blank line above the next block rather than a block.
///
/// A picture written into the message (`data:image/png;base64,…`) is shown as an attached one
/// is: a PNG, JPEG, GIF or WebP by its own bytes, within the caps for one and for a message
/// (`data.rs`); any other, or one past the caps, is a placeholder.
///
/// Never panics; reads at most [`MAX_INPUT`] bytes besides the pictures written into the
/// message, and gives at most [`MAX_BLOCKS`] blocks (plus the tables that hold some of them).
pub fn parse(html: &str) -> Vec<Block> {
    build(html, true)
}

/// [`parse`], without decoding the pictures written into the message: for when only the text
/// and the look are wanted (plain text, an HTML draft or a paste in the composer).
pub fn parse_text(html: &str) -> Vec<Block> {
    build(html, false)
}

fn build(html: &str, decode: bool) -> Vec<Block> {
    let mut builder = Builder::new(decode);
    let mut tokens = Tokenizer::new(truncate(html, MAX_INPUT.saturating_add(MAX_DATA_TEXT)));
    // The text of the pictures written into the message, which isn't counted against MAX_INPUT.
    let mut pictures = 0usize;
    while let Some(token) = tokens.next() {
        if builder.count >= MAX_BLOCKS {
            break;
        }
        if let Token::Open(tag) = &token
            && tag.name == "img"
            && let Some(src) = tag.raw_attr("src").filter(|src| data::is_data(src))
        {
            pictures = pictures.saturating_add(src.len());
        }
        let over = tokens.pos().saturating_sub(pictures).saturating_sub(MAX_INPUT);
        if over > 0 {
            // The rest of the text that ends past the input read, as if the input ended there.
            if let Token::Text(text) = token {
                builder.token(Token::Text(truncate(text, text.len().saturating_sub(over))));
            }
            break;
        }
        builder.token(token);
    }
    builder.finish()
}

/// The inline style in force.
#[derive(Clone, Debug, Default, PartialEq)]
struct Style {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    code: bool,
    color: Option<Rgb>,
    background: Option<Rgb>,
    size: Option<u8>,
    font: Option<Rc<str>>,
    align: Align,
    line_height: Option<LineHeight>,
    link: Option<Rc<str>>,
    /// The font size in force is too small to be meant to be read (2 pixels or less): text
    /// written at it is not shown. It lasts until an element inside names a size that can be
    /// read, or carries a class or an id ([`Delta::hooked`]), and it hides nothing but text:
    /// newsletters set a size of nothing around their pictures and columns to close the gaps
    /// between them.
    tiny: bool,
    /// The element that asked for that size carries a class or an id itself: what is in the
    /// elements inside it is not held to it.
    tiny_hooked: bool,
}

impl Style {
    fn span(&self, text: String) -> Span {
        Span {
            text,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            code: self.code,
            link: self.link.as_deref().map(str::to_string),
            color: self.color,
            background: self.background,
            size: self.size,
            font: self.font.as_deref().map(str::to_string),
        }
    }

    fn matches(&self, span: &Span) -> bool {
        self.bold == span.bold
            && self.italic == span.italic
            && self.underline == span.underline
            && self.strike == span.strike
            && self.code == span.code
            && self.color == span.color
            && self.background == span.background
            && self.size == span.size
            && self.font.as_deref() == span.font.as_deref()
            && self.link.as_deref() == span.link.as_deref()
    }
}

/// What one element changes in the style around it.
#[derive(Clone, Debug, Default)]
struct Delta {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    code: Option<bool>,
    /// `Some(None)`: back to the reader's own colour (a link doesn't take the colour around it).
    color: Option<Option<Rgb>>,
    background: Option<Rgb>,
    size: Option<Size>,
    font: Option<Rc<str>>,
    align: Option<Align>,
    /// `Some(None)`: `normal`.
    line_height: Option<Option<LineHeight>>,
    link: Option<Option<Rc<str>>>,
    /// A table cell: formatting elements and highlights from outside stop here (an unclosed `<b>`
    /// above a table does not make the table bold). Colour, size and alignment are inherited, as
    /// in a browser, and a link around a table still leads somewhere.
    cell: bool,
    /// The element carries a `class` or an `id`: the hooks by which a style sheet gives sizes.
    /// Style sheets are not read here, so a size of nothing around such an element says nothing
    /// of the size its text really has (builders set nothing on every cell and the sizes that
    /// count by class), and its text is shown.
    hooked: bool,
}

impl Delta {
    /// What a `style` attribute asks for.
    fn of(css: &Css) -> Delta {
        Delta {
            bold: css.bold,
            italic: css.italic,
            underline: css.underline,
            strike: css.strike,
            color: css.color.map(Some),
            size: css.size,
            font: css.font.as_deref().map(Rc::from),
            align: css.align,
            line_height: css.line_height,
            ..Delta::default()
        }
    }

    /// The style inside the element.
    fn apply(&self, parent: &Style) -> Style {
        let mut style = parent.clone();
        // A size of nothing is inherited, as in a browser, but not past a class or an id: not
        // by an element that carries one, and not from an element that does.
        style.tiny = parent.tiny && !parent.tiny_hooked && !self.hooked;
        style.tiny_hooked = false;
        if self.cell {
            style.bold = false;
            style.italic = false;
            style.underline = false;
            style.strike = false;
            style.code = false;
            style.background = None;
        }
        style.bold = self.bold.unwrap_or(style.bold);
        style.italic = self.italic.unwrap_or(style.italic);
        style.underline = self.underline.unwrap_or(style.underline);
        style.strike = self.strike.unwrap_or(style.strike);
        style.code = self.code.unwrap_or(style.code);
        if let Some(color) = self.color {
            style.color = color;
        }
        if self.background.is_some() {
            style.background = self.background;
        }
        if let Some(align) = self.align {
            style.align = align;
        }
        if let Some(line_height) = self.line_height {
            style.line_height = line_height;
        }
        if self.font.is_some() {
            style.font = self.font.clone();
        }
        if let Some(link) = &self.link {
            style.link = link.clone();
        }
        if let Some(size) = self.size {
            // A share of a size that is nothing is nothing still.
            let resolved = if parent.tiny && matches!(size, Size::Scale(_)) { None } else { size.resolve(parent.size) };
            match resolved {
                Some(px) => {
                    style.size = Some(px);
                    style.tiny = false;
                }
                None => {
                    style.tiny = true;
                    style.tiny_hooked = self.hooked;
                }
            }
        }
        style
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Numbering {
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

/// A `<div>`, a `<p>` and the like.
#[derive(Debug, Default)]
struct BlockNode {
    /// A paragraph gap follows it.
    margin: bool,
    /// It has a left border: its content is shown as a quote.
    quote: bool,
    /// It has a bottom border: a rule follows it.
    rule: bool,
    /// How that rule looks.
    rule_line: Line,
    /// The room from the sides it adds for what is inside it, taken back when it ends.
    inset: [u8; 2],
    /// Its bottom margin in pixels, when it gives one: the gap that follows it.
    gap: u8,
    /// An Outlook list paragraph.
    mso: bool,
    /// It has a background or a frame: its blocks are collected apart.
    boxed: Option<BoxBuild>,
}

#[derive(Debug)]
struct BoxBuild {
    background: Option<Rgb>,
    outline: bool,
    /// A `max-width` caps it (the whole room when none), and automatic margins centre it.
    width: Option<Length>,
    align: Align,
    padding: Option<Insets>,
    lines: [Line; 4],
    radius: u8,
    /// The room left and right of the box.
    margin: [u8; 2],
}

#[derive(Debug)]
struct TableBuild {
    rows: Vec<Row>,
    width: Option<Length>,
    background: Option<Rgb>,
    align: Align,
    /// `border="1"`.
    border: bool,
    /// A border on the table itself.
    outline: bool,
    lines: [Line; 4],
    radius: u8,
    margin: [u8; 2],
    /// Its bottom margin in pixels.
    gap_below: u8,
    /// `role="presentation"`.
    presentation: bool,
    /// It has `<th>`, `<thead>` or `<caption>`.
    header: bool,
    /// There was no `<table>`: a stray `<tr>` or `<td>` started it.
    implied: bool,
    /// Nested too deep to be a grid: its cells are not kept apart.
    flat: bool,
    /// At a cap: no more rows or cells are opened, the last cell takes what follows.
    full: bool,
    /// The row being read is the last one this table may have.
    last_row: bool,
    spacing: Option<u8>,
}

#[derive(Debug)]
struct RowBuild {
    cells: Vec<Cell>,
    background: Option<Rgb>,
    /// What its cells take when they don't say.
    valign: Option<VAlign>,
    /// At [`MAX_TABLE_COLUMNS`]: the last cell takes what follows.
    full: bool,
}

#[derive(Debug)]
struct CellBuild {
    header: bool,
    colspan: u16,
    rowspan: u16,
    background: Option<Rgb>,
    width: Option<Length>,
    border: Sides,
    lines: [Line; 4],
    padding: Option<Insets>,
    radius: u8,
    valign: Option<VAlign>,
}

/// What an open element is.
#[derive(Debug)]
enum Kind {
    /// `<b>`, `<a>`, `<font>`…: stays open across the end of a block, as in a browser.
    Format,
    /// `<span>` and the like.
    Inline,
    /// Outlook's written-out list marker (`mso-list:Ignore`): not shown, read for the marker.
    Marker,
    Block(BlockNode),
    List {
        /// The number of the next item of an ordered list.
        next: Option<u32>,
        numbering: Numbering,
        /// A paragraph gap follows the list (it has a bottom margin).
        margin: bool,
    },
    /// With the depth of its list.
    Item(u8),
    /// `margin`: a paragraph gap follows it.
    Quote {
        margin: bool,
    },
    Pre,
    Heading(u8),
    Table(Box<TableBuild>),
    Row(RowBuild),
    Cell(CellBuild),
}

/// An open element.
#[derive(Debug)]
struct Node {
    /// The element's name, with aliases folded (`strong` is `b`).
    name: &'static str,
    kind: Kind,
    delta: Delta,
    /// The style inside the element.
    style: Style,
    hidden: bool,
}

/// What the blocks of a frame are numbered against, saved while an inner frame is being read.
#[derive(Debug, Default)]
struct Context {
    quote: usize,
    lists: usize,
    pre: usize,
    heading: Option<u8>,
    item: Option<BlockKind>,
    mso_item: bool,
    inset: [u16; 2],
}

/// Where blocks are collected: the message, a table cell, or a block with its own background.
#[derive(Debug, Default)]
struct Frame {
    blocks: Vec<Block>,
    /// Blank lines waiting for the next block.
    space: u8,
    /// The pixels of margin waiting for the next block.
    gap: u8,
    outer: Context,
}

struct Builder {
    root: Frame,
    /// The frames inside the message, innermost last.
    frames: Vec<Frame>,
    /// Blocks made so far, in every frame.
    count: usize,
    /// Table cells made so far.
    cells: usize,
    /// The block being built; empty when none is.
    spans: Vec<Span>,
    kind: BlockKind,
    align: Align,
    line_height: Option<LineHeight>,
    /// Room from the sides, left and right, that the open elements that are not boxes ask for
    /// (the margins and padding of a `<div>` that has no frame of its own), in pixels.
    inset: [u16; 2],
    /// The style may have changed since the last character: the next one may need a new span.
    fresh: bool,
    nodes: Vec<Node>,
    base: Style,
    /// Structural elements opened past [`MAX_DEPTH`], whose end tags are skipped.
    overflow: usize,
    /// The same for inline elements.
    inline_overflow: usize,
    /// Open elements that are hidden; while there is one, nothing is added.
    hidden: usize,
    /// Open tables that a stray `<tr>` or `<td>` started.
    implied: usize,
    quote: usize,
    lists: usize,
    pre: usize,
    heading: Option<u8>,
    /// The marker of a list item that has no text yet.
    item: Option<BlockKind>,
    /// The item is an Outlook list paragraph, whose marker is still to be read.
    mso_item: bool,
    /// Open written-out markers, and their text.
    capture: usize,
    marker: String,
    /// Right after `<pre>`: a first line break is not part of the text.
    pre_start: bool,
    /// Inside `<template>`, which is never shown.
    template: usize,
    /// The last thing added was a picture, and the line it is in goes on: what comes next in
    /// that line, before a line break or a block of its own, stands beside it.
    after_picture: bool,
    /// Whether the pictures written into the message are decoded (not when only text is wanted).
    decode: bool,
    /// What those have taken so far.
    budget: Budget,
}

impl Builder {
    fn new(decode: bool) -> Builder {
        Builder {
            root: Frame::default(),
            frames: Vec::new(),
            count: 0,
            cells: 0,
            spans: Vec::new(),
            kind: BlockKind::Paragraph,
            align: Align::Left,
            line_height: None,
            inset: [0, 0],
            fresh: true,
            nodes: Vec::new(),
            base: Style::default(),
            overflow: 0,
            inline_overflow: 0,
            hidden: 0,
            implied: 0,
            quote: 0,
            lists: 0,
            pre: 0,
            heading: None,
            item: None,
            mso_item: false,
            capture: 0,
            marker: String::new(),
            pre_start: false,
            template: 0,
            after_picture: false,
            decode,
            budget: Budget::default(),
        }
    }

    fn token(&mut self, token: Token<'_>) {
        match token {
            Token::Text(text) => {
                if self.template == 0 {
                    self.text(text);
                }
            }
            Token::Open(tag) => {
                self.pre_start = false;
                if tag.name == "template" {
                    self.template = self.template.saturating_add(1);
                } else if self.template == 0 {
                    self.open(&tag);
                }
            }
            Token::Close(name) => {
                self.pre_start = false;
                if name == "template" {
                    self.template = self.template.saturating_sub(1);
                } else if self.template == 0 {
                    self.end(&name);
                }
            }
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush();
        while let Some(node) = self.nodes.pop() {
            self.leave(node);
        }
        self.flush();
        // Every frame was closed with its element; this only guards against a slip above.
        while let Some(frame) = self.frames.pop() {
            self.root.blocks.extend(frame.blocks);
        }
        self.root.blocks
    }

    fn frame(&mut self) -> &mut Frame {
        match self.frames.last_mut() {
            Some(frame) => frame,
            None => &mut self.root,
        }
    }

    fn style(&self) -> &Style {
        match self.nodes.last() {
            Some(node) => &node.style,
            None => &self.base,
        }
    }

    // ---- text ----

    fn text(&mut self, raw: &str) {
        if self.hidden > 0 {
            if self.capture > 0 && self.marker.len() < MAX_MARKER {
                let room = MAX_MARKER - self.marker.len();
                self.marker.extend(unescape(truncate(raw, 256)).chars().take(room));
            }
            return;
        }
        if self.style().tiny {
            return;
        }
        let text = unescape(raw);
        if self.pre > 0 {
            let mut chars = text.chars().peekable();
            while let Some(c) = chars.next() {
                let c = match c {
                    '\r' if chars.peek() == Some(&'\n') => continue,
                    '\r' => '\n',
                    c => c,
                };
                if std::mem::take(&mut self.pre_start) && c == '\n' {
                    continue;
                }
                if c == '\n' || c == '\t' || !c.is_control() {
                    self.push(c);
                }
            }
            return;
        }
        for c in text.chars() {
            if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C') {
                self.space();
            } else if !c.is_control() {
                self.push(c);
            }
        }
    }

    /// Collapsible white space: at most one space, and none at the start of a block or line.
    fn space(&mut self) {
        if self.spans.last().is_some_and(|last| !last.text.ends_with([' ', '\n'])) {
            self.append(' ');
        }
    }

    /// One visible character of the current block.
    fn push(&mut self, c: char) {
        if self.spans.is_empty() {
            self.settle();
            self.kind = if self.pre > 0 {
                BlockKind::Preformatted
            } else if let Some(item) = self.item.take() {
                item
            } else if let Some(level) = self.heading {
                BlockKind::Heading(level)
            } else if let Some(depth) = self.item_depth() {
                // A further paragraph of a list item stays under it.
                BlockKind::ListItem { marker: Marker::None, depth }
            } else {
                BlockKind::Paragraph
            };
            self.align = self.style().align;
            self.line_height = self.style().line_height;
            self.fresh = true;
        }
        self.append(c);
    }

    /// Add a character in the current style, to the last span when it has the same style.
    fn append(&mut self, c: char) {
        let style = match self.nodes.last() {
            Some(node) => &node.style,
            None => &self.base,
        };
        match self.spans.last_mut() {
            Some(last) if !self.fresh || style.matches(last) => last.text.push(c),
            _ => self.spans.push(style.span(c.to_string())),
        }
        self.fresh = false;
    }

    fn line_break(&mut self) {
        // A line of text too small to see is as high as nothing.
        if self.hidden > 0 || self.style().tiny {
            return;
        }
        if self.pre > 0 {
            self.push('\n');
            return;
        }
        // A break with no text before it is an empty line; right after a picture, it only ends
        // the picture's line.
        if self.spans.is_empty() {
            if !std::mem::take(&mut self.after_picture) {
                self.add_space();
            }
            return;
        }
        if let Some(last) = self.spans.last_mut() {
            let kept = last.text.trim_end_matches(' ').len();
            last.text.truncate(kept);
            if last.text.is_empty() {
                self.spans.pop();
                self.fresh = true;
            }
        }
        if !self.spans.is_empty() {
            self.append('\n');
        }
    }

    /// Drop white space at the end of the block being built; the number of line breaks dropped.
    fn trim_tail(&mut self, pre: bool) -> usize {
        let mut breaks = 0;
        while let Some(last) = self.spans.last_mut() {
            let kept = if pre { last.text.trim_end_matches('\n').len() } else { last.text.trim_end_matches([' ', '\n']).len() };
            breaks += last.text.get(kept..).map_or(0, |tail| tail.matches('\n').count());
            last.text.truncate(kept);
            if !last.text.is_empty() {
                break;
            }
            self.spans.pop();
            self.fresh = true;
        }
        breaks
    }

    // ---- blocks ----

    /// At least `lines` blank lines above the next block.
    fn want_space(&mut self, lines: u8) {
        if self.hidden == 0 {
            let frame = self.frame();
            frame.space = frame.space.max(lines.min(MAX_SPACE));
        }
    }

    /// At least `px` pixels of margin above the next block.
    fn want_gap(&mut self, px: u8) {
        if self.hidden == 0 && px > 0 {
            let frame = self.frame();
            frame.gap = frame.gap.max(px.min(MAX_BOX_PX));
        }
    }

    /// One more blank line above the next block.
    fn add_space(&mut self) {
        if self.hidden == 0 {
            let frame = self.frame();
            frame.space = frame.space.saturating_add(1).min(MAX_SPACE);
        }
    }

    /// End the block being built.
    fn flush(&mut self) {
        self.fresh = true;
        if self.spans.is_empty() {
            // Nothing in the line since the picture: whatever ends the block ends its line too.
            self.after_picture = false;
            return;
        }
        let pre = self.kind == BlockKind::Preformatted;
        let breaks = self.trim_tail(pre);
        let spans = std::mem::take(&mut self.spans);
        let kind = std::mem::replace(&mut self.kind, BlockKind::Paragraph);
        if spans.iter().all(|s| s.text.chars().all(is_blank)) {
            // A paragraph that holds only a no-break space is how mail programs write an empty
            // line; but the spaces between two pictures in a line only part them.
            if !self.after_picture {
                self.add_space();
            }
            return;
        }
        let beside = std::mem::take(&mut self.after_picture);
        let mut spans = spans;
        if beside {
            // The spaces between the picture and the text are the gap between them, drawn as such.
            // One pass: drop the blank spans in front in a single drain, then trim the first
            // span that has text (at a char boundary: `trim_start_matches` ends on one).
            let lead = spans.iter().position(|s| !s.text.chars().all(is_blank)).unwrap_or(spans.len());
            spans.drain(..lead);
            if let Some(first) = spans.first_mut() {
                let trimmed = first.text.trim_start_matches(is_blank);
                if trimmed.len() != first.text.len() {
                    first.text = trimmed.to_string();
                }
            }
        }
        let block = Block { quote: self.quote_depth(), align: self.align, beside, line_height: self.line_height, ..Block::new(kind, spans) };
        self.emit(block, false);
        // `text<br><br>` ends with an empty line.
        if !pre && breaks >= 2 {
            self.add_space();
        }
    }

    /// Add a block to the frame being read, under the blank lines that were waiting. Tables are
    /// `forced`: they hold blocks that were counted already, and must not be lost at the cap.
    fn emit(&mut self, mut block: Block, forced: bool) {
        if self.count >= MAX_BLOCKS && !forced {
            return;
        }
        self.count = self.count.saturating_add(1);
        let frame = self.frame();
        block.space = if frame.blocks.is_empty() { 0 } else { frame.space.min(MAX_SPACE) };
        block.gap = if frame.blocks.is_empty() { 0 } else { frame.gap };
        frame.space = 0;
        frame.gap = 0;
        frame.blocks.push(block);
        let inset = self.inset_now();
        if let Some(block) = self.frame().blocks.last_mut() {
            block.inset = [block.inset[0].saturating_add(inset[0]), block.inset[1].saturating_add(inset[1])];
        }
    }

    /// Add the blocks of a table that only wrapped them to the frame being read.
    ///
    /// The room a cell had inside (`padding`) is kept as far as a flow of blocks can: above the
    /// first block, below the last, and to the left and right of all of them.
    fn splice(&mut self, blocks: Vec<Block>, padding: Option<Insets>) {
        let quote = self.quote_depth();
        let pad = padding.unwrap_or_default();
        let inset = self.inset_now();
        let frame = self.frame();
        for (i, mut block) in blocks.into_iter().enumerate() {
            if i == 0 {
                block.space = if frame.blocks.is_empty() { 0 } else { block.space.max(frame.space).min(MAX_SPACE) };
                block.gap = if frame.blocks.is_empty() { 0 } else { block.gap.max(frame.gap).max(pad.top) };
                frame.space = 0;
                frame.gap = 0;
            }
            block.quote = block.quote.saturating_add(quote);
            block.inset =
                [block.inset[0].saturating_add(inset[0]).saturating_add(pad.left), block.inset[1].saturating_add(inset[1]).saturating_add(pad.right)];
            frame.blocks.push(block);
        }
        self.want_gap(pad.bottom);
    }

    /// The room blocks made now stand in from the sides, from the elements that are open.
    fn inset_now(&self) -> [u8; 2] {
        [u8::try_from(self.inset[0]).unwrap_or(u8::MAX), u8::try_from(self.inset[1]).unwrap_or(u8::MAX)]
    }

    fn quote_depth(&self) -> u8 {
        u8::try_from(self.quote).unwrap_or(u8::MAX)
    }

    fn rule(&mut self, line: Line) {
        if self.hidden > 0 {
            return;
        }
        self.flush();
        self.settle();
        let block = Block { quote: self.quote_depth(), line, ..Block::new(BlockKind::Rule, Vec::new()) };
        self.emit(block, false);
    }

    fn image(&mut self, tag: &Tag<'_>) {
        if self.hidden > 0 {
            return;
        }
        let css = css_of(tag);
        let px = |name: &str| tag.attr(name).and_then(|v| style::attr_px(&v));
        let (width, height) = (px("width"), px("height"));
        // A hidden or one-pixel picture is a tracker, not something to show.
        let tiny = |v: Option<f32>| v.is_some_and(|v| v <= 2.0);
        if css.hidden || tag.has("hidden") || tiny(width) || tiny(height) || tiny(css.width) || tiny(css.height) {
            return;
        }
        let alt = tag.attr("alt").unwrap_or_default();
        let alt: String = alt.split_whitespace().collect::<Vec<_>>().join(" ").chars().filter(|c| !c.is_control()).collect();
        let raw = tag.attr("src");
        let picture = match raw.as_deref().filter(|src| data::is_data(src)) {
            Some(written) => Some(data::picture(written, self.decode, &mut self.budget)),
            None => picture_url(raw).map(|src| (src, None)),
        };
        match picture {
            Some((src, data)) => {
                // Text in the line before it, or a picture with nothing but spaces since: the
                // picture stands in that line.
                let blank = self.spans.iter().all(|s| s.text.chars().all(is_blank));
                let beside = !blank || self.after_picture;
                self.flush();
                self.settle();
                let style = self.style().clone();
                let spans = if alt.is_empty() { Vec::new() } else { vec![style.span(alt)] };
                // The size the attributes give; where they give none, the one the style gives in
                // pixels (`width:100%` and `height:auto` say nothing here: the picture's own shape does).
                let size = |v: Option<f32>| v.map(|v| v.round().clamp(1.0, MAX_IMAGE_PX) as u16);
                let (width, height) = (size(width.or(css.width)), size(height.or(css.height)));
                let kind = BlockKind::Image { src, width, height, link: style.link.as_deref().map(str::to_string), data };
                let count = self.frame().blocks.len();
                let block = Block { quote: self.quote_depth(), align: style.align, beside, ..Block::new(kind, spans) };
                self.emit(block, false);
                // (Only when it was added: past the cap there is no line to go on.)
                self.after_picture = self.frame().blocks.len() > count;
            }
            // No picture to load: its description is all there is, where text can be read.
            None if self.style().tiny => {}
            None => {
                for c in alt.chars() {
                    if c == ' ' { self.space() } else { self.push(c) }
                }
            }
        }
    }

    // ---- the stack of open elements ----

    /// Open an element; `false` when the stack is full.
    fn push_node(&mut self, name: &'static str, kind: Kind, delta: Delta, hidden: bool) -> bool {
        if self.nodes.len() >= MAX_DEPTH {
            return false;
        }
        let style = delta.apply(self.style());
        if hidden {
            self.hidden = self.hidden.saturating_add(1);
        }
        self.nodes.push(Node { name, kind, delta, style, hidden });
        self.fresh = true;
        true
    }

    /// Work out again the styles of the nodes from `index` up, after the stack changed below them.
    fn restyle(&mut self, index: usize) {
        for i in index..self.nodes.len() {
            let parent = match i.checked_sub(1).and_then(|p| self.nodes.get(p)) {
                Some(parent) => parent.style.clone(),
                None => self.base.clone(),
            };
            if let Some(node) = self.nodes.get_mut(i) {
                node.style = node.delta.apply(&parent);
            }
        }
        self.fresh = true;
    }

    /// Close the node at `index` and what is open inside it. Formatting elements opened inside
    /// stay open, as in a browser (`<p><b>a</p><p>b</b>` is bold twice), unless `all` is set (the
    /// end of a table cell ends everything in it).
    fn close_at(&mut self, index: usize, all: bool) {
        let mut kept = Vec::new();
        while self.nodes.len() > index {
            let Some(node) = self.nodes.pop() else { break };
            let target = self.nodes.len() == index;
            if !all && !target && !node.hidden && matches!(node.kind, Kind::Format) {
                kept.push(node);
            } else {
                self.leave(node);
            }
        }
        while let Some(node) = kept.pop() {
            self.nodes.push(node);
        }
        self.restyle(index);
    }

    /// The nearest open node that `wanted` accepts, looking no further out than one `barrier` accepts.
    fn find(&self, wanted: impl Fn(&Node) -> bool, barrier: impl Fn(&Kind) -> bool) -> Option<usize> {
        for (i, node) in self.nodes.iter().enumerate().rev() {
            if wanted(node) {
                return Some(i);
            }
            if barrier(&node.kind) {
                return None;
            }
        }
        None
    }

    /// The nearest open table.
    fn table_index(&self) -> Option<usize> {
        self.nodes.iter().rposition(|n| matches!(n.kind, Kind::Table(_)))
    }

    fn table_at(&mut self, index: usize) -> Option<&mut TableBuild> {
        match self.nodes.get_mut(index).map(|n| &mut n.kind) {
            Some(Kind::Table(table)) => Some(table),
            _ => None,
        }
    }

    fn row_at(&mut self, index: usize) -> Option<&mut RowBuild> {
        match self.nodes.get_mut(index).map(|n| &mut n.kind) {
            Some(Kind::Row(row)) => Some(row),
            _ => None,
        }
    }

    /// The first node above `index` that `wanted` accepts.
    fn above(&self, index: usize, wanted: impl Fn(&Kind) -> bool) -> Option<usize> {
        self.nodes.iter().enumerate().skip(index + 1).find(|(_, n)| wanted(&n.kind)).map(|(i, _)| i)
    }

    /// The depth of the list item being read, inside the frame being read.
    fn item_depth(&self) -> Option<u8> {
        for node in self.nodes.iter().rev() {
            match &node.kind {
                Kind::Item(depth) => return Some(*depth),
                Kind::Cell(_) => return None,
                Kind::Block(block) if block.boxed.is_some() => return None,
                _ => {}
            }
        }
        None
    }

    /// Whether something open has a background of its own.
    fn tinted(&self) -> bool {
        self.nodes.iter().any(|n| match &n.kind {
            Kind::Block(block) => block.boxed.as_ref().is_some_and(|b| b.background.is_some()),
            Kind::Cell(cell) => cell.background.is_some(),
            Kind::Row(row) => row.background.is_some(),
            Kind::Table(table) => table.background.is_some(),
            _ => false,
        })
    }

    /// The nearest background something open has: what a block that names none is painted on.
    fn backdrop(&self) -> Option<Rgb> {
        self.nodes.iter().rev().find_map(|n| match &n.kind {
            Kind::Block(block) => block.boxed.as_ref().and_then(|b| b.background),
            Kind::Cell(cell) => cell.background,
            Kind::Row(row) => row.background,
            Kind::Table(table) => table.background,
            _ => None,
        })
    }

    /// Text or a picture outside every cell of a table that only a stray `<tr>` or `<td>` started
    /// ends that table: it comes after it, not before.
    fn settle(&mut self) {
        if self.implied == 0 {
            return;
        }
        let mut target = None;
        for (i, node) in self.nodes.iter().enumerate().rev() {
            match &node.kind {
                Kind::Cell(_) => return,
                Kind::Block(block) if block.boxed.is_some() => return,
                Kind::Table(table) => {
                    if table.implied {
                        target = Some(i);
                    }
                    break;
                }
                _ => {}
            }
        }
        if let Some(index) = target {
            self.close_at(index, true);
        }
    }

    /// Start collecting blocks apart, for a cell or a block with a background.
    fn enter_frame(&mut self) {
        self.flush();
        let outer = Context {
            quote: std::mem::take(&mut self.quote),
            lists: std::mem::take(&mut self.lists),
            pre: std::mem::take(&mut self.pre),
            heading: self.heading.take(),
            item: self.item.take(),
            mso_item: std::mem::take(&mut self.mso_item),
            inset: std::mem::take(&mut self.inset),
        };
        self.frames.push(Frame { blocks: Vec::new(), space: 0, gap: 0, outer });
    }

    /// The blocks collected since [`Builder::enter_frame`].
    fn exit_frame(&mut self) -> Vec<Block> {
        self.flush();
        let Some(frame) = self.frames.pop() else { return Vec::new() };
        self.quote = frame.outer.quote;
        self.lists = frame.outer.lists;
        self.pre = frame.outer.pre;
        self.heading = frame.outer.heading;
        self.item = frame.outer.item;
        self.mso_item = frame.outer.mso_item;
        self.inset = frame.outer.inset;
        frame.blocks
    }

    fn leave(&mut self, node: Node) {
        if node.hidden {
            self.hidden = self.hidden.saturating_sub(1);
        }
        match node.kind {
            Kind::Format | Kind::Inline => {}
            Kind::Marker => {
                self.capture = self.capture.saturating_sub(1);
                if self.capture == 0 {
                    if self.mso_item
                        && let Some(BlockKind::ListItem { marker, .. }) = &mut self.item
                    {
                        *marker = written_marker(&self.marker);
                    }
                    self.marker.clear();
                }
            }
            Kind::Block(block) => {
                self.flush();
                if block.mso {
                    self.item = None;
                    self.mso_item = false;
                }
                if let Some(boxed) = block.boxed {
                    let blocks = self.exit_frame();
                    self.end_box(boxed, blocks);
                }
                if block.quote {
                    self.quote = self.quote.saturating_sub(1);
                }
                if block.rule {
                    self.rule(block.rule_line);
                }
                self.inset = [self.inset[0].saturating_sub(u16::from(block.inset[0])), self.inset[1].saturating_sub(u16::from(block.inset[1]))];
                self.want_gap(block.gap);
                if block.margin {
                    self.want_space(1);
                }
            }
            Kind::List { margin, .. } => {
                self.flush();
                self.lists = self.lists.saturating_sub(1);
                if self.lists == 0 && margin {
                    self.want_space(1);
                }
            }
            Kind::Item(_) => {
                self.flush();
                self.item = None;
            }
            Kind::Quote { margin } => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
                if margin {
                    self.want_space(1);
                }
            }
            Kind::Pre => {
                self.flush();
                self.pre = self.pre.saturating_sub(1);
                self.want_space(1);
            }
            Kind::Heading(_) => {
                self.flush();
                self.heading = self.nodes.iter().rev().find_map(|n| match n.kind {
                    Kind::Heading(level) => Some(level),
                    _ => None,
                });
                self.want_space(1);
            }
            Kind::Table(table) => {
                self.flush();
                if table.implied {
                    self.implied = self.implied.saturating_sub(1);
                }
                let below = table.gap_below;
                self.end_table(*table);
                self.want_gap(below);
            }
            Kind::Row(row) => {
                self.flush();
                self.end_row(row);
            }
            Kind::Cell(cell) => {
                let blocks = self.exit_frame();
                self.end_cell(cell, blocks);
            }
        }
    }

    // ---- tables ----

    /// A block with a background or a frame of its own: one cell as wide as there is room.
    fn end_box(&mut self, boxed: BoxBuild, blocks: Vec<Block>) {
        if blocks.is_empty() {
            return;
        }
        let cell = Cell { blocks, background: boxed.background, padding: boxed.padding, radius: boxed.radius, ..Cell::default() };
        let table = Table {
            rows: vec![Row { cells: vec![cell] }],
            columns: 1,
            layout: true,
            outline: boxed.outline,
            width: boxed.width,
            lines: boxed.lines,
            radius: boxed.radius,
            ..Table::default()
        };
        let block = Block { quote: self.quote_depth(), align: boxed.align, inset: boxed.margin, ..Block::new(BlockKind::Table(table), Vec::new()) };
        self.emit(block, true);
    }

    fn end_cell(&mut self, cell: CellBuild, blocks: Vec<Block>) {
        let cell = Cell {
            blocks,
            header: cell.header,
            colspan: cell.colspan,
            rowspan: cell.rowspan,
            background: cell.background,
            width: cell.width,
            border: cell.border,
            lines: cell.lines,
            padding: cell.padding,
            radius: cell.radius,
            valign: cell.valign,
        };
        // Its row is the nearest one, this side of the table.
        for node in self.nodes.iter_mut().rev() {
            match &mut node.kind {
                Kind::Row(row) => {
                    row.cells.push(cell);
                    return;
                }
                Kind::Table(_) => break,
                _ => {}
            }
        }
        self.splice(cell.blocks, cell.padding);
    }

    fn end_row(&mut self, row: RowBuild) {
        let mut cells = row.cells;
        for cell in &mut cells {
            cell.background = cell.background.or(row.background);
        }
        if cells.is_empty() {
            return;
        }
        let table = self.nodes.iter_mut().rev().find_map(|n| match &mut n.kind {
            Kind::Table(table) => Some(table),
            _ => None,
        });
        if let Some(table) = table {
            table.rows.push(Row { cells });
            return;
        }
        for cell in cells {
            self.splice(cell.blocks, cell.padding);
        }
    }

    /// Turn what was read of a table into a block: a data table, a layout table, or, for a table
    /// that only wrapped a column of things, those things.
    fn end_table(&mut self, table: TableBuild) {
        if table.flat {
            return;
        }
        let mut rows = table.rows;
        let empty = |cell: &Cell| cell.blocks.is_empty() && cell.background.is_none() && !cell.border.any();
        // Rows and columns that only make room make none here.
        rows.retain(|r| !r.cells.iter().all(empty));
        let nested = rows.iter().flat_map(|r| &r.cells).any(|c| c.blocks.iter().any(|b| matches!(b.kind, BlockKind::Table(_))));
        let layout = if table.presentation {
            true
        } else if table.header || table.border {
            false
        } else {
            nested || !looks_like_data(&rows)
        };
        if layout {
            let width = rows.first().map_or(0, |r| r.cells.len());
            let grid = rows.iter().all(|r| r.cells.len() == width && r.cells.iter().all(|c| c.colspan == 1 && c.rowspan == 1));
            if grid {
                let mut used = vec![false; width];
                for row in &rows {
                    for (slot, cell) in used.iter_mut().zip(&row.cells) {
                        *slot |= !empty(cell);
                    }
                }
                for row in &mut rows {
                    let mut column = used.iter();
                    row.cells.retain(|_| column.next().copied().unwrap_or(true));
                }
            } else {
                for row in &mut rows {
                    row.cells.retain(|c| !empty(c));
                }
            }
            rows.retain(|r| !r.cells.is_empty());
        }
        if rows.is_empty() {
            return;
        }
        let columns =
            rows.iter().map(|r| r.cells.iter().map(|c| usize::from(c.colspan)).sum::<usize>()).max().unwrap_or(1).clamp(1, MAX_TABLE_COLUMNS);
        // A table that only wraps a column of things is those things. Unless the things are
        // pictures and nothing else, a row each: those are slices of one picture, and stay the
        // rows of a table, for whoever draws them to set them edge to edge.
        let framed = table.lines.iter().any(Line::is_set);
        let plain = table.background.is_none()
            && !table.outline
            && !framed
            && !table.border
            && table.margin == [0, 0]
            && !matches!(table.width, Some(Length::Px(_)));
        let picture = |cell: &Cell| !cell.blocks.is_empty() && cell.blocks.iter().all(|b| matches!(b.kind, BlockKind::Image { .. }));
        let slices = rows.len() >= 2 && rows.iter().all(|r| r.cells.iter().all(picture));
        if layout && plain && !slices && rows.iter().all(|r| r.cells.len() == 1 && r.cells.iter().all(|c| c.background.is_none() && !c.border.any()))
        {
            for (i, row) in rows.into_iter().enumerate() {
                for mut cell in row.cells {
                    if i > 0
                        && let Some(first) = cell.blocks.first_mut()
                    {
                        first.space = first.space.max(1);
                    }
                    self.splice(cell.blocks, cell.padding);
                }
            }
            return;
        }
        let kind = BlockKind::Table(Table {
            rows,
            columns: u16::try_from(columns).unwrap_or(u16::MAX),
            layout,
            border: table.border,
            outline: table.outline,
            width: table.width,
            background: table.background,
            lines: table.lines,
            radius: table.radius,
            spacing: table.spacing,
        });
        let block = Block { quote: self.quote_depth(), align: table.align, inset: table.margin, ..Block::new(kind, Vec::new()) };
        self.emit(block, true);
    }

    /// Open a table that a stray `<tr>` or `<td>` implies; its index, or `None` when the stack is full.
    fn imply_table(&mut self) -> Option<usize> {
        self.flush();
        let build = TableBuild {
            rows: Vec::new(),
            width: None,
            background: None,
            align: Align::Left,
            border: false,
            outline: false,
            lines: [Line::default(); 4],
            radius: 0,
            margin: [0, 0],
            gap_below: 0,
            presentation: false,
            header: false,
            implied: true,
            flat: self.frames.len() >= MAX_TABLE_DEPTH,
            full: false,
            last_row: false,
            spacing: None,
        };
        if !self.push_node("table", Kind::Table(Box::new(build)), Delta::default(), false) {
            return None;
        }
        self.implied = self.implied.saturating_add(1);
        Some(self.nodes.len().saturating_sub(1))
    }

    fn open_table(&mut self, tag: &Tag<'_>) {
        self.close_paragraph();
        self.flush();
        let css = css_of(tag);
        let border = tag.attr("border").is_some_and(|b| style::attr_px(&b).map_or(b.trim().is_empty(), |px| px > 0.0));
        let role = tag.attr("role").map(|r| r.trim().to_ascii_lowercase());
        let build = TableBuild {
            rows: Vec::new(),
            width: sized(&css, tag.attr("width").and_then(|w| style::attr_length(&w))),
            background: css.background.or_else(|| tag.attr("bgcolor").and_then(|c| style::attr_color(&c))).filter(|bg| Some(*bg) != self.backdrop()),
            align: tag.attr("align").and_then(|a| style::attr_align(&a)).or(css.centred().then_some(Align::Center)).unwrap_or(self.style().align),
            border,
            outline: css.framed(),
            lines: css.lines,
            radius: css.radius,
            margin: [css.margin_insets().left, css.margin_insets().right],
            gap_below: css.margin_insets().bottom,
            presentation: matches!(role.as_deref(), Some("presentation" | "none")),
            header: false,
            implied: false,
            flat: self.frames.len() >= MAX_TABLE_DEPTH,
            full: false,
            last_row: false,
            spacing: css.spacing.or_else(|| tag.attr("cellspacing").and_then(|v| style::spacing(&v))),
        };
        let hidden = css.hidden || tag.has("hidden");
        self.want_gap(css.margin_insets().top);
        if !self.push_node("table", Kind::Table(Box::new(build)), Delta { hooked: hooked(tag), ..Delta::of(&css) }, hidden) {
            self.overflow = self.overflow.saturating_add(1);
        }
    }

    fn open_row(&mut self, tag: &Tag<'_>) {
        if self.overflow > 0 {
            self.overflow = self.overflow.saturating_add(1);
            self.flush();
            return;
        }
        let Some(table) = self.table_index().or_else(|| self.imply_table()) else {
            self.overflow = self.overflow.saturating_add(1);
            return;
        };
        if self.table_at(table).is_none_or(|t| t.flat || t.full) {
            self.flush();
            return;
        }
        if let Some(row) = self.above(table, |k| matches!(k, Kind::Row(_))) {
            self.close_at(row, true);
        }
        self.flush();
        if let Some(t) = self.table_at(table) {
            t.last_row = t.rows.len() + 1 >= MAX_TABLE_ROWS;
        }
        let css = css_of(tag);
        let mut delta = Delta { hooked: hooked(tag), ..Delta::of(&css) };
        delta.align = delta.align.or_else(|| tag.attr("align").and_then(|a| style::attr_align(&a)));
        let background = css.background.or_else(|| tag.attr("bgcolor").and_then(|c| style::attr_color(&c))).filter(|bg| Some(*bg) != self.backdrop());
        let valign = css.valign.or_else(|| tag.attr("valign").and_then(|v| style::valign(&v)));
        let row = RowBuild { cells: Vec::new(), background, valign, full: false };
        let hidden = css.hidden || tag.has("hidden");
        if !self.push_node("tr", Kind::Row(row), delta, hidden) {
            self.overflow = self.overflow.saturating_add(1);
        }
    }

    fn open_cell(&mut self, tag: &Tag<'_>, header: bool) {
        if self.overflow > 0 {
            self.overflow = self.overflow.saturating_add(1);
            self.flush();
            return;
        }
        let Some(table) = self.table_index().or_else(|| self.imply_table()) else {
            self.overflow = self.overflow.saturating_add(1);
            return;
        };
        if self.table_at(table).is_none_or(|t| t.flat || t.full) {
            self.flush();
            return;
        }
        let row = match self.above(table, |k| matches!(k, Kind::Row(_))) {
            Some(row) => row,
            None => {
                // A cell without a row gets one, as in a browser.
                self.flush();
                if let Some(t) = self.table_at(table) {
                    t.last_row = t.rows.len() + 1 >= MAX_TABLE_ROWS;
                }
                if !self.push_node(
                    "tr",
                    Kind::Row(RowBuild { cells: Vec::new(), background: None, valign: None, full: false }),
                    Delta::default(),
                    false,
                ) {
                    self.overflow = self.overflow.saturating_add(1);
                    return;
                }
                self.nodes.len().saturating_sub(1)
            }
        };
        if self.row_at(row).is_none_or(|r| r.full) {
            self.flush();
            return;
        }
        if self.cells >= MAX_CELLS {
            if let Some(t) = self.table_at(table) {
                t.full = true;
            }
            self.flush();
            return;
        }
        if let Some(cell) = self.above(row, |k| matches!(k, Kind::Cell(_))) {
            self.close_at(cell, true);
        }
        self.flush();
        let css = css_of(tag);
        let span = |name: &str, max: u16| tag.attr(name).and_then(|v| v.trim().parse::<u32>().ok()).map_or(1, |n| n.clamp(1, u32::from(max)) as u16);
        let build = CellBuild {
            header,
            colspan: span("colspan", MAX_TABLE_COLUMNS as u16),
            rowspan: span("rowspan", MAX_ROW_SPAN),
            background: css.background.or_else(|| tag.attr("bgcolor").and_then(|c| style::attr_color(&c))).filter(|bg| Some(*bg) != self.backdrop()),
            width: sized(&css, tag.attr("width").and_then(|w| style::attr_length(&w))),
            border: {
                let [top, right, bottom, left] = css.borders;
                Sides { top, right, bottom, left }
            },
            lines: css.lines,
            padding: css.padding_insets(),
            radius: css.radius,
            valign: css.valign.or_else(|| tag.attr("valign").and_then(|v| style::valign(&v))).or_else(|| self.row_at(row).and_then(|r| r.valign)),
        };
        let mut delta = Delta { hooked: hooked(tag), ..Delta::of(&css) };
        delta.cell = true;
        delta.bold = delta.bold.or(header.then_some(true));
        delta.align = delta.align.or_else(|| tag.attr("align").and_then(|a| style::attr_align(&a)));
        let hidden = css.hidden || tag.has("hidden");
        if !self.push_node(if header { "th" } else { "td" }, Kind::Cell(build), delta, hidden) {
            self.overflow = self.overflow.saturating_add(1);
            return;
        }
        self.enter_frame();
        self.cells = self.cells.saturating_add(1);
        if let Some(r) = self.row_at(row) {
            r.full = r.cells.len() + 1 >= MAX_TABLE_COLUMNS;
        }
        if let Some(t) = self.table_at(table) {
            t.header |= header;
            t.full |= t.last_row;
        }
    }

    /// `<thead>`, `<caption>` and the like say the table holds data, when they belong to it.
    fn table_part(&mut self, data: bool) {
        self.flush();
        if !data {
            return;
        }
        for node in self.nodes.iter_mut().rev() {
            match &mut node.kind {
                Kind::Cell(_) => return,
                Kind::Table(table) => {
                    table.header = true;
                    return;
                }
                _ => {}
            }
        }
    }

    fn end_cell_tag(&mut self) {
        if self.overflow > 0 {
            self.overflow -= 1;
            return;
        }
        let Some(cell) = self.find(|n| matches!(n.kind, Kind::Cell(_)), |k| matches!(k, Kind::Table(_))) else { return };
        // A cell that takes what is left of a capped row or table stays open.
        let stays = self.nodes.iter().take(cell).rev().find_map(|n| match &n.kind {
            Kind::Row(row) => Some(row.full),
            _ => None,
        }) == Some(true)
            || self.table_index().and_then(|t| self.nodes.get(t)).is_some_and(|n| matches!(&n.kind, Kind::Table(t) if t.full));
        if stays {
            self.flush();
        } else {
            self.close_at(cell, true);
        }
    }

    fn end_row_tag(&mut self) {
        if self.overflow > 0 {
            self.overflow -= 1;
            return;
        }
        let Some(row) = self.find(|n| matches!(n.kind, Kind::Row(_)), |k| matches!(k, Kind::Table(_))) else { return };
        if self.table_index().and_then(|t| self.nodes.get(t)).is_some_and(|n| matches!(&n.kind, Kind::Table(t) if t.full)) {
            self.flush();
        } else {
            self.close_at(row, true);
        }
    }

    fn end_table_tag(&mut self) {
        if self.overflow > 0 {
            self.overflow -= 1;
            return;
        }
        if let Some(table) = self.table_index() {
            self.close_at(table, true);
        }
    }

    // ---- elements ----

    /// An inline element: a formatting element (`format`) or a `<span>`.
    fn open_inline(&mut self, name: &'static str, format: bool, tag: &Tag<'_>, mut delta: Delta) {
        let css = css_of(tag);
        let own = Delta::of(&css);
        delta.bold = own.bold.or(delta.bold);
        delta.italic = own.italic.or(delta.italic);
        delta.underline = own.underline.or(delta.underline);
        delta.strike = own.strike.or(delta.strike);
        delta.color = own.color.or(delta.color);
        delta.size = own.size.or(delta.size);
        delta.font = own.font.or(delta.font);
        delta.background = css.background.or(delta.background);
        delta.hooked = hooked(tag);
        let marker = css.mso_list == Some(MsoList::Ignore);
        let kind = if marker {
            Kind::Marker
        } else if format {
            Kind::Format
        } else {
            Kind::Inline
        };
        if !self.push_node(name, kind, delta, marker || css.hidden || tag.has("hidden")) {
            self.inline_overflow = self.inline_overflow.saturating_add(1);
        } else if marker {
            self.capture = self.capture.saturating_add(1);
        }
    }

    fn end_format(&mut self, name: &str) {
        if self.inline_overflow > 0 {
            self.inline_overflow -= 1;
            return;
        }
        let Some(index) =
            self.find(|n| n.name == name && matches!(n.kind, Kind::Format), |k| matches!(k, Kind::Cell(_) | Kind::Row(_) | Kind::Table(_)))
        else {
            return;
        };
        let node = self.nodes.remove(index);
        self.leave(node);
        self.restyle(index);
    }

    fn end_inline(&mut self, name: &str) {
        if self.inline_overflow > 0 {
            self.inline_overflow -= 1;
            return;
        }
        // As in a browser, the end tag is ignored when a block was opened inside the element.
        let mut target = None;
        for (i, node) in self.nodes.iter().enumerate().rev() {
            match &node.kind {
                Kind::Inline | Kind::Marker if node.name == name => {
                    target = Some(i);
                    break;
                }
                Kind::Inline | Kind::Marker | Kind::Format => {}
                _ => break,
            }
        }
        if let Some(index) = target {
            self.close_at(index, false);
        }
    }

    /// End an open `<p>`: every block element does, as in a browser.
    fn close_paragraph(&mut self) {
        let wanted = |n: &Node| n.name == "p" && matches!(n.kind, Kind::Block(_));
        if let Some(index) = self.find(wanted, |k| matches!(k, Kind::Cell(_) | Kind::Row(_) | Kind::Table(_))) {
            self.close_at(index, false);
        }
    }

    /// A `<div>`, a `<p>`, `<body>` and the like: a new line, and a style of its own.
    fn open_block(&mut self, name: &'static str, tag: &Tag<'_>) {
        self.close_paragraph();
        self.flush();
        let css = css_of(tag);
        let mut delta = Delta { hooked: hooked(tag), ..Delta::of(&css) };
        if name == "center" {
            delta.align = delta.align.or(Some(Align::Center));
        }
        delta.align = delta.align.or_else(|| tag.attr("align").and_then(|a| style::attr_align(&a)));
        let mut background = css.background;
        if name == "body" {
            delta.color = delta.color.or_else(|| tag.attr("text").and_then(|c| style::attr_color(&c)).map(Some));
            background = background.or_else(|| tag.attr("bgcolor").and_then(|c| style::attr_color(&c)));
        }
        if css.hidden || tag.has("hidden") {
            if !self.push_node(name, Kind::Block(BlockNode::default()), delta, true) {
                self.overflow = self.overflow.saturating_add(1);
            }
            return;
        }
        // Outlook's own paragraphs have no margin (its style sheet says so, and nothing is read
        // from style sheets); any other `<p>` has one unless it says it has none.
        let outlook = tag.attr("class").is_some_and(|c| c.split_whitespace().any(|c| c.starts_with("Mso")));
        let paragraph = name == "p" && !outlook;
        let margin_above = css.margin_top_zero.map_or(paragraph, |zero| !zero);
        let margin_below = css.margin_bottom_zero.map_or(paragraph, |zero| !zero);
        // White on what is white anyway is no background, and neither is the colour all around.
        let background = background.filter(|bg| !(near_white(*bg) && !self.tinted()) && Some(*bg) != self.backdrop());
        let outline = css.framed();
        let [top, _, bottom, left] = css.borders;
        // (Only a `max-width` makes a block a card; automatic margins alone have nothing to centre.)
        let capped = css.max_width.filter(|_| !paragraph && name != "body");
        let boxed = (background.is_some() || outline || capped.is_some()) && self.frames.len() < MAX_TABLE_DEPTH;
        let padding = css.padding_insets().unwrap_or_default();
        let margins = css.margin_insets();
        let quote = left && !outline;
        // A box keeps its padding inside it; any other block has it as room round what it holds.
        // (A quote has its own indent, and an Outlook list paragraph its level's.)
        let mso = matches!(css.mso_list, Some(MsoList::Level(_)));
        let plain = !boxed && !mso;
        let room = |margin: u8, padding: u8, padded: bool| margin.saturating_add(if padded { padding } else { 0 });
        let node = BlockNode {
            margin: margin_below,
            gap: room(margins.bottom, padding.bottom, !boxed),
            quote,
            rule: bottom && !outline,
            rule_line: css.lines[2],
            inset: if plain { [room(margins.left, padding.left, !quote), room(margins.right, padding.right, true)] } else { [0, 0] },
            mso,
            boxed: boxed.then_some(BoxBuild {
                background,
                outline,
                width: capped.or(Some(Length::Percent(100))),
                align: if css.centred() && capped.is_some() { Align::Center } else { Align::Left },
                padding: css.padding_insets(),
                lines: if outline { css.lines } else { [Line::default(); 4] },
                radius: if background.is_some() || outline { css.radius } else { 0 },
                margin: [margins.left, margins.right],
            }),
        };
        if margin_above {
            self.want_space(1);
        }
        self.want_gap(room(margins.top, padding.top, !boxed));
        if top && !outline {
            self.rule(css.lines[0]);
        }
        let (quote, mso, inset) = (node.quote, node.mso, node.inset);
        if !self.push_node(name, Kind::Block(node), delta, false) {
            self.overflow = self.overflow.saturating_add(1);
            return;
        }
        if quote {
            self.quote = self.quote.saturating_add(1);
        }
        self.inset = [self.inset[0].saturating_add(u16::from(inset[0])), self.inset[1].saturating_add(u16::from(inset[1]))];
        if boxed {
            self.enter_frame();
        }
        if mso && let Some(MsoList::Level(level)) = css.mso_list {
            self.item = Some(BlockKind::ListItem { marker: Marker::Bullet, depth: level.saturating_sub(1) });
            self.mso_item = true;
        }
    }

    fn end_block(&mut self, name: &str) {
        if self.overflow > 0 {
            self.overflow -= 1;
            self.flush();
            return;
        }
        let wanted = |n: &Node| n.name == name && matches!(n.kind, Kind::Block(_));
        match self.find(wanted, |k| matches!(k, Kind::Cell(_) | Kind::Row(_) | Kind::Table(_))) {
            Some(index) => self.close_at(index, false),
            None => {
                self.flush();
                // A stray `</p>` is an empty paragraph in a browser.
                if name == "p" {
                    self.want_space(1);
                }
            }
        }
    }

    /// An end tag of a list, a quote, a heading or `<pre>`.
    fn end_structure(&mut self, wanted: impl Fn(&Kind) -> bool, stop_at_list: bool) {
        if self.overflow > 0 {
            self.overflow -= 1;
            return;
        }
        let barrier = |k: &Kind| matches!(k, Kind::Cell(_) | Kind::Row(_) | Kind::Table(_)) || (stop_at_list && matches!(k, Kind::List { .. }));
        if let Some(index) = self.find(|n| wanted(&n.kind), barrier) {
            self.close_at(index, false);
        }
    }

    fn open_structure(&mut self, name: &'static str, kind: Kind, tag: &Tag<'_>) -> bool {
        let css = css_of(tag);
        let mut delta = Delta { hooked: hooked(tag), ..Delta::of(&css) };
        delta.align = delta.align.or_else(|| tag.attr("align").and_then(|a| style::attr_align(&a)));
        // A heading is bold unless it says it is not.
        if matches!(kind, Kind::Heading(_)) {
            delta.bold = delta.bold.or(Some(true));
        }
        if self.push_node(name, kind, delta, css.hidden || tag.has("hidden")) {
            true
        } else {
            self.overflow = self.overflow.saturating_add(1);
            false
        }
    }

    fn open_item(&mut self, tag: &Tag<'_>) {
        if let Some(index) =
            self.find(|n| matches!(n.kind, Kind::Item(_)), |k| matches!(k, Kind::List { .. } | Kind::Cell(_) | Kind::Row(_) | Kind::Table(_)))
        {
            self.close_at(index, false);
        }
        self.close_paragraph();
        self.flush();
        let depth = u8::try_from(self.lists.saturating_sub(1)).unwrap_or(u8::MAX);
        let value = tag.attr("value").and_then(|v| v.trim().parse::<u32>().ok());
        let barrier = |k: &Kind| matches!(k, Kind::Cell(_) | Kind::Row(_) | Kind::Table(_));
        let list = self.find(|n| matches!(n.kind, Kind::List { .. }), barrier);
        let mut marker = Marker::Bullet;
        if let Some(index) = list
            && let Some(Node { kind: Kind::List { next: Some(next), numbering, .. }, .. }) = self.nodes.get_mut(index)
        {
            let number = value.unwrap_or(*next);
            *next = number.saturating_add(1);
            marker = numbered(*numbering, number);
        }
        if self.open_structure("li", Kind::Item(depth), tag) {
            self.item = Some(BlockKind::ListItem { marker, depth });
        }
    }

    fn open(&mut self, tag: &Tag<'_>) {
        let on = Some(true);
        match tag.name.as_str() {
            "b" | "strong" => self.open_inline("b", true, tag, Delta { bold: on, ..Delta::default() }),
            "i" | "em" => self.open_inline("i", true, tag, Delta { italic: on, ..Delta::default() }),
            "cite" | "var" | "dfn" => self.open_inline("cite", true, tag, Delta { italic: on, ..Delta::default() }),
            "u" | "ins" => self.open_inline("u", true, tag, Delta { underline: on, ..Delta::default() }),
            "s" | "strike" | "del" => self.open_inline("s", true, tag, Delta { strike: on, ..Delta::default() }),
            "code" | "tt" | "kbd" | "samp" => self.open_inline("code", true, tag, Delta { code: on, ..Delta::default() }),
            "small" => self.open_inline("small", true, tag, Delta { size: Some(Size::Scale(0.85)), ..Delta::default() }),
            "big" => self.open_inline("big", true, tag, Delta { size: Some(Size::Scale(1.2)), ..Delta::default() }),
            "sub" | "sup" => self.open_inline("sub", true, tag, Delta { size: Some(Size::Scale(0.8)), ..Delta::default() }),
            "mark" => self.open_inline("mark", true, tag, Delta { background: Some(Rgb(0xff, 0xff, 0x00)), ..Delta::default() }),
            "font" => {
                let delta = Delta {
                    color: tag.attr("color").and_then(|c| style::attr_color(&c)).map(Some),
                    size: tag.attr("size").and_then(|s| style::font_tag_size(&s)),
                    font: tag.attr("face").and_then(|f| style::family(&f)).map(|f| Rc::from(f.as_str())),
                    ..Delta::default()
                };
                self.open_inline("font", true, tag, delta);
            }
            "a" => {
                // A link doesn't nest in a link.
                if let Some(index) = self.find(|n| n.name == "a", |k| matches!(k, Kind::Cell(_) | Kind::Row(_) | Kind::Table(_))) {
                    let node = self.nodes.remove(index);
                    self.leave(node);
                    self.restyle(index);
                }
                let link: Option<Rc<str>> = safe_url(tag.attr("href")).map(Rc::from);
                // A link has the reader's link colour unless it names one itself.
                let color = link.is_some().then_some(None);
                self.open_inline("a", true, tag, Delta { link: Some(link), color, ..Delta::default() });
            }
            "span" | "label" | "abbr" | "acronym" | "q" | "time" | "bdi" | "bdo" | "data" | "nobr" => {
                self.open_inline(inline_name(&tag.name), false, tag, Delta::default());
            }
            "br" => self.line_break(),
            "hr" => {
                self.close_paragraph();
                let css = css_of(tag);
                if !css.hidden {
                    // (An `hr` is drawn by its own look, a border of its own makes it neither thicker nor coloured.)
                    self.rule(Line::default());
                }
            }
            "img" => self.image(tag),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = tag.name.as_bytes().get(1).map_or(1, |d| d.saturating_sub(b'0')).clamp(1, 6);
                self.close_paragraph();
                // A heading does not nest in a heading.
                if let Some(index) =
                    self.nodes.len().checked_sub(1).filter(|i| self.nodes.get(*i).is_some_and(|n| matches!(n.kind, Kind::Heading(_))))
                {
                    self.close_at(index, false);
                }
                self.flush();
                self.want_space(1);
                if self.open_structure("h", Kind::Heading(level), tag) {
                    self.heading = Some(level);
                }
            }
            "ul" | "ol" | "menu" | "dir" => {
                self.close_paragraph();
                self.flush();
                // A list has a margin above and below unless it says it has none.
                let css = css_of(tag);
                if self.lists == 0 && css.margin_top_zero != Some(true) {
                    self.want_space(1);
                }
                let margin = css.margin_bottom_zero != Some(true);
                let start = || tag.attr("start").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(1);
                let next = (tag.name == "ol").then(start);
                let numbering = match tag.attr("type").as_deref().map(str::trim) {
                    Some("a") => Numbering::LowerAlpha,
                    Some("A") => Numbering::UpperAlpha,
                    Some("i") => Numbering::LowerRoman,
                    Some("I") => Numbering::UpperRoman,
                    _ => Numbering::Decimal,
                };
                if self.open_structure("ul", Kind::List { next, numbering, margin }, tag) {
                    self.lists += 1;
                }
            }
            "li" => self.open_item(tag),
            "blockquote" => {
                self.close_paragraph();
                self.flush();
                let css = css_of(tag);
                if css.margin_top_zero != Some(true) {
                    self.want_space(1);
                }
                if self.open_structure("blockquote", Kind::Quote { margin: css.margin_bottom_zero != Some(true) }, tag) {
                    self.quote += 1;
                }
            }
            "pre" | "xmp" | "listing" => {
                self.close_paragraph();
                self.flush();
                self.want_space(1);
                if self.open_structure("pre", Kind::Pre, tag) {
                    self.pre += 1;
                }
                self.pre_start = true;
            }
            "table" => self.open_table(tag),
            "tr" => self.open_row(tag),
            "td" | "th" => self.open_cell(tag, tag.name == "th"),
            "caption" | "thead" | "tfoot" => self.table_part(true),
            "tbody" | "html" | "head" | "option" | "optgroup" => self.table_part(false),
            name => {
                if let Some(name) = block_name(name) {
                    self.open_block(name, tag);
                }
            }
        }
    }

    fn end(&mut self, name: &str) {
        match name {
            "b" | "strong" => self.end_format("b"),
            "i" | "em" => self.end_format("i"),
            "cite" | "var" | "dfn" => self.end_format("cite"),
            "u" | "ins" => self.end_format("u"),
            "s" | "strike" | "del" => self.end_format("s"),
            "code" | "tt" | "kbd" | "samp" => self.end_format("code"),
            "small" | "big" | "mark" | "font" | "a" => self.end_format(name),
            "sub" | "sup" => self.end_format("sub"),
            "span" | "label" | "abbr" | "acronym" | "q" | "time" | "bdi" | "bdo" | "data" | "nobr" => self.end_inline(name),
            // Browsers read `</br>` as `<br>`.
            "br" => self.line_break(),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self.end_structure(|k| matches!(k, Kind::Heading(_)), false),
            "ul" | "ol" | "menu" | "dir" => self.end_structure(|k| matches!(k, Kind::List { .. }), false),
            "li" => self.end_structure(|k| matches!(k, Kind::Item(_)), true),
            "blockquote" => self.end_structure(|k| matches!(k, Kind::Quote { .. }), false),
            "pre" | "xmp" | "listing" => self.end_structure(|k| matches!(k, Kind::Pre), false),
            "table" => self.end_table_tag(),
            "tr" => self.end_row_tag(),
            "td" | "th" => self.end_cell_tag(),
            // The message's own style lasts to its end, whatever follows `</body>`.
            "caption" | "thead" | "tbody" | "tfoot" | "html" | "head" | "body" | "option" | "optgroup" => self.flush(),
            name => {
                if block_name(name).is_some() {
                    self.end_block(name);
                }
            }
        }
    }
}

/// The whitelisted facts of a tag's `style` attribute.
/// The width a table or cell asks for: its style's, else its attribute's, held to its `max-width`.
fn sized(css: &Css, attr: Option<Length>) -> Option<Length> {
    use Length::{Percent, Px};
    match (css.layout_width.or(attr), css.max_width) {
        (width, None) => width,
        (Some(Px(w)), Some(Px(m))) => Some(Px(w.min(m))),
        (Some(Px(w)), Some(Percent(_))) => Some(Px(w)),
        (Some(Percent(w)), Some(Percent(m))) => Some(Percent(w.min(m))),
        (Some(Percent(100)) | None, Some(Px(m))) => Some(Px(m)),
        (Some(Percent(w)), Some(Px(_))) => Some(Percent(w)),
        (None, Some(Percent(m))) => Some(Percent(m)),
    }
}

fn css_of(tag: &Tag<'_>) -> Css {
    tag.attr("style").map(|s| Css::parse(&s)).unwrap_or_default()
}

/// Whether an element carries a class or an id ([`Delta::hooked`]).
fn hooked(tag: &Tag<'_>) -> bool {
    tag.has("class") || tag.has("id")
}

/// Whether a background is white, or as good as.
fn near_white(color: Rgb) -> bool {
    color.0 >= 0xF8 && color.1 >= 0xF8 && color.2 >= 0xF8
}

/// The address of a link or picture, when it may be used. As in a browser, tabs and line breaks
/// inside it are removed and the ends are trimmed first; the check is made on what is left, so
/// `java&#10;script:` is seen for what it is.
fn safe_url(raw: Option<String>) -> Option<String> {
    let url: String = raw?.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let url = url.trim();
    is_safe_url(url).then(|| url.to_string())
}

/// The address of a picture, when it may be used: [`safe_url`], and besides an address that
/// leaves its scheme out (`//host/picture.png`, which a web page reads as "the way this page
/// came") is read as `https://`. A message came no such way, and `https` is the one that tells
/// the networks in between the least. Only for pictures: a link goes where it says, or nowhere.
pub(crate) fn picture_url(raw: Option<String>) -> Option<String> {
    let url: String = raw?.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let url = url.trim();
    // Two slashes and then a host: not `///`, and not a path on some file server (`\\host\share`).
    let schemeless = url.strip_prefix("//").is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_alphanumeric()));
    let url = if schemeless { format!("https:{url}") } else { url.to_string() };
    is_safe_url(&url).then_some(url)
}

/// Whether the rows read like a grid of short texts: at least two rows with something in two
/// cells, nearly every row as wide as the widest, and nothing in a cell but a little text.
fn looks_like_data(rows: &[Row]) -> bool {
    let width = |row: &Row| row.cells.iter().map(|c| usize::from(c.colspan)).sum::<usize>();
    let columns = rows.iter().map(width).max().unwrap_or(0);
    let filled = rows.iter().filter(|r| r.cells.iter().filter(|c| !c.blocks.is_empty()).count() >= 2).count();
    if filled < 2 || columns < 2 {
        return false;
    }
    let regular = rows.iter().filter(|r| width(r) == columns).count();
    if regular.saturating_mul(5) < rows.len().saturating_mul(4) {
        return false;
    }
    rows.iter().flat_map(|r| &r.cells).all(|cell| {
        cell.blocks.len() <= 3
            && cell.blocks.iter().all(|b| matches!(b.kind, BlockKind::Paragraph))
            && cell.blocks.iter().flat_map(|b| &b.spans).map(|s| s.text.len()).sum::<usize>() <= DATA_CELL_TEXT
    })
}

/// The marker of item `n` of an ordered list.
fn numbered(numbering: Numbering, n: u32) -> Marker {
    let label = match numbering {
        Numbering::Decimal => None,
        Numbering::LowerAlpha => alphabetic(n),
        Numbering::UpperAlpha => alphabetic(n).map(|s| s.to_ascii_uppercase()),
        Numbering::LowerRoman => roman(n),
        Numbering::UpperRoman => roman(n).map(|s| s.to_ascii_uppercase()),
    };
    match label {
        Some(label) => Marker::Label(format!("{label}.")),
        None => Marker::Number(n),
    }
}

/// `a`, `b`, … `z`, `aa`, `ab`, …
fn alphabetic(mut n: u32) -> Option<String> {
    if n == 0 {
        return None;
    }
    let mut letters = Vec::new();
    while n > 0 {
        n -= 1;
        letters.push(char::from(b'a' + (n % 26) as u8));
        n /= 26;
    }
    Some(letters.iter().rev().collect())
}

/// `i`, `ii`, … up to 3999.
fn roman(mut n: u32) -> Option<String> {
    if !(1..=3999).contains(&n) {
        return None;
    }
    let mut out = String::new();
    for (value, numeral) in [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ] {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    Some(out)
}

/// The marker Outlook wrote in front of a list paragraph: `1.` and `a)` are kept, a bullet
/// character (whatever symbol font it was meant for) is a bullet.
fn written_marker(text: &str) -> Marker {
    let text: String = text.chars().filter(|c| !is_blank(*c)).collect();
    let body = text.strip_suffix(['.', ')']).unwrap_or("");
    if body.is_empty() || body.chars().count() > 6 || !body.chars().all(char::is_alphanumeric) {
        return Marker::Bullet;
    }
    match body.parse::<u32>() {
        Ok(n) if text.ends_with('.') => Marker::Number(n),
        _ => Marker::Label(text),
    }
}

/// The name an inline container is kept under.
fn inline_name(name: &str) -> &'static str {
    match name {
        "label" => "label",
        "abbr" => "abbr",
        "acronym" => "acronym",
        "q" => "q",
        "time" => "time",
        "bdi" => "bdi",
        "bdo" => "bdo",
        "data" => "data",
        "nobr" => "nobr",
        _ => "span",
    }
}

/// Elements that start on a new line and carry no meaning of their own here.
fn block_name(name: &str) -> Option<&'static str> {
    const BLOCKS: [&str; 25] = [
        "p",
        "div",
        "body",
        "center",
        "section",
        "article",
        "aside",
        "header",
        "footer",
        "main",
        "nav",
        "address",
        "figure",
        "figcaption",
        "form",
        "fieldset",
        "legend",
        "dl",
        "dt",
        "dd",
        "details",
        "summary",
        "hgroup",
        "dialog",
        "noscript",
    ];
    BLOCKS.iter().copied().find(|b| *b == name)
}

#[cfg(test)]
pub(crate) mod tests;
