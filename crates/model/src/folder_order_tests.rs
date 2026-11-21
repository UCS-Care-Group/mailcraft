// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The order the owner puts an account's folders in (#1514): among the folders that share a
//! parent, kept on this computer, with Inbox, Drafts, Sent Items and Deleted Items leading.

use crate::*;

fn listed(mb: &Mailbox, account: Id, parent: Option<Id>) -> Vec<String> {
    let mut v: Vec<&Folder> = mb.folders.iter().filter(|f| f.account == account && f.parent == parent).collect();
    sort_as_listed(&mut v, parent.is_none());
    v.into_iter().map(|f| f.name.clone()).collect()
}

/// An account with Projects and Clients at its top level, and A and B under Projects.
fn mailbox() -> (Mailbox, Id, [Id; 4]) {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "a@example.org");
    let account = mb.accounts[0].id;
    let projects = mb.add_folder(account, None, "Projects").unwrap();
    let clients = mb.add_folder(account, None, "Clients").unwrap();
    let a = mb.add_folder(account, Some(projects), "A").unwrap();
    let b = mb.add_folder(account, Some(projects), "B").unwrap();
    (mb, account, [projects, clients, a, b])
}

const LEAD: [&str; 4] = ["Inbox", "Drafts", "Sent Items", "Deleted Items"];

/// Until the owner moves one the pane is as it always was, and nothing is written of an order.
#[test]
fn folders_are_listed_by_name_until_one_is_moved() {
    let (mb, account, _) = mailbox();
    let mut want: Vec<&str> = LEAD.to_vec();
    want.extend(["Archive", "Clients", "Junk Email", "Outbox", "Projects"]);
    assert_eq!(listed(&mb, account, None), want);
    let file = serde_json::to_string(&mb).unwrap();
    assert!(!file.contains("\"place\"") && !file.contains("favoritePlace"));
    // A file from before (no places) reads the same.
    let read: Mailbox = serde_json::from_str(&file).unwrap();
    assert_eq!(listed(&read, account, None), want);
}

/// Projects goes before Archive, then last; the four that lead stay; a subfolder moves among its
/// siblings only; the order survives the file.
#[test]
fn a_folder_moved_stays_where_it_was_put() {
    let (mut mb, account, [projects, clients, a, b]) = mailbox();
    let archive = mb.folder_of_kind(account, FolderKind::Archive).unwrap();
    assert_eq!(mb.reorder_folder(projects, Some(archive)), Ok(true));
    let mut want: Vec<&str> = LEAD.to_vec();
    want.extend(["Projects", "Archive", "Clients", "Junk Email", "Outbox"]);
    assert_eq!(listed(&mb, account, None), want);
    // `children` follows it too, after the standard folders it ranks.
    let custom: Vec<&str> = mb.children(account, None).iter().filter(|f| f.kind == FolderKind::Custom).map(|f| f.name.as_str()).collect();
    assert_eq!(custom, ["Projects", "Clients"]);
    assert_eq!(mb.reorder_folder(projects, None), Ok(true));
    assert_eq!(listed(&mb, account, None).last().map(String::as_str), Some("Projects"));
    assert_eq!(mb.reorder_folder(projects, None), Ok(false), "last already");
    assert_eq!(mb.reorder_folder(b, Some(a)), Ok(true));
    assert_eq!(listed(&mb, account, Some(projects)), ["B", "A"]);
    // A new folder comes after those placed.
    mb.add_folder(account, Some(projects), "0 first by name").unwrap();
    assert_eq!(listed(&mb, account, Some(projects)), ["B", "A", "0 first by name"]);
    let read: Mailbox = serde_json::from_str(&serde_json::to_string(&mb).unwrap()).unwrap();
    assert_eq!(listed(&read, account, Some(projects)), ["B", "A", "0 first by name"]);
    // Moved under another parent, it goes last there.
    mb.move_folder(clients, Some(projects)).unwrap();
    assert_eq!(listed(&mb, account, Some(projects)), ["B", "A", "0 first by name", "Clients"]);
}

/// What can't be moved is refused, and a refusal changes nothing.
#[test]
fn what_cant_be_moved_is_refused() {
    let (mut mb, account, [projects, clients, a, _]) = mailbox();
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let was = mb.folders.clone();
    for (id, before) in [
        (inbox, None),
        (projects, Some(inbox)),
        (projects, Some(projects)),
        (projects, Some(a)),
        (a, Some(clients)),
        (999_999, None),
        (projects, Some(999_999)),
    ] {
        assert!(mb.reorder_folder(id, before).is_err(), "{id} before {before:?}");
    }
    assert_eq!(mb.folders, was);
    // A subfolder of the Inbox doesn't lead: it moves.
    let sub = mb.add_folder(account, Some(inbox), "Sub").unwrap();
    let other = mb.add_folder(account, Some(inbox), "Other").unwrap();
    assert_eq!(mb.reorder_folder(sub, Some(other)), Ok(true));
}

/// A place that isn't a number in a file is passed over, as a file from before.
#[test]
fn a_bad_place_in_a_file_is_passed_over() {
    let (mb, account, [projects, ..]) = mailbox();
    let mut file: serde_json::Value = serde_json::to_value(&mb).unwrap();
    for f in file["folders"].as_array_mut().unwrap() {
        f["place"] = serde_json::json!("first");
        f["favoritePlace"] = serde_json::json!(-1);
    }
    let read: Mailbox = serde_json::from_value(file).unwrap();
    assert!(read.folders.iter().all(|f| f.place.is_none() && f.favorite_place.is_none()));
    assert_eq!(listed(&read, account, Some(projects)), ["A", "B"]);
}

/// A folder deleted into Deleted Items goes last there, whatever place it had, and is no longer
/// a favourite in any place; `children` lists as the pane does.
#[test]
fn a_deleted_folder_goes_last_and_leaves_the_favorites() {
    let (mut mb, account, [projects, clients, ..]) = mailbox();
    assert_eq!(mb.reorder_folder(clients, Some(projects)), Ok(true));
    assert!(mb.folder(clients).unwrap().place.is_some());
    mb.folder_mut(clients).unwrap().favorite = true;
    mb.folder_mut(clients).unwrap().favorite_place = Some(0);
    let deleted = mb.folder_of_kind(account, FolderKind::Deleted).unwrap();
    mb.add_folder(account, Some(deleted), "Aaa").unwrap();
    assert_eq!(mb.delete_folder(clients), Ok(false));
    let f = mb.folder(clients).unwrap();
    assert_eq!((f.parent, f.place, f.favorite, f.favorite_place), (Some(deleted), None, false, None));
    assert_eq!(listed(&mb, account, Some(deleted)), ["Aaa", "Clients"]);
    // A standard folder moved by the owner shows its place in `children` as in the pane.
    let archive = mb.folder_of_kind(account, FolderKind::Archive).unwrap();
    assert_eq!(mb.reorder_folder(archive, None), Ok(true));
    let names: Vec<&str> = mb.children(account, None).iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names.last(), Some(&"Archive"));
}
