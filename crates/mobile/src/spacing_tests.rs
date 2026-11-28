// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the phone's spacing and boxes are drawn from (#1966): blank paragraphs as blank lines, the
//! padding and lines of a box, and a coloured cell that holds nothing.

use crate::blocks::{BlockKindView, BlockView, InsetsView, LengthView, Pictures, TableView, views};

fn shown(html: &str) -> Vec<BlockView> {
    views(&mailcraft_html::parse(html), &Pictures { has_part: &|_| false, width: 400, dark: false })
}

fn table(blocks: &[BlockView]) -> &TableView {
    match blocks.first().map(|b| &b.kind) {
        Some(BlockKindView::Table { table }) => table,
        other => panic!("not a table: {other:?}"),
    }
}

#[test]
fn blank_paragraphs_and_breaks_are_blank_lines_above_the_next() {
    let blocks = shown(
        "<p>Monthly news</p><p>&nbsp;</p><p>&nbsp;</p><p>Second part</p><br><br><p>Third</p>\
         <div style='height:20px'>&nbsp;</div><p>Fourth</p>",
    );
    let spaces: Vec<u8> = blocks.iter().map(|b| b.space).collect();
    assert_eq!(spaces, [0, 3, 3, 2]);
}

#[test]
fn a_framed_box_keeps_its_padding_lines_and_margins() {
    let blocks = shown(
        "<div style='border:1px solid #444444;padding:16px;margin:12px'>\
         <table style='background:#f6f6f6'><tr><td>Invoice INV-1</td><td>Due 1 Dec</td></tr></table></div>",
    );
    let frame = table(&blocks);
    assert!(frame.outline && frame.layout);
    assert!(frame.lines.iter().all(|l| l.width == 1 && l.color == Some(0x444444)));
    let cell = frame.rows.first().and_then(|r| r.cells.first());
    assert_eq!(cell.and_then(|c| c.padding), Some(InsetsView { top: 16, right: 16, bottom: 16, left: 16 }));
    assert_eq!(blocks.first().map(|b| (b.inset_left, b.inset_right)), Some((12, 12)));
}

#[test]
fn a_coloured_cell_that_holds_nothing_is_kept_with_its_width() {
    let blocks = shown(
        "<table width='100%' style='border:1px solid #d0d7de'><tr><td width='4' style='background:#2da44e'></td>\
         <td style='padding:16px'>All systems operational.</td></tr></table>",
    );
    let bar = table(&blocks).rows.first().and_then(|r| r.cells.first());
    assert!(bar.is_some_and(|c| c.blocks.is_empty() && c.background == Some(0x2da44e) && c.width == Some(LengthView::Px { px: 4 })));
}

#[test]
fn a_framed_button_keeps_both_its_frame_and_its_fill() {
    let blocks = shown(
        "<table style='border:2px solid #8eaadb'><tr><td style='background:#1f3864;padding:8px 16px'>\
         <a href='https://example.org/info' style='color:#ffffff'>For further information</a></td></tr></table>",
    );
    let button = table(&blocks);
    assert!(button.outline && button.lines.iter().all(|l| l.width == 2 && l.color == Some(0x8eaadb)));
    assert_eq!(button.rows.first().and_then(|r| r.cells.first()).and_then(|c| c.background), Some(0x1f3864));
}

fn first_cell(blocks: &[BlockView]) -> Option<&crate::blocks::CellView> {
    table(blocks).rows.first().and_then(|r| r.cells.first())
}

fn all(px: u8) -> Option<InsetsView> {
    Some(InsetsView { top: px, right: px, bottom: px, left: px })
}

#[test]
fn cellpadding_reaches_every_cell_that_gives_none() {
    let blocks = shown(
        "<table border=1 cellpadding=10 cellspacing=0><tr><th>Item</th><th>Price</th></tr>\
         <tr><td>Seed trays</td><td style='padding:2px'>£24.00</td></tr></table>",
    );
    let paddings: Vec<_> = table(&blocks).rows.iter().flat_map(|r| &r.cells).map(|c| c.padding).collect();
    assert_eq!(paddings, [all(10), all(10), all(10), all(2)]);
    assert_eq!(first_cell(&shown("<table border=1 cellpadding=900><tr><td>x</td><td>y</td></tr></table>")).and_then(|c| c.padding), all(64));
}

#[test]
fn outlooks_own_spacing_reaches_the_phone_where_the_plain_one_is_missing() {
    let blocks = shown(
        "<table border=1><tr><td style='mso-padding-alt:6pt 12pt'>Order</td><td style='padding:1px;mso-padding-alt:30px'>Note</td></tr></table>",
    );
    let paddings: Vec<_> = table(&blocks).rows.iter().flat_map(|r| &r.cells).map(|c| c.padding).collect();
    assert_eq!(paddings, [Some(InsetsView { top: 8, right: 16, bottom: 8, left: 16 }), all(1)]);
    let blocks = shown("<p style='margin:0'>One</p><p style='mso-margin-top-alt:24px;margin-bottom:0'>Two</p>");
    assert_eq!(blocks.get(1).map(|b| b.gap), Some(24));
}

#[test]
fn an_empty_coloured_block_with_a_height_is_a_bar_and_without_colour_no_room() {
    let blocks = shown("<div style='height:4px;background:#2da44e'></div><div style='border:1px solid #d0d7de'>Card</div>");
    let bar = first_cell(&blocks);
    assert!(bar.is_some_and(|c| c.blocks.is_empty() && c.background == Some(0x2da44e)), "{blocks:?}");
    assert_eq!(bar.and_then(|c| c.padding), Some(InsetsView { top: 4, right: 0, bottom: 0, left: 0 }));
    let blocks = shown("<div style='height:9999px;background:#2da44e'></div>");
    assert_eq!(first_cell(&blocks).and_then(|c| c.padding).map(|p| p.top), Some(64));
    // Without a colour it is no block, and the text after it has no more room than it had.
    let blocks = shown("<p>One</p><div style='height:60px'></div><p>Two</p>");
    assert_eq!(blocks.iter().map(|b| b.space).collect::<Vec<_>>(), [0, 1]);
}
