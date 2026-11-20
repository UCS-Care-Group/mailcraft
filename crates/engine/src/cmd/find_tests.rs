// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Find and Replace in the message being written.

use mailcraft_model::rich::Selection;
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

/// What was written above the signature the sample account adds.
fn mine(s: &Session) -> String {
    let text = compose(s).body.plain();
    text.split("\n\nAvery Quinn").next().unwrap_or("").to_string()
}

fn writing(text: &str) -> Session {
    let mut s = session();
    run(&mut s, "mail.new", json!({"body": text}));
    run(&mut s, "compose.select", json!({"from": 0}));
    s
}

#[test]
fn find_selects_the_next_place_and_goes_round() {
    let mut s = writing("one Two two\nthree two");
    let r = run(&mut s, "edit.find", json!({"text": "two"}));
    assert_eq!((r["found"].as_bool(), r["from"].as_u64(), r["to"].as_u64(), r["count"].as_u64()), (Some(true), Some(4), Some(7), Some(3)));
    assert_eq!(compose(&s).body.selection(), Selection { anchor: 4, head: 7 });
    assert_eq!(run(&mut s, "edit.find", json!({"text": "two"}))["from"], 8);
    assert_eq!(run(&mut s, "edit.find", json!({"text": "two"}))["from"], 18);
    // Round to the first.
    assert_eq!(run(&mut s, "edit.find", json!({"text": "two"}))["from"], 4);
    // Backward, round to the last.
    assert_eq!(run(&mut s, "edit.find", json!({"text": "two", "backward": true}))["from"], 18);
    // Letter case, when asked.
    let r = run(&mut s, "edit.find", json!({"text": "two", "matchCase": true}));
    assert_eq!((r["from"].as_u64(), r["count"].as_u64()), (Some(8), Some(2)));
    // Not there: nothing selected anew, and no error.
    let before = compose(&s).body.selection();
    let r = run(&mut s, "edit.find", json!({"text": "four"}));
    assert_eq!(r["found"], false);
    assert_eq!(compose(&s).body.selection(), before);
    // Finding changes nothing written.
    assert!(!compose(&s).dirty);
}

#[test]
fn replace_takes_the_selection_when_it_is_the_text_then_finds_the_next() {
    let mut s = writing("cat and cat and Cat");
    // Nothing selected that is the text: only finds it.
    let r = run(&mut s, "edit.replace", json!({"text": "cat", "with": "dog"}));
    assert_eq!((r["replaced"].as_u64(), r["from"].as_u64()), (Some(0), Some(0)));
    assert_eq!(mine(&s), "cat and cat and Cat");
    let r = run(&mut s, "edit.replace", json!({"text": "cat", "with": "dog"}));
    assert_eq!((r["replaced"].as_u64(), r["from"].as_u64(), r["to"].as_u64()), (Some(1), Some(8), Some(11)));
    assert_eq!(mine(&s), "dog and cat and Cat");
    assert!(compose(&s).dirty);
    run(&mut s, "edit.replace", json!({"text": "cat", "with": "dog"}));
    run(&mut s, "edit.replace", json!({"text": "cat", "with": "dog"}));
    assert_eq!(mine(&s), "dog and dog and dog");
    // Each one its own step.
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(mine(&s), "dog and dog and Cat");
}

#[test]
fn replace_all_is_one_step_and_says_how_many() {
    let mut s = writing("cat and cat and Cat");
    let r = run(&mut s, "edit.replace", json!({"text": "cat", "with": "horse", "all": true, "matchCase": true}));
    assert_eq!(r["replaced"], 2);
    assert_eq!(mine(&s), "horse and horse and Cat");
    let r = run(&mut s, "edit.replace", json!({"text": "CAT", "with": "", "all": true}));
    assert_eq!(r["replaced"], 1);
    assert_eq!(mine(&s), "horse and horse and ");
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(mine(&s), "horse and horse and Cat");
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(mine(&s), "cat and cat and Cat");
    // Nothing there: nothing changes and nothing goes on the undo.
    let r = run(&mut s, "edit.replace", json!({"text": "zebra", "with": "x", "all": true}));
    assert_eq!(r["replaced"], 0);
    assert!(!s.enabled("compose.undo"));
}

/// `teh` as its own word, including next to punctuation and at either end, and the same letters
/// inside a longer word, beside an apostrophe or `_`, or beside a non-ASCII letter.
const TEH: &str = "teh Tehran tehee Teh teh, (teh) t\u{e9}h teh\u{e9} \u{e9}teh teh_x teh's teh\u{2019}s \u{2019}teh teh";

#[test]
fn replace_all_whole_word_leaves_substrings_and_undoes_in_one_step() {
    let mut s = writing(TEH);
    let r = run(&mut s, "edit.replace", json!({"text": "teh", "with": "the", "all": true, "wholeWord": true}));
    assert_eq!(r["replaced"], 5);
    // `Teh` matches when letter case does not. `Tehran`, `tehee`, `tehé`, `éteh`, `teh_x`,
    // `teh's`, `teh’s` and `’teh` stay; `téh` was never the text.
    assert_eq!(mine(&s), "the Tehran tehee the the, (the) t\u{e9}h teh\u{e9} \u{e9}teh teh_x teh's teh\u{2019}s \u{2019}teh the");
    run(&mut s, "compose.undo", json!({}));
    assert_eq!(mine(&s), TEH);
    assert!(!s.enabled("compose.undo"));
}

#[test]
fn replace_all_whole_word_with_match_case() {
    let mut s = writing("teh Teh Tehran tehee Teh,");
    let r = run(&mut s, "edit.replace", json!({"text": "teh", "with": "the", "all": true, "matchCase": true, "wholeWord": true}));
    assert_eq!(r["replaced"], 1);
    assert_eq!(mine(&s), "the Teh Tehran tehee Teh,");
    let mut s = writing("Teh Tehran tehee");
    let r = run(&mut s, "edit.replace", json!({"text": "Teh", "with": "The", "all": true, "matchCase": true, "wholeWord": true}));
    assert_eq!(r["replaced"], 1);
    assert_eq!(mine(&s), "The Tehran tehee");
}

#[test]
fn find_whole_word_counts_and_selects_only_whole_words() {
    let mut s = writing("teh Tehran tehee teh");
    let r = run(&mut s, "edit.find", json!({"text": "teh", "wholeWord": true}));
    assert_eq!((r["found"].as_bool(), r["from"].as_u64(), r["to"].as_u64(), r["count"].as_u64()), (Some(true), Some(0), Some(3), Some(2)));
    // The next whole word, skipping `Tehran` and `tehee`.
    let r = run(&mut s, "edit.find", json!({"text": "teh", "wholeWord": true}));
    assert_eq!((r["from"].as_u64(), r["to"].as_u64(), r["count"].as_u64()), (Some(17), Some(20), Some(2)));
    // Backward from there returns to the first, still counting only whole words.
    let r = run(&mut s, "edit.find", json!({"text": "teh", "wholeWord": true, "backward": true}));
    assert_eq!((r["from"].as_u64(), r["count"].as_u64()), (Some(0), Some(2)));
    let r = run(&mut s, "edit.find", json!({"text": "teh"}));
    assert_eq!(r["count"], 4);
    // `téh` is not `teh`; `tehé` holds `teh` but not as a whole word.
    let mut s = writing("t\u{e9}h teh teh\u{e9}");
    let r = run(&mut s, "edit.find", json!({"text": "teh", "wholeWord": true}));
    assert_eq!((r["from"].as_u64(), r["to"].as_u64(), r["count"].as_u64()), (Some(4), Some(7), Some(1)));
    assert_eq!(run(&mut s, "edit.find", json!({"text": "teh"}))["count"], 2);
    assert!(!compose(&s).dirty);
}

#[test]
fn whole_word_off_still_replaces_inside_a_word() {
    let mut s = writing(TEH);
    let r = run(&mut s, "edit.replace", json!({"text": "teh", "with": "the", "all": true}));
    assert_eq!(r["replaced"], 13);
    assert_eq!(mine(&s), "the theran theee the the, (the) t\u{e9}h the\u{e9} \u{e9}the the_x the's the\u{2019}s \u{2019}the the");
}

#[test]
fn what_is_looked_for_is_one_line_and_short() {
    let mut s = writing("abc");
    for params in [
        json!({}),
        json!({"text": ""}),
        json!({"text": 5}),
        json!({"text": "a\nb"}),
        json!({"text": "x".repeat(256)}),
        json!({"text": "a", "with": "b\r\n"}),
        json!({"text": "a", "with": "y".repeat(256)}),
        json!({"text": "a", "with": null}),
    ] {
        let id = if params.get("with").is_some() { "edit.replace" } else { "edit.find" };
        assert!(s.run(id, &params).is_err(), "{id} {params}");
    }
    assert_eq!(mine(&s), "abc");
    // A 255-character text is fine.
    assert_eq!(run(&mut s, "edit.find", json!({"text": "x".repeat(255)}))["found"], false);
    // No message: not enabled.
    run(&mut s, "compose.discard", json!({}));
    assert!(!s.enabled("edit.find") && !s.enabled("edit.replace"));
}

#[test]
fn a_range_past_the_owners_text_is_kept_to_it() {
    let mut s = writing("abc");
    let len = compose(&s).body.text().len();
    assert!(s.run("format.bold", &json!({"from": 0, "to": len + 1})).is_err());
    run(&mut s, "format.bold", json!({"from": 1, "to": 2}));
    assert!(compose(&s).body.text().all(1, 2, |st| st.bold).unwrap());
}
