// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::scan::MAX_SCAN;
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
fn curly_apostrophes_at_the_edges_are_not_part_of_the_word() {
    let dict = dict();
    assert!(dict.check_text("‘color’ is fine", 8).is_empty());
    assert!(dict.check_text("’color’", 8).is_empty());
    assert!(dict.check_text("don’t stop", 8).is_empty());
    let text = "‘teh’";
    let ranges = dict.check_text(text, 8);
    assert_eq!(ranges.len(), 1, "{ranges:?}");
    assert_eq!(text.get(ranges[0].clone()).unwrap_or(""), "teh");
}

#[test]
fn a_token_cut_by_the_scan_cap_is_not_reported() {
    let dict = dict();
    let clipped = " ".repeat(MAX_SCAN.saturating_sub(2)) + "zzq rest";
    assert!(dict.check_text(&clipped, 8).is_empty(), "the cap must not check a cut-off fragment");

    let complete = " ".repeat(MAX_SCAN.saturating_sub(3)) + "teh leftover";
    let ranges = dict.check_text(&complete, 8);
    assert_eq!(ranges.len(), 1, "{ranges:?}");
    assert_eq!(complete.get(ranges[0].clone()).unwrap_or(""), "teh");

    let early = format!("teh {}", " ".repeat(MAX_SCAN));
    let ranges = dict.check_text(&early, 8);
    assert_eq!(ranges.len(), 1, "{ranges:?}");
    assert_eq!(early.get(ranges[0].clone()).unwrap_or(""), "teh");
}

#[test]
fn combining_marks_stay_on_the_word() {
    let dict = dict();
    assert!(dict.check_text("co\u{0308}operate", 8).is_empty(), "decomposed coöperate");
    assert!(dict.check_text("cafe\u{0301}", 8).is_empty(), "decomposed café");
    let text = "co\u{0308}peratex";
    let ranges = dict.check_text(text, 8);
    assert_eq!(ranges.len(), 1, "{ranges:?}");
    assert_eq!(text.get(ranges[0].clone()).unwrap_or(""), text);
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
