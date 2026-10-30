// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the sender hid in a message (#599) is in search and rules, and is not in its text.
use crate::rule::Condition;
use crate::search::Query;
use crate::{Body, MAX_HIDDEN_TEXT, Message, words};

fn newsletter() -> Message {
    Message {
        subject: "The autumn cheeses".into(),
        body: Body { text: "Three new cheeses.".into(), html: None },
        hidden_text: "Preview: the dairy open day is on the fourteenth".into(),
        ..Default::default()
    }
}

#[test]
fn search_finds_what_the_sender_hid() {
    let m = newsletter();
    for query in ["open day", "fourteenth", "body:fourteenth", "cheeses"] {
        assert!(Query::parse(query).matches(&m, 0), "{query}");
    }
    assert!(!Query::parse("unrelated").matches(&m, 0));
    // The visible text is the message's own.
    assert!(!m.body.text.contains("open day"));
    // And the one list of what search reads, which the word index uses too, has it.
    let mut seen = Vec::new();
    words::texts(&m, &m.body.text, |_, text| {
        seen.push(text.to_string());
        false
    });
    assert!(seen.iter().any(|t| t.contains("open day")), "{seen:?}");
}

#[test]
fn a_rule_on_the_body_matches_what_the_sender_hid() {
    let m = newsletter();
    assert!(Condition::BodyContains("open day".into()).matches(&m));
    assert!(Condition::BodyContains("THREE NEW".into()).matches(&m));
    assert!(!Condition::BodyContains("lighthouse".into()).matches(&m));
}

#[test]
fn a_message_without_hidden_text_reads_as_before() {
    let m = Message { body: Body { text: "Plain words".into(), html: None }, ..Default::default() };
    assert!(Query::parse("plain").matches(&m, 0));
    assert!(!Query::parse("preview").matches(&m, 0));
    let json = serde_json::to_string(&m).unwrap_or_default();
    assert!(!json.contains("hiddenText"), "{json}");
    let with = serde_json::to_string(&newsletter()).unwrap_or_default();
    let back: Message = serde_json::from_str(&with).unwrap_or_default();
    assert_eq!(back.hidden_text, newsletter().hidden_text);
}

#[test]
fn a_hostile_file_cannot_load_a_huge_hidden_text() {
    let json = format!(
        r#"{{"accounts": [{{"id": 1, "name": "A"}}], "folders": [{{"id": 5, "account": 1, "name": "Inbox", "kind": "inbox"}}],
        "messages": {{"3": {{"id": 3, "folder": 5, "subject": "ok", "hiddenText": "{}"}}, "4": {{"id": 4, "folder": 5, "hiddenText": "short"}}}}, "nextId": 9}}"#,
        "é".repeat(MAX_HIDDEN_TEXT)
    );
    let mut mb: crate::Mailbox = serde_json::from_str(&json).unwrap();
    assert!(mb.message(3).unwrap().hidden_text.len() > MAX_HIDDEN_TEXT);
    let fixed = mb.repair();
    assert!(fixed.iter().any(|f| f.contains("hidden text")), "{fixed:?}");
    let cut = &mb.message(3).unwrap().hidden_text;
    assert!(cut.len() <= MAX_HIDDEN_TEXT && cut.len() > MAX_HIDDEN_TEXT - 4 && cut.chars().all(|c| c == 'é'));
    assert_eq!(mb.message(4).unwrap().hidden_text, "short");
}
