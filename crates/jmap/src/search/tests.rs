// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox searched on its server (#1725): the filter a search is asked with, the search over the
//! stand-in with made-up mail, and what a hostile or failing server makes of it.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use mailcraft_model::FolderKind;
use mailcraft_model::sync::SearchPlace;
use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::connection::tests::any_json;
use crate::standin::{SESSION, StandIn, TOKEN};
use crate::{CAPABILITY_CORE, Credentials, HttpRequest, HttpResponse, block_on};

const ME: &str = "ada@example.org";

struct Ids {
    inbox: String,
    trash: String,
    projects: String,
}

fn world() -> (StandIn, Ids) {
    let stand_in = StandIn::new();
    let ids = stand_in.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        let trash = d.mailbox("Trash", None, Some("trash"));
        let projects = d.mailbox("Projects", None, None);
        Ids { inbox, trash, projects }
    });
    (stand_in, ids)
}

fn mail(subject: &str) -> Vec<u8> {
    format!(
        "From: Zoe Park <zoe@example.org>\r\nTo: Ada <{ME}>\r\nSubject: {subject}\r\nMessage-ID: <{}@example.org>\r\nDate: Thu, 15 Oct 2026 09:30:00 +0200\r\n\r\nHello about {subject}.\r\n",
        subject.replace(' ', "-")
    )
    .into_bytes()
}

fn at(n: usize) -> String {
    format!("2026-01-01T{:02}:{:02}:{:02}Z", n / 3600, (n % 3600) / 60, n % 60)
}

fn deliver(stand_in: &StandIn, mailbox: &str, subject: &str, n: usize) -> String {
    stand_in.with(|d| d.deliver(mailbox, &mail(subject), &["$seen"], &at(n)))
}

/// A search of the whole account, with Trash left out and the three mailboxes known here.
fn request(ids: &Ids, query: &str, next: &str) -> JmapSearchRequest {
    let place = |id: &str, kind| SearchPlace { id: id.to_owned(), kind };
    JmapSearchRequest {
        account: 3,
        email: ME.into(),
        session: SESSION.into(),
        bearer: true,
        ticket: 9,
        outside: vec![ids.trash.clone()],
        places: vec![place(&ids.projects, FolderKind::Custom), place(&ids.inbox, FolderKind::Inbox), place(&ids.trash, FolderKind::Deleted)],
        query: query.into(),
        next: next.into(),
        ..Default::default()
    }
}

fn search_over<H: HttpExchange>(http: H, session: &str, request: &JmapSearchRequest) -> GraphSearchResult {
    let connection = block_on(Connection::connect(http, session, Credentials::bearer(TOKEN).unwrap())).unwrap();
    block_on(search_messages(&connection, request, &AtomicBool::new(false)))
}

fn search(stand_in: &StandIn, request: &JmapSearchRequest) -> GraphSearchResult {
    search_over(stand_in.clone(), SESSION, request)
}

fn subjects(result: &GraphSearchResult) -> Vec<&str> {
    result.messages.iter().map(|m| m.subject.as_str()).collect()
}

fn filter_json(request: &JmapSearchRequest) -> Option<Value> {
    filter_of(request).map(|filter| serde_json::to_value(filter).unwrap())
}

#[test]
fn the_filter_says_where_to_look_and_each_term_as_a_condition() {
    let (_, ids) = world();
    assert_eq!(
        filter_json(&request(&ids, "budget", "")).unwrap(),
        json!({"operator": "AND", "conditions": [{"inMailboxOtherThan": [ids.trash]}, {"text": "budget"}]})
    );
    let one = JmapSearchRequest { folder: ids.inbox.clone(), ..request(&ids, "from:zoe subject:\"spring plan\" has:attachment importance:high", "") };
    // (The importance is not sent: it is checked on what comes back.)
    assert_eq!(
        filter_json(&one).unwrap(),
        json!({"operator": "AND", "conditions": [
            {"inMailbox": ids.inbox},
            {"from": "zoe"},
            {"subject": "\"spring plan\""},
            {"hasAttachment": true},
        ]})
    );
    // Nothing to leave out, one term: a condition by itself.
    let bare = JmapSearchRequest { outside: Vec::new(), ..request(&ids, "body:budget", "") };
    assert_eq!(filter_json(&bare).unwrap(), json!({"body": "budget"}));
    // What the standard has no condition for, and what a server isn't asked at all.
    assert_eq!(filter_json(&request(&ids, "attachment:report", "")), None);
    assert_eq!(filter_json(&request(&ids, "is:unread", "")), None);
    // An importance and nothing else would read the whole mailbox: not asked.
    assert_eq!(filter_json(&request(&ids, "importance:high", "")), None);
    // A mailbox that is no id is no place to search.
    assert_eq!(filter_json(&JmapSearchRequest { folder: "no id".into(), ..request(&ids, "budget", "") }), None);
}

#[test]
fn a_word_never_carries_a_quote_or_a_backslash_into_a_phrase() {
    let words = |words: &[&str]| text_of(&words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>());
    assert_eq!(words(&["bud\"get\\"]).as_deref(), Some("budget"));
    assert_eq!(words(&["spring\"", "pl\\an\u{1}"]).as_deref(), Some("\"spring plan\""));
    assert_eq!(words(&["\"", "\\"]), None);
    assert!(words(&[&"x".repeat(5_000)]).unwrap().chars().count() <= MAX_TEXT);
}

#[test]
fn a_search_of_the_account_finds_mail_in_every_mailbox_but_the_ones_left_out() {
    let (stand_in, ids) = world();
    deliver(&stand_in, &ids.inbox, "Budget for the spring", 1);
    deliver(&stand_in, &ids.inbox, "Lunch on Friday", 2);
    deliver(&stand_in, &ids.projects, "The budget of the old project", 3);
    deliver(&stand_in, &ids.trash, "A budget thrown away", 4);
    let found = search(&stand_in, &request(&ids, "budget", ""));
    assert!(found.errors.is_empty() && found.next.is_empty() && !found.offline, "{found:?}");
    assert_eq!((found.account, found.email.as_str(), found.ticket), (3, ME, 9));
    assert_eq!(subjects(&found), ["The budget of the old project", "Budget for the spring"]);
    let folders: Vec<&str> = found.messages.iter().map(|m| m.server.folder.as_str()).collect();
    assert_eq!(folders, [ids.projects.as_str(), ids.inbox.as_str()]);
    // What a list shows of each, as a run brings it.
    let first = &found.messages[1];
    assert_eq!(
        (first.from.email.as_str(), first.preview.as_str(), first.server.read),
        ("zoe@example.org", "Hello about Budget for the spring.", true)
    );
    assert!(first.received > 0 && first.server.size > 0 && !first.message_id.is_empty());
    // Two calls, nothing that changes anything.
    assert_eq!(stand_in.calls(), ["Email/query", "Email/get"]);
}

#[test]
fn a_search_of_one_mailbox_asks_that_mailbox_and_a_field_is_that_field() {
    let (stand_in, ids) = world();
    deliver(&stand_in, &ids.inbox, "Budget for the spring", 1);
    deliver(&stand_in, &ids.projects, "The budget of the old project", 2);
    let one = |query: &str| JmapSearchRequest { folder: ids.projects.clone(), ..request(&ids, query, "") };
    assert_eq!(subjects(&search(&stand_in, &one("budget"))), ["The budget of the old project"]);
    assert_eq!(subjects(&search(&stand_in, &one("subject:\"old project\""))), ["The budget of the old project"]);
    assert!(search(&stand_in, &one("to:zoe")).messages.is_empty());
    assert_eq!(search(&stand_in, &one("from:zoe body:hello")).messages.len(), 1);
    assert!(search(&stand_in, &one("has:attachment")).messages.is_empty());
    // Nothing found is one call, not two.
    assert_eq!(stand_in.calls().iter().filter(|call| *call == "Email/get").count(), 3);
}

#[test]
fn a_search_goes_on_from_where_the_last_page_ended() {
    let (stand_in, ids) = world();
    for n in 0..120 {
        deliver(&stand_in, &ids.inbox, &format!("Budget line {n}"), n);
    }
    let first = search(&stand_in, &request(&ids, "budget", ""));
    assert_eq!((first.messages.len(), first.next.as_str()), (PAGE, "50"));
    assert_eq!(first.messages[0].subject, "Budget line 119");
    let second = search(&stand_in, &request(&ids, "budget", &first.next));
    assert_eq!((second.messages.len(), second.next.as_str()), (PAGE, "100"));
    let third = search(&stand_in, &request(&ids, "budget", &second.next));
    assert_eq!((third.messages.len(), third.next.as_str()), (20, ""));
    let mut all: Vec<&str> = [&first, &second, &third].iter().flat_map(|r| r.messages.iter().map(|m| m.server.id.as_str())).collect();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 120);
    // What is no position begins at the start; one past the end finds nothing and ends.
    assert_eq!(search(&stand_in, &request(&ids, "budget", "junk")).messages[0].subject, "Budget line 119");
    let past = search(&stand_in, &request(&ids, "budget", "5000"));
    assert!(past.messages.is_empty() && past.next.is_empty() && past.errors.is_empty());
}

#[test]
fn an_email_in_several_mailboxes_is_found_once_in_the_one_a_run_lists_it_in() {
    let (stand_in, ids) = world();
    let both = deliver(&stand_in, &ids.projects, "Budget in two places", 1);
    assert!(stand_in.with(|d| d.add_to_mailbox(&both, &ids.inbox)));
    let found = search(&stand_in, &JmapSearchRequest { folder: ids.projects.clone(), ..request(&ids, "budget", "") });
    assert_eq!(found.messages.len(), 1);
    assert_eq!(found.messages[0].server.folder, ids.inbox, "the Inbox lists it, as in a run");
    // A mailbox this computer doesn't have is no place for it: with none left, it is passed over.
    let mut unknown = request(&ids, "budget", "");
    unknown.places.clear();
    assert!(search(&stand_in, &unknown).messages.is_empty());
}

#[test]
fn what_a_server_cannot_be_asked_is_said_and_nothing_is_sent() {
    let (stand_in, ids) = world();
    for query in ["attachment:report", "is:unread", "-budget"] {
        let nothing = search(&stand_in, &request(&ids, query, ""));
        assert_eq!(nothing.errors, ["Nothing of this search can be asked of the server."]);
    }
    assert_eq!(stand_in.api_requests(), 0);
    // Called off before it began: nothing is asked and nothing said.
    let connection = block_on(Connection::connect(stand_in.clone(), SESSION, Credentials::bearer(TOKEN).unwrap())).unwrap();
    let called_off = block_on(search_messages(&connection, &request(&ids, "budget", ""), &AtomicBool::new(true)));
    assert!(called_off.errors.is_empty() && called_off.messages.is_empty());
    assert_eq!(stand_in.api_requests(), 0);
}

#[test]
fn what_stops_a_search_is_said_without_the_search_or_the_owner_and_with_where_to_go_on() {
    let (stand_in, ids) = world();
    deliver(&stand_in, &ids.inbox, "Budget for the spring", 1);
    // A busy server.
    stand_in.with(|d| d.refuse_next.push_back(Some((429, Some(7)))));
    let busy = search(&stand_in, &request(&ids, "budget from:zoe", "50"));
    assert_eq!((busy.busy_for, busy.next.as_str()), (Some(7), "50"));
    assert_eq!(busy.errors.len(), 1);
    assert!(busy.errors.iter().all(|line| !line.contains("budget") && !line.contains("zoe") && !line.contains('@')), "{busy:?}");
    // A mailbox the server can't read just now: the method's error, said as a line.
    stand_in.with(|d| d.broken_mailboxes.insert(ids.inbox.clone(), "serverFail"));
    let broken = search(&stand_in, &JmapSearchRequest { folder: ids.inbox.clone(), ..request(&ids, "budget", "") });
    assert!(broken.messages.is_empty() && broken.errors.len() == 1 && !broken.offline, "{broken:?}");
    // A sign-in that failed before the search began.
    let failed = failed_search(&request(&ids, "budget", ""), &Error::CredentialsRefused);
    assert!(failed.messages.is_empty() && failed.errors.len() == 1 && failed.next.is_empty());
    assert_eq!((failed.account, failed.ticket), (3, 9));
    let unreachable = failed_search(&request(&ids, "budget", "100"), &Error::Transport(TransportError::Unreachable));
    assert!(unreachable.offline && unreachable.next == "100");
}

// ------------------------------------------------------------------------------------------
// A scripted server, for answers no honest server gives

type Reply = std::result::Result<HttpResponse, TransportError>;

#[derive(Clone, Default)]
struct Script {
    replies: Rc<RefCell<VecDeque<Reply>>>,
}

impl HttpExchange for Script {
    async fn exchange(&self, _: &HttpRequest) -> Reply {
        self.replies.borrow_mut().pop_front().unwrap_or(Err(TransportError::Unreachable))
    }
}

fn reply(body: &Value) -> Reply {
    Ok(HttpResponse::new(200, serde_json::to_vec(body).unwrap()).with_field("Content-Type", "application/json"))
}

fn answer(method: &str, answer: Value) -> Reply {
    reply(&json!({"methodResponses": [[method, answer, "c0"]], "sessionState": "1"}))
}

fn scripted(replies: Vec<Reply>) -> Script {
    let session = json!({
        "capabilities": {CAPABILITY_CORE: {"maxCallsInRequest": 16, "maxObjectsInGet": 500}, CAPABILITY_MAIL: {}},
        "accounts": {"A1": {"name": ME, "isPersonal": true, "accountCapabilities": {CAPABILITY_MAIL: {}}}},
        "primaryAccounts": {CAPABILITY_MAIL: "A1"},
        "apiUrl": "/jmap/api",
        "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
        "uploadUrl": "/jmap/upload/{accountId}/",
        "state": "1",
    });
    let script = Script::default();
    script.replies.borrow_mut().push_back(reply(&session));
    script.replies.borrow_mut().extend(replies);
    script
}

fn asked() -> JmapSearchRequest {
    let place = |id: &str, kind| SearchPlace { id: id.to_owned(), kind };
    JmapSearchRequest {
        account: 3,
        email: ME.into(),
        session: "https://jmap.example.org/jmap/session".into(),
        bearer: true,
        ticket: 9,
        places: vec![place("M1", FolderKind::Inbox), place("M2", FolderKind::Custom)],
        query: "budget".into(),
        ..Default::default()
    }
}

fn search_scripted(replies: Vec<Reply>, request: &JmapSearchRequest) -> GraphSearchResult {
    search_over(scripted(replies), "https://jmap.example.org/jmap/session", request)
}

#[test]
fn a_server_that_lists_more_than_was_asked_or_lies_about_its_place_is_held_to_the_page() {
    let many: Vec<String> = (0..500).map(|n| format!("E{n}")).collect();
    let emails: Vec<Value> =
        (0..500).map(|n| json!({"id": format!("E{n}"), "mailboxIds": {"M1": true}, "receivedAt": "2026-01-01T00:00:00Z"})).collect();
    let found =
        search_scripted(vec![answer("Email/query", json!({"ids": many, "total": 10_000})), answer("Email/get", json!({"list": emails}))], &asked());
    assert_eq!(found.messages.len(), PAGE);
    assert_eq!(found.next, "50");
    // A total that says there is more after a page that listed nothing: the search ends.
    let empty = search_scripted(vec![answer("Email/query", json!({"ids": [], "total": 10_000}))], &asked());
    assert!(empty.next.is_empty() && empty.messages.is_empty() && empty.errors.is_empty());
    // Ids that are none, an id twice, an email that wasn't asked for, one in no known mailbox.
    let ids = json!(["E1", "E 2", 7, "E1", "E3", "E4"]);
    let list = json!([
        {"id": "E1", "mailboxIds": {"M2": true, "M1": true}},
        {"id": "E9", "mailboxIds": {"M1": true}},
        {"id": "E3", "mailboxIds": {"M7": true}},
        {"id": "E4", "mailboxIds": "nowhere"},
        "junk",
    ]);
    let odd = search_scripted(vec![answer("Email/query", json!({"ids": ids})), answer("Email/get", json!({"list": list}))], &asked());
    let named: Vec<(&str, &str)> = odd.messages.iter().map(|m| (m.server.id.as_str(), m.server.folder.as_str())).collect();
    assert_eq!(named, [("E1", "M1")]);
    assert!(odd.next.is_empty() && odd.errors.is_empty());
}

#[test]
fn a_server_that_goes_away_between_the_two_calls_leaves_the_page_to_ask_again() {
    let request = JmapSearchRequest { next: "50".into(), ..asked() };
    let gone = search_scripted(vec![answer("Email/query", json!({"ids": ["E1"], "total": 51}))], &request);
    assert!(gone.offline && gone.messages.is_empty() && gone.next == "50" && gone.errors.len() == 1, "{gone:?}");
}

proptest! {
    #[test]
    fn any_answer_to_a_search_is_read_or_refused_and_never_panics(
        found in prop_oneof![any_json(), Just(json!({"ids": ["E1", "E2"], "total": 2}))],
        got in prop_oneof![any_json(), Just(json!({"list": [{"id": "E1", "mailboxIds": {"M1": true}}]}))],
        noise in proptest::collection::vec(any::<u8>(), 0..64),
        next in "[ -~]{0,12}",
    ) {
        let request = JmapSearchRequest { next, ..asked() };
        let garbled: Reply = Ok(HttpResponse::new(200, noise));
        for replies in [vec![answer("Email/query", found.clone()), answer("Email/get", got.clone())], vec![garbled.clone()], vec![answer("Email/query", found.clone()), garbled.clone()]] {
            let result = search_scripted(replies, &request);
            // Only what was asked for, a page at the most, each in a mailbox this computer has.
            prop_assert!(result.messages.len() <= PAGE);
            prop_assert!(result.messages.iter().all(|m| Id::new(&m.server.id).is_ok() && ["M1", "M2"].contains(&m.server.folder.as_str())));
            prop_assert!(result.next.is_empty() || result.next.parse::<u64>().is_ok());
        }
    }
}

#[test]
fn an_importance_is_not_sent_and_is_checked_on_what_the_server_found() {
    let (stand_in, ids) = world();
    let urgent = b"From: Zoe Park <zoe@example.org>\r\nSubject: Budget sign-off\r\nimportance: High\r\nMessage-ID: <u@example.org>\r\n\r\nToday.\r\n";
    stand_in.with(|d| d.deliver(&ids.inbox, urgent, &[], &at(1)));
    deliver(&stand_in, &ids.inbox, "Budget for the spring", 2);
    assert_eq!(subjects(&search(&stand_in, &request(&ids, "budget importance:high", ""))), ["Budget sign-off"]);
    assert!(search(&stand_in, &request(&ids, "budget importance:low", "")).messages.is_empty());
    assert_eq!(search(&stand_in, &request(&ids, "budget", "")).messages.len(), 2);
}
