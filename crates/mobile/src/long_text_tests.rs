// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A text block is sent in pieces the phone can draw: no piece past [`MAX_BLOCK_CHARS`], every
//! character kept in order (#1966).

use mailcraft_html::{Block, BlockKind, Marker, Span};

use crate::blocks::{BlockKindView, BlockView, MAX_BLOCK_CHARS, MarkerView, Pictures, views};

fn of(blocks: &[Block]) -> Vec<BlockView> {
    views(blocks, &Pictures { has_part: &|_| false, width: 400, dark: false })
}

fn paragraph(text: &str) -> Block {
    Block::new(BlockKind::Paragraph, vec![Span::plain(text)])
}

fn chars(v: &BlockView) -> usize {
    v.spans.iter().map(|s| s.text.chars().count()).sum()
}

/// What is left of the text when every space and line break is taken out: what must survive.
fn ink(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn ink_of(views: &[BlockView]) -> String {
    views.iter().flat_map(|v| &v.spans).map(|s| ink(&s.text)).collect()
}

#[test]
fn a_block_of_the_most_characters_goes_whole_and_one_more_goes_in_two() {
    let whole = "a".repeat(MAX_BLOCK_CHARS);
    assert_eq!(of(&[paragraph(&whole)]).len(), 1);
    let more = format!("{whole}b");
    let views = of(&[paragraph(&more)]);
    assert_eq!(views.len(), 2);
    assert_eq!(ink_of(&views), more);
}

#[test]
fn a_paragraph_of_130_000_lines_goes_as_pieces_of_whole_lines() {
    let text = "x\n".repeat(130_000);
    let views = of(&[paragraph(&text)]);
    assert!(views.len() > 1);
    assert!(views.iter().all(|v| chars(v) <= MAX_BLOCK_CHARS), "a piece is too long");
    assert_eq!(ink_of(&views), ink(&text));
    // Cut at line breaks: each piece starts a line of its own, with no room above, not beside.
    assert!(views.iter().skip(1).all(|v| !v.beside && v.space == 0 && v.gap == 0 && v.spans.iter().all(|s| !s.text.starts_with('\n'))));
}

#[test]
fn a_word_longer_than_a_block_is_cut_between_characters_and_kept() {
    // Letters of two, three and four bytes: a cut never falls inside one.
    let text = "é漢🙂".repeat(5_000);
    let views = of(&[paragraph(&text), paragraph("after")]);
    assert!(views.iter().all(|v| chars(v) <= MAX_BLOCK_CHARS));
    assert_eq!(ink_of(&views), format!("{text}after"));
}

#[test]
fn text_is_cut_after_a_space_where_it_has_no_line_break() {
    let text = "seed trays ".repeat(1_000);
    let views = of(&[paragraph(&text)]);
    assert!(views.len() > 1);
    assert!(views.iter().take(views.len() - 1).all(|v| v.spans.last().is_some_and(|s| s.text.ends_with(' '))));
    assert_eq!(ink_of(&views), ink(&text));
}

#[test]
fn spans_keep_their_look_across_a_cut() {
    let mut bold = Span::plain("b".repeat(MAX_BLOCK_CHARS));
    bold.bold = true;
    let views = of(&[Block::new(BlockKind::Paragraph, vec![Span::plain("plain "), bold])]);
    assert_eq!(views.len(), 2);
    assert!(views.get(1).is_some_and(|v| v.spans.iter().all(|s| s.bold)));
}

#[test]
fn a_long_list_item_has_its_marker_once() {
    let item = Block::new(BlockKind::ListItem { marker: Marker::Bullet, depth: 1 }, vec![Span::plain("y ".repeat(MAX_BLOCK_CHARS))]);
    let views = of(&[item]);
    assert_eq!(views.len(), 2);
    assert!(matches!(views.first().map(|v| &v.kind), Some(BlockKindView::ListItem { marker: MarkerView::Bullet, depth: 1 })));
    assert!(matches!(views.get(1).map(|v| &v.kind), Some(BlockKindView::ListItem { marker: MarkerView::Nothing, depth: 1 })));
}

#[test]
fn text_going_on_in_a_line_starts_a_new_one_past_the_most_a_block_has() {
    let part = |beside: bool| {
        let mut b = paragraph(&"z".repeat(MAX_BLOCK_CHARS / 3 + 1));
        b.beside = beside;
        b
    };
    let views = of(&[part(false), part(true), part(true)]);
    // Two thirds and a bit go on in one line; the third would pass the most, and starts its own.
    assert_eq!(views.iter().map(|v| v.beside).collect::<Vec<_>>(), [false, true, false]);
}

#[test]
fn a_hostile_body_of_a_million_characters_is_sent_whole_in_as_few_blocks_as_it_needs() {
    let text = "w".repeat(1_000_000);
    let views = of(&[paragraph(&text)]);
    // Counted, not timed: each piece is as full as it may be, so the work is one pass.
    assert_eq!(views.len(), 1_000_000 / MAX_BLOCK_CHARS);
    assert!(views.iter().all(|v| chars(v) <= MAX_BLOCK_CHARS));
    assert_eq!(ink_of(&views).len(), text.len());
}

/// The text of each block of [`of`] `text` once its first `before` characters are `a`s.
fn cut_after(before: usize, rest: &str) -> Vec<String> {
    let text = format!("{}{rest}", "a".repeat(before));
    of(&[paragraph(&text)]).iter().map(|v| v.spans.iter().map(|s| s.text.as_str()).collect()).collect()
}

#[test]
fn a_cut_between_characters_keeps_each_letter_whole() {
    // Each cut would fall inside the letter after 3,999 `a`s: a family joined by ZWJs, an `e` with
    // its accent, a heart with its emoji selector, a flag of two regional indicators, a skin tone.
    for letter in ["👨\u{200D}👩\u{200D}👧", "e\u{301}", "❤\u{FE0F}", "🇬🇧", "👋🏽", "ש\u{5B8}"] {
        let texts = cut_after(MAX_BLOCK_CHARS - 1, &format!("{letter}{}", "b".repeat(10)));
        assert_eq!(texts.len(), 2, "{letter:?}");
        assert!(
            texts.get(1).is_some_and(|t| t.starts_with(letter)),
            "{letter:?} was cut: {:?}",
            texts.get(1).map(|t| t.chars().take(4).collect::<String>())
        );
    }
    // Two flags side by side: the cut may fall between them, never inside one.
    let texts = cut_after(MAX_BLOCK_CHARS - 2, "🇬🇧🇫🇷bbbb");
    assert!(texts.get(1).is_some_and(|t| t.starts_with("🇫🇷")), "{:?}", texts.get(1));
}

#[test]
fn a_word_split_between_two_spans_is_kept_whole() {
    // 3,995 characters of words, then `Ga` and, in bold, `rden tools`: the room ends inside `Garden`.
    let mut bold = Span::plain("rden tools");
    bold.bold = true;
    let block = Block::new(BlockKind::Paragraph, vec![Span::plain(format!("{}Ga", "seed ".repeat(799))), bold]);
    let views = of(&[block]);
    assert_eq!(views.len(), 2);
    let second: String = views.get(1).map(|v| v.spans.iter().map(|s| s.text.as_str()).collect()).unwrap_or_default();
    assert_eq!(second, "Garden tools");
    assert!(views.get(1).and_then(|v| v.spans.last()).is_some_and(|s| s.bold && s.text == "rden tools"));
    assert!(views.iter().all(|v| chars(v) <= MAX_BLOCK_CHARS));
}

#[test]
fn a_line_break_left_over_makes_no_block_of_its_own() {
    let views = of(&[paragraph(&format!("{}\n", "x".repeat(MAX_BLOCK_CHARS)))]);
    assert_eq!(views.len(), 1, "{:?}", views.iter().map(chars).collect::<Vec<_>>());
}

/// A data table (one with lines, which the parser keeps for a cell alone) round `blocks` in its first cell.
fn table_of(blocks: Vec<Block>) -> Block {
    let mut parsed = mailcraft_html::parse("<table border=\"1\"><tr><td>x</td><td>y</td></tr></table>");
    let mut table = parsed.remove(0);
    if let BlockKind::Table(t) = &mut table.kind
        && let Some(cell) = t.rows.first_mut().and_then(|r| r.cells.first_mut())
    {
        cell.blocks = blocks;
    }
    table
}

#[test]
fn a_table_the_phone_draws_as_its_text_ends_the_line_before_it() {
    let mut after = paragraph(&"y".repeat(1_500));
    after.beside = true;
    // In the cell of the deepest table the phone draws: 3,000 characters, a table it flattens, and
    // text going on in a line after that table; the line before the table does not count.
    let mut blocks = vec![paragraph(&"z".repeat(3_000)), table_of(vec![paragraph("deep")]), after];
    for _ in 0..crate::blocks::PHONE_TABLE_DEPTH {
        blocks = vec![table_of(blocks)];
    }
    let mut cell = of(&blocks);
    for _ in 0..crate::blocks::PHONE_TABLE_DEPTH {
        cell = match cell.first().map(|v| &v.kind) {
            Some(BlockKindView::Table { table }) => table.rows.first().and_then(|r| r.cells.first()).map(|c| c.blocks.clone()).unwrap_or_default(),
            _ => Vec::new(),
        };
    }
    // The 3,000, the flattened table's two cells, and the text after it, still in its line.
    assert_eq!(cell.iter().map(|v| v.beside).collect::<Vec<_>>(), [false, false, false, true]);
}

#[test]
fn hostile_runs_of_marks_joiners_and_flags_are_sent_whole() {
    for text in [
        "\u{301}".repeat(10_000),
        "\u{200D}".repeat(9_001),
        "🇬".repeat(8_001),
        format!("a{}", "👨\u{200D}".repeat(5_000)),
        format!("{} {}", "x".repeat(3_000), "\u{FE0F}".repeat(6_000)),
    ] {
        let views = of(&[paragraph(&text)]);
        assert!(views.iter().all(|v| chars(v) <= MAX_BLOCK_CHARS && chars(v) > 0));
        assert_eq!(ink_of(&views), ink(&text));
    }
}

fn texts(views: &[BlockView]) -> Vec<String> {
    views.iter().map(|v| v.spans.iter().map(|s| s.text.as_str()).collect()).collect()
}

#[test]
fn blank_lines_the_sender_wrote_between_pieces_are_kept() {
    for kind in [BlockKind::Paragraph, BlockKind::Preformatted] {
        let text = format!("{}\n\n\n{}", "a".repeat(MAX_BLOCK_CHARS - 1), "word ".repeat(1_000));
        let joined = texts(&of(&[Block::new(kind.clone(), vec![Span::plain(&text)])])).join("\n");
        // A piece's own break stands for one line break; the other two are kept.
        assert!(joined.starts_with(&format!("{}\n\n\nword", "a".repeat(MAX_BLOCK_CHARS - 1))), "{kind:?}");
    }
}

#[test]
fn the_first_piece_keeps_the_blocks_room_above_and_its_marker() {
    // The block starts with a line break: the first piece's text is past it.
    let text = format!("\n{}", "x".repeat(4_500));
    let mut p = paragraph(&text);
    p.space = 1;
    let views = of(&[p]);
    assert_eq!(views.first().map(|v| v.space), Some(1));
    let item = Block::new(BlockKind::ListItem { marker: Marker::Bullet, depth: 0 }, vec![Span::plain(&text)]);
    let views = of(&[item]);
    assert!(matches!(views.first().map(|v| &v.kind), Some(BlockKindView::ListItem { marker: MarkerView::Bullet, .. })));
    assert_eq!(views.iter().filter(|v| matches!(v.kind, BlockKindView::ListItem { marker: MarkerView::Bullet, .. })).count(), 1);
}

#[test]
fn a_letter_split_between_two_spans_is_kept_whole() {
    let bold = |text: &str| {
        let mut s = Span::plain(text);
        s.bold = true;
        s
    };
    // The accent in the next span, with room for one more character, and with none.
    for (plain, styled) in
        [("a".repeat(MAX_BLOCK_CHARS - 1), "e\u{301}bbbbbbbbbb"), (format!("{}e", "a".repeat(MAX_BLOCK_CHARS - 1)), "\u{301}bbbbbbbbbb")]
    {
        let views = of(&[Block::new(BlockKind::Paragraph, vec![Span::plain(&plain), bold(styled)])]);
        let all = texts(&views);
        assert!(all.get(1).is_some_and(|t| t.starts_with("e\u{301}")), "{:?}", all.get(1).map(|t| t.chars().take(3).collect::<String>()));
        assert_eq!(ink_of(&views), format!("{plain}{styled}"));
    }
}

#[test]
fn a_split_block_takes_no_more_of_the_budget_than_its_pieces() {
    // A block of two spans cut in two takes 4 nodes (itself, its spans, its second piece); then as
    // many one-span paragraphs (2 each) as fill the rest exactly: the last keeps its text.
    let long = Block::new(BlockKind::Paragraph, vec![Span::plain("x".repeat(MAX_BLOCK_CHARS)), Span::plain("y")]);
    let rest = (crate::blocks::MAX_NODES - 4) / 2;
    let blocks: Vec<Block> = std::iter::once(long).chain((0..rest).map(|_| paragraph("z"))).collect();
    let views = of(&blocks);
    assert_eq!(views.len(), rest + 2);
    assert_eq!(views.last().map(chars), Some(1));
}

#[test]
fn a_flattened_tables_last_line_counts_in_the_run_after_it() {
    let mut after = paragraph(&"y".repeat(1_500));
    after.beside = true;
    // The table's last cell holds a block of the most characters; the text after it can't join it.
    let mut table = table_of(Vec::new());
    if let BlockKind::Table(t) = &mut table.kind
        && let Some(cell) = t.rows.first_mut().and_then(|r| r.cells.last_mut())
    {
        cell.blocks = vec![paragraph(&"d".repeat(MAX_BLOCK_CHARS))];
    }
    let mut blocks = vec![table, after];
    for _ in 0..crate::blocks::PHONE_TABLE_DEPTH {
        blocks = vec![table_of(blocks)];
    }
    let mut cell = of(&blocks);
    for _ in 0..crate::blocks::PHONE_TABLE_DEPTH {
        cell = match cell.first().map(|v| &v.kind) {
            Some(BlockKindView::Table { table }) => table.rows.first().and_then(|r| r.cells.first()).map(|c| c.blocks.clone()).unwrap_or_default(),
            _ => Vec::new(),
        };
    }
    assert_eq!(cell.last().map(chars), Some(1_500));
    assert_eq!(cell.last().map(|v| v.beside), Some(false));
}

#[test]
fn a_short_start_before_a_word_longer_than_a_block_is_not_a_block_alone() {
    let views = of(&[paragraph(&format!("c d e {}", "x".repeat(5_000)))]);
    assert_eq!(views.first().map(chars), Some(MAX_BLOCK_CHARS));
}

#[test]
fn a_space_at_the_seam_of_two_spans_is_where_a_piece_ends() {
    let mut bold = Span::plain("y".repeat(100));
    bold.bold = true;
    let views = of(&[Block::new(BlockKind::Paragraph, vec![Span::plain(format!("{} ", "w".repeat(3_990))), bold])]);
    assert_eq!(texts(&views).get(1).map(String::len), Some(100));
}
