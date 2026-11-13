// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The changes of contacts made here, taken up to the server with `ContactCard/set` (RFC 9610 §3.5,
//! RFC 8620 §5.3), before the contacts are read. What is asked, and in what order, is told with the run (the
//! documentation of [`crate::contacts`]); here is how one change becomes what is asked, and what an answer is
//! taken for.

use std::collections::{HashMap, HashSet};

use mailcraft_model::remote::{ContactField, ContactOp, ContactOpKind, ContactOutcome, ContactResult, MAX_CONTACT_OPS};
use serde_json::{Map, Value, json};

use super::card::{birth, notes};
use super::{GET_BYTES, Run, write};
use crate::mail::refusal;
use crate::reader::Stop;
use crate::{HttpExchange, Id, Redactor};

/// The most the answer to one `ContactCard/set`, to the look for what an answer left out, and to the list
/// of the address books may be, in bytes.
const SET_BYTES: usize = 2 << 20;
/// Most entries of a list in an answer that are looked at, and most address books.
const MAX_LISTED: usize = 10_000;
const MAX_BOOKS: usize = 500;
/// The longest key of an entry of a card that is named in a patch (an `Id` of the standard is at most 255).
const MAX_KEY: usize = 255;
/// Most keys tried to find one a card does not have yet.
const MAX_KEYS: usize = 1_000;
/// What a change weighs in a request besides its texts, in bytes, and the least a call may
/// weigh before it is full.
const OP_BYTES: usize = 4 << 10;
const LEAST_BUDGET: usize = 64 << 10;

/// Why a change is refused here.
const NOT_AN_ID: &str = "the contact's id is not one a server hands out";
const NO_BOOK: &str = "the account has no address book that can be written to";
const SEVERAL_NOTES: &str = "the server keeps several notes with this contact";
const NO_NEW_ID: &str = "the server made the contact and did not say what it calls it";
const NO_KEY: &str = "the server keeps this contact in a way that can not be added to";

/// The parts that are changed entry by entry, so that the card as the server has it is read first.
const CAREFUL: [ContactField; 2] = [ContactField::Birthday, ContactField::Notes];

/// What is to be done about a change before the server is asked.
pub(super) enum Plan<'a> {
    /// A card is made.
    Create,
    /// The card of this id is changed or destroyed.
    Named(&'a str),
    /// It names no part: there is nothing to do, and it is done.
    Nothing,
    /// It cannot be asked.
    Refuse(&'static str),
}

/// Why a change that writes web addresses is not asked when one of them is none a card keeps.
const NOT_A_WEB_ADDRESS: &str = "a web address has to begin with http:// or https://";

pub(super) fn plan(op: &ContactOp) -> Plan<'_> {
    let writes_links = match op.kind {
        ContactOpKind::Create => true,
        ContactOpKind::Update => op.changed.contains(&ContactField::Websites),
        ContactOpKind::Delete => false,
    };
    match op.kind {
        _ if writes_links && super::write::has_unkept_link(&op.card.websites) => Plan::Refuse(NOT_A_WEB_ADDRESS),
        ContactOpKind::Create => Plan::Create,
        _ if Id::new(&op.id).is_err() => Plan::Refuse(NOT_AN_ID),
        ContactOpKind::Update if op.changed.is_empty() => Plan::Nothing,
        _ => Plan::Named(&op.id),
    }
}

/// What became of `op`.
fn outcome(op: &ContactOp, result: ContactResult) -> ContactOutcome {
    let changed = if op.kind == ContactOpKind::Update && result == ContactResult::Done { op.changed.clone() } else { Vec::new() };
    ContactOutcome { kind: op.kind, contact: op.contact, id: op.id.clone(), result, changed, error: String::new() }
}

fn refused(op: &ContactOp, why: &str) -> ContactOutcome {
    ContactOutcome { error: why.to_string(), ..outcome(op, ContactResult::Refused) }
}

/// Whether the update `op` changes a part for which the card as the server has it is read first.
fn careful(op: &ContactOp) -> bool {
    op.kind == ContactOpKind::Update && op.changed.iter().any(|field| CAREFUL.contains(field))
}

/// About how many bytes `op` adds to a request.
fn weight(op: &ContactOp) -> usize {
    let card = &op.card;
    let labeled = card.emails.iter().chain(&card.phones).map(|entry| entry.label.len().saturating_add(entry.value.len()));
    let postal =
        card.addresses.iter().map(|a| [&a.label, &a.street, &a.city, &a.region, &a.postal_code, &a.country].iter().map(|s| s.len()).sum::<usize>());
    let names = [&card.display_name, &card.first, &card.middle, &card.last, &card.nickname, &card.company, &card.title, &card.department];
    let texts = names.iter().map(|s| s.len()).chain(labeled).chain(postal).chain(card.websites.iter().map(String::len)).chain([card.notes.len()]);
    let texts = texts.fold(0usize, usize::saturating_add);
    // (A text may take two bytes for one where it is escaped.)
    OP_BYTES.saturating_add(texts.saturating_mul(2))
}

/// `key` as a part of a path of a patch (a JSON Pointer, RFC 6901 §3): an error for one too long to name.
fn pointer(key: &str) -> Result<String, &'static str> {
    if key.is_empty() || key.len() > MAX_KEY {
        return Err(NO_KEY);
    }
    Ok(key.replace('~', "~0").replace('/', "~1"))
}

/// Put into `patch` the one entry `entry` of the map `name` of a card (`None`: take that entry away), and
/// leave every other entry of the map as `server`, the card as the server has it, holds it. The entry is the
/// one of the key `target`; without one, a new entry under the first key from `prefix` that the map has not.
/// A card that has no such map gets one.
fn keyed(
    patch: &mut Map<String, Value>,
    server: &Map<String, Value>,
    name: &str,
    target: Option<&str>,
    prefix: &str,
    entry: Option<Value>,
) -> Result<(), &'static str> {
    let have = server.get(name).and_then(Value::as_object);
    match (entry, have, target) {
        (Some(entry), Some(_), Some(key)) => {
            patch.insert(format!("{name}/{}", pointer(key)?), entry);
        }
        (Some(entry), Some(have), None) => {
            let free = (1..=MAX_KEYS).map(|n| format!("{prefix}{n}")).find(|key| !have.contains_key(key)).ok_or(NO_KEY)?;
            patch.insert(format!("{name}/{free}"), entry);
        }
        (Some(entry), None, _) => {
            patch.insert(name.to_string(), json!({format!("{prefix}1"): entry}));
        }
        (None, Some(_), Some(key)) => {
            patch.insert(format!("{name}/{}", pointer(key)?), Value::Null);
        }
        // Nothing to take away.
        (None, _, _) => {}
    }
    Ok(())
}

/// The notes of `op`, for a card whose notes the server has as `server` holds them. Notes are replaced
/// whole, so they are replaced only where that loses nothing: the server has one note or none. Where it has
/// several and they read as the contact's notes already, there is nothing to do; else the change is refused.
fn notes_patch(patch: &mut Map<String, Value>, server: &Map<String, Value>, op: &ContactOp) -> Result<(), &'static str> {
    let want = write::written(ContactField::Notes, &op.card)?;
    // (No notes at all, however the server says that.)
    let have = server.get("notes").filter(|have| !have.is_null() && have.as_object().is_none_or(|have| !have.is_empty()));
    let several = have.and_then(Value::as_object).is_some_and(|have| have.len() > 1);
    if have == want.as_ref() || (several && notes(have) == write::noted(&op.card.notes)) {
        return Ok(());
    }
    if several {
        return Err(SEVERAL_NOTES);
    }
    patch.insert(write::property(ContactField::Notes).to_string(), want.unwrap_or(Value::Null));
    Ok(())
}

/// The patch that the update `op` is (RFC 8620 §5.3): the properties of the parts it names, each whole (or
/// `null` for a part that is empty now), and for the birthday and the notes what leaves the card's other
/// anniversaries and notes alone, given the card as the server has it (`server`, which is looked at for
/// those two only). Nothing else of the card is named. Empty: there is nothing to change.
pub(super) fn patch(op: &ContactOp, server: &Map<String, Value>) -> Result<Map<String, Value>, &'static str> {
    let mut patch = Map::new();
    for field in ContactField::ALL.into_iter().filter(|field| op.changed.contains(field)) {
        let name = write::property(field);
        match field {
            ContactField::Birthday => {
                let entry = op.card.birthday.map(write::anniversary).transpose()?;
                keyed(&mut patch, server, name, birth(server).map(|(key, _)| key), write::BIRTH, entry)?;
            }
            ContactField::Notes => notes_patch(&mut patch, server, op)?,
            _ => {
                patch.insert(name.to_string(), write::written(field, &op.card)?.unwrap_or(Value::Null));
            }
        }
    }
    Ok(patch)
}

/// What the answer to a `ContactCard/set` says of `op`, which was asked under `key` (its creation id, or the
/// id of its card): `None` when it says nothing of it. Whatever the answer is, this reads it without
/// trusting it: only this key is looked up.
pub(super) fn outcome_of(answer: &Map<String, Value>, op: &ContactOp, key: &str, secrets: &Redactor) -> Option<ContactOutcome> {
    let under = |name: &str| answer.get(name).and_then(Value::as_object).and_then(|map| map.get(key));
    let (done, failed) = match op.kind {
        ContactOpKind::Create => {
            if let Some(made) = under("created") {
                let id = made.get("id").and_then(Value::as_str).filter(|id| Id::new(id).is_ok());
                return Some(match id {
                    Some(id) => ContactOutcome { id: id.to_string(), ..outcome(op, ContactResult::Done) },
                    None => refused(op, NO_NEW_ID),
                });
            }
            (false, under("notCreated"))
        }
        ContactOpKind::Update => (under("updated").is_some(), under("notUpdated")),
        ContactOpKind::Delete => {
            let destroyed = answer.get("destroyed").and_then(Value::as_array);
            (destroyed.is_some_and(|ids| ids.iter().take(MAX_LISTED).any(|gone| gone.as_str() == Some(key))), under("notDestroyed"))
        }
    };
    if done {
        return Some(outcome(op, ContactResult::Done));
    }
    let failed = failed?;
    Some(match failed.get("type").and_then(Value::as_str) {
        Some("notFound") if op.kind != ContactOpKind::Create => outcome(op, ContactResult::Gone),
        _ => refused(op, &refusal(secrets, failed)),
    })
}

/// The address book a new card goes into, of those the answer to an `AddressBook/get` lists: the one that
/// is the default, else the first by their order and their ids, of those the account may write to.
pub(super) fn book_of(answer: &Map<String, Value>) -> Option<String> {
    let list = answer.get("list").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut books: Vec<(bool, u64, &str)> = list
        .iter()
        .take(MAX_BOOKS)
        .filter(|book| book.get("myRights").and_then(|rights| rights.get("mayWrite")).and_then(Value::as_bool) != Some(false))
        .filter_map(|book| {
            let id = book.get("id").and_then(Value::as_str).filter(|id| Id::new(id).is_ok())?;
            let default = book.get("isDefault").and_then(Value::as_bool) == Some(true);
            Some((!default, book.get("sortOrder").and_then(Value::as_u64).unwrap_or(0), id))
        })
        .collect();
    books.sort_unstable();
    books.first().map(|(_, _, id)| (*id).to_string())
}

/// What is known of the address book new cards go into.
enum Book {
    /// Not asked yet.
    Unknown,
    Is(String),
    /// The account has none it may write to.
    None,
    /// It could not be asked: the cards that would be made wait for the next run.
    Failed,
}

/// The changes of one run as they go up.
struct Ups<'r> {
    book: Book,
    /// Cards whose later changes wait for the next run: an earlier one was refused or went unanswered.
    waiting: HashSet<String>,
    done: &'r mut Vec<ContactOutcome>,
    /// The first trouble of a call that did not end the run.
    line: Option<String>,
    asked: usize,
}

impl<H: HttpExchange> Run<'_, H> {
    /// Take the changes `ops` up, and say in `done` what became of each that was answered. An error is what
    /// ends the run (stopped, the sign-in refused, the server busy); a call that failed otherwise is a line
    /// (`up`), and the run goes on to read.
    pub(super) async fn changes(&self, ops: &[ContactOp], up: &mut Option<String>, done: &mut Vec<ContactOutcome>) -> Result<(), Stop> {
        if ops.is_empty() {
            return Ok(());
        }
        let limits = self.calls.connection.client().limits();
        let at_once = usize::try_from(limits.max_objects_in_set()).unwrap_or(usize::MAX).clamp(1, MAX_CONTACT_OPS);
        let budget = usize::try_from(limits.max_size_request() / 2).unwrap_or(usize::MAX).max(LEAST_BUDGET);
        let mut ups = Ups { book: Book::Unknown, waiting: HashSet::new(), done, line: None, asked: 0 };
        let mut rest = ops.iter().enumerate().take(MAX_CONTACT_OPS).peekable();
        let mut ended = Ok(());
        while rest.peek().is_some() {
            // The next call's changes: as many as the server takes, no two of one card, and no more than
            // fit a request. What is known of a change without asking is said at once.
            let mut chunk: Vec<(usize, &ContactOp)> = Vec::new();
            let mut named: HashSet<&str> = HashSet::new();
            let mut weighs = 0usize;
            while chunk.len() < at_once {
                let Some((at, op)) = rest.peek().copied() else { break };
                let id = match plan(op) {
                    Plan::Refuse(why) => {
                        ups.done.push(refused(op, why));
                        rest.next();
                        continue;
                    }
                    Plan::Nothing => {
                        ups.done.push(outcome(op, ContactResult::Done));
                        rest.next();
                        continue;
                    }
                    Plan::Create => None,
                    Plan::Named(id) => Some(id),
                };
                if id.is_some_and(|id| ups.waiting.contains(id)) {
                    rest.next();
                    continue;
                }
                let with = weighs.saturating_add(weight(op));
                if !chunk.is_empty() && (with > budget || id.is_some_and(|id| named.contains(id))) {
                    break;
                }
                weighs = with;
                named.extend(id);
                chunk.push((at, op));
                rest.next();
            }
            if chunk.is_empty() {
                continue;
            }
            match self.call(chunk, &mut ups).await {
                Ok(()) => {}
                // This call's trouble: none of its changes is answered, and the rest wait.
                Err(Stop::Failed(line) | Stop::Down(line)) => {
                    ups.line.get_or_insert(line);
                    break;
                }
                Err(stop) => {
                    ended = Err(stop);
                    break;
                }
            }
        }
        log::debug!("jmap contacts: {} of {} changes were answered ({} asked of the server)", ups.done.len(), ops.len(), ups.asked);
        if ups.line.is_some() {
            log::info!("jmap contacts: a call that takes changes up failed; they are worked out again for the next run");
        }
        *up = ups.line;
        ended
    }

    /// One `ContactCard/set` for `chunk` (each change with its place among the request's), after what has to
    /// be known first: the address book, for a card that is made, and the cards as the server has them, for
    /// those changed entry by entry.
    async fn call(&self, chunk: Vec<(usize, &ContactOp)>, ups: &mut Ups<'_>) -> Result<(), Stop> {
        if matches!(ups.book, Book::Unknown) && chunk.iter().any(|(_, op)| op.kind == ContactOpKind::Create) {
            ups.book = self.book(ups).await?;
        }
        let read_first: Vec<&str> = chunk.iter().filter(|(_, op)| careful(op)).map(|(_, op)| op.id.as_str()).collect();
        // (Of those cards, only what the changes of this call have to leave alone.)
        let parts = CAREFUL.into_iter().filter(|field| chunk.iter().any(|(_, op)| careful(op) && op.changed.contains(field)));
        let properties: Vec<&str> = ["id"].into_iter().chain(parts.map(write::property)).collect();
        let had = if read_first.is_empty() { None } else { self.had(&read_first, &properties, ups).await? };
        let mut cards: HashMap<&str, &Map<String, Value>> = HashMap::new();
        for entry in had.iter().flat_map(|had| had.get("list").and_then(Value::as_array)).flatten().take(MAX_LISTED) {
            if let Some(card) = entry.as_object()
                && let Some(id) = card.get("id").and_then(Value::as_str).and_then(|id| read_first.iter().find(|asked| **asked == id).copied())
            {
                cards.entry(id).or_insert(card);
            }
        }
        let gone = not_found(had.as_ref());
        let nothing = Map::new();

        let (mut create, mut update, mut destroy) = (Map::new(), Map::new(), Vec::new());
        let mut asked: Vec<(&ContactOp, String)> = Vec::new();
        for (at, op) in chunk {
            match op.kind {
                ContactOpKind::Create => match &ups.book {
                    Book::Is(book) => match write::new_card(book, &op.card) {
                        Ok(card) => {
                            // (Named by its place among the changes: nothing of the contact is in it.)
                            let key = format!("k{at}");
                            create.insert(key.clone(), Value::Object(card));
                            asked.push((op, key));
                        }
                        Err(why) => ups.done.push(refused(op, why)),
                    },
                    Book::None => ups.done.push(refused(op, NO_BOOK)),
                    Book::Unknown | Book::Failed => {}
                },
                ContactOpKind::Update => {
                    let server = if !careful(op) {
                        &nothing
                    } else if let Some(card) = cards.get(op.id.as_str()) {
                        card
                    } else {
                        // The server has not got it, or did not say: then the change waits.
                        if gone.contains(op.id.as_str()) {
                            ups.done.push(outcome(op, ContactResult::Gone));
                        } else {
                            ups.waiting.insert(op.id.clone());
                        }
                        continue;
                    };
                    match patch(op, server) {
                        Ok(patch) if patch.is_empty() => ups.done.push(outcome(op, ContactResult::Done)),
                        Ok(patch) => {
                            update.insert(op.id.clone(), Value::Object(patch));
                            asked.push((op, op.id.clone()));
                        }
                        Err(why) => {
                            ups.waiting.insert(op.id.clone());
                            ups.done.push(refused(op, why));
                        }
                    }
                }
                ContactOpKind::Delete => {
                    destroy.push(Value::String(op.id.clone()));
                    asked.push((op, op.id.clone()));
                }
            }
        }
        if asked.is_empty() {
            return Ok(());
        }
        let mut args = Map::new();
        args.insert("accountId".into(), Value::String(self.calls.account.as_str().to_string()));
        for (name, part) in [("create", create), ("update", update)] {
            if !part.is_empty() {
                args.insert(name.into(), Value::Object(part));
            }
        }
        if !destroy.is_empty() {
            args.insert("destroy".into(), Value::Array(destroy));
        }
        ups.asked = ups.asked.saturating_add(asked.len());
        let answer = self.calls.one_call("ContactCard/set", Value::Object(args), SET_BYTES).await?;

        // Changes the answer says nothing of. A server that keeps to the specification names every id it
        // was asked about; one that passes over a card it does not have would be asked the same again at
        // every run, so those are looked up, and what the server has not got is gone.
        let mut silent: Vec<&ContactOp> = Vec::new();
        for (op, key) in asked {
            match outcome_of(&answer, op, &key, &self.calls.secrets) {
                Some(outcome) => {
                    if outcome.result == ContactResult::Refused && op.kind != ContactOpKind::Create {
                        ups.waiting.insert(key);
                    }
                    ups.done.push(outcome);
                }
                None if op.kind == ContactOpKind::Create => {}
                None => {
                    ups.waiting.insert(key);
                    silent.push(op);
                }
            }
        }
        if silent.is_empty() {
            return Ok(());
        }
        let ids: Vec<&str> = silent.iter().map(|op| op.id.as_str()).collect();
        let args = json!({"accountId": self.calls.account.as_str(), "ids": ids, "properties": ["id"]});
        match self.calls.one_call("ContactCard/get", args, SET_BYTES).await {
            Ok(found) => {
                let gone = not_found(Some(&found));
                for op in silent.into_iter().filter(|op| gone.contains(op.id.as_str())) {
                    ups.done.push(outcome(op, ContactResult::Gone));
                }
            }
            // The look alone failed: they are asked again with the next run.
            Err(Stop::Failed(_) | Stop::Down(_)) => {}
            Err(stop) => return Err(stop),
        }
        Ok(())
    }

    /// `AddressBook/get`: the address book new cards go into. A call that fails for itself is a line, and
    /// the cards wait.
    async fn book(&self, ups: &mut Ups<'_>) -> Result<Book, Stop> {
        let args = json!({"accountId": self.calls.account.as_str(), "ids": null, "properties": ["id", "isDefault", "sortOrder", "myRights"]});
        match self.calls.one_call("AddressBook/get", args, SET_BYTES).await {
            Ok(answer) => Ok(book_of(&answer).map_or(Book::None, Book::Is)),
            Err(Stop::Failed(line)) => {
                ups.line.get_or_insert(line);
                Ok(Book::Failed)
            }
            Err(stop) => Err(stop),
        }
    }

    /// `ContactCard/get` of the cards `ids`, for what an update of a birthday or the notes has to leave
    /// alone: of their anniversaries and notes, the `properties` asked for. `None`, and a line,
    /// when the call fails for itself: those changes wait.
    async fn had(&self, ids: &[&str], properties: &[&str], ups: &mut Ups<'_>) -> Result<Option<Map<String, Value>>, Stop> {
        let args = json!({"accountId": self.calls.account.as_str(), "ids": ids, "properties": properties});
        match self.calls.one_call("ContactCard/get", args, GET_BYTES).await {
            Ok(answer) => Ok(Some(answer)),
            Err(Stop::Failed(line)) => {
                ups.line.get_or_insert(line);
                Ok(None)
            }
            Err(stop) => Err(stop),
        }
    }
}

/// The ids an answer to a `get` says the server has not got.
fn not_found(answer: Option<&Map<String, Value>>) -> HashSet<&str> {
    let list = answer.and_then(|answer| answer.get("notFound")).and_then(Value::as_array);
    list.into_iter().flatten().take(MAX_LISTED).filter_map(Value::as_str).collect()
}
