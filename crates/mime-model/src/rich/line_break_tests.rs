// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A manual line break stays inside its paragraph and goes out as a line break, never as a mark.

use mailcraft_model::rich::{LINE_BREAK, ListKind, Paragraph, RichText, Run};

use super::{from_html, from_html_with_pictures, to_html};

fn marked(html: &str) -> bool {
    html.chars().any(|c| matches!(c, '\u{00B6}' | '\u{00B7}' | '\u{2192}' | '\u{21B5}' | LINE_BREAK))
}

#[test]
fn a_line_break_is_a_break_inside_the_paragraph() {
    let text = format!("Hello  world\t{LINE_BREAK}next");
    let t = RichText::from_paragraphs(vec![Paragraph::new(vec![Run::plain(&text)], ListKind::None)]);
    assert_eq!(t.paragraphs().len(), 1);
    assert_eq!(t.text(), text);
    let html = to_html(&t);
    assert_eq!(html, "<div>Hello &nbsp;world&nbsp;&nbsp;&nbsp;&nbsp;<br>next</div>\r\n");
    assert!(!marked(&html), "{html}");
    assert_eq!(t.to_plain(), "Hello  world\t\nnext");
    assert!(!marked(&t.to_plain()));
    // A draft of ours keeps the break inside the one paragraph. The tab comes back as spaces.
    let draft = from_html_with_pictures(&html, &mut |_| None);
    assert_eq!(draft.paragraphs().len(), 1);
    assert_eq!(draft.text(), format!("Hello  world    {LINE_BREAK}next"));
    assert!(!marked(&draft.to_plain()));
    // HTML pasted from elsewhere, or a message, still starts a paragraph at the break.
    let pasted = from_html(&html);
    assert_eq!(pasted.paragraphs().len(), 2);
    assert_eq!(pasted.text(), "Hello  world    \nnext");
    assert!(!pasted.text().contains(LINE_BREAK));
}

#[test]
fn a_break_inside_a_list_item_stays_in_that_item() {
    let text = format!("Agenda{LINE_BREAK}see notes");
    let t = RichText::from_paragraphs(vec![Paragraph::new(vec![Run::plain(&text)], ListKind::Numbered)]);
    let html = to_html(&t);
    assert!(html.contains("<br>"), "{html}");
    assert_eq!(html.matches("<li>").count(), 1, "{html}");
    let back = from_html_with_pictures(&html, &mut |_| None);
    assert_eq!(back.paragraphs().len(), 1);
    assert_eq!(back.paragraphs()[0].list, ListKind::Numbered);
    assert_eq!(back.text(), text);
    assert_eq!(to_html(&back), html);
    // What is sent is still one numbered item. Reading the same HTML as pasted mail splits it.
    assert_eq!(back.to_plain(), "1. Agenda\nsee notes");
    let pasted = from_html(&html);
    assert_eq!(pasted.paragraphs().len(), 2);
    assert!(pasted.paragraphs().iter().all(|p| p.list == ListKind::Numbered));
}

#[test]
fn a_line_break_on_its_own_is_an_empty_line() {
    let t = RichText::from_plain(&LINE_BREAK.to_string());
    assert_eq!(to_html(&t), "<div><br></div>\r\n");
    assert!(!marked(&t.to_plain()));
}
