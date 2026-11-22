// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A manual line break stays inside its paragraph and goes out as a line break, never as a mark.

use mailcraft_model::rich::{LINE_BREAK, ListKind, Paragraph, RichText, Run};

use super::{from_html, to_html};

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
    // Reading that HTML back splits on the break, as any other `<br>` does. The marks never return.
    let back = from_html(&html);
    assert_eq!(back.text(), "Hello  world    \nnext");
    assert!(!back.text().contains(LINE_BREAK));
}

#[test]
fn a_line_break_on_its_own_is_an_empty_line() {
    let t = RichText::from_plain(&LINE_BREAK.to_string());
    assert_eq!(to_html(&t), "<div><br></div>\r\n");
    assert!(!marked(&t.to_plain()));
}
