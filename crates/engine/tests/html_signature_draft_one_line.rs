// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A draft whose own text is one empty line above its HTML signature opens again as it was saved:
//! that line is the owner's, written in the draft's HTML, and is kept, so the signature is still
//! shown as it goes out, under the text and out of the keyboard's reach. Where the document only
//! opens, there is no line to keep, whatever the head holds or a tag whose name only starts
//! with `br`.
//!
//! The people and the styles are invented. Nothing here reads a file, fetches or sends.

use mailcraft_engine::{Session, sample};
use mailcraft_model::Signature;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// Each paragraph of the message being written: whether it is the signature's.
fn layout(s: &Session) -> Vec<bool> {
    s.compose.as_ref().expect("a message is being written").body.text().paragraphs().iter().map(|p| p.signature).collect()
}

/// A session writing a new message with the HTML signature "Studio".
fn writing() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let account = s.mailbox.default_account().expect("the sample mailbox has an account").id;
    let mut sig = Signature::plain_text("Studio", "Northwind Studio card\nPriya Raman");
    sig.set_html(Some(
        "<html><head><style>.studio-card { color: #1f4e79; }</style></head><body><div class=\"studio-card\">Northwind Studio card</div><div>Priya Raman</div></body></html>".into(),
    ))
    .expect("the model keeps the HTML");
    s.mailbox.signatures.push(sig);
    run(&mut s, "signature.defaults", json!({"account": account, "new": "Studio"}));
    run(&mut s, "mail.new", json!({"account": account, "to": "ada@example.org", "subject": "Card"}));
    s
}

#[test]
fn one_empty_line_above_the_signature_is_kept_when_the_draft_opens_again() {
    let mut s = writing();
    // The owner takes out one of the two empty lines above the signature.
    run(&mut s, "compose.delete", json!({"from": 0, "to": 1}));
    assert_eq!(layout(&s), [false, true, true], "fixture guard: one empty line, then the signature");
    assert!(s.compose.as_ref().and_then(|c| c.signature_html_shown()).is_some(), "fixture guard: the signature is shown as it goes out");

    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": saved}));
    assert_eq!(layout(&s), [false, true, true], "the empty line above the signature is gone");
    assert!(s.compose.as_ref().and_then(|c| c.signature_html_shown()).is_some(), "the signature is no longer shown as it goes out");
}

#[test]
fn only_a_line_break_in_the_body_is_an_empty_line_above_the_signature() {
    let mut s = writing();
    // The owner's text is one empty line, which the draft's HTML writes as a `<br>`.
    run(&mut s, "compose.delete", json!({"from": 0, "to": 1}));
    assert_eq!(layout(&s), [false, true, true], "fixture guard: one empty line, then the signature");
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    run(&mut s, "compose.close", json!({}));
    // That line break made into what only looks like one: a tag whose name only starts with `br`,
    // which reads as no text, and a `<br>` in a comment of the head.
    let draft = s.mailbox.message_mut(saved).expect("the saved draft");
    let html = draft.body.html.clone().expect("an HTML draft");
    let body = html.find("<body").expect("fixture guard: the draft's HTML has a body");
    let open = body + html[body..].find('>').expect("the body's start tag ends") + 1;
    let line = open + html[open..].find("<br").expect("fixture guard: the empty line's break");
    assert!(html[line..].contains("Northwind Studio card"), "fixture guard: the break is above the signature: {html}");
    draft.body.html = Some(format!("{}<!-- <br> -->{}<brx{}", &html[..body], &html[body..line], &html[line + 3..]));
    run(&mut s, "mail.editDraft", json!({"id": saved}));
    // As a draft whose document only opens above the signature.
    assert_eq!(layout(&s), [true, true], "an empty line was made of what is no line break");
}
