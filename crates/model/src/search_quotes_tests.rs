// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #367: what is in quotes is text, even when it reads like a keyword.

use crate::Message;
use crate::search::{Query, ServerTerm, keyword_pills, keyword_spans, suggestions};

fn finds(q: &str, m: &Message) -> bool {
    Query::parse(q).matches(m, 0)
}

fn unread() -> Message {
    Message { read: false, subject: "Lunch".into(), ..Default::default() }
}

fn notes() -> Message {
    Message { read: true, subject: "Notes on is:unread".into(), ..Default::default() }
}

/// `"is:unread"` looks for the words, as `subject:"is:unread"` does, not for unread mail.
#[test]
fn a_keyword_in_quotes_is_text() {
    assert!(finds("\"is:unread\"", &notes()) && !finds("\"is:unread\"", &unread()));
    assert!(finds("is:unread", &unread()) && !finds("is:unread", &notes()), "without quotes, as before");
    let budget = Message { read: true, subject: "Budget from: ada".into(), ..Default::default() };
    assert!(finds("\"from: ada\"", &budget));
    // A `-` before the quotes leaves the words out; one inside them is text.
    assert!(!finds("-\"is:unread\"", &notes()) && finds("-\"is:unread\"", &unread()));
    assert!(finds("\"-is:unread\"", &notes()) && !finds("\"-is:unread\"", &unread()));
}

/// A keyword before a value in quotes takes it, whatever the value reads like.
#[test]
fn a_value_in_quotes_is_the_keywords() {
    assert!(finds("subject: \"is:unread\"", &notes()) && !finds("subject: \"is:unread\"", &unread()));
    assert!(finds("subject:\"is:unread\"", &notes()));
    assert_eq!(keyword_spans("subject: \"is:unread\"").len(), 1);
}

/// The search box draws no pill and offers nothing for a keyword in quotes.
#[test]
fn the_box_shows_quoted_text_as_text() {
    for q in ["\"is:unread\"", "-\"is:unread\"", "\"category:gr", "\"is:"] {
        assert!(keyword_spans(q).is_empty(), "{q}");
        assert!(keyword_pills(q, Some(q.len())).is_empty(), "{q}");
        assert!(suggestions(q, q.len(), &[]).is_empty(), "{q}");
    }
    let q = "\"category:\" ";
    assert!(suggestions(q, q.len(), &[crate::Category { name: "Green".into(), color: crate::Rgb(0, 0, 0) }]).is_empty());
}

/// A server is asked for the words in quotes, as for any other words.
#[test]
fn a_server_is_asked_for_the_words() {
    let split = Query::parse("\"is:unread\"").for_server();
    assert!(split.local.is_empty());
    assert!(matches!(split.terms.as_slice(), [ServerTerm::Words { field: None, .. }]), "{:?}", split.terms);
}
