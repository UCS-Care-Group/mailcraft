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
