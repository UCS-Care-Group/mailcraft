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
pub(crate) const MAX_NODES: usize = 60_000;

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

/// The most characters one text block is sent with. The phone draws a block's text as one piece,
/// which can be no taller than 2^24 px; at its largest (48 px text in lines of 255 px, a font scale
/// of 2 and 4 px to the dp, a character to a line in a narrow cell) a line is about 2,040 px, so this
/// many characters stay under half of that. A longer block goes as several, one under another, cut
/// at a line break, else after a space, else between two characters: every character kept, in order.
pub(crate) const MAX_BLOCK_CHARS: usize = 4_000;

/// Text, as the phone joins it into one piece when it goes on in a line: anything not a picture or a table.
fn is_text(kind: &BlockKindView) -> bool {
    !matches!(kind, BlockKindView::Image { .. } | BlockKindView::Table { .. })
}

fn chars_of(v: &BlockView) -> usize {
    v.spans.iter().fold(0usize, |n, s| n.saturating_add(s.text.chars().count()))
}

/// A space a piece may end after: not a line break (a break of its own) nor a no-break space.
fn breaks_line(c: char) -> bool {
    c.is_whitespace() && c != '\n' && !no_break(c)
}

/// Where to end a piece within `head` (the most one may still take), at a line break or after a
/// space: the byte it ends at and the byte the next starts at. A line break goes (the next block
/// starts the line); a space stays. A line break `head` starts with is not one: it is the blank
/// line the piece starts with. None when `head` has neither.
fn break_in(head: &str) -> Option<(usize, usize)> {
    if let Some(i) = head.rfind('\n').filter(|&i| i > 0) {
        return Some((i, i.saturating_add(1)));
    }
    head.char_indices().rev().find(|(_, c)| breaks_line(*c)).map(|(i, c)| {
        let at = i.saturating_add(c.len_utf8());
        (at, at)
    })
}

/// A character that belongs with the one before it: a combining mark (Latin, Greek, Cyrillic,
/// Hebrew, Arabic and the general blocks), a joiner, a variation selector, a skin tone or a tag.
fn extends(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0300..=0x036F
            | 0x0483..=0x0489
            | 0x0591..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5
            | 0x05C7
            | 0x0610..=0x061A
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06DC
            | 0x06DF..=0x06E4
            | 0x06E7..=0x06E8
            | 0x06EA..=0x06ED
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x200C..=0x200D
            | 0x20D0..=0x20FF
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
            | 0x1F3FB..=0x1F3FF
            | 0xE0020..=0xE007F
            | 0xE0100..=0xE01EF
    )
}

/// A space that holds the words either side of it together.
fn no_break(c: char) -> bool {
    matches!(c, '\u{A0}' | '\u{2007}' | '\u{202F}')
}

fn regional(c: char) -> bool {
    matches!(u32::from(c), 0x1F1E6..=0x1F1FF)
}

/// `at` (a character boundary of `text`) moved back to the nearest boundary between two letters
/// as one sees them: not before a mark or joiner, not after a joiner, not inside a flag's pair, not
/// either side of a no-break space.
/// `at` itself when there is none before it (a run of marks as long as a block).
// lean: the common clusters only, not all of UAX #29 (Indic conjuncts, Hangul syllables in jamo).
fn whole_at(text: &str, at: usize) -> usize {
    let mut cut = at;
    loop {
        let before = text.get(..cut).and_then(|t| t.chars().next_back());
        let after = text.get(cut..).and_then(|t| t.chars().next());
        let inside = match (before, after) {
            (Some(b), Some(a)) => {
                extends(a)
                    || b == '\u{200D}'
                    || no_break(b)
                    || no_break(a)
                    || (regional(b) && regional(a) && text.get(..cut).is_some_and(|t| t.chars().rev().take_while(|c| regional(*c)).count() % 2 == 1))
            }
            _ => false,
        };
        match before {
            Some(b) if inside && cut > b.len_utf8() => cut -= b.len_utf8(),
            _ if inside => return at,
            _ => return cut,
        }
    }
}

/// Where the piece that starts at byte `from` of `text` (the block's spans joined) ends, and where
/// the next starts; None when the rest is one piece. At its last line break or space, unless what
/// follows that is a word longer than a piece (that word is cut where the room ends, rather than
/// leave a few words alone); else, a word longer than a piece, between two letters. One line break
/// at the cut goes with it (the next block starts the line); a piece may start with others.
fn next_cut(text: &str, from: usize) -> Option<(usize, usize)> {
    let rest = text.get(from..)?;
    let at = rest.char_indices().nth(MAX_BLOCK_CHARS)?.0;
    // Whether a piece from `after` on finds a break: a word of a whole piece, and the space after it.
    let breaks = |after: &str| match after.char_indices().nth(MAX_BLOCK_CHARS.saturating_add(1)) {
        None => true,
        Some((n, _)) => after.get(..n).and_then(break_in).is_some(),
    };
    let broken = rest.get(..at).and_then(break_in).filter(|&(_, resume)| rest.get(resume..).is_some_and(breaks));
    let (end, resume) = match broken {
        Some((end, resume)) if end < resume => (end, resume),
        // After a space, or between two letters: not before a mark that belongs to the letter before.
        found => {
            let cut = whole_at(rest, found.map_or(at, |(end, _)| end));
            // A line break right at the cut goes, as at any line break: the next block starts the line.
            if rest.get(..cut).is_some_and(|t| t.ends_with('\n')) && cut > 1 {
                (cut.saturating_sub(1), cut)
            } else if rest.get(cut..).is_some_and(|t| t.starts_with('\n')) {
                (cut, cut.saturating_add(1))
            } else {
                (cut, cut)
            }
        }
    };
    Some((from.saturating_add(end), from.saturating_add(resume)))
}

#[cfg(test)]
thread_local! {
    /// How many spans [`spans_in`] has looked at: what pins its one pass in a test.
    pub(crate) static SPANS_LOOKED_AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// What of `spans` (each starting at its byte of `starts` in the joined text) lies in `from..end`:
/// only the spans from the one `from` falls in are looked at, so a block is cut in one pass.
fn spans_in(spans: &[SpanView], starts: &[usize], from: usize, end: usize) -> Vec<SpanView> {
    let first = starts.partition_point(|&start| start <= from).saturating_sub(1);
    spans
        .iter()
        .zip(starts)
        .skip(first)
        .take_while(|(_, start)| {
            #[cfg(test)]
            SPANS_LOOKED_AT.with(|n| n.set(n.get().saturating_add(1)));
            **start < end
        })
        .filter_map(|(s, &start)| {
            let a = from.max(start).saturating_sub(start);
            let b = end.min(start.saturating_add(s.text.len())).saturating_sub(start);
            s.text.get(a..b).filter(|t| !t.is_empty()).map(|t| SpanView { text: t.to_string(), ..s.clone() })
        })
        .collect()
}

/// `v` as blocks of at most [`MAX_BLOCK_CHARS`] characters each, cut where [`next_cut`] says, the
/// cuts seen across its spans; each piece after the first taken from `budget` (past it the rest is
/// left out, as with any block). The first piece with text is `v`'s own, with its room above and its
/// marker; the others go on under it. Of the line breaks, only one at a cut goes (the break between
/// two blocks stands for it); every other is kept, blank lines between pieces and at either end.
fn pieces(v: BlockView, budget: &mut Budget) -> Vec<BlockView> {
    if !is_text(&v.kind) || chars_of(&v) <= MAX_BLOCK_CHARS {
        return vec![v];
    }
    let mut first = v;
    let spans = std::mem::take(&mut first.spans);
    let text: String = spans.iter().map(|s| s.text.as_str()).collect();
    let starts: Vec<usize> = spans
        .iter()
        .scan(0usize, |at, s| {
            let start = *at;
            *at = at.saturating_add(s.text.len());
            Some(start)
        })
        .collect();
    // A piece after the first goes on under it: no room above, and a list item's marker only once.
    let mut next = first.clone();
    next.space = 0;
    next.gap = 0;
    next.beside = false;
    if let BlockKindView::ListItem { depth, .. } = next.kind {
        next.kind = BlockKindView::ListItem { marker: MarkerView::Nothing, depth };
    }
    let mut out: Vec<BlockView> = Vec::new();
    let mut from = 0usize;
    while from < text.len() {
        let (end, resume) = next_cut(&text, from).unwrap_or((text.len(), text.len()));
        let piece = spans_in(&spans, &starts, from, end);
        if !piece.is_empty() {
            if !out.is_empty() && !budget.take() {
                return out;
            }
            let like = if out.is_empty() { &first } else { &next };
            out.push(BlockView { spans: piece, ..like.clone() });
        }
        // Never stands still: a cut is always past `from`.
        from = resume.max(from.saturating_add(1));
    }
    out
}

/// The run of text going on in one line after `v`, after a run of `run` characters: `v` made to
/// start a line of its own where it would make that run longer than a block may be.
fn run_after(run: usize, v: &mut BlockView) -> usize {
    if !is_text(&v.kind) {
        return 0;
    }
    let n = chars_of(v);
    if v.beside && run.saturating_add(n) > MAX_BLOCK_CHARS {
        v.beside = false;
    }
    if v.beside { run.saturating_add(n) } else { n }
}

fn blocks_of(blocks: &[Block], pictures: &Pictures<'_>, budget: &mut Budget, depth: usize) -> Vec<BlockView> {
    let mut out = Vec::new();
    // The characters of the text going on in one line so far: the phone draws such a run as one piece.
    let mut run = 0usize;
    for b in blocks {
        if !budget.take() {
            break;
        }
        match &b.kind {
            // Deeper than the phone lays out: the grid goes, its text stays, cell by cell in
            // reading order, as the parser itself does past its own limit.
            BlockKind::Table(t) if depth >= PHONE_TABLE_DEPTH => {
                // Its cells' text goes on the line as the phone joins it, and what follows after its last line.
                for c in t.rows.iter().flat_map(|r| &r.cells) {
                    for mut v in blocks_of(&c.blocks, pictures, budget, depth) {
                        run = run_after(run, &mut v);
                        out.push(v);
                    }
                }
            }
            _ => {
                for mut v in pieces(block(b, pictures, budget, depth), budget) {
                    run = run_after(run, &mut v);
                    out.push(v);
                }
            }
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
