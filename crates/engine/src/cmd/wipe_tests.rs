// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Removing an account wipes its local copy (#177): the bodies and attachments only its mail
//! names leave the mailbox's content folder as soon as a state without the account is saved,
//! and content another account's mail names too stays.

use std::path::{Path, PathBuf};

use mailcraft_model::{Attachment, Body, FolderKind, Id, Mailbox, Message};
use serde_json::json;

use crate::Session;

/// A folder of this test's own in the system's temporary folder, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("mailcraft-wipe-engine-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn blobs(mailbox: &Path) -> usize {
    let dir = crate::store::blob_dir(mailbox);
    std::fs::read_dir(dir).into_iter().flatten().flatten().flat_map(|sub| std::fs::read_dir(sub.path()).into_iter().flatten().flatten()).count()
}

fn big(tag: &str) -> String {
    format!("{tag} ").repeat(2_000)
}

fn add(s: &mut Session, account: Id, body: &str, attachment: &[u8]) -> Id {
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let attachments =
        vec![Attachment { name: "a.bin".into(), mime: "application/octet-stream".into(), data: attachment.to_vec().into(), ..Default::default() }];
    s.mailbox
        .add_message(Message { account, folder: inbox, body: Body { text: body.into(), html: None }, attachments, ..Default::default() })
        .unwrap()
}

#[test]
fn removing_an_account_removes_its_content_files_and_keeps_what_another_account_names() {
    let tmp = Scratch::new("remove");
    let path = tmp.0.join("mailbox.mailcraft");
    let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
    let keep = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let going = s.run("account.add", &json!({"email": "grace@example.org"})).unwrap()["id"].as_u64().unwrap();
    let shared_attachment = vec![7u8; 4_000];
    add(&mut s, keep, &big("kept body"), &shared_attachment);
    add(&mut s, going, &big("going body"), &shared_attachment);
    add(&mut s, going, &big("going body two"), &[9u8; 5_000]);
    s.path = Some(path.clone());
    s.run("file.save", &json!({})).unwrap();
    // Two bodies and two attachments for the keeper, three for the leaver (one shared): 2 + 2 + 3.
    let before = blobs(&path);
    assert!(before >= 5, "{before}");

    s.run("account.remove", &json!({"id": going})).unwrap();
    // (The window forgets Undo with it: an Undo step would hold the mail.)
    s.forget_history();
    s.run("file.save", &json!({})).unwrap();

    let after = blobs(&path);
    assert!(after < before, "{after} of {before}: nothing went");
    // Only what the account that stays names: its body and the attachment both have.
    assert_eq!(after, 2, "{after}");
    // The mailbox that remains opens, with the shared attachment intact.
    let reopened = crate::store::open(&path).unwrap();
    assert_eq!(reopened.mailbox.messages.len(), 1);
    assert!(reopened.mailbox.accounts.iter().all(|a| a.id != going));
}

#[test]
fn a_shared_mailbox_closed_wipes_its_content_too() {
    let tmp = Scratch::new("shared");
    let path = tmp.0.join("mailbox.mailcraft");
    let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
    let owner = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    add(&mut s, shared, &big("team body"), &[3u8; 3_000]);
    s.path = Some(path.clone());
    s.run("file.save", &json!({})).unwrap();
    assert!(blobs(&path) >= 2);

    s.run("account.removeSharedMailbox", &json!({"id": shared})).unwrap();
    s.forget_history();
    s.run("file.save", &json!({})).unwrap();

    assert_eq!(blobs(&path), 0);
}
