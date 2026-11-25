// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The spelling filter (#1797): addresses and links stay out, and the owner's words stay out.

use std::collections::BTreeSet;
use std::ops::Range;

use mailcraft_model::rich::{ListKind, OBJECT, Paragraph, Run};
use mailcraft_spell::HostChecker;

use crate::spelling::{Filter, MAX_CUSTOM_WORDS, MAX_PARAGRAPH_BYTES, MAX_WORD_CHARS, check_paragraph, check_plain, clean_custom_word, word_matches};

/// The stand-in from the composer's tests: Italian, with ranges that are not words.
struct Italian;

impl HostChecker for Italian {
    fn locales(&self) -> Vec<String> {
        vec!["it-IT".to_string()]
    }

    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>> {
        if locale != "it-IT" {
            return None;
        }
        let mut ranges = vec![1..2, 1000..1001];
        for (at, word) in text.match_indices("sopratutto") {
            ranges.push(at..at + word.len());
        }
        Some(ranges)
    }

    fn suggest(&self, _locale: &str, _word: &str) -> Vec<String> {
        Vec::new()
    }
}

/// Flags each of these words wherever it stands, and only the locale `en`.
struct Words<'a>(&'a [&'a str]);

impl HostChecker for Words<'_> {
    fn locales(&self) -> Vec<String> {
        vec!["en".to_string()]
    }

    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>> {
        if locale != "en" {
            return None;
        }
        let mut ranges = Vec::new();
        for word in self.0 {
            let mut from = 0usize;
            while let Some(at) = text.get(from..).and_then(|rest| rest.find(word)) {
                let start = from.saturating_add(at);
                let end = start.saturating_add(word.len());
                ranges.push(start..end);
                from = end;
            }
        }
        Some(ranges)
    }

    fn suggest(&self, _locale: &str, _word: &str) -> Vec<String> {
        Vec::new()
    }
}

fn words_of(report: &crate::spelling::Report) -> Vec<&str> {
    report.issues.iter().map(|issue| issue.word.as_str()).collect()
}

#[test]
fn spelling_offsets_exclude_links_addresses_and_signatures() {
    let mut link = Run::plain("sopratutto");
    link.style.link = Some("https://example.org".into());
    let paragraph = Paragraph::new(vec![Run::plain("È 📧 sopratutto sopratutto@example.org "), link], ListKind::None);
    let report = check_paragraph(&Italian, "it-IT", &paragraph, &Filter::default());
    assert!(report.available);
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].span, 4..14);
    assert!(!check_paragraph(&Italian, "fr-FR", &paragraph, &Filter::default()).available);
    let signed = Paragraph { signature: true, ..paragraph };
    assert!(check_paragraph(&Italian, "it-IT", &signed, &Filter::default()).issues.is_empty());
}

#[test]
fn a_paragraph_over_the_cap_is_partial_and_unchecked() {
    let oversized = Paragraph::new(vec![Run::plain("a".repeat(MAX_PARAGRAPH_BYTES + 1))], ListKind::None);
    let report = check_paragraph(&Italian, "it-IT", &oversized, &Filter::default());
    assert!(report.partial);
    assert!(report.available);
    assert!(report.issues.is_empty());
}

#[test]
fn an_address_split_between_plain_and_linked_text_is_not_marked() {
    let mut suffix = Run::plain("@example.org");
    suffix.style.link = Some("mailto:sopratutto@example.org".into());
    let mut prefix = Run::plain("delivery@");
    prefix.style.link = Some("mailto:delivery@sopratutto.example".into());
    let paragraph = Paragraph::new(vec![Run::plain("sopratutto"), suffix, Run::plain(" "), prefix, Run::plain("sopratutto.example")], ListKind::None);
    assert!(check_paragraph(&Italian, "it-IT", &paragraph, &Filter::default()).issues.is_empty());
}

#[test]
fn typo_beside_a_link_is_still_marked() {
    let mut link = Run::plain("https://example.org");
    link.style.link = Some("https://example.org".into());
    let paragraph = Paragraph::new(vec![Run::plain("sopratutto—"), link], ListKind::None);
    let report = check_paragraph(&Italian, "it-IT", &paragraph, &Filter::default());
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].span, 0..10);
}

#[test]
fn a_url_path_with_a_dash_is_protected_across_formatting_runs() {
    for address in ["https://example.org/a—", "WWW.example.org/a—"] {
        let mut prefix = Run::plain(address);
        prefix.style.link = Some("https://example.org/a".into());
        let paragraph = Paragraph::new(vec![prefix, Run::plain("sopratutto")], ListKind::None);
        assert!(check_paragraph(&Italian, "it-IT", &paragraph, &Filter::default()).issues.is_empty(), "{address}");
    }
}

#[test]
fn a_typo_before_a_parenthesized_address_is_still_marked() {
    let mut link = Run::plain("info@example.org");
    link.style.link = Some("mailto:info@example.org".into());
    let paragraph = Paragraph::new(vec![Run::plain("sopratutto("), link, Run::plain(")")], ListKind::None);
    let report = check_paragraph(&Italian, "it-IT", &paragraph, &Filter::default());
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].span, 0..10);
}

#[test]
fn balanced_parentheses_in_a_url_path_are_protected() {
    for address in ["https://example.org/a(", "www.example.org/a("] {
        let mut prefix = Run::plain(address);
        prefix.style.link = Some("https://example.org/a".into());
        let paragraph = Paragraph::new(vec![prefix, Run::plain("sopratutto)")], ListKind::None);
        assert!(check_paragraph(&Italian, "it-IT", &paragraph, &Filter::default()).issues.is_empty(), "{address}");
    }
    let mut link = Run::plain("https://example.org");
    link.style.link = Some("https://example.org".into());
    let paragraph = Paragraph::new(vec![Run::plain("sopratutto("), link, Run::plain(")sopratutto")], ListKind::None);
    let spans: Vec<_> = check_paragraph(&Italian, "it-IT", &paragraph, &Filter::default()).issues.into_iter().map(|issue| issue.span).collect();
    assert_eq!(spans, [0..10, 31..41]);
}

#[test]
fn a_typo_before_a_comma_separated_address_is_still_marked() {
    let mut link = Run::plain("info@example.org");
    link.style.link = Some("mailto:info@example.org".into());
    let paragraph = Paragraph::new(vec![Run::plain("sopratutto,"), link], ListKind::None);
    let report = check_paragraph(&Italian, "it-IT", &paragraph, &Filter::default());
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].span, 0..10);
}

#[test]
fn a_picture_run_is_masked_and_an_object_character_is_not_a_word() {
    let mut hidden = Run::plain("sopratutto");
    hidden.picture = Some(1);
    let paragraph = Paragraph::new(vec![hidden, Run::plain(" sopratutto")], ListKind::None);
    let report = check_paragraph(&Italian, "it-IT", &paragraph, &Filter::default());
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].span, 11..21);

    struct Everything;
    impl HostChecker for Everything {
        fn locales(&self) -> Vec<String> {
            Vec::new()
        }
        fn check(&self, _locale: &str, text: &str) -> Option<Vec<Range<usize>>> {
            // One range covering the text, so the object character sits inside the word.
            #[allow(clippy::single_range_in_vec_init)]
            let ranges = vec![0..text.len()];
            Some(ranges)
        }
        fn suggest(&self, _locale: &str, _word: &str) -> Vec<String> {
            Vec::new()
        }
    }
    let marked = Paragraph::new(vec![Run::plain(format!("ab{OBJECT}cd"))], ListKind::None);
    assert!(check_paragraph(&Everything, "en", &marked, &Filter::default()).issues.is_empty());
}

#[test]
fn plain_text_treats_newlines_as_spaces_and_skips_addresses() {
    let host = Words(&["xyzzy", "sopratutto"]);
    let broken = check_plain(&host, "en", "Hello\nxyzzy\r\nthere", &Filter::default());
    assert_eq!(broken.issues.len(), 1);
    assert_eq!(broken.issues[0].span, 6..11);
    let address = check_plain(&host, "en", "sopratutto sopratutto@example.org", &Filter::default());
    assert_eq!(words_of(&address), ["sopratutto"]);
    assert_eq!(address.issues[0].span, 0..10);
    let url = check_plain(&host, "en", "see https://example.org/sopratutto now", &Filter::default());
    assert!(url.issues.is_empty());
    let huge = "a".repeat(MAX_PARAGRAPH_BYTES + 1);
    let partial = check_plain(&host, "en", &huge, &Filter::default());
    assert!(partial.partial && partial.available && partial.issues.is_empty());
    assert!(!check_plain(&host, "nope", "xyzzy", &Filter::default()).available);
    assert!(check_plain(&host, "en", "", &Filter::default()).issues.is_empty());
}

#[test]
fn filters_drop_custom_words_ignored_words_uppercase_and_numbers() {
    let host = Words(&["sopratutto", "ASAP", "PDFs", "NASA", "3pm", "MP3", "B2B", "hello"]);
    let text = "sopratutto ASAP PDFs NASA 3pm MP3 B2B hello";
    let all = check_plain(&host, "en", text, &Filter::default());
    assert_eq!(words_of(&all), ["sopratutto", "ASAP", "PDFs", "NASA", "3pm", "MP3", "B2B", "hello"]);

    let custom = vec!["sopratutto".to_string()];
    let ignored = BTreeSet::from(["hello".to_string()]);
    let filter = Filter { custom: &custom, ignored: &ignored, ignore_uppercase: true, ignore_numbers: true };
    assert_eq!(words_of(&check_plain(&host, "en", text, &filter)), ["PDFs"]);
}

#[test]
fn a_lowercase_entry_matches_any_casing_and_a_capital_matches_exactly_or_all_caps() {
    assert!(word_matches("nasa", "NASA"));
    assert!(word_matches("nasa", "Nasa"));
    assert!(word_matches("Nasa", "Nasa"));
    assert!(word_matches("Nasa", "NASA"));
    assert!(!word_matches("Nasa", "nasa"));
    assert!(!word_matches("Nasa", "nAsA"));
    assert!(word_matches("don't", "don’t"));
    assert!(word_matches("Don't", "DON’T"));
    assert!(!word_matches("Don't", "don't"));
    assert!(!word_matches("", "nasa"));
    assert_eq!((MAX_CUSTOM_WORDS, MAX_WORD_CHARS), (10_000, 64));
}

#[test]
fn clean_custom_word_keeps_a_short_word_and_refuses_the_rest() {
    assert_eq!(clean_custom_word("  well-known  ").as_deref(), Some("well-known"));
    assert_eq!(clean_custom_word("it's").as_deref(), Some("it's"));
    assert_eq!(clean_custom_word("it’s").as_deref(), Some("it’s"));
    let longest = "é".repeat(64);
    assert_eq!(clean_custom_word(&longest).as_deref(), Some(longest.as_str()));
    assert!(clean_custom_word(&"é".repeat(65)).is_none());
    assert!(clean_custom_word("").is_none());
    assert!(clean_custom_word("   ").is_none());
    assert!(clean_custom_word("hello world").is_none());
    assert!(clean_custom_word("foo@bar").is_none());
    assert!(clean_custom_word("a\nb").is_none());
    assert!(clean_custom_word("a\u{0}b").is_none());
    assert!(clean_custom_word("ok!").is_none());
    assert!(clean_custom_word("café").is_some());
}

#[test]
fn the_built_in_dictionary_marks_a_misspelling_and_refuses_another_locale() {
    let paragraph = Paragraph::new(vec![Run::plain("I saw teh cat")], ListKind::None);
    let report = check_paragraph(&mailcraft_spell::BuiltIn, mailcraft_spell::EN_US_TAG, &paragraph, &Filter::default());
    assert!(report.available);
    assert!(!report.partial);
    assert_eq!(words_of(&report), ["teh"]);
    assert_eq!(report.issues[0].span, 6..9);
    let subject = check_plain(&mailcraft_spell::BuiltIn, mailcraft_spell::EN_US_TAG, "Lunch tomorow", &Filter::default());
    assert_eq!(words_of(&subject), ["tomorow"]);
    assert_eq!(subject.issues[0].span, 6..13);
    assert!(!check_plain(&mailcraft_spell::BuiltIn, "fr-FR", "teh", &Filter::default()).available);
}
