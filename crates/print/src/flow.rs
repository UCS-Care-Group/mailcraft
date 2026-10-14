//! Text and blocks flowed down pages: lines wrapped to a width, display blocks (paragraphs,
//! headings, lists, quotes, preformatted text, rules, pictures, tables) as pieces of a page, and
//! pieces placed on pages one after another with page breaks between them.

use std::collections::HashMap;
use std::sync::Arc;

use mailcraft_html::{Align, Block, BlockKind, Marker, Span, Table};

use crate::font::{self, FontKind};
use crate::picture::{self, Picture};
use crate::{Item, MAX_PAGES, Page, Paper, Rgb, Text, ink};

/// The size body text is printed at.
pub(crate) const BODY_SIZE: f32 = 10.5;
/// The most lines one block wraps to; a block longer than that is cut (hundreds of pages).
const MAX_LINES: usize = 30_000;
/// The deepest a table inside a table is laid out as a table; deeper ones are read row by row.
const MAX_TABLE_NESTING: usize = 2;
/// The most columns laid out side by side; a wider table is read row by row.
const MAX_GRID_COLUMNS: u16 = 10;
const LINK: Rgb = Rgb(0, 70, 160);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Style {
    pub font: FontKind,
    pub italic: bool,
    pub size: f32,
    pub color: Rgb,
    pub underline: bool,
    pub strike: bool,
}

impl Style {
    pub fn new(font: FontKind, size: f32) -> Style {
        Style { font, italic: false, size, color: ink::TEXT, underline: false, strike: false }
    }
}

/// Text in one style.
#[derive(Clone, Debug)]
pub(crate) struct Run {
    pub text: String,
    pub style: Style,
}

impl Run {
    pub fn new(text: impl Into<String>, style: Style) -> Run {
        Run { text: text.into(), style }
    }
}

/// A piece of a page: what it draws, from its own top-left corner, and how tall it is. A gap is
/// space between pieces, which a new page doesn't start with.
#[derive(Clone, Debug, Default)]
pub(crate) struct Chunk {
    pub height: f32,
    pub items: Vec<Item>,
    pub gap: bool,
}

impl Chunk {
    pub fn gap(height: f32) -> Chunk {
        Chunk { height, items: Vec::new(), gap: true }
    }
}

/// One piece of a line being built.
struct Piece {
    text: String,
    style: Style,
    width: f32,
}

#[derive(Default)]
struct Line {
    pieces: Vec<Piece>,
    width: f32,
}

impl Line {
    fn has_text(&self) -> bool {
        self.pieces.iter().any(|p| !p.text.trim().is_empty())
    }

    fn trim_end(&mut self) {
        while self.pieces.last().is_some_and(|p| p.text.trim().is_empty()) {
            if let Some(p) = self.pieces.pop() {
                self.width -= p.width;
            }
        }
    }
}

/// The runs wrapped to `width`: one piece of a page per line. A word longer than a line is
/// broken where it reaches the edge; a line break in the text starts a new line.
pub(crate) fn wrap(runs: &[Run], width: f32, align: Align) -> Vec<Chunk> {
    wrap_in(runs, width, align, false)
}

/// [`wrap`], for text that runs right to left when `rtl`: each line is set from the right, its
/// pieces in the order they are read ([`mailcraft_html::bidi::visual_order`]).
pub(crate) fn wrap_in(runs: &[Run], width: f32, align: Align, rtl: bool) -> Vec<Chunk> {
    let width = width.max(12.0);
    let mut lines: Vec<(Line, f32)> = Vec::new();
    let mut line = Line::default();
    // The tallest size on the line (for an empty line, the size of what came before).
    let mut tallest: f32 = runs.first().map_or(BODY_SIZE, |r| r.style.size);
    let mut line_size: f32 = 0.0;
    let finish = |line: &mut Line, line_size: &mut f32, tallest: f32, lines: &mut Vec<(Line, f32)>| -> bool {
        line.trim_end();
        let size = if *line_size > 0.0 { *line_size } else { tallest };
        lines.push((std::mem::take(line), size));
        *line_size = 0.0;
        lines.len() < MAX_LINES
    };
    'runs: for run in runs {
        let style = run.style;
        tallest = style.size;
        // Line breaks first: cleaning takes them out with the other control characters.
        let pieces: Vec<&str> = run.text.split('\n').collect();
        let many = pieces.len() > 1;
        for (index, part) in pieces.iter().enumerate() {
            if index > 0 && !finish(&mut line, &mut line_size, tallest, &mut lines) {
                break 'runs;
            }
            let part = font::clean(style.font, &part.replace('\r', "").replace('\t', "    "));
            for token in tokens(&part) {
                let is_space = token.chars().all(char::is_whitespace);
                if is_space {
                    if line.pieces.is_empty() && !many {
                        continue;
                    }
                    let w = font::width(style.font, token, style.size);
                    line.pieces.push(Piece { text: token.to_string(), style, width: w });
                    line.width += w;
                    continue;
                }
                let w = font::width(style.font, token, style.size);
                if line.width + w > width && line.has_text() && !finish(&mut line, &mut line_size, tallest, &mut lines) {
                    break 'runs;
                }
                if w <= width - line.width {
                    line.pieces.push(Piece { text: token.to_string(), style, width: w });
                    line.width += w;
                    line_size = line_size.max(style.size);
                    continue;
                }
                // Longer than the room on the line: broken at the edge.
                let mut part = String::new();
                let mut part_w = 0.0;
                for c in token.chars() {
                    let cw = font::width(style.font, c.encode_utf8(&mut [0; 4]), style.size);
                    if line.width + part_w + cw > width && (!part.is_empty() || line.has_text()) {
                        if !part.is_empty() {
                            line.pieces.push(Piece { text: std::mem::take(&mut part), style, width: part_w });
                            line.width += part_w;
                            line_size = line_size.max(style.size);
                        }
                        part_w = 0.0;
                        if !finish(&mut line, &mut line_size, tallest, &mut lines) {
                            break 'runs;
                        }
                    }
                    part.push(c);
                    part_w += cw;
                }
                if !part.is_empty() {
                    line.pieces.push(Piece { text: part, style, width: part_w });
                    line.width += part_w;
                    line_size = line_size.max(style.size);
                }
            }
        }
    }
    if line.has_text() || lines.is_empty() || !line.pieces.is_empty() {
        finish(&mut line, &mut line_size, tallest, &mut lines);
    }
    lines.into_iter().map(|(line, size)| line_chunk(line, size, width, align, rtl)).collect()
}

/// Words and the spaces between them.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (i, c) in text.char_indices() {
        let is_space = c == ' ' || c == '\u{3000}';
        match space {
            Some(s) if s != is_space => {
                if let Some(t) = text.get(start..i) {
                    out.push(t);
                }
                start = i;
            }
            _ => {}
        }
        space = Some(is_space);
        // Text without spaces between words (Chinese, Japanese) can break after each character.
        if !is_space && is_cjk(c) {
            let end = i + c.len_utf8();
            if let Some(t) = text.get(start..end) {
                out.push(t);
            }
            start = end;
            space = None;
        }
    }
    if let Some(t) = text.get(start..)
        && !t.is_empty()
    {
        out.push(t);
    }
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{2E80}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7AF}' | '\u{F900}'..='\u{FAFF}' | '\u{FF00}'..='\u{FFEF}')
}

fn line_chunk(mut line: Line, size: f32, width: f32, align: Align, rtl: bool) -> Chunk {
    if rtl {
        // Drawn from the left: the line's characters in the order they are drawn, in pieces of
        // one style again.
        let styled: Vec<(char, Style)> = line.pieces.iter().flat_map(|p| p.text.chars().map(move |c| (c, p.style))).collect();
        let text: String = styled.iter().map(|(c, _)| *c).collect();
        let (drawn, order) = mailcraft_html::bidi::visual_order(&text, mailcraft_html::bidi::Direction::Rtl);
        let mut pieces: Vec<Piece> = Vec::new();
        for (c, i) in drawn.chars().zip(order) {
            let Some((_, style)) = styled.get(i) else { continue };
            match pieces.last_mut() {
                Some(last) if last.style == *style => last.text.push(c),
                _ => pieces.push(Piece { text: c.to_string(), style: *style, width: 0.0 }),
            }
        }
        for piece in &mut pieces {
            piece.width = font::width(piece.style.font, &piece.text, piece.style.size);
        }
        line.width = pieces.iter().map(|p| p.width).sum();
        line.pieces = pieces;
    }
    let height = size * 1.32;
    let baseline = size * 1.02;
    let x0 = match align {
        Align::Left => 0.0,
        Align::Center => ((width - line.width) / 2.0).max(0.0),
        Align::Right => (width - line.width).max(0.0),
    };
    let mut items = Vec::new();
    let mut x = x0;
    // Pieces in the same style are one run of text.
    let mut run: Option<(f32, String, Style, f32)> = None;
    let flush = |run: &mut Option<(f32, String, Style, f32)>, items: &mut Vec<Item>| {
        if let Some((rx, text, style, w)) = run.take() {
            if !text.trim().is_empty() || style.underline {
                items.push(Item::Text(Text {
                    x: rx,
                    y: baseline,
                    size: style.size,
                    font: style.font,
                    italic: style.italic,
                    color: style.color,
                    text: text.clone(),
                }));
            }
            let stroke = (style.size / 16.0).max(0.5);
            if style.underline && !text.trim().is_empty() {
                let y = baseline + style.size * 0.12;
                items.push(Item::Line { x1: rx, y1: y, x2: rx + w, y2: y, width: stroke, color: style.color });
            }
            if style.strike && !text.trim().is_empty() {
                let y = baseline - style.size * 0.3;
                items.push(Item::Line { x1: rx, y1: y, x2: rx + w, y2: y, width: stroke, color: style.color });
            }
        }
    };
    for piece in line.pieces {
        match &mut run {
            Some((_, text, style, w)) if *style == piece.style => {
                text.push_str(&piece.text);
                *w += piece.width;
            }
            _ => {
                flush(&mut run, &mut items);
                run = Some((x, piece.text, piece.style, piece.width));
            }
        }
        x += piece.width;
    }
    flush(&mut run, &mut items);
    Chunk { height, items, gap: false }
}

/// The pictures of a print: the ones the message carries, by their `cid:` address, decoded the
/// first time they are shown.
#[derive(Default)]
pub(crate) struct Pictures {
    sources: HashMap<String, Arc<Vec<u8>>>,
    decoded: HashMap<String, Option<usize>>,
    pub list: Vec<Picture>,
}

impl Pictures {
    pub fn new(sources: &[(String, Arc<Vec<u8>>)]) -> Pictures {
        Pictures { sources: sources.iter().map(|(k, v)| (k.trim().to_string(), v.clone())).collect(), ..Pictures::default() }
    }

    /// The picture at this address, if it is one the message carries and it can be printed.
    fn get(&mut self, src: &str) -> Option<usize> {
        let src = src.trim();
        if let Some(found) = self.decoded.get(src) {
            return *found;
        }
        let found = if self.list.len() >= picture::MAX_PICTURES {
            None
        } else {
            self.sources.get(src).and_then(|bytes| picture::decode(bytes)).map(|p| {
                self.list.push(p);
                self.list.len() - 1
            })
        };
        self.decoded.insert(src.to_string(), found);
        found
    }
}

/// What laying out blocks needs to know.
pub(crate) struct Body<'a> {
    pub pictures: &'a mut Pictures,
    /// The tallest a piece may be: the height of a page's body.
    pub max_height: f32,
}

fn run_style(span: &Span, base: Style, bold: bool) -> Style {
    let mut style = base;
    if span.code {
        style.font = FontKind::Mono;
        style.size = base.size * 0.9;
    } else if span.bold || bold {
        style.font = FontKind::Bold;
    }
    style.italic = span.italic;
    style.underline = span.underline;
    style.strike = span.strike;
    if let Some(px) = span.size {
        style.size = (f32::from(px) * 0.75).clamp(7.0, 24.0);
    }
    if let Some(c) = span.color {
        style.color = readable(c);
    }
    if span.link.is_some() && span.color.is_none() {
        style.color = LINK;
        style.underline = true;
    }
    style
}

/// A colour of the message's that reads on white paper: one too pale to read is printed grey.
fn readable(c: Rgb) -> Rgb {
    let luminance = 0.2126 * f32::from(c.0) + 0.7152 * f32::from(c.1) + 0.0722 * f32::from(c.2);
    if luminance > 200.0 { ink::DIM } else { c }
}

fn runs(spans: &[Span], base: Style, bold: bool) -> Vec<Run> {
    spans.iter().map(|s| Run::new(s.text.clone(), run_style(s, base, bold))).collect()
}

/// Display blocks as pieces of a page `width` wide.
pub(crate) fn blocks(body: &mut Body, blocks: &[Block], width: f32, depth: usize, bold: bool) -> Vec<Chunk> {
    let mut out = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let mut chunks = block(body, b, width - quote_indent(b.quote), depth, bold);
        if i > 0 && b.space > 0 && !matches!(b.kind, BlockKind::ListItem { .. }) {
            chunks.insert(0, Chunk::gap(f32::from(b.space.min(4)) * BODY_SIZE * 0.6));
        }
        if b.quote > 0 {
            chunks = quoted(chunks, b.quote);
        }
        out.extend(chunks);
    }
    out
}

fn quote_indent(levels: u8) -> f32 {
    f32::from(levels.min(8)) * 12.0
}

/// Quoted pieces: moved in, with a bar beside them for each level.
fn quoted(chunks: Vec<Chunk>, levels: u8) -> Vec<Chunk> {
    let indent = quote_indent(levels);
    chunks
        .into_iter()
        .map(|c| {
            let mut items: Vec<Item> = c.items.iter().map(|i| i.moved(indent, 0.0)).collect();
            for level in 0..levels.min(8) {
                let x = f32::from(level) * 12.0 + 3.0;
                items.push(Item::Line { x1: x, y1: 0.0, x2: x, y2: c.height, width: 1.5, color: ink::GRID });
            }
            Chunk { height: c.height, items, gap: c.gap }
        })
        .collect()
}

fn block(body: &mut Body, b: &Block, width: f32, depth: usize, bold: bool) -> Vec<Chunk> {
    let base = Style::new(FontKind::Regular, BODY_SIZE);
    // Text that runs right to left (by its first letter) is set against the right edge.
    let rtl = mailcraft_html::bidi::is_rtl(&b.text());
    let align = if rtl && b.align == Align::Left { Align::Right } else { b.align };
    match &b.kind {
        BlockKind::Paragraph => wrap_in(&runs(&b.spans, base, bold), width, align, rtl),
        BlockKind::Heading(level) => {
            let size = match level {
                1 => 18.0,
                2 => 15.0,
                3 => 13.0,
                4 => 12.0,
                _ => 11.0,
            };
            let mut out = vec![Chunk::gap(size * 0.4)];
            out.extend(wrap_in(&runs(&b.spans, Style::new(FontKind::Bold, size), true), width, align, rtl));
            out
        }
        BlockKind::ListItem { marker, depth: level } => {
            let indent = 18.0 * f32::from((*level).min(8) + 1);
            let mut lines = wrap(&runs(&b.spans, base, bold), width - indent, b.align);
            let label = match marker {
                Marker::Bullet => ["•", "◦", "▪"].get(usize::from(*level) % 3).copied().unwrap_or("•").to_string(),
                Marker::Number(n) => format!("{n}."),
                Marker::Label(s) => s.chars().take(12).collect(),
                Marker::None => String::new(),
            };
            for line in &mut lines {
                line.items = line.items.iter().map(|i| i.moved(indent, 0.0)).collect();
            }
            if let Some(first) = lines.first_mut()
                && !label.is_empty()
            {
                let label = font::clean(FontKind::Regular, &label);
                let w = font::width(FontKind::Regular, &label, BODY_SIZE);
                first.items.push(Item::Text(Text {
                    x: (indent - 5.0 - w).max(0.0),
                    y: BODY_SIZE * 1.02,
                    size: BODY_SIZE,
                    font: FontKind::Regular,
                    italic: false,
                    color: ink::TEXT,
                    text: label,
                }));
            }
            lines
        }
        BlockKind::Preformatted => {
            let style = Style::new(FontKind::Mono, 9.0);
            let text: String = b.spans.iter().map(|s| s.text.as_str()).collect();
            wrap(&[Run::new(text, style)], width, Align::Left)
        }
        BlockKind::Rule => {
            vec![Chunk { height: 10.0, items: vec![Item::Line { x1: 0.0, y1: 5.0, x2: width, y2: 5.0, width: 0.75, color: ink::GRID }], gap: false }]
        }
        BlockKind::Image { src, width: w, height: h, .. } => image(body, src, *w, *h, width, b.align),
        BlockKind::Table(t) => table(body, t, width, depth, bold),
    }
}

fn image(body: &mut Body, src: &str, w: Option<u16>, h: Option<u16>, width: f32, align: Align) -> Vec<Chunk> {
    // A picture of two pixels or less is there to be counted, not seen.
    if w.is_some_and(|w| w <= 2) || h.is_some_and(|h| h <= 2) {
        return Vec::new();
    }
    let embedded = mailcraft_html::is_embedded(src);
    let found = if embedded { body.pictures.get(src) } else { None };
    let Some(index) = found else {
        let label = if embedded { "[Picture]" } else { "[Picture from the web: not downloaded]" };
        let style = Style { color: ink::DIM, ..Style::new(FontKind::Regular, 8.5) };
        let text = font::clean(style.font, label);
        let tw = font::width(style.font, &text, style.size) + 12.0;
        let x = match align {
            Align::Left => 0.0,
            Align::Center => ((width - tw) / 2.0).max(0.0),
            Align::Right => (width - tw).max(0.0),
        };
        return vec![Chunk {
            height: 20.0,
            items: vec![
                Item::Rect { x, y: 2.0, w: tw.min(width), h: 16.0, fill: None, stroke: Some(ink::LIGHT), width: 0.75 },
                Item::Text(Text { x: x + 6.0, y: 13.5, size: style.size, font: style.font, italic: false, color: style.color, text }),
            ],
            gap: false,
        }];
    };
    let Some(p) = body.pictures.list.get(index) else { return Vec::new() };
    // As big as the message says (pixels at 96 to the inch), else as its pixels, within the page.
    let (pw, ph) = (p.width as f32, p.height as f32);
    let (mut dw, mut dh) = match (w, h) {
        (Some(w), Some(h)) => (f32::from(w) * 0.75, f32::from(h) * 0.75),
        (Some(w), None) => (f32::from(w) * 0.75, f32::from(w) * 0.75 * ph / pw),
        (None, Some(h)) => (f32::from(h) * 0.75 * pw / ph, f32::from(h) * 0.75),
        (None, None) => (pw * 0.75, ph * 0.75),
    };
    let max_h = body.max_height * 0.9;
    if dw > width {
        dh *= width / dw;
        dw = width;
    }
    if dh > max_h {
        dw *= max_h / dh;
        dh = max_h;
    }
    let x = match align {
        Align::Left => 0.0,
        Align::Center => ((width - dw) / 2.0).max(0.0),
        Align::Right => (width - dw).max(0.0),
    };
    vec![Chunk { height: dh + 4.0, items: vec![Item::Image { x, y: 2.0, w: dw, h: dh, picture: index }], gap: false }]
}

fn stack(chunks: &[Chunk]) -> (f32, Vec<Item>) {
    let mut y = 0.0;
    let mut items = Vec::new();
    for c in chunks {
        items.extend(c.items.iter().map(|i| i.moved(0.0, y)));
        y += c.height;
    }
    (y, items)
}

fn table(body: &mut Body, t: &Table, width: f32, depth: usize, bold: bool) -> Vec<Chunk> {
    let columns = t.columns.clamp(1, 64);
    let linear = |body: &mut Body| {
        let mut out = Vec::new();
        for row in &t.rows {
            for cell in &row.cells {
                out.extend(blocks(body, &cell.blocks, width, depth + 1, bold || cell.header));
            }
        }
        out
    };
    if depth >= MAX_TABLE_NESTING || columns > MAX_GRID_COLUMNS {
        return linear(body);
    }
    let lined = t.border || !t.layout;
    let pad = if lined { 3.0 } else { 1.5 };
    let col = width / f32::from(columns);
    let mut out = Vec::new();
    for row in &t.rows {
        let mut cells = Vec::new();
        let mut x = 0.0;
        for cell in &row.cells {
            let span = f32::from(cell.colspan.clamp(1, columns));
            let w = (col * span).min(width - x);
            if w < 8.0 {
                break;
            }
            let content = blocks(body, &cell.blocks, w - 2.0 * pad, depth + 1, bold || cell.header);
            let (h, items) = stack(&content);
            cells.push((x, w, h, items, cell));
            x += w;
        }
        let row_h = cells.iter().map(|c| c.2).fold(0.0, f32::max) + 2.0 * pad;
        if row_h > body.max_height {
            // Taller than a page: its cells one after another.
            for cell in &row.cells {
                out.extend(blocks(body, &cell.blocks, width, depth + 1, bold || cell.header));
            }
            continue;
        }
        let mut items = Vec::new();
        for (x, w, _, content, cell) in cells {
            if let Some(bg) = cell.background {
                let luminance = 0.2126 * f32::from(bg.0) + 0.7152 * f32::from(bg.1) + 0.0722 * f32::from(bg.2);
                if luminance > 150.0 {
                    items.push(Item::Rect { x, y: 0.0, w, h: row_h, fill: Some(bg), stroke: None, width: 0.0 });
                }
            }
            items.extend(content.iter().map(|i| i.moved(x + pad, pad)));
            if lined || cell.border.any() {
                items.push(Item::Rect { x, y: 0.0, w, h: row_h, fill: None, stroke: Some(ink::GRID), width: 0.5 });
            }
        }
        out.push(Chunk { height: row_h, items, gap: false });
    }
    if out.is_empty() {
        return linear(body);
    }
    out
}

/// Pieces placed on pages, one after another, with what repeats on every page.
pub(crate) struct Flow {
    pub left: f32,
    pub width: f32,
    pub top: f32,
    pub bottom: f32,
    pub pages: Vec<Page>,
    pub y: f32,
    /// The page limit was reached: nothing more is placed.
    pub full: bool,
}

impl Flow {
    pub fn new(paper: Paper, margin: f32, top: f32, bottom: f32) -> Flow {
        Flow {
            left: margin,
            width: paper.width - 2.0 * margin,
            top,
            bottom: paper.height - bottom,
            pages: vec![Page::default()],
            y: top,
            full: false,
        }
    }

    pub fn body_height(&self) -> f32 {
        self.bottom - self.top
    }

    fn at_top(&self) -> bool {
        self.y <= self.top + 0.01
    }

    pub fn new_page(&mut self) {
        if self.full {
            return;
        }
        if self.pages.len() >= MAX_PAGES {
            self.full = true;
            let note = font::clean(FontKind::Regular, &format!("The rest isn't printed: a print has at most {MAX_PAGES} pages."));
            if let Some(page) = self.pages.last_mut() {
                page.items.push(Item::Text(Text {
                    x: self.left,
                    y: self.bottom + 14.0,
                    size: 9.0,
                    font: FontKind::Regular,
                    italic: true,
                    color: ink::DIM,
                    text: note,
                }));
            }
            return;
        }
        self.pages.push(Page::default());
        self.y = self.top;
    }

    /// A piece `x` in from the left margin, on a new page if it doesn't fit on this one.
    pub fn place(&mut self, chunk: &Chunk, x: f32) {
        if self.full {
            return;
        }
        if chunk.gap {
            if !self.at_top() {
                self.y += chunk.height;
            }
            return;
        }
        if self.y + chunk.height > self.bottom + 0.01 && !self.at_top() {
            self.new_page();
            if self.full {
                return;
            }
        }
        let (dx, dy) = (self.left + x, self.y);
        if let Some(page) = self.pages.last_mut() {
            page.items.extend(chunk.items.iter().map(|i| i.moved(dx, dy)));
        }
        self.y += chunk.height;
    }

    pub fn place_all(&mut self, chunks: &[Chunk], x: f32) {
        for c in chunks {
            if self.full {
                break;
            }
            self.place(c, x);
        }
    }
}

/// What every page of a print has: the name at the top, with a heavy rule under it, and the
/// page number at the bottom.
pub(crate) fn decorate(pages: &mut [Page], paper: Paper, margin: f32, name: &str) {
    let name = font::fit(FontKind::Bold, name, 14.0, paper.width - 2.0 * margin);
    for (i, page) in pages.iter_mut().enumerate() {
        let mut head = Vec::new();
        if !name.is_empty() {
            head.push(Item::Text(Text {
                x: margin,
                y: margin - 4.0,
                size: 14.0,
                font: FontKind::Bold,
                italic: false,
                color: ink::TEXT,
                text: name.clone(),
            }));
        }
        head.push(Item::Rect { x: margin, y: margin + 2.0, w: paper.width - 2.0 * margin, h: 2.5, fill: Some(ink::RULE), stroke: None, width: 0.0 });
        let number = format!("{}", i + 1);
        let w = font::width(FontKind::Regular, &number, 9.0);
        page.items.splice(0..0, head);
        page.items.push(Item::Text(Text {
            x: (paper.width - w) / 2.0,
            y: paper.height - margin + 24.0,
            size: 9.0,
            font: FontKind::Regular,
            italic: false,
            color: ink::DIM,
            text: number,
        }));
    }
}
