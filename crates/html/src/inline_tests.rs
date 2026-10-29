// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #597: `display: inline-block` makes boxes that stand on a line; a picture stays in its line.

use crate::corpus::{STYLE_INLINE, hostile};
use crate::{Align, Block, BlockKind, Insets, Length, MAX_INLINE_BOXES, Rgb, Table, parse};

fn boxes(blocks: &[Block]) -> Vec<&Table> {
    blocks
        .iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Table(t) if t.inline => Some(t),
            _ => None,
        })
        .collect()
}

fn text_of(t: &Table) -> String {
    t.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.blocks).map(Block::text).collect::<Vec<_>>().join(" ")
}

#[test]
fn links_drawn_as_buttons_are_boxes_that_keep_their_own_look() {
    let blocks = parse(
        "<p>Pick one</p><a href='https://example.org/a' style='display:inline-block;padding:10px 22px;background:#2f5d50;color:#fff;border-radius:4px'>Yes</a> \
         <a href='https://example.org/b' style='display:inline-block;padding:8px;border:2px solid #2f5d50'>No</a>",
    );
    let found = boxes(&blocks);
    assert_eq!(found.len(), 2, "{blocks:?}");
    let (yes, no) = (found[0], found[1]);
    assert_eq!(text_of(yes), "Yes");
    assert_eq!(yes.rows[0].cells[0].background, Some(Rgb(0x2f, 0x5d, 0x50)));
    assert_eq!(yes.rows[0].cells[0].padding, Some(Insets { top: 10, right: 22, bottom: 10, left: 22 }));
    assert_eq!(yes.radius, 4);
    assert!(no.outline && no.lines[0].width == 2, "{no:?}");
    // The words are still the link.
    let cell = &yes.rows[0].cells[0];
    assert_eq!(cell.blocks[0].spans[0].link.as_deref(), Some("https://example.org/a"));
    // The box has no width of its own: it is as wide as what is in it.
    assert_eq!(yes.width, None);
}

#[test]
fn columns_keep_their_widths() {
    let blocks = parse(
        "<div style='text-align:center'><div style='display:inline-block;width:260px;padding:8px;background:#eef3fb'>Left</div>\
         <div style='display:inline-block;width:50%;max-width:300px;vertical-align:top'>Right</div></div>",
    );
    let found = boxes(&blocks);
    assert_eq!(found.len(), 2, "{blocks:?}");
    assert_eq!(found[0].width, Some(Length::Px(260)));
    // A percentage under a `max-width` in pixels stays the percentage.
    assert_eq!(found[1].width, Some(Length::Percent(50)));
    // Where the line is put is what the text-align of the holder says.
    let aligns: Vec<Align> = blocks.iter().filter(|b| matches!(&b.kind, BlockKind::Table(t) if t.inline)).map(|b| b.align).collect();
    assert_eq!(aligns, [Align::Center, Align::Center]);
}

#[test]
fn only_inline_block_makes_a_box_and_only_one_with_something_to_show() {
    let boxed = |html: &str| !boxes(&parse(html)).is_empty();
    assert!(boxed("<span style='display:inline-block;padding:4px'>x</span>"));
    assert!(boxed("<span style='display:INLINE-BLOCK;background:#ff0000'>x</span>"));
    assert!(boxed("<div style='display:inline-block;width:200px'>x</div>"));
    // Nothing of its own to lay out: it stays in the text.
    assert!(!boxed("<p>a <span style='display:inline-block'>x</span> b</p>"));
    // Not exactly that value.
    assert!(!boxed("<span style='display:inline;padding:4px'>x</span>"));
    assert!(!boxed("<span style='display:inline-flex;padding:4px'>x</span>"));
    assert!(!boxed("<span style='display:inline-table;padding:4px'>x</span>"));
    // The last word on it is the one that counts.
    assert!(!boxed("<div style='display:inline-block;display:block;padding:4px'>x</div>"));
    // Hidden stays hidden: the last word counts, as for anything hidden.
    assert!(!boxed("<span style='display:inline-block;display:none;padding:4px'>x</span>"));
    assert!(boxed("<span style='display:none;display:inline-block;padding:4px'>x</span>"));
    // A style sheet's rule makes a box as a style does.
    assert!(boxed("<style>span{display:inline-block;padding:4px}</style><span>x</span>"));
}

#[test]
fn a_class_rule_in_a_style_sheet_makes_columns_that_stand_side_by_side() {
    let blocks = parse(
        "<style>.col{display:inline-block;width:300px;vertical-align:top;background:#eef3fb}</style>\
         <div><div class='col'>Left</div><div class='col'>Right</div></div>",
    );
    let found = boxes(&blocks);
    assert_eq!(found.len(), 2, "{blocks:?}");
    assert_eq!(found[0].width, Some(Length::Px(300)));
    assert_eq!(text_of(found[1]), "Right");
    assert_eq!(blocks.iter().filter(|b| b.beside).count(), 1, "{blocks:?}");
    // An inline style still wins over the rule.
    let blocks = parse("<style>.col{display:inline-block;width:300px}</style><div class='col' style='display:block'>x</div>");
    assert!(boxes(&blocks).is_empty(), "{blocks:?}");
}

#[test]
fn text_in_a_span_without_a_box_does_not_leave_its_line() {
    let blocks = parse("<p>one <span style='display:inline-block'>two</span> three</p>");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text(), "one two three");
}

#[test]
fn a_table_can_be_a_box_too() {
    let blocks = parse(
        "<table role='presentation' style='display:inline-block;width:200px'><tr><td>A</td></tr></table>\
         <table role='presentation' style='display:inline-block;width:200px'><tr><td>B</td></tr></table>",
    );
    assert_eq!(boxes(&blocks).len(), 2, "{blocks:?}");
}

#[test]
fn boxes_are_capped_and_nesting_is_bounded() {
    let many = "<span style='display:inline-block;padding:2px'>b</span>".repeat(MAX_INLINE_BOXES * 3);
    let blocks = parse(&many);
    assert_eq!(boxes(&blocks).len(), MAX_INLINE_BOXES);
    // What is past the cap is still read: as text in a line.
    assert!(blocks.iter().map(Block::text).collect::<String>().len() >= MAX_INLINE_BOXES);
    let deep = hostile().into_iter().find(|(n, _)| *n == "deep inline boxes").map(|(_, h)| h).unwrap_or_default();
    let blocks = parse(&deep);
    assert!(blocks.iter().any(|b| matches!(b.kind, BlockKind::Table(_))));
}

#[test]
fn the_open_day_notice_has_its_columns_and_buttons_and_a_picture_in_its_line() {
    let blocks = parse(STYLE_INLINE);
    // The picture and the text after it are one line (in the card that holds the page).
    fn first_line(blocks: &[Block]) -> Option<&[Block]> {
        if let Some(at) = blocks.iter().position(|b| matches!(b.kind, BlockKind::Image { .. })) {
            return blocks.get(at..at + 2);
        }
        blocks.iter().find_map(|b| match &b.kind {
            BlockKind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).find_map(|c| first_line(&c.blocks)),
            _ => None,
        })
    }
    let line = first_line(&blocks).expect("the mark and its text");
    assert!(line[1].beside && line[1].text().starts_with("The garden opens"), "{blocks:?}");
    // The card around the page holds the columns and the buttons: five boxes.
    fn all(blocks: &[Block], out: &mut Vec<String>) {
        for b in blocks {
            if let BlockKind::Table(t) = &b.kind {
                if t.inline {
                    out.push(text_of(t));
                }
                for c in t.rows.iter().flat_map(|r| &r.cells) {
                    all(&c.blocks, out);
                }
            }
        }
    }
    let mut found = Vec::new();
    all(&blocks, &mut found);
    assert_eq!(found.len(), 5, "{found:?}");
    assert!(found[0].starts_with("Morning") && found[1].starts_with("Afternoon"), "{found:?}");
    assert_eq!(&found[2..], ["Register", "Map", "Share a lift"]);
}

#[test]
fn a_picture_in_a_line_of_text_is_beside_what_is_around_it() {
    let blocks = parse("<p>Hello <img src='cid:a@example.org' width='16' height='16' alt='a'> and goodbye</p>");
    assert_eq!(blocks.len(), 3, "{blocks:?}");
    assert!(!blocks[0].beside && blocks[1].beside && blocks[2].beside);
}

#[test]
fn a_badge_in_a_sentence_stays_in_the_sentence() {
    let html = "<p>Your order <span style='display:inline-block;padding:0 4px;background:#ffeeaa'>#1234</span> has shipped.</p>";
    let blocks = parse(html);
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0].text(), "Your order #1234 has shipped.");
    // The highlight is kept, as it was.
    assert!(blocks[0].spans.iter().any(|s| s.text.contains("#1234") && s.background.is_some()), "{blocks:?}");
    assert_eq!(crate::to_text(html).trim(), "Your order #1234 has shipped.");
    let quoted = crate::quotable(html);
    assert!(quoted.contains("Your order") && quoted.contains("#1234") && quoted.contains("has shipped."), "{quoted}");
    assert!(!quoted.contains("Your order\n") && !quoted.contains("<table"), "{quoted}");
    // A link drawn with no padding is no box either, in a sentence or on its own.
    let link = "<p>See <a href='https://example.org/x' style='display:inline-block;padding:0'>the page</a> now.</p>";
    let blocks = parse(link);
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0].text(), "See the page now.");
    assert!(boxes(&parse("<a href='https://example.org/x' style='display:inline-block;padding:0 0 0 0'>x</a>")).is_empty());
    // Standing alone, with something to show, it is a box.
    assert_eq!(boxes(&parse("<div><span style='display:inline-block;padding:0 4px;background:#ffeeaa'>#1234</span></div>")).len(), 1);
}

#[test]
fn a_box_that_opens_a_line_of_text_stays_in_the_line() {
    let badge = "<p><span style='display:inline-block;padding:0 4px;background:#ffeeaa'>NEW</span> Your order has shipped.</p>";
    let blocks = parse(badge);
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0].text(), "NEW Your order has shipped.");
    assert!(blocks[0].spans.iter().any(|s| s.text.contains("NEW") && s.background.is_some()), "{blocks:?}");
    assert_eq!(crate::to_text(badge).trim(), "NEW Your order has shipped.");
    let quoted = crate::quotable(badge);
    let between = quoted.split("NEW").nth(1).and_then(|r| r.split("Your order").next()).unwrap_or("<p>");
    assert!(quoted.contains("Your order has shipped.") && !quoted.contains("<table"), "{quoted}");
    assert!(!between.contains("<p") && !between.contains("</p") && !between.contains("<br") && !between.contains("<div"), "{quoted}");
    // A row of links with bars between them.
    let a = "<a href='https://example.org/x' style='display:inline-block;padding:4px;background:#eee'>";
    let nav = format!("<p>{a}A</a> | {a}B</a></p>");
    let blocks = parse(&nav);
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0].text(), "A | B");
    assert_eq!(crate::to_text(&nav).trim(), "A <https://example.org/x> | B <https://example.org/x>");
    let quoted = crate::quotable(&nav);
    let words = quoted.split("</a>").nth(1).unwrap_or("<p>");
    assert!(words.trim_start().starts_with('|') && !words.contains("<p") && !words.contains("<div"), "{quoted}");
    // The words keep their links.
    assert_eq!(blocks[0].spans.iter().filter(|s| s.link.is_some()).count(), 2, "{blocks:?}");
    // Standing alone, or with only other boxes beside it, it is still a box; so is one that is
    // followed by a line of its own.
    assert_eq!(boxes(&parse(&format!("<p>{a}A</a></p>"))).len(), 1);
    assert_eq!(boxes(&parse(&format!("<p>{a}A</a> {a}B</a></p>"))).len(), 2);
    assert_eq!(boxes(&parse(&format!("<p>{a}A</a><br>after</p>"))).len(), 1);
    assert_eq!(boxes(&parse(&format!("<div>{a}A</a></div><p>after</p>"))).len(), 1);
}

#[test]
fn hidden_boxes_do_not_count_and_a_no_break_space_between_boxes_is_a_gap() {
    let hidden = "<table style='display:inline-block;visibility:hidden;width:100px'><tr><td>x</td></tr></table>".repeat(MAX_INLINE_BOXES * 2);
    let blocks = parse(&format!("{hidden}<div style='display:inline-block;width:100px'>real</div>"));
    assert_eq!(boxes(&blocks).len(), 1, "{}", blocks.len());
    let b = "<div style='display:inline-block;width:300px'>x</div>";
    let spaced: Vec<bool> = boxes(&parse(&format!("{b}&nbsp;{b}"))).iter().map(|t| t.spaced).collect();
    assert_eq!(spaced, [false, true]);
    // And the no-break space is not a paragraph of its own.
    assert_eq!(parse(&format!("{b}&nbsp;{b}")).len(), 2);
}

#[test]
fn white_space_between_boxes_is_what_makes_a_gap_and_a_size_of_nothing_has_none() {
    let b = "<div style='display:inline-block;width:300px;font-size:14px'>x</div>";
    let spaced = |html: &str| boxes(&parse(html)).iter().map(|t| t.spaced).collect::<Vec<_>>();
    assert_eq!(spaced(&format!("{b}{b}")), [false, false]);
    assert_eq!(spaced(&format!("{b}\n  {b}")), [false, true]);
    // Written at a size of nothing, as hybrid emails do, the white space is not there.
    assert_eq!(spaced(&format!("<div style='font-size:0'>{b}\n  {b}</div>")), [false, false]);
    assert_eq!(spaced(&format!("<table><tr><td style='font-size:0'>{b}\n{b}</td></tr></table>")), [false, false]);
}

#[test]
fn the_margins_of_a_box_are_its_own_and_not_the_rooms_round_the_line() {
    let blocks = parse("<div style='display:inline-block;width:300px;margin:0 10px'>x</div>");
    let found = boxes(&blocks);
    assert_eq!(found[0].box_margin, [10, 10]);
    assert_eq!(blocks[0].inset, [0, 0]);
}

#[test]
fn a_box_closed_from_the_middle_closes_its_own_frame() {
    // The link ends while a block inside it is open: what is inside is closed first.
    let blocks =
        parse("<a href='https://example.org/x' style='display:inline-block;padding:4px'>Go<div style='background:#eeeeee'>inside</div></a> after");
    let text: Vec<String> = blocks
        .iter()
        .map(|b| match &b.kind {
            BlockKind::Table(t) => format!("[{t:?}]"),
            _ => b.text(),
        })
        .collect();
    assert_eq!(text.last().map(String::as_str), Some("after"), "{text:?}");
    assert!(text.iter().any(|t| t.contains("Go") && t.contains("inside")), "{text:?}");
    // And with the end tag before the inner one is closed.
    let blocks =
        parse("<a href='https://example.org/x' style='display:inline-block;padding:4px'>Go<div style='background:#eeeeee'>inside</a> after</div>");
    let all: String = blocks.iter().map(|b| format!("{b:?}")).collect();
    assert!(all.contains("inside") && all.contains("after") && all.contains("Go"), "{all}");
    // "after" is not inside the box; "inside" is.
    let tables: Vec<&Table> = boxes(&blocks);
    assert_eq!(tables.len(), 1, "{blocks:?}");
    assert!(format!("{:?}", tables[0]).contains("inside") && !format!("{:?}", tables[0]).contains("after"));
}

#[test]
fn only_boxes_that_were_made_count_against_the_cap() {
    // Where the stack of open elements is full a box cannot be opened: it is not counted.
    let full = "<i>".repeat(200);
    let refused = "<div><span style='display:inline-block;padding:2px'>b</span></div>".repeat(MAX_INLINE_BOXES * 2);
    let after = "<div><span style='display:inline-block;padding:2px'>made</span>";
    let blocks = parse(&format!("{full}{refused}{}{after}", "</i>".repeat(200)));
    assert_eq!(boxes(&blocks).len(), 1, "{blocks:?}");
}

#[test]
fn boxes_share_a_line_only_when_nothing_ends_it_between_them() {
    let beside = |html: &str| parse(html).iter().filter(|b| matches!(&b.kind, BlockKind::Table(t) if t.inline)).map(|b| b.beside).collect::<Vec<_>>();
    let b = "<span style='display:inline-block;padding:4px'>x</span>";
    assert_eq!(beside(&format!("{b} {b}{b}")), [false, true, true]);
    assert_eq!(beside(&format!("<div>{b}{b}</div><div>{b}{b}</div>")), [false, true, false, true]);
    let d = "<div style='display:inline-block;width:100px'>x</div>";
    assert_eq!(beside(&format!("<div>{d}\n{d}</div>")), [false, true]);
    assert_eq!(beside(&format!("{b}<br>{b}")), [false, false]);
    assert_eq!(beside(&format!("{b}<p>text</p>{b}")), [false, false]);
}
