// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tables floated to a side (`<table align="left">`, `<table align="right">`) stand side by side,
//! as boxes on a line, when they are all their holder holds; anywhere else they stay as they were.

use crate::{Align, Block, BlockKind, Length, MAX_INLINE_BOXES, Table, parse};

/// The boxes on a line at the top of `blocks`, with their blocks.
fn boxes(blocks: &[Block]) -> Vec<(&Block, &Table)> {
    blocks
        .iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Table(t) if t.inline => Some((b, t)),
            _ => None,
        })
        .collect()
}

fn text_of(t: &Table) -> String {
    t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.blocks).map(Block::text).collect::<Vec<_>>().join(" ")
}

/// A label and its value, as a bill writes them: one table at the left and one at the right of a
/// cell.
const PAIR: &str = "<table width='100%'><tr><td style='padding:0 20px'>\
     <table cellpadding=0 cellspacing=0 align='left' border=0><tr><td align='left'>Invoice number</td></tr></table>\n\
     <table cellpadding=0 cellspacing=0 align='right' border=0><tr><td align='right'>INV-0001/10/26</td></tr></table>\n\
     </td></tr></table>";

#[test]
fn a_table_at_the_left_and_one_at_the_right_stand_on_one_line() {
    let blocks = parse(PAIR);
    let found = boxes(&blocks);
    assert_eq!(found.len(), 2, "{blocks:?}");
    let ((label, left), (value, right)) = (found[0], found[1]);
    assert_eq!((text_of(left), text_of(right)), ("Invoice number".to_string(), "INV-0001/10/26".to_string()));
    assert!(left.float && right.float, "{blocks:?}");
    // Each goes to its own side, and the second is in the line of the first, with no gap of white space.
    assert_eq!((label.align, value.align), (Align::Left, Align::Right));
    assert!(!label.beside && value.beside && !right.spaced, "{blocks:?}");
    // No width of their own: as wide as what is in them.
    assert_eq!((left.width, right.width), (None, None));
}

#[test]
fn a_side_written_in_capitals_or_with_spaces_floats_too() {
    let blocks = parse(&PAIR.replace("align='left' border", "align=' LEFT ' border").replace("align='right' border", "align='Right' border"));
    let found = boxes(&blocks);
    assert_eq!(found.len(), 2, "{blocks:?}");
    assert_eq!((found[0].0.align, found[1].0.align), (Align::Left, Align::Right), "{blocks:?}");
}

#[test]
fn a_greeting_and_a_framed_notice_stand_side_by_side_in_their_shares() {
    let html = "<table width='100%'><tr><td valign='top'>\
         <table width='55%' align='left'><tr><td><b>Good morning,</b></td></tr><tr><td>we have issued your invoice.</td></tr></table>\
         <table width='42%' align='left'><tr><td>\
           <table width='100%' align='right'><tr><td style='border:2px solid #7f7f7f'>Stay safe</td></tr></table>\
         </td></tr></table>\
         </td></tr></table>";
    let blocks = parse(html);
    let found = boxes(&blocks);
    assert_eq!(found.len(), 2, "{blocks:?}");
    let ((_, greeting), (notice_block, notice)) = (found[0], found[1]);
    assert_eq!((greeting.width, notice.width), (Some(Length::Percent(55)), Some(Length::Percent(42))));
    assert!(notice_block.beside && notice_block.align == Align::Left, "{blocks:?}");
    // The greeting keeps its two rows (a box isn't unwrapped into the text around it).
    assert_eq!(text_of(greeting), "Good morning, we have issued your invoice.");
    // The frame inside the notice floats alone, the whole width of its holder: a table as before.
    let inner = notice.rows.first().and_then(|r| r.cells.first()).and_then(|c| c.blocks.first());
    assert!(matches!(inner.map(|b| &b.kind), Some(BlockKind::Table(t)) if !t.inline && !t.float), "{inner:?}");
}

#[test]
fn tables_not_floated_side_by_side_stay_where_they_were() {
    let table = |align: &str, text: &str| format!("<table align='{align}'><tr><td>{text}</td></tr></table>");
    let (a, b) = (table("left", "A"), table("right", "B"));
    for html in [
        // Alone: as wide as it is, at its side.
        format!("<div>{b}</div>"),
        // Text or a block beside it would flow round it in a browser.
        format!("<div>{a}{b} and some words</div>"),
        format!("<div>{a}<p>Words</p>{b}</div>"),
        format!("<div>{a}<span>x</span>{b}</div>"),
        format!("<div>{a}<br>{b}</div>"),
        // The whole width: nothing stands beside it.
        format!("<div>{}{}</div>", table("left\" width=\"100%", "A"), table("left' style='width:100%", "B")),
        // Not a side.
        format!("<div>{}{}</div>", table("center", "A"), table("justify", "B")),
        // A table that isn't floated in the same holder.
        format!("<div>{a}<table><tr><td>C</td></tr></table>{b}</div>"),
    ] {
        let blocks = parse(&html);
        assert!(boxes(&blocks).is_empty(), "{html}: {blocks:?}");
        assert!(crate::blocks_to_text(&blocks).contains('B'), "{html}: {blocks:?}");
    }
    // A lone table is what it was: one that only wraps a line of text is that text.
    let blocks = parse(&format!("<div>{b}</div>"));
    assert_eq!(blocks.iter().map(Block::text).collect::<Vec<_>>(), ["B"], "{blocks:?}");
}

#[test]
fn what_is_hidden_or_white_space_between_floated_tables_does_not_part_them() {
    let table = |align: &str, text: &str| format!("<table align='{align}'><tr><td>{text}</td></tr></table>");
    let html =
        format!("<div>\n  {}<!-- a note -->&nbsp;<span style='display:none'>preview</span>\n{}\n</div>", table("left", "A"), table("right", "B"));
    let blocks = parse(&html);
    let found = boxes(&blocks);
    assert_eq!(found.len(), 2, "{blocks:?}");
    assert!(found[1].0.beside && !found[1].1.spaced, "{blocks:?}");
}

#[test]
fn tables_floated_at_the_top_of_a_message_stand_side_by_side_too() {
    let html = "<html><body><table align=left width=200><tr><td>A</td></tr></table>\
                <table align=right width=200><tr><td>B</td></tr></table></body></html>";
    let blocks = parse(html);
    let found = boxes(&blocks);
    assert_eq!(found.len(), 2, "{blocks:?}");
    assert!(found[1].0.beside, "{blocks:?}");
}

#[test]
fn tables_laid_out_as_boxes_by_their_style_stand_in_one_line() {
    // #597 made them boxes, but each stood on a line of its own.
    let blocks = parse(
        "<div><table role='presentation' style='display:inline-block;width:200px'><tr><td>A</td></tr></table>\n\
         <table role='presentation' style='display:inline-block;width:200px'><tr><td>B</td></tr></table></div>",
    );
    let found = boxes(&blocks);
    assert_eq!(found.len(), 2, "{blocks:?}");
    assert!(!found[0].1.float && !found[1].1.float);
    // White space between them is the gap between them, as between other boxes.
    assert!(found[1].0.beside && found[1].1.spaced, "{blocks:?}");
}

#[test]
fn floated_tables_are_capped_as_boxes_are() {
    let many = format!("<div>{}</div>", "<table align=left><tr><td>b</td></tr></table>".repeat(MAX_INLINE_BOXES * 3));
    let blocks = parse(&many);
    assert_eq!(boxes(&blocks).len(), MAX_INLINE_BOXES);
    // Past the cap, the tables are what they were; nothing is lost.
    assert_eq!(crate::blocks_to_text(&blocks).matches('b').count(), MAX_INLINE_BOXES * 3);
}

#[test]
fn floated_tables_in_a_wrapper_in_an_aligned_cell_stand_together_where_it_puts_them() {
    // The wrapper stays a table and stands where the cell's `align` says, as wide as the floats
    // together (a browser's), and the floats in it stand at its sides, the label first.
    for (attr, align) in [("right", Align::Right), ("center", Align::Center)] {
        let html = format!(
            "<table width='600'><tr><td align='{attr}'><table><tr><td>\
               <table align='left'><tr><td>Label</td></tr></table><table align='right'><tr><td>Value</td></tr></table>\
             </td></tr></table></td></tr></table>"
        );
        let blocks = parse(&html);
        // The 600-pixel table's cell holds the wrapper, and the wrapper the pair.
        let cell_blocks = |b: Option<&Block>| match b.map(|b| &b.kind) {
            Some(BlockKind::Table(t)) => t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| c.blocks.clone()).collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let held = cell_blocks(blocks.first());
        let wrapper = held.first();
        assert!(
            matches!(wrapper.map(|b| (&b.kind, b.align)), Some((BlockKind::Table(t), a)) if !t.inline && t.width.is_none() && a == align),
            "{attr}: {blocks:?}"
        );
        let inner = cell_blocks(wrapper);
        let found = boxes(&inner);
        assert_eq!(found.len(), 2, "{attr}: {blocks:?}");
        assert_eq!((found[0].0.align, found[1].0.align), (Align::Left, Align::Right), "{attr}: {blocks:?}");
        assert_eq!((text_of(found[0].1), text_of(found[1].1)), ("Label".to_string(), "Value".to_string()));
    }
}
