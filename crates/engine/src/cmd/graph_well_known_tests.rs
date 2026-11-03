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
    s.mailbox.queued.push(Queued::DeleteFolder { account, id: "co".into() });
    take(&mut s, account, server(false));
    assert!(flagged(&s, account, "co"));
    assert!(s.mailbox.queued.is_empty());
    assert!(!folder_changes(&s.mailbox, account, 10).0.iter().any(|c| matches!(c, FolderChange::Delete { .. })));
}

#[test]
fn a_deletion_queued_for_a_folder_that_is_the_servers_own_is_dropped() {
    let (mut s, account) = session();
    take(&mut s, account, server(false));
    s.mailbox.queued.push(Queued::DeleteFolder { account, id: "co".into() });
    s.mailbox.queued.push(Queued::DeleteFolder { account, id: "pr".into() });
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
