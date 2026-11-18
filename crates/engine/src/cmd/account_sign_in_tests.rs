// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1556: an account that has to be signed in again. One mark for every kind of account
//! (`Account::sign_in_needed`), noted by the host's side of a run, kept in the mailbox file,
//! said by `account.list`, and left alone by Undo.

use std::path::PathBuf;

use mailcraft_model::Id;
use serde_json::{Value, json};

use super::account::note_sign_in_needed;
use crate::Session;

fn add(s: &mut Session, params: Value) -> Id {
    s.run("account.add", &params).unwrap()["id"].as_u64().unwrap()
}

fn listed(s: &mut Session, id: Id) -> Value {
    let list = s.run("account.list", &json!({})).unwrap();
    list["accounts"].as_array().unwrap().iter().find(|a| a["id"] == json!(id)).unwrap()["signInNeeded"].clone()
}

fn needed(s: &Session, id: Id) -> bool {
    s.mailbox.account(id).unwrap().sign_in_needed
}

/// Every kind of account with a server takes the one mark; a change is said once, and a mark
/// that is there already changes nothing (a round by itself turned down again).
#[test]
fn every_kind_of_account_with_a_server_takes_the_one_mark() {
    let mut s = Session::empty();
    let imap = add(&mut s, json!({"email": "imap@example.org", "incoming": {"host": "imap.example.org"}, "outgoing": {"host": "smtp.example.org"}}));
    let google = add(
        &mut s,
        json!({"email": "ada@example.org", "auth": "oauth2", "provider": "google", "incoming": {"host": "imap.gmail.com"}, "outgoing": {"host": "smtp.gmail.com"}}),
    );
    let microsoft = add(&mut s, json!({"email": "bo@example.org", "kind": "microsoft"}));
    let exchange = add(&mut s, json!({"email": "cy@example.org", "kind": "exchange", "ews": {"url": "https://mail.example.org/EWS/Exchange.asmx"}}));
    let undo = s.undo_label();
    for id in [imap, google, microsoft, exchange] {
        assert_eq!(listed(&mut s, id), json!(false));
        s.dirty = false;
        assert!(note_sign_in_needed(&mut s, id, true));
        assert!(needed(&s, id) && s.dirty, "the file is to keep it");
        assert_eq!(listed(&mut s, id), json!(true));
        s.dirty = false;
        assert!(!note_sign_in_needed(&mut s, id, true));
        assert!(!s.dirty);
    }
    assert_eq!(s.mailbox.needing_sign_in().iter().map(|a| a.id).collect::<Vec<_>>(), [imap, google, microsoft, exchange]);
    // A sign-in that went through takes it away, and only that account's.
    assert!(note_sign_in_needed(&mut s, google, false));
    assert!(!needed(&s, google) && needed(&s, imap));
    assert!(!note_sign_in_needed(&mut s, google, false));
    // It is no step to undo.
    assert_eq!(s.undo_label(), undo);
}

/// An account on this computer alone has no sign-in to need; a shared mailbox's is its owner's,
/// whose sign-in reads it; an account that isn't there is nothing.
#[test]
fn a_shared_mailbox_marks_its_owner_and_a_local_account_never_is() {
    let mut s = Session::empty();
    let owner = add(&mut s, json!({"email": "noor@example.org", "kind": "microsoft"}));
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert!(note_sign_in_needed(&mut s, shared, true));
    assert!(needed(&s, owner) && !needed(&s, shared));
    assert!(note_sign_in_needed(&mut s, shared, false));
    assert!(!needed(&s, owner));
    let local = add(&mut s, json!({"email": "here@example.org"}));
    assert!(!note_sign_in_needed(&mut s, local, true));
    assert!(!needed(&s, local));
    assert!(!note_sign_in_needed(&mut s, 999_999, true));
}

/// Undo and Redo of what the owner did leave the mark as it is now: it is the server's word.
#[test]
fn undo_and_redo_leave_the_mark_as_it_is_now() {
    let mut s = Session::empty();
    let id = add(&mut s, json!({"email": "imap@example.org", "incoming": {"host": "imap.example.org"}}));
    let was = s.mailbox.account(id).unwrap().name.clone();
    s.run("account.edit", &json!({"id": id, "name": "Work"})).unwrap();
    note_sign_in_needed(&mut s, id, true);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().name, was);
    assert!(needed(&s, id));
    note_sign_in_needed(&mut s, id, false);
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().name, "Work");
    assert!(!needed(&s, id));
}

/// An account that an Undo took away and a Redo brings back keeps the mark it had when it went:
/// nothing signed it in meanwhile.
#[test]
fn an_account_brought_back_keeps_its_mark() {
    let mut s = Session::empty();
    let id = add(&mut s, json!({"email": "imap@example.org", "incoming": {"host": "imap.example.org"}}));
    note_sign_in_needed(&mut s, id, true);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.account(id).is_none());
    s.run("edit.redo", &json!({})).unwrap();
    assert!(needed(&s, id));
    assert_eq!(listed(&mut s, id), json!(true));
}

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The mark is kept in the file: a restart still says it, and a file from before there was one
/// opens with none; a clear one isn't written.
#[test]
fn the_mark_is_kept_in_the_mailbox_file() {
    let dir = Temp(std::env::temp_dir().join(format!("mailcraft-1556-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("mailbox.mailcraft");
    let mut s = Session::empty();
    let (a, b) = (
        add(&mut s, json!({"email": "a@example.org", "incoming": {"host": "imap.example.org"}})),
        add(&mut s, json!({"email": "b@example.org", "incoming": {"host": "imap.example.org"}})),
    );
    note_sign_in_needed(&mut s, b, true);
    crate::store::save(&path, &s.mailbox).unwrap();
    let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
    assert_eq!(text.matches(r#""signInNeeded":true"#).count(), 1, "{text}");
    let opened = crate::store::open(&path).unwrap().mailbox;
    assert!(!opened.account(a).unwrap().sign_in_needed && opened.account(b).unwrap().sign_in_needed);
    // Cleared, it opens cleared.
    note_sign_in_needed(&mut s, b, false);
    crate::store::save(&path, &s.mailbox).unwrap();
    assert!(crate::store::open(&path).unwrap().mailbox.needing_sign_in().is_empty());
    // A file from before there was one says nothing, which is none.
    let before: mailcraft_model::Account = serde_json::from_str(r#"{"id": 7, "email": "c@example.org", "kind": "imap"}"#).unwrap();
    assert!(!before.sign_in_needed);
    assert!(!serde_json::to_string(&before).unwrap().contains("signInNeeded"));
}
