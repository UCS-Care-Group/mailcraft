// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The server's own folders (Conversation History, Sync Issues and the folders in it, #779) against
//! the strict stand-in: told apart by the ids the server gives for their distinguished names, in a
//! request of their own that never fails the run but for a refused sign-in or a busy server, and
//! never renamed, moved or deleted.

use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{ChangeOutcome, EwsMailRequest, EwsMailResult, FolderChange};

use crate::standin::{AUTHORIZATION, StandIn, block_on};
use crate::*;

const URL: &str = "https://mail.example.org/EWS/Exchange.asmx";

fn client() -> Client {
    Client::new(URL, Secret::new(AUTHORIZATION)).unwrap()
}

fn request() -> EwsMailRequest {
    EwsMailRequest { email: "ada@example.org".into(), url: URL.into(), list_folders: true, ..Default::default() }
}

/// Every piece of the run, taken in one after the other.
fn run(standin: &StandIn, request: &EwsMailRequest) -> EwsMailResult {
    let mut all = EwsMailResult::default();
    block_on(sync_mail(&client(), standin, request, &AtomicBool::new(false), &mut |_| true, &mut |piece| all.absorb(piece))).unwrap();
    all
}

/// A stand-in whose mailbox has Conversation History and Sync Issues with its three folders, and a
/// folder of the owner's that is also called "Conversation History", in the Inbox. The ids of the
/// server's five and of the owner's.
fn mailbox(standin: &StandIn) -> (Vec<String>, String) {
    let mut s = standin.state();
    let mut own = vec![s.add_own_folder("conversationhistory", "Conversation History", "msgfolderroot")];
    own.push(s.add_own_folder("syncissues", "Sync Issues", "msgfolderroot"));
    for (name, shown) in [("conflicts", "Conflicts"), ("localfailures", "Local Failures"), ("serverfailures", "Server Failures")] {
        own.push(s.add_own_folder(name, shown, "syncissues"));
    }
    let mine = s.add_own_folder("", "Conversation History", "inbox");
    (own, mine)
}

fn marked(result: &EwsMailResult) -> &[String] {
    &result.folders.as_ref().unwrap().well_known
}

#[test]
fn the_servers_own_folders_are_marked_by_id_and_the_owners_namesake_is_not() {
    let standin = StandIn::new();
    let (own, mine) = mailbox(&standin);
    let result = run(&standin, &request());
    for id in &own {
        assert!(marked(&result).contains(id), "one of the server's own is marked");
    }
    let inbox = standin.state().folder_id("inbox").unwrap();
    assert!(marked(&result).contains(&inbox), "the special folders are the server's own too");
    assert!(!marked(&result).contains(&mine), "a folder of the owner's of the same name is not");
    let top = standin.state().folder_id("msgfolderroot").unwrap();
    assert!(!marked(&result).contains(&top), "the top of the mail is no folder here");
    assert!(result.errors.is_empty() && result.whole, "{:?}", result.errors);
}

#[test]
fn a_delta_that_brings_no_folder_still_marks_them() {
    let standin = StandIn::new();
    let (own, _) = mailbox(&standin);
    let first = run(&standin, &request());
    // A later run from the state the first gave: the server has nothing new to say of its folders.
    let later = run(&standin, &EwsMailRequest { hierarchy: first.hierarchy.clone(), ..request() });
    let folders = later.folders.as_ref().unwrap();
    assert!(!folders.afresh && folders.changed.is_empty(), "a delta with nothing in it");
    for id in &own {
        assert!(folders.well_known.contains(id));
    }
}

#[test]
fn a_server_whose_schema_lacks_a_name_has_the_others_asked_in_groups() {
    let standin = StandIn::new();
    let (own, _) = mailbox(&standin);
    standin.state().unknown_names = vec!["conversationhistory".into()];
    let result = run(&standin, &request());
    assert!(!marked(&result).contains(&own[0]), "a name the server doesn't know is not there");
    for id in &own[1..] {
        assert!(marked(&result).contains(id), "the four it knows are marked");
    }
    assert!(result.errors.is_empty() && result.whole, "Send/Receive doesn't fail over it: {:?}", result.errors);
    // All of them, then the Outbox, Conversation History and Sync Issues with its three; then the
    // special folders.
    let getfolders = standin.state().requests.iter().filter(|r| r.operation == "GetFolder").count();
    assert_eq!(getfolders, 1 + 3 + 1);
}

#[test]
fn a_server_whose_schema_lacks_one_of_a_group_has_that_group_asked_name_by_name() {
    let standin = StandIn::new();
    let (own, _) = mailbox(&standin);
    standin.state().unknown_names = vec!["localfailures".into()];
    let result = run(&standin, &request());
    assert!(!marked(&result).contains(&own[3]));
    assert!([0, 1, 2, 4].iter().all(|&n| marked(&result).contains(&own[n])));
    assert!(result.errors.is_empty() && result.whole, "{:?}", result.errors);
}

#[test]
fn a_refused_lookup_is_taken_as_none_there_and_the_run_goes_on() {
    let standin = StandIn::new();
    let (own, _) = mailbox(&standin);
    standin.state().deliver("inbox", "Kickoff", false);
    standin.state().refuse_names = Some((vec!["conversationhistory".into()], 403));
    let result = run(&standin, &request());
    assert!(own.iter().all(|id| !marked(&result).contains(id)), "none of them is taken as there");
    let inbox = standin.state().folder_id("inbox").unwrap();
    assert!(marked(&result).contains(&inbox), "the special folders are still marked");
    assert!(result.errors.is_empty() && result.whole, "the run went well: {:?}", result.errors);
    assert!(result.pages.iter().flat_map(|p| &p.messages).any(|m| m.subject == "Kickoff"), "the Inbox was read");
}

#[test]
fn a_lookup_the_server_fails_over_has_the_names_asked_in_groups() {
    let standin = StandIn::new();
    let (own, _) = mailbox(&standin);
    // (A 500 that is no fault: no answer of the service, for the request that names it.)
    standin.state().refuse_names = Some((vec!["conversationhistory".into()], 500));
    let result = run(&standin, &request());
    assert!(!marked(&result).contains(&own[0]));
    assert!(own[1..].iter().all(|id| marked(&result).contains(id)));
    assert!(result.errors.is_empty() && result.whole, "{:?}", result.errors);
}

#[test]
fn a_refused_sign_in_or_a_busy_server_on_the_lookup_ends_the_run_as_any_request_does() {
    let standin = StandIn::new();
    mailbox(&standin);
    standin.state().refuse_names = Some((vec!["conversationhistory".into()], 401));
    let result = run(&standin, &request());
    assert!(result.auth_refused && !result.whole);
    for busy in [429, 503] {
        let standin = StandIn::new();
        mailbox(&standin);
        standin.state().refuse_names = Some((vec!["conversationhistory".into()], busy));
        let result = run(&standin, &request());
        assert!(result.busy_for.is_some() && !result.whole, "{busy}");
    }
}

#[test]
fn a_change_of_one_of_the_servers_own_is_refused_without_asking_and_the_list_is_read_afresh() {
    let standin = StandIn::new();
    let (own, mine) = mailbox(&standin);
    let first = run(&standin, &request());
    let history = own[0].clone();
    let changes = vec![
        FolderChange::Rename { folder: 10, id: history.clone(), name: "Chats".into() },
        FolderChange::Move { folder: 10, id: own[1].clone(), parent: mine.clone() },
        FolderChange::Delete { id: history.clone(), shown: Default::default() },
        FolderChange::Rename { folder: 11, id: mine.clone(), name: "My chats".into() },
    ];
    let result = run(&standin, &EwsMailRequest { changes, hierarchy: first.hierarchy.clone(), ..request() });
    for done in result.changes.iter().take(3) {
        assert!(matches!(&done.outcome, ChangeOutcome::Refused { why } if why.contains("server's own")), "{:?}", done.outcome);
    }
    assert!(matches!(&result.changes[3].outcome, ChangeOutcome::Done { .. }), "the owner's folder is renamed");
    let state = standin.state();
    assert_eq!(state.writes, ["UpdateFolder"], "only the owner's folder was asked about");
    assert!(state.deletes.is_empty());
    assert!(state.folders.iter().any(|f| f.id == history && f.name == "Conversation History"));
    drop(state);
    // A folder deleted here comes back from a read of the list afresh, marked.
    let folders = result.folders.as_ref().unwrap();
    assert!(folders.afresh && folders.changed.iter().any(|f| f.id == history), "the folder list is read afresh");
    assert!(folders.well_known.contains(&history));
}

#[test]
fn a_run_that_changes_no_folder_and_lists_none_asks_nothing_more() {
    let standin = StandIn::new();
    mailbox(&standin);
    let light = EwsMailRequest { list_folders: false, ..request() };
    run(&standin, &light);
    let getfolders = standin.state().requests.iter().filter(|r| r.operation == "GetFolder").count();
    assert_eq!(getfolders, 1, "the special folders alone");
}

#[test]
fn a_run_that_lists_no_folders_still_reads_them_afresh_after_a_refused_deletion() {
    let standin = StandIn::new();
    let (own, _) = mailbox(&standin);
    let first = run(&standin, &request());
    let light = EwsMailRequest {
        list_folders: false,
        hierarchy: first.hierarchy.clone(),
        changes: vec![FolderChange::Delete { id: own[0].clone(), shown: Default::default() }],
        ..request()
    };
    let result = run(&standin, &light);
    assert!(matches!(&result.changes[0].outcome, ChangeOutcome::Refused { .. }));
    let folders = result.folders.as_ref().expect("the folder list is read");
    assert!(folders.afresh && folders.changed.iter().any(|f| f.id == own[0]) && folders.well_known.contains(&own[0]));
}

#[test]
fn a_deletion_the_server_refuses_has_the_folder_list_read_afresh() {
    let standin = StandIn::new();
    let (_, mine) = mailbox(&standin);
    let first = run(&standin, &request());
    // (The Inbox is no folder of [`OWN`]: the deletion is asked, and the server refuses it. What
    // it holds, the owner's folder, was shown, AU3-H7.)
    let inbox = standin.state().folder_id("inbox").unwrap();
    let shown = mailcraft_model::sync::Shown::of("Inbox", [inbox.as_str(), mine.as_str()], []);
    let result = run(
        &standin,
        &EwsMailRequest { hierarchy: first.hierarchy.clone(), changes: vec![FolderChange::Delete { id: inbox.clone(), shown }], ..request() },
    );
    assert!(matches!(&result.changes[0].outcome, ChangeOutcome::OwnFolder { why } if why.contains("doesn't let")));
    assert_eq!(standin.state().writes, ["DeleteFolder"]);
    let folders = result.folders.as_ref().unwrap();
    assert!(folders.afresh && folders.changed.iter().any(|f| f.id == inbox), "a delta wouldn't bring it back");
}

#[test]
fn a_change_of_the_servers_outbox_is_refused_without_asking() {
    let standin = StandIn::new();
    mailbox(&standin);
    // (One that holds mail is listed as a folder like any other.)
    standin.state().deliver("outbox", "Waiting", true);
    let outbox = standin.state().folder_id("outbox").unwrap();
    let changes = vec![FolderChange::Rename { folder: 10, id: outbox.clone(), name: "Mine".into() }];
    let result = run(&standin, &EwsMailRequest { changes, ..request() });
    assert!(matches!(&result.changes[0].outcome, ChangeOutcome::Refused { why } if why.contains("server's own")));
    assert!(standin.state().writes.is_empty());
    assert!(marked(&result).contains(&outbox));
}
