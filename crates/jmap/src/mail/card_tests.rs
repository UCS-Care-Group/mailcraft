// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A JSContact card read as a contact, made and patched (CS10).

use std::collections::BTreeSet;

use mailcraft_model::contact_sync::ContactField;
use mailcraft_model::{Labeled, PostalAddress};
use mailcraft_time::Date;
use proptest::prelude::*;
use serde_json::{Value, json};

use super::card::{made, patch, read};
use crate::connection::tests::any_json;

/// A card as another program wrote it, with what MailCraft doesn't show beside what it does.
fn ada() -> Value {
    json!({
        "@type": "Card", "version": "1.0", "id": "CC1", "uid": "urn:uuid:ada", "kind": "individual",
        "addressBookIds": {"AB1": true},
        "name": {"full": "Ada Lovelace", "components": [
            {"kind": "title", "value": "Countess"}, {"kind": "given", "value": "Ada"},
            {"kind": "given2", "value": "Augusta"}, {"kind": "surname", "value": "Lovelace"}], "isOrdered": true},
        "nicknames": {"k1": {"name": "Ada"}},
        "organizations": {"o1": {"name": "Analytical Engines", "units": [{"name": "Research"}]}},
        "titles": {"t1": {"name": "Programmer"}, "t2": {"name": "Lead", "kind": "role"}},
        "emails": {"e1": {"address": "ada@example.org", "contexts": {"work": true}, "pref": 1},
                   "e2": {"address": "ada@home.example", "contexts": {"private": true}}},
        "phones": {"p1": {"number": "+44 20 7946 0001", "features": {"mobile": true}}},
        "addresses": {"a1": {"components": [{"kind": "number", "value": "12"}, {"kind": "name", "value": "Engine Row"},
                       {"kind": "locality", "value": "London"}, {"kind": "postcode", "value": "N1"}, {"kind": "country", "value": "UK"},
                       {"kind": "apartment", "value": "3"}], "contexts": {"work": true}}},
        "links": {"l1": {"uri": "https://ada.example"}},
        "anniversaries": {"b1": {"kind": "birth", "date": {"@type": "PartialDate", "year": 1815, "month": 12, "day": 10}},
                          "w1": {"kind": "wedding", "date": {"@type": "PartialDate", "year": 1835, "month": 7, "day": 8}}},
        "notes": {"n1": {"note": "Met at the salon.", "author": {"name": "Zoe"}}},
        "keywords": {"VIP": true, "Friends": true},
        "imAddresses": {"i1": {"handle": "ada"}},
    })
}

#[test]
fn a_card_reads_as_the_contact_it_shows() {
    let c = read(&ada()).unwrap();
    assert_eq!(c.id, "CC1");
    assert_eq!((c.display_name.as_str(), c.first.as_str(), c.middle.as_str(), c.last.as_str()), ("Ada Lovelace", "Ada", "Augusta", "Lovelace"));
    assert_eq!(
        (c.nickname.as_str(), c.company.as_str(), c.department.as_str(), c.title.as_str()),
        ("Ada", "Analytical Engines", "Research", "Programmer")
    );
    assert_eq!(c.emails, vec![Labeled::new("work", "ada@example.org"), Labeled::new("home", "ada@home.example")]);
    assert_eq!(c.phones, vec![Labeled::new("mobile", "+44 20 7946 0001")]);
    assert_eq!(
        c.addresses,
        vec![PostalAddress {
            label: "work".into(),
            street: "12 Engine Row".into(),
            city: "London".into(),
            postal_code: "N1".into(),
            country: "UK".into(),
            ..Default::default()
        }]
    );
    assert_eq!(c.websites, vec!["https://ada.example".to_string()]);
    assert_eq!(c.birthday, Date::new(1815, 12, 10));
    assert_eq!(c.notes, "Met at the salon.");
    assert_eq!(c.categories, Some(vec!["Friends".to_string(), "VIP".to_string()]));
}

#[test]
fn a_group_is_no_contact_and_a_card_with_no_id_is_none() {
    assert!(read(&json!({"id": "G1", "kind": "group", "members": {"urn:uuid:ada": true}})).is_none());
    assert!(read(&json!({"id": "not an id!", "name": {"full": "X"}})).is_none());
    assert!(read(&json!("a string")).is_none());
}

#[test]
fn a_change_patches_only_the_properties_of_the_fields_changed_and_keeps_what_isnt_shown() {
    let server = ada();
    let mut values = read(&server).unwrap();
    // Nothing changed: nothing to send, whatever fields are named.
    assert!(patch(&server, &values, &ContactField::ALL.into_iter().collect()).is_empty());

    values.last = "King".into();
    values.emails.push(Labeled::new("", "countess@example.org"));
    values.phones = vec![Labeled::new("work", "+44 20 7946 0001")];
    values.notes = "Met at the salon. Wrote the first program.".into();
    let fields: BTreeSet<ContactField> = [ContactField::Last, ContactField::Emails, ContactField::Phones, ContactField::Notes].into();
    let p = patch(&server, &values, &fields);
    let keys: BTreeSet<&str> = p.keys().map(String::as_str).collect();
    assert_eq!(keys, ["emails", "name", "notes", "phones"].into());
    // The name keeps its title part and its full name; only the parts MailCraft keeps change.
    assert_eq!(p["name"]["full"], "Ada Lovelace");
    let parts = p["name"]["components"].as_array().unwrap();
    assert!(parts.contains(&json!({"kind": "title", "value": "Countess"})));
    assert!(parts.contains(&json!({"kind": "surname", "value": "King"})));
    assert!(!parts.iter().any(|p| p["value"] == "Lovelace"));
    // The addresses the card had keep their keys and what else they hold (the preference); the new one
    // gets a key of its own.
    assert_eq!(p["emails"]["e1"], server["emails"]["e1"]);
    assert_eq!(p["emails"]["e2"], server["emails"]["e2"]);
    assert_eq!(p["emails"].as_object().unwrap().len(), 3);
    assert!(p["emails"].as_object().unwrap().values().any(|e| e == &json!({"address": "countess@example.org"})));
    // The number, relabelled, is the same entry with its context instead of its feature.
    assert_eq!(p["phones"], json!({"p1": {"number": "+44 20 7946 0001", "contexts": {"work": true}}}));
    assert_eq!(p["notes"], json!({"n1": {"note": "Met at the salon. Wrote the first program."}}));
    // A field not named is not sent, though it differs.
    values.company = "Babbage & Co".into();
    assert!(!patch(&server, &values, &fields).contains_key("organizations"));
}

#[test]
fn emptied_fields_take_their_entries_away_and_others_stay() {
    let server = ada();
    let mut values = read(&server).unwrap();
    (values.title, values.department, values.birthday, values.categories) = (String::new(), String::new(), None, Some(vec![]));
    let fields: BTreeSet<ContactField> = [ContactField::Title, ContactField::Department, ContactField::Birthday, ContactField::Categories].into();
    let p = patch(&server, &values, &fields);
    // The role stays; the job title goes.
    assert_eq!(p["titles"], json!({"t2": {"name": "Lead", "kind": "role"}}));
    assert_eq!(p["organizations"], json!({"o1": {"name": "Analytical Engines"}}));
    // The wedding stays.
    assert_eq!(p["anniversaries"], json!({"w1": server["anniversaries"]["w1"]}));
    assert_eq!(p["keywords"], Value::Null);
}

#[test]
fn a_new_contact_is_a_whole_card_in_its_address_book() {
    let values = mailcraft_model::remote::RemoteContact {
        first: "Bo".into(),
        last: "Park".into(),
        emails: vec![Labeled::new("work", "bo@example.org")],
        birthday: Date::new(1990, 2, 3),
        categories: Some(vec!["Team".into()]),
        ..Default::default()
    };
    let card = made(&values, "AB7", "urn:uuid:x");
    assert_eq!(card["@type"], "Card");
    assert_eq!(card["uid"], "urn:uuid:x");
    assert_eq!(card["addressBookIds"], json!({"AB7": true}));
    assert_eq!(card["emails"], json!({"e1": {"address": "bo@example.org", "contexts": {"work": true}}}));
    assert_eq!(card["keywords"], json!({"Team": true}));
    assert!(!card.contains_key("notes") && !card.contains_key("id"));
    // It reads back as what was sent.
    let mut back = read(&Value::Object(card.into_iter().chain([("id".to_string(), json!("CC9"))]).collect())).unwrap();
    back.id.clear();
    assert_eq!(back, values);
}

proptest! {
    #[test]
    fn whatever_a_server_sends_reads_and_patches_without_panicking(card in any_json(), other in any_json()) {
        let _ = read(&card);
        let values = read(&other).unwrap_or_default();
        let _ = patch(&card, &values, &ContactField::ALL.into_iter().collect());
        let _ = super::card::books_of(&card);
    }
}
