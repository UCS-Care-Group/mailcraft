// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account's contacts kept in step after its mail (JMAP for Contacts, RFC 9610; #1068, #1076), when the
//! server offers them and the request asks ([`JmapMailRequest::contacts`]).
//!
//! **Whose.** The account's own address books (the session's primary account for contacts), and those
//! of every other account the session lists with contacts that isn't the user's own (`isPersonal`
//! false): a person who shared address books, known by the account's name when that is an e-mail
//! address ([`FolderOwner::Person`]); one that isn't is passed over. [`MAX_ACCOUNTS`] at the most.
//!
//! **In this order**, for each of them:
//!
//! 1. `AddressBook/get`: the address books, each a contacts folder. The default one (`isDefault`, else
//!    the first by id) is its owner's main folder (server id empty, as the model has it). One the user
//!    may not read (`myRights.mayRead`) is passed over; one is writable when `myRights.mayWrite` says so
//!    and the account isn't read-only. [`MAX_CONTACT_FOLDERS`] of them in a run at the most.
//! 2. The changes made here (`ContactCard/set`), the server's `maxObjectsInSet` and [`MAX_SET`] to a call:
//!    a new contact is a whole card in its folder's address book; a change is a patch of only the
//!    properties of the fields the owner changed, worked out from the card as the server has it now
//!    (`ContactCard/get` first: a card it no longer has is gone), so nothing else of the card is
//!    written over (ADR 0028); a deletion takes the card out of its folder's address book, and
//!    destroys it only when that was the last one it was in (as the card read just before says: a
//!    book another device adds it to between that read and the `set` is not seen, a window JMAP gives
//!    no per-card guard for; when the read doesn't say which books it is in, it isn't destroyed).
//!    `ifInState` is not sent: a card's state is
//!    the whole account's, and a change elsewhere to another card must not refuse this one (changes
//!    merge field by field, in the engine). What each became of: done; gone (`notFound`); denied
//!    (`forbidden`); refused for good (`invalidProperties`, `overQuota`, `tooLarge` and anything else
//!    the server says no with); failed for now (`serverFail`, `serverUnavailable`, `rateLimit`, a call
//!    that failed or got no answer), to be tried again later.
//! 3. The cards: from where they were last read up to (`ContactCard/changes` from the owner's state,
//!    [`MAX_CHANGES`] to a call, [`MAX_ROUNDS`] calls in a run, the rest the next run), or all of them
//!    (`ContactCard/query` and `ContactCard/get`, a page at a time, [`MAX_CONTACTS`] at the most) when there
//!    is no state, the server can't count changes from it (`cannotCalculateChanges`), or it lists an
//!    address book that isn't here yet. A list read whole is complete only when the server's query
//!    stayed the same while it was read and nothing was left out. A group card is no contact
//!    ([`card::read`]).
//!
//! What comes back is one [`ContactsDownload`] for each address book listed, with the state its cards
//! were read up to (none for a whole read that wasn't complete: the next run reads whole again), the
//! cards it has, and, read from changes, the cards gone from it: destroyed, or now only in other books.
//! A card in several address books is in each one's download.
//!
//! **Trouble.** A refused sign-in, a busy server and a stop end the contacts there, as for mail (what
//! was answered is handed on, the rest waits); any other failure of an account's step is one line, and
//! the next account goes on. Everything the server says is hostile: ids are held to the JMAP alphabet,
//! answers to sizes, lists to counts; the lines name no contact, no address book and no address.

use std::collections::{BTreeMap, BTreeSet};

use mailcraft_model::contact_sync::{
    ContactChangeKind, ContactOp, ContactOutcome, ContactResult, ContactsUploaded, FolderOwner, MAX_CONTACT_FOLDERS, RemoteContactFolder,
};
use mailcraft_model::remote::{ContactsDownload, MAX_CONTACTS, RemoteContact};
use mailcraft_model::sync::{JmapContactPlace, JmapContactsRequest};
use serde_json::{Map, Value, json};

use super::{Run, Stop, arguments, card, get, refusal};
use crate::{ApiRequest, BuildError, CallFailure, HttpExchange, Id, MethodErrorKind};

/// The capability of contacts (RFC 9610 §1.4).
pub const CAPABILITY_CONTACTS: &str = "urn:ietf:params:jmap:contacts";

/// Most accounts whose address books are read: the account's own and others'.
pub(super) const MAX_ACCOUNTS: usize = 32;
/// Most changes to contacts in one `ContactCard/set`.
pub(super) const MAX_SET: usize = 50;
/// Most changes a `ContactCard/changes` is asked for, and most such calls for one account in a run.
pub(super) const MAX_CHANGES: usize = 500;
pub(super) const MAX_ROUNDS: usize = 10;
/// Most cards asked for in one `ContactCard/get`, and one page of a whole read.
const PAGE: usize = 100;
/// The most the answer to a request may be, in bytes: a list of address books, a page of cards (a
/// card's notes can be long; photos are never asked for), a list of changes, a `set`.
const BOOKS_BYTES: usize = 1 << 20;
const CARDS_BYTES: usize = 8 << 20;
const CHANGES_BYTES: usize = 2 << 20;
const SET_BYTES: usize = 2 << 20;

/// An address book as a run uses it.
struct Book {
    id: String,
    name: String,
    default: bool,
    writable: bool,
}

/// One owner's account and its address books.
struct Place {
    account: String,
    owner: FolderOwner,
    books: Vec<Book>,
    /// `books` is every book the server lists (none cut off), but `unreadable`: one among neither
    /// is gone.
    whole: bool,
    unreadable: Vec<String>,
}

impl Place {
    /// The address book a folder names: by id, or (empty) the default one.
    fn book(&self, folder: &str) -> Option<&Book> {
        self.books.iter().find(|b| if folder.is_empty() { b.default } else { b.id == folder && !b.default })
    }

    fn folder(&self, book: &Book) -> RemoteContactFolder {
        RemoteContactFolder {
            id: if book.default { String::new() } else { book.id.clone() },
            name: book.name.clone(),
            parent: String::new(),
            owner: self.owner.clone(),
            writable: book.writable,
        }
    }
}

/// What the contacts part of a run came to.
#[derive(Default)]
pub(super) struct Contacts {
    pub(super) sent: Option<ContactsUploaded>,
    pub(super) downloads: Vec<ContactsDownload>,
    /// The owners whose address books were listed in full (the result's `contact_listed`).
    pub(super) listed: Vec<JmapContactPlace>,
    /// Why some of it couldn't be done, for a line.
    pub(super) trouble: Option<String>,
    /// Why it ended early: a refused sign-in, a busy server, a stop.
    pub(super) stop: Option<Stop>,
}

/// A request of contacts' method calls, answered in `most` bytes at the most.
fn contacts_request(most: usize) -> Result<ApiRequest, BuildError> {
    let mut request = ApiRequest::new();
    request.using(CAPABILITY_CONTACTS)?;
    request.set_response_limit(most);
    Ok(request)
}

/// What a `SetError` comes to (RFC 8620 §5.3, RFC 9610).
fn result_of(error: &Value) -> ContactResult {
    match error.get("type").and_then(Value::as_str).unwrap_or("") {
        "notFound" => ContactResult::Gone,
        "forbidden" => ContactResult::Denied,
        "serverFail" | "serverUnavailable" | "serverPartialFail" | "rateLimit" => ContactResult::Failed,
        _ => ContactResult::Refused,
    }
}

/// The card says which address books it is in (`addressBookIds`, an object), as RFC 9610 has it.
fn knows_books(card: &Value) -> bool {
    card.get("addressBookIds").is_some_and(Value::is_object)
}

/// What became of `op` when the server's answer to it never came (`why`): failed, to be tried again.
/// A new card may have been made all the same, and a new uid would make it twice: it is answered as
/// made under a name not heard, which the engine sends again only once a read of its folder shows
/// it isn't there (and asks for that read: [`JmapContactPlace::state`] left empty).
fn unheard(op: &ContactOp, why: &str) -> ContactOutcome {
    let result = if op.kind == ContactChangeKind::Create { ContactResult::Done } else { ContactResult::Failed };
    ContactOutcome { op: op.clone(), result, error: why.to_string(), ..Default::default() }
}

/// A uid for a new card (RFC 9553 §2.1.9): a UUID's shape, made of the time, the contact and a count,
/// so that no two are the same.
fn new_uid(contact: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static MADE: AtomicU64 = AtomicU64::new(0);
    let mix = |mut x: u64| {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    };
    let nth = MADE.fetch_add(1, Ordering::Relaxed);
    let a = mix(mailcraft_time::now().unsigned_abs() ^ mix(contact) ^ nth.rotate_left(40));
    let b = mix(a ^ nth);
    format!("urn:uuid:{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}", a >> 32, (a >> 16) & 0xFFFF, a & 0xFFF, (b >> 48) & 0xFFF, b & 0xFFFF_FFFF_FFFF)
}

impl<H: HttpExchange> Run<'_, H> {
    /// The contacts part of the run (see the module); `read_at` is when the run began, as this
    /// computer's clock has it.
    pub(super) async fn contacts(&self, request: &JmapContactsRequest, read_at: i64) -> Contacts {
        let mut out = Contacts::default();
        let session = self.connection.client().session();
        if !session.has_capability(CAPABILITY_CONTACTS) {
            // A server without contacts: the changes wait (there is nowhere to send them).
            return out;
        }
        let mut accounts: Vec<(String, FolderOwner, bool)> = Vec::new();
        if let Some(own) = session.primary_account(CAPABILITY_CONTACTS).or_else(|| Some(self.connection.client().account_id()))
            && let Some(a) = session.account(own).filter(|a| a.has_capability(CAPABILITY_CONTACTS))
        {
            accounts.push((own.as_str().to_string(), FolderOwner::Own, a.is_read_only()));
        }
        for (id, a) in session.accounts() {
            if accounts.len() >= MAX_ACCOUNTS {
                break;
            }
            let person = FolderOwner::Person(a.name().trim().to_string()).tidied();
            // (Two accounts under one name can't be told apart here: the first is kept, the other
            // passed over, so that neither's changes go to the other.)
            if !a.is_personal()
                && a.has_capability(CAPABILITY_CONTACTS)
                && !accounts.iter().any(|(had, ..)| had == id.as_str())
                && let Some(person) = person
                && !accounts.iter().any(|(_, owner, _)| *owner == person)
            {
                accounts.push((id.as_str().to_string(), person, a.is_read_only()));
            }
        }
        let mut places = Vec::new();
        let mut books = 0usize;
        for (account, owner, read_only) in accounts {
            match self.books(&account, read_only).await {
                Ok((mut list, unreadable, complete)) => {
                    let room = MAX_CONTACT_FOLDERS.saturating_sub(books);
                    // A list cut short here doesn't say which books are gone. One the user may not
                    // read is listed still: not gone.
                    let whole = complete && list.len() <= room;
                    if whole {
                        let ids =
                            list.iter().map(|b| if b.default { String::new() } else { b.id.clone() }).chain(unreadable.iter().cloned()).collect();
                        out.listed.push(JmapContactPlace { owner: owner.clone(), state: String::new(), books: ids });
                    }
                    list.truncate(room);
                    books = books.saturating_add(list.len());
                    places.push(Place { account, owner, books: list, whole, unreadable });
                }
                Err(stop) => {
                    if self.contacts_trouble(&mut out, stop) {
                        return out;
                    }
                }
            }
        }
        if !request.ops.is_empty() {
            let mut sent = ContactsUploaded { account: self.request.account, email: self.request.email.clone(), ..Default::default() };
            let stop = self.send_contacts(&request.ops, &places, &mut sent).await;
            out.sent = Some(sent);
            if let Some(stop) = stop
                && self.contacts_trouble(&mut out, stop)
            {
                return out;
            }
        }
        for place in &places {
            let here = request.places.iter().find(|p| p.owner == place.owner);
            match self.read_cards(place, here, read_at).await {
                Ok(mut downloads) => out.downloads.append(&mut downloads),
                Err(stop) => {
                    if self.contacts_trouble(&mut out, stop) {
                        return out;
                    }
                }
            }
        }
        out
    }

    /// Note `stop`: a line for a step's trouble (false: go on), or the end of the contacts (true).
    fn contacts_trouble(&self, out: &mut Contacts, stop: Stop) -> bool {
        match stop {
            Stop::Failed(error) | Stop::Down(error) => {
                out.trouble.get_or_insert_with(|| self.said(&error));
                false
            }
            stop => {
                if matches!(stop, Stop::Auth)
                    && let Some(sent) = out.sent.as_mut()
                {
                    sent.token_refused = true;
                }
                out.stop = Some(stop);
                true
            }
        }
    }

    /// A request of one call of contacts, and its answer.
    async fn contacts_call(&self, method: &str, args: Map<String, Value>, most: usize) -> Result<Value, Stop> {
        self.check()?;
        let mut request = contacts_request(most)?;
        let call = request.add_call(method, args)?;
        let mut answers = self.post(&request).await?;
        Self::take(&mut answers, &call, method)
    }

    /// `AddressBook/get` of `account`: the books the user may read, the default one first.
    /// Also the ids of those listed that the user may not read, and whether the list is all of them:
    /// every entry read, none cut off.
    async fn books(&self, account: &str, read_only: bool) -> Result<(Vec<Book>, Vec<String>, bool), Stop> {
        let args = arguments(&get(account, None, &["id", "name", "isDefault", "myRights"]))?;
        let answer = self.contacts_call("AddressBook/get", args, BOOKS_BYTES).await?;
        let list = answer.get("list").and_then(Value::as_array);
        let readable = |b: &Value| b.get("id").and_then(Value::as_str).is_some_and(|id| Id::new(id).is_ok());
        let complete = list.is_some_and(|l| l.len() <= MAX_CONTACT_FOLDERS && l.iter().all(readable));
        let mut unreadable = Vec::new();
        let mut books: Vec<Book> = list
            .into_iter()
            .flatten()
            .take(MAX_CONTACT_FOLDERS)
            .filter_map(|b| {
                let id = b.get("id").and_then(Value::as_str).filter(|id| Id::new(id).is_ok())?;
                let right = |name: &str| b.get("myRights").and_then(|r| r.get(name)).and_then(Value::as_bool);
                // (A server that says nothing of the rights lets them be read, and says when a change can't be made.)
                if right("mayRead") == Some(false) {
                    unreadable.push(id.to_string());
                    return None;
                }
                let name = b.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let default = b.get("isDefault").and_then(Value::as_bool).unwrap_or(false);
                Some(Book { id: id.to_string(), name, default, writable: !read_only && right("mayWrite").unwrap_or(true) })
            })
            .collect();
        books.sort_by(|a, b| b.default.cmp(&a.default).then_with(|| a.id.cmp(&b.id)));
        // One default at the most, and one at all when there are books: the first.
        for (at, book) in books.iter_mut().enumerate() {
            book.default = at == 0;
        }
        Ok((books, unreadable, complete))
    }

    /// Send `ops` to the address books of `places` (see the module), noting what became of each in
    /// `sent`. A stop that ends the contacts is handed back; the ops not answered then wait.
    async fn send_contacts(&self, ops: &[ContactOp], places: &[Place], sent: &mut ContactsUploaded) -> Option<Stop> {
        let limit = usize::try_from(self.connection.client().limits().max_objects_in_set()).unwrap_or(usize::MAX).clamp(1, MAX_SET);
        let outcome = |op: &ContactOp, result: ContactResult, error: &str| ContactOutcome {
            op: op.clone(),
            result,
            error: error.to_string(),
            ..Default::default()
        };
        // Ops by the account they go to, in order; those that can't go are answered at once.
        let mut by_place: BTreeMap<usize, Vec<(&ContactOp, String)>> = BTreeMap::new();
        for op in ops {
            let place = places.iter().position(|p| p.owner == op.owner);
            let book = place.and_then(|at| places.get(at)).and_then(|p| p.book(&op.folder));
            match (place, book, op.kind) {
                (_, _, ContactChangeKind::Change | ContactChangeKind::Delete) if Id::new(&op.remote).is_err() => {
                    sent.outcomes.push(outcome(op, ContactResult::Refused, "the contact's id is not one a server hands out"));
                }
                (Some(at), Some(book), _) if book.writable => by_place.entry(at).or_default().push((op, book.id.clone())),
                (Some(_), Some(_), _) => sent.outcomes.push(outcome(op, ContactResult::Denied, "the address book can only be read")),
                // Its owner's address books couldn't be listed (or the share is gone): tried again later,
                // never taken for gone.
                (None, ..) => sent.outcomes.push(outcome(op, ContactResult::Failed, "the server didn't list the address books")),
                // The address book is gone (the books were listed in full, and it isn't among them): a
                // deletion is done. A change or a new contact is left unanswered, never refused (that
                // would undo it here): the engine keeps it on this computer as it drops the folder.
                // A book merely cut off the list, or one that can't be read now, is not gone: the op
                // waits.
                (Some(at), None, ContactChangeKind::Delete) if places.get(at).is_some_and(|p| p.whole && !p.unreadable.contains(&op.folder)) => {
                    sent.outcomes.push(outcome(op, ContactResult::Gone, ""))
                }
                _ => {}
            }
        }
        for (at, list) in by_place {
            let Some(place) = places.get(at) else { continue };
            for chunk in list.chunks(limit) {
                let before = sent.outcomes.len();
                if let Err(stop) = self.send_chunk(place, chunk, sent).await {
                    // Only the ops not answered already (one found done before the call failed stays so).
                    let answered: BTreeSet<u64> = sent.outcomes.get(before..).unwrap_or_default().iter().map(|o| o.op.contact).collect();
                    let open = chunk.iter().filter(|(op, _)| !answered.contains(&op.contact));
                    match stop {
                        Stop::Failed(error) | Stop::Down(error) => {
                            let why = self.said(&error);
                            sent.outcomes.extend(open.map(|(op, _)| unheard(op, &why)));
                        }
                        Stop::Busy(seconds, why) => {
                            let failed = open.map(|(op, _)| ContactOutcome {
                                retry_after: Some(seconds),
                                status: 429,
                                ..outcome(op, ContactResult::Failed, &why)
                            });
                            sent.outcomes.extend(failed);
                            return Some(Stop::Busy(seconds, why));
                        }
                        stop => return Some(stop),
                    }
                }
            }
        }
        None
    }

    /// One `ContactCard/set` of `chunk` (each op with its address book) in `place`'s account.
    async fn send_chunk(&self, place: &Place, chunk: &[(&ContactOp, String)], sent: &mut ContactsUploaded) -> Result<(), Stop> {
        // The cards as the server has them: for the changes' patches, and the address books a
        // deleted one is in.
        let mut wanted: Vec<&str> = chunk.iter().filter(|(op, _)| op.kind != ContactChangeKind::Create).map(|(op, _)| op.remote.as_str()).collect();
        wanted.sort_unstable();
        wanted.dedup();
        // (A card the server says nothing of is neither had nor gone: the call fails, nothing is sent.)
        let (cards, _, unanswered) = self.raw_cards(&place.account, &wanted).await?;
        if unanswered {
            return Err(Stop::Failed(crate::connection::unusable("cards asked for and not answered").into()));
        }
        let (mut create, mut update, mut destroy) = (Map::new(), Map::new(), Vec::new());
        // The address books each deleted card is to be taken out of.
        let mut out_of: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        // What each op was asked as: its creation id, or the card's.
        let mut asked: Vec<(&ContactOp, String)> = Vec::new();
        for (n, (op, book)) in chunk.iter().enumerate() {
            match op.kind {
                ContactChangeKind::Create => {
                    let creation = format!("c{n}");
                    create.insert(creation.clone(), Value::Object(card::made(&op.values, book, &new_uid(op.contact))));
                    asked.push((op, creation));
                }
                ContactChangeKind::Change => match cards.get(&op.remote) {
                    None => sent.outcomes.push(ContactOutcome { op: (*op).clone(), result: ContactResult::Gone, ..Default::default() }),
                    Some(server) => {
                        let patch = card::patch(server, &op.values, &op.fields);
                        if patch.is_empty() {
                            // The server has it so already.
                            sent.outcomes.push(ContactOutcome {
                                op: (*op).clone(),
                                result: ContactResult::Done,
                                remote: op.remote.clone(),
                                status: 200,
                                ..Default::default()
                            });
                        } else {
                            update.insert(op.remote.clone(), Value::Object(patch));
                            asked.push((op, op.remote.clone()));
                        }
                    }
                },
                ContactChangeKind::Delete => match cards.get(&op.remote) {
                    None => sent.outcomes.push(ContactOutcome { op: (*op).clone(), result: ContactResult::Gone, ..Default::default() }),
                    // Which books it is in, unsaid: never destroyed on a guess.
                    Some(c) if !knows_books(c) => {
                        sent.outcomes.push(unheard(op, "the server didn't say which address books the contact is in"));
                    }
                    Some(_) => {
                        out_of.entry(op.remote.as_str()).or_default().insert(book.as_str());
                        asked.push((op, op.remote.clone()));
                    }
                },
            }
        }
        // A deletion takes the card out of its folder's address book only: it is destroyed when that
        // leaves it in none, and otherwise patched out (`addressBookIds/<id>` to null). A move between
        // books is the same rule.
        for (remote, books) in &out_of {
            let elsewhere = cards
                .get(*remote)
                .and_then(|c| c.get("addressBookIds"))
                .and_then(Value::as_object)
                .is_some_and(|m| m.iter().any(|(id, on)| *on == Value::Bool(true) && !books.contains(id.as_str())));
            if elsewhere {
                let patch = update.entry(remote.to_string()).or_insert_with(|| Value::Object(Map::new()));
                if let Some(patch) = patch.as_object_mut() {
                    patch.extend(books.iter().map(|b| (format!("addressBookIds/{b}"), Value::Null)));
                }
            } else {
                // (A change to it in the same call is pointless: it is answered as gone.)
                update.remove(*remote);
                destroy.push(Value::String(remote.to_string()));
            }
        }
        let patched: BTreeSet<String> = update.keys().cloned().collect();
        if asked.is_empty() {
            return Ok(());
        }
        let args = Map::from_iter([
            ("accountId".to_string(), json!(place.account)),
            ("create".to_string(), if create.is_empty() { Value::Null } else { Value::Object(create) }),
            ("update".to_string(), if update.is_empty() { Value::Null } else { Value::Object(update) }),
            ("destroy".to_string(), if destroy.is_empty() { Value::Null } else { Value::Array(destroy) }),
        ]);
        let answer = self.contacts_call("ContactCard/set", args, SET_BYTES).await?;
        let map = |name: &str| answer.get(name).and_then(Value::as_object);
        let destroyed: BTreeSet<&str> = answer.get("destroyed").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        for (op, key) in asked {
            let done = |remote: String| ContactOutcome { op: op.clone(), result: ContactResult::Done, remote, status: 200, ..Default::default() };
            let refused = |error: &Value| ContactOutcome {
                op: op.clone(),
                result: result_of(error),
                error: refusal(&self.secrets, error),
                ..Default::default()
            };
            let said = match op.kind {
                ContactChangeKind::Create => match (map("created").and_then(|m| m.get(&key)), map("notCreated").and_then(|m| m.get(&key))) {
                    // (A new card the server names by no id that can be read: made, its name not heard.)
                    (Some(made), _) => Some(done(card::id_of(made).unwrap_or_default())),
                    (None, Some(error)) => Some(refused(error)),
                    (None, None) => None,
                },
                ContactChangeKind::Change | ContactChangeKind::Delete if patched.contains(&key) => {
                    match (map("updated").is_some_and(|m| m.contains_key(&key)), map("notUpdated").and_then(|m| m.get(&key))) {
                        (true, _) => Some(done(key.clone())),
                        (false, Some(error)) => Some(refused(error)),
                        (false, None) => None,
                    }
                }
                ContactChangeKind::Change => {
                    destroyed.contains(key.as_str()).then(|| ContactOutcome { op: op.clone(), result: ContactResult::Gone, ..Default::default() })
                }
                ContactChangeKind::Delete => match (destroyed.contains(key.as_str()), map("notDestroyed").and_then(|m| m.get(&key))) {
                    (true, _) => Some(done(key.clone())),
                    (false, Some(error)) => Some(refused(error)),
                    (false, None) => None,
                },
            };
            // An answer that says nothing of it: tried again later, as for a failure.
            sent.outcomes.push(said.unwrap_or_else(|| unheard(op, "the server didn't say")));
        }
        Ok(())
    }

    /// The cards of `place`'s address books: from the state `here` has when it may, else all of them.
    async fn read_cards(&self, place: &Place, here: Option<&JmapContactPlace>, read_at: i64) -> Result<Vec<ContactsDownload>, Stop> {
        let state = here.map(|h| h.state.as_str()).unwrap_or("");
        let known = |b: &Book| here.is_some_and(|h| h.books.iter().any(|id| if b.default { id.is_empty() } else { *id == b.id }));
        if !state.is_empty() && place.books.iter().all(known) {
            match self.changes_since(place, state, read_at).await {
                Err(Stop::Failed(crate::Failure::Call(CallFailure::Method(error)))) if *error.kind() == MethodErrorKind::CannotCalculateChanges => {}
                other => return other,
            }
        }
        self.read_whole(place, read_at).await
    }

    /// A download for each address book of `place`.
    fn downloads(
        &self,
        place: &Place,
        cards: &[(RemoteContact, Vec<String>)],
        gone: &[String],
        complete: bool,
        delta: &str,
        read_at: i64,
    ) -> Vec<ContactsDownload> {
        place
            .books
            .iter()
            .map(|book| {
                let mut contacts = Vec::new();
                let mut elsewhere = Vec::new();
                for (c, books) in cards {
                    if books.contains(&book.id) {
                        if contacts.len() < MAX_CONTACTS {
                            contacts.push(c.clone());
                        }
                    } else {
                        elsewhere.push(c.id.clone());
                    }
                }
                ContactsDownload {
                    account: self.request.account,
                    email: self.request.email.clone(),
                    complete,
                    contacts,
                    folder: Some(place.folder(book)),
                    // (Read whole, what isn't listed is gone anyway.)
                    gone: if complete { Vec::new() } else { gone.iter().cloned().chain(elsewhere).collect() },
                    delta: delta.to_string(),
                    read_at,
                }
            })
            .collect()
    }

    /// The cards that changed since `state` (`ContactCard/changes`), and those destroyed.
    async fn changes_since(&self, place: &Place, state: &str, read_at: i64) -> Result<Vec<ContactsDownload>, Stop> {
        let mut since = state.to_string();
        // Each id once, as the last round says: changed, or destroyed.
        let mut changed: BTreeSet<String> = BTreeSet::new();
        let mut destroyed: BTreeSet<String> = BTreeSet::new();
        for _ in 0..MAX_ROUNDS {
            let args = Map::from_iter([
                ("accountId".to_string(), json!(place.account)),
                ("sinceState".to_string(), json!(since)),
                ("maxChanges".to_string(), json!(MAX_CHANGES)),
            ]);
            let answer = self.contacts_call("ContactCard/changes", args, CHANGES_BYTES).await?;
            let ids = |name: &str| -> Vec<String> {
                answer
                    .get(name)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|id| Id::new(id).is_ok())
                    .take(MAX_CHANGES)
                    .map(str::to_string)
                    .collect()
            };
            for id in ids("created").into_iter().chain(ids("updated")) {
                destroyed.remove(&id);
                changed.insert(id);
            }
            for id in ids("destroyed") {
                changed.remove(&id);
                destroyed.insert(id);
            }
            let new = answer.get("newState").and_then(Value::as_str).map(mailcraft_model::contact_sync::delta).unwrap_or_default();
            if new.is_empty() {
                return Err(Stop::Failed(crate::connection::unusable("a list of changes without a new state").into()));
            }
            let more = answer.get("hasMoreChanges").and_then(Value::as_bool).unwrap_or(false);
            let moved = new != since;
            since = new;
            if !more || !moved || changed.len().saturating_add(destroyed.len()) >= MAX_CONTACTS {
                break;
            }
        }
        let ids: Vec<&str> = changed.iter().map(String::as_str).collect();
        let (cards, missing) = self.cards(&place.account, &ids).await?;
        destroyed.extend(missing);
        let gone: Vec<String> = destroyed.into_iter().collect();
        Ok(self.downloads(place, &cards, &gone, false, &since, read_at))
    }

    /// The cards `ids` of `account` as the server has them, a page at a time (`maxObjectsInGet`): by
    /// id, each once and only those asked for; the ids it said it hasn't got; and whether it left any
    /// out of both (then nothing can be taken as gone or had).
    async fn raw_cards(&self, account: &str, ids: &[&str]) -> Result<(BTreeMap<String, Value>, BTreeSet<String>, bool), Stop> {
        let page = usize::try_from(self.connection.client().limits().max_objects_in_get()).unwrap_or(usize::MAX).clamp(1, PAGE);
        let (mut cards, mut missing) = (BTreeMap::new(), BTreeSet::new());
        for chunk in ids.chunks(page) {
            let asked: BTreeSet<&str> = chunk.iter().copied().collect();
            let args = arguments(&get(account, Some(chunk), &card::CARD_PROPERTIES))?;
            let answer = self.contacts_call("ContactCard/get", args, CARDS_BYTES).await?;
            for c in answer.get("list").and_then(Value::as_array).into_iter().flatten().take(page) {
                if let Some(id) = card::id_of(c).filter(|id| asked.contains(id.as_str())) {
                    cards.entry(id).or_insert_with(|| c.clone());
                }
            }
            let not_found = answer.get("notFound").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str);
            missing.extend(not_found.filter(|id| asked.contains(id) && !cards.contains_key(*id)).map(str::to_string));
        }
        let unanswered = ids.iter().any(|id| !cards.contains_key(*id) && !missing.contains(*id));
        Ok((cards, missing, unanswered))
    }

    /// The cards `ids` of `account`: those it has (a group's left out), and the ids it said it hasn't
    /// got. One it says nothing of fails the read (where changes were read up to then stays).
    async fn cards(&self, account: &str, ids: &[&str]) -> Result<(Vec<(RemoteContact, Vec<String>)>, Vec<String>), Stop> {
        let (raw, not_found, unanswered) = self.raw_cards(account, ids).await?;
        if unanswered {
            return Err(Stop::Failed(crate::connection::unusable("cards asked for and not answered").into()));
        }
        // A card that doesn't say which books it is in would read as gone from all of them.
        if raw.values().any(|c| !knows_books(c)) {
            return Err(Stop::Failed(crate::connection::unusable("a card that doesn't say its address books").into()));
        }
        let mut missing: Vec<String> = not_found.into_iter().collect();
        let mut cards = Vec::new();
        for c in raw.values() {
            match card::read(c) {
                Some(contact) => cards.push((contact, card::books_of(c))),
                // A group, now: as good as gone for the contacts.
                None => missing.extend(card::id_of(c)),
            }
        }
        Ok((cards, missing))
    }

    /// Every card of `place`'s account (`ContactCard/query`, then the cards a page at a time): complete
    /// when the query stayed the same throughout and every card was read.
    async fn read_whole(&self, place: &Place, read_at: i64) -> Result<Vec<ContactsDownload>, Stop> {
        let page = usize::try_from(self.connection.client().limits().max_objects_in_get()).unwrap_or(usize::MAX).clamp(1, PAGE);
        let mut cards = Vec::new();
        // The state before anything is read: changes from it bring whatever this read misses.
        let args = arguments(&get(&place.account, Some(&[]), &["id"]))?;
        let first = self.contacts_call("ContactCard/get", args, CHANGES_BYTES).await?;
        let state = first.get("state").and_then(Value::as_str).map(mailcraft_model::contact_sync::delta).unwrap_or_default();
        let (mut position, mut query_state, mut complete) = (0usize, None::<String>, false);
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while position < MAX_CONTACTS {
            let args = Map::from_iter([
                ("accountId".to_string(), json!(place.account)),
                ("position".to_string(), json!(position)),
                ("limit".to_string(), json!(page)),
                ("calculateTotal".to_string(), json!(true)),
            ]);
            let answer = self.contacts_call("ContactCard/query", args, CHANGES_BYTES).await?;
            let now = answer.get("queryState").and_then(Value::as_str).unwrap_or("").to_string();
            if query_state.get_or_insert_with(|| now.clone()) != &now || now.is_empty() {
                // It changed while it was read: what was read stands, nothing is taken as gone.
                complete = false;
                break;
            }
            let ids: Vec<&str> = answer
                .get("ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|id| Id::new(id).is_ok())
                .take(page)
                .collect();
            let total = answer.get("total").and_then(Value::as_u64).and_then(|t| usize::try_from(t).ok());
            if ids.is_empty() {
                complete = total.is_some_and(|t| t <= position) && position < MAX_CONTACTS;
                break;
            }
            // Each id once in the whole read, and each answered with its card, saying its books:
            // anything else (one listed twice, one not there, one not answered, one whose books are
            // unsaid, which is left out) leaves the read incomplete.
            let fresh = ids.iter().all(|id| seen.insert((*id).to_string()));
            let (got, not_found, unanswered) = self.raw_cards(&place.account, &ids).await?;
            let unsaid = got.values().any(|c| !knows_books(c));
            cards.extend(got.values().filter(|c| knows_books(c)).filter_map(|c| Some((card::read(c)?, card::books_of(c)))));
            position = position.saturating_add(ids.len());
            if !fresh || unanswered || unsaid || !not_found.is_empty() {
                // A card listed and then not there: changed meanwhile.
                break;
            }
            if total.is_some_and(|t| position >= t) {
                // (Past the cap, the last page was cut: not all of it.)
                complete = position <= MAX_CONTACTS;
                break;
            }
        }
        // Read whole but not all of it: no state, so that the next run reads whole again.
        let delta = if complete { state } else { String::new() };
        Ok(self.downloads(place, &cards, &[], complete, &delta, read_at))
    }
}
