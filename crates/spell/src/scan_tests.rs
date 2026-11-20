// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

fn dict() -> &'static Dictionary {
    en_us().expect("embedded en_US dictionary")
}

#[test]
fn one_misspelling_in_a_sentence() {
    let ranges = dict().check_text("I saw teh cat", 8);
    assert_eq!(ranges, vec![6..9]);
    assert_eq!(&"I saw teh cat"[6..9], "teh");
}

#[test]
fn edges_digits_and_other_scripts_are_skipped() {
    let dict = dict();
    assert!(dict.check_text("'color' is fine, and 2026 and 4th.", 8).is_empty());
    assert!(dict.check_text("Привет café naïve", 8).is_empty());
    let mixed = dict.check_text("heлlo there", 8);
    assert!(mixed.is_empty(), "a token with a non-Latin letter is not a misspelling: {mixed:?}");
    let flagged = dict.check_text("users' 1th", 8);
    assert_eq!(flagged.len(), 1, "{flagged:?}");
    let word = "users' 1th".get(flagged[0].clone()).unwrap_or("");
    assert_eq!(word, "1th");
}

#[test]
fn ranges_respect_the_limit_and_character_boundaries() {
    let text = "teh xyzzy café qqq";
    let ranges = dict().check_text(text, 1);
    assert_eq!(ranges.len(), 1);
    for range in dict().check_text(text, 8) {
        assert!(range.start <= range.end && range.end <= text.len());
        assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
    }
}
