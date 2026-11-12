// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Display blocks laid out on pages. Lengths are points, y grows down from the top of the page.
//!
//! Each block becomes slabs, pieces that are never cut (a line of text, a picture, a table row),
//! stacked in order; the slabs are then dealt onto pages, a slab that doesn't fit starting the
//! next one.

use mailcraft_html::{Align, Block, BlockKind, Cell, Length, Line, LineStyle, Marker, Rgb, Span, Table, VAlign};

use crate::font::{FaceSet, Style};
use crate::picture::Pictures;
use crate::{Doc, MAX_PAGES, Options};

/// Points to a CSS pixel.
const PX: f32 = 0.75;
/// The size of body text, in points.
const BASE: f32 = 10.5;
/// The room round the page's content, in points.
const MARGIN: f32 = 54.0;
/// The deepest tables and quotes are nested before what is deeper is laid out flat.
const MAX_DEPTH: usize = 12;
/// The most columns a table is laid out with.
const MAX_COLUMNS: usize = 64;
const TEXT: Rgb = Rgb(0x1f, 0x1f, 0x1f);
const MUTED: Rgb = Rgb(0x60, 0x60, 0x60);
const RULE: Rgb = Rgb(0xb0, 0xb0, 0xb0);
const QUOTE_BAR: Rgb = Rgb(0xc4, 0xc4, 0xc4);
const LINK: Rgb = Rgb(0x0b, 0x57, 0xd0);

/// Something drawn on a page.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Op {
    /// Glyphs (new ids of the face's subset) from `x` on the baseline `y`.
    Text {
        x: f32,
        y: f32,
        face: usize,
        size: f32,
        glyphs: Vec<u16>,
        text: String,
        color: Rgb,
        italic: bool,
        fake_bold: bool,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Rgb,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        color: Rgb,
        style: LineStyle,
    },
    Image {
        index: usize,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    Link {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        url: String,
    },
}

impl Op {
    fn moved(mut self, dx: f32, dy: f32) -> Op {
        match &mut self {
            Op::Text { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            Op::Rect { x, y, .. } | Op::Image { x, y, .. } | Op::Link { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            Op::Line { x1, y1, x2, y2, .. } => {
                *x1 += dx;
                *x2 += dx;
                *y1 += dy;
                *y2 += dy;
            }
        }
        self
    }
}

/// A page: what is drawn on it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Page {
    pub ops: Vec<Op>,
}

impl Page {
    /// Its text, a line for each baseline, top to bottom.
    pub fn text(&self) -> String {
        let mut runs: Vec<(f32, f32, &str)> = self
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Text { x, y, text, .. } => Some((*y, *x, text.as_str())),
                _ => None,
            })
            .collect();
        runs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let mut out = String::new();
        let mut last: Option<f32> = None;
        for (y, _, text) in runs {
            match last {
                Some(l) if (y - l).abs() < 0.5 => {}
                Some(_) => out.push('\n'),
                None => {}
            }
            out.push_str(text);
            last = Some(y);
        }
        out
    }
}

/// A piece that is never cut across pages.
#[derive(Clone, Debug, Default)]
struct Slab {
    height: f32,
    ops: Vec<Op>,
    /// Room between pieces: dropped at the top of a page.
    space: bool,
    /// A heading: kept on the page of what follows it.
    keep: bool,
}

impl Slab {
    fn gap(height: f32) -> Slab {
        Slab { height, space: true, ..Slab::default() }
    }
}

struct Ctx<'a> {
    faces: &'a mut FaceSet,
    pictures: &'a mut Pictures,
    doc: &'a Doc,
    /// The tallest a slab may be: the page's content height.
    page_height: f32,
}

/// The pages of `docs`, each starting on a page of its own, with the page labels at their feet.
pub(crate) fn pages(docs: &[Doc], faces: &mut FaceSet, pictures: &mut Pictures, options: &Options) -> Vec<Page> {
    let paper = options.paper;
    let width = (paper.width - 2.0 * MARGIN).max(72.0);
    let top = MARGIN;
    let bottom = (paper.height - MARGIN).max(top + 72.0);
    let mut pages: Vec<Page> = Vec::new();
    for doc in docs {
        let mut ctx = Ctx { faces, pictures, doc, page_height: bottom - top };
        let mut slabs = header(&mut ctx, doc, width);
        lay_blocks(&mut ctx, &doc.blocks, width, 0, &mut slabs);
        deal(&slabs, &mut pages, top, bottom);
        if pages.len() >= MAX_PAGES {
            pages.truncate(MAX_PAGES);
            break;
        }
    }
    if !options.page_label.is_empty() {
        let count = pages.len();
        for (n, page) in pages.iter_mut().enumerate() {
            let label = options.page_label.replace("{n}", &(n + 1).to_string()).replace("{m}", &count.to_string());
            let line = text_line(faces, &[Span::plain(label)], Style::Regular, 8.0, MUTED);
            let x = paper.width - MARGIN - line.width;
            let y = paper.height - MARGIN / 2.0;
            page.ops.extend(line.ops(x, y));
        }
    }
    pages
}

/// `slabs` onto pages from a new page on.
fn deal(slabs: &[Slab], pages: &mut Vec<Page>, top: f32, bottom: f32) {
    pages.push(Page::default());
    let mut y = top;
    for (i, slab) in slabs.iter().enumerate() {
        if pages.len() > MAX_PAGES {
            return;
        }
        if slab.space {
            if y > top && y + slab.height <= bottom {
                if let Some(page) = pages.last_mut() {
                    page.ops.extend(slab.ops.iter().cloned().map(|op| op.moved(MARGIN, y)));
                }
                y += slab.height;
            } else if y > top {
                pages.push(Page::default());
                y = top;
            }
            continue;
        }
        let next = if slab.keep { slabs.get(i + 1..).and_then(|rest| rest.iter().find(|s| !s.space)).map_or(0.0, |s| s.height) } else { 0.0 };
        if y > top && (y + slab.height > bottom || (slab.keep && y + slab.height + next > bottom)) {
            pages.push(Page::default());
            y = top;
        }
        if let Some(page) = pages.last_mut() {
            page.ops.extend(slab.ops.iter().cloned().map(|op| op.moved(MARGIN, y)));
        }
        y += slab.height;
    }
}

/// The heading and the label lines over a document, and the line under them.
fn header(ctx: &mut Ctx, doc: &Doc, width: f32) -> Vec<Slab> {
    let mut out = Vec::new();
    if !doc.title.trim().is_empty() {
        let spans = [Span::plain(doc.title.trim())];
        out.extend(paragraph(ctx, &spans, &Para { style: Style::Bold, size: 15.0, width, ..Para::default() }));
        out.push(Slab::gap(6.0));
    }
    let label_width = doc
        .fields
        .iter()
        .map(|(label, _)| text_line(ctx.faces, &[Span::plain(label.as_str())], Style::Bold, 9.5, MUTED).width)
        .fold(0.0, f32::max)
        .min(width / 3.0);
    for (label, value) in &doc.fields {
        let label_line = text_line(ctx.faces, &[Span::plain(label.as_str())], Style::Bold, 9.5, MUTED);
        let indent = if label_width > 0.0 { label_width + 10.0 } else { 0.0 };
        let lines = paragraph(ctx, &[Span::plain(value.as_str())], &Para { size: 9.5, width: width - indent, ..Para::default() });
        for (n, mut line) in lines.into_iter().enumerate() {
            line.ops = line.ops.into_iter().map(|op| op.moved(indent, 0.0)).collect();
            if n == 0 {
                let baseline = line.ops.iter().find_map(|op| if let Op::Text { y, .. } = op { Some(*y) } else { None }).unwrap_or(line.height * 0.8);
                line.ops.extend(label_line.ops(0.0, baseline));
            }
            out.push(line);
        }
    }
    out.push(Slab::gap(6.0));
    out.push(Slab {
        height: 1.0,
        ops: vec![Op::Line { x1: 0.0, y1: 0.5, x2: width, y2: 0.5, width: 1.0, color: TEXT, style: LineStyle::Solid }],
        ..Slab::default()
    });
    out.push(Slab::gap(14.0));
    out
}

/// How a paragraph is set.
#[derive(Clone)]
struct Para {
    style: Style,
    size: f32,
    width: f32,
    align: Align,
    /// Line height as a factor of the size, or points.
    line_height: Option<mailcraft_html::LineHeight>,
    pre: bool,
    color: Rgb,
    /// Room on the left, before the text: a list's, a quote's.
    indent: f32,
}

impl Default for Para {
    fn default() -> Para {
        Para { style: Style::Regular, size: BASE, width: 400.0, align: Align::Left, line_height: None, pre: false, color: TEXT, indent: 0.0 }
    }
}

/// A run of glyphs in one face, size and look.
#[derive(Clone, Debug)]
struct Run {
    face: usize,
    size: f32,
    glyphs: Vec<u16>,
    text: String,
    width: f32,
    color: Rgb,
    background: Option<Rgb>,
    underline: bool,
    strike: bool,
    italic: bool,
    fake_bold: bool,
    link: Option<String>,
    ascent: f32,
    descent: f32,
}

/// What a line may be broken between: a word, a space, a line break.
#[derive(Clone, Debug, Default)]
struct Atom {
    runs: Vec<Run>,
    width: f32,
    space: bool,
    newline: bool,
}

/// A line of runs, laid out from x = 0.
#[derive(Clone, Debug, Default)]
struct TextLine {
    runs: Vec<(f32, Run)>,
    width: f32,
}

impl TextLine {
    /// The runs as ops with the baseline at `y`, from `x`.
    fn ops(&self, x: f32, y: f32) -> Vec<Op> {
        let mut ops = Vec::new();
        for (rx, run) in &self.runs {
            let x0 = x + rx;
            if let Some(bg) = run.background {
                ops.push(Op::Rect { x: x0, y: y - run.ascent, w: run.width, h: run.ascent - run.descent, color: bg });
            }
            if !run.glyphs.is_empty() {
                ops.push(Op::Text {
                    x: x0,
                    y,
                    face: run.face,
                    size: run.size,
                    glyphs: run.glyphs.clone(),
                    text: run.text.clone(),
                    color: run.color,
                    italic: run.italic,
                    fake_bold: run.fake_bold,
                });
            }
            let thick = (run.size * 0.06).max(0.5);
            if run.underline {
                let uy = y + run.size * 0.12;
                ops.push(Op::Line { x1: x0, y1: uy, x2: x0 + run.width, y2: uy, width: thick, color: run.color, style: LineStyle::Solid });
            }
            if run.strike {
                let sy = y - run.size * 0.3;
                ops.push(Op::Line { x1: x0, y1: sy, x2: x0 + run.width, y2: sy, width: thick, color: run.color, style: LineStyle::Solid });
            }
            if let Some(url) = &run.link {
                ops.push(Op::Link { x: x0, y: y - run.ascent, w: run.width, h: run.ascent - run.descent, url: url.clone() });
            }
        }
        ops
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF
        | 0x20000..=0x2FFFD | 0x30000..=0x3FFFD)
}

/// Text that shows nothing and is left out: soft hyphens, zero-width marks, direction marks.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}' | '\u{34f}')
}

/// The style of a span in a paragraph set as `para`.
fn span_style(span: &Span, para: &Para) -> Style {
    if span.code || para.pre || para.style == Style::Mono {
        Style::Mono
    } else if span.bold || para.style == Style::Bold {
        Style::Bold
    } else {
        Style::Regular
    }
}

/// The spans as atoms: words, spaces and line breaks.
fn atoms(faces: &mut FaceSet, spans: &[Span], para: &Para) -> Vec<Atom> {
    let mut out: Vec<Atom> = Vec::new();
    let mut word = Atom::default();
    let flush = |word: &mut Atom, out: &mut Vec<Atom>| {
        if !word.runs.is_empty() {
            out.push(std::mem::take(word));
        }
    };
    for span in spans {
        let style = span_style(span, para);
        let size = span.size.map_or(para.size, |px| (f32::from(px) * PX).clamp(5.0, 48.0));
        let (asc, desc) = faces.extent(style);
        let color = span.color.unwrap_or(if span.link.is_some() { LINK } else { para.color });
        let link = span
            .link
            .as_deref()
            .filter(|u| mailcraft_html::is_safe_url(u) && !u.trim_start().to_ascii_lowercase().starts_with("cid:"))
            .map(str::to_string);
        let make_run = |face: usize, fake_bold: bool| Run {
            face,
            size,
            glyphs: Vec::new(),
            text: String::new(),
            width: 0.0,
            color,
            background: span.background,
            underline: span.underline || (span.link.is_some() && span.color.is_none()),
            strike: span.strike,
            italic: span.italic,
            fake_bold,
            link: link.clone(),
            ascent: asc * size,
            descent: desc * size,
        };
        for c in span.text.chars() {
            if is_invisible(c) {
                if c == '\u{200b}' {
                    flush(&mut word, &mut out);
                }
                continue;
            }
            if c == '\n' || c == '\u{2028}' || c == '\u{2029}' {
                flush(&mut word, &mut out);
                out.push(Atom { newline: true, runs: vec![make_run(0, false)], ..Atom::default() });
                continue;
            }
            let (c, is_space) = match c {
                '\t' => (' ', true),
                ' ' => (' ', true),
                '\u{a0}' => (' ', false),
                c if c.is_whitespace() => (' ', true),
                c if c.is_control() => continue,
                c => (c, false),
            };
            if is_space {
                flush(&mut word, &mut out);
                if !para.pre && out.last().is_none_or(|a| a.space || a.newline) {
                    continue;
                }
            }
            let cjk = is_cjk(c);
            if cjk {
                flush(&mut word, &mut out);
            }
            let placed = faces.place(c, style);
            let gid = faces.used.get_mut(placed.face).map_or(0, |u| u.use_glyph(placed.gid, &c.to_string()));
            let advance = placed.advance * size;
            let atom = if is_space { &mut Atom { space: true, ..Atom::default() } } else { &mut word };
            let fits = atom.runs.last().is_some_and(|r| r.face == placed.face && r.fake_bold == placed.fake_bold && r.size == size);
            if !fits {
                atom.runs.push(make_run(placed.face, placed.fake_bold));
            }
            if let Some(run) = atom.runs.last_mut() {
                run.glyphs.push(gid);
                run.text.push(c);
                run.width += advance;
            }
            atom.width += advance;
            if is_space {
                let space = std::mem::take(atom);
                out.push(space);
            } else if cjk {
                flush(&mut word, &mut out);
            }
        }
        flush(&mut word, &mut out);
    }
    flush(&mut word, &mut out);
    out
}

/// An atom too wide for a line, cut into pieces that fit `width`, character by character.
fn split_atom(atom: Atom, width: f32) -> Vec<Atom> {
    let mut out = Vec::new();
    let mut piece = Atom::default();
    for run in atom.runs {
        let per = if run.glyphs.is_empty() { 0.0 } else { run.width / run.glyphs.len() as f32 };
        let chars: Vec<char> = run.text.chars().collect();
        for (i, gid) in run.glyphs.iter().enumerate() {
            if piece.width + per > width && piece.width > 0.0 {
                out.push(std::mem::take(&mut piece));
            }
            let fits = piece.runs.last().is_some_and(|r| r.face == run.face && r.size == run.size);
            if !fits {
                piece.runs.push(Run { glyphs: Vec::new(), text: String::new(), width: 0.0, ..run.clone() });
            }
            if let Some(r) = piece.runs.last_mut() {
                r.glyphs.push(*gid);
                if let Some(c) = chars.get(i) {
                    r.text.push(*c);
                }
                r.width += per;
            }
            piece.width += per;
        }
    }
    if !piece.runs.is_empty() {
        out.push(piece);
    }
    out
}

/// Atoms broken into lines of at most `width`.
fn break_lines(atoms: Vec<Atom>, width: f32) -> Vec<TextLine> {
    let mut lines = Vec::new();
    let mut line = TextLine::default();
    let mut ended_by_break = false;
    let finish = |line: &mut TextLine, lines: &mut Vec<TextLine>| {
        while line.runs.last().is_some_and(|(_, r)| r.text.chars().all(|c| c == ' ')) {
            if let Some((_, r)) = line.runs.pop() {
                line.width -= r.width;
            }
        }
        lines.push(std::mem::take(line));
    };
    let mut queue: std::collections::VecDeque<Atom> = atoms.into();
    while let Some(atom) = queue.pop_front() {
        if atom.newline {
            if line.runs.is_empty()
                && let Some(run) = atom.runs.first()
            {
                line.runs.push((0.0, Run { glyphs: Vec::new(), text: String::new(), width: 0.0, ..run.clone() }));
            }
            finish(&mut line, &mut lines);
            ended_by_break = true;
            continue;
        }
        ended_by_break = false;
        if atom.space && line.runs.is_empty() {
            continue;
        }
        if atom.width > width && !atom.space {
            if line.width > 0.0 {
                finish(&mut line, &mut lines);
            }
            for piece in split_atom(atom, width).into_iter().rev() {
                queue.push_front(piece);
            }
            if queue.front().is_some_and(|a| a.width > width)
                && let Some(piece) = queue.pop_front()
            {
                for run in piece.runs {
                    let w = run.width;
                    line.runs.push((line.width, run));
                    line.width += w;
                }
                finish(&mut line, &mut lines);
            }
            continue;
        }
        if line.width + atom.width > width && line.width > 0.0 && !atom.space {
            finish(&mut line, &mut lines);
        }
        for run in atom.runs {
            let w = run.width;
            line.runs.push((line.width, run));
            line.width += w;
        }
    }
    if !line.runs.is_empty() || ended_by_break {
        finish(&mut line, &mut lines);
    }
    lines
}

/// One line of text, not broken.
fn text_line(faces: &mut FaceSet, spans: &[Span], style: Style, size: f32, color: Rgb) -> TextLine {
    let para = Para { style, size, color, ..Para::default() };
    let mut line = TextLine::default();
    for atom in atoms(faces, spans, &para) {
        for run in atom.runs {
            let w = run.width;
            line.runs.push((line.width, run));
            line.width += w;
        }
    }
    line
}

/// A paragraph as slabs, a line each.
fn paragraph(ctx: &mut Ctx, spans: &[Span], para: &Para) -> Vec<Slab> {
    let width = (para.width - para.indent).max(12.0);
    let lines = break_lines(atoms(ctx.faces, spans, para), width);
    let (asc, desc) = ctx.faces.extent(para.style);
    let mut out = Vec::new();
    for line in lines {
        let ascent = line.runs.iter().map(|(_, r)| r.ascent).fold(asc * para.size, f32::max);
        let descent = line.runs.iter().map(|(_, r)| r.descent).fold(desc * para.size, f32::min);
        let size = line.runs.iter().map(|(_, r)| r.size).fold(para.size, f32::max);
        let height = para.line_height.map_or(size * 1.3, |lh| lh.of(size)).max(ascent - descent);
        let baseline = (height - (ascent - descent)) / 2.0 + ascent;
        let x = para.indent
            + match para.align {
                Align::Left => 0.0,
                Align::Center => ((width - line.width) / 2.0).max(0.0),
                Align::Right => (width - line.width).max(0.0),
            };
        out.push(Slab { height, ops: line.ops(x, baseline), ..Slab::default() });
    }
    out
}

/// The size and style of a block's text.
fn block_text(kind: &BlockKind) -> (f32, Style) {
    match kind {
        BlockKind::Heading(level) => {
            let scale = match level {
                1 => 1.6,
                2 => 1.35,
                3 => 1.17,
                4 => 1.0,
                5 => 0.9,
                _ => 0.85,
            };
            (BASE * scale, Style::Bold)
        }
        BlockKind::Preformatted => (BASE * 0.9, Style::Mono),
        _ => (BASE, Style::Regular),
    }
}

/// `blocks` as slabs into `out`, `width` wide from x = 0.
fn lay_blocks(ctx: &mut Ctx, blocks: &[Block], width: f32, depth: usize, out: &mut Vec<Slab>) {
    let mut i = 0;
    while let Some(block) = blocks.get(i) {
        if i > 0 {
            let (size, _) = block_text(&block.kind);
            let gap = if block.gap > 0 { f32::from(block.gap) * PX } else { f32::from(block.space.min(3)) * size * 0.8 };
            if gap > 0.0 {
                let mut slab = Slab::gap(gap);
                let shared = blocks.get(i - 1).map_or(0, |b| b.quote).min(block.quote).min(8);
                for level in 0..shared {
                    let x = f32::from(level) * 12.0 + 3.0;
                    slab.ops.push(Op::Line { x1: x, y1: 0.0, x2: x, y2: gap, width: 1.5, color: QUOTE_BAR, style: LineStyle::Solid });
                }
                out.push(slab);
            }
        }
        let quote = f32::from(block.quote.min(8)) * 12.0;
        let left = (f32::from(block.inset[0]) * PX).min(width / 3.0) + quote;
        let right = (f32::from(block.inset[1]) * PX).min(width / 3.0);
        let inner = (width - left - right).max(24.0);
        let start = out.len();
        match &block.kind {
            BlockKind::Image { .. } => {
                let mut row = vec![block];
                while let Some(next) = blocks.get(i + 1).filter(|b| b.beside && matches!(b.kind, BlockKind::Image { .. })) {
                    row.push(next);
                    i += 1;
                }
                pictures_row(ctx, &row, inner, out);
            }
            BlockKind::Rule => {
                let w = f32::from(block.line.width.max(1)) * PX;
                let color = block.line.color.unwrap_or(RULE);
                let h = w + 8.0;
                out.push(Slab {
                    height: h,
                    ops: vec![Op::Line { x1: 0.0, y1: h / 2.0, x2: inner, y2: h / 2.0, width: w, color, style: block.line.style }],
                    ..Slab::default()
                });
            }
            BlockKind::Table(table) => {
                if depth >= MAX_DEPTH {
                    for cell in table.rows.iter().flat_map(|r| &r.cells) {
                        lay_blocks(ctx, &cell.blocks, inner, depth + 1, out);
                    }
                } else {
                    lay_table(ctx, table, block.align, inner, depth, out);
                }
            }
            kind => {
                let (size, style) = block_text(kind);
                let (indent, marker) = match kind {
                    BlockKind::ListItem { marker, depth } => (18.0 * f32::from(depth.saturating_add(1).min(8)), Some((marker, *depth))),
                    _ => (0.0, None),
                };
                let para = Para {
                    style,
                    size,
                    width: inner,
                    align: block.align,
                    line_height: block.line_height,
                    pre: matches!(kind, BlockKind::Preformatted),
                    color: TEXT,
                    indent,
                };
                let mut lines = paragraph(ctx, &block.spans, &para);
                if lines.is_empty() && marker.is_some() {
                    lines.push(Slab { height: size * 1.3, ..Slab::default() });
                }
                if let (Some((marker, level)), Some(first)) = (marker, lines.first_mut()) {
                    let label = match marker {
                        Marker::Bullet => ["•", "◦", "▪"].get(usize::from(level % 3)).copied().unwrap_or("•").to_string(),
                        Marker::Number(n) => format!("{n}."),
                        Marker::Label(l) => l.chars().take(12).collect(),
                        Marker::None => String::new(),
                    };
                    if !label.is_empty() {
                        let line = text_line(ctx.faces, &[Span::plain(label)], Style::Regular, size, TEXT);
                        let baseline =
                            first.ops.iter().find_map(|op| if let Op::Text { y, .. } = op { Some(*y) } else { None }).unwrap_or(first.height * 0.78);
                        first.ops.extend(line.ops((indent - 5.0 - line.width).max(0.0), baseline));
                    }
                }
                if let BlockKind::Heading(_) = kind
                    && let Some(last) = lines.last_mut()
                {
                    last.keep = true;
                }
                out.extend(lines);
            }
        }
        for slab in out.iter_mut().skip(start) {
            if slab.space {
                continue;
            }
            slab.ops = std::mem::take(&mut slab.ops).into_iter().map(|op| op.moved(left, 0.0)).collect();
            for level in 0..block.quote.min(8) {
                let x = f32::from(level) * 12.0 + 3.0;
                slab.ops.push(Op::Line { x1: x, y1: 0.0, x2: x, y2: slab.height, width: 1.5, color: QUOTE_BAR, style: LineStyle::Solid });
            }
        }
        i += 1;
    }
}

/// The bytes of the picture a block shows, when the message carries them.
fn picture_bytes<'a>(doc: &'a Doc, block: &'a Block) -> Option<&'a [u8]> {
    let BlockKind::Image { src, data, .. } = &block.kind else { return None };
    if let Some(picture) = data {
        return Some(picture.0.as_slice());
    }
    let id = src.trim().get(4..).filter(|_| src.trim().get(..4).is_some_and(|s| s.eq_ignore_ascii_case("cid:")))?;
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    doc.pictures.iter().find(|(cid, _)| cid.trim_start_matches('<').trim_end_matches('>').eq_ignore_ascii_case(id)).map(|(_, b)| b.as_ref())
}

/// Pictures side by side on a line, wrapping when it is full; one that can't be printed shows its
/// alternative text.
fn pictures_row(ctx: &mut Ctx, row: &[&Block], width: f32, out: &mut Vec<Slab>) {
    let mut placed: Vec<(f32, f32, Vec<Op>)> = Vec::new();
    for block in row {
        let BlockKind::Image { width: w_attr, height: h_attr, link, .. } = &block.kind else { continue };
        let pad = block.image_padding.unwrap_or_default();
        let (pl, pr, pt, pb) = (f32::from(pad.left) * PX, f32::from(pad.right) * PX, f32::from(pad.top) * PX, f32::from(pad.bottom) * PX);
        let picture = picture_bytes(ctx.doc, block).and_then(|bytes| ctx.pictures.add(bytes));
        let Some((index, pw, ph)) = picture else {
            let alt: Vec<Span> = block.spans.iter().filter(|s| !s.text.trim().is_empty()).cloned().collect();
            if !alt.is_empty() {
                let para = Para { width, color: MUTED, ..Para::default() };
                let lines = paragraph(ctx, &alt, &para);
                let h: f32 = lines.iter().map(|s| s.height).sum();
                let mut ops = Vec::new();
                let mut y = 0.0;
                for line in lines {
                    ops.extend(line.ops.into_iter().map(|op| op.moved(0.0, y)));
                    y += line.height;
                }
                let w = ops.iter().filter_map(|op| if let Op::Text { .. } = op { Some(width) } else { None }).next().unwrap_or(0.0);
                placed.push((w.min(width), h, ops));
            }
            continue;
        };
        let (iw, ih) = (pw as f32 * PX, ph as f32 * PX);
        let (mut w, mut h) = match (w_attr, h_attr) {
            (Some(w), Some(h)) => (f32::from(*w) * PX, f32::from(*h) * PX),
            (Some(w), None) => (f32::from(*w) * PX, f32::from(*w) * PX * ih / iw.max(0.01)),
            (None, Some(h)) => (f32::from(*h) * PX * iw / ih.max(0.01), f32::from(*h) * PX),
            (None, None) => (iw, ih),
        };
        let room = (width - pl - pr).max(12.0);
        if w > room {
            h *= room / w;
            w = room;
        }
        let tall = (ctx.page_height - pt - pb).max(12.0);
        if h > tall {
            w *= tall / h;
            h = tall;
        }
        if w <= 0.0 || h <= 0.0 || !w.is_finite() || !h.is_finite() {
            continue;
        }
        let mut ops = vec![Op::Image { index, x: pl, y: pt, w, h }];
        if let Some(url) = link.as_deref().filter(|u| mailcraft_html::is_safe_url(u)) {
            ops.push(Op::Link { x: pl, y: pt, w, h, url: url.to_string() });
        }
        placed.push((w + pl + pr, h + pt + pb, ops));
    }
    let align = row.first().map_or(Align::Left, |b| b.align);
    let mut line: Vec<(f32, f32, Vec<Op>)> = Vec::new();
    let emit = |line: &mut Vec<(f32, f32, Vec<Op>)>, out: &mut Vec<Slab>| {
        if line.is_empty() {
            return;
        }
        let total: f32 = line.iter().map(|p| p.0).sum();
        let height = line.iter().map(|p| p.1).fold(0.0, f32::max);
        let mut x = match align {
            Align::Left => 0.0,
            Align::Center => ((width - total) / 2.0).max(0.0),
            Align::Right => (width - total).max(0.0),
        };
        let mut ops = Vec::new();
        for (w, h, item) in line.drain(..) {
            ops.extend(item.into_iter().map(|op| op.moved(x, height - h)));
            x += w;
        }
        out.push(Slab { height, ops, ..Slab::default() });
    };
    for item in placed {
        let used: f32 = line.iter().map(|p| p.0).sum();
        if used + item.0 > width + 0.5 && !line.is_empty() {
            emit(&mut line, out);
        }
        line.push(item);
    }
    emit(&mut line, out);
}

/// The narrowest and widest a run of blocks wants to be: its longest word or picture, and its
/// longest line unbroken.
fn measure(ctx: &mut Ctx, blocks: &[Block], depth: usize) -> (f32, f32) {
    let mut min: f32 = 0.0;
    let mut max: f32 = 0.0;
    let mut row = 0.0;
    for block in blocks {
        let inset = (f32::from(block.inset[0]) + f32::from(block.inset[1])) * PX + f32::from(block.quote.min(8)) * 12.0;
        let (lo, hi) = match &block.kind {
            BlockKind::Image { width, height, .. } => {
                let w = match (width, picture_bytes(ctx.doc, block).and_then(|b| ctx.pictures.add(b))) {
                    (Some(w), _) => f32::from(*w) * PX,
                    (None, Some((_, pw, ph))) => height.map_or(pw as f32 * PX, |h| f32::from(h) * PX * pw as f32 / (ph as f32).max(1.0)),
                    (None, None) => 0.0,
                };
                (w, w)
            }
            BlockKind::Rule => (0.0, 0.0),
            BlockKind::Table(table) if depth < MAX_DEPTH => {
                let (lo, hi) = measure_table(ctx, table, depth + 1);
                match table.width {
                    Some(Length::Px(px)) => {
                        let w = f32::from(px) * PX;
                        (lo.max(w.min(lo.max(1.0) * 4.0)), w.max(lo))
                    }
                    _ => (lo, hi),
                }
            }
            BlockKind::Table(_) => (0.0, 0.0),
            kind => {
                let (size, style) = block_text(kind);
                let para = Para { style, size, pre: matches!(kind, BlockKind::Preformatted), ..Para::default() };
                let indent = if let BlockKind::ListItem { depth, .. } = kind { 18.0 * f32::from(depth.saturating_add(1).min(8)) } else { 0.0 };
                let mut lo: f32 = 0.0;
                let mut line: f32 = 0.0;
                let mut hi: f32 = 0.0;
                for atom in atoms(ctx.faces, &block.spans, &para) {
                    if atom.newline {
                        hi = hi.max(line);
                        line = 0.0;
                        continue;
                    }
                    if !atom.space {
                        lo = lo.max(atom.width);
                    }
                    line += atom.width;
                }
                hi = hi.max(line);
                (lo + indent, hi + indent)
            }
        };
        if block.beside {
            row += hi;
            max = max.max(row);
        } else {
            row = hi;
            max = max.max(hi + inset);
        }
        min = min.max(lo.min(240.0) + inset);
    }
    (min, max)
}

/// A table's grid: for each row, each cell with the column it starts in.
fn grid(table: &Table) -> Vec<Vec<(usize, usize, &Cell)>> {
    let columns = usize::from(table.columns).clamp(1, MAX_COLUMNS);
    let mut taken: Vec<usize> = vec![0; columns];
    let mut rows = Vec::new();
    for row in &table.rows {
        let mut cells = Vec::new();
        let mut col = 0;
        for cell in &row.cells {
            while taken.get(col).is_some_and(|t| *t > 0) {
                col += 1;
            }
            if col >= columns {
                break;
            }
            let span = usize::from(cell.colspan.max(1)).min(columns - col);
            for t in taken.iter_mut().skip(col).take(span) {
                *t = usize::from(cell.rowspan.max(1));
            }
            cells.push((col, span, cell));
            col += span;
        }
        for t in taken.iter_mut() {
            *t = t.saturating_sub(1);
        }
        rows.push(cells);
    }
    rows
}

fn padding(table: &Table, cell: &Cell) -> [f32; 4] {
    match cell.padding {
        Some(p) => [f32::from(p.top) * PX, f32::from(p.right) * PX, f32::from(p.bottom) * PX, f32::from(p.left) * PX],
        None if table.border || cell.border.any() => [2.5, 3.5, 2.5, 3.5],
        None => [1.0, 1.5, 1.0, 1.5],
    }
}

fn spacing(table: &Table) -> f32 {
    table.spacing.map_or(if table.border { 1.5 } else { 0.0 }, |s| f32::from(s) * PX)
}

/// The narrowest and widest each column wants to be.
fn columns(ctx: &mut Ctx, table: &Table, depth: usize) -> Vec<(f32, f32, Option<f32>)> {
    let count = usize::from(table.columns).clamp(1, MAX_COLUMNS);
    let mut cols = vec![(0.0f32, 0.0f32, None::<f32>); count];
    let grid = grid(table);
    let mut spanned = Vec::new();
    for row in &grid {
        for (col, span, cell) in row {
            let pad = padding(table, cell);
            let (lo, hi) = if cell.header { measure(ctx, &emboldened(&cell.blocks), depth) } else { measure(ctx, &cell.blocks, depth) };
            let (lo, hi) = (lo + pad[1] + pad[3], hi + pad[1] + pad[3]);
            if *span == 1 {
                if let Some(c) = cols.get_mut(*col) {
                    c.0 = c.0.max(lo);
                    c.1 = c.1.max(hi);
                    if let Some(Length::Px(px)) = cell.width {
                        let w = f32::from(px) * PX;
                        c.2 = Some(c.2.map_or(w, |o: f32| o.max(w)));
                    }
                }
            } else {
                spanned.push((*col, *span, lo, hi));
            }
        }
    }
    for (col, span, lo, hi) in spanned {
        let range = col..col + span;
        let (have_lo, have_hi) = cols.get(range.clone()).map_or((0.0, 0.0), |c| c.iter().fold((0.0, 0.0), |a, x| (a.0 + x.0, a.1 + x.1)));
        let n = span as f32;
        for c in cols.iter_mut().skip(col).take(span) {
            if lo > have_lo {
                c.0 += (lo - have_lo) / n;
            }
            if hi > have_hi {
                c.1 += (hi - have_hi) / n;
            }
        }
    }
    for c in cols.iter_mut() {
        if let Some(fixed) = c.2 {
            c.1 = c.1.max(fixed.max(c.0));
        }
        c.1 = c.1.max(c.0);
    }
    cols
}

fn measure_table(ctx: &mut Ctx, table: &Table, depth: usize) -> (f32, f32) {
    let cols = columns(ctx, table, depth);
    let gaps = spacing(table) * (cols.len() as f32 + 1.0);
    let lo: f32 = cols.iter().map(|c| c.0).sum::<f32>() + gaps;
    let hi: f32 = cols.iter().map(|c| c.2.unwrap_or(c.1)).sum::<f32>() + gaps;
    (lo, hi.max(lo))
}

/// The width of each column for a table `width` wide (or less, when it wants less).
fn column_widths(ctx: &mut Ctx, table: &Table, width: f32, depth: usize) -> (Vec<f32>, f32) {
    let cols = columns(ctx, table, depth);
    let gaps = spacing(table) * (cols.len() as f32 + 1.0);
    let room = (width - gaps).max(cols.len() as f32 * 6.0);
    let asked = match table.width {
        Some(Length::Px(px)) => Some((f32::from(px) * PX - gaps).min(room)),
        Some(Length::Percent(p)) => Some(room * f32::from(p.min(100)) / 100.0),
        None => None,
    };
    let lo: f32 = cols.iter().map(|c| c.0).sum();
    let hi: f32 = cols.iter().map(|c| c.2.unwrap_or(c.1)).sum();
    let target = asked.unwrap_or(hi.min(room)).max(lo.min(room));
    let widths: Vec<f32> = if target >= hi && hi > 0.0 {
        cols.iter().map(|c| c.2.unwrap_or(c.1) * target / hi).collect()
    } else if target >= lo && hi > lo {
        let k = (target - lo) / (hi - lo);
        cols.iter().map(|c| c.0 + (c.2.unwrap_or(c.1) - c.0).max(0.0) * k).collect()
    } else if lo > 0.0 {
        cols.iter().map(|c| c.0 * target / lo).collect()
    } else {
        vec![target / cols.len().max(1) as f32; cols.len()]
    };
    let pcts = percents(table, cols.len());
    let widths = if pcts.iter().any(Option::is_some) { with_percents(&cols, &pcts, target) } else { widths };
    let total = widths.iter().sum::<f32>() + gaps;
    (widths, total)
}

/// The share of the table each column asks for, from its cells of one column (`width="25%"`).
fn percents(table: &Table, count: usize) -> Vec<Option<f32>> {
    let mut out = vec![None; count];
    for row in grid(table) {
        for (col, span, cell) in row {
            if let (1, Some(Length::Percent(p)), Some(slot)) = (span, cell.width, out.get_mut(col)) {
                let p = f32::from(p.min(100)) / 100.0;
                *slot = Some(slot.map_or(p, |o: f32| o.max(p)));
            }
        }
    }
    out
}

/// Column widths for a table `target` wide where some columns ask for a share of it: each of
/// those gets its share (never less than its longest word), the others what is left by what they
/// want, and a table of shares alone is stretched to its width.
fn with_percents(cols: &[(f32, f32, Option<f32>)], pcts: &[Option<f32>], target: f32) -> Vec<f32> {
    let mut widths: Vec<f32> = cols.iter().zip(pcts).map(|(c, p)| p.map_or(0.0, |p| (target * p).max(c.0))).collect();
    let used: f32 = widths.iter().sum();
    let rest: Vec<usize> = pcts.iter().enumerate().filter(|(_, p)| p.is_none()).map(|(i, _)| i).collect();
    if rest.is_empty() {
        if used > 0.0 && used < target {
            widths.iter_mut().for_each(|w| *w *= target / used);
        }
        return widths;
    }
    let lo: f32 = rest.iter().filter_map(|i| cols.get(*i)).map(|c| c.0).sum();
    let hi: f32 = rest.iter().filter_map(|i| cols.get(*i)).map(|c| c.2.unwrap_or(c.1)).sum();
    let room = (target - used).max(lo);
    for i in rest {
        let (Some(c), Some(w)) = (cols.get(i), widths.get_mut(i)) else { continue };
        let want = c.2.unwrap_or(c.1);
        *w = if hi > 0.0 { (want * room / hi).max(c.0) } else { room / pcts.len().max(1) as f32 };
    }
    widths
}

/// `blocks` with all their text bold: a header cell's.
fn emboldened(blocks: &[Block]) -> Vec<Block> {
    blocks
        .iter()
        .map(|b| {
            let mut b = b.clone();
            for span in &mut b.spans {
                span.bold = true;
            }
            b
        })
        .collect()
}

/// The lines a cell draws on its sides (top, right, bottom, left), as width and colour.
fn cell_lines(table: &Table, cell: &Cell) -> [Option<(f32, Rgb, LineStyle)>; 4] {
    let sides = [cell.border.top, cell.border.right, cell.border.bottom, cell.border.left];
    let mut out = [None; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        let line: Line = cell.lines.get(i).copied().unwrap_or_default();
        if line.is_set() {
            *slot = Some((f32::from(line.width) * PX, line.color.unwrap_or(RULE), line.style));
        } else if table.border || sides.get(i).copied().unwrap_or(false) {
            *slot = Some((0.75, RULE, LineStyle::Solid));
        }
    }
    out
}

/// A table as slabs, a row each; a row taller than a page is laid out cell after cell instead.
fn lay_table(ctx: &mut Ctx, table: &Table, align: Align, width: f32, depth: usize, out: &mut Vec<Slab>) {
    let (widths, total) = column_widths(ctx, table, width, depth);
    let gap = spacing(table);
    let x0 = match align {
        Align::Left => 0.0,
        Align::Center => ((width - total) / 2.0).max(0.0),
        Align::Right => (width - total).max(0.0),
    };
    let starts: Vec<f32> = widths
        .iter()
        .scan(x0 + gap, |x, w| {
            let at = *x;
            *x += w + gap;
            Some(at)
        })
        .collect();
    let grid = grid(table);
    let rows = grid.len();
    for (r, row) in grid.iter().enumerate() {
        let mut cells: Vec<(f32, f32, [f32; 4], Vec<Slab>, &Cell)> = Vec::new();
        for (col, span, cell) in row {
            let x = starts.get(*col).copied().unwrap_or(x0);
            let w = widths.iter().skip(*col).take(*span).sum::<f32>() + gap * (*span as f32 - 1.0);
            let pad = padding(table, cell);
            let mut slabs = Vec::new();
            let bold;
            let blocks = if cell.header {
                bold = emboldened(&cell.blocks);
                &bold
            } else {
                &cell.blocks
            };
            lay_blocks(ctx, blocks, (w - pad[1] - pad[3]).max(6.0), depth + 1, &mut slabs);
            let mut slabs: Vec<Slab> = slabs.into_iter().skip_while(|s| s.space).collect();
            while slabs.last().is_some_and(|s| s.space) {
                slabs.pop();
            }
            cells.push((x, w, pad, slabs, cell));
        }
        let content = |c: &(f32, f32, [f32; 4], Vec<Slab>, &Cell)| c.3.iter().map(|s| s.height).sum::<f32>() + c.2[0] + c.2[2];
        let height = cells.iter().map(content).fold(0.0, f32::max);
        let top = if r == 0 { gap } else { 0.0 };
        let bottom = gap;
        if height + top + bottom > ctx.page_height {
            for (_, _, _, slabs, _) in cells {
                out.extend(slabs);
            }
            continue;
        }
        let slab_h = height + top + bottom;
        let mut ops = Vec::new();
        if let Some(bg) = table.background {
            ops.push(Op::Rect { x: x0, y: 0.0, w: total, h: slab_h, color: bg });
        }
        for (x, w, pad, slabs, cell) in cells {
            let ch = slabs.iter().map(|s| s.height).sum::<f32>() + pad[0] + pad[2];
            if let Some(bg) = cell.background {
                ops.push(Op::Rect { x, y: top, w, h: height, color: bg });
            }
            let shift = match cell.valign {
                Some(VAlign::Middle) => (height - ch) / 2.0,
                Some(VAlign::Bottom) => height - ch,
                _ => 0.0,
            };
            let mut y = top + pad[0] + shift.max(0.0);
            for slab in slabs {
                ops.extend(slab.ops.into_iter().map(|op| op.moved(x + pad[3], y)));
                y += slab.height;
            }
            let lines = cell_lines(table, cell);
            let (l, t, rr, b) = (x, top, x + w, top + height);
            let edges = [(l, t, rr, t), (rr, t, rr, b), (l, b, rr, b), (l, t, l, b)];
            for (line, (x1, y1, x2, y2)) in lines.iter().zip(edges) {
                if let Some((lw, color, style)) = line {
                    ops.push(Op::Line { x1, y1, x2, y2, width: *lw, color: *color, style: *style });
                }
            }
        }
        if table.outline {
            let lines = table.lines;
            let get = |i: usize| -> (f32, Rgb, LineStyle) {
                let l = lines.get(i).copied().unwrap_or_default();
                if l.is_set() { (f32::from(l.width) * PX, l.color.unwrap_or(RULE), l.style) } else { (0.75, RULE, LineStyle::Solid) }
            };
            let (x1, x2) = (x0, x0 + total);
            if r == 0 {
                let (w, c, s) = get(0);
                ops.push(Op::Line { x1, y1: 0.0, x2, y2: 0.0, width: w, color: c, style: s });
            }
            let (w, c, s) = get(1);
            ops.push(Op::Line { x1: x2, y1: 0.0, x2, y2: slab_h, width: w, color: c, style: s });
            let (w, c, s) = get(3);
            ops.push(Op::Line { x1, y1: 0.0, x2: x1, y2: slab_h, width: w, color: c, style: s });
            if r + 1 == rows {
                let (w, c, s) = get(2);
                ops.push(Op::Line { x1, y1: slab_h, x2, y2: slab_h, width: w, color: c, style: s });
            }
        }
        out.push(Slab { height: slab_h, ops, ..Slab::default() });
    }
}
