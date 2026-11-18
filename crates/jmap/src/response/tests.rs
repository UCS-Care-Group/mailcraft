// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::request::ApiRequest;
use serde_json::json;

fn calls(count: usize) -> (ApiRequest, Vec<CallId>) {
    let mut request = ApiRequest::new();
    let ids = (0..count).map(|_| request.add_call("Core/echo", Map::new()).unwrap()).collect();
    (request, ids)
}

// RFC 8620 §3.4.1: the example response, with an error for the third call.
#[test]
fn reads_example_response() {
    let (_, ids) = calls(3);
    let body = br#"{
          "methodResponses": [
            ["method1", {"arg1": 3, "arg2": "foo"}, "c0"],
            ["method2", {"isBlah": true}, "c1"],
            ["anotherResponseFromMethod2", {"data": 10, "yetmoredata": "Hello"}, "c1"],
            ["error", {"type": "unknownMethod"}, "c2"]
          ],
          "sessionState": "75128aab4b1b"
        }"#;
    let mut response = ApiResponse::parse(body, &Redactor::new()).unwrap();
    assert_eq!(response.session_state(), "75128aab4b1b");
    assert_eq!(response.take_named(&ids[0], "method1").unwrap()["arg2"], json!("foo"));
    let second = response.take(&ids[1]).unwrap();
    assert_eq!(second.iter().map(MethodResponse::name).collect::<Vec<_>>(), ["method2", "anotherResponseFromMethod2"]);
    match response.take(&ids[2]) {
        Err(CallFailure::Method(error)) => {
            assert_eq!(error.kind(), &MethodErrorKind::UnknownMethod)
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(response.take(&ids[0]), Err(CallFailure::Missing));
}

// RFC 8620 §3.6.2: an error is one call's failure; unknown types are kept (and treated like serverFail).
#[test]
fn method_errors() {
    let (_, ids) = calls(2);
    let body = json!({
        "methodResponses": [
            ["error", {"type": "somethingNew", "description": "Line one\nwith secret-pass inside"}, "c0"],
            ["Core/echo", {}, "c1"]
        ],
        "sessionState": "s1"
    });
    let mut redactor = Redactor::new();
    redactor.add("secret-pass");
    let mut response = ApiResponse::parse(body.to_string().as_bytes(), &redactor).unwrap();
    match response.take(&ids[0]) {
        Err(CallFailure::Method(error)) => {
            assert_eq!(error.kind(), &MethodErrorKind::Other("somethingNew".into()));
            assert_eq!(error.description().unwrap().as_str(), "Line one with [redacted] inside");
            assert!(!error.to_string().contains('\n'));
        }
        other => panic!("{other:?}"),
    }
    assert!(response.take(&ids[1]).is_ok());
    assert!(MethodErrorKind::ServerUnavailable.may_succeed_later());
    assert_eq!(MethodErrorKind::from_type(None), MethodErrorKind::Other(String::new()));
    assert_eq!(MethodErrorKind::from_type(Some("x y")), MethodErrorKind::Other(String::new()));
    assert_eq!(MethodErrorKind::from_type(Some("stateMismatch")), MethodErrorKind::StateMismatch);
}

// Requirement 3.5: an error type that echoes a secret is not kept.
#[test]
fn error_type_echoing_a_secret() {
    let (_, ids) = calls(1);
    let mut redactor = Redactor::new();
    redactor.add("hunter22");
    let body = json!({"methodResponses": [["error", {"type": "hunter22"}, "c0"]], "sessionState": "s"});
    let mut response = ApiResponse::parse(body.to_string().as_bytes(), &redactor).unwrap();
    match response.take(&ids[0]) {
        Err(CallFailure::Method(error)) => {
            assert_eq!(error.kind(), &MethodErrorKind::Other(String::new()))
        }
        other => panic!("{other:?}"),
    }
}

// RFC 8620 §3.6.2: nothing more happens within a call after its error.
#[test]
fn responses_after_error_are_ignored() {
    let (_, ids) = calls(1);
    let body = json!({
        "methodResponses": [["Foo/set", {}, "c0"], ["error", {"type": "serverPartialFail"}, "c0"], ["Foo/get", {}, "c0"]],
        "sessionState": "s"
    });
    let mut response = ApiResponse::parse(body.to_string().as_bytes(), &Redactor::new()).unwrap();
    assert!(matches!(response.take(&ids[0]), Err(CallFailure::Method(error)) if error.kind() == &MethodErrorKind::ServerPartialFail));
}

// RFC 8621 §7.5: an implicit call answers after the method's own response, under the same call id. Its
// error does not unsay what the method answered before it.
#[test]
fn what_a_call_answered_before_its_error_can_be_taken_with_the_error() {
    let (_, ids) = calls(4);
    let body = json!({
        "methodResponses": [
            ["Foo/set", {"created": {"k": {"id": "F1"}}}, "c0"], ["error", {"type": "serverUnavailable"}, "c0"], ["Foo/set", {"late": true}, "c0"],
            ["error", {"type": "serverFail"}, "c1"],
            ["Foo/get", {}, "c2"],
        ],
        "sessionState": "s"
    });
    let parsed = ApiResponse::parse(body.to_string().as_bytes(), &Redactor::new()).unwrap();
    // The plain way still takes the call for failed.
    assert!(matches!(parsed.clone().take_named(&ids[0], "Foo/set"), Err(CallFailure::Method(_))));
    let mut response = parsed;
    let (answer, rest, after) = response.take_named_before_error(&ids[0], "Foo/set");
    assert_eq!(answer.unwrap()["created"]["k"]["id"], "F1");
    assert!(rest.is_empty());
    assert_eq!(after.map(|error| error.kind().clone()), Some(MethodErrorKind::ServerUnavailable));
    // An error with nothing before it is the call's failure; another name is not taken for the one asked.
    let (answer, rest, after) = response.take_named_before_error(&ids[1], "Foo/set");
    assert!(matches!(answer, Err(CallFailure::Method(error)) if error.kind() == &MethodErrorKind::ServerFail) && rest.is_empty() && after.is_none());
    assert_eq!(response.take_named_before_error(&ids[2], "Foo/set"), (Err(CallFailure::UnexpectedResponse), Vec::new(), None));
    assert_eq!(response.take_named_before_error(&ids[3], "Foo/set"), (Err(CallFailure::Missing), Vec::new(), None));
    assert_eq!(response.take_named_before_error(&ids[0], "Foo/set"), (Err(CallFailure::Missing), Vec::new(), None), "taken once");
}

// RFC 8621 §7.5: the implicit call may answer as a response of its own, under the same call id.
#[test]
fn what_else_a_call_answered_is_handed_back_beside_the_response_asked_for() {
    let (_, ids) = calls(2);
    let body = json!({
        "methodResponses": [
            ["Foo/set", {"created": {"k": {"id": "F1"}}}, "c0"], ["Bar/set", {"notUpdated": {"B1": {"type": "forbidden"}}}, "c0"],
            ["Bar/set", {"updated": {"B2": null}}, "c1"], ["Foo/set", {}, "c1"], ["Baz/set", {}, "c1"], ["error", {"type": "serverFail"}, "c1"],
        ],
        "sessionState": "s"
    });
    let mut response = ApiResponse::parse(body.to_string().as_bytes(), &Redactor::new()).unwrap();
    let (answer, rest, after) = response.take_named_before_error(&ids[0], "Foo/set");
    assert_eq!(answer.unwrap()["created"]["k"]["id"], "F1");
    assert_eq!(rest.iter().map(MethodResponse::name).collect::<Vec<_>>(), ["Bar/set"]);
    assert_eq!(rest[0].arguments()["notUpdated"]["B1"]["type"], "forbidden");
    assert!(after.is_none());
    // In the order they came, wherever the one asked for stood, and with the error that ended the call.
    let (answer, rest, after) = response.take_named_before_error(&ids[1], "Foo/set");
    assert!(answer.is_ok_and(|answer| answer.is_empty()));
    assert_eq!(rest.iter().map(MethodResponse::name).collect::<Vec<_>>(), ["Bar/set", "Baz/set"]);
    assert!(after.is_some());
}

// Requirement 3.3: a name other than the one asked for is not silently accepted.
#[test]
fn unexpected_names() {
    let (_, ids) = calls(1);
    let body = json!({"methodResponses": [["Other/get", {}, "c0"]], "sessionState": "s"});
    let mut response = ApiResponse::parse(body.to_string().as_bytes(), &Redactor::new()).unwrap();
    assert_eq!(response.take_named(&ids[0], "Foo/get"), Err(CallFailure::UnexpectedResponse));
}

// RFC 8620 §3.3–§3.4: createdIds are returned when given; invalid ids are dropped.
#[test]
fn created_ids() {
    let body = json!({"methodResponses": [], "sessionState": "s", "createdIds": {"k1": "M1", "bad key": "M2", "k3": "bad value"}});
    let response = ApiResponse::parse(body.to_string().as_bytes(), &Redactor::new()).unwrap();
    let created = response.created_ids().unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(created.get(&Id::new("k1").unwrap()).unwrap().as_str(), "M1");
}

// RFC 8620 §3.2 and §3.4: anything that is not a proper Response object is unusable.
#[test]
fn refuses_bad_shapes() {
    for body in [
        r#"[]"#,
        r#"{"sessionState":"s"}"#,
        r#"{"methodResponses":[]}"#,
        r#"{"methodResponses":{},"sessionState":"s"}"#,
        r#"{"methodResponses":[["a",{},"c0","extra"]],"sessionState":"s"}"#,
        r#"{"methodResponses":[["a",[],"c0"]],"sessionState":"s"}"#,
        r#"{"methodResponses":[[1,{},"c0"]],"sessionState":"s"}"#,
        r#"{"methodResponses":[["a",{}]],"sessionState":"s"}"#,
        r#"{"methodResponses":["x"],"sessionState":"s"}"#,
        r#"{"methodResponses":[["",{},"c0"]],"sessionState":"s"}"#,
        r#"{"methodResponses":[["a b",{},"c0"]],"sessionState":"s"}"#,
        r#"{"methodResponses":[],"sessionState":7}"#,
        r#"{"methodResponses":[],"sessionState":"s","createdIds":[]}"#,
    ] {
        assert!(matches!(ApiResponse::parse(body.as_bytes(), &Redactor::new()), Err(Error::Unusable(Unusable::Shape(_)))), "{body}");
    }
    assert_eq!(ApiResponse::parse(b"{", &Redactor::new()).unwrap_err(), Error::Unusable(Unusable::InvalidJson));
}

// RFC 8620 §8.4 and §8.5 (resource exhaustion): the number of responses is bounded.
#[test]
fn bounds() {
    let responses: Vec<Value> = (0..=MAX_METHOD_RESPONSES).map(|i| json!(["Core/echo", {}, format!("c{i}")])).collect();
    let body = json!({"methodResponses": responses, "sessionState": "s"});
    assert!(matches!(ApiResponse::parse(body.to_string().as_bytes(), &Redactor::new()), Err(Error::Unusable(Unusable::Shape(_)))));
    let body = json!({"methodResponses": [["Core/echo", {}, "c".repeat(256)]], "sessionState": "s"});
    assert!(ApiResponse::parse(body.to_string().as_bytes(), &Redactor::new()).is_err());
}
