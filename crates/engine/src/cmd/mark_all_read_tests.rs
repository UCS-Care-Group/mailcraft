// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1449: `mail.markAllRead` reads `folder` strictly, an id and a folder that is there: a value
//! that isn't one is refused rather than quietly marking the folder in view, and the answer names
//! the folder it marked.

use mailcraft_model::{FolderKind, Id, Message};
use serde_json::json;

use crate::{Session, sample};

const NOW: i64 = 1_791_993_600;

/// The sample mailbox, its Inbox in view, and one unread message in the Inbox and one in a folder
/// of the owner's own.
struct Setup {
    s: Session,
    inbox: Id,
    other: Id,
    here: Id,
    there: Id,
}

fn setup() -> Setup {
    let mut s = Session::at(sample::mailbox(NOW, 0), NOW, 0);
    let account = s.mailbox.default_account().unwrap().id;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.view.folder = inbox;
    let other = s.mailbox.add_folder(account, None, "Later").unwrap();
    let here = s.mailbox.add_message(Message { folder: inbox, account, subject: "Here".into(), ..Default::default() }).unwrap();
    let there = s.mailbox.add_message(Message { folder: other, account, subject: "There".into(), ..Default::default() }).unwrap();
    Setup { s, inbox, other, here, there }
}

#[test]
fn a_folder_that_isn_t_an_id_is_refused_and_nothing_is_marked() {
    for wrong in [json!("12"), json!(null), json!(0), json!(1.5)] {
        let mut t = setup();
        let refused = t.s.run("mail.markAllRead", &json!({"folder": wrong})).unwrap_err().to_string();
        assert!(refused.contains("`folder`"), "{wrong}: {refused}");
        assert!(!t.s.mailbox.message(t.here).unwrap().read && !t.s.mailbox.message(t.there).unwrap().read, "{wrong}: something was marked");
    }
}

#[test]
fn the_folder_named_is_the_one_marked() {
    let mut t = setup();
    let other = t.other;
    let answer = t.s.run("mail.markAllRead", &json!({"folder": other})).unwrap();
    assert_eq!(answer["folder"].as_u64(), Some(other), "the answer names the folder it marked: {answer}");
    assert!(t.s.mailbox.message(t.there).unwrap().read);
    assert!(!t.s.mailbox.message(t.here).unwrap().read, "another folder's mail was marked");
}

#[test]
fn no_folder_marks_the_folder_in_view() {
    let mut t = setup();
    let answer = t.s.run("mail.markAllRead", &json!({})).unwrap();
    assert_eq!(answer["folder"].as_u64(), Some(t.inbox));
    assert!(t.s.mailbox.message(t.here).unwrap().read);
    assert!(!t.s.mailbox.message(t.there).unwrap().read);
}

#[test]
fn a_folder_that_isn_t_there_is_refused() {
    let mut t = setup();
    let missing = t.s.mailbox.folders.iter().map(|f| f.id).max().unwrap_or(0).saturating_add(1_000);
    let refused = t.s.run("mail.markAllRead", &json!({"folder": missing})).unwrap_err().to_string();
    assert!(refused.contains("no folder"), "{refused}");
    assert!(!t.s.mailbox.message(t.here).unwrap().read);
}
