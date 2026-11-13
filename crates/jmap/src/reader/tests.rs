// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reading of one kind of data, whatever the kind: a made-up one (`Thing/query`, `Thing/get`) against a
//! scripted server, for the paging of the list, the halving of a call that fails, and what an answer is read
//! for.

use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use super::*;
use crate::test_support::{FakeExchange, made_secret};
use crate::{CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL, Credentials, HttpRequest, HttpResponse, Method, block_on};

const SESSION: &str = "https://jmap.example.org/jmap/session";
/// The most an answer with things may be here: small, so that one too large is cheap to make.
const THING_BYTES: usize = 4 << 10;

type Reply = std::result::Result<HttpResponse, TransportError>;

fn reply(status: u16, body: &Value) -> Reply {
    Ok(HttpResponse::new(status, serde_json::to_vec(body).unwrap()).with_field("Content-Type", "application/json"))
}

/// What the one call of a request answers.
fn answer(method: &str, arguments: Value) -> Reply {
    reply(200, &json!({"methodResponses": [[method, arguments, "c0"]], "sessionState": "1"}))
}

/// A page of the list, the server not saying how many there are.
fn page(ids: Value) -> Reply {
    answer("Thing/query", json!({"ids": ids}))
}

fn things(list: Value) -> Reply {
    answer("Thing/get", json!({"list": list, "notFound": []}))
}

fn refused() -> Reply {
    answer("error", json!({"type": "invalidArguments"}))
}

fn thing(id: &str) -> Value {
    json!({"id": id, "word": format!("thing {id}")})
}

/// A connection over a script: a session whose server takes `max_get` objects in a `get`, then `replies`.
fn connected(max_get: u64, replies: Vec<Reply>) -> Connection<FakeExchange> {
    let doc = json!({
        "capabilities": {CAPABILITY_CORE: {"maxCallsInRequest": 16, "maxObjectsInGet": max_get}, CAPABILITY_MAIL: {}, CAPABILITY_CONTACTS: {}},
        "accounts": {"A1": {"name": "ada@example.org", "isPersonal": true, "accountCapabilities": {CAPABILITY_MAIL: {}, CAPABILITY_CONTACTS: {}}}},
        "primaryAccounts": {CAPABILITY_MAIL: "A1", CAPABILITY_CONTACTS: "A1"},
        "apiUrl": "/jmap/api",
        "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
        "uploadUrl": "/jmap/upload/{accountId}/",
        "state": "1",
    });
    let mut script = vec![reply(200, &doc)];
    script.extend(replies);
    block_on(Connection::connect(FakeExchange::new(script), SESSION, Credentials::bearer(&made_secret("tk-3f9a7c")).unwrap())).unwrap()
}

/// Things, `most` of them at the most, the list asked `listing` besides and a `get` asked `reading`.
fn kind_of(most: usize, listing: Value, reading: Value) -> Kind {
    let map = |value: Value| value.as_object().cloned().unwrap_or_default();
    Kind {
        query: "Thing/query",
        get: "Thing/get",
        listing: map(listing),
        reading: map(reading),
        most,
        get_bytes: THING_BYTES,
        no_list: "the answer has no list of things",
        repeats: "the list of things repeats itself",
        too_long: "the list of things has too many pages",
    }
}

fn kind() -> Kind {
    kind_of(1_000, json!({}), json!({}))
}

/// A thing is its word; one without a word is none, and one whose word is no text can not be read.
fn made(_: &str, entry: &Value) -> Made<String> {
    match entry.get("word") {
        Some(Value::String(word)) => Made::Item(word.clone()),
        Some(_) => Made::Unreadable,
        None => Made::Other,
    }
}

/// What a run of `kind` over `connection` comes to: how it ended, and what it read.
fn read(connection: &Connection<FakeExchange>, kind: &Kind) -> (Result<(), Stop>, Outcome<String>) {
    let cancel = AtomicBool::new(false);
    let calls = Calls::new(connection, &cancel, "ada@example.org", SESSION, Id::new("A1").unwrap(), CAPABILITY_CONTACTS);
    let mut out = Outcome::default();
    let ended = block_on(calls.read_all(kind, &mut out, made));
    (ended, out)
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

/// The ids each `get` asked for, in the order they were sent.
fn gets(connection: &Connection<FakeExchange>) -> Vec<Value> {
    calls(connection).into_iter().filter(|(method, _)| method == "Thing/get").map(|(_, args)| args["ids"].clone()).collect()
}

fn words(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| format!("thing {id}")).collect()
}

#[test]
fn each_call_asks_what_the_kind_says_and_holds_its_answer_to_size() {
    let listing = json!({"filter": {"inBox": "B1"}, "expand": true});
    let reading = json!({"properties": ["id", "word"]});
    let connection = connected(500, vec![page(json!(["T1", "T2"])), page(json!([])), things(json!([thing("T1"), thing("T2")]))]);
    let (ended, out) = read(&connection, &kind_of(1_000, listing, reading));
    assert!(ended.is_ok() && out.clean());
    assert_eq!(out.items, words(&["T1", "T2"]));
    let asked = calls(&connection);
    let first = json!({"accountId": "A1", "filter": {"inBox": "B1"}, "expand": true, "position": 0, "limit": PAGE, "calculateTotal": true});
    assert_eq!(asked[0], ("Thing/query".to_string(), first));
    assert_eq!(asked[1].1["position"], 2);
    assert_eq!(asked[2], ("Thing/get".to_string(), json!({"accountId": "A1", "ids": ["T1", "T2"], "properties": ["id", "word"]})));
    let limits: Vec<usize> = sent(&connection).iter().map(HttpRequest::max_response_bytes).collect();
    assert_eq!(limits, [QUERY_BYTES, QUERY_BYTES, THING_BYTES]);
    // What the kind asks besides never takes the place of the account, the page or the ids.
    let odd = json!({"accountId": "B9", "position": 77, "limit": 1, "calculateTotal": false, "ids": ["X"]});
    let connection = connected(500, vec![page(json!(["T1"])), page(json!([])), things(json!([thing("T1")]))]);
    let (ended, out) = read(&connection, &kind_of(1_000, odd.clone(), odd));
    assert!(ended.is_ok() && out.clean());
    let asked = calls(&connection);
    assert_eq!(asked[0].1, json!({"accountId": "A1", "position": 0, "limit": PAGE, "calculateTotal": true, "ids": ["X"]}));
    assert_eq!((&asked[2].1["accountId"], &asked[2].1["ids"]), (&json!("A1"), &json!(["T1"])));
}

#[test]
fn the_list_is_read_page_by_page_to_its_end() {
    // The server says how many there are: the list ends there, and the items go as many as it takes.
    let counted = |ids: Value, total: u64| answer("Thing/query", json!({"ids": ids, "total": total}));
    let replies =
        vec![counted(json!(["T1", "T2"]), 3), counted(json!(["T3"]), 3), things(json!([thing("T1"), thing("T2")])), things(json!([thing("T3")]))];
    let connection = connected(2, replies);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && out.clean() && out.listed);
    assert_eq!(out.items, words(&["T1", "T2", "T3"]));
    let asked = calls(&connection);
    assert_eq!((&asked[0].1["position"], &asked[1].1["position"]), (&json!(0), &json!(2)));
    assert_eq!(gets(&connection), [json!(["T1", "T2"]), json!(["T3"])]);
    // It does not say: until a page is empty. An id listed twice is asked for once.
    let replies = vec![page(json!(["T1", "T2"])), page(json!(["T2", "T3"])), page(json!([])), things(json!([thing("T1"), thing("T2"), thing("T3")]))];
    let connection = connected(500, replies);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && out.clean());
    assert_eq!(gets(&connection), [json!(["T1", "T2", "T3"])]);
    assert_eq!(calls(&connection)[2].1["position"], 4);
    // A list of nothing is read, and whole.
    let connection = connected(500, vec![page(json!([]))]);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && out.clean() && out.items.is_empty());
    assert_eq!(sent(&connection).len(), 1);
    // A server that takes none in a `get`, or says so, is asked for one at a time.
    let connection = connected(0, vec![page(json!(["T1", "T2"])), page(json!([])), things(json!([thing("T1")])), things(json!([thing("T2")]))]);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && out.clean());
    assert_eq!(gets(&connection), [json!(["T1"]), json!(["T2"])]);
}

#[test]
fn a_page_that_brings_nothing_new_ends_the_list() {
    let connection = connected(500, vec![page(json!(["T1", "T2"])), page(json!(["T2", "T1"])), things(json!([thing("T1"), thing("T2")]))]);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok());
    assert_eq!(out.items, words(&["T1", "T2"]));
    assert!(out.short && !out.clean() && !out.capped);
    assert!(out.why.as_deref().is_some_and(|why| why.contains("the list of things repeats itself")), "{:?}", out.why);
    assert_eq!(sent(&connection).len(), 3);
}

#[test]
fn a_list_is_read_to_so_many_pages_and_so_many_ids_and_no_further() {
    // A list that goes on for ever, a new id to a page: as many pages as are read, and then no more.
    let mut replies: Vec<Reply> = (0..MAX_PAGES).map(|n| page(json!([format!("T{n}")]))).collect();
    replies.extend((0..MAX_PAGES / MAX_CHUNK).map(|_| things(json!([]))));
    let connection = connected(500, replies);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && out.short && !out.clean());
    assert!(out.why.as_deref().is_some_and(|why| why.contains("the list of things has too many pages")), "{:?}", out.why);
    let asked = calls(&connection);
    assert_eq!(asked.iter().filter(|(method, _)| method == "Thing/query").count(), MAX_PAGES);
    assert_eq!(gets(&connection).iter().map(|ids| ids.as_array().unwrap().len()).sum::<usize>(), MAX_PAGES);
    // More ids than the kind reads: the first of them, and it is said that there are more.
    let connection = connected(500, vec![page(json!(["T1", "T2", "T3", "T4", "T5"])), things(json!([thing("T1"), thing("T2"), thing("T3")]))]);
    let (ended, out) = read(&connection, &kind_of(3, json!({}), json!({})));
    assert!(ended.is_ok() && out.capped && !out.clean() && !out.short && out.why.is_none());
    assert_eq!(out.items, words(&["T1", "T2", "T3"]));
    assert_eq!(sent(&connection).len(), 2);
    // A page is no more than was asked for, and a call for items no more than is asked at a time.
    let long: Vec<Value> = (0..PAGE + 300).map(|n| json!(format!("L{n}"))).collect();
    let connection = connected(100_000, vec![page(json!(long)), page(json!([])), things(json!([])), things(json!([])), things(json!([]))]);
    let (ended, out) = read(&connection, &kind());
    assert_eq!(calls(&connection)[1].1["position"], PAGE);
    let asked = gets(&connection);
    assert_eq!(asked[0].as_array().unwrap().len(), MAX_CHUNK);
    assert_eq!(asked[0][0], "L0");
    assert!(ended.is_ok() && !out.clean());
    assert_eq!(out.unread, PAGE);
}

#[test]
fn only_an_id_of_the_protocol_is_asked_for() {
    let listed = json!([5, null, "a b", "../x", ["T0"], {"id": "T0"}, "", "T1"]);
    let connection = connected(500, vec![page(listed), page(json!([])), things(json!([thing("T1")]))]);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok());
    assert_eq!(out.items, words(&["T1"]));
    assert!(out.short && !out.clean() && out.why.is_none() && out.unread == 0);
    assert_eq!(gets(&connection), [json!(["T1"])]);
}

#[test]
fn a_list_that_fails_is_the_callers_before_its_first_page_and_told_after_it() {
    for first in [refused(), answer("Thing/query", json!({"ids": "T1"})), answer("Thing/query", json!({}))] {
        let connection = connected(500, vec![first]);
        let (ended, out) = read(&connection, &kind());
        assert!(matches!(ended, Err(Stop::Failed(_))));
        assert!(!out.listed && out.items.is_empty());
        assert_eq!(sent(&connection).len(), 1);
    }
    // After a page: what was listed is read all the same.
    let counted = answer("Thing/query", json!({"ids": ["T1"], "total": 2}));
    let connection = connected(500, vec![counted, refused(), things(json!([thing("T1")]))]);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && out.listed && out.short && out.why.is_some() && !out.clean());
    assert_eq!(out.items, words(&["T1"]));
    // What ends a run ends it wherever it happens, and what was read until then stays.
    let connection = connected(1, vec![page(json!(["T1", "T2"])), page(json!([])), things(json!([thing("T1")])), reply(401, &json!({}))]);
    let (ended, out) = read(&connection, &kind());
    assert!(matches!(ended, Err(Stop::Auth)));
    assert_eq!(out.items, words(&["T1"]));
    let connection = connected(500, vec![page(json!(["T1"])), reply(500, &json!({}))]);
    let (ended, out) = read(&connection, &kind());
    assert!(matches!(ended, Err(Stop::Down(_))) && out.listed);
}

#[test]
fn a_call_that_fails_is_halved_until_the_item_that_breaks_it_is_found() {
    let replies = vec![
        page(json!(["T1", "T2", "T3", "T4"])),
        page(json!([])),
        // All four, then T1 and T2 together, then T1 by itself: refused. T2 by itself comes, and the others.
        refused(),
        refused(),
        refused(),
        things(json!([thing("T2")])),
        things(json!([thing("T3"), thing("T4")])),
    ];
    let connection = connected(500, replies);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok());
    assert_eq!(out.items, words(&["T2", "T3", "T4"]));
    assert_eq!(out.unread, 1);
    assert!(out.why.is_some() && !out.short && !out.clean());
    assert_eq!(gets(&connection), [json!(["T1", "T2", "T3", "T4"]), json!(["T1", "T2"]), json!(["T1"]), json!(["T2"]), json!(["T3", "T4"])]);
    // An answer too large for its request is halved the same way; one without its list too.
    let padded = answer("Thing/get", json!({"list": [thing("T1"), thing("T2")], "pad": "x".repeat(THING_BYTES)}));
    let bare = answer("Thing/get", json!({"notFound": []}));
    let replies = vec![page(json!(["T1", "T2"])), page(json!([])), padded, bare, things(json!([thing("T2")]))];
    let connection = connected(500, replies);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok());
    assert_eq!(out.items, words(&["T2"]));
    assert_eq!(gets(&connection), [json!(["T1", "T2"]), json!(["T1"]), json!(["T2"])]);
    assert_eq!(out.unread, 1, "T1 by itself came without a list");
}

#[test]
fn three_calls_that_fail_one_after_the_other_give_up_the_rest() {
    // Every call refused: 200 are halved down to one, and after the third that can not be halved it ends.
    let ids: Vec<String> = (0..MAX_CHUNK + 50).map(|n| format!("T{n}")).collect();
    let mut replies = vec![page(json!(ids)), page(json!([]))];
    replies.extend((0..40).map(|_| refused()));
    let connection = connected(500, replies);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && out.items.is_empty());
    assert!(out.short && out.why.is_some() && !out.clean());
    assert_eq!(out.unread, MAX_FAILURES);
    let sizes: Vec<usize> = gets(&connection).iter().map(|ids| ids.as_array().unwrap().len()).collect();
    assert_eq!(sizes, [200, 100, 50, 25, 12, 6, 3, 1, 2, 1, 1]);
    // A call that comes in between starts the count again.
    let replies =
        vec![page(json!(["T1", "T2", "T3", "T4", "T5"])), page(json!([])), refused(), refused(), things(json!([thing("T3")])), refused(), refused()];
    let connection = connected(1, replies);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && !out.short);
    assert_eq!((out.items, out.unread), (words(&["T3"]), 4));
}

#[test]
fn a_call_is_halved_so_many_times_in_a_list_and_no_more() {
    // Pairs of which the first breaks its call: each takes one halving, until there are none left.
    let pairs = MAX_SPLITS + 2;
    let ids: Vec<String> = (0..pairs).flat_map(|n| [format!("B{n}"), format!("G{n}")]).collect();
    let mut replies = vec![page(json!(ids)), page(json!([]))];
    for n in 0..MAX_SPLITS {
        replies.extend([refused(), refused(), things(json!([thing(&format!("G{n}"))]))]);
    }
    replies.extend([refused(), refused()]);
    let connection = connected(2, replies);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok());
    assert_eq!(out.items.len(), MAX_SPLITS);
    // One of each pair that was halved, and both of the two that were not.
    assert_eq!(out.unread, MAX_SPLITS + 4);
    assert!(!out.short, "two in a row, not three");
    assert_eq!(gets(&connection).len(), MAX_SPLITS * 3 + 2);
}

#[test]
fn only_what_was_asked_for_is_read_and_each_once() {
    // Gone: said so, and no trouble. What `notFound` names besides is nobody's.
    let gone = answer("Thing/get", json!({"list": [thing("T1")], "notFound": ["T2", "T9", 5]}));
    let connection = connected(500, vec![page(json!(["T1", "T2"])), page(json!([])), gone]);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && out.clean());
    assert_eq!(out.items, words(&["T1"]));
    // Left out of both lists; a thing nobody asked for and a thing twice are not read.
    let short = things(json!([thing("T1"), thing("T7"), thing("T1"), "T2", {"id": 2}, null]));
    let connection = connected(500, vec![page(json!(["T1", "T2"])), page(json!([])), short]);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && !out.clean() && out.why.is_none());
    assert_eq!((out.items, out.unread), (words(&["T1"]), 1));
    // No more of an answer is looked at than was asked for.
    let mut long: Vec<Value> = (0..50).map(|n| thing(&format!("Z{n}"))).collect();
    long.push(thing("T1"));
    let connection = connected(500, vec![page(json!(["T1"])), page(json!([])), things(json!(long))]);
    let (_, out) = read(&connection, &kind());
    assert_eq!((out.items.len(), out.unread), (0, 1));
    // Everything of a call gone: more likely lost by the server than removed by its owner.
    let all = answer("Thing/get", json!({"list": [], "notFound": ["T1", "T2"]}));
    let connection = connected(500, vec![page(json!(["T1", "T2"])), page(json!([])), all]);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && out.items.is_empty() && !out.clean());
    assert_eq!(out.unread, 2);
    // The one thing of a call gone is gone.
    let one = answer("Thing/get", json!({"list": [], "notFound": ["T1"]}));
    let connection = connected(500, vec![page(json!(["T1"])), page(json!([])), one]);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok() && out.items.is_empty() && out.clean());
}

#[test]
fn what_is_no_item_is_answered_for_and_what_can_not_be_read_is_not() {
    let list = json!([{"id": "T1"}, {"id": "T2", "word": 7}, thing("T3")]);
    let connection = connected(500, vec![page(json!(["T1", "T2", "T3"])), page(json!([])), things(list)]);
    let (ended, out) = read(&connection, &kind());
    assert!(ended.is_ok());
    assert_eq!(out.items, words(&["T3"]));
    assert_eq!(out.unread, 1, "the one that could not be read");
    assert!(!out.clean() && out.why.is_none() && !out.short);
}

#[test]
fn a_run_that_is_called_off_asks_nothing() {
    let connection = connected(500, vec![page(json!(["T1"]))]);
    let cancel = AtomicBool::new(true);
    let calls = Calls::new(&connection, &cancel, "ada@example.org", SESSION, Id::new("A1").unwrap(), CAPABILITY_CONTACTS);
    let mut out: Outcome<String> = Outcome::default();
    let ended = block_on(calls.read_all(&kind(), &mut out, made));
    assert!(matches!(ended, Err(Stop::Cancelled)));
    assert!(!out.listed && sent(&connection).is_empty());
}
