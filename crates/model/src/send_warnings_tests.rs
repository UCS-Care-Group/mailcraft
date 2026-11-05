// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send warnings (#509): each rule, when it asks and, as much, when it doesn't.
//!
//! The people and domains are invented.

use std::collections::HashSet;

use proptest::prelude::*;

use crate::send_warnings::{SendWarning, check, has_outsiders, lookalike, outsiders_to_bcc, seen_on};
use crate::sender_warnings::{Facts, FactsBuilder};
use crate::{Account, Address, Message};

/// Avery at Lumen, who keeps Maya and Dev as contacts and has written to Lin at Harbor Works and
/// to a friend at a public provider.
fn facts() -> Facts {
    let me = Account { email: "avery@lumen.example".into(), display_name: "Avery Quinn".into(), ..Account::default() };
    let mut b = FactsBuilder::default().accounts([&me]);
    b.known("maya.okafor@lumen.example", "Maya Okafor");
    b.known("dev.patel@example.org", "Dev Patel");
    b.known("lin.zhao@harborworks.example", "");
    b.known("sam.friend@gmail.com", "");
    b.build()
}

/// Personal accounts only: no organisation.
fn personal() -> Facts {
    let me = Account { email: "avery.q@gmail.com".into(), display_name: "Avery Quinn".into(), ..Account::default() };
    FactsBuilder::default().accounts([&me]).build()
}

fn to(list: &[&str]) -> Vec<Address> {
    list.iter().map(|a| Address::new("", *a)).collect()
}

fn kinds(to_: &[&str], cc: &[&str], bcc: &[&str]) -> Vec<&'static str> {
    check(&to(to_), &to(cc), &to(bcc), &facts(), &HashSet::new()).iter().map(SendWarning::kind).collect()
}

// ---- lookalike recipients ----

#[test]
fn a_domain_one_character_away_is_asked_about() {
    let f = facts();
    assert_eq!(lookalike(&f, "lin.zhao@harborwork.example").as_deref(), Some("harborworks.example"));
    assert_eq!(lookalike(&f, "accounts@harb0rworks.example").as_deref(), Some("harborworks.example"));
    // Two letters side by side swapped is one edit; two edits away is somebody else.
    assert_eq!(lookalike(&f, "lin.zhao@harbowrorks.example").as_deref(), Some("harborworks.example"));
    assert_eq!(lookalike(&f, "lin.zhao@harbrowoks.example"), None);
    assert_eq!(lookalike(&f, "lin.zhao@hrabrorks.example"), None);
    // A typo of a public provider's domain too.
    assert_eq!(lookalike(&f, "sam.friend@gmial.com").as_deref(), Some("gmail.com"));
    assert_eq!(kinds(&["lin.zhao@harborwork.example"], &[], &[]), ["lookalikeRecipient"]);
}

#[test]
fn a_look_alike_character_in_the_domain_is_asked_about() {
    // Cyrillic а in place of the Latin a.
    assert_eq!(lookalike(&facts(), "lin.zhao@h\u{430}rborworks.example").as_deref(), Some("harborworks.example"));
}

#[test]
fn the_part_before_the_at_one_character_away_is_asked_about() {
    let f = facts();
    assert_eq!(lookalike(&f, "maya.okafur@lumen.example").as_deref(), Some("maya.okafor@lumen.example"));
    assert_eq!(lookalike(&f, "mayaokafor@lumen.example").as_deref(), Some("maya.okafor@lumen.example"));
    assert_eq!(lookalike(&f, "dev.pate1@example.org").as_deref(), Some("dev.patel@example.org"));
    // Only against the known addresses at that domain.
    assert_eq!(lookalike(&f, "maya.okafur@example.org"), None);
    // Short names are one letter from many others.
    let mut b = FactsBuilder::default();
    b.known("anna@lumen.example", "");
    assert_eq!(lookalike(&b.build(), "anne@lumen.example"), None);
}

#[test]
fn numbered_mailboxes_and_dots_a_provider_ignores_are_not_lookalikes() {
    let mut b = FactsBuilder::default();
    b.known("sales1@harborworks.example", "");
    b.known("room101@harborworks.example", "");
    b.known("maya.okafor@gmail.com", "");
    let f = b.build();
    assert_eq!(lookalike(&f, "sales2@harborworks.example"), None);
    assert_eq!(lookalike(&f, "room102@harborworks.example"), None);
    assert_eq!(lookalike(&f, "mayaokafor@gmail.com"), None, "the same mailbox at Gmail");
    // A letter for a digit still is one.
    assert_eq!(lookalike(&f, "saless@harborworks.example").as_deref(), Some("sales1@harborworks.example"));
    assert_eq!(lookalike(&f, "maya.okafur@gmail.com").as_deref(), Some("maya.okafor@gmail.com"));
}

#[test]
fn a_lookalike_is_shown_for_what_it_is() {
    // A look-alike letter shows as the domain goes over the wire, and what draws nothing is left out.
    let w = check(&to(&["Lin.Zhao\u{200b}@h\u{430}rborworks.example"]), &[], &[], &facts(), &HashSet::new());
    let SendWarning::LookalikeRecipient { address, like } = &w[0] else { panic!("{w:?}") };
    assert!(address.starts_with("lin.zhao@xn--") && !address.contains('\u{200b}'), "{address}");
    assert_eq!(like, "harborworks.example");
    let long = format!("{}@harborwork.example", "x".repeat(5_000));
    let w = check(&to(&[long.as_str()]), &[], &[], &facts(), &HashSet::new());
    let SendWarning::LookalikeRecipient { address, .. } = &w[0] else { panic!("{w:?}") };
    assert!(address.chars().count() < 200);
}

#[test]
fn what_is_known_is_never_asked_about() {
    let f = facts();
    for same in [
        "maya.okafor@lumen.example",
        "Maya.Okafor@LUMEN.example",
        " maya.okafor@lumen.example ",
        "dev.patel@example.org",
        "avery@lumen.example",
        "AVERY@lumen.example",
        // The owner's organisation's domain is known, so a new colleague there is nobody's lookalike.
        "jordan.reyes@lumen.example",
        // Somebody at a known domain whose name is nobody's.
        "accounts@harborworks.example",
        // A domain nobody here writes to, and like no known one.
        "hello@brightfield.example",
    ] {
        assert_eq!(lookalike(&f, same), None, "{same}");
    }
    assert!(kinds(&["maya.okafor@lumen.example", "Dev.Patel@Example.org"], &["lin.zhao@harborworks.example"], &[]).is_empty());
}

#[test]
fn a_known_domain_written_the_other_way_is_the_same() {
    let mut b = FactsBuilder::default();
    b.known("kai@xn--mnchen-3ya.example", "");
    let f = b.build();
    assert_eq!(lookalike(&f, "kai@münchen.example"), None);
    assert!(f.knows("KAI@München.example"));
}

#[test]
fn bcc_recipients_are_looked_at_for_lookalikes() {
    assert_eq!(kinds(&[], &[], &["lin.zhao@harborwork.example"]), ["lookalikeRecipient"]);
    // Once each, however often it is named.
    assert_eq!(kinds(&["lin.zhao@harborwork.example"], &["LIN.ZHAO@harborwork.example"], &[]), ["lookalikeRecipient"]);
}

// ---- outsiders who would see each other ----

const FIVE_AT_TWO: [&str; 5] =
    ["a.one@brightfield.example", "b.two@brightfield.example", "c.three@brightfield.example", "d.four@corvid.example", "e.five@corvid.example"];

#[test]
fn five_outsiders_at_two_domains_are_asked_about() {
    assert_eq!(kinds(&FIVE_AT_TWO, &[], &[]), ["outsidersSeeEachOther"]);
    // To and Cc together.
    assert_eq!(kinds(&FIVE_AT_TWO[..2], &FIVE_AT_TWO[2..], &[]), ["outsidersSeeEachOther"]);
    let w = check(&to(&FIVE_AT_TWO), &[], &[], &facts(), &HashSet::new());
    assert_eq!(w[0], SendWarning::Outsiders { count: 5, domains: 2, addresses: FIVE_AT_TWO.iter().map(|a| a.to_string()).collect() });
    assert_eq!(w[0].json()["count"], 5);
    assert!(has_outsiders(&w));
}

#[test]
fn four_outsiders_or_one_domain_is_not() {
    assert!(kinds(&FIVE_AT_TWO[..4], &[], &[]).is_empty(), "four");
    let one_domain = [
        "a@brightfield.example",
        "b@brightfield.example",
        "c@brightfield.example",
        "d@brightfield.example",
        "e@brightfield.example",
        "f@mail.brightfield.example",
    ];
    assert!(kinds(&one_domain, &[], &[]).is_empty(), "one organisation's, however many");
    // The same address twice is one person.
    let twice = [
        "a.one@brightfield.example",
        "A.ONE@brightfield.example",
        "b.two@brightfield.example",
        "c.three@brightfield.example",
        "d.four@corvid.example",
    ];
    assert!(kinds(&twice, &[], &[]).is_empty());
    // In Bcc they see nobody.
    assert!(kinds(&[], &[], &FIVE_AT_TWO).is_empty());
    assert!(kinds(&FIVE_AT_TWO[..2], &[], &FIVE_AT_TWO[2..]).is_empty());
}

#[test]
fn colleagues_and_the_owner_are_not_outsiders() {
    let mut list = FIVE_AT_TWO[..4].to_vec();
    list.extend(["maya.okafor@lumen.example", "avery@lumen.example", "jordan@sales.lumen.example"]);
    assert!(kinds(&list, &[], &[]).is_empty());
}

#[test]
fn without_an_organisation_everybody_else_is_outside() {
    let w = check(&to(&FIVE_AT_TWO), &[], &[], &personal(), &HashSet::new());
    assert!(has_outsiders(&w));
    let mut list = FIVE_AT_TWO[..4].to_vec();
    list.push("avery.q@gmail.com");
    assert!(check(&to(&list), &[], &[], &personal(), &HashSet::new()).is_empty(), "the owner is not an outsider");
}

#[test]
fn a_reply_to_people_who_saw_each_other_already_is_not_asked_about() {
    let original = Message { from: Address::new("A", FIVE_AT_TWO[0]), to: to(&FIVE_AT_TWO[1..3]), cc: to(&FIVE_AT_TWO[3..]), ..Message::default() };
    let seen = seen_on(&original);
    assert!(check(&to(&FIVE_AT_TWO), &[], &[], &facts(), &seen).is_empty());
    // One more who wasn't there sees them all.
    let mut more = FIVE_AT_TWO.to_vec();
    more.push("f.six@kestrel.example");
    assert!(has_outsiders(&check(&to(&more), &[], &[], &facts(), &seen)));
}

#[test]
fn move_to_bcc_takes_only_the_outsiders_and_keeps_their_names() {
    let f = facts();
    let mut to_line = vec![Address::new("Maya Okafor", "maya.okafor@lumen.example"), Address::new("Bea Two", "b.two@brightfield.example")];
    to_line.extend(to(&FIVE_AT_TWO[2..]));
    let cc = vec![Address::new("Al One", "a.one@brightfield.example"), Address::new("", "avery@lumen.example")];
    let bcc = to(&["e.five@corvid.example"]);
    let (t, c, b) = outsiders_to_bcc(&f, &to_line, &cc, &bcc);
    assert_eq!(t, vec![Address::new("Maya Okafor", "maya.okafor@lumen.example")]);
    assert_eq!(c, vec![Address::new("", "avery@lumen.example")]);
    let emails: Vec<&str> = b.iter().map(|a| a.email.as_str()).collect();
    assert_eq!(
        emails,
        ["e.five@corvid.example", "b.two@brightfield.example", "c.three@brightfield.example", "d.four@corvid.example", "a.one@brightfield.example"]
    );
    assert_eq!(b[1].name, "Bea Two");
    // Moved, nothing is left to ask about them.
    assert!(!has_outsiders(&check(&t, &c, &b, &f, &HashSet::new())));
}

#[test]
fn duplicates_padding_the_list_dont_hide_a_lookalike() {
    // 1,000 copies of a known address before the lookalike: each counts once.
    let mut list = vec!["maya.okafor@lumen.example"; crate::send_warnings::MAX_RECIPIENTS + 500];
    list.push("lin.zhao@harborwork.example");
    assert_eq!(kinds(&list, &[], &[]), ["lookalikeRecipient"]);
    // The same across the lines.
    let padding = vec!["Maya.Okafor@lumen.example"; crate::send_warnings::MAX_RECIPIENTS];
    assert_eq!(kinds(&padding, &padding, &["lin.zhao@harborwork.example"]), ["lookalikeRecipient"]);
}

#[test]
fn duplicates_padding_the_list_dont_hide_outsiders() {
    let mut list = vec!["maya.okafor@lumen.example"; crate::send_warnings::MAX_RECIPIENTS + 10];
    list.extend(FIVE_AT_TWO);
    assert_eq!(kinds(&list, &[], &[]), ["outsidersSeeEachOther"]);
}

#[test]
fn more_recipients_than_are_checked_is_a_warning_itself() {
    // Colleagues, every one known by domain: nothing to say about any, but they are too many to
    // look at, and what isn't looked at isn't let through unasked.
    let many: Vec<String> = (0..=crate::send_warnings::MAX_RECIPIENTS).map(|i| format!("person{i}@lumen.example")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(kinds(&many, &[], &[]), ["tooManyRecipients"]);
    // Up to the limit, they are looked at.
    assert!(kinds(&many[..crate::send_warnings::MAX_RECIPIENTS], &[], &[]).is_empty());
    // And a lookalike past the first ones still makes it a warning.
    let mut with = many[..crate::send_warnings::MAX_RECIPIENTS - 1].to_vec();
    with.push("lin.zhao@harborwork.example");
    assert_eq!(kinds(&with, &[], &[]), ["lookalikeRecipient"]);
}

#[test]
fn lookalikes_said_are_bounded() {
    let many: Vec<String> = (0..100).map(|i| format!("p{i}@harborwork.example")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let w = check(&to(&many), &[], &[], &facts(), &HashSet::new());
    assert_eq!(w.iter().filter(|w| w.kind() == "lookalikeRecipient").count(), crate::send_warnings::MAX_SAID);
}

proptest! {
    #[test]
    fn any_recipients_never_panic(lines in proptest::collection::vec("[ -~\u{80}-\u{2ff}]{0,40}", 0..30)) {
        let list: Vec<Address> = lines.iter().map(|l| Address::new("", l)).collect();
        let half = list.len() / 2;
        let f = facts();
        let _ = check(list.get(..half).unwrap_or_default(), list.get(half..).unwrap_or_default(), &list, &f, &HashSet::new());
        let _ = outsiders_to_bcc(&f, &list, &list, &list);
    }
}
