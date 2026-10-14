//! MailCraft HTML for mail.
//!
//! Mail HTML is hostile: it comes from strangers, it is often broken, and it may try to run
//! scripts or load trackers. This crate never renders it. [`parse`] turns it into display
//! [`Block`]s (paragraphs, headings, list items, quotes, pictures, tables) that a UI draws itself,
//! and [`to_text`] turns it into plain text for search and plain-text replies.
//! [`text_blocks`] and [`from_text`] do the same for plain-text bodies. [`preview`] and
//! [`text_preview`] give the first words of a message on one line, for a message list.
//!
//! There is no CSS engine here. A short whitelist of presentational facts is read from inline
//! `style` attributes and old-style attributes (`style.rs`): text and background colour, bold,
//! italic, underline, strike, alignment, font size, whether something is hidden, and whether a
//! paragraph has a margin or a table cell a border at all. `<style>` blocks are dropped and
//! classes are never resolved.
//!
//! Every function takes any string, never panics, and is bounded: input over [`MAX_INPUT`] bytes is
//! cut, nesting is capped, tables are capped ([`MAX_TABLE_ROWS`], [`MAX_TABLE_COLUMNS`],
//! [`MAX_TABLE_DEPTH`], [`MAX_CELLS`]), and the work is linear in the input.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod bidi;
mod blocks;
#[doc(hidden)]
pub mod corpus;
mod entities;
mod preview;
mod style;
mod text;
mod tokenizer;

use serde::{Deserialize, Serialize};

pub use blocks::parse;
pub use entities::{escape, unescape};
pub use preview::{MAX_PREVIEW_CHARS, preview, text_preview};
pub use text::{blocks_to_text, from_text, text_blocks, to_text};

/// Longest input [`parse`], [`to_text`], [`text_blocks`], [`from_text`], [`preview`] and
/// [`text_preview`] read, in bytes; the rest is ignored.
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

/// A width a table or a cell asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Length {
    /// CSS pixels.
    Px(u16),
    /// Of the space there is, 1–100.
    Percent(u8),
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
    /// A picture; `src` is the (safe) URL or `cid:` reference, the spans hold the alt text. Only a
    /// `cid:` picture can ever be shown: nothing is fetched, so any other is a placeholder.
    Image {
        src: String,
        /// The size the message asks for, in CSS pixels.
        width: Option<u16>,
        height: Option<u16>,
        /// Where the picture leads when it is inside a link.
        link: Option<String>,
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
}

impl Block {
    /// A block of `kind` with no quoting, alignment or gap.
    pub fn new(kind: BlockKind, spans: Vec<Span>) -> Block {
        Block { kind, spans, quote: 0, align: Align::Left, space: 0 }
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
}

impl Default for Cell {
    fn default() -> Self {
        Cell { blocks: Vec::new(), header: false, colspan: 1, rowspan: 1, background: None, width: None, border: Sides::default() }
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
/// break inside a URL is an old trick to hide the scheme).
pub fn is_safe_url(url: &str) -> bool {
    let url = url.trim_matches(' ');
    if url.chars().any(char::is_control) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once(':') else { return false };
    !rest.is_empty() && ["http", "https", "mailto", "tel", "cid"].iter().any(|s| scheme.eq_ignore_ascii_case(s))
}

/// Whether a picture's address points into the message itself (`cid:`). Any other picture would
/// have to be fetched, which never happens.
pub fn is_embedded(src: &str) -> bool {
    src.trim_start().get(..4).is_some_and(|s| s.eq_ignore_ascii_case("cid:"))
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
    fn embedded_pictures_are_cid_only() {
        assert!(is_embedded("cid:logo@example.org") && is_embedded("CID:x"));
        for remote in ["https://example.org/a.png", "http://cid:x", "", "ci", "data:image/png;base64,AAAA"] {
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
        let picture = Block::new(BlockKind::Image { src: "cid:a".into(), width: None, height: None, link: None }, vec![Span::plain("alt")]);
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
