// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts on the stand-in (JMAP for Contacts, RFC 9610), once a test turns them on
//! ([`Contacts::on`]): the capability in the session, for the account and for another one a person
//! shared address books from ([`Contacts::shared`]), and `AddressBook/get`, `ContactCard/get`,
//! `ContactCard/changes`, `ContactCard/query` and `ContactCard/set`.
//!
//! - **Cards** are JSON as a client sent them; `id` and `addressBookIds` are the server's to check: a card
//!   is in one address book of its account at least, which the user may write to (`forbidden`), and has a
//!   `uid` no other card of the account has (`invalidProperties`). An update is a patch of whole
//!   properties (a path with a `/` in it, too, one level down); `id` and `uid` can't be patched.
//! - **States.** One counter for the whole stand-in, `c<n>`; `ContactCard/changes` answers from its log, at
//!   most `maxChanges` ids, and says `cannotCalculateChanges` for a state older than
//!   [`Contacts::forget_before`] or one it never gave.
//! - **Trouble on demand.** [`Contacts::refused`] makes `ContactCard/set` refuse a card (by id; `new` for
//!   every card it is asked to make) with the error given; [`Contacts::query_moves`] gives every
//!   `ContactCard/query` a state of its own, as a server does whose cards change while they are read.
//! - **What it was sent.** [`Contacts::patches`]: every patch of an update, as it came.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::changing::{id_list, object, or_null, set_error};
use super::{ACCOUNT, Data, Outcome, allow_only, ids, invalid, number, refuse};

/// The capability of contacts.
pub const CONTACTS: &str = "urn:ietf:params:jmap:contacts";

/// An address book on the stand-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Book {
    /// The account it is in: [`ACCOUNT`], or the shared one's id.
    pub account: String,
    pub id: String,
    pub name: String,
    pub default: bool,
    pub may_read: bool,
    pub may_write: bool,
}

/// What the stand-in holds of contacts.
#[derive(Default)]
pub struct Contacts {
    /// The session offers contacts, and their methods are there.
    pub on: bool,
    /// Another account, a person's who shared address books: its id and its name.
    pub shared: Option<(String, String)>,
    pub books: Vec<Book>,
    /// The cards, by id: the account each is in, and the card.
    pub cards: BTreeMap<String, (String, Value)>,
    /// A state older than this one can't be counted from.
    pub forget_before: u32,
    pub refused: BTreeMap<String, (String, String)>,
    pub query_moves: bool,
    pub patches: Vec<(String, Value)>,
    /// What changed, in order: the state it made, the account, the card and how.
    log: Vec<(u32, String, String, Change)>,
    version: u32,
    next: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    Created,
    Updated,
    Destroyed,
}

impl Contacts {
    fn state(&self) -> String {
        format!("c{}", self.version)
    }

    fn note(&mut self, account: &str, id: &str, change: Change) {
        self.version = self.version.saturating_add(1);
        self.log.push((self.version, account.to_string(), id.to_string(), change));
    }

    /// An address book of `account` (writable, readable): its id.
    pub fn book(&mut self, account: &str, name: &str, default: bool) -> String {
        self.next = self.next.saturating_add(1);
        let id = format!("AB{}", self.next);
        self.books.push(Book { account: account.into(), id: id.clone(), name: name.into(), default, may_read: true, may_write: true });
        id
    }

    /// A card made on the server (by another device): its id.
    pub fn card(&mut self, account: &str, mut card: Value) -> String {
        self.next = self.next.saturating_add(1);
        let id = format!("CC{}", self.next);
        if let Some(map) = card.as_object_mut() {
            map.insert("id".into(), json!(id));
        }
        self.cards.insert(id.clone(), (account.to_string(), card));
        self.note(account, &id, Change::Created);
        id
    }

    /// A card changed on the server: `property` set to `value` (`null` takes it away).
    pub fn change(&mut self, id: &str, property: &str, value: Value) -> bool {
        let Some((account, card)) = self.cards.get_mut(id) else { return false };
        let account = account.clone();
        if let Some(map) = card.as_object_mut() {
            if value.is_null() {
                map.remove(property);
            } else {
                map.insert(property.into(), value);
            }
        }
        self.note(&account, id, Change::Updated);
        true
    }

    /// A card deleted on the server.
    pub fn remove(&mut self, id: &str) -> bool {
        let Some((account, _)) = self.cards.remove(id) else { return false };
        self.note(&account, id, Change::Destroyed);
        true
    }

    /// The card `id`, as the server has it.
    pub fn get(&self, id: &str) -> Option<&Value> {
        self.cards.get(id).map(|(_, card)| card)
    }

    /// The first card whose full name (`name/full`) is `name`: its id and the card.
    pub fn named(&self, name: &str) -> Option<(&String, &Value)> {
        self.cards.iter().find(|(_, (_, c))| c.pointer("/name/full").and_then(Value::as_str) == Some(name)).map(|(id, (_, c))| (id, c))
    }
}

impl Data {
    /// The account a contacts method names: this one, or the shared one.
    fn contacts_account(&self, args: &Map<String, Value>) -> Outcome<String> {
        match args.get("accountId").and_then(Value::as_str) {
            Some(ACCOUNT) => Ok(ACCOUNT.to_string()),
            Some(id) if self.contacts.shared.as_ref().is_some_and(|(shared, _)| shared == id) => Ok(id.to_string()),
            Some(_) => Err(refuse("accountNotFound", "no such account")),
            None => Err(invalid("accountId is needed")),
        }
    }

    /// The session, with contacts in it when they are on.
    pub(super) fn with_contacts(&self, mut session: Value) -> Value {
        if !self.contacts.on {
            return session;
        }
        for (at, says) in [
            ("/capabilities".to_string(), json!({})),
            (format!("/accounts/{ACCOUNT}/accountCapabilities"), json!({"maxAddressBooksPerCard": null, "mayCreateAddressBook": false})),
            ("/primaryAccounts".to_string(), json!(ACCOUNT)),
        ] {
            if let Some(Value::Object(map)) = session.pointer_mut(&at) {
                map.insert(CONTACTS.to_string(), says);
            }
        }
        if let Some((id, name)) = &self.contacts.shared
            && let Some(Value::Object(accounts)) = session.pointer_mut("/accounts")
        {
            accounts.insert(
                id.clone(),
                json!({"name": name, "isPersonal": false, "isReadOnly": false, "accountCapabilities": {CONTACTS: {"maxAddressBooksPerCard": null, "mayCreateAddressBook": false}}}),
            );
        }
        session
    }

    /// One of the methods of contacts.
    pub(super) fn contacts_method(&mut self, name: &str, args: &Map<String, Value>) -> Outcome<Value> {
        match name {
            "AddressBook/get" => self.address_book_get(args),
            "ContactCard/get" => self.card_get(args),
            "ContactCard/changes" => self.card_changes(args),
            "ContactCard/query" => self.card_query(args),
            _ => self.card_set(args),
        }
    }

    fn address_book_get(&self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "ids", "properties"])?;
        let account = self.contacts_account(args)?;
        let wanted = ids(args, self.max_get)?;
        let list: Vec<Value> = self
            .contacts
            .books
            .iter()
            .filter(|b| b.account == account && wanted.as_ref().is_none_or(|w| w.contains(&b.id)))
            .map(|b| {
                json!({"id": b.id, "name": b.name, "isDefault": b.default, "sortOrder": 0, "isSubscribed": true,
                    "myRights": {"mayRead": b.may_read, "mayWrite": b.may_write, "mayShare": false, "mayDelete": false}})
            })
            .collect();
        Ok(json!({"accountId": account, "state": self.contacts.state(), "list": list, "notFound": []}))
    }

    fn card_get(&self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "ids", "properties"])?;
        let account = self.contacts_account(args)?;
        let wanted = ids(args, self.max_get)?;
        let properties: Option<Vec<String>> = match args.get("properties") {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => Some(items.iter().filter_map(Value::as_str).map(str::to_string).collect()),
            Some(_) => return Err(invalid("properties is a list, or null")),
        };
        let readable = |c: &Value| {
            c.get("addressBookIds")
                .and_then(Value::as_object)
                .is_some_and(|ids| ids.keys().any(|b| self.contacts.books.iter().any(|book| &book.id == b && book.may_read)))
        };
        let shown = |c: &Value| match &properties {
            None => c.clone(),
            Some(names) => Value::Object(
                c.as_object().into_iter().flatten().filter(|(k, _)| *k == "id" || names.contains(k)).map(|(k, v)| (k.clone(), v.clone())).collect(),
            ),
        };
        let mut list = Vec::new();
        let mut not_found = Vec::new();
        let ids: Vec<String> = match wanted {
            Some(ids) => ids,
            None => self.contacts.cards.iter().filter(|(_, (a, _))| *a == account).map(|(id, _)| id.clone()).collect(),
        };
        for id in ids {
            match self.contacts.cards.get(&id) {
                Some((a, c)) if *a == account && readable(c) => list.push(shown(c)),
                _ => not_found.push(id),
            }
        }
        Ok(json!({"accountId": account, "state": self.contacts.state(), "list": list, "notFound": not_found}))
    }

    fn card_changes(&self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "sinceState", "maxChanges"])?;
        let account = self.contacts_account(args)?;
        let since = args.get("sinceState").and_then(Value::as_str).ok_or_else(|| invalid("sinceState is a string"))?;
        let most = number(args, "maxChanges")?.unwrap_or(usize::MAX).max(1);
        let from =
            since.strip_prefix('c').and_then(|n| n.parse::<u32>().ok()).filter(|n| *n >= self.contacts.forget_before && *n <= self.contacts.version);
        let Some(from) = from else { return Err(refuse("cannotCalculateChanges", "that state is too old")) };
        let mut seen: Vec<(String, Change, Change)> = Vec::new();
        let mut new_state = self.contacts.version;
        let mut more = false;
        for (version, _, id, change) in self.contacts.log.iter().filter(|(v, a, ..)| *v > from && *a == account) {
            let full = seen.len() >= most;
            match seen.iter_mut().find(|(had, ..)| had == id) {
                Some(entry) => entry.2 = *change,
                None if full => {
                    more = true;
                    new_state = version.saturating_sub(1);
                    break;
                }
                None => seen.push((id.clone(), *change, *change)),
            }
        }
        let (mut created, mut updated, mut destroyed) = (Vec::new(), Vec::new(), Vec::new());
        for (id, first, last) in seen {
            match (first, last) {
                (Change::Created, Change::Destroyed) => {}
                (_, Change::Destroyed) => destroyed.push(id),
                (Change::Created, _) => created.push(id),
                _ => updated.push(id),
            }
        }
        Ok(json!({"accountId": account, "oldState": since, "newState": format!("c{new_state}"), "hasMoreChanges": more,
            "created": created, "updated": updated, "destroyed": destroyed}))
    }

    fn card_query(&mut self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "filter", "sort", "position", "limit", "calculateTotal"])?;
        let account = self.contacts_account(args)?;
        let book = args.get("filter").and_then(|f| f.get("inAddressBook")).and_then(Value::as_str).map(str::to_string);
        let all: Vec<&String> = self
            .contacts
            .cards
            .iter()
            .filter(|(_, (a, c))| {
                *a == account && book.as_ref().is_none_or(|b| c.pointer(&format!("/addressBookIds/{b}")) == Some(&Value::Bool(true)))
            })
            // (What the user may not read isn't listed either.)
            .filter(|(_, (_, c))| {
                c.get("addressBookIds")
                    .and_then(Value::as_object)
                    .is_some_and(|ids| ids.keys().any(|b| self.contacts.books.iter().any(|book| &book.id == b && book.may_read)))
            })
            .map(|(id, _)| id)
            .collect();
        let position = number(args, "position")?.unwrap_or(0);
        let limit = number(args, "limit")?.unwrap_or(usize::MAX).min(self.max_get);
        let ids: Vec<&String> = all.iter().skip(position).take(limit).copied().collect();
        let total = all.len();
        let query_state = if self.contacts.query_moves {
            self.contacts.next = self.contacts.next.saturating_add(1);
            format!("q{}", self.contacts.next)
        } else {
            self.contacts.state()
        };
        Ok(
            json!({"accountId": account, "queryState": query_state, "canCalculateChanges": false, "position": position, "ids": ids, "total": total, "limit": limit}),
        )
    }

    /// Whether `card` may be in the address books it names, in `account`: the error when not.
    fn card_fits(&self, account: &str, card: &Map<String, Value>, own: Option<&str>) -> Result<(), Value> {
        let books: Vec<&String> = card.get("addressBookIds").and_then(Value::as_object).map(|m| m.keys().collect()).unwrap_or_default();
        if books.is_empty() || books.iter().any(|b| !self.contacts.books.iter().any(|book| &book.id == *b && book.account == account)) {
            return Err(
                json!({"type": "invalidProperties", "description": "a card is in an address book of its account", "properties": ["addressBookIds"]}),
            );
        }
        if books.iter().any(|b| self.contacts.books.iter().any(|book| &book.id == *b && !book.may_write)) {
            return Err(set_error("forbidden", "the address book can only be read"));
        }
        let uid = card.get("uid").and_then(Value::as_str).unwrap_or("");
        let taken = self
            .contacts
            .cards
            .iter()
            .any(|(id, (a, c))| a == account && Some(id.as_str()) != own && c.get("uid").and_then(Value::as_str) == Some(uid));
        if uid.is_empty() || taken {
            return Err(json!({"type": "invalidProperties", "description": "a card has a uid of its own", "properties": ["uid"]}));
        }
        Ok(())
    }

    fn card_set(&mut self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "ifInState", "create", "update", "destroy"])?;
        let account = self.contacts_account(args)?;
        let old = self.contacts.state();
        let (Some(create), Some(update)) = (object(args, "create"), object(args, "update")) else {
            return Err(invalid("create and update are objects, or null"));
        };
        let destroy = id_list(args, "destroy")?;
        let count = create.map_or(0, Map::len).saturating_add(update.map_or(0, Map::len)).saturating_add(destroy.len());
        if count > self.max_set {
            return Err(refuse("requestTooLarge", format!("{count} changes, and {} are taken at a time", self.max_set)));
        }
        let refused = |data: &Data, id: &str| data.contacts.refused.get(id).map(|(kind, why)| set_error(kind, why));
        let (mut created, mut not_created) = (Map::new(), Map::new());
        for (creation, card) in create.into_iter().flatten() {
            let made = match (refused(self, "new"), card.as_object()) {
                (Some(error), _) => Err(error),
                (None, None) => Err(set_error("invalidProperties", "a card is an object")),
                (None, Some(card)) if card.contains_key("id") => Err(set_error("invalidProperties", "the server gives the id")),
                (None, Some(card)) => self.card_fits(&account, card, None).map(|()| card.clone()),
            };
            match made {
                Ok(card) => {
                    let id = self.contacts.card(&account, Value::Object(card));
                    created.insert(creation.clone(), json!({"id": id}));
                }
                Err(error) => {
                    not_created.insert(creation.clone(), error);
                }
            }
        }
        let (mut updated, mut not_updated) = (Map::new(), Map::new());
        for (id, patch) in update.into_iter().flatten() {
            self.contacts.patches.push((id.clone(), patch.clone()));
            let result = match (refused(self, id), self.contacts.cards.get(id), patch.as_object()) {
                (Some(error), ..) => Err(error),
                (None, Some((a, _)), _) if *a != account => Err(set_error("notFound", "no such card")),
                (None, None, _) => Err(set_error("notFound", "no such card")),
                (None, _, None) => Err(set_error("invalidPatch", "a patch is an object")),
                (None, Some((_, card)), Some(patch)) => {
                    let mut card = card.as_object().cloned().unwrap_or_default();
                    let mut bad = None;
                    for (path, value) in patch {
                        let (top, below) = path.split_once('/').map_or((path.as_str(), None), |(t, b)| (t, Some(b)));
                        if matches!(top, "id" | "uid") || below.is_some_and(|b| b.contains('/')) {
                            bad = Some(set_error("invalidPatch", "that property can't be patched"));
                            break;
                        }
                        match below {
                            None if value.is_null() => {
                                card.remove(top);
                            }
                            None => {
                                card.insert(top.to_string(), value.clone());
                            }
                            Some(key) => match card.get_mut(top).and_then(Value::as_object_mut) {
                                Some(inner) if value.is_null() => {
                                    inner.remove(key);
                                }
                                Some(inner) => {
                                    inner.insert(key.to_string(), value.clone());
                                }
                                None => {
                                    bad = Some(set_error("invalidPatch", "the path isn't there"));
                                    break;
                                }
                            },
                        }
                    }
                    match bad {
                        Some(error) => Err(error),
                        None => self.card_fits(&account, &card, Some(id)).map(|()| card),
                    }
                }
            };
            match result {
                Ok(card) => {
                    if let Some(entry) = self.contacts.cards.get_mut(id) {
                        entry.1 = Value::Object(card);
                    }
                    self.contacts.note(&account, id, Change::Updated);
                    updated.insert(id.clone(), Value::Null);
                }
                Err(error) => {
                    not_updated.insert(id.clone(), error);
                }
            }
        }
        let (mut destroyed, mut not_destroyed) = (Vec::new(), Map::new());
        for id in destroy {
            let writable = self
                .contacts
                .get(&id)
                .and_then(|c| c.get("addressBookIds"))
                .and_then(Value::as_object)
                .is_some_and(|m| m.keys().all(|b| self.contacts.books.iter().any(|book| &book.id == b && book.may_write)));
            match refused(self, &id) {
                Some(error) => {
                    not_destroyed.insert(id, error);
                }
                None if self.contacts.cards.get(&id).is_none_or(|(a, _)| *a != account) => {
                    not_destroyed.insert(id, set_error("notFound", "no such card"));
                }
                None if !writable => {
                    not_destroyed.insert(id, set_error("forbidden", "the address book can only be read"));
                }
                None => {
                    self.contacts.remove(&id);
                    destroyed.push(id);
                }
            }
        }
        Ok(json!({
            "accountId": account,
            "oldState": old,
            "newState": self.contacts.state(),
            "created": or_null(created),
            "updated": or_null(updated),
            "destroyed": if destroyed.is_empty() { Value::Null } else { json!(destroyed) },
            "notCreated": or_null(not_created),
            "notUpdated": or_null(not_updated),
            "notDestroyed": or_null(not_destroyed),
        }))
    }
}
