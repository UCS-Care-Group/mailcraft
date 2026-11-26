// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Display-block records: nested tables keep every word (#1967).

use mailcraft_html::{BlockKind, MAX_TABLE_DEPTH};

use crate::blocks::{BlockKindView, BlockView, Pictures, views};

fn words(blocks: &[BlockView], out: &mut String) {
    for b in blocks {
        for s in &b.spans {
            out.push_str(&s.text);
            out.push(' ');
        }
        if let BlockKindView::Table { table } = &b.kind {
            for c in table.rows.iter().flat_map(|r| &r.cells) {
                words(&c.blocks, out);
            }
        }
    }
}

fn depth(blocks: &[BlockView]) -> usize {
    blocks
        .iter()
        .map(|b| match &b.kind {
            BlockKindView::Table { table } => 1 + table.rows.iter().flat_map(|r| &r.cells).map(|c| depth(&c.blocks)).max().unwrap_or(0),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

fn shown(blocks: &[mailcraft_html::Block]) -> Vec<BlockView> {
    views(blocks, &Pictures { has_part: &|_| false, width: 400, dark: false })
}

/// `n` tables one inside the next, each cell starting with `levelN`, the last one with a sentence.
fn nested(n: usize) -> String {
    let mut html = String::new();
    for i in 0..n {
        // (A border keeps a one-cell table a table: the parser unwraps a bare one.)
        html.push_str(&format!("<table border=1><tr><td>level{i} "));
    }
    html.push_str("the bottom line of the message");
    html.push_str(&"</td></tr></table>".repeat(n));
    html
}

#[test]
fn tables_nested_18_and_25_deep_keep_every_word() {
    for n in [18, 25] {
        let shown = shown(&mailcraft_html::parse(&nested(n)));
        let mut text = String::new();
        words(&shown, &mut text);
        for i in 0..n {
            assert!(text.contains(&format!("level{i} ")), "{n} deep: level{i} lost in {text:?}");
        }
        assert!(text.contains("the bottom line of the message"), "{n} deep: {text:?}");
        assert_eq!(depth(&shown), n.min(MAX_TABLE_DEPTH), "{n} deep");
    }
}

#[test]
fn a_table_past_the_parsers_depth_becomes_its_cells_text_in_order() {
    // Wrap the parser's deepest output in one more table: the innermost grid goes, its words stay.
    let mut blocks = mailcraft_html::parse(&nested(MAX_TABLE_DEPTH));
    let mut outer = mailcraft_html::parse("<table border=1><tr><td>outer</td></tr></table>");
    if let Some(BlockKind::Table(t)) = outer.first_mut().map(|b| &mut b.kind)
        && let Some(cell) = t.rows.first_mut().and_then(|r| r.cells.first_mut())
    {
        cell.blocks.append(&mut blocks);
    }
    let shown = shown(&outer);
    assert_eq!(depth(&shown), MAX_TABLE_DEPTH);
    let mut text = String::new();
    words(&shown, &mut text);
    let order: Vec<usize> = ["outer", "level0 ", "level19 ", "the bottom line"].iter().filter_map(|w| text.find(w)).collect();
    assert_eq!(order.len(), 4, "{text:?}");
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{text:?}");
}
