// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in held to the standard, and a client held to the stand-in.

use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::connection::tests::any_json;
use crate::test_support::made_secret;
use crate::{ApiRequest, CallFailure, Connection, Credentials, Error, block_on};

fn mail(subject: &str) -> Vec<u8> {
    format!("From: Zoe Park <zoe@example.org>\r\nTo: {USER}\r\nSubject: {subject}\r\nMessage-ID: <{subject}@example.org>\r\n\r\nHello about {subject}.\r\n")
        .into_bytes()
}

/// A stand-in with an Inbox of three emails (two of them unread, the third flagged) and an empty Sent.
fn filled() -> (StandIn, Ids) {
    let stand_in = StandIn::new();
    let ids = stand_in.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        let sent = d.mailbox("Sent", None, Some("sent"));
        let first = d.deliver(&inbox, &mail("first"), &["$seen"], "2026-10-14T09:00:00Z");
        let second = d.deliver(&inbox, &mail("second"), &[], "2026-10-15T06:00:00Z");
        let third = d.deliver(&inbox, &mail("third"), &["$flagged"], "2026-10-15T07:30:00Z");
        Ids { inbox, sent, first, second, third }
    });
    (stand_in, ids)
}

struct Ids {
    inbox: String,
    sent: String,
    first: String,
    second: String,
    third: String,
}

fn connect(stand_in: &StandIn) -> Connection<StandIn> {
    connect_with(stand_in, Credentials::bearer(TOKEN).unwrap()).unwrap()
}

fn connect_with(stand_in: &StandIn, credentials: Credentials) -> Result<Connection<StandIn>, Error> {
    block_on(Connection::connect(stand_in.clone(), SESSION, credentials))
}

/// The answers to `calls` (each a method and its arguments), sent in one request of mail's, in order.
fn run_calls(connection: &Connection<StandIn>, calls: Vec<(&str, Value)>) -> Result<Vec<Result<Value, CallFailure>>, Error> {
    let mut request = ApiRequest::new();
    request.using(MAIL).unwrap();
    let ids: Vec<_> =
        calls.into_iter().map(|(method, args)| (request.add_call(method, args.as_object().cloned().unwrap()).unwrap(), method)).collect();
    let mut answers = block_on(connection.call(&request, connection.client().redactor()))?;
    Ok(ids.iter().map(|(call, method)| answers.take_named(call, method).map(Value::Object)).collect())
}

/// A reference to what the call `call` of the same request answered (RFC 8620 §3.7).
fn reference_to(call: usize, name: &str, path: &str) -> Value {
    json!({"resultOf": format!("c{call}"), "name": name, "path": path})
}

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

const API: &str = "https://jmap.standin.example/jmap/api";
const SESSION_URL: &str = "https://jmap.standin.example/jmap/session";

/// A GET of `url` with these header fields, and what it was answered.
fn get(stand_in: &StandIn, url: &str, fields: &[(&str, &str)]) -> Result<HttpResponse, TransportError> {
    stand_in.answer("GET", url, fields, b"")
}

/// A request to the API with these header fields after its `Content-Type: application/json`, and what it was
/// answered.
fn post(stand_in: &StandIn, headers: &[(&str, &str)], body: &[u8]) -> HttpResponse {
    let mut fields = vec![("Content-Type", "application/json")];
    fields.extend_from_slice(headers);
    stand_in.answer("POST", API, &fields, body).unwrap()
}

/// Method calls as the API has them, and the answer as JSON.
fn raw(stand_in: &StandIn, using: Value, calls: Value) -> (u16, Value) {
    let body = serde_json::to_vec(&json!({"using": using, "methodCalls": calls})).unwrap();
    let resp = post(stand_in, &[("Authorization", &bearer())], &body);
    (resp.status(), serde_json::from_slice(resp.body()).unwrap_or(Value::Null))
}

/// What a method call came to: its result, or the `type` of its error.
type Outcome = std::result::Result<Value, String>;

/// The answer to the one call of a request.
fn one(stand_in: &StandIn, call: Value) -> Outcome {
    let (status, body) = raw(stand_in, json!([CORE, MAIL]), json!([call]));
    assert_eq!(status, 200, "{body}");
    let answer = body["methodResponses"][0].clone();
    if answer[0] == "error" { Err(answer[1]["type"].as_str().unwrap().to_string()) } else { Ok(answer[1].clone()) }
}

#[test]
fn a_client_connects_with_either_kind_of_sign_in() {
    let (stand_in, _) = filled();
    for credentials in [Credentials::bearer(TOKEN).unwrap(), Credentials::basic(USER, &made_secret(PASSWORD)).unwrap()] {
        let connection = connect_with(&stand_in, credentials).unwrap();
        let client = connection.client();
        assert_eq!(client.account_id().as_str(), ACCOUNT);
        assert_eq!((client.limits().max_objects_in_get(), client.limits().max_calls_in_request()), (500, 16));
        assert!(client.has_capability(MAIL) && client.has_capability(CORE) && !client.has_capability("urn:ietf:params:jmap:submission"));
        assert_eq!(
            client.download_address(&Id::new("B1").unwrap(), "message/rfc822", "m.eml").unwrap().as_str(),
            "https://jmap.standin.example/jmap/download/A1/B1/m.eml?type=message%2Frfc822"
        );
        assert_eq!(client.upload_address().unwrap().as_str(), "https://jmap.standin.example/jmap/upload/A1/");
    }
    let seen: Vec<(String, String, u16)> = stand_in.seen().into_iter().map(|s| (s.method, s.path, s.status)).collect();
    assert_eq!(seen, vec![("GET".to_string(), "/jmap/session".to_string(), 200); 2]);
    // The limits are the ones it was told to state.
    stand_in.with(|d| (d.max_calls, d.max_get) = (3, 100));
    let connection = connect(&stand_in);
    assert_eq!((connection.client().limits().max_objects_in_get(), connection.client().limits().max_calls_in_request()), (100, 3));
    assert!(format!("{stand_in:?}").contains("emails: 3"));
}

#[test]
fn mailboxes_come_with_their_counts() {
    let (stand_in, ids) = filled();
    stand_in.with(|d| d.deliver(&ids.sent, &mail("draft"), &["$draft"], "2026-10-16T00:00:00Z"));
    let client = connect(&stand_in);
    let get = |args: Value| run_calls(&client, vec![("Mailbox/get", args)]).unwrap().remove(0);
    let all = get(json!({"accountId": ACCOUNT, "ids": null})).unwrap();
    assert_eq!(
        all["list"],
        json!([
            {"id": ids.inbox, "name": "Inbox", "parentId": null, "role": "inbox", "sortOrder": 0, "totalEmails": 3, "unreadEmails": 2},
            {"id": ids.sent, "name": "Sent", "parentId": null, "role": "sent", "sortOrder": 0, "totalEmails": 1, "unreadEmails": 0},
        ])
    );
    assert_eq!(all["notFound"], json!([]));
    let some = get(json!({"accountId": ACCOUNT, "ids": [ids.sent, "M999", "nope"], "properties": ["totalEmails"]})).unwrap();
    assert_eq!(some["list"], json!([{"id": ids.sent, "totalEmails": 1}]));
    assert_eq!(some["notFound"], json!(["M999", "nope"]));
    // Another client reads a message: the count and the state follow.
    let before = all["state"].clone();
    assert!(stand_in.with(|d| d.set_keyword(&ids.second, "$seen", true)));
    assert!(!stand_in.with(|d| d.set_keyword("E999", "$seen", true)));
    let after = get(json!({"accountId": ACCOUNT, "ids": [ids.inbox], "properties": ["unreadEmails"]})).unwrap();
    assert_eq!(after["list"], json!([{"id": ids.inbox, "unreadEmails": 1}]));
    assert_ne!(after["state"], before);
    assert_eq!(stand_in.with(|d| d.mailbox_named("Sent").map(str::to_string)), Some(ids.sent));
}

#[test]
fn the_newest_are_listed_and_read_in_one_request_by_a_reference() {
    let (stand_in, ids) = filled();
    let client = connect(&stand_in);
    let calls = vec![
        (
            "Email/query",
            json!({"accountId": ACCOUNT, "filter": {"inMailbox": ids.inbox}, "sort": [{"property": "receivedAt", "isAscending": false}], "limit": 2, "calculateTotal": true}),
        ),
        (
            "Email/get",
            json!({"accountId": ACCOUNT, "#ids": reference_to(0, "Email/query", "/ids"), "properties": ["id", "keywords", "receivedAt", "size", "preview", "headers"]}),
        ),
    ];
    let answers = run_calls(&client, calls).unwrap();
    let query = answers[0].as_ref().unwrap();
    assert_eq!(query["ids"], json!([ids.third, ids.second]));
    assert_eq!((&query["total"], &query["position"], &query["canCalculateChanges"]), (&json!(3), &json!(0), &json!(false)));
    let list = &answers[1].as_ref().unwrap()["list"];
    assert_eq!(list[0]["id"], json!(ids.third));
    assert_eq!(list[0]["keywords"], json!({"$flagged": true}));
    assert_eq!(list[0]["receivedAt"], "2026-10-15T07:30:00Z");
    assert_eq!(list[0]["size"], mail("third").len());
    assert_eq!(list[0]["preview"], "Hello about third.");
    assert_eq!(list[0]["headers"][0], json!({"name": "From", "value": " Zoe Park <zoe@example.org>"}));
    let names: Vec<&str> = list[0]["headers"].as_array().unwrap().iter().map(|h| h["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["From", "To", "Subject", "Message-ID"]);
    assert_eq!(list[0].as_object().unwrap().len(), 6, "only the properties asked for, and the id");
    assert_eq!(list[1]["keywords"], json!({}));
    // That was one request for both calls, after the one for the session.
    let requests = stand_in.seen();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests.last().map(|r| (r.status, r.calls.clone())), Some((200, vec!["Email/query".to_string(), "Email/get".to_string()])));
    assert_eq!(stand_in.calls(), ["Email/query", "Email/get"]);
}

#[test]
fn a_query_pages_sorts_and_counts_as_asked() {
    let (stand_in, ids) = filled();
    // Two received together: in the order they were delivered.
    let fourth = stand_in.with(|d| d.deliver(&ids.inbox, &mail("fourth"), &[], "2026-10-15T07:30:00Z"));
    let query = |extra: Value| {
        let mut args = json!({"accountId": ACCOUNT, "filter": {"inMailbox": ids.inbox}});
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        one(&stand_in, json!(["Email/query", args, "c0"]))
    };
    let ids_of = |result: Outcome| result.unwrap()["ids"].clone();
    assert_eq!(ids_of(query(json!({}))), json!([ids.first, ids.second, ids.third, fourth]), "oldest first unless told");
    assert_eq!(ids_of(query(json!({"sort": [{"property": "receivedAt", "isAscending": true}]}))), json!([ids.first, ids.second, ids.third, fourth]));
    assert_eq!(ids_of(query(json!({"sort": [{"property": "receivedAt", "isAscending": false}]}))), json!([fourth, ids.third, ids.second, ids.first]));
    assert_eq!(ids_of(query(json!({"position": 1, "limit": 2}))), json!([ids.second, ids.third]));
    assert_eq!(ids_of(query(json!({"position": 9}))), json!([]));
    // A negative position counts from the end, and stops at the start; the answer says where it began.
    assert_eq!(query(json!({"position": -2})).unwrap()["ids"], json!([ids.third, fourth]));
    assert_eq!(query(json!({"position": -2})).unwrap()["position"], 2);
    assert_eq!(query(json!({"position": -1, "limit": 5})).unwrap()["ids"], json!([fourth]));
    let from_far_before = query(json!({"position": -100})).unwrap();
    assert_eq!((from_far_before["ids"].as_array().map(Vec::len), from_far_before["position"].clone()), (Some(4), json!(0)));
    assert_eq!(query(json!({"position": i64::MIN})).unwrap()["position"], 0);
    assert_eq!(query(json!({"collapseThreads": false})).unwrap()["ids"].as_array().map(Vec::len), Some(4));
    assert!(query(json!({"collapseThreads": null})).is_ok());
    // It says the limit it applied when that is its own, and only then (RFC 8620 §5.5).
    assert_eq!(query(json!({})).unwrap()["limit"], 500);
    assert_eq!(query(json!({"limit": 100_000})).unwrap()["limit"], 500);
    assert_eq!(query(json!({"limit": null})).unwrap()["limit"], 500);
    for asked in [0, 1, 10, 500] {
        assert!(query(json!({ "limit": asked })).unwrap().get("limit").is_none(), "limit {asked}");
    }
    assert_eq!(ids_of(query(json!({"limit": 0}))), json!([]));
    assert!(query(json!({})).unwrap().get("total").is_none(), "the total only when asked");
    assert_eq!(query(json!({"calculateTotal": true, "limit": 1})).unwrap()["total"], 4);
    // No more than 500 at a time.
    stand_in.with(|d| {
        for n in 0..600 {
            d.deliver(&ids.sent, b"Subject: x\r\n\r\n", &[], &format!("2026-01-01T00:{:02}:{:02}Z", n / 60, n % 60));
        }
    });
    let args = json!({"accountId": ACCOUNT, "filter": {"inMailbox": ids.sent}, "limit": 100_000, "calculateTotal": true});
    let found = one(&stand_in, json!(["Email/query", args, "c0"])).unwrap();
    assert_eq!((found["ids"].as_array().map(Vec::len), &found["total"]), (Some(500), &json!(600)));
}

#[test]
fn messages_are_shown_as_a_server_shows_them() {
    let stand_in = StandIn::new();
    let (folded, lf, bare, attached, wide) = stand_in.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        let folded = d.deliver(
            &inbox,
            b"Subject: one\r\n two\r\n\tthree\r\nX-Empty:\r\nnot a header\r\n: nameless\r\nSpaced : value\r\n\r\nBody\r\n  with   gaps\r\n",
            &[],
            "2026-01-01T00:00:01Z",
        );
        let lf = d.deliver(&inbox, b"Subject: lf\n\nBody\n", &[], "2026-01-01T00:00:02Z");
        let bare = d.deliver(&inbox, b"Subject: no body", &[], "2026-01-01T00:00:03Z");
        let attached = d.deliver(
            &inbox,
            b"Content-Type: multipart/mixed\r\n\r\nContent-DISPOSITION: Attachment; filename=a.txt\r\n",
            &[],
            "2026-01-01T00:00:04Z",
        );
        let mut long = b"Subject: long\r\n\r\n".to_vec();
        long.extend("é".repeat(1000).bytes());
        long.extend([0xff, 0xfe]);
        let wide = d.deliver(&inbox, &long, &[], "2026-01-01T00:00:05Z");
        (folded, lf, bare, attached, wide)
    });
    let get = |id: &str| {
        let args = json!({"accountId": ACCOUNT, "ids": [id], "properties": ["headers", "preview", "hasAttachment", "blobId", "mailboxIds"]});
        one(&stand_in, json!(["Email/get", args, "c0"])).unwrap()["list"][0].clone()
    };
    let email = get(&folded);
    assert_eq!(
        email["headers"],
        json!([{"name": "Subject", "value": " one\r\n two\r\n\tthree"}, {"name": "X-Empty", "value": ""}]),
        "folds are kept; a line with no colon, a field with no name and one with a blank before its colon are none"
    );
    assert_eq!(email["preview"], "Body with gaps");
    assert_eq!(email["hasAttachment"], false);
    assert!(email["blobId"].as_str().is_some_and(|id| Id::new(id).is_ok()));
    assert_eq!(email["mailboxIds"].as_object().map(|m| m.len()), Some(1));
    assert_eq!((get(&lf)["headers"][0]["value"].clone(), get(&lf)["preview"].clone()), (json!(" lf"), json!("Body")));
    assert_eq!((get(&bare)["preview"].clone(), get(&bare)["headers"].as_array().map(Vec::len)), (json!(""), Some(1)));
    assert_eq!(get(&attached)["hasAttachment"], true);
    assert_eq!(get(&wide)["preview"].as_str().map(|p| p.chars().count()), Some(MAX_PREVIEW));
    // A message with too many header fields gives the first 500.
    let many = stand_in.with(|d| {
        let mailbox = d.mailbox_named("Inbox").map(str::to_string).unwrap();
        let raw: String = (0..MAX_HEADERS + 50).map(|n| format!("X-{n}: v\r\n")).collect();
        d.deliver(&mailbox, raw.as_bytes(), &[], "2026-01-01T00:00:06Z")
    });
    assert_eq!(get(&many)["headers"].as_array().map(Vec::len), Some(MAX_HEADERS));
}

#[test]
fn what_is_shown_of_an_email_is_not_the_message() {
    let (stand_in, ids) = filled();
    let shown = format!("{:?}", stand_in.with(|d| d.email(&ids.first).cloned()));
    assert!(shown.contains("raw_bytes") && shown.contains(&ids.first) && !shown.contains("Hello") && !shown.contains("zoe@"), "{shown}");
}

#[test]
fn the_default_properties_leave_out_the_raw_headers() {
    let (stand_in, ids) = filled();
    let email = one(&stand_in, json!(["Email/get", {"accountId": ACCOUNT, "ids": [ids.first]}, "c0"])).unwrap()["list"][0].clone();
    let mut names: Vec<&str> = email.as_object().unwrap().keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["blobId", "hasAttachment", "id", "keywords", "mailboxIds", "preview", "receivedAt", "size"]);
    assert_eq!(email["mailboxIds"], json!({ids.inbox.clone(): true}));
}

#[test]
fn a_call_that_is_wrong_is_refused_for_the_reason_a_server_gives() {
    let (stand_in, ids) = filled();
    let inbox = &ids.inbox;
    let many: Vec<String> = (0..501).map(|n| format!("E{n}")).collect();
    let account = json!(ACCOUNT);
    let cases = vec![
        ("a method it has not", json!(["Foo/bar", {}, "c0"]), "unknownMethod"),
        ("a method of another kind", json!(["Email/set", {"accountId": account}, "c0"]), "unknownMethod"),
        ("no account", json!(["Mailbox/get", {}, "c0"]), "invalidArguments"),
        ("another account", json!(["Mailbox/get", {"accountId": "A2"}, "c0"]), "accountNotFound"),
        ("an account that is no text", json!(["Mailbox/get", {"accountId": 1}, "c0"]), "invalidArguments"),
        ("arguments that are no object", json!(["Mailbox/get", [], "c0"]), "invalidArguments"),
        ("an argument it has not", json!(["Mailbox/get", {"accountId": account, "idz": []}, "c0"]), "invalidArguments"),
        ("ids that are no list", json!(["Mailbox/get", {"accountId": account, "ids": "M1"}, "c0"]), "invalidArguments"),
        ("an id with a space", json!(["Mailbox/get", {"accountId": account, "ids": ["M 1"]}, "c0"]), "invalidArguments"),
        ("an id with a slash", json!(["Mailbox/get", {"accountId": account, "ids": ["a/b"]}, "c0"]), "invalidArguments"),
        ("an id with an accent", json!(["Mailbox/get", {"accountId": account, "ids": ["é"]}, "c0"]), "invalidArguments"),
        ("an empty id", json!(["Mailbox/get", {"accountId": account, "ids": [""]}, "c0"]), "invalidArguments"),
        ("a long id", json!(["Mailbox/get", {"accountId": account, "ids": ["a".repeat(513)]}, "c0"]), "invalidArguments"),
        ("a number for an id", json!(["Mailbox/get", {"accountId": account, "ids": [1]}, "c0"]), "invalidArguments"),
        ("too many ids", json!(["Email/get", {"accountId": account, "ids": many}, "c0"]), "requestTooLarge"),
        ("every email", json!(["Email/get", {"accountId": account, "ids": null}, "c0"]), "requestTooLarge"),
        ("no ids at all", json!(["Email/get", {"accountId": account}, "c0"]), "requestTooLarge"),
        ("a property it has not", json!(["Email/get", {"accountId": account, "ids": [], "properties": ["bodyStructure"]}, "c0"]), "invalidArguments"),
        ("properties that are no list", json!(["Email/get", {"accountId": account, "ids": [], "properties": "id"}, "c0"]), "invalidArguments"),
        ("a body to fetch", json!(["Email/get", {"accountId": account, "ids": [], "fetchTextBodyValues": true}, "c0"]), "invalidArguments"),
        ("no filter", json!(["Email/query", {"accountId": account}, "c0"]), "unsupportedFilter"),
        ("a null filter", json!(["Email/query", {"accountId": account, "filter": null}, "c0"]), "unsupportedFilter"),
        ("a filter on text", json!(["Email/query", {"accountId": account, "filter": {"text": "x"}}, "c0"]), "unsupportedFilter"),
        (
            "a filter operator",
            json!(["Email/query", {"accountId": account, "filter": {"operator": "AND", "conditions": []}}, "c0"]),
            "unsupportedFilter",
        ),
        (
            "a filter on more",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox, "hasKeyword": "$seen"}}, "c0"]),
            "unsupportedFilter",
        ),
        ("a filter that is no object", json!(["Email/query", {"accountId": account, "filter": [inbox]}, "c0"]), "unsupportedFilter"),
        ("a mailbox that is none", json!(["Email/query", {"accountId": account, "filter": {"inMailbox": "M999"}}, "c0"]), "invalidArguments"),
        ("a mailbox that is no id", json!(["Email/query", {"accountId": account, "filter": {"inMailbox": "M 1"}}, "c0"]), "invalidArguments"),
        (
            "a sort by size",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "sort": [{"property": "size"}]}, "c0"]),
            "unsupportedSort",
        ),
        (
            "a sort with a collation",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "sort": [{"property": "receivedAt", "collation": "i;octet"}]}, "c0"]),
            "unsupportedSort",
        ),
        (
            "a sort that is no list",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "sort": "receivedAt"}, "c0"]),
            "unsupportedSort",
        ),
        (
            "a comparator that is no object",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "sort": ["receivedAt"]}, "c0"]),
            "unsupportedSort",
        ),
        (
            "an isAscending that is no boolean",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "sort": [{"property": "receivedAt", "isAscending": 1}]}, "c0"]),
            "invalidArguments",
        ),
        (
            "a position that is text",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "position": "0"}, "c0"]),
            "invalidArguments",
        ),
        (
            "a position that is no integer",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "position": 1.5}, "c0"]),
            "invalidArguments",
        ),
        (
            "threads collapsed",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "collapseThreads": true}, "c0"]),
            "invalidArguments",
        ),
        (
            "collapseThreads that is text",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "collapseThreads": "no"}, "c0"]),
            "invalidArguments",
        ),
        (
            "an anchor offset",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "anchorOffset": 1}, "c0"]),
            "invalidArguments",
        ),
        (
            "a limit that is text",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "limit": "9"}, "c0"]),
            "invalidArguments",
        ),
        (
            "a total that is text",
            json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "calculateTotal": "yes"}, "c0"]),
            "invalidArguments",
        ),
        ("an anchor", json!(["Email/query", {"accountId": account, "filter": {"inMailbox": inbox}, "anchor": "E1"}, "c0"]), "invalidArguments"),
    ];
    for (what, call, kind) in cases {
        assert_eq!(one(&stand_in, call), Err(kind.to_string()), "{what}");
    }
    // More mailboxes than a `get` answers: it names them, or it refuses.
    stand_in.with(|d| d.max_get = 1);
    assert_eq!(one(&stand_in, json!(["Mailbox/get", {"accountId": account, "ids": null}, "c0"])), Err("requestTooLarge".to_string()));
    assert_eq!(one(&stand_in, json!(["Mailbox/get", {"accountId": account, "ids": [inbox, ids.sent]}, "c0"])), Err("requestTooLarge".to_string()));
    assert!(one(&stand_in, json!(["Mailbox/get", {"accountId": account, "ids": [inbox]}, "c0"])).is_ok());
}

#[test]
fn a_method_needs_its_capability_to_be_named() {
    let (stand_in, _) = filled();
    let call = json!([["Mailbox/get", {"accountId": ACCOUNT}, "c0"]]);
    let (status, body) = raw(&stand_in, json!([CORE]), call.clone());
    assert_eq!(
        (status, body["methodResponses"][0][0].clone(), body["methodResponses"][0][1]["type"].clone()),
        (200, json!("error"), json!("unknownMethod"))
    );
    assert_eq!(raw(&stand_in, json!([]), call.clone()).1["methodResponses"][0][0], "error");
    for using in [json!([MAIL]), json!([CORE, MAIL]), json!([MAIL, MAIL])] {
        assert_eq!(raw(&stand_in, using, call.clone()).1["methodResponses"][0][0], "Mailbox/get");
    }
}

#[test]
fn a_reference_is_resolved_as_the_standard_has_it() {
    let (stand_in, ids) = filled();
    let account = json!(ACCOUNT);
    let by = |path: &str, name: &str| json!({"resultOf": "c0", "name": name, "path": path});
    // `*` over a list joins what is found at the end of the path.
    let calls = json!([
        ["Mailbox/get", {"accountId": account, "ids": null, "properties": ["name"]}, "c0"],
        ["Mailbox/get", {"accountId": account, "#ids": by("/list/*/id", "Mailbox/get"), "properties": ["role"]}, "c1"],
    ]);
    let (_, body) = raw(&stand_in, json!([CORE, MAIL]), calls);
    assert_eq!(body["methodResponses"][1][1]["list"], json!([{"id": ids.inbox, "role": "inbox"}, {"id": ids.sent, "role": "sent"}]));
    // The path is an RFC 6901 pointer.
    let value = json!({"a/b": {"c~d": [1, 2]}, "list": [{"ids": ["x"]}, {"ids": ["y", "z"]}]});
    assert_eq!(pointer(&value, &["a~1b", "c~0d", "1"]), Some(json!(2)));
    assert_eq!(pointer(&value, &["list", "*", "ids"]), Some(json!(["x", "y", "z"])));
    assert_eq!(pointer(&value, &[]), Some(value.clone()));
    for no in [&["a~1b", "c~0d", "2"][..], &["a~1b", "c~0d", "x"], &["a~1b", "c~0d", "1", "1"], &["nope"], &["list", "*", "nope"]] {
        assert_eq!(pointer(&value, no), None, "{no:?}");
    }
    let list = json!({"a": [10, 20]});
    assert_eq!((pointer(&list, &["a", "0"]), pointer(&list, &["a", "1"])), (Some(json!(10)), Some(json!(20))));
    for index in ["01", "00", "+1", "-0", " 1", "1 ", "", "1e0", "0x1", "\u{663}"] {
        assert_eq!(pointer(&list, &["a", index]), None, "{index:?}");
    }
    let broken = [
        ("a name that is not the call's", json!({"#ids": by("/ids", "Email/query")})),
        ("a call that is not there", json!({"#ids": {"resultOf": "c9", "name": "Mailbox/get", "path": "/ids"}})),
        ("a call that comes later", json!({"#ids": {"resultOf": "c1", "name": "Mailbox/get", "path": "/ids"}})),
        ("a path that is none", json!({"#ids": by("/nope", "Mailbox/get")})),
        ("a path with no slash", json!({"#ids": by("list", "Mailbox/get")})),
        ("a path of too many segments", json!({"#ids": by(&"/list".repeat(MAX_POINTER + 1), "Mailbox/get")})),
    ];
    for (what, extra) in broken {
        let mut args = json!({"accountId": account});
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let calls = json!([["Mailbox/get", {"accountId": account}, "c0"], ["Mailbox/get", args, "c1"]]);
        let (_, body) = raw(&stand_in, json!([CORE, MAIL]), calls);
        assert_eq!(body["methodResponses"][1][1]["type"], "invalidResultReference", "{what}: {body}");
    }
    // A reference to a call that failed, a reference that is incomplete, and a value given twice.
    let failed = json!([
        ["Mailbox/get", {"accountId": "A2"}, "c0"],
        ["Mailbox/get", {"accountId": account, "#ids": by("/ids", "Mailbox/get")}, "c1"],
        ["Mailbox/get", {"accountId": account, "#ids": {"resultOf": "c0"}}, "c2"],
        ["Mailbox/get", {"accountId": account, "ids": [], "#ids": by("/ids", "Mailbox/get")}, "c3"],
    ]);
    let (_, body) = raw(&stand_in, json!([CORE, MAIL]), failed);
    let kinds: Vec<&str> = (0..4).map(|n| body["methodResponses"][n][1]["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["accountNotFound", "invalidResultReference", "invalidArguments", "invalidArguments"]);
}

#[test]
fn a_request_that_is_wrong_as_a_whole_is_refused_as_a_whole() {
    let (stand_in, _) = filled();
    let auth = [("Authorization", bearer())];
    let send = |body: &[u8]| {
        let resp = post(&stand_in, &[("Authorization", &auth[0].1)], body);
        (resp.status(), serde_json::from_slice::<Value>(resp.body()).unwrap())
    };
    let kind = |body: &[u8]| {
        let (status, doc) = send(body);
        assert_eq!(status, 400);
        (doc["type"].as_str().unwrap().to_string(), doc["status"].clone())
    };
    let problem = |name: &str| (format!("urn:ietf:params:jmap:error:{name}"), json!(400));
    assert_eq!(kind(b"not json"), problem("notJSON"));
    assert_eq!(kind(b""), problem("notJSON"));
    for body in [
        json!({}),
        json!([]),
        json!({"using": [CORE]}),
        json!({"methodCalls": []}),
        json!({"using": "x", "methodCalls": []}),
        json!({"using": [CORE, 1], "methodCalls": []}),
        json!({"using": [CORE], "methodCalls": {}}),
        json!({"using": [CORE], "methodCalls": [["Mailbox/get", {}]]}),
        json!({"using": [CORE], "methodCalls": [[1, {}, "c0"]]}),
        json!({"using": [CORE], "methodCalls": [["Mailbox/get", {}, 0]]}),
        json!({"using": [CORE], "methodCalls": ["Mailbox/get"]}),
    ] {
        assert_eq!(kind(&serde_json::to_vec(&body).unwrap()), problem("notRequest"), "{body}");
    }
    assert_eq!(kind(&serde_json::to_vec(&json!({"using": [CORE, "urn:example:x"], "methodCalls": []})).unwrap()), problem("unknownCapability"));
    assert_eq!(send(&serde_json::to_vec(&json!({"using": [], "methodCalls": []})).unwrap()).1["methodResponses"], json!([]));
    // A request over `maxSizeRequest` is refused unread.
    let mut big = br#"{"using":[],"methodCalls":[],"pad":""#.to_vec();
    big.extend(vec![b'x'; MAX_REQUEST]);
    big.extend(br#""}"#);
    let (status, doc) = send(&big);
    assert_eq!((status, doc["type"].clone(), doc["limit"].clone()), (400, json!("urn:ietf:params:jmap:error:limit"), json!("maxSizeRequest")));
    // 16 calls are taken, a 17th is not; and what is not taken is not run.
    let calls = |n: usize| json!((0..n).map(|i| json!(["Mailbox/get", {"accountId": ACCOUNT}, format!("c{i}")])).collect::<Vec<_>>());
    let (status, doc) = raw(&stand_in, json!([CORE, MAIL]), calls(16));
    assert_eq!((status, doc["methodResponses"].as_array().map(Vec::len)), (200, Some(16)));
    let before = stand_in.calls().len();
    let (status, doc) = raw(&stand_in, json!([CORE, MAIL]), calls(17));
    assert_eq!((status, doc["limit"].clone(), doc["type"].clone()), (400, json!("maxCallsInRequest"), json!("urn:ietf:params:jmap:error:limit")));
    assert_eq!(stand_in.calls().len(), before);
    stand_in.with(|d| d.max_calls = 2);
    assert_eq!(raw(&stand_in, json!([CORE, MAIL]), calls(3)).0, 400);
    assert_eq!(raw(&stand_in, json!([CORE, MAIL]), calls(2)).0, 200);
}

#[test]
fn the_address_the_method_and_the_content_type_are_held_to() {
    let (stand_in, _) = filled();
    let body = serde_json::to_vec(&json!({"using": [CORE, MAIL], "methodCalls": []})).unwrap();
    let status_of = |method: &str, address: &str, headers: &[(&str, &str)]| {
        let token = bearer();
        let fields: Vec<(&str, &str)> = std::iter::once(("Authorization", token.as_str())).chain(headers.iter().copied()).collect();
        stand_in.answer(method, address, &fields, &body).map(|r| r.status())
    };
    let api = API;
    let json_type = [("Content-Type", "application/json")];
    assert_eq!(status_of("POST", api, &json_type), Ok(200));
    for ok in ["application/json", "Application/JSON", "application/json; charset=utf-8", " application/json ;charset=UTF-8"] {
        assert_eq!(status_of("POST", api, &[("content-type", ok)]), Ok(200), "{ok}");
    }
    for bad in ["text/plain", "application/jsonx", "application/json-seq", "", "json"] {
        assert_eq!(status_of("POST", api, &[("Content-Type", bad)]), Ok(400), "{bad}");
    }
    assert_eq!(status_of("POST", api, &[]), Ok(400), "no content type at all");
    // ... and it says so as the standard has it (RFC 8620 §3.6.1).
    let refused = stand_in.answer("POST", api, &[("Content-Type", "text/plain"), ("Authorization", &bearer())], &body).unwrap();
    let problem: Value = serde_json::from_slice(refused.body()).unwrap();
    assert_eq!((refused.status(), problem["type"].clone()), (400, json!("urn:ietf:params:jmap:error:notJSON")));
    assert_eq!(refused.content_type(), Some("application/problem+json"));
    assert_eq!(status_of("GET", api, &json_type), Ok(405));
    assert_eq!(status_of("POST", "https://jmap.standin.example/jmap/session", &json_type), Ok(405));
    assert_eq!(status_of("GET", "https://jmap.standin.example/jmap/session?account=x", &[]), Ok(200), "the query is not part of the path");
    assert_eq!(status_of("GET", "https://jmap.standin.example/jmap/nothing", &[]), Ok(404));
    assert_eq!(status_of("GET", "https://jmap.standin.example/", &[]), Ok(404));
    // Another place, or no place, is no connection at all.
    for elsewhere in
        ["https://jmap.example.org/jmap/api", "http://jmap.standin.example/jmap/api", "https://jmap.standin.example:8443/jmap/api", "nonsense", ""]
    {
        assert_eq!(status_of("POST", elsewhere, &json_type), Err(TransportError::Unreachable), "{elsewhere}");
    }
}

#[test]
fn only_the_credentials_it_knows_are_let_in() {
    let (stand_in, _) = filled();
    let basic = |user: &str, pass: &str| format!("Basic {}", mailcraft_charset::base64::STANDARD.encode(format!("{user}:{pass}")));
    let status_with = |authorization: Option<&str>| {
        let fields: Vec<(&str, &str)> = authorization.map(|value| ("authorization", value)).into_iter().collect();
        get(&stand_in, SESSION_URL, &fields).unwrap().status()
    };
    for (n, (authorization, expected)) in [
        (None, 401),
        (Some(bearer()), 200),
        (Some(format!("bearer {TOKEN}")), 200),
        (Some(basic(USER, PASSWORD)), 200),
        (Some(basic(USER, &format!("{PASSWORD}:more"))), 401),
        (Some(basic("other@standin.example", PASSWORD)), 401),
        (Some(basic(USER, "wrong")), 401),
        (Some(format!("Bearer {TOKEN}x")), 401),
        (Some(format!("Bearer {}", &TOKEN[1..])), 401),
        (Some("Bearer".to_string()), 401),
        (Some(TOKEN.to_string()), 401),
        (Some("Basic !!!not-base64".to_string()), 401),
        (Some(format!("Basic {}", mailcraft_charset::base64::STANDARD.encode([0xff, b':', 0xfe]))), 401),
        (Some(format!("Basic {}", mailcraft_charset::base64::STANDARD.encode(USER))), 401),
        (Some("Digest x".to_string()), 401),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(status_with(authorization.as_deref()), expected, "case {n}");
    }
    // The API wants them too, and a request that was refused ran nothing.
    let refused = post(&stand_in, &[], b"{}");
    assert_eq!(refused.status(), 401);
    assert!(stand_in.calls().is_empty());
    // No bearer token held: only Basic.
    stand_in.with(|d| d.token = None);
    assert_eq!(status_with(Some(&bearer())), 401);
    assert_eq!(status_with(Some(&basic(USER, PASSWORD))), 200);
}

#[test]
fn a_token_that_is_no_longer_held_is_refused_with_401() {
    let (stand_in, _) = filled();
    let client = connect(&stand_in);
    stand_in.with(|d| d.token = Some("rotated-token".into()));
    // A client that is connected finds out at its next request, and one that connects at once.
    let refused = run_calls(&client, vec![("Mailbox/get", json!({"accountId": ACCOUNT}))]);
    assert!(matches!(refused, Err(Error::CredentialsRefused)), "{refused:?}");
    let refused = connect_with(&stand_in, Credentials::bearer(TOKEN).unwrap());
    assert!(matches!(refused, Err(Error::CredentialsRefused)), "{refused:?}");
    let renewed = connect_with(&stand_in, Credentials::bearer("rotated-token").unwrap()).unwrap();
    assert!(run_calls(&renewed, vec![("Mailbox/get", json!({"accountId": ACCOUNT}))]).unwrap()[0].is_ok());
    let statuses: Vec<u16> = stand_in.seen().iter().map(|s| s.status).collect();
    assert_eq!(statuses, [200, 401, 401, 200, 200]);
}

#[test]
fn the_stand_in_is_shared_with_a_thread_that_drives_a_client() {
    fn shared<T: Send + Sync>() {}
    shared::<StandIn>();
    let (stand_in, ids) = filled();
    let (worker, inbox) = (stand_in.clone(), ids.inbox.clone());
    let listed = std::thread::spawn(move || {
        let client = connect(&worker);
        let args = json!({"accountId": ACCOUNT, "filter": {"inMailbox": inbox}});
        let answers = run_calls(&client, vec![("Email/query", args)]).unwrap();
        answers[0].as_ref().unwrap()["ids"].as_array().map(Vec::len)
    })
    .join()
    .unwrap();
    assert_eq!(listed, Some(3));
    assert_eq!(stand_in.calls(), ["Email/query"], "what the thread asked is what this clone saw");
}

#[test]
fn what_it_answers_says_what_it_is() {
    let (stand_in, ids) = filled();
    let session = get(&stand_in, SESSION_URL, &[("Authorization", &bearer())]).unwrap();
    assert_eq!((session.status(), session.content_type(), session.retry_after()), (200, Some("application/json"), None));
    let refused = get(&stand_in, SESSION_URL, &[]).unwrap();
    assert_eq!((refused.status(), refused.content_type()), (401, Some("application/problem+json")));
    let blob = blob_of(&stand_in, &ids.first);
    let octets =
        get(&stand_in, &format!("https://jmap.standin.example/jmap/download/{ACCOUNT}/{blob}/m.eml"), &[("Authorization", &bearer())]).unwrap();
    assert_eq!((octets.status(), octets.content_type()), (200, Some("application/octet-stream")));
    stand_in.with(|d| d.refuse_next = [Some((503, Some(7)))].into());
    let busy = post(&stand_in, &[("Authorization", &bearer())], b"{}");
    assert_eq!((busy.status(), busy.content_type(), busy.retry_after()), (503, Some("application/problem+json"), Some("7")));
}

#[test]
fn what_it_remembers_of_the_requests_is_bounded() {
    let (stand_in, _) = filled();
    for _ in 0..MAX_SEEN + 10 {
        assert_eq!(get(&stand_in, SESSION_URL, &[("Authorization", &bearer())]).unwrap().status(), 200);
    }
    assert_eq!(stand_in.seen().len(), MAX_SEEN);
}

/// What a download of `path` came to, signed in as `authorization` says.
fn download(stand_in: &StandIn, path: &str, authorization: Option<&str>) -> Result<HttpResponse, TransportError> {
    let fields: Vec<(&str, &str)> = authorization.map(|value| ("Authorization", value)).into_iter().collect();
    get(stand_in, &format!("https://jmap.standin.example{path}"), &fields)
}

/// The blob of an email, as `Email/get` names it.
fn blob_of(stand_in: &StandIn, email: &str) -> String {
    let call = json!(["Email/get", {"accountId": ACCOUNT, "ids": [email], "properties": ["blobId", "size"]}, "c0"]);
    one(stand_in, call).unwrap()["list"][0]["blobId"].as_str().unwrap().to_string()
}

#[test]
fn a_blob_is_the_octets_of_its_email_at_the_address_the_session_names() {
    let (stand_in, ids) = filled();
    let client = connect(&stand_in);
    for (id, subject) in [(&ids.first, "first"), (&ids.second, "second"), (&ids.third, "third")] {
        let blob = blob_of(&stand_in, id);
        let address = client.client().download_address(&Id::new(&blob).unwrap(), "message/rfc822", "message.eml").unwrap();
        let resp = get(&stand_in, address.as_str(), &[("Authorization", &bearer())]).unwrap();
        assert_eq!((resp.status(), resp.body()), (200, mail(subject).as_slice()), "{subject}");
    }
    // Either way of signing in is let in; the size `Email/get` states is the number of octets.
    let blob = blob_of(&stand_in, &ids.first);
    let path = format!("/jmap/download/{ACCOUNT}/{blob}/message.eml");
    let basic = format!("Basic {}", mailcraft_charset::base64::STANDARD.encode(format!("{USER}:{PASSWORD}")));
    assert_eq!(download(&stand_in, &path, Some(&basic)).unwrap().body().len(), mail("first").len());
    let sized = one(&stand_in, json!(["Email/get", {"accountId": ACCOUNT, "ids": [ids.first], "properties": ["size"]}, "c0"])).unwrap();
    assert_eq!(sized["list"][0]["size"], mail("first").len());
    // The octets are the ones that were delivered, whatever they are.
    let raw = stand_in.with(|d| {
        let inbox = d.mailbox_named("Inbox").map(str::to_string).unwrap();
        d.deliver(&inbox, &[0xff, 0x00, b'\r', b'\n', 0xfe, b'\n', b'\n'], &[], "2026-10-16T00:00:00Z")
    });
    let odd = download(&stand_in, &format!("/jmap/download/{ACCOUNT}/{}/m.eml?type=x", blob_of(&stand_in, &raw)), Some(&bearer())).unwrap();
    assert_eq!(odd.body(), [0xff, 0x00, b'\r', b'\n', 0xfe, b'\n', b'\n']);
    // The requests it was sent are listed with the address and its status, and no method called.
    let seen = stand_in.seen();
    let last = seen.last().unwrap();
    assert_eq!((last.method.as_str(), last.status, last.calls.len()), ("GET", 200, 0));
    assert!(last.path.starts_with("/jmap/download/A1/") && last.path.ends_with("/m.eml"), "the query is not part of the path: {}", last.path);
}

#[test]
fn a_download_is_refused_or_not_found_as_the_standard_has_it() {
    let (stand_in, ids) = filled();
    let blob = blob_of(&stand_in, &ids.first);
    let status = |path: &str, authorization: Option<&str>| download(&stand_in, path, authorization).unwrap().status();
    let good = format!("/jmap/download/{ACCOUNT}/{blob}/message.eml");
    assert_eq!(status(&good, Some(&bearer())), 200);
    // The credentials are asked for, as for any address: a request that has none learns nothing of the blob.
    assert_eq!(status(&good, None), 401);
    assert_eq!(status(&good, Some("Bearer wrong")), 401);
    let refused = download(&stand_in, &good, None).unwrap();
    assert!(!String::from_utf8_lossy(refused.body()).contains("Hello"), "no octets of the message");
    // Not an account it has, not a blob it has, the email's id for the blob's, no name, or an address of another shape.
    for (what, path) in [
        ("another account", format!("/jmap/download/A2/{blob}/message.eml")),
        ("no blob", format!("/jmap/download/{ACCOUNT}/B999/message.eml")),
        ("the email's id", format!("/jmap/download/{ACCOUNT}/{}/message.eml", ids.first)),
        ("a blob that is no id", format!("/jmap/download/{ACCOUNT}/a%20b/message.eml")),
        ("no name", format!("/jmap/download/{ACCOUNT}/{blob}/")),
        ("no name or slash", format!("/jmap/download/{ACCOUNT}/{blob}")),
        ("a name and more", format!("/jmap/download/{ACCOUNT}/{blob}/a/b")),
        ("no blob or name", format!("/jmap/download/{ACCOUNT}")),
        ("nothing", "/jmap/download/".to_string()),
        ("a longer prefix", format!("/jmap/downloads/{ACCOUNT}/{blob}/m.eml")),
    ] {
        assert_eq!(status(&path, Some(&bearer())), 404, "{what}");
    }
    // Only a GET fetches a blob.
    let post = stand_in.answer(
        "POST",
        &format!("https://jmap.standin.example{good}"),
        &[("Content-Type", "application/json"), ("Authorization", &bearer())],
        b"{}",
    );
    assert_eq!(post.unwrap().status(), 404);
    // A blob goes with its email.
    assert!(stand_in.with(|d| d.remove(&ids.first)));
    assert!(!stand_in.with(|d| d.remove(&ids.first)), "it was there once");
    assert_eq!(status(&good, Some(&bearer())), 404);
    let gone = one(&stand_in, json!(["Email/get", {"accountId": ACCOUNT, "ids": [ids.first, ids.second]}, "c0"])).unwrap();
    assert_eq!((gone["notFound"].clone(), gone["list"].as_array().map(Vec::len)), (json!([ids.first]), Some(1)));
}

#[test]
fn a_blob_over_the_size_asked_for_is_no_answer() {
    let (stand_in, ids) = filled();
    let connection = connect(&stand_in);
    let secrets = connection.client().redactor().clone();
    let fetch = |blob: &str, most: usize| block_on(connection.download(&Id::new(blob).unwrap(), "message/rfc822", "message.eml", most, &secrets));
    let blob = blob_of(&stand_in, &ids.first);
    let size = mail("first").len();
    assert_eq!(fetch(&blob, size - 1), Err(Error::Unusable(crate::Unusable::BodyTooLarge)));
    assert_eq!(fetch(&blob, size).unwrap().len(), size);
    // An oversized blob, a real one: 3 MB come to a client that takes 2 MB at most only as an error.
    let big = stand_in.with(|d| {
        let inbox = d.mailbox_named("Inbox").map(str::to_string).unwrap();
        d.deliver(&inbox, &vec![b'x'; 3 << 20], &[], "2026-10-16T00:00:00Z")
    });
    let blob = blob_of(&stand_in, &big);
    assert_eq!(fetch(&blob, 2 << 20), Err(Error::Unusable(crate::Unusable::BodyTooLarge)));
    assert_eq!(fetch(&blob, 3 << 20).unwrap().len(), 3 << 20);
}

#[test]
fn an_email_taken_away_is_not_found_and_the_state_follows() {
    let (stand_in, ids) = filled();
    let before = one(&stand_in, json!(["Mailbox/get", {"accountId": ACCOUNT, "ids": [ids.inbox], "properties": ["totalEmails"]}, "c0"])).unwrap();
    assert!(stand_in.with(|d| d.remove(&ids.second)));
    let after = one(&stand_in, json!(["Mailbox/get", {"accountId": ACCOUNT, "ids": [ids.inbox], "properties": ["totalEmails"]}, "c0"])).unwrap();
    assert_eq!((before["list"][0]["totalEmails"].clone(), after["list"][0]["totalEmails"].clone()), (json!(3), json!(2)));
    assert_ne!(before["state"], after["state"]);
    let found = one(&stand_in, json!(["Email/query", {"accountId": ACCOUNT, "filter": {"inMailbox": ids.inbox}}, "c0"])).unwrap();
    assert_eq!(found["ids"], json!([ids.first, ids.third]));
    assert!(!stand_in.with(|d| d.remove("E999")));
}

/// The answer to a call that has an account and the arguments `extra`, or the `type` of its error.
fn call(stand_in: &StandIn, method: &str, extra: Value) -> Outcome {
    let mut args = json!({"accountId": ACCOUNT});
    args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    one(stand_in, json!([method, args, "c0"]))
}

#[test]
fn a_standard_server_parses_the_header_for_a_client_and_one_that_names_by_place_does_not() {
    let (stand_in, ids) = filled();
    let coded = stand_in.with(|d| {
        d.deliver(
            &ids.inbox,
            "From: =?UTF-8?Q?Zo=C3=AB_Park?= <zoe@example.org>\r\nTo: Ada <ada@example.org>, bob@example.org\r\nCc: Cy <cy@example.org>\r\nSubject: =?UTF-8?B?Q2Fmw6k=?=\r\nMessage-ID: <coded@example.org>\r\nDate: Thu, 15 Oct 2026 07:30:00 +0200\r\n\r\nBody\r\n"
                .as_bytes(),
            &[],
            "2026-10-16T00:00:00Z",
        )
    });
    let parsed = ["threadId", "subject", "from", "to", "cc", "messageId", "sentAt"];
    let email = call(&stand_in, "Email/get", json!({"ids": [coded, ids.first], "properties": parsed})).unwrap()["list"].clone();
    assert_eq!(email[0]["threadId"], json!(coded));
    assert_eq!(email[0]["subject"], "Café");
    assert_eq!(email[0]["from"], json!([{"name": "Zoë Park", "email": "zoe@example.org"}]));
    assert_eq!(email[0]["to"], json!([{"name": "Ada", "email": "ada@example.org"}, {"name": null, "email": "bob@example.org"}]));
    assert_eq!(
        (&email[0]["cc"], &email[0]["messageId"], &email[0]["sentAt"]),
        (&json!([{"name": "Cy", "email": "cy@example.org"}]), &json!(["coded@example.org"]), &json!("2026-10-15T05:30:00Z"))
    );
    // What the message has not, it has as null (no Cc, no Date).
    assert_eq!((&email[1]["cc"], &email[1]["sentAt"], &email[1]["subject"]), (&Value::Null, &Value::Null, &json!("first")));
    // A server that names a message by where it is leaves the reading of the header to the client.
    stand_in.with(|d| d.profile = Profile::by_place());
    for property in parsed {
        assert_eq!(
            call(&stand_in, "Email/get", json!({"ids": [coded], "properties": [property]})),
            Err("invalidArguments".to_string()),
            "{property}"
        );
    }
    let raw = call(&stand_in, "Email/get", json!({"ids": [coded], "properties": ["headers"]})).unwrap()["list"][0]["headers"].clone();
    assert_eq!(raw[3], json!({"name": "Subject", "value": " =?UTF-8?B?Q2Fmw6k=?="}));
    assert_eq!(Profile::default(), Profile::standard());
}

#[test]
fn a_server_that_names_by_place_keeps_an_email_in_one_mailbox_and_refuses_to_change_mailboxes() {
    let (stand_in, ids) = filled();
    let session = || -> Value {
        let resp = get(&stand_in, SESSION_URL, &[("Authorization", &bearer())]).unwrap();
        serde_json::from_slice::<Value>(resp.body()).unwrap()["accounts"][ACCOUNT]["accountCapabilities"][MAIL]["maxMailboxesPerEmail"].clone()
    };
    // A store of the standard kind lets an email lie in several, and has no `Mailbox/set` here.
    assert_eq!(session(), Value::Null);
    assert!(stand_in.with(|d| d.add_to_mailbox(&ids.second, &ids.sent)));
    assert!(!stand_in.with(|d| d.add_to_mailbox("E999", &ids.sent)) && !stand_in.with(|d| d.add_to_mailbox(&ids.second, "M999")));
    let both = call(&stand_in, "Email/get", json!({"ids": [ids.second], "properties": ["mailboxIds"]})).unwrap();
    assert_eq!(both["list"][0]["mailboxIds"], json!({ids.inbox.clone(): true, ids.sent.clone(): true}));
    assert_eq!(call(&stand_in, "Mailbox/get", json!({"ids": [ids.sent], "properties": ["totalEmails"]})).unwrap()["list"][0]["totalEmails"], 1);
    assert_eq!(call(&stand_in, "Mailbox/set", json!({"create": {}})), Err("unknownMethod".to_string()));
    // One that names by place keeps an email in the one, and refuses to make, rename or remove a mailbox.
    stand_in.with(|d| d.profile = Profile::by_place());
    assert_eq!(session(), 1);
    assert!(!stand_in.with(|d| d.add_to_mailbox(&ids.first, &ids.sent)));
    let change = json!({"create": {"k1": {"name": "New"}}, "update": {(ids.sent.clone()): {"name": "Renamed"}}, "destroy": [ids.inbox, "M999"]});
    let refused = call(&stand_in, "Mailbox/set", change).unwrap();
    let forbidden = |what: &str| refused[what].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v["type"].clone())).collect::<Vec<_>>();
    assert_eq!(forbidden("notCreated"), [("k1".to_string(), json!("forbidden"))]);
    assert_eq!(forbidden("notUpdated"), [(ids.sent.clone(), json!("forbidden"))]);
    assert_eq!(forbidden("notDestroyed"), [(ids.inbox.clone(), json!("forbidden")), ("M999".to_string(), json!("forbidden"))]);
    assert_eq!(refused["oldState"], refused["newState"]);
    // Nothing changed, and it holds the arguments to the standard as the other methods do.
    assert_eq!(stand_in.with(|d| d.mailboxes.len()), 2);
    assert_eq!(call(&stand_in, "Mailbox/set", json!({"create": "x"})), Err("invalidArguments".to_string()));
    assert_eq!(call(&stand_in, "Mailbox/set", json!({"destroy": {}})), Err("invalidArguments".to_string()));
    assert_eq!(call(&stand_in, "Mailbox/set", json!({"frob": 1})), Err("invalidArguments".to_string()));
    assert_eq!(one(&stand_in, json!(["Mailbox/set", {"accountId": "A2"}, "c0"])), Err("accountNotFound".to_string()));
    assert!(call(&stand_in, "Mailbox/set", json!({})).is_ok());
}

#[test]
fn an_email_is_moved_to_another_mailbox_as_another_client_does() {
    let (stand_in, ids) = filled();
    assert_eq!(stand_in.with(|d| d.move_to(&ids.second, &ids.sent)), Some(ids.second.clone()), "the same id");
    assert!(stand_in.with(|d| d.move_to("E999", &ids.sent)).is_none() && stand_in.with(|d| d.move_to(&ids.second, "M999")).is_none());
    let moved = call(&stand_in, "Email/get", json!({"ids": [ids.second], "properties": ["mailboxIds"]})).unwrap();
    assert_eq!(moved["list"][0]["mailboxIds"], json!({ids.sent.clone(): true}));
    let listed = |mailbox: &str| call(&stand_in, "Email/query", json!({"filter": {"inMailbox": mailbox}})).unwrap()["ids"].clone();
    assert!(listed(&ids.sent).as_array().unwrap().contains(&json!(ids.second)));
    assert!(!listed(&ids.inbox).as_array().unwrap().contains(&json!(ids.second)));
    assert_eq!(listed(&ids.inbox).as_array().unwrap().len(), 2);
    // A server that names mail by where it lies gives a moved email another id, and the old one is gone.
    stand_in.with(|d| d.profile = Profile::by_place());
    let renamed = stand_in.with(|d| d.move_to(&ids.third, &ids.sent)).unwrap();
    assert_ne!(renamed, ids.third);
    // Moved to the mailbox it lies in alone, it has not moved, and keeps its id.
    assert_eq!(stand_in.with(|d| d.move_to(&renamed, &ids.sent)), Some(renamed.clone()));
    let old = call(&stand_in, "Email/get", json!({"ids": [ids.third, renamed], "properties": ["mailboxIds"]})).unwrap();
    assert_eq!((old["list"].as_array().unwrap().len(), old["notFound"].clone()), (1, json!([ids.third])));
    assert_eq!(old["list"][0]["id"], json!(renamed));
}

#[test]
fn a_mailbox_it_can_not_open_answers_as_if_it_held_nothing() {
    let (stand_in, ids) = filled();
    stand_in.with(|d| d.hollow.insert(ids.inbox.clone()));
    // Nothing listed, a total of nothing, and the emails of it not found; the counts of the mailbox are what they were.
    let found = call(&stand_in, "Email/query", json!({"filter": {"inMailbox": ids.inbox}, "calculateTotal": true})).unwrap();
    assert_eq!((found["ids"].clone(), found["total"].clone()), (json!([]), json!(0)));
    let got = call(&stand_in, "Email/get", json!({"ids": [ids.first, ids.second], "properties": ["id"]})).unwrap();
    assert_eq!((got["list"].clone(), got["notFound"].clone()), (json!([]), json!([ids.first, ids.second])));
    let boxes = call(&stand_in, "Mailbox/get", json!({"ids": [ids.inbox], "properties": ["totalEmails"]})).unwrap();
    assert_eq!(boxes["list"][0]["totalEmails"], 3);
    // The others are as they were.
    assert_eq!(call(&stand_in, "Email/query", json!({"filter": {"inMailbox": ids.sent}, "calculateTotal": true})).unwrap()["total"], 0);
    stand_in.with(|d| d.hollow.clear());
    assert_eq!(call(&stand_in, "Email/query", json!({"filter": {"inMailbox": ids.inbox}, "calculateTotal": true})).unwrap()["total"], 3);
    // One that lists its emails but can not find them when they are asked for by id.
    stand_in.with(|d| d.forgetful.insert(ids.inbox.clone()));
    assert_eq!(call(&stand_in, "Email/query", json!({"filter": {"inMailbox": ids.inbox}, "calculateTotal": true})).unwrap()["total"], 3);
    let got = call(&stand_in, "Email/get", json!({"ids": [ids.first, ids.second], "properties": ["id"]})).unwrap();
    assert_eq!((got["list"].clone(), got["notFound"].clone()), (json!([]), json!([ids.first, ids.second])));
    // An email that is also elsewhere is found there.
    assert!(stand_in.with(|d| d.add_to_mailbox(&ids.first, &ids.sent)));
    let got = call(&stand_in, "Email/get", json!({"ids": [ids.first, ids.second], "properties": ["id"]})).unwrap();
    assert_eq!((got["list"].clone(), got["notFound"].clone()), (json!([{"id": ids.first}]), json!([ids.second])));
    stand_in.with(|d| d.forgetful.clear());
    // One that finds them for their keywords and loses them for their headers.
    stand_in.with(|d| d.forgetful_headers.insert(ids.inbox.clone()));
    let got = call(&stand_in, "Email/get", json!({"ids": [ids.second], "properties": ["keywords"]})).unwrap();
    assert_eq!((got["list"].as_array().unwrap().len(), got["notFound"].clone()), (1, json!([])));
    let got = call(&stand_in, "Email/get", json!({"ids": [ids.second], "properties": ["headers"]})).unwrap();
    assert_eq!((got["list"].clone(), got["notFound"].clone()), (json!([]), json!([ids.second])));
    stand_in.with(|d| d.forgetful_headers.clear());
    // One whose counts it leaves out.
    stand_in.with(|d| d.uncounted.insert(ids.inbox.clone()));
    let boxes = call(&stand_in, "Mailbox/get", json!({"properties": ["name", "totalEmails", "unreadEmails"]})).unwrap();
    let inbox = boxes["list"].as_array().unwrap().iter().find(|m| m["id"] == json!(ids.inbox)).unwrap().clone();
    let sent = boxes["list"].as_array().unwrap().iter().find(|m| m["id"] == json!(ids.sent)).unwrap().clone();
    assert_eq!(inbox, json!({"id": ids.inbox, "name": "Inbox"}));
    assert_eq!(sent["totalEmails"], 1);
}

#[test]
fn trouble_is_made_on_demand() {
    let (stand_in, ids) = filled();
    let api = |stand_in: &StandIn| {
        let body = serde_json::to_vec(
            &json!({"using": [CORE, MAIL], "methodCalls": [["Mailbox/get", {"accountId": ACCOUNT, "properties": ["name"]}, "c0"]]}),
        )
        .unwrap();
        let resp = post(stand_in, &[("Authorization", &bearer())], &body);
        (resp.status(), resp.retry_after().and_then(|seconds| seconds.parse::<u32>().ok()))
    };
    // The next requests to the API are refused, or let through, in turn; the session is not one of them.
    stand_in.with(|d| d.refuse_next = [Some((429, Some(5))), None, Some((503, None))].into());
    let _ = connect(&stand_in);
    assert_eq!([api(&stand_in), api(&stand_in), api(&stand_in), api(&stand_in)], [(429, Some(5)), (200, None), (503, None), (200, None)]);
    // A token that lives for so many requests to the API, then is gone (Basic still gets in).
    stand_in.with(|d| (d.drop_token_after, d.token) = (Some(2), Some(TOKEN.into())));
    let served = stand_in.api_requests();
    assert_eq!([api(&stand_in).0, api(&stand_in).0, api(&stand_in).0, api(&stand_in).0], [200, 200, 401, 401]);
    assert_eq!(stand_in.with(|d| (d.token.clone(), d.drop_token_after)), (None, None));
    assert_eq!(stand_in.api_requests(), served + 4);
    // A mailbox that can't be listed, and emails that can't be read, fail their own call only.
    stand_in.with(|d| {
        (d.token, d.broken_mailboxes, d.broken_emails) = (Some(TOKEN.into()), [(ids.sent.clone(), "serverFail")].into(), [ids.third.clone()].into())
    });
    assert_eq!(call(&stand_in, "Email/query", json!({"filter": {"inMailbox": ids.sent}})), Err("serverFail".to_string()));
    stand_in.with(|d| d.broken_mailboxes.insert(ids.inbox.clone(), "serverUnavailable"));
    assert_eq!(call(&stand_in, "Email/query", json!({"filter": {"inMailbox": ids.inbox}})), Err("serverUnavailable".to_string()));
    stand_in.with(|d| d.broken_mailboxes.remove(&ids.inbox));
    assert!(call(&stand_in, "Email/query", json!({"filter": {"inMailbox": ids.inbox}})).is_ok());
    let with_headers = |wanted: [&String; 2]| call(&stand_in, "Email/get", json!({"ids": wanted, "properties": ["id", "headers"]}));
    assert_eq!(with_headers([&ids.first, &ids.third]), Err("serverFail".to_string()));
    assert!(with_headers([&ids.first, &ids.second]).is_ok());
    // (The keywords of the same emails come all the same.)
    assert!(call(&stand_in, "Email/get", json!({"ids": [ids.first, ids.third], "properties": ["id", "keywords"]})).is_ok());
}

#[test]
fn what_each_call_asked_is_kept_with_its_references_resolved() {
    let (stand_in, ids) = filled();
    let client = connect(&stand_in);
    let query = json!({"accountId": ACCOUNT, "filter": {"inMailbox": ids.inbox}, "sort": [{"property": "receivedAt", "isAscending": false}], "limit": 2, "calculateTotal": true});
    let get = json!({"accountId": ACCOUNT, "#ids": reference_to(0, "Email/query", "/ids"), "properties": ["id", "keywords"]});
    run_calls(&client, vec![("Email/query", query), ("Email/get", get), ("Mailbox/get", json!({"accountId": ACCOUNT}))]).unwrap();
    // A call that refers to what isn't there has nothing resolved to keep.
    run_calls(&client, vec![("Email/get", json!({"accountId": ACCOUNT, "#ids": reference_to(7, "Email/query", "/ids")}))]).unwrap();
    let asked = stand_in.asked();
    assert_eq!(asked.len(), 3);
    assert_eq!(
        (asked[0].method.as_str(), asked[0].mailbox.as_deref(), asked[0].limit, asked[0].ids),
        ("Email/query", Some(ids.inbox.as_str()), Some(2), None)
    );
    assert_eq!(
        (asked[1].method.as_str(), asked[1].ids, asked[1].properties.clone()),
        ("Email/get", Some(2), vec!["id".to_string(), "keywords".to_string()])
    );
    assert_eq!((asked[2].method.as_str(), asked[2].mailbox.clone(), asked[2].ids, asked[2].properties.is_empty()), ("Mailbox/get", None, None, true));
    // Two requests to the API (and the session before them).
    assert_eq!(stand_in.api_requests(), 2);
}

#[test]
fn an_email_removed_is_gone_for_every_client() {
    let (stand_in, ids) = filled();
    let before = call(&stand_in, "Mailbox/get", json!({"ids": [ids.inbox]})).unwrap()["state"].clone();
    assert!(stand_in.with(|d| d.remove(&ids.second)));
    assert!(!stand_in.with(|d| d.remove(&ids.second)));
    let listed = call(&stand_in, "Email/query", json!({"filter": {"inMailbox": ids.inbox}})).unwrap();
    assert_eq!(listed["ids"], json!([ids.first, ids.third]));
    let got = call(&stand_in, "Email/get", json!({"ids": [ids.second, ids.third], "properties": ["id"]})).unwrap();
    assert_eq!((got["list"].as_array().map(Vec::len), got["notFound"].clone()), (Some(1), json!([ids.second])));
    assert_ne!(got["state"], before);
}

/// Requests with what it takes to run in some places, and anything at all in the others.
fn request_like() -> impl Strategy<Value = Value> {
    let call = (
        proptest::sample::select(vec!["Mailbox/get", "Email/query", "Email/get", "Core/echo", "x"]),
        prop_oneof![
            any_json(),
            proptest::collection::vec(
                (
                    proptest::sample::select(vec!["accountId", "ids", "#ids", "filter", "sort", "position", "limit", "properties", "calculateTotal"]),
                    prop_oneof![
                        any_json(),
                        Just(json!(ACCOUNT)),
                        Just(json!({"inMailbox": "M1"})),
                        Just(json!([{"property": "receivedAt"}])),
                        Just(json!({"resultOf": "c0", "name": "Email/query", "path": "/ids"})),
                        Just(json!({"resultOf": "c0", "name": "Mailbox/get", "path": "/list/*/id"})),
                        Just(json!(["M1", "E1", "E2"])),
                    ],
                ),
                0..6
            )
            .prop_map(|members| Value::Object(members.into_iter().map(|(name, value)| (name.to_string(), value)).collect())),
        ],
        proptest::sample::select(vec!["c0", "c1", "c2", "x"]),
    );
    (prop_oneof![Just(json!([CORE, MAIL])), any_json()], proptest::collection::vec(call, 0..20)).prop_map(|(using, calls)| {
        json!({"using": using, "methodCalls": calls.into_iter().map(|(name, args, id)| json!([name, args, id])).collect::<Vec<_>>()})
    })
}

proptest! {
    #[test]
    fn whatever_it_is_sent_it_answers_and_never_panics(request in prop_oneof![request_like(), any_json()], noise in proptest::collection::vec(any::<u8>(), 0..64)) {
        let (stand_in, _) = filled();
        for body in [serde_json::to_vec(&request).unwrap(), noise] {
            let resp = post(&stand_in, &[("Authorization", &bearer())], &body);
            prop_assert!(matches!(resp.status(), 200 | 400), "{}", resp.status());
            let doc: Value = serde_json::from_slice(resp.body()).unwrap();
            if resp.status() == 200 {
                let calls = request.get("methodCalls").and_then(Value::as_array).map_or(0, Vec::len);
                prop_assert_eq!(doc["methodResponses"].as_array().map(Vec::len), Some(calls));
            } else {
                prop_assert!(doc["type"].as_str().is_some_and(|t| t.starts_with("urn:ietf:params:jmap:error:")));
            }
        }
    }

    #[test]
    fn whatever_the_headers_say_it_lets_in_the_credentials_or_not(authorization in ".{0,80}", kind in ".{0,40}") {
        let (stand_in, _) = filled();
        let status = post(&stand_in, &[("Authorization", &authorization), ("content-type", &kind)], b"{}").status();
        prop_assert!(matches!(status, 400 | 401 | 415));
    }
}
