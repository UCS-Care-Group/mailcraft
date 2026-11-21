// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A spelling replacement in the Subject is one step of the message's undo. Characters typed
//! into the Subject are not: the field undoes those itself.

use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

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

/// Character offsets of the first `word` in `text`.
fn span(text: &str, word: &str) -> (usize, usize) {
    let byte = text.find(word).unwrap();
    let from = text[..byte].chars().count();
    (from, from + word.chars().count())
}

#[test]
fn typing_the_subject_is_not_a_message_undo_step() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"subject": "Lunch tomorow"}));
    assert_eq!(compose(&s).subject, "Lunch tomorow");
    assert!(s.run("compose.undo", &json!({})).is_err(), "the field undoes what was typed, not compose.undo");
}

#[test]
fn a_replacement_is_one_undo_step_and_only_that_word() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    let subject = "Café résumé tomorow";
    run(&mut s, "compose.set", json!({"subject": subject}));
    run(&mut s, "compose.type", json!({"text": "Hi"}));
    let body = compose(&s).body.plain();
    let (from, to) = span(subject, "tomorow");
    assert_eq!((from, to), (12, 19));

    run(&mut s, "compose.replaceSubject", json!({"from": from, "to": to, "expected": "tomorow", "text": "tomorrow"}));
    assert_eq!(compose(&s).subject, "Café résumé tomorrow");
    assert_eq!(compose(&s).body.plain(), body);

    run(&mut s, "compose.undo", json!({}));
    assert_eq!(compose(&s).subject, subject, "one undo puts the word back and leaves the body");
    assert_eq!(compose(&s).body.plain(), body);

    run(&mut s, "compose.redo", json!({}));
    assert_eq!(compose(&s).subject, "Café résumé tomorrow");
    assert_eq!(compose(&s).body.plain(), body);

    run(&mut s, "compose.undo", json!({}));
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(compose(&s).subject, subject);
    assert_ne!(compose(&s).body.plain(), body, "the next undo is the typing, not another subject step");
}

#[test]
fn only_the_occurrence_at_the_offsets_changes() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    let subject = "tomorow and tomorow";
    run(&mut s, "compose.set", json!({"subject": subject}));
    let (from, to) = span(subject, "tomorow");
    run(&mut s, "compose.replaceSubject", json!({"from": from, "to": to, "expected": "tomorow", "text": "tomorrow"}));
    assert_eq!(compose(&s).subject, "tomorrow and tomorow");
}

#[test]
fn a_replacement_is_refused_when_the_subject_changed_and_leaves_no_step() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"subject": "Lunch tomorow"}));
    let err = s.run("compose.replaceSubject", &json!({"from": 6, "to": 13, "expected": "tomorrow", "text": "tomorrow"}));
    assert!(err.is_err());
    assert_eq!(compose(&s).subject, "Lunch tomorow");
    assert!(s.run("compose.undo", &json!({})).is_err());

    let (from, to) = span("Lunch tomorow", "tomorow");
    run(&mut s, "compose.replaceSubject", json!({"from": from, "to": to, "expected": "tomorow", "text": "tomorrow"}));
    run(&mut s, "compose.set", json!({"subject": "Lunch today"}));
    assert!(s.run("compose.undo", &json!({})).is_err(), "typing over the replacement drops that step");
    assert_eq!(compose(&s).subject, "Lunch today");
}
