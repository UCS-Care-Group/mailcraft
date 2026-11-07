// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A folder deleted for good here is deleted on the server only when the server holds nothing in
//! it or under it that the owner wasn't shown (AU3-H7), against the strict Graph stand-in. Every
//! name and id is invented.

use std::time::Duration;

use mailcraft_model::sync::Shown;

use super::*;
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };

fn run_at(url: &str, changes: Vec<FolderChange>) -> GraphMailResult {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), changes, ..Default::default() };
    sync_folders_with(url, TOKEN, &request, Arc::new(Gate::new()), QUICK)
}

fn run(server: &StandIn, changes: Vec<FolderChange>) -> GraphMailResult {
    run_at(&server.url(), changes)
}

/// A server with "Old projects" in Deleted Items, a subfolder "2025" in it, and two messages in
/// each; the ids of the two folders and of the four messages.
fn server() -> (StandIn, String, String, Vec<String>) {
    let server = StandIn::start().unwrap();
    let mut state = server.state();
    let deleted = state.well_known("deleteditems");
    let top = state.add_folder("Old projects", Some(&deleted));
    let sub = state.add_folder("2025", Some(&top));
    let mut messages = Vec::new();
    for (n, folder) in [&top, &top, &sub, &sub].into_iter().enumerate() {
        messages.push(state.mailbox.add(folder, &format!("Plan {n}"), 1_800_000_000 + n as i64));
    }
    drop(state);
    (server, top, sub, messages)
}

/// The deletion of `top` with what the owner was shown: `folders` and `messages`.
fn delete(top: &str, folders: &[&String], messages: &[String]) -> FolderChange {
    let shown = Shown::of("Old projects", folders.iter().map(|f| f.as_str()), messages.iter().map(String::as_str));
    FolderChange::Delete { id: top.to_string(), shown }
}

fn deletes_asked(server: &StandIn) -> usize {
    server.state().requests.iter().filter(|a| a.method == "DELETE").count()
}

fn still_there(server: &StandIn, id: &str) -> bool {
    server.state().folders.iter().any(|f| f.id == id)
}

#[test]
fn a_folder_as_it_was_shown_is_deleted() {
    let (server, top, sub, messages) = server();
    let result = run(&server, vec![delete(&top, &[&top, &sub], &messages)]);
    assert!(matches!(result.changes[0].outcome, ChangeOutcome::Done { .. }), "{:?}", result.changes);
    assert_eq!(deletes_asked(&server), 1);
    assert!(!still_there(&server, &top) && !still_there(&server, &sub));
}

#[test]
fn mail_that_arrived_since_keeps_the_folder_and_says_how_much() {
    let (server, top, sub, messages) = server();
    // Two messages arrived after the owner last saw it: one in it, one in its subfolder.
    server.state().mailbox.add(&top, "Arrived since", 1_800_000_100);
    server.state().mailbox.add(&sub, "Arrived since too", 1_800_000_200);
    let result = run(&server, vec![delete(&top, &[&top, &sub], &messages)]);
    assert_eq!(result.changes[0].outcome, ChangeOutcome::NotShown { items: 2 }, "{:?}", result.changes);
    assert_eq!(deletes_asked(&server), 0, "nothing was deleted");
    assert!(still_there(&server, &top) && still_there(&server, &sub));
    // The listing after it brings the folder back.
    assert!(result.listing.is_some_and(|l| l.folders.iter().any(|f| f.id == top)));
}

#[test]
fn a_folder_made_elsewhere_under_it_keeps_it() {
    let (server, top, sub, messages) = server();
    server.state().add_folder("Made on the phone", Some(&sub));
    let result = run(&server, vec![delete(&top, &[&top, &sub], &messages)]);
    assert_eq!(result.changes[0].outcome, ChangeOutcome::NotShown { items: 1 }, "{:?}", result.changes);
    assert_eq!(deletes_asked(&server), 0);
    assert!(still_there(&server, &top));
}

#[test]
fn a_deletion_from_before_that_says_nothing_shown_goes_only_for_an_empty_folder() {
    let (server, top, _, _) = server();
    let result = run(&server, vec![FolderChange::Delete { id: top.clone(), shown: Shown::default() }]);
    // Two folders under it (the subfolder) and four messages: five things not shown.
    assert_eq!(result.changes[0].outcome, ChangeOutcome::NotShown { items: 5 }, "{:?}", result.changes);
    assert_eq!(deletes_asked(&server), 0);

    let deleted = server.state().well_known("deleteditems");
    let empty = server.state().add_folder("Empty", Some(&deleted));
    let result = run(&server, vec![FolderChange::Delete { id: empty.clone(), shown: Shown::default() }]);
    assert!(matches!(result.changes[0].outcome, ChangeOutcome::Done { .. }), "{:?}", result.changes);
    assert!(!still_there(&server, &empty));
}

#[test]
fn a_check_that_fails_deletes_nothing() {
    for path in ["messages", "childFolders"] {
        let (server, top, sub, messages) = server();
        server.state().fail_next.push((format!("GET /v1.0/me/mailFolders/{sub}/{path}"), 500, "ErrorInternalServerError".into()));
        let result = run(&server, vec![delete(&top, &[&top, &sub], &messages)]);
        assert!(matches!(&result.changes[0].outcome, ChangeOutcome::Refused { why } if why.contains("check")), "{path}: {:?}", result.changes);
        assert_eq!(deletes_asked(&server), 0, "{path}");
        assert!(still_there(&server, &top), "{path}");
    }
}

#[test]
fn a_server_busy_partway_through_the_check_leaves_the_deletion_waiting() {
    let (server, top, sub, messages) = server();
    // The folder itself is looked at; its subfolder's mail is "not now" however often it is asked.
    for _ in 0..12 {
        server.state().fail_next.push((format!("GET /v1.0/me/mailFolders/{sub}/messages"), 429, "TooManyRequests".into()));
    }
    let result = run(&server, vec![delete(&top, &[&top, &sub], &messages)]);
    assert!(result.changes.is_empty(), "the deletion waits, to be checked again: {:?}", result.changes);
    assert!(!result.errors.is_empty());
    assert_eq!(deletes_asked(&server), 0);
    assert!(still_there(&server, &top));
}

#[test]
fn a_server_that_cant_be_reached_is_asked_nothing_and_nothing_is_taken_for_done() {
    let (server, top, sub, messages) = server();
    // (A port nothing listens on.)
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let result = run_at(&url, vec![delete(&top, &[&top, &sub], &messages)]);
    assert!(result.changes.is_empty(), "the deletion waits: {:?}", result.changes);
    assert!(!result.errors.is_empty());
    assert!(still_there(&server, &top));
}

#[test]
fn a_folder_already_gone_on_the_server_is_gone() {
    let (server, top, sub, messages) = server();
    server.state().remove_folder(&top);
    let result = run(&server, vec![delete(&top, &[&top, &sub], &messages)]);
    assert_eq!(result.changes[0].outcome, ChangeOutcome::Gone, "{:?}", result.changes);
    assert_eq!(deletes_asked(&server), 0);
}
