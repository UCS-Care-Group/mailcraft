// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Insert › Date & Time and Review › Word Count in the message being written (#507), and
//! Review › Spelling & Grammar with the built-in dictionary (#1797).

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

/// English in Britain writes the day first (#978); a French interface there keeps French's.
#[test]
fn the_date_is_written_the_way_of_the_region() {
    let mut s = session();
    s.date_region = Some("GB".into());
    s.prefs.twelve_hour = false;
    assert_eq!(date_time_text(&s, "long", s.now()).unwrap(), "Wednesday 14 October 2026");
    assert_eq!(date_time_text(&s, "longDate", s.now()).unwrap(), "14 October 2026");
    assert_eq!(date_time_text(&s, "medium", s.now()).unwrap(), "14 Oct 2026");
    assert_eq!(date_time_text(&s, "numericTime", s.now()).unwrap(), "14/10/2026 16:05");
    assert_eq!(date_time_text(&s, "monthYear", s.now()).unwrap(), "October 2026");
    s.interface_language = Lang::French;
    assert_eq!(date_time_text(&s, "longDate", s.now()).unwrap(), "14 octobre 2026");
    s.interface_language = Lang::English;
    s.date_region = Some("US".into());
    assert_eq!(date_time_text(&s, "numeric", s.now()).unwrap(), "10/14/2026");
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

#[test]
fn review_spelling_finds_a_misspelt_word_with_no_os_dictionary() {
    let mut s = writing("I saw teh cat at https://teh.example today");
    run(&mut s, "compose.set", json!({"subject": "Lunch tomorow"}));
    let r = run(&mut s, "review.spelling", json!({}));
    assert_eq!(r["language"], "en-US");
    assert_eq!(r["dictionary"], "built-in");
    assert_eq!(r["partial"], false);
    let issues = r["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 2, "{r}");
    assert_eq!(issues[0]["location"], "subject");
    assert_eq!(issues[0]["word"], "tomorow");
    assert_eq!((issues[0]["from"].as_u64(), issues[0]["to"].as_u64()), (Some(6), Some(13)));
    assert!(issues[0]["suggestions"].as_array().unwrap().contains(&json!("tomorrow")));
    assert_eq!(issues[1]["location"], "body");
    assert_eq!(issues[1]["word"], "teh");
    assert_eq!((issues[1]["from"].as_u64(), issues[1]["to"].as_u64()), (Some(6), Some(9)));
    assert!(issues[1]["suggestions"].as_array().unwrap().contains(&json!("the")));
}

fn words_of(r: &Value) -> Vec<&str> {
    r["issues"].as_array().unwrap().iter().map(|issue| issue["word"].as_str().unwrap()).collect()
}

fn issue<'a>(r: &'a Value, word: &str) -> &'a Value {
    r["issues"].as_array().unwrap().iter().find(|issue| issue["word"] == word).unwrap_or_else(|| panic!("no {word} in {r}"))
}

#[test]
fn spelling_is_bound_to_f7_alone() {
    let s = session();
    assert_eq!(s.registry.get("review.spelling").map(|c| c.shortcut), Some("F7"));
    let bound: Vec<&str> = s.registry.all().iter().filter(|c| c.shortcut.split(" / ").any(|k| k == "F7")).map(|c| c.id).collect();
    assert_eq!(bound, ["review.spelling"]);
}

#[test]
fn spelling_needs_a_message_being_written() {
    let mut s = session();
    let e = s.run("review.spelling", &json!({})).unwrap_err().to_string();
    assert!(e.contains("no message is being written"), "{e}");
    assert!((s.registry.get("review.spelling").unwrap().enabled)(&s).is_some());
}

#[test]
fn spelling_accepts_only_the_built_in_language() {
    let mut s = writing("I saw teh cat");
    for lang in ["en-US", "en-us", "EN-US", "en-US-x-builtin", "EN-us-x-builtin"] {
        let r = run(&mut s, "review.spelling", json!({"language": lang}));
        assert_eq!(r["language"], "en-US", "{lang}");
        assert_eq!(r["dictionary"], "built-in");
        assert_eq!(r["version"], mailcraft_spell::EN_US_VERSION);
        assert_eq!(words_of(&r), ["teh"]);
    }
    for lang in [json!("fr-FR"), json!("en"), json!("en-GB"), json!(""), json!(1), json!(true)] {
        let e = s.run("review.spelling", &json!({"language": lang})).unwrap_err().to_string();
        assert!(e.contains("only en-US"), "{lang}: {e}");
        assert_eq!(body(&s), "I saw teh cat");
    }
}

#[test]
fn spelling_can_leave_the_subject_out() {
    let mut s = writing("I saw teh cat");
    run(&mut s, "compose.set", json!({"subject": "Lunch tomorow"}));
    let r = run(&mut s, "review.spelling", json!({"includeSubject": false}));
    assert_eq!(r["partial"], false);
    assert!(r["issues"].as_array().unwrap().iter().all(|issue| issue["location"] == "body"), "{r}");
    assert_eq!(words_of(&r), ["teh"]);
    let e = s.run("review.spelling", &json!({"includeSubject": "no"})).unwrap_err().to_string();
    assert!(e.contains("includeSubject"), "{e}");
}

#[test]
fn custom_words_are_accepted_and_may_be_suggested() {
    let mut s = writing("teh tehh");
    assert!(words_of(&run(&mut s, "review.spelling", json!({}))).contains(&"teh"));
    let r = run(&mut s, "review.spelling", json!({"customWords": ["teh"]}));
    assert!(!words_of(&r).contains(&"teh"), "{r}");
    let near = issue(&r, "tehh");
    assert!(near["suggestions"].as_array().unwrap().contains(&json!("teh")), "{near}");
    assert!(near["suggestions"].as_array().unwrap().len() <= 6);
}

#[test]
fn ignored_words_are_not_reported() {
    let mut s = writing("I saw teh cat");
    run(&mut s, "compose.set", json!({"subject": "Lunch tomorow"}));
    let r = run(&mut s, "review.spelling", json!({"ignore": ["teh"]}));
    assert_eq!(words_of(&r), ["tomorow"]);
    assert_eq!(r["issues"][0]["location"], "subject");
}

#[test]
fn custom_word_lists_skip_bad_entries_and_refuse_the_wrong_shape() {
    let mut s = writing("I saw teh cat");
    let long = "a".repeat(crate::spelling::MAX_WORD_CHARS + 1);
    let r = run(&mut s, "review.spelling", json!({"customWords": ["teh", "two words", "", "bad!", long], "ignore": ["   ", "has space"]}));
    assert!(words_of(&r).is_empty(), "{r}");
    for params in [json!({"customWords": "teh"}), json!({"customWords": ["teh", 1]}), json!({"ignore": {"word": "teh"}}), json!({"ignore": [true]})] {
        let e = s.run("review.spelling", &params).unwrap_err().to_string();
        assert!(e.contains("list of words"), "{params}: {e}");
    }
    let too_many = Value::Array(vec![json!("a"); crate::spelling::MAX_CUSTOM_WORDS + 1]);
    for key in ["customWords", "ignore"] {
        let e = s.run("review.spelling", &json!({key: too_many.clone()})).unwrap_err().to_string();
        assert!(e.contains("10000"), "{key}: {e}");
    }
    assert_eq!(body(&s), "I saw teh cat");
}

#[test]
fn a_body_range_selects_the_reported_word() {
    let text = "Hello there\n\nCafé — naïve teh";
    let mut s = writing(text);
    let before = body(&s);
    let r = run(&mut s, "review.spelling", json!({}));
    assert_eq!(r["partial"], false);
    let teh = issue(&r, "teh");
    assert_eq!(teh["location"], "body");
    let from = teh["from"].as_u64().unwrap();
    let to = teh["to"].as_u64().unwrap();
    assert!(before.chars().take(from as usize).any(|c| !c.is_ascii()), "non-ASCII before the typo: {before}");
    assert!(from > "Hello there".chars().count() as u64, "the typo is not in the first paragraph");
    let selected = run(&mut s, "compose.select", json!({"from": from, "to": to}));
    assert_eq!((selected["from"].as_u64(), selected["to"].as_u64()), (Some(from), Some(to)));
    let plain = body(&s);
    let got: String = plain.chars().skip(from as usize).take((to - from) as usize).collect();
    assert_eq!(got, "teh");
    assert_eq!(plain, before);
}

#[test]
fn a_link_a_url_and_an_email_are_not_reported() {
    let mut s = writing("Click teh now, mail ada@example.org or https://teh.example/x today");
    run(&mut s, "compose.link", json!({"from": 6, "to": 9, "url": "https://example.org/teh"}));
    let r = run(&mut s, "review.spelling", json!({}));
    for word in words_of(&r) {
        assert_ne!(word, "teh", "{r}");
        assert!(
            !word.contains('@') && !word.contains('/') && !word.eq_ignore_ascii_case("https") && !word.eq_ignore_ascii_case("example"),
            "{word} in {r}"
        );
    }
}

#[test]
fn more_than_256_typos_is_a_partial_report() {
    let mut s = writing(&"teh ".repeat(mailcraft_spell::MAX_ISSUES + 1));
    let r = run(&mut s, "review.spelling", json!({}));
    assert_eq!(r["partial"], true);
    let issues = r["issues"].as_array().unwrap();
    assert_eq!(issues.len(), mailcraft_spell::MAX_ISSUES);
    assert!(issues.iter().all(|issue| issue["word"] == "teh" && issue["suggestions"].as_array().unwrap().len() <= 6));
}

#[test]
fn a_paragraph_over_8_kb_is_partial() {
    let mut s = writing(&format!("teh\n{}", "a".repeat(crate::spelling::MAX_PARAGRAPH_BYTES + 1)));
    let r = run(&mut s, "review.spelling", json!({}));
    assert_eq!(r["partial"], true);
    assert_eq!(words_of(&r), ["teh"]);
    assert_eq!(r["issues"][0]["location"], "body");
}

#[test]
fn a_signature_being_edited_is_checked_without_the_message_subject() {
    let mut s = writing("Hello there");
    run(&mut s, "compose.set", json!({"subject": "Lunch tomorow"}));
    run(&mut s, "signature.set", json!({"name": "Proof", "text": "Best teh"}));
    run(&mut s, "signature.open", json!({"name": "Proof"}));
    assert!(s.compose.is_some() && s.signature_edit.is_some());
    let r = run(&mut s, "review.spelling", json!({"includeSubject": true}));
    assert!(r["issues"].as_array().unwrap().iter().all(|issue| issue["location"] == "body"), "{r}");
    assert_eq!(words_of(&r), ["teh"]);
    let teh = issue(&r, "teh");
    let (from, to) = (teh["from"].as_u64().unwrap(), teh["to"].as_u64().unwrap());
    run(&mut s, "compose.select", json!({"from": from, "to": to}));
    let plain = s.doc().unwrap().0.plain();
    let got: String = plain.chars().skip(from as usize).take((to - from) as usize).collect();
    assert_eq!(got, "teh");
    run(&mut s, "signature.close", json!({"discard": true}));
    let r = run(&mut s, "review.spelling", json!({}));
    assert_eq!(words_of(&r), ["tomorow"]);
    assert_eq!(r["issues"][0]["location"], "subject");
}
