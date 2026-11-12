// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A folder the server keeps although no well-known name tells it apart (RSS Feeds, #1069): when
//! the server refuses to move or delete it as one of its own, it is put back where the server has
//! it, the owner is told, and it is never offered for renaming, moving or deleting again. A folder
//! of the owner's still goes. The servers' answers are written out by hand, as `mailcraft-net`
//! gives them.

use serde_json::json;

use mailcraft_model::sync::FolderChangeDone;

use super::*;

const ACCOUNT_EMAIL: &str = "owner@example.org";

fn folder(id: &str, name: &str, kind: FolderKind, well_known: bool) -> GraphFolder {
    GraphFolder { id: id.into(), name: name.into(), kind, well_known, ..Default::default() }
}

/// The special folders, RSS Feeds (which no well-known name marks) and Projects (the owner's).
fn server() -> Vec<GraphFolder> {
    vec![
        folder("in", "Inbox", FolderKind::Inbox, true),
        folder("dr", "Drafts", FolderKind::Drafts, true),
        folder("se", "Sent Items", FolderKind::Sent, true),
        folder("de", "Deleted Items", FolderKind::Deleted, true),
        folder("ju", "Junk Email", FolderKind::Junk, true),
        folder("rs", "RSS Feeds", FolderKind::Custom, false),
        folder("pr", "Projects", FolderKind::Custom, false),
    ]
}

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ACCOUNT_EMAIL, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    take(&mut s, account, Vec::new());
    (s, account)
}

/// A run that answered `changes` and listed the server's folders as they are.
fn take(s: &mut Session, account: Id, changes: Vec<FolderChangeDone>) -> Value {
    let listing = FolderListing { folders: server(), complete: true };
    let result = GraphMailResult { account, email: ACCOUNT_EMAIL.into(), changes, listing: Some(listing), ..Default::default() };
    apply_graph_mail(s, result).unwrap()
}

fn own() -> ChangeOutcome {
    ChangeOutcome::OwnFolder { why: "the server keeps this folder and doesn't let it be moved or deleted".into() }
}

#[test]
fn a_folder_the_server_wont_let_go_of_is_put_back_said_and_never_offered_again() {
    let (mut s, account) = session();
    let rss = s.mailbox.folder_by_server(account, "rs").unwrap();
    s.view.folder = rss;
    // Not known as the server's yet: it can be deleted here, into Deleted Items.
    assert!(s.enabled("folder.delete"));
    s.run("folder.delete", &json!({"id": rss})).unwrap();
    assert_eq!(s.mailbox.folder(rss).unwrap().parent, s.mailbox.folder_of_kind(account, FolderKind::Deleted));
    let changes = graph_mail_request(&s, account).unwrap().changes;
    assert!(matches!(changes.as_slice(), [FolderChange::Move { id, .. }] if id == "rs"), "{changes:?}");

    // The server says it is one of its own.
    let refused = changes.into_iter().map(|change| FolderChangeDone { change, outcome: own() }).collect();
    let answer = take(&mut s, account, refused);
    let errors = answer["errors"].to_string();
    assert!(errors.contains("RSS Feeds") && errors.contains("server keeps this folder"), "{errors}");
    let f = s.mailbox.folder(rss).unwrap();
    assert_eq!((f.parent, f.name.as_str(), f.server_owned()), (None, "RSS Feeds", true));
    // Nothing more goes up for it, and it can't be renamed, moved or deleted again.
    assert_eq!(folder_changes(&s.mailbox, account, 10), (vec![], 0));
    s.view.folder = rss;
    for id in ["folder.rename", "folder.move", "folder.delete"] {
        assert!(!s.enabled(id), "{id}");
    }
    let said = s.run("folder.delete", &json!({"id": rss})).unwrap_err().to_string();
    assert!(said.contains("server keeps this folder"), "{said}");
    assert!(s.run("folder.rename", &json!({"id": rss, "name": "Feeds"})).is_err());
    assert_eq!(s.mailbox.folder(rss).map(|f| (f.name.as_str(), f.parent)), Some(("RSS Feeds", None)));
    // The mark outlasts the next listing, which doesn't say it.
    take(&mut s, account, Vec::new());
    assert!(s.mailbox.folder(rss).unwrap().server_owned());
}

#[test]
fn a_folder_numbered_on_its_way_into_deleted_items_gets_its_name_back() {
    let (mut s, account) = session();
    // A folder of the same name is in Deleted Items already: the one deleted is numbered there.
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.add_folder(account, Some(deleted), "RSS Feeds").unwrap();
    let rss = s.mailbox.folder_by_server(account, "rs").unwrap();
    s.run("folder.delete", &json!({"id": rss})).unwrap();
    assert_eq!(s.mailbox.folder(rss).unwrap().name, "RSS Feeds (2)");
    let move_only: Vec<FolderChangeDone> = graph_mail_request(&s, account)
        .unwrap()
        .changes
        .into_iter()
        .filter(|c| matches!(c, FolderChange::Move { id, .. } | FolderChange::Rename { id, .. } if id == "rs"))
        .map(|change| FolderChangeDone { change, outcome: own() })
        .collect();
    take(&mut s, account, move_only);
    let f = s.mailbox.folder(rss).unwrap();
    assert_eq!((f.parent, f.name.as_str()), (None, "RSS Feeds"));
}

#[test]
fn a_deletion_for_good_the_server_refuses_as_its_own_brings_it_back_marked() {
    let (mut s, account) = session();
    let rss = s.mailbox.folder_by_server(account, "rs").unwrap();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    // Already in Deleted Items here (an earlier version left it there): deleted for good.
    s.mailbox.folder_mut(rss).unwrap().parent = Some(deleted);
    s.run("folder.delete", &json!({"id": rss})).unwrap();
    assert!(s.mailbox.folder_by_server(account, "rs").is_none());
    let changes: Vec<FolderChangeDone> = graph_mail_request(&s, account)
        .unwrap()
        .changes
        .into_iter()
        .filter(|c| matches!(c, FolderChange::Delete { id, .. } if id == "rs"))
        .map(|change| FolderChangeDone { change, outcome: own() })
        .collect();
    assert_eq!(changes.len(), 1);
    let answer = take(&mut s, account, changes);
    assert!(answer["errors"].to_string().contains("Couldn't delete a folder"), "{answer}");
    // The server still has it where it was: back here, marked, and nothing waits to go.
    let back = s.mailbox.folder_by_server(account, "rs").unwrap();
    let f = s.mailbox.folder(back).unwrap();
    assert_eq!((f.parent, f.server_owned()), (None, true));
    assert!(s.mailbox.queued.is_empty());
    assert_eq!(folder_changes(&s.mailbox, account, 10), (vec![], 0));
}

#[test]
fn a_folder_of_the_owners_still_goes_and_an_ordinary_refusal_marks_nothing() {
    let (mut s, account) = session();
    let projects = s.mailbox.folder_by_server(account, "pr").unwrap();
    s.view.folder = projects;
    assert!(s.enabled("folder.delete"));
    s.run("folder.delete", &json!({"id": projects})).unwrap();
    let changes = graph_mail_request(&s, account).unwrap().changes;
    // The server says no for another reason (a 400): put back, and still the owner's.
    let refused = changes
        .into_iter()
        .map(|change| FolderChangeDone { change, outcome: ChangeOutcome::Refused { why: "the server says the change isn't one it can make".into() } })
        .collect();
    take(&mut s, account, refused);
    let f = s.mailbox.folder(projects).unwrap();
    assert_eq!((f.parent, f.server_owned()), (None, false));
    s.view.folder = projects;
    assert!(s.enabled("folder.delete"));
    s.run("folder.delete", &json!({"id": projects})).unwrap();
    assert_eq!(s.mailbox.folder(projects).unwrap().parent, s.mailbox.folder_of_kind(account, FolderKind::Deleted));
}

#[test]
fn a_bare_403_puts_the_folder_back_by_its_name_says_so_and_marks_nothing() {
    // A 403 without the server's code for its own folders may pass (a policy changed, a token
    // short of a scope): the folder goes back where the server has it, its name too, the owner
    // is told, nothing is sent again, and it can be deleted later.
    let (mut s, account) = session();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.add_folder(account, Some(deleted), "Projects").unwrap();
    let projects = s.mailbox.folder_by_server(account, "pr").unwrap();
    s.run("folder.delete", &json!({"id": projects})).unwrap();
    assert_eq!(s.mailbox.folder(projects).unwrap().name, "Projects (2)");
    let refused: Vec<FolderChangeDone> = graph_mail_request(&s, account)
        .unwrap()
        .changes
        .into_iter()
        .filter(|c| matches!(c, FolderChange::Move { id, .. } | FolderChange::Rename { id, .. } if id == "pr"))
        .map(|change| FolderChangeDone {
            change,
            outcome: ChangeOutcome::Refused { why: "the server says this account may not make that change".into() },
        })
        .collect();
    assert!(!refused.is_empty());
    let answer = take(&mut s, account, refused);
    let errors = answer["errors"].to_string();
    assert!(errors.contains("Couldn't move the folder \\\"Projects\\\"") && errors.contains("may not"), "{errors}");
    let f = s.mailbox.folder(projects).unwrap();
    assert_eq!((f.parent, f.name.as_str(), f.server_owned()), (None, "Projects", false));
    let again = folder_changes(&s.mailbox, account, 10).0;
    assert!(!again.iter().any(|c| matches!(c, FolderChange::Move { id, .. } | FolderChange::Rename { id, .. } if id == "pr")), "{again:?}");
    s.view.folder = projects;
    for id in ["folder.rename", "folder.move", "folder.delete"] {
        assert!(s.enabled(id), "{id}");
    }
    s.run("folder.delete", &json!({"id": projects})).unwrap();
    assert_eq!(s.mailbox.folder(projects).unwrap().parent, Some(deleted));
}
