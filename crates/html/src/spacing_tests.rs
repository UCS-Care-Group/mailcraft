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
    // Nothing hidden makes a bar, or a band of colour: not under a hidden element, not a hidden cell.
    for hidden in [
        "<div hidden><div style='height:64px;background:#c00030'></div></div>",
        "<div style='display:none'><div style='height:64px;background:#c00030'></div></div>",
        "<div style='display:none'><table><tr><td height=9 bgcolor='#c00030'></td><td>x</td></tr></table></div>",
        "<table><tr><td bgcolor='#c00030' hidden></td></tr></table>",
        "<table><tr style='display:none' bgcolor='#c00030'><td>x</td></tr></table>",
        "<table style='display:none'><tr bgcolor='#c00030'><td>x</td></tr></table>",
        "<table><tr><td style='display:none;border:2px solid #c00030'></td></tr></table>",
        "<table><tr><td hidden height=9 bgcolor='#c00030'></td></tr></table>",
        "<table><tr><td style='display:none;height:9px;background:#c00030'></td></tr></table>",
    ] {
        let blocks = parse(&format!("{hidden}<p>Visible</p>"));
        assert_eq!(blocks.iter().map(Block::text).collect::<Vec<_>>(), ["Visible"], "{hidden}: {blocks:?}");
    }
    // A row's colour is the bar's where the cell has none.
    assert_eq!(bar_of("<table><tr bgcolor='#c00030'><td height=4></td></tr></table>"), Some((top(4), red, 0)));
    // No colour: no bar, and no room beyond the blank lines there were.
    assert_eq!(parse("<div style='height:40px'></div>"), Vec::new());
    let blocks = parse("<p>One</p><div style='height:40px'></div><p>Two</p>");
    assert_eq!(blocks.iter().map(|b| (b.text(), b.space)).collect::<Vec<_>>(), [("One".into(), 0), ("Two".into(), 1)]);
}

fn css(style: &str) -> crate::style::Css {
    crate::style::Css::parse(style)
}

#[test]
fn outlooks_padding_stands_in_where_the_plain_one_is_missing() {
    assert_eq!(css("mso-padding-alt:6pt 12pt").padding, [Some(8), Some(16), Some(8), Some(16)]);
    assert_eq!(css("mso-padding-alt:0in 5.4pt 0in 5.4pt").padding, [Some(0), Some(7), Some(0), Some(7)]);
    // The plain property wins on every side it gives, whichever comes first.
    assert_eq!(css("padding:2px;mso-padding-alt:30px").padding, [Some(2); 4]);
    assert_eq!(css("mso-padding-alt:30px;padding:2px").padding, [Some(2); 4]);
    assert_eq!(css("padding-left:0;mso-padding-alt:5px").padding, [Some(5), Some(5), Some(5), Some(0)]);
    // Repeated: the last that can be read.
    assert_eq!(css("mso-padding-alt:4px;mso-padding-alt:9px").padding, [Some(9); 4]);
    assert_eq!(css("mso-padding-alt:4px;mso-padding-alt:bogus").padding, [Some(4); 4]);
}

#[test]
fn hostile_outlook_padding_is_clamped_or_ignored() {
    assert_eq!(css("mso-padding-alt:9999px 100in").padding, [Some(MAX_BOX_PX); 4]);
    for junk in ["-5px", "calc(4px)", "10", "auto", "1px 2px 3px 4px 5px", "NaN", "1e3px", "url(x)", ""] {
        assert_eq!(css(&format!("mso-padding-alt:{junk}")).padding, [None; 4], "{junk}");
    }
}

#[test]
fn outlooks_margins_stand_in_where_the_plain_ones_are_missing() {
    let margins = |style: &str| {
        let c = css(style);
        (c.margin[0], c.margin[2], c.margin_top_zero, c.margin_bottom_zero)
    };
    assert_eq!(margins("mso-margin-top-alt:24px;mso-margin-bottom-alt:3px"), (Some(24), Some(3), Some(false), Some(true)));
    assert_eq!(margins("mso-margin-top-alt:12pt"), (Some(16), None, Some(false), None));
    // The plain margin wins, written before or after; `margin` itself counts for both.
    assert_eq!(margins("margin-top:0;mso-margin-top-alt:24px"), (Some(0), None, Some(true), None));
    assert_eq!(margins("mso-margin-bottom-alt:24px;margin-bottom:2px").1, Some(2));
    assert_eq!(margins("margin:0;mso-margin-top-alt:30px;mso-margin-bottom-alt:30px"), (Some(0), Some(0), Some(true), Some(true)));
    // A plain `auto` is a margin written too.
    assert_eq!(margins("margin-top:auto;mso-margin-top-alt:24px").0, None);
    assert_eq!(margins("margin:auto;mso-margin-top-alt:24px;mso-margin-bottom-alt:24px"), (None, None, None, None));
    assert_eq!(margins("mso-margin-bottom-alt:24px;margin-bottom:auto").1, None);
    // `auto` (Word's own paragraph spacing), nonsense and the hostile ask nothing; huge is capped, negative is none.
    for junk in ["auto", "calc(9px)", "10", "NaN", "bogus", ""] {
        assert_eq!(margins(&format!("mso-margin-top-alt:{junk}")), (None, None, None, None), "{junk}");
    }
    assert_eq!(margins("mso-margin-top-alt:99999px").0, Some(MAX_BOX_PX));
    assert_eq!(margins("mso-margin-top-alt:-40px"), (Some(0), None, Some(true), None));
    assert_eq!(margins("mso-margin-top-alt:8px;mso-margin-top-alt:auto").0, None);
    // A plain margin that cannot be read says nothing: Outlook's stands in.
    assert_eq!(margins("margin-top:10;mso-margin-top-alt:24px"), (Some(24), None, Some(false), None));
    assert_eq!(margins("mso-margin-bottom-alt:2px;margin-bottom:calc(9px)").1, Some(2));
    assert_eq!(margins("mso-margin-top-alt:8px;mso-margin-top-alt:bogus").0, Some(8));
}

#[test]
fn outlooks_spacing_reaches_the_blocks() {
    let blocks = parse("<p style='margin:0'>One</p><p style='mso-margin-top-alt:24px;margin-bottom:0'>Two</p>");
    assert_eq!(blocks.get(1).map(|b| b.gap), Some(24));
    let blocks = parse("<table border=1 cellpadding=3><tr><td style='mso-padding-alt:6pt 12pt'>Order</td><td>Due</td></tr></table>");
    let found: Vec<Option<Insets>> = cells(table(&blocks)).iter().map(|c| c.padding).collect();
    // A cell's own Outlook padding is its own, and beats the table's `cellpadding`.
    assert_eq!(found, [Some(Insets { top: 8, right: 16, bottom: 8, left: 16 }), all(3)]);
}

#[test]
fn a_divider_cell_given_no_room_is_still_a_pixel() {
    let grey = Some(Rgb(0xcc, 0xcc, 0xcc));
    // Its text too small to keep: a pixel, as a browser draws it, not nothing.
    let html = "<table cellpadding=0 cellspacing=0 width=600><tr><td bgcolor=#ccc style='font-size:1px;line-height:1px'>&nbsp;</td></tr></table>";
    assert_eq!(bar_of(html), Some((top(1), grey, 0)));
    // Nothing in it, or text of no size at all: no pixel, as in a browser.
    assert_eq!(bar_of("<table><tr bgcolor=#ccc><td style='padding:0'></td><td>x</td></tr></table>"), Some((top(0), grey, 0)));
    let none = "<table cellpadding=0><tr><td bgcolor=#ccc style='font-size:0;line-height:0'>&nbsp;</td><td>x</td></tr></table>";
    assert_eq!(bar_of(none), Some((top(0), grey, 0)));
    // Room given, or none said (the reader's own band): as it was.
    assert_eq!(bar_of("<table cellpadding=2><tr><td bgcolor=#ccc></td></tr></table>"), Some((all(2), grey, 0)));
    assert_eq!(bar_of("<table><tr><td bgcolor=#ccc></td></tr></table>"), Some((None, grey, 0)));
}

/// The texts of each row's cells, `-` for one that holds none.
fn grid(html: &str) -> Vec<Vec<String>> {
    let blocks = parse(html);
    table(&blocks)
        .rows
        .iter()
        .map(|r| r.cells.iter().map(|c| if c.blocks.is_empty() { "-".to_string() } else { c.blocks.iter().map(Block::text).collect() }).collect())
        .collect()
}

#[test]
fn a_cell_hidden_but_in_its_place_keeps_its_column() {
    // `visibility:hidden` and the like keep the cell's room, as in a browser: C stays under B.
    for how in ["visibility:hidden", "opacity:0", "max-height:0", "max-width:0", "visibility:collapse"] {
        let html = format!(
            "<table border=1><tr><td>A</td><td>B</td></tr><tr><td style='{how};background:#c00030;border:2px solid red'>x</td><td>C</td></tr></table>"
        );
        assert_eq!(grid(&html), [vec!["A", "B"], vec!["-", "C"]], "{how}");
        let blocks = parse(&html);
        let hidden = table(&blocks).rows.get(1).and_then(|r| r.cells.first());
        assert!(
            hidden.is_some_and(|c| c.background.is_none() && !c.border.any() && c.lines.iter().all(|l| !l.is_set()) && c.padding.is_none()),
            "{how}: {hidden:?}"
        );
    }
    // Its row's colour does not paint it either.
    let blocks = parse("<table border=1><tr bgcolor=red><td style='visibility:hidden'>x</td><td>C</td></tr></table>");
    let colours: Vec<_> = cells(table(&blocks)).iter().map(|c| c.background).collect();
    assert_eq!(colours, [None, Some(Rgb(255, 0, 0))]);
    // A layout table keeps its column too, rather than giving it to the next cell.
    let html = "<table role=presentation width=400><tr><td width=200 style='visibility:hidden'>x</td><td>C</td></tr></table>";
    assert_eq!(grid(html), [vec!["-", "C"]]);
    // A row whose cells are all hidden in place keeps its place under a row span.
    let html =
        "<table border=1><tr><td rowspan=2>A</td><td>B</td></tr><tr><td style='visibility:hidden'>X</td></tr><tr><td>C</td><td>D</td></tr></table>";
    assert_eq!(grid(html), [vec!["A", "B"], vec!["-"], vec!["C", "D"]]);
    // A table all of whose cells are hidden shows nothing.
    assert_eq!(parse("<table><tr><td style='visibility:hidden'>x</td></tr></table><p>After</p>").len(), 1);
    // A row span keeps its rows too.
    let html =
        "<table border=1><tr><td rowspan=2 style='visibility:hidden'>x</td><td>A</td></tr><tr><td>B</td></tr><tr><td>C</td><td>D</td></tr></table>";
    let blocks = parse(html);
    let first = table(&blocks).rows.first().map(|r| r.cells.iter().map(|c| c.rowspan).collect::<Vec<_>>());
    assert_eq!(first, Some(vec![2, 1]));
    assert_eq!(grid(html), [vec!["-", "A"], vec!["B"], vec!["C", "D"]]);
    // A cell that is not there (`display:none`, `hidden`) is gone, as in a browser.
    assert_eq!(
        grid("<table border=1><tr><td>A</td><td>B</td></tr><tr><td style='display:none'>x</td><td>C</td></tr></table>"),
        [vec!["A", "B"], vec!["C"]]
    );
    assert_eq!(grid("<table border=1><tr><td>A</td><td>B</td></tr><tr><td hidden>x</td><td>C</td></tr></table>"), [vec!["A", "B"], vec!["C"]]);
}

#[test]
fn a_hidden_row_of_a_composed_grid_is_left_out() {
    let blocks = crate::parse_text_with_tables("<table><tr><td>A</td></tr><tr style='display:none'><td>B</td></tr><tr><td>C</td></tr></table>", None);
    let rows = table(&blocks).rows.len();
    assert_eq!(rows, 2, "{blocks:?}");
}
