// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the reading pane doesn't show of a message is not in a reply's or a forward's quote of
//! it, in HTML or in plain text, nor in what is sent: however the elements that leave it out
//! nest. The people and addresses are invented.

use mailcraft_model::{Address, Message};
use serde_json::json;

use super::quote_tests::{compose, run, session};
use crate::Session;

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
/// its theme, with what the message's `@media` hid there left out; one read nowhere, as a wide
/// light pane reads it.
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
            s.read_at(id, env, false);
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
                    s.read_at(id, mailcraft_html::Env::default(), as_text);
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
