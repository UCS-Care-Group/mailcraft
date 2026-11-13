// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One JSContact card (RFC 9553), as a server sent it, made into the contact the engine takes in.
//!
//! Everything is read by name and by type: a property that is not what the standard says it is, is as good
//! as absent. Every map and every list is read for its first few entries only, every string is cut before
//! it is kept. A card's media (its picture, a logo, a sound) are not read at all, whether written into the
//! card, named as a blob or as an address on the web: nothing is ever fetched for a card.
//!
//! What is read here is what `write` writes: the bounds below and which entry a birthday is read from
//! ([`birth`]) are shared with it, so that a contact that went up comes back the same.

use mailcraft_model::remote::RemoteContact;
use mailcraft_model::{Labeled, PostalAddress};
use mailcraft_time::Date;
use serde_json::{Map, Value};

use crate::reader::cut;

/// Longest name, number, address or label kept, in bytes: the model cuts again, to less.
pub(super) const MAX_LINE: usize = 1_024;
/// Longest note kept of one entry, and of all of them together.
pub(super) const MAX_NOTE: usize = 16 << 10;
const MAX_NOTES: usize = 64 << 10;
/// Longest web address kept: a longer one is left out, since half an address is none.
pub(super) const MAX_URI: usize = 2_048;
/// Most entries of a map that are looked at, and most that are kept of them.
const MAX_SCAN: usize = 200;
pub(super) const MAX_ENTRIES: usize = 50;
/// Most components of a name or of a postal address that are read.
const MAX_PARTS: usize = 64;

/// The card `value`, which the server calls `id`, as a contact. None for what is no person and no
/// organisation (a group, a device, a location), and for what is no card at all.
pub(crate) fn card(id: &str, value: &Value) -> Option<RemoteContact> {
    let card = value.as_object()?;
    if !matches!(card.get("kind").and_then(Value::as_str), None | Some("individual" | "org")) {
        return None;
    }
    let mut contact = RemoteContact { id: id.to_string(), ..RemoteContact::default() };
    name(card, &mut contact);
    contact.nickname = entries(card.get("nicknames")).first().map(|(_, nick)| line(nick.get("name"))).unwrap_or_default();
    if let Some((_, organization)) = entries(card.get("organizations")).first() {
        contact.company = line(organization.get("name"));
        let units = organization.get("units").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        contact.department = units.iter().take(MAX_PARTS).map(|unit| line(unit.get("name"))).find(|unit| !unit.is_empty()).unwrap_or_default();
    }
    contact.title = entries(card.get("titles")).first().map(|(_, title)| line(title.get("name"))).unwrap_or_default();
    contact.emails = entries(card.get("emails"))
        .into_iter()
        .map(|(_, email)| Labeled { label: label(email, false), value: line(email.get("address")) })
        .filter(|email| !email.value.is_empty())
        .collect();
    contact.phones = entries(card.get("phones"))
        .into_iter()
        .map(|(_, phone)| Labeled { label: label(phone, true), value: line(phone.get("number")) })
        .filter(|phone| !phone.value.is_empty())
        .collect();
    contact.addresses = entries(card.get("addresses")).into_iter().map(|(_, address)| postal(address)).filter(|a| !a.is_empty()).collect();
    contact.websites = websites(card);
    contact.birthday = birth(card).map(|(_, day)| day);
    contact.notes = notes(card.get("notes"));
    contact.modified =
        card.get("updated").and_then(Value::as_str).filter(|at| at.len() <= 40).and_then(|at| mailcraft_time::iso_to_timestamp(at, 0)).unwrap_or(0);
    Some(contact)
}

/// A property that is text, cut to a line's length; empty when it is anything else.
fn line(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).map(|text| cut(text, MAX_LINE).to_string()).unwrap_or_default()
}

/// How much an entry is preferred: 1 the most, 100 the least (RFC 9553 §1.5.4), and after them those that
/// do not say.
fn pref(entry: &Map<String, Value>) -> u64 {
    entry.get("pref").and_then(Value::as_u64).filter(|pref| (1..=100).contains(pref)).unwrap_or(u64::MAX)
}

/// The entries of a map of objects (an `Id[...]` of the standard), the preferred first and then by their
/// keys, so that the order is the same whatever order the server wrote them in. Of a map with more than
/// [`MAX_SCAN`] entries the rest is not looked at, and [`MAX_ENTRIES`] are kept.
fn entries(value: Option<&Value>) -> Vec<(&str, &Map<String, Value>)> {
    let Some(map) = value.and_then(Value::as_object) else { return Vec::new() };
    let mut list: Vec<(u64, &str, &Map<String, Value>)> =
        map.iter().take(MAX_SCAN).filter_map(|(key, entry)| entry.as_object().map(|entry| (pref(entry), key.as_str(), entry))).collect();
    list.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    list.into_iter().take(MAX_ENTRIES).map(|(_, key, entry)| (key, entry)).collect()
}

/// Whether the set `name` of an entry (its `contexts`, its `features`) has `member`.
fn has(entry: &Map<String, Value>, name: &str, member: &str) -> bool {
    entry.get(name).and_then(|set| set.get(member)).and_then(Value::as_bool) == Some(true)
}

/// What an entry is labelled: its own label, else (for a phone) what kind of line it is, else where it is
/// used. `private` is the standard's word for what the address book calls home.
fn label(entry: &Map<String, Value>, phone: bool) -> String {
    let own = line(entry.get("label"));
    if !own.is_empty() {
        return own;
    }
    if phone && let Some(feature) = ["mobile", "fax", "pager"].into_iter().find(|feature| has(entry, "features", feature)) {
        return feature.to_string();
    }
    if has(entry, "contexts", "work") {
        return "work".to_string();
    }
    if has(entry, "contexts", "private") {
        return "home".to_string();
    }
    // Another context (`billing`, `delivery`), the first in the order of their names.
    let contexts = entry.get("contexts").and_then(Value::as_object);
    let mut others: Vec<&str> = contexts
        .into_iter()
        .flat_map(|contexts| contexts.iter().take(MAX_PARTS))
        .filter(|(_, on)| on.as_bool() == Some(true))
        .map(|(context, _)| context.as_str())
        .collect();
    others.sort_unstable();
    others.first().map(|context| cut(context, 32).to_string()).unwrap_or_default()
}

/// Add `part` to `whole` after a space, unless that would make it longer than a line.
fn append(whole: &mut String, part: &str) {
    let part = cut(part, MAX_LINE);
    if part.is_empty() || whole.len().saturating_add(part.len()) >= MAX_LINE {
        return;
    }
    if !whole.is_empty() {
        whole.push(' ');
    }
    whole.push_str(part);
}

/// The components of a name or of an address: the kind and the value of each that has both, the first
/// [`MAX_PARTS`] of them.
fn components(of: &Map<String, Value>) -> Vec<(&str, &str)> {
    let list = of.get("components").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    list.iter()
        .take(MAX_PARTS)
        .filter_map(|part| Some((part.get("kind").and_then(Value::as_str)?, part.get("value").and_then(Value::as_str)?)))
        .collect()
}

/// The name: the full one as the display name (left empty when the card has none: the engine makes one of
/// the parts), and the given, the second given and the family names.
fn name(card: &Map<String, Value>, contact: &mut RemoteContact) {
    let Some(name) = card.get("name").and_then(Value::as_object) else { return };
    contact.display_name = line(name.get("full"));
    let parts = components(name);
    for (kind, value) in &parts {
        match *kind {
            "given" => append(&mut contact.first, value),
            "given2" => append(&mut contact.middle, value),
            "surname" => append(&mut contact.last, value),
            _ => {}
        }
    }
    // A second family name comes after the first, wherever the card wrote it.
    for (_, value) in parts.iter().filter(|(kind, _)| *kind == "surname2") {
        append(&mut contact.last, value);
    }
}

/// A postal address, from its components, or from the one text of it when it has none that are used.
pub(super) fn postal(address: &Map<String, Value>) -> PostalAddress {
    let parts = components(address);
    let of = |kind: &str| {
        let mut whole = String::new();
        for (_, value) in parts.iter().filter(|(k, _)| *k == kind) {
            append(&mut whole, value);
        }
        whole
    };
    // The street: its name and the number in it, in the card's order when the card says its order means
    // something, else the name first.
    let mut street = String::new();
    if address.get("isOrdered").and_then(Value::as_bool) == Some(true) {
        for (_, value) in parts.iter().filter(|(kind, _)| matches!(*kind, "number" | "name" | "direction")) {
            append(&mut street, value);
        }
    } else {
        for kind in ["name", "number", "direction"] {
            append(&mut street, &of(kind));
        }
    }
    // Then what says where in the street: the building, the flat, the floor, the room, a box.
    for (_, value) in parts.iter().filter(|(kind, _)| matches!(*kind, "block" | "building" | "apartment" | "floor" | "room" | "postOfficeBox")) {
        let value = cut(value, MAX_LINE);
        if !value.is_empty() && street.len().saturating_add(value.len()).saturating_add(2) < MAX_LINE {
            if !street.is_empty() {
                street.push_str(", ");
            }
            street.push_str(value);
        }
    }
    let mut out = PostalAddress {
        label: label(address, false),
        street,
        city: of("locality"),
        region: of("region"),
        postal_code: of("postcode"),
        country: of("country"),
    };
    if out.is_empty() {
        out.street = line(address.get("full"));
    }
    out
}

/// The web addresses of the card's links and of its online services: `http` and `https` only, each once.
fn websites(card: &Map<String, Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, entry) in entries(card.get("links")).into_iter().chain(entries(card.get("onlineServices"))) {
        let Some(uri) = entry.get("uri").and_then(Value::as_str).map(str::trim).filter(|uri| uri.len() <= MAX_URI) else { continue };
        let web = ["http://", "https://"].iter().any(|scheme| uri.get(..scheme.len()).is_some_and(|start| start.eq_ignore_ascii_case(scheme)));
        if web && out.len() < MAX_ENTRIES && !out.iter().any(|have| have == uri) {
            out.push(uri.to_string());
        }
    }
    out
}

/// The day of birth, and the key of the anniversary it is read from: the first anniversary of that kind that
/// names a whole day, as a date of year, month and day or as a moment in UTC.
pub(super) fn birth(card: &Map<String, Value>) -> Option<(&str, Date)> {
    entries(card.get("anniversaries"))
        .into_iter()
        .filter(|(_, anniversary)| anniversary.get("kind").and_then(Value::as_str) == Some("birth"))
        .find_map(|(key, anniversary)| day_of(anniversary).map(|day| (key, day)))
}

/// The day an anniversary names, when it names a whole one.
fn day_of(anniversary: &Map<String, Value>) -> Option<Date> {
    let date = anniversary.get("date")?.as_object()?;
    if let Some(utc) = date.get("utc").and_then(Value::as_str) {
        return utc.get(..10).and_then(Date::parse_iso);
    }
    let number = |name: &str| date.get(name).and_then(Value::as_u64);
    let year = i32::try_from(number("year")?).ok()?;
    Date::new(year, u8::try_from(number("month")?).ok()?, u8::try_from(number("day")?).ok()?)
}

/// The notes, one after the other with an empty line between.
pub(super) fn notes(value: Option<&Value>) -> String {
    let mut out = String::new();
    for (_, entry) in entries(value) {
        let note = entry.get("note").and_then(Value::as_str).map(|note| cut(note, MAX_NOTE)).unwrap_or_default();
        if note.is_empty() || out.len().saturating_add(note.len()).saturating_add(2) > MAX_NOTES {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(note);
    }
    out
}
