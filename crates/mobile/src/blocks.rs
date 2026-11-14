// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The display blocks of a message as UniFFI records, mirroring `mailcraft_html`'s `Block`,
//! `Span`, `Table`, `Row` and `Cell` (which stays free of UniFFI). Only what the native drawing
//! needs is carried; nothing is fetched, and a picture's bytes come through `Core::picture` by
//! the reference a block names.

use mailcraft_html::{Align, Block, BlockKind, Cell, Insets, Length, LineHeight, LineStyle, Marker, Rgb, Row, Sides, Span, Table, VAlign};

use crate::text::MAX_URL;

/// Deepest nesting of tables that is carried (the parser keeps to 12).
const MAX_DEPTH: usize = 16;
/// Most blocks, cells and spans in one message's records: past it the rest is left out.
const MAX_NODES: usize = 60_000;

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SpanView {
    pub text: String,
    pub bold: bool,
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
    /// Font families asked for, lower case and comma-joined.
    pub font: Option<String>,
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
    SpanView {
        text: s.text.clone(),
        bold: s.bold,
        italic: s.italic,
        underline: s.underline,
        strike: s.strike,
        code: s.code,
        link: link(s.link.as_deref()),
        color: s.color.map(rgb),
        background: s.background.map(rgb),
        size: s.size,
        font: s.font.clone(),
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

fn blocks_of(blocks: &[Block], pictures: &Pictures<'_>, budget: &mut Budget, depth: usize) -> Vec<BlockView> {
    let mut out = Vec::new();
    for b in blocks {
        if !budget.take() {
            break;
        }
        out.push(block(b, pictures, budget, depth));
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
        BlockKind::Table(t) if depth < MAX_DEPTH => BlockKindView::Table { table: table(t, pictures, budget, depth + 1) },
        // Nested deeper than any parser output: its text stays, its grid goes.
        BlockKind::Table(_) => BlockKindView::Paragraph,
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
    }
}

fn table(t: &Table, pictures: &Pictures<'_>, budget: &mut Budget, depth: usize) -> TableView {
    let mut rows = Vec::new();
    for r in &t.rows {
        if !budget.take() {
            break;
        }
        rows.push(row(r, pictures, budget, depth));
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
    }
}

fn row(r: &Row, pictures: &Pictures<'_>, budget: &mut Budget, depth: usize) -> RowView {
    let mut cells = Vec::new();
    for c in &r.cells {
        if !budget.take() {
            break;
        }
        cells.push(cell(c, pictures, budget, depth));
    }
    RowView { cells }
}

fn cell(c: &Cell, pictures: &Pictures<'_>, budget: &mut Budget, depth: usize) -> CellView {
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
    }
}

fn sides(s: Sides) -> SidesView {
    SidesView { top: s.top, right: s.right, bottom: s.bottom, left: s.left }
}
