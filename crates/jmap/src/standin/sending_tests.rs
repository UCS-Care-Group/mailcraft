// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in sending mail, held to the standard: the upload, `Email/import`, `Identity/get` and
//! `EmailSubmission/set` (`Email/set`: `changing_tests.rs`). Every message and address is invented; nothing leaves the process.

use proptest::prelude::*;
use serde_json::json;

use super::changing_tests::{ME, bearer, keywords, mail, raw_using};
use super::*;
use crate::connection::tests::any_json;

const UPLOAD: &str = "https://jmap.standin.example/jmap/upload/A1/";

struct Ids {
    inbox: String,
    sent: String,
    first: String,
    second: String,
    me: String,
}

/// A stand-in with an Inbox of two emails and an empty Sent, that sends as `ada@example.org`.
fn filled() -> (StandIn, Ids) {
    let stand_in = StandIn::new();
    let ids = stand_in.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        let sent = d.mailbox("Sent", None, Some("sent"));
        let first = d.deliver(&inbox, &mail("zoe@example.org", ME), &["$seen", "$custom"], "2026-10-14T09:00:00Z");
        let second = d.deliver(&inbox, &mail("zoe@example.org", ME), &[], "2026-10-15T06:00:00Z");
        let me = d.identity(ME);
        Ids { inbox, sent, first, second, me }
    });
    (stand_in, ids)
}

/// Method calls as the API has them, naming every capability: the status and the answer.
fn raw(stand_in: &StandIn, calls: Value) -> (u16, Value) {
    raw_using(stand_in, json!([CORE, MAIL, SUBMISSION]), calls)
}

/// The answer to one call: its result, or the `type` of its error.
fn one(stand_in: &StandIn, method: &str, mut args: Value) -> std::result::Result<Value, String> {
    args["accountId"] = json!(ACCOUNT);
    let (status, body) = raw(stand_in, json!([[method, args, "c0"]]));
    assert_eq!(status, 200, "{body}");
    let answer = body["methodResponses"][0].clone();
    if answer[0] == "error" { Err(answer[1]["type"].as_str().unwrap().to_string()) } else { Ok(answer[1].clone()) }
}

fn upload(stand_in: &StandIn, octets: &[u8]) -> (u16, Value) {
    let answer = stand_in.answer("POST", UPLOAD, &[("Content-Type", "message/rfc822"), ("Authorization", &bearer())], octets).unwrap();
    assert_eq!(answer.content_type(), Some(if answer.status() == 201 { "application/json" } else { "application/problem+json" }));
    (answer.status(), serde_json::from_slice(answer.body()).unwrap_or(Value::Null))
}

#[test]
fn the_session_offers_sending_unless_a_test_takes_it_away() {
    let (stand_in, _) = filled();
    let session = |stand_in: &StandIn| {
        let answer = stand_in.answer("GET", SESSION, &[("Authorization", &bearer())], b"").unwrap();
        serde_json::from_slice::<Value>(answer.body()).unwrap()
    };
    let offered = session(&stand_in);
    assert!(offered["capabilities"][SUBMISSION].is_object() && offered["accounts"][ACCOUNT]["accountCapabilities"][SUBMISSION].is_object());
    assert_eq!(
        (offered["primaryAccounts"][SUBMISSION].as_str(), offered["capabilities"][CORE]["maxObjectsInSet"].as_u64()),
        (Some(ACCOUNT), Some(500))
    );
    // A method of sending needs its capability named.
    let (_, body) = raw_using(&stand_in, json!([CORE, MAIL]), json!([["Identity/get", {"accountId": ACCOUNT}, "c0"]]));
    assert_eq!(body["methodResponses"][0][1]["type"], "unknownMethod");
    let (_, body) = raw_using(&stand_in, json!([CORE, SUBMISSION]), json!([["Email/set", {"accountId": ACCOUNT}, "c0"]]));
    assert_eq!(body["methodResponses"][0][1]["type"], "unknownMethod");

    stand_in.with(|d| d.submission = false);
    let without = session(&stand_in);
    assert!(without["capabilities"].get(SUBMISSION).is_none() && without["primaryAccounts"].get(SUBMISSION).is_none());
    let (status, body) = raw(&stand_in, json!([["Identity/get", {"accountId": ACCOUNT}, "c0"]]));
    assert_eq!((status, body["type"].as_str()), (400, Some("urn:ietf:params:jmap:error:unknownCapability")));
    // (What changes mail is mail's own, and still there.)
    let (_, body) = raw_using(&stand_in, json!([CORE, MAIL]), json!([["Email/set", {"accountId": ACCOUNT}, "c0"]]));
    assert_eq!(body["methodResponses"][0][0], "Email/set");
}

#[test]
fn an_upload_becomes_a_blob_for_the_credentials_it_lets_in() {
    let (stand_in, _) = filled();
    let (status, body) = upload(&stand_in, b"From: a@example.org\r\n\r\nx");
    let blob = body["blobId"].as_str().unwrap().to_string();
    assert_eq!(
        (status, body["accountId"].as_str(), body["type"].as_str(), body["size"].as_u64()),
        (201, Some(ACCOUNT), Some("message/rfc822"), Some(24))
    );
    assert_eq!(stand_in.with(|d| d.uploads.get(&blob).cloned()), Some(b"From: a@example.org\r\n\r\nx".to_vec()));
    assert_eq!(stand_in.seen().last().map(|seen| (seen.method.as_str(), seen.path.as_str(), seen.status)), Some(("POST", "/jmap/upload/A1/", 201)));
    // Nobody else, no other account, no other method, and no more than it takes.
    let refused =
        |url: &str, method: &str, authorization: &str| stand_in.answer(method, url, &[("Authorization", authorization)], b"x").unwrap().status();
    assert_eq!(refused(UPLOAD, "POST", "Bearer other"), 401);
    assert_eq!(refused("https://jmap.standin.example/jmap/upload/A2/", "POST", &bearer()), 404);
    assert_eq!(refused("https://jmap.standin.example/jmap/upload/A1/more", "POST", &bearer()), 404);
    assert_eq!(refused(UPLOAD, "GET", &bearer()), 404);
    stand_in.with(|d| d.max_upload = 10);
    assert_eq!(upload(&stand_in, b"01234567890").0, 413);
    assert_eq!(upload(&stand_in, b"0123456789").0, 201);
    // Trouble on demand, once each, with the seconds to stay away.
    stand_in.with(|d| d.refuse_next_upload.extend([(503, Some(7)), (400, None)]));
    let busy = stand_in.answer("POST", UPLOAD, &[("Authorization", &bearer())], b"x").unwrap();
    assert_eq!((busy.status(), busy.retry_after()), (503, Some("7")));
    assert_eq!((upload(&stand_in, b"x").0, upload(&stand_in, b"x").0), (400, 201));
    // Only so many are kept: the last ones, by when they came and not by how their ids sort.
    let ids: Vec<String> = (0..1_010).map(|_| upload(&stand_in, b"x").1["blobId"].as_str().unwrap().to_string()).collect();
    let kept: Vec<String> = stand_in.with(|d| d.uploads.keys().cloned().collect());
    assert_eq!(kept.len(), 1_000);
    let mut newest = ids[10..].to_vec();
    newest.sort();
    assert_eq!(kept, newest, "the ten oldest of these, and everything before them, are gone");
    assert!(stand_in.with(|d| !d.uploads.contains_key(&blob)));
}

#[test]
fn a_blob_is_imported_as_an_email_where_and_how_the_call_says() {
    let (stand_in, ids) = filled();
    let blob = upload(&stand_in, &mail(ME, "bob@example.org")).1["blobId"].as_str().unwrap().to_string();
    let entry = |mailboxes: Value| json!({"blobId": blob.clone(), "mailboxIds": mailboxes, "keywords": {"$seen": true, "$draft": true}});
    let answer =
        one(&stand_in, "Email/import", json!({"emails": {"k1": entry(json!({ids.sent.clone(): true})), "k2": entry(json!({"M999": true}))}}))
            .unwrap();
    let made = answer["created"]["k1"].clone();
    let id = made["id"].as_str().unwrap().to_string();
    assert_eq!((made["blobId"].as_str(), made["size"].as_u64()), (Some(blob.as_str()), Some(mail(ME, "bob@example.org").len() as u64)));
    assert_eq!(answer["notCreated"]["k2"]["type"], "invalidProperties");
    stand_in.with(|d| {
        let email = d.email(&id).unwrap();
        assert_eq!((email.mailboxes.iter().collect::<Vec<_>>(), email.received.as_str()), (vec![&ids.sent], d.now.as_str()));
        assert_eq!(email.keywords.iter().map(String::as_str).collect::<Vec<_>>(), ["$draft", "$seen"]);
    });
    // It reads like any other email, and its blob downloads.
    let got = one(&stand_in, "Email/get", json!({"ids": [id.clone()], "properties": ["subject", "mailboxIds"]})).unwrap();
    assert_eq!(got["list"][0]["subject"], "hello");
    // What an import needs, or it is refused for that entry.
    let wrong = [
        json!({"mailboxIds": {ids.sent.clone(): true}}),
        json!({"blobId": "B999", "mailboxIds": {ids.sent.clone(): true}}),
        json!({"blobId": blob.clone()}),
        json!({"blobId": blob.clone(), "mailboxIds": {}}),
        json!({"blobId": blob.clone(), "mailboxIds": {ids.sent.clone(): false}}),
        json!({"blobId": blob.clone(), "mailboxIds": {ids.sent.clone(): true}, "keywords": ["$seen"]}),
        json!({"blobId": blob.clone(), "mailboxIds": {ids.sent.clone(): true}, "receivedAt": "yesterday"}),
        json!({"blobId": blob.clone(), "mailboxIds": {ids.sent.clone(): true}, "subject": "x"}),
        json!("blob"),
    ];
    for entry in wrong {
        let answer = one(&stand_in, "Email/import", json!({"emails": {"k1": entry.clone()}})).unwrap();
        assert_eq!((answer["created"].clone(), answer["notCreated"]["k1"]["type"].as_str()), (Value::Null, Some("invalidProperties")), "{entry}");
    }
    let dated = json!({"blobId": blob.clone(), "mailboxIds": {ids.inbox.clone(): true}, "receivedAt": "2026-10-16T08:00:00Z"});
    let answer = one(&stand_in, "Email/import", json!({"emails": {"k1": dated}})).unwrap();
    let id = answer["created"]["k1"]["id"].as_str().unwrap().to_string();
    assert_eq!(stand_in.with(|d| d.email(&id).map(|e| (e.received.clone(), e.keywords.len()))), Some(("2026-10-16T08:00:00Z".into(), 0)));
    for call in [json!({}), json!({"emails": []}), json!({"emails": {}, "ifInState": "s0"}), json!({"emails": {}, "other": 1})] {
        assert!(one(&stand_in, "Email/import", call.clone()).is_err(), "{call}");
    }
}

#[test]
fn the_identities_are_listed_whole_or_by_id() {
    let (stand_in, ids) = filled();
    let all = one(&stand_in, "Identity/get", json!({})).unwrap();
    let emails: Vec<&str> = all["list"].as_array().unwrap().iter().map(|identity| identity["email"].as_str().unwrap()).collect();
    assert_eq!(emails, [USER, ME]);
    assert_eq!(
        all["list"][1],
        json!({"id": ids.me.clone(), "name": "", "email": ME, "replyTo": null, "bcc": null, "textSignature": "", "htmlSignature": "", "mayDelete": false})
    );
    let some = one(&stand_in, "Identity/get", json!({"ids": [ids.me.clone(), "I999"], "properties": ["email"]})).unwrap();
    assert_eq!((some["list"].clone(), some["notFound"].clone()), (json!([{"id": ids.me.clone(), "email": ME}]), json!(["I999"])));
    assert_eq!(one(&stand_in, "Identity/get", json!({"properties": ["password"]})), Err("invalidArguments".into()));
}

/// An email of the account's own, imported into Sent as a draft: its id.
fn imported(stand_in: &StandIn, ids: &Ids, from: &str, to: &str) -> String {
    let blob = upload(stand_in, &mail(from, to)).1["blobId"].as_str().unwrap().to_string();
    let entry = json!({"blobId": blob, "mailboxIds": {ids.sent.clone(): true}, "keywords": {"$draft": true}});
    one(stand_in, "Email/import", json!({"emails": {"k1": entry}})).unwrap()["created"]["k1"]["id"].as_str().unwrap().to_string()
}

#[test]
fn a_send_is_an_import_and_a_submission_in_one_request_and_what_was_submitted_is_kept() {
    let (stand_in, ids) = filled();
    let blob = upload(&stand_in, &mail(ME, "bob@example.org")).1["blobId"].as_str().unwrap().to_string();
    let envelope = json!({"mailFrom": {"email": ME, "parameters": null}, "rcptTo": [{"email": "bob@example.org"}, {"email": "erin@example.org", "parameters": null}]});
    let (status, body) = raw(
        &stand_in,
        json!([
            ["Email/import", {"accountId": ACCOUNT, "emails": {"m": {"blobId": blob, "mailboxIds": {ids.sent.clone(): true}, "keywords": {"$seen": true, "$draft": true}}}}, "c0"],
            ["EmailSubmission/set", {
                "accountId": ACCOUNT,
                "create": {"s": {"emailId": "#m", "identityId": ids.me.clone(), "envelope": envelope}},
                "onSuccessUpdateEmail": {"#s": {"keywords/$draft": null}},
            }, "c1"],
        ]),
    );
    assert_eq!(status, 200, "{body}");
    let responses = body["methodResponses"].as_array().unwrap();
    // Three responses: the import, the submission, and the `Email/set` the submission did besides, as its own call.
    assert_eq!(
        responses.iter().map(|r| (r[0].as_str().unwrap(), r[2].as_str().unwrap())).collect::<Vec<_>>(),
        [("Email/import", "c0"), ("EmailSubmission/set", "c1"), ("Email/set", "c1")]
    );
    let email = responses[0][1]["created"]["m"]["id"].as_str().unwrap().to_string();
    let submission = responses[1][1]["created"]["s"]["id"].as_str().unwrap().to_string();
    assert_eq!(responses[2][1]["updated"], json!({email.clone(): null}));
    assert_eq!(keywords(&stand_in, &email), ["$seen"]);
    stand_in.with(|d| {
        assert_eq!(d.submitted.len(), 1);
        let sent = &d.submitted[0];
        assert_eq!(
            (sent.id.as_str(), sent.email.as_str(), sent.identity.as_str(), sent.mail_from.as_str()),
            (submission.as_str(), email.as_str(), ids.me.as_str(), ME)
        );
        assert_eq!(sent.rcpt_to, ["bob@example.org", "erin@example.org"]);
        assert_eq!(sent.raw, mail(ME, "bob@example.org"));
        let shown = format!("{sent:?}");
        assert!(!shown.contains("example.org") && !shown.contains("hello"), "{shown}");
    });
    assert_eq!(stand_in.calls(), ["Email/import", "EmailSubmission/set"]);

    // Without an envelope it is made of the message's own fields; and the email may be taken away after.
    let other = imported(&stand_in, &ids, ME, "bob@example.org");
    let answer = raw(&stand_in, json!([["EmailSubmission/set", {"accountId": ACCOUNT, "create": {"s": {"emailId": other.clone(), "identityId": ids.me.clone()}}, "onSuccessDestroyEmail": ["#s"]}, "c0"]])).1;
    assert_eq!(answer["methodResponses"][1][1]["destroyed"], json!([other.clone()]));
    stand_in.with(|d| {
        assert_eq!(
            (d.submitted[1].mail_from.as_str(), d.submitted[1].rcpt_to.clone()),
            (ME, vec!["bob@example.org".to_string(), "carol@example.org".to_string()])
        );
        assert!(d.email(&other).is_none());
    });
    // A creation id of another request names nothing.
    let stale = one(&stand_in, "EmailSubmission/set", json!({"create": {"s": {"emailId": "#m", "identityId": ids.me.clone()}}})).unwrap();
    assert_eq!(stale["notCreated"]["s"]["type"], "invalidProperties");
}

#[test]
fn a_submission_is_refused_for_the_reason_the_standard_names() {
    let (stand_in, ids) = filled();
    let own = imported(&stand_in, &ids, ME, "bob@example.org");
    let foreign = imported(&stand_in, &ids, "zoe@example.org", "bob@example.org");
    let whole = stand_in.with(|d| d.identity("*@example.net"));
    let envelope =
        |from: &str, to: &[&str]| json!({"mailFrom": {"email": from}, "rcptTo": to.iter().map(|email| json!({"email": email})).collect::<Vec<_>>()});
    let refused = |submission: Value| {
        let answer = one(&stand_in, "EmailSubmission/set", json!({"create": {"s": submission.clone()}})).unwrap();
        assert_eq!(answer["created"], Value::Null, "{submission}");
        answer["notCreated"]["s"]["type"].as_str().unwrap().to_string()
    };
    let with = |email: &str, identity: &str, envelope: Value| json!({"emailId": email, "identityId": identity, "envelope": envelope});
    assert_eq!(refused(with(&own, &ids.me, envelope("zoe@example.org", &["bob@example.org"]))), "forbiddenMailFrom");
    assert_eq!(refused(with(&foreign, &ids.me, envelope(ME, &["bob@example.org"]))), "forbiddenFrom");
    assert_eq!(refused(with(&own, &whole, envelope("ada@example.net", &["bob@example.org"]))), "forbiddenFrom");
    assert_eq!(refused(with(&own, &ids.me, envelope(ME, &[]))), "noRecipients");
    assert_eq!(refused(with(&own, &ids.me, envelope(ME, &["bob@example.org", "no address", "x"]))), "invalidRecipients");
    assert_eq!(refused(with(&own, "I999", envelope(ME, &["bob@example.org"]))), "invalidProperties");
    assert_eq!(refused(with("E999", &ids.me, envelope(ME, &["bob@example.org"]))), "invalidProperties");
    assert_eq!(refused(with(&own, &ids.me, json!({"mailFrom": ME, "rcptTo": []}))), "invalidProperties");
    assert_eq!(refused(with(&own, &ids.me, json!({"mailFrom": {"email": ME}, "rcptTo": ["bob@example.org"]}))), "invalidProperties");
    assert_eq!(refused(json!({"emailId": own.clone(), "identityId": ids.me.clone(), "sendAt": "2030-01-01T00:00:00Z"})), "invalidProperties");
    assert_eq!(refused(json!("send it")), "invalidProperties");
    stand_in.with(|d| d.max_recipients = 2);
    let many = one(
        &stand_in,
        "EmailSubmission/set",
        json!({"create": {"s": with(&own, &ids.me, envelope(ME, &["a@example.org", "b@example.org", "c@example.org"]))}}),
    )
    .unwrap();
    assert_eq!((many["notCreated"]["s"]["type"].as_str(), many["notCreated"]["s"]["maxRecipients"].as_u64()), (Some("tooManyRecipients"), Some(2)));
    stand_in.with(|d| d.refuse_submission = Some(("forbiddenToSend".into(), "not today".into())));
    let answer = one(
        &stand_in,
        "EmailSubmission/set",
        json!({"create": {"s": with(&own, &ids.me, envelope(ME, &["bob@example.org"]))}, "onSuccessUpdateEmail": {"#s": {"keywords/$draft": null}}}),
    )
    .unwrap();
    assert_eq!(answer["notCreated"]["s"], json!({"type": "forbiddenToSend", "description": "not today"}));
    // Nothing was sent, and the draft of a submission that failed stays one.
    assert!(stand_in.with(|d| d.submitted.is_empty()) && keywords(&stand_in, &own) == ["$draft"]);
    // A submission is made here, and neither changed nor undone; and the call is held to its arguments.
    for wrong in [
        json!({"update": {"S1": {"undoStatus": "canceled"}}}),
        json!({"destroy": ["S1"]}),
        json!({"create": []}),
        json!({"onSuccessDestroyEmail": "#s"}),
        json!({"onSuccessUpdateEmail": []}),
        json!({"sendAll": true}),
    ] {
        assert_eq!(one(&stand_in, "EmailSubmission/set", wrong.clone()), Err("invalidArguments".into()), "{wrong}");
    }
    // An identity for a whole domain sends as any address of it.
    stand_in.with(|d| d.refuse_submission = None);
    let theirs = imported(&stand_in, &ids, "ada@example.net", "bob@example.org");
    let answer =
        one(&stand_in, "EmailSubmission/set", json!({"create": {"s": with(&theirs, &whole, envelope("Ada@Example.NET", &["bob@example.org"]))}}))
            .unwrap();
    assert!(answer["created"]["s"]["id"].is_string(), "{answer}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    // Whatever it is sent as the arguments of what changes mail and sends it, and whatever is uploaded, the
    // stand-in answers, and never panics.
    #[test]
    fn it_answers_whatever_it_is_asked_to_change_or_send(args in any_json(), patch in any_json(), octets in proptest::collection::vec(any::<u8>(), 0..200)) {
        let (stand_in, ids) = filled();
        let (status, body) = upload(&stand_in, &octets);
        prop_assert_eq!(status, 201);
        let blob = body["blobId"].clone();
        let shaped = [
            json!({"accountId": ACCOUNT, "update": {ids.first.clone(): patch.clone()}, "destroy": patch.clone()}),
            json!({"accountId": ACCOUNT, "emails": {"m": patch.clone(), "n": {"blobId": blob, "mailboxIds": {ids.sent.clone(): true}, "keywords": patch.clone()}}}),
            json!({"accountId": ACCOUNT, "create": {"s": patch.clone(), "t": {"emailId": ids.second.clone(), "identityId": ids.me.clone(), "envelope": patch.clone()}}, "onSuccessUpdateEmail": patch.clone(), "onSuccessDestroyEmail": patch}),
            args,
        ];
        for args in shaped {
            for method in ["Email/set", "Email/import", "Identity/get", "EmailSubmission/set"] {
                let (status, body) = raw(&stand_in, json!([[method, args.clone(), "c0"], [method, args.clone(), "c1"]]));
                prop_assert_eq!(status, 200);
                prop_assert!(body["methodResponses"].as_array().is_some_and(|responses| responses.len() >= 2));
            }
        }
    }
}
