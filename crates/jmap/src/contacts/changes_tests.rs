// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The changes of contacts going up: what is asked of the server for each kind and in what order, what a
//! patch names, what the answers are taken for, in how many calls, and what a hostile answer makes of it.
//! Over a scripted server that answers call by call; every name and address is invented.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;

use mailcraft_model::Labeled;
use mailcraft_model::remote::{ContactField, ContactOp, ContactOpKind, ContactOutcome, ContactResult, MAX_CONTACT_OPS, RemoteContact};
use mailcraft_time::Date;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

use super::changes::{book_of, outcome_of, patch};
use super::write::{new_card, property, written};
use super::write_tests::ada;
use super::*;
use crate::connection::tests::any_json;
use crate::test_support::made_secret;
use crate::{CAPABILITY_CORE, CAPABILITY_MAIL, Credentials, HttpRequest, HttpResponse, Id, Method, Redactor, TransportError, block_on};

const ME: &str = "ada@example.org";
const SESSION: &str = "https://jmap.example.org/jmap/session";
const API: &str = "https://jmap.example.org/jmap/api";
const TOKEN: &str = "tok-3n-s3cret-77";
const UP: &str = "Couldn't take the changes of contacts up to the server: ";

type Reply = std::result::Result<HttpResponse, TransportError>;

/// What a scripted server says to one call.
enum Said {
    Is(Value),
    /// An error for the call, of this type and with this description.
    Error(&'static str, String),
    /// The whole request answered with this status, and a `Retry-After` of so many seconds.
    Status(u16, Option<u32>),
    /// No connection.
    Down,
    /// An answer that is no JMAP.
    Garbled,
    /// An answer without the call in it.
    Missing,
}

/// A server that answers each call as a function says (given its name, its arguments, and how many calls
/// came before it), or as [`usual`] when the function has nothing to say; it keeps what it was asked.
#[derive(Clone)]
struct Server {
    core: Value,
    answer: Rc<dyn Fn(&str, &Value, usize) -> Option<Said>>,
    asked: Rc<RefCell<Vec<(String, Value)>>>,
    bodies: Rc<RefCell<Vec<Value>>>,
    elsewhere: Rc<RefCell<Vec<String>>>,
}

impl Server {
    fn new(answer: impl Fn(&str, &Value, usize) -> Option<Said> + 'static) -> Server {
        Server::with(json!({}), answer)
    }

    /// A server whose session states the core limits `core`.
    fn with(core: Value, answer: impl Fn(&str, &Value, usize) -> Option<Said> + 'static) -> Server {
        Server { core, answer: Rc::new(answer), asked: Rc::default(), bodies: Rc::default(), elsewhere: Rc::default() }
    }

    /// A server that takes every change.
    fn agreeing() -> Server {
        Server::new(|_, _, _| None)
    }

    /// The calls it was asked, in order: their names.
    fn methods(&self) -> Vec<String> {
        self.asked.borrow().iter().map(|(name, _)| name.clone()).collect()
    }

    /// The arguments of each call to `method`, in order.
    fn args_of(&self, method: &str) -> Vec<Value> {
        self.asked.borrow().iter().filter(|(name, _)| name == method).map(|(_, args)| args.clone()).collect()
    }

    /// The `ContactCard/set` calls it was asked.
    fn sets(&self) -> Vec<Value> {
        self.args_of("ContactCard/set")
    }
}

fn json_reply(status: u16, body: &Value) -> Reply {
    Ok(HttpResponse::new(status, serde_json::to_vec(body).unwrap()).with_field("Content-Type", "application/json"))
}

impl HttpExchange for Server {
    async fn exchange(&self, request: &HttpRequest) -> Reply {
        if request.method() == Method::Get {
            return json_reply(
                200,
                &json!({
                    "capabilities": {CAPABILITY_CORE: self.core, CAPABILITY_MAIL: {}, CAPABILITY_CONTACTS: {}},
                    "accounts": {"A1": {"name": ME, "isPersonal": true, "accountCapabilities": {CAPABILITY_MAIL: {}, CAPABILITY_CONTACTS: {}}}},
                    "primaryAccounts": {CAPABILITY_MAIL: "A1", CAPABILITY_CONTACTS: "A1"},
                    "apiUrl": "/jmap/api",
                    "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
                    "uploadUrl": "/jmap/upload/{accountId}/",
                    "state": "1",
                }),
            );
        }
        if request.url() != API {
            self.elsewhere.borrow_mut().push(request.url().to_string());
        }
        let body: Value = serde_json::from_slice(request.body()).unwrap_or(Value::Null);
        let call = body["methodCalls"][0].clone();
        let (name, args) = (call[0].as_str().unwrap_or_default().to_string(), call[1].clone());
        let before = self.asked.borrow().len();
        self.asked.borrow_mut().push((name.clone(), args.clone()));
        self.bodies.borrow_mut().push(body);
        let said = (self.answer)(&name, &args, before).unwrap_or_else(|| Said::Is(usual(&name, &args)));
        let responses = match said {
            Said::Is(value) => json!([[name, value, call[2]]]),
            Said::Error(kind, description) => json!([["error", {"type": kind, "description": description}, call[2]]]),
            Said::Missing => json!([]),
            Said::Status(status, retry_after) => {
                let answer = HttpResponse::new(status, Vec::new());
                return Ok(match retry_after {
                    Some(seconds) => answer.with_field("Retry-After", &seconds.to_string()),
                    None => answer,
                });
            }
            Said::Down => return Err(TransportError::TimedOut),
            Said::Garbled => return json_reply(200, &json!({"methodResponses": "no"})),
        };
        json_reply(200, &json!({"methodResponses": responses, "sessionState": "1"}))
    }
}

/// What a server that keeps to the standard and takes every change answers: one address book, the default;
/// every card made (`N` and its creation id), changed and destroyed as asked; every card asked for there,
/// with nothing in it; and one card, `K9`, in the list.
fn usual(name: &str, args: &Value) -> Value {
    match name {
        "AddressBook/get" => json!({"accountId": "A1", "state": "1", "notFound": [], "list": [
            {"id": "B1", "name": "Contacts", "isDefault": true, "sortOrder": 0, "myRights": {"mayRead": true, "mayWrite": true}}]}),
        "ContactCard/set" => {
            let created: Map<String, Value> =
                args["create"].as_object().into_iter().flatten().map(|(key, _)| (key.clone(), json!({"id": format!("N{key}")}))).collect();
            let updated: Map<String, Value> = args["update"].as_object().into_iter().flatten().map(|(id, _)| (id.clone(), Value::Null)).collect();
            json!({"accountId": "A1", "oldState": "1", "newState": "2", "created": created, "updated": updated, "destroyed": args["destroy"]})
        }
        "ContactCard/get" => {
            let list: Vec<Value> = args["ids"].as_array().into_iter().flatten().map(|id| json!({"id": id})).collect();
            json!({"accountId": "A1", "state": "2", "list": list, "notFound": []})
        }
        "ContactCard/query" => json!({"accountId": "A1", "ids": ["K9"], "position": 0, "total": 1}),
        _ => Value::Null,
    }
}

/// The cards a `ContactCard/get` asks for, as `cards` has them (by id); the others are not found.
fn having(args: &Value, cards: &[Value]) -> Said {
    let (mut list, mut gone) = (Vec::new(), Vec::new());
    for id in args["ids"].as_array().into_iter().flatten() {
        match cards.iter().find(|card| card["id"] == *id) {
            Some(card) => list.push(card.clone()),
            None => gone.push(id.clone()),
        }
    }
    Said::Is(json!({"accountId": "A1", "state": "1", "list": list, "notFound": gone}))
}

/// Whether a call reads cards before they are changed (and is not the look for what an answer left out,
/// nor the reading of the contacts).
fn reads_first(name: &str, args: &Value) -> bool {
    name == "ContactCard/get" && args["properties"].as_array().is_some_and(|properties| properties.len() > 1)
}

fn is_look(name: &str, args: &Value) -> bool {
    name == "ContactCard/get" && args["properties"] == json!(["id"])
}

fn asking(changes: Vec<ContactOp>) -> JmapContactsRequest {
    JmapContactsRequest { account: 7, email: ME.into(), session: SESSION.into(), bearer: true, changes }
}

fn run_until(server: &Server, changes: Vec<ContactOp>, cancel: &AtomicBool) -> JmapContactsResult {
    let connection = block_on(Connection::connect(server.clone(), SESSION, Credentials::bearer(&made_secret(TOKEN)).unwrap())).unwrap();
    let got = block_on(sync_contacts(&connection, &asking(changes), cancel));
    assert!(server.elsewhere.borrow().is_empty(), "a request went elsewhere: {:?}", server.elsewhere.borrow());
    assert_eq!((got.account, got.email.as_str(), got.session.as_str()), (7, ME, SESSION));
    got
}

fn run(server: &Server, changes: Vec<ContactOp>) -> JmapContactsResult {
    run_until(server, changes, &AtomicBool::new(false))
}

/// Whether the run went on to read the contacts, and read them all.
fn read_all(got: &JmapContactsResult) -> bool {
    got.contacts.as_ref().is_some_and(|read| read.complete && read.contacts.iter().map(|contact| contact.id.as_str()).eq(["K9"]))
}

fn create(contact: u64, card: RemoteContact) -> ContactOp {
    ContactOp { kind: ContactOpKind::Create, contact, card: RemoteContact { id: String::new(), ..card }, ..Default::default() }
}

fn update(id: &str, card: RemoteContact, changed: &[ContactField]) -> ContactOp {
    ContactOp { kind: ContactOpKind::Update, contact: 40, id: id.into(), card: RemoteContact { id: id.into(), ..card }, changed: changed.to_vec() }
}

fn delete(id: &str) -> ContactOp {
    ContactOp { kind: ContactOpKind::Delete, contact: 0, id: id.into(), ..Default::default() }
}

fn named(first: &str) -> RemoteContact {
    RemoteContact { first: first.into(), ..Default::default() }
}

/// What became of `op`, when it is `result` and nothing else is said.
fn told(op: &ContactOp, result: ContactResult) -> ContactOutcome {
    let changed = if result == ContactResult::Done { op.changed.clone() } else { Vec::new() };
    ContactOutcome { kind: op.kind, contact: op.contact, id: op.id.clone(), result, changed, error: String::new() }
}

fn refusal_of(op: &ContactOp, why: &str) -> ContactOutcome {
    ContactOutcome { error: why.into(), ..told(op, ContactResult::Refused) }
}

/// A card as a server has it, with what a contact here does not hold: a wedding day, a sound, a note of
/// another's.
fn kept() -> Value {
    json!({
        "id": "K1",
        "anniversaries": {
            "w1": {"kind": "wedding", "date": {"year": 1835, "month": 7, "day": 8}},
            "x": {"kind": "birth", "date": {"year": 1815, "month": 12, "day": 9}, "place": {"full": "London"}},
        },
        "media": {"s": {"kind": "sound", "uri": "https://example.org/ada.ogg"}, "p9": {"kind": "photo", "blobId": "G1", "mediaType": "image/png"}},
        "notes": {"q": {"note": "An old note.", "created": "2020-01-01T00:00:00Z"}},
    })
}

fn card_of(value: &Value) -> &Map<String, Value> {
    value.as_object().unwrap()
}

fn keys(patch: &Map<String, Value>) -> Vec<&str> {
    patch.keys().map(String::as_str).collect()
}

#[test]
fn a_contact_is_made_in_the_default_address_book_before_the_contacts_are_read() {
    let server = Server::agreeing();
    let contact = RemoteContact {
        display_name: "Zoe Park".into(),
        first: "Zoe".into(),
        last: "Park".into(),
        emails: vec![Labeled::new("work", "zoe@example.org")],
        birthday: Date::new(1990, 2, 3),
        ..Default::default()
    };
    let op = create(21, contact.clone());
    let got = run(&server, vec![op.clone()]);
    // Done, with the id the server gave it.
    assert_eq!(got.changed, [ContactOutcome { id: "Nk0".into(), ..told(&op, ContactResult::Done) }]);
    // The address book, the card, and then the reading: what is read shows the change.
    assert_eq!(server.methods(), ["AddressBook/get", "ContactCard/set", "ContactCard/query", "ContactCard/get"]);
    assert_eq!(
        server.args_of("AddressBook/get"),
        [json!({"accountId": "A1", "ids": null, "properties": ["id", "isDefault", "sortOrder", "myRights"]})]
    );
    assert_eq!(
        server.sets(),
        [json!({"accountId": "A1", "create": {"k0": {
            "@type": "Card", "version": "1.0", "addressBookIds": {"B1": true},
            "name": {"full": "Zoe Park", "components": [{"kind": "given", "value": "Zoe"}, {"kind": "surname", "value": "Park"}]},
            "emails": {"e1": {"address": "zoe@example.org", "contexts": {"work": true}, "pref": 1}},
            "anniversaries": {"b1": {"kind": "birth", "date": {"year": 1990, "month": 2, "day": 3}}},
        }}})]
    );
    assert!(read_all(&got) && got.errors.is_empty(), "{got:?}");

    // A contact with everything goes whole, as `write` writes it; the creation id is the change's place among
    // the request's and says nothing of the contact.
    let server = Server::agreeing();
    let ops = vec![delete("K5"), create(22, ada()), create(23, named("Bo"))];
    let got = run(&server, ops.clone());
    let sets = server.sets();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0]["create"]["k1"], Value::Object(new_card("B1", &ada()).unwrap()));
    assert_eq!(keys(card_of(&sets[0]["create"])), ["k1", "k2"]);
    assert_eq!(sets[0]["destroy"], json!(["K5"]));
    let ids: Vec<(&str, ContactResult)> = got.changed.iter().map(|done| (done.id.as_str(), done.result)).collect();
    assert_eq!(ids, [("K5", ContactResult::Done), ("Nk1", ContactResult::Done), ("Nk2", ContactResult::Done)]);
    assert_eq!(got.changed.iter().map(|done| done.contact).collect::<Vec<_>>(), [0, 22, 23]);
}

#[test]
fn the_address_book_is_the_default_else_the_first_that_can_be_written_to() {
    let of = |list: Value| book_of(card_of(&json!({"list": list})));
    let book = |id: &str, default: bool, order: u64| json!({"id": id, "isDefault": default, "sortOrder": order, "myRights": {"mayWrite": true}});
    assert_eq!(of(json!([book("B3", false, 0), book("B2", true, 9), book("B1", false, 0)])).as_deref(), Some("B2"));
    // No default: by their order, then by their ids.
    assert_eq!(of(json!([book("B9", false, 1), book("B3", false, 5), book("B2", false, 1)])).as_deref(), Some("B2"));
    assert_eq!(of(json!([{"id": "B7"}, {"id": "B4", "sortOrder": 3}])).as_deref(), Some("B7"));
    // One that may not be written to is none, default or not; rights that are not said do not forbid.
    let read_only = json!({"id": "B1", "isDefault": true, "myRights": {"mayRead": true, "mayWrite": false}});
    assert_eq!(of(json!([read_only, book("B5", false, 4), {"id": "B6", "sortOrder": 2, "myRights": {}}])).as_deref(), Some("B6"));
    assert_eq!(of(json!([read_only])), None);
    // What is no address book, and an id that is none.
    for list in
        [json!(null), json!("B1"), json!({"B1": {}}), json!([]), json!([null, 7, "B1", ["B1"], {"id": 7}, {"id": "a b"}, {"id": ""}, {"name": "B1"}])]
    {
        assert_eq!(of(list.clone()), None, "{list}");
    }
    // Of a great many, the first few hundred are looked at.
    let many: Vec<Value> = (0..5_000).map(|n| book(&format!("B{n}"), n == 4_999, 5)).collect();
    assert_eq!(of(Value::Array(many)).as_deref(), Some("B0"));
}

#[test]
fn without_an_address_book_to_write_to_every_contact_to_be_made_is_refused() {
    let server = Server::with(json!({"maxObjectsInSet": 1}), |name, _, _| {
        (name == "AddressBook/get")
            .then(|| Said::Is(json!({"list": [{"id": "B1", "isDefault": true, "myRights": {"mayWrite": false}}], "notFound": []})))
    });
    let ops = vec![create(21, ada()), delete("K2"), create(22, named("Bo"))];
    let got = run(&server, ops.clone());
    let why = "the account has no address book that can be written to";
    assert_eq!(got.changed, [refusal_of(&ops[0], why), told(&ops[1], ContactResult::Done), refusal_of(&ops[2], why)]);
    // Asked once in the run, and no card was sent anywhere.
    assert_eq!(server.methods(), ["AddressBook/get", "ContactCard/set", "ContactCard/query", "ContactCard/get"]);
    assert_eq!(server.sets(), [json!({"accountId": "A1", "destroy": ["K2"]})]);
    assert!(read_all(&got) && got.errors.is_empty());
}

#[test]
fn each_part_changed_alone_is_a_patch_of_that_part_and_nothing_else() {
    let kept = kept();
    let day = json!({"kind": "birth", "date": {"year": 1815, "month": 12, "day": 10}});
    for field in ContactField::ALL {
        let made = patch(&update("K1", ada(), &[field]), card_of(&kept)).unwrap();
        let whole = written(field, &ada()).unwrap().unwrap();
        let expected = match field {
            ContactField::Birthday => json!({"anniversaries/x": day}),
            _ => json!({property(field): whole}),
        };
        assert_eq!(Value::Object(made), expected, "{field:?}");
    }
    // The paths, said out: one for each part, and none that is not a property of the part.
    let all = patch(&update("K1", ada(), &ContactField::ALL), card_of(&kept)).unwrap();
    assert_eq!(keys(&all), ["addresses", "anniversaries/x", "emails", "links", "name", "nicknames", "notes", "organizations", "phones", "titles"]);
    // A part named twice is written once; no part named is no patch.
    let twice = patch(&update("K1", ada(), &[ContactField::Title, ContactField::Name, ContactField::Title]), card_of(&kept)).unwrap();
    assert_eq!(keys(&twice), ["name", "titles"]);
    assert!(patch(&update("K1", ada(), &[]), card_of(&kept)).unwrap().is_empty());
}

#[test]
fn an_update_goes_as_its_patch_after_the_card_is_read_for_what_it_must_leave_alone() {
    // Parts that are replaced whole: nothing is read first.
    let server = Server::agreeing();
    let op = update("K1", ada(), &[ContactField::Name, ContactField::Emails, ContactField::Websites]);
    let got = run(&server, vec![op.clone()]);
    assert_eq!(got.changed, [told(&op, ContactResult::Done)]);
    assert_eq!(got.changed[0].changed, [ContactField::Name, ContactField::Emails, ContactField::Websites]);
    assert_eq!(server.methods(), ["ContactCard/set", "ContactCard/query", "ContactCard/get"]);
    let whole = new_card("B1", &ada()).unwrap();
    assert_eq!(
        server.sets(),
        [json!({"accountId": "A1", "update": {"K1": {"name": whole["name"], "emails": whole["emails"], "links": whole["links"]}}})]
    );

    // Every part at once: the card is read for its anniversaries and notes, and no more of it.
    let server = Server::new(|name, args, _| reads_first(name, args).then(|| having(args, &[kept()])));
    let op = update("K1", ada(), &ContactField::ALL);
    let got = run(&server, vec![op.clone(), update("K2", named("Bo"), &[ContactField::Birthday])]);
    // (The card that is gone is known before the server is asked to change the other.)
    assert_eq!(got.changed, [told(&update("K2", named("Bo"), &[ContactField::Birthday]), ContactResult::Gone), told(&op, ContactResult::Done)]);
    assert_eq!(server.methods(), ["ContactCard/get", "ContactCard/set", "ContactCard/query", "ContactCard/get"]);
    assert_eq!(server.args_of("ContactCard/get")[0], json!({"accountId": "A1", "ids": ["K1", "K2"], "properties": ["id", "anniversaries", "notes"]}));
    let sets = server.sets();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0]["update"]["K1"], Value::Object(patch(&op, card_of(&kept())).unwrap()));
    assert_eq!(keys(card_of(&sets[0]["update"])), ["K1"]);
    assert!(read_all(&got) && got.errors.is_empty());

    // Only what the call changes is asked for.
    let server = Server::agreeing();
    run(&server, vec![update("K1", ada(), &[ContactField::Birthday, ContactField::Name]), update("K2", ada(), &[ContactField::Title])]);
    assert_eq!(server.args_of("ContactCard/get")[0], json!({"accountId": "A1", "ids": ["K1"], "properties": ["id", "anniversaries"]}));
}

#[test]
fn a_part_that_is_empty_now_is_null_and_what_is_not_there_is_not_taken_away() {
    let emptied = update("K1", RemoteContact::default(), &ContactField::ALL);
    assert_eq!(
        Value::Object(patch(&emptied, card_of(&kept())).unwrap()),
        json!({
            "name": null, "nicknames": null, "organizations": null, "titles": null, "emails": null, "phones": null, "addresses": null,
            "links": null, "anniversaries/x": null, "notes": null,
        })
    );
    // A card without a birthday or a note: there is nothing of those to take away.
    for bare in [
        json!({"id": "K1"}),
        json!({"id": "K1", "anniversaries": null, "media": {}, "notes": {}}),
        json!({"id": "K1", "anniversaries": {"w1": {"kind": "wedding"}}}),
    ] {
        let made = patch(&emptied, card_of(&bare)).unwrap();
        assert_eq!(keys(&made), ["addresses", "emails", "links", "name", "nicknames", "organizations", "phones", "titles"], "{bare}");
    }
    // Through a run: an update whose patch comes to nothing is done, and the server is not asked to change.
    let server = Server::agreeing();
    let op = update("K1", RemoteContact::default(), &[ContactField::Birthday, ContactField::Notes]);
    let got = run(&server, vec![op.clone()]);
    assert_eq!(got.changed, [told(&op, ContactResult::Done)]);
    assert!(server.sets().is_empty());
}

#[test]
fn a_birthday_is_changed_by_its_own_key_and_the_other_entries_stay() {
    let op = update("K1", ada(), &[ContactField::Birthday]);
    let day = written(ContactField::Birthday, &ada()).unwrap().unwrap()["b1"].clone();
    let patched = |server: Value| Value::Object(patch(&op, card_of(&server)).unwrap());
    // The entry that is read is the one replaced: nothing names the wedding day, and nothing the card's media.
    assert_eq!(patched(kept()), json!({"anniversaries/x": day}));
    // A card that has the map and no entry of its own in it: a new key, and not one that is taken.
    let other = json!({"id": "K1", "anniversaries": {"b1": {"kind": "wedding"}, "b2": {"kind": "death"}}});
    assert_eq!(patched(other), json!({"anniversaries/b3": day}));
    assert_eq!(patched(json!({"id": "K1", "anniversaries": {}})), json!({"anniversaries/b1": day}));
    // A card that has no such map (or what is none) gets one: a path into nothing would be refused.
    for bare in [json!({"id": "K1"}), json!({"id": "K1", "anniversaries": null}), json!({"id": "K1", "anniversaries": []})] {
        assert_eq!(patched(bare.clone()), json!({"anniversaries": {"b1": day}}), "{bare}");
    }
    // Of several, the one that is read: the preferred birthday that names a day.
    let several = json!({
        "id": "K1",
        "anniversaries": {"a": {"kind": "birth", "date": {"year": 1815}}, "c": {"kind": "birth", "date": {"year": 1815, "month": 1, "day": 2}},
            "b": {"kind": "birth", "pref": 1, "date": {"utc": "1815-12-10T00:00:00Z"}}},
    });
    assert_eq!(patched(several.clone()), json!({"anniversaries/b": day}));
    let emptied = update("K1", RemoteContact::default(), &[ContactField::Birthday]);
    assert_eq!(Value::Object(patch(&emptied, card_of(&several)).unwrap()), json!({"anniversaries/b": null}));
    // A key is written as a part of a path; one too long to be a key is not named at all.
    let born = json!({"kind": "birth", "date": {"year": 1815, "month": 1, "day": 2}});
    let odd = json!({"id": "K1", "anniversaries": {"a/b~c": born}});
    assert_eq!(keys(card_of(&patched(odd))), ["anniversaries/a~1b~0c"]);
    let long = json!({"id": "K1", "anniversaries": {"k".repeat(300): born}});
    assert_eq!(patch(&op, card_of(&long)).err(), Some("the server keeps this contact in a way that can not be added to"));
    // A map that has every key that would be tried.
    let full: Map<String, Value> = (1..=1_000).map(|n| (format!("b{n}"), json!({"kind": "wedding"}))).collect();
    assert!(patch(&op, card_of(&json!({"id": "K1", "anniversaries": full}))).is_err());
}

#[test]
fn notes_are_replaced_only_where_none_is_lost() {
    let op = update("K1", ada(), &[ContactField::Notes]);
    let ours = written(ContactField::Notes, &ada()).unwrap().unwrap();
    let patched = |server: Value| patch(&op, card_of(&server)).map(Value::Object);
    // One note, or none: replaced whole.
    for few in
        [kept(), json!({"id": "K1"}), json!({"id": "K1", "notes": null}), json!({"id": "K1", "notes": {}}), json!({"id": "K1", "notes": {"z": 7}})]
    {
        assert_eq!(patched(few.clone()), Ok(json!({"notes": ours})), "{few}");
    }
    // Exactly the note that would be written: nothing to do.
    assert_eq!(patched(json!({"id": "K1", "notes": ours})), Ok(json!({})));
    // Several that are not the contact's notes: refused, and nothing of them is named.
    let several = json!({"id": "K1", "notes": {"a": {"note": "First programmer."}, "b": {"note": "Likes weaving.", "author": {"name": "Zoe"}}}});
    assert_eq!(patched(several.clone()), Err("the server keeps several notes with this contact"));
    let emptied = update("K1", RemoteContact::default(), &[ContactField::Notes, ContactField::Name]);
    assert_eq!(patch(&emptied, card_of(&several)).err(), Some("the server keeps several notes with this contact"));
    // Several that read as the contact's notes already: nothing to do.
    let same = json!({"id": "K1", "notes": {"a": {"note": "First programmer."}, "b": {"note": "Likes looms."}}});
    assert_eq!(patched(same), Ok(json!({})));
    // One note taken away.
    let cleared = update("K1", RemoteContact::default(), &[ContactField::Notes]);
    assert_eq!(patch(&cleared, card_of(&kept())).map(Value::Object), Ok(json!({"notes": null})));

    // Through a run: the refusal is told, the server is not asked to change the card, and the later changes
    // of that card wait.
    let server = Server::new(move |name, args, _| reads_first(name, args).then(|| having(args, std::slice::from_ref(&several))));
    let ops = vec![op.clone(), delete("K1"), update("K3", named("Bo"), &[ContactField::Name])];
    let got = run(&server, ops.clone());
    assert_eq!(got.changed, [refusal_of(&op, "the server keeps several notes with this contact"), told(&ops[2], ContactResult::Done)]);
    assert_eq!(server.sets().len(), 1);
    assert_eq!(keys(card_of(&server.sets()[0])), ["accountId", "update"]);
    assert!(read_all(&got) && got.errors.is_empty());
}

#[test]
fn a_contact_is_deleted_and_one_the_server_has_not_got_is_gone() {
    let server = Server::agreeing();
    let op = delete("K1");
    let got = run(&server, vec![op.clone()]);
    assert_eq!(got.changed, [told(&op, ContactResult::Done)]);
    assert_eq!(server.sets(), [json!({"accountId": "A1", "destroy": ["K1"]})]);
    assert_eq!(server.methods(), ["ContactCard/set", "ContactCard/query", "ContactCard/get"]);

    // `notFound`, for a change and for a deletion; and a card that is gone when it is read first.
    let server = Server::new(|name, args, _| match name {
        "ContactCard/set" => Some(Said::Is(json!({
            "created": {"k3": {"id": "N1"}}, "updated": null, "destroyed": [],
            "notUpdated": {"K1": {"type": "notFound"}}, "notDestroyed": {"K2": {"type": "notFound", "description": "no such card"}},
        }))),
        _ if reads_first(name, args) => Some(having(args, &[])),
        _ => None,
    });
    let ops = vec![update("K1", ada(), &[ContactField::Name]), delete("K2"), update("K4", ada(), &[ContactField::Birthday]), create(9, named("Bo"))];
    let got = run(&server, ops.clone());
    assert_eq!(
        got.changed,
        [
            told(&ops[2], ContactResult::Gone),
            told(&ops[0], ContactResult::Gone),
            told(&ops[1], ContactResult::Gone),
            ContactOutcome { id: "N1".into(), ..told(&ops[3], ContactResult::Done) },
        ]
    );
    assert!(got.changed.iter().all(|done| done.changed.is_empty() && done.error.is_empty()));
    // The card that was gone already was not named to the server again.
    assert_eq!(keys(card_of(&server.sets()[0]["update"])), ["K1"]);
    assert!(read_all(&got) && got.errors.is_empty());
}

#[test]
fn a_refusal_says_why_without_an_address_or_the_secret() {
    let server = Server::new(|name, _, _| {
        (name == "ContactCard/set").then(|| {
            Said::Is(json!({
                "notCreated": {"k0": {"type": "invalidProperties", "properties": ["name"]}, "k4": {"type": "notFound"}},
                "notUpdated": {"K1": {"type": "forbidden", "description": format!("zoe@example.org may not change this for {ME} (token {TOKEN}) at {SESSION}\nat all")}},
                "notDestroyed": {"K2": {"type": format!("x{TOKEN}")}, "K3": {"type": "forbidden", "description": "  "}},
            }))
        })
    });
    let ops = vec![create(21, ada()), update("K1", ada(), &[ContactField::Name]), delete("K2"), delete("K3"), create(22, named("Bo"))];
    let got = run(&server, ops.clone());
    assert_eq!(got.changed.len(), 5);
    for (done, op) in got.changed.iter().zip(&ops) {
        assert_eq!((done.kind, done.contact, done.id.as_str(), done.result), (op.kind, op.contact, op.id.as_str(), ContactResult::Refused));
        assert!(done.changed.is_empty());
        for hidden in ["zoe@example.org", ME, TOKEN, "example.org", SESSION, "\n", "Ada", "Engines"] {
            assert!(!done.error.contains(hidden), "{hidden} shows in {:?}", done.error);
        }
        assert!(!done.error.is_empty() && done.error.chars().count() <= 200, "{:?}", done.error);
    }
    // The server's words where it gave some, cleaned; else the kind of error; else that it did not say. A
    // card that could not be made is refused, whatever the server calls the reason.
    let whys: Vec<&str> = got.changed.iter().map(|done| done.error.as_str()).collect();
    assert_eq!(whys[0], "invalidProperties");
    assert!(whys[1].contains("may not change this"), "{:?}", whys[1]);
    assert_eq!(whys[2], "the server refused it without saying why");
    assert_eq!(whys[3], "forbidden");
    assert_eq!(whys[4], "notFound");
    // A refusal is told with the change and is nothing wrong with the run.
    assert!(read_all(&got) && got.errors.is_empty(), "{got:?}");
}

#[test]
fn a_contact_the_server_made_without_saying_what_it_calls_it_is_not_made_again() {
    for made in [json!(null), json!({}), json!({"id": 7}), json!({"id": "a b"}), json!({"id": "x".repeat(300)}), json!("N1"), json!(["N1"])] {
        let said = made.clone();
        let server = Server::new(move |name, _, _| (name == "ContactCard/set").then(|| Said::Is(json!({"created": {"k0": said}}))));
        let op = create(21, ada());
        let got = run(&server, vec![op.clone()]);
        assert_eq!(got.changed, [refusal_of(&op, "the server made the contact and did not say what it calls it")], "{made}");
    }
}

#[test]
fn a_change_the_answer_passes_over_is_looked_up_and_what_the_server_has_not_got_is_gone() {
    // The set answers for K1 alone. Of the others, the server has K3 still and has neither K2 nor K4.
    let server = Server::new(|name, args, _| match name {
        "ContactCard/set" => Some(Said::Is(json!({"accountId": "A1", "updated": {"K1": null}}))),
        _ if is_look(name, args) => Some(Said::Is(json!({"accountId": "A1", "list": [{"id": "K3"}], "notFound": ["K2", "K4", "K1", "K9", 7]}))),
        _ => None,
    });
    let name = [ContactField::Name];
    let ops = vec![update("K1", ada(), &name), update("K2", ada(), &name), delete("K3"), delete("K4"), create(21, ada()), delete("K5")];
    let got = run(&server, ops.clone());
    assert_eq!(got.changed, [told(&ops[0], ContactResult::Done), told(&ops[1], ContactResult::Gone), told(&ops[3], ContactResult::Gone)]);
    // One look, for the ids alone, of the cards that went unanswered: not of the card that was to be made,
    // which has no id to look for.
    assert_eq!(server.methods(), ["AddressBook/get", "ContactCard/set", "ContactCard/get", "ContactCard/query", "ContactCard/get"]);
    assert_eq!(
        server.bodies.borrow()[2],
        json!({
            "using": [CAPABILITY_CORE, CAPABILITY_CONTACTS],
            "methodCalls": [["ContactCard/get", {"accountId": "A1", "ids": ["K2", "K3", "K4", "K5"], "properties": ["id"]}, "c0"]],
        })
    );
    // Nothing is wrong with the run: what was not reported is worked out again for the next.
    assert!(read_all(&got) && got.errors.is_empty(), "{got:?}");

    // A look that fails reports nothing of them, says nothing, and the run goes on to read.
    for (what, said) in [
        ("an error", Said::Error("serverFail", format!("{ME} can not be read"))),
        ("no answer", Said::Missing),
        ("no connection", Said::Down),
        ("no JMAP", Said::Garbled),
        ("wrong types", Said::Is(json!({"notFound": {"K2": true}, "list": "K4"}))),
    ] {
        let said = RefCell::new(Some(said));
        let server = Server::new(move |name, args, _| match name {
            "ContactCard/set" => Some(Said::Is(json!({"updated": {"K1": null}}))),
            _ if is_look(name, args) => said.borrow_mut().take(),
            _ => None,
        });
        let got = run(&server, ops.clone());
        assert_eq!(got.changed, [told(&ops[0], ContactResult::Done)], "{what}");
        assert!(read_all(&got) && got.errors.is_empty(), "{what}: {got:?}");
    }

    // After a card went unanswered, its later changes wait for the next run.
    let server = Server::new(|name, _, _| (name == "ContactCard/set").then(|| Said::Is(json!({"updated": {}}))));
    let got = run(&server, vec![update("K1", ada(), &name), delete("K1"), update("K1", ada(), &name)]);
    assert!(got.changed.is_empty());
    assert_eq!(server.sets().len(), 1);
}

#[test]
fn changes_go_as_many_to_a_call_as_the_server_takes_and_none_twice() {
    let ids_of = |set: &Value| -> Vec<String> {
        let update = set["update"].as_object().into_iter().flatten().map(|(id, _)| id.clone());
        let create = set["create"].as_object().into_iter().flatten().map(|(key, _)| key.clone());
        update.chain(create).chain(set["destroy"].as_array().into_iter().flatten().map(|id| id.as_str().unwrap().to_string())).collect()
    };
    // Two to a call.
    let server = Server::with(json!({"maxObjectsInSet": 2}), |_, _, _| None);
    let ops: Vec<ContactOp> = vec![delete("K1"), update("K2", ada(), &[ContactField::Name]), create(21, ada()), delete("K4"), create(22, ada())];
    let got = run(&server, ops.clone());
    assert_eq!(server.sets().iter().map(&ids_of).collect::<Vec<_>>(), [vec!["K2", "K1"], vec!["k2", "K4"], vec!["k4"]]);
    assert_eq!(got.changed.len(), 5);
    assert_eq!(server.args_of("AddressBook/get").len(), 1);
    // A hundred to a run, whatever the server would take; the rest come with the next request.
    let server = Server::with(json!({"maxObjectsInSet": 100_000}), |_, _, _| None);
    let many: Vec<ContactOp> = (0..130).map(|n| delete(&format!("K{n}"))).collect();
    let got = run(&server, many);
    assert_eq!(server.sets().len(), 1);
    assert_eq!(ids_of(&server.sets()[0]).len(), MAX_CONTACT_OPS);
    assert_eq!(got.changed.len(), MAX_CONTACT_OPS);
    assert_eq!(got.changed.last().unwrap().id, "K99");
    // A server that takes none at a time is given one.
    let server = Server::with(json!({"maxObjectsInSet": 0}), |_, _, _| None);
    run(&server, vec![delete("K1"), delete("K2")]);
    assert_eq!(server.sets().len(), 2);
    // Two changes of one card are two calls, in their order.
    let server = Server::agreeing();
    let got = run(&server, vec![update("K1", ada(), &[ContactField::Name]), delete("K2"), delete("K1"), delete("K3")]);
    assert_eq!(server.sets().iter().map(&ids_of).collect::<Vec<_>>(), [vec!["K1", "K2"], vec!["K1", "K3"]]);
    assert_eq!(got.changed.len(), 4);
    // No more to a call than fit a request: two contacts with long notes do not go together.
    let server = Server::with(json!({"maxSizeRequest": 100_000}), |_, _, _| None);
    let heavy = RemoteContact { notes: "n".repeat(25_000), ..ada() };
    let got = run(&server, vec![update("K1", heavy.clone(), &[ContactField::Name]), update("K2", heavy, &[ContactField::Name]), delete("K3")]);
    assert_eq!(server.sets().iter().map(&ids_of).collect::<Vec<_>>(), [vec!["K1"], vec!["K2", "K3"]]);
    assert_eq!(got.changed.len(), 3);
    assert!(got.errors.is_empty());
}

#[test]
fn what_can_not_be_asked_is_refused_without_asking_and_what_names_no_part_is_done() {
    let server = Server::agreeing();
    let long = "x".repeat(300);
    let ops = vec![
        update("a b", ada(), &[ContactField::Name]),
        delete(""),
        delete("K1/../K2"),
        update(&long, ada(), &[]),
        update("K7", ada(), &[]),
        delete("zoe@example.org"),
    ];
    let got = run(&server, ops.clone());
    let why = "the contact's id is not one a server hands out";
    assert_eq!(
        got.changed,
        [
            refusal_of(&ops[0], why),
            refusal_of(&ops[1], why),
            refusal_of(&ops[2], why),
            refusal_of(&ops[3], why),
            told(&ops[4], ContactResult::Done),
            refusal_of(&ops[5], why),
        ]
    );
    // Nothing was asked but the reading.
    assert_eq!(server.methods(), ["ContactCard/query", "ContactCard/get"]);
    assert!(read_all(&got) && got.errors.is_empty());
}

#[test]
fn a_birthday_no_calendar_has_is_refused_here() {
    let server = Server::agreeing();
    let odd = RemoteContact { birthday: Some(Date { year: 2001, month: 2, day: 30 }), ..ada() };
    let ops = vec![
        create(21, odd.clone()),
        // (A day that is not sent is not looked at.)
        update("K3", odd.clone(), &[ContactField::Title]),
        update("K4", odd, &[ContactField::Birthday]),
    ];
    let got = run(&server, ops.clone());
    let why = "the birthday is not a day a server keeps";
    assert_eq!(got.changed, [refusal_of(&ops[0], why), refusal_of(&ops[2], why), told(&ops[1], ContactResult::Done)]);
    // Nothing of the refused ones went to the server.
    assert_eq!(server.sets(), [json!({"accountId": "A1", "update": {"K3": {"titles": {"t1": {"name": "Analyst"}}}}})]);
}

/// Changes that take every step there is: the address book, the cards read first, the set, and (with
/// [`stepping`]) the look for a card the set's answer left out.
fn every_step() -> Vec<ContactOp> {
    vec![create(21, named("Bo")), update("K1", ada(), &[ContactField::Birthday]), delete("K2"), delete("K3")]
}

/// A server that says nothing of `K3` in a set, and answers the call after `before` others as `at` says.
fn stepping(before: usize, at: impl Fn() -> Option<Said> + 'static) -> Server {
    Server::new(move |name, _, n| {
        if n == before
            && let Some(said) = at()
        {
            return Some(said);
        }
        (name == "ContactCard/set").then(|| Said::Is(json!({"created": {"k0": {"id": "N1"}}, "updated": {"K1": null}, "destroyed": ["K2"]})))
    })
}

const STEPS: [&str; 4] = ["AddressBook/get", "ContactCard/get", "ContactCard/set", "ContactCard/get"];

#[test]
fn a_refused_sign_in_and_a_busy_server_end_the_run_at_every_step() {
    // (With nothing in the way: the four steps, then the reading.)
    let server = stepping(usize::MAX, || None);
    let got = run(&server, every_step());
    assert_eq!(server.methods()[..4], STEPS);
    assert_eq!(got.changed.len(), 3);
    assert!(read_all(&got));

    for step in 0..STEPS.len() {
        // What the set answered is handed over when the run ends after it.
        let answered = if step == 3 { 3 } else { 0 };
        // 401, and 403: the owner signs in again, and nothing is read.
        for status in [401, 403] {
            let server = stepping(step, move || Some(Said::Status(status, None)));
            let got = run(&server, every_step());
            assert!(got.auth_refused && !got.stopped && got.busy_for.is_none() && got.errors.is_empty(), "step {step}: {got:?}");
            assert_eq!(server.methods(), STEPS[..=step], "step {step}");
            assert_eq!((got.changed.len(), got.contacts.is_none()), (answered, true), "step {step}");
        }
        // 429 and 503: the server is left alone as long as it asks.
        for (status, retry_after, seconds) in [(429, Some(120), 120), (503, None, 30)] {
            let server = stepping(step, move || Some(Said::Status(status, retry_after)));
            let got = run(&server, every_step());
            assert_eq!((got.busy_for, got.auth_refused, got.stopped), (Some(seconds), false, false), "step {step}, {status}");
            assert_eq!(server.methods(), STEPS[..=step], "step {step}, {status}");
            assert_eq!((got.changed.len(), got.contacts.is_none()), (answered, true), "step {step}, {status}");
            assert_eq!(got.errors.len(), 1);
            assert!(got.errors[0].starts_with(UP) && !got.errors[0].contains(ME) && !got.errors[0].contains(TOKEN), "{:?}", got.errors);
        }
    }
}

#[test]
fn cancel_is_looked_at_before_every_step() {
    // Stopped before it began: nothing is asked, nothing is told, and the changes come again.
    let server = Server::agreeing();
    let got = run_until(&server, every_step(), &AtomicBool::new(true));
    assert!(got.stopped && got.changed.is_empty() && got.contacts.is_none() && got.errors.is_empty(), "{got:?}");
    assert!(server.methods().is_empty());
    // Stopped while a step was answered: the next is not asked.
    for step in 0..STEPS.len() {
        let cancel = Rc::new(AtomicBool::new(false));
        let stops = cancel.clone();
        let server = stepping(step, move || {
            stops.store(true, Ordering::Relaxed);
            None
        });
        let got = run_until(&server, every_step(), &cancel);
        assert!(got.stopped && !got.auth_refused && got.errors.is_empty() && got.contacts.is_none(), "step {step}: {got:?}");
        assert_eq!(server.methods().len(), step + 1, "step {step}");
        // What the set answered is still told (the look for the card it left out was not made).
        assert_eq!(got.changed.len(), if step >= 2 { 3 } else { 0 }, "step {step}");
    }
}

#[test]
fn a_call_that_fails_as_a_whole_answers_none_of_its_changes_and_the_contacts_are_still_read() {
    let failing = |said: fn() -> Said| {
        let said = RefCell::new(Some(said));
        Server::with(
            json!({"maxObjectsInSet": 2}),
            move |name, _, _| if name == "ContactCard/set" { said.borrow_mut().take().map(|said| said()) } else { None },
        )
    };
    let cases: [(&str, fn() -> Said); 7] = [
        ("an error", || Said::Error("serverFail", format!("{ME} may not write (token {TOKEN})"))),
        ("too many", || Said::Error("requestTooLarge", String::new())),
        ("not now", || Said::Error("serverUnavailable", String::new())),
        ("no answer to the call", || Said::Missing),
        ("no connection", || Said::Down),
        ("no JMAP", || Said::Garbled),
        ("500", || Said::Status(500, None)),
    ];
    for (what, said) in cases {
        let server = failing(said);
        let got = run(&server, vec![delete("K1"), delete("K2"), delete("K3")]);
        // None of the call's changes is told, and the call that would follow is not made in this run.
        assert!(got.changed.is_empty(), "{what}: {:?}", got.changed);
        assert_eq!(server.sets().len(), 1, "{what}");
        assert!(!got.stopped && !got.auth_refused && got.busy_for.is_none(), "{what}: {got:?}");
        assert_eq!(got.errors.len(), 1, "{what}: {:?}", got.errors);
        assert!(got.errors[0].starts_with(UP), "{what}: {:?}", got.errors);
        for hidden in [ME, TOKEN, "example.org", "K1"] {
            assert!(!got.errors[0].contains(hidden), "{what}: {hidden} shows in {:?}", got.errors);
        }
        // The contacts are read all the same, and all of them.
        assert_eq!(server.methods(), ["ContactCard/set", "ContactCard/query", "ContactCard/get"], "{what}");
        assert!(read_all(&got), "{what}: {got:?}");
    }
    // A set whose answer is nothing that can be read: nothing is told, and nothing is wrong with the run.
    let server = Server::new(|name, _, _| (name == "ContactCard/set").then(|| Said::Is(json!("done"))));
    let got = run(&server, vec![create(21, ada())]);
    assert!(got.changed.is_empty() && read_all(&got), "{got:?}");
}

#[test]
fn what_has_to_be_known_first_and_can_not_be_had_leaves_those_changes_for_the_next_run() {
    let ops = vec![
        create(21, named("Bo")),
        update("K1", ada(), &[ContactField::Birthday]),
        update("K2", ada(), &[ContactField::Name]),
        delete("K3"),
        update("K1", ada(), &[ContactField::Name]),
    ];
    // The address books can not be listed: the contact is not made (nor refused), the rest goes.
    let server = Server::new(|name, _, _| (name == "AddressBook/get").then(|| Said::Error("serverFail", format!("{ME}?"))));
    let got = run(&server, ops.clone());
    assert_eq!(
        got.changed.iter().map(|done| (done.id.as_str(), done.result)).collect::<Vec<_>>(),
        [("K1", ContactResult::Done), ("K2", ContactResult::Done), ("K3", ContactResult::Done), ("K1", ContactResult::Done)]
    );
    assert!(server.sets().iter().all(|set| set.get("create").is_none()));
    assert_eq!(got.errors.len(), 1);
    assert!(got.errors[0].starts_with(UP) && !got.errors[0].contains(ME) && read_all(&got), "{got:?}");
    // The cards can not be read first: the change that needs it waits, and so do the later ones of its card.
    let server = Server::new(|name, args, _| reads_first(name, args).then(|| Said::Error("serverFail", String::new())));
    let got = run(&server, ops.clone());
    assert_eq!(
        got.changed.iter().map(|done| (done.id.as_str(), done.result)).collect::<Vec<_>>(),
        [("Nk0", ContactResult::Done), ("K2", ContactResult::Done), ("K3", ContactResult::Done)]
    );
    assert_eq!(server.sets().len(), 1);
    assert_eq!(keys(card_of(&server.sets()[0]["update"])), ["K2"]);
    assert_eq!(got.errors.len(), 1);
    assert!(got.errors[0].starts_with(UP) && read_all(&got), "{got:?}");
    // The answer says nothing of the card that was to be read: that change waits, without a word.
    let server =
        Server::new(|name, args, _| reads_first(name, args).then(|| Said::Is(json!({"list": [{"id": "K8"}, "K1", {"id": 7}], "notFound": "K1"}))));
    let got = run(&server, ops.clone());
    assert_eq!(got.changed.len(), 3);
    assert!(got.errors.is_empty() && read_all(&got));
    // No connection for either: the changes wait, all of them, and the contacts are read.
    for step in ["AddressBook/get", "ContactCard/get"] {
        let failed = RefCell::new(false);
        let server = Server::new(move |name, _, _| (name == step && !failed.replace(true)).then_some(Said::Down));
        let got = run(&server, ops.clone());
        assert!(got.changed.is_empty() && server.sets().is_empty(), "{step}");
        assert_eq!(got.errors.len(), 1, "{step}");
        assert!(got.errors[0].starts_with(UP) && read_all(&got), "{step}: {got:?}");
    }
}

#[test]
fn after_a_refusal_the_later_changes_of_that_card_wait_for_the_next_run() {
    let server = Server::new(|name, args, _| {
        (name == "ContactCard/set" && args["update"].get("K1").is_some())
            .then(|| Said::Is(json!({"notUpdated": {"K1": {"type": "forbidden"}}, "destroyed": ["K2"]})))
    });
    let ops = vec![update("K1", ada(), &[ContactField::Name]), delete("K2"), delete("K1"), delete("K3")];
    let got = run(&server, ops.clone());
    assert_eq!(got.changed, [refusal_of(&ops[0], "forbidden"), told(&ops[1], ContactResult::Done), told(&ops[3], ContactResult::Done)]);
    assert_eq!(server.sets()[1], json!({"accountId": "A1", "destroy": ["K3"]}));
}

#[test]
fn a_server_without_contacts_is_told_nothing() {
    let doc = json!({
        "capabilities": {CAPABILITY_CORE: {}, CAPABILITY_MAIL: {}},
        "accounts": {"A1": {"name": ME, "isPersonal": true, "accountCapabilities": {CAPABILITY_MAIL: {}}}},
        "primaryAccounts": {CAPABILITY_MAIL: "A1"}, "apiUrl": "/jmap/api",
        "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}", "uploadUrl": "/jmap/upload/{accountId}/", "state": "1",
    });
    let http = crate::test_support::FakeExchange::new(vec![json_reply(200, &doc)]);
    let connection = block_on(Connection::connect(http, SESSION, Credentials::bearer(&made_secret(TOKEN)).unwrap())).unwrap();
    let got = block_on(sync_contacts(&connection, &asking(every_step()), &AtomicBool::new(false)));
    assert!(got.not_offered && got.changed.is_empty() && got.errors.is_empty() && got.contacts.is_none());
    assert_eq!(connection.http().sent().len(), 1);
}

#[test]
fn a_hostile_answer_to_a_set_is_read_for_the_changes_asked_and_nothing_else() {
    let secrets = Redactor::new();
    let (made, changed, gone) = (create(21, ada()), update("K1", ada(), &[ContactField::Name]), delete("K1"));
    let of = |answer: Value, op: &ContactOp| outcome_of(card_of(&answer), op, if op.kind == ContactOpKind::Create { "k0" } else { "K1" }, &secrets);
    // Wrong types say nothing.
    for answer in [
        json!({}),
        json!({"created": ["k0"], "updated": ["K1"], "destroyed": {"K1": true}}),
        json!({"created": "k0", "updated": "K1", "notUpdated": ["K1"], "destroyed": "K1", "notDestroyed": 7, "notCreated": [["k0"]]}),
        json!({"created": {"k1": {"id": "N1"}}, "updated": {"K2": null}, "destroyed": ["K2", 7, null, {"K1": 1}], "notUpdated": {"K2": {"type": "notFound"}}}),
    ] {
        for op in [&made, &changed, &gone] {
            assert_eq!(of(answer.clone(), op), None, "{answer}");
        }
    }
    // A change is not answered by what the server says of another kind of change.
    assert_eq!(of(json!({"destroyed": ["K1"], "notDestroyed": {"K1": {"type": "notFound"}}}), &changed), None);
    assert_eq!(of(json!({"updated": {"K1": null}, "notUpdated": {"K1": {"type": "notFound"}}}), &gone), None);
    assert_eq!(of(json!({"updated": {"k0": null}, "destroyed": ["k0"]}), &made), None);
    // Done wins over a failure the same answer claims: a contact is not taken for gone on a muddle.
    assert_eq!(of(json!({"updated": {"K1": null}, "notUpdated": {"K1": {"type": "notFound"}}}), &changed), Some(told(&changed, ContactResult::Done)));
    assert_eq!(of(json!({"destroyed": ["K1"], "notDestroyed": {"K1": {"type": "notFound"}}}), &gone), Some(told(&gone, ContactResult::Done)));
    // An error that is no object, or has no type, is a refusal that does not say why; a huge one is cut.
    for error in [json!("no"), json!(null), json!({}), json!({"type": 7}), json!({"type": "not found"}), json!({"type": "x".repeat(65)})] {
        let outcome = of(json!({"notUpdated": {"K1": error.clone()}}), &changed);
        assert_eq!(outcome, Some(refusal_of(&changed, "the server refused it without saying why")), "{error}");
    }
    let huge = of(json!({"notDestroyed": {"K1": {"type": "forbidden", "description": "word ".repeat(100_000)}}}), &gone).unwrap();
    assert!(huge.error.starts_with("word word") && huge.error.chars().count() <= 200, "{} characters", huge.error.chars().count());
    // Thousands of entries: the one asked for is found, by its id.
    let mut updated: Map<String, Value> = (0..20_000).map(|n| (format!("X{n}"), Value::Null)).collect();
    assert_eq!(of(json!({"updated": updated.clone()}), &changed), None);
    updated.insert("K1".into(), Value::Null);
    assert_eq!(of(json!({"updated": updated}), &changed), Some(told(&changed, ContactResult::Done)));
    let listed: Vec<String> = (0..20_000).map(|n| format!("X{n}")).collect();
    assert_eq!(of(json!({"destroyed": listed}), &gone), None);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    // Whatever a server answers at any step of taking changes up, the run ends without a panic, tells of no
    // change that was not asked, of none twice, and names a new contact only by what is an id.
    #[test]
    fn any_answers_to_the_changes_are_read_without_panicking(
        answer in any_json(),
        entry in any_json(),
        under in proptest::sample::select(vec!["created", "notCreated", "updated", "notUpdated", "destroyed", "notDestroyed", "list", "notFound"]),
        key in proptest::sample::select(vec!["k0", "K1", "K2", "K3", "B1", "id"]),
    ) {
        let secrets = Redactor::new();
        let mut shaped = json!({});
        shaped[under] = json!({key: entry.clone()});
        let listed = json!({under: [key, entry.clone(), {"id": key, "anniversaries": entry.clone(), "media": entry.clone(), "notes": entry.clone(), "myRights": entry}]});
        for answer in [answer, shaped, listed] {
            for op in every_step() {
                if let Some(answer) = answer.as_object() {
                    let _ = outcome_of(answer, &op, key, &secrets);
                    let _ = book_of(answer);
                    let _ = patch(&update("K1", ada(), &ContactField::ALL), answer);
                }
            }
            // As the answer of every step at once, and of each step alone.
            for step in [usize::MAX, 0, 1, 2, 3] {
                let said = answer.clone();
                let server = Server::new(move |name, _, n| (!name.starts_with("ContactCard/query") && (step == usize::MAX || step == n)).then(|| Said::Is(said.clone())));
                let ops = every_step();
                let got = run(&server, ops.clone());
                prop_assert!(got.changed.len() <= ops.len());
                for done in &got.changed {
                    let known = ops.iter().filter(|op| op.kind == done.kind && (op.kind == ContactOpKind::Create || op.id == done.id)).count();
                    prop_assert_eq!(known, 1);
                    prop_assert!(done.kind != ContactOpKind::Create || done.result != ContactResult::Done || Id::new(&done.id).is_ok());
                    prop_assert!(done.error.chars().count() <= 200);
                }
                let mut whose: Vec<(ContactOpKind, u64, &str)> = got.changed.iter().map(|done| (done.kind, done.contact, done.id.as_str())).collect();
                whose.sort_by(|a, b| (a.1, a.2).cmp(&(b.1, b.2)));
                whose.dedup();
                prop_assert_eq!(whose.len(), got.changed.len());
                // Whatever came back, every request went to the API, and a set never names more than was asked.
                for set in server.sets() {
                    let maps = ["create", "update"].iter().map(|part| set[*part].as_object().map_or(0, Map::len)).sum::<usize>();
                    prop_assert!(maps + set["destroy"].as_array().map_or(0, Vec::len) <= ops.len());
                }
            }
        }
    }

    // Any card as the server's, for any change: a patch names only properties of the parts that changed,
    // whole or by one key of theirs.
    #[test]
    fn a_patch_names_only_the_parts_that_changed(server in any_json(), entry in any_json(), fields in proptest::sample::subsequence(ContactField::ALL.to_vec(), 0..=10), empty in any::<bool>(), key in "[a-z/~]{1,4}") {
        let shaped = json!({"id": "K1", "anniversaries": {key: {"kind": "birth", "date": {"year": 1900, "month": 1, "day": 1}}, "z": entry.clone()},
            "media": {"z": entry.clone()}, "notes": {"z": entry}});
        let op = update("K1", if empty { RemoteContact::default() } else { ada() }, &fields);
        for server in [server, shaped] {
            let Some(server) = server.as_object() else { continue };
            if let Ok(made) = patch(&op, server) {
                for path in made.keys() {
                    let property_named = path.split('/').next().unwrap_or_default();
                    prop_assert!(fields.iter().any(|field| property(*field) == property_named), "{} for {:?}", path, fields);
                    prop_assert!(path.matches('/').count() <= 1);
                    // (Only the birthday is changed by a key.)
                    prop_assert!(!path.contains('/') || property_named == "anniversaries");
                }
                prop_assert!(made.len() <= fields.len());
            }
        }
    }
}

/// A web address that a card would not keep is not written around: the change is refused and says why,
/// before anything is asked, so the owner's address is not lost at the next read.
#[test]
fn a_change_with_a_web_address_that_is_none_is_refused_and_nothing_is_asked() {
    for odd in ["www.example.org", "ftp://example.org/file", "example"] {
        let card = RemoteContact { first: "Dee".into(), websites: vec!["https://example.org".into(), odd.into()], ..Default::default() };
        let create = ContactOp { kind: ContactOpKind::Create, contact: 3, card: card.clone(), ..Default::default() };
        assert!(matches!(super::changes::plan(&create), super::changes::Plan::Refuse(why) if why.contains("http://")), "{odd}");
        let update =
            ContactOp { kind: ContactOpKind::Update, contact: 3, id: "K1".into(), card: card.clone(), changed: vec![ContactField::Websites] };
        assert!(matches!(super::changes::plan(&update), super::changes::Plan::Refuse(_)), "{odd}");
        // A change that does not write the web addresses goes, whatever they are.
        let other = ContactOp { changed: vec![ContactField::Title], ..update };
        assert!(matches!(super::changes::plan(&other), super::changes::Plan::Named("K1")), "{odd}");
    }
    let fine = RemoteContact { first: "Dee".into(), websites: vec![" HTTPS://example.org/a ".into(), String::new()], ..Default::default() };
    assert!(matches!(
        super::changes::plan(&ContactOp { kind: ContactOpKind::Create, card: fine, ..Default::default() }),
        super::changes::Plan::Create
    ));
}
