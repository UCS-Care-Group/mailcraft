// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Display-block records: nested tables keep every word (#1967).

use mailcraft_html::{BlockKind, MAX_TABLE_DEPTH};

use mailcraft_html::reading;

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

/// A reply's signature as a typical mail program writes it (invented people, example.org): a logo,
/// a thin bar of colour, the name and numbers, a row of small icons floated left, a link drawn
/// without its line (#1966).
const SIGNATURE: &str = r##"<p>Thanks, see you Thursday.</p>
<table cellpadding="0" cellspacing="0" style="border-collapse:collapse">
<tr>
<td width="96" valign="top"><img src="cid:logo@example.org" width="88" height="88" alt="Fern Hollow"></td>
<td width="3" style="background-color:#2e7d32"></td>
<td style="padding-left:12px" valign="top">
<p><b>Ada Example</b><br>Head gardener</p>
<p>T: <a href="tel:+441632960000" style="text-decoration:none">01632 960000</a></p>
<div>
<table align="left" cellpadding="0" cellspacing="0"><tr><td style="padding-right:6px"><a href="https://social.example/a"><img src="https://img.example.org/a.png" width="20" height="20" alt="A"></a></td></tr></table>
<table align="left" cellpadding="0" cellspacing="0"><tr><td style="padding-right:6px"><a href="https://social.example/b"><img src="https://img.example.org/b.png" width="20" height="20" alt="B"></a></td></tr></table>
<table align="left" cellpadding="0" cellspacing="0"><tr><td style="padding-right:6px"><a href="https://social.example/c"><img src="https://img.example.org/c.png" width="20" height="20" alt="C"></a></td></tr></table>
<table align="left" cellpadding="0" cellspacing="0"><tr><td style="padding-right:6px"><a href="https://social.example/d"><img src="https://img.example.org/d.png" width="20" height="20" alt="D"></a></td></tr></table>
</div>
</td>
</tr>
</table>"##;

fn tables(blocks: &[BlockView], out: &mut Vec<crate::blocks::TableView>) {
    for b in blocks {
        if let BlockKindView::Table { table } = &b.kind {
            out.push(table.clone());
            for c in table.rows.iter().flat_map(|r| &r.cells) {
                tables(&c.blocks, out);
            }
        }
    }
}

#[test]
fn a_signature_carries_the_desktops_rules_for_its_bar_its_icons_and_its_links() {
    let shown = shown(&mailcraft_html::parse(SIGNATURE));
    let mut all = Vec::new();
    tables(&shown, &mut all);
    assert_eq!(all.len(), 5, "the signature and its four icon boxes");
    // Every table is stacked below the desktop's least column, not a rule of the phone's own.
    assert!(all.iter().all(|t| t.min_column == reading::MIN_LAYOUT_COLUMN));
    let Some(sig) = all.first() else { return };
    let cells: Vec<_> = sig.rows.iter().flat_map(|r| &r.cells).collect();
    // The bar holds nothing: it keeps its colour and its width (its band of room is the phone's
    // cell rule, as on the desktop).
    let bar = cells.get(1).copied();
    assert_eq!(bar.map(|c| c.blocks.len()), Some(0));
    assert_eq!(bar.and_then(|c| c.background), Some(0x2e7d32));
    assert_eq!(bar.and_then(|c| c.width), Some(crate::blocks::LengthView::Px { px: 3 }));
    // The icons are floated boxes on one line, touching.
    let icons = all.get(1..).unwrap_or_default();
    assert!(icons.iter().all(|t| t.inline && t.float && t.gap_before == 0));
    // The number is a link drawn without its line.
    let mut bare = Vec::new();
    spans_of(&shown, &mut bare);
    assert!(bare.iter().any(|s| s.text.contains("01632 960000") && s.bare));
    assert!(bare.iter().any(|s| s.text.contains("Thanks") && !s.bare));
}

#[test]
fn boxes_parted_by_white_space_have_the_desktops_gap() {
    let html = r#"<div><div style="display:inline-block;width:120px">Ada Example</div> <div style="display:inline-block;width:120px">Bo Example</div></div>"#;
    let shown = shown(&mailcraft_html::parse(html));
    let mut all = Vec::new();
    tables(&shown, &mut all);
    let gaps: Vec<u8> = all.iter().filter(|t| t.inline).map(|t| t.gap_before).collect();
    assert_eq!(gaps, vec![0, reading::BOX_GAP]);
}

fn spans_of(blocks: &[BlockView], out: &mut Vec<crate::blocks::SpanView>) {
    for b in blocks {
        out.extend(b.spans.iter().cloned());
        if let BlockKindView::Table { table } = &b.kind {
            for c in table.rows.iter().flat_map(|r| &r.cells) {
                spans_of(&c.blocks, out);
            }
        }
    }
}
