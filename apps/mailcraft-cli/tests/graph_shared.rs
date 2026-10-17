//! A shared mailbox beside its owner's (#70), from end to end: the engine's `graph_mail_request`
//! and `apply_graph_mail` with `mailcraft_net::graph::sync_mail` in between, against the strict
//! Graph stand-in. Search, unread counts and Favorites take it in here, on this computer, and
//! nothing of it reaches the owner's mailbox (`/me`) or goes up. Nothing here leaves this machine;
//! every address and message is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::folder::favorites;
use mailcraft_engine::cmd::graph_mail::{apply_graph_mail, graph_mail_request};
use mailcraft_model::{FolderKind, Id, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::{Value, json};

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;
/// Addresses of this file's own: the gate that counts a mailbox's requests is the process's.
const OWNER: &str = "lena@example.org";
const SHARED: &str = "frontdesk@example.org";

/// One whole run of `account` against `server`, each piece taken in as it comes.
fn run(server: &StandIn, s: &mut Session, account: Id) {
    let request = graph_mail_request(s, account).unwrap();
    let _routed = server.route();
    let cancel = AtomicBool::new(false);
    mailcraft_net::graph::sync_mail(TOKEN, &request, &cancel, &mut |piece| {
        apply_graph_mail(s, piece).unwrap();
    });
}

/// What the stand-in holds of the owner's own messages, in an order that compares.
fn owners_messages(server: &StandIn) -> Vec<String> {
    let state = server.state();
    let mut all: Vec<String> = state.mailbox.messages.iter().map(|(id, m)| format!("{id} {m:?}")).collect();
    all.sort();
    all
}

fn accounts_of(s: &Session, ids: &[Id]) -> Vec<Id> {
    let mut out: Vec<Id> = ids.iter().filter_map(|id| s.mailbox.message(*id)).map(|m| m.account).collect();
    out.sort_unstable();
    out
}

#[test]
fn a_shared_mailbox_is_searched_counted_and_a_favourite_here_and_nothing_of_it_reaches_the_owners_mailbox() {
    let server = StandIn::start().unwrap();
    server.state().add_other(SHARED, true);
    // Both mailboxes name their messages alike: the same ids and subjects, other messages.
    let mine = server.state().well_known("inbox");
    server.state().mailbox.generate(&mine, 3, NOW);
    let theirs = {
        let mut state = server.state();
        let other = state.other(SHARED).unwrap();
        let inbox = other.folders.iter().find(|f| f.well_known == "inbox").unwrap().id.clone();
        other.mailbox.generate(&inbox, 3, NOW);
        inbox
    };
    assert_ne!(mine, theirs);

    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.shared_mailboxes = true;
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": SHARED})).unwrap()["id"].as_u64().unwrap();
    run(&server, &mut s, owner);
    run(&server, &mut s, shared);
    let (my_inbox, their_inbox) = (s.mailbox.folder_of_kind(owner, FolderKind::Inbox).unwrap(), s.mailbox.folder_by_server(shared, &theirs).unwrap());
    assert_eq!((s.mailbox.count(my_inbox), s.mailbox.count(their_inbox)), (3, 3));
    let before = owners_messages(&server);

    // Search: All Mailboxes takes in both; the folder in view is its own mailbox's.
    s.run("view.search", &json!({"query": "message", "scope": "everywhere"})).unwrap();
    let all: Vec<Id> = s.rows().iter().map(|r| r.id).collect();
    assert_eq!(accounts_of(&s, &all), [owner, owner, owner, shared, shared, shared]);
    s.run("folder.select", &json!({"id": their_inbox})).unwrap();
    s.run("view.search", &json!({"query": "message", "scope": "folder"})).unwrap();
    let here: Vec<Id> = s.rows().iter().map(|r| r.id).collect();
    assert_eq!(accounts_of(&s, &here), [shared, shared, shared]);

    // Unread: one read in the shared mailbox here leaves the owner's as they were.
    let one = here.first().copied().unwrap();
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    assert_eq!((s.mailbox.unread_count(my_inbox), s.mailbox.unread_count(their_inbox)), (3, 2));
    let listed = s.run("folder.list", &json!({})).unwrap();
    let unread = |id: Id| listed["accounts"].as_array().unwrap().iter().find(|a| a["id"] == id).map(|a| a["unread"].clone());
    assert_eq!((unread(owner), unread(shared)), (Some(json!(3)), Some(json!(2))));

    // A favourite of the shared mailbox's, kept here.
    s.run("folder.favorite", &json!({"id": their_inbox, "value": true})).unwrap();
    assert!(favorites(&s).contains(&their_inbox));

    // The next runs: the shared mailbox's is at its own address and only reads; the owner's
    // says nothing of the shared mailbox.
    server.state().requests.clear();
    run(&server, &mut s, shared);
    let asked: Vec<(String, String, String)> = server.state().requests.iter().map(|r| (r.method.clone(), r.path.join("/"), r.body.clone())).collect();
    assert!(!asked.is_empty());
    for (method, path, body) in &asked {
        assert!(path == "v1.0/$batch" || path.starts_with(&format!("v1.0/users/{SHARED}/")), "{method} {path}");
        assert!(!path.contains("/me/") && !path.starts_with("v1.0/me"), "{method} {path}");
        assert!(method == "GET" || path == "v1.0/$batch", "nothing is written: {method} {path}");
        assert!(!body.contains("/me/") && !body.contains("\"PATCH\"") && !body.contains("\"DELETE\""), "{body}");
    }
    server.state().requests.clear();
    run(&server, &mut s, owner);
    assert!(!server.state().requests.is_empty());
    for r in server.state().requests.iter() {
        let said = format!("{} {} {}", r.target, r.path.join("/"), r.body);
        assert!(!said.to_lowercase().contains(SHARED), "the owner's run names the shared mailbox: {said}");
    }
    assert_eq!(owners_messages(&server), before, "the owner's messages are as they were");
    // The read mark waits here (changes go up in a later slice), and the favourite is still one.
    assert!(s.mailbox.message(one).is_some_and(|m| m.read));
    assert!(favorites(&s).contains(&their_inbox));
    let again: Value = s.run("folder.list", &json!({})).unwrap();
    assert_eq!(again["accounts"].as_array().unwrap().iter().find(|a| a["id"] == shared).map(|a| a["sharedOf"].clone()), Some(json!(owner)));
}

/// In a shared mailbox too, what changed on the server after every folder had come to its end
/// comes down with the next run (#148): read elsewhere (said as the message's id and `isRead`
/// alone, as Exchange Online says it), flagged, categorised; what was marked here stays so.
#[test]
fn what_changed_in_a_shared_mailbox_on_the_server_after_a_whole_run_comes_down_with_the_next() {
    const OWNER: &str = "rowan@example.org";
    const SHARED: &str = "orders@example.org";
    let server = StandIn::start().unwrap();
    server.state().add_other(SHARED, true);
    let (inbox, ids) = {
        let mut state = server.state();
        let other = state.other(SHARED).unwrap();
        other.mailbox.read_alone = true;
        let inbox = other.folders.iter().find(|f| f.well_known == "inbox").unwrap().id.clone();
        let ids = other.mailbox.generate(&inbox, 4, NOW);
        (inbox, ids)
    };
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.shared_mailboxes = true;
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": SHARED})).unwrap()["id"].as_u64().unwrap();
    run(&server, &mut s, owner);
    run(&server, &mut s, shared);
    let their_inbox = s.mailbox.folder_by_server(shared, &inbox).unwrap();
    assert_eq!(s.mailbox.unread_count(their_inbox), 4);
    let here = |s: &Session, id: &str| {
        s.mailbox.messages.values().find(|m| m.account == shared && m.server.as_ref().is_some_and(|s| s.id == *id)).map(|m| m.id).unwrap()
    };
    // Marked read here: it waits on this computer.
    let mine = here(&s, &ids[3]);
    s.run("mail.markRead", &json!({"ids": [mine]})).unwrap();
    {
        let mut state = server.state();
        let other = state.other(SHARED).unwrap();
        other.mailbox.update(&ids[0], |m| m.read = true);
        other.mailbox.update(&ids[1], |m| m.flag = "flagged".into());
        other.mailbox.update(&ids[2], |m| m.categories = vec!["Blue category".into()]);
    }
    run(&server, &mut s, shared);
    let m = |s: &Session, n: usize| s.mailbox.message(here(s, &ids[n])).unwrap().clone();
    assert!(m(&s, 0).read, "read elsewhere");
    assert_eq!(m(&s, 1).flag, mailcraft_model::FlagStatus::Flagged);
    assert_eq!(m(&s, 2).categories, ["Blue category"]);
    assert!(m(&s, 3).read, "marked read here");
    assert_eq!(s.mailbox.unread_count(their_inbox), 2);
}
