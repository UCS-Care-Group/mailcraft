// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The server's own folders of a Microsoft account (Conversation History and the like, #756): never
//! renamed, moved or deleted, whatever an earlier version left behind.

use serde_json::json;

use super::*;

const ACCOUNT_EMAIL: &str = "owner@example.org";

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": ACCOUNT_EMAIL, "kind": "microsoft"})).unwrap();
    let account = added["id"].as_u64().unwrap();
    (s, account)
}

fn folder(id: &str, name: &str, kind: FolderKind, well_known: bool) -> GraphFolder {
    GraphFolder { id: id.into(), name: name.into(), kind, well_known, ..Default::default() }
}

/// The special folders, Conversation History (the server's own) and Projects (the owner's).
fn server(own: bool) -> Vec<GraphFolder> {
    vec![
        folder("in", "Inbox", FolderKind::Inbox, true),
        folder("dr", "Drafts", FolderKind::Drafts, true),
        folder("se", "Sent Items", FolderKind::Sent, true),
        folder("de", "Deleted Items", FolderKind::Deleted, true),
        folder("ju", "Junk Email", FolderKind::Junk, true),
        folder("ou", "Outbox", FolderKind::Outbox, true),
        folder("co", "Conversation History", FolderKind::Custom, own),
        folder("pr", "Projects", FolderKind::Custom, false),
    ]
}

fn take(s: &mut Session, account: Id, folders: Vec<GraphFolder>) {
    let listing = FolderListing { folders, complete: true };
    let result = GraphMailResult { account, email: ACCOUNT_EMAIL.into(), listing: Some(listing), ..Default::default() };
    apply_graph_mail(s, result).unwrap();
}

fn flagged(s: &Session, account: Id, id: &str) -> bool {
    s.mailbox.folder(s.mailbox.folder_by_server(account, id).unwrap()).unwrap().server_owned()
}

#[test]
fn the_listing_marks_the_servers_own_folders_and_not_the_owners() {
    let (mut s, account) = session();
    take(&mut s, account, server(true));
    assert!(flagged(&s, account, "co") && flagged(&s, account, "in"));
    assert!(!flagged(&s, account, "pr"));
}

#[test]
fn a_folder_an_earlier_version_moved_is_put_back_and_no_move_follows() {
    let (mut s, account) = session();
    // Before the update the server's own folder wasn't known as one.
    take(&mut s, account, server(false));
    let co = s.mailbox.folder_by_server(account, "co").unwrap();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted);
    s.mailbox.folder_mut(co).unwrap().parent = deleted;
    s.mailbox.folder_mut(co).unwrap().name = "Mine".into();
    assert_eq!(folder_changes(&s.mailbox, account, 10).1, 2, "the old version would send a rename and a move");

    take(&mut s, account, server(true));
    let f = s.mailbox.folder(co).unwrap();
    assert_eq!((f.parent, f.name.as_str()), (None, "Conversation History"));
    assert_eq!(folder_changes(&s.mailbox, account, 10), (vec![], 0));
}

#[test]
fn nothing_is_sent_for_a_flagged_folder_even_before_the_listing_puts_it_back() {
    let (mut s, account) = session();
    take(&mut s, account, server(true));
    let co = s.mailbox.folder_by_server(account, "co").unwrap();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted);
    s.mailbox.folder_mut(co).unwrap().parent = deleted;
    s.mailbox.folder_mut(co).unwrap().name = "Mine".into();
    assert_eq!(folder_changes(&s.mailbox, account, 10), (vec![], 0));
    assert_eq!(unpushed(&s, account), 0);
}

#[test]
fn one_odd_answer_does_not_clear_the_mark() {
    let (mut s, account) = session();
    take(&mut s, account, server(true));
    take(&mut s, account, server(false));
    assert!(flagged(&s, account, "co"));
}

#[test]
fn a_deletion_queued_for_a_folder_marked_earlier_is_dropped_though_one_listing_says_false() {
    let (mut s, account) = session();
    take(&mut s, account, server(true));
    s.mailbox.queued.push(Queued::DeleteFolder { account, id: "co".into(), shown: Default::default() });
    take(&mut s, account, server(false));
    assert!(flagged(&s, account, "co"));
    assert!(s.mailbox.queued.is_empty());
    assert!(!folder_changes(&s.mailbox, account, 10).0.iter().any(|c| matches!(c, FolderChange::Delete { .. })));
}

#[test]
fn a_deletion_queued_for_a_folder_that_is_the_servers_own_is_dropped() {
    let (mut s, account) = session();
    take(&mut s, account, server(false));
    s.mailbox.queued.push(Queued::DeleteFolder { account, id: "co".into(), shown: Default::default() });
    s.mailbox.queued.push(Queued::DeleteFolder { account, id: "pr".into(), shown: Default::default() });
    take(&mut s, account, server(true));
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
}

#[test]
fn rename_move_and_delete_are_greyed_out_for_the_servers_own_folder() {
    let (mut s, account) = session();
    take(&mut s, account, server(true));
    s.view.folder = s.mailbox.folder_by_server(account, "co").unwrap();
    for id in ["folder.rename", "folder.move", "folder.delete"] {
        assert!(!s.enabled(id), "{id}");
    }
    s.view.folder = s.mailbox.folder_by_server(account, "pr").unwrap();
    for id in ["folder.rename", "folder.move", "folder.delete"] {
        assert!(s.enabled(id), "{id}");
    }
}

#[test]
fn no_new_folder_is_made_in_the_servers_own_folder() {
    let (mut s, account) = session();
    take(&mut s, account, server(true));
    let co = s.mailbox.folder_by_server(account, "co").unwrap();
    assert!(s.run("folder.new", &json!({"name": "Inside", "parent": co})).is_err());
    assert!(s.mailbox.children(account, Some(co)).is_empty());
    // The special folders, which are the server's own too, take new folders.
    let inbox = s.mailbox.folder_by_server(account, "in").unwrap();
    s.run("folder.new", &json!({"name": "Inside", "parent": inbox})).unwrap();
}

#[test]
fn emptying_deleted_items_leaves_a_servers_own_folder_in_it_and_takes_the_rest() {
    let (mut s, account) = session();
    take(&mut s, account, server(false));
    // An earlier version moved Conversation History into Deleted Items before it was known.
    let co = s.mailbox.folder_by_server(account, "co").unwrap();
    let pr = s.mailbox.folder_by_server(account, "pr").unwrap();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.folder_mut(co).unwrap().parent = Some(deleted);
    s.mailbox.folder_mut(pr).unwrap().parent = Some(deleted);
    s.mailbox.folder_mut(co).unwrap().server.as_mut().unwrap().well_known = true;
    s.run("folder.empty", &json!({"id": deleted})).unwrap();
    assert!(s.mailbox.folder(co).is_some(), "the server's own stays");
    assert!(s.mailbox.folder(pr).is_none(), "the owner's goes");
}

#[test]
fn an_owners_folder_called_conversation_history_beside_the_real_one_is_still_the_owners() {
    let (mut s, account) = session();
    let mut folders = server(true);
    folders.push(GraphFolder { parent: "in".into(), ..folder("mine", "Conversation History", FolderKind::Custom, false) });
    take(&mut s, account, folders);
    assert!(flagged(&s, account, "co"));
    assert!(!flagged(&s, account, "mine"));
    let mine = s.mailbox.folder_by_server(account, "mine").unwrap();
    let co = s.mailbox.folder_by_server(account, "co").unwrap();
    s.view.folder = mine;
    assert!(s.enabled("folder.rename") && s.enabled("folder.delete"));
    s.run("folder.rename", &json!({"id": mine, "name": "Old chats"})).unwrap();
    let pr = s.mailbox.folder_by_server(account, "pr").unwrap();
    s.run("folder.move", &json!({"id": mine, "parent": pr})).unwrap();
    s.run("folder.delete", &json!({"id": mine})).unwrap();
    // The server's own is left as it is.
    assert!(s.run("folder.rename", &json!({"id": co, "name": "Old chats"})).is_err());
    assert!(s.run("folder.delete", &json!({"id": co})).is_err());
    assert_eq!(s.mailbox.folder(co).map(|f| (f.name.as_str(), f.parent)), Some(("Conversation History", None)));
}
