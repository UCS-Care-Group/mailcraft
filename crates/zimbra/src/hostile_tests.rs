// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whatever a server answers, the readers never panic, and what they take is held to its sizes.

use proptest::prelude::*;
use serde_json::{Value, json};

use crate::error::{Error, Fault};
use crate::http::{HttpResponse, Secret};
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
fn only_a_full_mime_fetch_asks_for_a_larger_response() {
    let token = crate::http::Secret::new("synthetic-token");
    let fetch = fetch::request("https://mail.example.org", &token, "257").unwrap();
    assert_eq!(fetch.max_response_bytes(), crate::http::RESPONSE_LIMIT);
    assert_eq!(fetch.max_response_bytes(), 32 * 1024 * 1024);
    let search = items::by_id("https://mail.example.org", &token, &["257".into()]).unwrap();
    assert_eq!(search.max_response_bytes(), crate::http::SOAP_LIMIT);
}

#[test]
fn oversized_message_text_does_not_wedge_a_sync_page() {
    for text in ["x".repeat(20 * 1024), "界".repeat(8 * 1024)] {
        for field in ["mid", "address", "tn"] {
            let mut message = json!({"id": "257", "l": "2", "mid": "<kept@example.org>", "e": [{"t": "f", "a": "ada@example.org"}]});
            if field == "mid" {
                message["mid"] = json!(format!("<{text}>"));
            } else if field == "tn" {
                message["tn"] = json!(format!("{text},Kept"));
            } else {
                message["e"][0]["a"] = json!(text);
            }
            let response = answer(200, &json!({"Body": {"SearchResponse": {"m": [message]}}}));
            let page = items::answer(&response).unwrap();
            assert_eq!(page.messages.len(), 1);
            let result = mailcraft_model::sync::ZimbraMailResult {
                pages: vec![mailcraft_model::sync::ZimbraMessagePage {
                    folder: "2".into(),
                    messages: page.messages,
                    last: true,
                    ..Default::default()
                }],
                ..Default::default()
            };
            assert!(result.check().is_ok(), "{field} must not make a successfully read page unusable");
        }
    }
}

#[test]
fn a_probe_refuses_an_id_that_is_not_digits() {
    // Cleaning must never make a value pass for another id: "2 5 7" is not "257".
    let token = Secret::new("t");
    for id in ["25x7", "2 5 7", " 257", "257\n", "", "٢٥٧"] {
        assert!(items::probe("https://zimbra.example.org", &token, id).is_err(), "{id}");
    }
    assert!(items::probe("https://zimbra.example.org", &token, "257").is_ok());
}

#[test]
fn a_probe_accepts_only_the_one_message_it_requested() {
    let message = json!({"id":"257","l":"2"});
    for (messages, requested, accepted) in [
        (json!([message.clone()]), "257", true),
        (json!([message.clone()]), "258", false),
        (json!([message.clone(), message]), "257", false),
        (json!([]), "257", false),
    ] {
        let response = answer(200, &json!({"Body":{"GetMsgResponse":{"m":messages}}}));
        let result = items::probe_answer(&response, requested);
        if accepted {
            assert_eq!(result.unwrap().unwrap().server.id, requested);
        } else {
            assert_eq!(result, Err(Error::Unreadable));
        }
    }
    for code in ["mail.NO_SUCH_MSG", "mail.NO_SUCH_FOLDER"] {
        let response = answer(500, &json!({"Body":{"Fault":{"Detail":{"Error":{"Code":code}}}}}));
        let result = items::probe_answer(&response, "257");
        if code == "mail.NO_SUCH_MSG" {
            assert_eq!(result, Ok(None));
        } else {
            assert!(result.is_err());
        }
    }
}

#[test]
fn typed_deletions_without_messages_keep_their_token() {
    for group in [
        json!({"ids": "500", "cn": [{"ids": "500"}]}),
        json!({"ids": "500", "folder": [{"ids": "500"}]}),
        json!({"ids": "500", "appt": {"ids": "500"}}),
        json!({"cn": [{"ids": "500"}], "m": []}),
    ] {
        let response = answer(200, &json!({"Body": {"SyncResponse": {"token": "5", "deleted": [group]}}}));
        let delta = delta::answer(&response, false).unwrap();
        assert_eq!(delta.token, "5");
        assert!(delta.deleted.is_empty());
    }
    let response = answer(200, &json!({"Body": {"SyncResponse": {"token": "5", "deleted": [{"cn": [{"ids": "500"}], "m": [{"ids": "501"}]}]}}}));
    assert_eq!(delta::answer(&response, false).unwrap().deleted, ["501"]);
    for group in [
        json!({}),
        json!({"cn": null}),
        json!({"cn": 7}),
        json!({"cn": []}),
        json!({"cn": [{}]}),
        json!({"cn": [{"ids": "bad"}]}),
        json!({"cn": [{"ids": "500"}], "m": null}),
    ] {
        let response = answer(200, &json!({"Body": {"SyncResponse": {"token": "5", "deleted": [group]}}}));
        assert_eq!(delta::answer(&response, false), Err(Error::Unreadable));
    }
    let ids = (1..=1001).map(|id| id.to_string()).collect::<Vec<_>>().join(",");
    let response = answer(200, &json!({"Body": {"SyncResponse": {"token": "5", "deleted": [{"cn": [{"ids": ids}]}]}}}));
    assert_eq!(delta::answer(&response, false), Err(Error::Unreadable));
}

#[test]
fn ambiguous_values_and_incomplete_pages_are_refused() {
    for id in ["a:b:c", ":2", "a b:2", "a:0"] {
        let response = answer(200, &json!({"Body": {"GetFolderResponse": {"folder": {"id": "1", "folder": [{"id": id, "name": "bad"}]}}}}));
        assert!(!folders::answer(&response).unwrap().complete);
    }
    for token in [json!(true), json!(["5", "6"]), json!({"_content": "1\u{0}2"})] {
        let response = answer(200, &json!({"Body": {"SyncResponse": {"token": token}}}));
        assert_eq!(delta::answer(&response, false), Err(Error::Unreadable));
    }
    for more in [json!("garbage"), json!(2), json!(-1), json!(null), json!("tr\u{0}ue")] {
        let search = answer(200, &json!({"Body": {"SearchResponse": {"more": more}}}));
        assert_eq!(items::answer(&search), Err(Error::Unreadable));
        let sync = answer(200, &json!({"Body": {"SyncResponse": {"token": "5", "more": more}}}));
        assert_eq!(delta::answer(&sync, false), Err(Error::Unreadable));
    }
    for location in [json!(null), json!({}), json!("a:b:c"), json!("2\u{0}3")] {
        let response = answer(200, &json!({"Body": {"SearchResponse": {"m": [{"id": "257", "l": location}]}}}));
        assert_eq!(items::answer(&response), Err(Error::Unreadable));
    }
    let messages: Vec<_> = (0..=items::PAGE).map(|n| json!({"id": (257 + n).to_string(), "l": "2"})).collect();
    let response = answer(200, &json!({"Body": {"SearchResponse": {"m": messages}}}));
    assert_eq!(items::answer(&response), Err(Error::Unreadable));
    for entry in [json!(null), json!({}), json!({"m": null}), json!({"m": 7})] {
        let response = answer(200, &json!({"Body": {"SyncResponse": {"token": "5", "deleted": [entry]}}}));
        assert_eq!(delta::answer(&response, false), Err(Error::Unreadable));
    }
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
fn leading_and_trailing_controls_are_not_trimmed_into_valid_ids() {
    for control in ['\n', '\r', '\t', '\0', '\u{b}', '\u{85}'] {
        for id in ["257", "other:257"] {
            for raw in [format!("{control}{id}"), format!("{id}{control}")] {
                for value in [json!(raw), json!({"_content": raw}), json!([raw])] {
                    assert!(crate::json::own_id(Some(&value)).is_none(), "{value:?}");
                    assert!(!crate::json::foreign_id(Some(&value)), "{value:?}");
                }
                let changed = answer(200, &json!({"Body":{"SyncResponse":{"token":"5","m":[{"id":raw}]}}}));
                assert_eq!(delta::answer(&changed, false), Err(Error::Unreadable), "{raw:?}");
                let hit = answer(200, &json!({"Body":{"SearchResponse":{"m":[{"id":raw,"l":"2"}]}}}));
                assert_eq!(items::answer(&hit), Err(Error::Unreadable), "{raw:?}");
            }
        }
    }
}

#[test]
fn ids_of_other_mailboxes_and_odd_ids_are_passed_over() {
    // A hit whose id is no id at all can't be told from a cached message the listing left out:
    // the whole answer is refused, like an item that names no folder.
    // (Each by itself and in a folder that is one, so that it is the id that is refused.)
    for id in ["0", "-4", "12345678901"] {
        let odd = answer(200, &json!({"Body": {"SearchResponse": {"m": [{"id": id, "l": "2"}]}}}));
        assert_eq!(items::answer(&odd), Err(Error::Unreadable), "{id}");
    }
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

#[test]
fn a_sign_in_refused_by_status_alone_is_a_sign_in_refused() {
    // A proxy or a single sign-on in front of the service answers 401 or 403 with a page, not a
    // fault: the owner is asked to sign in again, as for the fault.
    for status in [401u16, 403] {
        let page = HttpResponse::new(status, b"<html>Sign in</html>".to_vec());
        assert_eq!(items::answer(&page), Err(Error::AuthRefused));
        assert_eq!(folders::answer(&answer(status, &json!({"Body": {}}))), Err(Error::AuthRefused));
    }
    // A fault the server wrote keeps its own word.
    let denied = answer(403, &json!({"Body": {"Fault": {"Detail": {"Error": {"Code": "service.PERM_DENIED"}}}}}));
    assert_eq!(items::answer(&denied), Err(Error::Fault(Fault::PermissionDenied)));
}

#[test]
fn a_page_in_place_of_a_message_is_no_message() {
    // What a server or a proxy answers with 200 when it doesn't take the sign-in: taken for the
    // message, it would be kept as its text and the message never asked for again.
    // Sent as a page, it is the sign-in that wasn't taken, whatever it holds.
    let html = b"<!DOCTYPE html><html><body>Sign in</body></html>".to_vec();
    for kind in ["text/html", "Text/HTML; charset=utf-8", " application/xhtml+xml"] {
        assert_eq!(fetch::answer(&HttpResponse::new(200, html.clone()).with_field("Content-Type", kind)), Err(Error::AuthRefused), "{kind}");
        let typed = HttpResponse::new(200, b"Subject: x\r\n\r\nbody".to_vec()).with_field("Content-Type", kind);
        assert_eq!(fetch::answer(&typed), Err(Error::AuthRefused), "{kind}");
    }
    // One that only begins as a page does, sent as a message or as nothing, is no message
    // either, but says nothing of the sign-in: a stored message that is broken this way would
    // otherwise ask the owner to sign in again on every run.
    for kind in [None, Some("message/rfc822"), Some("text/plain")] {
        for body in [html.as_slice(), b"  \r\n<HTML lang=en>".as_slice()] {
            let response = HttpResponse::new(200, body.to_vec());
            let response = match kind {
                Some(kind) => response.with_field("Content-Type", kind),
                None => response,
            };
            assert_eq!(fetch::answer(&response), Err(Error::Unreadable), "{kind:?}");
        }
    }
    // A message whose own text is HTML is a message, whatever type the service sends it as.
    let mail = b"Subject: x\r\nContent-Type: text/html\r\n\r\n<html>hello</html>".to_vec();
    for kind in [Some("message/rfc822"), Some("text/plain"), None] {
        let response = HttpResponse::new(200, mail.clone());
        let response = match kind {
            Some(kind) => response.with_field("Content-Type", kind),
            None => response,
        };
        assert_eq!(fetch::answer(&response), Ok(mail.clone()), "{kind:?}");
    }
}

#[test]
fn a_deleted_note_is_a_typed_deletion_that_removes_no_mail() {
    // `note` (Zimbra's `MailConstants.E_NOTE`) is one of the types `SyncResponse`'s `deleted`
    // breaks down by: unread, a delta that deletes a note would never be gone on from.
    let only = answer(200, &json!({"Body": {"SyncResponse": {"token": "6", "deleted": [{"ids": "500", "note": [{"ids": "500"}]}]}}}));
    let delta = delta::answer(&only, false).unwrap();
    assert!(delta.deleted.is_empty() && delta.token == "6");
    let odd = answer(200, &json!({"Body": {"SyncResponse": {"token": "6", "deleted": [{"ids": "500", "note": [{"ids": "5 00"}]}]}}}));
    assert_eq!(delta::answer(&odd, false), Err(Error::Unreadable));
}

#[test]
fn a_probe_with_people_in_no_list_shape_is_refused() {
    for people in [json!("unreadable"), json!(7), json!(true)] {
        let one = answer(200, &json!({"Body": {"GetMsgResponse": {"m": [{"id": "257", "l": "2", "e": people}]}}}));
        assert_eq!(items::probe_answer(&one, "257"), Err(Error::Unreadable));
    }
    let fine = answer(200, &json!({"Body": {"GetMsgResponse": {"m": [{"id": "257", "l": "2", "e": {"t": "t", "a": "bob@example.org"}}]}}}));
    assert_eq!(items::probe_answer(&fine, "257").unwrap().unwrap().to.len(), 1);
}

#[test]
fn an_address_that_can_not_be_kept_whole_is_left_out_and_said() {
    // Cut to size or cleaned it would be another address: left out, the list says it is cut.
    let long = format!("{}@example.org", "a".repeat(16 * 1024));
    for odd in [long, "bob\u{0}@example.org".to_string()] {
        let m = items::message(&json!({"id": "300", "l": "2", "e": [{"t": "t", "a": odd}, {"t": "t", "a": "ada@example.org"}]})).unwrap();
        assert_eq!(m.to.len(), 1);
        assert!(m.cut);
    }
}

#[test]
fn a_list_of_more_ids_than_asked_for_is_no_list() {
    assert_eq!(crate::json::ids(Some(&json!("1,2")), 2), Some(vec!["1".to_string(), "2".to_string()]));
    assert_eq!(crate::json::ids(Some(&json!("1,2")), 1), None);
    // Ids of another mailbox don't count; what follows the last one taken is still read.
    assert_eq!(crate::json::ids(Some(&json!("1,x:9")), 1), Some(vec!["1".to_string()]));
    assert_eq!(crate::json::ids(Some(&json!("1,bad id")), 1), None);
}
