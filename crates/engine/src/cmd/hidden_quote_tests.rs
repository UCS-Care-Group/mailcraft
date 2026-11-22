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
    let m = Message {
        account: s.mailbox.accounts[0].id,
        folder: s.mailbox.inbox().unwrap(),
        message_id: "minutes-1@example.org".into(),
        from: Address::new("Rui Sousa", "rui@example.org"),
        to: vec![Address::new("Avery Quinn", "avery@example.org")],
        subject: "Minutes".into(),
        date: 1_791_000_000,
        body: mailcraft_model::Body { text: String::new(), html: Some(html) },
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
        assert!(shown.contains("Hello Avery,") && !shown.contains("Approved"), "{shape}: {shown}");
        for command in ["mail.reply", "mail.forward"] {
            run(&mut s, command, json!({"id": id}));
            let q = compose(&s).quote.clone().unwrap();
            assert!(q.html.contains("Hello Avery,") && !q.html.contains("Approved"), "{shape} {command}: {}", q.html);
            assert!(q.text.contains("Hello Avery,") && !q.text.contains("Approved"), "{shape} {command}: {}", q.text);
            run(&mut s, "compose.set", json!({"to": "maya@example.org"}));
            let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
            let body = &s.mailbox.message(sent).unwrap().body;
            assert!(!body.text.contains("Approved") && !body.html.as_deref().unwrap_or("").contains("Approved"), "{shape} {command}");
        }
    }
}
