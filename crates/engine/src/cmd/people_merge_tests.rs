// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People › Merge Duplicates (#506).

use mailcraft_model::{Contact, Id, Labeled, Origin, PostalAddress};
use serde_json::{Value, json};

use super::{MAX_MERGE, email_key, find, phone_key};
use crate::Session;
use crate::agent::{Access, Class, classify};

fn add(s: &mut Session, c: Contact) -> Id {
    s.mailbox.add_contact(c)
}

fn person(first: &str, last: &str, email: &str, phone: &str) -> Contact {
    Contact {
        first: first.into(),
        last: last.into(),
        emails: if email.is_empty() { vec![] } else { vec![Labeled::new("work", email)] },
        phones: if phone.is_empty() { vec![] } else { vec![Labeled::new("mobile", phone)] },
        ..Default::default()
    }
}

fn groups(s: &mut Session, params: Value) -> Vec<Value> {
    s.run("people.findDuplicates", &params).unwrap()["groups"].as_array().cloned().unwrap_or_default()
}

fn err(s: &mut Session, id: &str, params: Value) -> String {
    s.run(id, &params).map(|v| v.to_string()).unwrap_err().to_string()
}

#[test]
fn keys_compare_addresses_and_numbers() {
    assert_eq!(email_key("  Ada@Example.ORG "), Some("ada@example.org".into()));
    assert_eq!(email_key("not an address"), None);
    // Digits only, the last ten: national and international forms are one number.
    assert_eq!(phone_key("+44 20 7946 0000"), phone_key("020 7946 0000"));
    assert_eq!(phone_key("+1 (555) 010-0100"), phone_key("555.010.0100"));
    assert_ne!(phone_key("+1 555 010 0100"), phone_key("+1 555 010 0101"));
    // Too short to tell people apart.
    assert_eq!(phone_key("x 123456"), None);
    assert_eq!(phone_key("1234567").as_deref(), Some("1234567"));
    assert_eq!(phone_key(""), None);
    assert_eq!(phone_key(&"9".repeat(100_000)).map(|k| k.len()), Some(10));
}

#[test]
fn groups_by_shared_address_or_number_and_chains() {
    let mut s = Session::empty();
    let a = add(&mut s, person("Ada", "Lovelace", "ada@example.org", ""));
    let b = add(&mut s, Contact { company: "Analytical Engines".into(), ..person("Ada", "", " ADA@example.org", "+44 20 7946 0000") });
    let c = add(&mut s, person("A.", "Lovelace", "", "020 7946 0000"));
    let lone = add(&mut s, person("Grace", "Hopper", "grace@example.org", "1234"));
    let _short = add(&mut s, person("Short", "Code", "", "1234"));
    let (found, more) = find(&s.mailbox);
    assert_eq!(more, 0);
    assert_eq!(found.len(), 1, "{found:?}");
    let g = &found[0];
    assert_eq!(g.ids, vec![a, b, c]);
    assert!(!g.ids.contains(&lone));
    // The one with the most filled in is suggested.
    assert_eq!(g.keep, b);
    assert!(g.mergeable());
    assert_eq!(g.shares.len(), 2, "{:?}", g.shares);
    let listed = groups(&mut s, json!({}));
    assert_eq!(listed.len(), 1);
    assert!(listed[0]["fields"].as_array().unwrap().iter().any(|f| f["field"] == "first"));
}

#[test]
fn a_selection_is_one_group_whatever_it_shares() {
    let mut s = Session::empty();
    let a = add(&mut s, person("Ada", "", "a@example.org", ""));
    let b = add(&mut s, person("Bea", "", "b@example.org", ""));
    let g = groups(&mut s, json!({"ids": [a, b]}));
    assert_eq!(g.len(), 1);
    assert_eq!(g[0]["ids"], json!([a, b]));
    assert!(err(&mut s, "people.findDuplicates", json!({"ids": [a]})).contains("fewer than two"));
    assert!(err(&mut s, "people.findDuplicates", json!({"ids": [a, a]})).contains("twice"));
    assert!(err(&mut s, "people.findDuplicates", json!({"ids": [a, 999_999]})).contains("no contact"));
}

#[test]
fn merge_takes_choices_unions_lists_and_one_undo_brings_all_back() {
    let mut s = Session::empty();
    let keep = add(
        &mut s,
        Contact {
            company: "Analytical Engines".into(),
            categories: vec!["Family".into()],
            notes: "Met at the Society.".into(),
            ..person("Ada", "Lovelace", "ada@example.org", "+44 20 7946 0000")
        },
    );
    let other = add(
        &mut s,
        Contact {
            title: "Countess".into(),
            company: "Babbage & Co".into(),
            categories: vec!["family".into(), "Work".into()],
            websites: vec!["https://ada.example".into()],
            addresses: vec![PostalAddress { street: "1 St James's Square".into(), city: "London".into(), ..Default::default() }],
            favorite: true,
            notes: "Prefers letters.".into(),
            emails: vec![Labeled::new("home", "ADA@example.org"), Labeled::new("work", "countess@example.org")],
            phones: vec![Labeled::new("work", "+44 (20) 7946-0000"), Labeled::new("home", "020 7946 0999")],
            ..person("Augusta", "Lovelace", "", "")
        },
    );
    let before = s.mailbox.contacts.clone();
    s.sel.contact = Some(other);
    let out = s.run("people.merge", &json!({"keep": keep, "others": [other], "fields": {"first": other}})).unwrap();
    assert_eq!(out["id"], json!(keep));
    let c = s.mailbox.contact(keep).unwrap().clone();
    assert!(s.mailbox.contact(other).is_none());
    assert_eq!(s.sel.contact, Some(keep));
    // The choice, the kept contact's own, and what only the other had.
    assert_eq!((c.first.as_str(), c.company.as_str(), c.title.as_str()), ("Augusta", "Analytical Engines", "Countess"));
    // Each address and number once (by the address, by the digits).
    let emails: Vec<&str> = c.emails.iter().map(|e| e.value.as_str()).collect();
    assert_eq!(emails, ["ada@example.org", "countess@example.org"]);
    let phones: Vec<&str> = c.phones.iter().map(|e| e.value.as_str()).collect();
    assert_eq!(phones, ["+44 20 7946 0000", "020 7946 0999"]);
    assert_eq!(c.categories, ["Family", "Work"]);
    assert_eq!(c.websites.len(), 1);
    assert_eq!(c.addresses.len(), 1);
    assert!(c.favorite);
    assert_eq!(c.notes, "Met at the Society.\n\nPrefers letters.");
    assert_eq!(s.undo_label(), Some("Merge Contacts"));
    assert!(s.undo());
    assert_eq!(s.mailbox.contacts, before);
}

#[test]
fn a_list_from_one_contact_and_nothing_left_unnamed() {
    let mut s = Session::empty();
    let keep = add(&mut s, person("Ada", "", "ada@example.org", ""));
    let other = add(&mut s, person("", "", "other@example.org", ""));
    s.run("people.merge", &json!({"keep": keep, "others": [other], "fields": {"emails": keep}})).unwrap();
    let emails: Vec<String> = s.mailbox.contact(keep).unwrap().emails.iter().map(|e| e.value.clone()).collect();
    assert_eq!(emails, ["ada@example.org"]);
}

#[test]
fn refuses_what_it_cant_do_and_changes_nothing() {
    let mut s = Session::empty();
    let a = add(&mut s, person("Ada", "", "ada@example.org", ""));
    let b = add(&mut s, person("Ada", "L", "ada@example.org", ""));
    let before = s.mailbox.contacts.clone();
    assert!(err(&mut s, "people.merge", json!({"keep": a, "others": [a]})).contains("both kept and merged"));
    assert!(err(&mut s, "people.merge", json!({"keep": a, "others": [b, b]})).contains("twice"));
    assert!(err(&mut s, "people.merge", json!({"keep": a, "others": [999_999]})).contains("no contact"));
    assert!(err(&mut s, "people.merge", json!({"keep": 999_999, "others": [a]})).contains("no contact"));
    assert!(err(&mut s, "people.merge", json!({"keep": a, "others": []})).contains("no contact to merge"));
    assert!(err(&mut s, "people.merge", json!({"keep": a})).contains("`others`"));
    assert!(err(&mut s, "people.merge", json!({"keep": a, "others": [b], "fields": {"colour": a}})).contains("colour"));
    assert!(err(&mut s, "people.merge", json!({"keep": a, "others": [b], "fields": {"first": 999_999}})).contains("not one of those"));
    let many: Vec<u64> = (1..=MAX_MERGE as u64).collect();
    assert!(err(&mut s, "people.merge", json!({"keep": a, "others": many})).contains("At most 100"));
    assert_eq!(s.mailbox.contacts, before);
}

#[test]
fn a_contact_a_server_keeps_is_not_merged_and_says_why() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let mine = add(&mut s, person("Ada", "", "ada@example.org", ""));
    let kept = add(
        &mut s,
        Contact { origin: Origin { account, id: "AAMk".into(), ..Default::default() }, ..person("Ada", "Lovelace", "ada@example.org", "") },
    );
    let before = s.mailbox.contacts.clone();
    for (keep, other) in [(mine, kept), (kept, mine)] {
        let why = err(&mut s, "people.merge", json!({"keep": keep, "others": [other]}));
        assert!(why.contains("kept by the server of") && why.contains("can't be merged"), "{why}");
    }
    assert_eq!(s.mailbox.contacts, before);
    let g = groups(&mut s, json!({}));
    assert_eq!(g.len(), 1);
    assert_eq!(g[0]["mergeable"], json!(false));
    assert_eq!(g[0]["keep"], json!(mine));
    assert_eq!(g[0]["locked"][0]["id"], json!(kept));
    assert!(g[0]["locked"][0]["why"].as_str().unwrap().contains("kept by the server"));
}

#[test]
fn an_agents_merge_is_asked_about_and_finding_is_not() {
    let mut s = Session::empty();
    let a = add(&mut s, person("Ada", "", "ada@example.org", ""));
    let b = add(&mut s, person("Ada", "L", "ada@example.org", ""));
    assert_eq!(classify(&s, "people.merge", &json!({"keep": a, "others": [b]})), Access::Owner(Class::Delete));
    assert_eq!(classify(&s, "people.findDuplicates", &json!({})), Access::Safe);
}

#[test]
fn many_contacts_are_grouped_in_one_pass() {
    let mut s = Session::empty();
    // A switchboard number that 300 have links none of them; 150 contacts chained two by two
    // (each shares an address with the next) are one group, listed as 100 and the rest counted;
    // then 20,000 contacts in 10,000 pairs.
    for i in 0..300u32 {
        add(&mut s, person("Desk", &i.to_string(), "", "+1 555 010 0100"));
    }
    for i in 0..150u32 {
        let c = Contact {
            emails: vec![Labeled::new("work", format!("c{i}@example.org")), Labeled::new("home", format!("c{}@example.org", i + 1))],
            ..person("Chain", &i.to_string(), "", "")
        };
        add(&mut s, c);
    }
    for i in 0..10_000u32 {
        let email = format!("p{i}@example.org");
        add(&mut s, person("A", &i.to_string(), &email, ""));
        add(&mut s, person("B", &i.to_string(), &email.to_uppercase(), ""));
    }
    let started = std::time::Instant::now();
    let (found, more) = find(&s.mailbox);
    assert!(started.elapsed().as_secs() < 20);
    assert_eq!(found.len() + more, 10_001);
    assert_eq!(found.len(), super::MAX_GROUPS);
    assert_eq!((found[0].ids.len(), found[0].more), (MAX_MERGE, 50));
    assert!(found.iter().skip(1).all(|g| g.ids.len() == 2));
    assert!(found.iter().all(|g| g.ids.iter().all(|id| s.mailbox.contact(*id).is_some_and(|c| c.first != "Desk"))));
}

#[test]
fn what_many_share_links_nobody_but_what_else_they_share_does() {
    let mut s = Session::empty();
    // info@ on six contacts links none of them; on five it does.
    let six: Vec<Id> = (0..6).map(|i| add(&mut s, person("Six", &i.to_string(), "info@six.example", ""))).collect();
    let five: Vec<Id> = (0..5).map(|i| add(&mut s, person("Five", &i.to_string(), "info@five.example", ""))).collect();
    // Two of the six have a mobile of their own in common, and are a group by it.
    for id in &six[..2] {
        s.mailbox.contact_mut(*id).unwrap().phones.push(Labeled::new("mobile", "07700 900123"));
    }
    let (found, _) = find(&s.mailbox);
    let ids: Vec<Vec<Id>> = found.iter().map(|g| g.ids.clone()).collect();
    assert_eq!(ids, vec![six[..2].to_vec(), five]);
    assert_eq!(found[0].shares, ["07700 900123"]);
}

#[test]
fn a_merge_never_cuts_what_the_kept_contact_has() {
    let mut s = Session::empty();
    let long = "n".repeat(super::MAX_NOTES + 20_000);
    let many: Vec<String> = (0..1200).map(|i| format!("Category {i}")).collect();
    let keep = add(&mut s, Contact { notes: long.clone(), categories: many.clone(), ..person("Ada", "", "ada@example.org", "") });
    let other =
        add(&mut s, Contact { notes: long.to_uppercase(), categories: vec!["category 3".into()], ..person("Ada", "L", "ada@example.org", "") });
    s.run("people.merge", &json!({"keep": keep, "others": [other]})).unwrap();
    let c = s.mailbox.contact(keep).unwrap();
    // (The other's notes and category are the kept one's in other capitals: taken once.)
    assert_eq!((c.notes.len(), c.categories.len()), (long.len(), many.len()));
}

#[test]
fn a_merge_that_would_grow_past_what_a_contact_holds_is_refused_by_name() {
    let mut s = Session::empty();
    let half = "a".repeat(super::MAX_NOTES / 2 + 10);
    let keep = add(&mut s, Contact { notes: half.clone(), ..person("Ada", "", "ada@example.org", "") });
    let other = add(&mut s, Contact { notes: "b".repeat(super::MAX_NOTES / 2 + 10), ..person("Ada", "L", "ada@example.org", "") });
    let before = s.mailbox.contacts.clone();
    let why = err(&mut s, "people.merge", json!({"keep": keep, "others": [other]}));
    assert!(why.contains("notes would be too large"), "{why}");
    assert_eq!(s.mailbox.contacts, before);
    // One contact's notes alone are taken whole.
    s.run("people.merge", &json!({"keep": keep, "others": [other], "fields": {"notes": keep}})).unwrap();
    assert_eq!(s.mailbox.contact(keep).unwrap().notes, half);
    let a = add(&mut s, Contact { categories: (0..600).map(|i| format!("a{i}")).collect(), ..person("Bea", "", "bea@example.org", "") });
    let b = add(&mut s, Contact { categories: (0..600).map(|i| format!("b{i}")).collect(), ..person("Bea", "B", "bea@example.org", "") });
    assert!(err(&mut s, "people.merge", json!({"keep": a, "others": [b]})).contains("categories would be too large"));
}

#[test]
fn long_values_are_shown_cut_and_marked() {
    let mut s = Session::empty();
    let a = add(&mut s, Contact { notes: "é".repeat(5000), ..person("Ada", "", "ada@example.org", "") });
    add(&mut s, Contact { notes: "short".into(), ..person("Ada", "L", "ada@example.org", "") });
    let g = groups(&mut s, json!({}));
    let notes = g[0]["fields"].as_array().unwrap().iter().find(|f| f["field"] == "notes").unwrap()["values"].clone();
    let long = notes.as_array().unwrap().iter().find(|v| v["id"] == json!(a)).unwrap().clone();
    assert_eq!((long["value"].as_str().unwrap().chars().count(), &long["truncated"]), (2000, &json!(true)));
    assert!(notes.as_array().unwrap().iter().any(|v| v["value"] == "short" && v.get("truncated").is_none()));
}

#[test]
fn copies_of_one_card_are_a_group_however_many() {
    let mut s = Session::empty();
    // The same card imported six times: its address and number are on more than five contacts,
    // but the names are the same (whatever the spaces and capitals).
    let names = ["Ada Lovelace", " ada  lovelace", "ADA LOVELACE", "Ada Lovelace", "Ada Lovelace ", "Ada lovelace"];
    let copies: Vec<Id> =
        names.iter().map(|n| add(&mut s, Contact { display_name: (*n).into(), ..person("", "", "ada@example.org", "+44 20 7946 0000") })).collect();
    // A switchboard of six different people is still nobody's group.
    for i in 0..6 {
        add(&mut s, person("Desk", &i.to_string(), "", "+1 555 010 0100"));
    }
    let (found, _) = find(&s.mailbox);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].ids, copies);
    assert!(!found[0].shares.is_empty());
}

#[test]
fn numbers_of_different_countries_are_both_kept() {
    let mut s = Session::empty();
    // The same last ten digits, two countries: one group (the owner's call), two numbers kept.
    let a = add(&mut s, person("Ada", "", "ada@example.org", "+44 20 7946 0000"));
    let b = add(&mut s, person("Ada", "", "", "+1 207 946 0000"));
    assert_eq!(find(&s.mailbox).0.len(), 1);
    s.run("people.merge", &json!({"keep": a, "others": [b]})).unwrap();
    let phones: Vec<String> = s.mailbox.contact(a).unwrap().phones.iter().map(|p| p.value.clone()).collect();
    assert_eq!(phones, ["+44 20 7946 0000", "+1 207 946 0000"]);
}

#[test]
fn a_merge_past_what_an_edit_keeps_is_refused_and_one_within_survives_an_edit() {
    let mut s = Session::empty();
    let mails = |from: u32, n: u32| (from..from + n).map(|i| Labeled::new("work", format!("a{i}@example.org"))).collect::<Vec<_>>();
    let a = add(&mut s, Contact { emails: mails(0, 60), ..person("Ada", "", "", "") });
    let b = add(&mut s, Contact { emails: [mails(0, 1), mails(100, 60)].concat(), ..person("Ada", "L", "", "") });
    let before = s.mailbox.contacts.clone();
    let why = err(&mut s, "people.merge", json!({"keep": a, "others": [b]}));
    assert!(why.contains("e-mail addresses would be too large (120, at most 100)"), "{why}");
    assert_eq!(s.mailbox.contacts, before);
    // 60 and 40 more: 100, which the contact form sends back whole and people.edit keeps.
    s.mailbox.contact_mut(b).unwrap().emails = [mails(0, 1), mails(100, 40)].concat();
    s.run("people.merge", &json!({"keep": a, "others": [b]})).unwrap();
    let merged = s.mailbox.contact(a).unwrap().clone();
    assert_eq!(merged.emails.len(), 100);
    let emails: Vec<Value> = merged.emails.iter().map(|e| json!({"label": e.label, "value": e.value})).collect();
    s.run("people.edit", &json!({"id": a, "emails": emails, "phones": [], "categories": merged.categories, "notes": merged.notes})).unwrap();
    assert_eq!(s.mailbox.contact(a).unwrap(), &merged);
}
