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
