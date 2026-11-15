// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1319: `folder.new` makes the folder in the account it is given, an account it reads strictly
//! and checks to be there, and a `parent` of another account's is refused rather than quietly
//! taking the folder in view's account.

use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use crate::{Session, sample};

const NOW: i64 = 1_791_993_600;

/// The sample mailbox and a second account of its own, with the view on the first one's Inbox.
fn setup() -> (Session, Id, Id, Id) {
    let mut s = Session::at(sample::mailbox(NOW, 0), NOW, 0);
    let first = s.mailbox.default_account().unwrap().id;
    let inbox = s.mailbox.folder_of_kind(first, FolderKind::Inbox).unwrap();
    s.view.folder = inbox;
    let second = s.run("account.add", &json!({"email": "noor@example.org"})).unwrap()["id"].as_u64().unwrap();
    let theirs = s.mailbox.folder_of_kind(second, FolderKind::Inbox).unwrap();
    (s, first, inbox, theirs)
}

fn made(s: &mut Session, params: Value) -> Value {
    s.run("folder.new", &params).unwrap_or_else(|e| panic!("{params}: {e}"))
}

#[test]
fn a_folder_is_made_in_the_account_named() {
    let (mut s, first, inbox, theirs) = setup();
    let answer = made(&mut s, json!({"name": "Archive 2026", "account": theirs}));
    let id = answer["id"].as_u64().unwrap();
    assert_eq!(answer["account"].as_u64(), Some(theirs), "the answer names the account it was made in: {answer}");
    assert_eq!(s.mailbox.folder(id).unwrap().account, theirs, "not the account in view's");
    assert_eq!(s.mailbox.folder(id).unwrap().parent, None, "at that account's top level, not under the folder in view");
    assert_eq!(s.view.folder, inbox, "and the window stays where it was");
    assert_ne!(s.mailbox.folder(id).unwrap().account, first);
}

#[test]
fn the_folder_in_view_is_below_when_it_is_that_account_s() {
    let (mut s, _, inbox, _) = setup();
    let id = made(&mut s, json!({"name": "Projects", "account": s.mailbox.folder(inbox).unwrap().account}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.folder(id).unwrap().parent, Some(inbox), "under the folder in view of that account");

    let (mut s, _, _, theirs) = setup();
    let id = made(&mut s, json!({"name": "Projects", "account": theirs}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.folder(id).unwrap().parent, Some(theirs), "under the folder in view of that account");
}

#[test]
fn a_parent_of_another_account_s_is_refused() {
    let (mut s, _, inbox, theirs) = setup();
    let refuse = s.run("folder.new", &json!({"name": "Mine", "parent": inbox, "account": theirs})).unwrap_err().to_string();
    assert!(refuse.contains("is not that account's"), "{refuse}");
    assert!(s.mailbox.folders.iter().all(|f| f.name != "Mine"), "nothing was made");

    // With no account, the parent decides the account, as it always did.
    let id = made(&mut s, json!({"name": "Mine", "parent": inbox}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.folder(id).unwrap().account, s.mailbox.folder(inbox).unwrap().account);
}

#[test]
fn an_account_that_isn_t_an_id_is_refused() {
    let (mut s, _, _, theirs) = setup();
    for wrong in [json!("2"), json!(0), json!(1.5), json!(null)] {
        let refuse = s.run("folder.new", &json!({"name": "Wrong", "account": wrong, "parent": null})).unwrap_err().to_string();
        assert!(refuse.contains("`account`") || refuse.contains("account with id"), "{wrong}: {refuse}");
        assert!(s.mailbox.folders.iter().all(|f| f.name != "Wrong"), "{wrong}: nothing was made");
    }
    let refuse = s.run("folder.new", &json!({"name": "Wrong", "account": theirs + 1_000, "parent": null})).unwrap_err().to_string();
    assert!(refuse.contains("no account with id"), "{refuse}");
    assert!(s.mailbox.folders.iter().all(|f| f.name != "Wrong"), "nothing was made");
}
