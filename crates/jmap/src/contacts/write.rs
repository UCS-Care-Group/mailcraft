// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A contact written as the properties of a JSContact card (RFC 9553): the other way of what `card` reads.
//!
//! A contact is ten parts ([`ContactField`]), and each part is one property of the card
//! ([`property`]) with one value ([`written`]): so a card that is made has every part that is not empty, and a
//! card that is changed has the parts that changed and nothing else. A part with nothing in it has no value
//! (`None`): it is left out of a new card, and `null` in a patch.
//!
//! What is written is what `card` reads back as the same contact: the entries of a map have short keys in
//! their order (`e1`, `e2`...) and say how much they are preferred, so that the order is the same after
//! reading; a label that is a context or a feature of the standard (`work`, `home` for `private`, and for a
//! number `mobile`, `fax`, `pager`) is written as that, any other as the entry's own label. Every text is cut
//! to what `card` keeps of it, and a value of one line has no line break and no other control character in
//! it. A contact's picture is no part: a card's `media` are never written.

use mailcraft_model::remote::{ContactField, RemoteContact};
use mailcraft_model::{Labeled, PostalAddress};
use mailcraft_time::Date;
use serde_json::{Map, Value, json};

use super::card::{MAX_ENTRIES, MAX_LINE, MAX_NOTE, MAX_URI, birth, postal};
use crate::reader::cut;

/// Longest component of a name or of a postal address that is written: `card` puts components together
/// into a line, and takes none that fills the line by itself.
const MAX_PART: usize = MAX_LINE.saturating_sub(1);

/// The keys of the entries that are the only one of their map, and what the keys of the others begin with.
pub(super) const NICKNAME: &str = "n1";
pub(super) const ORGANIZATION: &str = "o1";
pub(super) const TITLE: &str = "t1";
pub(super) const NOTE: &str = "n1";
/// (With a number after it: `b1`, unless the card has that key for something else.)
pub(super) const BIRTH: &str = "b";
const EMAIL: &str = "e";
const PHONE: &str = "p";
const ADDRESS: &str = "a";
const LINK: &str = "l";

/// Why a part can not be written.
pub(super) const NOT_A_DAY: &str = "the birthday is not a day a server keeps";

/// `text` as a value of one line: every control character a space, cut to `most` bytes.
pub(super) fn one_line(text: &str, most: usize) -> String {
    let flat: String = text.chars().take(most.saturating_add(1)).map(|c| if c.is_control() { ' ' } else { c }).collect();
    cut(&flat, most).to_string()
}

/// `text` as a value of several lines: its line ends are `\n`, it has no other control character but the
/// tabulator, and it is cut to `most` bytes.
pub(super) fn lines(text: &str, most: usize) -> String {
    let clean: String = text.chars().filter(|c| matches!(c, '\n' | '\t') || !c.is_control()).take(most.saturating_add(1)).collect();
    cut(&clean, most).to_string()
}

/// The property of a card that holds the part `field`.
pub(super) fn property(field: ContactField) -> &'static str {
    match field {
        ContactField::Name => "name",
        ContactField::Nickname => "nicknames",
        ContactField::Organization => "organizations",
        ContactField::Title => "titles",
        ContactField::Emails => "emails",
        ContactField::Phones => "phones",
        ContactField::Addresses => "addresses",
        ContactField::Websites => "links",
        ContactField::Birthday => "anniversaries",
        ContactField::Notes => "notes",
    }
}

/// The whole value of the property that holds the part `field` of `contact`: `None` when the contact has
/// nothing there. An error for a part that can not go to a server as it is.
pub(super) fn written(field: ContactField, contact: &RemoteContact) -> Result<Option<Value>, &'static str> {
    let only = |key: &str, entry: Option<Value>| entry.map(|entry| json!({key: entry}));
    Ok(match field {
        ContactField::Name => name(contact),
        ContactField::Nickname => only(NICKNAME, named(&contact.nickname)),
        ContactField::Organization => only(ORGANIZATION, organization(contact)),
        ContactField::Title => only(TITLE, named(&contact.title)),
        ContactField::Emails => labeled(&contact.emails, EMAIL, "address", false),
        ContactField::Phones => labeled(&contact.phones, PHONE, "number", true),
        ContactField::Addresses => addresses(&contact.addresses),
        ContactField::Websites => links(&contact.websites),
        ContactField::Birthday => only(&first(BIRTH), contact.birthday.map(anniversary).transpose()?),
        ContactField::Notes => only(NOTE, note(&contact.notes)),
    })
}

/// A card that is `contact`, new, in the address book `book`: every part of it that is not empty.
pub(super) fn new_card(book: &str, contact: &RemoteContact) -> Result<Map<String, Value>, &'static str> {
    let mut card = Map::new();
    card.insert("@type".into(), Value::from("Card"));
    card.insert("version".into(), Value::from("1.0"));
    card.insert("addressBookIds".into(), json!({book: true}));
    for field in ContactField::ALL {
        if let Some(value) = written(field, contact)? {
            card.insert(property(field).into(), value);
        }
    }
    Ok(card)
}

/// The first key that begins with `prefix`.
fn first(prefix: &str) -> String {
    format!("{prefix}1")
}

/// The key of the entry after `before` others.
fn key(prefix: &str, before: usize) -> String {
    format!("{prefix}{}", before.saturating_add(1))
}

/// An entry that is a name and nothing else (a nickname, a title).
fn named(text: &str) -> Option<Value> {
    let text = one_line(text, MAX_LINE);
    (!text.is_empty()).then(|| json!({"name": text}))
}

/// The name: the display name as the full one, and the given, the second given and the family names.
fn name(contact: &RemoteContact) -> Option<Value> {
    let full = one_line(&contact.display_name, MAX_LINE);
    let components: Vec<Value> = [("given", &contact.first), ("given2", &contact.middle), ("surname", &contact.last)]
        .into_iter()
        .map(|(kind, value)| (kind, one_line(value, MAX_PART)))
        .filter(|(_, value)| !value.is_empty())
        .map(|(kind, value)| json!({"kind": kind, "value": value}))
        .collect();
    let mut name = Map::new();
    if !full.is_empty() {
        name.insert("full".into(), Value::String(full));
    }
    if !components.is_empty() {
        name.insert("components".into(), Value::Array(components));
    }
    (!name.is_empty()).then_some(Value::Object(name))
}

/// The company, and the department as its one unit.
fn organization(contact: &RemoteContact) -> Option<Value> {
    let (company, department) = (one_line(&contact.company, MAX_LINE), one_line(&contact.department, MAX_LINE));
    let mut organization = Map::new();
    if !company.is_empty() {
        organization.insert("name".into(), Value::String(company));
    }
    if !department.is_empty() {
        organization.insert("units".into(), json!([{"name": department}]));
    }
    (!organization.is_empty()).then_some(Value::Object(organization))
}

/// Say on `entry` what it is labelled: as `card` reads a label, the other way.
fn label(entry: &mut Map<String, Value>, label: &str, phone: bool) {
    let label = one_line(label, MAX_LINE);
    match label.as_str() {
        "" => {}
        "work" => {
            entry.insert("contexts".into(), json!({"work": true}));
        }
        "home" => {
            entry.insert("contexts".into(), json!({"private": true}));
        }
        feature @ ("mobile" | "fax" | "pager") if phone => {
            entry.insert("features".into(), json!({feature: true}));
        }
        _ => {
            entry.insert("label".into(), Value::String(label));
        }
    }
}

/// How much the entry after `before` others is preferred: in their order (RFC 9553 §1.5.3, 1 to 100).
fn pref(before: usize) -> Value {
    Value::from(before.saturating_add(1).min(100))
}

/// E-mail addresses or numbers: each that is not empty, [`MAX_ENTRIES`] at the most, in their order.
fn labeled(list: &[Labeled], prefix: &str, value: &str, phone: bool) -> Option<Value> {
    let mut map = Map::new();
    let kept = list.iter().map(|entry| (entry, one_line(&entry.value, MAX_LINE))).filter(|(_, value)| !value.is_empty()).take(MAX_ENTRIES);
    for (before, (from, text)) in kept.enumerate() {
        let mut entry = Map::new();
        entry.insert(value.into(), Value::String(text));
        label(&mut entry, &from.label, phone);
        entry.insert("pref".into(), pref(before));
        map.insert(key(prefix, before), Value::Object(entry));
    }
    (!map.is_empty()).then_some(Value::Object(map))
}

/// One postal address: its parts as components (the street, with the number in it, as the one component
/// `name`), `None` when it has none.
fn address(from: &PostalAddress, before: usize) -> Option<Value> {
    let street = lines(&from.street, MAX_PART);
    let parts = [
        ("name", street.clone()),
        ("locality", one_line(&from.city, MAX_PART)),
        ("region", one_line(&from.region, MAX_PART)),
        ("postcode", one_line(&from.postal_code, MAX_PART)),
        ("country", one_line(&from.country, MAX_PART)),
    ];
    if parts.iter().all(|(_, value)| value.is_empty()) {
        return None;
    }
    let components = |with_street: bool| -> Value {
        parts
            .iter()
            .filter(|(kind, value)| !value.is_empty() && (with_street || *kind != "name"))
            .map(|(kind, value)| json!({"kind": kind, "value": value}))
            .collect()
    };
    let mut entry = Map::new();
    entry.insert("components".into(), components(true));
    label(&mut entry, &from.label, false);
    entry.insert("pref".into(), pref(before));
    // A street that would not come back as it is (its lines, say) goes as the address's one text instead.
    if postal(&entry).street != street {
        entry.insert("components".into(), components(false));
        entry.insert("full".into(), Value::String(street));
    }
    Some(Value::Object(entry))
}

fn addresses(list: &[PostalAddress]) -> Option<Value> {
    let mut map = Map::new();
    for from in list {
        if map.len() >= MAX_ENTRIES {
            break;
        }
        if let Some(entry) = address(from, map.len()) {
            map.insert(key(ADDRESS, map.len()), entry);
        }
    }
    (!map.is_empty()).then_some(Value::Object(map))
}

/// The web addresses, as links: `http` and `https` only, none that `card` would not read (too long, or with
/// a control character in it), each once.
fn links(list: &[String]) -> Option<Value> {
    let mut kept: Vec<&str> = Vec::new();
    for uri in list.iter().map(|uri| uri.trim()) {
        let web = ["http://", "https://"].iter().any(|scheme| uri.get(..scheme.len()).is_some_and(|start| start.eq_ignore_ascii_case(scheme)));
        if web && uri.len() <= MAX_URI && !uri.chars().any(char::is_control) && kept.len() < MAX_ENTRIES && !kept.contains(&uri) {
            kept.push(uri);
        }
    }
    let map: Map<String, Value> =
        kept.iter().enumerate().map(|(before, uri)| (key(LINK, before), json!({"uri": uri, "pref": pref(before)}))).collect();
    (!map.is_empty()).then_some(Value::Object(map))
}

/// Whether `list` holds a web address that [`links`] would leave out for what it is (not `http` or `https`,
/// too long, a control character in it): written without it, the owner's address would be lost at the next
/// read, so a change that holds one is refused and said.
pub(super) fn has_unkept_link(list: &[String]) -> bool {
    list.iter().map(|uri| uri.trim()).filter(|uri| !uri.is_empty()).any(|uri| {
        let web = ["http://", "https://"].iter().any(|scheme| uri.get(..scheme.len()).is_some_and(|start| start.eq_ignore_ascii_case(scheme)));
        !web || uri.len() > MAX_URI || uri.chars().any(char::is_control)
    })
}

/// The anniversary that is the day of birth `day`: an error for a day that `card` would not read back.
pub(super) fn anniversary(day: Date) -> Result<Value, &'static str> {
    let entry = json!({"kind": "birth", "date": {"year": day.year, "month": day.month, "day": day.day}});
    let mut card = Map::new();
    card.insert("anniversaries".into(), json!({first(BIRTH): entry.clone()}));
    if birth(&card).map(|(_, read)| read) == Some(day) { Ok(entry) } else { Err(NOT_A_DAY) }
}

/// The notes as they are written: all of them as one, cut to what `card` reads of a note.
pub(super) fn noted(text: &str) -> String {
    lines(text, MAX_NOTE)
}

/// The notes, as one note.
pub(super) fn note(text: &str) -> Option<Value> {
    let text = noted(text);
    (!text.is_empty()).then(|| json!({"note": text}))
}
