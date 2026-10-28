// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! More results on the server (R6.2): what the engine asks a host to search, and what it makes
//! of what comes back. (Against the Graph stand-in: `crates/net/src/graph/mail/search_tests.rs`,
//! and the window's part in `crates/ui-egui/src/server_search.rs`.)

use mailcraft_model::sync::{
    FolderListing, GraphFolder, GraphMailResult, GraphMessage, GraphSearchResult, MessagePage, ServerMessage, SharedRefusal,
};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use super::*;
use crate::cmd::graph_mail::apply_graph_mail;

const OWNER: &str = "owner@example.org";
const SHARED: &str = "support@example.org";
const NOW: i64 = 1_790_000_000;

/// A session with a local account, a Microsoft account and a shared mailbox opened through it,
/// each Microsoft mailbox with its Inbox, Deleted Items and Archive listed; their ids.
fn session() -> (Session, Id, Id, Id) {
    let mut s = Session::empty();
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap();
    for (account, email) in [(owner, OWNER), (shared, SHARED)] {
        let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
        let folders = vec![
            folder("in", "Inbox", FolderKind::Inbox),
            folder("de", "Deleted Items", FolderKind::Deleted),
            folder("ar", "Archive", FolderKind::Archive),
        ];
        let listing =
            GraphMailResult { account, email: email.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() };
        apply_graph_mail(&mut s, listing).unwrap();
    }
    (s, local, owner, shared)
}

fn found(id: &str, folder: &str, subject: &str, received: i64) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: folder.into(), ..Default::default() },
        subject: subject.into(),
        received,
        sent: received - 30,
        preview: format!("About {subject}."),
        ..Default::default()
    }
}

fn search(s: &mut Session, query: &str, scope: &str) {
    s.run("view.search", &json!({"query": query, "scope": scope})).unwrap();
}

fn asked(s: &mut Session) -> Vec<mailcraft_model::sync::GraphSearchRequest> {
    serde_json::from_value(s.run("sync.graphSearchRequest", &json!({})).unwrap()["requests"].clone()).unwrap()
}

fn answer(s: &mut Session, request: &mailcraft_model::sync::GraphSearchRequest, messages: Vec<GraphMessage>, next: &str) -> Value {
    let result = GraphSearchResult {
        account: request.account,
        email: request.email.clone(),
        ticket: request.ticket,
        messages,
        next: next.into(),
        ..Default::default()
    };
    s.run("sync.applyGraphSearch", &json!({"result": result})).unwrap()
}

fn subjects(s: &Session) -> Vec<String> {
    s.server_search.results.iter().map(|r| r.found.subject.clone()).collect()
}

#[test]
fn all_mailboxes_asks_each_microsoft_mailbox_once_and_current_folder_that_folder() {
    let (mut s, local, owner, shared) = session();
    // Nothing is asked of anybody until the owner asks.
    search(&mut s, "budget", "everywhere");
    assert!(!s.server_search.asked());
    let requests = asked(&mut s);
    let whose: Vec<(Id, bool, &str)> = requests.iter().map(|r| (r.account, r.shared, r.folder.as_str())).collect();
    assert_eq!(whose, [(owner, false, ""), (shared, true, "")], "not the local account");
    assert!(requests.iter().all(|r| r.query == "budget" && r.next.is_empty()));
    // Asked again while they are out: nothing more.
    assert!(asked(&mut s).is_empty());

    // Current Folder: that folder of that mailbox.
    s.view.folder = s.mailbox.folder_by_server(shared, "ar").unwrap();
    search(&mut s, "budget", "folder");
    let requests = asked(&mut s);
    assert_eq!(requests.iter().map(|r| (r.account, r.folder.as_str())).collect::<Vec<_>>(), [(shared, "ar")]);
    // A folder of a local account, or one the server doesn't have yet: nothing to ask.
    s.view.folder = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    search(&mut s, "budget", "folder");
    assert!(crate::cmd::search_server::offered_reason(&s).is_some());
    assert!(s.run("sync.graphSearchRequest", &json!({})).is_err());
    // The shared mailbox closed: only the owner's own.
    s.run("account.removeSharedMailbox", &json!({"address": SHARED})).unwrap();
    search(&mut s, "budget", "everywhere");
    assert_eq!(asked(&mut s).iter().map(|r| r.account).collect::<Vec<_>>(), [owner]);
}

#[test]
fn results_come_fifty_a_page_newest_first_and_mail_here_shows_once_locally() {
    let (mut s, _, owner, _) = session();
    s.run("account.removeSharedMailbox", &json!({"address": SHARED})).unwrap();
    // One message the server will find is here already.
    let inbox = s.mailbox.folder_by_server(owner, "in").unwrap();
    let page = MessagePage {
        folder: "in".into(),
        messages: vec![found("AAkA-here", "in", "Budget here", NOW)],
        link: "https://graph.example.org/d".into(),
        ..Default::default()
    };
    apply_graph_mail(&mut s, GraphMailResult { account: owner, email: OWNER.into(), pages: vec![page], partial: true, ..Default::default() })
        .unwrap();
    search(&mut s, "budget", "everywhere");
    let first = asked(&mut s).remove(0);
    let mut messages: Vec<GraphMessage> = (0..50).map(|i| found(&format!("AAkA-{i}"), "in", &format!("Budget {i}"), NOW - 60 * (i + 1))).collect();
    messages.push(found("AAkA-here", "in", "Budget here", NOW));
    // The server's order is by when sent: arrival may differ, and newest first is by arrival.
    messages.swap(0, 7);
    let said = answer(&mut s, &first, messages, "https://graph.microsoft.com/v1.0/me/messages?$skip=50");
    assert_eq!((said["taken"].as_u64(), said["more"].as_bool()), (Some(50), Some(true)));
    let shown = subjects(&s);
    assert_eq!(shown.first().map(String::as_str), Some("Budget 0"));
    assert!(!shown.contains(&"Budget here".to_string()), "shown once, among the local results");
    assert!(s.server_search.results.iter().all(|r| r.folder == inbox && r.opened.is_none()));
    // Nothing of it is in the mailbox.
    assert_eq!(s.mailbox.count(inbox), 1);
    // The next page goes on from the server's link; a page that came twice adds nothing.
    let next = asked(&mut s).remove(0);
    assert_eq!(next.next, "https://graph.microsoft.com/v1.0/me/messages?$skip=50");
    let again: Vec<GraphMessage> = (45..60).map(|i| found(&format!("AAkA-{i}"), "in", &format!("Budget {i}"), NOW - 60 * (i + 1))).collect();
    let said = answer(&mut s, &next, again, "");
    assert_eq!((said["taken"].as_u64(), said["more"].as_bool()), (Some(10), Some(false)));
    assert_eq!(s.server_search.results.len(), 60);
    assert!(asked(&mut s).is_empty(), "at its end");
}

#[test]
fn what_graph_doesnt_understand_is_checked_here_and_deleted_mail_stays_out_of_everywhere() {
    let (mut s, _, owner, _) = session();
    s.run("account.removeSharedMailbox", &json!({"address": SHARED})).unwrap();
    search(&mut s, "budget is:unread -draft", "everywhere");
    let request = asked(&mut s).remove(0);
    assert!(s.server_search.checked_here);
    let mut read = found("AAkA-read", "in", "Budget read", NOW);
    read.server.read = true;
    let messages = vec![
        found("AAkA-1", "in", "Budget unread", NOW - 60),
        read,
        found("AAkA-2", "in", "Budget draft", NOW - 120),
        found("AAkA-3", "de", "Budget deleted", NOW - 180),
        found("AAkA-4", "nowhere", "Budget in a folder not listed", NOW - 240),
    ];
    answer(&mut s, &request, messages, "");
    assert_eq!(subjects(&s), ["Budget unread"]);
    // Current Folder on Deleted Items finds what is there.
    s.view.folder = s.mailbox.folder_by_server(owner, "de").unwrap();
    search(&mut s, "budget", "folder");
    assert!(!s.server_search.checked_here);
    let request = asked(&mut s).remove(0);
    assert_eq!(request.folder, "de");
    answer(&mut s, &request, vec![found("AAkA-3", "de", "Budget deleted", NOW - 180)], "");
    assert_eq!(subjects(&s), ["Budget deleted"]);
}

/// #329: a day in words is checked here, on the session's day; tomorrow it is another search.
#[test]
fn a_day_in_words_is_checked_here_on_the_sessions_day() {
    let (mut s, _, _, _) = session();
    s.clock = crate::Clock { now: Some(NOW), offset: Some(0), zone: None };
    s.run("account.removeSharedMailbox", &json!({"address": SHARED})).unwrap();
    search(&mut s, "budget received:today", "everywhere");
    let request = asked(&mut s).remove(0);
    assert!(s.server_search.checked_here);
    let messages = vec![found("AAkA-1", "in", "Budget today", NOW - 60), found("AAkA-2", "in", "Budget two days ago", NOW - 2 * 86_400)];
    answer(&mut s, &request, messages, "");
    assert_eq!(subjects(&s), ["Budget today"]);
    // A day later the same words are a new search: what came for the old one is let go.
    s.clock.now = Some(NOW + 86_400);
    assert_eq!(asked(&mut s).len(), 1);
    assert!(subjects(&s).is_empty());
    // A search that doesn't need the day keeps what the server found past midnight, and when the
    // first day of the week changes.
    search(&mut s, "budget", "everywhere");
    let request = asked(&mut s).remove(0);
    answer(&mut s, &request, vec![found("AAkA-1", "in", "Budget today", NOW - 60)], "");
    s.clock.now = Some(NOW + 2 * 86_400);
    s.prefs.week_start = mailcraft_time::Weekday::Mon;
    assert!(is_current(&s));
    assert_eq!(subjects(&s), ["Budget today"]);
}

#[test]
fn a_new_search_forgets_the_results_and_an_answer_for_an_old_one_is_not_taken() {
    let (mut s, _, owner, _) = session();
    s.run("account.removeSharedMailbox", &json!({"address": SHARED})).unwrap();
    search(&mut s, "budget", "everywhere");
    let old = asked(&mut s).remove(0);
    answer(&mut s, &old, vec![found("AAkA-1", "in", "Budget", NOW)], "next");
    assert_eq!(s.server_search.results.len(), 1);
    search(&mut s, "lunch", "everywhere");
    // (What the window shows is for this search: the old results are gone.)
    let new = asked(&mut s).remove(0);
    assert!(new.ticket != old.ticket && new.next.is_empty());
    assert!(s.server_search.results.is_empty());
    let said = answer(&mut s, &old, vec![found("AAkA-2", "in", "Budget 2", NOW)], "");
    assert_eq!(said["stale"].as_bool(), Some(true));
    assert!(s.server_search.results.is_empty());
    // Cleared: forgotten too.
    answer(&mut s, &new, vec![found("AAkA-3", "in", "Lunch", NOW)], "");
    assert_eq!(s.server_search.results.len(), 1);
    s.run("view.clearSearch", &json!({})).unwrap();
    assert!(s.server_search.results.is_empty());
    // An answer for another mailbox than the one it names is refused.
    search(&mut s, "lunch", "everywhere");
    let mut request = asked(&mut s).remove(0);
    request.email = "stranger@example.org".into();
    let result = GraphSearchResult { account: owner, email: request.email.clone(), ticket: request.ticket, ..Default::default() };
    assert!(s.run("sync.applyGraphSearch", &json!({"result": result})).is_err());
}

#[test]
fn opening_a_result_puts_it_in_its_folder_to_be_fetched_and_selects_it() {
    let (mut s, _, _, shared) = session();
    search(&mut s, "budget", "everywhere");
    let requests = asked(&mut s);
    let theirs = requests.iter().find(|r| r.account == shared).unwrap().clone();
    let mut message = found("AAkA-1", "ar", "Budget archived", NOW);
    message.has_attachments = true;
    answer(&mut s, &theirs, vec![message], "");
    let archive = s.mailbox.folder_by_server(shared, "ar").unwrap();
    let before = s.mailbox.count(archive);
    let id = s.run("search.openServerResult", &json!({"index": 0})).unwrap()["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert_eq!((m.account, m.folder, m.unfetched, m.subject.as_str()), (shared, archive, true, "Budget archived"));
    assert!(m.has_attachments(), "the paperclip, before it is fetched");
    assert_eq!(m.server.as_ref().map(|sv| sv.id.as_str()), Some("AAkA-1"));
    assert_eq!(s.mailbox.count(archive), before + 1);
    assert_eq!(s.sel.messages, [id]);
    // The fetch it needs is the one any message not fetched gets.
    let fetch = crate::cmd::graph_mail::graph_fetch_request(&s, shared, Some(id), 1).unwrap();
    assert_eq!((fetch.ids.as_slice(), fetch.shared), (["AAkA-1".to_string()].as_slice(), true));
    // Opened again: the same message, not a second one.
    let again = s.run("search.openServerResult", &json!({"index": 0})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(again, id);
    assert_eq!(s.mailbox.count(archive), before + 1);
    // And a run that brings it changes nothing: it is the same message by its server id.
    let page = MessagePage {
        folder: "ar".into(),
        messages: vec![found("AAkA-1", "ar", "Budget archived", NOW)],
        link: "https://graph.example.org/d".into(),
        ..Default::default()
    };
    apply_graph_mail(&mut s, GraphMailResult { account: shared, email: SHARED.into(), pages: vec![page], partial: true, ..Default::default() })
        .unwrap();
    assert_eq!(s.mailbox.count(archive), before + 1);
    assert!(s.run("search.openServerResult", &json!({"index": 7})).is_err());
}

#[test]
fn a_refusal_is_said_and_the_mailbox_is_not_asked_again_for_this_search() {
    let (mut s, _, owner, shared) = session();
    search(&mut s, "budget", "everywhere");
    for request in asked(&mut s) {
        let mut result = GraphSearchResult { account: request.account, email: request.email.clone(), ticket: request.ticket, ..Default::default() };
        if request.account == shared {
            result.refused = Some(SharedRefusal::NoAccess);
            result.errors = vec![format!("{}.", SharedRefusal::NoAccess.said())];
        } else {
            result.offline = true;
            result.errors = vec!["Couldn't search on Microsoft's server: couldn't reach graph.microsoft.com: no route".into()];
        }
        s.run("sync.applyGraphSearch", &json!({"result": result})).unwrap();
    }
    let search = &s.server_search;
    assert!(search.offline && search.errors.len() == 2, "{:?}", search.errors);
    assert!(search.errors.iter().any(|e| e.contains("Full Access")));
    // The one that can't be reached is asked again when the owner asks; the refusal isn't.
    assert_eq!(asked(&mut s).iter().map(|r| r.account).collect::<Vec<_>>(), [owner]);
    assert!(s.server_search.errors.is_empty() && !s.server_search.offline, "asked again: said afresh");
}

#[test]
fn nothing_goes_to_the_server_for_a_search_it_cant_understand() {
    let (mut s, _, _, _) = session();
    search(&mut s, "is:unread category:red", "everywhere");
    assert!(crate::cmd::search_server::offered_reason(&s).is_some());
    assert!(s.run("sync.graphSearchRequest", &json!({})).is_err());
}

/// #493: Current Mailbox and Subfolders ask the account in view alone, for all of it; what comes
/// back is kept as the search here keeps it: the mailbox without Deleted Items, or what is under
/// the folder in view.
#[test]
fn current_mailbox_and_subfolders_ask_the_account_in_view_and_keep_what_they_look_at() {
    let (mut s, _, owner, shared) = session();
    let replies = || {
        vec![
            found("AAkA-in", "in", "Budget in", NOW),
            found("AAkA-ar", "ar", "Budget archived", NOW - 60),
            found("AAkA-de", "de", "Budget deleted", NOW - 120),
        ]
    };
    s.view.folder = s.mailbox.folder_by_server(owner, "in").unwrap();
    search(&mut s, "budget", "mailbox");
    let requests = asked(&mut s);
    assert_eq!(requests.iter().map(|r| (r.account, r.folder.as_str())).collect::<Vec<_>>(), [(owner, "")]);
    answer(&mut s, &requests[0], replies(), "");
    assert_eq!(subjects(&s), ["Budget in", "Budget archived"]);
    // Another folder of the same mailbox: the same search, so what was found is kept.
    s.view.folder = s.mailbox.folder_by_server(owner, "ar").unwrap();
    asked(&mut s);
    assert_eq!(subjects(&s), ["Budget in", "Budget archived"]);

    // Subfolders of the shared mailbox's Inbox: that mailbox asked, only its Inbox kept.
    s.view.folder = s.mailbox.folder_by_server(shared, "in").unwrap();
    search(&mut s, "budget", "subfolders");
    let requests = asked(&mut s);
    assert_eq!(requests.iter().map(|r| (r.account, r.folder.as_str())).collect::<Vec<_>>(), [(shared, "")]);
    answer(&mut s, &requests[0], replies(), "");
    assert_eq!(subjects(&s), ["Budget in"]);
}
