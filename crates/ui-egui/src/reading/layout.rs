//! Where everything of a message goes: display blocks to positioned things to paint.
//!
//! This is done once for a message at a width and in a theme, and kept ([`super::view`]): painting
//! a frame then only walks the result and paints what is in view. Laying out runs through a
//! budget, a stretch of the message per frame, so a very long message shows its top at once.
//!
//! Text is not kept as glyphs (a long message would take hundreds of megabytes): each piece keeps
//! its layout job and its measured size, and the pieces in view are shaped again when painted,
//! which egui caches from frame to frame.
//!
//! Tables: columns get the widths their contents and the source's hints call for, squeezed to the
//! room there is. A data table is never squeezed below its longest words (the pane scrolls
//! sideways instead) and never stacked. A layout table whose columns would be too narrow is
//! stacked: its cells one under the other, each as wide as the room.

use std::ops::Range;
use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Color32, Rect, Stroke, Vec2, pos2, vec2};
use mailcraft_html::bidi::{self, Direction};
use mailcraft_html::{Align, Block, BlockKind, Cell, Length, Marker, Sides, Span, Table};
use mailcraft_model::Message;

use super::colors::Paper;
use super::images::{self, Pictures};
use crate::theme::{Tokens, mono, regular, semibold};

/// The size of text that asks for none, in points.
pub const BASE_SIZE: f32 = 13.0;
/// Line height, as a multiple of the font size.
const LINE: f32 = 1.45;
/// The gap above an ordinary paragraph, and what each further blank line adds.
const PARAGRAPH_GAP: f32 = 10.0;
const BLANK_LINE: f32 = 18.0;
/// How far each level of quoting moves text in, and how many levels get a bar and an indent.
const QUOTE_INDENT: f32 = 14.0;
pub const MAX_QUOTE_LEVELS: u8 = 8;
/// A list item's indent, and what each level of nesting adds (up to [`MAX_LIST_DEPTH`]).
const LIST_INDENT: f32 = 22.0;
const LIST_STEP: f32 = 18.0;
const MAX_LIST_DEPTH: u8 = 8;
const RULE_HEIGHT: f32 = 13.0;
/// The gap between pictures in a line, and between lines of pictures.
const PICTURE_GAP: f32 = 6.0;
/// The tallest a picture is shown, in points.
const MAX_PICTURE_HEIGHT: f32 = 6000.0;
/// The size of the stand-in for a picture that is not shown.
const CHIP_HEIGHT: f32 = 24.0;
const CHIP_FONT: f32 = 11.5;
/// Padding inside a cell of a data table, and inside a cell or block that is painted.
const DATA_PADDING: Vec2 = vec2(8.0, 5.0);
const BOX_PADDING: Vec2 = vec2(12.0, 9.0);
/// The gaps between the columns and rows of a layout table that is not painted.
const COLUMN_GAP: f32 = 16.0;
const ROW_GAP: f32 = 6.0;
/// A layout table is stacked rather than given columns narrower than this.
const MIN_LAYOUT_COLUMN: f32 = 110.0;
/// Nothing is laid out narrower than this, however little room there is.
const MIN_WIDTH: f32 = 40.0;
/// What is in a table cell is given at least this much room, and a column is at least this wide
/// inside its padding: a column of single digits does not squeeze its neighbours' words apart.
const MIN_CELL: f32 = 24.0;
/// The tallest row of a data table whose cells are centred in it, in points.
const MAX_CENTRED_ROW: f32 = 600.0;
/// The widest anything is measured to be.
const WIDE: f32 = 4000.0;
/// Text up to this long is measured exactly when columns are sized; longer text is estimated.
const MEASURED_CHARS: usize = 200;
/// A word wider than this does not widen its column: it is broken when room is short (an
/// address, a long number).
const MAX_WORD: f32 = 110.0;
/// Text is laid out in pieces of about this many characters, so that a huge paragraph costs no
/// more to keep or to scroll than many small ones.
pub const PIECE_CHARS: usize = 4_000;
/// Tables nested deeper than this are stacked (the parser caps nesting well below it).
const MAX_NESTING: usize = 24;
/// Most pictures that stand in one line run.
const MAX_RUN: usize = 64;

/// Most tabs written between two texts of a table row when they are copied.
const MAX_TABS: u8 = 64;

/// What [`Item::seg`] is for a thing that is not selected by itself: a background, a line, a
/// list's bullet or number.
pub const NO_SEG: u32 = u32::MAX;

/// One thing to paint.
#[derive(Clone, Debug)]
pub struct Item {
    /// Where, from the top left of the message.
    pub rect: Rect,
    pub kind: ItemKind,
    /// Its number among the things of the message that can be selected, in reading order: the
    /// texts, the pictures and what stands for a picture ([`Laid::segs`]). The same whatever the
    /// width and whichever pictures have come, so a selection outlives a new layout.
    pub seg: u32,
}

/// What stands between a text and the one before it when both are copied as plain text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Join {
    /// Line breaks: one for a new line, one more for each blank line above the block.
    pub lines: u8,
    /// Tabs after those: the cells crossed in a table row since the text before.
    pub tabs: u8,
    /// The text goes on in the paragraph of the one before, which was cut for its length.
    pub cont: bool,
    /// Where there is neither line break nor tab: the white space that was dropped between the
    /// two (at the cut in a long paragraph, or between two pictures in a line).
    pub glue: Option<char>,
}

impl Join {
    /// Write what stands between.
    pub fn write(&self, out: &mut String) {
        out.extend(std::iter::repeat_n('\n', usize::from(self.lines)));
        out.extend(std::iter::repeat_n('\t', usize::from(self.tabs)));
        if self.lines == 0 && self.tabs == 0 {
            out.extend(self.glue);
        }
    }
}

/// The white space that was dropped where a long paragraph was cut in two; `None` where it had
/// to be cut inside a word.
pub type Glue = Option<char>;

#[derive(Clone, Debug)]
pub enum ItemKind {
    /// A filled rectangle: a background, a rule, a quote bar, a cell border.
    Fill {
        color: Color32,
        radius: f32,
    },
    /// A line round a table or a block.
    Outline {
        color: Color32,
    },
    Text(TextItem),
    /// A list bullet; its shape goes by the list's depth.
    Bullet {
        depth: u8,
        color: Color32,
    },
    Picture(PictureItem),
    Placeholder(PlaceholderItem),
}

/// A piece of text: a paragraph, or part of a very long one.
#[derive(Clone, Debug)]
pub struct TextItem {
    /// The text, formatted, wrapped to the item's width.
    pub job: Arc<LayoutJob>,
    /// Where the text's origin is, from the left of the item (centred and right-aligned text is
    /// laid out around its origin).
    pub anchor: f32,
    /// The parts that lead somewhere: character ranges and their addresses.
    pub links: Vec<(Range<usize>, Arc<str>)>,
    /// What is painted behind it: what its colours were checked against (and are, by the tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub paper: Color32,
    /// How many characters it has.
    pub chars: u32,
    /// What stands between it and the text before when both are copied.
    pub join: Join,
    /// What a copy that starts at its beginning writes in front of it: a list item's marker.
    pub lead: String,
    /// Text that runs right to left is laid out as it is drawn (`job`), and selected and
    /// copied as it was written (`logical`; `chars` counts its characters): for each character
    /// of `job`, where it is in the text as written (`u32::MAX` for a line break put in where a
    /// line was wrapped).
    pub order: Option<Arc<[u32]>>,
    /// The text as written, when `order` is there.
    pub logical: Option<Arc<str>>,
}

impl TextItem {
    /// The text a selection counts in and a copy takes from: as it was written.
    pub fn written_text(&self) -> &str {
        self.logical.as_deref().unwrap_or(&self.job.text)
    }

    /// The characters `part` of the text as written.
    pub fn written(&self, part: Range<usize>) -> String {
        super::select::char_slice(self.written_text(), part.start, part.end).to_string()
    }

    /// Where a place in `job` (a drawn character) is in the text as written.
    pub fn written_at(&self, drawn: usize) -> usize {
        match &self.order {
            None => drawn,
            Some(order) => match order.get(drawn) {
                Some(i) if *i != u32::MAX => *i as usize,
                // A line break that was put in, or the end: the end of what was drawn before.
                _ => order.get(..drawn.min(order.len())).unwrap_or(&[]).iter().rev().find(|i| **i != u32::MAX).map_or(0, |i| *i as usize + 1),
            },
        }
    }

    /// The stretch of `job` that shows the characters `part` of the text as written (from the
    /// first drawn of them to the last).
    pub fn drawn_range(&self, part: Range<usize>) -> Range<usize> {
        let Some(order) = &self.order else { return part };
        let mut drawn = order.iter().enumerate().filter(|(_, i)| **i != u32::MAX && part.contains(&(**i as usize))).map(|(v, _)| v);
        let Some(first) = drawn.next() else { return 0..0 };
        let last = drawn.next_back().unwrap_or(first);
        first..last + 1
    }
}

/// A picture: one of the message's own, or one from the web that the owner asked for.
#[derive(Clone, Debug)]
pub struct PictureItem {
    pub data: Arc<Vec<u8>>,
    pub alt: String,
    pub link: Option<Arc<str>>,
    pub paper: Paper,
}

/// What stands where a picture is not shown: it is not the message's own (and was not asked
/// for, or has not come), or it is missing, or it cannot be decoded.
#[derive(Clone, Debug)]
pub struct PlaceholderItem {
    pub label: String,
    pub src: Arc<str>,
    pub link: Option<Arc<str>>,
    pub paper: Paper,
    /// The picture is not part of the message: showing it means downloading it.
    pub remote: bool,
    /// The label is the picture's own description, which a copy takes in its place.
    pub described: bool,
    /// What stands between that description and the text before when both are copied.
    pub join: Join,
}

/// What a layout came to, for the note above the message and for tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub texts: usize,
    pub pictures: usize,
    pub placeholders: usize,
    /// Placeholders for pictures that are not part of the message.
    pub remote: usize,
    pub tables: usize,
    /// Layout tables that were stacked for want of room.
    pub stacked: usize,
}

/// A message laid out, as far as it has been.
#[derive(Debug, Default)]
pub struct Laid {
    pub items: Vec<Item>,
    /// What can be selected, in reading order: where each such thing is in `items`.
    pub segs: Vec<u32>,
    /// The size of what is laid out so far. Wider than the width asked for when a data table needs it.
    pub size: Vec2,
    pub done: bool,
    pub stats: Stats,
    width: f32,
    /// Pictures from the web that have been downloaded are shown (the owner asked for them).
    remote: bool,
    /// Top-level blocks laid out.
    next: usize,
    y: f32,
    prev: Option<Prev>,
    /// What is to stand between the last text laid out and the next one, when they are copied.
    pend: Join,
    /// How many texts were given their [`Join`] so far.
    joined: usize,
    /// Per top-level block: `Some` when it goes on in the paragraph of the block before.
    cuts: Arc<[Option<Glue>]>,
}

impl Laid {
    /// Nothing laid out yet, for a pane `width` points wide.
    pub fn new(width: f32) -> Laid {
        let width = if width.is_finite() { width.clamp(MIN_WIDTH, WIDE) } else { MIN_WIDTH };
        Laid { width, size: vec2(width, 0.0), ..Laid::default() }
    }

    /// The same, showing the pictures from the web that have been downloaded: for a message
    /// whose owner asked for them. Without this they are placeholders, downloaded or not.
    pub fn showing_remote(self, remote: bool) -> Laid {
        Laid { remote, ..self }
    }

    /// The same, knowing which of the top-level blocks are parts of one paragraph that was cut
    /// for its length, and what was dropped at each cut: a copy puts them together again.
    pub fn cut(self, cuts: Arc<[Option<Glue>]>) -> Laid {
        Laid { cuts, ..self }
    }

    /// The `seg`th thing that can be selected.
    pub fn segment(&self, seg: usize) -> Option<&Item> {
        self.segs.get(seg).and_then(|index| self.items.get(*index as usize))
    }

    #[cfg(test)]
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Lay out more of `blocks`: until they are all done, or about `budget` characters of text
    /// were laid out in this call. `message` is where `cid:` pictures are looked up.
    pub fn advance(&mut self, ctx: &egui::Context, blocks: &[Block], message: Option<&Message>, pictures: &mut Pictures, budget: usize) {
        if self.done {
            return;
        }
        let t = Tokens::get(ctx);
        let cuts = self.cuts.clone();
        let mut cx = Cx {
            ctx,
            t,
            message,
            pictures,
            remote: self.remote,
            frame: images::frame(ctx),
            items: std::mem::take(&mut self.items),
            segs: std::mem::take(&mut self.segs),
            pend: self.pend,
            joined: self.joined,
            cuts: &cuts,
            work: 0,
            stats: self.stats,
            right: self.size.x,
        };
        let mut flow = Flow { x: 0.0, width: self.width, y: self.y, paper: Paper::window(&t), prev: self.prev.take() };
        while self.next < blocks.len() && cx.work < budget {
            self.next += place(&mut cx, &mut flow, blocks, self.next, 0).max(1);
        }
        self.y = flow.y;
        self.prev = flow.prev;
        self.items = cx.items;
        self.segs = cx.segs;
        self.pend = cx.pend;
        self.joined = cx.joined;
        self.stats = cx.stats;
        self.size = vec2(cx.right.max(self.width), flow.y);
        self.done = self.next >= blocks.len();
    }
}

/// What laying out needs at hand.
struct Cx<'a> {
    ctx: &'a egui::Context,
    t: Tokens,
    message: Option<&'a Message>,
    pictures: &'a mut Pictures,
    /// Pictures from the web are shown when they have come.
    remote: bool,
    frame: u64,
    items: Vec<Item>,
    /// Where the things that can be selected are in `items`.
    segs: Vec<u32>,
    /// What is to stand between the last text and the next when they are copied: gathered from
    /// the blocks, rows and cells that begin in between, and taken by the next text.
    pend: Join,
    /// How many texts have taken theirs.
    joined: usize,
    /// Per top-level block: `Some` when it goes on in the paragraph of the block before.
    cuts: &'a [Option<Glue>],
    /// Characters of text laid out in this call.
    work: usize,
    stats: Stats,
    /// The right edge of the widest thing so far.
    right: f32,
}

impl Cx<'_> {
    fn push(&mut self, rect: Rect, kind: ItemKind) {
        self.right = self.right.max(rect.max.x);
        self.items.push(Item { rect, kind, seg: NO_SEG });
    }

    /// Push a thing that can be selected: a text, a picture, or what stands for a picture.
    fn push_seg(&mut self, rect: Rect, kind: ItemKind) {
        let (Ok(seg), Ok(index)) = (u32::try_from(self.segs.len()), u32::try_from(self.items.len())) else {
            return self.push(rect, kind);
        };
        self.right = self.right.max(rect.max.x);
        self.segs.push(index);
        self.items.push(Item { rect, kind, seg });
    }

    /// The next text starts a new line, under `blank` empty ones. Not in a cell that has shown
    /// no text yet: the tab that leads into the cell is all there is between.
    fn line(&mut self, blank: u8) {
        if self.pend.tabs == 0 {
            self.pend = Join { lines: self.pend.lines.max(1 + blank.min(3)), ..Join::default() };
        }
    }

    /// The next text starts a row of a table, or a cell that stands under the one before.
    fn row(&mut self) {
        self.pend = Join { lines: self.pend.lines.max(1), ..Join::default() };
    }

    /// What stands between the text before and the one that is pushed now.
    fn join(&mut self) -> Join {
        self.joined += 1;
        std::mem::take(&mut self.pend)
    }

    fn fill(&mut self, rect: Rect, color: Color32) {
        self.push(rect, ItemKind::Fill { color, radius: 0.0 });
    }

    /// Reserve a place in the painting order for a background whose size is not known yet.
    fn slot(&mut self) -> usize {
        self.items.push(Item { rect: Rect::NOTHING, kind: ItemKind::Fill { color: Color32::TRANSPARENT, radius: 0.0 }, seg: NO_SEG });
        self.items.len() - 1
    }

    fn fill_slot(&mut self, slot: usize, rect: Rect, color: Color32, radius: f32) {
        self.right = self.right.max(rect.max.x);
        if let Some(item) = self.items.get_mut(slot) {
            *item = Item { rect, kind: ItemKind::Fill { color, radius }, seg: NO_SEG };
        }
    }

    fn measure(&mut self, job: LayoutJob) -> Arc<egui::Galley> {
        self.ctx.fonts_mut(|f| f.layout_job(job))
    }
}

/// What the block above was, for the gap under it and for joining quote bars.
#[derive(Clone, Copy, Debug)]
struct Prev {
    quote: u8,
    bottom: f32,
    kind: PrevKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrevKind {
    Text,
    Heading,
    List,
    Picture,
    DataTable,
    Other,
}

/// A column of blocks being laid out: the message, or a cell.
struct Flow {
    x: f32,
    width: f32,
    y: f32,
    paper: Paper,
    prev: Option<Prev>,
}

/// Lay out the blocks of a cell (or of anything inside the message); the bottom of the last one.
fn flow(cx: &mut Cx<'_>, blocks: &[Block], x: f32, y: f32, width: f32, paper: Paper, depth: usize) -> f32 {
    let mut flow = Flow { x, width, y, paper, prev: None };
    let mut i = 0;
    while i < blocks.len() {
        i += place(cx, &mut flow, blocks, i, depth).max(1);
    }
    flow.y
}

/// The space between a block and the one above it.
fn gap(prev: &Prev, block: &Block) -> f32 {
    let asked = match block.space {
        0 => 0.0,
        n => PARAGRAPH_GAP + f32::from(n.min(3) - 1) * BLANK_LINE,
    };
    let least = match (&block.kind, prev.kind) {
        (BlockKind::ListItem { .. }, PrevKind::List) => 2.0,
        (BlockKind::Heading(_), _) | (_, PrevKind::Heading) => 4.0,
        (BlockKind::Table(table), _) if !table.layout => 8.0,
        (_, PrevKind::DataTable) => 8.0,
        (BlockKind::Image { .. }, _) | (_, PrevKind::Picture) => 4.0,
        _ => 0.0,
    };
    asked.max(least)
}

/// Lay out the block at `index` (and, for a picture, the pictures that follow it in its line);
/// how many blocks that was.
fn place(cx: &mut Cx<'_>, flow: &mut Flow, blocks: &[Block], index: usize, depth: usize) -> usize {
    let Some(block) = blocks.get(index) else { return 1 };
    let quote = block.quote.min(MAX_QUOTE_LEVELS);
    let indent = (f32::from(quote) * QUOTE_INDENT).min((flow.width - MIN_WIDTH).max(0.0));
    let top = flow.y + flow.prev.as_ref().map_or(0.0, |prev| gap(prev, block));
    let x = flow.x + indent;
    let width = (flow.width - indent).max(MIN_CELL.min(flow.width));
    // What a copy writes between this block's text and the text before: nothing of its own for
    // the first block of a cell (the cell says), the dropped space for a part of a paragraph
    // that was cut, else a line break and the blank lines above the block.
    match cx.cuts.get(index).copied().flatten().filter(|_| depth == 0) {
        Some(glue) => cx.pend = Join { cont: true, glue, ..Join::default() },
        None if flow.prev.is_some() => cx.line(block.space),
        None => {}
    }
    let mut used = 1;
    let (bottom, kind) = match &block.kind {
        BlockKind::Rule => {
            let line = Rect::from_min_size(pos2(x, top + (RULE_HEIGHT / 2.0).floor()), vec2(width, 1.0));
            cx.fill(line, flow.paper.line(&cx.t));
            (top + RULE_HEIGHT, PrevKind::Other)
        }
        BlockKind::Image { .. } => {
            used = run_length(blocks, index);
            let run = blocks.get(index..index + used).unwrap_or_default();
            (pictures(cx, run, x, top, width, flow.paper), PrevKind::Picture)
        }
        BlockKind::Table(table) => {
            let bottom = table_block(cx, table, block.align, x, top, width, flow.paper, depth);
            (bottom, if table.layout { PrevKind::Other } else { PrevKind::DataTable })
        }
        other => {
            let kind = match other {
                BlockKind::Heading(_) => PrevKind::Heading,
                BlockKind::ListItem { .. } => PrevKind::List,
                _ => PrevKind::Text,
            };
            (text(cx, block, x, top, width, flow.paper), kind)
        }
    };
    // A bar down the left edge for each level of quoting, joined to the bar of the block above.
    for level in 0..quote {
        let from = match &flow.prev {
            Some(prev) if prev.quote > level => prev.bottom,
            _ => top,
        };
        let bar = Rect::from_min_max(
            pos2(flow.x + f32::from(level) * QUOTE_INDENT + 2.0, from),
            pos2(flow.x + f32::from(level) * QUOTE_INDENT + 4.0, bottom),
        );
        cx.fill(bar, flow.paper.line(&cx.t));
    }
    flow.prev = Some(Prev { quote, bottom, kind });
    flow.y = bottom;
    used
}

// ---- text ----

/// The size and line height of a kind of block's text.
fn type_scale(kind: &BlockKind) -> (f32, f32) {
    match kind {
        BlockKind::Heading(1) => (20.0, 1.3),
        BlockKind::Heading(2) => (17.0, 1.3),
        BlockKind::Heading(3) => (15.0, 1.35),
        BlockKind::Heading(_) => (13.5, 1.4),
        BlockKind::Preformatted => (12.0, 1.4),
        _ => (BASE_SIZE, LINE),
    }
}

fn halign(align: Align) -> egui::Align {
    match align {
        Align::Left => egui::Align::LEFT,
        Align::Center => egui::Align::Center,
        Align::Right => egui::Align::RIGHT,
    }
}

/// The font a span of a block of `kind` is set in.
fn font(span: &Span, kind: &BlockKind) -> egui::FontId {
    let (base, _) = type_scale(kind);
    let size = span.size.map_or(base, f32::from);
    if matches!(kind, BlockKind::Preformatted) {
        mono(size.min(base))
    } else if span.code {
        mono(size - 0.5)
    } else if span.bold {
        semibold(size)
    } else {
        regular(size)
    }
}

/// The spans of a block as text to lay out: formatted, coloured for the paper they are on, wrapped
/// at `wrap`. With it, the parts that are links and the number of characters.
fn text_job(
    t: &Tokens,
    spans: &[Span],
    kind: &BlockKind,
    paper: Paper,
    quoted: bool,
    wrap: f32,
    align: Align,
) -> (LayoutJob, Vec<(Range<usize>, Arc<str>)>, usize) {
    let (base, line) = type_scale(kind);
    let pre = matches!(kind, BlockKind::Preformatted);
    let mut job = LayoutJob { wrap: TextWrapping { max_width: wrap, ..TextWrapping::default() }, halign: halign(align), ..LayoutJob::default() };
    let mut links = Vec::new();
    let mut chars = 0;
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        let size = span.size.map_or(base, f32::from);
        let font = font(span, kind);
        // A highlight is the paper its text is read on.
        let behind = span.background.map_or(paper, |bg| Paper::of(bg, t));
        let ink = behind.ink(t, span.color, span.link.is_some(), quoted);
        let stroke = Stroke::new(1.0, ink);
        let background = if span.background.is_some() {
            behind.fill
        } else if span.code && !pre {
            paper.shade(t)
        } else {
            Color32::TRANSPARENT
        };
        let format = TextFormat {
            font_id: font,
            color: ink,
            italics: span.italic,
            underline: if span.underline || span.link.is_some() { stroke } else { Stroke::NONE },
            strikethrough: if span.strike { stroke } else { Stroke::NONE },
            background,
            line_height: Some((size * line).round()),
            ..TextFormat::default()
        };
        let count = span.text.chars().count();
        if let Some(url) = &span.link {
            links.push((chars..chars + count, Arc::from(url.as_str())));
        }
        chars += count;
        job.append(&span.text, 0.0, format);
    }
    (job, links, chars)
}

/// The spans cut into pieces of about [`PIECE_CHARS`] characters, at white space where there is
/// some. A line break or a space a piece ends at is dropped: the next piece starts a new line anyway.
#[cfg(test)]
pub fn pieces(spans: &[Span]) -> Vec<Vec<Span>> {
    cut(spans).into_iter().map(|(piece, _)| piece).collect()
}

/// The spans cut into pieces of about [`PIECE_CHARS`] characters, at white space where there is
/// some, each with the white space that was dropped in front of it, for a copy to put back.
pub fn cut(spans: &[Span]) -> Vec<(Vec<Span>, Glue)> {
    let total: usize = spans.iter().map(|s| s.text.len()).sum();
    if total <= PIECE_CHARS {
        return vec![(spans.to_vec(), None)];
    }
    let mut out: Vec<(Vec<Span>, Glue)> = Vec::new();
    let mut piece: Vec<Span> = Vec::new();
    let mut count = 0usize;
    // What was dropped at the cut in front of the piece that is being gathered.
    let mut dropped: Glue = None;
    for span in spans {
        let mut start = 0;
        for (at, c) in span.text.char_indices() {
            count += 1;
            let cut = (count >= PIECE_CHARS && c.is_whitespace()) || count >= PIECE_CHARS * 2;
            if !cut {
                continue;
            }
            let end = if c.is_whitespace() { at } else { at + c.len_utf8() };
            if let Some(text) = span.text.get(start..end).filter(|t| !t.is_empty()) {
                piece.push(Span { text: text.to_string(), ..span.clone() });
            }
            if !piece.is_empty() {
                out.push((std::mem::take(&mut piece), dropped));
                dropped = None;
            }
            // A space dropped where nothing was gathered yet stands in front of what follows.
            dropped = dropped.or(c.is_whitespace().then_some(c));
            start = at + c.len_utf8();
            count = 0;
        }
        if let Some(text) = span.text.get(start..).filter(|t| !t.is_empty()) {
            piece.push(Span { text: text.to_string(), ..span.clone() });
        }
    }
    if !piece.is_empty() {
        out.push((piece, dropped));
    }
    out
}

/// How far a list item's text is moved in.
fn list_indent(kind: &BlockKind, width: f32) -> f32 {
    match kind {
        BlockKind::ListItem { depth, .. } => {
            (LIST_INDENT + f32::from((*depth).min(MAX_LIST_DEPTH)) * LIST_STEP).min((width - MIN_WIDTH * 1.5).max(0.0))
        }
        _ => 0.0,
    }
}

/// A paragraph that runs right to left, laid out as it is drawn: wrapped where `job` wraps at its
/// width, each line put in the order it is drawn ([`bidi::visual_order`]), the lines one under
/// the other. With it, where each drawn character is in the text as written (`u32::MAX` for a
/// line break that was put in) and the links where they are drawn.
fn right_to_left(cx: &mut Cx<'_>, job: &LayoutJob, links: &[(Range<usize>, Arc<str>)]) -> (LayoutJob, Vec<u32>, Vec<(Range<usize>, Arc<str>)>) {
    let chars: Vec<char> = job.text.chars().collect();
    // The section each character is in.
    let mut section_of: Vec<usize> = Vec::with_capacity(chars.len());
    for (index, section) in job.sections.iter().enumerate() {
        let count = job.text.get(section.byte_range.start.0..section.byte_range.end.0).map_or(0, |t| t.chars().count());
        section_of.extend(std::iter::repeat_n(index, count));
    }
    let format_at =
        |i: usize| section_of.get(i).and_then(|s| job.sections.get(*s)).or(job.sections.last()).map(|s| s.format.clone()).unwrap_or_default();
    // The lines, as character ranges: broken where the text breaks itself, and between words
    // where a line would be wider than the room. (Not from egui's own wrapping of the text: it
    // counts more glyphs than characters in right-to-left runs.)
    let room = job.wrap.max_width;
    let mut lines: Vec<(Range<usize>, bool)> = Vec::new();
    let mut start = 0usize;
    let mut used = 0.0f32;
    let mut i = 0usize;
    while i < chars.len() {
        if chars.get(i) == Some(&'\n') {
            lines.push((start..i, true));
            i += 1;
            start = i;
            used = 0.0;
            continue;
        }
        // A word and the spaces after it.
        let from = i;
        while i < chars.len() && chars.get(i).is_some_and(|c| !c.is_whitespace()) {
            i += 1;
        }
        let word_end = i;
        while i < chars.len() && chars.get(i).is_some_and(|c| c.is_whitespace() && *c != '\n') {
            i += 1;
        }
        let measure = |cx: &mut Cx<'_>, to: usize| -> f32 {
            let text: String = chars.get(from..to).unwrap_or(&[]).iter().collect();
            if text.is_empty() {
                return 0.0;
            }
            let probe = LayoutJob {
                wrap: TextWrapping { max_width: f32::INFINITY, ..TextWrapping::default() },
                ..LayoutJob::single_section(text, format_at(from))
            };
            cx.measure(probe).size().x
        };
        let word = measure(cx, word_end);
        if used > 0.0 && used + word > room {
            lines.push((start..from, false));
            start = from;
            used = 0.0;
        }
        used += measure(cx, i);
    }
    lines.push((start..chars.len(), false));
    let mut out =
        LayoutJob { wrap: TextWrapping { max_width: f32::INFINITY, ..TextWrapping::default() }, halign: job.halign, ..LayoutJob::default() };
    let mut order: Vec<u32> = Vec::with_capacity(chars.len() + lines.len());
    let count = lines.len();
    for (n, (range, own_break)) in lines.into_iter().enumerate() {
        let line: String = chars.get(range.clone()).unwrap_or(&[]).iter().collect();
        let (drawn, map) = bidi::visual_order(&line, Direction::Rtl);
        // egui's shaper turns each run of Hebrew or Arabic letters round itself.
        let (drawn, map) = bidi::for_shaper(&drawn, &map);
        let mut buf = [0u8; 4];
        for (c, i) in drawn.chars().zip(map) {
            let written = range.start + i;
            out.append(c.encode_utf8(&mut buf), 0.0, format_at(written));
            order.push(u32::try_from(written).unwrap_or(u32::MAX));
        }
        if n + 1 < count {
            out.append("\n", 0.0, format_at(range.end.saturating_sub(1)));
            order.push(if own_break { u32::try_from(range.end).unwrap_or(u32::MAX) } else { u32::MAX });
        }
    }
    // The links, where their characters are drawn.
    let mut drawn_links: Vec<(Range<usize>, Arc<str>)> = Vec::new();
    for (v, written) in order.iter().enumerate() {
        let link = links.iter().find(|(r, _)| r.contains(&(*written as usize))).map(|(_, url)| url.clone());
        match (drawn_links.last_mut(), link) {
            (Some((r, url)), Some(link)) if r.end == v && Arc::ptr_eq(url, &link) => r.end = v + 1,
            (_, Some(link)) => drawn_links.push((v..v + 1, link)),
            _ => {}
        }
    }
    (out, order, drawn_links)
}

/// Lay out a paragraph, a heading, a list item or preformatted text; its bottom.
fn text(cx: &mut Cx<'_>, block: &Block, x: f32, y: f32, width: f32, paper: Paper) -> f32 {
    // Text that runs right to left (by its first letter) sits against the right edge, its list
    // markers on the right.
    let rtl = !matches!(block.kind, BlockKind::Preformatted) && block.spans.iter().find_map(|s| bidi::direction(&s.text)) == Some(Direction::Rtl);
    let align = if rtl && block.align == Align::Left { Align::Right } else { block.align };
    let mut indent = list_indent(&block.kind, width);
    let left = x;
    // A long marker (item 4294967295 of a list) makes room for itself rather than hang out of the list.
    if let BlockKind::ListItem { marker, .. } = &block.kind
        && let Some(label) = marker_label(marker)
    {
        let size = block.spans.first().and_then(|s| s.size).map_or(BASE_SIZE, f32::from);
        let wide = cx.ctx.fonts_mut(|f| f.layout_no_wrap(label, regular(size), Color32::WHITE).size().x) + 10.0;
        indent = indent.max(wide).min((width - MIN_WIDTH * 1.5).max(0.0));
    }
    let (x, width) = if rtl { (x, (width - indent).max(MIN_CELL.min(width))) } else { (x + indent, (width - indent).max(MIN_CELL.min(width))) };
    let quoted = block.quote > 0;
    let mut y = y;
    // Whether a piece of the block has been pushed: the next one goes on in its paragraph.
    let mut begun = false;
    for (n, (piece, glue)) in cut(&block.spans).iter().enumerate() {
        let (job, links, chars) = text_job(&cx.t, piece, &block.kind, paper, quoted, width, align);
        if chars == 0 {
            continue;
        }
        let (job, links, chars, order, logical) = if rtl {
            let logical: Arc<str> = Arc::from(job.text.as_str());
            let (drawn, order, links) = right_to_left(cx, &job, &links);
            (drawn, links, chars, Some(Arc::from(order)), Some(logical))
        } else {
            (job, links, chars, None, None)
        };
        let galley = cx.measure(job.clone());
        let height = galley.size().y;
        if n == 0
            && let BlockKind::ListItem { marker, depth } = &block.kind
        {
            let row = galley.rows.first().map_or(height, |r| r.height());
            let first = piece.first();
            let size = first.and_then(|s| s.size).map_or(BASE_SIZE, f32::from);
            let ink = paper.ink(&cx.t, first.and_then(|s| s.color).filter(|_| first.is_some_and(|s| s.link.is_none())), false, quoted);
            if rtl {
                list_marker_right(cx, marker, *depth, pos2(x + width, y), x + width + indent, row, size, ink, paper.fill);
            } else {
                list_marker(cx, marker, *depth, pos2(x, y), left, row, size, ink, paper.fill);
            }
        }
        let anchor = match align {
            Align::Left => 0.0,
            Align::Center => width / 2.0,
            Align::Right => width,
        };
        let join = if begun { Join { cont: true, glue: *glue, ..Join::default() } } else { cx.join() };
        let lead = match &block.kind {
            BlockKind::ListItem { marker, depth } if !begun => copied_marker(marker, *depth),
            _ => String::new(),
        };
        begun = true;
        let item = TextItem {
            job: Arc::new(job),
            anchor,
            links,
            paper: paper.fill,
            chars: u32::try_from(chars).unwrap_or(u32::MAX),
            join,
            lead,
            order,
            logical,
        };
        cx.push_seg(Rect::from_min_size(pos2(x, y), vec2(width, height)), ItemKind::Text(item));
        cx.work += chars;
        cx.stats.texts += 1;
        y += height;
    }
    y
}

/// A marker that is written out: a number or a label.
fn marker_label(marker: &Marker) -> Option<String> {
    match marker {
        Marker::Number(n) => Some(format!("{n}.")),
        Marker::Label(label) => Some(label.chars().take(12).collect()),
        Marker::Bullet | Marker::None => None,
    }
}

/// A list item's marker as a copy writes it in front of the item, a tab between the two (the
/// way a word processor writes a list as plain text). A bullet is the sign that is painted for
/// its depth: a disc, a ring or a square.
fn copied_marker(marker: &Marker, depth: u8) -> String {
    match marker_label(marker) {
        Some(label) => format!("{label}\t"),
        None if matches!(marker, Marker::Bullet) => {
            let sign = match depth % 3 {
                0 => '\u{2022}',
                1 => '\u{25e6}',
                _ => '\u{25aa}',
            };
            format!("{sign}\t")
        }
        None => String::new(),
    }
}

/// What stands in front of a list item whose text starts at `at`: a bullet or a number, never
/// further left than `left`.
fn list_marker(cx: &mut Cx<'_>, marker: &Marker, depth: u8, at: egui::Pos2, left: f32, row: f32, size: f32, ink: Color32, paper: Color32) {
    let Some(label) = marker_label(marker) else {
        if matches!(marker, Marker::Bullet) {
            let r = (size * 0.19).clamp(2.0, 4.0);
            let center = pos2((at.x - 11.0).max(left + r), at.y + row / 2.0);
            cx.push(Rect::from_center_size(center, Vec2::splat(r * 2.0)), ItemKind::Bullet { depth, color: ink });
        }
        return;
    };
    let format = TextFormat { font_id: regular(size), color: ink, line_height: Some((size * LINE).round()), ..TextFormat::default() };
    let job = LayoutJob { wrap: TextWrapping { max_width: f32::INFINITY, ..TextWrapping::default() }, ..LayoutJob::single_section(label, format) };
    let galley = cx.measure(job.clone());
    let rect = Rect::from_min_size(pos2((at.x - 6.0 - galley.size().x).max(left), at.y), galley.size());
    // Not a text of the message: it is not selected, and a copy writes it with its item.
    let number = TextItem {
        job: Arc::new(job),
        anchor: 0.0,
        links: Vec::new(),
        paper,
        chars: 0,
        join: Join::default(),
        lead: String::new(),
        order: None,
        logical: None,
    };
    cx.push(rect, ItemKind::Text(number));
}

/// The same for a list item that runs right to left, whose text ends at `at` (its right edge):
/// the marker on its right, never further right than `right`.
#[allow(clippy::too_many_arguments)]
fn list_marker_right(cx: &mut Cx<'_>, marker: &Marker, depth: u8, at: egui::Pos2, right: f32, row: f32, size: f32, ink: Color32, paper: Color32) {
    let Some(label) = marker_label(marker) else {
        if matches!(marker, Marker::Bullet) {
            let r = (size * 0.19).clamp(2.0, 4.0);
            let center = pos2((at.x + 11.0).min(right - r), at.y + row / 2.0);
            cx.push(Rect::from_center_size(center, Vec2::splat(r * 2.0)), ItemKind::Bullet { depth, color: ink });
        }
        return;
    };
    // `1.` reads `.1` from the right.
    let label: String = bidi::visual_order(&label, Direction::Rtl).0;
    let format = TextFormat { font_id: regular(size), color: ink, line_height: Some((size * LINE).round()), ..TextFormat::default() };
    let job = LayoutJob { wrap: TextWrapping { max_width: f32::INFINITY, ..TextWrapping::default() }, ..LayoutJob::single_section(label, format) };
    let galley = cx.measure(job.clone());
    let left = (at.x + 6.0).min(right - galley.size().x);
    let rect = Rect::from_min_size(pos2(left, at.y), galley.size());
    let number = TextItem {
        job: Arc::new(job),
        anchor: 0.0,
        links: Vec::new(),
        paper,
        chars: 0,
        join: Join::default(),
        lead: String::new(),
        order: None,
        logical: None,
    };
    cx.push(rect, ItemKind::Text(number));
}

// ---- pictures ----

/// How many pictures stand in one line from `index`: those that follow the first directly.
fn run_length(blocks: &[Block], index: usize) -> usize {
    let Some(first) = blocks.get(index) else { return 1 };
    let follows = |b: &&Block| matches!(b.kind, BlockKind::Image { .. }) && b.space == 0 && b.quote == first.quote && b.align == first.align;
    1 + blocks.iter().skip(index + 1).take(MAX_RUN - 1).take_while(follows).count()
}

/// A picture to place: its size here, and what is shown.
struct Tile<'a> {
    size: Vec2,
    block: &'a Block,
    /// The picture's bytes when it is one of the message's own and can be shown.
    data: Option<Arc<Vec<u8>>>,
    label: String,
    /// The label is the picture's own description, not a word of ours for want of one.
    described: bool,
    /// The size the picture asks for, or has: what a table column is sized by.
    wanted: f32,
}

/// What a picture block comes to in `room` points of width. `shown` counts it; measuring a column does not.
fn tile<'a>(cx: &mut Cx<'_>, block: &'a Block, room: f32, shown: bool) -> Tile<'a> {
    let BlockKind::Image { src, width, height, .. } = &block.kind else {
        return Tile { size: Vec2::ZERO, block, data: None, label: String::new(), described: false, wanted: 0.0 };
    };
    let alt = block.text();
    let alt = alt.trim();
    // One of the message's own; or one from the web, when the owner asked and it has come.
    let asked_for = cx.remote && images::is_remote(src);
    let mut bytes = cx.message.and_then(|m| images::attachment(m, src)).map(|a| a.data.clone());
    if bytes.is_none()
        && asked_for
        && let Some(images::Remote::Ready(data)) = cx.pictures.remote(src.trim(), cx.frame)
    {
        bytes = Some(data);
    }
    let bytes = bytes.filter(|_| cx.stats.pictures < images::MAX_PICTURES_PER_MESSAGE);
    let natural = bytes.and_then(|data| cx.pictures.size(&data, cx.frame).map(|size| (data, size)));
    let asked = |v: &Option<u16>| v.map(f32::from).filter(|v| *v >= 1.0);
    if let Some((data, [w, h])) = natural {
        let (w, h) = (w as f32, h as f32);
        let mut size = match (asked(width), asked(height)) {
            (Some(aw), Some(ah)) => vec2(aw, ah),
            (Some(aw), None) => vec2(aw, h * aw / w),
            (None, Some(ah)) => vec2(w * ah / h, ah),
            (None, None) => vec2(w, h),
        };
        let wanted = size.x.min(super::MAX_BODY_WIDTH);
        // Never wider than the room there is, nor absurdly tall; the shape is kept.
        let scale = (room / size.x).min(MAX_PICTURE_HEIGHT / size.y).min(1.0);
        size = vec2((size.x * scale).max(1.0), (size.y * scale).max(1.0));
        if shown {
            cx.stats.pictures += 1;
        }
        return Tile { size, block, data: Some(data), label: alt.to_string(), described: !alt.is_empty(), wanted };
    }
    // Not shown: a stand-in the size of its description, or just a mark for a small picture without one.
    let small = width.is_some_and(|w| w <= 48) || height.is_some_and(|h| h <= 48);
    let described = !alt.is_empty();
    let label = if described {
        alt.chars().take(120).collect()
    } else if small {
        String::new()
    } else {
        tl!("Picture").to_string()
    };
    let text = if label.is_empty() {
        0.0
    } else {
        cx.ctx.fonts_mut(|f| f.layout_no_wrap(label.clone(), regular(CHIP_FONT), Color32::WHITE).size().x) + 10.0
    };
    if shown {
        cx.stats.placeholders += 1;
        if !mailcraft_html::is_embedded(src) {
            cx.stats.remote += 1;
        }
    }
    // One that was asked for and is on its way keeps the room it says it needs, so that the
    // message does not jump when it comes.
    if let (true, Some(w), Some(h)) = (asked_for, asked(width), asked(height)) {
        let scale = (room / w).min(MAX_PICTURE_HEIGHT / h).min(1.0);
        let size = vec2((w * scale).max(1.0), (h * scale).max(1.0));
        return Tile { size, block, data: None, label, described, wanted: w.min(super::MAX_BODY_WIDTH) };
    }
    let wanted = CHIP_HEIGHT + text;
    Tile { size: vec2(wanted.min(room.max(CHIP_HEIGHT)), CHIP_HEIGHT), block, data: None, label, described, wanted }
}

/// Lay out pictures that stand in one line, wrapping to more lines as needed; the bottom.
fn pictures(cx: &mut Cx<'_>, run: &[Block], x: f32, y: f32, width: f32, paper: Paper) -> f32 {
    let tiles: Vec<Tile<'_>> = run.iter().map(|b| tile(cx, b, width, true)).collect();
    let align = run.first().map_or(Align::Left, |b| b.align);
    let mut y = y;
    let mut start = 0;
    // Whether a description of this run was already given its place in a copy.
    let mut said = false;
    while start < tiles.len() {
        // As many as fit in this line, at least one.
        let mut end = start;
        let mut line = 0.0f32;
        let mut height = 0.0f32;
        while let Some(tile) = tiles.get(end) {
            let wider = line + tile.size.x + if end > start { PICTURE_GAP } else { 0.0 };
            if end > start && wider > width + 0.5 {
                break;
            }
            line = wider;
            height = height.max(tile.size.y);
            end += 1;
        }
        let mut at = x + match align {
            Align::Left => 0.0,
            Align::Center => ((width - line) / 2.0).max(0.0),
            Align::Right => (width - line).max(0.0),
        };
        for tile in tiles.get(start..end).unwrap_or_default() {
            let BlockKind::Image { src, link, .. } = &tile.block.kind else { continue };
            let rect = Rect::from_min_size(pos2(at, y + height - tile.size.y), tile.size);
            let link = link.as_deref().map(Arc::from);
            match &tile.data {
                Some(data) => cx.push_seg(rect, ItemKind::Picture(PictureItem { data: data.clone(), alt: tile.label.clone(), link, paper })),
                None => {
                    let remote = !mailcraft_html::is_embedded(src);
                    // A picture that is not shown is copied as its description, when it has one;
                    // a space stands between two of those in a line.
                    let described = tile.described && !tile.label.is_empty();
                    let join = match described {
                        true if said => Join { glue: Some(' '), ..cx.join() },
                        true => cx.join(),
                        false => Join::default(),
                    };
                    said |= described;
                    let label = tile.label.clone();
                    let stand_in = PlaceholderItem { label, src: Arc::from(src.as_str()), link, paper, remote, described, join };
                    cx.push_seg(rect, ItemKind::Placeholder(stand_in));
                }
            }
            at += tile.size.x + PICTURE_GAP;
        }
        y += height + if end < tiles.len() { PICTURE_GAP } else { 0.0 };
        start = end.max(start + 1);
    }
    y
}

// ---- how wide things want to be ----

/// The narrowest a thing can be made without breaking words, and the width it takes when it has
/// all the room it wants.
#[derive(Clone, Copy, Debug, Default)]
struct Extent {
    min: f32,
    max: f32,
}

fn blocks_extent(cx: &mut Cx<'_>, blocks: &[Block], depth: usize) -> Extent {
    let mut out = Extent::default();
    let mut i = 0;
    while let Some(block) = blocks.get(i) {
        let indent = f32::from(block.quote.min(MAX_QUOTE_LEVELS)) * QUOTE_INDENT;
        let mut used = 1;
        let extent = match &block.kind {
            BlockKind::Rule => Extent::default(),
            BlockKind::Image { .. } => {
                used = run_length(blocks, i);
                let mut extent = Extent::default();
                for (n, picture) in blocks.iter().skip(i).take(used).enumerate() {
                    let tile = tile(cx, picture, WIDE, false);
                    // A picture that asks for a size keeps it as long as it can; any other gives way.
                    let least = if tile.data.is_some() && matches!(picture.kind, BlockKind::Image { width: Some(_), .. }) {
                        tile.wanted.min(160.0)
                    } else {
                        tile.wanted.min(48.0)
                    };
                    extent.min = extent.min.max(least);
                    extent.max += tile.wanted + if n > 0 { PICTURE_GAP } else { 0.0 };
                }
                extent
            }
            BlockKind::Table(table) => table_extent(cx, table, depth),
            _ => text_extent(cx, block),
        };
        out.min = out.min.max(extent.min + indent);
        out.max = out.max.max(extent.max + indent);
        i += used.max(1);
    }
    Extent { min: out.min.min(WIDE), max: out.max.min(WIDE) }
}

fn text_extent(cx: &mut Cx<'_>, block: &Block) -> Extent {
    let indent = list_indent(&block.kind, WIDE);
    let chars: usize = block.spans.iter().map(|s| s.text.len()).sum();
    if chars > PIECE_CHARS {
        // Too long to look at closely: it fills whatever it is given.
        return Extent { min: (MAX_WORD + indent).min(WIDE), max: WIDE };
    }
    // The widest word and the longest line, from the widths of their glyphs.
    let (word, line) = cx.ctx.fonts_mut(|fonts| {
        let (mut word, mut widest_word, mut line, mut longest_line) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for span in &block.spans {
            let font = font(span, &block.kind);
            for c in span.text.chars() {
                if c == '\n' {
                    (word, line) = (0.0, 0.0);
                    continue;
                }
                let width = fonts.glyph_width(&font, c);
                line += width;
                word = if c.is_whitespace() { 0.0 } else { word + width };
                widest_word = widest_word.max(word);
                longest_line = longest_line.max(line);
            }
        }
        (widest_word, longest_line)
    });
    let max = if chars <= MEASURED_CHARS {
        // Short enough to shape: a cell's text must not wrap for want of half a point.
        let (job, ..) = text_job(&cx.t, &block.spans, &block.kind, Paper::window(&cx.t), false, f32::INFINITY, Align::Left);
        cx.measure(job).size().x.max(line) + 1.0
    } else {
        line + 2.0
    };
    // A long word is broken rather than given a column of its own width.
    let min = (word + 2.0).min(MAX_WORD).min(max);
    Extent { min: (min + indent).min(WIDE), max: (max + indent).min(WIDE) }
}

// ---- tables ----

/// A cell and where it sits in the grid.
struct Placed<'a> {
    cell: &'a Cell,
    row: usize,
    col: usize,
    cols: usize,
    rows: usize,
}

struct Grid<'a> {
    cells: Vec<Placed<'a>>,
    columns: usize,
    rows: usize,
}

/// Give every cell its place, minding the cells that span rows and columns. A cell that finds no
/// room left in its row goes on in a new one: nothing is ever dropped or laid over another.
fn grid(table: &Table) -> Grid<'_> {
    let columns = usize::from(table.columns).clamp(1, mailcraft_html::MAX_TABLE_COLUMNS);
    // Per column, how many rows a cell from above still covers.
    let mut covered = vec![0usize; columns];
    let mut cells = Vec::new();
    let mut row = 0usize;
    for source in &table.rows {
        let mut col = 0usize;
        for cell in &source.cells {
            loop {
                while covered.get(col).is_some_and(|c| *c > 0) {
                    col += 1;
                }
                if col < columns {
                    break;
                }
                for c in &mut covered {
                    *c = c.saturating_sub(1);
                }
                row += 1;
                col = 0;
            }
            let wanted = usize::from(cell.colspan).clamp(1, columns - col);
            let cols = covered.iter().skip(col).take(wanted).take_while(|c| **c == 0).count().max(1);
            let rows = usize::from(cell.rowspan).max(1);
            for c in covered.iter_mut().skip(col).take(cols) {
                *c = rows;
            }
            cells.push(Placed { cell, row, col, cols, rows });
            col += cols;
        }
        for c in &mut covered {
            *c = c.saturating_sub(1);
        }
        row += 1;
    }
    for placed in &mut cells {
        placed.rows = placed.rows.min(row.saturating_sub(placed.row)).max(1);
    }
    Grid { cells, columns, rows: row }
}

/// Whether the table itself is painted: a background, or lines.
fn painted(table: &Table) -> bool {
    table.background.is_some() || table.border || table.outline
}

/// The room between a cell's edge and what is in it.
fn padding(table: &Table, cell: &Cell) -> Vec2 {
    if !table.layout {
        DATA_PADDING
    } else if cell.blocks.is_empty() {
        // A cell that is only a colour or a line: a band or a divider.
        vec2(0.0, 3.0)
    } else if painted(table) || cell.background.is_some() || cell.border.any() {
        BOX_PADDING
    } else {
        Vec2::ZERO
    }
}

/// The gaps between the columns and between the rows.
fn gaps(table: &Table) -> Vec2 {
    if !table.layout || painted(table) {
        return Vec2::ZERO;
    }
    // Rows of pictures and nothing else are slices of one picture: each stands right on the
    // one below, as the message has them, or the picture falls apart into stripes.
    let picture = |block: &Block| matches!(block.kind, BlockKind::Image { .. });
    let slices = table.rows.len() >= 2 && table.rows.iter().all(|row| row.cells.iter().all(|cell| cell.blocks.iter().all(picture)));
    vec2(COLUMN_GAP, if slices { 0.0 } else { ROW_GAP })
}

/// What each column needs: its least and its wanted width, and whether the source fixed it.
struct Columns {
    min: Vec<f32>,
    max: Vec<f32>,
    fixed: Vec<bool>,
    /// The share of the table's width a column asks for, in percent; 0 for none.
    share: Vec<f32>,
}

fn columns(cx: &mut Cx<'_>, table: &Table, grid: &Grid<'_>, depth: usize) -> Columns {
    let n = grid.columns;
    let mut out = Columns { min: vec![0.0; n], max: vec![0.0; n], fixed: vec![false; n], share: vec![0.0; n] };
    let gap = gaps(table).x;
    let mut spanning = Vec::new();
    for placed in &grid.cells {
        let pad = padding(table, placed.cell).x * 2.0;
        let extent = blocks_extent(cx, &placed.cell.blocks, depth + 1);
        // A cell with something in it gets the least room anything is laid out in.
        let least = if placed.cell.blocks.is_empty() { 0.0 } else { MIN_CELL };
        let (min, mut max) = (extent.min.max(least) + pad, extent.max.max(least) + pad);
        if placed.cols > 1 {
            spanning.push((placed.col, placed.cols, min, max));
            continue;
        }
        match placed.cell.width {
            Some(Length::Px(w)) => {
                max = f32::from(w).max(min);
                if let Some(fixed) = out.fixed.get_mut(placed.col) {
                    *fixed = true;
                }
            }
            Some(Length::Percent(p)) => {
                if let Some(share) = out.share.get_mut(placed.col) {
                    *share = share.max(f32::from(p));
                }
            }
            None => {}
        }
        if let (Some(lo), Some(hi)) = (out.min.get_mut(placed.col), out.max.get_mut(placed.col)) {
            *lo = lo.max(min);
            *hi = hi.max(max);
        }
    }
    // The columns a cell spans hold it together: what they lack is shared out among them.
    for (col, cols, min, max) in spanning {
        let between = gap * (cols - 1) as f32;
        for (values, need) in [(&mut out.min, min - between), (&mut out.max, max - between)] {
            let have: f32 = values.iter().skip(col).take(cols).sum();
            if need > have {
                let each = (need - have) / cols as f32;
                for v in values.iter_mut().skip(col).take(cols) {
                    *v += each;
                }
            }
        }
    }
    for (lo, hi) in out.min.iter_mut().zip(&mut out.max) {
        *lo = lo.min(WIDE);
        *hi = hi.max(*lo).min(WIDE);
    }
    out
}

fn table_extent(cx: &mut Cx<'_>, table: &Table, depth: usize) -> Extent {
    if depth >= MAX_NESTING {
        return Extent { min: MIN_WIDTH, max: WIDE };
    }
    let grid = grid(table);
    let columns = columns(cx, table, &grid, depth);
    let between = gaps(table).x * grid.columns.saturating_sub(1) as f32;
    let mut extent = Extent { min: columns.min.iter().sum::<f32>() + between, max: columns.max.iter().sum::<f32>() + between };
    // A layout table can be stacked: then it needs no more than its widest column.
    if table.layout && grid.columns > 1 {
        extent.min = columns.min.iter().copied().fold(0.0, f32::max);
    }
    match table.width {
        Some(Length::Px(w)) => extent.max = f32::from(w).max(extent.min),
        Some(Length::Percent(_)) => extent.max = WIDE,
        None => {}
    }
    Extent { min: extent.min.min(WIDE), max: extent.max.clamp(extent.min.min(WIDE), WIDE) }
}

/// The width of each column in `room` points, and of the table; `None` when a layout table's
/// columns would be too narrow and it is to be stacked.
fn column_widths(cx: &mut Cx<'_>, table: &Table, grid: &Grid<'_>, room: f32, depth: usize) -> Option<(Vec<f32>, f32)> {
    let n = grid.columns;
    let between = gaps(table).x * n.saturating_sub(1) as f32;
    let asked = match table.width {
        Some(Length::Px(w)) => Some(f32::from(w).min(room)),
        Some(Length::Percent(p)) => Some(room * f32::from(p.min(100)) / 100.0),
        None => None,
    };
    // One column that is told its width needs no measuring.
    if let (1, Some(width)) = (n, asked) {
        let width = width.max(MIN_CELL.min(room));
        return Some((vec![width], width));
    }
    let mut columns = columns(cx, table, grid, depth);
    let least: f32 = columns.min.iter().sum::<f32>() + between;
    let most: f32 = columns.max.iter().sum::<f32>() + between;
    let mut total = asked.unwrap_or(most.min(room));
    if table.layout {
        if n > 1 && (least > room + 0.5 || room / (n as f32) < MIN_LAYOUT_COLUMN) {
            return None;
        }
        total = total.min(room).max(least.min(room));
    } else {
        // A data table is never squeezed below its words: the pane scrolls sideways instead.
        total = total.max(least);
    }
    let inner = (total - between).max(0.0);
    // A column that asks for a share of the width gets it, as far as its words allow.
    for ((share, max), (min, fixed)) in columns.share.iter().zip(&mut columns.max).zip(columns.min.iter().zip(&mut columns.fixed)) {
        if *share > 0.0 {
            *max = (inner * share / 100.0).max(*min);
            *fixed = true;
        }
    }
    let (sum_min, sum_max): (f32, f32) = (columns.min.iter().sum(), columns.max.iter().sum());
    let widths: Vec<f32> = if sum_max <= inner {
        // Room to spare goes to the columns that were not told their width, by what they hold.
        let spare = inner - sum_max;
        let flexible: f32 = columns.max.iter().zip(&columns.fixed).filter(|(_, fixed)| !**fixed).map(|(max, _)| *max).sum();
        columns
            .max
            .iter()
            .zip(&columns.fixed)
            .map(|(max, fixed)| {
                if flexible > 0.0 {
                    if *fixed { *max } else { max + spare * max / flexible }
                } else if sum_max > 0.0 {
                    max + spare * max / sum_max
                } else {
                    inner / n as f32
                }
            })
            .collect()
    } else if sum_min <= inner {
        let stretch = (sum_max - sum_min).max(1e-3);
        columns.min.iter().zip(&columns.max).map(|(min, max)| min + (inner - sum_min) * (max - min) / stretch).collect()
    } else {
        columns.min.iter().map(|min| min * inner / sum_min.max(1e-3)).collect()
    };
    let total = widths.iter().sum::<f32>() + between;
    Some((widths, total))
}

/// The lines a cell asks for, as thin rectangles on the edges of `rect`. A line between two cells
/// straddles their edge, so the lines of both are one line, not two; a line on an edge of the
/// table (`outer`) lies inside it, so the table is no larger for its lines.
fn cell_lines(cx: &mut Cx<'_>, rect: Rect, sides: Sides, outer: Sides, color: Color32) {
    let edge = |at: f32, outer: bool, low: bool| match (outer, low) {
        (false, _) => (at - 0.5, at + 0.5),
        (true, true) => (at, at + 1.0),
        (true, false) => (at - 1.0, at),
    };
    let (left, _) = edge(rect.min.x, outer.left, true);
    let (_, right) = edge(rect.max.x, outer.right, false);
    let (top, _) = edge(rect.min.y, outer.top, true);
    let (_, bottom) = edge(rect.max.y, outer.bottom, false);
    if sides.top {
        let (from, to) = edge(rect.min.y, outer.top, true);
        cx.fill(Rect::from_min_max(pos2(left, from), pos2(right, to)), color);
    }
    if sides.bottom {
        let (from, to) = edge(rect.max.y, outer.bottom, false);
        cx.fill(Rect::from_min_max(pos2(left, from), pos2(right, to)), color);
    }
    if sides.left {
        let (from, to) = edge(rect.min.x, outer.left, true);
        cx.fill(Rect::from_min_max(pos2(from, top), pos2(to, bottom)), color);
    }
    if sides.right {
        let (from, to) = edge(rect.max.x, outer.right, false);
        cx.fill(Rect::from_min_max(pos2(from, top), pos2(to, bottom)), color);
    }
}

/// The paper a cell's content is read on, and whether the cell paints it.
fn cell_paper(cx: &Cx<'_>, table: &Table, cell: &Cell, around: Paper) -> (Paper, bool) {
    match cell.background {
        Some(bg) => (Paper::of(bg, &cx.t), true),
        // A header cell of a data table is shaded a little when the message gives it no colour.
        None if cell.header && !table.layout => (Paper { fill: around.shade(&cx.t), own: around.own }, true),
        None => (around, false),
    }
}

/// What a copy writes between the texts of a table's row, kept up while the row's cells are
/// laid out: a line break in front of each row after the first (the table's own block said what
/// stands in front of that one), and tabs between its cells.
struct Across {
    /// What stood in front of the row, and how many texts had their place in a copy then.
    began: (Join, usize),
    /// The same as of the row's last text, and the cells crossed since.
    before: Join,
    texts: usize,
    tabs: u8,
    first: bool,
    /// The table only places things: a cell without text (an icon, a gap) counts for nothing.
    layout: bool,
    /// The cells are told apart by tabs: always in a data table; in one that only places things,
    /// when no cell of the row holds more than a line (a label and its value, a row of buttons).
    /// Cells that hold more are columns of a page: each is copied as lines of its own.
    tabbed: bool,
}

impl Across {
    fn begin<'a>(cx: &mut Cx<'_>, table: &Table, first_row: bool, mut cells: impl Iterator<Item = &'a Cell>) -> Across {
        if !first_row {
            cx.row();
        }
        let one_line = |cell: &Cell| cell.blocks.len() <= 1 && !cell.blocks.iter().any(|b| matches!(b.kind, BlockKind::Table(_)));
        let tabbed = !table.layout || cells.all(one_line);
        Across { began: (cx.pend, cx.joined), before: cx.pend, texts: cx.joined, tabs: 0, first: true, layout: table.layout, tabbed }
    }

    /// The next cell of the row is about to be laid out.
    fn cell(&mut self, cx: &mut Cx<'_>) {
        if std::mem::take(&mut self.first) {
            return;
        }
        if !self.tabbed {
            return cx.row();
        }
        if cx.joined != self.texts {
            (self.before, self.texts, self.tabs) = (Join::default(), cx.joined, 0);
        }
        self.tabs = match self.layout {
            // One tab between two texts of the row, and none in front of its first.
            true => u8::from(cx.joined != self.began.1),
            false => self.tabs.saturating_add(1).min(MAX_TABS),
        };
        cx.pend = Join { lines: self.before.lines, tabs: self.before.tabs.saturating_add(self.tabs).min(MAX_TABS), ..Join::default() };
    }

    /// The row is laid out. One without text leaves nothing behind, nor do the cells without
    /// text at the end of one.
    fn end(self, cx: &mut Cx<'_>) {
        if cx.joined == self.began.1 {
            cx.pend = self.began.0;
        } else {
            cx.pend.tabs = 0;
        }
    }
}

/// Lay out a table; its bottom.
fn table_block(cx: &mut Cx<'_>, table: &Table, align: Align, x: f32, y: f32, room: f32, paper: Paper, depth: usize) -> f32 {
    cx.stats.tables += 1;
    let grid = grid(table);
    let widths = if depth >= MAX_NESTING { None } else { column_widths(cx, table, &grid, room, depth) };
    let Some((widths, total)) = widths else {
        cx.stats.stacked += 1;
        return stacked(cx, table, x, y, room, paper, depth);
    };
    let x = x + match align {
        Align::Left => 0.0,
        Align::Center => ((room - total) / 2.0).max(0.0),
        Align::Right => (room - total).max(0.0),
    };
    let around = table.background.map_or(paper, |bg| Paper::of(bg, &cx.t));
    let table_slot = table.background.map(|_| cx.slot());
    let gap = gaps(table);
    // The left edge of each column, and the right edge of the last.
    let mut edges = Vec::with_capacity(widths.len() + 1);
    let mut at = x;
    for width in &widths {
        edges.push(at);
        at += width + gap.x;
    }
    let span = |col: usize, cols: usize| -> (f32, f32) {
        let left = edges.get(col).copied().unwrap_or(x);
        let last = col + cols.max(1) - 1;
        let right = edges.get(last).copied().unwrap_or(left) + widths.get(last).copied().unwrap_or(0.0);
        (left, right.max(left))
    };
    // Rows from the top; a cell that spans rows ends with the last of them.
    let mut tops = Vec::with_capacity(grid.rows);
    let mut bottoms: Vec<f32> = Vec::with_capacity(grid.rows);
    let mut done: Vec<(usize, Option<usize>, Paper, f32)> = Vec::with_capacity(grid.cells.len());
    // The cells of the row being laid out: which, the items they pushed, and their bottom.
    let mut row_items: Vec<(usize, Range<usize>, f32)> = Vec::new();
    let mut next = 0;
    let mut top = y;
    for row in 0..grid.rows {
        tops.push(top);
        let cells = grid.cells.iter().skip(next).take_while(|p| p.row == row).map(|p| p.cell);
        let mut across = Across::begin(cx, table, row == 0, cells);
        while let Some(placed) = grid.cells.get(next).filter(|p| p.row == row) {
            across.cell(cx);
            let (left, right) = span(placed.col, placed.cols);
            let pad = padding(table, placed.cell);
            let (inside, own) = cell_paper(cx, table, placed.cell, around);
            let slot = own.then(|| cx.slot());
            let width = (right - left - pad.x * 2.0).max(MIN_CELL.min(right - left));
            let first_item = cx.items.len();
            let bottom = flow(cx, &placed.cell.blocks, left + pad.x.min((right - left) / 2.0), top + pad.y, width, inside, depth + 1) + pad.y;
            row_items.push((next, first_item..cx.items.len(), bottom));
            done.push((next, slot, inside, bottom));
            next += 1;
        }
        across.end(cx);
        let bottom = done
            .iter()
            .filter_map(|(index, _, _, bottom)| grid.cells.get(*index).filter(|p| p.row + p.rows - 1 == row).map(|_| *bottom))
            .fold(top, f32::max);
        // A cell of a data table holds its content in the middle of the row's height, as a
        // `<td>` does when nothing says otherwise. (A layout table's cells stay at the top: they
        // are mostly written `valign="top"`, which is not read.)
        // A row taller than a screen keeps them at the top, where they can be found.
        if !table.layout && bottom - top <= MAX_CENTRED_ROW {
            for (index, items, cell_bottom) in row_items.drain(..) {
                let single = grid.cells.get(index).is_some_and(|p| p.rows == 1);
                let dy = ((bottom - cell_bottom) / 2.0).floor();
                if single && dy > 0.0 {
                    for item in cx.items.get_mut(items).into_iter().flatten() {
                        if item.rect.is_finite() {
                            item.rect = item.rect.translate(vec2(0.0, dy));
                        }
                    }
                }
            }
        }
        row_items.clear();
        bottoms.push(bottom);
        top = bottom + gap.y;
    }
    let bottom = bottoms.last().copied().unwrap_or(y);
    let line = around.line(&cx.t);
    let single = grid.cells.len() == 1 && table.width.is_none();
    for (index, slot, inside, _) in done {
        let Some(placed) = grid.cells.get(index) else { continue };
        let (left, right) = span(placed.col, placed.cols);
        let (Some(top), Some(low)) = (tops.get(placed.row), bottoms.get(placed.row + placed.rows - 1)) else { continue };
        let rect = Rect::from_min_max(pos2(left, *top), pos2(right, low.max(*top)));
        if let Some(slot) = slot {
            // A lone cell with a colour and no width of its own is a button.
            cx.fill_slot(slot, rect, inside.fill, if single { 4.0 } else { 0.0 });
        }
        let sides = if table.border { Sides::ALL } else { placed.cell.border };
        let outer = Sides {
            top: placed.row == 0,
            left: placed.col == 0,
            right: placed.col + placed.cols >= grid.columns,
            bottom: placed.row + placed.rows >= grid.rows,
        };
        cell_lines(cx, rect, sides, outer, line);
    }
    let whole = Rect::from_min_max(pos2(x, y), pos2(x + total, bottom));
    if let (Some(slot), Some(bg)) = (table_slot, table.background) {
        cx.fill_slot(slot, whole, Paper::of(bg, &cx.t).fill, 0.0);
    }
    // A table with lines round every cell has its outline from them.
    if table.outline && !table.border {
        cx.push(whole, ItemKind::Outline { color: paper.line(&cx.t) });
    }
    bottom
}

/// A layout table with no room for its columns: its cells one under the other, each as wide as the room.
fn stacked(cx: &mut Cx<'_>, table: &Table, x: f32, y: f32, room: f32, paper: Paper, depth: usize) -> f32 {
    let around = table.background.map_or(paper, |bg| Paper::of(bg, &cx.t));
    let table_slot = table.background.map(|_| cx.slot());
    let line = around.line(&cx.t);
    let between = gaps(table).y;
    let mut at = y;
    let mut first = true;
    for (n, row) in table.rows.iter().enumerate() {
        // A copy does not go by the room there happened to be: the cells of a row are told
        // apart as they are when they stand side by side.
        let mut across = Across::begin(cx, table, n == 0, row.cells.iter());
        for cell in &row.cells {
            across.cell(cx);
            if !first {
                at += between;
            }
            first = false;
            let pad = padding(table, cell);
            let (inside, own) = cell_paper(cx, table, cell, around);
            let slot = own.then(|| cx.slot());
            let width = (room - pad.x * 2.0).max(MIN_CELL.min(room));
            let bottom = if depth >= MAX_NESTING {
                // Too deep to go on: what is in the cell is not laid out.
                at + pad.y * 2.0
            } else {
                flow(cx, &cell.blocks, x + pad.x.min(room / 2.0), at + pad.y, width, inside, depth + 1) + pad.y
            };
            let rect = Rect::from_min_max(pos2(x, at), pos2(x + room, bottom));
            if let Some(slot) = slot {
                cx.fill_slot(slot, rect, inside.fill, 0.0);
            }
            cell_lines(cx, rect, if table.border { Sides::ALL } else { cell.border }, Sides::ALL, line);
            at = bottom;
        }
        across.end(cx);
    }
    let whole = Rect::from_min_max(pos2(x, y), pos2(x + room, at));
    if let (Some(slot), Some(bg)) = (table_slot, table.background) {
        cx.fill_slot(slot, whole, Paper::of(bg, &cx.t).fill, 0.0);
    }
    if table.outline && !table.border {
        cx.push(whole, ItemKind::Outline { color: paper.line(&cx.t) });
    }
    at
}
