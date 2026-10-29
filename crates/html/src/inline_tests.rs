// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #597: `display: inline-block` makes boxes that stand on a line; a picture stays in its line.

use crate::corpus::{STYLE_INLINE, hostile};
use crate::{Align, Block, BlockKind, Insets, Length, MAX_INLINE_BOXES, Marker, Rgb, Table, parse};

fn boxes(blocks: &[Block]) -> Vec<&Table> {
    blocks
        .iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Table(t) if t.inline => Some(t),
            _ => None,
        })
        .collect()
}

/// The boxes at any depth: in the cells of tables and in other boxes.
fn all_boxes(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|b| match &b.kind {
            BlockKind::Table(t) => usize::from(t.inline) + t.rows.iter().flat_map(|r| &r.cells).map(|c| all_boxes(&c.blocks)).sum::<usize>(),
            _ => 0,
        })
        .sum()
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
    // Standing alone, or with only other boxes beside it, it is still a box; one that is followed
    // by a line of its own in another block is too, and one with a line break and a line of text
    // after it in the same block is a run of the text.
    assert_eq!(boxes(&parse(&format!("<p>{a}A</a></p>"))).len(), 1);
    assert_eq!(boxes(&parse(&format!("<p>{a}A</a> {a}B</a></p>"))).len(), 2);
    assert_eq!(boxes(&parse(&format!("<p>{a}A</a><br>after</p>"))).len(), 0);
    assert_eq!(boxes(&parse(&format!("<div>{a}A</a></div><p>after</p>"))).len(), 1);
}

/// What the message was before boxes on a line: `display: inline-block` read as `inline`.
fn as_before(html: &str) -> Vec<Block> {
    parse(&html.replace("display:inline-block", "display:inline"))
}

const BADGE: &str = "<span style='display:inline-block;padding:0 4px;background:#ffeeaa'>NEW</span>";

#[test]
fn a_box_with_anything_else_in_its_block_is_drawn_in_the_line_as_it_was() {
    let b = BADGE;
    let a = "<a href='https://example.org/x' style='display:inline-block;padding:4px;background:#eee'>";
    let table = "<table><tr><td>x</td></tr></table>";
    let picture = "<img src='cid:a@example.org' width='16' height='16' alt='a'>";
    for html in [
        // Text with it: the badge is part of the sentence, the heading, the item or the pre.
        format!("<p>{b} Summer sale</p>"),
        format!("<p>Your order {b} has shipped.</p>"),
        format!("<p>{b}, and more</p>"),
        format!("<p>{b}'s new</p>"),
        format!("<p><a href='https://example.org/'>{b}</a>after link</p>"),
        format!("<p>{b}&nbsp;Summer sale</p>"),
        format!("<p>{b}&zwnj; offer</p>"),
        format!("<p>{b} {b} after</p>"),
        format!("<p>{a}A</a> | {a}B</a></p>"),
        format!("{b} after"),
        format!("<h2>{b} Summer sale</h2>"),
        format!("<ul><li>{b} Free shipping</li></ul>"),
        format!("<pre>{b} pre</pre>"),
        // A line break, a picture or a line of its own after it, in the same block.
        format!("<p>{b}<br>next</p>"),
        format!("<p>{a}A</a><br>after</p>"),
        format!("<div>{b}{picture}</div>"),
        format!("<div>{picture}{b}</div>"),
        // A `<p>` that a block ends, and the box after it: the box is in the line of the text after it.
        format!("<p>intro<div>x</div>{b} sale</p>"),
        format!("<p>intro{table}{b} sale</p>"),
        format!("<p>intro<h2>x</h2>{b} sale</p>"),
        format!("<p>intro<ul><li>x</li></ul>{b} sale</p>"),
        format!("<div><p>intro<div>x</div>{b} sale</p></div>"),
        format!("<table><tr><td><p>intro<div>x</div>{b} sale</p></td></tr></table>"),
        // An element that its end tag closes under another name, as the build does.
        "<div><strong style='display:inline-block;padding:0 4px;background:#ffeeaa'>NEW</b> sale</div>".to_string(),
        "<div><em style='display:inline-block;padding:0 4px;background:#ffeeaa'>NEW</i> sale</div>".to_string(),
        format!("<div><h2>x</h3>{b} sale</div>"),
        format!("<div><h2>x<h3>y</h3>{b} sale</div>"),
        format!("<div><ul><li>a</ol>{b} sale</div>"),
        // An `<h>` is no heading, and nothing ends at its end tag.
        format!("<div><h>x</h>{b} sale</div>"),
        format!("<p>a <h>mid</h> {b} b</p>"),
        format!("<div><pre>a</xmp>{b} sale</div>"),
        // A link that a link ends.
        format!("<div>{a}A<a href='https://example.org/y'>B</a></a></div>"),
        // Where a holder is more than blocks of text: list items, headings, preformatted text,
        // quotes. A box would cost them their marker, their size or their line.
        format!("<ul><li>{b}</li><li>second</li></ul>"),
        format!("<ol><li>{b}</li><li>second</li></ol>"),
        format!("<ol><li>{b}&nbsp;</li><li>second</li></ol>"),
        format!("<ol><li>{b}&zwnj;</li><li>second</li></ol>"),
        format!("<ul><li>{b} {b}</li></ul>"),
        format!("<ul><li>{b}{b}</li></ul>"),
        format!("<ul><li>{b}{table}</li></ul>"),
        format!("<ul><li>{b}{picture}</li></ul>"),
        format!("<ul><li>{b}<hr></li></ul>"),
        format!("<ul><li>{b}<br>next line</li></ul>"),
        format!("<ul><li>{b}<div>x</div></li></ul>"),
        format!("<ul><li>{b}<ul><li>nested</li></ul></li></ul>"),
        format!("<ul><li>{b}</li>text between</ul>"),
        format!("<ul><li>{b}</li></ul>after list"),
        format!("<ul><li><div>{b}</div></li></ul>"),
        format!("<h2>{b}</h2><h3>sub</h3>"),
        format!("<h2>{b}</h2>after heading"),
        format!("<pre>{b}</pre>after pre"),
        format!("<pre>{b}\nnext</pre>"),
        format!("<blockquote>{b}</blockquote>after quote"),
    ] {
        let found = parse(&html);
        assert_eq!(found, as_before(&html), "{html}");
        assert!(boxes(&found).is_empty(), "{html}: {found:?}");
    }
    // A tag that only looks like a heading's is an unknown one, whose text stays in the line.
    let unknown = parse("<p>a <h>mid</h> b</p>");
    assert_eq!(unknown.iter().map(Block::text).collect::<Vec<_>>(), ["a mid b"], "{unknown:?}");
    assert!(matches!(unknown[0].kind, BlockKind::Paragraph));
    let unknown = parse(&format!("<div><h>x</h>{b} sale</div>"));
    assert!(boxes(&unknown).is_empty() && unknown.iter().any(|b| b.text().contains("NEW sale")), "{unknown:?}");
    // The box before the block that ends the paragraph is a box of its own; the one after is in its line.
    let html = format!("<p>{a}A</a><div>x</div>{b} more</p>");
    let found = parse(&html);
    assert_eq!(boxes(&found).len(), 1, "{found:?}");
    assert!(found.iter().any(|b| b.text() == "NEW more"), "{found:?}");
    // The gaps and the marks of those lines are the ones the text has.
    let blocks = parse(&format!("<ol><li>{b}&nbsp;</li><li>second</li></ol>"));
    let marks: Vec<_> =
        blocks.iter().filter_map(|b| if let BlockKind::ListItem { marker, .. } = &b.kind { Some(marker.clone()) } else { None }).collect();
    assert_eq!(marks, [Marker::Number(1), Marker::Number(2)], "{blocks:?}");
    let blocks = parse(&format!("<p>{b} {b} after</p>"));
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0].text(), "NEW NEW after");
    let blocks = parse(&format!("<p>{b}<br>next</p>"));
    assert_eq!(
        blocks.iter().map(Block::text).collect::<Vec<_>>(),
        as_before(&format!("<p>{b}<br>next</p>")).iter().map(Block::text).collect::<Vec<_>>()
    );
    // Copying them gives the text, in a line.
    let sentence = format!("<p>{b} Your order has shipped.</p>");
    assert_eq!(crate::to_text(&sentence).trim(), "NEW Your order has shipped.");
    let quoted = crate::quotable(&sentence);
    assert!(quoted.contains("Your order has shipped.") && !quoted.contains("<table"), "{quoted}");
    // The blocks that were made are counted as before: as many as the message may have are all there.
    let small = format!("<p>{b} x</p>");
    let reps = crate::MAX_BLOCKS - 50;
    assert_eq!(parse(&small.repeat(reps)).len(), reps);
}

#[test]
fn a_holder_that_holds_boxes_and_nothing_else_makes_boxes() {
    let b = BADGE;
    let a = "<a href='https://example.org/x' style='display:inline-block;padding:4px;background:#eee'>";
    let count = |html: &str| all_boxes(&parse(html));
    // Alone, or with others, with the white space that hybrid emails and navigation bars have.
    assert_eq!(count(&format!("<div>{b}</div>")), 1);
    assert_eq!(count(&format!("<p>{b}&nbsp;</p>")), 1);
    assert_eq!(count(&format!("<p>{b}&zwnj;{b}</p>")), 2);
    assert_eq!(count(&format!("<div>\n  {b}\n  {b}\n</div>")), 2);
    assert_eq!(count(&format!("<div>{b}<!-- gap -->{b}</div>")), 2);
    // Something that is hidden is not in the line.
    assert_eq!(count(&format!("<div>{b}<span style='display:none'>hidden words</span></div>")), 1);
    assert_eq!(count(&format!("<div>{b}<img src='https://example.org/p.gif' style='display:none'></div>")), 1);
    assert_eq!(count(&format!("<div>{b}<p hidden>words</p></div>")), 1);
    // A navigation row.
    let nav = format!("<p>{a}Home</a> {a}Orders</a> {a}Help</a></p>");
    let blocks = parse(&nav);
    assert_eq!(boxes(&blocks).len(), 3, "{blocks:?}");
    assert_eq!(boxes(&blocks).iter().map(|t| text_of(t)).collect::<Vec<_>>(), ["Home", "Orders", "Help"]);
    assert_eq!(blocks.iter().map(|b| b.beside).collect::<Vec<_>>(), [false, true, true]);
    // Boxes that stand in one line come out in one line of text, as the words did.
    let text = crate::to_text(&nav);
    assert!(!text.trim().contains('\n'), "{text:?}");
    assert_eq!(text, crate::to_text(&nav.replace("display:inline-block", "display:inline")));
    // In a quote, and in a quote in a quote, the row is one line of the quote too.
    for quoted in [
        format!("<blockquote><div>{a}Home</a> {a}Orders</a></div></blockquote>"),
        format!("<blockquote><blockquote><p>{a}Home</a> {a}Orders</a></p></blockquote></blockquote>"),
    ] {
        let text = crate::to_text(&quoted);
        assert_eq!(text, crate::to_text(&quoted.replace("display:inline-block", "display:inline")), "{quoted}");
        assert_eq!(text.trim().lines().count(), 1, "{text:?}");
    }
    assert_eq!(boxes(&blocks).iter().map(|t| t.spaced).collect::<Vec<_>>(), [false, true, true]);
    // A row of buttons under a paragraph of its own, and with blocks after it, are boxes.
    assert_eq!(count(&format!("<div><p>Pick one</p>{a}Yes</a> {a}No</a><p>Thanks</p></div>")), 2);
    // The columns of a hybrid email, in the cell of a table.
    let d = "<div style='display:inline-block;width:300px;vertical-align:top;font-size:14px'>col</div>";
    assert_eq!(count(&format!("<table><tr><td style='font-size:0'>{d}{d}</td></tr></table>")), 2);
    // In a box, the box is part of what the holder of it holds, and is looked at as a holder.
    assert_eq!(count(&format!("<div><div style='display:inline-block;width:300px'>{b}{b}</div></div>")), 3);
    assert_eq!(count(&format!("<div><div style='display:inline-block;width:300px'>words {b}</div></div>")), 1);
    // The next block does not follow the box that ends the block before it.
    let beside = |html: &str| parse(html).iter().filter(|b| matches!(&b.kind, BlockKind::Table(t) if t.inline)).map(|b| b.beside).collect::<Vec<_>>();
    assert_eq!(beside(&format!("<div>{b}</div><div>{b}</div>")), [false, false]);
    assert_eq!(beside(&format!("<div>{b}</div>{b}")), [false, false]);
    assert_eq!(beside(&format!("<p>{b}</p><h3>sub</h3><p>{b}</p>")), [false, false]);
}

#[test]
fn a_look_ahead_reads_what_a_style_sheet_says_and_stops_at_its_bounds() {
    // The sheet's `display: inline-block` is read for the others in the holder too.
    let css = "<style>.col{display:inline-block;width:100px;background:#eef3fb}.gone{display:none}</style>";
    let blocks = parse(&format!("{css}<div><div class='col'>a</div><div class='col'>b</div><p class='gone'>words</p></div>"));
    assert_eq!(boxes(&blocks).len(), 2, "{blocks:?}");
    // A word in the holder, among the boxes of a sheet, keeps them in the line.
    let blocks = parse(&format!("{css}<div>words <span class='col'>a</span> <span class='col'>b</span></div>"));
    assert!(boxes(&blocks).is_empty(), "{blocks:?}");
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    // A holder too long to look through is drawn in the line, as before.
    let b = "<span style='display:inline-block;padding:2px'>b</span>";
    let long = format!("<div>{b}{}</div>", "<i></i>".repeat(10_000));
    assert!(boxes(&parse(&long)).is_empty());
    let fits = format!("<div>{b}{}</div>", "<i></i>".repeat(100));
    assert_eq!(boxes(&parse(&fits)).len(), 1);
    // Many holders, each with boxes and a second box in them, are looked at once each.
    let many =
        "<div><span style='display:inline-block;padding:2px'>b</span> <span style='display:inline-block;padding:2px'>c</span></div>".repeat(200);
    assert_eq!(boxes(&parse(&many)).len(), MAX_INLINE_BOXES);
    // A holder that is never closed is read to the end of the message, within the bounds.
    let open = format!("<div>{}", "<span style='display:inline-block;padding:2px'>b</span> ".repeat(3_000));
    assert!(boxes(&parse(&open)).len() <= MAX_INLINE_BOXES);
    // Nesting deeper than a look follows is drawn in the line.
    let deep = format!("<div>{b}{}x{}</div>", "<span>".repeat(100), "</span>".repeat(100));
    assert!(boxes(&parse(&deep)).is_empty());
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
    let blocks = parse(
        "<div><a href='https://example.org/x' style='display:inline-block;padding:4px'>Go<div style='background:#eeeeee'>inside</div></a></div><p>after</p>",
    );
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
    let blocks = parse(
        "<div><a href='https://example.org/x' style='display:inline-block;padding:4px'>Go<div style='background:#eeeeee'>inside</a></div><p>after</p>",
    );
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
    assert_eq!(beside(&format!("{b}<br>{b}")), []);
    assert_eq!(beside(&format!("{b}<p>text</p>{b}")), [false, false]);
}

#[test]
fn boxes_that_hold_paragraphs_are_paragraphs_in_the_text_and_one_line_boxes_are_a_line() {
    let col = |inner: &str| format!("<div style='display:inline-block;width:300px;font-size:14px'>{inner}</div>");
    let plain = |html: &str| html.replace("display:inline-block;width:300px", "width:300px");
    for (one, two) in [
        ("<p>Para one</p><p>Para two</p>", "<p>Para three</p><p>Para four</p>"),
        ("<h2>Head A</h2><p>x</p><ul><li>y</li></ul>", "<h2>Head B</h2><p>Para three</p>"),
        ("<blockquote>quoted</blockquote>", "<p>Para</p>"),
        ("first<br>second", "third<br>fourth"),
        ("<img src='cid:a@example.org' alt='pic'>words", "<table><tr><td>cell</td><td>cell two</td></tr></table>"),
        ("<p>Para one</p><p>Para two</p>", "one line"),
        ("one line", "<p>Para three</p><p>Para four</p>"),
    ] {
        let html = format!("<table><tr><td>{}{}</td></tr></table>", col(one), col(two));
        assert_eq!(crate::to_text(&html), crate::to_text(&plain(&html)), "{html}");
    }
    let html = format!("<div>{}{}</div>", col("<p>Para one</p><p>Para two</p>"), col("<p>Para three</p><p>Para four</p>"));
    assert_eq!(crate::to_text(&html).trim(), "Para one\n\nPara two\n\nPara three\n\nPara four");
}

#[test]
fn the_look_reads_the_sheet_for_the_same_pane_as_the_build() {
    use crate::media::Env;
    let html = "<style>@media (max-width:480px){.c{display:inline-block;padding:4px;background:#eeeeee}}</style>\
                <div><span class='c'>a</span> <span class='c'>b</span></div>";
    // In a narrow pane the rule applies to both: they are boxes, and the holder holds nothing else.
    let narrow = crate::parse_in(html, &Env::for_pane(400.0, false)).blocks;
    assert_eq!(boxes(&narrow).len(), 2, "{narrow:?}");
    // In a wide one it does not: they are words.
    let wide = crate::parse_in(html, &Env::for_pane(900.0, false)).blocks;
    assert!(boxes(&wide).is_empty(), "{wide:?}");
    assert_eq!(wide.iter().map(Block::text).collect::<Vec<_>>(), ["a b"]);
    // Something the sheet hides for the pane is not in the line.
    let hid = "<style>@media (max-width:480px){.gone{display:none}}</style><div><span style='display:inline-block;padding:4px'>a</span><span class='gone'>words</span></div>";
    assert_eq!(boxes(&crate::parse_in(hid, &Env::for_pane(400.0, false)).blocks).len(), 1);
    assert!(boxes(&crate::parse_in(hid, &Env::for_pane(900.0, false)).blocks).is_empty());
}

#[test]
fn a_box_a_dark_rule_gave_colours_to_is_a_shown_element_of_the_design() {
    use crate::media::Env;
    let css = "<style>.c{display:inline-block;padding:4px;background:#ffffff;color:#000000}\
               @media (prefers-color-scheme:dark){.c{background:#202020;color:#eeeeee}}</style>";
    let dark = Env::for_pane(900.0, true);
    for body in [
        // A box of the holder, a span, a div, a table; and the same where the look leaves it in the line.
        "<div><span class='c'>a</span></div>",
        "<div><div class='c'>a</div></div>",
        "<div>words <span class='c'>a</span></div>",
        "<div><table class='c'><tr><td>a</td></tr></table></div>",
    ] {
        let read = crate::parse_in(&format!("{css}{body}"), &dark);
        assert!(read.dark_design, "{body}: {:?}", read.blocks);
        assert!(!crate::parse_in(&format!("{css}{body}"), &Env::for_pane(900.0, false)).dark_design, "{body}");
    }
}

#[test]
fn what_a_sheet_said_of_a_start_tag_is_kept_for_the_looks_that_come_after() {
    use crate::look::{Holder, Look};
    use crate::media::Env;
    use crate::tokenizer::{Token, Tokenizer};
    let html = "<style>.c{display:inline-block;width:10px}</style><div><div class='c'>a</div><div class='c'>b</div></div>";
    let mut look = Look::new(html, &Env::default());
    let mut tokens = Tokenizer::new(html);
    let Some(Token::Open(mut first)) = tokens.next() else { panic!("a start tag") };
    look.apply(0, &mut first);
    assert_eq!(look.kept(), 0);
    let holder = Holder { id: tokens.pos(), at: (tokens.pos(), 1), name: "div" };
    assert!(look.only_boxes(&holder));
    assert_eq!(look.kept(), 2);
    // The build comes to those tags: what was said is still there, and not asked for again.
    let Some(Token::Open(mut second)) = tokens.next() else { panic!("a start tag") };
    look.apply(1, &mut second);
    assert!(second.sheet.as_ref().is_some_and(|a| a.css.inline_block));
    assert_eq!(look.kept(), 2);
}

#[test]
fn a_look_ahead_does_not_spend_the_sheets_steps_twice() {
    // Many rules every element has to be tried against, two columns of many paragraphs that the
    // look reads before the build does, and a copy for phones that a rule hides after them.
    // (Twelve rules is about as many as the sheet's steps allow when each element is asked about
    // once; asked about twice, the rule that hides the copy for phones is not reached.)
    let rules: String = (0..12).map(|n| format!("[data-k{n}]{{color:#111111}}")).collect();
    let column = format!("<div class='b'>{}</div>", "<p>words</p>".repeat(400));
    let html = format!(
        "<style>{rules}.b{{display:inline-block;width:280px}}.mob{{display:none}}</style><div>{column}{column}</div><div class='mob'>MOBILE COPY</div>"
    );
    let blocks = parse(&html);
    let all: String = blocks.iter().map(|b| format!("{b:?}")).collect();
    assert!(!all.contains("MOBILE COPY"), "the hidden copy shows");
    assert!(all.contains("words"));
}
