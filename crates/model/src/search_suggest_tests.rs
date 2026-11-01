// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #308: what the search box offers for the word being typed.

use crate::search::{Query, Suggestion, keyword_spans, suggestions};
use crate::{Category, Message, Rgb};

fn categories() -> Vec<Category> {
    ["Red category", "Green category", "Blue category", "Project Atlas", "Grüße"]
        .iter()
        .map(|n| Category { name: n.to_string(), color: Rgb(1, 2, 3) })
        .collect()
}

/// What is offered with the cursor at the end of `q`.
fn offered(q: &str) -> Vec<String> {
    offered_at(q, q.len())
}

fn offered_at(q: &str, cursor: usize) -> Vec<String> {
    suggestions(q, cursor, &categories()).into_iter().map(|s| s.label).collect()
}

/// `q` with `s` taken.
fn taken(q: &str, s: &Suggestion) -> String {
    format!("{}{}{}", &q[..s.range.start], s.text, &q[s.range.end..])
}

fn first(q: &str) -> Suggestion {
    suggestions(q, q.len(), &categories()).into_iter().next().unwrap_or_else(|| panic!("nothing offered for {q:?}"))
}

/// The start of a keyword offers the keyword and its colon, a `-` before it kept; each keyword
/// once, by its first name.
#[test]
fn the_start_of_a_keyword_offers_it() {
    assert_eq!(offered("cat"), ["category:"]);
    assert_eq!(offered("-cat"), ["-category:"]);
    assert_eq!(offered("CAT"), ["category:"]);
    assert_eq!(offered("c"), ["cc:", "category:"]);
    assert_eq!(offered("ha"), ["has:"]);
    assert_eq!(offered("to"), ["to:"]);
    assert_eq!(offered("rec"), ["received:"]);
    assert_eq!(first("budget cat").text, "category:");
    assert_eq!(first("budget cat").range, 7..10);
    // Words that start no keyword, a word in quotes, and text with a colon that is no keyword.
    for q in ["budget", "", "-", "\"fro", "re:", "x cat ", "categoryx", "Đ", "10:30"] {
        assert!(offered(q).is_empty(), "{q}");
    }
}

/// The word being typed is the one the cursor is at the end of, wherever it stands in the query;
/// inside a word, nothing is offered.
#[test]
fn the_word_being_typed_is_where_the_cursor_is() {
    let q = "budget cat from:ada";
    assert_eq!(offered_at(q, 10), ["category:"]);
    assert!(offered_at(q, 8).is_empty(), "inside the word");
    assert!(offered_at(q, 0).is_empty() && offered_at(q, 6).is_empty());
    let s = suggestions(q, 10, &categories()).remove(0);
    assert_eq!(taken(q, &s), "budget category: from:ada");
}

/// After `category:`, the mailbox's categories: those whose name starts with what was typed
/// first, then those that hold it, ignoring case and accents; a name with a space in quotes.
#[test]
fn after_category_the_mailboxs_categories() {
    assert_eq!(offered("category:"), ["Red category", "Green category", "Blue category", "Project Atlas", "Grüße"]);
    assert_eq!(offered("category:gr"), ["Green category", "Grüße"]);
    assert_eq!(offered("category:GRUSS"), ["Grüße"]);
    assert_eq!(offered("category:cat"), ["Red category", "Green category", "Blue category"]);
    assert_eq!(offered("category:atlas"), ["Project Atlas"]);
    assert!(offered("category:nothing").is_empty());
    // What was typed is matched word by word, not across the space between two words.
    assert!(offered("category:ctat").is_empty());
    assert_eq!(offered("category:\"project at"), ["Project Atlas"]);
    let s = first("category:\"green c");
    assert_eq!((s.text.as_str(), s.color), ("category:\"Green category\" ", Some(Rgb(1, 2, 3))));
    // As it was typed: a `-`, the keyword's capital, its other name, a space after its colon.
    assert_eq!(first("-category:bl").text, "-category:\"Blue category\" ");
    assert_eq!(first("Category:bl").text, "Category:\"Blue category\" ");
    assert_eq!(first("categories:bl").text, "categories:\"Blue category\" ");
    let q = "budget category: gre";
    let s = first(q);
    assert_eq!((s.range.clone(), taken(q, &s)), (7..q.len(), "budget category:\"Green category\" ".to_string()));
    assert_eq!(offered("category: ").len(), 5);
    // A name with no space needs no quotes.
    assert_eq!(first("category:grü").text, "category:Grüße ");
}

/// One letter that starts a name offers only the names it starts, not every one that holds it
/// (`category:g` and the "… category" names); the others come when none starts with it, or from
/// two letters on.
#[test]
fn names_that_only_hold_one_letter_wait_for_a_second() {
    let cats: Vec<Category> = ["Green", "Red category", "Agenda"].iter().map(|n| Category { name: n.to_string(), color: Rgb(1, 2, 3) }).collect();
    let offered = |q: &str| suggestions(q, q.len(), &cats).into_iter().map(|s| s.label).collect::<Vec<_>>();
    assert_eq!(offered("category:g"), ["Green"]);
    assert_eq!(offered("category:a"), ["Agenda"]);
    assert_eq!(offered("category:d"), ["Red category", "Agenda"], "none starts with it");
    assert_eq!(offered("category:re"), ["Red category", "Green"], "two letters");
}

/// A value with no letter or digit in it (`&`) is matched as it was typed, as the search matches
/// it: only the categories that hold it are offered, not every one.
#[test]
fn a_value_of_marks_offers_the_categories_that_hold_it() {
    let cats: Vec<Category> = ["R&D", "Red category", "Q&A"].iter().map(|n| Category { name: n.to_string(), color: Rgb(1, 2, 3) }).collect();
    let offered = |q: &str| suggestions(q, q.len(), &cats).into_iter().map(|s| s.label).collect::<Vec<_>>();
    assert_eq!(offered("category:&"), ["R&D", "Q&A"]);
    assert_eq!(offered("category:\"&"), ["R&D", "Q&A"], "in quotes too");
    assert!(offered("category:#").is_empty());
    assert_eq!(offered("category:").len(), 3, "nothing typed: all of them");
    for name in offered("category:&") {
        let m = Message { categories: vec![name.clone()], ..Default::default() };
        assert!(Query::parse("category:&").matches(&m, 0), "{name} is found by what was typed");
    }
}

/// Taken, a category's suggestion finds the mail of that category, and the search box reads it
/// as a keyword.
#[test]
fn a_category_taken_finds_its_mail() {
    let q = taken("category:green", &first("category:green"));
    let green = Message { categories: vec!["Green category".into()], ..Default::default() };
    let red = Message { categories: vec!["Red category".into()], ..Default::default() };
    let query = Query::parse(&q);
    assert!(query.matches(&green, 0) && !query.matches(&red, 0));
    assert_eq!(keyword_spans(&q).len(), 1);
}

/// In the middle of a query the word is replaced and the space after it kept, not doubled.
#[test]
fn in_the_middle_of_a_query_the_rest_stays() {
    let q = "category:gr is:unread";
    let s = suggestions(q, 11, &categories()).remove(0);
    assert_eq!(taken(q, &s), "category:\"Green category\" is:unread");
}

/// After `is:` and `has:`, what they take; nothing after the other keywords (people need someone
/// to ask: `search_people_tests`).
#[test]
fn after_is_and_has_what_they_take() {
    assert_eq!(offered("is:"), ["unread", "read", "flagged", "important"]);
    assert_eq!(offered("is:r"), ["read"]);
    assert_eq!(offered("-IS:Un"), ["unread"]);
    assert_eq!(first("is: fl").text, "is:flagged ");
    assert_eq!(offered("has:"), ["attachment"]);
    for q in ["from:", "from: ad", "to:", "subject: fr", "before:", "importance:", "body:x", "hasattachment:"] {
        assert!(offered(q).is_empty(), "{q}");
    }
}

/// What would leave the query as it is isn't offered: a value typed whole.
#[test]
fn what_is_typed_whole_is_not_offered_again() {
    for q in ["is:unread", "is:read", "has:attachment", "category:\"Green category\"", "category:Grüße"] {
        assert!(offered(q).is_empty(), "{q}");
    }
    // Typed whole but in other letters, it is offered as the category is written.
    assert_eq!(first("category:grüße").text, "category:Grüße ");
}

/// A saved mailbox's categories are hostile: empty and odd names are left out, a name is
/// offered once, and the list is short.
#[test]
fn a_mailboxs_odd_categories_are_left_out() {
    let named = |n: &str| Category { name: n.to_string(), color: Rgb(0, 0, 0) };
    let mut odd = vec![named(""), named("   "), named("line\nbreak"), named(&"x".repeat(300)), named("Blue"), named("Blue")];
    odd.extend((0..5_000).map(|i| named(&format!("Tag {i}"))));
    let shown: Vec<String> = suggestions("category:", 9, &odd).into_iter().map(|s| s.label).collect();
    assert_eq!(shown.len(), 8);
    assert_eq!(shown.first().map(String::as_str), Some("Blue"));
    assert_eq!(shown.iter().filter(|n| *n == "Blue").count(), 1);
    // Only so many of a mailbox's categories are looked through.
    let mut many: Vec<Category> = (0..1_000).map(|i| named(&format!("Tag {i}"))).collect();
    many.push(named("Zebra"));
    assert!(suggestions("category:zeb", 12, &many).is_empty());
    assert_eq!(suggestions("category:zeb", 12, &many[900..]).len(), 1);
    // Quotes and backslashes in a name are written so the search reads the name back.
    let q = taken("category:a", &suggestions("category:a", 10, &[named("a \"b\" \\c")]).remove(0));
    assert_eq!(q, "category:\"a \\\"b\\\" \\\\c\" ");
    let m = Message { categories: vec!["a \"b\" \\c".into()], ..Default::default() };
    assert!(Query::parse(&q).matches(&m, 0));
}

/// Hostile cursors: past the end, inside a character; never a panic.
#[test]
fn any_cursor_is_safe() {
    for q in ["Đcat", "category:Grü", "\u{301}is:", "-", ""] {
        for c in [0, 1, 2, q.len(), usize::MAX] {
            for s in suggestions(q, c, &categories()) {
                assert!(q.get(s.range.clone()).is_some(), "{q} {c}");
            }
        }
    }
    assert_eq!(offered_at("cat", usize::MAX), ["category:"]);
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

    /// Whatever the query, the cursor and the categories: a short list, each suggestion standing
    /// on character boundaries of the query, and taking it leaves text the search reads.
    #[test]
    fn whatever_is_typed_the_suggestions_stand_in_it(
        q in "[a-z:\"\\\\ -]{0,12}|.{0,24}",
        cursor in proptest::prelude::any::<usize>(),
        near in 0usize..32,
        names in proptest::collection::vec(".{0,12}", 0..12),
    ) {
        let cats: Vec<Category> = names.iter().map(|n| Category { name: n.clone(), color: Rgb(0, 0, 0) }).collect();
        for c in [cursor, near] {
            let all = suggestions(&q, c, &cats);
            proptest::prop_assert!(all.len() <= 8);
            let mut at = c.min(q.len());
            while !q.is_char_boundary(at) {
                at -= 1;
            }
            for s in all {
                proptest::prop_assert!(q.get(s.range.clone()).is_some());
                proptest::prop_assert!(s.range.start <= s.range.end && s.range.end == at);
                let _ = Query::parse(&taken(&q, &s));
                // A category, taken, finds what has it (and with a `-` before it, leaves it out).
                if s.color.is_some() {
                    let m = Message { categories: vec![s.label.clone()], ..Default::default() };
                    proptest::prop_assert_eq!(Query::parse(&s.text).matches(&m, 0), !s.text.starts_with('-'), "{:?}", s.text);
                }
            }
        }
    }
}

/// Each keyword the search reads is offered from its start, except the other names of one that
/// is. Each value offered after `is:` and `has:` is read as that keyword's, not as text.
#[test]
fn what_is_offered_is_what_the_search_reads() {
    const OTHER_NAMES: [&str; 5] = ["categories", "attachments", "hasattachment", "hasattachments", "sent"];
    for k in crate::search::KEYWORDS.iter().filter(|k| !OTHER_NAMES.contains(k)) {
        assert!(offered(k).contains(&format!("{k}:")), "{k}: is not offered");
    }
    for keyword in ["is:", "has:"] {
        let values = offered(keyword);
        assert!(!values.is_empty(), "{keyword}");
        for v in values {
            let q = format!("{keyword}{v}");
            assert_ne!(Query::parse(&q), Query::parse(&format!("\"{q}\"")), "{q} is read as text");
        }
    }
}
