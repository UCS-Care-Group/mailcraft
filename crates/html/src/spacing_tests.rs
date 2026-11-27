// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The room a message asks for in three old ways (#1966): `cellpadding` on a table, Outlook's own
//! `mso-` spacing, and a `height` on a block or cell of colour that holds nothing (a bar).

use crate::{Block, BlockKind, Cell, Insets, MAX_BOX_PX, Rgb, Table, parse};

fn table(blocks: &[Block]) -> &Table {
    match blocks.first().map(|b| &b.kind) {
        Some(BlockKind::Table(t)) => t,
        other => panic!("not a table: {other:?}"),
    }
}

fn cells(t: &Table) -> Vec<&Cell> {
    t.rows.iter().flat_map(|r| &r.cells).collect()
}

fn all(px: u8) -> Option<Insets> {
    Some(Insets { top: px, right: px, bottom: px, left: px })
}

/// The padding of the first cell of a bordered table with `cellpadding` written as `attr`.
fn cell_padding(attr: &str) -> Option<Insets> {
    let blocks = parse(&format!("<table border=1 cellpadding={attr}><tr><td>Item</td><td>£4.00</td></tr></table>"));
    cells(table(&blocks)).first().and_then(|c| c.padding)
}

#[test]
fn cellpadding_is_the_room_in_every_cell_that_gives_none() {
    let blocks = parse(
        "<table border=1 cellpadding=10><tr><td>Seed trays</td><td style='padding:2px'>£24.00</td>\
         <td style='padding-left:0'>Paid</td></tr></table>",
    );
    let found: Vec<Option<Insets>> = cells(table(&blocks)).iter().map(|c| c.padding).collect();
    // The cell's own padding wins; a side it gives is its own and the others are the table's.
    assert_eq!(found, [all(10), all(2), Some(Insets { top: 10, right: 10, bottom: 10, left: 0 })]);
    assert_eq!(cell_padding("'4px'"), all(4));
    assert_eq!(cell_padding("0"), all(0));
    assert_eq!(cell_padding("2.6"), all(3));
}

#[test]
fn hostile_cellpadding_is_clamped_or_ignored() {
    for huge in ["99999", "1e9", "65", "4294967296"] {
        let px = cell_padding(huge);
        assert!(px.is_none() || px == all(MAX_BOX_PX), "{huge}: {px:?}");
    }
    assert_eq!(cell_padding("65"), all(MAX_BOX_PX));
    for junk in ["-5", "10%", "10em", "calc(5px)", "NaN", "infinity", "''", "x", "'1px 2px'"] {
        assert_eq!(cell_padding(junk), None, "{junk}");
    }
    // Only a table's own cells take it, not those of a table inside them.
    let blocks =
        parse("<table cellpadding=12 border=1><tr><td><table border=1><tr><td>Inner</td><td>x</td></tr></table></td><td>Outer</td></tr></table>");
    let outer = cells(table(&blocks));
    assert_eq!(outer.get(1).and_then(|c| c.padding), all(12));
    let inner = outer.first().and_then(|c| c.blocks.first()).map(|b| match &b.kind {
        BlockKind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).map(|c| c.padding).collect::<Vec<_>>(),
        _ => Vec::new(),
    });
    assert_eq!(inner, Some(vec![None, None]));
}

/// The first cell of what `html` gives, wrapped or not, and its colour.
fn bar_of(html: &str) -> Option<(Option<Insets>, Option<Rgb>, usize)> {
    let blocks = parse(html);
    let BlockKind::Table(t) = &blocks.first()?.kind else { return None };
    let cell = t.rows.first()?.cells.first()?;
    Some((cell.padding, cell.background, cell.blocks.len()))
}

fn top(px: u8) -> Option<Insets> {
    Some(Insets { top: px, ..Insets::default() })
}

#[test]
fn an_empty_block_of_colour_with_a_height_is_a_bar_of_it() {
    let red = Some(Rgb(0xc0, 0x00, 0x30));
    for html in [
        "<div style='height:4px;background:#c00030'></div><p>Card</p>",
        "<div style='height:4px;background-color:#c00030;font-size:0;line-height:0'>&nbsp;</div><p>Card</p>",
        "<p style='height:3pt;background:#c00030;margin:0'></p><p>Card</p>",
    ] {
        assert_eq!(bar_of(html), Some((top(4), red, 0)), "{html}");
    }
    // A cell: its style's height or its attribute's, on top of its own padding.
    assert_eq!(bar_of("<table><tr><td height=4 bgcolor='#c00030'></td></tr></table>"), Some((top(4), red, 0)));
    assert_eq!(
        bar_of("<table><tr><td style='height:6px;background:#c00030;padding:0 8px'>&nbsp;</td></tr></table>"),
        Some((Some(Insets { top: 6, right: 8, bottom: 0, left: 8 }), red, 0))
    );
    // What holds something keeps its blocks, and the height asks nothing of it.
    assert_eq!(bar_of("<div style='height:40px;background:#c00030'>Sale</div>"), Some((None, red, 1)));
}

#[test]
fn a_bar_is_capped_and_a_height_without_colour_makes_no_room() {
    let red = Some(Rgb(0xc0, 0x00, 0x30));
    for huge in ["height:900px", "height:99999999px", "height:100in", "height:4px;height:2000px"] {
        assert_eq!(bar_of(&format!("<div style='{huge};background:#c00030'></div>")), Some((top(MAX_BOX_PX), red, 0)), "{huge}");
    }
    // A repeated height: the last that can be read.
    assert_eq!(bar_of("<div style='height:4px;height:bogus;background:#c00030'></div>"), Some((top(4), red, 0)));
    for junk in ["height:-4px", "height:0", "height:50%", "height:4em", "height:calc(4px)", "height:NaN", "height:auto"] {
        assert_eq!(parse(&format!("<div style='{junk};background:#c00030'></div>")), Vec::new(), "{junk}");
    }
    assert_eq!(
        bar_of("<table><tr><td height='-9' bgcolor='#c00030'>x</td><td height=99999 bgcolor='#c00030'></td></tr></table>").map(|(p, ..)| p),
        Some(None)
    );
    // No colour: no bar, and no room beyond the blank lines there were.
    assert_eq!(parse("<div style='height:40px'></div>"), Vec::new());
    let blocks = parse("<p>One</p><div style='height:40px'></div><p>Two</p>");
    assert_eq!(blocks.iter().map(|b| (b.text(), b.space)).collect::<Vec<_>>(), [("One".into(), 0), ("Two".into(), 1)]);
}
