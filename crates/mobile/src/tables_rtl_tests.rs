// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1966: the phone gets each table cell's place in its grid and each block's reading direction,
//! as the reading pane works them out. The messages, people and text are made up.

use crate::blocks::{AlignView, BlockKindView, BlockView, CellView, Pictures, TableView, views};

fn shown(html: &str) -> Vec<BlockView> {
    views(&mailcraft_html::parse(html), &Pictures { has_part: &|_| false, width: 400, dark: false })
}

fn table(blocks: &[BlockView]) -> &TableView {
    blocks
        .iter()
        .find_map(|b| match &b.kind {
            BlockKindView::Table { table } => Some(table),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no table in {blocks:?}"))
}

fn cells(t: &TableView) -> Vec<&CellView> {
    t.rows.iter().flat_map(|r| &r.cells).collect()
}

fn text(c: &CellView) -> String {
    c.blocks.iter().flat_map(|b| &b.spans).map(|s| s.text.as_str()).collect()
}

/// Each cell's text and place: row, column, rows, columns.
fn places(t: &TableView) -> Vec<(String, u32, u16, u32, u16)> {
    cells(t).into_iter().map(|c| (text(c), c.row, c.col, c.rows, c.cols)).collect()
}

const ORDER: &str = "<p>Your order from the garden shop:</p><table border=1>\
    <tr><th>Order</th><th>Item</th><th>Qty</th><th>Price</th></tr>\
    <tr><td rowspan=2>A-1042</td><td>Seed trays</td><td>4</td><td>€12.00</td></tr>\
    <tr><td>Twine, 100 m</td><td>2</td><td>€5.40</td></tr>\
    <tr><td colspan=3>Total</td><td>€17.40</td></tr></table>";

#[test]
fn an_order_table_sends_each_cells_place_with_its_spans() {
    let blocks = shown(ORDER);
    let t = table(&blocks);
    assert!(!t.layout);
    let got = places(t);
    let want = [
        ("Order", 0, 0, 1, 1),
        ("Item", 0, 1, 1, 1),
        ("Qty", 0, 2, 1, 1),
        ("Price", 0, 3, 1, 1),
        ("A-1042", 1, 0, 2, 1),
        ("Seed trays", 1, 1, 1, 1),
        ("4", 1, 2, 1, 1),
        ("€12.00", 1, 3, 1, 1),
        ("Twine, 100 m", 2, 1, 1, 1),
        ("2", 2, 2, 1, 1),
        ("€5.40", 2, 3, 1, 1),
        ("Total", 3, 0, 1, 3),
        ("€17.40", 3, 3, 1, 1),
    ];
    let want: Vec<_> = want.iter().map(|(s, r, c, rs, cs)| (s.to_string(), *r, *c, *rs, *cs)).collect();
    assert_eq!(got, want);
    assert!(cells(t).iter().take(4).all(|c| c.header), "the first row is <th>");
    assert!(cells(t).iter().skip(4).all(|c| !c.header));
}

#[test]
fn a_wide_statement_of_eight_columns_is_a_data_grid_of_single_cells() {
    let head = ["Date", "Ref", "Payee", "Category", "Paid in", "Paid out", "Balance", "Note"];
    let mut html = String::from("<table border=1><tr>");
    head.iter().for_each(|h| html.push_str(&format!("<th>{h}</th>")));
    html.push_str("</tr>");
    for n in 0..3 {
        html.push_str(&format!(
            "<tr><td>0{n} Mar</td><td>TX-90{n}</td><td>Fern Hollow nursery</td><td>Garden</td>\
             <td align=right></td><td align=right>€6{n}.50</td><td align=right>€1,2{n}0.00</td><td>Compost</td></tr>"
        ));
    }
    html.push_str("</table>");
    let blocks = shown(&html);
    let t = table(&blocks);
    assert_eq!((t.columns, t.layout), (8, false));
    for c in cells(t) {
        assert_eq!((c.rows, c.cols), (1, 1), "{}", text(c));
    }
    let last_row: Vec<_> = cells(t).into_iter().filter(|c| c.row == 3).map(|c| c.col).collect();
    assert_eq!(last_row, (0..8).collect::<Vec<u16>>());
    // A column the message aligns keeps its side in every cell.
    let balance = cells(t).into_iter().find(|c| text(c) == "€1,220.00").and_then(|c| c.blocks.first()).map(|b| b.align);
    assert_eq!(balance, Some(AlignView::Right));
}

fn block<'a>(blocks: &'a [BlockView], start: &str) -> &'a BlockView {
    blocks.iter().find(|b| b.spans.first().is_some_and(|s| s.text.starts_with(start))).unwrap_or_else(|| panic!("no block {start:?} in {blocks:?}"))
}

#[test]
fn a_right_to_left_reply_with_a_left_to_right_quote_sends_each_direction() {
    let html = "<div dir=rtl><p>שלום דנה, ההזמנה יצאה היום ותגיע ביום שלישי.</p>\
                <p>مرحبا، الطلب جاهز للشحن منذ الصباح.</p></div>\
                <blockquote dir=ltr><p>On Monday, Ada wrote:</p><p>Could you send the seed trays this week?</p></blockquote>";
    let blocks = shown(html);
    for start in ["שלום", "مرحبا"] {
        let b = block(&blocks, start);
        assert!(b.rtl, "{start}");
        assert_eq!(b.align, AlignView::Right, "{start}");
    }
    let quoted = block(&blocks, "On Monday");
    assert!(!quoted.rtl);
    assert_eq!(quoted.align, AlignView::Left);
    assert!(quoted.quote > 0);
}

#[test]
fn a_mixed_direction_table_sends_each_cells_direction() {
    let html = "<table border=1><tr><th dir=rtl>מוצר</th><th>Item</th></tr>\
                <tr><td dir=rtl>מגש זרעים</td><td>Seed tray</td></tr>\
                <tr><td dir=auto>صندوق أدوات</td><td dir=auto>Tool box</td></tr></table>";
    let blocks = shown(html);
    let t = table(&blocks);
    let dirs: Vec<_> = cells(t).into_iter().map(|c| (text(c), c.blocks.first().map(|b| (b.rtl, b.align)))).collect();
    let r = Some((true, AlignView::Right));
    let l = Some((false, AlignView::Left));
    // A header cell too stands at the side it reads from.
    let want = [("מוצר", r), ("Item", l), ("מגש זרעים", r), ("Seed tray", l), ("صندوق أدوات", r), ("Tool box", l)];
    let want: Vec<_> = want.iter().map(|(s, d)| (s.to_string(), *d)).collect();
    assert_eq!(dirs, want);
}

#[test]
fn plain_text_in_hebrew_reads_from_the_right() {
    let blocks = views(
        &mailcraft_html::text_blocks("שלום דנה,\nהמסמך מצורף.\n\nThanks,\nDana\n"),
        &Pictures { has_part: &|_| false, width: 400, dark: false },
    );
    assert!(block(&blocks, "שלום").rtl);
    assert!(!block(&blocks, "Thanks").rtl);
}
