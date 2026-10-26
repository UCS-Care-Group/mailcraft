// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Insert › Date & Time and Review › Word Count in the message being written (#507).

use mailcraft_i18n::Lang;
use mailcraft_model::RichText;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use super::writing_tools::{DATE_TIME_FORMATS, count, date_time_choices, date_time_text};
use crate::{Session, sample};

/// Wednesday, October 14, 2026, 4:05 PM, on a clock at UTC.
fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 5, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn body(s: &Session) -> String {
    s.compose.as_ref().unwrap().body.plain()
}

/// A new message whose body is `text` alone (the signature typed over).
fn writing(text: &str) -> Session {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    let len = s.compose.as_ref().unwrap().body.text().len();
    run(&mut s, "compose.select", json!({"from": 0, "to": len}));
    run(&mut s, "compose.type", json!({"text": text}));
    assert_eq!(body(&s), text);
    s
}

#[test]
fn every_format_says_now_on_the_owners_clock() {
    let s = session();
    let said: Vec<(&str, String)> = date_time_choices(&s, s.now());
    let want = [
        ("long", "Wednesday, October 14, 2026"),
        ("longDate", "October 14, 2026"),
        ("medium", "Oct 14, 2026"),
        ("numeric", "10/14/2026"),
        ("iso", "2026-10-14"),
        ("numericTime", "10/14/2026 4:05 PM"),
        ("time", "4:05 PM"),
        ("monthYear", "October 2026"),
    ];
    assert_eq!(said, want.iter().map(|(f, t)| (*f, t.to_string())).collect::<Vec<_>>());
    assert_eq!(said.len(), DATE_TIME_FORMATS.len());
    assert_eq!(date_time_text(&s, "nonsense", s.now()), None);
}

#[test]
fn the_date_is_in_the_interface_language_and_its_clock() {
    let mut s = session();
    s.interface_language = Lang::French;
    s.prefs.twelve_hour = false;
    assert_eq!(date_time_text(&s, "longDate", s.now()).unwrap(), "14 octobre 2026");
    assert_eq!(date_time_text(&s, "numeric", s.now()).unwrap(), "14/10/2026");
    assert_eq!(date_time_text(&s, "time", s.now()).unwrap(), "16:05");
    // ISO is the same in every language.
    assert_eq!(date_time_text(&s, "iso", s.now()).unwrap(), "2026-10-14");
}

#[test]
fn the_date_goes_in_at_the_caret_and_over_a_selection() {
    let mut s = writing("Seen on . Thanks");
    run(&mut s, "compose.select", json!({"from": 8}));
    let r = run(&mut s, "insert.dateTime", json!({"format": "medium"}));
    assert_eq!(r["text"], "Oct 14, 2026");
    assert_eq!(body(&s), "Seen on Oct 14, 2026. Thanks");
    assert_eq!(r["from"], 20);
    assert!(s.compose.as_ref().unwrap().dirty);
    // Over a selection, as typing is: "Thanks" becomes the time.
    run(&mut s, "compose.select", json!({"from": 22, "to": 28}));
    run(&mut s, "insert.dateTime", json!({"format": "time"}));
    assert_eq!(body(&s), "Seen on Oct 14, 2026. 4:05 PM");
    // And it is undone as typing is.
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(body(&s), "Seen on Oct 14, 2026. Thanks");
}

#[test]
fn the_date_is_of_the_instant_named() {
    // What the dialog showed when it opened goes in, whatever the clock says by OK.
    let mut s = writing("");
    let before = s.now() - 2 * 86_400;
    let r = run(&mut s, "insert.dateTime", json!({"format": "long", "at": before}));
    assert_eq!(r["text"], "Monday, October 12, 2026");
    assert_eq!(body(&s), "Monday, October 12, 2026");
    for at in [json!("yesterday"), json!(true), json!(1e300)] {
        let e = s.run("insert.dateTime", &json!({"format": "iso", "at": at})).unwrap_err().to_string();
        assert!(e.contains("`at`"), "{at}: {e}");
    }
    // Far out of range is a date at the end of the calendar, not a panic.
    run(&mut s, "insert.dateTime", json!({"format": "iso", "at": i64::MAX}));
    run(&mut s, "insert.dateTime", json!({"format": "time", "at": i64::MIN}));
}

#[test]
fn a_plain_text_message_takes_the_date_too() {
    let mut s = writing("Today: ");
    run(&mut s, "compose.formatPlain", json!({}));
    run(&mut s, "compose.select", json!({"from": 7}));
    run(&mut s, "insert.dateTime", json!({"format": "iso"}));
    assert_eq!(body(&s), "Today: 2026-10-14");
}

#[test]
fn nothing_is_inserted_without_a_message_or_a_format() {
    let mut s = session();
    let e = s.run("insert.dateTime", &json!({"format": "long"})).unwrap_err().to_string();
    assert!(e.contains("no message is being written"), "{e}");
    assert!(s.run("review.wordCount", &json!({})).is_err());
    let mut s = writing("Hello");
    for params in [json!({}), json!({"format": "full"}), json!({"format": 5}), json!(null)] {
        let e = s.run("insert.dateTime", &params).unwrap_err().to_string();
        assert!(e.contains("long, longDate"), "{params}: {e}");
        assert_eq!(body(&s), "Hello");
    }
}

#[test]
fn word_count_counts_the_body() {
    let mut s = writing("Hello, world!\n\n  don't stop  \n日本");
    // A word split by a change of formatting is still one word.
    run(&mut s, "format.bold", json!({"from": 7, "to": 10}));
    run(&mut s, "compose.select", json!({"from": 3}));
    let r = run(&mut s, "review.wordCount", json!({}));
    assert_eq!(r, json!({"words": 6, "characters": 23, "charactersWithSpaces": 29, "paragraphs": 3, "selection": false}));
    // Plain text counts the same.
    run(&mut s, "compose.formatPlain", json!({}));
    assert_eq!(run(&mut s, "review.wordCount", json!({}))["words"], 6);
}

#[test]
fn word_count_counts_the_selection_when_there_is_one() {
    let mut s = writing("Hello, world!\nSecond line here");
    run(&mut s, "compose.select", json!({"from": 7, "to": 20}));
    let r = run(&mut s, "review.wordCount", json!({}));
    assert_eq!(r, json!({"words": 2, "characters": 12, "charactersWithSpaces": 12, "paragraphs": 2, "selection": true}));
    // Named in the params, backwards too; out of range is refused, not a panic.
    let r = run(&mut s, "review.wordCount", json!({"from": 5, "to": 0}));
    assert_eq!((r["words"].as_u64(), r["selection"].as_bool()), (Some(1), Some(true)));
    assert!(s.run("review.wordCount", &json!({"from": 0, "to": 1_000_000})).is_err());
    // A `from` alone counts from there to the end; a `to` alone, from the start.
    let r = run(&mut s, "review.wordCount", json!({"from": 14}));
    assert_eq!((r["words"].as_u64(), r["paragraphs"].as_u64(), r["selection"].as_bool()), (Some(3), Some(1), Some(true)));
    let r = run(&mut s, "review.wordCount", json!({"to": 5}));
    assert_eq!((r["words"].as_u64(), r["characters"].as_u64(), r["selection"].as_bool()), (Some(1), Some(5), Some(true)));
    // An empty part named is nothing, not the whole body.
    let r = run(&mut s, "review.wordCount", json!({"from": 3, "to": 3}));
    assert_eq!((r["words"].as_u64(), r["selection"].as_bool()), (Some(0), Some(true)));
    // With nothing selected and nothing named: the whole body.
    run(&mut s, "compose.select", json!({"from": 3}));
    assert_eq!(run(&mut s, "review.wordCount", json!({}))["selection"], false);
    // Counting changes nothing.
    assert_eq!(body(&s), "Hello, world!\nSecond line here");
}

#[test]
fn words_are_unicode_words() {
    let words = |t: &str| count(&RichText::from_plain(t)).words;
    assert_eq!(words(""), 0);
    assert_eq!(words("  — !! "), 0);
    assert_eq!(words("don't 3.14 e-mail"), 4);
    // Each ideograph is a word; a run of katakana is one.
    assert_eq!(words("東京タワー"), 3);
    assert_eq!(words("tab\tseparated\u{a0}words"), 3);
    // Addresses are several words, as their parts are.
    assert_eq!(words("maya@lumen.example"), 2);
    assert!(words("https://www.example.org/notes") > 1);
}

#[test]
fn a_huge_body_is_counted_in_one_pass() {
    // The most a message holds (`MAX_CHARS`), as many paragraphs as it may have.
    let line = "word ".repeat(19);
    let text = vec![line.as_str(); 10_000].join("\n");
    let c = count(&RichText::from_plain(&text));
    assert_eq!((c.words, c.paragraphs), (190_000, 10_000));
}
