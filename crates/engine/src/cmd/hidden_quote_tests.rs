// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the reading pane doesn't show of a message is not in a reply's or a forward's quote of
//! it, in HTML or in plain text, nor in what is sent: however the elements that leave it out
//! nest. The people and addresses are invented.

use mailcraft_model::{Address, Message};
use serde_json::json;

use super::quote_tests::{compose, run, session};
use crate::{ReadIn, Session};

/// What each shape leaves out, in a message received in the Inbox.
const SHAPES: [&str; 8] = [
    "<svg><template></svg>",
    "<math><template></math>",
    "<noscript><template></noscript>",
    "<p hidden><template></p>",
    "<div style=\"display:none\"><template></div>",
    "<select><template></select>",
    "<button><template></button>",
    "<object><template></object>",
];

fn minutes(s: &mut Session, shape: &str) -> u64 {
    let html = format!("<html><body><p>Hello Avery,</p>{shape}<p>Approved: send the invoice to accounts@example.org</p></body></html>");
    message(s, &html, "")
}

fn message(s: &mut Session, html: &str, text: &str) -> u64 {
    let m = Message {
        account: s.mailbox.accounts[0].id,
        folder: s.mailbox.inbox().unwrap(),
        message_id: "minutes-1@example.org".into(),
        from: Address::new("Rui Sousa", "rui@example.org"),
        to: vec![Address::new("Avery Quinn", "avery@example.org")],
        subject: "Minutes".into(),
        date: 1_791_000_000,
        body: mailcraft_model::Body { text: text.into(), html: Some(html.into()) },
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

#[test]
fn what_the_pane_leaves_out_is_not_quoted_or_sent() {
    for shape in SHAPES {
        let mut s = session();
        let id = minutes(&mut s, shape);
        let shown = mailcraft_html::blocks_to_text(&mailcraft_html::parse(s.mailbox.message(id).unwrap().body.html.as_deref().unwrap()));
        assert!(shown.contains("Hello Avery,"), "{shape}: {shown}");
        // (A template inside SVG or MathML is none, and one inside a hidden block ends with it:
        // the pane shows what follows those; whatever it shows, the quote says, and no more.)
        let approved = shown.contains("Approved");
        assert_eq!(approved, matches!(&shape[..5], "<svg>" | "<math" | "<p hi" | "<div "), "{shape}: {shown}");
        for command in ["mail.reply", "mail.forward"] {
            run(&mut s, command, json!({"id": id}));
            let q = compose(&s).quote.clone().unwrap();
            assert!(q.html.contains("Hello Avery,") && q.html.contains("Approved") == approved, "{shape} {command}: {}", q.html);
            assert!(q.text.contains("Hello Avery,") && q.text.contains("Approved") == approved, "{shape} {command}: {}", q.text);
            run(&mut s, "compose.set", json!({"to": "maya@example.org"}));
            let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
            let body = &s.mailbox.message(sent).unwrap().body;
            assert_eq!(body.text.contains("Approved"), approved, "{shape} {command}");
            assert_eq!(body.html.as_deref().unwrap_or("").contains("Approved"), approved, "{shape} {command}");
        }
    }
}

/// A reply or forward quotes the message as the pane it was read in read it: at its width and in
/// its theme, with what the message's `@media` hid there left out.
#[test]
fn a_message_is_quoted_as_the_pane_it_was_read_in_read_it() {
    let html = "<style>@media (prefers-color-scheme:dark){.d{display:none}}@media (max-width:1279px){.n{display:none}}</style>\
                <p>Hello Avery,</p><p class=d>Light only</p><p class=n>Wide only</p>";
    for (env, kept, gone) in [
        (mailcraft_html::Env::for_pane(800.0, true), &[][..], &["Light only", "Wide only"][..]),
        (mailcraft_html::Env::for_pane(1600.0, true), &["Wide only"][..], &["Light only"][..]),
        (mailcraft_html::Env::for_pane(800.0, false), &["Light only"][..], &["Wide only"][..]),
        (mailcraft_html::Env::default(), &["Light only", "Wide only"][..], &[][..]),
    ] {
        for command in ["mail.reply", "mail.replyAll", "mail.forward"] {
            let mut s = session();
            let id = message(&mut s, html, "");
            s.read_at(id, ReadIn::Pane, env, false);
            run(&mut s, command, json!({"id": id}));
            let q = compose(&s).quote.clone().unwrap();
            for word in kept.iter().chain(["Hello Avery,"].iter()) {
                assert!(q.html.contains(word) && q.text.contains(word), "{env:?} {command} {word}: {}", q.html);
            }
            for word in gone {
                assert!(!q.html.contains(word) && !q.text.contains(word), "{env:?} {command} {word}: {}", q.html);
            }
        }
    }
}

/// HTML beside a text part that says something else: a reply or forward quotes the one the pane
/// showed, as it said; for a message no pane has shown, the text part only when the HTML holds no
/// text and no picture at all (a picture the pane might show is never traded for words it didn't).
#[test]
fn the_html_or_the_text_part_is_quoted_as_the_pane_showed_it() {
    let shapes = [
        ("<img src=\"cid:logo%40news.example\">", false),
        ("<img src=\"cid:<logo@news.example>\">", false),
        ("<img src=\"cid:missing@news.example\">", false),
        ("<img src=\"https://pics.example/a.png\" alt=\"ALTWORDS\">", false),
        ("<p>Shown words</p>", false),
        ("<script>x()</script>", true),
        ("<p hidden>Hidden words</p>", true),
    ];
    for (html, unread_as_text) in shapes {
        for said in [None, Some(false), Some(true)] {
            for command in ["mail.reply", "mail.forward"] {
                let mut s = session();
                let id = message(&mut s, html, "SENDER ALTERNATIVE");
                if let Some(as_text) = said {
                    s.read_at(id, ReadIn::Pane, mailcraft_html::Env::default(), as_text);
                }
                let as_text = said.unwrap_or(unread_as_text);
                run(&mut s, command, json!({"id": id}));
                let c = compose(&s).clone();
                assert_eq!(c.quote.is_none(), as_text, "{html} {said:?} {command}");
                assert_eq!(c.format == crate::BodyFormat::Plain, as_text, "{html} {said:?} {command}");
                assert_eq!(c.body.plain().contains("SENDER ALTERNATIVE"), as_text, "{html} {said:?} {command}");
                assert!(c.quote.is_none_or(|q| !q.text.contains("SENDER ALTERNATIVE") && !q.html.contains("Hidden")), "{html} {said:?} {command}");
            }
        }
    }
}

/// Light only below 1280 points wide and dark, Wide only at 1280 or more.
const LOOKS: &str = "<style>@media (prefers-color-scheme:dark){.d{display:none}}@media (max-width:1279px){.n{display:none}}</style>\
                     <p>Hello Avery,</p><p class=d>Light only</p><p class=n>Wide only</p>";

/// The words of [`LOOKS`] a reply quotes (in its HTML and its text alike), by `params`.
fn quoted_looks(s: &mut Session, command: &str, params: serde_json::Value) -> Vec<&'static str> {
    run(s, command, params);
    let q = compose(s).quote.clone().unwrap();
    let words = ["Hello Avery,", "Light only", "Wide only"];
    for word in words {
        assert_eq!(q.html.contains(word), q.text.contains(word), "{command} {word}: {}", q.html);
    }
    words.into_iter().filter(|w| q.html.contains(w)).collect()
}

/// A message read nowhere (or forgotten) is quoted with only what every pane shows of it: what
/// its `@media` hides in any width or theme is left out, and so is a text part when some pane
/// might show its HTML.
#[test]
fn a_message_read_nowhere_is_quoted_as_every_pane_shows_it() {
    for command in ["mail.reply", "mail.replyAll", "mail.forward"] {
        let mut s = session();
        let id = message(&mut s, LOOKS, "");
        assert_eq!(quoted_looks(&mut s, command, json!({"id": id})), ["Hello Avery,"], "{command}");
    }
    // Words only a narrow pane shows: HTML some pane shows something of is quoted as HTML.
    let mut s = session();
    let html = "<style>@media (min-width:600px){.n{display:none}}</style><p class=n>Narrow words</p>";
    let id = message(&mut s, html, "SENDER ALTERNATIVE");
    run(&mut s, "mail.reply", json!({"id": id}));
    let c = compose(&s).clone();
    assert!(c.quote.is_some() && !c.body.plain().contains("SENDER ALTERNATIVE"), "{:?}", c.body.plain());
}

/// What the pane and each window of its own read is kept apart: a reply asked for in one quotes
/// the message as that one read it, whichever drew it last; one asked for where it wasn't read
/// quotes what every pane shows.
#[test]
fn a_reply_quotes_the_message_as_the_place_it_was_asked_for_read_it() {
    let wide = mailcraft_html::Env::for_pane(1600.0, false);
    let narrow = mailcraft_html::Env::for_pane(800.0, false);
    for command in ["mail.reply", "mail.replyAll", "mail.forward", "mail.replyAndDelete"] {
        for pane_last in [false, true] {
            let mut s = session();
            let id = message(&mut s, LOOKS, "");
            if pane_last {
                s.read_at(id, ReadIn::Window(7), narrow, false);
                s.read_at(id, ReadIn::Pane, wide, false);
            } else {
                s.read_at(id, ReadIn::Pane, wide, false);
                s.read_at(id, ReadIn::Window(7), narrow, false);
            }
            let from_pane = quoted_looks(&mut s, command, json!({"id": id}));
            assert_eq!(from_pane, ["Hello Avery,", "Light only", "Wide only"], "{command} {pane_last}");
            let from_window = quoted_looks(&mut s, command, json!({"id": id, "window": 7}));
            assert_eq!(from_window, ["Hello Avery,", "Light only"], "{command} {pane_last}");
            let elsewhere = quoted_looks(&mut s, command, json!({"id": id, "window": 8}));
            assert_eq!(elsewhere, ["Hello Avery,"], "{command} {pane_last}");
        }
    }
    // A window that isn't a number is refused, not taken for the pane.
    let mut s = session();
    let id = message(&mut s, LOOKS, "");
    for command in ["mail.reply", "mail.forward", "mail.replyAndDelete"] {
        for window in [json!("7"), json!(-1), json!(null), json!(1.5)] {
            assert!(s.run(command, &json!({"id": id, "window": window})).is_err(), "{command} {window}");
        }
    }
}

/// What a pane said is kept for the 1024 places and messages said most recently: one said again
/// stays; one forgotten is quoted with what every pane shows, never as a wide light pane reads it.
#[test]
fn a_reply_after_many_messages_were_read_quotes_no_more_than_every_pane_shows() {
    let narrow_dark = mailcraft_html::Env::for_pane(800.0, true);
    let mut s = session();
    let kept = message(&mut s, LOOKS, "");
    let forgotten = message(&mut s, LOOKS, "");
    s.read_at(kept, ReadIn::Pane, mailcraft_html::Env::default(), false);
    s.read_at(forgotten, ReadIn::Pane, narrow_dark, false);
    for n in 0..2000u64 {
        s.read_at(1_000_000 + n, ReadIn::Window(n % 3), narrow_dark, false);
        if n % 500 == 0 {
            s.read_at(kept, ReadIn::Pane, mailcraft_html::Env::default(), false);
        }
    }
    assert!(s.read_as(forgotten, ReadIn::Pane).is_none());
    assert_eq!(s.read_as(kept, ReadIn::Pane), Some((mailcraft_html::Env::default(), false)));
    assert_eq!(quoted_looks(&mut s, "mail.reply", json!({"id": kept})), ["Hello Avery,", "Light only", "Wide only"]);
    assert_eq!(quoted_looks(&mut s, "mail.reply", json!({"id": forgotten})), ["Hello Avery,"]);
}
