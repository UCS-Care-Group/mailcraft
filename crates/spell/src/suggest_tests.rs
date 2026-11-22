// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

fn dict() -> &'static Dictionary {
    en_us().expect("embedded en_US dictionary")
}

fn has(word: &str, expect: &str) -> Vec<String> {
    let got = dict().suggest(word, &[]);
    assert!(got.iter().any(|item| item.eq_ignore_ascii_case(expect)), "{word} -> {got:?}, wanted {expect}");
    got
}

#[test]
fn edits_replacements_and_ngrams() {
    let teh = has("teh", "the");
    assert!(teh.iter().position(|word| word.eq_ignore_ascii_case("the")).unwrap_or(9) < 3, "{teh:?}");
    has("tomorow", "tomorrow");
    has("neccesary", "necessary");
    has("acomodate", "accommodate");
    has("alot", "a lot");
}

#[test]
fn common_typos_put_the_intended_word_first() {
    let cases = [
        ("teh", "the"),
        ("thier", "their"),
        ("wich", "which"),
        ("realy", "really"),
        ("adress", "address"),
        ("recieve", "receive"),
        ("seperate", "separate"),
        ("definately", "definitely"),
        ("occured", "occurred"),
        ("untill", "until"),
        ("wierd", "weird"),
        ("accomodate", "accommodate"),
        ("tommorow", "tomorrow"),
        ("goverment", "government"),
        ("beleive", "believe"),
        ("begining", "beginning"),
        ("calender", "calendar"),
        ("concious", "conscious"),
        ("becuase", "because"),
        ("buisness", "business"),
        ("freind", "friend"),
        ("knowlege", "knowledge"),
    ];
    for (typo, want) in cases {
        let got = dict().suggest(typo, &[]);
        assert_eq!(got.first().map(String::as_str), Some(want), "{typo} -> {got:?}");
        assert!(got.iter().all(|word| no_short_piece(word)), "{typo} -> {got:?}");
    }
}

fn no_short_piece(word: &str) -> bool {
    word.split(' ').all(|part| {
        let n = part.chars().count();
        part.is_empty() || n != 1 || part.eq_ignore_ascii_case("a") || part.eq_ignore_ascii_case("i")
    })
}

#[test]
fn title_case_keeps_a_candidate_with_internal_capitals() {
    let mcdonald = Dictionary::parse("SET UTF-8\nREP 1\nREP mcdonld McDonald\n", "1\nMcDonald\n").expect("parse");
    let got = mcdonald.suggest("Mcdonld", &[]);
    assert!(got.iter().any(|word| word == "McDonald"), "{got:?}");
    assert!(got.iter().all(|word| mcdonald.is_correct(word)), "{got:?}");

    let nasa = Dictionary::parse("SET UTF-8\nTRY esa\n", "1\nNASA\n").expect("parse");
    let got = nasa.suggest("Nass", &[]);
    assert!(got.iter().any(|word| word == "NASA"), "{got:?}");
    assert!(got.iter().all(|word| nasa.is_correct(word)), "{got:?}");

    let plain = Dictionary::parse("SET UTF-8\nTRY ht\n", "1\nthe\n").expect("parse");
    let got = plain.suggest("Teh", &[]);
    assert!(got.iter().any(|word| word == "The"), "{got:?}");
    assert!(got.iter().all(|word| plain.is_correct(word)), "{got:?}");
}

#[test]
fn sharp_s_restore_keeps_the_whole_uppercase_mapping() {
    assert_eq!(crate::fold::restore_accents("straßee", "STRASSE"), "STRASSE");
    assert_eq!(crate::fold::restore_accents("straße", "strasse"), "straße");
    let dict = Dictionary::parse("SET UTF-8\nREP 1\nREP straßee STRASSE\n", "1\nSTRASSE\n").expect("parse");
    let got = dict.suggest("straßee", &[]);
    assert!(got.iter().any(|word| word == "STRASSE"), "{got:?}");
    assert!(got.iter().all(|word| dict.is_correct(word)), "{got:?}");
}

#[test]
fn ligature_restore_consumes_its_whole_expansion() {
    assert_eq!(crate::fold::restore_accents("encyclopædai", "encyclopaedia"), "encyclopædia");
    assert_eq!(crate::fold::fold_accents("encyclopædia"), "encyclopaedia");
    assert_eq!(crate::fold::restore_accents("café", "cafe"), "café");

    let dict = Dictionary::parse("SET UTF-8\n", "1\nencyclopaedia\n").expect("parse");
    let got = dict.suggest("encyclopædai", &[]);
    assert!(got.iter().any(|word| word == "encyclopædia"), "{got:?}");
    assert!(got.iter().all(|word| dict.is_correct(word)), "{got:?}");
}

#[test]
fn suggestion_case_and_accents() {
    let upper = dict().suggest("TEH", &[]);
    assert!(upper.iter().any(|word| word == "THE"), "{upper:?}");
    let title = dict().suggest("Teh", &[]);
    assert!(title.iter().any(|word| word == "The"), "{title:?}");
    let accented = dict().suggest("caféé", &[]);
    assert!(accented.iter().any(|word| word.contains('é') || word.contains('É')), "{accented:?}");
    assert!(!accented.iter().any(|word| word == "caféé"));
}

#[test]
fn nosuggest_words_are_never_offered() {
    let dict = dict();
    assert!(dict.is_correct("shitload"));
    for typo in ["shitlod", "shitlaod", "shitloadd", "shitloads"] {
        let got = dict.suggest(typo, &[]);
        assert!(got.iter().all(|word| !word.eq_ignore_ascii_case("shitload")), "{typo} -> {got:?}");
    }
}

#[test]
fn an_extra_word_keeps_its_stored_spelling() {
    let dict = Dictionary::parse("SET UTF-8\nTRY t\n", "0\n").expect("parse");
    let got = dict.suggest("mailcraf", &["MailCraft"]);
    assert!(got.iter().any(|word| word == "MailCraft"), "{got:?}");
    assert!(got.iter().all(|word| word != "mailcraft"), "{got:?}");
}

#[test]
fn custom_words_are_candidates() {
    let got = dict().suggest("mailcraf", &["MailCraft"]);
    assert!(got.iter().any(|word| word.eq_ignore_ascii_case("MailCraft")), "{got:?}");
}

#[test]
fn custom_word_past_256_is_still_a_candidate() {
    let mut owned = Vec::new();
    for n in 0..256 {
        owned.push(format!("zzz{n}word"));
    }
    owned.push("MailCraft".to_string());
    let extra: Vec<&str> = owned.iter().map(String::as_str).collect();
    let got = dict().suggest("mailcraf", &extra);
    assert!(got.iter().any(|word| word.eq_ignore_ascii_case("MailCraft")), "{got:?}");
}

#[test]
fn release_timings() {
    let started = std::time::Instant::now();
    let built =
        Dictionary::parse(include_str!("../../../assets/dictionaries/en_US/en_US.aff"), include_str!("../../../assets/dictionaries/en_US/en_US.dic"))
            .expect("parse");
    let build_ms = started.elapsed().as_secs_f64() * 1000.0;
    let bytes = built.estimate_bytes();
    let parts = built.memory_parts();
    let _ = built.suggest("neccesary", &[]);
    let words = ["neccesary", "acomodate", "tomorow", "teh", "recieve", "seperate"];
    let rounds = 5u32;
    let started = std::time::Instant::now();
    for _ in 0..rounds {
        for word in words {
            let _ = built.suggest(word, &[]);
        }
    }
    let suggest_ms = started.elapsed().as_secs_f64() * 1000.0 / f64::from(rounds * words.len() as u32);
    eprintln!(
        "en_US build {build_ms:.1} ms, suggest {suggest_ms:.2} ms, estimate {bytes} bytes, stems {} (stems {} map {} grams {} other {})",
        built.stem_count(),
        parts[0],
        parts[1],
        parts[2],
        parts[3]
    );
    if !cfg!(debug_assertions) {
        assert!(suggest_ms < 30.0, "suggest {suggest_ms:.2} ms");
        assert!(build_ms < 1000.0, "build {build_ms:.1} ms");
    }
}

#[test]
fn ordinary_prose() {
    let text = "The committee will meet tomorrow to discuss the necessary changes. Please bring your notes, and do not forget the well-known report about the e-mail from Paris.";
    let dict = dict();
    let ranges = dict.check_text(text, 32);
    for range in &ranges {
        eprintln!("flagged: {}", text.get(range.clone()).unwrap_or(""));
    }
    assert!(ranges.is_empty(), "ordinary prose was flagged");
}
