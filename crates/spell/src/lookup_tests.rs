// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

fn dict() -> &'static Dictionary {
    en_us().expect("embedded en_US dictionary")
}

#[test]
fn embedded_list_loads() {
    let dict = dict();
    assert!(dict.stem_count() > 40_000, "{}", dict.stem_count());
    assert_eq!(EN_US_VERSION, "2026.02.25");
    assert_eq!(EN_US_TAG, "en-US-x-builtin");
}

#[test]
fn inflections_possessives_ordinals_and_hyphens() {
    let dict = dict();
    for word in [
        "running",
        "boxes",
        "happier",
        "unbelievable",
        "rerun",
        "disconnected",
        "reconnected",
        "creating",
        "don't",
        "Bob's",
        "users",
        "well-known",
        "e-mail",
        "1st",
        "22nd",
        "111th",
        "1234th",
        "123rd",
    ] {
        assert!(dict.is_correct(word), "{word}");
    }
    assert!(dict.is_correct("don’t"), "curly apostrophe");
    assert!(!dict.is_correct("1th"));
    assert!(!dict.is_correct("2th"));
    assert!(!dict.is_correct("11st"));
}

#[test]
fn casing_and_accents() {
    let dict = dict();
    assert!(dict.is_correct("Paris"));
    assert!(dict.is_correct("PARIS"));
    assert!(!dict.is_correct("paris"));
    assert!(dict.is_correct("hello"));
    assert!(dict.is_correct("Hello"));
    assert!(dict.is_correct("HELLO"));
    assert!(dict.is_correct("McDonald"));
    assert!(dict.is_correct("MCDONALD"));
    assert!(!dict.is_correct("McDONALD"));
    assert!(dict.is_correct("iPhone"));
    assert!(dict.is_correct("IPHONE"));
    assert!(!dict.is_correct("iphone"));
    assert!(!dict.is_correct("Iphone"));
    for word in ["café", "naïve", "résumé", "Éclair"] {
        assert!(dict.is_correct(word), "{word}");
    }
}

#[test]
fn builtin_host_speaks_only_its_tag() {
    let host = BuiltIn;
    assert_eq!(host.locales(), vec![EN_US_TAG.to_owned()]);
    assert!(host.check("en-US", "teh").is_none());
    assert!(host.suggest("en-US", "teh").is_empty());
    let ranges = host.check(EN_US_TAG, "I saw teh cat").expect("available");
    assert_eq!(ranges, vec![6..9]);
    assert!(!host.suggest(EN_US_TAG, "teh").is_empty());
}
