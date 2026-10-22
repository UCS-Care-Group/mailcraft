// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Connecting over a scripted exchange: the address asked for, the redirects followed and not, the sessions
//! refused, and what a request's answer is held to.

use std::time::Duration;

use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;
use crate::test_support::{FakeExchange, block_on, made_secret};
use crate::{CAPABILITY_CORE, HttpRequest, HttpResponse, Method, RequestProblem, TransportError};

const USER: &str = "ada@example.org";
const PASSWORD: &str = "open-sesame-42";

fn credentials() -> Credentials {
    Credentials::basic(USER, &made_secret(PASSWORD)).unwrap()
}

/// A session of the standard kind, its addresses on its own origin.
fn session() -> Value {
    json!({
        "capabilities": {"urn:ietf:params:jmap:core": {"maxCallsInRequest": 16}, "urn:ietf:params:jmap:mail": {}},
        "accounts": {"A1": {"name": USER, "isPersonal": true, "accountCapabilities": {"urn:ietf:params:jmap:mail": {}}}},
        "primaryAccounts": {"urn:ietf:params:jmap:mail": "A1"},
        "username": USER,
        "apiUrl": "/jmap/api/",
        "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
        "uploadUrl": "/jmap/upload/{accountId}/",
        "state": "s1",
    })
}

fn json_answer(status: u16, body: &Value) -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse::new(status, body.to_string().into_bytes()).with_field("Content-Type", "application/json"))
}

fn redirect(status: u16, to: &str) -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse::new(status, Vec::new()).with_field("Location", to))
}

fn connect(answers: Vec<Result<HttpResponse, TransportError>>, typed: &str) -> (Result<Connection<FakeExchange>, Error>, Vec<String>) {
    let exchange = FakeExchange::new(answers);
    let connected = block_on(Connection::connect(exchange, typed, credentials()));
    let sent = match &connected {
        Ok(connection) => connection.http().sent(),
        Err(_) => Vec::new(),
    };
    (connected, sent.iter().map(|request| request.url().to_string()).collect())
}

fn connected(more: Vec<Result<HttpResponse, TransportError>>) -> Connection<FakeExchange> {
    let mut answers = vec![json_answer(200, &session())];
    answers.extend(more);
    connect(answers, "https://jmap.example.org/jmap/session").0.unwrap()
}

#[test]
fn a_bare_host_is_asked_for_its_well_known_session_and_sent_on_within_its_origin() {
    // RFC 8620 §2.2: the well-known address may redirect.
    let (connected, sent) = connect(vec![redirect(307, "/jmap/session"), json_answer(200, &session())], "jmap.example.org");
    let connection = connected.unwrap();
    assert_eq!(sent, ["https://jmap.example.org/.well-known/jmap", "https://jmap.example.org/jmap/session"]);
    assert_eq!(connection.session_address().as_str(), "https://jmap.example.org/jmap/session");
    assert_eq!(connection.client().account_id().as_str(), "A1");
    assert_eq!(connection.client().session().api_address().as_str(), "https://jmap.example.org/jmap/api/");
}

#[test]
fn a_redirect_to_another_origin_is_the_callers_to_follow() {
    let exchange = FakeExchange::new(vec![redirect(301, "https://mail.example.net/jmap/session")]);
    match block_on(Connection::connect(exchange, "jmap.example.org", credentials())) {
        Err(Error::Redirected { target, same_origin: false }) => assert_eq!(target.as_str(), "https://mail.example.net/jmap/session"),
        other => panic!("{other:?}"),
    }
    // Nor is one to plain http, even on the same host.
    let exchange = FakeExchange::new(vec![redirect(302, "http://jmap.example.org/jmap/session")]);
    assert!(matches!(block_on(Connection::connect(exchange, "jmap.example.org", credentials())), Err(Error::RedirectRefused(_))));
}

#[test]
fn redirects_within_the_origin_are_followed_a_few_times_only() {
    let answers = (0..10).map(|n| redirect(302, &format!("/jmap/session{n}"))).collect();
    let exchange = FakeExchange::new(answers);
    let refused = block_on(Connection::connect(exchange, "jmap.example.org", credentials()));
    assert!(matches!(refused, Err(Error::Redirected { same_origin: true, .. })), "{refused:?}");
}

#[test]
fn a_session_that_sends_the_credentials_elsewhere_is_refused() {
    for (key, elsewhere) in [
        ("apiUrl", "https://api.example.net/jmap/"),
        ("downloadUrl", "https://files.example.net/{accountId}/{blobId}/{name}?type={type}"),
        ("uploadUrl", "https://jmap.example.org:8443/upload/{accountId}/"),
    ] {
        let mut doc = session();
        doc[key] = json!(elsewhere);
        let (refused, _) = connect(vec![json_answer(200, &doc)], "https://jmap.example.org/jmap/session");
        assert!(matches!(refused, Err(Error::Unusable(Unusable::Shape(_)))), "{key}: {refused:?}");
    }
}

#[test]
fn a_session_without_mail_is_refused() {
    let mut doc = session();
    doc["capabilities"] = json!({"urn:ietf:params:jmap:core": {}});
    let (refused, _) = connect(vec![json_answer(200, &doc)], "https://jmap.example.org/jmap/session");
    assert_eq!(refused.unwrap_err(), Error::NoMailAccount);
    let mut doc = session();
    doc["accounts"]["A1"]["accountCapabilities"] = json!({});
    let (refused, _) = connect(vec![json_answer(200, &doc)], "https://jmap.example.org/jmap/session");
    assert_eq!(refused.unwrap_err(), Error::NoMailAccount);
}

#[test]
fn plain_http_is_for_this_computer_alone() {
    let (refused, _) = connect(Vec::new(), "http://jmap.example.org/jmap/session");
    assert!(matches!(refused, Err(Error::Address(_))), "{refused:?}");
    // `localhost` exactly, not a name under it.
    let (refused, _) = connect(Vec::new(), "http://jmap.localhost/jmap/session");
    assert!(matches!(refused, Err(Error::Address(_))), "{refused:?}");
    let mut doc = session();
    doc["apiUrl"] = json!("/api");
    let exchange = FakeExchange::new(vec![json_answer(200, &doc)]);
    assert!(block_on(Connection::connect(exchange, "http://127.0.0.1:8080/jmap/session", credentials())).is_ok());
}

/// A request of one call, whose answer may be `most` bytes at the most.
fn one_call(most: usize) -> ApiRequest {
    let mut request = ApiRequest::new();
    request.using(CAPABILITY_MAIL).unwrap();
    request.set_response_limit(most);
    request.add_call("Mailbox/get", serde_json::Map::new()).unwrap();
    request
}

#[test]
fn an_answer_over_what_the_request_can_need_is_refused_unread() {
    let answer = json!({"methodResponses": [["Mailbox/get", {"list": []}, "c0"]], "sessionState": "s1"});
    let size = answer.to_string().len();
    let connection = connected(vec![json_answer(200, &answer), json_answer(200, &answer)]);
    let redactor = connection.client().redactor().clone();
    assert!(block_on(connection.call(&one_call(size), &redactor)).is_ok());
    assert_eq!(block_on(connection.call(&one_call(size - 1), &redactor)).unwrap_err(), Error::Unusable(Unusable::BodyTooLarge));
    // The exchange is told the limit before the body: one that stops there gives the same error.
    let sent = connection.http().sent();
    assert_eq!(sent.iter().skip(1).map(HttpRequest::max_response_bytes).collect::<Vec<_>>(), [size, size - 1]);
    let session = HttpResponse::new(200, session().to_string().into_bytes()).with_field("Content-Type", "application/json");
    let exchange = FakeExchange::capping(vec![Ok(session), json_answer(200, &answer)]);
    let capped = block_on(Connection::connect(exchange, "https://jmap.example.org/jmap/session", credentials())).unwrap();
    assert_eq!(block_on(capped.call(&one_call(size - 1), &redactor)).unwrap_err(), Error::Unusable(Unusable::BodyTooLarge));
    // An error is read as an error, its body cut to the size the core reads of a problem (an exchange that keeps
    // to the request's limit may cut it sooner).
    let busy = Ok(HttpResponse::new(429, vec![b' '; 4096]).with_field("Retry-After", "30"));
    let connection = connected(vec![busy]);
    let error = block_on(connection.call(&one_call(16), &redactor)).unwrap_err();
    assert_eq!(error, Error::Busy { retry_after: Some(RetryAfter::Delay(Duration::from_secs(30))) });
    assert_eq!(busy_seconds(&error), Some(30));
}

#[test]
fn a_request_is_one_post_to_the_api_with_the_credentials() {
    let answer = json!({"methodResponses": [["Mailbox/get", {"list": []}, "c0"]], "sessionState": "s1"});
    let connection = connected(vec![json_answer(200, &answer)]);
    let redactor = connection.client().redactor().clone();
    let answers = block_on(connection.call(&one_call(1 << 20), &redactor)).unwrap();
    assert!(answers.call_ids().eq(["c0"]));
    let sent = connection.http().sent();
    let post = sent.last().unwrap();
    assert_eq!((post.method(), post.url()), (Method::Post, "https://jmap.example.org/jmap/api/"));
    assert!(post.fields().any(|(name, value)| name == "Authorization" && value.starts_with("Basic ")));
    let body: Value = serde_json::from_slice(post.body()).unwrap();
    assert_eq!(body["using"], json!(["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"]));
}

#[test]
fn what_a_server_says_is_cleaned_with_the_redactor_given() {
    let leaky =
        json!({"type": "urn:ietf:params:jmap:error:notRequest", "status": 400, "detail": format!("no, {PASSWORD}, and not for cy@example.net")});
    let connection = connected(vec![json_answer(400, &leaky)]);
    let mut redactor = connection.client().redactor().clone();
    redactor.add_personal("cy@example.net");
    let error = block_on(connection.call(&one_call(1 << 20), &redactor)).unwrap_err();
    assert!(matches!(&error, Error::Refused { problem: Some(RequestProblem::NotRequest), .. }), "{error:?}");
    let text = error.to_string();
    assert!(!text.contains(PASSWORD) && !text.contains("cy@example.net"), "{text}");
    let shown = format!("{connection:?}");
    assert!(!shown.contains(USER) && !shown.contains(PASSWORD) && !shown.contains("A1") && !shown.contains("example.org"));
}

#[test]
fn a_blob_the_server_does_not_have_is_not_found() {
    let connection = connected(vec![Ok(HttpResponse::new(404, Vec::new())), Ok(HttpResponse::new(200, b"Subject: hi\r\n\r\nhi\r\n".to_vec()))]);
    let redactor = connection.client().redactor().clone();
    let blob = Id::new("B1").unwrap();
    assert_eq!(block_on(connection.download(&blob, "message/rfc822", "message.eml", 1 << 20, &redactor)), Err(Error::NotFound));
    assert_eq!(block_on(connection.download(&blob, "message/rfc822", "message.eml", 1 << 20, &redactor)).unwrap().len(), 19);
    let sent = connection.http().sent();
    assert_eq!(sent.last().map(|request| request.url()), Some("https://jmap.example.org/jmap/download/A1/B1/message.eml?type=message%2Frfc822"));
    assert_eq!(sent.last().map(HttpRequest::max_response_bytes), Some(1 << 20), "the exchange is told the limit");
}

#[test]
fn a_busy_server_asks_for_seconds_one_at_least_and_a_date_is_no_number_here() {
    let busy = |retry_after| Error::Busy { retry_after };
    assert_eq!(busy_seconds(&busy(Some(RetryAfter::Delay(Duration::ZERO)))), Some(1));
    assert_eq!(busy_seconds(&busy(Some(RetryAfter::Delay(Duration::from_secs(90))))), Some(90));
    assert_eq!(busy_seconds(&busy(RetryAfter::parse("Sat, 29 Oct 1994 19:43:31 GMT"))), None);
    assert_eq!(busy_seconds(&busy(None)), None);
    assert_eq!(busy_seconds(&Error::NotFound), None);
}

/// Any JSON at all, in the names and values a session and an answer deal in: for the property tests of the
/// crate.
pub(crate) fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>().prop_map(|n| serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)),
        "[ -~]{0,12}".prop_map(Value::from),
        proptest::sample::select(vec![
            CAPABILITY_MAIL,
            CAPABILITY_CORE,
            "A1",
            "error",
            "c0",
            "c1",
            "Mailbox/get",
            "Email/query",
            "/jmap/api",
            "https://jmap.example.org/x/{accountId}",
            "http://jmap.example.org/x",
            "//other.example.net/x",
            "https://user:pw@jmap.example.org/",
            "line\r\nbreak",
        ])
        .prop_map(Value::from),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        let name = proptest::sample::select(vec![
            "capabilities",
            "accounts",
            "primaryAccounts",
            "apiUrl",
            "maxObjectsInGet",
            "maxCallsInRequest",
            "type",
            "description",
            CAPABILITY_MAIL,
            CAPABILITY_CORE,
            "x",
        ]);
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            proptest::collection::vec((name, inner), 0..6)
                .prop_map(|members| Value::Object(members.into_iter().map(|(name, value)| (name.to_string(), value)).collect())),
        ]
    })
}

/// A session with what it takes to be used in some places, and anything at all in the others.
fn session_like() -> impl Strategy<Value = Value> {
    let part = |valid: Value| prop_oneof![3 => Just(valid), 1 => any_json()];
    let valid = session();
    (
        part(valid["capabilities"].clone()),
        part(valid["accounts"].clone()),
        part(valid["primaryAccounts"].clone()),
        part(valid["apiUrl"].clone()),
        part(valid["downloadUrl"].clone()),
        part(valid["uploadUrl"].clone()),
    )
        .prop_map(|(capabilities, accounts, primary, api, download, upload)| {
            json!({"capabilities": capabilities, "accounts": accounts, "primaryAccounts": primary, "apiUrl": api, "downloadUrl": download, "uploadUrl": upload, "state": "s1"})
        })
}

proptest! {
    #[test]
    fn any_session_is_connected_to_or_refused_and_never_panics(doc in prop_oneof![session_like(), any_json()], noise in proptest::collection::vec(any::<u8>(), 0..64), status in prop_oneof![Just(200u16), 300u16..600]) {
        for body in [doc.to_string().into_bytes(), noise.clone()] {
            let answer = HttpResponse::new(status, body).with_field("Content-Type", "application/json").with_field("Location", "/elsewhere");
            let exchange = FakeExchange::new(vec![Ok(answer)]);
            if let Ok(connection) = block_on(Connection::connect(exchange, "jmap.example.org", credentials())) {
                // What it connected to keeps the credentials on the session's origin.
                prop_assert!(connection.client().session().endpoints_share_origin());
                prop_assert!(connection.client().has_capability(CAPABILITY_MAIL));
            }
        }
    }
}
