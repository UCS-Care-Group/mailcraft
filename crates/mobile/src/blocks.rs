// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The display blocks of a message as UniFFI records, mirroring `mailcraft_html`'s `Block`,
//! `Span`, `Table`, `Row` and `Cell` (which stays free of UniFFI). Only what the native drawing
//! needs is carried; nothing is fetched, and a picture's bytes come through `Core::picture` by
//! the reference a block names.

use mailcraft_html::fonts::{self, Generic};
use mailcraft_html::{
    Align, Block, BlockKind, Cell, Insets, Length, LineHeight, LineStyle, MAX_TABLE_DEPTH, Marker, Place, Rgb, Row, Sides, Span, Table, VAlign,
    reading,
};

use crate::text::MAX_URL;

/// Most blocks, cells and spans in one message's records: past it the rest is left out.
const MAX_NODES: usize = 60_000;

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SpanView {
    pub text: String,
    pub bold: bool,
    /// 1 to 1000: what the message asks for (700 for bold text that names no number, 400 for
    /// the rest), or what the family's name says (`Segoe UI Light`) when it asks for neither.
    pub weight: u16,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    /// Where the text leads: always an `http`, `https`, `mailto`, `tel` or `cid` address, shown
    /// to the owner as it is before it is followed.
    pub link: Option<String>,
    /// `0xRRGGBB`: what the message asks for; the app adjusts it to the paper it draws on.
    pub color: Option<u32>,
    pub background: Option<u32>,
    /// CSS pixels.
    pub size: Option<u8>,
    /// The phone font the families asked for come to ([`mailcraft_html::fonts::stand_in`]).
    pub face: FaceView,
    /// A link drawn without its line: the message says so.
    #[uniffi(default = false)]
    pub bare: bool,
}

/// A kind of phone font.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FaceView {
    /// The phone's own sans-serif.
    Own,
    Serif,
    Mono,
    Condensed,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum MarkerView {
    Bullet,
    Number { number: u32 },
    Label { text: String },
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum AlignView {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum VAlignView {
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LineStyleView {
    Solid,
    Dashed,
    Dotted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LineView {
    /// CSS pixels, 0 (no line) to 4.
    pub width: u8,
    pub style: LineStyleView,
    pub color: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct InsetsView {
    pub top: u8,
    pub right: u8,
    pub bottom: u8,
    pub left: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SidesView {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LengthView {
    Px { px: u16 },
    Percent { percent: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LineHeightView {
    /// Hundredths of the font size.
    Scale {
        hundredths: u16,
    },
    Px {
        px: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum BlockKindView {
    Paragraph,
    Heading {
        level: u8,
    },
    ListItem {
        marker: MarkerView,
        depth: u8,
    },
    Preformatted,
    Rule,
    /// A picture. `picture` names it for `Core::picture` when its bytes are here (a part of the
    /// message, or written into it); a picture on the web has none, and is fetched only when the
    /// owner allows.
    Image {
        src: String,
        width: Option<u16>,
        height: Option<u16>,
        link: Option<String>,
        picture: Option<String>,
    },
    Table {
        table: TableView,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BlockView {
    pub kind: BlockKindView,
    pub spans: Vec<SpanView>,
    /// Blockquote nesting depth.
    pub quote: u8,
    pub align: AlignView,
    /// Blank lines above, 0 to 3.
    pub space: u8,
    /// Goes on in the line of the block before.
    pub beside: bool,
    /// Room above in pixels, when the message says (it then replaces `space`).
    pub gap: u8,
    pub line_height: Option<LineHeightView>,
    pub inset_left: u8,
    pub inset_right: u8,
    pub image_padding: Option<InsetsView>,
    pub line: LineView,
    /// The block reads from the right (its direction, not its side: `align` is that), as the
    /// message's `dir` or `direction` says, or a plain-text paragraph's first strong letter.
    #[uniffi(default = false)]
    pub rtl: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TableView {
    pub rows: Vec<RowView>,
    pub columns: u16,
    /// Only positions things (stacks when the columns don't fit); a data table is a grid.
    pub layout: bool,
    pub border: bool,
    pub outline: bool,
    pub width: Option<LengthView>,
    pub background: Option<u32>,
    /// Top, right, bottom, left.
    pub lines: Vec<LineView>,
    pub radius: u8,
    pub spacing: Option<u8>,
    pub inline: bool,
    pub spaced: bool,
    pub box_margin_left: u8,
    pub box_margin_right: u8,
    pub float: bool,
    /// A layout table is stacked rather than given columns that hold something narrower than
    /// this, in dp: the desktop's rule (`mailcraft_html::reading::MIN_LAYOUT_COLUMN`).
    #[uniffi(default = 110)]
    pub min_column: u8,
    /// A box on a line: the gap in front of it when it follows another, in dp.
    #[uniffi(default = 0)]
    pub gap_before: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RowView {
    pub cells: Vec<CellView>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CellView {
    pub blocks: Vec<BlockView>,
    pub header: bool,
    pub colspan: u16,
    pub rowspan: u16,
    pub background: Option<u32>,
    pub width: Option<LengthView>,
    pub border: SidesView,
    /// Top, right, bottom, left.
    pub lines: Vec<LineView>,
    pub padding: Option<InsetsView>,
    pub radius: u8,
    pub valign: Option<VAlignView>,
    /// Its place in the table's grid, as the reading pane puts it (`mailcraft_html::grid`): its
    /// first row and column and the rows and columns it covers. No two cells overlap.
    #[uniffi(default = 0)]
    pub row: u32,
    #[uniffi(default = 0)]
    pub col: u16,
    #[uniffi(default = 1)]
    pub rows: u32,
    #[uniffi(default = 1)]
    pub cols: u16,
}

fn rgb(c: Rgb) -> u32 {
    (u32::from(c.0) << 16) | (u32::from(c.1) << 8) | u32::from(c.2)
}

fn line(l: &mailcraft_html::Line) -> LineView {
    LineView {
        width: l.width,
        style: match l.style {
            LineStyle::Solid => LineStyleView::Solid,
            LineStyle::Dashed => LineStyleView::Dashed,
            LineStyle::Dotted => LineStyleView::Dotted,
        },
        color: l.color.map(rgb),
    }
}

fn insets(i: Insets) -> InsetsView {
    InsetsView { top: i.top, right: i.right, bottom: i.bottom, left: i.left }
}

fn length(l: Length) -> LengthView {
    match l {
        Length::Px(px) => LengthView::Px { px },
        Length::Percent(percent) => LengthView::Percent { percent },
    }
}

fn span(s: &Span) -> SpanView {
    let (face, named) = s.font.as_deref().map_or((Generic::Own, 400), fonts::stand_in);
    let weight = s.weight.unwrap_or(if s.bold { 700 } else { named });
    SpanView {
        text: s.text.clone(),
        bold: s.bold,
        weight,
        italic: s.italic,
        underline: s.underline,
        strike: s.strike,
        code: s.code,
        link: link(s.link.as_deref()),
        color: s.color.map(rgb),
        background: s.background.map(rgb),
        size: s.size,
        face: match face {
            Generic::Own => FaceView::Own,
            Generic::Serif => FaceView::Serif,
            Generic::Mono => FaceView::Mono,
            Generic::Condensed => FaceView::Condensed,
        },
        bare: s.bare,
    }
}

/// A link that is too long to be an address anyone typed is left out, never cut: a cut address
/// leads somewhere else.
fn link(l: Option<&str>) -> Option<String> {
    l.filter(|l| l.len() <= MAX_URL).map(str::to_string)
}

/// How a picture block is named for `Core::picture`: the `cid:` it asks for when an attachment
/// has that content id, or `data:<n>:<width>:<dark>` for the n-th picture written into the
/// message, read at that width and theme.
pub(crate) struct Pictures<'a> {
    pub has_part: &'a dyn Fn(&str) -> bool,
    pub width: u16,
    pub dark: bool,
}

/// Records for `blocks`, at most [`MAX_NODES`] of them in all.
pub(crate) fn views(blocks: &[Block], pictures: &Pictures<'_>) -> Vec<BlockView> {
    let mut budget = Budget { nodes: MAX_NODES, data: 0 };
    blocks_of(blocks, pictures, &mut budget, 0)
}

struct Budget {
    nodes: usize,
    /// Pictures written into the message met so far, in document order.
    data: usize,
}

impl Budget {
    fn take(&mut self) -> bool {
        match self.nodes.checked_sub(1) {
            Some(left) => {
                self.nodes = left;
                true
            }
            None => false,
        }
    }
}

/// The most tables the phone draws inside one another (fewer than the parser keeps): each table asks
/// what is in it of its size, so the phone's work grows with every level.
/// lean: four, as mail rarely nests deeper; raise with care, as the work is per level.
pub(crate) const PHONE_TABLE_DEPTH: usize = if MAX_TABLE_DEPTH < 4 { MAX_TABLE_DEPTH } else { 4 };

fn blocks_of(blocks: &[Block], pictures: &Pictures<'_>, budget: &mut Budget, depth: usize) -> Vec<BlockView> {
    let mut out = Vec::new();
    for b in blocks {
        if !budget.take() {
            break;
        }
        match &b.kind {
            // Deeper than the phone lays out: the grid goes, its text stays, cell by cell in
            // reading order, as the parser itself does past its own limit.
            BlockKind::Table(t) if depth >= PHONE_TABLE_DEPTH => {
                for c in t.rows.iter().flat_map(|r| &r.cells) {
                    out.extend(blocks_of(&c.blocks, pictures, budget, depth));
                }
            }
            _ => out.push(block(b, pictures, budget, depth)),
        }
    }
    out
}

fn block(b: &Block, pictures: &Pictures<'_>, budget: &mut Budget, depth: usize) -> BlockView {
    let mut spans = Vec::new();
    for s in &b.spans {
        if !budget.take() {
            break;
        }
        spans.push(span(s));
    }
    let kind = match &b.kind {
        BlockKind::Paragraph => BlockKindView::Paragraph,
        BlockKind::Heading(level) => BlockKindView::Heading { level: *level },
        BlockKind::ListItem { marker, depth } => BlockKindView::ListItem {
            marker: match marker {
                Marker::Bullet => MarkerView::Bullet,
                Marker::Number(number) => MarkerView::Number { number: *number },
                Marker::Label(text) => MarkerView::Label { text: text.clone() },
                Marker::None => MarkerView::Nothing,
            },
            depth: *depth,
        },
        BlockKind::Preformatted => BlockKindView::Preformatted,
        BlockKind::Rule => BlockKindView::Rule,
        BlockKind::Image { src, width, height, link: to, data } => {
            let picture = if data.is_some() {
                let n = budget.data;
                budget.data = budget.data.saturating_add(1);
                Some(format!("data:{n}:{}:{}", pictures.width, u8::from(pictures.dark)))
            } else if (pictures.has_part)(src) {
                Some(src.clone())
            } else {
                None
            };
            BlockKindView::Image { src: src.clone(), width: *width, height: *height, link: link(to.as_deref()), picture }
        }
        // `blocks_of` lays out a table past the phone's depth as its cells' blocks.
        BlockKind::Table(t) => BlockKindView::Table { table: table(t, pictures, budget, depth.saturating_add(1)) },
    };
    BlockView {
        kind,
        spans,
        quote: b.quote,
        align: match b.align {
            Align::Left => AlignView::Left,
            Align::Center => AlignView::Center,
            Align::Right => AlignView::Right,
        },
        space: b.space,
        beside: b.beside,
        gap: b.gap,
        line_height: b.line_height.map(|h| match h {
            LineHeight::Scale(hundredths) => LineHeightView::Scale { hundredths },
            LineHeight::Px(px) => LineHeightView::Px { px },
        }),
        inset_left: b.inset[0],
        inset_right: b.inset[1],
        image_padding: b.image_padding.map(insets),
        line: line(&b.line),
        rtl: b.rtl,
    }
}

fn table(t: &Table, pictures: &Pictures<'_>, budget: &mut Budget, depth: usize) -> TableView {
    let mut places = mailcraft_html::grid(t).places.into_iter();
    let mut rows = Vec::new();
    for r in &t.rows {
        if !budget.take() {
            break;
        }
        rows.push(row(r, &mut places, pictures, budget, depth));
    }
    TableView {
        rows,
        columns: t.columns,
        layout: t.layout,
        border: t.border,
        outline: t.outline,
        width: t.width.map(length),
        background: t.background.map(rgb),
        lines: t.lines.iter().map(line).collect(),
        radius: t.radius,
        spacing: t.spacing,
        inline: t.inline,
        spaced: t.spaced,
        box_margin_left: t.box_margin[0],
        box_margin_right: t.box_margin[1],
        float: t.float,
        min_column: reading::MIN_LAYOUT_COLUMN,
        gap_before: reading::box_gap(t),
    }
}

fn row(r: &Row, places: &mut impl Iterator<Item = Place>, pictures: &Pictures<'_>, budget: &mut Budget, depth: usize) -> RowView {
    let mut cells = Vec::new();
    for c in &r.cells {
        // Taken before the budget: once that runs out nothing after is kept, so places never slip.
        let place = places.next().unwrap_or(Place { row: 0, col: 0, rows: 1, cols: 1 });
        if !budget.take() {
            break;
        }
        cells.push(cell(c, place, pictures, budget, depth));
    }
    RowView { cells }
}

fn cell(c: &Cell, place: Place, pictures: &Pictures<'_>, budget: &mut Budget, depth: usize) -> CellView {
    let wide = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let narrow = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    CellView {
        blocks: blocks_of(&c.blocks, pictures, budget, depth),
        header: c.header,
        colspan: c.colspan,
        rowspan: c.rowspan,
        background: c.background.map(rgb),
        width: c.width.map(length),
        border: sides(c.border),
        lines: c.lines.iter().map(line).collect(),
        padding: c.padding.map(insets),
        radius: c.radius,
        valign: c.valign.map(|v| match v {
            VAlign::Top => VAlignView::Top,
            VAlign::Middle => VAlignView::Middle,
            VAlign::Bottom => VAlignView::Bottom,
        }),
        row: wide(place.row),
        col: narrow(place.col),
        rows: wide(place.rows),
        cols: narrow(place.cols),
    }
}

fn sides(s: Sides) -> SidesView {
    SidesView { top: s.top, right: s.right, bottom: s.bottom, left: s.left }
}
