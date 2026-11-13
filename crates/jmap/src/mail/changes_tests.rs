// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The changes of messages going up: what is asked of the server for each kind, what its answers are taken
//! for, in how many calls, in which pieces, and what a hostile answer makes of it. Over the stand-in and over
//! scripted servers; every message and address is invented.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{JmapChange, JmapChangeDone, JmapOutcome};
use proptest::prelude::*;
use serde_json::json;

use super::changes::{MAX_CHANGES, outcome_of, set_arguments};
use super::*;
use crate::connection::tests::any_json;
use crate::standin::{Profile, SESSION, StandIn, TOKEN};
use crate::test_support::made_secret;
use crate::{CAPABILITY_CORE as CORE, CAPABILITY_MAIL as MAIL, Credentials, HttpRequest, HttpResponse, Method, block_on};

pub(super) const ME: &str = "ada@example.org";
pub(super) const ID: u64 = 3;
/// The session of the scripted servers, and the token they are signed in to with.
pub(super) const FAKE_SESSION: &str = "https://jmap.example.org/jmap/session";
pub(super) const FAKE_TOKEN: &str = "tok-3n-s3cret-77";

/// What a scripted server answers to one request.
pub(super) type Reply = Result<HttpResponse, TransportError>;

/// A server that answers as a function says (given the request, and how many came before it), and keeps
/// what it was sent.
#[derive(Clone)]
pub(super) struct Fake {
    answer: Rc<dyn Fn(&HttpRequest, usize) -> Reply>,
    pub(super) sent: Rc<RefCell<Vec<HttpRequest>>>,
}

impl Fake {
    pub(super) fn new(answer: impl Fn(&HttpRequest, usize) -> Reply + 'static) -> Fake {
        Fake { answer: Rc::new(answer), sent: Rc::default() }
    }

    /// The arguments of each call to `method`, in the order they were made.
    pub(super) fn args_of(&self, method: &str) -> Vec<Value> {
        self.sent.borrow().iter().flat_map(calls_of).filter(|(name, _)| name == method).map(|(_, args)| args).collect()
    }

    /// The methods each request to the API called.
    pub(super) fn posts(&self) -> Vec<Vec<String>> {
        let sent = self.sent.borrow();
        let posts = sent.iter().filter(|req| req.method() == Method::Post);
        posts.map(|req| calls_of(req).into_iter().map(|(name, _)| name).collect()).collect()
    }
}

impl HttpExchange for Fake {
    async fn exchange(&self, req: &HttpRequest) -> Reply {
        let before = self.sent.borrow().len();
        self.sent.borrow_mut().push(req.clone());
        (self.answer)(req, before)
    }
}

/// The method calls of a request to the API: name and arguments.
pub(super) fn calls_of(req: &HttpRequest) -> Vec<(String, Value)> {
    let body: Value = serde_json::from_slice(req.body()).unwrap_or(Value::Null);
    let calls = body.get("methodCalls").and_then(Value::as_array).cloned().unwrap_or_default();
    calls.iter().map(|call| (call[0].as_str().unwrap_or_default().to_string(), call[1].clone())).collect()
}

pub(super) fn json_reply(status: u16, body: &Value) -> Reply {
    Ok(HttpResponse::new(status, serde_json::to_vec(body).unwrap()).with_field("Content-Type", "application/json"))
}

/// An answer of `status` with no body, and a `Retry-After` of so many seconds.
pub(super) fn status_reply(status: u16, retry_after: Option<u32>) -> Reply {
    let answer = HttpResponse::new(status, Vec::new());
    Ok(match retry_after {
        Some(seconds) => answer.with_field("Retry-After", &seconds.to_string()),
        None => answer,
    })
}

/// What a scripted server says to one method call.
pub(super) enum Answer {
    Is(Value),
    Error(&'static str, String),
    Missing,
}

/// The session of a scripted server: its core limits as `core` says.
pub(super) fn session_reply(core: Value) -> Reply {
    json_reply(
        200,
        &json!({
            "capabilities": {CORE: core, MAIL: {}},
            "accounts": {"A1": {"name": ME, "isPersonal": true, "accountCapabilities": {MAIL: {}}}},
            "primaryAccounts": {MAIL: "A1"},
            "apiUrl": "https://jmap.example.org/jmap/api",
            "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
            "uploadUrl": "/jmap/upload/{accountId}/",
            "state": "1",
        }),
    )
}

/// A request to the API answered call by call, by `f` (given the call's name and arguments).
pub(super) fn api_reply(req: &HttpRequest, f: impl Fn(&str, &Value) -> Answer) -> Reply {
    let body: Value = serde_json::from_slice(req.body()).unwrap_or(Value::Null);
    let calls = body.get("methodCalls").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut responses = Vec::new();
    for call in &calls {
        let (name, id) = (call[0].as_str().unwrap_or_default(), call[2].clone());
        match f(name, &call[1]) {
            Answer::Is(value) => responses.push(json!([name, value, id])),
            Answer::Error(kind, description) => responses.push(json!(["error", {"type": kind, "description": description}, id])),
            Answer::Missing => {}
        }
    }
    json_reply(200, &json!({"methodResponses": responses, "sessionState": "1"}))
}

/// What a server with an empty Inbox, Sent and Drafts answers to the calls that read: so that a run over a
/// scripted server can go on to read after what the test is about.
pub(super) fn reading(name: &str, _args: &Value) -> Answer {
    match name {
        "Mailbox/get" => Answer::Is(json!({"accountId": "A1", "state": "1", "notFound": [], "list": [
            {"id": "M1", "name": "Inbox", "parentId": null, "role": "inbox", "totalEmails": 0, "unreadEmails": 0},
            {"id": "M2", "name": "Sent", "parentId": null, "role": "sent", "totalEmails": 0, "unreadEmails": 0},
            {"id": "M3", "name": "Drafts", "parentId": null, "role": "drafts", "totalEmails": 0, "unreadEmails": 0},
        ]})),
        "Email/query" => {
            Answer::Is(json!({"accountId": "A1", "queryState": "1", "canCalculateChanges": false, "position": 0, "ids": [], "total": 0}))
        }
        "Email/get" => Answer::Is(json!({"accountId": "A1", "state": "1", "list": [], "notFound": []})),
        _ => Answer::Missing,
    }
}

/// A scripted server of the standard kind: the session, and each call answered by `f`, or as [`reading`]
/// answers it when `f` has nothing to say (`None`).
pub(super) fn server(f: impl Fn(&str, &Value) -> Option<Answer> + 'static) -> Fake {
    Fake::new(move |req, _| {
        if req.method() == Method::Get {
            return session_reply(json!({"maxCallsInRequest": 16}));
        }
        api_reply(req, |name, args| f(name, args).unwrap_or_else(|| reading(name, args)))
    })
}

/// A run that lists the mailboxes and reads them all, for the account on the server at `session`.
pub(super) fn asks(session: &str) -> JmapMailRequest {
    JmapMailRequest {
        account: ID,
        email: ME.into(),
        session: session.into(),
        bearer: true,
        window: 50,
        list_folders: true,
        scope: JmapScope::All,
        ..Default::default()
    }
}

/// Every piece of a run over `http`, signed in with `token`.
pub(super) fn run_over<H: HttpExchange>(http: H, token: &str, request: &JmapMailRequest, cancel: &AtomicBool) -> Vec<JmapMailResult> {
    let pieces = run_unchecked(http, token, request, cancel);
    shaped(&pieces);
    pieces
}

/// [`run_over`], for a request that is not for the account of these tests: the pieces as they came.
pub(super) fn run_unchecked<H: HttpExchange>(http: H, token: &str, request: &JmapMailRequest, cancel: &AtomicBool) -> Vec<JmapMailResult> {
    let connection = block_on(Connection::connect(http, &request.session, Credentials::bearer(&made_secret(token)).unwrap())).unwrap();
    let mut pieces = Vec::new();
    block_on(sync_mail(&connection, request, cancel, &mut |piece| pieces.push(piece)));
    pieces
}

pub(super) fn run_fake(fake: &Fake, request: &JmapMailRequest) -> Vec<JmapMailResult> {
    run_over(fake.clone(), FAKE_TOKEN, request, &AtomicBool::new(false))
}

pub(super) fn run_stand_in(stand_in: &StandIn, request: &JmapMailRequest) -> Vec<JmapMailResult> {
    run_over(stand_in.clone(), TOKEN, request, &AtomicBool::new(false))
}

/// What every run is like: pieces for the account, all `partial` but the last; each of the others one thing
/// (what became of some changes, the list, or a window), and in that order.
pub(super) fn shaped(pieces: &[JmapMailResult]) {
    let (last, rest) = pieces.split_last().expect("a run ends with a piece");
    assert!(!last.partial && last.pages.is_empty() && last.listing.is_none() && last.changes.is_empty(), "{last:?}");
    assert!(rest.iter().all(|piece| piece.partial && piece.errors.is_empty() && !piece.whole && !piece.stopped), "{rest:?}");
    assert!(pieces.iter().all(|piece| piece.account == ID && piece.email == ME && !piece.session.is_empty() && piece.sent.is_empty()));
    let kinds: Vec<usize> = rest
        .iter()
        .map(|piece| match (piece.changes.len(), &piece.listing, piece.pages.len()) {
            (1.., None, 0) => 0,
            (0, Some(_), 0) => 1,
            (0, None, 1) => 2,
            _ => panic!("a piece that is not one thing: {piece:?}"),
        })
        .collect();
    assert!(kinds.is_sorted(), "the changes, the list, the windows: {kinds:?}");
    assert!(!(last.whole && (last.stopped || last.auth_refused || last.busy_for.is_some() || !last.errors.is_empty())), "{last:?}");
}

pub(super) fn last(pieces: &[JmapMailResult]) -> &JmapMailResult {
    pieces.last().expect("a run ends with a piece")
}

pub(super) fn changes_of(pieces: &[JmapMailResult]) -> Vec<JmapChangeDone> {
    pieces.iter().flat_map(|piece| piece.changes.iter().cloned()).collect()
}

/// Whether the run went on to read: it has a window.
pub(super) fn read_on(pieces: &[JmapMailResult]) -> bool {
    pieces.iter().any(|piece| !piece.pages.is_empty())
}

fn mail(subject: &str) -> Vec<u8> {
    format!("From: Zoe Park <zoe@example.org>\r\nTo: Ada <{ME}>\r\nSubject: {subject}\r\nMessage-ID: <{subject}@example.org>\r\n\r\nHello.\r\n")
        .into_bytes()
}

struct Ids {
    inbox: String,
    sent: String,
    projects: String,
}

/// A stand-in with an Inbox, a Sent and a folder, and no mail.
fn world() -> (StandIn, Ids) {
    let stand_in = StandIn::new();
    let ids = stand_in.with(|d| Ids {
        inbox: d.mailbox("Inbox", None, Some("inbox")),
        sent: d.mailbox("Sent", None, Some("sent")),
        projects: d.mailbox("Projects", None, None),
    });
    (stand_in, ids)
}

fn deliver(stand_in: &StandIn, mailbox: &str, subject: &str, keywords: &[&str]) -> String {
    stand_in.with(|d| d.deliver(mailbox, &mail(subject), keywords, "2026-10-15T06:00:00Z"))
}

fn marks(id: &str, read: Option<bool>, flagged: Option<bool>, answered: Option<bool>, forwarded: Option<bool>) -> JmapChange {
    JmapChange::Keywords { message: 11, id: id.into(), read, flagged, answered, forwarded }
}

fn moved(id: &str, from: &str, to: &str) -> JmapChange {
    JmapChange::Move { message: 12, id: id.into(), from: from.into(), to: to.into() }
}

fn destroyed(id: &str) -> JmapChange {
    JmapChange::Destroy { id: id.into() }
}

fn done() -> JmapOutcome {
    JmapOutcome::Done { id: String::new() }
}

/// A run over the stand-in that takes `changes` up.
fn change(stand_in: &StandIn, changes: Vec<JmapChange>) -> Vec<JmapMailResult> {
    run_stand_in(stand_in, &JmapMailRequest { changes, ..asks(SESSION) })
}

/// A run over a scripted server that takes `changes` up.
fn change_fake(fake: &Fake, changes: Vec<JmapChange>) -> Vec<JmapMailResult> {
    run_fake(fake, &JmapMailRequest { changes, ..asks(FAKE_SESSION) })
}

/// A server that says of every email an `Email/set` names that it was updated or destroyed.
fn agreeing() -> Fake {
    server(|name, args| {
        (name == "Email/set").then(|| {
            let updated: Map<String, Value> = args["update"].as_object().into_iter().flatten().map(|(id, _)| (id.clone(), Value::Null)).collect();
            Answer::Is(json!({"accountId": "A1", "updated": updated, "destroyed": args["destroy"].clone()}))
        })
    })
}

#[test]
fn each_kind_of_change_is_done_before_the_mailboxes_are_read_and_what_is_read_shows_it() {
    let (stand_in, ids) = world();
    let first = deliver(&stand_in, &ids.inbox, "first", &["$flagged", "$custom"]);
    let second = deliver(&stand_in, &ids.inbox, "second", &["$seen"]);
    let third = deliver(&stand_in, &ids.inbox, "third", &[]);
    let changes = vec![marks(&first, Some(true), Some(false), Some(true), Some(true)), moved(&second, &ids.inbox, &ids.projects), destroyed(&third)];
    let pieces = change(&stand_in, changes.clone());

    // One piece with what became of the three, before the list and the windows.
    assert_eq!(pieces[0].changes, changes.iter().map(|change| JmapChangeDone { change: change.clone(), outcome: done() }).collect::<Vec<_>>());
    assert!(pieces[0].listing.is_none() && pieces[1].listing.is_some());
    assert_eq!(changes_of(&pieces).len(), 3);
    let end = last(&pieces);
    assert!(end.whole && end.errors.is_empty(), "{end:?}");

    // The server has it: only the marks named were touched, the move moved, the deleted is gone.
    stand_in.with(|d| {
        let keywords: Vec<&str> = d.email(&first).unwrap().keywords.iter().map(String::as_str).collect();
        assert_eq!(keywords, ["$answered", "$custom", "$forwarded", "$seen"]);
        assert_eq!(d.email(&second).unwrap().mailboxes.iter().collect::<Vec<_>>(), [&ids.projects]);
        assert!(d.email(&second).unwrap().keywords.contains("$seen"), "a move leaves the marks alone");
        assert!(d.email(&third).is_none());
    });
    // One `Email/set` for all three, and it came before anything was read.
    assert_eq!(stand_in.calls().iter().filter(|call| *call == "Email/set").count(), 1);
    assert_eq!(stand_in.calls()[..2], ["Email/set", "Mailbox/get"]);
    // What was read afterwards shows the result.
    let page = |mailbox: &str| pieces.iter().flat_map(|p| p.pages.iter()).find(|page| page.folder == mailbox).unwrap();
    let inbox = page(&ids.inbox);
    assert_eq!(inbox.messages.iter().map(|m| m.server.id.as_str()).collect::<Vec<_>>(), [first.as_str()]);
    assert!(inbox.messages[0].server.read && inbox.messages[0].server.answered && inbox.messages[0].server.forwarded);
    assert_eq!(page(&ids.projects).messages.iter().map(|m| m.server.id.as_str()).collect::<Vec<_>>(), [second.as_str()]);
    let listing = pieces[1].listing.as_ref().unwrap();
    assert_eq!(listing.folders.iter().map(|f| (f.total, f.unread)).collect::<Vec<_>>(), [(1, 0), (0, 0), (1, 0)], "the counts are the ones after");
    assert!(ids.sent != ids.inbox);
}

#[test]
fn the_patches_touch_the_named_keywords_and_mailboxes_and_nothing_else() {
    let fake = agreeing();
    let changes = vec![
        marks("E1", Some(true), Some(false), None, None),
        marks("E2", None, None, Some(false), Some(true)),
        marks("E3", Some(false), None, None, None),
        moved("E4", "M1", "M9"),
        destroyed("E5"),
        destroyed("E6"),
    ];
    let pieces = change_fake(&fake, changes.clone());
    assert_eq!(changes_of(&pieces).iter().map(|done| done.outcome.clone()).collect::<Vec<_>>(), vec![done(); 6]);
    // The whole of the call: the account, the patches, the ids to destroy.
    assert_eq!(
        fake.args_of("Email/set"),
        [json!({
            "accountId": "A1",
            "update": {
                "E1": {"keywords/$seen": true, "keywords/$flagged": null},
                "E2": {"keywords/$answered": null, "keywords/$forwarded": true},
                "E3": {"keywords/$seen": null},
                "E4": {"mailboxIds/M1": null, "mailboxIds/M9": true},
            },
            "destroy": ["E5", "E6"],
        })]
    );
    // It is a request of its own, the first of the run, and names mail's capability and no other.
    let sent = fake.sent.borrow();
    let first: Value = serde_json::from_slice(sent[1].body()).unwrap();
    assert_eq!(first["using"], json!([CORE, MAIL]));
    assert_eq!(first["methodCalls"].as_array().unwrap().len(), 1);
    assert_eq!(fake.posts()[0], ["Email/set"]);
    // A call with nothing to update has no `update`, and one with nothing to destroy no `destroy`.
    let only = set_arguments("A1", &[(&destroyed("E5"), "E5".to_string())]);
    assert_eq!(Value::Object(only), json!({"accountId": "A1", "destroy": ["E5"]}));
    let only = set_arguments("A1", &[(&marks("E1", Some(true), None, None, None), "E1".to_string())]);
    assert_eq!(Value::Object(only), json!({"accountId": "A1", "update": {"E1": {"keywords/$seen": true}}}));
}

#[test]
fn a_move_that_gives_the_email_a_new_id_says_it_and_the_later_changes_of_the_run_name_it_by_that() {
    let (stand_in, ids) = world();
    stand_in.with(|d| d.profile = Profile::by_place());
    let email = deliver(&stand_in, &ids.inbox, "moving", &[]);
    let changes = vec![moved(&email, &ids.inbox, &ids.projects), marks(&email, Some(true), None, None, None)];
    let pieces = change(&stand_in, changes.clone());
    let new = stand_in.with(|d| d.emails[0].id.clone());
    assert_ne!(new, email);
    // The move says the new id; the marks, asked under the old one here, went to the email all the same,
    // and are not taken for those of a message that is gone.
    assert_eq!(
        changes_of(&pieces),
        [
            JmapChangeDone { change: changes[0].clone(), outcome: JmapOutcome::Done { id: new.clone() } },
            JmapChangeDone { change: changes[1].clone(), outcome: done() },
        ]
    );
    stand_in.with(|d| {
        let moved = d.email(&new).unwrap();
        assert!(moved.keywords.contains("$seen") && moved.mailboxes.contains(&ids.projects) && moved.mailboxes.len() == 1);
    });
    // Two calls: a call never holds two changes of one email.
    let sets: Vec<Value> = stand_in.seen().iter().flat_map(|seen| seen.calls.clone()).filter(|call| call == "Email/set").map(Value::from).collect();
    assert_eq!(sets.len(), 2);
    assert!(last(&pieces).whole);
}

#[test]
fn a_message_the_server_no_longer_has_is_gone() {
    let (stand_in, ids) = world();
    let kept = deliver(&stand_in, &ids.inbox, "kept", &[]);
    let changes = vec![marks("E999", Some(true), None, None, None), destroyed("E998"), moved("E997", &ids.inbox, &ids.sent), destroyed(&kept)];
    let pieces = change(&stand_in, changes);
    let outcomes: Vec<JmapOutcome> = changes_of(&pieces).into_iter().map(|done| done.outcome).collect();
    assert_eq!(outcomes, [JmapOutcome::Gone, JmapOutcome::Gone, JmapOutcome::Gone, done()]);
    // Nothing wrong with the run.
    assert!(last(&pieces).whole && last(&pieces).errors.is_empty());
}

#[test]
fn a_refusal_says_why_without_an_address_or_the_secret() {
    let (stand_in, ids) = world();
    let (one, two, three) =
        (deliver(&stand_in, &ids.inbox, "one", &[]), deliver(&stand_in, &ids.inbox, "two", &[]), deliver(&stand_in, &ids.inbox, "three", &[]));
    stand_in.with(|d| {
        let said = format!("zoe@example.org may not change this for {ME} (token {TOKEN}) at {SESSION}");
        d.refused_changes.insert(one.clone(), ("forbidden".into(), said));
        d.refused_changes.insert(two.clone(), ("tooManyKeywords".into(), String::new()));
        d.refused_changes.insert(three.clone(), (format!("x{TOKEN}"), String::new()));
    });
    // (A move into a mailbox the server has not is refused by the server itself.)
    let four = deliver(&stand_in, &ids.inbox, "four", &[]);
    let changes = vec![marks(&one, Some(true), None, None, None), destroyed(&two), destroyed(&three), moved(&four, &ids.inbox, "M404")];
    let pieces = change(&stand_in, changes);
    let whys: Vec<String> = changes_of(&pieces)
        .into_iter()
        .map(|done| match done.outcome {
            JmapOutcome::Refused { why } => why,
            other => panic!("not refused: {other:?}"),
        })
        .collect();
    assert_eq!(whys.len(), 4);
    for why in &whys {
        for hidden in ["zoe@example.org", ME, TOKEN, "example.org", SESSION, "\n"] {
            assert!(!why.contains(hidden), "{hidden} shows in {why:?}");
        }
        assert!(!why.is_empty() && why.chars().count() <= 200, "{why:?}");
    }
    // The server's words where it gave some, cleaned; else the kind of error; else that it did not say.
    assert!(whys[0].contains("may not change this"), "{:?}", whys[0]);
    assert_eq!(whys[1], "tooManyKeywords");
    assert_eq!(whys[2], "the server refused it without saying why");
    assert!(whys[3].contains("mailbox"), "{:?}", whys[3]);
    // A refusal is told with the change and is nothing wrong with the run: no line, and it is whole.
    let end = last(&pieces);
    assert!(end.whole && end.errors.is_empty() && read_on(&pieces), "{end:?}");
    assert!(stand_in.with(|d| d.email(&two).is_some()));
}

#[test]
fn a_change_the_answer_does_not_mention_is_not_reported_and_the_later_ones_of_its_email_wait() {
    // The server answers for E1 and says nothing of E2 (nor of E4, which it is asked to destroy).
    let fake = server(|name, _| {
        (name == "Email/set").then(|| Answer::Is(json!({"accountId": "A1", "updated": {"E1": null, "E3": null}, "notUpdated": {}})))
    });
    let changes = vec![
        marks("E1", Some(true), None, None, None),
        marks("E2", Some(true), None, None, None),
        destroyed("E4"),
        // The same emails again: a call of their own.
        moved("E2", "M1", "M2"),
        marks("E3", None, Some(true), None, None),
    ];
    let pieces = change_fake(&fake, changes.clone());
    assert_eq!(
        changes_of(&pieces),
        [JmapChangeDone { change: changes[0].clone(), outcome: done() }, JmapChangeDone { change: changes[4].clone(), outcome: done() }]
    );
    // E2's move was not asked: its first change got no answer, so the order would not hold.
    let sets = fake.args_of("Email/set");
    assert_eq!(sets.len(), 2);
    assert_eq!(sets[1], json!({"accountId": "A1", "update": {"E3": {"keywords/$flagged": true}}}));
    // Nothing is wrong with the run for it: the engine works the unanswered ones out again.
    assert!(last(&pieces).errors.is_empty() && read_on(&pieces));
}

#[test]
fn after_a_refusal_the_later_changes_of_that_email_wait_for_the_next_run() {
    let fake = server(|name, args| {
        (name == "Email/set").then(|| {
            let refused = args["update"].get("E1").map(|_| json!({"E1": {"type": "forbidden"}}));
            Answer::Is(json!({"accountId": "A1", "updated": {"E2": null}, "notUpdated": refused}))
        })
    });
    let changes = vec![moved("E1", "M1", "M2"), moved("E1", "M2", "M3"), marks("E2", Some(true), None, None, None)];
    let pieces = change_fake(&fake, changes.clone());
    assert_eq!(
        changes_of(&pieces),
        [
            JmapChangeDone { change: changes[0].clone(), outcome: JmapOutcome::Refused { why: "forbidden".into() } },
            JmapChangeDone { change: changes[2].clone(), outcome: done() },
        ]
    );
    assert_eq!(fake.args_of("Email/set").len(), 2, "the second move of E1 was never asked");
    assert!(fake.args_of("Email/set").iter().all(|args| args["update"].get("E1").is_none_or(|patch| patch.get("mailboxIds/M3").is_none())));
}

#[test]
fn changes_go_as_many_to_a_call_as_the_server_takes_and_500_to_a_run() {
    // Two to a call: five changes are three calls, a piece for each.
    let (stand_in, ids) = world();
    stand_in.with(|d| d.max_set = 2);
    let emails: Vec<String> = (0..5).map(|n| deliver(&stand_in, &ids.inbox, &format!("m{n}"), &[])).collect();
    let pieces = change(&stand_in, emails.iter().map(|id| marks(id, Some(true), None, None, None)).collect());
    assert_eq!(pieces.iter().map(|piece| piece.changes.len()).filter(|n| *n > 0).collect::<Vec<_>>(), [2, 2, 1]);
    assert_eq!(stand_in.calls().iter().filter(|call| *call == "Email/set").count(), 3);
    assert!(stand_in.with(|d| d.emails.iter().all(|e| e.keywords.contains("$seen"))));
    assert!(last(&pieces).whole);

    // 500 to a run, whatever the request brings: the rest get no answer, and come again.
    let fake = agreeing();
    let many: Vec<JmapChange> = (0..MAX_CHANGES + 7).map(|n| marks(&format!("E{n}"), Some(true), None, None, None)).collect();
    let pieces = change_fake(&fake, many.clone());
    let answered = changes_of(&pieces);
    assert_eq!(answered.len(), MAX_CHANGES);
    assert_eq!(answered.last().map(|done| &done.change), many.get(MAX_CHANGES - 1));
    assert_eq!(fake.args_of("Email/set").iter().map(|args| args["update"].as_object().unwrap().len()).collect::<Vec<_>>(), [500]);
    assert!(last(&pieces).whole, "what is left for the next run is nothing wrong");

    // A server that takes one at a time is asked one at a time.
    let one = Fake::new(|req, _| {
        if req.method() == Method::Get {
            return session_reply(json!({"maxCallsInRequest": 16, "maxObjectsInSet": 1}));
        }
        api_reply(req, |name, args| match name {
            "Email/set" => {
                Answer::Is(json!({"updated": args["update"].as_object().unwrap().keys().map(|id| (id.clone(), Value::Null)).collect::<Map<_, _>>()}))
            }
            _ => reading(name, args),
        })
    });
    let pieces = change_fake(&one, many[..3].to_vec());
    assert_eq!((changes_of(&pieces).len(), one.args_of("Email/set").len()), (3, 3));
}

#[test]
fn an_id_no_server_hands_out_is_refused_without_asking_and_no_mark_named_is_nothing_to_do() {
    let fake = agreeing();
    let long = "x".repeat(256);
    let changes = vec![
        marks("a b", Some(true), None, None, None),
        marks("", Some(true), None, None, None),
        destroyed("E1/../E2"),
        destroyed(&long),
        moved("E1", "M1", "keywords/$seen"),
        moved("E1", "M 1", "M2"),
        moved("E\r\n1", "M1", "M2"),
        marks("E7", None, None, None, None),
    ];
    let pieces = change_fake(&fake, changes.clone());
    let outcomes: Vec<JmapOutcome> = changes_of(&pieces).into_iter().map(|done| done.outcome).collect();
    let refused = |why: &str| JmapOutcome::Refused { why: why.into() };
    let (id, mailbox) = (refused("the message's id is not one a server hands out"), refused("a mailbox's id is not one a server hands out"));
    assert_eq!(outcomes, [id.clone(), id.clone(), id.clone(), id.clone(), mailbox.clone(), mailbox, id, done()]);
    assert!(fake.args_of("Email/set").is_empty(), "nothing was asked");
    assert!(last(&pieces).whole && read_on(&pieces));
    // With one that can be asked among them, only that one is.
    let fake = agreeing();
    let pieces = change_fake(&fake, vec![destroyed("no/id"), destroyed("E1")]);
    assert_eq!(fake.args_of("Email/set"), [json!({"accountId": "A1", "destroy": ["E1"]})]);
    assert_eq!(changes_of(&pieces).len(), 2);
}

#[test]
fn a_call_that_fails_as_a_whole_answers_none_of_its_changes_and_the_run_goes_on_to_read() {
    let changes = vec![marks("E1", Some(true), None, None, None), destroyed("E2")];
    // The server answers the call with an error.
    let refusing = server(|name, _| (name == "Email/set").then(|| Answer::Error("accountReadOnly", format!("{ME} may not write"))));
    // The server leaves the call out of its answer.
    let silent = server(|name, _| (name == "Email/set").then_some(Answer::Missing));
    // The connection goes for that one request.
    let dropped = Fake::new(|req, n| match (req.method(), n) {
        (Method::Get, _) => session_reply(json!({})),
        (_, 1) => Err(TransportError::TimedOut),
        _ => api_reply(req, reading),
    });
    // The answer is no JMAP.
    let garbled = Fake::new(|req, n| match (req.method(), n) {
        (Method::Get, _) => session_reply(json!({})),
        (_, 1) => json_reply(200, &json!({"methodResponses": "no"})),
        _ => api_reply(req, reading),
    });
    for (what, fake) in [("an error", refusing), ("no answer to the call", silent), ("no connection", dropped), ("no JMAP", garbled)] {
        let pieces = change_fake(&fake, changes.clone());
        assert!(changes_of(&pieces).is_empty(), "{what}");
        let end = last(&pieces);
        assert!(!end.whole && !end.stopped && !end.auth_refused && end.busy_for.is_none(), "{what}: {end:?}");
        assert_eq!(end.errors.len(), 1, "{what}: {:?}", end.errors);
        assert!(end.errors[0].starts_with("Couldn't take the changes of messages up to the server: "), "{what}: {:?}", end.errors);
        assert!(!end.errors[0].contains(ME) && !end.errors[0].contains(FAKE_TOKEN), "{what}: {:?}", end.errors);
        assert!(read_on(&pieces) && pieces.iter().any(|piece| piece.listing.is_some()), "{what}: the run goes on to read");
        assert_eq!(fake.args_of("Email/set").len(), 1, "{what}: asked once");
    }
    // After a call failed, the calls that would follow it are not made in this run.
    let failing = Fake::new(|req, _| {
        if req.method() == Method::Get {
            return session_reply(json!({"maxObjectsInSet": 1}));
        }
        api_reply(req, |name, args| if name == "Email/set" { Answer::Error("serverFail", String::new()) } else { reading(name, args) })
    });
    let pieces = change_fake(&failing, changes);
    assert_eq!(failing.args_of("Email/set").len(), 1);
    assert!(changes_of(&pieces).is_empty() && read_on(&pieces));
}

#[test]
fn a_refused_sign_in_a_busy_server_and_a_stop_end_the_run_at_the_changes() {
    let changes = vec![marks("E1", Some(true), None, None, None)];
    let answering = |status: u16, retry_after: Option<u32>| {
        Fake::new(move |req, _| if req.method() == Method::Get { session_reply(json!({})) } else { status_reply(status, retry_after) })
    };
    // 401: the owner signs in again, and nothing is read.
    let fake = answering(401, None);
    let pieces = change_fake(&fake, changes.clone());
    let end = last(&pieces);
    assert!(end.auth_refused && !end.whole && end.errors.is_empty() && pieces.len() == 1, "{end:?}");
    assert_eq!(fake.posts(), [["Email/set"]]);
    // 429 and 503: the server is left alone as long as it asks, and nothing more is asked.
    for (status, retry_after, seconds) in [(429, Some(120), 120), (503, None, 30)] {
        let fake = answering(status, retry_after);
        let pieces = change_fake(&fake, changes.clone());
        let end = last(&pieces);
        assert_eq!((end.busy_for, end.whole, end.auth_refused, pieces.len()), (Some(seconds), false, false, 1), "{status}");
        assert_eq!(fake.posts().len(), 1, "{status}");
    }
    // Stopped before it began: nothing is asked, and the changes come again.
    let fake = agreeing();
    let cancel = AtomicBool::new(true);
    let pieces = run_over(fake.clone(), FAKE_TOKEN, &JmapMailRequest { changes: changes.clone(), ..asks(FAKE_SESSION) }, &cancel);
    assert!(last(&pieces).stopped && pieces.len() == 1 && fake.posts().is_empty());
    // Stopped by the owner while the second call would go: the first call's answers are handed on.
    let cancel = Rc::new(AtomicBool::new(false));
    let stops = cancel.clone();
    let fake = Fake::new(move |req, _| {
        if req.method() == Method::Get {
            return session_reply(json!({"maxObjectsInSet": 1}));
        }
        stops.store(true, std::sync::atomic::Ordering::Relaxed);
        api_reply(req, |_, args| Answer::Is(json!({"updated": {"E1": null}, "destroyed": args["destroy"].clone()})))
    });
    let two = vec![marks("E1", Some(true), None, None, None), destroyed("E2")];
    let pieces = run_over(fake.clone(), FAKE_TOKEN, &JmapMailRequest { changes: two.clone(), ..asks(FAKE_SESSION) }, &cancel);
    assert_eq!(changes_of(&pieces), [JmapChangeDone { change: two[0].clone(), outcome: done() }]);
    assert!(last(&pieces).stopped && fake.posts().len() == 1);
}

#[test]
fn a_hostile_answer_is_read_for_the_ids_asked_and_nothing_else() {
    let change = marks("E1", Some(true), None, None, None);
    let gone = destroyed("E1");
    let secrets = Redactor::new();
    let of = |answer: Value, change: &JmapChange| outcome_of(&answer, change, "E1", &secrets);
    // Wrong types say nothing.
    for answer in [
        json!(null),
        json!([]),
        json!({"updated": ["E1"], "destroyed": {"E1": true}}),
        json!({"updated": "E1", "notUpdated": ["E1"], "destroyed": "E1", "notDestroyed": 7}),
        json!({"updated": {"E2": null}, "destroyed": ["E2", 7, null, {"E1": 1}], "notUpdated": {"E2": {"type": "notFound"}}}),
    ] {
        assert_eq!((of(answer.clone(), &change), of(answer.clone(), &gone)), (None, None), "{answer}");
    }
    // A new id is believed only when it is an id, and another one.
    let new = |entry: Value| of(json!({"updated": {"E1": entry}}), &change);
    assert_eq!(new(json!({"id": "E9"})), Some(JmapOutcome::Done { id: "E9".into() }));
    for entry in [
        json!(null),
        json!(7),
        json!("E9"),
        json!({"id": "E1"}),
        json!({"id": "a b"}),
        json!({"id": 9}),
        json!({"id": "x".repeat(300)}),
        json!({"blobId": "B2"}),
    ] {
        assert_eq!(new(entry.clone()), Some(done()), "{entry}");
    }
    // An update is not answered by what the server says it destroyed, nor the other way round.
    assert_eq!(of(json!({"destroyed": ["E1"], "notDestroyed": {"E1": {"type": "notFound"}}}), &change), None);
    assert_eq!(of(json!({"updated": {"E1": null}, "notUpdated": {"E1": {"type": "notFound"}}}), &gone), None);
    // Done wins over a failure the same answer claims: a message is not taken for gone on a muddle.
    assert_eq!(of(json!({"updated": {"E1": null}, "notUpdated": {"E1": {"type": "notFound"}}}), &change), Some(done()));
    // An error that is no object, or has no type, is a refusal that does not say why; a huge one is cut.
    for error in [json!("no"), json!(null), json!({}), json!({"type": 7}), json!({"type": "not found"}), json!({"type": "x".repeat(65)})] {
        let outcome = of(json!({"notUpdated": {"E1": error.clone()}}), &change);
        assert_eq!(outcome, Some(JmapOutcome::Refused { why: "the server refused it without saying why".into() }), "{error}");
    }
    let huge = of(json!({"notDestroyed": {"E1": {"type": "forbidden", "description": "word ".repeat(100_000)}}}), &gone);
    let Some(JmapOutcome::Refused { why }) = huge else { panic!("not refused") };
    assert!(why.starts_with("word word") && why.chars().count() <= 200, "{} characters", why.chars().count());
    // Thousands of entries: the one asked for is found, by its id.
    let mut updated: Map<String, Value> = (0..20_000).map(|n| (format!("X{n}"), Value::Null)).collect();
    assert_eq!(of(json!({"updated": updated.clone()}), &change), None);
    updated.insert("E1".into(), Value::Null);
    assert_eq!(of(json!({"updated": updated}), &change), Some(done()));
    let listed: Vec<String> = (0..20_000).map(|n| format!("X{n}")).collect();
    assert_eq!(of(json!({"destroyed": listed}), &gone), None);

    // And a whole run over such a server ends, with nothing reported that was not said.
    let fake = server(|name, _| {
        (name == "Email/set")
            .then(|| Answer::Is(json!({"updated": ["E1"], "notUpdated": "E1", "destroyed": {"E2": true}, "notDestroyed": [[["E2"]]]})))
    });
    let pieces = change_fake(&fake, vec![change, destroyed("E2")]);
    assert!(changes_of(&pieces).is_empty() && read_on(&pieces));
}

/// Whether a call is the look for the emails an `Email/set` said nothing of.
fn is_look(name: &str, args: &Value) -> bool {
    name == "Email/get" && args["properties"] == json!(["id"])
}

/// A server whose `Email/set` answers for `E1` alone, and whose look for the others is answered by `look`.
fn passing_over(look: impl Fn() -> Answer + 'static) -> Fake {
    server(move |name, args| match name {
        "Email/set" => Some(Answer::Is(json!({"accountId": "A1", "updated": {"E1": null}}))),
        _ if is_look(name, args) => Some(look()),
        _ => None,
    })
}

/// The changes those servers are asked: `E1` is answered for, the others are not.
fn passed_over() -> Vec<JmapChange> {
    vec![
        marks("E1", Some(true), None, None, None),
        destroyed("E2"),
        marks("E3", None, Some(true), None, None),
        moved("E4", "M1", "M2"),
        destroyed("E5"),
    ]
}

fn body_of(req: &HttpRequest) -> Value {
    serde_json::from_slice(req.body()).unwrap()
}

#[test]
fn an_email_the_answer_passes_over_is_looked_up_and_one_the_server_has_not_got_is_gone() {
    // The server has E3 still, has neither E2 nor E4, and says nothing of E5 here either.
    let fake = passing_over(|| Answer::Is(json!({"accountId": "A1", "state": "1", "list": [{"id": "E3"}], "notFound": ["E2", "E4"]})));
    let changes = passed_over();
    let pieces = change_fake(&fake, changes.clone());
    assert_eq!(
        changes_of(&pieces),
        [
            JmapChangeDone { change: changes[0].clone(), outcome: done() },
            // A destroy and an update the answer left out, of emails the server has not got.
            JmapChangeDone { change: changes[1].clone(), outcome: JmapOutcome::Gone },
            JmapChangeDone { change: changes[3].clone(), outcome: JmapOutcome::Gone },
        ]
    );
    // One look, for the ids alone, of the emails that went unanswered and no other; then the run reads.
    let posts = fake.posts();
    assert_eq!(posts[..2], [vec!["Email/set".to_string()], vec!["Email/get".to_string()]]);
    let looks: Vec<Value> =
        fake.sent.borrow().iter().filter(|req| calls_of(req).iter().any(|(name, args)| is_look(name, args))).map(body_of).collect();
    assert_eq!(
        looks,
        [json!({
            "using": [CORE, MAIL],
            "methodCalls": [["Email/get", {"accountId": "A1", "ids": ["E2", "E3", "E4", "E5"], "properties": ["id"]}, "c0"]],
        })]
    );
    // Nothing is wrong with the run for it: what was not reported is asked again with the next.
    let end = last(&pieces);
    assert!(end.whole && end.errors.is_empty() && read_on(&pieces), "{end:?}");

    // A server that keeps to the standard is not asked a second time.
    let fake = agreeing();
    change_fake(&fake, passed_over());
    assert!(fake.sent.borrow().iter().flat_map(calls_of).all(|(name, args)| !is_look(&name, &args)));
}

#[test]
fn an_email_that_is_looked_up_and_found_is_not_reported_and_its_later_changes_wait() {
    let fake = passing_over(|| Answer::Is(json!({"list": [{"id": "E2"}, {"id": "E3"}], "notFound": []})));
    // E3 again after it went unanswered: not asked in this run, so that the order holds.
    let mut changes = passed_over();
    changes.push(destroyed("E3"));
    let pieces = change_fake(&fake, changes.clone());
    assert_eq!(changes_of(&pieces), [JmapChangeDone { change: changes[0].clone(), outcome: done() }]);
    assert_eq!(fake.args_of("Email/set").len(), 1);
    assert!(last(&pieces).whole && read_on(&pieces));
}

#[test]
fn a_look_that_fails_reports_nothing_of_those_emails_and_the_run_goes_on_to_read() {
    let failing = passing_over(|| Answer::Error("serverFail", format!("{ME} can not be read")));
    let unknown = passing_over(|| Answer::Error("unknownMethod", String::new()));
    let silent = passing_over(|| Answer::Missing);
    let answering_third = |third: fn() -> Reply| {
        Fake::new(move |req, n| match (req.method(), n) {
            (Method::Get, _) => session_reply(json!({})),
            (_, 1) => api_reply(req, |_, _| Answer::Is(json!({"updated": {"E1": null}}))),
            (_, 2) => third(),
            _ => api_reply(req, reading),
        })
    };
    let dropped = answering_third(|| Err(TransportError::TimedOut));
    let garbled = answering_third(|| json_reply(200, &json!({"methodResponses": "no"})));
    let broken = answering_third(|| status_reply(500, None));
    let cases = [
        ("an error", failing),
        ("no such method", unknown),
        ("no answer", silent),
        ("no connection", dropped),
        ("no JMAP", garbled),
        ("500", broken),
    ];
    for (what, fake) in cases {
        let changes = passed_over();
        let pieces = change_fake(&fake, changes.clone());
        // What the set answered is told; what it did not is left for the next run, as before there was a look.
        assert_eq!(changes_of(&pieces), [JmapChangeDone { change: changes[0].clone(), outcome: done() }], "{what}");
        let end = last(&pieces);
        assert!(!end.stopped && !end.auth_refused && end.busy_for.is_none(), "{what}: {end:?}");
        assert!(end.errors.iter().all(|line| !line.contains(ME) && !line.contains(FAKE_TOKEN)), "{what}: {:?}", end.errors);
        assert!(read_on(&pieces) && pieces.iter().any(|piece| piece.listing.is_some()), "{what}: the run goes on to read");
        assert_eq!(fake.posts()[..2], [vec!["Email/set".to_string()], vec!["Email/get".to_string()]], "{what}: looked once");
        assert_eq!(fake.args_of("Email/set").len(), 1, "{what}");
    }
}

#[test]
fn a_refused_sign_in_a_busy_server_and_a_stop_end_the_run_at_the_look() {
    let answering = |status: u16, retry_after: Option<u32>| {
        Fake::new(move |req, n| match (req.method(), n) {
            (Method::Get, _) => session_reply(json!({})),
            (_, 1) => api_reply(req, |_, _| Answer::Is(json!({"updated": {"E1": null}}))),
            _ => status_reply(status, retry_after),
        })
    };
    let changes = passed_over();
    let answered = [JmapChangeDone { change: changes[0].clone(), outcome: done() }];
    // 401: the owner signs in again; what the set answered is handed on, and nothing is read.
    let fake = answering(401, None);
    let pieces = change_fake(&fake, changes.clone());
    let end = last(&pieces);
    assert!(end.auth_refused && !end.whole && end.errors.is_empty(), "{end:?}");
    assert_eq!(changes_of(&pieces), answered);
    assert_eq!(fake.posts(), [["Email/set"], ["Email/get"]]);
    assert!(!read_on(&pieces));
    // 429 and 503: the server is left alone as long as it asks.
    for (status, retry_after, seconds) in [(429, Some(120), 120), (503, None, 30)] {
        let fake = answering(status, retry_after);
        let pieces = change_fake(&fake, changes.clone());
        let end = last(&pieces);
        assert_eq!((end.busy_for, end.whole, end.auth_refused), (Some(seconds), false, false), "{status}");
        assert_eq!(changes_of(&pieces), answered, "{status}");
        assert_eq!(fake.posts().len(), 2, "{status}");
    }
    // Stopped by the owner while the set was answered: the look is not made.
    let cancel = Rc::new(AtomicBool::new(false));
    let stops = cancel.clone();
    let fake = Fake::new(move |req, _| {
        if req.method() == Method::Get {
            return session_reply(json!({}));
        }
        stops.store(true, std::sync::atomic::Ordering::Relaxed);
        api_reply(req, |_, _| Answer::Is(json!({"updated": {"E1": null}})))
    });
    let pieces = run_over(fake.clone(), FAKE_TOKEN, &JmapMailRequest { changes: changes.clone(), ..asks(FAKE_SESSION) }, &cancel);
    assert_eq!(changes_of(&pieces), answered);
    assert!(last(&pieces).stopped && !read_on(&pieces));
    assert_eq!(fake.posts(), [["Email/set"]]);
}

#[test]
fn a_hostile_answer_to_the_look_makes_nothing_gone_that_it_does_not_name() {
    let reported = |answer: Value| {
        let fake = passing_over(move || Answer::Is(answer.clone()));
        let pieces = change_fake(&fake, passed_over());
        assert!(!last(&pieces).partial && read_on(&pieces));
        changes_of(&pieces).into_iter().skip(1).map(|done| (done.change.id().to_string(), done.outcome)).collect::<Vec<_>>()
    };
    // Wrong types, and ids that were never asked about: nothing is gone.
    for answer in [
        json!(null),
        json!([]),
        json!("notFound"),
        json!({}),
        json!({"notFound": null}),
        json!({"notFound": "E2"}),
        json!({"notFound": {"E2": true}}),
        json!({"notFound": [7, null, ["E2"], {"id": "E3"}, true, "X1", "E1", "", "e2"]}),
        json!({"list": [{"id": "E2"}], "notfound": ["E2"]}),
    ] {
        assert_eq!(reported(answer.clone()), [], "{answer}");
    }
    // Thousands of ids: the first of them are looked at, the asked ones among them found, and no more.
    let crowd = |at: usize| {
        let mut ids: Vec<String> = (0..20_000).map(|n| format!("X{n}")).collect();
        ids.insert(at, "E4".into());
        json!({"notFound": ids})
    };
    assert_eq!(reported(crowd(5)), [("E4".to_string(), JmapOutcome::Gone)]);
    assert_eq!(reported(crowd(15_000)), []);
    // An id named many times is gone once.
    assert_eq!(reported(json!({"notFound": ["E2", "E2", "E2"]})), [("E2".to_string(), JmapOutcome::Gone)]);
}

#[test]
fn a_stand_in_that_passes_over_what_it_does_not_know_has_its_emails_looked_up() {
    let (stand_in, ids) = world();
    let kept = deliver(&stand_in, &ids.inbox, "kept", &[]);
    let stays = deliver(&stand_in, &ids.inbox, "stays", &[]);
    stand_in.with(|d| d.passes_over_unknown = true);
    let changes = vec![marks("E999", Some(true), None, None, None), destroyed("E998"), destroyed(&kept), marks(&stays, Some(true), None, None, None)];
    let pieces = change(&stand_in, changes);
    let outcomes: Vec<JmapOutcome> = changes_of(&pieces).into_iter().map(|done| done.outcome).collect();
    // (The answered ones first, then what the look found gone.)
    assert_eq!(outcomes, [done(), done(), JmapOutcome::Gone, JmapOutcome::Gone]);
    let calls: Vec<Vec<String>> = stand_in.seen().iter().filter(|seen| !seen.calls.is_empty()).map(|seen| seen.calls.clone()).collect();
    assert_eq!(calls[..2], [vec!["Email/set".to_string()], vec!["Email/get".to_string()]]);
    assert!(last(&pieces).whole && last(&pieces).errors.is_empty());
    assert!(stand_in.with(|d| d.email(&kept).is_none() && d.email(&stays).is_some()));
}

#[test]
fn the_look_asks_no_more_ids_at_a_time_than_the_server_answers_in_a_get() {
    // A stand-in that takes 500 changes in a `set` and answers three ids in a `get` (it has three
    // mailboxes to list): seven emails it passes over are three looks, none refused as too large, and every
    // one of them is gone.
    let (stand_in, _) = world();
    stand_in.with(|d| (d.passes_over_unknown, d.max_get) = (true, 3));
    let changes: Vec<JmapChange> = (0..7).map(|n| destroyed(&format!("E99{n}"))).collect();
    let pieces = change(&stand_in, changes.clone());
    let told = changes_of(&pieces);
    assert_eq!(told.iter().map(|done| &done.change).collect::<Vec<_>>(), changes.iter().collect::<Vec<_>>());
    assert!(told.iter().all(|done| done.outcome == JmapOutcome::Gone));
    let calls: Vec<Vec<String>> = stand_in.seen().iter().filter(|seen| !seen.calls.is_empty()).map(|seen| seen.calls.clone()).collect();
    assert_eq!(
        calls[..4],
        [vec!["Email/set".to_string()], vec!["Email/get".to_string()], vec!["Email/get".to_string()], vec!["Email/get".to_string()]]
    );
    assert!(last(&pieces).whole && last(&pieces).errors.is_empty(), "{:?}", last(&pieces));

    // What a session says of its limit is held to sense: none named, or an absurd one, is one look for a
    // run's changes; nought is one id at a time, not none.
    let looks = |core: Value| {
        let fake = Fake::new(move |req, _| {
            if req.method() == Method::Get {
                return session_reply(core.clone());
            }
            api_reply(req, |name, args| match name {
                "Email/set" => Answer::Is(json!({"accountId": "A1", "updated": {"E1": null}})),
                _ if is_look(name, args) => Answer::Is(json!({"list": [], "notFound": args["ids"].clone()})),
                _ => reading(name, args),
            })
        });
        let pieces = change_fake(&fake, passed_over());
        assert_eq!(changes_of(&pieces).len(), 5, "each is answered, or gone");
        let sizes: Vec<usize> =
            fake.args_of("Email/get").iter().filter(|args| is_look("Email/get", args)).map(|args| args["ids"].as_array().unwrap().len()).collect();
        sizes
    };
    assert_eq!(looks(json!({"maxCallsInRequest": 16})), [4]);
    assert_eq!(looks(json!({"maxCallsInRequest": 16, "maxObjectsInGet": 1e15})), [4]);
    assert_eq!(looks(json!({"maxCallsInRequest": 16, "maxObjectsInGet": 3, "maxObjectsInSet": 500})), [3, 1]);
    assert_eq!(looks(json!({"maxCallsInRequest": 16, "maxObjectsInGet": 0})), [1, 1, 1, 1]);
}

#[test]
fn a_look_that_fails_ends_the_looking_and_keeps_what_the_looks_before_it_found() {
    // Two ids to a look; the second look fails: what the first found gone is gone, the rest is asked again.
    let count = std::cell::Cell::new(0usize);
    let fake = Fake::new(move |req, _| {
        if req.method() == Method::Get {
            return session_reply(json!({"maxCallsInRequest": 16, "maxObjectsInGet": 2}));
        }
        api_reply(req, |name, args| match name {
            "Email/set" => Answer::Is(json!({"accountId": "A1", "updated": {"E1": null}})),
            _ if is_look(name, args) => {
                count.set(count.get() + 1);
                if count.get() == 1 {
                    Answer::Is(json!({"list": [], "notFound": args["ids"].clone()}))
                } else {
                    Answer::Error("serverFail", "not now".to_string())
                }
            }
            _ => reading(name, args),
        })
    });
    let changes = passed_over();
    let pieces = change_fake(&fake, changes.clone());
    // (E1 answered by the set; E2 and E3 gone by the first look; E4 and E5 asked again with the next run.)
    let told: Vec<JmapChange> = changes_of(&pieces).into_iter().map(|done| done.change).collect();
    assert_eq!(told, changes[..3]);
    assert_eq!(fake.args_of("Email/get").iter().filter(|args| is_look("Email/get", args)).count(), 2);
    assert!(read_on(&pieces));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    // Whatever a server answers an `Email/set` with, the answer is read without a panic, and a change is
    // reported only as one of the three things a server can say of it.
    #[test]
    fn any_answer_to_a_set_is_read_without_panicking(answer in any_json(), under in proptest::sample::select(vec!["updated", "notUpdated", "destroyed", "notDestroyed"]), entry in any_json()) {
        let secrets = Redactor::new();
        let mut shaped_answer = json!({});
        shaped_answer[under] = json!({"E1": entry.clone()});
        for answer in [answer, shaped_answer, json!({under: ["E1", entry]})] {
            for change in [marks("E1", Some(true), None, None, None), moved("E1", "M1", "M2"), destroyed("E1")] {
                if let Some(JmapOutcome::Done { id }) = outcome_of(&answer, &change, "E1", &secrets) {
                    prop_assert!(id.is_empty() || Id::new(&id).is_ok());
                }
            }
            // And as the answer of a whole run.
            let said = answer.clone();
            let fake = server(move |name, _| (name == "Email/set").then(|| Answer::Is(said.clone())));
            let pieces = change_fake(&fake, vec![marks("E1", Some(true), None, None, None), destroyed("E2")]);
            prop_assert!(changes_of(&pieces).len() <= 2);
            prop_assert!(!last(&pieces).partial);
        }
    }

    // Whatever a server answers the look for the emails a set passed over with, it is read without a panic,
    // and only changes that were asked are reported, each once.
    #[test]
    fn any_answer_to_the_look_is_read_without_panicking(answer in any_json(), listed in proptest::collection::vec("(E[1-6]|X[0-9]|.{0,4})", 0..12)) {
        for answer in [answer, json!({"notFound": listed.clone()}), json!({"notFound": [listed.clone()], "list": listed})] {
            let fake = passing_over(move || Answer::Is(answer.clone()));
            let pieces = change_fake(&fake, passed_over());
            let told = changes_of(&pieces);
            prop_assert!(told.len() <= 5 && !last(&pieces).partial);
            let mut ids: Vec<&str> = told.iter().map(|done| done.change.id()).collect();
            prop_assert!(ids.iter().all(|id| ["E1", "E2", "E3", "E4", "E5"].contains(id)));
            ids.sort_unstable();
            ids.dedup();
            prop_assert_eq!(ids.len(), told.len());
        }
    }
}
