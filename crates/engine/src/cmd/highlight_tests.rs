// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Text Highlight Color, Format Painter and Insert › Symbol in the message being written.

use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use super::{SYMBOLS, symbol_ok};
use crate::{Compose, Session, sample};

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn compose(s: &Session) -> &Compose {
    s.compose.as_ref().unwrap()
}

fn mine(s: &Session) -> String {
    let text = compose(s).body.plain();
    text.split("\n\nAvery Quinn").next().unwrap_or("").to_string()
}

fn writing(text: &str) -> Session {
    let mut s = session();
    run(&mut s, "mail.new", json!({"body": text}));
    run(&mut s, "compose.select", json!({"from": 0}));
    s
}

#[test]
fn a_highlight_goes_behind_the_text_and_out_in_the_html() {
    let mut s = writing("mark this");
    let r = run(&mut s, "format.highlight", json!({"value": "#FF0", "from": 0, "to": 4}));
    assert_eq!(r["value"], "#ffff00");
    let text = compose(&s).body.text();
    assert!(text.all(0, 4, |st| st.highlight.as_deref() == Some("#ffff00")).unwrap());
    assert!(text.all(5, 9, |st| st.highlight.is_none()).unwrap());
    let html = mailcraft_mime::rich::to_html(text);
    assert!(html.contains("<span style=\"background-color:#ffff00\">mark</span>"), "{html}");
    // And it comes back from the HTML (a draft opened again, a paste).
    let back = mailcraft_mime::rich::from_html(&html);
    assert!(back.all(0, 4, |st| st.highlight.as_deref() == Some("#ffff00")).unwrap());
    // Off again; one step each.
    run(&mut s, "format.highlight", json!({"value": null, "from": 0, "to": 4}));
    assert!(compose(&s).body.text().all(0, 4, |st| st.is_plain()).unwrap());
    run(&mut s, "compose.undo", json!({}));
    assert!(compose(&s).body.text().all(0, 4, |st| st.highlight.is_some()).unwrap());
    for bad in [json!({}), json!({"value": "yellow"}), json!({"value": "#12345g"}), json!({"value": 5})] {
        assert!(s.run("format.highlight", &bad).is_err(), "{bad}");
    }
    // Not in plain text.
    run(&mut s, "compose.formatPlain", json!({}));
    assert!(!s.enabled("format.highlight"));
}

#[test]
fn format_painter_copies_a_look_and_puts_it_on_the_next_selection() {
    let mut s = writing("one two three");
    run(&mut s, "format.bold", json!({"from": 0, "to": 3}));
    run(&mut s, "format.highlight", json!({"value": "#c5e1a5", "from": 0, "to": 3}));
    run(&mut s, "compose.link", json!({"url": "https://example.org", "from": 0, "to": 3}));
    run(&mut s, "compose.select", json!({"from": 0, "to": 3}));
    let before = compose(&s).body.text().clone();
    let r = run(&mut s, "edit.formatPainter", json!({}));
    assert_eq!((r["copied"].as_bool(), r["look"]["bold"].as_bool()), (Some(true), Some(true)));
    // The link is where the text leads, not how it looks.
    assert!(r["look"].get("link").is_none());
    assert_eq!(compose(&s).body.text(), &before);
    run(&mut s, "edit.formatPainter", json!({"apply": true, "from": 8, "to": 13}));
    let text = compose(&s).body.text();
    assert!(text.all(8, 13, |st| st.bold && st.highlight.as_deref() == Some("#c5e1a5") && st.link.is_none()).unwrap());
    assert!(text.all(4, 7, |st| st.is_plain()).unwrap());
    // Put away once used.
    assert!(compose(&s).body.painter().is_none());
    assert!(s.run("edit.formatPainter", &json!({"apply": true})).is_err());
    // A second press puts it away unused.
    run(&mut s, "edit.formatPainter", json!({}));
    assert!(compose(&s).body.painter().is_some());
    assert_eq!(run(&mut s, "edit.formatPainter", json!({}))["copied"], false);
    assert!(compose(&s).body.painter().is_none());
    // One step back.
    run(&mut s, "compose.undo", json!({}));
    assert!(compose(&s).body.text().all(8, 13, |st| st.is_plain()).unwrap());
}

#[test]
fn a_symbol_goes_in_at_the_caret() {
    let mut s = writing("ab");
    run(&mut s, "compose.select", json!({"from": 1}));
    assert_eq!(run(&mut s, "insert.symbol", json!({"char": "€"}))["code"], "U+20AC");
    run(&mut s, "insert.symbol", json!({"code": "U+2192"}));
    run(&mut s, "insert.symbol", json!({"code": 169}));
    run(&mut s, "insert.symbol", json!({"code": "b0"}));
    assert_eq!(mine(&s), "a€→©°b");
    // Works in plain text too.
    run(&mut s, "compose.formatPlain", json!({}));
    run(&mut s, "insert.symbol", json!({"char": "±"}));
    assert_eq!(mine(&s), "a€→©°±b");
}

#[test]
fn what_isnt_a_symbol_is_refused() {
    let mut s = writing("ab");
    for bad in [
        json!({}),
        json!({"char": ""}),
        json!({"char": "ab"}),
        json!({"char": "\n"}),
        json!({"char": "\u{0}"}),
        json!({"char": "\u{202e}"}),
        json!({"char": "\u{200b}"}),
        json!({"char": "\u{fffc}"}),
        json!({"char": " "}),
        json!({"char": "\u{2028}"}),
        json!({"char": "\u{e000}"}),
        json!({"code": 0x110000}),
        json!({"code": 0xd800}),
        json!({"code": -1}),
        json!({"code": 1.5}),
        json!({"code": "U+FFFF"}),
        json!({"code": "U+FDD0"}),
        json!({"code": "U+1234567"}),
        json!({"code": "xyz"}),
        json!({"code": ""}),
        json!({"code": 18446744073709551615u64}),
        json!({"char": "€", "code": 65}),
    ] {
        assert!(s.run("insert.symbol", &bad).is_err(), "{bad}");
    }
    assert_eq!(mine(&s), "ab");
    assert!(SYMBOLS.iter().all(|c| symbol_ok(*c)));
}
