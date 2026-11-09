// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #482: a shared mailbox opened through an Exchange server account: what it is, which server and
//! sign-in it uses (its owner's, as they are now), what a run names, and that its folders and mail
//! are its own.

use mailcraft_model::sync::{EwsFolderDeltas, EwsMailResult, EwsMessagePage, GraphFolder, GraphMessage, ServerMessage};
use mailcraft_model::{AccountKind, Address, FolderKind, Id};
use serde_json::json;

use crate::Session;
use crate::cmd::ews_mail::{apply_ews_mail, ews_fetch_request, ews_mail_request};

const OWNER: &str = "ada@example.org";
const SHARED: &str = "support@example.org";
const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "exchange", "ews": {"url": URL, "username": "CORP\\ada"}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    (s, owner)
}

fn add(s: &mut Session, owner: Id, address: &str) -> Id {
    s.run("account.addSharedMailbox", &json!({"account": owner, "address": address})).unwrap()["id"].as_u64().unwrap()
}

fn folder(id: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() }
}

#[test]
fn a_shared_mailbox_opens_under_an_exchange_server_account_with_its_owners_server_and_sign_in() {
    let (mut s, owner) = session();
    let id = add(&mut s, owner, SHARED);
    let a = s.mailbox.account(id).unwrap();
    assert_eq!((a.kind, a.shared_of, a.email.as_str()), (AccountKind::Exchange, Some(owner), SHARED));
    let server = a.ews.as_ref().expect("a server of its own to keep its folder list's state in");
    assert_eq!((server.url.as_str(), server.username.as_str()), (URL, "CORP\\ada"), "the owner's server, signed in as the owner");
    assert!(server.hierarchy.is_empty());
    assert_eq!(s.mailbox.ews_owner_of(id).map(|o| o.id), Some(owner));
    assert!(s.mailbox.owner_of(id).is_none(), "no Microsoft account reads it");
    // It has the standard folders to begin with, as any account has.
    assert!(s.mailbox.folder_of_kind(id, FolderKind::Inbox).is_some());
}

#[test]
fn an_owner_that_signs_in_with_its_address_signs_the_shared_mailbox_in_with_that_address_not_its_own() {
    let mut s = Session::empty();
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "exchange", "ews": {"url": URL}})).unwrap()["id"].as_u64().unwrap();
    let id = add(&mut s, owner, SHARED);
    assert_eq!(s.mailbox.account(id).unwrap().ews.as_ref().map(|e| e.username.as_str()), Some(OWNER));
    let request = ews_mail_request(&s, id, false).unwrap();
    assert_eq!((request.username.as_str(), request.email.as_str(), request.mailbox.as_str()), (OWNER, SHARED, SHARED));
}

#[test]
fn what_cannot_open_a_shared_mailbox_is_refused() {
    let (mut s, owner) = session();
    let id = add(&mut s, owner, SHARED);
    let refuse = |s: &mut Session, params: serde_json::Value| s.run("account.addSharedMailbox", &params).unwrap_err().to_string();
    assert!(refuse(&mut s, json!({"account": owner, "address": SHARED})).contains("already here"));
    assert!(refuse(&mut s, json!({"account": owner, "address": OWNER})).contains("own address"));
    assert!(refuse(&mut s, json!({"account": id, "address": "other@example.org"})).contains("shared mailbox can't open another"));
    assert!(refuse(&mut s, json!({"account": owner, "address": "not an address"})).contains("isn't an e-mail address"));
    let local = s.mailbox.local_account();
    assert!(refuse(&mut s, json!({"account": local, "address": "other@example.org"})).contains("Microsoft Graph or is an Exchange server account"));
    assert_eq!(s.mailbox.shared_mailboxes(owner).len(), 1);
}

#[test]
fn an_exchange_account_whose_server_cannot_be_used_opens_nothing() {
    let (mut s, owner) = session();
    s.mailbox.account_mut(owner).unwrap().ews.as_mut().unwrap().url = "http://exchange.example.org/EWS".into();
    let why = s.run("account.addSharedMailbox", &json!({"account": owner, "address": SHARED})).unwrap_err().to_string();
    assert!(why.contains("server address that can be used"), "{why}");
}

#[test]
fn a_run_for_the_shared_mailbox_names_it_and_uses_the_owners_server_as_it_is_now() {
    let (mut s, owner) = session();
    let id = add(&mut s, owner, SHARED);
    // The owner trusts a certificate after the shared mailbox was opened: the shared mailbox's
    // runs trust it too, and the copy it was made with can't have drifted.
    s.mailbox.account_mut(owner).unwrap().ews.as_mut().unwrap().trusted = "ab".repeat(32);
    let request = ews_mail_request(&s, id, false).unwrap();
    assert_eq!((request.mailbox.as_str(), request.email.as_str(), request.url.as_str()), (SHARED, SHARED, URL));
    assert_eq!((request.username.as_str(), request.trusted.len()), ("CORP\\ada", 64));
    // The owner's own runs name no mailbox.
    assert!(ews_mail_request(&s, owner, false).unwrap().mailbox.is_empty());
    // A message fetched whole is fetched at the owner's server, signed in as the owner.
    let fetch = ews_fetch_request(&s, id, None, 10).unwrap();
    assert_eq!((fetch.url.as_str(), fetch.username.as_str(), fetch.trusted.len()), (URL, "CORP\\ada", 64));
}

fn result(account: Id, email: &str, folders: EwsFolderDeltas) -> EwsMailResult {
    EwsMailResult {
        account,
        email: email.into(),
        url: URL.into(),
        folders: Some(folders),
        hierarchy: "H1".into(),
        partial: true,
        ..Default::default()
    }
}

#[test]
fn the_shared_mailboxes_folders_and_mail_are_its_own_and_the_owners_are_not_touched() {
    let (mut s, owner) = session();
    let id = add(&mut s, owner, SHARED);
    // The same server ids in both mailboxes (a server's ids are its mailbox's): two accounts, two sets.
    let list = vec![folder("F1", "Inbox", FolderKind::Inbox), folder("F5", "Projects", FolderKind::Custom)];
    apply_ews_mail(&mut s, result(owner, OWNER, EwsFolderDeltas { changed: list.clone(), afresh: true, ..Default::default() })).unwrap();
    apply_ews_mail(&mut s, result(id, SHARED, EwsFolderDeltas { changed: list, afresh: true, ..Default::default() })).unwrap();
    let page = EwsMessagePage {
        folder: "F1".into(),
        messages: vec![GraphMessage {
            server: ServerMessage { id: "M1".into(), folder: "F1".into(), change_key: "CK".into(), ..Default::default() },
            message_id: "m1@example.org".into(),
            subject: "For the team".into(),
            from: Address::new("Zoe", "zoe@example.org"),
            received: 1_000,
            ..Default::default()
        }],
        afresh: true,
        last: true,
        ..Default::default()
    };
    apply_ews_mail(&mut s, EwsMailResult { pages: vec![page], ..result(id, SHARED, EwsFolderDeltas::default()) }).unwrap();
    let mine = s.mailbox.folder_by_server(owner, "F1").unwrap();
    let theirs = s.mailbox.folder_by_server(id, "F1").unwrap();
    assert_ne!(mine, theirs);
    assert_eq!(s.mailbox.count(theirs), 1);
    assert_eq!(s.mailbox.count(mine), 0, "the owner's Inbox did not get the shared mailbox's mail");
    assert_eq!(s.mailbox.account(id).unwrap().ews.as_ref().map(|e| e.hierarchy.as_str()), Some("H1"));
    assert!(s.mailbox.account(owner).unwrap().ews.as_ref().is_some_and(|e| e.hierarchy == "H1"), "each has its own state");
}

#[test]
fn a_result_for_another_mailbox_or_another_server_is_not_taken_in_by_the_shared_one() {
    let (mut s, owner) = session();
    let id = add(&mut s, owner, SHARED);
    let wrong_mailbox = result(id, OWNER, EwsFolderDeltas::default());
    assert!(apply_ews_mail(&mut s, wrong_mailbox).is_err());
    let wrong_server = EwsMailResult { url: "https://other.example.org/EWS/Exchange.asmx".into(), ..result(id, SHARED, EwsFolderDeltas::default()) };
    assert!(apply_ews_mail(&mut s, wrong_server).is_err());
}

#[test]
fn a_shared_mailbox_closes_with_what_is_kept_of_it_and_goes_with_its_owner() {
    let (mut s, owner) = session();
    let first = add(&mut s, owner, SHARED);
    let second = add(&mut s, owner, "sales@example.org");
    s.run("account.removeSharedMailbox", &json!({"address": SHARED})).unwrap();
    assert!(s.mailbox.account(first).is_none() && s.mailbox.account(second).is_some());
    assert!(s.mailbox.account(owner).is_some());
    s.run("account.remove", &json!({"id": owner})).unwrap();
    assert!(s.mailbox.account(second).is_none(), "a shared mailbox can't outlive the account that reads it");
}

#[test]
fn a_shared_mailbox_survives_a_save_and_a_reopen() {
    let (mut s, owner) = session();
    let id = add(&mut s, owner, SHARED);
    let json = serde_json::to_string(&s.mailbox.account(id).unwrap()).unwrap();
    let back: mailcraft_model::Account = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, s.mailbox.account(id).unwrap());
    assert_eq!((back.kind, back.shared_of), (AccountKind::Exchange, Some(owner)));
}
