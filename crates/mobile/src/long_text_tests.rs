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
fn a_hostile_body_of_a_million_characters_is_sent_whole_and_quickly() {
    let text = "w".repeat(1_000_000);
    let start = std::time::Instant::now();
    let views = of(&[paragraph(&text)]);
    assert!(start.elapsed().as_secs() < 5);
    assert!(views.iter().all(|v| chars(v) <= MAX_BLOCK_CHARS));
    assert_eq!(ink_of(&views).len(), text.len());
}
