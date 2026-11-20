// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A signature's style sheet goes with the message scoped to the signature: an element of the same
//! class outside it, in the original a reply quotes, keeps its own look, in the saved draft and in
//! the message sent, before and after it is written as MIME and read back, while the signature's
//! words get the rule's colour.
//!
//! The people, the message and the styles are invented. Nothing here reads a file, fetches or sends.

use mailcraft_engine::{Session, sample};
use mailcraft_html::{Block, BlockKind, Rgb, Span};
use mailcraft_mime_model::WriteOptions;
use mailcraft_model::{Body, Message, Signature};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

const NAME: &str = "Studio";
const CARD: Rgb = Rgb(0x1f, 0x4e, 0x79);

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// The span the display reader makes of `text`, looking inside tables.
fn span_of(blocks: &[Block], text: &str) -> Option<Span> {
    blocks.iter().find_map(|b| match &b.kind {
        BlockKind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).find_map(|c| span_of(&c.blocks, text)),
        _ => b.spans.iter().find(|s| s.text.contains(text)).cloned(),
    })
}

/// The signature's words have the rule's colour and the quoted card's words don't, in `m` and in
/// `m` written as MIME and read back.
fn assert_scoped(label: &str, m: &Message) {
    let written = mailcraft_mime_model::write(m, &WriteOptions { include_bcc: true, ..Default::default() });
    let parsed = mailcraft_mime_model::parse(&written);
    for (what, m) in [("stored", m), ("written", &parsed)] {
        let html = m.body.html.as_deref().unwrap_or_else(|| panic!("{label} ({what}): an HTML message"));
        let blocks = mailcraft_html::parse(html);
        let ours = span_of(&blocks, "Northwind Studio card").unwrap_or_else(|| panic!("{label} ({what}): the signature's words: {html}"));
        assert_eq!(ours.color, Some(CARD), "{label} ({what}): the signature's rule styles its words: {html}");
        let quoted = span_of(&blocks, "Harbour card").unwrap_or_else(|| panic!("{label} ({what}): the quoted words: {html}"));
        assert_ne!(quoted.color, Some(CARD), "{label} ({what}): the signature's rule reaches the quoted original: {html}");
    }
}

#[test]
fn an_element_of_the_signatures_class_in_a_quoted_original_keeps_its_own_look() {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let mut sig = Signature::plain_text(NAME, "Northwind Studio card\nPriya Raman");
    sig.set_html(Some(
        "<html><head><style>.studio-card { color: #1f4e79; }</style></head><body><div class=\"studio-card\">Northwind Studio card</div><div>Priya Raman</div></body></html>".into(),
    ))
    .expect("the model keeps the HTML");
    s.mailbox.signatures.push(sig);
    let original = Message {
        folder: s.mailbox.inbox().expect("an Inbox"),
        account: s.mailbox.accounts[0].id,
        subject: "Cards".into(),
        body: Body { text: "Harbour card".into(), html: Some("<html><body><div class=\"studio-card\">Harbour card</div></body></html>".into()) },
        ..Default::default()
    };
    let id = s.mailbox.add_message(original).expect("the original is kept");
    run(&mut s, "mail.reply", json!({"id": id}));
    run(&mut s, "compose.signature", json!({"name": NAME}));
    assert!(s.compose.as_ref().and_then(|c| c.signature_html()).is_some(), "fixture guard: the HTML signature is in the reply");
    assert!(
        s.compose.as_ref().is_some_and(|c| c.quote.as_ref().is_some_and(|q| q.html.contains("studio-card"))),
        "fixture guard: the quote keeps the class"
    );

    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    assert_scoped("draft", &s.mailbox.message(saved).expect("the draft").clone());
    run(&mut s, "compose.set", json!({"to": "ada@example.org"}));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().expect("the sent message's id");
    assert_scoped("sent", &s.mailbox.message(sent).expect("the sent message").clone());
}
