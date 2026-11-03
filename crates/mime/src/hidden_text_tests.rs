// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the sender hid in the HTML of a message (#599) is kept apart from its text.
use crate::parse;

fn html_message(html: &str) -> Vec<u8> {
    format!("From: a@example.org\r\nSubject: s\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}").into_bytes()
}

#[test]
fn an_html_message_keeps_what_it_hides_apart() {
    let m = parse(&html_message(
        "<style>.pre{display:none}</style><div class=pre>Preview: open day on the 14th &nbsp;&zwnj;&nbsp;</div><p>The cheeses are here.</p>",
    ));
    assert_eq!(m.body.text, "The cheeses are here.");
    assert_eq!(m.hidden_text, "Preview: open day on the 14th");
    assert!(m.body.html.is_some());
}

#[test]
fn a_plain_part_stays_the_text_and_the_html_still_gives_its_hidden_words() {
    let raw = "From: a@example.org\r\nSubject: s\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n\
        --b\r\nContent-Type: text/plain\r\n\r\nThe plain words.\r\n\
        --b\r\nContent-Type: text/html\r\n\r\n<p>The html words.</p><p style=\"display:none\">Only for programs.</p>\r\n--b--\r\n";
    let m = parse(raw.as_bytes());
    assert_eq!(m.body.text, "The plain words.");
    assert_eq!(m.hidden_text, "Only for programs.");
}

#[test]
fn plain_messages_and_html_that_hides_nothing_have_none() {
    assert_eq!(parse(b"Subject: s\r\n\r\nJust words.\r\n").hidden_text, "");
    assert_eq!(parse(&html_message("<p>Nothing hidden.</p>")).hidden_text, "");
}

#[test]
fn hidden_text_is_capped_and_a_hostile_message_reads() {
    let m = parse(&html_message(&format!("<div style='display:none'>{}</div><p>x</p>", "word ".repeat(200_000))));
    assert!(m.hidden_text.len() <= mailcraft_html::MAX_HIDDEN_TEXT);
    assert_eq!(m.body.text, "x");
}
