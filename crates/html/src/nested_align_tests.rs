// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The `align` of a cell, a row, a `<div>` or a `<p>`, and a `<center>`, don't reach the text of
//! a table inside them, as in a browser; a style's `text-align` does. (So Chrome does for a
//! message with a doctype; without one, it stops every alignment at a table.)

use crate::{Align, Block, BlockKind, parse};

/// Every block that holds text or a picture, with its alignment, tables opened.
fn aligned(blocks: &[Block]) -> Vec<(String, Align)> {
    let mut out = Vec::new();
    for block in blocks {
        match &block.kind {
            BlockKind::Table(t) => {
                for cell in t.rows.iter().flat_map(|r| &r.cells) {
                    out.extend(aligned(&cell.blocks));
                }
            }
            BlockKind::Image { .. } => out.push(("[picture]".to_string(), block.align)),
            _ => out.push((block.text(), block.align)),
        }
    }
    out
}

fn align_of(blocks: &[Block], text: &str) -> Option<Align> {
    aligned(blocks).into_iter().find(|(t, _)| t == text).map(|(_, a)| a)
}

/// A greeting in a table the whole width of a cell that centres what it holds.
fn greeting(around: &str, close: &str) -> String {
    format!(
        "<table width='100%'><tr>{around}\
           <table width='100%'><tr><td><b>Good morning,</b></td></tr><tr><td>we have issued your invoice.</td></tr></table>\
         {close}</tr></table>"
    )
}

#[test]
fn an_aligning_attribute_outside_a_table_leaves_its_text_at_the_left() {
    for (around, close) in [
        ("<td align='center'>", "</td>"),
        ("<td><center>", "</center></td>"),
        ("<td><div align='center'>", "</div></td>"),
        ("<td align='right'>", "</td>"),
    ] {
        let blocks = parse(&greeting(around, close));
        assert_eq!(align_of(&blocks, "we have issued your invoice."), Some(Align::Left), "{around}: {blocks:?}");
        assert_eq!(align_of(&blocks, "Good morning,"), Some(Align::Left), "{around}: {blocks:?}");
    }
    // A row's `align` is the same.
    let blocks = parse(&greeting("<td>", "</td>").replace("<table width='100%'><tr><td>", "<table width='100%'><tr align='center'><td>"));
    assert_eq!(align_of(&blocks, "we have issued your invoice."), Some(Align::Left), "{blocks:?}");
}

#[test]
fn a_style_that_centres_still_reaches_into_a_table() {
    for around in ["<td style='text-align:center'>", "<td align='right' style='text-align:center'>"] {
        let blocks = parse(&greeting(around, "</td>"));
        assert_eq!(align_of(&blocks, "we have issued your invoice."), Some(Align::Center), "{around}: {blocks:?}");
    }
    // A style that puts it back at the left inside a centring attribute: the left it is.
    let blocks = parse(&greeting("<td align='center'><div style='text-align:right'>", "</div></td>"));
    assert_eq!(align_of(&blocks, "we have issued your invoice."), Some(Align::Right), "{blocks:?}");
    // And a centring attribute inside a style: the table's text starts again at the left.
    let blocks = parse(&greeting("<td style='text-align:right'><div align='center'>", "</div></td>"));
    assert_eq!(align_of(&blocks, "we have issued your invoice."), Some(Align::Left), "{blocks:?}");
}

#[test]
fn inside_the_table_its_own_alignment_holds_and_beside_it_the_centring_does() {
    let html = "<table width='100%'><tr><td align='center'>\
                  <p>Your invoice</p>\
                  <table width='100%'><tr><td align='right'>Total</td></tr><tr><td style='text-align:center'>Thank you</td></tr>\
                  <tr><td>Questions?</td></tr></table>\
                  <p>See you</p>\
                </td></tr></table>";
    let blocks = parse(html);
    assert_eq!(align_of(&blocks, "Your invoice"), Some(Align::Center), "{blocks:?}");
    assert_eq!(align_of(&blocks, "See you"), Some(Align::Center), "{blocks:?}");
    assert_eq!(align_of(&blocks, "Total"), Some(Align::Right), "{blocks:?}");
    assert_eq!(align_of(&blocks, "Thank you"), Some(Align::Center), "{blocks:?}");
    assert_eq!(align_of(&blocks, "Questions?"), Some(Align::Left), "{blocks:?}");
    // The table's own style sets its text.
    let blocks = parse(
        &greeting("<td align='center'>", "</td>")
            .replace("<table width='100%'><tr><td><b>", "<table width='100%' style='text-align:right'><tr><td><b>"),
    );
    assert_eq!(align_of(&blocks, "Good morning,"), Some(Align::Right), "{blocks:?}");
}

#[test]
fn a_framed_table_stands_centred_with_its_text_at_the_left() {
    let html = "<table width='100%'><tr><td align='center'>\
                  <table width='300' style='border:1px solid #7f7f7f'><tr><td>Stay safe</td></tr><tr><td>Check the sender.</td></tr></table>\
                </td></tr></table>";
    let blocks = parse(html);
    let frame = blocks.iter().find(|b| matches!(&b.kind, BlockKind::Table(t) if t.outline));
    assert_eq!(frame.map(|b| b.align), Some(Align::Center), "{blocks:?}");
    assert_eq!(align_of(&blocks, "Stay safe"), Some(Align::Left), "{blocks:?}");
    assert_eq!(align_of(&blocks, "Check the sender."), Some(Align::Left), "{blocks:?}");
}

#[test]
fn a_table_as_wide_as_what_it_holds_still_stands_where_the_centring_puts_it() {
    // A logo in a wrapper table: a browser makes the table as wide as the picture and centres it.
    let logo = "<table width='100%'><tr><td align='center'>\
                  <table cellpadding='0' cellspacing='0' border='0'><tr><td><img src='cid:logo@example.org' width='120' height='40'></td></tr></table>\
                </td></tr></table>";
    let blocks = parse(logo);
    assert_eq!(align_of(&blocks, "[picture]"), Some(Align::Center), "{blocks:?}");
    // One line of words too.
    let blocks = parse(&logo.replace("<img src='cid:logo@example.org' width='120' height='40'>", "Invoice"));
    assert_eq!(align_of(&blocks, "Invoice"), Some(Align::Center), "{blocks:?}");
    // A table inside a table that starts again at the left stands at the left.
    let twice = "<table width='100%'><tr><td align='center'>\
                   <table width='100%'><tr><td>\
                     <table width='200' style='border:1px solid #7f7f7f'><tr><td>Inner</td></tr></table>\
                   </td></tr></table>\
                 </td></tr></table>";
    let blocks = parse(twice);
    let inner = blocks.iter().find(|b| matches!(&b.kind, BlockKind::Table(t) if t.outline));
    assert_eq!(inner.map(|b| b.align), Some(Align::Left), "{blocks:?}");
}

#[test]
fn the_align_of_a_heading_a_list_or_the_body_reaches_into_a_table() {
    // A browser starts a table again at the left only from the `align` of a cell, a row, a
    // `<div>` or a `<p>`, and from a `<center>`; any other `align` aligns as a style does.
    let table = "<table width='100%'><tr><td>Hello</td></tr><tr><td>World</td></tr></table>";
    for (open, close) in [
        ("<body align='center'>", "</body>"),
        ("<section align='center'>", "</section>"),
        ("<ul align='center'><li>", "</li></ul>"),
        ("<h2 align='center'>", "</h2>"),
    ] {
        let blocks = parse(&format!("{open}{table}{close}"));
        assert_eq!(align_of(&blocks, "World"), Some(Align::Center), "{open}: {blocks:?}");
    }
}

#[test]
fn words_in_a_table_outside_its_cells_are_aligned_as_round_the_table() {
    // They stand before the table, as in a browser, so the cell round it centres them.
    let html = "<table width='600'><tr><td align='center'>\
                  <table width='300' border='1'><tr><td>Item</td></tr>Thanks for your order<tr><td>Total</td></tr></table>\
                </td></tr></table>";
    let blocks = parse(html);
    assert_eq!(align_of(&blocks, "Thanks for your order"), Some(Align::Center), "{blocks:?}");
    assert_eq!(align_of(&blocks, "Item"), Some(Align::Left), "{blocks:?}");
    // In a row, outside its cells, too; and a row's own `align` isn't theirs.
    let blocks = parse(&html.replace("<tr><td>Total", "<tr align='right'>Paid<td>Total"));
    assert_eq!(align_of(&blocks, "Paid"), Some(Align::Center), "{blocks:?}");
    assert_eq!(align_of(&blocks, "Total"), Some(Align::Right), "{blocks:?}");
    // In an element there, and a picture too; one that aligns them, as it says.
    for (between, text) in [
        ("<b>Thanks</b>", "Thanks"),
        ("<div>Thanks</div>", "Thanks"),
        ("<span style='color:#336699'>Thanks</span>", "Thanks"),
        ("<a href='https://shop.example/'><img src='cid:logo@example.org' width='20' height='20'></a>", "[picture]"),
    ] {
        let blocks = parse(&html.replace("Thanks for your order", between));
        assert_eq!(align_of(&blocks, text), Some(Align::Center), "{between}: {blocks:?}");
    }
    let blocks = parse(&html.replace("Thanks for your order", "<div align='right'>Thanks</div>"));
    assert_eq!(align_of(&blocks, "Thanks"), Some(Align::Right), "{blocks:?}");
}

#[test]
fn a_column_narrower_than_the_line_stands_where_the_centring_puts_it() {
    // A wrapper a share of the line wide in a centred cell stays a table, centred, with its text
    // at the left; one the whole line wide is unwrapped, as before.
    for (attr, align) in [("center", Align::Center), ("right", Align::Right)] {
        let blocks =
            parse(&format!("<table width='600'><tr><td align='{attr}'><table width='80%'><tr><td>Hello there</td></tr></table></td></tr></table>"));
        let held: Vec<&Block> = match blocks.first().map(|b| &b.kind) {
            Some(BlockKind::Table(t)) => t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.blocks).collect(),
            _ => Vec::new(),
        };
        assert!(
            matches!(held.first().map(|b| (&b.kind, b.align)), Some((BlockKind::Table(t), a)) if t.width == Some(crate::Length::Percent(80)) && a == align),
            "{attr}: {blocks:?}"
        );
        assert_eq!(align_of(&blocks, "Hello there"), Some(Align::Left), "{attr}: {blocks:?}");
    }
    let blocks = parse("<table width='600'><tr><td align='center'><table width='100%'><tr><td>Hello there</td></tr></table></td></tr></table>");
    let held: Vec<&Block> = match blocks.first().map(|b| &b.kind) {
        Some(BlockKind::Table(t)) => t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.blocks).collect(),
        _ => Vec::new(),
    };
    assert!(matches!(held.as_slice(), [b] if !matches!(b.kind, BlockKind::Table(_))), "{blocks:?}");
    assert_eq!(align_of(&blocks, "Hello there"), Some(Align::Left), "{blocks:?}");
}

#[test]
fn a_heading_cell_in_a_table_that_starts_again_at_the_left_is_centred() {
    // A browser centres a `<th>` unless what is round it aligns it.
    let html = "<table width='100%'><tr><td align='center'>\
                  <table width='300'><tr><th>Item</th><th align='right'>Price</th></tr><tr><td>Tea</td><td>3</td></tr></table>\
                </td></tr></table>";
    let blocks = parse(html);
    assert_eq!(align_of(&blocks, "Item"), Some(Align::Center), "{blocks:?}");
    assert_eq!(align_of(&blocks, "Price"), Some(Align::Right), "{blocks:?}");
    assert_eq!(align_of(&blocks, "Tea"), Some(Align::Left), "{blocks:?}");
    // A row that aligns it: as the row says.
    let blocks = parse(&html.replace("<tr><th>Item", "<tr align='left'><th>Item"));
    assert_eq!(align_of(&blocks, "Item"), Some(Align::Left), "{blocks:?}");
}

#[test]
fn a_table_too_deep_to_be_laid_out_keeps_its_own_alignment() {
    let open = "<table><tr><td>".repeat(crate::MAX_TABLE_DEPTH + 1);
    let html = format!(
        "{open}<table><tr><td align='center'><table style='text-align:right'><tr><td><b>Deep</b> and more</td></tr></table></td></tr></table>"
    );
    let blocks = parse(&html);
    let found = aligned(&blocks).into_iter().find(|(t, _)| t.contains("Deep"));
    assert_eq!(found.map(|(_, a)| a), Some(Align::Right), "{blocks:?}");
}
