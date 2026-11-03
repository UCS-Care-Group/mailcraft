// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People › Merge Duplicates (#506): contacts that are the same person, found by an e-mail
//! address or a phone number they share, and put together into one.
//!
//! **What counts as the same.** Two contacts are offered together when they have an e-mail
//! address in common (compared without the spaces round it, and without telling capitals from
//! small letters) or a phone number in common. Phone numbers are compared by their digits alone,
//! whatever the spaces, brackets, dashes and dots between them; a number of fewer than
//! [`MIN_PHONE_DIGITS`] digits (an extension, a short code) is never compared. Of a longer one only
//! its last [`PHONE_TAIL`] digits count, so that `+44 20 7946 0000` and `020 7946 0000`, or
//! `+1 (555) 010-0100` and `555 010 0100`, are the same number. Matches chain: when A shares an
//! address with B and B a number with C, the three are one group. An address or a number that more
//! than [`MAX_LINK`] contacts have (a switchboard, a family's landline, `info@`) belongs to many
//! people, not to one: it links only those of them with the same name (trimmed, in small letters,
//! one space between words), so that the same card imported six times is still one group. Finding
//! them is two passes over the contacts with an index by address and number, never a comparison of
//! every pair.
//!
//! **What a merge does.** One contact is kept; it takes, field by field, the value the owner chose
//! (`fields`). Without a choice, a field of one value (a name, the company, the birthday) keeps the
//! kept contact's, or when that is empty the first of the others' that isn't; the fields of
//! several values (e-mail addresses, phone numbers, postal addresses, web pages, categories) and the
//! notes take everybody's, each once (phone numbers by all their digits: grouping looks at the last
//! ten, keeping looks at them all). A favourite stays one. Nothing the kept contact has is ever
//! cut, then or later: a merged field that would hold more than `people.edit` and the vCard reader
//! keep ([`MAX_VALUES`], [`MAX_CATEGORIES`], notes whose vCard line is longer than [`MAX_NOTES`]
//! bytes), where the kept contact's own didn't, is refused and named. Then the others are removed: one
//! command, so one undo step brings them all back. A contact that an account's server keeps (a
//! Microsoft account's contacts are read from it, and nothing changed here goes back) is never
//! merged, kept or removed: the command refuses it, and the list of groups says why.

use std::collections::{BTreeMap, HashMap, HashSet};

use mailcraft_model::{Contact, Id, Labeled, Mailbox, PostalAddress};
use mailcraft_time::Date;
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Most contacts one merge takes, the kept one with them; a group of more lists this many.
pub const MAX_MERGE: usize = 100;
/// The fewest digits a phone number has to have to be compared with another.
pub const MIN_PHONE_DIGITS: usize = 7;
/// The digits at the end of a phone number that are compared.
pub const PHONE_TAIL: usize = 10;
/// Most groups `people.findDuplicates` lists; the rest are counted.
pub const MAX_GROUPS: usize = 500;
/// Most contacts an address or a number links: one more of them have it is somebody else's too.
pub const MAX_LINK: usize = 5;
/// Of each contact, the addresses and numbers looked at (an import may give one hundreds).
const KEYS_EACH: usize = 50;
/// Most e-mail addresses, phone numbers, postal addresses and web pages a merged contact holds:
/// what `people.edit` keeps of each list (and the contact form sends the whole list back), and
/// what the vCard reader keeps.
pub const MAX_VALUES: usize = 100;
/// Most categories a merged contact holds: the vCard reader's 500 (`people.edit` keeps 1,000).
pub const MAX_CATEGORIES: usize = 500;
/// Longest the notes' vCard line may be, in bytes, escapes and `NOTE:` included: the vCard
/// reader's longest line, so that an export read back keeps all of them.
pub const MAX_NOTES: usize = 1 << 20;
/// Characters of a value `people.findDuplicates` shows; a longer one is cut and marked.
const SHOWN_CHARS: usize = 2000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("people.findDuplicates", "Find Duplicate Contacts", "People", find_duplicates).pure().params(FIND),
        CommandSpec::new("people.merge", "Merge Contacts", "Home › Actions", merge).params(MERGE),
    ]
}

const FIND: &str = r#"{"ids"?: [number] (2 to 100 contacts to merge whatever they share; default: every group of contacts that share an e-mail address or a phone number)} → {"groups": [{"ids", "keep" (suggested), "names", "shares", "locked": [{"id", "why"}], "mergeable", "fields": [{"field", "values": [{"id", "value"}]}]}], "more"}"#;
const MERGE: &str = r#"{"keep": number, "others": [number] (1 to 99, removed after the merge), "fields"?: {"<field>": number (take that contact's value) | "all" (everybody's, for emails, phones, addresses, websites, categories, notes)}} fields: displayName, first, middle, last, nickname, company, title, department, birthday, notes, emails, phones, addresses, websites, categories"#;

/// The fields of one value.
pub const SCALARS: &[&str] = &["displayName", "first", "middle", "last", "nickname", "company", "title", "department", "birthday"];
/// The fields of several values, which a merge takes everybody's of unless told otherwise.
pub const LISTS: &[&str] = &["emails", "phones", "addresses", "websites", "categories", "notes"];

/// An e-mail address as it is compared: trimmed and in small letters; `None` for something
/// that isn't one.
pub fn email_key(raw: &str) -> Option<String> {
    let t = raw.trim();
    (t.len() <= 320 && t.contains('@') && t.len() > 2).then(|| t.to_lowercase())
}

/// A phone number as it is compared: its last [`PHONE_TAIL`] digits; `None` for one of fewer than
/// [`MIN_PHONE_DIGITS`] digits.
pub fn phone_key(raw: &str) -> Option<String> {
    let digits: Vec<char> = raw.chars().take(200).filter(char::is_ascii_digit).collect();
    if digits.len() < MIN_PHONE_DIGITS {
        return None;
    }
    let from = digits.len().saturating_sub(PHONE_TAIL);
    Some(digits.get(from..).unwrap_or_default().iter().collect())
}

/// One group of contacts that look like the same person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// By id, at most [`MAX_MERGE`].
    pub ids: Vec<Id>,
    /// The contact suggested to keep: the one with the most filled in, of those that can change.
    pub keep: Id,
    /// The addresses and numbers they share, as the first of them writes them.
    pub shares: Vec<String>,
    /// Those that can't be merged, and why.
    pub locked: Vec<(Id, mailcraft_i18n::Text)>,
    /// Contacts of the group past [`MAX_MERGE`], not listed.
    pub more: usize,
}

impl Group {
    /// The contacts of the group that can be merged.
    pub fn free(&self) -> Vec<Id> {
        self.ids.iter().copied().filter(|id| !self.locked.iter().any(|(l, _)| l == id)).collect()
    }

    /// Whether two or more of them can be merged.
    pub fn mergeable(&self) -> bool {
        self.free().len() >= 2
    }
}

/// Why `c` can't be merged, when an account's server keeps it.
pub fn locked(mailbox: &Mailbox, c: &Contact) -> Option<mailcraft_i18n::Text> {
    let account = super::remote::keeper(mailbox, &c.origin)?;
    let who = super::remote::who(account);
    Some(said!("It is kept by the server of {who}, and changes made here are not sent back yet, so it can't be merged here.", who))
}

/// How much of a contact is filled in.
fn filled(c: &Contact) -> usize {
    let scalars = SCALARS.iter().filter(|f| !scalar(c, f).is_empty()).count();
    scalars
        .saturating_add(c.emails.len())
        .saturating_add(c.phones.len())
        .saturating_add(c.addresses.len())
        .saturating_add(c.websites.len())
        .saturating_add(c.categories.len())
        .saturating_add(usize::from(!c.notes.trim().is_empty()))
}

/// Path-halving find of a union-find over indexes; never out of bounds.
fn root(parent: &mut [usize], mut i: usize) -> usize {
    // (Each step goes up a tree that is shorter than the number of nodes.)
    for _ in 0..parent.len() {
        let up = parent.get(i).copied().unwrap_or(i);
        if up == i {
            return i;
        }
        let grand = parent.get(up).copied().unwrap_or(up);
        if let Some(slot) = parent.get_mut(i) {
            *slot = grand;
        }
        i = grand;
    }
    i
}

fn join(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (root(parent, a), root(parent, b));
    if ra != rb {
        // The smaller index is the root, so a group's root is its first contact.
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        if let Some(slot) = parent.get_mut(hi) {
            *slot = lo;
        }
    }
}

/// The group made of `contacts` themselves (a selection), whatever they share.
fn group_of(mailbox: &Mailbox, contacts: &[&Contact], more: usize, shares: Vec<String>) -> Group {
    let locked: Vec<(Id, mailcraft_i18n::Text)> = contacts.iter().filter_map(|c| locked(mailbox, c).map(|why| (c.id, why))).collect();
    let free = |c: &&&Contact| !locked.iter().any(|(l, _)| *l == c.id);
    // Of those that can change, the one with the most filled in; the first when it is a tie.
    let keep = contacts.iter().filter(free).max_by(|a, b| filled(a).cmp(&filled(b)).then(b.id.cmp(&a.id))).or(contacts.first()).map_or(0, |c| c.id);
    Group { ids: contacts.iter().map(|c| c.id).collect(), keep, shares, locked, more }
}

/// The addresses and numbers of `c` as they are compared, each once, and as `c` writes them.
fn keys(c: &Contact) -> Vec<(String, &str)> {
    let emails = c.emails.iter().take(KEYS_EACH).filter_map(|e| email_key(&e.value).map(|k| (format!("e:{k}"), e.value.trim())));
    let phones = c.phones.iter().take(KEYS_EACH).filter_map(|ph| phone_key(&ph.value).map(|k| (format!("p:{k}"), ph.value.trim())));
    let mut out: Vec<(String, &str)> = Vec::new();
    for (k, w) in emails.chain(phones) {
        if !out.iter().any(|(x, _)| *x == k) {
            out.push((k, w));
        }
    }
    out
}

/// A contact's name as two copies of one card are compared by: trimmed, in small letters, one
/// space between words.
fn same_name(c: &Contact) -> String {
    c.name().chars().take(300).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Every group of two or more contacts that share an e-mail address or a phone number (that no
/// more than [`MAX_LINK`] contacts have), the first [`MAX_GROUPS`] by their first contact's id,
/// and how many more there are.
pub fn find(mailbox: &Mailbox) -> (Vec<Group>, usize) {
    let all: Vec<&Contact> = mailbox.contacts.values().map(AsRef::as_ref).collect();
    let mut parent: Vec<usize> = (0..all.len()).collect();
    // How many contacts have each address or number, who has it first, and as what.
    let mut seen: HashMap<String, (usize, usize, String)> = HashMap::new();
    for (i, c) in all.iter().enumerate() {
        for (key, written) in keys(c) {
            seen.entry(key).and_modify(|(n, _, _)| *n = n.saturating_add(1)).or_insert_with(|| (1, i, written.to_string()));
        }
    }
    // What links: an address or a number of a few, or of many with the same name. Who has it
    // first, as what, and whether anybody else joined them by it.
    let mut links: HashMap<String, (usize, String, bool)> = HashMap::new();
    for (i, c) in all.iter().enumerate() {
        let name = same_name(c);
        for (key, written) in keys(c) {
            let many = seen.get(&key).is_some_and(|(n, _, _)| *n > MAX_LINK);
            let link = match (many, name.is_empty()) {
                (false, _) => key,
                (true, false) => format!("{key}\u{0}{name}"),
                (true, true) => continue,
            };
            match links.get_mut(&link) {
                Some((first, _, joined)) if *first != i => {
                    *joined = true;
                    join(&mut parent, *first, i);
                }
                Some(_) => {}
                None => {
                    links.insert(link, (i, written.to_string(), false));
                }
            }
        }
    }
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..all.len() {
        let r = root(&mut parent, i);
        members.entry(r).or_default().push(i);
    }
    // What each group shares, by its root.
    let mut shares: HashMap<usize, Vec<String>> = HashMap::new();
    for (first, written, _) in links.values().filter(|(_, _, joined)| *joined) {
        let r = root(&mut parent, *first);
        let list = shares.entry(r).or_default();
        if list.len() < 20 {
            list.push(written.clone());
        }
    }
    let mut groups = Vec::new();
    let mut more = 0usize;
    for (r, list) in members.into_iter().filter(|(_, l)| l.len() >= 2) {
        if groups.len() >= MAX_GROUPS {
            more = more.saturating_add(1);
            continue;
        }
        let contacts: Vec<&Contact> = list.iter().take(MAX_MERGE).filter_map(|i| all.get(*i).copied()).collect();
        let mut said = shares.remove(&r).unwrap_or_default();
        said.sort_by_key(|s| s.to_lowercase());
        groups.push(group_of(mailbox, &contacts, list.len().saturating_sub(MAX_MERGE), said));
    }
    (groups, more)
}

/// The group of the contacts `ids` (a selection of two or more), whatever they share.
pub fn selection(mailbox: &Mailbox, ids: &[Id]) -> Result<Group, CmdError> {
    let contacts = members(mailbox, ids)?;
    if contacts.len() < 2 {
        return Err(bad("`ids` names fewer than two contacts"));
    }
    Ok(group_of(mailbox, &contacts, 0, Vec::new()))
}

/// The contacts `ids`, each once and all there, at most [`MAX_MERGE`].
fn members<'a>(mailbox: &'a Mailbox, ids: &[Id]) -> Result<Vec<&'a Contact>, CmdError> {
    if ids.len() > MAX_MERGE {
        return Err(failed!("At most {max} contacts can be merged at once.", max = MAX_MERGE));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        if !seen.insert(*id) {
            return Err(failed!("The contact {id} is named twice.", id = *id));
        }
        out.push(mailbox.contact(*id).ok_or_else(|| failed!("no contact with id {id}", id = *id))?);
    }
    Ok(out)
}

/// A field of one value, as text (a birthday as `YYYY-MM-DD`).
pub fn scalar(c: &Contact, field: &str) -> String {
    match field {
        "displayName" => c.display_name.trim().to_string(),
        "first" => c.first.trim().to_string(),
        "middle" => c.middle.trim().to_string(),
        "last" => c.last.trim().to_string(),
        "nickname" => c.nickname.trim().to_string(),
        "company" => c.company.trim().to_string(),
        "title" => c.title.trim().to_string(),
        "department" => c.department.trim().to_string(),
        "birthday" => c.birthday.map(Date::iso).unwrap_or_default(),
        _ => String::new(),
    }
}

fn set_scalar(to: &mut Contact, from: &Contact, field: &str) {
    match field {
        "displayName" => to.display_name.clone_from(&from.display_name),
        "first" => to.first.clone_from(&from.first),
        "middle" => to.middle.clone_from(&from.middle),
        "last" => to.last.clone_from(&from.last),
        "nickname" => to.nickname.clone_from(&from.nickname),
        "company" => to.company.clone_from(&from.company),
        "title" => to.title.clone_from(&from.title),
        "department" => to.department.clone_from(&from.department),
        "birthday" => to.birthday = from.birthday,
        _ => {}
    }
}

/// A field of several values, as one line of text (for showing what differs).
pub fn list_text(c: &Contact, field: &str) -> String {
    let join = |v: Vec<String>| v.into_iter().filter(|s| !s.trim().is_empty()).collect::<Vec<_>>().join("; ");
    match field {
        "emails" => join(c.emails.iter().map(|e| e.value.trim().to_string()).collect()),
        "phones" => join(c.phones.iter().map(|e| e.value.trim().to_string()).collect()),
        "addresses" => join(c.addresses.iter().map(PostalAddress::one_line).collect()),
        "websites" => join(c.websites.iter().map(|w| w.trim().to_string()).collect()),
        "categories" => join(c.categories.iter().map(|w| w.trim().to_string()).collect()),
        "notes" => c.notes.trim().to_string(),
        _ => String::new(),
    }
}

/// The text of a field, of either kind.
pub fn field_text(c: &Contact, field: &str) -> String {
    if SCALARS.contains(&field) { scalar(c, field) } else { list_text(c, field) }
}

/// Where a merged field comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// That contact's value.
    From(Id),
    /// Everybody's (a field of several values); for a field of one value, as without a choice.
    All,
}

/// The fields whose values differ between `ids`: each with every contact's value that isn't
/// empty, first of each value only.
pub fn differing(mailbox: &Mailbox, ids: &[Id]) -> Vec<(&'static str, Vec<(Id, String)>)> {
    let contacts: Vec<&Contact> = ids.iter().filter_map(|id| mailbox.contact(*id)).collect();
    let mut out = Vec::new();
    for field in SCALARS.iter().chain(LISTS) {
        let mut values: Vec<(Id, String)> = Vec::new();
        for c in &contacts {
            let v = field_text(c, field);
            if !v.is_empty() && !values.iter().any(|(_, x)| x.eq_ignore_ascii_case(&v)) {
                values.push((c.id, v));
            }
        }
        if values.len() >= 2 {
            out.push((*field, values));
        }
    }
    out
}

fn dedup_push<T, K: Eq + std::hash::Hash>(into: &mut Vec<T>, seen: &mut HashSet<K>, item: T, key: K) {
    if seen.insert(key) {
        into.push(item);
    }
}

/// What two addresses or numbers are kept once by: an address as it is compared when finding
/// duplicates, a number by all its digits (not the last ten that grouping looks at: two numbers in
/// different countries are both kept), else the text in small letters.
fn labeled_key(l: &Labeled, phone: bool) -> String {
    let k = if phone {
        Some(l.value.chars().take(200).filter(char::is_ascii_digit).collect::<String>()).filter(|d| !d.is_empty())
    } else {
        email_key(&l.value)
    };
    k.unwrap_or_else(|| l.value.trim().to_lowercase())
}

/// Everybody's values of a field of several, each once, in the order of `from`.
fn union(to: &mut Contact, from: &[&Contact], field: &str) {
    match field {
        "emails" | "phones" => {
            let phone = field == "phones";
            let (mut out, mut seen) = (Vec::new(), HashSet::new());
            for c in from {
                for l in if phone { &c.phones } else { &c.emails } {
                    if !l.value.trim().is_empty() {
                        dedup_push(&mut out, &mut seen, l.clone(), labeled_key(l, phone));
                    }
                }
            }
            if phone { to.phones = out } else { to.emails = out }
        }
        "addresses" => {
            let (mut out, mut seen) = (Vec::new(), HashSet::new());
            for a in from.iter().flat_map(|c| &c.addresses).filter(|a| !a.is_empty()) {
                dedup_push(&mut out, &mut seen, a.clone(), a.one_line().to_lowercase());
            }
            to.addresses = out;
        }
        "websites" | "categories" => {
            let (mut out, mut seen) = (Vec::new(), HashSet::new());
            for c in from {
                for w in if field == "websites" { &c.websites } else { &c.categories } {
                    if !w.trim().is_empty() {
                        dedup_push(&mut out, &mut seen, w.trim().to_string(), w.trim().to_lowercase());
                    }
                }
            }
            if field == "websites" { to.websites = out } else { to.categories = out }
        }
        "notes" => {
            let (mut out, mut seen): (Vec<&str>, HashSet<String>) = (Vec::new(), HashSet::new());
            for n in from.iter().map(|c| c.notes.trim()).filter(|n| !n.is_empty()) {
                dedup_push(&mut out, &mut seen, n, n.to_lowercase());
            }
            to.notes = out.join("\n\n");
        }
        _ => {}
    }
}

fn set_list(to: &mut Contact, from: &Contact, field: &str) {
    match field {
        "emails" => to.emails.clone_from(&from.emails),
        "phones" => to.phones.clone_from(&from.phones),
        "addresses" => to.addresses.clone_from(&from.addresses),
        "websites" => to.websites.clone_from(&from.websites),
        "categories" => to.categories.clone_from(&from.categories),
        "notes" => to.notes.clone_from(&from.notes),
        _ => {}
    }
}

/// The contact `keep` as it is after merging `others` into it, with `picks` per field; nothing
/// changes. Refuses what the command refuses.
pub fn merged(mailbox: &Mailbox, keep: Id, others: &[Id], picks: &BTreeMap<String, Pick>) -> Result<Contact, CmdError> {
    if others.is_empty() {
        return Err(bad("`others` names no contact to merge into the kept one"));
    }
    if others.contains(&keep) {
        return Err(failed!("The contact {id} is both kept and merged into itself.", id = keep));
    }
    let all: Vec<Id> = std::iter::once(keep).chain(others.iter().copied()).collect();
    let contacts = members(mailbox, &all)?;
    for c in &contacts {
        if let Some(account) = super::remote::keeper(mailbox, &c.origin) {
            let (name, who) = (c.name(), super::remote::who(account));
            return Err(failed!(
                "\"{name}\" is kept by the server of {who}. Changes made here are not sent back yet, so it can't be merged here.",
                name,
                who
            ));
        }
    }
    let Some(first) = contacts.first() else { return Err(fail("the contact is gone")) };
    let mut out: Contact = (*first).clone();
    for (field, pick) in picks {
        let known = SCALARS.contains(&field.as_str()) || LISTS.contains(&field.as_str());
        if !known {
            return Err(bad(format!("`fields` has `{}`, which is not a field a merge chooses", field.chars().take(40).collect::<String>())));
        }
        if let Pick::From(id) = pick
            && !all.contains(id)
        {
            return Err(failed!("The contact {id} chosen for a field is not one of those merged.", id = *id));
        }
    }
    for field in SCALARS {
        let from = match picks.get(*field) {
            Some(Pick::From(id)) => contacts.iter().find(|c| c.id == *id).copied(),
            // The kept contact's, else the first of the others' that has one.
            _ => contacts.iter().find(|c| !scalar(c, field).is_empty()).copied(),
        };
        if let Some(from) = from {
            set_scalar(&mut out, from, field);
        }
    }
    for field in LISTS {
        match picks.get(*field) {
            Some(Pick::From(id)) => {
                if let Some(from) = contacts.iter().find(|c| c.id == *id) {
                    set_list(&mut out, from, field);
                }
            }
            _ => union(&mut out, &contacts, field),
        }
    }
    // Nothing is cut: a field that grows past what a contact holds is refused, by name.
    for field in LISTS {
        let (now, before) = (size(&out, field), size(first, field));
        if now > before && now > cap(field) {
            return Err(failed!(
                "The merged contact's {field} would be too large ({now}, at most {max}), so the contacts aren't merged. Choose one contact's {field} for it, or shorten them first.",
                field = mailcraft_i18n::Arg::words(field_label(field)),
                now,
                max = cap(field)
            ));
        }
    }
    out.favorite = contacts.iter().any(|c| c.favorite);
    if out.uid.trim().is_empty() {
        out.uid = contacts.iter().map(|c| c.uid.trim()).find(|u| !u.is_empty()).unwrap_or_default().to_string();
    }
    if out.name().is_empty() {
        return Err(fail("a contact needs a name, a company or an e-mail address"));
    }
    Ok(out)
}

/// How large a field of several values is: characters of the notes, else values.
fn size(c: &Contact, field: &str) -> usize {
    match field {
        "emails" => c.emails.len(),
        "phones" => c.phones.len(),
        "addresses" => c.addresses.len(),
        "websites" => c.websites.len(),
        "categories" => c.categories.len(),
        "notes" => notes_line(&c.notes),
        _ => 0,
    }
}

/// How long the notes' line of a vCard is, in bytes: `NOTE:`, and each backslash, comma,
/// semicolon and line break written as two.
fn notes_line(notes: &str) -> usize {
    let escaped = notes.chars().filter(|c| matches!(c, '\\' | ',' | ';' | '\n')).count();
    if notes.trim().is_empty() { 0 } else { notes.len().saturating_add(escaped).saturating_add(5) }
}

/// The most a merge makes a field of several values.
fn cap(field: &str) -> usize {
    match field {
        "notes" => MAX_NOTES,
        "categories" => MAX_CATEGORIES,
        _ => MAX_VALUES,
    }
}

/// What a field of several values is called in a refusal.
fn field_label(field: &str) -> &'static str {
    match field {
        "emails" => "e-mail addresses",
        "phones" => "phone numbers",
        "addresses" => "addresses",
        "websites" => "web pages",
        "categories" => "categories",
        _ => "notes",
    }
}

/// The ids of `key` in `v`, an array of positive whole numbers.
fn id_list(v: &Value, key: &str) -> Result<Vec<Id>, CmdError> {
    let list = v.get(key).and_then(Value::as_array).ok_or_else(|| bad(format!("`{key}` (an array of contact ids) is required")))?;
    if list.len() > MAX_MERGE {
        return Err(failed!("At most {max} contacts can be merged at once.", max = MAX_MERGE));
    }
    list.iter().map(|x| x.as_u64().filter(|i| *i != 0).ok_or_else(|| bad(format!("`{key}` must hold contact ids")))).collect()
}

/// `fields` of `people.merge`.
fn picks(v: &Value) -> Result<BTreeMap<String, Pick>, CmdError> {
    let mut out = BTreeMap::new();
    let Some(fields) = v.get("fields").filter(|f| !f.is_null()) else { return Ok(out) };
    let fields = fields.as_object().ok_or_else(|| bad("`fields` is an object of field names"))?;
    for (k, x) in fields.iter().take(64) {
        let pick = match x {
            Value::String(s) if s == "all" => Pick::All,
            _ => Pick::From(x.as_u64().filter(|i| *i != 0).ok_or_else(|| bad("each of `fields` is a contact id or \"all\""))?),
        };
        out.insert(k.clone(), pick);
    }
    Ok(out)
}

fn group_json(mailbox: &Mailbox, g: &Group) -> Value {
    let name = |id: &Id| mailbox.contact(*id).map(Contact::name).unwrap_or_default();
    let fields: Vec<Value> = differing(mailbox, &g.free())
        .into_iter()
        .map(|(f, values)| json!({"field": f, "values": values.into_iter().map(|(id, value)| shown(id, &value)).collect::<Vec<_>>()}))
        .collect();
    json!({
        "ids": g.ids, "keep": g.keep, "names": g.ids.iter().map(name).collect::<Vec<_>>(), "shares": g.shares,
        "locked": g.locked.iter().map(|(id, why)| json!({"id": id, "why": why.to_string()})).collect::<Vec<_>>(),
        "mergeable": g.mergeable(), "fields": fields, "more": g.more,
    })
}

/// A contact's value of a field as `findDuplicates` lists it: at most [`SHOWN_CHARS`] characters,
/// with `"truncated": true` when it was longer.
fn shown(id: Id, value: &str) -> Value {
    match value.char_indices().nth(SHOWN_CHARS) {
        Some((cut, _)) => json!({"id": id, "value": value.get(..cut).unwrap_or_default(), "truncated": true}),
        None => json!({"id": id, "value": value}),
    }
}

fn find_duplicates(s: &mut Session, v: &Value) -> CmdResult {
    let mailbox = &s.mailbox;
    if v.get("ids").is_some_and(|x| !x.is_null()) {
        let g = selection(mailbox, &id_list(v, "ids")?)?;
        return Ok(json!({"groups": [group_json(mailbox, &g)], "more": 0}));
    }
    let (groups, more) = find(mailbox);
    Ok(json!({"groups": groups.iter().map(|g| group_json(mailbox, g)).collect::<Vec<_>>(), "more": more}))
}

fn merge(s: &mut Session, v: &Value) -> CmdResult {
    let keep = p::req_id(v, "keep")?;
    let others = id_list(v, "others")?;
    if others.len() >= MAX_MERGE {
        return Err(failed!("At most {max} contacts can be merged at once.", max = MAX_MERGE));
    }
    let contact = merged(&s.mailbox, keep, &others, &picks(v)?)?;
    *s.mailbox.contact_mut(keep)? = contact;
    for id in &others {
        s.mailbox.remove_contact(*id)?;
    }
    if s.sel.contact.is_none_or(|c| others.contains(&c) || c == keep) {
        s.sel.contact = Some(keep);
    }
    Ok(json!({"id": keep, "removed": others}))
}

#[cfg(test)]
#[path = "people_merge_tests.rs"]
mod people_merge_tests;
