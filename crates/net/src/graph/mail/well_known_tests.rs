// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The server's own folders in a Microsoft account's listing (Conversation History and the like,
//! #756), against the strict Graph stand-in. Every name is invented.

use std::time::Duration;

use super::*;
use crate::graph::standin::StandIn;

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };

fn run(server: &StandIn) -> GraphMailResult {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), ..Default::default() };
    sync_folders_with(&server.url(), crate::graph::standin::TOKEN, &request, Arc::new(Gate::new()), QUICK)
}

/// A mailbox with Conversation History and a folder of the owner's.
fn server() -> StandIn {
    let server = StandIn::start().unwrap();
    let mut state = server.state();
    state.add_folder("Projects", None);
    // (The stand-in's own is not shown, as most of Exchange's are not: this one is.)
    if let Some(folder) = state.folders.iter_mut().find(|f| f.well_known == "conversationhistory") {
        folder.hidden = false;
    }
    drop(state);
    server
}

fn mark(listing: &FolderListing, name: &str) -> Option<bool> {
    listing.folders.iter().find(|f| f.name == name).map(|f| f.well_known)
}

#[test]
fn the_servers_own_folders_are_marked_and_the_owners_and_the_top_are_not() {
    let server = server();
    let outbox = server.state().well_known("outbox");
    server.state().mailbox.add(&outbox, "Stuck on its way", 1_800_000_000);
    let listing = run(&server).listing.unwrap();
    assert!(listing.complete);
    assert_eq!(mark(&listing, "Conversation History"), Some(true));
    assert_eq!(mark(&listing, "Inbox"), Some(true));
    assert_eq!(mark(&listing, "Outbox"), Some(true), "the server's Outbox, once it holds mail");
    assert_eq!(mark(&listing, "Projects"), Some(false));
    // The top of the mailbox is not a folder of the listing.
    assert_eq!(mark(&listing, "Top of Information Store"), None);
}

#[test]
fn a_mailbox_without_those_folders_lists_whole_and_marks_none_of_the_owners() {
    let server = StandIn::start().unwrap();
    server.state().add_folder("Projects", None);
    let listing = run(&server).listing.unwrap();
    assert!(listing.complete);
    assert_eq!(mark(&listing, "Projects"), Some(false));
    assert!(listing.folders.iter().all(|f| f.well_known == (f.kind != FolderKind::Custom)), "only the special folders are marked");
}

#[test]
fn what_a_mailbox_wont_say_of_a_folder_only_told_apart_does_not_end_the_listing() {
    for (status, code) in [(403, "ErrorAccessDenied"), (400, "ErrorInvalidRequest")] {
        let server = server();
        // Every name that is only told apart, in the batch and again one at a time.
        for alias in ["clutter", "conflicts", "localfailures", "recoverableitemsdeletions", "searchfolders", "syncissues"] {
            for _ in 0..2 {
                server.state().fail_next.push((format!("GET /v1.0/me/mailFolders/{alias}"), status, code.into()));
            }
        }
        let result = run(&server);
        let listing = result.listing.unwrap();
        assert!(listing.complete, "{status}: {:?}", result.errors);
        assert_eq!(mark(&listing, "Inbox"), Some(true), "{status}");
    }
}

#[test]
fn a_server_error_once_for_one_of_them_is_asked_again_and_the_folder_is_still_marked() {
    let server = server();
    server.state().fail_next.push(("GET /v1.0/me/mailFolders/conversationhistory".into(), 500, "ServiceUnavailable".into()));
    let listing = run(&server).listing.unwrap();
    assert!(listing.complete);
    assert_eq!(mark(&listing, "Conversation History"), Some(true));
}

#[test]
fn a_server_error_every_time_for_one_of_them_takes_it_as_not_there() {
    for status in [500, 502] {
        let server = server();
        // However often it is asked.
        for _ in 0..12 {
            server.state().fail_next.push(("GET /v1.0/me/mailFolders/conversationhistory".into(), status, "ServiceUnavailable".into()));
        }
        let result = run(&server);
        let listing = result.listing.unwrap();
        assert!(listing.complete, "{status}: {:?}", result.errors);
        assert_eq!(mark(&listing, "Conversation History"), Some(false), "{status}: not known this run");
        assert_eq!(mark(&listing, "Inbox"), Some(true), "{status}");
    }
}

#[test]
fn a_batch_of_them_the_server_fails_takes_them_all_as_not_there() {
    let server = server();
    // The special folders' batch, then that of the others: both 500.
    for _ in 0..2 {
        server.state().fail_next.push(("POST /v1.0/$batch".into(), 500, "ServiceUnavailable".into()));
    }
    let result = run(&server);
    let listing = result.listing.unwrap();
    assert!(listing.complete, "{:?}", result.errors);
    assert_eq!(mark(&listing, "Inbox"), Some(true), "the special folders, one at a time");
    assert_eq!(mark(&listing, "Conversation History"), Some(false));
    // Not asked one by one either.
    let alone = server.state().requests.iter().filter(|a| a.method == "GET" && a.target.contains("mailFolders/conversationhistory")).count();
    assert_eq!(alone, 0);
}

#[test]
fn an_owners_folder_called_conversation_history_is_not_the_servers() {
    let server = server();
    let inbox = server.state().well_known("inbox");
    server.state().add_folder("Conversation History", Some(&inbox));
    let listing = run(&server).listing.unwrap();
    assert!(listing.complete);
    let mut marks: Vec<(bool, bool)> =
        listing.folders.iter().filter(|f| f.name == "Conversation History").map(|f| (f.parent == inbox, f.well_known)).collect();
    marks.sort_unstable();
    assert_eq!(marks, [(false, true), (true, false)], "the server's at the top, the owner's under the Inbox");
}

#[test]
fn a_change_of_one_of_them_is_refused_without_asking_and_the_listing_goes_on() {
    let server = server();
    let own = server.state().well_known("conversationhistory");
    let deleted = server.state().well_known("deleteditems");
    let projects = server.state().folders.iter().find(|f| f.name == "Projects").map(|f| f.id.clone()).unwrap();
    let changes = vec![
        FolderChange::Rename { folder: 41, id: own.clone(), name: "Mine".into() },
        FolderChange::Move { folder: 41, id: own.clone(), parent: deleted.clone() },
        FolderChange::Delete { id: own.clone(), shown: Default::default() },
        // The owner's own still goes.
        FolderChange::Rename { folder: 42, id: projects.clone(), name: "Projects 2026".into() },
    ];
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), changes, ..Default::default() };
    let result = sync_folders_with(&server.url(), crate::graph::standin::TOKEN, &request, Arc::new(Gate::new()), QUICK);
    assert_eq!(result.changes.len(), 4, "{:?}", result.changes);
    for done in result.changes.iter().take(3) {
        assert!(matches!(&done.outcome, ChangeOutcome::Refused { why } if why.contains("server's own")), "{done:?}");
    }
    assert!(matches!(result.changes[3].outcome, ChangeOutcome::Done { .. }), "{:?}", result.changes[3]);
    // Nothing was asked of the server's own folder but to tell it apart.
    let asked = server.state().requests.iter().filter(|a| a.method != "GET" && a.target.contains(&own)).count();
    assert_eq!(asked, 0);
    assert!(result.listing.is_some_and(|l| l.complete), "{:?}", result.errors);
    assert_eq!(server.state().folders.iter().find(|f| f.id == own).map(|f| f.name.clone()).as_deref(), Some("Conversation History"));
}

#[test]
fn a_sign_in_or_a_slow_down_for_one_of_them_still_ends_the_listing() {
    for (status, code) in [(401, "InvalidAuthenticationToken"), (429, "TooManyRequests")] {
        let server = server();
        // However often it is asked.
        for _ in 0..12 {
            server.state().fail_next.push(("GET /v1.0/me/mailFolders/syncissues".into(), status, code.into()));
        }
        let result = run(&server);
        assert!(result.listing.is_none() && !result.errors.is_empty(), "{status}: {:?}", result.listing.map(|l| l.complete));
    }
}
