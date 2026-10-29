// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft HTML for mail.
//!
//! Mail HTML is hostile: it comes from strangers, it is often broken, and it may try to run
//! scripts or load trackers. This crate never renders it. [`parse`] turns it into display
//! [`Block`]s (paragraphs, headings, list items, quotes, pictures, tables) that a UI draws itself,
//! and [`to_text`] turns it into plain text for search and plain-text replies.
//! [`text_blocks`] and [`from_text`] do the same for plain-text bodies. [`preview`] and
//! [`text_preview`] give the first words of a message on one line, for a message list.
//! [`quotable`] writes a message's HTML out again for the original under a reply or forward: its
//! look, without anything that runs, embeds or tracks.
//!
//! There is no CSS engine here. A short whitelist of presentational facts is read from inline
//! `style` attributes and old-style attributes (`style.rs`): text and background colour, bold,
//! italic, underline, strike, alignment, font size, whether something is hidden, and whether a
//! paragraph has a margin or a table cell a border at all. The message's `<style>` blocks are
//! read for the same whitelist and nothing else (`sheet.rs`: selectors, specificity, order,
//! `!important`, within caps); they are never run and nothing in them is fetched.
//!
//! Every function takes any string, never panics, and is bounded: input over [`MAX_INPUT`] bytes is
//! cut, nesting is capped, tables are capped ([`MAX_TABLE_ROWS`], [`MAX_TABLE_COLUMNS`],
//! [`MAX_TABLE_DEPTH`], [`MAX_CELLS`]), and the work is linear in the input.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod blocks;
#[doc(hidden)]
pub mod corpus;
mod data;
mod entities;
mod look;
mod media;
mod preview;
mod quote;
mod sheet;
mod style;
mod text;
mod tokenizer;

use serde::{Deserialize, Serialize};

pub use blocks::{MAX_HIDDEN_TEXT, Read, parse, parse_in, parse_text};
pub use data::{MAX_DATA_PICTURE_BYTES, MAX_DATA_PICTURES, MAX_DATA_PICTURES_BYTES, Picture};
pub use entities::{escape, unescape};
pub use media::{Env, verdicts as media_verdicts};

/// The media lists of a message's style sheets and `<style media>` attributes, each once, in the
/// order they are met (inside `@media` too, to its depth): what its look can change with. A
/// message with none looks the same in every pane and theme. At most 400.
pub fn media_lists(html: &str) -> Vec<String> {
    sheet::media_lists(html)
}
pub use preview::{MAX_PREVIEW_CHARS, preview, text_preview};
pub use quote::quotable;
pub use text::{blocks_to_text, from_text, text_and_hidden, text_blocks, to_text};

/// How many rules a style sheet's text gives, as the reading of a message's `<style>` blocks reads
/// it ([`MAX_SHEET_BYTES`] of it, [`MAX_SHEET_RULES`] rules). For the fuzz target.
#[doc(hidden)]
pub fn style_sheet_rules(text: &str) -> usize {
    sheet::rules(text).len()
}

/// Most bytes of style sheet read from one message; the rest is ignored.
pub const MAX_SHEET_BYTES: usize = sheet::MAX_SHEET;

/// Most rules read from one message's style sheets.
pub const MAX_SHEET_RULES: usize = sheet::MAX_RULES;

/// Longest input [`parse`], [`to_text`], [`text_blocks`], [`from_text`], [`preview`],
/// [`text_preview`] and [`quotable`] read, in bytes; the rest is ignored. [`parse`] and
/// [`to_text`] don't count the pictures written into the message (`data:`) against it, and read
/// as much more as the most those may take.
pub const MAX_INPUT: usize = 2 * 1024 * 1024;

/// Most blocks one document gives, counting those inside table cells; what follows is dropped.
pub const MAX_BLOCKS: usize = 20_000;

/// Most rows one table keeps; the text of further rows stays, in the last cell.
pub const MAX_TABLE_ROWS: usize = 1_000;

/// Most cells one table row keeps (and the most columns a cell may span); the text of further
/// cells stays, in the last one.
pub const MAX_TABLE_COLUMNS: usize = 64;

/// Deepest tables (and blocks with a background of their own) may nest; a table nested deeper
/// keeps its text and loses its grid.
pub const MAX_TABLE_DEPTH: usize = 12;

/// Most table cells one document keeps.
pub const MAX_CELLS: usize = 20_000;

/// Most rows a cell may span.
pub const MAX_ROW_SPAN: u16 = 200;

/// The smallest font size a message can ask for, in CSS pixels. Smaller text is shown at this
/// size (text of 2 pixels or less is a hiding trick and is not shown at all).
pub const MIN_FONT_PX: u8 = 10;

/// The largest font size a message can ask for, in CSS pixels.
pub const MAX_FONT_PX: u8 = 32;

/// A colour a message asks for (sRGB).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// How a block sits in its line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Where a cell's content sits in the height of its row, when the source says (`valign`,
/// `vertical-align`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VAlign {
    Top,
    Middle,
    Bottom,
}

/// The most boxes a message can lay out on lines (`display: inline-block`); past it the elements
/// are read as they were without it.
pub const MAX_INLINE_BOXES: usize = 256;

/// The widest gap between cells read from `cellspacing` or `border-spacing`, in pixels.
pub const MAX_SPACING_PX: u8 = 16;

/// A width a table or a cell asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Length {
    /// CSS pixels.
    Px(u16),
    /// Of the space there is, 1–100.
    Percent(u8),
}

/// The most a message can ask for as padding or margin on a side, in CSS pixels.
pub const MAX_BOX_PX: u8 = 64;

/// The thickest line a message can ask for round a box, in CSS pixels (a thinner one than a
/// pixel is a pixel).
pub const MAX_LINE_PX: u8 = 4;

/// The roundest corner a message can ask for, in CSS pixels.
pub const MAX_RADIUS_PX: u8 = 24;

/// The least and most a message can ask of a line's height, in hundredths of its font size.
pub const MIN_LINE_HEIGHT: u16 = 100;
pub const MAX_LINE_HEIGHT: u16 = 250;

/// Room on the four sides of a box, in CSS pixels, each at most [`MAX_BOX_PX`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Insets {
    pub top: u8,
    pub right: u8,
    pub bottom: u8,
    pub left: u8,
}

impl Insets {
    pub fn any(self) -> bool {
        self.top > 0 || self.right > 0 || self.bottom > 0 || self.left > 0
    }
}

/// How a line round a box is drawn. The quieter styles of CSS (`double`, `groove`, `inset`...) are
/// drawn solid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

/// The line on one side of a box. A width of 0 is no line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Line {
    /// CSS pixels, 0 to [`MAX_LINE_PX`].
    pub width: u8,
    pub style: LineStyle,
    /// The colour the message asks for; a UI adjusts it to the paper it is drawn on.
    pub color: Option<Rgb>,
}

impl Line {
    /// A plain one-pixel line in the reader's own line colour.
    pub const THIN: Line = Line { width: 1, style: LineStyle::Solid, color: None };

    pub fn is_set(&self) -> bool {
        self.width > 0
    }
}

/// How tall a line of text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LineHeight {
    /// Hundredths of the font size, [`MIN_LINE_HEIGHT`] to [`MAX_LINE_HEIGHT`].
    Scale(u16),
    /// CSS pixels, 1 to 200 (a UI holds it to the same range of the font size).
    Px(u8),
}

impl LineHeight {
    /// The height of a line of text of `size` pixels, held to 1 to 2.5 times the size.
    pub fn of(self, size: f32) -> f32 {
        let size = if size.is_finite() { size.max(1.0) } else { 1.0 };
        let wanted = match self {
            LineHeight::Scale(hundredths) => size * f32::from(hundredths) / 100.0,
            LineHeight::Px(px) => f32::from(px),
        };
        wanted.clamp(size * f32::from(MIN_LINE_HEIGHT) / 100.0, size * f32::from(MAX_LINE_HEIGHT) / 100.0)
    }
}

/// The sides of a table cell that have a line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Sides {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl Sides {
    pub const ALL: Sides = Sides { top: true, right: true, bottom: true, left: true };

    pub fn any(self) -> bool {
        self.top || self.right || self.bottom || self.left
    }
}

/// A run of text with inline formatting.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
    /// The text colour the message asks for. A UI checks it against the background it really
    /// paints on before using it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    /// A highlight behind the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Rgb>,
    /// Font size in CSS pixels, [`MIN_FONT_PX`] to [`MAX_FONT_PX`]; `None` for the reader's own size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u8>,
    /// The first font family asked for, lower case (`calibri`), for pasting into the composer,
    /// which keeps the few it knows. The reading pane draws in its own font.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
}

impl Span {
    /// Unformatted text.
    pub fn plain(text: impl Into<String>) -> Span {
        Span { text: text.into(), ..Span::default() }
    }

    /// Text that opens `url`.
    pub fn linked(text: impl Into<String>, url: impl Into<String>) -> Span {
        Span { text: text.into(), link: Some(url.into()), ..Span::default() }
    }

    /// Whether two spans look the same, whatever their text.
    pub fn same_format(&self, other: &Span) -> bool {
        self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.strike == other.strike
            && self.code == other.code
            && self.link == other.link
            && self.color == other.color
            && self.background == other.background
            && self.size == other.size
    }
}

/// What stands in front of a list item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Marker {
    Bullet,
    /// The item's number in an ordered list.
    Number(u32),
    /// A marker the source wrote out or asked for by type (`a.`, `iv.`).
    Label(String),
    /// No marker: a further paragraph of the item above, indented like it.
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockKind {
    Paragraph,
    /// Level 1–6.
    Heading(u8),
    /// `depth`: 0 for a top-level list.
    ListItem {
        marker: Marker,
        depth: u8,
    },
    Preformatted,
    /// A horizontal rule (no spans).
    Rule,
    /// A picture; `src` is the (safe) URL or `cid:` reference, the spans hold the alt text. A
    /// picture written into the message (`data:`) has `src` `data:` and its type, never its
    /// content, and its bytes in `data` when it can be shown. Nothing is fetched here: a picture
    /// on the web is shown only when the owner asks and a host brings it.
    Image {
        src: String,
        /// The size the message asks for, in CSS pixels.
        width: Option<u16>,
        height: Option<u16>,
        /// Where the picture leads when it is inside a link.
        link: Option<String>,
        /// A picture written into the message: its bytes, a PNG, JPEG, GIF or WebP by what they
        /// are, within [`MAX_DATA_PICTURE_BYTES`] and the caps for a message.
        #[serde(skip)]
        data: Option<Picture>,
    },
    /// A table, or a block with a background of its own (one row, one cell). No spans.
    Table(Table),
}

/// One thing to draw: a paragraph, a heading, a list item, a rule, a picture or a table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub kind: BlockKind,
    pub spans: Vec<Span>,
    /// Blockquote nesting depth inside the block's container (the message, or a table cell): 0 for
    /// text that isn't quoted.
    pub quote: u8,
    #[serde(default)]
    pub align: Align,
    /// Blank lines above the block: 0 when it follows the one before directly (the lines of an
    /// Outlook or Gmail message), 1 for an ordinary paragraph gap, up to 3. Always 0 for the first
    /// block of its container.
    #[serde(default)]
    pub space: u8,
    /// The block goes on in the line of the block before: a picture in a line of text, or the
    /// text that follows a picture in its line (`<img> 12`). Never set on the first block of its
    /// container, nor where a line break or a block of its own came in between.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub beside: bool,
    /// Room above the block in pixels, from a margin of the block or the one above (the larger of
    /// the two), at most [`MAX_BOX_PX`]. When it is set it is the gap, instead of `space`'s.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub gap: u8,
    /// How tall its lines are, where the message says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<LineHeight>,
    /// Room kept free left and right of the block, in pixels: the margins and padding of the
    /// elements round it that have no frame of their own, saturating at 255.
    #[serde(default, skip_serializing_if = "is_no_inset")]
    pub inset: [u8; 2],
    /// How a rule looks, where the source draws it with a border (a `Rule` block only).
    #[serde(default, skip_serializing_if = "is_default_line")]
    pub line: Line,
}

fn is_no_inset(inset: &[u8; 2]) -> bool {
    *inset == [0, 0]
}

fn is_default_line(line: &Line) -> bool {
    *line == Line::default()
}

fn is_zero_u8(n: &u8) -> bool {
    *n == 0
}

impl Block {
    /// A block of `kind` with no quoting, alignment or gap.
    pub fn new(kind: BlockKind, spans: Vec<Span>) -> Block {
        Block { kind, spans, quote: 0, align: Align::Left, space: 0, beside: false, gap: 0, line_height: None, inset: [0, 0], line: Line::default() }
    }

    /// The block's text without formatting (not what is inside a table: see [`has_text`]).
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// A table. `layout` tells the two uses of tables in mail apart: a data table is a grid to read
/// across and down; a layout table only positions things, and may be shown as a stack.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub rows: Vec<Row>,
    /// The widest row, in columns (at most [`MAX_TABLE_COLUMNS`]).
    pub columns: u16,
    pub layout: bool,
    /// The source asks for a line on every side of every cell (the `border` attribute). A cell
    /// may also ask for lines of its own: [`Cell::border`].
    pub border: bool,
    /// The source asks for a line around the whole table (or the block with its own background).
    pub outline: bool,
    pub width: Option<Length>,
    pub background: Option<Rgb>,
    /// The lines round the whole table (top, right, bottom, left) as the source draws them. Set
    /// together with `outline` when the source frames it; a UI draws `outline` alone as thin lines.
    #[serde(default)]
    pub lines: [Line; 4],
    /// Rounded corners, in pixels.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub radius: u8,
    /// The gap between its cells (`cellspacing`, `border-spacing`), in pixels, where the source
    /// gives one; `None` leaves it to the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spacing: Option<u8>,
    /// The source lays it out as a box on a line (`display: inline-block`): the boxes that follow
    /// one another stand side by side and wrap when the line is full. Its `width` is the box's,
    /// and `None` is as wide as what is in it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inline: bool,
    /// A box on a line: the source has white space between it and the box before it on the line,
    /// which is a gap there (none when it has not, or when it was written at a size of nothing).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub spaced: bool,
    /// A box on a line: the room its margins keep free left and right of it, in pixels.
    #[serde(default, skip_serializing_if = "is_no_inset")]
    pub box_margin: [u8; 2],
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub blocks: Vec<Block>,
    /// A `<th>`.
    pub header: bool,
    /// Columns and rows the cell covers, at least 1.
    pub colspan: u16,
    pub rowspan: u16,
    pub background: Option<Rgb>,
    pub width: Option<Length>,
    /// The sides the cell asks to have a line on. Whether, never how thick or what colour.
    #[serde(default)]
    pub border: Sides,
    /// How those lines look, side by side (top, right, bottom, left).
    #[serde(default)]
    pub lines: [Line; 4],
    /// The room inside the cell where the source gives it; `None` when it says nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<Insets>,
    /// Rounded corners, in pixels.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub radius: u8,
    /// Where its content sits in the row, its own or its row's; `None` when the source says nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valign: Option<VAlign>,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            blocks: Vec::new(),
            header: false,
            colspan: 1,
            rowspan: 1,
            background: None,
            width: None,
            border: Sides::default(),
            lines: [Line::default(); 4],
            padding: None,
            radius: 0,
            valign: None,
        }
    }
}

/// Whether any of `blocks` holds text to read, looking inside tables. Pictures and rules don't count.
pub fn has_text(blocks: &[Block]) -> bool {
    blocks.iter().any(|b| match &b.kind {
        BlockKind::Table(table) => table.rows.iter().any(|r| r.cells.iter().any(|c| has_text(&c.blocks))),
        BlockKind::Image { .. } | BlockKind::Rule => false,
        _ => b.spans.iter().any(|s| !s.text.trim().is_empty()),
    })
}

/// Every picture block in `blocks`, in reading order, looking inside tables.
pub fn images(blocks: &[Block]) -> Vec<&Block> {
    fn walk<'a>(blocks: &'a [Block], out: &mut Vec<&'a Block>) {
        for b in blocks {
            match &b.kind {
                BlockKind::Image { .. } => out.push(b),
                BlockKind::Table(table) => {
                    for cell in table.rows.iter().flat_map(|r| &r.cells) {
                        walk(&cell.blocks, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(blocks, &mut out);
    out
}

/// Whether a link may be followed or a picture loaded: `http`, `https`, `mailto`, `tel` and `cid`
/// only (the scheme in any letter case). Everything else is refused: `javascript:`, `data:`,
/// `file:`, `vbscript:`, relative URLs, and anything holding a control character (a tab or line
/// break inside a URL is an old trick to hide the scheme) or a character that changes how text
/// looks without being seen (a right-to-left override or isolate, a zero-width mark, a byte-order
/// mark: what is shown of the address would not be where it goes).
pub fn is_safe_url(url: &str) -> bool {
    let url = url.trim_matches(' ');
    if url.chars().any(|c| c.is_control() || is_format(c)) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once(':') else { return false };
    if rest.is_empty() || !["http", "https", "mailto", "tel", "cid"].iter().any(|s| scheme.eq_ignore_ascii_case(s)) {
        return false;
    }
    // A web address the parser rejects, or that has no host, would be taken by the system's
    // opener for a relative file path (and become a `file:` address): it is no link.
    if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
        return mailcraft_link::WebLink::parse(url).is_some();
    }
    true
}

/// Unicode's format characters (general category Cf): the one list, in `mailcraft-names` (#454).
pub use mailcraft_names::is_format;

/// Whether a picture's address points into the message itself: an attached picture (`cid:`), or
/// one written into it (`data:`). Any other picture would have to be fetched.
pub fn is_embedded(src: &str) -> bool {
    src.trim_start().get(..4).is_some_and(|s| s.eq_ignore_ascii_case("cid:")) || data::is_data(src)
}

/// `s` cut to at most `max` bytes, on a character boundary.
pub(crate) fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or("")
}

/// Whether a character shows nothing: white space, a no-break space, or a zero-width mark
/// (newsletters pad their preview text with these).
pub(crate) fn is_blank(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}')
}

#[cfg(test)]
mod data_tests;
#[cfg(test)]
mod inline_tests;
#[cfg(test)]
mod text_link_tests;

#[cfg(test)]
mod url_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_urls() {
        for ok in [
            "http://example.org",
            "HTTPS://example.org/a?b=c#d",
            "mailto:ada@example.org",
            "tel:+15550100",
            "cid:logo@example.org",
            " https://example.org ",
        ] {
            assert!(is_safe_url(ok), "{ok}");
        }
        for bad in [
            "",
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "data:text/html,<b>x</b>",
            "file:///etc/passwd",
            "vbscript:msgbox(1)",
            "java\nscript:alert(1)",
            "java\tscript:alert(1)",
            "https://example.org/\n",
            "\u{0}https://example.org",
            "/relative/path",
            "//example.org/x",
            "example.org",
            "https:",
            " javascript:alert(1)",
            "ftp://example.org",
            "about:blank",
        ] {
            assert!(!is_safe_url(bad), "{bad:?}");
        }
    }

    #[test]
    fn urls_with_invisible_characters_are_refused() {
        for bad in [
            "https://bank.example/\u{202e}moc.live",
            "https://bank.example\u{2067}.evil.example\u{2069}/",
            "https://ba\u{200b}nk.example/",
            "https://bank.example\u{200d}/",
            "\u{feff}https://example.org",
            "https://exa\u{00ad}mple.org",
            "mailto:ada\u{2066}@example.org",
            "https://example.org/\u{e0041}",
        ] {
            assert!(!is_safe_url(bad), "{bad:?}");
        }
        // Letters of any script are not format characters.
        assert!(is_safe_url("https://bücher.example/straße"));
    }

    #[test]
    fn embedded_pictures_are_attached_or_written_in() {
        assert!(is_embedded("cid:logo@example.org") && is_embedded("CID:x"));
        assert!(is_embedded("data:image/png") && is_embedded(" DATA:x"));
        for remote in ["https://example.org/a.png", "http://cid:x", "", "ci", "dat:image/png;base64,AAAA"] {
            assert!(!is_embedded(remote), "{remote}");
        }
    }

    #[test]
    fn truncation_keeps_char_boundaries() {
        assert_eq!(truncate("héllo", 2), "h");
        assert_eq!(truncate("héllo", 3), "hé");
        assert_eq!(truncate("héllo", 99), "héllo");
        assert_eq!(truncate("é", 1), "");
        assert_eq!(truncate("", 0), "");
    }

    #[test]
    fn span_and_block_helpers() {
        let b = Block {
            quote: 1,
            ..Block::new(BlockKind::ListItem { marker: Marker::Number(2), depth: 1 }, vec![Span::linked("x", "https://example.org")])
        };
        assert_eq!(b.text(), "x");
        assert!(Span::plain("a").same_format(&Span::plain("b")));
        assert!(!Span::plain("a").same_format(&Span::linked("a", "cid:x")));
        assert!(!Span::plain("a").same_format(&Span { color: Some(Rgb(1, 2, 3)), ..Span::plain("a") }));
        assert!(!Span::plain("a").same_format(&Span { size: Some(16), ..Span::plain("a") }));
    }

    #[test]
    fn text_and_pictures_are_found_inside_tables() {
        let picture =
            Block::new(BlockKind::Image { src: "cid:a".into(), width: None, height: None, link: None, data: None }, vec![Span::plain("alt")]);
        let cell = |blocks| Cell { blocks, ..Cell::default() };
        let table = |cells| Block::new(BlockKind::Table(Table { rows: vec![Row { cells }], columns: 1, ..Table::default() }), Vec::new());
        assert!(!has_text(&[]));
        assert!(!has_text(&[picture.clone(), Block::new(BlockKind::Rule, Vec::new())]));
        assert!(!has_text(&[table(vec![cell(vec![picture.clone()])])]));
        let nested = table(vec![cell(vec![table(vec![cell(vec![Block::new(BlockKind::Paragraph, vec![Span::plain("deep")]), picture.clone()])])])]);
        assert!(has_text(std::slice::from_ref(&nested)));
        assert_eq!(images(&[picture, nested]).len(), 2);
    }
}
