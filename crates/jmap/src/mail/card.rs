// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A contact card (JSContact, RFC 9553, as JMAP for Contacts keeps it, RFC 9610 §3) and MailCraft's
//! contact, both ways.
//!
//! - **Read** ([`read`]): what a contact shows (names, organisation, job title, e-mail addresses, phone
//!   numbers, postal addresses, web pages, birthday, notes, categories) from the card's properties. A
//!   group (`kind` `group`) is not a contact and is passed over. Every list is cut to [`MAX_ENTRIES`];
//!   the rest is cut to size by the model ([`RemoteContact::tidy`]).
//! - **Made** ([`made`]): a whole card for a new contact, in the address book given.
//! - **Changed** ([`patch`]): a patch (RFC 8620 §5.3) of only the properties of the fields the owner
//!   changed, each worked out from the server's card as it is now, so that what the card holds and
//!   MailCraft doesn't show (a name's title, an address's extra parts, an entry's preference, other
//!   notes' authors, members no field names) is kept: an entry the contact still has keeps its key and
//!   everything else of it.

use mailcraft_model::Labeled;
use mailcraft_model::PostalAddress;
use mailcraft_model::contact_sync::ContactField;
use mailcraft_model::remote::RemoteContact;
use mailcraft_time::Date;
use serde_json::{Map, Value, json};

use crate::Id;

/// Most entries of one of a card's lists that are read, and most address books a card is read in.
pub(super) const MAX_ENTRIES: usize = 64;

/// The properties of a card that are read.
pub(super) const CARD_PROPERTIES: [&str; 15] = [
    "id",
    "addressBookIds",
    "kind",
    "name",
    "nicknames",
    "organizations",
    "titles",
    "emails",
    "phones",
    "addresses",
    "links",
    "anniversaries",
    "notes",
    "keywords",
    "uid",
];

/// The entries of a card's map property (`emails`, `phones`…), in the order a person reads them: by
/// preference (`pref`, 1 first; none last), then by key.
fn entries<'a>(card: &'a Value, property: &str) -> Vec<(&'a String, &'a Map<String, Value>)> {
    let Some(map) = card.get(property).and_then(Value::as_object) else { return Vec::new() };
    let mut list: Vec<(&String, &Map<String, Value>)> =
        map.iter().filter_map(|(key, entry)| Some((key, entry.as_object()?))).take(MAX_ENTRIES).collect();
    let pref = |entry: &Map<String, Value>| entry.get("pref").and_then(Value::as_u64).unwrap_or(u64::MAX);
    list.sort_by(|a, b| pref(a.1).cmp(&pref(b.1)).then_with(|| a.0.cmp(b.0)));
    list
}

fn text<'a>(entry: &'a Map<String, Value>, member: &str) -> &'a str {
    entry.get(member).and_then(Value::as_str).unwrap_or("")
}

fn has(entry: &Map<String, Value>, set: &str, name: &str) -> bool {
    entry.get(set).and_then(|s| s.get(name)).and_then(Value::as_bool).unwrap_or(false)
}

/// The label MailCraft gives an entry: a phone's kind (`mobile`, `fax`), else its context (`work`,
/// `home` for `private`), else the card's own label, else none.
fn label_of(entry: &Map<String, Value>) -> String {
    let label = if has(entry, "features", "mobile") {
        "mobile"
    } else if has(entry, "features", "fax") {
        "fax"
    } else if has(entry, "contexts", "work") {
        "work"
    } else if has(entry, "contexts", "private") {
        "home"
    } else {
        text(entry, "label")
    };
    label.to_string()
}

/// `entry` with what says `label` set as a card says it, and every other word MailCraft labels with
/// taken away; the contexts and features it doesn't label with (`billing`, `voice`…) stay.
fn labelled(mut entry: Map<String, Value>, label: &str, phone: bool) -> Map<String, Value> {
    let set = |entry: &mut Map<String, Value>, member: &str, take: &[&str], put: Option<&str>| {
        let mut words = entry.get(member).and_then(Value::as_object).cloned().unwrap_or_default();
        for word in take {
            words.remove(*word);
        }
        if let Some(word) = put {
            words.insert(word.to_string(), Value::Bool(true));
        }
        if words.is_empty() {
            entry.remove(member);
        } else {
            entry.insert(member.to_string(), Value::Object(words));
        }
    };
    let low = label.trim().to_ascii_lowercase();
    let context = match low.as_str() {
        "work" | "business" => Some("work"),
        "home" | "private" => Some("private"),
        _ => None,
    };
    let feature = match low.as_str() {
        "mobile" | "cell" if phone => Some("mobile"),
        "fax" if phone => Some("fax"),
        _ => None,
    };
    set(&mut entry, "contexts", &["work", "private"], context);
    if phone {
        set(&mut entry, "features", &["mobile", "fax"], feature);
    }
    entry.remove("label");
    if !low.is_empty() && context.is_none() && feature.is_none() {
        entry.insert("label".into(), Value::String(label.trim().to_string()));
    }
    entry
}

/// A postal address of a card as MailCraft keeps one: the street from its number and street name
/// (or its whole text when it has no parts), the town, region, postcode and country.
fn address_of(entry: &Map<String, Value>) -> PostalAddress {
    let mut street: Vec<&str> = Vec::new();
    let mut a = PostalAddress { label: label_of(entry), ..Default::default() };
    for part in entry.get("components").and_then(Value::as_array).into_iter().flatten().take(MAX_ENTRIES) {
        let value = part.get("value").and_then(Value::as_str).unwrap_or("").trim();
        match part.get("kind").and_then(Value::as_str).unwrap_or("") {
            "number" | "name" => street.push(value),
            "locality" => a.city = value.to_string(),
            "region" => a.region = value.to_string(),
            "postcode" => a.postal_code = value.to_string(),
            "country" => a.country = value.to_string(),
            _ => {}
        }
    }
    a.street = street.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    if a.street.is_empty() && a.city.is_empty() && a.region.is_empty() && a.postal_code.is_empty() && a.country.is_empty() {
        a.street = text(entry, "full").trim().to_string();
    }
    a
}

/// The birthday of a card: a whole date (a year with no day, or a day with no year, is none).
fn birthday_of(card: &Value) -> Option<Date> {
    entries(card, "anniversaries").into_iter().filter(|(_, a)| text(a, "kind") == "birth").find_map(|(_, a)| {
        let date = a.get("date")?;
        if let Some(utc) = date.get("utc").and_then(Value::as_str) {
            let (y, m, d) = (utc.get(0..4)?.parse().ok()?, utc.get(5..7)?.parse().ok()?, utc.get(8..10)?.parse().ok()?);
            return Date::new(y, m, d);
        }
        let number = |name: &str| date.get(name).and_then(Value::as_u64);
        Date::new(i32::try_from(number("year")?).ok()?, u8::try_from(number("month")?).ok()?, u8::try_from(number("day")?).ok()?)
    })
}

/// The first organisation of a card (by key): its key and entry.
fn organisation(card: &Value) -> Option<(&String, &Map<String, Value>)> {
    entries(card, "organizations").into_iter().next()
}

/// The first job title of a card that is not a role (by preference, then key).
fn job_title(card: &Value) -> Option<(&String, &Map<String, Value>)> {
    entries(card, "titles").into_iter().find(|(_, t)| text(t, "kind") != "role")
}

/// The notes of a card that are read: those with text, [`MAX_ENTRIES`] at the most, by key.
fn shown_notes(card: &Value) -> Vec<(&String, &str)> {
    let mut notes: Vec<(&String, &str)> = card
        .get("notes")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, n)| Some((key, n.get("note")?.as_str()?.trim())))
        .filter(|(_, n)| !n.is_empty())
        .take(MAX_ENTRIES)
        .collect();
    notes.sort_by(|a, b| a.0.cmp(b.0));
    notes
}

/// The notes of a card, as one text: each note, by key, a blank line between them.
fn notes_of(card: &Value) -> String {
    shown_notes(card).into_iter().map(|(_, n)| n).collect::<Vec<_>>().join("\n\n")
}

/// The address books a card is in (the ids the server gave). Not capped: a book left out would read
/// as one the card left (the answer's size bounds it).
pub(super) fn books_of(card: &Value) -> Vec<String> {
    card.get("addressBookIds")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(id, on)| **on == Value::Bool(true) && Id::new(id).is_ok())
        .map(|(id, _)| id.clone())
        .collect()
}

/// The card's id, when it is one a server hands out.
pub(super) fn id_of(card: &Value) -> Option<String> {
    card.get("id").and_then(Value::as_str).filter(|id| Id::new(id).is_ok()).map(str::to_string)
}

/// The contact a card is; `None` for one with no id that can be read, and for a group (which is no
/// contact: spec, contact groups are not read).
pub(super) fn read(card: &Value) -> Option<RemoteContact> {
    let id = id_of(card)?;
    if card.get("kind").and_then(Value::as_str) == Some("group") {
        return None;
    }
    Some(RemoteContact { id, ..fields_of(card) })
}

/// What a contact shows of a card, whatever its id and kind.
fn fields_of(card: &Value) -> RemoteContact {
    let mut c = RemoteContact::default();
    if let Some(name) = card.get("name").and_then(Value::as_object) {
        c.display_name = text(name, "full").to_string();
        let parts = name.get("components").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let part = |kind: &str| {
            parts
                .iter()
                .take(MAX_ENTRIES)
                .filter(|p| p.get("kind").and_then(Value::as_str) == Some(kind))
                .filter_map(|p| p.get("value").and_then(Value::as_str))
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        };
        (c.first, c.middle, c.last) = (part("given"), part("given2"), part("surname"));
    }
    c.nickname = entries(card, "nicknames").into_iter().map(|(_, n)| text(n, "name").trim()).find(|n| !n.is_empty()).unwrap_or("").to_string();
    if let Some((_, org)) = organisation(card) {
        c.company = text(org, "name").to_string();
        c.department = org
            .get("units")
            .and_then(Value::as_array)
            .and_then(|u| u.first())
            .and_then(|u| u.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
    }
    c.title = job_title(card).map(|(_, t)| text(t, "name").to_string()).unwrap_or_default();
    c.emails = entries(card, "emails").into_iter().map(|(_, e)| Labeled::new(label_of(e), text(e, "address").trim())).collect();
    c.phones = entries(card, "phones").into_iter().map(|(_, p)| Labeled::new(label_of(p), text(p, "number").trim())).collect();
    c.addresses = entries(card, "addresses").into_iter().map(|(_, a)| address_of(a)).filter(|a| !a.is_empty()).collect();
    c.websites = entries(card, "links").into_iter().map(|(_, l)| text(l, "uri").trim().to_string()).filter(|u| !u.is_empty()).collect();
    c.birthday = birthday_of(card);
    c.notes = notes_of(card);
    let mut keywords: Vec<String> = card
        .get("keywords")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, on)| **on == Value::Bool(true))
        .map(|(k, _)| k.clone())
        .take(MAX_ENTRIES)
        .collect();
    keywords.sort();
    c.categories = Some(keywords);
    c
}

/// A key for a new entry of `map` (`<prefix><n>`) that no entry has.
fn fresh_key(map: &Map<String, Value>, prefix: &str) -> String {
    let mut n = map.len().saturating_add(1);
    loop {
        let key = format!("{prefix}{n}");
        if !map.contains_key(&key) {
            return key;
        }
        n = n.saturating_add(1);
    }
}

/// A map property rebuilt from `wanted`: an entry the server has that still reads as one wanted is
/// kept whole under its key (each once); then one with the same value (`same`) is given the wanted
/// label by `make`, under its key; with `pair`, one still left is paired with the next server entry
/// still left, in order, and `make` changes that one (an edit keeps what MailCraft doesn't show of
/// it); a wanted one with none is made by `make` under a new key. Entries no longer wanted go; those
/// past what was read ([`MAX_ENTRIES`]) were never shown, and stay as they are.
#[allow(clippy::too_many_arguments)]
fn rebuilt<T: PartialEq>(
    server: &Value,
    property: &str,
    prefix: &str,
    wanted: &[T],
    pair: bool,
    read: impl Fn(&Map<String, Value>) -> T,
    same: impl Fn(&Map<String, Value>, &T) -> bool,
    make: impl Fn(Option<&Map<String, Value>>, &T) -> Map<String, Value>,
) -> Value {
    let had = entries(server, property);
    let mut used = vec![false; had.len()];
    let mut slots: Vec<Option<usize>> = vec![None; wanted.len()];
    for pass in 0..3 {
        if pass == 2 && !pair {
            break;
        }
        for (slot, want) in slots.iter_mut().zip(wanted) {
            if slot.is_some() {
                continue;
            }
            let fits = |e: &Map<String, Value>| match pass {
                0 => read(e) == *want,
                1 => same(e, want),
                _ => true,
            };
            if let Some(i) = had.iter().enumerate().position(|(i, (_, e))| !used.get(i).copied().unwrap_or(true) && fits(e)) {
                if let Some(u) = used.get_mut(i) {
                    *u = true;
                }
                *slot = Some(i);
            }
        }
    }
    // New keys stay clear of those the server had.
    let mut taken: Map<String, Value> = had.iter().map(|(key, _)| ((*key).clone(), Value::Null)).collect();
    let mut out = Map::new();
    if let Some(all) = server.get(property).and_then(Value::as_object) {
        for (key, entry) in all.iter().filter(|(_, e)| e.is_object()) {
            if !had.iter().any(|(k, _)| *k == key) {
                taken.insert(key.clone(), Value::Null);
                out.insert(key.clone(), entry.clone());
            }
        }
    }
    for (slot, want) in slots.into_iter().zip(wanted) {
        match slot.and_then(|i| had.get(i)) {
            Some((key, e)) if read(e) == *want => out.insert((*key).clone(), Value::Object((*e).clone())),
            Some((key, e)) => out.insert((*key).clone(), Value::Object(make(Some(e), want))),
            None => {
                let key = fresh_key(&taken, prefix);
                taken.insert(key.clone(), Value::Null);
                out.insert(key, Value::Object(make(None, want)))
            }
        };
    }
    if out.is_empty() { Value::Null } else { Value::Object(out) }
}

/// An e-mail address or a phone number as a card writes it: `entry` (the server's, when it has the
/// same value) with `want`'s label, else a new one.
fn labelled_value(entry: Option<&Map<String, Value>>, member: &str, want: &Labeled, phone: bool) -> Map<String, Value> {
    let entry = entry.cloned().unwrap_or_else(|| Map::from_iter([(member.to_string(), Value::String(want.value.trim().to_string()))]));
    labelled(entry, &want.label, phone)
}

/// A postal address as a card writes one.
fn address_entry(a: &PostalAddress) -> Map<String, Value> {
    let parts: Vec<Value> =
        [("name", &a.street), ("locality", &a.city), ("region", &a.region), ("postcode", &a.postal_code), ("country", &a.country)]
            .into_iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(kind, v)| json!({"kind": kind, "value": v.trim()}))
            .collect();
    labelled(Map::from_iter([("components".to_string(), Value::Array(parts))]), &a.label, false)
}

/// The server's postal address `entry` changed to say `a`: the parts MailCraft shows replaced (the
/// street as one `name` part, in place of the number and street name), every other part (an
/// apartment, a district) and member (its coordinates, its time zone) kept; its whole text goes, as
/// it no longer says the same.
fn address_edited(entry: &Map<String, Value>, a: &PostalAddress) -> Map<String, Value> {
    const SHOWN: [&str; 6] = ["number", "name", "locality", "region", "postcode", "country"];
    let mut out = entry.clone();
    let kept = entry
        .get("components")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|p| p.get("kind").and_then(Value::as_str).is_none_or(|k| !SHOWN.contains(&k) && k != "separator"));
    let fresh = address_entry(a);
    let mut parts: Vec<Value> = fresh.get("components").and_then(Value::as_array).cloned().unwrap_or_default();
    parts.extend(kept.cloned());
    for member in ["full", "isOrdered", "defaultSeparator"] {
        out.remove(member);
    }
    out.insert("components".into(), Value::Array(parts));
    labelled(out, &a.label, false)
}

fn birthday_entry(d: Date) -> Value {
    json!({"kind": "birth", "date": {"@type": "PartialDate", "year": d.year, "month": d.month, "day": d.day}})
}

/// The `name` property: `server`'s name (none for a new card) with the full name from `values` when
/// `full`, and those of its given, middle and surname parts named in `parts` from `values`; the other
/// members and parts (a title, a suffix, a part not named) stay. `null` when nothing is left.
fn name_value(server: Option<&Map<String, Value>>, values: &RemoteContact, full: bool, parts: &[&str]) -> Value {
    let mut name = server.cloned().unwrap_or_default();
    if full {
        match values.display_name.trim() {
            "" => name.remove("full"),
            shown => name.insert("full".into(), Value::String(shown.to_string())),
        };
    }
    if !parts.is_empty() {
        // (Separators belong to the parts' order, which changes: they go.)
        let mut components: Vec<Value> = name
            .get("components")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|p| p.get("kind").and_then(Value::as_str).is_some_and(|k| !parts.contains(&k) && k != "separator"))
            .cloned()
            .collect();
        for (kind, value) in [("given", &values.first), ("given2", &values.middle), ("surname", &values.last)] {
            if parts.contains(&kind) && !value.trim().is_empty() {
                components.push(json!({"kind": kind, "value": value.trim()}));
            }
        }
        name.remove("isOrdered");
        name.remove("defaultSeparator");
        if components.is_empty() {
            name.remove("components");
        } else {
            name.insert("components".into(), Value::Array(components));
        }
    }
    if name.is_empty() { Value::Null } else { Value::Object(name) }
}

/// A whole card for a new contact `values`, in the address book `book`, under `uid`.
pub(super) fn made(values: &RemoteContact, book: &str, uid: &str) -> Map<String, Value> {
    let empty = json!({});
    let mut card = Map::new();
    card.insert("@type".into(), json!("Card"));
    card.insert("version".into(), json!("1.0"));
    card.insert("uid".into(), json!(uid));
    card.insert("kind".into(), json!("individual"));
    card.insert("addressBookIds".into(), json!({book: true}));
    for (property, value) in patch(&empty, values, &ContactField::ALL.into_iter().collect()) {
        if !value.is_null() {
            card.insert(property, value);
        }
    }
    card
}

/// The patch that writes the fields `fields` of `values` to the card `server` (see the module): one
/// entry for each property those fields are kept in, each property whole; none for a field whose
/// property already says what `values` does.
pub(super) fn patch(server: &Value, values: &RemoteContact, fields: &std::collections::BTreeSet<ContactField>) -> Map<String, Value> {
    use ContactField as F;
    let mut out = Map::new();
    let wants = |f: F| fields.contains(&f);
    let now = fields_of(server);
    let full = wants(F::DisplayName) && now.display_name.trim() != values.display_name.trim();
    let parts: Vec<&str> = [
        (F::First, "given", &now.first, &values.first),
        (F::Middle, "given2", &now.middle, &values.middle),
        (F::Last, "surname", &now.last, &values.last),
    ]
    .into_iter()
    .filter(|(f, _, was, is)| wants(*f) && was.trim() != is.trim())
    .map(|(_, kind, ..)| kind)
    .collect();
    let name = server.get("name").and_then(Value::as_object);
    match (full, parts.is_empty(), name) {
        // The full name alone, on a card that has a name: that member only, so that nothing else
        // of the name (its parts, as another device may just have changed them) is written.
        (true, true, Some(n)) => {
            let shown = values.display_name.trim();
            let parted = n.get("components").and_then(Value::as_array).is_some_and(|c| !c.is_empty());
            match (shown, parted) {
                // (A name is a full name or parts, RFC 9553 §2.2.1.2: with neither, it goes.)
                ("", false) => out.insert("name".into(), Value::Null),
                ("", true) => out.insert("name/full".into(), Value::Null),
                (shown, _) => out.insert("name/full".into(), json!(shown)),
            };
        }
        (false, true, _) => {}
        _ => {
            out.insert("name".into(), name_value(name, values, full, &parts));
        }
    }
    if wants(F::Nickname) && now.nickname.trim() != values.nickname.trim() {
        let mut map = server.get("nicknames").and_then(Value::as_object).cloned().unwrap_or_default();
        let first = entries(server, "nicknames").into_iter().next().map(|(k, _)| k.clone());
        match (first, values.nickname.trim()) {
            (Some(key), "") => {
                map.remove(&key);
            }
            (Some(key), nick) => {
                if let Some(Value::Object(entry)) = map.get_mut(&key) {
                    entry.insert("name".into(), json!(nick));
                }
            }
            (None, "") => {}
            (None, nick) => {
                let key = fresh_key(&map, "k");
                map.insert(key, json!({"name": nick}));
            }
        }
        out.insert("nicknames".into(), if map.is_empty() { Value::Null } else { Value::Object(map) });
    }
    let company = wants(F::Company) && now.company.trim() != values.company.trim();
    let department = wants(F::Department) && now.department.trim() != values.department.trim();
    if company || department {
        let mut map = server.get("organizations").and_then(Value::as_object).cloned().unwrap_or_default();
        let key = organisation(server).map(|(k, _)| k.clone()).unwrap_or_else(|| fresh_key(&map, "o"));
        let mut org = map.get(&key).and_then(Value::as_object).cloned().unwrap_or_default();
        if company {
            match values.company.trim() {
                "" => org.remove("name"),
                name => org.insert("name".into(), json!(name)),
            };
        }
        if department {
            let mut units: Vec<Value> = org.get("units").and_then(Value::as_array).cloned().unwrap_or_default();
            match (values.department.trim(), units.is_empty()) {
                ("", false) => {
                    units.remove(0);
                }
                ("", true) => {}
                (dept, true) => units.push(json!({"name": dept})),
                (dept, false) => {
                    if let Some(Value::Object(unit)) = units.first_mut() {
                        unit.insert("name".into(), json!(dept));
                    }
                }
            }
            if units.is_empty() {
                org.remove("units");
            } else {
                org.insert("units".into(), Value::Array(units));
            }
        }
        // (An organisation is a name or units, RFC 9553 §2.2.3: with neither, it goes.)
        if !org.contains_key("name") && !org.contains_key("units") {
            map.remove(&key);
        } else {
            map.insert(key, Value::Object(org));
        }
        out.insert("organizations".into(), if map.is_empty() { Value::Null } else { Value::Object(map) });
    }
    if wants(F::Title) && now.title.trim() != values.title.trim() {
        let mut map = server.get("titles").and_then(Value::as_object).cloned().unwrap_or_default();
        match (job_title(server).map(|(k, _)| k.clone()), values.title.trim()) {
            (Some(key), "") => {
                map.remove(&key);
            }
            (Some(key), title) => {
                if let Some(Value::Object(entry)) = map.get_mut(&key) {
                    entry.insert("name".into(), json!(title));
                }
            }
            (None, "") => {}
            (None, title) => {
                let key = fresh_key(&map, "t");
                map.insert(key, json!({"name": title}));
            }
        }
        out.insert("titles".into(), if map.is_empty() { Value::Null } else { Value::Object(map) });
    }
    let filled = |list: &[Labeled]| list.iter().filter(|l| !l.value.trim().is_empty()).cloned().collect::<Vec<_>>();
    if wants(F::Emails) && filled(&now.emails) != filled(&values.emails) {
        let read = |e: &Map<String, Value>| Labeled::new(label_of(e), text(e, "address").trim());
        let same = |e: &Map<String, Value>, w: &Labeled| text(e, "address").trim().eq_ignore_ascii_case(w.value.trim());
        let make = |e: Option<&Map<String, Value>>, w: &Labeled| labelled_value(e, "address", w, false);
        out.insert("emails".into(), rebuilt(server, "emails", "e", &filled(&values.emails), false, read, same, make));
    }
    if wants(F::Phones) && filled(&now.phones) != filled(&values.phones) {
        let read = |p: &Map<String, Value>| Labeled::new(label_of(p), text(p, "number").trim());
        let same = |e: &Map<String, Value>, w: &Labeled| text(e, "number").trim() == w.value.trim();
        let make = |e: Option<&Map<String, Value>>, w: &Labeled| labelled_value(e, "number", w, true);
        out.insert("phones".into(), rebuilt(server, "phones", "p", &filled(&values.phones), false, read, same, make));
    }
    let addresses: Vec<PostalAddress> = values.addresses.iter().filter(|a| !a.is_empty()).cloned().collect();
    if wants(F::Addresses) && now.addresses != addresses {
        let same = |e: &Map<String, Value>, w: &PostalAddress| PostalAddress { label: w.label.clone(), ..address_of(e) } == *w;
        let make = |e: Option<&Map<String, Value>>, w: &PostalAddress| match e {
            Some(e) if same(e, w) => labelled(e.clone(), &w.label, false),
            Some(e) => address_edited(e, w),
            None => address_entry(w),
        };
        out.insert("addresses".into(), rebuilt(server, "addresses", "a", &addresses, true, address_of, same, make));
    }
    let sites: Vec<String> = values.websites.iter().map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect();
    if wants(F::Websites) && now.websites != sites {
        let read = |l: &Map<String, Value>| text(l, "uri").trim().to_string();
        let make = |_: Option<&Map<String, Value>>, w: &String| Map::from_iter([("uri".to_string(), json!(w))]);
        out.insert("links".into(), rebuilt(server, "links", "l", &sites, false, read, |_, _| false, make));
    }
    if wants(F::Birthday) && now.birthday != values.birthday {
        let mut map = server.get("anniversaries").and_then(Value::as_object).cloned().unwrap_or_default();
        // The birth entry read (else the first): a new date goes in it, its place and the rest stay.
        let births: Vec<String> =
            entries(server, "anniversaries").into_iter().filter(|(_, a)| text(a, "kind") == "birth").map(|(k, _)| k.clone()).collect();
        let shown = births
            .iter()
            .find(|k| map.get(*k).is_some_and(|a| birthday_of(&json!({"anniversaries": {"x": a}})).is_some()))
            .or(births.first())
            .cloned();
        match (values.birthday, shown) {
            (Some(day), Some(key)) => {
                if let Some(Value::Object(entry)) = map.get_mut(&key) {
                    entry.insert("date".into(), birthday_entry(day).get("date").cloned().unwrap_or(Value::Null));
                }
            }
            (Some(day), None) => {
                let key = fresh_key(&map, "b");
                map.insert(key, birthday_entry(day));
            }
            // No birthday: every birth entry that reads as one goes (one left would show); one that
            // doesn't (a day with no year, a place alone) was never shown, and stays.
            (None, _) => map
                .retain(|_, a| a.get("kind").and_then(Value::as_str) != Some("birth") || birthday_of(&json!({"anniversaries": {"x": a}})).is_none()),
        }
        out.insert("anniversaries".into(), if map.is_empty() { Value::Null } else { Value::Object(map) });
    }
    if wants(F::Notes) && now.notes.trim() != values.notes.trim() {
        let note = values.notes.trim();
        // The notes read (as [`notes_of`] picks them); the rest were never shown, and stay.
        let shown: Vec<String> = shown_notes(server).into_iter().map(|(k, _)| k.clone()).collect();
        let all = server.get("notes").and_then(Value::as_object).cloned().unwrap_or_default();
        let mut map: Map<String, Value> = all.iter().filter(|(k, _)| !shown.contains(k)).map(|(k, n)| (k.clone(), n.clone())).collect();
        match (note, shown.as_slice()) {
            ("", _) => {}
            // One note: its text changes, its author and time stay. (Several are read as one text,
            // which can't be split back: the edited text replaces them.)
            (note, [key]) => {
                let mut entry = all.get(key).and_then(Value::as_object).cloned().unwrap_or_default();
                entry.insert("note".into(), json!(note));
                map.insert(key.clone(), Value::Object(entry));
            }
            (note, _) => {
                // (Under the first key read, so that the next read shows it.)
                let key = shown.first().cloned().unwrap_or_else(|| fresh_key(&all, "n"));
                map.insert(key, json!({"note": note}));
            }
        }
        out.insert("notes".into(), if map.is_empty() { Value::Null } else { Value::Object(map) });
    }
    if wants(F::Categories)
        && let Some(categories) = &values.categories
    {
        let mut wanted: Vec<String> = categories.iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
        wanted.sort();
        wanted.dedup();
        if now.categories.as_ref() != Some(&wanted) {
            // (Keywords past those read were never shown: they stay.)
            let unseen = server
                .get("keywords")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .filter(|(_, on)| **on == Value::Bool(true))
                .skip(MAX_ENTRIES)
                .map(|(k, _)| k.clone());
            let map: Map<String, Value> = wanted.into_iter().chain(unseen).map(|c| (c, Value::Bool(true))).collect();
            out.insert("keywords".into(), if map.is_empty() { Value::Null } else { Value::Object(map) });
        }
    }
    out
}
