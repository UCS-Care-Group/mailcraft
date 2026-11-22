// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A spelling replacement in the Subject is one step of the message's undo. Characters typed
//! into the Subject are not: the field undoes those itself.

use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::session::SubjectStep;
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

/// A picture's width and a Subject replacement share one undo list: the replacement is the later
/// step, and putting the width back does not bring the replacement with it.
#[test]
fn picture_width_and_subject_replacement_undo_and_redo_in_order() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    let mut png = b"\x89PNG\r\n\x1a\nIHDR....".to_vec();
    png.resize(png.len() + 8, 7);
    let data = mailcraft_charset::base64::STANDARD.encode(png);
    let id = run(&mut s, "insert.picture", json!({"data": data, "file": "chart.png"}))["id"].as_u64().unwrap();
    run(&mut s, "compose.picture.set", json!({"id": id, "width": 80}));
    run(&mut s, "compose.set", json!({"subject": "Lunch tomorow"}));
    let (from, to) = span("Lunch tomorow", "tomorow");
    run(&mut s, "compose.replaceSubject", json!({"from": from, "to": to, "expected": "tomorow", "text": "tomorrow"}));
    assert_eq!(compose(&s).subject, "Lunch tomorrow");
    assert_eq!(picture_width(&s, id), Some(80));

    run(&mut s, "compose.undo", json!({}));
    assert_eq!(compose(&s).subject, "Lunch tomorow");
    assert_eq!(picture_width(&s, id), Some(80));

    run(&mut s, "compose.undo", json!({}));
    assert_eq!(picture_width(&s, id), None);
    assert_eq!(compose(&s).subject, "Lunch tomorow");

    run(&mut s, "compose.redo", json!({}));
    assert_eq!(picture_width(&s, id), Some(80));
    assert_eq!(compose(&s).subject, "Lunch tomorow");

    run(&mut s, "compose.redo", json!({}));
    assert_eq!(compose(&s).subject, "Lunch tomorrow");
    assert_eq!(picture_width(&s, id), Some(80));
}

#[test]
fn a_body_edit_past_the_undo_cap_is_undone_before_the_subject() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"subject": "Hello tomorow"}));
    for n in 0..100 {
        run(&mut s, "compose.set", json!({"body": format!("step {n}")}));
    }
    let capped = compose(&s).body.plain();
    assert_eq!(compose(&s).body.undo_len(), 100);
    let (from, to) = span("Hello tomorow", "tomorow");
    run(&mut s, "compose.replaceSubject", json!({"from": from, "to": to, "expected": "tomorow", "text": "tomorrow"}));
    run(&mut s, "compose.set", json!({"body": "newest"}));
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(compose(&s).body.plain(), capped, "the newest body edit is undone first");
    assert_eq!(compose(&s).subject, "Hello tomorrow");
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(compose(&s).subject, "Hello tomorow");
}

#[test]
fn body_markers_stay_within_the_retained_undo_steps() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"subject": "Hello tomorow"}));
    for n in 0..200 {
        run(&mut s, "compose.set", json!({"body": format!("step {n}")}));
    }
    assert_eq!(compose(&s).body.undo_len(), 100, "the text keeps at most 100 snapshots");
    let (from, to) = span("Hello tomorow", "tomorow");
    run(&mut s, "compose.replaceSubject", json!({"from": from, "to": to, "expected": "tomorow", "text": "tomorrow"}));
    let markers = compose(&s).subject_edits.undo.iter().filter(|step| matches!(step, SubjectStep::Body)).count();
    assert_eq!(markers, compose(&s).body.undo_len(), "each marker has a snapshot the text can still restore");
    assert!(matches!(compose(&s).subject_edits.undo.last(), Some(SubjectStep::Subject { .. })));
}

#[test]
fn a_subject_replacement_ends_the_body_typing_group() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"subject": "Hello tomorow", "body": ""}));
    run(&mut s, "compose.type", json!({"text": "A"}));
    let (from, to) = span("Hello tomorow", "tomorow");
    run(&mut s, "compose.replaceSubject", json!({"from": from, "to": to, "expected": "tomorow", "text": "tomorrow"}));
    run(&mut s, "compose.type", json!({"text": "B"}));
    assert_eq!(compose(&s).body.plain(), "AB");
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(compose(&s).body.plain(), "A");
    assert_eq!(compose(&s).subject, "Hello tomorrow");
}

#[test]
fn resizing_a_picture_after_undoing_a_subject_correction_drops_that_redo() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    let mut png = b"\x89PNG\r\n\x1a\nIHDR....".to_vec();
    png.resize(png.len() + 8, 7);
    let data = mailcraft_charset::base64::STANDARD.encode(png);
    let id = run(&mut s, "insert.picture", json!({"data": data, "file": "chart.png"}))["id"].as_u64().unwrap();
    run(&mut s, "compose.set", json!({"subject": "Lunch tomorow"}));
    let (from, to) = span("Lunch tomorow", "tomorow");
    run(&mut s, "compose.replaceSubject", json!({"from": from, "to": to, "expected": "tomorow", "text": "tomorrow"}));
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(compose(&s).subject, "Lunch tomorow");

    run(&mut s, "compose.picture.set", json!({"id": id, "width": 80}));
    assert_eq!(picture_width(&s, id), Some(80));
    if s.run("compose.redo", &json!({})).is_ok() {
        assert_eq!(picture_width(&s, id), Some(80));
    }
    assert_eq!(compose(&s).subject, "Lunch tomorow", "redo does not bring the subject correction back");

    run(&mut s, "compose.undo", json!({}));
    run(&mut s, "compose.replaceSubject", json!({"from": from, "to": to, "expected": "tomorow", "text": "tomorrow"}));
    run(&mut s, "compose.undo", json!({}));
    let same = picture_width(&s, id);
    run(&mut s, "compose.picture.set", json!({"id": id, "width": same}));
    run(&mut s, "compose.redo", json!({}));
    assert_eq!(compose(&s).subject, "Lunch tomorrow", "an unchanged width keeps the subject redo");
}

#[test]
fn inserting_a_picture_after_undoing_a_subject_correction_drops_that_redo() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"subject": "Lunch tomorow"}));
    let (from, to) = span("Lunch tomorow", "tomorow");
    run(&mut s, "compose.replaceSubject", json!({"from": from, "to": to, "expected": "tomorow", "text": "tomorrow"}));
    run(&mut s, "compose.undo", json!({}));
    let mut png = b"\x89PNG\r\n\x1a\nIHDR....".to_vec();
    png.resize(png.len() + 8, 7);
    let data = mailcraft_charset::base64::STANDARD.encode(png);
    run(&mut s, "insert.picture", json!({"data": data, "file": "chart.png"}));
    let _ = s.run("compose.redo", &json!({}));
    assert_eq!(compose(&s).subject, "Lunch tomorow");
}

fn picture_width(s: &Session, id: u64) -> Option<u16> {
    compose(s).pictures.iter().find(|p| u64::from(p.id) == id).and_then(|p| p.width)
}
