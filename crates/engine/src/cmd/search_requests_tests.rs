// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! More results on the server for an Exchange server and a JMAP account (#1725): which mailboxes
//! a search goes to, what their hosts are asked, and what is made of what comes back, with
//! results made by hand. (Against the servers' stand-ins: `crates/ews/src/find_tests.rs`,
//! `crates/jmap/src/search/tests.rs`.)

use mailcraft_model::sync::{
    EwsFolderDeltas, EwsMailResult, EwsMessagePage, FolderListing, GraphFolder, GraphMessage, GraphSearchRequest, GraphSearchResult, JmapMailResult,
    MessagePage, ServerMessage,
};
use mailcraft_model::{AccountKind, FolderKind, Id};
use serde_json::{Value, json};

use super::*;
use crate::cmd::ews_mail::apply_ews_mail;
use crate::cmd::jmap_mail::apply_jmap_mail;
use crate::cmd::{search_requests, search_server};

const EWS_ME: &str = "ada@example.org";
const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";
const JMAP_ME: &str = "bo@example.org";
const JMAP_SESSION: &str = "https://jmap.example.org/jmap/session";
const NOW: i64 = 1_790_000_000;

fn folder(id: &str, parent: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), parent: parent.into(), name: name.into(), kind, ..Default::default() }
}

/// The folders both accounts have: the Inbox, Deleted Items with a folder in it, Junk E-mail, and
/// a folder of the owner's with one inside it.
fn folders() -> Vec<GraphFolder> {
    vec![
        folder("pr", "", "Projects", FolderKind::Custom),
        folder("ap", "pr", "Apollo", FolderKind::Custom),
        folder("in", "", "Inbox", FolderKind::Inbox),
        folder("de", "", "Deleted Items", FolderKind::Deleted),
        folder("old", "de", "Old", FolderKind::Custom),
        folder("ju", "", "Junk E-mail", FolderKind::Junk),
    ]
}

/// A session with a local account, an Exchange server account and a JMAP account, the two with
/// their folders listed; the ids of the two.
fn session() -> (Session, Id, Id) {
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": "local@example.org"})).unwrap();
    let ews = s.run("account.add", &json!({"email": EWS_ME, "kind": "exchange", "ews": {"url": URL, "username": "EXAMPLE\\ada"}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    let deltas = EwsFolderDeltas { changed: folders(), afresh: true, ..Default::default() };
    let listed = EwsMailResult {
        account: ews,
        email: EWS_ME.into(),
        url: URL.into(),
        partial: true,
        folders: Some(deltas),
        hierarchy: "H1".into(),
        ..Default::default()
    };
    apply_ews_mail(&mut s, listed).unwrap();
    let jmap = s.run("account.add", &json!({"email": JMAP_ME, "kind": "jmap", "jmap": {"session": JMAP_SESSION, "bearer": true}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    let listing = Some(FolderListing { folders: folders(), complete: true });
    apply_jmap_mail(&mut s, JmapMailResult { account: jmap, email: JMAP_ME.into(), partial: true, listing, ..Default::default() }).unwrap();
    (s, ews, jmap)
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

fn asked(s: &mut Session) -> Vec<GraphSearchRequest> {
    serde_json::from_value(s.run("sync.graphSearchRequest", &json!({})).unwrap()["requests"].clone()).unwrap()
}

fn page_for(s: &mut Session, account: Id) -> GraphSearchRequest {
    asked(s).into_iter().find(|r| r.account == account).unwrap()
}

fn answer(s: &mut Session, request: &GraphSearchRequest, messages: Vec<GraphMessage>, next: &str) -> Value {
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

fn view(s: &mut Session, account: Id, server_id: &str) {
    s.view.folder = s.mailbox.folder_by_server(account, server_id).unwrap();
}

#[test]
fn a_search_of_everything_asks_the_exchange_and_the_jmap_account_and_not_the_local_one() {
    let (mut s, ews, jmap) = session();
    search(&mut s, "budget", "everywhere");
    assert_eq!(search_server::offered_reason(&s), None);
    let requests = asked(&mut s);
    let whose: Vec<(Id, &str, &str)> = requests.iter().map(|r| (r.account, r.email.as_str(), r.folder.as_str())).collect();
    assert_eq!(whose, [(ews, EWS_ME, ""), (jmap, JMAP_ME, "")]);
    // Current Folder: that folder, by the id its server knows it by.
    view(&mut s, jmap, "ap");
    search(&mut s, "budget", "folder");
    let requests = asked(&mut s);
    assert_eq!(requests.iter().map(|r| (r.account, r.folder.as_str())).collect::<Vec<_>>(), [(jmap, "ap")]);
    // An account whose server's address can't be used is not asked.
    s.mailbox.accounts.iter_mut().find(|a| a.id == ews).unwrap().ews = None;
    search(&mut s, "budget", "everywhere");
    assert_eq!(asked(&mut s).iter().map(|r| r.account).collect::<Vec<_>>(), [jmap]);
}

#[test]
fn an_exchange_server_is_asked_folder_by_folder_the_inbox_first_without_deleted_and_junk() {
    let (mut s, ews, _) = session();
    search(&mut s, "budget from:zoe", "everywhere");
    let page = page_for(&mut s, ews);
    let request = search_requests::ews(&s, &page).unwrap();
    assert_eq!(request.folders, ["in", "pr", "ap"]);
    assert_eq!((request.account, request.email.as_str(), request.url.as_str(), request.username.as_str()), (ews, EWS_ME, URL, "EXAMPLE\\ada"));
    assert_eq!((request.ticket, request.query.as_str(), request.next.as_str()), (page.ticket, "budget from:zoe", ""));

    // A folder and what is under it: those, Deleted Items included when that is the folder.
    view(&mut s, ews, "pr");
    search(&mut s, "budget", "subfolders");
    let page = page_for(&mut s, ews);
    assert_eq!(search_requests::ews(&s, &page).unwrap().folders, ["pr", "ap"]);
    view(&mut s, ews, "de");
    search(&mut s, "budget", "subfolders");
    let page = page_for(&mut s, ews);
    assert_eq!(search_requests::ews(&s, &page).unwrap().folders, ["de", "old"]);
    // One folder: that one, and where the last page ended goes with it.
    search(&mut s, "budget", "folder");
    let page = GraphSearchRequest { next: "50 q de".into(), ..page_for(&mut s, ews) };
    let request = search_requests::ews(&s, &page).unwrap();
    assert_eq!((request.folders.as_slice(), request.next.as_str()), (["de".to_string()].as_slice(), "50 q de"));
}

#[test]
fn a_jmap_server_is_told_what_stays_out_and_which_mailboxes_are_here() {
    let (mut s, _, jmap) = session();
    search(&mut s, "budget", "everywhere");
    let page = page_for(&mut s, jmap);
    let request = search_requests::jmap(&s, &page).unwrap();
    assert_eq!((request.account, request.email.as_str(), request.session.as_str(), request.bearer), (jmap, JMAP_ME, JMAP_SESSION, true));
    assert_eq!((request.folder.as_str(), request.query.as_str(), request.ticket), ("", "budget", page.ticket));
    let mut outside = request.outside.clone();
    outside.sort();
    assert_eq!(outside, ["de", "ju", "old"]);
    let inbox = request.places.iter().find(|place| place.id == "in").unwrap();
    assert_eq!((request.places.len(), inbox.kind), (6, FolderKind::Inbox));

    // A folder and what is under it is asked of the whole account, with nothing left out: what
    // is under the folder is kept when it comes back.
    view(&mut s, jmap, "de");
    search(&mut s, "budget", "subfolders");
    let page = page_for(&mut s, jmap);
    let request = search_requests::jmap(&s, &page).unwrap();
    assert!(request.folder.is_empty() && request.outside.is_empty());
    // One folder: that mailbox.
    search(&mut s, "budget", "folder");
    let page = GraphSearchRequest { next: "50".into(), ..page_for(&mut s, jmap) };
    let request = search_requests::jmap(&s, &page).unwrap();
    assert_eq!((request.folder.as_str(), request.outside.len(), request.next.as_str()), ("de", 0, "50"));
}

#[test]
fn the_requests_are_for_accounts_of_their_own_kind_alone() {
    let (mut s, ews, jmap) = session();
    search(&mut s, "budget", "everywhere");
    let (for_ews, for_jmap) = (by_hand(&s, ews), by_hand(&s, jmap));
    assert!(search_requests::jmap(&s, &for_ews).is_err());
    assert!(search_requests::ews(&s, &for_jmap).is_err());
    assert!(search_requests::ews(&s, &GraphSearchRequest { account: 9_999, ..Default::default() }).is_err());
    // Neither request shows the server, the owner or the search.
    let shown = format!("{:?} {:?}", search_requests::ews(&s, &for_ews).unwrap(), search_requests::jmap(&s, &for_jmap).unwrap());
    for secret in ["example.org", "budget", "EXAMPLE"] {
        assert!(!shown.contains(secret), "{shown}");
    }
}

/// A page for `account` made by hand, as the first of a search.
fn by_hand(s: &Session, account: Id) -> GraphSearchRequest {
    let a = s.mailbox.account(account).unwrap();
    GraphSearchRequest { account, email: a.email.clone(), query: "budget".into(), ticket: s.server_search.ticket, ..Default::default() }
}

#[test]
fn what_the_servers_find_is_held_newest_first_and_opened_into_its_folder_to_be_fetched() {
    let (mut s, ews, jmap) = session();
    search(&mut s, "budget", "everywhere");
    let requests = asked(&mut s);
    let ews_page = requests.iter().find(|r| r.account == ews).unwrap();
    let jmap_page = requests.iter().find(|r| r.account == jmap).unwrap();
    let said =
        answer(&mut s, ews_page, vec![found("E-1", "ap", "Budget of Apollo", NOW - 50), found("E-2", "old", "Budget thrown away", NOW)], "50 q pr");
    assert_eq!((said["taken"].as_u64(), said["more"].as_bool()), (Some(1), Some(true)), "what is in Deleted Items stays out");
    answer(&mut s, jmap_page, vec![found("J-1", "in", "Budget for the spring", NOW - 10), found("J-2", "nowhere", "Budget elsewhere", NOW)], "");
    assert_eq!(subjects(&s), ["Budget for the spring", "Budget of Apollo"]);
    assert!(s.server_search.more(), "the Exchange server has more to give");

    // Opened: in its folder, not fetched yet, selected, with the id its server knows it by.
    let id = s.run("search.openServerResult", &json!({"index": 1})).unwrap()["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert_eq!((m.account, m.unfetched, m.subject.as_str()), (ews, true, "Budget of Apollo"));
    assert_eq!(Some(m.folder), s.mailbox.folder_by_server(ews, "ap"));
    assert_eq!(m.server.as_ref().map(|server| server.id.as_str()), Some("E-1"));
    assert_eq!(s.sel.messages, [id]);
    assert_eq!(s.mailbox.account(ews).unwrap().kind, AccountKind::Exchange);
    // The host is then asked for it whole, as for any message of the account not fetched yet.
    let fetch = crate::cmd::ews_mail::ews_fetch_request(&s, ews, Some(id), 1).unwrap();
    assert_eq!(fetch.ids, ["E-1"]);
    // Opened again: the same message, not another.
    assert_eq!(s.run("search.openServerResult", &json!({"index": 1})).unwrap()["id"].as_u64(), Some(id));
    let jid = s.run("search.openServerResult", &json!({"index": 0})).unwrap()["id"].as_u64().unwrap();
    let fetch = crate::cmd::jmap_mail::jmap_fetch_request(&s, jmap, Some(jid), 1).unwrap();
    assert_eq!(fetch.ids, ["J-1"]);
}

#[test]
fn a_message_here_hides_the_servers_only_when_the_search_here_shows_it() {
    let (mut s, ews, _) = session();
    // Two messages of the Inbox are here as headers: one says the word in its subject, the
    // other only in its text, which is on the server.
    let page = EwsMessagePage {
        folder: "in".into(),
        messages: vec![found("E-1", "in", "Budget for the spring", NOW - 100), found("E-2", "in", "Notes from Tuesday", NOW - 200)],
        afresh: true,
        last: true,
        state: "S1".into(),
        ..Default::default()
    };
    let read = EwsMailResult { account: ews, email: EWS_ME.into(), url: URL.into(), partial: true, pages: vec![page], ..Default::default() };
    apply_ews_mail(&mut s, read).unwrap();
    search(&mut s, "budget", "everywhere");
    let request = page_for(&mut s, ews);
    let said = answer(
        &mut s,
        &request,
        vec![found("E-1", "in", "Budget for the spring", NOW - 100), found("E-2", "in", "Notes from Tuesday", NOW - 200)],
        "",
    );
    assert_eq!(said["taken"].as_u64(), Some(1));
    assert_eq!(subjects(&s), ["Notes from Tuesday"], "shown once: the one the search here doesn't find");
    // Opening it selects the message that is here, and adds none.
    let before = s.mailbox.messages.len();
    let id = s.run("search.openServerResult", &json!({"index": 0})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.messages.len(), before);
    assert_eq!(s.mailbox.message(id).unwrap().server.as_ref().unwrap().id, "E-2");
    // The same page again brings nothing twice.
    let again = GraphSearchResult {
        account: ews,
        email: EWS_ME.into(),
        ticket: s.server_search.ticket,
        messages: vec![found("E-2", "in", "Notes from Tuesday", NOW - 200)],
        ..Default::default()
    };
    assert_eq!(search_server::apply(&mut s, again).unwrap()["taken"].as_u64(), Some(0));
}

#[test]
fn a_page_of_a_jmap_window_and_a_search_result_are_one_message() {
    let (mut s, _, jmap) = session();
    search(&mut s, "budget", "everywhere");
    let request = page_for(&mut s, jmap);
    answer(&mut s, &request, vec![found("J-1", "in", "Budget for the spring", NOW)], "");
    let id = s.run("search.openServerResult", &json!({"index": 0})).unwrap()["id"].as_u64().unwrap();
    // A run then lists the same email in the same mailbox: it stays the one message.
    let window =
        MessagePage { folder: "in".into(), messages: vec![found("J-1", "in", "Budget for the spring", NOW)], afresh: true, ..Default::default() };
    apply_jmap_mail(&mut s, JmapMailResult { account: jmap, email: JMAP_ME.into(), partial: true, pages: vec![window], ..Default::default() })
        .unwrap();
    let mine: Vec<Id> = s.mailbox.messages.values().filter(|m| m.account == jmap).map(|m| m.id).collect();
    assert_eq!(mine, [id]);
}
