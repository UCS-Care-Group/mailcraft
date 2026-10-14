//! A message in Memo Style: the owner's name at the top of every page under which the header
//! block (From, Sent, To, Cc, Subject, Attachments) and then the body, from the same display
//! blocks the reading pane shows, with page numbers at the bottom.

use std::sync::Arc;

use mailcraft_html::{Align, Block};

use crate::flow::{self, BODY_SIZE, Body, Flow, Pictures, Run, Style};
use crate::font::FontKind;
use crate::{Document, Paper};

/// The page's margins.
pub const MARGIN: f32 = 54.0;
const LABEL_WIDTH: f32 = 92.0;

/// A message as it is printed: the header lines as they are shown (dates in the owner's
/// format), the body's blocks, and the pictures the message carries itself by their `cid:`
/// address as the body writes it.
#[derive(Clone, Debug, Default)]
pub struct Memo {
    /// The account owner's name, at the top of each page.
    pub owner: String,
    pub from: String,
    pub sent: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    /// The names of the attachments.
    pub attachments: Vec<String>,
    /// `High` or `Low`; empty for normal importance.
    pub importance: String,
    pub blocks: Vec<Block>,
    pub pictures: Vec<(String, Arc<Vec<u8>>)>,
}

/// One message.
pub fn layout(memo: &Memo, paper: Paper) -> Document {
    layout_many(std::slice::from_ref(memo), paper)
}

/// Messages one after another, each starting on a new page.
pub fn layout_many(memos: &[Memo], paper: Paper) -> Document {
    let mut flow = Flow::new(paper, MARGIN, MARGIN + 18.0, MARGIN);
    let mut pictures = Pictures::default();
    for (i, memo) in memos.iter().enumerate() {
        if i > 0 {
            flow.new_page();
        }
        if flow.full {
            break;
        }
        let mut more = Pictures::new(&memo.pictures);
        // Pictures are numbered across the whole print.
        std::mem::swap(&mut more.list, &mut pictures.list);
        header(&mut flow, memo);
        let width = flow.width;
        let max_height = flow.body_height();
        let chunks = flow::blocks(&mut Body { pictures: &mut more, max_height }, &memo.blocks, width, 0, false);
        flow.place_all(&chunks, 0.0);
        std::mem::swap(&mut more.list, &mut pictures.list);
    }
    let owner = memos.first().map(|m| m.owner.clone()).unwrap_or_default();
    let mut pages = std::mem::take(&mut flow.pages);
    flow::decorate(&mut pages, paper, MARGIN, &owner);
    let title = memos.first().map(|m| m.subject.clone()).unwrap_or_default();
    Document { title, paper, pages, pictures: pictures.list }
}

fn header(flow: &mut Flow, memo: &Memo) {
    let label_style = Style::new(FontKind::Bold, BODY_SIZE);
    let value_style = Style::new(FontKind::Regular, BODY_SIZE);
    let attachments = memo.attachments.iter().take(200).cloned().collect::<Vec<_>>().join("; ");
    let lines: [(&str, &str); 7] = [
        ("From:", &memo.from),
        ("Sent:", &memo.sent),
        ("To:", &memo.to),
        ("Cc:", &memo.cc),
        ("Subject:", &memo.subject),
        ("Attachments:", &attachments),
        ("Importance:", &memo.importance),
    ];
    for (label, value) in lines {
        // To and Subject are always there; the others only when they say something.
        if value.trim().is_empty() && !matches!(label, "To:" | "Subject:") {
            continue;
        }
        // A header can be long (a list of hundreds of recipients): it gets a few lines.
        let value: String = value.chars().take(4000).collect();
        let mut rows = flow::wrap(&[Run::new(value, value_style)], flow.width - LABEL_WIDTH, Align::Left);
        rows.truncate(40);
        let label_rows = flow::wrap(&[Run::new(label, label_style)], LABEL_WIDTH, Align::Left);
        if let (Some(first), Some(l)) = (rows.first_mut(), label_rows.first()) {
            let moved: Vec<_> = first.items.iter().map(|i| i.moved(LABEL_WIDTH, 0.0)).collect();
            first.items = l.items.clone();
            first.items.extend(moved);
        }
        for row in rows.iter_mut().skip(1) {
            row.items = row.items.iter().map(|i| i.moved(LABEL_WIDTH, 0.0)).collect();
        }
        flow.place_all(&rows, 0.0);
    }
    flow.place(&flow::Chunk { height: 16.0, items: Vec::new(), gap: false }, 0.0);
}
