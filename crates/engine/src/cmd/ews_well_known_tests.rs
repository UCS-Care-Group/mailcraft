// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The server's own folders of an Exchange server account (Conversation History, Sync Issues and
//! the like, #779), as the engine takes them in: marked by the ids the run names, whether the delta
//! of the folder list brings them or not, the mark kept once set, and never renamed, moved or
//! deleted. (Against `mailcraft-ews`'s stand-in: `crates/ews/src/own_folders_tests.rs`.)

use mailcraft_model::Queued;
use mailcraft_model::sync::{ChangeOutcome, EwsFolderDeltas, FolderChange, FolderChangeDone, GraphFolder};
use serde_json::json;

use super::*;

const ME: &str = "ada@example.org";
const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": URL}})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn folder(id: &str, parent: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), parent: parent.into(), name: name.into(), kind, ..Default::default() }
}

/// The server's folders: the special ones, Conversation History (the server's own), Projects and a
/// folder of the owner's in the Inbox that is also called "Conversation History".
fn folders() -> Vec<GraphFolder> {
    vec![
        folder("in", "", "Inbox", FolderKind::Inbox),
        folder("de", "", "Deleted Items", FolderKind::Deleted),
        folder("co", "", "Conversation History", FolderKind::Custom),
        folder("pr", "", "Projects", FolderKind::Custom),
        folder("mine", "in", "Conversation History", FolderKind::Custom),
    ]
}

fn piece(account: Id) -> EwsMailResult {
    EwsMailResult { account, email: ME.into(), url: URL.into(), partial: true, ..Default::default() }
}

/// The folder list as a run brings it, with the ids of the server's own it named.
fn take(s: &mut Session, account: Id, deltas: EwsFolderDeltas) {
    apply_ews_mail(s, EwsMailResult { folders: Some(deltas), hierarchy: "HIER".into(), ..piece(account) }).unwrap();
}

/// A first read, before the server's own folders were told apart (an earlier version).
fn first_read(s: &mut Session, account: Id) {
    take(s, account, EwsFolderDeltas { changed: folders(), afresh: true, ..Default::default() });
}

/// A later run whose delta brings nothing, naming the server's own.
fn empty_delta(own: &[&str]) -> EwsFolderDeltas {
    EwsFolderDeltas { well_known: own.iter().map(|id| (*id).to_owned()).collect(), ..Default::default() }
}

fn here(s: &Session, account: Id, id: &str) -> Id {
    s.mailbox.folder_by_server(account, id).unwrap()
}

fn marked(s: &Session, account: Id, id: &str) -> bool {
    s.mailbox.folder(here(s, account, id)).unwrap().server_owned()
}

#[test]
fn a_delta_that_brings_nothing_marks_the_servers_own_folders_of_a_mailbox_read_before() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    assert!(!marked(&s, account, "co"), "an earlier version didn't tell it apart");
    take(&mut s, account, empty_delta(&["in", "de", "co"]));
    assert!(marked(&s, account, "co") && marked(&s, account, "in"));
    assert!(!marked(&s, account, "mine") && !marked(&s, account, "pr"), "matched by id, not by name");
}

#[test]
fn the_mark_is_kept_when_a_later_run_could_not_ask() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    take(&mut s, account, empty_delta(&["co"]));
    // The lookup failed: nothing is named, and the folder is in the delta besides.
    take(&mut s, account, EwsFolderDeltas { changed: vec![folder("co", "", "Conversation History", FolderKind::Custom)], ..Default::default() });
    assert!(marked(&s, account, "co"));
    take(&mut s, account, EwsFolderDeltas::default());
    assert!(marked(&s, account, "co"));
}

#[test]
fn rename_move_and_delete_are_greyed_out_and_refused_for_the_servers_own() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    take(&mut s, account, empty_delta(&["in", "de", "co"]));
    let (co, mine, pr) = (here(&s, account, "co"), here(&s, account, "mine"), here(&s, account, "pr"));
    s.view.folder = co;
    for id in ["folder.rename", "folder.move", "folder.delete"] {
        assert!(!s.enabled(id), "{id}");
    }
    assert!(s.run("folder.rename", &json!({"id": co, "name": "Chats"})).is_err());
    assert!(s.run("folder.move", &json!({"id": co, "parent": pr})).is_err());
    assert!(s.run("folder.delete", &json!({"id": co})).is_err());
    assert_eq!(s.mailbox.folder(co).map(|f| (f.name.as_str(), f.parent)), Some(("Conversation History", None)));
    // The owner's namesake is the owner's.
    s.view.folder = mine;
    for id in ["folder.rename", "folder.move", "folder.delete"] {
        assert!(s.enabled(id), "{id}");
    }
    s.run("folder.rename", &json!({"id": mine, "name": "Old chats"})).unwrap();
    assert!(graph_mail::folder_changes(&s.mailbox, account, 10).0.iter().any(|c| matches!(c, FolderChange::Rename { id, .. } if id == "mine")));
}

#[test]
fn a_change_an_earlier_version_left_is_undone_and_never_sent() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    let co = here(&s, account, "co");
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted);
    // Moved into Deleted Items and renamed before it was told apart.
    s.mailbox.folder_mut(co).unwrap().parent = deleted;
    s.mailbox.folder_mut(co).unwrap().name = "Mine".into();
    assert_eq!(graph_mail::folder_changes(&s.mailbox, account, 10).1, 2, "the old version would send a rename and a move");
    take(&mut s, account, empty_delta(&["co"]));
    let f = s.mailbox.folder(co).unwrap();
    assert_eq!((f.parent, f.name.as_str()), (None, "Conversation History"), "put back where the server has it");
    assert_eq!(graph_mail::folder_changes(&s.mailbox, account, 10), (vec![], 0));
    let request = ews_mail_request(&s, account, false).unwrap();
    assert!(request.changes.is_empty());
}

#[test]
fn a_deletion_queued_of_one_no_longer_here_is_dropped() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    // Deleted for good by an earlier version: gone here, its deletion waiting.
    s.mailbox.folders.retain(|f| f.server.as_ref().is_none_or(|server| server.id != "co"));
    s.mailbox.queued.push(Queued::DeleteFolder { account, id: "co".into(), shown: Default::default() });
    s.mailbox.queued.push(Queued::DeleteFolder { account, id: "pr".into(), shown: Default::default() });
    take(&mut s, account, empty_delta(&["co"]));
    let left: Vec<&str> = s
        .mailbox
        .queued
        .iter()
        .filter_map(|q| match q {
            Queued::DeleteFolder { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(left, ["pr"], "the owner's own folder still goes");
    // Nothing would bring it back from a delta: the next run reads the folder list afresh.
    let request = ews_mail_request(&s, account, false).unwrap();
    assert!(request.hierarchy.is_empty() && request.list_folders, "the folder list is read afresh");
    take(&mut s, account, EwsFolderDeltas { changed: folders(), afresh: true, well_known: vec!["co".into()], ..Default::default() });
    assert!(marked(&s, account, "co"), "back, and marked");
}

#[test]
fn a_dropped_deletion_of_one_still_here_keeps_the_folder_list_going_on_from_its_state() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    s.mailbox.queued.push(Queued::DeleteFolder { account, id: "co".into(), shown: Default::default() });
    take(&mut s, account, empty_delta(&["co"]));
    assert!(s.mailbox.queued.is_empty());
    assert_eq!(ews_mail_request(&s, account, false).unwrap().hierarchy, "HIER");
}

#[test]
fn a_refused_deletion_is_dropped_and_the_folder_comes_back_from_the_read_afresh_marked() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    s.mailbox.folders.retain(|f| f.server.as_ref().is_none_or(|server| server.id != "co"));
    s.mailbox.queued.push(Queued::DeleteFolder { account, id: "co".into(), shown: Default::default() });
    let refused = FolderChangeDone {
        change: FolderChange::Delete { id: "co".into(), shown: Default::default() },
        outcome: ChangeOutcome::Refused { why: "it is one of the server's own folders, which may not be renamed, moved or deleted".into() },
    };
    apply_ews_mail(&mut s, EwsMailResult { changes: vec![refused], ..piece(account) }).unwrap();
    assert!(s.mailbox.queued.is_empty());
    // The folder list's state went with the refusal: a run that stopped here (a 429, called off,
    // a read of the list that failed) leaves the next one to read it afresh.
    assert!(ews_mail_request(&s, account, false).unwrap().hierarchy.is_empty(), "a refused deletion reads the folder list afresh");
    // The run read the folder list afresh, as it does after such a refusal, and goes on from there.
    take(&mut s, account, EwsFolderDeltas { changed: folders(), afresh: true, well_known: vec!["co".into()], ..Default::default() });
    assert!(marked(&s, account, "co"));
    assert_eq!(ews_mail_request(&s, account, false).unwrap().hierarchy, "HIER");
}

#[test]
fn a_run_that_stops_after_a_refused_deletion_reads_the_folder_list_afresh_next_time() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    s.mailbox.folders.retain(|f| f.server.as_ref().is_none_or(|server| server.id != "pr"));
    s.mailbox.queued.push(Queued::DeleteFolder { account, id: "pr".into(), shown: Default::default() });
    let refused = FolderChangeDone {
        change: FolderChange::Delete { id: "pr".into(), shown: Default::default() },
        outcome: ChangeOutcome::Refused { why: "access denied".into() },
    };
    // The push piece of a run the server then asked to leave alone: no folder piece follows.
    apply_ews_mail(&mut s, EwsMailResult { changes: vec![refused], ..piece(account) }).unwrap();
    let request = ews_mail_request(&s, account, false).unwrap();
    assert!(request.hierarchy.is_empty() && request.list_folders, "the next run reads the folder list afresh");
    take(&mut s, account, EwsFolderDeltas { changed: folders(), afresh: true, ..Default::default() });
    assert!(s.mailbox.folder_by_server(account, "pr").is_some(), "the folder the server kept is back");
}

#[test]
fn one_of_the_servers_own_deleted_here_before_the_upgrade_comes_back() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    // Deleted by an earlier version, whose deletion the server refused and the refusal dropped:
    // gone here, nothing queued, the folder list going on from its state.
    s.mailbox.folders.retain(|f| f.server.as_ref().is_none_or(|server| server.id != "co"));
    assert!(s.mailbox.queued.is_empty());
    assert_eq!(ews_mail_request(&s, account, false).unwrap().hierarchy, "HIER");
    // A delta that names it as the server's own, but neither brings it nor says it went.
    take(&mut s, account, empty_delta(&["in", "de", "co"]));
    let request = ews_mail_request(&s, account, false).unwrap();
    assert!(request.hierarchy.is_empty() && request.list_folders, "the folder list is read afresh");
    take(
        &mut s,
        account,
        EwsFolderDeltas { changed: folders(), afresh: true, well_known: vec!["in".into(), "de".into(), "co".into()], ..Default::default() },
    );
    assert!(marked(&s, account, "co"), "back, and marked");
    assert_eq!(ews_mail_request(&s, account, false).unwrap().hierarchy, "HIER", "and the list goes on from its state again");
}

#[test]
fn one_of_the_servers_own_that_a_delta_says_went_is_not_read_afresh() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    s.mailbox.folders.retain(|f| f.server.as_ref().is_none_or(|server| server.id != "co"));
    // (The server's Outbox while it is empty: named as its own, and among the deleted.)
    take(&mut s, account, EwsFolderDeltas { deleted: vec!["co".into()], well_known: vec!["in".into(), "co".into()], ..Default::default() });
    assert_eq!(ews_mail_request(&s, account, false).unwrap().hierarchy, "HIER");
    // Nor is a read afresh checked: the listing is what it brings.
    take(
        &mut s,
        account,
        EwsFolderDeltas { changed: folders(), afresh: true, well_known: vec!["in".into(), "elsewhere".into()], ..Default::default() },
    );
    assert_eq!(ews_mail_request(&s, account, false).unwrap().hierarchy, "HIER");
}

#[test]
fn a_hostile_list_of_the_servers_own_is_refused_or_marks_only_what_it_names() {
    let (mut s, account) = session();
    first_read(&mut s, account);
    let odd = EwsFolderDeltas { well_known: vec!["co".into(), "bad\nid".into()], ..Default::default() };
    assert!(apply_ews_mail(&mut s, EwsMailResult { folders: Some(odd), ..piece(account) }).is_err());
    assert!(!marked(&s, account, "co"), "nothing of a result refused is taken in");
    let mut own = vec!["co".to_owned()];
    own.extend((0..50).map(|n| format!("unknown-{n}")));
    take(&mut s, account, EwsFolderDeltas { well_known: own, ..Default::default() });
    assert!(marked(&s, account, "co") && !marked(&s, account, "pr"));
    assert!(
        s.mailbox.folders.iter().all(|f| f.server.as_ref().is_none_or(|server| !server.id.starts_with("unknown"))),
        "no folder is made of a name"
    );
}
