// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! More results on the server (R6.2) against the strict Graph stand-in, and against answers
//! that are anything at all. Nothing here leaves this machine; every message is invented.

use std::sync::Arc;
use std::time::Duration;

use mailcraft_model::sync::{GraphSearchRequest, GraphSearchResult, SharedRefusal};
use proptest::prelude::*;
use serde_json::json;

use super::search::{search_messages_with, search_value};
use crate::fake::{FakeHttp, json_answer};
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 2, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;
const SHARED: &str = "support@example.org";

fn request(email: &str, shared: bool, folder: &str, query: &str) -> GraphSearchRequest {
    GraphSearchRequest { account: 3, email: email.into(), shared, ticket: 7, folder: folder.into(), query: query.into(), next: String::new() }
}

fn ask(base: &str, request: &GraphSearchRequest) -> GraphSearchResult {
    search_messages_with(base, TOKEN, request, Arc::new(Gate::new()), QUICK)
}

fn subjects(result: &GraphSearchResult) -> Vec<&str> {
    result.messages.iter().map(|m| m.subject.as_str()).collect()
}

#[test]
fn what_goes_to_the_server_is_what_its_search_understands() {
    assert_eq!(search_value("budget").as_deref(), Some(r#""budget""#));
    assert_eq!(
        search_value(r#"budget from:ada@example.org subject:"q4 plan" has:attachment is:important"#).as_deref(),
        Some(r#""budget AND from:ada@example.org AND subject:\"q4 plan\" AND hasAttachments:true AND importance:high""#)
    );
    // What it has no words for is checked here: nothing of it goes.
    assert_eq!(search_value("budget -lunch is:unread category:red after:2026-01-01 importance:normal").as_deref(), Some(r#""budget""#));
    assert_eq!(search_value("is:unread"), None);
    assert_eq!(search_value(""), None);
    // The syntax's own words and marks are only words in it.
    assert_eq!(search_value(r#"AND OR (x) a\b c*d "e"f"#).as_deref(), Some(r#""and AND or AND x AND \"a b\" AND \"c d\" AND ef""#));
    // Case and accents as the owner wrote them: the server folds them as it does.
    assert_eq!(search_value("Gvožđar").as_deref(), Some(r#""Gvožđar""#));
    // #301: a whole address goes whole, one with a `+` in quotes, where the syntax reads `+` as
    // text; one it can't take as it is stays to be checked here, and nothing else is lost.
    assert_eq!(search_value("from:Ada@Example.org").as_deref(), Some(r#""from:ada@example.org""#));
    assert_eq!(search_value("from:ada+news@example.org budget").as_deref(), Some(r#""from:\"ada+news@example.org\" AND budget""#));
    assert_eq!(search_value("Ada+News@example.org").as_deref(), Some(r#""\"Ada+News@example.org\"""#));
    assert_eq!(search_value("c+plus").as_deref(), Some(r#""\"c plus\"""#));
    assert_eq!(search_value("from:ada!news@example.org budget").as_deref(), Some(r#""budget""#));
}

/// #301: an address reaches the server in one piece (a plus-address too), the server finds that
/// address alone, and what it finds is what a search here finds; what it finds is checked here as
/// well.
#[test]
fn an_address_is_searched_on_the_server_as_it_is_here() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let senders =
        ["ada+news@example.org", "ada@example.org", "adam+newsletter@example.org", "ADA+NEWS@example.org", "ada.smith@example.org", "ΟΣ@example.org"];
    {
        let mut state = server.state();
        for (i, from) in senders.into_iter().enumerate() {
            let id = state.mailbox.add(&inbox, from, NOW - 60 * i as i64);
            state.mailbox.messages.get_mut(&id).unwrap().from = from.into();
        }
    }
    // Each message's subject is its sender. `alone`: the server's answer is the search's by itself.
    for (query, sent, alone) in [
        ("from:ada+news@example.org", r#""from:\"ada+news@example.org\"""#, true),
        ("from:ADA@example.org", r#""from:ada@example.org""#, true),
        ("from:ada", r#""from:ada""#, true),
        // A final sigma is a sigma, here and there.
        ("from:οσ@example.org", r#""from:οσ@example.org""#, true),
        // Not a whole address here, so not one there either: words, in both.
        ("from:ada@-example.org", r#""from:ada@-example.org""#, true),
        // Words here round a whole address, which the server would read as that address: not
        // sent, so the server finds more, and what is checked here leaves the search's answer.
        ("from:<ada@example.org> subject:ada", r#""subject:ada""#, false),
    ] {
        let found = ask(&server.url(), &request("owner@example.org", false, "", query));
        assert!(found.errors.is_empty(), "{query}: {:?}", found.errors);
        let asked = server.state().requests.last().cloned().unwrap();
        let param = asked.query.iter().find(|(n, _)| n == "$search").map(|(_, v)| v.clone());
        assert_eq!(param.as_deref(), Some(sent), "{query}");
        // What the server finds, once what is checked here has looked at it, is what a search
        // here finds; and with nothing left to check, by itself.
        let parsed = mailcraft_model::search::Query::parse(query);
        let local = parsed.for_server().local;
        let message =
            |from: &str| mailcraft_model::Message { from: mailcraft_model::Address::new("", from), subject: from.into(), ..Default::default() };
        let mut by_here: Vec<&str> = senders.into_iter().filter(|from| parsed.matches(&message(from), 0)).collect();
        by_here.sort();
        let mut by_server: Vec<&str> = subjects(&found);
        by_server.sort();
        let checked: Vec<&str> = by_server.iter().copied().filter(|from| local.matches(&message(from), 0)).collect();
        assert_eq!(checked, by_here, "{query}");
        assert_eq!(by_server == by_here, alone, "{query}: {by_server:?}");
    }
}

/// #194: a space after a keyword's colon sends the keyword with its word; a keyword with nothing
/// after it yet sends nothing of itself.
#[test]
fn a_keyword_with_a_space_after_its_colon_goes_as_one_clause() {
    let both = Some(r#""from:stuart AND subject:iqa""#);
    assert_eq!(search_value("from: stuart subject: iqa").as_deref(), both);
    assert_eq!(search_value("from:stuart subject:iqa").as_deref(), both);
    assert_eq!(search_value("from: stuart subject:").as_deref(), Some(r#""from:stuart""#));
    assert_eq!(search_value("from: stuart su").as_deref(), Some(r#""from:stuart AND su""#));
    assert_eq!(search_value("subject: \"IQA audit\"").as_deref(), Some(r#""subject:\"IQA audit\"""#));
    assert_eq!(search_value("subject:"), None);
}

#[test]
fn the_whole_mailbox_is_searched_fifty_a_page_newest_first_and_the_next_page_follows_the_servers_link() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let archive = server.state().well_known("archive");
    {
        let mut state = server.state();
        for i in 0..70 {
            state.mailbox.add(&inbox, &format!("Budget {i}"), NOW - 60 * i);
        }
        state.mailbox.add(&archive, "Budget archived", NOW + 60);
        state.mailbox.add(&inbox, "Lunch", NOW + 120);
    }
    let first = ask(&server.url(), &request("owner@example.org", false, "", "budget"));
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!((first.account, first.ticket, first.email.as_str()), (3, 7, "owner@example.org"));
    assert_eq!(first.messages.len(), 50);
    assert_eq!(first.messages[0].subject, "Budget archived");
    assert_eq!(first.messages[0].server.folder, archive, "each in its folder");
    assert!(first.messages.windows(2).all(|w| w[0].received >= w[1].received));
    assert!(!first.next.is_empty());
    let asked = server.state().requests.last().cloned().unwrap();
    assert_eq!(asked.path.join("/"), "v1.0/me/messages");
    let param = |name: &str| asked.query.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    assert_eq!(param("$search").as_deref(), Some("\"budget\""));
    assert_eq!(param("$top").as_deref(), Some("50"));
    assert_eq!(param("$select").as_deref(), Some(super::down::FIELDS));
    assert!(param("$filter").is_none() && param("$orderby").is_none());
    assert!(asked.header("prefer").contains("ImmutableId") && asked.header("consistencylevel").is_empty());

    let second = ask(&server.url(), &GraphSearchRequest { next: first.next.clone(), ..request("owner@example.org", false, "", "budget") });
    assert_eq!(second.messages.len(), 21);
    assert!(second.next.is_empty(), "at its end");
    assert!(subjects(&first).iter().chain(subjects(&second).iter()).all(|s| s.starts_with("Budget")));

    // One folder: that folder alone.
    let folder = ask(&server.url(), &request("owner@example.org", false, &archive, "budget"));
    assert_eq!(subjects(&folder), ["Budget archived"]);
    let asked = server.state().requests.last().cloned().unwrap();
    assert_eq!(asked.path, ["v1.0", "me", "mailFolders", archive.as_str(), "messages"]);
    assert_eq!(server.state().searches, 3);
}

#[test]
fn a_shared_mailbox_is_searched_at_its_own_address_and_nothing_of_it_reaches_me() {
    let server = StandIn::start().unwrap();
    server.state().add_other(SHARED, true);
    let theirs = server.state().other(SHARED).unwrap().folders.iter().find(|f| f.well_known == "inbox").unwrap().id.clone();
    let mine = server.state().well_known("inbox");
    server.state().other(SHARED).unwrap().mailbox.generate(&theirs, 60, NOW);
    server.state().mailbox.add(&mine, "Message 0 of the owner", NOW);
    let first = ask(&server.url(), &request(SHARED, true, "", "message"));
    assert_eq!(first.messages.len(), 50, "{:?}", first.errors);
    assert!(first.messages.iter().all(|m| m.server.folder == theirs));
    let second = ask(&server.url(), &GraphSearchRequest { next: first.next.clone(), ..request(SHARED, true, "", "message") });
    assert_eq!(second.messages.len(), 10);
    let folder = ask(&server.url(), &request(SHARED, true, &theirs, "subject:message"));
    assert_eq!(folder.messages.len(), 50);
    for asked in server.state().requests.iter() {
        let path = asked.path.join("/");
        assert!(path.starts_with(&format!("v1.0/users/{SHARED}/")), "{path}");
    }
    // A next page at the owner's own mailbox is never asked for a shared one.
    let before = server.state().requests.len();
    let astray = format!("{}/v1.0/me/messages?$search=%22message%22&$skip=50", server.url());
    let refused = ask(&server.url(), &GraphSearchRequest { next: astray, ..request(SHARED, true, "", "message") });
    assert!(refused.messages.is_empty() && !refused.errors.is_empty());
    assert_eq!(server.state().requests.len(), before);
    // Nor anywhere but Graph, for any mailbox.
    let elsewhere = ask(
        &server.url(),
        &GraphSearchRequest { next: "https://elsewhere.example/v1.0/me/messages".into(), ..request("owner@example.org", false, "", "x") },
    );
    assert!(!elsewhere.errors.is_empty());
    assert_eq!(server.state().requests.len(), before);

    // Without Full Access: said as a run says it, with no address in it.
    server.state().add_other("closed@example.org", false);
    let closed = ask(&server.url(), &request("closed@example.org", true, "", "message"));
    assert_eq!(closed.refused, Some(SharedRefusal::NoAccess));
    assert!(closed.errors.iter().all(|e| !e.contains("closed@")) && closed.errors.iter().any(|e| e.contains("Full Access")), "{:?}", closed.errors);
    // An address with no mailbox.
    let nobody = ask(&server.url(), &request("nobody@example.org", true, "", "message"));
    assert_eq!(nobody.refused, Some(SharedRefusal::NoMailbox));
}

#[test]
fn refusals_are_said_in_the_words_a_run_has_and_the_query_is_in_none_of_them() {
    let server = StandIn::start().unwrap();
    for (status, code, says) in [
        (403, "ErrorAccessDenied", "may not read its mail"),
        (429, "TooManyRequests", "busy"),
        (503, "ServiceUnavailable", "busy"),
        (400, "ErrorInvalidRequest", "status 400"),
    ] {
        server.state().search_refused = Some((status, code));
        let result = ask(&server.url(), &request("owner@example.org", false, "", "secretword"));
        assert!(result.messages.is_empty());
        assert!(result.errors.iter().any(|e| e.contains(says)), "{status}: {:?}", result.errors);
        assert!(result.errors.iter().all(|e| !e.contains("secretword")));
        assert_eq!(result.busy_for.is_some(), status != 403 && status != 400, "{status}");
        assert!(!result.offline);
    }
    // A sign-in it doesn't take.
    server.state().search_refused = None;
    let refused = search_messages_with(&server.url(), "another-token", &request("owner@example.org", false, "", "x"), Arc::new(Gate::new()), QUICK);
    assert!(refused.errors.iter().any(|e| e.contains("didn't take the sign-in")), "{:?}", refused.errors);
    // Nothing to ask: nothing asked.
    let before = server.state().requests.len();
    let nothing = ask(&server.url(), &request("owner@example.org", false, "", "is:unread"));
    assert!(!nothing.errors.is_empty() && server.state().requests.len() == before);
    // A folder id that isn't one.
    let bad = ask(&server.url(), &request("owner@example.org", false, "a b\"c", "x"));
    assert!(!bad.errors.is_empty() && server.state().requests.len() == before);
    // No network: offline.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let offline = ask(&format!("http://127.0.0.1:{port}"), &request("owner@example.org", false, "", "x"));
    assert!(offline.offline && !offline.errors.is_empty());
}

#[test]
fn a_hostile_answer_is_read_for_what_can_be_read_and_nothing_more() {
    let entry = |id: &str, folder: &str| json!({"id": id, "parentFolderId": folder, "receivedDateTime": "2026-10-12T08:00:00Z", "subject": "Budget", "isRead": "maybe"});
    let body = json!({
        "value": [
            entry("AAkA-1", "AQMk-in"),
            entry("AAkA-1", "AQMk-in"),
            entry("bad id with spaces", "AQMk-in"),
            entry("AAkA-2", ""),
            {"id": "AAkA-3"},
            "not a message",
            null,
            {"id": "AAkA-4", "parentFolderId": "AQMk-in", "receivedDateTime": "2026-10-12T09:00:00Z", "subject": "x".repeat(100_000), "from": {"emailAddress": 7}},
        ],
        "@odata.nextLink": "https://elsewhere.example.org/v1.0/me/messages?$skip=50",
    });
    let server = FakeHttp::answering(json_answer("200 OK", &body.to_string()));
    let result = ask(&server.url(""), &request("owner@example.org", false, "", "budget"));
    let ids: Vec<&str> = result.messages.iter().map(|m| m.server.id.as_str()).collect();
    assert_eq!(ids, ["AAkA-4", "AAkA-1"], "each once, newest first; what can't be read passed over");
    assert!(result.messages.iter().all(|m| m.subject.chars().count() <= 998));
    assert!(result.next.is_empty(), "a next page elsewhere is not followed");
    for junk in ["", "{", "[]", "{\"value\": 5}", "{\"value\": {}}"] {
        let server = FakeHttp::answering(json_answer("200 OK", junk));
        let result = ask(&server.url(""), &request("owner@example.org", false, "", "budget"));
        assert!(result.messages.is_empty() && !result.errors.is_empty(), "{junk}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the owner types, what goes is one value in quotes, with no quote, backslash or
    /// bracket in it but a phrase's own `\"`.
    #[test]
    fn whatever_is_typed_the_value_is_one_the_syntax_reads(query in "\\PC{0,200}") {
        if let Some(value) = search_value(&query) {
            let inner = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap();
            let unescaped = inner.replace("\\\"", "");
            prop_assert!(!unescaped.contains('"') && !unescaped.contains('\\') && !unescaped.contains('(') && !unescaped.contains(')'));
            prop_assert!(value.len() <= 2_000);
        }
    }

    /// Whatever bytes come back, a search never panics.
    #[test]
    fn whatever_comes_back_never_panics(body in proptest::collection::vec(any::<u8>(), 0..400)) {
        let mut answer = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n".to_vec();
        answer.extend(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        answer.extend(&body);
        let server = FakeHttp::answering(answer);
        let _ = ask(&server.url(""), &request("owner@example.org", false, "", "budget"));
    }
}

/// #301: an address that starts with a mark is sent in quotes, so the server does not read it as
/// query syntax; one that starts with a letter or digit goes as it is.
#[test]
fn an_address_starting_with_a_mark_is_sent_quoted() {
    assert_eq!(search_value("from:_ada@example.org").as_deref(), Some(r#""from:\"_ada@example.org\"""#));
    assert_eq!(search_value("from:'ada@example.org").as_deref(), Some(r#""from:\"'ada@example.org\"""#));
    assert_eq!(search_value("from:1ada@example.org").as_deref(), Some(r#""from:1ada@example.org""#));
}
