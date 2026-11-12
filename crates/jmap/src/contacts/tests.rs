// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts: a scripted server for what is asked and how the answers are read, cards of every shape for
//! the mapping, and whatever a server may say for what must never panic.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use mailcraft_model::{Labeled, PostalAddress};
use mailcraft_time::Date;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

use super::card::card;
use super::*;
use crate::connection::tests::any_json;
use crate::reader::{MAX_CHUNK, PAGE, QUERY_BYTES};
use crate::test_support::{FakeExchange, made_secret};
use crate::{CAPABILITY_CORE, CAPABILITY_MAIL, Credentials, HttpRequest, HttpResponse, Method, RetryAfter, TransportError, block_on};

const SESSION: &str = "https://jmap.example.org/jmap/session";
const API: &str = "https://jmap.example.org/jmap/api";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR-not-much-of-a-picture";
const JPEG: &[u8] = b"\xff\xd8\xff\xe0\0\x10JFIF-not-much-of-a-picture";

type Reply = std::result::Result<HttpResponse, TransportError>;

fn reply(status: u16, body: &Value) -> Reply {
    Ok(HttpResponse::new(status, serde_json::to_vec(body).unwrap()).with_field("Content-Type", "application/json"))
}

fn octets(status: u16, body: &[u8]) -> Reply {
    Ok(HttpResponse::new(status, body.to_vec()))
}

/// What the one call of a request answers.
fn answer(method: &str, arguments: Value) -> Reply {
    reply(200, &json!({"methodResponses": [[method, arguments, "c0"]], "sessionState": "1"}))
}

fn query(ids: Value) -> Reply {
    let total = ids.as_array().map_or(0, Vec::len);
    answer("ContactCard/query", json!({"ids": ids, "position": 0, "total": total}))
}

fn get(list: Value) -> Reply {
    answer("ContactCard/get", json!({"list": list, "notFound": []}))
}

fn refused(kind: &str) -> Reply {
    answer("error", json!({"type": kind}))
}

/// A session whose account `A1` has mail and contacts, the server taking `max_get` objects in a `get`.
fn session_doc(max_get: u64) -> Value {
    json!({
        "capabilities": {CAPABILITY_CORE: {"maxCallsInRequest": 16, "maxObjectsInGet": max_get}, CAPABILITY_MAIL: {}, CAPABILITY_CONTACTS: {}},
        "accounts": {"A1": {"name": "ada@example.org", "isPersonal": true, "accountCapabilities": {CAPABILITY_MAIL: {}, CAPABILITY_CONTACTS: {}}}},
        "primaryAccounts": {CAPABILITY_MAIL: "A1", CAPABILITY_CONTACTS: "A1"},
        "apiUrl": "/jmap/api",
        "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
        "uploadUrl": "/jmap/upload/{accountId}/",
        "state": "1",
    })
}

fn connect<H: HttpExchange>(http: H) -> Connection<H> {
    block_on(Connection::connect(http, SESSION, Credentials::bearer(&made_secret("tk-3f9a7c")).unwrap())).unwrap()
}

/// A connection over a script: the session `doc`, then `replies`.
fn connected_with(doc: &Value, replies: Vec<Reply>) -> Connection<FakeExchange> {
    let mut script = vec![reply(200, doc)];
    script.extend(replies);
    connect(FakeExchange::new(script))
}

fn connected(replies: Vec<Reply>) -> Connection<FakeExchange> {
    connected_with(&session_doc(500), replies)
}

fn asked() -> JmapContactsRequest {
    JmapContactsRequest { account: 7, email: "ada@example.org".into(), session: SESSION.into(), bearer: true, changes: Vec::new() }
}

fn run<H: HttpExchange>(connection: &Connection<H>) -> JmapContactsResult {
    block_on(sync_contacts(connection, &asked(), &AtomicBool::new(false)))
}

/// The requests after the session's.
fn sent(connection: &Connection<FakeExchange>) -> Vec<HttpRequest> {
    connection.http().sent().into_iter().skip(1).collect()
}

/// The one call of each API request sent: its method and its arguments.
fn calls(connection: &Connection<FakeExchange>) -> Vec<(String, Value)> {
    sent(connection)
        .iter()
        .filter(|request| request.method() == Method::Post)
        .map(|request| {
            let body: Value = serde_json::from_slice(request.body()).unwrap();
            (body["methodCalls"][0][0].as_str().unwrap().to_string(), body["methodCalls"][0][1].clone())
        })
        .collect()
}

fn download(got: &JmapContactsResult) -> &ContactsDownload {
    got.contacts.as_ref().expect("the contacts were read")
}

fn ids(got: &JmapContactsResult) -> Vec<&str> {
    download(got).contacts.iter().map(|contact| contact.id.as_str()).collect()
}

fn data_uri(kind: &str, bytes: &[u8]) -> String {
    format!("data:{kind};base64,{}", mailcraft_charset::base64::STANDARD.encode(bytes))
}

fn plain(id: &str) -> Value {
    json!({"id": id, "name": {"full": format!("Person {id}")}})
}

fn ada() -> Value {
    json!({
        "id": "K1", "@type": "Card", "version": "1.0", "uid": "urn:uuid:0f0e", "kind": "individual", "updated": "2026-10-14T09:00:00Z",
        "name": {"full": "Ada Example", "components": [
            {"kind": "surname2", "value": "Lovelace"}, {"kind": "given", "value": "Ada"}, {"kind": "given2", "value": "Maria"},
            {"kind": "surname", "value": "Example"}, {"kind": "credential", "value": "PhD"}]},
        "nicknames": {"n1": {"name": "Addie"}, "n2": {"name": "A."}},
        "organizations": {"o1": {"name": "Engines Ltd", "units": [{"name": "Research"}, {"name": "Looms"}]}, "o2": {"name": "Other"}},
        "titles": {"t1": {"name": "Analyst"}, "t2": {"name": "Poet"}},
        "emails": {
            "e1": {"address": "ada@home.example", "contexts": {"private": true}},
            "e2": {"address": "ada@work.example", "contexts": {"work": true}, "pref": 1},
            "e3": {"address": "ada@example.org", "label": "club", "contexts": {"work": true}},
            "e4": {"address": ""}},
        "phones": {
            "p1": {"number": "+1 555 0100", "features": {"mobile": true, "voice": true}, "contexts": {"private": true}},
            "p2": {"number": "+1 555 0101", "contexts": {"work": true}},
            "p3": {"number": "+1 555 0102", "features": {"fax": true}},
            "p4": {"number": "+1 555 0103", "label": "boat", "features": {"mobile": true}}},
        "addresses": {
            "a1": {"contexts": {"work": true}, "components": [
                {"kind": "number", "value": "12"}, {"kind": "name", "value": "Mill Lane"}, {"kind": "apartment", "value": "Flat 3"},
                {"kind": "locality", "value": "Leeds"}, {"kind": "region", "value": "Yorkshire"}, {"kind": "postcode", "value": "LS1 1AA"},
                {"kind": "country", "value": "UK"}], "countryCode": "GB"},
            "a2": {"full": "1 Old Road, York", "contexts": {"private": true}},
            "a3": {"isOrdered": true, "contexts": {"billing": true}, "components": [
                {"kind": "number", "value": "54321"}, {"kind": "separator", "value": " "}, {"kind": "name", "value": "Oak St"},
                {"kind": "locality", "value": "Reston"}]}},
        "links": {"l1": {"uri": "https://ada.example/"}, "l2": {"uri": "mailto:ada@example.org"}, "l3": {"uri": "HTTP://old.example/x"}},
        "onlineServices": {"s1": {"service": "Chat", "uri": "https://chat.example/ada"}, "s2": {"service": "Talk", "user": "ada"}},
        "anniversaries": {
            "a": {"kind": "wedding", "date": {"year": 1835, "month": 7, "day": 8}},
            "b": {"kind": "birth", "date": {"month": 12, "day": 10}},
            "c": {"kind": "birth", "date": {"year": 1815, "month": 12, "day": 10}}},
        "notes": {"n1": {"note": "First."}, "n2": {"note": " Second. "}, "n3": {"note": 3}},
        "media": {
            "m0": {"kind": "logo", "uri": data_uri("image/png", JPEG)},
            "m1": {"kind": "photo", "uri": "https://pictures.example/ada.png"},
            "m2": {"kind": "photo", "uri": data_uri("image/png", PNG), "blobId": "B1"}},
    })
}

// ------------------------------------------------------------------------------------------
// Whether, and whose

#[test]
fn a_server_without_contacts_is_no_failure_and_is_asked_nothing() {
    // Not advertised at all.
    let mut doc = session_doc(500);
    doc["capabilities"].as_object_mut().unwrap().remove(CAPABILITY_CONTACTS);
    // Advertised, and no account offers them.
    let mut none = session_doc(500);
    none["accounts"]["A1"]["accountCapabilities"].as_object_mut().unwrap().remove(CAPABILITY_CONTACTS);
    none["primaryAccounts"].as_object_mut().unwrap().remove(CAPABILITY_CONTACTS);
    // Advertised, and the account named for them does not offer them, nor the one for mail.
    let mut named = session_doc(500);
    named["accounts"]["A1"]["accountCapabilities"].as_object_mut().unwrap().remove(CAPABILITY_CONTACTS);
    for doc in [doc, none, named] {
        let connection = connected_with(&doc, vec![]);
        let got = run(&connection);
        assert!(got.not_offered && got.contacts.is_none() && got.errors.is_empty() && !got.stopped && !got.auth_refused);
        assert_eq!((got.account, got.email.as_str(), got.session.as_str()), (7, "ada@example.org", SESSION));
        assert!(sent(&connection).is_empty());
    }
}

#[test]
fn the_account_the_session_names_for_contacts_is_read() {
    let mut doc = session_doc(500);
    doc["accounts"]["A1"]["accountCapabilities"].as_object_mut().unwrap().remove(CAPABILITY_CONTACTS);
    doc["accounts"]["C1"] = json!({"name": "book", "isPersonal": true, "accountCapabilities": {CAPABILITY_CONTACTS: {}}});
    doc["primaryAccounts"][CAPABILITY_CONTACTS] = json!("C1");
    let connection = connected_with(&doc, vec![query(json!(["K1"])), get(json!([plain("K1")]))]);
    let got = run(&connection);
    assert!(download(&got).complete && got.errors.is_empty());
    assert_eq!(ids(&got), ["K1"]);
    assert_eq!(calls(&connection).len(), 2);
    assert!(calls(&connection).iter().all(|(_, args)| args["accountId"] == "C1"));
    // No account named for contacts: the one for mail, when it offers them.
    let mut doc = session_doc(500);
    doc["primaryAccounts"].as_object_mut().unwrap().remove(CAPABILITY_CONTACTS);
    let connection = connected_with(&doc, vec![query(json!([]))]);
    let got = run(&connection);
    assert!(!got.not_offered && download(&got).complete && download(&got).contacts.is_empty());
    assert_eq!(calls(&connection)[0].1["accountId"], "A1");
}

// ------------------------------------------------------------------------------------------
// The run

#[test]
fn the_cards_are_listed_read_and_made_into_contacts() {
    let company = json!({
        "id": "K2", "kind": "org", "name": {"full": "Engines Ltd"},
        "anniversaries": {"b": {"kind": "birth", "date": {"@type": "Timestamp", "utc": "1990-02-03T04:05:06Z"}}},
        "media": {"m": {"kind": "photo", "blobId": "B9", "mediaType": "image/jpeg"}},
    });
    let group = json!({"id": "K3", "kind": "group", "name": {"full": "Friends"}, "members": {"urn:uuid:0f0e": true}});
    let replies = vec![query(json!(["K1", "K2", "K3"])), get(json!([ada(), company, group]))];
    let connection = connected(replies);
    let got = run(&connection);
    assert!(got.errors.is_empty() && !got.not_offered && !got.stopped && !got.auth_refused && got.busy_for.is_none(), "{:?}", got.errors);
    assert_eq!((got.account, got.email.as_str(), got.session.as_str()), (7, "ada@example.org", SESSION));
    let down = download(&got);
    assert_eq!((down.account, down.email.as_str(), down.complete), (7, "ada@example.org", true));
    assert_eq!(ids(&got), ["K1", "K2"], "a group is no contact");

    let ada = &down.contacts[0];
    assert_eq!(ada.change_key, "");
    assert_eq!(
        (ada.display_name.as_str(), ada.first.as_str(), ada.middle.as_str(), ada.last.as_str()),
        ("Ada Example", "Ada", "Maria", "Example Lovelace")
    );
    assert_eq!(
        (ada.nickname.as_str(), ada.company.as_str(), ada.department.as_str(), ada.title.as_str()),
        ("Addie", "Engines Ltd", "Research", "Analyst")
    );
    // The preferred first, then by key; the label, else the context.
    assert_eq!(
        ada.emails,
        [Labeled::new("work", "ada@work.example"), Labeled::new("home", "ada@home.example"), Labeled::new("club", "ada@example.org")]
    );
    assert_eq!(
        ada.phones,
        [
            Labeled::new("mobile", "+1 555 0100"),
            Labeled::new("work", "+1 555 0101"),
            Labeled::new("fax", "+1 555 0102"),
            Labeled::new("boat", "+1 555 0103")
        ]
    );
    let work = PostalAddress {
        label: "work".into(),
        street: "Mill Lane 12, Flat 3".into(),
        city: "Leeds".into(),
        region: "Yorkshire".into(),
        postal_code: "LS1 1AA".into(),
        country: "UK".into(),
    };
    let home = PostalAddress { label: "home".into(), street: "1 Old Road, York".into(), ..Default::default() };
    let billing = PostalAddress { label: "billing".into(), street: "54321 Oak St".into(), city: "Reston".into(), ..Default::default() };
    assert_eq!(ada.addresses, [work, home, billing]);
    assert_eq!(ada.websites, ["https://ada.example/", "HTTP://old.example/x", "https://chat.example/ada"]);
    assert_eq!(ada.birthday, Date::new(1815, 12, 10));
    assert_eq!(ada.notes, "First.\n\nSecond.");
    assert_eq!(ada.modified, Date::new(2026, 10, 14).unwrap().to_days() * 86_400 + 9 * 3_600);

    let company = &down.contacts[1];
    assert_eq!((company.display_name.as_str(), company.first.as_str(), company.last.as_str()), ("Engines Ltd", "", ""));
    assert_eq!(company.birthday, Date::new(1990, 2, 3));
    assert_eq!(company.modified, 0);

    // What was asked: the list and the cards, and nothing for a picture, in the card or in a blob.
    let requests = sent(&connection);
    assert_eq!(requests.len(), 2);
    let body: Value = serde_json::from_slice(requests[0].body()).unwrap();
    assert_eq!(body["using"], json!([CAPABILITY_CORE, CAPABILITY_CONTACTS]));
    assert_eq!(body["methodCalls"], json!([["ContactCard/query", {"accountId": "A1", "position": 0, "limit": PAGE, "calculateTotal": true}, "c0"]]));
    assert_eq!((requests[0].url(), requests[0].max_response_bytes()), (API, QUERY_BYTES));
    let body: Value = serde_json::from_slice(requests[1].body()).unwrap();
    assert_eq!(body["methodCalls"], json!([["ContactCard/get", {"accountId": "A1", "ids": ["K1", "K2", "K3"]}, "c0"]]));
    assert_eq!((requests[1].url(), requests[1].max_response_bytes()), (API, GET_BYTES));
}

#[test]
fn the_list_is_read_page_by_page_and_the_cards_as_many_as_the_server_takes() {
    // The server says how many there are: the list ends there.
    let page = |ids: Value, total: u64| answer("ContactCard/query", json!({"ids": ids, "total": total}));
    let replies = vec![page(json!(["K1", "K2"]), 3), page(json!(["K3"]), 3), get(json!([plain("K1"), plain("K2")])), get(json!([plain("K3")]))];
    let connection = connected_with(&session_doc(2), replies);
    let got = run(&connection);
    assert!(download(&got).complete && got.errors.is_empty(), "{:?}", got.errors);
    assert_eq!(ids(&got), ["K1", "K2", "K3"]);
    let asked = calls(&connection);
    assert_eq!(
        asked.iter().map(|(method, _)| method.as_str()).collect::<Vec<_>>(),
        ["ContactCard/query", "ContactCard/query", "ContactCard/get", "ContactCard/get"]
    );
    assert_eq!((&asked[0].1["position"], &asked[1].1["position"]), (&json!(0), &json!(2)));
    assert_eq!((&asked[2].1["ids"], &asked[3].1["ids"]), (&json!(["K1", "K2"]), &json!(["K3"])));
    // It does not say: until a page is empty. An id listed twice is asked for once.
    let bare = |ids: Value| answer("ContactCard/query", json!({"ids": ids}));
    let replies = vec![bare(json!(["K1", "K2"])), bare(json!(["K2", "K3"])), bare(json!([])), get(json!([plain("K1"), plain("K2"), plain("K3")]))];
    let connection = connected(replies);
    let got = run(&connection);
    assert!(download(&got).complete && got.errors.is_empty());
    assert_eq!(ids(&got), ["K1", "K2", "K3"]);
    assert_eq!(calls(&connection)[2].1["position"], 4);
}

#[test]
fn a_list_that_does_not_end_is_not_followed_for_ever() {
    let same = || answer("ContactCard/query", json!({"ids": ["K1", "K2"]}));
    let connection = connected(vec![same(), same(), get(json!([plain("K1"), plain("K2")]))]);
    let got = run(&connection);
    assert_eq!(ids(&got), ["K1", "K2"]);
    assert!(!download(&got).complete);
    assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
    assert_eq!(sent(&connection).len(), 3);
}

#[test]
fn what_is_gone_since_the_list_is_no_trouble_and_what_is_left_out_is() {
    // Gone: said so.
    let gone = answer("ContactCard/get", json!({"list": [plain("K1")], "notFound": ["K2", "K9", 5]}));
    let connection = connected(vec![query(json!(["K1", "K2"])), gone]);
    let got = run(&connection);
    assert!(download(&got).complete && got.errors.is_empty());
    assert_eq!(ids(&got), ["K1"]);
    // Left out of both lists; a card nobody asked for and a card twice are not read.
    let short = get(json!([plain("K1"), plain("K7"), plain("K1"), "K2", {"id": 2}]));
    let connection = connected(vec![query(json!(["K1", "K2"])), short]);
    let got = run(&connection);
    assert_eq!(ids(&got), ["K1"]);
    assert!(!download(&got).complete);
    assert_eq!(got.errors, ["Couldn't read 1 contact from the server: the answer left them out."]);
    // Every card of a call gone: more likely an address book the server lost.
    let all = answer("ContactCard/get", json!({"list": [], "notFound": ["K1", "K2"]}));
    let connection = connected(vec![query(json!(["K1", "K2"])), all]);
    let got = run(&connection);
    assert!(!download(&got).complete && download(&got).contacts.is_empty() && got.errors.len() == 1);
}

#[test]
fn a_call_for_cards_that_fails_is_halved_and_the_rest_are_read() {
    let replies = vec![
        query(json!(["K1", "K2", "K3", "K4"])),
        get(json!([plain("K1"), plain("K2")])),
        // K3 and K4 together, then K3 by itself: refused. K4 by itself comes.
        refused("invalidArguments"),
        refused("invalidArguments"),
        get(json!([plain("K4")])),
    ];
    let connection = connected_with(&session_doc(2), replies);
    let got = run(&connection);
    assert_eq!(ids(&got), ["K1", "K2", "K4"]);
    assert!(!download(&got).complete, "one card was not read");
    assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
    assert!(got.errors[0].starts_with("Couldn't read the contacts from the server: "), "{}", got.errors[0]);
    let asked: Vec<Value> = calls(&connection).into_iter().skip(1).map(|(_, args)| args["ids"].clone()).collect();
    assert_eq!(asked, [json!(["K1", "K2"]), json!(["K3", "K4"]), json!(["K3"]), json!(["K4"])]);
    // An answer too large for its request is halved the same way.
    let padded = answer("ContactCard/get", json!({"list": [plain("K1"), plain("K2")], "pad": "x".repeat(GET_BYTES)}));
    let connection = connected(vec![query(json!(["K1", "K2"])), padded, get(json!([plain("K1")])), get(json!([plain("K2")]))]);
    let got = run(&connection);
    assert!(download(&got).complete && got.errors.is_empty(), "{:?}", got.errors);
    assert_eq!(ids(&got), ["K1", "K2"]);
}

#[test]
fn a_server_that_fails_part_of_the_way_leaves_what_was_read_and_says_so() {
    // The second page of the list is refused: the first is read.
    let page = answer("ContactCard/query", json!({"ids": ["K1"], "total": 2}));
    let connection = connected(vec![page, refused("invalidArguments"), get(json!([plain("K1")]))]);
    let got = run(&connection);
    assert_eq!(ids(&got), ["K1"]);
    assert!(!download(&got).complete && got.errors.len() == 1);
    // The server breaks while the cards are read: what came is kept, nothing more is asked.
    let page = query(json!(["K1", "K2", "K3"]));
    let detail = json!({"detail": "no book for ada@example.org at https://jmap.example.org/jmap/session"});
    let connection = connected_with(&session_doc(1), vec![page, get(json!([plain("K1")])), reply(500, &detail), get(json!([plain("K3")]))]);
    let got = run(&connection);
    assert_eq!(ids(&got), ["K1"]);
    assert!(!download(&got).complete);
    assert_eq!(got.errors.len(), 1);
    let line = &got.errors[0];
    assert!(line.starts_with("Couldn't read the contacts from the server: ") && line.contains("no book for"), "{line}");
    assert!(!line.contains("ada@") && !line.contains("jmap.example.org"), "{line}");
    assert_eq!(sent(&connection).len(), 3);
    // Nothing could be listed at all: no contacts, and why.
    for first in [refused("invalidArguments"), octets(500, b""), octets(200, b"not json"), answer("ContactCard/query", json!({"ids": "K1"}))] {
        let connection = connected(vec![first]);
        let got = run(&connection);
        assert!(got.contacts.is_none() && got.errors.len() == 1 && !got.auth_refused && got.busy_for.is_none(), "{:?}", got.errors);
        assert_eq!(sent(&connection).len(), 1);
    }
}

#[test]
fn a_refused_sign_in_and_a_busy_server_end_the_run_with_their_word() {
    for status in [401, 403] {
        let connection = connected(vec![octets(status, b"")]);
        let got = run(&connection);
        assert!(got.auth_refused && got.contacts.is_none() && got.errors.is_empty() && got.busy_for.is_none() && !got.stopped);
    }
    let busy = |status: u16, wait: Option<u32>| {
        let answer = HttpResponse::new(status, Vec::new());
        Ok(match wait {
            Some(seconds) => answer.with_field("Retry-After", &seconds.to_string()),
            None => answer,
        })
    };
    for (status, wait, expected) in [(429, Some(120), 120), (503, None, 30), (429, None, 30)] {
        let connection = connected(vec![busy(status, wait)]);
        let got = run(&connection);
        assert_eq!(got.busy_for, Some(expected));
        assert!(got.contacts.is_none() && !got.auth_refused && !got.stopped && got.errors.len() == 1);
        // And while the cards are read: what was listed is handed over, not complete.
        let connection = connected(vec![query(json!(["K1"])), busy(status, wait)]);
        let got = run(&connection);
        assert_eq!(got.busy_for, Some(expected));
        assert!(!download(&got).complete && download(&got).contacts.is_empty());
    }
    let connection = connected(vec![query(json!(["K1"])), octets(401, b"")]);
    let got = run(&connection);
    assert!(got.auth_refused && !download(&got).complete);
}

#[test]
fn a_sign_in_that_failed_is_said_as_a_run_would_say_it() {
    let refused = failed_contacts(&asked(), &Error::CredentialsRefused);
    assert!(refused.auth_refused && refused.errors.is_empty() && refused.contacts.is_none());
    assert_eq!((refused.account, refused.email.as_str(), refused.session.as_str()), (7, "ada@example.org", SESSION));
    let busy = failed_contacts(&asked(), &Error::Busy { retry_after: None });
    assert_eq!((busy.busy_for, busy.errors.len()), (Some(30), 1));
    let waited = failed_contacts(&asked(), &Error::Busy { retry_after: Some(RetryAfter::Delay(std::time::Duration::from_secs(9))) });
    assert_eq!(waited.busy_for, Some(9));
    let down = failed_contacts(&asked(), &Error::Transport(TransportError::Unreachable));
    assert!(!down.auth_refused && down.busy_for.is_none());
    assert_eq!(down.errors.len(), 1);
    assert!(down.errors[0].starts_with("Couldn't reach the server: "), "{}", down.errors[0]);
}

/// An exchange that sets a flag when it is asked for the request numbered `at` (the session's is the
/// first), and answers from a script.
struct Flipping {
    inner: FakeExchange,
    at: usize,
    seen: Cell<usize>,
    flag: Arc<AtomicBool>,
}

impl HttpExchange for Flipping {
    async fn exchange(&self, request: &HttpRequest) -> Reply {
        self.seen.set(self.seen.get() + 1);
        if self.seen.get() == self.at {
            self.flag.store(true, Ordering::Relaxed);
        }
        self.inner.exchange(request).await
    }
}

#[test]
fn cancel_is_looked_at_before_every_request() {
    // Before anything: the server is not asked.
    let connection = connected(vec![query(json!(["K1"]))]);
    let got = block_on(sync_contacts(&connection, &asked(), &AtomicBool::new(true)));
    assert!(got.stopped && got.contacts.is_none() && got.errors.is_empty());
    assert!(sent(&connection).is_empty());
    // While the list comes, and while the first cards come.
    for (at, read, requests) in [(2, 0, 2), (3, 1, 3)] {
        let flag = Arc::new(AtomicBool::new(false));
        let script = vec![reply(200, &session_doc(1)), query(json!(["K1", "K2"])), get(json!([plain("K1")])), get(json!([plain("K2")]))];
        let connection = connect(Flipping { inner: FakeExchange::new(script), at, seen: Cell::new(0), flag: flag.clone() });
        let got = block_on(sync_contacts(&connection, &asked(), &flag));
        assert!(got.stopped && got.errors.is_empty(), "{at}");
        assert_eq!((download(&got).contacts.len(), download(&got).complete), (read, false), "{at}");
        assert_eq!(connection.http().inner.sent().len(), requests, "{at}");
    }
    // An exchange called off because of it is the run stopped, not a server that is gone.
    let flag = Arc::new(AtomicBool::new(false));
    let script = vec![reply(200, &session_doc(500)), Err(TransportError::Aborted)];
    let connection = connect(Flipping { inner: FakeExchange::new(script), at: 2, seen: Cell::new(0), flag: flag.clone() });
    let got = block_on(sync_contacts(&connection, &asked(), &flag));
    assert!(got.stopped && got.errors.is_empty() && got.contacts.is_none());
}

// ------------------------------------------------------------------------------------------
// Pictures

#[test]
fn a_cards_picture_is_never_asked_for_wherever_it_lies() {
    let cards = json!([
        {"id": "K1", "media": {"m": {"kind": "photo", "uri": "https://tracker.example/ada.png"}}},
        {"id": "K2", "media": {"m": {"kind": "photo", "uri": "http://jmap.example.org/jmap/download/A1/B1/x"}}},
        {"id": "K3", "media": {"m": {"kind": "photo", "uri": "//tracker.example/x", "blobId": "../../x"}}},
        {"id": "K4", "media": {"m": {"kind": "photo", "uri": "https://tracker.example/b.png", "blobId": "B4"}}},
        {"id": "K5", "media": {"m": {"kind": "sound", "blobId": "B5"}}},
        {"id": "K6", "media": {"m": {"kind": "photo", "uri": data_uri("image/png", PNG)}}},
    ]);
    let connection = connected(vec![query(json!(["K1", "K2", "K3", "K4", "K5", "K6"])), get(cards), octets(200, PNG)]);
    let got = run(&connection);
    assert!(download(&got).complete && got.errors.is_empty());
    assert_eq!(ids(&got), ["K1", "K2", "K3", "K4", "K5", "K6"]);
    // The list and the cards: no blob is downloaded, and no address of a card is ever followed.
    let urls: Vec<String> = sent(&connection).iter().map(|request| request.url().to_string()).collect();
    assert_eq!(urls, [API, API]);
}

// ------------------------------------------------------------------------------------------
// Cards of every shape

#[test]
fn a_card_that_is_no_person_or_no_card_is_no_contact() {
    for kind in ["group", "device", "location", "application", "x-robot", ""] {
        assert!(card("K1", &json!({"kind": kind, "name": {"full": "X"}})).is_none(), "{kind}");
    }
    for not_a_card in [json!(null), json!("K1"), json!(7), json!([{"id": "K1"}])] {
        assert!(card("K1", &not_a_card).is_none());
    }
    for kind in [json!("individual"), json!("org"), json!(null), json!(7), json!(["group"])] {
        assert!(card("K1", &json!({"kind": kind, "name": {"full": "X"}})).is_some(), "{kind}");
    }
    assert!(card("K1", &json!({})).is_some());
}

#[test]
fn what_is_not_what_the_standard_says_is_as_good_as_absent() {
    let wrong = json!({
        "id": 7, "kind": 5, "updated": {"at": "now"},
        "name": ["Ada"], "nicknames": "Addie", "organizations": [{"name": "X"}], "titles": {"t": "Analyst", "u": {"name": 4}},
        "emails": {"e": "ada@example.org", "f": {"address": ["ada@example.org"]}, "g": {"address": "bo@example.org", "contexts": "work", "pref": "1", "label": 9}},
        "phones": {"p": {"number": 5550100, "features": ["mobile"]}, "q": {"number": "5550101", "features": {"mobile": "yes"}, "contexts": {"work": 1}}},
        "addresses": {"a": {"components": {"kind": "name", "value": "Mill Lane"}, "full": 12}, "b": {"components": [1, null, {"kind": "name"}, {"value": "x"}, {"kind": 1, "value": 2}]}},
        "links": {"l": {"uri": 5}, "m": "https://ada.example/", "n": {"uri": "javascript:alert(1)"}, "o": {"uri": "https://"}},
        "onlineServices": [],
        "anniversaries": {"a": {"kind": "birth", "date": "1815-12-10"}, "b": {"kind": "birth", "date": {"year": "1815", "month": 12, "day": 10}},
            "c": {"kind": "birth", "date": {"year": 1815, "month": 2, "day": 30}}, "d": {"kind": "birth", "date": {"utc": "soon"}},
            "e": {"kind": "birth", "date": {"year": 1e99, "month": -1, "day": 1.5}}},
        "notes": "A note.", "media": {"m": {"kind": "photo", "uri": 5, "blobId": 6}, "n": "photo"},
    });
    let read = card("K1", &wrong).unwrap();
    let expected = mailcraft_model::remote::RemoteContact {
        id: "K1".into(),
        emails: vec![Labeled::new("", "bo@example.org")],
        phones: vec![Labeled::new("", "5550101")],
        websites: vec!["https://".into()],
        ..Default::default()
    };
    assert_eq!(read, expected);
}

#[test]
fn what_a_card_holds_is_cut_to_size() {
    // (Long enough to be cut everywhere, and small enough that thousands of them are a test and no burden.)
    let long = "é".repeat(12_000);
    let short = "é".repeat(700);
    let many = |entry: &dyn Fn(usize) -> Value| Value::Object((0..3_000).map(|n| (format!("k{n:05}"), entry(n))).collect());
    let parts: Vec<Value> = (0..3_000).map(|n| json!({"kind": if n % 2 == 0 { "given" } else { "name" }, "value": format!("part{n}")})).collect();
    let contexts = many(&|_| json!(true));
    let huge = json!({
        "name": {"full": long, "components": parts},
        "nicknames": {"n": {"name": long}},
        "organizations": {"o": {"name": long, "units": (0..3_000).map(|_| json!({"name": short})).collect::<Vec<_>>()}},
        "titles": {"t": {"name": long}},
        "emails": many(&|n| json!({"address": format!("p{n}@example.org"), "label": short})),
        "phones": many(&|n| json!({"number": format!("{n}{short}"), "contexts": if n < 60 { contexts.clone() } else { json!({}) }})),
        "addresses": many(&|n| json!({"full": format!("{n} {short}"), "components": if n < 60 { json!(parts) } else { json!([]) }})),
        "links": many(&|n| json!({"uri": format!("https://site{n}.example/")})),
        "onlineServices": {"s": {"uri": format!("https://x.example/{long}")}},
        "notes": many(&|n| json!({"note": if n < 60 { &long } else { &short }})),
        "media": many(&|_| json!({"kind": "photo", "uri": format!("data:image/png;base64,{short}")})),
        "anniversaries": many(&|_| json!({"kind": "birth", "date": {"utc": short}})),
    });
    let read = card("K1", &huge).unwrap();
    for name in [&read.display_name, &read.first, &read.nickname, &read.company, &read.department, &read.title] {
        assert!(!name.is_empty() && name.len() <= 1_024, "{}", name.len());
    }
    assert_eq!((read.emails.len(), read.phones.len(), read.addresses.len(), read.websites.len()), (50, 50, 50, 50));
    assert!(read.emails.iter().chain(&read.phones).all(|entry| entry.label.len() <= 1_024 && entry.value.len() <= 1_024));
    assert_eq!(read.emails[0].value, "p0@example.org", "the first of them, in the order of their keys");
    assert!(read.addresses.iter().all(|address| address.street.len() <= 1_024 && address.label.len() <= 32));
    assert!(!read.notes.is_empty() && read.notes.len() <= 64 << 10, "{}", read.notes.len());
    assert!(read.birthday.is_none());
}

#[test]
fn a_hostile_list_and_hostile_cards_are_read_within_bounds() {
    // A list of what is no id, of an id twice, and of more than was asked for.
    let mut listed: Vec<Value> = vec![json!(5), json!(null), json!("a b"), json!("../x"), json!(["K0"]), json!("K1"), json!("K1")];
    listed.extend((0..2_000).map(|n| json!(format!("X{n}"))));
    let page = answer("ContactCard/query", json!({"ids": listed, "total": "many", "position": -4}));
    // The cards: one that is real, and the rest are not what was asked.
    let mut cards: Vec<Value> = vec![json!(null), json!({"id": "K1", "name": {"full": "Real"}}), json!({"id": "K1", "name": {"full": "Again"}})];
    cards.extend((0..3_000).map(|n| json!({"id": format!("Z{n}")})));
    let replies =
        vec![page, answer("ContactCard/query", json!({"ids": []})), answer("ContactCard/get", json!({"list": cards, "notFound": {"K1": true}}))];
    let connection = connected(replies);
    let got = run(&connection);
    assert_eq!(ids(&got), ["K1"]);
    assert_eq!(download(&got).contacts[0].display_name, "Real");
    assert!(!download(&got).complete, "the list named what is no id, and cards were left out");
    assert_eq!(got.errors.len(), 1);
    // A page is no more than was asked for, and a call no more than the server takes.
    let asked = calls(&connection);
    assert_eq!(asked[1].1["position"], PAGE);
    assert_eq!(asked[2].1["ids"].as_array().unwrap().len(), MAX_CHUNK);
    // Whatever the answers, no request goes anywhere but to the API of the session.
    assert!(sent(&connection).iter().all(|request| request.url() == API));
}

proptest! {
    #[test]
    fn any_value_as_a_card_is_read_or_passed_over_and_never_panics(value in any_json(), other in any_json(), text in ".{0,60}") {
        let _ = card("K1", &value);
        let names = ["kind", "name", "nicknames", "organizations", "titles", "emails", "phones", "addresses", "links", "onlineServices",
            "anniversaries", "notes", "media", "updated"];
        // The value as every property, and as every property of an entry of every property.
        let mut flat = Map::new();
        let mut deep = Map::new();
        for name in names {
            flat.insert(name.to_string(), value.clone());
            let entry = json!({
                "name": value, "full": other, "components": [{"kind": text, "value": value}, {"kind": "given", "value": other}, value],
                "address": value, "number": other, "contexts": value, "features": other, "label": value, "pref": other, "uri": value,
                "units": [value, {"name": other}], "kind": "birth", "date": {"year": value, "month": other, "day": value, "utc": text},
                "note": other, "blobId": value, "mediaType": text,
            });
            deep.insert(name.to_string(), json!({"a": entry, text.clone(): value, "c": other}));
        }
        flat.remove("kind");
        deep.remove("kind");
        for made in [Value::Object(flat), Value::Object(deep.clone())] {
            let contact = card("K1", &made).expect("a card of a kind that is read");
            prop_assert!(contact.emails.len() <= 50 && contact.phones.len() <= 50 && contact.addresses.len() <= 50 && contact.websites.len() <= 50);
            prop_assert!(contact.websites.iter().all(|site| site.to_ascii_lowercase().starts_with("http")));
        }
        // As a photo's address and as an anniversary's day, any text.
        let made = json!({"updated": text, "media": {"m": {"kind": "photo", "uri": format!("data:image/png;base64,{text}")}, "n": {"kind": "photo", "uri": text}},
            "anniversaries": {"b": {"kind": "birth", "date": {"utc": text}}}});
        let _ = card("K1", &made);
    }

    #[test]
    fn any_answers_are_read_or_refused_and_never_panic(first in any_json(), second in any_json(), blob in proptest::collection::vec(any::<u8>(), 0..64)) {
        let real = json!({"ids": ["K1", "K2"], "total": 2});
        let cards = json!({"list": [{"id": "K1", "media": {"m": {"kind": "photo", "blobId": "B1"}}}, second.clone()], "notFound": first.clone()});
        for (listing, reading) in [(first.clone(), second.clone()), (real.clone(), second.clone()), (real, cards)] {
            let replies = vec![answer("ContactCard/query", listing), answer("ContactCard/get", reading), octets(200, &blob), octets(200, &blob)];
            let connection = connected(replies);
            let got = run(&connection);
            if let Some(down) = &got.contacts {
                prop_assert!(down.contacts.iter().all(|contact| ["K1", "K2"].contains(&contact.id.as_str())) || !down.complete || down.contacts.len() <= 500);
                prop_assert!(!down.complete || got.errors.is_empty());
            }
            prop_assert!(sent(&connection).iter().all(|request| request.url() == API));
        }
    }
}
