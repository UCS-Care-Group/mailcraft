//! Contacts in Card Style and tasks (or any list) in Table Style.

use mailcraft_html::Align;

use crate::flow::{self, Chunk, Flow, Run, Style};
use crate::font::{self, FontKind};
use crate::{Document, Item, Paper, Text, ink};

const MARGIN: f32 = 54.0;
/// The most cards or rows printed.
pub const MAX_ROWS: usize = 20_000;

/// A contact's card: the name across the top, then label and value lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub title: String,
    pub fields: Vec<(String, String)>,
}

/// Cards in two columns, down the first and then the second, page after page.
pub fn cards(owner: &str, heading: &str, cards: &[Card], paper: Paper) -> Document {
    let mut flow = Flow::new(paper, MARGIN, MARGIN + 18.0, MARGIN);
    let gutter = 18.0;
    let col = (flow.width - gutter) / 2.0;
    place_heading(&mut flow, heading);
    let label_w = 70.0;
    let mut column = 0;
    let mut top = flow.y;
    for card in cards.iter().take(MAX_ROWS) {
        let mut chunks = Vec::new();
        let name = font::fit(FontKind::Bold, &card.title, 10.5, col - 8.0);
        chunks.push(Chunk {
            height: 18.0,
            items: vec![
                Item::Rect { x: 0.0, y: 0.0, w: col, h: 16.0, fill: Some(ink::SHADE), stroke: None, width: 0.0 },
                Item::Text(Text { x: 4.0, y: 11.5, size: 10.5, font: FontKind::Bold, italic: false, color: ink::TEXT, text: name }),
            ],
            gap: false,
        });
        for (label, value) in card.fields.iter().take(60) {
            let value: String = value.chars().take(2000).collect();
            let mut rows = flow::wrap(&[Run::new(value, Style::new(FontKind::Regular, 8.5))], col - label_w - 4.0, Align::Left);
            rows.truncate(12);
            for row in &mut rows {
                row.items = row.items.iter().map(|i| i.moved(label_w + 4.0, 0.0)).collect();
            }
            if let Some(first) = rows.first_mut() {
                let label = font::fit(FontKind::Regular, label, 8.5, label_w - 4.0);
                first.items.push(Item::Text(Text {
                    x: 4.0,
                    y: 8.5 * 1.02,
                    size: 8.5,
                    font: FontKind::Regular,
                    italic: false,
                    color: ink::DIM,
                    text: label,
                }));
            }
            chunks.extend(rows);
        }
        let height: f32 = chunks.iter().map(|c| c.height).sum::<f32>() + 12.0;
        // The card goes where there is room: this column, the next, or a new page.
        if flow.y + height > flow.bottom && flow.y > top + 0.01 {
            if column == 0 {
                column = 1;
                flow.y = top;
            } else {
                column = 0;
                flow.new_page();
                top = flow.y;
            }
        }
        if flow.full {
            break;
        }
        let x = if column == 0 { 0.0 } else { col + gutter };
        let start_page = flow.pages.len();
        for c in &chunks {
            flow.place(c, x);
        }
        // A card longer than the column ran onto a new page: carry on from there in the first.
        if flow.pages.len() != start_page {
            column = 0;
            top = flow.top;
        }
        flow.y += 12.0;
    }
    finish(flow, paper, owner, heading)
}

/// A column of a table: its heading and its share of the width.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub title: String,
    pub weight: f32,
}

/// Rows under column headings, the headings again at the top of each page.
pub fn table(owner: &str, heading: &str, columns: &[Column], rows: &[Vec<String>], paper: Paper) -> Document {
    let mut flow = Flow::new(paper, MARGIN, MARGIN + 18.0, MARGIN);
    place_heading(&mut flow, heading);
    let total: f32 = columns.iter().map(|c| c.weight.max(0.1)).sum::<f32>().max(0.1);
    let widths: Vec<f32> = columns.iter().map(|c| flow.width * c.weight.max(0.1) / total).collect();
    let pad = 3.0;
    let row_chunk = |cells: &[String], bold: bool, shade: bool| -> Chunk {
        let mut x = 0.0;
        let mut parts = Vec::new();
        for (i, w) in widths.iter().enumerate() {
            let value: String = cells.get(i).map(|s| s.chars().take(2000).collect()).unwrap_or_default();
            let style = Style::new(if bold { FontKind::Bold } else { FontKind::Regular }, 9.0);
            let mut lines = flow::wrap(&[Run::new(value, style)], w - 2.0 * pad, Align::Left);
            lines.truncate(8);
            let mut y = 0.0;
            let mut items = Vec::new();
            for l in &lines {
                items.extend(l.items.iter().map(|it| it.moved(x + pad, y + pad)));
                y += l.height;
            }
            parts.push((items, y));
            x += w;
        }
        let height = parts.iter().map(|p| p.1).fold(0.0, f32::max) + 2.0 * pad;
        let mut items = Vec::new();
        if shade {
            items.push(Item::Rect { x: 0.0, y: 0.0, w: x, h: height, fill: Some(ink::SHADE), stroke: None, width: 0.0 });
        }
        for (p, _) in parts {
            items.extend(p);
        }
        items.push(Item::Line {
            x1: 0.0,
            y1: height,
            x2: x,
            y2: height,
            width: if bold { 0.8 } else { 0.4 },
            color: if bold { ink::RULE } else { ink::GRID },
        });
        Chunk { height, items, gap: false }
    };
    let titles: Vec<String> = columns.iter().map(|c| c.title.clone()).collect();
    let head = row_chunk(&titles, true, true);
    flow.place(&head, 0.0);
    for row in rows.iter().take(MAX_ROWS) {
        let chunk = row_chunk(row, false, false);
        let pages = flow.pages.len();
        if flow.y + chunk.height > flow.bottom {
            flow.new_page();
        }
        if flow.full {
            break;
        }
        if flow.pages.len() != pages {
            flow.place(&head, 0.0);
        }
        flow.place(&chunk, 0.0);
    }
    if rows.is_empty() {
        let none = flow::wrap(
            &[Run::new("There is nothing to print.", Style { color: ink::DIM, ..Style::new(FontKind::Regular, 9.0) })],
            flow.width,
            Align::Left,
        );
        flow.place_all(&none, 0.0);
    }
    finish(flow, paper, owner, heading)
}

fn place_heading(flow: &mut Flow, heading: &str) {
    if heading.is_empty() {
        return;
    }
    let lines = flow::wrap(&[Run::new(heading, Style::new(FontKind::Bold, 16.0))], flow.width, Align::Left);
    flow.place_all(&lines, 0.0);
    flow.place(&Chunk { height: 8.0, items: Vec::new(), gap: false }, 0.0);
}

fn finish(mut flow: Flow, paper: Paper, owner: &str, heading: &str) -> Document {
    let mut pages = std::mem::take(&mut flow.pages);
    flow::decorate(&mut pages, paper, MARGIN, owner);
    Document { title: heading.to_string(), paper, pages, pictures: Vec::new() }
}
