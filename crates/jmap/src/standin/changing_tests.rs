// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in changing mail, held to the standard: `Email/set`. Every message and address is invented.

use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::connection::tests::any_json;
use crate::test_support::made_secret;

pub(super) const API: &str = "https://jmap.standin.example/jmap/api";
pub(super) const ME: &str = "ada@example.org";

pub(super) fn bearer() -> String {
    format!("Bearer {}", made_secret(TOKEN))
}

pub(super) fn mail(from: &str, to: &str) -> Vec<u8> {
    format!(
        "From: {from}
To: {to}
Cc: carol@example.org
Subject: hello
Message-ID: <1@example.org>

Hi.
"
    )
    .into_bytes()
}

struct Ids {
    inbox: String,
    sent: String,
    first: String,
    second: String,
}

/// A stand-in with an Inbox of two emails and an empty Sent.
fn filled() -> (StandIn, Ids) {
    let stand_in = StandIn::new();
    let ids = stand_in.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        let sent = d.mailbox("Sent", None, Some("sent"));
        let first = d.deliver(&inbox, &mail("zoe@example.org", ME), &["$seen", "$custom"], "2026-10-14T09:00:00Z");
        let second = d.deliver(&inbox, &mail("zoe@example.org", ME), &[], "2026-10-15T06:00:00Z");
        Ids { inbox, sent, first, second }
    });
    (stand_in, ids)
}

/// Method calls as the API has them, naming the capabilities in `using`: the status and the answer.
pub(super) fn raw_using(stand_in: &StandIn, using: Value, calls: Value) -> (u16, Value) {
    let body = serde_json::to_vec(&json!({"using": using, "methodCalls": calls})).unwrap();
    let answer = stand_in.answer("POST", API, &[("Content-Type", "application/json"), ("Authorization", &bearer())], &body).unwrap();
    (answer.status(), serde_json::from_slice(answer.body()).unwrap_or(Value::Null))
}

/// The answer to one call: its result, or the `type` of its error.
fn one(stand_in: &StandIn, method: &str, mut args: Value) -> std::result::Result<Value, String> {
    args["accountId"] = json!(ACCOUNT);
    let (status, body) = raw_using(stand_in, json!([CORE, MAIL]), json!([[method, args, "c0"]]));
    assert_eq!(status, 200, "{body}");
    let answer = body["methodResponses"][0].clone();
    if answer[0] == "error" { Err(answer[1]["type"].as_str().unwrap().to_string()) } else { Ok(answer[1].clone()) }
}

pub(super) fn keywords(stand_in: &StandIn, email: &str) -> Vec<String> {
    stand_in.with(|d| d.email(email).map(|e| e.keywords.iter().cloned().collect()).unwrap_or_default())
}

#[test]
fn what_changes_mail_needs_the_capability_of_mail_named() {
    let (stand_in, _) = filled();
    let (_, body) = raw_using(&stand_in, json!([CORE]), json!([["Email/set", {"accountId": ACCOUNT}, "c0"]]));
    assert_eq!(body["methodResponses"][0][1]["type"], "unknownMethod");
    let (_, body) = raw_using(&stand_in, json!([CORE, MAIL]), json!([["Email/set", {"accountId": ACCOUNT}, "c0"]]));
    assert_eq!(body["methodResponses"][0][0], "Email/set");
}

#[test]
fn an_email_is_patched_moved_and_destroyed_as_the_standard_has_it() {
    let (stand_in, ids) = filled();
    let (first, second) = (ids.first.clone(), ids.second.clone());
    // Keywords one by one: the others stay.
    let answer = one(
        &stand_in,
        "Email/set",
        json!({"update": {first.clone(): {"keywords/$seen": null, "keywords/$flagged": true, "keywords/$answered": null}}}),
    )
    .unwrap();
    assert_eq!((answer["updated"].clone(), answer["notUpdated"].clone()), (json!({first.clone(): null}), Value::Null));
    assert_ne!(answer["oldState"], answer["newState"]);
    assert_eq!(keywords(&stand_in, &first), ["$custom", "$flagged"]);
    // The whole set at once.
    one(&stand_in, "Email/set", json!({"update": {first.clone(): {"keywords": {"$seen": true}}}})).unwrap();
    assert_eq!(keywords(&stand_in, &first), ["$seen"]);
    // A move: out of one mailbox, into the other. And into both, on a server that lets an email lie in several.
    one(
        &stand_in,
        "Email/set",
        json!({"update": {first.clone(): {format!("mailboxIds/{}", ids.inbox): null, format!("mailboxIds/{}", ids.sent): true}}}),
    )
    .unwrap();
    assert_eq!(stand_in.with(|d| d.email(&first).unwrap().mailboxes.iter().cloned().collect::<Vec<_>>()), std::slice::from_ref(&ids.sent));
    one(&stand_in, "Email/set", json!({"update": {first.clone(): {"mailboxIds": {ids.inbox.clone(): true, ids.sent.clone(): true}}}})).unwrap();
    assert_eq!(stand_in.with(|d| d.email(&first).unwrap().mailboxes.len()), 2);

    // What is wrong is refused for the one email, each for its reason, and nothing of it is done.
    let refused = |patch: Value| {
        let answer = one(&stand_in, "Email/set", json!({"update": {second.clone(): patch}})).unwrap();
        assert_eq!(answer["updated"], Value::Null, "{answer}");
        answer["notUpdated"][second.as_str()]["type"].as_str().unwrap().to_string()
    };
    assert_eq!(refused(json!({"keywords/$seen": "yes"})), "invalidPatch");
    assert_eq!(refused(json!({"keywords/$seen": false})), "invalidPatch");
    assert_eq!(refused(json!({"keywords/a b": true})), "invalidPatch");
    assert_eq!(refused(json!({"subject": "new"})), "invalidProperties");
    assert_eq!(refused(json!({"keywords/$seen/more": true})), "invalidProperties");
    assert_eq!(refused(json!({"mailboxIds/M999": true})), "invalidProperties");
    assert_eq!(refused(json!({format!("mailboxIds/{}", ids.inbox): null})), "invalidProperties");
    assert_eq!(refused(json!({"keywords": ["$seen"]})), "invalidProperties");
    assert_eq!(refused(json!("patch")), "invalidPatch");
    assert!(keywords(&stand_in, &second).is_empty());
    // An email it has not, for an update and for a destroy; one a test makes it refuse.
    stand_in.with(|d| d.refused_changes.insert(second.clone(), ("forbidden".into(), "not this one".into())));
    let answer = one(&stand_in, "Email/set", json!({"update": {"E999": {"keywords/$seen": true}, second.clone(): {"keywords/$seen": true}}, "destroy": ["E998", second.clone(), first.clone()]})).unwrap();
    assert_eq!(
        answer["notUpdated"],
        json!({"E999": {"type": "notFound", "description": "no such email"}, second.clone(): {"type": "forbidden", "description": "not this one"}})
    );
    assert_eq!(answer["notDestroyed"]["E998"]["type"], "notFound");
    assert_eq!(answer["notDestroyed"][second.as_str()]["type"], "forbidden");
    assert_eq!(answer["destroyed"], json!([first.clone()]));
    assert!(stand_in.with(|d| d.email(&first).is_none() && d.email(&second).is_some()));

    // The call as a whole: nothing is made here, a state that is another, too many at once, wrong types.
    let made = one(&stand_in, "Email/set", json!({"create": {"k1": {"subject": "x"}}})).unwrap();
    assert_eq!((made["created"].clone(), made["notCreated"]["k1"]["type"].as_str()), (Value::Null, Some("forbidden")));
    assert_eq!(one(&stand_in, "Email/set", json!({"ifInState": "s0"})), Err("stateMismatch".into()));
    let state = stand_in.with(|d| d.state());
    assert!(one(&stand_in, "Email/set", json!({"ifInState": state})).is_ok());
    stand_in.with(|d| d.max_set = 2);
    assert_eq!(one(&stand_in, "Email/set", json!({"update": {"E1": {}, "E2": {}}, "destroy": ["E3"]})), Err("requestTooLarge".into()));
    for wrong in [
        json!({"update": []}),
        json!({"destroy": {}}),
        json!({"destroy": ["a b"]}),
        json!({"create": 7}),
        json!({"ifInState": 7}),
        json!({"upd": {}}),
    ] {
        assert_eq!(one(&stand_in, "Email/set", wrong.clone()), Err("invalidArguments".into()), "{wrong}");
    }
}

#[test]
fn a_move_gives_a_new_id_on_a_server_that_names_mail_by_where_it_lies() {
    let (stand_in, ids) = filled();
    stand_in.with(|d| d.profile = Profile::by_place());
    let moving = json!({format!("mailboxIds/{}", ids.inbox): null, format!("mailboxIds/{}", ids.sent): true});
    let answer = one(&stand_in, "Email/set", json!({"update": {ids.first.clone(): moving}})).unwrap();
    let new = answer["updated"][ids.first.as_str()]["id"].as_str().unwrap().to_string();
    assert!(new != ids.first && stand_in.with(|d| d.email(&ids.first).is_none() && d.email(&new).is_some_and(|e| e.mailboxes.contains(&ids.sent))));
    // A change that moves nothing keeps the id, and an email lies in one mailbox there.
    let answer = one(&stand_in, "Email/set", json!({"update": {new.clone(): {"keywords/$seen": true}}})).unwrap();
    assert_eq!(answer["updated"], json!({new.clone(): null}));
    let answer = one(&stand_in, "Email/set", json!({"update": {new.clone(): {format!("mailboxIds/{}", ids.inbox): true}}})).unwrap();
    assert_eq!(answer["notUpdated"][new.as_str()]["type"], "tooManyMailboxes");
}

#[test]
fn a_server_that_passes_over_what_it_does_not_know_says_nothing_of_it() {
    let (stand_in, ids) = filled();
    stand_in.with(|d| d.passes_over_unknown = true);
    let answer = one(
        &stand_in,
        "Email/set",
        json!({"update": {"E999": {"keywords/$seen": true}, ids.first.clone(): {"keywords/$flagged": true}}, "destroy": ["E998", ids.second.clone()]}),
    )
    .unwrap();
    assert_eq!((answer["updated"].clone(), answer["destroyed"].clone()), (json!({ids.first.clone(): null}), json!([ids.second.clone()])));
    assert_eq!((answer["notUpdated"].clone(), answer["notDestroyed"].clone()), (Value::Null, Value::Null));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    // Whatever it is sent as the arguments of what changes mail, the stand-in answers, and never panics.
    #[test]
    fn it_answers_whatever_it_is_asked_to_change(args in any_json(), patch in any_json()) {
        let (stand_in, ids) = filled();
        let shaped = [json!({"accountId": ACCOUNT, "update": {ids.first.clone(): patch.clone()}, "destroy": patch}), args];
        for args in shaped {
            let (status, body) = raw_using(&stand_in, json!([CORE, MAIL]), json!([["Email/set", args.clone(), "c0"], ["Email/set", args.clone(), "c1"]]));
            prop_assert_eq!(status, 200);
            prop_assert!(body["methodResponses"].as_array().is_some_and(|responses| responses.len() >= 2));
        }
    }
}
