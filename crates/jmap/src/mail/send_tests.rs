// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Outbox going out: what is asked of the server for a message and in which order, what its answers are
//! taken for, what is never asked twice, and what a hostile answer makes of it. Over the stand-in and over
//! scripted servers; every message and address is invented, and nothing leaves the process.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::sync::{JmapChange, MAX_SENDS, SendDone, SendOutcome, SendUp};
use mailcraft_model::{Address, Body, Message};
use proptest::prelude::*;
use serde_json::json;

use super::changes_tests::{
    Answer, FAKE_SESSION, FAKE_TOKEN, Fake, ME, Reply, api_reply, asks, calls_of, changes_of, is_upload, json_reply, last, read_on, reading,
    run_fake, run_over, run_stand_in, run_unchecked, sent_of, session_reply, status_reply,
};
use super::send::{
    BAD_RECIPIENT, BAD_SENDER, Filing, MAX_RECIPIENTS, NO_IDENTITY, NO_MAILBOX, NO_RECIPIENTS, NO_SUBMISSION, NOTHING, Settled, TOO_LARGE, TOO_MANY,
    blob_of_size, existing_of, filing_for, filing_of, identity_of, is_address, not_made, recipients, same_octets, settled, written,
};
use super::*;
use crate::connection::tests::any_json;
use crate::standin::{SESSION, StandIn, TOKEN};
use crate::{CAPABILITY_CORE as CORE, CAPABILITY_MAIL as MAIL, CAPABILITY_SUBMISSION as SUBMISSION, HttpRequest, HttpResponse, Method};

/// Wednesday 2026-10-14 16:00 UTC.
const THEN: i64 = 1_791_993_600;

fn people(list: &[&str]) -> Vec<Address> {
    list.iter().map(|email| Address { name: String::new(), email: email.to_string() }).collect()
}

/// A message of the account's, as the composer leaves one in the Outbox.
fn message(to: &[&str], cc: &[&str], bcc: &[&str]) -> Message {
    Message {
        id: 41,
        from: Address::new("Ada", ME),
        to: people(to),
        cc: people(cc),
        bcc: people(bcc),
        subject: "Lunch".into(),
        message_id: "lunch-1@example.org".into(),
        date: THEN,
        body: Body { text: "Shall we?".into(), html: None },
        ..Default::default()
    }
}

fn outbox(id: u64, message: Message) -> SendUp {
    SendUp { message: id, draft: None, mime: Some(Box::new(message)) }
}

/// A message to one person, waiting in the Outbox as the message `id`.
fn simple(id: u64) -> SendUp {
    outbox(id, message(&["bob@example.org"], &[], &[]))
}

fn held(why: &str) -> SendOutcome {
    SendOutcome::Held { lost: false, why: why.into() }
}

fn lost() -> SendOutcome {
    SendOutcome::Held { lost: true, why: String::new() }
}

fn is(message: u64, outcome: SendOutcome) -> SendDone {
    SendDone { message, outcome, draft: None, draft_left: String::new() }
}

/// What a server that sends answers to the calls of a send (and to the others as [`reading`] does).
fn sending(name: &str, args: &Value) -> Answer {
    match name {
        "Identity/get" => Answer::Is(json!({"accountId": "A1", "state": "1", "notFound": [], "list": [{"id": "I1", "email": ME}]})),
        "Email/import" => Answer::Is(json!({"accountId": "A1", "created": {"m": {"id": "E50", "blobId": "B7", "threadId": "T1", "size": 1}}})),
        "EmailSubmission/set" => Answer::Is(json!({"accountId": "A1", "created": {"s": {"id": "S1", "undoStatus": "final"}}})),
        "Email/set" => Answer::Is(json!({"accountId": "A1", "destroyed": args["destroy"].clone()})),
        _ => reading(name, args),
    }
}

fn uploaded() -> Reply {
    json_reply(201, &json!({"accountId": "A1", "blobId": "B7", "type": "message/rfc822", "size": 1}))
}

/// A scripted server that sends: the session, an upload taken, and each call answered by `f`, or as
/// [`sending`] answers it when `f` has nothing to say.
fn sender(f: impl Fn(&str, &Value) -> Option<Answer> + 'static) -> Fake {
    sender_but(move |req, _| {
        (!is_upload(req) && req.method() == Method::Post).then(|| api_reply(req, |name, args| f(name, args).unwrap_or_else(|| sending(name, args))))
    })
}

/// A scripted server that sends, but for the requests `f` answers itself (given the request and how many
/// came before it: the session is the first, then the identities, the roles, the upload and the submission).
fn sender_but(f: impl Fn(&HttpRequest, usize) -> Option<Reply> + 'static) -> Fake {
    Fake::new(move |req, n| {
        if let Some(reply) = f(req, n) {
            return reply;
        }
        match req.method() {
            Method::Get => session_reply(json!({"maxCallsInRequest": 16}), true),
            _ if is_upload(req) => uploaded(),
            _ => api_reply(req, sending),
        }
    })
}

fn send_fake(fake: &Fake, sends: Vec<SendUp>) -> Vec<JmapMailResult> {
    run_fake(fake, &JmapMailRequest { sends, ..asks(FAKE_SESSION) })
}

struct Ids {
    inbox: String,
    sent: String,
    drafts: String,
}

/// A stand-in with an Inbox, a Sent and a Drafts, that sends as the account's address.
fn world() -> (StandIn, Ids) {
    let stand_in = StandIn::new();
    let ids = stand_in.with(|d| {
        d.identity(ME);
        Ids {
            inbox: d.mailbox("Inbox", None, Some("inbox")),
            sent: d.mailbox("Sent", None, Some("sent")),
            drafts: d.mailbox("Drafts", None, Some("drafts")),
        }
    });
    (stand_in, ids)
}

fn send(stand_in: &StandIn, sends: Vec<SendUp>) -> Vec<JmapMailResult> {
    run_stand_in(stand_in, &JmapMailRequest { sends, ..asks(SESSION) })
}

/// The paths of the uploads the stand-in was sent.
fn uploads(stand_in: &StandIn) -> usize {
    stand_in.seen().iter().filter(|seen| seen.path.starts_with("/jmap/upload/")).count()
}

#[test]
fn a_message_is_uploaded_imported_into_sent_and_submitted_with_its_envelope_in_one_request() {
    let fake = sender(|_, _| None);
    let mail =
        message(&["bob@example.org", "carol@example.org"], &["carol@example.org", "dan@example.org"], &["erin@example.org", "bob@EXAMPLE.org", " "]);
    let pieces = send_fake(&fake, vec![outbox(41, mail)]);
    assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)]);
    let end = last(&pieces);
    assert!(end.whole && end.errors.is_empty(), "{end:?}");

    // In order: the identities, the roles, the upload, one request that imports and submits; then the reading.
    let posts = fake.posts();
    assert_eq!(
        posts[..5],
        [vec!["Identity/get"], vec!["Mailbox/get"], vec!["upload"], vec!["Email/import", "EmailSubmission/set"], vec!["Mailbox/get"]]
    );
    assert_eq!(fake.args_of("Identity/get"), [json!({"accountId": "A1", "properties": ["id", "email"]})]);
    assert_eq!(fake.args_of("Mailbox/get")[0], json!({"accountId": "A1", "properties": ["id", "role"]}));

    // The upload: a POST of the message to the session's upload address, as a message.
    let uploads = fake.uploads();
    assert_eq!(uploads.len(), 1);
    let upload = &uploads[0];
    assert_eq!((upload.method(), upload.url()), (Method::Post, "https://jmap.example.org/jmap/upload/A1/"));
    let fields: Vec<(&str, &str)> = upload.fields().collect();
    assert!(fields.contains(&("Content-Type", "message/rfc822")) && fields.iter().any(|(name, _)| *name == "Authorization"));
    let text = String::from_utf8_lossy(upload.body()).to_string();
    let read = mailcraft_mime_model::parse(upload.body());
    assert_eq!(
        (read.subject.as_str(), read.from.email.as_str(), read.to.len(), read.cc.len(), read.body.text.trim()),
        ("Lunch", ME, 2, 2, "Shall we?")
    );
    // Who is in Bcc is in the envelope only.
    assert!(read.bcc.is_empty() && !text.contains("erin@example.org") && !text.to_ascii_lowercase().contains("bcc"), "{text}");

    // The import: the blob the upload made, into the mailbox whose role is `sent`, read and a draft still.
    assert_eq!(
        fake.args_of("Email/import"),
        [json!({"accountId": "A1", "emails": {"m": {"blobId": "B7", "mailboxIds": {"M2": true}, "keywords": {"$seen": true, "$draft": true}}}})]
    );
    // The submission: the import by its creation id, the identity, and everybody once (To, Cc, Bcc).
    let rcpt = |email: &str| json!({"email": email, "parameters": null});
    assert_eq!(
        fake.args_of("EmailSubmission/set"),
        [json!({
            "accountId": "A1",
            "create": {"s": {
                "emailId": "#m",
                "identityId": "I1",
                "envelope": {
                    "mailFrom": {"email": ME, "parameters": null},
                    "rcptTo": [rcpt("bob@example.org"), rcpt("carol@example.org"), rcpt("dan@example.org"), rcpt("erin@example.org")],
                },
            }},
            "onSuccessUpdateEmail": {"#s": {"keywords/$draft": null}},
        })]
    );
    let sent = fake.sent.borrow();
    let request: Value = serde_json::from_slice(sent[4].body()).unwrap();
    assert_eq!(request["using"], json!([CORE, MAIL, SUBMISSION]));
    let identities: Value = serde_json::from_slice(sent[1].body()).unwrap();
    assert_eq!(identities["using"], json!([CORE, MAIL, SUBMISSION]));
}

#[test]
fn what_is_sent_lies_in_sent_without_its_draft_mark_and_the_window_read_after_shows_it() {
    let (stand_in, ids) = world();
    let mail = message(&["bob@example.org"], &[], &["erin@example.org"]);
    let pieces = send(&stand_in, vec![outbox(41, mail), simple(42)]);
    assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent), is(42, SendOutcome::Sent)]);
    // A piece for each, before the list.
    assert_eq!(pieces.iter().take_while(|piece| piece.listing.is_none()).map(|piece| piece.sent.len()).collect::<Vec<_>>(), [1, 1]);
    stand_in.with(|d| {
        assert_eq!(d.submitted.len(), 2);
        let first = &d.submitted[0];
        assert_eq!((first.mail_from.as_str(), first.rcpt_to.clone()), (ME, vec!["bob@example.org".to_string(), "erin@example.org".to_string()]));
        let email = d.email(&first.email).expect("the email submitted is the one imported");
        assert_eq!(email.mailboxes.iter().collect::<Vec<_>>(), [&ids.sent]);
        assert_eq!(email.keywords.iter().map(String::as_str).collect::<Vec<_>>(), ["$seen"], "the draft mark is off again");
        assert!(!String::from_utf8_lossy(&first.raw).contains("erin@example.org"));
        assert_eq!(d.identities.iter().find(|identity| identity.id == first.identity).map(|identity| identity.email.as_str()), Some(ME));
    });
    // The window of Sent, read afterwards, has both.
    let sent = pieces.iter().flat_map(|p| p.pages.iter()).find(|page| page.folder == ids.sent).unwrap();
    assert_eq!(sent.messages.len(), 2);
    assert!(sent.messages.iter().all(|m| m.subject == "Lunch" && m.draft == Some(false) && m.server.read));
    assert!(last(&pieces).whole);
    // The identities and the roles once, then for each message an upload and one request.
    let calls = stand_in.calls();
    assert_eq!(
        calls[..7],
        ["Identity/get", "Mailbox/get", "Email/import", "EmailSubmission/set", "Email/import", "EmailSubmission/set", "Mailbox/get"]
    );
    assert_eq!(uploads(&stand_in), 2);
    assert!(stand_in.with(|d| d.mailbox_named("Inbox") == Some(ids.inbox.as_str())));
}

#[test]
fn a_server_without_a_sent_mailbox_files_it_in_drafts_and_one_with_neither_holds_it() {
    let stand_in = StandIn::new();
    let drafts = stand_in.with(|d| {
        d.identity(ME);
        d.mailbox("Inbox", None, Some("inbox"));
        d.mailbox("Drafts", None, Some("DRAFTS"))
    });
    let pieces = send(&stand_in, vec![simple(41)]);
    assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)]);
    stand_in.with(|d| {
        let email = d.email(&d.submitted[0].email).unwrap();
        assert!(email.mailboxes.contains(&drafts) && !email.keywords.contains("$draft"));
    });

    let bare = StandIn::new();
    bare.with(|d| {
        d.identity(ME);
        d.mailbox("Inbox", None, Some("inbox"));
    });
    let pieces = send(&bare, vec![simple(41), simple(42)]);
    assert_eq!(sent_of(&pieces), [is(41, held(NO_MAILBOX)), is(42, held(NO_MAILBOX))]);
    assert_eq!((uploads(&bare), bare.with(|d| d.submitted.len())), (0, 0));
    assert!(last(&pieces).whole, "a message held is told with the message, and is nothing wrong with the run");

    // Read from a hostile list: the first of the role, by an id that is one.
    let list = json!({"list": [7, {"id": "a b", "role": "sent"}, {"id": "M5", "role": 9}, {"id": "M6", "role": "drafts"}, {"id": "M7", "role": "Sent"}, {"id": "M8", "role": "sent"}]});
    assert_eq!(filing_of(&list), Some("M7".into()));
    assert_eq!(filing_of(&json!({"list": [{"id": "M6", "role": "drafts"}, {"id": "M1", "role": "inbox"}]})), Some("M6".into()));
    for none in [json!({}), json!({"list": {}}), json!({"list": [{"id": "M1", "role": "inbox"}, {"role": "sent"}]}), json!(null)] {
        assert_eq!(filing_of(&none), None, "{none}");
    }
}

#[test]
fn a_server_that_takes_one_call_to_a_request_is_asked_for_the_import_and_then_the_submission() {
    let (stand_in, ids) = world();
    stand_in.with(|d| d.max_calls = 1);
    let pieces = send(&stand_in, vec![simple(41)]);
    assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)]);
    let requests: Vec<Vec<String>> = stand_in.seen().into_iter().map(|seen| seen.calls).filter(|calls| !calls.is_empty()).take(4).collect();
    assert_eq!(requests, [vec!["Identity/get"], vec!["Mailbox/get"], vec!["Email/import"], vec!["EmailSubmission/set"]]);
    stand_in.with(|d| {
        let email = d.email(&d.submitted[0].email).unwrap();
        assert!(email.mailboxes.contains(&ids.sent) && !email.keywords.contains("$draft"));
        assert_eq!(d.emails.len(), 1);
    });
    assert!(ids.drafts != ids.sent);
}

#[test]
fn without_the_capability_of_sending_every_message_is_held_and_nothing_is_asked_for_it() {
    let (stand_in, _) = world();
    stand_in.with(|d| d.submission = false);
    let pieces = send(&stand_in, vec![simple(41), simple(42)]);
    assert_eq!(sent_of(&pieces), [is(41, held(NO_SUBMISSION)), is(42, held(NO_SUBMISSION))]);
    assert_eq!(uploads(&stand_in), 0);
    assert_eq!(stand_in.calls()[0], "Mailbox/get", "the run goes straight on to read");
    assert!(last(&pieces).whole && read_on(&pieces));
}

#[test]
fn the_identity_is_the_accounts_own_or_one_for_its_whole_domain_and_without_one_the_message_is_held() {
    // The stand-in's own identity is another address.
    let stand_in = StandIn::new();
    stand_in.with(|d| {
        d.mailbox("Inbox", None, Some("inbox"));
        d.mailbox("Sent", None, Some("sent"));
    });
    let pieces = send(&stand_in, vec![simple(41)]);
    assert_eq!(sent_of(&pieces), [is(41, held(NO_IDENTITY))]);
    assert_eq!((uploads(&stand_in), stand_in.with(|d| d.submitted.len())), (0, 0));
    assert_eq!(stand_in.calls()[..2], ["Identity/get", "Mailbox/get"], "the roles are not asked for: the next call is the list");
    assert!(last(&pieces).whole);

    // One for the whole domain will do.
    let whole = stand_in.with(|d| d.identity("*@Example.ORG"));
    let pieces = send(&stand_in, vec![simple(41)]);
    assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)]);
    assert_eq!(stand_in.with(|d| d.submitted[0].identity.clone()), whole);
    // The account's own is taken before it, letters compared without regard to case.
    let own = stand_in.with(|d| d.identity("ADA@Example.org"));
    send(&stand_in, vec![simple(42)]);
    assert_eq!(stand_in.with(|d| d.submitted[1].identity.clone()), own);

    // Read from a hostile list.
    let list = json!({"list": [
        null, 7, {"id": "I1"}, {"email": ME}, {"id": "a b", "email": ME}, {"id": 5, "email": ME}, {"id": "I2", "email": 5},
        {"id": "I3", "email": "*@example.org"}, {"id": "I4", "email": "*@example.org"}, {"id": "I5", "email": "x*@example.org"},
        {"id": "I6", "email": "ada@example.org.evil.example"}, {"id": "I7", "email": "Ada@EXAMPLE.org"},
    ]});
    assert_eq!(identity_of(&list, ME), Some("I7".into()));
    assert_eq!(identity_of(&list, "zoe@example.org"), Some("I3".into()));
    assert_eq!(identity_of(&list, "zoe@example.net"), None);
    let thousands: Vec<Value> = (0..5_000).map(|n| json!({"id": format!("I{n}"), "email": format!("p{n}@example.org")})).collect();
    assert_eq!(identity_of(&json!({"list": thousands}), ME), None);
    for none in [json!(null), json!({}), json!({"list": "I1"}), json!({"list": {"I1": {"id": "I1", "email": ME}}})] {
        assert_eq!(identity_of(&none, ME), None, "{none}");
    }
}

#[test]
fn a_message_larger_than_the_server_takes_is_held_before_anything_is_uploaded() {
    let (stand_in, _) = world();
    stand_in.with(|d| d.max_upload = 600);
    let mut big = message(&["bob@example.org"], &[], &[]);
    big.body.text = "All work and no play. ".repeat(40);
    let pieces = send(&stand_in, vec![outbox(41, big), simple(42)]);
    // The big one is held; the small one, after it, goes.
    assert_eq!(sent_of(&pieces), [is(41, held(TOO_LARGE)), is(42, SendOutcome::Sent)]);
    assert_eq!(uploads(&stand_in), 1, "only the small one was uploaded");
    assert!(last(&pieces).whole);
}

#[test]
fn an_upload_the_server_refuses_holds_the_message_and_one_it_could_not_take_now_leaves_it_waiting() {
    // Refused for good: held, with why, and the next message is tried.
    for (status, why) in [(400, "the server would not take the message: "), (413, TOO_LARGE), (404, "the server would not take the message: ")] {
        let (stand_in, _) = world();
        stand_in.with(|d| d.refuse_next_upload.push_back((status, None)));
        let pieces = send(&stand_in, vec![simple(41), simple(42)]);
        let sent = sent_of(&pieces);
        assert!(
            matches!(&sent[0], SendDone { message: 41, outcome: SendOutcome::Held { lost: false, why: said }, .. } if said.starts_with(why)),
            "{status}: {sent:?}"
        );
        assert_eq!(sent[1], is(42, SendOutcome::Sent), "{status}");
        assert_eq!(stand_in.with(|d| d.submitted.len()), 1, "{status}");
        assert!(last(&pieces).whole, "{status}");
    }
    // The sign-in refused, the server busy: the run ends, and the message is not reported, so it waits.
    let (stand_in, _) = world();
    stand_in.with(|d| d.refuse_next_upload.push_back((401, None)));
    let pieces = send(&stand_in, vec![simple(41), simple(42)]);
    assert!(sent_of(&pieces).is_empty() && last(&pieces).auth_refused && !read_on(&pieces));
    for (status, retry_after, seconds) in [(429, Some(60), 60), (503, None, 30)] {
        let (stand_in, _) = world();
        stand_in.with(|d| d.refuse_next_upload.push_back((status, retry_after)));
        let pieces = send(&stand_in, vec![simple(41), simple(42)]);
        assert!(sent_of(&pieces).is_empty() && !read_on(&pieces), "{status}");
        assert_eq!((last(&pieces).busy_for, uploads(&stand_in), stand_in.with(|d| d.submitted.len())), (Some(seconds), 1, 0), "{status}");
    }
    // No connection, a server error, an answer that names no blob (or one that is no id): nothing was sent,
    // the Outbox waits (the second message is not tried), a line says so, and the run goes on to read.
    let answers: Vec<(&str, Rc<dyn Fn() -> Reply>)> = vec![
        ("no connection", Rc::new(|| Err(TransportError::Unreachable))),
        ("timed out", Rc::new(|| Err(TransportError::TimedOut))),
        ("a server error", Rc::new(|| status_reply(500, None))),
        ("no blob", Rc::new(|| json_reply(201, &json!({"accountId": "A1"})))),
        ("a blob that is no id", Rc::new(|| json_reply(201, &json!({"blobId": "B 7/../x"})))),
        ("no JSON", Rc::new(|| json_reply(201, &json!("B7")))),
    ];
    for (what, answer) in answers {
        let fake = sender_but(move |req, _| is_upload(req).then(|| answer()));
        let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
        assert!(sent_of(&pieces).is_empty(), "{what}");
        assert_eq!(fake.uploads().len(), 1, "{what}");
        assert!(fake.args_of("EmailSubmission/set").is_empty(), "{what}");
        let end = last(&pieces);
        assert!(!end.whole && end.errors.len() == 1 && end.errors[0].starts_with("Couldn't send what waits in the Outbox: "), "{what}: {end:?}");
        assert!(read_on(&pieces), "{what}");
    }
}

#[test]
fn a_submission_the_server_refuses_holds_the_message_with_why_and_takes_the_copy_away_again() {
    let leaky = format!("zoe@example.org is not {ME}; token {TOKEN}; see {SESSION}");
    let cases = [
        ("forbiddenFrom", "the server does not let this account send with that From address"),
        ("forbiddenMailFrom", "the server does not let this account send from its address"),
        ("forbiddenToSend", "the server does not let this account send mail"),
        ("tooManyRecipients", "the message has more recipients than the server takes"),
        ("invalidRecipients", "the server says a recipient's address is not valid"),
        ("noRecipients", NO_RECIPIENTS),
        ("tooLarge", TOO_LARGE),
        ("overQuota", "the account is over its quota on the server"),
        ("invalidEmail", "the server says the message is not one it can send"),
    ];
    for (kind, why) in cases {
        let (stand_in, _) = world();
        stand_in.with(|d| d.refuse_submission = Some((kind.into(), leaky.clone())));
        let pieces = send(&stand_in, vec![simple(41)]);
        assert_eq!(sent_of(&pieces), [is(41, held(why))], "{kind}");
        // The email it imported is gone again, and nothing was submitted.
        assert_eq!(stand_in.with(|d| (d.emails.len(), d.submitted.len())), (0, 0), "{kind}");
        assert_eq!(stand_in.calls()[2..5], ["Email/import", "EmailSubmission/set", "Email/set"], "{kind}");
        assert!(last(&pieces).whole && last(&pieces).errors.is_empty(), "{kind}");
    }
    // An error the standard does not name: the server's words, cleaned of every address and of the secret.
    let (stand_in, _) = world();
    stand_in.with(|d| d.refuse_submission = Some(("somethingElse".into(), leaky.clone())));
    let pieces = send(&stand_in, vec![simple(41)]);
    let sent = sent_of(&pieces);
    let SendOutcome::Held { lost: false, why } = &sent[0].outcome else { panic!("not held: {sent:?}") };
    for hidden in ["zoe@example.org", ME, TOKEN, SESSION, "example.org"] {
        assert!(!why.contains(hidden), "{hidden} shows in {why:?}");
    }
    assert!(why.contains("is not") && why.chars().count() <= 200, "{why:?}");
    // And with no words, its kind.
    stand_in.with(|d| d.refuse_submission = Some(("somethingElse".into(), String::new())));
    assert_eq!(sent_of(&send(&stand_in, vec![simple(41)])), [is(41, held("somethingElse"))]);

    // The server's own rules: more recipients than it takes. The message after it goes all the same.
    let (stand_in, _) = world();
    stand_in.with(|d| d.max_recipients = 1);
    let pieces = send(&stand_in, vec![outbox(41, message(&["bob@example.org", "carol@example.org"], &[], &[])), simple(42)]);
    assert_eq!(sent_of(&pieces), [is(41, held("the message has more recipients than the server takes")), is(42, SendOutcome::Sent)]);
    assert_eq!(stand_in.with(|d| (d.emails.len(), d.submitted.len())), (1, 1));
}

#[test]
fn a_blob_the_server_will_not_import_holds_the_message_and_sends_nothing() {
    let fake = sender(|name, _| match name {
        "Email/import" => Some(Answer::Is(json!({"notCreated": {"m": {"type": "overQuota", "description": format!("no room for {ME}")}}}))),
        "EmailSubmission/set" => Some(Answer::Is(json!({"notCreated": {"s": {"type": "invalidProperties", "properties": ["emailId"]}}}))),
        _ => None,
    });
    let pieces = send_fake(&fake, vec![simple(41)]);
    // (Why the import failed is why: the submission only failed for want of it.)
    let sent = sent_of(&pieces);
    let SendOutcome::Held { lost: false, why } = &sent[0].outcome else { panic!("not held: {sent:?}") };
    assert!(why.starts_with("the server would not take the message: no room for") && !why.contains(ME), "{why:?}");
    assert!(fake.args_of("Email/set").is_empty(), "nothing was imported, so nothing is taken away");

    // The import fails as a call, and the submission that needed it with it.
    let fake = sender(|name, _| match name {
        "Email/import" => Some(Answer::Error("accountReadOnly", format!("{ME} may not write"))),
        "EmailSubmission/set" => Some(Answer::Error("invalidResultReference", String::new())),
        _ => None,
    });
    let pieces = send_fake(&fake, vec![simple(41)]);
    let sent = sent_of(&pieces);
    let SendOutcome::Held { lost: false, why } = &sent[0].outcome else { panic!("not held: {sent:?}") };
    assert!(why.starts_with("the server would not take the message: ") && why.contains("accountReadOnly") && !why.contains(ME), "{why:?}");

    // The server says to try later: the message waits (it is not reported), with a line.
    for (call, taken_away) in [("Email/import", 0), ("EmailSubmission/set", 1)] {
        let fake = sender(move |name, _| match name {
            name if name == call => Some(Answer::Error("serverUnavailable", String::new())),
            "EmailSubmission/set" => Some(Answer::Error("invalidResultReference", String::new())),
            _ => None,
        });
        let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
        assert!(sent_of(&pieces).is_empty(), "{call}");
        assert_eq!(fake.uploads().len(), 1, "{call}: the rest of the Outbox waits too");
        assert!(last(&pieces).errors.len() == 1 && read_on(&pieces), "{call}");
        // (The copy the import made of a message that did not go is taken away again.)
        assert_eq!(fake.args_of("Email/set").len(), taken_away, "{call}");
    }
}

#[test]
fn a_submission_whose_answer_is_lost_is_held_as_one_that_may_have_gone_and_never_asked_again() {
    // The request with the submission is the fifth (the session, the identities, the roles, the upload).
    type Answering = Rc<dyn Fn(&HttpRequest) -> Reply>;
    let answers: Vec<(&str, Answering)> = vec![
        ("no answer in time", Rc::new(|_| Err(TransportError::TimedOut))),
        ("the connection went", Rc::new(|_| Err(TransportError::Other))),
        ("called off by the transport", Rc::new(|_| Err(TransportError::Aborted))),
        ("an answer too large to read", Rc::new(|_| Err(TransportError::BodyTooLarge))),
        ("a server error", Rc::new(|_| status_reply(500, None))),
        ("a gateway that gave up", Rc::new(|_| status_reply(504, None))),
        ("an answer that is no JSON", Rc::new(|_| Ok(HttpResponse::new(200, b"<html>".to_vec()).with_field("Content-Type", "application/json")))),
        ("an answer that is no JMAP", Rc::new(|_| json_reply(200, &json!({"methodResponses": 7})))),
    ];
    for (what, answer) in answers {
        let fake = sender_but(move |req, n| (n == 4).then(|| answer(req)));
        let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
        // Held as one that may have gone: not sent again by itself.
        assert_eq!(sent_of(&pieces), [is(41, lost())], "{what}");
        // Asked once, and nothing more of the Outbox in this run: not the second message either.
        assert_eq!(fake.args_of("EmailSubmission/set").len(), 1, "{what}");
        assert_eq!(fake.uploads().len(), 1, "{what}");
        assert!(fake.args_of("Email/set").is_empty(), "{what}: nothing is taken away on a guess");
        let end = last(&pieces);
        assert!(read_on(&pieces) && !end.whole, "{what}");
    }

    // An answer that came, and says nothing of the submission: held the same way. The server is there, so
    // the next message is tried; each is asked once.
    let vague: Vec<(&str, Answering)> = vec![
        ("an answer without the calls", Rc::new(|req| api_reply(req, |_, _| Answer::Missing))),
        (
            "an answer that says nothing of the submission",
            Rc::new(|req| {
                api_reply(req, |name, args| if name == "EmailSubmission/set" { Answer::Is(json!({"created": null})) } else { sending(name, args) })
            }),
        ),
        (
            "a submission that is not one",
            Rc::new(|req| {
                api_reply(
                    req,
                    |name, args| if name == "EmailSubmission/set" { Answer::Is(json!({"created": {"s": null}})) } else { sending(name, args) },
                )
            }),
        ),
    ];
    for (what, answer) in vague {
        let fake = sender_but(move |req, n| (n == 4).then(|| answer(req)));
        let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
        assert_eq!(sent_of(&pieces), [is(41, lost()), is(42, SendOutcome::Sent)], "{what}");
        assert_eq!((fake.args_of("EmailSubmission/set").len(), fake.uploads().len()), (2, 2), "{what}");
        assert!(fake.args_of("Email/set").is_empty(), "{what}: nothing is taken away on a guess");
    }

    // What says the request never reached the server's reading of it sends nothing: the message waits.
    let not_taken: Vec<(&str, Rc<dyn Fn() -> Reply>)> = vec![
        ("no connection", Rc::new(|| Err(TransportError::Unreachable))),
        ("no secure connection", Rc::new(|| Err(TransportError::Tls))),
        ("a redirect", Rc::new(|| Ok(HttpResponse::new(307, Vec::new()).with_field("Location", "/elsewhere")))),
    ];
    for (what, answer) in not_taken {
        let fake = sender_but(move |_, n| (n == 4).then(|| answer()));
        let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
        assert!(sent_of(&pieces).is_empty(), "{what}");
        assert_eq!((fake.args_of("EmailSubmission/set").len(), fake.uploads().len()), (1, 1), "{what}");
        assert!(last(&pieces).errors.len() == 1 && read_on(&pieces), "{what}");
    }
    // A request the server would not read (400, with why) is a message held with that, and not a lost one.
    let fake = sender_but(|_, n| {
        (n == 4).then(|| json_reply(400, &json!({"type": "urn:ietf:params:jmap:error:notRequest", "status": 400, "detail": "no"})))
    });
    let pieces = send_fake(&fake, vec![simple(41)]);
    let sent = sent_of(&pieces);
    assert!(
        matches!(&sent[0].outcome, SendOutcome::Held { lost: false, why } if why.starts_with("the server would not take the request to send it: ")),
        "{sent:?}"
    );
}

#[test]
fn a_run_stopped_between_the_upload_and_the_submission_sends_nothing() {
    let cancel = Rc::new(AtomicBool::new(false));
    let stops = cancel.clone();
    let fake = sender_but(move |req, _| {
        if is_upload(req) {
            stops.store(true, Ordering::Relaxed);
        }
        None
    });
    let pieces = run_over(fake.clone(), FAKE_TOKEN, &JmapMailRequest { sends: vec![simple(41), simple(42)], ..asks(FAKE_SESSION) }, &cancel);
    assert!(sent_of(&pieces).is_empty() && last(&pieces).stopped && pieces.len() == 1);
    assert_eq!(fake.posts(), [vec!["Identity/get"], vec!["Mailbox/get"], vec!["upload"]]);

    // Stopped while the submission was on its way: nobody knows whether it went, and it is held so.
    let cancel = Rc::new(AtomicBool::new(false));
    let stops = cancel.clone();
    let fake = sender_but(move |_, n| {
        (n == 4).then(|| {
            stops.store(true, Ordering::Relaxed);
            Err(TransportError::Aborted)
        })
    });
    let pieces = run_over(fake.clone(), FAKE_TOKEN, &JmapMailRequest { sends: vec![simple(41), simple(42)], ..asks(FAKE_SESSION) }, &cancel);
    assert_eq!(sent_of(&pieces), [is(41, lost())]);
    assert!(last(&pieces).stopped && !read_on(&pieces) && fake.uploads().len() == 1);

    // Stopped before the run: nothing at all.
    let fake = sender(|_, _| None);
    let pieces = run_over(fake.clone(), FAKE_TOKEN, &JmapMailRequest { sends: vec![simple(41)], ..asks(FAKE_SESSION) }, &AtomicBool::new(true));
    assert!(sent_of(&pieces).is_empty() && last(&pieces).stopped && fake.posts().is_empty());
}

#[test]
fn a_refused_sign_in_and_a_busy_server_end_the_run_at_every_step_and_the_message_waits() {
    // The requests of a send, after the session: the identities, the roles, the upload, the submission.
    for (step, what) in [(1, "the identities"), (2, "the roles"), (3, "the upload"), (4, "the submission")] {
        let fake = sender_but(move |_, n| (n == step).then(|| status_reply(401, None)));
        let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
        let end = last(&pieces);
        assert!(sent_of(&pieces).is_empty() && end.auth_refused && !end.whole && end.errors.is_empty() && pieces.len() == 1, "{what}: {end:?}");
        assert_eq!(fake.posts().len(), step, "{what}: nothing is asked after");

        for (status, retry_after, seconds) in [(429, Some(90), 90), (503, None, 30)] {
            let fake = sender_but(move |_, n| (n == step).then(|| status_reply(status, retry_after)));
            let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
            let end = last(&pieces);
            assert!(sent_of(&pieces).is_empty() && !end.whole && !end.auth_refused && pieces.len() == 1, "{what} {status}: {end:?}");
            assert_eq!((end.busy_for, fake.posts().len()), (Some(seconds), step), "{what} {status}");
        }
    }
    // After a message went, the next one's trouble leaves the first reported.
    let fake = sender_but(|req, n| (n > 4 && is_upload(req)).then(|| status_reply(401, None)));
    let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
    assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)]);
    assert!(last(&pieces).auth_refused && pieces.len() == 2);

    // The identities or the roles could not be had for another reason: a line, the Outbox waits, the run reads.
    for (call, n) in [("Identity/get", 1), ("Mailbox/get", 2)] {
        let fake = sender_but(move |req, at| (at == n).then(|| api_reply(req, |_, _| Answer::Error("serverFail", format!("{ME} broke it")))));
        let pieces = send_fake(&fake, vec![simple(41)]);
        let end = last(&pieces);
        assert!(sent_of(&pieces).is_empty() && fake.uploads().is_empty() && read_on(&pieces), "{call}");
        assert!(
            end.errors.len() == 1 && end.errors[0].starts_with("Couldn't send what waits in the Outbox: ") && !end.errors[0].contains(ME),
            "{call}: {end:?}"
        );
    }
}

#[test]
fn an_address_that_could_end_a_line_or_begin_another_is_no_recipient_and_the_message_is_held() {
    let hostile = [
        "bob@example.org\r\nRCPT TO:<eve@example.org>",
        "bob@example.org\nx",
        "bob@example.org\0",
        "bob smith@example.org",
        "bob@exa mple.org",
        "<bob@example.org>",
        "bob@example.org>",
        "bob",
        "@example.org",
        "bob@",
        "bob@@example.org",
        "bob@example.org,eve@example.org",
        "\"bob\"@example.org",
        "bob@example.org\t eve@example.org",
    ];
    for email in hostile {
        assert!(!is_address(email), "{email:?}");
        let fake = sender(|_, _| None);
        // Among good ones, in any of the three fields.
        for (to, cc, bcc) in
            [(vec![email], vec![], vec![]), (vec!["dan@example.org"], vec![email], vec![]), (vec!["dan@example.org"], vec![], vec![email])]
        {
            let pieces = send_fake(&fake, vec![outbox(41, message(&to, &cc, &bcc))]);
            assert_eq!(sent_of(&pieces), [is(41, held(BAD_RECIPIENT))], "{email:?}");
        }
        assert!(fake.uploads().is_empty() && fake.args_of("EmailSubmission/set").is_empty(), "{email:?}");
    }
    let long_local = format!("{}@example.org", "a".repeat(65));
    let long = format!("a@{}.example.org", "b".repeat(250));
    assert!(!is_address(&long_local) && !is_address(&long) && !is_address(""));
    for good in ["bob@example.org", "bob.smith+tag@mail.example.org", "o'brien@example.org", "b\u{f6}b@b\u{fc}cher.example"] {
        assert!(is_address(good), "{good}");
    }

    // Who a message goes to: everybody once, in order; blanks passed over; nobody is nobody.
    let all = recipients(&message(
        &["bob@example.org", "", "  dan@example.org "],
        &["bob@example.org", "Bob@example.org"],
        &["dan@EXAMPLE.ORG", "erin@example.org"],
    ));
    assert_eq!(all, Ok(vec!["bob@example.org".to_string(), "dan@example.org".into(), "Bob@example.org".into(), "erin@example.org".into()]));
    assert_eq!(recipients(&message(&[], &[" "], &[""])), Err(NO_RECIPIENTS));
    assert_eq!(recipients(&message(&["bob@example.org"], &[], &["x y@example.org"])), Err(BAD_RECIPIENT));
    let crowd: Vec<String> = (0..=MAX_RECIPIENTS).map(|n| format!("p{n}@example.org")).collect();
    let crowd: Vec<&str> = crowd.iter().map(String::as_str).collect();
    assert_eq!(recipients(&message(&crowd, &[], &[])), Err(TOO_MANY));
    assert_eq!(recipients(&message(&crowd[1..], &[], &[])).map(|all| all.len()), Ok(MAX_RECIPIENTS));

    // A message with nobody to go to, with too many, and with nothing to send, are held without an upload.
    let fake = sender(|_, _| None);
    let nothing = SendUp { message: 43, draft: None, mime: None };
    let pieces = send_fake(&fake, vec![outbox(41, message(&[], &[], &[])), outbox(42, message(&crowd, &[], &[])), nothing, simple(44)]);
    assert_eq!(sent_of(&pieces), [is(41, held(NO_RECIPIENTS)), is(42, held(TOO_MANY)), is(43, held(NOTHING)), is(44, SendOutcome::Sent)]);
    assert_eq!(fake.uploads().len(), 1);

    // An account whose own address is none sends nothing.
    let fake = sender(|_, _| None);
    let mut request = JmapMailRequest { sends: vec![simple(41)], ..asks(FAKE_SESSION) };
    request.email = "ada@example.org\r\nMAIL FROM:<eve@example.org>".into();
    let pieces = run_unchecked(fake.clone(), FAKE_TOKEN, &request, &AtomicBool::new(false));
    assert_eq!(sent_of(&pieces), [is(41, held(BAD_SENDER))]);
    assert!(fake.uploads().is_empty() && fake.args_of("Identity/get").is_empty());
}

#[test]
fn the_message_goes_without_its_bcc_field_and_dated() {
    let mut mail = message(&["bob@example.org"], &["carol@example.org"], &["erin@example.org"]);
    mail.subject = "Lunch\r\nBcc: eve@example.org".into();
    let octets = written(&mail);
    let text = String::from_utf8_lossy(&octets).to_string();
    assert!(!text.contains("erin@example.org") && !text.lines().any(|line| line.to_ascii_lowercase().starts_with("bcc:")), "{text}");
    assert!(text.contains("bob@example.org") && text.contains("carol@example.org"));
    assert_eq!(mailcraft_mime_model::parse(&octets).date, THEN);
    // One that was never dated is dated now.
    mail.date = 0;
    let dated = mailcraft_mime_model::parse(&written(&mail)).date;
    assert!(dated > 1_700_000_000, "{dated}");
}

#[test]
fn only_as_many_messages_as_a_run_sends_are_tried_and_the_rest_wait() {
    let fake = sender(|_, _| None);
    let many: Vec<SendUp> = (0..u64::try_from(MAX_SENDS).unwrap() + 3).map(|n| simple(100 + n)).collect();
    let pieces = send_fake(&fake, many);
    let sent = sent_of(&pieces);
    assert_eq!((sent.len(), fake.uploads().len()), (MAX_SENDS, MAX_SENDS));
    assert!(sent.iter().all(|done| done.outcome == SendOutcome::Sent));
    assert_eq!(fake.args_of("Identity/get").len(), 1, "the identities are asked for once");
    assert!(last(&pieces).whole);
}

#[test]
fn the_changes_go_first_then_the_outbox_then_the_reading_each_in_pieces_of_its_own() {
    let (stand_in, ids) = world();
    let email = stand_in.with(|d| d.deliver(&ids.inbox, b"From: zoe@example.org\r\nSubject: hello\r\n\r\nHi.\r\n", &[], "2026-10-15T06:00:00Z"));
    let request = JmapMailRequest {
        changes: vec![JmapChange::Keywords { message: 9, id: email.clone(), read: Some(true), flagged: None, answered: None, forwarded: None }],
        sends: vec![simple(41), simple(42)],
        ..asks(SESSION)
    };
    let pieces = run_stand_in(&stand_in, &request);
    let kinds: Vec<&str> = pieces
        .iter()
        .map(|piece| match piece {
            piece if !piece.changes.is_empty() => "changes",
            piece if !piece.sent.is_empty() => "sent",
            piece if piece.listing.is_some() => "list",
            piece if !piece.pages.is_empty() => "window",
            _ => "end",
        })
        .collect();
    assert_eq!(kinds, ["changes", "sent", "sent", "list", "window", "window", "window", "end"]);
    assert_eq!((changes_of(&pieces).len(), sent_of(&pieces).len()), (1, 2));
    assert!(last(&pieces).whole);
    let calls = stand_in.calls();
    assert_eq!(calls[..3], ["Email/set", "Identity/get", "Mailbox/get"]);
    // Taken in at once, the pieces add up.
    let mut whole = JmapMailResult::default();
    pieces.into_iter().for_each(|piece| whole.absorb(piece));
    assert_eq!((whole.changes.len(), whole.sent.len(), whole.pages.len(), whole.whole), (1, 2, 3, true));
}

#[test]
fn the_answers_to_a_send_are_read_without_trusting_them() {
    let secrets = Redactor::new();
    let map = |value: Value| value.as_object().cloned().unwrap();
    let made = || -> Result<Map<String, Value>, CallFailure> { Ok(map(json!({"created": {"m": {"id": "E50"}}}))) };
    let says = |imported, submitted: Value| settled(&secrets, imported, Ok(map(submitted)));
    // Sent only when the answer has the submission among those created.
    assert_eq!(says(made(), json!({"created": {"s": {"id": "S1"}}})).outcome, Some(SendOutcome::Sent));
    assert_eq!(says(made(), json!({"created": {"s": {}}})).outcome, Some(SendOutcome::Sent));
    for vague in [
        json!({}),
        json!({"created": null}),
        json!({"created": []}),
        json!({"created": {"s": null}}),
        json!({"created": {"s": "S1"}}),
        json!({"created": {"t": {"id": "S1"}}}),
        json!({"created": "s", "notCreated": ["s"]}),
        json!({"updated": {"s": {}}}),
    ] {
        let said = says(made(), vague.clone());
        assert_eq!(said, Settled { outcome: Some(lost()), orphan: None, later: None }, "{vague}");
    }
    // Refused: with why, and the email the import made is to be taken away.
    let refused = says(made(), json!({"notCreated": {"s": {"type": "forbiddenToSend"}}}));
    assert_eq!(refused, Settled { outcome: Some(held("the server does not let this account send mail")), orphan: Some("E50".into()), later: None });
    // (Both at once: sent is what the answer says first, and nothing is taken away.)
    let both = says(made(), json!({"created": {"s": {"id": "S1"}}, "notCreated": {"s": {"type": "forbiddenToSend"}}}));
    assert_eq!(both, Settled { outcome: Some(SendOutcome::Sent), orphan: None, later: None });
    // A refusal that is no object, and a huge one.
    assert_eq!(says(made(), json!({"notCreated": {"s": 7}})).outcome, Some(held("the server refused it without saying why")));
    let huge = says(made(), json!({"notCreated": {"s": {"type": "odd", "description": "no ".repeat(200_000)}}}));
    assert!(matches!(&huge.outcome, Some(SendOutcome::Held { lost: false, why }) if why.chars().count() <= 200));
    // An id of the import that is none is no email to take away.
    let odd = settled(&secrets, Ok(map(json!({"created": {"m": {"id": "a/b"}}}))), Ok(map(json!({"notCreated": {"s": {"type": "tooLarge"}}}))));
    assert_eq!(odd, Settled { outcome: Some(held(TOO_LARGE)), orphan: None, later: None });
    // Neither call answered: nobody knows.
    assert_eq!(settled(&secrets, Err(CallFailure::Missing), Err(CallFailure::Missing)).outcome, Some(lost()));
    assert_eq!(settled(&secrets, Err(CallFailure::UnexpectedResponse), Err(CallFailure::UnexpectedResponse)).outcome, Some(lost()));
    assert_eq!(settled(&secrets, made(), Err(CallFailure::Missing)), Settled { outcome: Some(lost()), orphan: None, later: None });
}

/// A scripted server that sends and takes `calls` method calls to a request: each call answered by `f`, or
/// as [`sending`] answers it when `f` has nothing to say.
fn sender_of(calls: u32, f: impl Fn(&str, &Value) -> Option<Answer> + 'static) -> Fake {
    Fake::new(move |req, _| match req.method() {
        Method::Get => session_reply(json!({"maxCallsInRequest": calls}), true),
        _ if is_upload(req) => uploaded(),
        _ => api_reply(req, |name, args| f(name, args).unwrap_or_else(|| sending(name, args))),
    })
}

fn names_of(req: &HttpRequest) -> Vec<String> {
    calls_of(req).into_iter().map(|(name, _)| name).collect()
}

// RFC 8620 §3.6.2: `serverPartialFail` is the one error of a call after which the server is not as it was.
#[test]
fn a_submission_the_server_says_partly_happened_is_held_as_one_that_may_have_gone_and_its_copy_stays() {
    // In one request with the import, and where the server takes one call to a request.
    for calls in [16, 1] {
        let fake = sender_of(calls, |name, _| (name == "EmailSubmission/set").then(|| Answer::Error("serverPartialFail", String::new())));
        let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
        // The server is there, so the next message is tried; each is asked once.
        assert_eq!(sent_of(&pieces), [is(41, lost()), is(42, lost())], "{calls}");
        assert_eq!(fake.args_of("EmailSubmission/set").len(), 2, "{calls}");
        assert!(fake.args_of("Email/set").is_empty(), "{calls}: the copy of a message that may have gone is not taken away");
    }
    // The import says so, and the submission's own answer does not say that it was not made.
    let fake = sender(|name, _| match name {
        "Email/import" => Some(Answer::Error("serverPartialFail", String::new())),
        "EmailSubmission/set" => Some(Answer::Missing),
        _ => None,
    });
    assert_eq!(sent_of(&send_fake(&fake, vec![simple(41)])), [is(41, lost())]);
    // It does say so: held with that.
    let fake = sender(|name, _| match name {
        "Email/import" => Some(Answer::Error("serverPartialFail", String::new())),
        "EmailSubmission/set" => Some(Answer::Is(json!({"notCreated": {"s": {"type": "invalidProperties"}}}))),
        _ => None,
    });
    assert_eq!(sent_of(&send_fake(&fake, vec![simple(41)])), [is(41, held("invalidProperties"))]);
    assert!(fake.args_of("Email/set").is_empty());
}

// RFC 8620 §2: a capability's methods are for the accounts whose `accountCapabilities` list it.
#[test]
fn an_account_without_the_capability_of_sending_holds_every_message_though_the_session_offers_it() {
    let fake = Fake::new(|req, _| match req.method() {
        Method::Get => json_reply(
            200,
            &json!({
                "capabilities": {CORE: {"maxCallsInRequest": 16}, MAIL: {}, SUBMISSION: {}},
                "accounts": {"A1": {"name": ME, "isPersonal": true, "accountCapabilities": {MAIL: {}}}},
                "primaryAccounts": {MAIL: "A1"},
                "apiUrl": "https://jmap.example.org/jmap/api",
                "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
                "uploadUrl": "/jmap/upload/{accountId}/",
                "state": "1",
            }),
        ),
        _ => api_reply(req, sending),
    });
    let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
    assert_eq!(sent_of(&pieces), [is(41, held(NO_SUBMISSION)), is(42, held(NO_SUBMISSION))]);
    assert!(fake.args_of("Identity/get").is_empty() && fake.uploads().is_empty(), "nothing is asked for it");
    assert!(last(&pieces).whole && read_on(&pieces));
}

#[test]
fn a_refused_sign_in_and_a_busy_server_met_while_tidying_up_end_the_run_with_the_message_reported() {
    // The server refuses every submission; the request that takes the copy away again is answered by `reply`.
    let tidying = |reply: Rc<dyn Fn() -> Reply>| {
        sender_but(move |req, _| {
            if req.method() != Method::Post || is_upload(req) {
                return None;
            }
            if names_of(req) == ["Email/set"] {
                return Some(reply());
            }
            Some(api_reply(req, |name, args| match name {
                "EmailSubmission/set" => Answer::Is(json!({"notCreated": {"s": {"type": "forbiddenToSend"}}})),
                _ => sending(name, args),
            }))
        })
    };
    let refused = held("the server does not let this account send mail");
    let fake = tidying(Rc::new(|| status_reply(401, None)));
    let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
    assert_eq!(sent_of(&pieces), [is(41, refused.clone())], "what is known of the message is told");
    let end = last(&pieces);
    assert!(end.auth_refused && !end.whole && !read_on(&pieces), "{end:?}");
    assert_eq!(fake.uploads().len(), 1, "the next message is not begun");

    for (status, retry_after, seconds) in [(429, Some(90), 90), (503, None, 30)] {
        let fake = tidying(Rc::new(move || status_reply(status, retry_after)));
        let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
        assert_eq!(sent_of(&pieces), [is(41, refused.clone())], "{status}");
        assert_eq!((last(&pieces).busy_for, fake.uploads().len(), read_on(&pieces)), (Some(seconds), 1, false), "{status}");
    }
    // Any other failure of it is passed over: the run goes on.
    let answers: Vec<Rc<dyn Fn() -> Reply>> = vec![Rc::new(|| status_reply(500, None)), Rc::new(|| Err(TransportError::TimedOut))];
    for answer in answers {
        let fake = tidying(answer);
        let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
        assert_eq!(sent_of(&pieces), [is(41, refused.clone()), is(42, refused.clone())]);
        assert!(read_on(&pieces) && last(&pieces).whole);
    }
}

// RFC 8621 §7.5: the implicit `Email/set` of `onSuccessUpdateEmail` is answered after the submission's own
// response, under the same call id.
#[test]
fn a_submission_that_was_made_is_sent_though_the_update_of_its_email_failed_after_it() {
    // The calls answered as [`sending`] does, and after the submission's own answer an error of its call.
    fn then_fails(req: &HttpRequest, kind: &str) -> Reply {
        let body: Value = serde_json::from_slice(req.body()).unwrap();
        let mut responses = Vec::new();
        for call in body["methodCalls"].as_array().unwrap() {
            let (name, id) = (call[0].as_str().unwrap(), call[2].clone());
            if let Answer::Is(value) = sending(name, &call[1]) {
                responses.push(json!([name, value, id]));
            }
            if name == "EmailSubmission/set" {
                responses.push(json!(["error", {"type": kind}, id]));
            }
        }
        json_reply(200, &json!({"methodResponses": responses, "sessionState": "1"}))
    }
    for calls in [16, 1] {
        for kind in ["serverUnavailable", "serverFail", "serverPartialFail"] {
            let fake = Fake::new(move |req, _| match req.method() {
                Method::Get => session_reply(json!({"maxCallsInRequest": calls}), true),
                _ if is_upload(req) => uploaded(),
                _ if names_of(req).iter().any(|name| name == "EmailSubmission/set") => then_fails(req, kind),
                _ => api_reply(req, sending),
            });
            let pieces = send_fake(&fake, vec![simple(41), simple(42)]);
            // Sent, and so not offered again; the Outbox goes on.
            assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent), is(42, SendOutcome::Sent)], "{calls} {kind}");
            assert_eq!(fake.args_of("EmailSubmission/set").len(), 2, "{calls} {kind}");
            // The email is not destroyed: its draft mark is asked off once more.
            let unmark = json!({"accountId": "A1", "update": {"E50": {"keywords/$draft": null}}});
            assert_eq!(fake.args_of("Email/set"), [unmark.clone(), unmark], "{calls} {kind}");
            assert!(last(&pieces).whole && read_on(&pieces), "{calls} {kind}");
        }
    }
}

#[test]
fn a_message_imported_and_never_submitted_waits_and_goes_once_with_the_next_run() {
    // A server that takes one call to a request, whose fourth request to the API (the identities, the roles
    // and the import before it) is the submission: turned away unread.
    for no_duplicates in [true, false] {
        for status in [401, 503] {
            let (stand_in, ids) = world();
            stand_in.with(|d| {
                d.max_calls = 1;
                d.no_duplicates = no_duplicates;
                d.refuse_next.extend([None, None, None, Some((status, None))]);
            });
            let pieces = send(&stand_in, vec![simple(41)]);
            assert!(sent_of(&pieces).is_empty(), "{no_duplicates} {status}: it waits");
            assert_eq!((last(&pieces).auth_refused, last(&pieces).busy_for.is_some()), (status == 401, status == 503));
            // The copy is there, a draft still; nothing went, and nothing was taken away on a guess.
            stand_in.with(|d| {
                assert_eq!((d.emails.len(), d.submitted.len()), (1, 0), "{no_duplicates} {status}");
                assert!(d.emails[0].keywords.contains("$draft"));
            });
            assert!(!stand_in.calls().iter().any(|call| call == "Email/set"));

            // The next run: it goes, once. A server that keeps no two emails alike sends the copy it has.
            let pieces = send(&stand_in, vec![simple(41)]);
            assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)], "{no_duplicates} {status}");
            stand_in.with(|d| {
                assert_eq!((d.submitted.len(), d.emails.len()), (1, if no_duplicates { 1 } else { 2 }), "{no_duplicates} {status}");
                let email = d.email(&d.submitted[0].email).unwrap();
                assert!(email.mailboxes.contains(&ids.sent) && !email.keywords.contains("$draft"));
            });
            assert!(last(&pieces).whole);
        }
    }
    // The run stopped between the import and the submission: the same.
    let cancel = Rc::new(AtomicBool::new(false));
    let stops = cancel.clone();
    let fake = Fake::new(move |req, _| {
        if names_of(req) == ["Email/import"] {
            stops.store(true, Ordering::Relaxed);
        }
        match req.method() {
            Method::Get => session_reply(json!({"maxCallsInRequest": 1}), true),
            _ if is_upload(req) => uploaded(),
            _ => api_reply(req, sending),
        }
    });
    let pieces = run_over(fake.clone(), FAKE_TOKEN, &JmapMailRequest { sends: vec![simple(41)], ..asks(FAKE_SESSION) }, &cancel);
    assert!(sent_of(&pieces).is_empty() && last(&pieces).stopped);
    assert!(fake.args_of("EmailSubmission/set").is_empty() && fake.args_of("Email/set").is_empty());
}

// RFC 8621 §4.8: an import of an email the account has is refused with `alreadyExists` and its `existingId`.
// RFC 8620 §6: a blob's id names its octets and nothing else.
#[test]
fn an_email_the_server_has_already_is_submitted_only_when_it_is_of_the_blob_uploaded() {
    // The import is refused for the email `existing`; the submission that named the import is not made for
    // want of it; E9 is of the blob `blob`; and a submission of E9 itself is answered with `of_copy`.
    fn has_it(calls: u32, existing: &'static str, blob: &'static str, of_copy: fn() -> Answer) -> Fake {
        sender_of(calls, move |name, args| match name {
            "Email/import" => Some(Answer::Is(json!({"notCreated": {"m": {"type": "alreadyExists", "existingId": existing}}}))),
            "Email/get" if args["ids"] == json!(["E9"]) => {
                Some(Answer::Is(json!({"list": [{"id": "E8", "blobId": "B7"}, {"id": "E9", "blobId": blob}]})))
            }
            "EmailSubmission/set" if args["create"]["s"]["emailId"] == "#m" => {
                Some(Answer::Is(json!({"notCreated": {"s": {"type": "invalidProperties", "properties": ["emailId"]}}})))
            }
            "EmailSubmission/set" => Some(of_copy()),
            _ => None,
        })
    }
    let made = || Answer::Is(json!({"created": {"s": {"id": "S1"}}}));
    let emails = |fake: &Fake| fake.args_of("EmailSubmission/set").iter().map(|args| args["create"]["s"]["emailId"].clone()).collect::<Vec<_>>();
    for (calls, asked) in [(16, vec![json!("#m"), json!("E9")]), (1, vec![json!("E9")])] {
        // The same octets (the upload answered `B7`): the email the server has is the one submitted.
        let fake = has_it(calls, "E9", "B7", made);
        let pieces = send_fake(&fake, vec![simple(41)]);
        assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)], "{calls}");
        assert_eq!(emails(&fake), asked, "{calls}");
        assert_eq!(fake.args_of("Email/get")[0], json!({"accountId": "A1", "ids": ["E9"], "properties": ["id", "blobId", "size", "mailboxIds"]}));
        assert!(fake.args_of("Email/set").is_empty() && last(&pieces).whole, "{calls}");

        // Another blob, so perhaps other octets (an email that only shares its Message-ID): never submitted.
        // The message is held with the server's refusal.
        let fake = has_it(calls, "E9", "B8", made);
        let sent = sent_of(&send_fake(&fake, vec![simple(41)]));
        assert!(
            matches!(&sent[0].outcome, SendOutcome::Held { lost: false, why } if why.starts_with("the server would not take the message: ")),
            "{calls}: {sent:?}"
        );
        assert!(!emails(&fake).contains(&json!("E9")), "{calls}");

        // An `existingId` that is no id: nothing is asked of it.
        let fake = has_it(calls, "E 9", "B7", made);
        let sent = sent_of(&send_fake(&fake, vec![simple(41)]));
        assert!(matches!(&sent[0].outcome, SendOutcome::Held { lost: false, .. }), "{calls}: {sent:?}");
        assert!(!emails(&fake).contains(&json!("E9")), "{calls}");

        // The server will not send the copy: held with why, and the copy, which was there before, stays.
        let fake = has_it(calls, "E9", "B7", || Answer::Is(json!({"notCreated": {"s": {"type": "forbiddenToSend"}}})));
        let pieces = send_fake(&fake, vec![simple(41)]);
        assert_eq!(sent_of(&pieces), [is(41, held("the server does not let this account send mail"))], "{calls}");
        assert!(fake.args_of("Email/set").is_empty(), "{calls}: an email this run did not make is not destroyed");

        // Its submission got no answer of use: held as one that may have gone.
        let fake = has_it(calls, "E9", "B7", || Answer::Missing);
        assert_eq!(sent_of(&send_fake(&fake, vec![simple(41)])), [is(41, lost())], "{calls}");
        assert_eq!(emails(&fake), asked, "{calls}: and not asked again");
    }
    // Where the answer does not say that the submission in the same request was not made, nothing more is
    // asked: a second submission could be a second delivery.
    let fake = sender(|name, _| match name {
        "Email/import" => Some(Answer::Is(json!({"notCreated": {"m": {"type": "alreadyExists", "existingId": "E9"}}}))),
        "EmailSubmission/set" => Some(Answer::Missing),
        _ => None,
    });
    let sent = sent_of(&send_fake(&fake, vec![simple(41)]));
    assert!(matches!(&sent[0].outcome, SendOutcome::Held { lost: false, .. }), "{sent:?}");
    assert_eq!(fake.args_of("EmailSubmission/set").len(), 1);

    // Read without trusting them.
    let map = |value: Value| -> Result<Map<String, Value>, CallFailure> { Ok(value.as_object().cloned().unwrap()) };
    assert_eq!(existing_of(&map(json!({"notCreated": {"m": {"type": "alreadyExists", "existingId": "E9"}}}))), Some("E9".into()));
    for none in [
        json!({}),
        json!({"notCreated": {"m": {"type": "overQuota", "existingId": "E9"}}}),
        json!({"notCreated": {"m": {"type": "alreadyExists"}}}),
        json!({"notCreated": {"m": {"type": "alreadyExists", "existingId": 9}}}),
        json!({"notCreated": {"x": {"type": "alreadyExists", "existingId": "E9"}}}),
        json!({"notCreated": ["m"]}),
    ] {
        assert_eq!(existing_of(&map(none.clone())), None, "{none}");
    }
    assert_eq!(existing_of(&Err(CallFailure::Missing)), None);
    assert!(not_made(&map(json!({"notCreated": {"s": {}}}))) && not_made(&map(json!({"created": {"s": null}, "notCreated": {"s": 7}}))));
    for unsaid in [json!({}), json!({"notCreated": null}), json!({"created": {"s": {}}, "notCreated": {"s": {}}}), json!({"notCreated": {"t": {}}})] {
        assert!(!not_made(&map(unsaid.clone())), "{unsaid}");
    }
    assert!(!not_made(&Err(CallFailure::Missing)) && !not_made(&Err(CallFailure::UnexpectedResponse)));
    assert!(same_octets(&json!({"list": [null, {"id": "E9"}, {"id": "E9", "blobId": "B7"}]}), "E9", "B7"));
    for other in [json!(null), json!({"list": {}}), json!({"list": [{"id": "E8", "blobId": "B7"}]}), json!({"list": [{"id": "E9", "blobId": 7}]})] {
        assert!(!same_octets(&other, "E9", "B7"), "{other}");
    }
}

/// The downloads of a blob a scripted server was asked for.
fn downloads(fake: &Fake) -> usize {
    fake.sent.borrow().iter().filter(|req| req.method() == Method::Get && req.url().contains("/jmap/download/")).count()
}

// RFC 8620 §6: a blob's id names its octets, and nothing keeps a server from giving the same octets another
// id. RFC 8621 §4.1.1: an email's `blobId` is of the raw message, and its `size` the octets of it.
#[test]
fn an_email_the_server_has_under_another_blob_id_is_submitted_only_when_it_holds_the_octets_uploaded() {
    // The stand-in keeps no two emails alike and gives every upload an id of its own. The first run's
    // submission is turned away unread (a server that takes one call to a request), so its copy stays.
    for calls in [1, 16] {
        let (stand_in, ids) = world();
        stand_in.with(|d| {
            d.max_calls = 1;
            (d.no_duplicates, d.blob_per_upload) = (true, true);
            d.refuse_next.extend([None, None, None, Some((503, None))]);
        });
        assert!(sent_of(&send(&stand_in, vec![simple(41)])).is_empty(), "{calls}: it waits");
        // The next run uploads the same octets, which get another id; the email it has is of the first.
        stand_in.with(|d| d.max_calls = calls);
        let pieces = send(&stand_in, vec![simple(41)]);
        assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)], "{calls}");
        stand_in.with(|d| {
            assert_eq!((d.submitted.len(), d.emails.len(), d.uploads.len()), (1, 1, 2), "{calls}");
            let email = d.email(&d.submitted[0].email).unwrap();
            assert!(email.mailboxes.contains(&ids.sent) && !email.keywords.contains("$draft"), "{calls}");
        });
        // Its blob was read once to see that, and nothing was destroyed.
        assert_eq!(stand_in.seen().iter().filter(|seen| seen.path.starts_with("/jmap/download/")).count(), 1, "{calls}");
        assert!(last(&pieces).whole, "{calls}");
    }

    // A scripted server that has the email E9 already, of the blob B8 (the upload is answered `B7`): it
    // says E9 has `more` octets more than were uploaded, and gives what `blob` makes of the upload for B8.
    fn has_it(calls: u32, more: usize, blob: fn(Vec<u8>) -> Reply) -> Fake {
        let kept: Rc<RefCell<Vec<u8>>> = Rc::default();
        Fake::new(move |req, _| match req.method() {
            Method::Get if req.url().contains("/jmap/download/") => {
                assert!(req.url().contains("/A1/B8/"), "the blob of the email it has, and no other");
                blob(kept.borrow().clone())
            }
            Method::Get => session_reply(json!({"maxCallsInRequest": calls}), true),
            _ if is_upload(req) => {
                *kept.borrow_mut() = req.body().to_vec();
                uploaded()
            }
            _ => {
                let size = kept.borrow().len() + more;
                api_reply(req, |name, args| match name {
                    "Email/import" => Answer::Is(json!({"notCreated": {"m": {"type": "alreadyExists", "existingId": "E9"}}})),
                    "Email/get" if args["ids"] == json!(["E9"]) => Answer::Is(json!({"list": [{"id": "E9", "blobId": "B8", "size": size}]})),
                    "EmailSubmission/set" if args["create"]["s"]["emailId"] == "#m" => {
                        Answer::Is(json!({"notCreated": {"s": {"type": "invalidProperties", "properties": ["emailId"]}}}))
                    }
                    "EmailSubmission/set" => Answer::Is(json!({"created": {"s": {"id": "S1"}}})),
                    _ => sending(name, args),
                })
            }
        })
    }
    let emails = |fake: &Fake| fake.args_of("EmailSubmission/set").iter().map(|args| args["create"]["s"]["emailId"].clone()).collect::<Vec<_>>();
    let refused = |sent: &[SendDone]| matches!(&sent[0].outcome, SendOutcome::Held { lost: false, why } if why.starts_with("the server would not take the message: "));
    for calls in [16, 1] {
        // The same octets under the other id: the email the server has is the one submitted, once.
        let fake = has_it(calls, 0, |octets| Ok(HttpResponse::new(200, octets)));
        let pieces = send_fake(&fake, vec![simple(41)]);
        assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)], "{calls}");
        assert_eq!(emails(&fake).iter().filter(|email| **email == json!("E9")).count(), 1, "{calls}");
        assert_eq!(downloads(&fake), 1, "{calls}");
        assert!(fake.args_of("Email/set").is_empty() && last(&pieces).whole, "{calls}");

        // As many octets, and one of them another: an email that only resembles the message. Never submitted.
        let fake = has_it(calls, 0, |mut octets| {
            let last = octets.last_mut().unwrap();
            *last = last.wrapping_add(1);
            Ok(HttpResponse::new(200, octets))
        });
        let sent = sent_of(&send_fake(&fake, vec![simple(41)]));
        assert!(refused(&sent), "{calls}: {sent:?}");
        assert!(!emails(&fake).contains(&json!("E9")) && downloads(&fake) == 1, "{calls}");

        // Another size: not the upload, and its blob is not asked for.
        let fake = has_it(calls, 1, |octets| Ok(HttpResponse::new(200, octets)));
        let sent = sent_of(&send_fake(&fake, vec![simple(41)]));
        assert!(refused(&sent), "{calls}: {sent:?}");
        assert!(!emails(&fake).contains(&json!("E9")) && downloads(&fake) == 0, "{calls}");

        // A blob larger than the email was said to be, and one the server will not give: not seen to be the same.
        for blob in [
            (|mut octets| {
                octets.push(b'x');
                Ok(HttpResponse::new(200, octets))
            }) as fn(Vec<u8>) -> Reply,
            |_| status_reply(404, None),
        ] {
            let fake = has_it(calls, 0, blob);
            let sent = sent_of(&send_fake(&fake, vec![simple(41)]));
            assert!(refused(&sent), "{calls}: {sent:?}");
            assert!(!emails(&fake).contains(&json!("E9")), "{calls}");
        }

        // A refused sign-in and a busy server met there end the run, and the message waits.
        for status in [401, 503] {
            let fake = if status == 401 { has_it(calls, 0, |_| status_reply(401, None)) } else { has_it(calls, 0, |_| status_reply(503, None)) };
            let pieces =
                run_unchecked(fake.clone(), FAKE_TOKEN, &JmapMailRequest { sends: vec![simple(41)], ..asks(FAKE_SESSION) }, &AtomicBool::new(false));
            assert!(sent_of(&pieces).is_empty(), "{calls} {status}: it waits");
            assert_eq!((last(&pieces).auth_refused, last(&pieces).busy_for.is_some()), (status == 401, status == 503), "{calls} {status}");
            assert!(!emails(&fake).contains(&json!("E9")), "{calls} {status}");
        }
    }

    // Read without trusting it.
    let listed = json!({"list": [null, {"id": "E8", "blobId": "B1", "size": 9}, {"id": "E9", "blobId": "B8", "size": 9}]});
    assert_eq!(blob_of_size(&listed, "E9", 9).as_ref().map(Id::as_str), Some("B8"));
    assert_eq!(blob_of_size(&listed, "E9", 8), None);
    assert_eq!(blob_of_size(&listed, "E7", 9), None);
    for other in [
        json!(null),
        json!({"list": {}}),
        json!({"list": [{"id": "E9", "blobId": "B 8", "size": 9}]}),
        json!({"list": [{"id": "E9", "blobId": "B8", "size": "9"}]}),
        json!({"list": [{"id": "E9", "size": 9}]}),
        json!({"list": [{"id": "E9", "blobId": "B8"}]}),
    ] {
        assert_eq!(blob_of_size(&other, "E9", 9), None, "{other}");
    }
}

// RFC 8621 §7.5: the implicit `Email/set` of `onSuccessUpdateEmail` answers as an `Email/set` response of
// its own, after the submission's and under the same call id. An email it would not update is in `notUpdated`.
#[test]
fn a_draft_mark_the_server_says_it_did_not_take_off_is_asked_off_once_more() {
    for calls in [16, 1] {
        // The stand-in makes the submission and will not change the email, whatever id it gets.
        let (stand_in, ids) = world();
        stand_in.with(|d| {
            d.max_calls = calls;
            d.refused_changes.extend((1..100).map(|n| (format!("E{n}"), ("forbidden".to_string(), String::new()))));
        });
        let pieces = send(&stand_in, vec![simple(41)]);
        assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)], "{calls}");
        // Asked off once more, by itself (the stand-in refuses that too); the email is not destroyed, and
        // the run reads on.
        assert_eq!(stand_in.calls().iter().filter(|call| *call == "Email/set").count(), 1, "{calls}");
        stand_in.with(|d| {
            assert_eq!((d.submitted.len(), d.emails.len()), (1, 1), "{calls}");
            assert!(d.emails[0].mailboxes.contains(&ids.sent) && d.emails[0].keywords.contains("$draft"), "{calls}");
        });
        assert!(last(&pieces).whole && read_on(&pieces), "{calls}");

        // A scripted server whose implicit answer says which email it did not update.
        let implicit = |email: &'static str| {
            Fake::new(move |req, _| match req.method() {
                Method::Get => session_reply(json!({"maxCallsInRequest": calls}), true),
                _ if is_upload(req) => uploaded(),
                _ if names_of(req).iter().any(|name| name == "EmailSubmission/set") => {
                    let body: Value = serde_json::from_slice(req.body()).unwrap();
                    let mut responses = Vec::new();
                    for call in body["methodCalls"].as_array().unwrap() {
                        let (name, id) = (call[0].as_str().unwrap(), call[2].clone());
                        if let Answer::Is(value) = sending(name, &call[1]) {
                            responses.push(json!([name, value, id]));
                        }
                        if name == "EmailSubmission/set" {
                            responses
                                .push(json!(["Email/set", {"accountId": "A1", "updated": null, "notUpdated": {email: {"type": "forbidden"}}}, id]));
                        }
                    }
                    json_reply(200, &json!({"methodResponses": responses, "sessionState": "1"}))
                }
                _ => api_reply(req, sending),
            })
        };
        let fake = implicit("E50");
        assert_eq!(sent_of(&send_fake(&fake, vec![simple(41)])), [is(41, SendOutcome::Sent)], "{calls}");
        assert_eq!(fake.args_of("EmailSubmission/set").len(), 1, "{calls}");
        assert_eq!(fake.args_of("Email/set"), [json!({"accountId": "A1", "update": {"E50": {"keywords/$draft": null}}})], "{calls}");
        // Another email's: nothing of this message is left to tidy up.
        let fake = implicit("E51");
        assert_eq!(sent_of(&send_fake(&fake, vec![simple(41)])), [is(41, SendOutcome::Sent)], "{calls}");
        assert!(fake.args_of("Email/set").is_empty(), "{calls}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    // Whatever a server answers the import and the submission with, the answers are read without a panic, a
    // message is taken for sent only when the answer has its submission among those created, and a whole
    // run over such a server ends with at most one word about the message.
    #[test]
    fn any_answer_to_a_submission_is_read_without_panicking(imported in any_json(), submitted in any_json(), under in proptest::sample::select(vec!["created", "notCreated", "updated"]), entry in any_json()) {
        let secrets = Redactor::new();
        let mut shaped = json!({});
        shaped[under] = json!({"s": entry.clone(), "m": entry});
        for (imported, submitted) in [(imported.clone(), submitted.clone()), (shaped.clone(), submitted), (imported, shaped.clone()), (shaped.clone(), shaped)] {
            let created = submitted.get("created").and_then(|created| created.get("s")).is_some_and(Value::is_object);
            let as_call = |value: &Value| value.as_object().cloned().ok_or(CallFailure::Missing);
            let said = settled(&secrets, as_call(&imported), as_call(&submitted));
            prop_assert_eq!(said.outcome == Some(SendOutcome::Sent), created && submitted.is_object());
            if let Some(orphan) = &said.orphan {
                prop_assert!(Id::new(orphan).is_ok());
            }

            let (import, submission) = (imported.clone(), submitted.clone());
            let fake = sender(move |name, _| match name {
                "Email/import" => Some(Answer::Is(import.clone())),
                "EmailSubmission/set" => Some(Answer::Is(submission.clone())),
                _ => None,
            });
            let pieces = send_fake(&fake, vec![simple(41)]);
            let sent = sent_of(&pieces);
            prop_assert!(sent.len() <= 1);
            prop_assert_eq!(fake.args_of("EmailSubmission/set").len(), 1);
            if sent.first().is_some_and(|done| done.outcome == SendOutcome::Sent) {
                prop_assert!(created);
            }
        }
    }

    // Whatever a server lists as its identities and its mailboxes, and answers an upload with, a run ends.
    #[test]
    fn any_identities_mailboxes_and_upload_answers_end_the_run(identities in any_json(), mailboxes in any_json(), blob in any_json()) {
        let _ = (identity_of(&identities, ME), filing_of(&mailboxes));
        let fake = sender_but(move |req, n| match n {
            1 => Some(api_reply(req, |_, _| Answer::Is(identities.clone()))),
            2 => Some(api_reply(req, |_, _| Answer::Is(mailboxes.clone()))),
            _ if is_upload(req) => Some(json_reply(201, &blob)),
            _ => None,
        });
        let pieces = send_fake(&fake, vec![simple(41)]);
        prop_assert!(sent_of(&pieces).len() <= 1);
        prop_assert!(!last(&pieces).partial);
    }
}

// RFC 8621 §4.8: `alreadyExists` is of the whole account. §7.5: `onSuccessUpdateEmail` is a patch of the
// email once it is sent.
#[test]
fn an_email_the_server_has_already_in_another_mailbox_is_filed_in_sent_too_once_it_is_sent() {
    for calls in [1, 16] {
        // The first run's submission is turned away unread, so its copy stays, a draft in Sent.
        let (stand_in, ids) = world();
        let archive = stand_in.with(|d| {
            d.max_calls = 1;
            d.no_duplicates = true;
            d.refuse_next.extend([None, None, None, Some((401, None))]);
            d.mailbox("Archive", None, Some("archive"))
        });
        assert!(sent_of(&send(&stand_in, vec![simple(41)])).is_empty(), "{calls}: it waits");
        // Another program moves the copy to Archive.
        stand_in.with(|d| {
            d.max_calls = calls;
            d.emails[0].mailboxes = [archive.clone()].into_iter().collect();
        });
        let pieces = send(&stand_in, vec![simple(41)]);
        assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)], "{calls}");
        stand_in.with(|d| {
            assert_eq!((d.submitted.len(), d.emails.len()), (1, 1), "{calls}");
            let email = d.email(&d.submitted[0].email).unwrap();
            // Filed in Sent as well, left in Archive, and no draft any more.
            assert!(email.mailboxes.contains(&ids.sent) && email.mailboxes.contains(&archive), "{calls}: {:?}", email.mailboxes);
            assert!(!email.keywords.contains("$draft"), "{calls}");
        });
        assert!(last(&pieces).whole, "{calls}");
    }
}

/// The marks that are the mailbox's alone (read, answered, flagged) are not in what is sent: the octets of
/// a message are the same whatever of them changes between two runs, so that a copy a first run left is
/// known again by its octets.
#[test]
fn the_octets_sent_are_the_same_whatever_marks_of_the_mailbox_change() {
    let mail = message(&["bob@example.org"], &[], &[]);
    let plain = written(&mail);
    for marked in [
        Message { read: true, ..mail.clone() },
        Message { answered: true, ..mail.clone() },
        Message { flag: mailcraft_model::FlagStatus::Flagged, ..mail.clone() },
        Message { flag: mailcraft_model::FlagStatus::Complete, read: true, ..mail.clone() },
    ] {
        assert_eq!(written(&marked), plain);
    }
    let text = String::from_utf8_lossy(&written(&Message { read: true, answered: true, ..mail.clone() })).to_ascii_lowercase();
    assert!(!text.contains("\r\nstatus:") && !text.contains("\r\nx-status:") && !text.contains("x-mailcraft-flag"), "{text}");

    // Over two runs: the first leaves its copy (the submission turned away unread), the message is read in
    // between, and the second submits that copy instead of importing another.
    let (stand_in, ids) = world();
    stand_in.with(|d| {
        d.max_calls = 1;
        d.no_duplicates = true;
        d.refuse_next.extend([None, None, None, Some((401, None))]);
    });
    assert!(sent_of(&send(&stand_in, vec![simple(41)])).is_empty(), "it waits");
    let read = outbox(41, Message { read: true, flag: mailcraft_model::FlagStatus::Flagged, ..mail });
    assert_eq!(sent_of(&send(&stand_in, vec![read])), [is(41, SendOutcome::Sent)]);
    stand_in.with(|d| {
        assert_eq!((d.submitted.len(), d.emails.len()), (1, 1));
        assert!(d.emails[0].mailboxes.contains(&ids.sent) && !d.emails[0].keywords.contains("$draft"));
    });
}

// RFC 8621 §1.3.1: `maxMailboxesPerEmail`; §4.6: `tooManyMailboxes`.
#[test]
fn a_copy_on_a_server_that_keeps_an_email_in_one_mailbox_is_moved_into_sent_once_it_is_sent() {
    let (stand_in, ids) = world();
    let archive = stand_in.with(|d| {
        d.max_calls = 1;
        d.no_duplicates = true;
        d.refuse_next.extend([None, None, None, Some((401, None))]);
        d.mailbox("Archive", None, Some("archive"))
    });
    assert!(sent_of(&send(&stand_in, vec![simple(41)])).is_empty(), "it waits");
    // Another program moves the copy to Archive, on a server that keeps an email in one mailbox.
    stand_in.with(|d| {
        d.max_calls = 16;
        d.profile.one_mailbox = true;
        d.emails[0].mailboxes = [archive.clone()].into_iter().collect();
    });
    let pieces = send(&stand_in, vec![simple(41)]);
    assert_eq!(sent_of(&pieces), [is(41, SendOutcome::Sent)]);
    stand_in.with(|d| {
        assert_eq!((d.submitted.len(), d.emails.len()), (1, 1));
        // Moved into Sent (one more mailbox would be one too many), and no draft any more.
        assert_eq!(d.emails[0].mailboxes, [ids.sent.clone()].into_iter().collect());
        assert!(!d.emails[0].keywords.contains("$draft"));
    });
    // Nothing was left to ask again.
    assert_eq!(stand_in.calls().iter().filter(|call| *call == "Email/set").count(), 0);
    assert!(last(&pieces).whole);
    // How a copy is filed: not at all where it lies already; beside the others; moved where one more is
    // too many; and one whose mailboxes are not said is taken for one mailbox.
    let answer = json!({"list": [{"id": "E1", "mailboxIds": {"M1": true, "M2": true}}, {"id": "E2"}]});
    assert_eq!(filing_for(&answer, "E1", "M2", Some(1)), None);
    assert_eq!(filing_for(&answer, "E1", "M3", None), Some(Filing { mailbox: "M3".into(), alone: false }));
    assert_eq!(filing_for(&answer, "E1", "M3", Some(3)), Some(Filing { mailbox: "M3".into(), alone: false }));
    assert_eq!(filing_for(&answer, "E1", "M3", Some(2)), Some(Filing { mailbox: "M3".into(), alone: true }));
    assert_eq!(filing_for(&answer, "E2", "M3", Some(1)), Some(Filing { mailbox: "M3".into(), alone: true }));
    assert_eq!(filing_for(&answer, "E2", "M3", Some(2)), Some(Filing { mailbox: "M3".into(), alone: false }));
}

thread_local! {
    static LOGGED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// A logger that keeps what this thread logs, for a test to read.
struct Kept;

impl log::Log for Kept {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        LOGGED.with(|logged| logged.borrow_mut().push(record.args().to_string()));
    }
    fn flush(&self) {}
}

/// Secrets never touch a log: nor does an id of the server's.
#[test]
fn what_a_send_logs_names_no_email_of_the_servers() {
    static KEPT: Kept = Kept;
    if log::set_logger(&KEPT).is_ok() {
        log::set_max_level(log::LevelFilter::Trace);
    }
    LOGGED.with(|logged| logged.borrow_mut().clear());
    // The server refuses the submission, and the request that takes the copy away again fails.
    let fake = sender_but(|req, _| {
        if req.method() != Method::Post || is_upload(req) {
            return None;
        }
        if names_of(req) == ["Email/set"] {
            return Some(status_reply(500, None));
        }
        Some(api_reply(req, |name, args| match name {
            "EmailSubmission/set" => Answer::Is(json!({"notCreated": {"s": {"type": "forbiddenToSend"}}})),
            _ => sending(name, args),
        }))
    });
    let pieces = send_fake(&fake, vec![simple(41)]);
    assert_eq!(sent_of(&pieces), [is(41, held("the server does not let this account send mail"))]);
    let logged = LOGGED.with(|logged| logged.borrow().clone());
    assert!(logged.iter().any(|line| line.contains("could not be tidied up")), "{logged:?}");
    for line in &logged {
        assert!(!line.contains("E50") && !line.contains("B7") && !line.contains(ME), "{line}");
    }
}
