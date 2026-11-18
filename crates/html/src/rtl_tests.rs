// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #603: Hebrew and Arabic mail stands at the right in the reading pane. In HTML a paragraph reads
//! the way `dir` or `direction` says, as in a browser, and none says left to right; in plain text,
//! where there is no markup, its first strong letter says. One that reads from the right is
//! aligned right unless the message names a side. The text is kept as it was written. The
//! messages are made up.

use crate::nested_align_tests::aligned;
use crate::{Align, Block, parse, text_blocks};

fn align_of(html: &str, text: &str) -> Align {
    let blocks = parse(html);
    let all = aligned(&blocks);
    all.iter().find(|(t, _)| t == text).map(|(_, a)| *a).unwrap_or_else(|| panic!("no block {text:?} in {all:?}"))
}

#[test]
fn a_hebrew_or_arabic_paragraph_stands_at_the_right_where_the_message_says_rtl() {
    // (What Outlook and Word write.)
    let html = "<p dir=\"rtl\">שלום רב, הפגישה ביום שני ב-10:30.</p><p dir=\"rtl\">مرحبا، الاجتماع يوم 3 مايو.</p><p>Thanks, Dana</p>";
    assert_eq!(align_of(html, "שלום רב, הפגישה ביום שני ב-10:30."), Align::Right);
    assert_eq!(align_of(html, "مرحبا، الاجتماع يوم 3 مايو."), Align::Right);
    assert_eq!(align_of(html, "Thanks, Dana"), Align::Left);
}

#[test]
fn an_html_paragraph_with_no_direction_stays_at_the_left_whatever_its_letters_are() {
    let html = "<p>عمر Smith wrote the minutes.</p><p>שלום רב</p><pre>שלום
Hello again</pre><p><span dir=ltr>ACME</span> שלום</p>";
    assert_eq!(align_of(html, "عمر Smith wrote the minutes."), Align::Left);
    assert_eq!(align_of(html, "שלום רב"), Align::Left);
    assert_eq!(
        align_of(
            html,
            "שלום
Hello again"
        ),
        Align::Left
    );
    assert_eq!(align_of(html, "ACME שלום"), Align::Left);
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
fn dir_auto_takes_the_paragraphs_first_letter_where_the_message_asks_for_it() {
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
fn hebrew_in_a_table_cell_and_a_list_stands_at_the_right_where_the_message_says_rtl() {
    let html =
        "<table><tr><td dir=rtl>מוצר</td><td>Price</td></tr></table><ul dir=rtl><li>פריט ראשון</li></ul><ul><li>First item</li><li>שלום</li></ul>";
    assert_eq!(align_of(html, "מוצר"), Align::Right);
    assert_eq!(align_of(html, "Price"), Align::Left);
    assert_eq!(align_of(html, "פריט ראשון"), Align::Right);
    assert_eq!(align_of(html, "First item"), Align::Left);
    assert_eq!(align_of(html, "שלום"), Align::Left, "no direction, no side");
}

#[test]
fn text_beside_a_picture_stays_in_its_line_unless_the_message_gives_a_direction() {
    let blocks = parse("<p><img src='cid:logo' width=20 height=20> שלום</p>");
    let text = blocks.iter().find(|b| b.text() == "שלום");
    assert_eq!(text.map(|b| (b.beside, b.align)), Some((true, Align::Left)));
}

#[test]
fn a_picture_and_its_text_keep_to_one_side_where_the_message_says_it_reads_from_the_right() {
    let blocks = parse("<p dir=rtl><img src='cid:logo' width=20 height=20> שלום</p>");
    let got: Vec<(bool, Align)> = blocks.iter().map(|b| (b.beside, b.align)).collect();
    assert_eq!(got, [(false, Align::Right), (true, Align::Right)]);
}

#[test]
fn text_after_a_picture_keeps_the_side_of_its_paragraph_however_long_it_is() {
    let long = "שלום ".repeat(120);
    let html = format!("<p dir=rtl>שלום <img src='cid:logo' width=16 height=16> {long}</p>");
    let blocks = parse(&html);
    let sides: Vec<Align> = blocks.iter().filter(|b| matches!(b.kind, crate::BlockKind::Paragraph)).map(|b| b.align).collect();
    assert!(sides.len() >= 2, "text before and after the picture: {sides:?}");
    assert!(sides.iter().all(|a| *a == Align::Right), "{sides:?}");
    // With no direction the paragraph is left to right, whole.
    let blocks = parse(&format!("<p>שלום <img src='cid:logo' width=16 height=16> {long}</p>"));
    assert!(blocks.iter().all(|b| b.align == Align::Left));
}

#[test]
fn align_justify_as_an_attribute_names_no_side() {
    assert_eq!(align_of("<p dir=rtl align=justify>Hello</p>", "Hello"), Align::Right);
    assert_eq!(align_of("<div dir=rtl align=JUSTIFY><p>Hello</p></div>", "Hello"), Align::Right);
    assert_eq!(align_of("<table dir=rtl><tr><td align=justify>Hello</td></tr></table>", "Hello"), Align::Right);
    assert_eq!(align_of("<ul dir=rtl align=justify><li>Hello</li></ul>", "Hello"), Align::Right);
    assert_eq!(align_of("<p align=justify>Hello</p>", "Hello"), Align::Left);
    // A side named by the attribute still is one.
    assert_eq!(align_of("<p dir=rtl align=left>Hello</p>", "Hello"), Align::Left);
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
