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
    // A flag that is no flag can't be read off either: the sign-in's answer is refused.
    let odd = answer(200, &json!({"Body": {"AuthResponse": {"twoFactorAuthRequired": {"_content": "tr\u{0}ue"}}}}));
    assert_eq!(auth::answer(&odd).map(|_| ()), Err(Error::Unreadable));
    for token in ["", "with space", "a\"b", &"x".repeat(5000)] {
        let odd = answer(200, &json!({"Body": {"AuthResponse": {"authToken": [{"_content": token}]}}}));
        assert_eq!(auth::answer(&odd).map(|_| ()), Err(Error::Unreadable), "{token:?}");
    }
    for token in ["12 34", "1\u{0}2"] {
        // A character cleaning would take out must never make a token it wasn't pass for the
        // one the server sent: the page it comes with is refused, its token never kept.
        let odd = answer(200, &json!({"Body": {"SyncResponse": {"token": token}}}));
        assert_eq!(delta::answer(&odd, false), Err(Error::Unreadable), "{token:?}");
    }
}

#[test]
fn ids_of_other_mailboxes_and_odd_ids_are_passed_over() {
    // A hit whose id is no id at all can't be told from a cached message the listing left out:
    // the whole answer is refused, like an item that names no folder.
    let odd = answer(200, &json!({"Body": {"SearchResponse": {"m": [{"id": "0"}, {"id": "-4"}, {"id": "12345678901"}]}}}));
    assert_eq!(items::answer(&odd), Err(Error::Unreadable));
    let found = answer(
        200,
        &json!({"Body": {"SearchResponse": {"m": [
            {"id": "a1b2:12", "l": "2"}, {"id": "257", "l": "x:2", "su": "another mailbox's"}, {"id": "300", "l": "2", "su": "ok\u{0}\r\nyes"}
        ]}}}),
    );
    let page = items::answer(&found).unwrap();
    assert!(!page.more);
    assert_eq!(page.messages.len(), 1);
    assert_eq!(
        (page.messages[0].server.id.as_str(), page.messages[0].server.folder.as_str(), page.messages[0].subject.as_str()),
        ("300", "2", "ok  yes")
    );
    // An id of another mailbox's item in a deletion list is passed over; an entry that is no
    // id at all would have the page's token kept while a deletion is lost — so the answer is
    // refused.
    let deleted = answer(200, &json!({"Body": {"SyncResponse": {"token": "5", "deleted": [{"m": {"ids": "1,a:3,2,4"}}]}}}));
    assert_eq!(delta::answer(&deleted, false).unwrap().deleted, ["1", "2", "4"]);
    let deleted = answer(200, &json!({"Body": {"SyncResponse": {"token": "5", "deleted": [{"m": {"ids": "1,x,2"}}]}}}));
    assert_eq!(delta::answer(&deleted, false), Err(Error::Unreadable));
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

#[test]
fn an_item_with_no_folder_is_unreadable_not_removed() {
    // A summary that names the item but not the folder that holds it can't tell the run moved
    // or removed: answered as an empty folder, the run would count it deleted and move its
    // token on past the change, so the whole answer is refused.
    let found = answer(200, &json!({"Body": {"SearchResponse": {"m": [{"id": "257", "su": "where am I"}]}}}));
    assert_eq!(items::answer(&found), Err(Error::Unreadable));
    // A folder the item says is another mailbox's passes it over, like a foreign id does.
    let foreign = answer(200, &json!({"Body": {"SearchResponse": {"m": [{"id": "257", "l": "998:2", "su": "not ours"}]}}}));
    assert_eq!(items::answer(&foreign).unwrap().messages.len(), 0);
    // A `more` that is no flag can't tell the page ends the read either: read off, the run
    // would count what it never listed as gone.
    let tail = answer(200, &json!({"Body": {"SearchResponse": {"more": {}, "m": [{"id": "257", "l": "2"}]}}}));
    assert_eq!(items::answer(&tail), Err(Error::Unreadable));
    let tail = answer(200, &json!({"Body": {"SyncResponse": {"more": {}, "token": "5"}}}));
    assert_eq!(delta::answer(&tail, false), Err(Error::Unreadable));
}

#[test]
fn a_text_of_many_byte_characters_is_held_to_the_byte_limit() {
    // The parser's cap and the model validator's are both bytes: five thousand four-byte
    // characters are 20,000 bytes, over the 16 KiB either side allows, so the text is cut at
    // 4,096 whole characters rather than passed on to fail the piece's check.
    let found = answer(
        200,
        &json!({"Body": {"SearchResponse": {"m": [{"id": "257", "l": "2", "d": 1, "su": "\u{1F600}".repeat(5_000), "fr": "ü".repeat(9_000)}]}}}),
    );
    let page = items::answer(&found).unwrap();
    let message = &page.messages[0];
    assert_eq!((message.subject.len(), message.subject.chars().count()), (16 * 1024, 4 * 1024));
    assert_eq!((message.preview.len(), message.preview.chars().count()), (16 * 1024, 8 * 1024));
}
