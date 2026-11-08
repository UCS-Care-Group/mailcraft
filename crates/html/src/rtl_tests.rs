// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #603: Hebrew and Arabic mail stands at the right in the reading pane. A paragraph reads the
//! way `dir`, `direction` or its first strong letter says, and one that reads from the right is
//! aligned right unless the message names a side. The letters keep the order they were written
//! in. The messages are made up.

use crate::{Align, Block, BlockKind, parse, text_blocks};

/// Every block that holds text, with its alignment, tables opened.
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

fn align_of(html: &str, text: &str) -> Align {
    let blocks = parse(html);
    let all = aligned(&blocks);
    all.iter().find(|(t, _)| t == text).map(|(_, a)| *a).unwrap_or_else(|| panic!("no block {text:?} in {all:?}"))
}

#[test]
fn a_hebrew_or_arabic_paragraph_stands_at_the_right_without_being_told() {
    let html = "<p>שלום רב, הפגישה ביום שני ב-10:30.</p><p>مرحبا، الاجتماع يوم 3 مايو.</p><p>Thanks, Dana</p>";
    assert_eq!(align_of(html, "שלום רב, הפגישה ביום שני ב-10:30."), Align::Right);
    assert_eq!(align_of(html, "مرحبا، الاجتماع يوم 3 مايو."), Align::Right);
    assert_eq!(align_of(html, "Thanks, Dana"), Align::Left);
}

#[test]
fn a_paragraph_that_starts_with_a_number_or_a_latin_word_goes_by_its_first_letter() {
    let html = "<p>2024 דוח שנתי</p><p>Invoice INV-42 עבור מרץ</p><p>«ACME» ١٢٣ قسيمة</p>";
    assert_eq!(align_of(html, "2024 דוח שנתי"), Align::Right, "digits are weak");
    assert_eq!(align_of(html, "Invoice INV-42 עבור מרץ"), Align::Left, "a Latin word first");
    assert_eq!(align_of(html, "«ACME» ١٢٣ قسيمة"), Align::Left);
}

#[test]
fn the_text_keeps_the_order_it_was_written_in() {
    let blocks = parse("<div dir=rtl>שלום 123 world</div>");
    assert_eq!(blocks.first().map(Block::text).as_deref(), Some("שלום 123 world"));
}

#[test]
fn dir_rtl_puts_even_latin_text_at_the_right_and_dir_ltr_keeps_hebrew_at_the_left() {
    assert_eq!(align_of("<div dir=rtl><p>Hello</p></div>", "Hello"), Align::Right);
    assert_eq!(align_of("<div dir=\"RTL\">Hello</div>", "Hello"), Align::Right, "Outlook writes it in capitals");
    assert_eq!(align_of("<p dir=ltr>שלום</p>", "שלום"), Align::Left);
    assert_eq!(align_of("<div dir=rtl><p dir=ltr>Hello</p></div>", "Hello"), Align::Left, "the nearer one wins");
    assert_eq!(align_of("<body dir=rtl><p>Hello</p></body>", "Hello"), Align::Right);
    assert_eq!(align_of("<html dir=rtl><body><p>Hello</p></body></html>", "Hello"), Align::Right);
}

#[test]
fn dir_auto_takes_the_paragraphs_first_letter() {
    let html = "<div dir=rtl><div dir=auto>Hello עולם</div><div dir=auto>עולם Hello</div></div>";
    assert_eq!(align_of(html, "Hello עולם"), Align::Left);
    assert_eq!(align_of(html, "עולם Hello"), Align::Right);
}

#[test]
fn css_direction_from_a_style_or_a_style_sheet() {
    assert_eq!(align_of("<p style='direction:rtl;unicode-bidi:embed'>Hello</p>", "Hello"), Align::Right);
    assert_eq!(align_of("<p style='direction:ltr'>שלום</p>", "שלום"), Align::Left);
    assert_eq!(align_of("<style>.he { direction: rtl }</style><p class=he>Hello</p>", "Hello"), Align::Right);
    // A style beats the attribute, as a browser's own style for `dir` loses to the message's.
    assert_eq!(align_of("<p dir=rtl style='direction:ltr'>Hello</p>", "Hello"), Align::Left);
}

#[test]
fn start_end_and_justify_follow_the_direction_and_sides_do_not() {
    assert_eq!(align_of("<p dir=rtl style='text-align:start'>Hello</p>", "Hello"), Align::Right);
    assert_eq!(align_of("<p dir=rtl style='text-align:end'>Hello</p>", "Hello"), Align::Left);
    assert_eq!(align_of("<p dir=rtl style='text-align:justify'>Hello</p>", "Hello"), Align::Right);
    assert_eq!(align_of("<p style='text-align:end'>Hello</p>", "Hello"), Align::Right);
    assert_eq!(align_of("<p style='text-align:left'>שלום</p>", "שלום"), Align::Left);
    assert_eq!(align_of("<p dir=rtl style='text-align:center'>שלום</p>", "שלום"), Align::Center);
    assert_eq!(align_of("<p align=left>שלום</p>", "שלום"), Align::Left);
    assert_eq!(align_of("<div style='text-align:end'><p dir=rtl>Hello</p></div>", "Hello"), Align::Left, "inherited end");
}

#[test]
fn a_direction_on_a_span_does_not_turn_its_paragraph_round() {
    assert_eq!(align_of("<p>Hello <span dir=rtl>שלום</span></p>", "Hello שלום"), Align::Left);
}

#[test]
fn hebrew_in_a_table_cell_and_a_list_stands_at_the_right() {
    let html = "<table><tr><td>מוצר</td><td>Price</td></tr></table><ul><li>פריט ראשון</li><li>First item</li></ul>";
    assert_eq!(align_of(html, "מוצר"), Align::Right);
    assert_eq!(align_of(html, "Price"), Align::Left);
    assert_eq!(align_of(html, "פריט ראשון"), Align::Right);
    assert_eq!(align_of(html, "First item"), Align::Left);
}

#[test]
fn text_beside_a_picture_stays_in_its_line_unless_the_message_gives_a_direction() {
    let blocks = parse("<p><img src='cid:logo' width=20 height=20> שלום</p>");
    let text = blocks.iter().find(|b| b.text() == "שלום");
    assert_eq!(text.map(|b| (b.beside, b.align)), Some((true, Align::Left)));
}

#[test]
fn plain_text_mail_stands_at_the_right_paragraph_by_paragraph() {
    let blocks = text_blocks("שלום דנה,\nהמסמך מצורף.\n\n2024 סיכום\n\nThanks,\nDana\n\n> مرحبا\n");
    let got: Vec<(String, Align)> = blocks.iter().map(|b| (b.text(), b.align)).collect();
    assert_eq!(
        got,
        [
            ("שלום דנה,\nהמסמך מצורף.".to_string(), Align::Right),
            ("2024 סיכום".to_string(), Align::Right),
            ("Thanks,\nDana".to_string(), Align::Left),
            ("مرحبا".to_string(), Align::Right),
        ]
    );
}
