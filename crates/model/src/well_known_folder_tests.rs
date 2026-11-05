// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The server's own folders (Conversation History and the like, #756) can't be renamed, moved or
//! deleted, nor moved into when they have no kind here.

use serde_json::json;

use crate::*;

fn mailbox() -> (Mailbox, Id) {
    let mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let account = mb.accounts[0].id;
    (mb, account)
}

/// A folder with a server id, the server's own when `own`.
fn served(mb: &mut Mailbox, account: Id, name: &str, own: bool) -> Id {
    let id = mb.add_folder(account, None, name).unwrap();
    mb.folder_mut(id).unwrap().server = Some(ServerFolder { id: format!("srv-{name}"), name: name.into(), well_known: own, ..Default::default() });
    id
}

#[test]
fn a_well_known_folder_is_not_renamed_moved_or_deleted() {
    let (mut mb, account) = mailbox();
    let own = served(&mut mb, account, "Conversation History", true);
    let target = mb.add_folder(account, None, "Projects").unwrap();
    assert!(mb.rename_folder(own, "Mine").is_err());
    assert!(mb.move_folder(own, Some(target)).is_err());
    assert!(mb.delete_folder(own).is_err());
    let f = mb.folder(own).unwrap();
    assert_eq!((f.name.as_str(), f.parent), ("Conversation History", None));
    assert!(mb.folder(own).is_some());
}

#[test]
fn a_folder_the_owner_made_still_goes_the_ways_it_did() {
    let (mut mb, account) = mailbox();
    let mine = served(&mut mb, account, "Projects", false);
    let other = mb.add_folder(account, None, "Receipts").unwrap();
    mb.rename_folder(mine, "Projects 2026").unwrap();
    mb.move_folder(mine, Some(other)).unwrap();
    mb.delete_folder(mine).unwrap();
}

#[test]
fn nothing_moves_into_a_well_known_folder_without_a_kind_but_into_the_special_ones() {
    let (mut mb, account) = mailbox();
    let own = served(&mut mb, account, "Conversation History", true);
    let mine = mb.add_folder(account, None, "Projects").unwrap();
    assert!(mb.move_folder(mine, Some(own)).is_err());
    assert_eq!(mb.folder(mine).unwrap().parent, None);
    // The Inbox is the server's own too, and takes folders.
    let inbox = mb.inbox().unwrap();
    mb.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "in".into(), well_known: true, ..Default::default() });
    mb.move_folder(mine, Some(inbox)).unwrap();
    assert_eq!(mb.folder(mine).unwrap().parent, Some(inbox));
}

#[test]
fn a_folder_of_a_file_from_before_has_no_mark_and_a_bad_one_reads_as_none() {
    let old: ServerFolder = serde_json::from_value(json!({"id": "x", "name": "Y"})).unwrap();
    assert!(!old.well_known);
    let bad: ServerFolder = serde_json::from_value(json!({"id": "x", "wellKnown": "yes"})).unwrap();
    assert!(!bad.well_known);
    let marked: ServerFolder = serde_json::from_value(json!({"id": "x", "wellKnown": true})).unwrap();
    assert!(marked.well_known);
    // Written only when set, so a file from this version opens in the last one.
    assert!(serde_json::to_value(&old).unwrap().get("wellKnown").is_none());
    assert_eq!(serde_json::to_value(&marked).unwrap()["wellKnown"], true);
}
