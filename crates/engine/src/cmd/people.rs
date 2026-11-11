// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People: contacts.

use mailcraft_model::{AccountChoice, Address, Contact, Id, Labeled, Mailbox, PostalAddress, clean_name};
use mailcraft_time::Date;
use serde_json::{Value, json};

use super::tasks::kept_at;
use super::{bad, done, fail, needs_message};
use crate::session::parse_enum;
use crate::{CmdError, CmdResult, CommandSpec, Module, PeopleSort, Session, p};

#[cfg(test)]
mod account_tests;
#[cfg(test)]
mod share_tests;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("people.new", "New Contact", "Home › New", new).key("Mod+Shift+C").params(CONTACT),
        CommandSpec::new("people.edit", "Edit Contact", "Contact", edit)
            .params(r#"{"id"?: number, …the fields of people.new}"#)
            .selection(needs_contact),
        CommandSpec::new("people.delete", "Delete", "Home › Delete", delete).params(ONE).selection(needs_contact),
        CommandSpec::new("people.select", "Select Contact", "People", select).pure().params(r#"{"id": number | null} (null selects nothing; anything else that isn't an id is refused)"#),
        CommandSpec::new("people.get", "Get Contact", "People", get).pure().params(ONE),
        CommandSpec::new("people.list", "List Contacts", "People", list)
            .pure()
            .params(r#"{"query"?: string, "account"?: "all" | "here" | number} (default: what the list shows; 0 is "here")"#),
        CommandSpec::new("people.search", "Search People", "Home › Find", search).pure().params(r#"{"query": string}"#),
        CommandSpec::new("people.favorite", "Add to Favorites", "Home › Actions", favorite)
            .params(r#"{"value"?: bool (default: toggle), "id"?: number}"#)
            .selection(needs_contact),
        CommandSpec::new("people.categorize", "Categorize", "Home › Tags", categorize)
            .params(r#"{"category": string, "value"?: bool, "id"?: number}"#)
            .selection(needs_contact),
        CommandSpec::new("people.arrange", "Arrange Contacts By", "View › Arrangement", arrange).pure().params(r#"{"by": "name"|"categories"}"#),
        CommandSpec::new("people.email", "Email", "Home › Communicate", email)
            .pure()
            .params(r#"{"id"?: number, "discard"?: true} (default: the selected contact; a message being written stays open beside it, #286; `discard` changes nothing)"#)
            .selection(needs_contact),
        CommandSpec::new("people.meeting", "Meeting", "Home › Communicate", meeting)
            .params(r#"{"id"?: number (the contact; default: the selected one), …the fields of calendar.newMeeting} — a new meeting with the contact invited, beside anyone "attendees" names: calendar.newMeeting, with its rules ("send": true invites them by mail from an account's calendar)"#)
            .selection(needs_contact),
        CommandSpec::new("people.forward", "Forward Contact", "Home › Share", forward)
            .pure()
            .params(r#"{"id"?: number (default: the selected contact), "discard"?: true} — starts a new message with the contact attached as a vCard (.vcf), for the owner to address and send; nothing is sent (a message being written stays open beside it, #286; `discard` changes nothing)"#)
            .selection(needs_contact),
        CommandSpec::new("people.fromMessage", "Add to Contacts", "Home › Actions", from_message)
            .params(r#"{"id"?: number} (a message; default: the selected one) — a new contact is kept on this computer; the list moves to show the contact"#)
            .selection(needs_message),
    ]
}

const ONE: &str = r#"{"id"?: number} (default: the selected contact). An `id` that names nothing (null, text, 0, a fraction) is refused"#;
const CONTACT: &str = r#"{"first"?: string, "last"?: string, "displayName"?: string, "company"?: string, "title"?: string, "department"?: string, "nickname"?: string, "email"?: string (primary) | "emails"?: [{"label", "value"}], "phone"?: string | "phones"?: [{"label", "value"}], "address"?: {"label", "street", "city", "region", "postalCode", "country"}, "website"?: string, "birthday"?: "YYYY-MM-DD" | null, "notes"?: string, "categories"?: [string], "favorite"?: bool, "keys"?: [{"keep": number (the card's key at that place, as people.get lists them)} | {"data": base64 (a Public key from a file: an X.509 certificate for mail or an OpenPGP Public key; a private key is refused)}] (the card's keys afterwards, in order; what isn't kept goes)} — it is kept on this computer; the list moves to show it"#;

fn needs_contact(s: &Session) -> Option<&'static str> {
    if s.sel.contact.is_none() { Some("select a contact first") } else { None }
}

/// Whether the account's contacts are read from its server.
pub fn keeps_contacts(mailbox: &Mailbox, account: Id) -> bool {
    mailbox.account(account).is_some_and(|a| a.keeps_contacts())
}

/// The accounts whose contacts are read from their servers.
pub fn contact_accounts(mailbox: &Mailbox) -> Vec<Id> {
    mailbox.accounts.iter().filter(|a| a.keeps_contacts()).map(|a| a.id).collect()
}

/// Whose contacts the list shows: what the owner chose, while it means something. With no account
/// that keeps contacts there is nothing to choose, and an account that no longer does is All
/// accounts.
pub fn shown_account(s: &Session) -> AccountChoice {
    match s.view.people_account {
        AccountChoice::Account(account) if keeps_contacts(&s.mailbox, account) => s.view.people_account,
        AccountChoice::Here if !contact_accounts(&s.mailbox).is_empty() => AccountChoice::Here,
        _ => AccountChoice::All,
    }
}

/// A contact just made or picked is in view: where the list doesn't show it, the list moves to it,
/// and a search that would leave it out is cleared, so its row is there beside its card.
pub(crate) fn reveal(s: &mut Session, id: Id) {
    let shown = shown_account(s);
    let Some(c) = s.mailbox.contact(id) else { return };
    let (choice, hidden, unmatched) = (AccountChoice::of(&c.origin), !shown.shows(&c.origin), !c.matches(&s.view.people_search));
    if hidden {
        s.view.people_account = choice;
    }
    if unmatched {
        s.view.people_search.clear();
    }
}

/// "On this computer" means something only beside an account that keeps contacts: once the last
/// of them goes, the list is back on All accounts, so one added later doesn't open People on it.
pub(crate) fn forget_here(s: &mut Session) {
    if s.view.people_account == AccountChoice::Here && contact_accounts(&s.mailbox).is_empty() {
        s.view.people_account = AccountChoice::All;
    }
}

fn target(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let id = p::id_strict(v, "id")?.or(s.sel.contact).ok_or_else(|| fail("select a contact first"))?;
    s.mailbox.contact(id).map(|c| c.id).ok_or_else(|| failed!("no contact with id {id}", id))
}

/// [`target`] for a command that changes or deletes the contact. A contact that an account's
/// server keeps is refused: changes are not sent back yet, and the next download would undo
/// them. (Its categories and whether it is a favourite are kept here, so those don't ask.)
fn own_target(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let id = target(s, v)?;
    if let Some(c) = s.mailbox.contact(id) {
        super::remote::changeable(&s.mailbox, &c.origin, "contact")?;
    }
    Ok(id)
}

fn line(v: &Value, k: &str) -> Option<String> {
    p::str(v, k).map(|t| t.replace(['\r', '\n'], " ").trim().chars().take(300).collect())
}

fn labeled(v: &Value, k: &str) -> Option<Vec<Labeled>> {
    let list = v.get(k)?.as_array()?;
    Some(
        list.iter()
            .take(100)
            .filter_map(|x| {
                let value = line(x, "value").or_else(|| x.as_str().map(|s| s.trim().to_string()))?;
                (!value.is_empty()).then(|| Labeled { label: line(x, "label").unwrap_or_default(), value })
            })
            .collect(),
    )
}

fn apply(c: &mut Contact, v: &Value) -> Result<(), CmdError> {
    for (key, field) in [
        ("first", &mut c.first),
        ("middle", &mut c.middle),
        ("last", &mut c.last),
        ("displayName", &mut c.display_name),
        ("company", &mut c.company),
        ("title", &mut c.title),
        ("department", &mut c.department),
        ("nickname", &mut c.nickname),
    ] {
        if let Some(t) = line(v, key) {
            *field = t;
        }
    }
    if let Some(n) = p::str(v, "notes") {
        c.notes = n.to_string();
    }
    if let Some(list) = labeled(v, "emails") {
        c.emails = list;
    }
    // `email` and `phone` set the first entry and leave the others.
    if let Some(e) = line(v, "email") {
        match (c.emails.first_mut(), e.is_empty()) {
            (Some(first), false) => first.value = e,
            (Some(_), true) => {
                c.emails.remove(0);
            }
            (None, false) => c.emails.push(Labeled::new("work", e)),
            (None, true) => {}
        }
    }
    if let Some(list) = labeled(v, "phones") {
        c.phones = list;
    }
    if let Some(ph) = line(v, "phone") {
        match (c.phones.first_mut(), ph.is_empty()) {
            (Some(first), false) => first.value = ph,
            (Some(_), true) => {
                c.phones.remove(0);
            }
            (None, false) => c.phones.push(Labeled::new("mobile", ph)),
            (None, true) => {}
        }
    }
    if let Some(a) = v.get("address").filter(|a| a.is_object()) {
        let addr = PostalAddress {
            label: line(a, "label").unwrap_or_else(|| "work".into()),
            street: p::str(a, "street").unwrap_or("").trim().chars().take(500).collect(),
            city: line(a, "city").unwrap_or_default(),
            region: line(a, "region").unwrap_or_default(),
            postal_code: line(a, "postalCode").unwrap_or_default(),
            country: line(a, "country").unwrap_or_default(),
        };
        c.addresses = if addr.is_empty() {
            c.addresses.iter().skip(1).cloned().collect()
        } else {
            std::iter::once(addr).chain(c.addresses.iter().skip(1).cloned()).collect()
        };
    }
    if let Some(w) = line(v, "website") {
        c.websites = if w.is_empty() {
            c.websites.iter().skip(1).cloned().collect()
        } else {
            std::iter::once(w).chain(c.websites.iter().skip(1).cloned()).collect()
        };
    }
    match v.get("birthday") {
        Some(Value::Null) => c.birthday = None,
        Some(_) => c.birthday = p::date(v, "birthday")?,
        None => {}
    }
    if let Some(cats) = p::strings(v, "categories") {
        c.categories = cats;
    }
    if let Some(f) = p::bool(v, "favorite") {
        c.favorite = f;
    }
    if let Some(list) = v.get("keys") {
        c.keys = super::keys::card_keys_param(&c.keys, list)?;
    }
    Ok(())
}

fn new(s: &mut Session, v: &Value) -> CmdResult {
    let mut c = Contact::default();
    apply(&mut c, v)?;
    if c.name().is_empty() {
        return Err(bad("a contact needs a name, a company or an e-mail address"));
    }
    // Its categories take the spelling of the list its account's items use (one made here is
    // this computer's: the shared list); a new name joins it.
    if p::strings(v, "categories").is_some() {
        c.categories = super::category::spelled_all(s, c.origin.account, &c.categories, &[], c.origin.account)?;
    }
    let id = s.mailbox.add_contact(c);
    s.sel.contact = Some(id);
    reveal(s, id);
    Ok(json!({"id": id}))
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let id = own_target(s, v)?;
    let mut c = s.mailbox.contact(id).cloned().ok_or_else(|| fail("the contact is gone"))?;
    let had = c.categories.clone();
    apply(&mut c, v)?;
    if c.name().is_empty() {
        return Err(fail("a contact needs a name, a company or an e-mail address"));
    }
    if p::strings(v, "categories").is_some() {
        c.categories = super::category::spelled_all(s, c.origin.account, &c.categories, &had, c.origin.account)?;
    }
    *s.mailbox.contact_mut(id)? = c;
    Ok(json!({"id": id}))
}

/// The contacts the People list shows, in its order ([`sorted_of`]): `people.list` and the
/// neighbour a delete selects use it.
pub fn sorted(s: &Session, query: &str) -> Vec<Id> {
    sorted_of(s, query, shown_account(s))
}

/// Contacts of `account` in the People list's order: by name, favourites first; arranged by
/// categories, by name alone (the list then puts each under each of its categories, and favourites
/// have no heading of their own there). Each contact once.
pub fn sorted_of(s: &Session, query: &str, account: AccountChoice) -> Vec<Id> {
    let mut v: Vec<&Contact> =
        s.mailbox.contacts.values().map(|c| c.as_ref()).filter(|c| account.shows(&c.origin)).filter(|c| c.matches(query)).collect();
    let by_name = |a: &Contact, b: &Contact| a.file_as().to_lowercase().cmp(&b.file_as().to_lowercase()).then(a.id.cmp(&b.id));
    match s.view.people_sort {
        PeopleSort::Name => v.sort_by(|a, b| b.favorite.cmp(&a.favorite).then_with(|| by_name(a, b))),
        PeopleSort::Categories => v.sort_by(|a, b| by_name(a, b)),
    }
    v.into_iter().map(|c| c.id).collect()
}

fn arrange(s: &mut Session, v: &Value) -> CmdResult {
    s.view.people_sort = parse_enum::<PeopleSort>(p::req_str(v, "by")?).ok_or_else(|| bad("`by` is name or categories"))?;
    Ok(json!({"by": s.view.people_sort}))
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let id = own_target(s, v)?;
    // The selection moves to the neighbour in the list.
    let order = sorted(s, &s.view.people_search);
    let at = order.iter().position(|x| *x == id);
    s.mailbox.remove_contact(id)?;
    if s.sel.contact == Some(id) {
        s.sel.contact = at.and_then(|i| order.get(i + 1).or_else(|| i.checked_sub(1).and_then(|j| order.get(j)))).copied();
    }
    Ok(json!({"deleted": id}))
}

fn select(s: &mut Session, v: &Value) -> CmdResult {
    // `null` selects nothing, as leaving `id` out does; an id named wrongly is refused (#553).
    let named = if v.get("id").is_some_and(Value::is_null) { None } else { p::id_strict(v, "id")? };
    s.sel.contact = match named {
        Some(id) => Some(s.mailbox.contact(id).map(|c| c.id).ok_or_else(|| failed!("no contact with id {id}", id))?),
        None => None,
    };
    if let Some(id) = s.sel.contact {
        reveal(s, id);
    }
    done()
}

fn contact_json(mailbox: &Mailbox, c: &Contact) -> Value {
    json!({
        "id": c.id, "name": c.name(), "fileAs": c.file_as(), "first": c.first, "middle": c.middle, "last": c.last, "displayName": c.display_name,
        "nickname": c.nickname, "company": c.company, "title": c.title, "department": c.department,
        "emails": c.emails, "phones": c.phones, "addresses": c.addresses, "websites": c.websites,
        "birthday": c.birthday.map(Date::iso), "notes": c.notes, "categories": c.categories, "favorite": c.favorite,
        "importedFrom": c.imported_from, "account": c.origin.account, "keptAt": kept_at(mailbox, &c.origin),
        "keys": c.keys.iter().map(|k| json!({"kind": k.kind.name(), "fingerprint": super::keys::card_key_facts(k).fingerprint, "uri": k.uri})).collect::<Vec<_>>(),
    })
}

fn get(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    s.mailbox.contact(id).map(|c| contact_json(&s.mailbox, c)).ok_or_else(|| fail("the contact is gone"))
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let query = p::str(v, "query").unwrap_or(&s.view.people_search).to_string();
    let account = match v.get("account") {
        Some(a) => AccountChoice::parse(a).ok_or_else(|| bad("`account` is all, here or an account's id"))?,
        None => shown_account(s),
    };
    // As `view.peopleAccount`: an account whose contacts aren't read from its server is refused,
    // not listed as empty.
    if matches!(account, AccountChoice::Account(id) if !keeps_contacts(&s.mailbox, id)) {
        return Err(bad("that account's contacts aren't read from its server"));
    }
    let row = |c: &Contact| {
        let kept_at = kept_at(&s.mailbox, &c.origin);
        json!({"id": c.id, "name": c.name(), "email": c.primary_email(), "company": c.company, "favorite": c.favorite, "importedFrom": c.imported_from, "account": c.origin.account, "keptAt": kept_at})
    };
    let items: Vec<Value> = sorted_of(s, &query, account).into_iter().filter_map(|id| s.mailbox.contact(id)).map(row).collect();
    Ok(json!({"account": account.code(), "contacts": items}))
}

fn search(s: &mut Session, v: &Value) -> CmdResult {
    s.view.people_search = p::req_str(v, "query")?.chars().filter(|c| !c.is_control()).take(200).collect();
    Ok(json!({"query": s.view.people_search, "found": sorted(s, &s.view.people_search).len()}))
}

fn favorite(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let c = s.mailbox.contact_mut(id)?;
    c.favorite = p::bool(v, "value").unwrap_or(!c.favorite);
    Ok(json!({"id": id, "favorite": c.favorite}))
}

fn categorize(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let name = clean_name(p::req_str(v, "category")?).ok_or_else(|| bad("`category` is empty"))?;
    let c = s.mailbox.contact_mut(id)?;
    let (account, has) = (c.origin.account, c.categories.iter().any(|x| x.eq_ignore_ascii_case(&name)));
    let on = p::bool(v, "value").unwrap_or(!has);
    // The contact takes the spelling of its account's list; a new name joins that list.
    let spelling = super::category::spelled(s, account, &name, on)?;
    let c = s.mailbox.contact_mut(id)?;
    c.categories.retain(|x| !x.eq_ignore_ascii_case(&name));
    if on {
        c.categories.push(spelling);
    }
    Ok(json!({"id": id, "categories": c.categories}))
}

fn email(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let to = s.mailbox.contact(id).and_then(Contact::address).ok_or_else(|| fail("this contact has no e-mail address"))?;
    // (Beside a message being written, which stays open, saved first: #286.)
    s.run("mail.new", &json!({"to": to.full()}))
}

fn from_message(s: &mut Session, v: &Value) -> CmdResult {
    let mid = p::message_strict(v)?.or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let from = s.mailbox.message(mid).map(|m| m.from.clone()).ok_or_else(|| failed!("no message with id {id}", id = mid))?;
    if !from.is_valid() {
        return Err(fail("the sender has no usable e-mail address"));
    }
    if let Some(existing) = s.mailbox.contact_by_email(&from.email).map(|c| c.id) {
        s.sel.contact = Some(existing);
        reveal(s, existing);
        s.view.module = Module::People;
        return Ok(json!({"id": existing, "existing": true}));
    }
    // "First Last" splits at the last space; a single word is a first name.
    let (first, last) = match from.name.trim().rsplit_once(' ') {
        Some((f, l)) => (f.trim().to_string(), l.trim().to_string()),
        None => (from.name.trim().to_string(), String::new()),
    };
    let id = s.mailbox.add_contact(Contact { first, last, emails: vec![Labeled::new("work", from.email)], ..Default::default() });
    s.sel.contact = Some(id);
    reveal(s, id);
    s.view.module = Module::People;
    Ok(json!({"id": id, "existing": false}))
}

/// The contact a command that writes to other people about it names: `id`, read strictly (named
/// wrongly, it is refused, never taken to mean the selection), else the selected one.
fn named(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let id = p::id_strict(v, "id")?.or(s.sel.contact).ok_or_else(|| fail("select a contact first"))?;
    s.mailbox.contact(id).map(|c| c.id).ok_or_else(|| failed!("no contact with id {id}", id))
}

/// A new meeting with the contact invited: `calendar.newMeeting`, which says what goes to whom.
fn meeting(s: &mut Session, v: &Value) -> CmdResult {
    let id = named(s, v)?;
    let to = s.mailbox.contact(id).and_then(Contact::address).ok_or_else(|| fail("this contact has no e-mail address"))?;
    let mut fields = match v {
        Value::Object(m) => m.clone(),
        Value::Null => serde_json::Map::new(),
        _ => return Err(bad("the parameters are an object")),
    };
    fields.remove("id");
    let more = match fields.get("attendees") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(t)) => t.clone(),
        Some(_) => return Err(bad("`attendees` is text: \"a@x; b@y\"")),
    };
    let listed = Address::parse_list(&more).iter().any(|a| a.same_mailbox(&to));
    let attendees = if listed {
        more
    } else if more.trim().is_empty() {
        to.full()
    } else {
        format!("{}; {more}", to.full())
    };
    fields.insert("attendees".into(), json!(attendees));
    s.run("calendar.newMeeting", &Value::Object(fields))
}

/// A file name for a contact's card: its name, without what a file name can't have.
fn card_name(c: &Contact) -> String {
    let name: String =
        c.name().chars().filter(|ch| !ch.is_control() && !matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')).take(80).collect();
    let name = name.trim().trim_matches('.').trim();
    if name.is_empty() { "contact.vcf".to_string() } else { format!("{name}.vcf") }
}

/// Forward Contact: a new message with the contact's card attached, for the owner to send.
fn forward(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id", "discard"])?;
    let id = named(s, v)?;
    let c = s.mailbox.contact(id).cloned().ok_or_else(|| fail("the contact is gone"))?;
    let card = mailcraft_vcard::write(&[&c]);
    let subject = mailcraft_names::shown(&c.name(), 200);
    let compose = s.run("mail.new", &json!({"subject": subject}))?["compose"].clone();
    s.run(
        "compose.attach",
        &json!({"name": card_name(&c), "data": super::base64_encode(card.as_bytes()), "mime": "text/vcard", "compose": compose}),
    )?;
    Ok(json!({"composing": true, "contact": id, "compose": compose}))
}
