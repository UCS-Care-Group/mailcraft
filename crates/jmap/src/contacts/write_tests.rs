// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A contact written as a card: the exact card of a contact with everything, each part by itself, what is
//! cut and what is left out, and that any contact comes back from its card as it went. Every name and
//! address is invented.

use mailcraft_model::remote::{ContactField, RemoteContact};
use mailcraft_model::{Labeled, PostalAddress};
use mailcraft_time::Date;
use proptest::prelude::*;
use serde_json::{Value, json};

use super::card::card;
use super::write::{NOT_A_DAY, lines, new_card, one_line, property, written};

fn labeled(label: &str, value: &str) -> Labeled {
    Labeled { label: label.into(), value: value.into() }
}

/// A contact with something in every part.
pub(super) fn ada() -> RemoteContact {
    RemoteContact {
        id: "K1".into(),
        display_name: "Ada Example".into(),
        first: "Ada".into(),
        middle: "Maria".into(),
        last: "Example".into(),
        nickname: "Addie".into(),
        company: "Engines Ltd".into(),
        title: "Analyst".into(),
        department: "Research".into(),
        emails: vec![
            labeled("work", "ada@example.org"),
            labeled("home", "ada@home.example.org"),
            labeled("old", "a@example.org"),
            labeled("", "x@example.org"),
        ],
        phones: vec![
            labeled("mobile", "+44 20 7946 0001"),
            labeled("work", "+44 20 7946 0002"),
            labeled("fax", "+44 20 7946 0003"),
            labeled("boat", "0004"),
        ],
        addresses: vec![
            PostalAddress {
                label: "work".into(),
                street: "12 Mill Lane\nUnit 4".into(),
                city: "Leeds".into(),
                region: "Yorkshire".into(),
                postal_code: "LS1 1AA".into(),
                country: "United Kingdom".into(),
            },
            PostalAddress { label: "summer".into(), city: "Whitby".into(), ..Default::default() },
        ],
        websites: vec!["https://example.org/ada".into(), "http://example.org/".into()],
        birthday: Date::new(1815, 12, 10),
        notes: "First programmer.\n\nLikes looms.".into(),
        ..Default::default()
    }
}

#[test]
fn a_new_card_is_every_part_of_the_contact_in_the_address_book() {
    let card = Value::Object(new_card("B7", &ada()).unwrap());
    assert_eq!(
        card,
        json!({
            "@type": "Card", "version": "1.0", "addressBookIds": {"B7": true},
            "name": {"full": "Ada Example", "components": [
                {"kind": "given", "value": "Ada"}, {"kind": "given2", "value": "Maria"}, {"kind": "surname", "value": "Example"}]},
            "nicknames": {"n1": {"name": "Addie"}},
            "organizations": {"o1": {"name": "Engines Ltd", "units": [{"name": "Research"}]}},
            "titles": {"t1": {"name": "Analyst"}},
            "emails": {
                "e1": {"address": "ada@example.org", "contexts": {"work": true}, "pref": 1},
                "e2": {"address": "ada@home.example.org", "contexts": {"private": true}, "pref": 2},
                "e3": {"address": "a@example.org", "label": "old", "pref": 3},
                "e4": {"address": "x@example.org", "pref": 4},
            },
            "phones": {
                "p1": {"number": "+44 20 7946 0001", "features": {"mobile": true}, "pref": 1},
                "p2": {"number": "+44 20 7946 0002", "contexts": {"work": true}, "pref": 2},
                "p3": {"number": "+44 20 7946 0003", "features": {"fax": true}, "pref": 3},
                "p4": {"number": "0004", "label": "boat", "pref": 4},
            },
            "addresses": {
                "a1": {"components": [
                    {"kind": "name", "value": "12 Mill Lane\nUnit 4"}, {"kind": "locality", "value": "Leeds"}, {"kind": "region", "value": "Yorkshire"},
                    {"kind": "postcode", "value": "LS1 1AA"}, {"kind": "country", "value": "United Kingdom"}],
                    "contexts": {"work": true}, "pref": 1},
                "a2": {"components": [{"kind": "locality", "value": "Whitby"}], "label": "summer", "pref": 2},
            },
            "links": {"l1": {"uri": "https://example.org/ada", "pref": 1}, "l2": {"uri": "http://example.org/", "pref": 2}},
            "anniversaries": {"b1": {"kind": "birth", "date": {"year": 1815, "month": 12, "day": 10}}},
            "notes": {"n1": {"note": "First programmer.\n\nLikes looms."}},
        })
    );
    // And it reads back as the contact it was made of.
    assert_eq!(super::card::card("K1", &card).unwrap(), ada());
}

#[test]
fn a_contact_with_nothing_in_it_is_a_card_with_nothing_but_its_address_book() {
    let card = Value::Object(new_card("B7", &RemoteContact::default()).unwrap());
    assert_eq!(card, json!({"@type": "Card", "version": "1.0", "addressBookIds": {"B7": true}}));
    for field in ContactField::ALL {
        assert_eq!(written(field, &RemoteContact::default()), Ok(None), "{field:?}");
    }
    // What is only white space, and entries without a value, are nothing either.
    let blank = RemoteContact {
        display_name: " \t".into(),
        first: "\r\n".into(),
        nickname: " ".into(),
        company: "\0".into(),
        emails: vec![labeled("work", " ")],
        phones: vec![labeled("mobile", "")],
        addresses: vec![PostalAddress { label: "work".into(), street: "\n".into(), ..Default::default() }],
        websites: vec![" ".into()],
        notes: "\n \n".into(),
        ..Default::default()
    };
    assert_eq!(Value::Object(new_card("B7", &blank).unwrap()), card);
}

#[test]
fn each_part_is_one_property_of_the_card() {
    let names: Vec<&str> = ContactField::ALL.into_iter().map(property).collect();
    assert_eq!(names, ["name", "nicknames", "organizations", "titles", "emails", "phones", "addresses", "links", "anniversaries", "notes"]);
    let whole = new_card("B7", &ada()).unwrap();
    for field in ContactField::ALL {
        assert_eq!(written(field, &ada()).unwrap().as_ref(), whole.get(property(field)), "{field:?}");
    }
    // A name of some parts only, and an organisation of one.
    let of = |contact: RemoteContact, field| written(field, &contact).unwrap();
    assert_eq!(
        of(RemoteContact { last: "Example".into(), ..Default::default() }, ContactField::Name),
        Some(json!({"components": [{"kind": "surname", "value": "Example"}]}))
    );
    assert_eq!(of(RemoteContact { display_name: "Ada".into(), ..Default::default() }, ContactField::Name), Some(json!({"full": "Ada"})));
    assert_eq!(
        of(RemoteContact { department: "Looms".into(), ..Default::default() }, ContactField::Organization),
        Some(json!({"o1": {"units": [{"name": "Looms"}]}}))
    );
    assert_eq!(
        of(RemoteContact { company: "Engines".into(), ..Default::default() }, ContactField::Organization),
        Some(json!({"o1": {"name": "Engines"}}))
    );
}

#[test]
fn a_label_is_a_context_or_a_feature_only_when_it_is_exactly_one() {
    let contact = RemoteContact {
        emails: vec![labeled("Work", "a@example.org"), labeled("mobile", "b@example.org"), labeled("private", "c@example.org")],
        phones: vec![labeled("pager", "1"), labeled("home", "2"), labeled("Mobile", "3"), labeled("fax\n", "4")],
        ..Default::default()
    };
    assert_eq!(
        written(ContactField::Emails, &contact).unwrap(),
        Some(json!({
            "e1": {"address": "a@example.org", "label": "Work", "pref": 1},
            "e2": {"address": "b@example.org", "label": "mobile", "pref": 2},
            "e3": {"address": "c@example.org", "label": "private", "pref": 3},
        }))
    );
    assert_eq!(
        written(ContactField::Phones, &contact).unwrap(),
        Some(json!({
            "p1": {"number": "1", "features": {"pager": true}, "pref": 1},
            "p2": {"number": "2", "contexts": {"private": true}, "pref": 2},
            "p3": {"number": "3", "label": "Mobile", "pref": 3},
            "p4": {"number": "4", "features": {"fax": true}, "pref": 4},
        }))
    );
    // Either way the label comes back as it was.
    let read = card("K1", &Value::Object(new_card("B1", &contact).unwrap())).unwrap();
    assert_eq!(read.emails, contact.emails);
    assert_eq!(read.phones.iter().map(|phone| phone.label.as_str()).collect::<Vec<_>>(), ["pager", "home", "Mobile", "fax"]);
}

#[test]
fn what_is_written_is_cut_to_what_is_read_and_a_line_has_no_line_break() {
    assert_eq!(one_line(" Ada\r\nExample\0\x1b[1m ", 100), "Ada  Example  [1m");
    assert_eq!(one_line(&"é".repeat(600), 1_024).len(), 1_024);
    assert_eq!(one_line(&"éa".repeat(600), 1_024).len(), 1_023);
    assert_eq!(lines(" one\r\ntwo\0\tthree\x07\n ", 100), "one\ntwo\tthree");
    assert_eq!(lines(&"€".repeat(10_000), 16 << 10).len(), 16_383);

    let long = "x".repeat(5_000);
    let contact = RemoteContact {
        display_name: long.clone(),
        first: long.clone(),
        nickname: format!("Ad\ndie{long}"),
        emails: vec![labeled(&long, &long)],
        addresses: vec![PostalAddress { street: format!("1 Mill Lane\r\n{long}"), city: long.clone(), ..Default::default() }],
        notes: "n".repeat(100_000),
        ..Default::default()
    };
    let made = Value::Object(new_card("B1", &contact).unwrap());
    assert_eq!(made["name"]["full"].as_str().unwrap().len(), 1_024);
    assert_eq!(made["name"]["components"][0]["value"].as_str().unwrap().len(), 1_023);
    assert!(made["nicknames"]["n1"]["name"].as_str().unwrap().starts_with("Ad die"));
    assert_eq!(made["emails"]["e1"]["address"].as_str().unwrap().len(), 1_024);
    assert_eq!(made["emails"]["e1"]["label"].as_str().unwrap().len(), 1_024);
    let street = made["addresses"]["a1"]["components"][0]["value"].as_str().unwrap();
    assert!(street.starts_with("1 Mill Lane\nxxx") && street.len() == 1_023, "{}", street.len());
    assert_eq!(made["notes"]["n1"]["note"].as_str().unwrap().len(), 16 << 10);
    // What was cut is what is read: nothing is lost on the way back that was not cut on the way up.
    let read = card("K1", &made).unwrap();
    assert_eq!(read.first.len(), 1_023);
    assert_eq!(read.addresses[0].street, street);
    assert_eq!((read.addresses[0].city.len(), read.notes.len()), (1_023, 16 << 10));
    // No more entries than are read.
    let many = RemoteContact {
        emails: (0..80).map(|n| labeled("", &format!("p{n}@example.org"))).collect(),
        websites: (0..80).map(|n| format!("https://example.org/{n}")).collect(),
        addresses: (0..80).map(|n| PostalAddress { city: format!("Town {n}"), ..Default::default() }).collect(),
        ..Default::default()
    };
    let made = Value::Object(new_card("B1", &many).unwrap());
    for name in ["emails", "links", "addresses"] {
        assert_eq!(made[name].as_object().unwrap().len(), 50, "{name}");
    }
    assert_eq!(made["emails"]["e50"], json!({"address": "p49@example.org", "pref": 50}));
    let read = card("K1", &made).unwrap();
    assert_eq!(
        read.emails.iter().map(|email| email.value.clone()).collect::<Vec<_>>(),
        (0..50).map(|n| format!("p{n}@example.org")).collect::<Vec<_>>()
    );
    assert_eq!(read.addresses.last().unwrap().city, "Town 49");
}

#[test]
fn only_addresses_on_the_web_are_links_each_once() {
    let contact = RemoteContact {
        websites: vec![
            "https://example.org/a".into(),
            "example.org".into(),
            "ftp://example.org/".into(),
            "javascript:alert(1)".into(),
            "mailto:ada@example.org".into(),
            " HTTP://example.org/b ".into(),
            "https://example.org/a".into(),
            format!("https://example.org/{}", "x".repeat(2_100)),
            "https://example.org/c\nd".into(),
            "data:text/html,hello".into(),
        ],
        ..Default::default()
    };
    assert_eq!(
        written(ContactField::Websites, &contact).unwrap(),
        Some(json!({"l1": {"uri": "https://example.org/a", "pref": 1}, "l2": {"uri": "HTTP://example.org/b", "pref": 2}}))
    );
    let none = RemoteContact { websites: vec!["example.org".into()], ..Default::default() };
    assert_eq!(written(ContactField::Websites, &none), Ok(None));
}

#[test]
fn a_birthday_no_calendar_has_is_not_written() {
    let on = |year, month, day| RemoteContact { birthday: Some(Date { year, month, day }), ..Default::default() };
    assert_eq!(
        written(ContactField::Birthday, &on(2000, 2, 29)).unwrap(),
        Some(json!({"b1": {"kind": "birth", "date": {"year": 2000, "month": 2, "day": 29}}}))
    );
    for odd in [on(2001, 2, 29), on(-44, 3, 15), on(0, 1, 1), on(1990, 13, 1), on(1990, 0, 0), on(12_000, 1, 1)] {
        assert_eq!(written(ContactField::Birthday, &odd), Err(NOT_A_DAY), "{:?}", odd.birthday);
        assert_eq!(new_card("B1", &odd).err(), Some(NOT_A_DAY));
    }
}

/// A label: the ones that are written as contexts and features, their look-alikes, and any text.
fn any_label() -> impl Strategy<Value = String> {
    prop_oneof![
        proptest::sample::select(vec!["", "work", "home", "mobile", "fax", "pager", "Work", "private", "other", "billing"]).prop_map(str::to_string),
        "\\PC{0,12}",
    ]
}

fn any_text() -> impl Strategy<Value = String> {
    prop_oneof!["\\PC{0,40}", ".{0,40}", "[a-z \\n\\t]{0,60}"]
}

fn any_labeled(value: impl Strategy<Value = String>) -> impl Strategy<Value = Vec<Labeled>> {
    proptest::collection::vec((any_label(), value).prop_map(|(label, value)| Labeled { label, value }), 0..6)
}

/// Any contact, within what a contact may be once it is tidied.
fn any_contact() -> impl Strategy<Value = RemoteContact> {
    let names = proptest::collection::vec(any_text(), 8);
    let addresses = proptest::collection::vec((any_label(), proptest::collection::vec(any_text(), 5)), 0..4);
    let websites =
        proptest::collection::vec(prop_oneof!["https?://example\\.org/[a-z0-9/?=%.]{0,20}", "[a-z]{0,6}://[a-z.]{0,12}", any_text()], 0..5);
    let birthday = proptest::option::of((1i32..=9999, 1u8..=12, 1u8..=31));
    let emails = any_labeled("[a-z0-9.]{1,10}@[a-z]{1,8}\\.example\\.org");
    (names, emails, any_labeled(any_text()), addresses, websites, birthday, any_text()).prop_map(
        |(names, emails, phones, addresses, websites, birthday, notes)| {
            let mut names = names.into_iter();
            let mut name = || names.next().unwrap_or_default();
            RemoteContact {
                id: "K1".into(),
                display_name: name(),
                first: name(),
                middle: name(),
                last: name(),
                nickname: name(),
                company: name(),
                title: name(),
                department: name(),
                emails,
                phones,
                addresses: addresses
                    .into_iter()
                    .map(|(label, parts)| {
                        let mut parts = parts.into_iter();
                        let mut part = || parts.next().unwrap_or_default();
                        PostalAddress { label, street: part(), city: part(), region: part(), postal_code: part(), country: part() }
                    })
                    .collect(),
                websites,
                birthday: birthday.and_then(|(year, month, day)| Date::new(year, month, day)),
                notes,
                ..Default::default()
            }
        },
    )
}

/// What `contact` is expected to come back as: itself, but for the web addresses, of which only those of
/// the web go up, each once.
fn expected(contact: &RemoteContact) -> RemoteContact {
    let mut kept: Vec<String> = Vec::new();
    for site in &contact.websites {
        let lower = site.to_ascii_lowercase();
        if (lower.starts_with("http://") || lower.starts_with("https://")) && !kept.contains(site) {
            kept.push(site.clone());
        }
    }
    RemoteContact { websites: kept, ..contact.clone() }
}

/// Every text in `value`: its strings, and the keys of its maps.
fn strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(list) => list.iter().for_each(|entry| strings(entry, out)),
        Value::Object(map) => map.iter().for_each(|(key, entry)| {
            out.push(key);
            strings(entry, out);
        }),
        _ => {}
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    // The round trip: any contact, tidied as the engine keeps it, written as a new card and read back as a
    // server's card, is the same contact.
    #[test]
    fn any_contact_comes_back_from_its_card_as_it_went(contact in any_contact()) {
        let mut contact = contact;
        contact.tidy();
        let made = new_card("B1", &contact);
        prop_assert!(made.is_ok(), "{made:?}");
        let made = Value::Object(made.unwrap_or_default());
        // (As a server hands it back: with its id, and through text.)
        let sent: Value = serde_json::from_str(&serde_json::to_string(&made).unwrap()).unwrap();
        let read = card("K1", &sent);
        prop_assert!(read.is_some());
        let mut read = read.unwrap_or_default();
        read.tidy();
        prop_assert_eq!(&read, &expected(&contact));
        // And each part by itself is the part of the whole card: an update writes what a creation does.
        for field in ContactField::ALL {
            let part = written(field, &contact);
            prop_assert_eq!(part.as_ref().ok().and_then(Option::as_ref), made.get(property(field)));
            prop_assert_eq!(field.same(&read, &contact), field != ContactField::Websites || read.websites == contact.websites);
        }
    }

    // A contact that was never tidied (control characters, any day) is written within
    // bounds or refused, and never panics; no value of one line has a line break in it.
    #[test]
    fn any_contact_at_all_is_written_or_refused(contact in any_contact(), text in ".{0,80}", day in any::<(i32, u8, u8)>()) {
        let wild = RemoteContact {
            first: text.clone(),
            nickname: format!("{text}\r\n{text}"),
            birthday: Some(Date { year: day.0, month: day.1, day: day.2 }),
            emails: vec![Labeled { label: text.clone(), value: text.clone() }],
            websites: vec![format!("https://example.org/{text}")],
            ..contact.clone()
        };
        for contact in [contact, wild] {
            for field in ContactField::ALL {
                if let Ok(Some(value)) = written(field, &contact) {
                    prop_assert!(serde_json::to_string(&value).unwrap().len() <= 64 << 10);
                    let mut texts = Vec::new();
                    strings(&value, &mut texts);
                    let several = matches!(field, ContactField::Notes | ContactField::Addresses);
                    prop_assert!(texts.iter().all(|text| !text.chars().any(|c| c.is_control() && !(several && matches!(c, '\n' | '\t')))), "{:?}", texts);
                    prop_assert!(texts.iter().all(|text| text.trim() == *text));
                }
            }
            if let Ok(made) = new_card("B1", &contact) {
                prop_assert!(card("K1", &Value::Object(made)).is_some());
            }
        }
    }
}
