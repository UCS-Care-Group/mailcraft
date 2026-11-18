// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whatever a server answers, the readers never panic, and what they take is held to its sizes.

use proptest::prelude::*;
use serde_json::{Value, json};

use crate::error::{Error, Fault};
use crate::http::HttpResponse;
use crate::{auth, delta, fetch, folders, items};

fn answer(status: u16, body: &Value) -> HttpResponse {
    HttpResponse::new(status, body.to_string().into_bytes())
}

fn junk() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        ".{0,12}".prop_map(Value::String),
        prop_oneof![Just("1"), Just("2"), Just("257"), Just("message"), Just("appointment"), Just("u,f")].prop_map(|s| json!(s)),
    ];
    leaf.prop_recursive(6, 300, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::btree_map(
                prop_oneof![
                    Just("Body"),
                    Just("Fault"),
                    Just("folder"),
                    Just("id"),
                    Just("name"),
                    Just("view"),
                    Just("m"),
                    Just("e"),
                    Just("f"),
                    Just("deleted"),
                    Just("ids"),
                    Just("token"),
                    Just("authToken"),
                    Just("_content"),
                    Just("more"),
                    Just("GetFolderResponse"),
                    Just("SearchResponse"),
                    Just("SyncResponse"),
                    Just("AuthResponse")
                ]
                .prop_map(String::from),
                inner,
                0..6
            )
            .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn hostile_answers_never_panic(body in junk(), status in prop_oneof![Just(200u16), Just(500u16), Just(503u16), any::<u16>()]) {
        let response = answer(status, &body);
        let _ = auth::answer(&response);
        let _ = folders::answer(&response);
        let _ = items::answer(&response);
        let _ = delta::answer(&response, false);
        let _ = delta::answer(&response, true);
        let _ = fetch::answer(&response);
        let wrapped = answer(status, &json!({"Body": {"GetFolderResponse": body.clone(), "SearchResponse": body.clone(), "SyncResponse": body}}));
        let _ = folders::answer(&wrapped);
        let _ = items::answer(&wrapped);
        let _ = delta::answer(&wrapped, false);
    }

    #[test]
    fn bytes_that_are_no_json_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let response = HttpResponse::new(200, bytes);
        prop_assert!(folders::answer(&response).is_err());
        let _ = fetch::answer(&response);
    }
}

#[test]
fn faults_are_read_by_their_code_alone() {
    let fault = |code: &str| answer(500, &json!({"Body": {"Fault": {"Reason": {"Text": "secret words"}, "Detail": {"Error": {"Code": code}}}}}));
    assert_eq!(folders::answer(&fault("service.AUTH_EXPIRED")), Err(Error::AuthRefused));
    assert_eq!(delta::answer(&fault("mail.MUST_RESYNC"), false), Err(Error::MustResync));
    assert_eq!(items::answer(&fault("mail.NO_SUCH_MSG")), Err(Error::NoSuchItem));
    assert_eq!(items::answer(&fault("service.PERM_DENIED")), Err(Error::Fault(Fault::PermissionDenied)));
    assert_eq!(items::answer(&fault("something.ELSE")), Err(Error::Fault(Fault::Other)));
    assert!(!Error::Fault(Fault::Other).to_string().contains("secret"));
    let busy = HttpResponse::new(503, Vec::new()).with_field("Retry-After", "120");
    assert_eq!(items::answer(&busy), Err(Error::Busy(120)));
    assert_eq!(items::answer(&HttpResponse::new(200, b"<html>Sign in</html>".to_vec())), Err(Error::Unreadable));
    assert_eq!(items::answer(&HttpResponse::new(502, b"<html>Bad gateway</html>".to_vec())), Err(Error::Status(502)));
}

#[test]
fn a_two_factor_account_and_odd_tokens_give_no_token() {
    let two = answer(200, &json!({"Body": {"AuthResponse": {"twoFactorAuthRequired": {"_content": true}}}}));
    assert_eq!(auth::answer(&two).map(|_| ()), Err(Error::TwoFactor));
    for token in ["", "with space", "a\"b", &"x".repeat(5000)] {
        let odd = answer(200, &json!({"Body": {"AuthResponse": {"authToken": [{"_content": token}]}}}));
        assert_eq!(auth::answer(&odd).map(|_| ()), Err(Error::Unreadable), "{token:?}");
    }
    let odd = answer(200, &json!({"Body": {"SyncResponse": {"token": "12 34"}}}));
    assert_eq!(delta::answer(&odd, false), Err(Error::Unreadable));
}

#[test]
fn ids_of_other_mailboxes_and_odd_ids_are_passed_over() {
    let found = answer(
        200,
        &json!({"Body": {"SearchResponse": {"m": [
            {"id": "a1b2:12", "l": "2"}, {"id": "0"}, {"id": "-4"}, {"id": "12345678901"}, {"id": "257", "l": "x:2", "su": "ok\u{0}\r\nyes"}
        ]}}}),
    );
    let (messages, more) = items::answer(&found).unwrap();
    assert!(!more);
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].server.id.as_str(), messages[0].server.folder.as_str(), messages[0].subject.as_str()), ("257", "", "ok  yes"));
    let deleted = answer(200, &json!({"Body": {"SyncResponse": {"token": "5", "deleted": [{"m": {"ids": "1,x,2,a:3,,4"}}]}}}));
    assert_eq!(delta::answer(&deleted, false).unwrap().deleted, ["1", "2", "4"]);
}

#[test]
fn a_folder_tree_that_goes_on_for_ever_is_cut_off_and_said_incomplete() {
    let mut deep = json!({"id": "9999", "name": "x"});
    for id in (300..500).rev() {
        deep = json!({"id": id.to_string(), "name": "x", "folder": [deep]});
    }
    // So deep a document isn't read at all (the reader's own limit on nesting); a tree within it
    // that is still too deep is cut off and said incomplete.
    let too_deep = folders::answer(&answer(200, &json!({"Body": {"GetFolderResponse": {"folder": {"id": "1", "folder": deep}}}})));
    assert_eq!(too_deep, Err(Error::Unreadable));
    let mut deep = json!({"id": "9999", "name": "x"});
    for id in (300..360).rev() {
        deep = json!({"id": id.to_string(), "name": "x", "folder": deep});
    }
    let listed = folders::answer(&answer(200, &json!({"Body": {"GetFolderResponse": {"folder": {"id": "1", "folder": deep}}}}))).unwrap();
    assert_eq!(listed.folders.len(), 61);
    let mut deeper = json!({"id": "9999", "name": "x"});
    for id in (300..400).rev() {
        deeper = json!({"id": id.to_string(), "name": "x", "folder": deeper});
    }
    match folders::answer(&answer(200, &json!({"Body": {"GetFolderResponse": {"folder": {"id": "1", "folder": deeper}}}}))) {
        Ok(listed) => assert!(!listed.complete && listed.folders.len() <= 66),
        Err(e) => assert_eq!(e, Error::Unreadable),
    }
}
