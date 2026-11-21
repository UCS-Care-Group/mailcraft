// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

const MINI_AFF: &str = "\
SET UTF-8
TRY abc
NOSUGGEST !
COMPOUNDMIN 1
ONLYINCOMPOUND c
COMPOUNDRULE 2
COMPOUNDRULE n*1t
COMPOUNDRULE n*mp
WORDCHARS 0123456789'
PFX A Y 1
PFX A 0 re .
SFX D Y 1
SFX D 0 ed [^ey]
SFX S Y 1
SFX S 0 s [^sxzhy]
REP 1
REP alot a_lot
";

const MINI_DIC: &str = "\
11
a
cat/S
lot
run/A
jump/D
0/nm
1/n1
1st/p
1th/tc
2/nm
2nd/p
4th/pt
bad/!
";

fn mini() -> Dictionary {
    Dictionary::parse(MINI_AFF, MINI_DIC).expect("mini dictionary")
}

#[test]
fn set_other_than_utf8_is_an_error() {
    let parsed = Dictionary::parse("SET ISO8859-1\n", "1\nhello\n");
    let Err(err) = parsed else { panic!("encoding should be refused") };
    assert_eq!(err, Error::UnsupportedEncoding);
    assert!(!err.to_string().is_empty());
}

#[test]
fn nosuggest_hyphen_is_not_offered_through_its_parts() {
    let dict = Dictionary::parse("SET UTF-8\nNOSUGGEST !\nREP 1\nREP cat-dgo cat-dog\n", "3\ncat-dog/!\ncat\ndog\n").expect("parse");
    assert!(dict.is_correct("cat-dog"));
    assert!(!dict.suggestable("cat-dog"));
    let got = dict.suggest("cat-dgo", &[]);
    assert!(got.iter().all(|word| !word.eq_ignore_ascii_case("cat-dog")), "{got:?}");

    let open = Dictionary::parse("SET UTF-8\n", "2\ncat\ndog\n").expect("parse");
    assert!(open.is_correct("cat-dog"));
    assert!(open.suggestable("cat-dog"));
}

#[test]
fn accented_stem_matches_its_own_spelling() {
    let accented = Dictionary::parse("SET UTF-8\n", "1\ncafé\n").expect("parse");
    assert!(accented.is_correct("café"));
    assert!(!accented.is_correct("cafe"));
    let ascii = Dictionary::parse("SET UTF-8\n", "1\ncafe\n").expect("parse");
    assert!(ascii.is_correct("cafe"));
    assert!(ascii.is_correct("café"));
    assert!(ascii.is_correct("Café"));
}

#[test]
fn malformed_lines_are_skipped() {
    let dict = Dictionary::parse("SET UTF-8\nNOT A REAL DIRECTIVE\nSFX\nPFX A Y\n", "\n\nhello\n/\n").expect("parse");
    assert!(dict.is_correct("hello"));
    assert!(!dict.is_correct("helloo"));
}

#[test]
fn mini_affixes_compounds_and_replacement_data() {
    let dict = mini();
    assert!(dict.is_correct("cat"));
    assert!(dict.is_correct("cats"));
    assert!(dict.is_correct("Cats"));
    assert!(dict.is_correct("CATS"));
    assert!(dict.is_correct("rerun"));
    assert!(dict.is_correct("jumped"));
    assert!(dict.is_correct("1st"));
    assert!(dict.is_correct("11th"));
    assert!(dict.is_correct("21st"));
    assert!(dict.is_correct("14th"));
    assert!(!dict.is_correct("1th"));
    assert!(!dict.is_correct("11st"));
    assert!(dict.is_correct("bad"));
    let suggestions = dict.suggest("alot", &[]);
    assert!(suggestions.iter().any(|word| word == "a lot"), "{suggestions:?}");
    assert!(!dict.suggest("baad", &[]).iter().any(|word| word.eq_ignore_ascii_case("bad")));
}
