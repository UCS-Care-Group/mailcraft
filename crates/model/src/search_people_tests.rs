// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #308, part 2: the people the search box offers after `from:`, `to:` and `cc:`.

use std::cell::RefCell;

use crate::search::{Query, Suggestion, suggestions_with};
use crate::{Address, Message};

fn people() -> Vec<Address> {
    vec![Address::new("Ada Lovelace", "ada@example.org"), Address::new("Adam Smith", "adam@example.org"), Address::new("", "admin@example.org")]
}

/// What is offered with the cursor at the end of `q`, when every one of `people` could be what
/// was typed.
fn offered_from(q: &str, people: Vec<Address>) -> Vec<Suggestion> {
    suggestions_with(q, q.len(), &[], |_| people)
}

fn offered(q: &str) -> Vec<Suggestion> {
    offered_from(q, people())
}

fn labels(q: &str) -> Vec<String> {
    offered(q).into_iter().map(|s| s.label).collect()
}

/// `q` with `s` taken.
fn taken(q: &str, s: &Suggestion) -> String {
    format!("{}{}{}", &q[..s.range.start], s.text, &q[s.range.end..])
}

/// After `from:`, `to:` and `cc:`, the people found for what was typed, in their order, each by
/// name (or by address, without one); the address goes after the keyword, as it was typed: a
/// `-`, a capital, a space after the colon.
#[test]
fn after_from_to_and_cc_the_people_found() {
    for key in ["from", "to", "cc", "-from", "-to", "-cc", "From"] {
        assert_eq!(labels(&format!("{key}:ad")), ["Ada Lovelace", "Adam Smith", "admin@example.org"], "{key}");
        assert_eq!(offered(&format!("{key}:ad"))[0].text, format!("{key}:ada@example.org "));
    }
    let s = &offered("from:ad")[1];
    assert_eq!((s.address.as_deref(), s.color), (Some("adam@example.org"), None));
    assert_eq!(offered("from:ad")[2].address.as_deref(), Some("admin@example.org"));
    let q = "budget from: ad is:unread";
    let s = suggestions_with(q, 15, &[], |_| people()).remove(0);
    assert_eq!(taken(q, &s), "budget from:ada@example.org is:unread");
    // What was typed is what the people are looked for by: in quotes, a name with a space.
    let asked = RefCell::new(Vec::new());
    for q in ["from:ada", "to: \"ada lo", "cc:\"Ada\"", "-from:Grüß"] {
        suggestions_with(q, q.len(), &[], |typed| {
            asked.borrow_mut().push(typed.to_string());
            Vec::new()
        });
    }
    assert_eq!(asked.into_inner(), ["ada", "ada lo", "Ada", "Grüß"]);
}

/// Nobody is looked for until something is typed after the keyword, nor after the other
/// keywords.
#[test]
fn nobody_until_something_is_typed() {
    for q in ["from:", "from: ", "to:\"", "cc: \" ", "subject:ad", "body:ad", "category:", "ad", "is:", "\"from:ad", "from:ad x"] {
        let asked = RefCell::new(false);
        let found = suggestions_with(q, q.len(), &[], |_| {
            *asked.borrow_mut() = true;
            people()
        });
        assert!(found.iter().all(|s| s.address.is_none()) && !*asked.borrow(), "{q}");
    }
}

/// Only an address the search reads back as that one address is offered, each once: not one in
/// brackets, with a comma, an IP address for a domain, or no `@`.
#[test]
fn only_whole_addresses_once() {
    let found = offered_from(
        "from:a",
        vec![
            Address::new("Brackets", "<ada@example.org>"),
            Address::new("Comma", "ada@example.org,"),
            Address::new("Literal", "ada@[192.0.2.1]"),
            Address::new("Nothing", "ada"),
            Address::new("Empty", ""),
            Address::new("Ada", "ada@example.org"),
            Address::new("Ada again", "ADA@example.org"),
            Address::new("Ada Smith", "\"ada smith\"@example.org"),
            Address::new("Long", format!("{}@example.org", "a".repeat(300))),
        ],
    );
    let shown: Vec<(&str, &str)> = found.iter().map(|s| (s.label.as_str(), s.text.as_str())).collect();
    assert_eq!(shown, [("Ada", "from:ada@example.org "), ("Ada Smith", "from:\"\\\"ada smith\\\"@example.org\" ")]);
}

/// Taken, a person's address finds that address and no other, and with a `-` leaves it out.
#[test]
fn a_person_taken_finds_that_address_alone() {
    let from = |email: &str| Message { from: Address::new("", email), ..Default::default() };
    let to = |email: &str| Message { to: vec![Address::new("", email)], ..Default::default() };
    for (q, m) in [
        ("from:ad", from("ADA@example.org")),
        ("to:ad", to("ada@example.org")),
        ("cc:ad", Message { cc: vec![Address::new("", "ada@example.org")], ..Default::default() }),
    ] {
        let query = Query::parse(&taken(q, &offered(q)[0]));
        assert!(query.is_address_term(0), "{q}");
        assert!(query.matches(&m, 0), "{q}");
    }
    let query = Query::parse(&taken("from:ad", &offered("from:ad")[0]));
    assert!(!query.matches(&from("adam@example.org"), 0) && !query.matches(&from("ada@example.org.evil"), 0));
    assert!(!Query::parse(&taken("-from:ad", &offered("-from:ad")[0])).matches(&from("ada@example.org"), 0));
    // A quoted name before the `@`, written so the search reads it back whole.
    let q = taken("from:a", &offered_from("from:a", vec![Address::new("", "\"ada smith\"@example.org")])[0]);
    assert!(Query::parse(&q).matches(&from("\"Ada Smith\"@example.org"), 0), "{q}");
}

/// A name from mail is shown plainly: no control characters, its spaces run together, cut short.
#[test]
fn names_are_shown_plainly() {
    let shown = |name: &str| offered_from("from:a", vec![Address { name: name.to_string(), email: "ada@example.org".into() }]).remove(0).label;
    assert_eq!(shown("Ada\r\n  Lovelace\u{0}\t"), "Ada Lovelace");
    assert_eq!(shown(" \u{7} "), "ada@example.org");
    assert_eq!(shown(&"é".repeat(5_000)).chars().count(), 256);
}

/// At most eight, and an address typed whole isn't offered again.
#[test]
fn a_short_list_and_nothing_typed_whole() {
    let many: Vec<Address> = (0..500).map(|i| Address::new(format!("Person {i}"), format!("p{i}@example.org"))).collect();
    assert_eq!(offered_from("to:p", many).len(), 8);
    assert!(offered_from("from:ada@example.org", people()).iter().all(|s| s.address.as_deref() != Some("ada@example.org")));
    assert!(offered_from("from:ada@example.org", vec![Address::new("Ada", "ada@example.org")]).is_empty());
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

    /// Whatever is typed and whoever is found: a short list standing in the query, each person's
    /// name without control characters, and each address, taken, finds a message from, to or cc
    /// that address (and with a `-` before it, leaves it out), and not one from another.
    #[test]
    fn whoever_is_found_their_address_finds_them(
        q in "-?(from|to|cc|From): ?\"?.{0,8}|.{0,24}",
        cursor in proptest::prelude::any::<usize>(),
        found in proptest::collection::vec((".{0,12}", "[a-z.+'\"]{0,6}@[a-z0-9-]{1,6}(\\.[a-z]{1,4})?|.{0,16}"), 0..24),
    ) {
        let found: Vec<Address> = found.into_iter().map(|(name, email)| Address { name, email }).collect();
        for c in [cursor, q.len()] {
            let all = suggestions_with(&q, c, &[], |_| found.clone());
            proptest::prop_assert!(all.len() <= 8);
            for s in all {
                proptest::prop_assert!(q.get(s.range.clone()).is_some());
                proptest::prop_assert!(!s.label.chars().any(char::is_control) && !s.label.is_empty());
                let Some(address) = s.address.clone() else { continue };
                let query = Query::parse(&s.text);
                proptest::prop_assert!(query.is_address_term(0), "{:?}", s.text);
                let to = |email: &str| Message {
                    from: Address::new("", email),
                    to: vec![Address::new("", email)],
                    cc: vec![Address::new("", email)],
                    ..Default::default()
                };
                proptest::prop_assert_eq!(query.matches(&to(&address), 0), !s.text.starts_with('-'), "{:?}", s.text);
                proptest::prop_assert_eq!(query.matches(&to(&format!("x{address}")), 0), s.text.starts_with('-'), "{:?}", s.text);
            }
        }
    }
}
