// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The examples of RFC 8620 (JMAP core), read into the types and written back.
//!
//! The examples are the RFC's own, edited only where the RFC's text is not JSON as printed: line breaks of
//! the page layout inside strings joined, the `...` that stands for more entries removed, and a missing
//! comma put in. Each edit is said where it is made.

mod support;

use mailcraft_jmap_types::{
    CAPABILITY_CORE, CAPABILITY_MAIL, CoreCapability, Invocation, MethodError, Request, RequestError, Response, ResultReference, Session, StateChange,
};
use support::round_trip;

/// RFC 8620 §2.1. Edits: the comma after the mail capability's `{}`, the two `...` in the accounts'
/// mail capabilities and the `...` of the contacts capability removed, and the wrapped `downloadUrl`
/// and `eventSourceUrl` joined.
const SESSION: &str = r#"{
  "capabilities": {
    "urn:ietf:params:jmap:core": {
      "maxSizeUpload": 50000000,
      "maxConcurrentUpload": 8,
      "maxSizeRequest": 10000000,
      "maxConcurrentRequest": 8,
      "maxCallsInRequest": 32,
      "maxObjectsInGet": 256,
      "maxObjectsInSet": 128,
      "collationAlgorithms": ["i;ascii-numeric", "i;ascii-casemap", "i;unicode-casemap"]
    },
    "urn:ietf:params:jmap:mail": {},
    "urn:ietf:params:jmap:contacts": {},
    "https://example.com/apis/foobar": { "maxFoosFinangled": 42 }
  },
  "accounts": {
    "A13824": {
      "name": "john@example.com",
      "isPersonal": true,
      "isReadOnly": false,
      "accountCapabilities": {
        "urn:ietf:params:jmap:mail": { "maxMailboxesPerEmail": null, "maxMailboxDepth": 10 },
        "urn:ietf:params:jmap:contacts": {}
      }
    },
    "A97813": {
      "name": "jane@example.com",
      "isPersonal": false,
      "isReadOnly": true,
      "accountCapabilities": {
        "urn:ietf:params:jmap:mail": { "maxMailboxesPerEmail": 1, "maxMailboxDepth": 10 }
      }
    }
  },
  "primaryAccounts": {
    "urn:ietf:params:jmap:mail": "A13824",
    "urn:ietf:params:jmap:contacts": "A13824"
  },
  "username": "john@example.com",
  "apiUrl": "https://jmap.example.com/api/",
  "downloadUrl": "https://jmap.example.com/download/{accountId}/{blobId}/{name}?accept={type}",
  "uploadUrl": "https://jmap.example.com/upload/{accountId}/",
  "eventSourceUrl": "https://jmap.example.com/eventsource/?types={types}&closeafter={closeafter}&ping={ping}",
  "state": "75128aab4b1b"
}"#;

#[test]
fn session_example_round_trips() {
    let session: Session = round_trip(SESSION);
    assert_eq!(session.api_url, "https://jmap.example.com/api/");
    assert_eq!(session.state, "75128aab4b1b");
    assert_eq!(session.primary_accounts.get("urn:ietf:params:jmap:mail").map(String::as_str), Some("A13824"));
    let jane = session.accounts.get("A97813").expect("the shared account");
    assert!(jane.is_read_only && !jane.is_personal);
    assert_eq!(jane.name, "jane@example.com");
}

#[test]
fn the_core_capability_reads_with_the_examples_spelling_of_the_concurrency_limit() {
    let session: Session = serde_json::from_str(SESSION).unwrap();
    let core = session.core_capability().expect("the core capability").expect("it reads");
    assert_eq!(core.max_concurrent_requests, 8);
    assert_eq!(core.max_calls_in_request, 32);
    assert_eq!(core.collation_algorithms, ["i;ascii-numeric", "i;ascii-casemap", "i;unicode-casemap"]);
    // Written back, the limit has the name the RFC's property list gives it.
    let written = serde_json::to_value(&core).unwrap();
    assert_eq!(written["maxConcurrentRequests"], 8);
    let _: CoreCapability = serde_json::from_value(written).unwrap();
}

/// RFC 8620 §3.3.1, as printed.
const REQUEST: &str = r#"{
  "using": [ "urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail" ],
  "methodCalls": [
    [ "method1", { "arg1": "arg1data", "arg2": "arg2data" }, "c1" ],
    [ "method2", { "arg1": "arg1data" }, "c2" ],
    [ "method3", {}, "c3" ]
  ]
}"#;

#[test]
fn request_example_round_trips() {
    let request: Request = round_trip(REQUEST);
    assert_eq!(request.using, [CAPABILITY_CORE, CAPABILITY_MAIL]);
    let Invocation(name, arguments, call_id) = &request.method_calls[1];
    assert_eq!((name.as_str(), call_id.as_str()), ("method2", "c2"));
    assert_eq!(arguments["arg1"], "arg1data");
    assert!(request.created_ids.is_none());
}

/// RFC 8620 §3.4.1, as printed.
const RESPONSE: &str = r#"{
  "methodResponses": [
    [ "method1", { "arg1": 3, "arg2": "foo" }, "c1" ],
    [ "method2", { "isBlah": true }, "c2" ],
    [ "anotherResponseFromMethod2", { "data": 10, "yetmoredata": "Hello" }, "c2"],
    [ "error", { "type":"unknownMethod" }, "c3" ]
  ],
  "sessionState": "75128aab4b1b"
}"#;

#[test]
fn response_example_round_trips_and_its_error_reads_as_a_method_error() {
    let response: Response = round_trip(RESPONSE);
    assert_eq!(response.session_state, "75128aab4b1b");
    assert_eq!(response.method_responses.len(), 4);
    let Invocation(name, arguments, call_id) = &response.method_responses[3];
    assert_eq!((name.as_str(), call_id.as_str()), ("error", "c3"));
    let error: MethodError = serde_json::from_value(serde_json::Value::Object(arguments.clone())).unwrap();
    assert_eq!(error.kind, "unknownMethod");
    assert_eq!(error.description, None);
}

/// RFC 8620 §3.6.1.1, the first example, the wrapped `detail` joined.
const PROBLEM_CAPABILITY: &str = r#"{
  "type": "urn:ietf:params:jmap:error:unknownCapability",
  "status": 400,
  "detail": "The Request object used capability 'https://example.com/apis/foobar', which is not supported by this server."
}"#;

/// RFC 8620 §3.6.1.1, the second example, the wrapped `detail` joined.
const PROBLEM_LIMIT: &str = r#"{
  "type": "urn:ietf:params:jmap:error:limit",
  "limit": "maxSizeRequest",
  "status": 400,
  "detail": "The request is larger than the server is willing to process."
}"#;

#[test]
fn request_level_error_examples_round_trip() {
    let capability: RequestError = round_trip(PROBLEM_CAPABILITY);
    assert_eq!(capability.kind, "urn:ietf:params:jmap:error:unknownCapability");
    assert_eq!(capability.status, Some(400));
    assert_eq!(capability.limit, None);
    let limit: RequestError = round_trip(PROBLEM_LIMIT);
    assert_eq!(limit.kind, "urn:ietf:params:jmap:error:limit");
    assert_eq!(limit.limit.as_deref(), Some("maxSizeRequest"));
}

/// RFC 8620 §3.6.2: a method error may carry further properties of its type; they are kept.
#[test]
fn a_method_error_keeps_its_further_properties() {
    let error: MethodError = round_trip(r#"{"type": "invalidArguments", "description": "no such filter", "arguments": ["filter"]}"#);
    assert_eq!(error.description.as_deref(), Some("no such filter"));
    assert_eq!(error.extra["arguments"], serde_json::json!(["filter"]));
}

/// RFC 8620 §3.7: a result reference, as the example of §5.7 writes one.
#[test]
fn result_reference_round_trips() {
    let reference: ResultReference = round_trip(r#"{ "resultOf": "0", "name": "Todo/query", "path": "/ids" }"#);
    assert_eq!(reference.result_of, "0");
    assert_eq!(reference.name, "Todo/query");
    assert_eq!(reference.path, "/ids");
}

#[test]
fn created_ids_round_trip_on_request_and_response() {
    let request: Request = round_trip(r##"{"using": [], "methodCalls": [], "createdIds": {"k1": "M1"}}"##);
    assert_eq!(request.created_ids.unwrap()["k1"], "M1");
    let response: Response = round_trip(r#"{"methodResponses": [], "createdIds": {"k1": "M1"}, "sessionState": "s"}"#);
    assert_eq!(response.created_ids.unwrap()["k1"], "M1");
}

/// RFC 8620 §7.1.1, as printed.
#[test]
fn state_change_example_round_trips() {
    let change: StateChange = round_trip(
        r#"{
          "@type": "StateChange",
          "changed": {
            "a3123": { "Email": "d35ecb040aab", "EmailDelivery": "428d565f2440", "CalendarEvent": "87accfac587a" },
            "a43461d": { "Mailbox": "0af7a512ce70", "CalendarEvent": "7a4297cecd76" }
          }
        }"#,
    );
    assert_eq!(change.kind, "StateChange");
    assert_eq!(change.changed["a43461d"]["Mailbox"], "0af7a512ce70");
    assert_eq!(change.changed["a3123"].len(), 3);
}
