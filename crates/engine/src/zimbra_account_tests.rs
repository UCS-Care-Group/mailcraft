// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Zimbra accounts, the skeleton (Z1): what the commands refuse with the switch off and on, and a
//! mailbox that holds one opening.

use mailcraft_model::{AccountKind, FolderKind, Message};
use serde_json::{Value, json};

use crate::Session;
use crate::cmd::zimbra_mail::{NOT_BUILT, SENDING_NOT_BUILT, SWITCHED_OFF};

const URL: &str = "https://zimbra.example.org";

fn zimbra(email: &str) -> Value {
    json!({"email": email, "kind": "zimbra", "zimbra": {"url": URL, "username": "ada"}})
}

fn said(result: Result<Value, crate::CmdError>) -> String {
    let said = result.expect_err("should have been refused").to_string();
    // (A command that is out of reach says so first.)
    said.strip_prefix("not available: ").map_or(said.clone(), str::to_string)
}

fn switched_on() -> Session {
    let mut s = Session::empty();
    s.zimbra = true;
    s
}

#[test]
fn with_the_switch_off_nothing_of_zimbra_is_made_or_run() {
    let mut s = Session::empty();
    assert!(said(s.run("account.add", &zimbra("ada@example.org"))).contains(SWITCHED_OFF));
    assert!(s.mailbox.accounts.iter().all(|a| a.kind != AccountKind::Zimbra));
    for command in ["sync.zimbraRequest", "sync.applyZimbra", "sync.zimbraFetchRequest"] {
        assert!(said(s.run(command, &json!({}))).contains(SWITCHED_OFF), "{command}");
    }
}

#[test]
fn a_zimbra_account_is_added_and_its_commands_answer() {
    let mut s = switched_on();
    let id = s.run("account.add", &zimbra("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    let a = s.mailbox.account(id).unwrap().clone();
    assert_eq!((a.kind, a.incoming.is_none(), a.outgoing.is_none(), a.oauth.is_none()), (AccountKind::Zimbra, true, true, true));
    let server = a.zimbra.clone().unwrap();
    assert_eq!((server.url.as_str(), server.username.as_str(), server.token.as_str()), (URL, "ada", ""));
    // What account.list says names the server, never where its changes go on from.
    let listed = s.run("account.list", &json!({})).unwrap();
    let row = listed["accounts"].as_array().unwrap().iter().find(|a| a["id"] == id).unwrap();
    assert_eq!((row["kind"].as_str(), row["zimbra"]["server"].as_str()), (Some("zimbra"), Some(URL)));
    assert_eq!(crate::cmd::account::servers_text(&a), format!("zimbra {URL}"));
    // The IMAP request is not for it, and its own isn't built.
    assert!(said(s.run("sync.request", &json!({"account": id}))).contains(NOT_BUILT));
    assert!(crate::cmd::account::sync_request(&s, id).is_err());
    assert!(said(s.run("sync.zimbraRequest", &json!({"account": id}))).contains(NOT_BUILT));
    assert!(said(s.run("sync.zimbraFetchRequest", &json!({"account": id}))).contains(NOT_BUILT));
    let result = json!({"result": {"account": id, "email": "ada@example.org"}});
    assert!(said(s.run("sync.applyZimbra", &result)).contains(NOT_BUILT));
    // A result that isn't one, or for another account, is refused before anything else.
    assert!(!said(s.run("sync.applyZimbra", &json!({"result": {"account": id, "email": "eve@example.org"}}))).contains(NOT_BUILT));
    assert!(!said(s.run("sync.applyZimbra", &json!({"result": {"account": id, "token": "no spaces"}}))).contains(NOT_BUILT));
}

#[test]
fn its_settings_are_checked_and_its_kind_is_kept() {
    let mut s = switched_on();
    for bad in [
        json!({"email": "ada@example.org", "kind": "zimbra"}),
        json!({"email": "ada@example.org", "kind": "zimbra", "zimbra": {"url": "http://zimbra.example.org"}}),
        json!({"email": "ada@example.org", "kind": "zimbra", "zimbra": {"url": "https://zimbra.example.org/?zauthtoken=x"}}),
        json!({"email": "ada@example.org", "kind": "zimbra", "zimbra": {"url": URL, "trusted": "nope"}}),
        json!({"email": "ada@example.org", "kind": "zimbra", "zimbra": "https://zimbra.example.org"}),
    ] {
        assert!(s.run("account.add", &bad).is_err(), "{bad}");
    }
    let id = s.run("account.add", &zimbra("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    // An edit that names no kind keeps it, and the server's token with it.
    s.mailbox.account_mut(id).unwrap().zimbra.as_mut().unwrap().token = "4711".into();
    s.run("account.edit", &json!({"id": id, "displayName": "Ada", "zimbra": {"username": "ada"}})).unwrap();
    let a = s.mailbox.account(id).unwrap();
    let server = a.zimbra.as_ref().unwrap();
    assert_eq!((a.kind, server.username.as_str(), server.token.as_str()), (AccountKind::Zimbra, "ada", "4711"));
    // It doesn't turn into another kind, nor another kind into it.
    assert!(s.run("account.edit", &json!({"id": id, "kind": "imap", "incoming": {"host": "imap.example.org"}})).is_err());
    let imap = s.run("account.add", &json!({"email": "bob@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    assert!(s.run("account.edit", &json!({"id": imap, "kind": "zimbra", "zimbra": {"url": URL}})).is_err());
    // With the switch off its settings stay as they are.
    s.zimbra = false;
    assert!(said(s.run("account.edit", &json!({"id": id, "zimbra": {"url": "https://other.example.org"}}))).contains(SWITCHED_OFF));
}

#[test]
fn a_message_from_a_zimbra_account_is_not_filed_as_sent() {
    let mut s = switched_on();
    let id = s.run("account.add", &zimbra("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    s.run("mail.new", &json!({"account": id, "to": "bob@example.org", "subject": "Hello", "body": "Hi"})).unwrap();
    assert!(said(s.run("compose.send", &json!({}))).contains(SENDING_NOT_BUILT));
    let sent = s.mailbox.folder_of_kind(id, FolderKind::Sent).unwrap();
    assert_eq!(s.mailbox.count(sent), 0);
    assert!(s.compose.is_some());
}

#[test]
fn a_mailbox_with_a_zimbra_account_opens_with_the_switch_off() {
    let mut s = switched_on();
    let id = s.run("account.add", &zimbra("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap();
    s.mailbox.add_message(Message { folder: inbox, subject: "Kept from the server".into(), date: 10, ..Default::default() }).unwrap();
    let json = serde_json::to_string(&s.mailbox).unwrap();
    assert!(json.contains("\"zimbra\"") && !json.contains("password"));
    let mb: mailcraft_model::Mailbox = serde_json::from_str(&json).unwrap();
    let opened = Session::new(mb);
    assert!(!opened.zimbra);
    let a = opened.mailbox.account(id).unwrap();
    assert_eq!((a.kind, a.zimbra.as_ref().map(|z| z.url.as_str())), (AccountKind::Zimbra, Some(URL)));
    assert_eq!(opened.mailbox.count(inbox), 1);
}

#[test]
fn identity_edits_with_the_switch_off_leave_zimbra_unchanged() {
    let mut s = switched_on();
    let id = s.run("account.add", &zimbra("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    s.zimbra = false;
    assert!(said(s.run("account.edit", &json!({"id":id,"email":"other@example.org"}))).contains(SWITCHED_OFF));
    assert_eq!(s.mailbox.account(id).unwrap().email, "ada@example.org");
    s.run("account.edit", &json!({"id":id,"displayName":"Ada"})).unwrap();
}

#[test]
fn permitted_mailbox_changes_clear_both_zimbra_checkpoints() {
    for patch in [
        json!({"zimbra":{"url":"https://other.example.org"}}),
        json!({"zimbra":{"username":"other"}}),
        json!({"email":"other@example.org","zimbra":{"username":""}}),
    ] {
        let mut s = switched_on();
        let id = s.run("account.add", &zimbra("ada@example.org")).unwrap()["id"].as_u64().unwrap();
        let server = s.mailbox.account_mut(id).unwrap().zimbra.as_mut().unwrap();
        server.token = "42".into();
        server.resume.push(mailcraft_model::sync::ZimbraResume { folder: "2".into(), cursor: "301".into(), sort: 1 });
        let mut edit = patch;
        edit["id"] = json!(id);
        s.run("account.edit", &edit).unwrap();
        let server = s.mailbox.account(id).unwrap().zimbra.as_ref().unwrap();
        assert!(server.token.is_empty() && server.resume.is_empty(), "{edit}");
    }
}

#[test]
fn display_trust_and_equivalent_identity_edits_keep_zimbra_checkpoints() {
    for patch in [
        json!({"displayName":"Ada"}),
        json!({"zimbra":{"trusted":"a".repeat(64)}}),
        json!({"zimbra":{"username":"ADA"}}),
        json!({"zimbra":{"url":format!("{URL}/")}}),
        json!({"email":"ADA@example.org"}),
    ] {
        let mut s = switched_on();
        let id = s.run("account.add", &zimbra("ada@example.org")).unwrap()["id"].as_u64().unwrap();
        let resume = mailcraft_model::sync::ZimbraResume { folder: "2".into(), cursor: "301".into(), sort: 1 };
        let server = s.mailbox.account_mut(id).unwrap().zimbra.as_mut().unwrap();
        server.token = "42".into();
        server.resume.push(resume.clone());
        let mut edit = patch;
        edit["id"] = json!(id);
        s.run("account.edit", &edit).unwrap();
        let server = s.mailbox.account(id).unwrap().zimbra.as_ref().unwrap();
        assert_eq!(server.token, "42", "{edit}");
        assert_eq!(server.resume, [resume], "{edit}");
    }
}

#[test]
fn implicit_sync_prefers_a_supported_account_over_zimbra() {
    let mut s = switched_on();
    let z = s.run("account.add", &zimbra("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    let imap = s.run("account.add", &json!({"email":"bob@example.org","incoming":{"host":"imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    let result = s.run("sync.request", &json!({})).unwrap();
    assert_eq!(result["account"]["id"], imap);
    assert!(said(s.run("sync.request", &json!({"account":z}))).contains(NOT_BUILT));
}

#[test]
fn an_unfetched_zimbra_message_cannot_be_copied_without_its_server_identity() {
    let mut s = switched_on();
    let id = s.run("account.add", &zimbra("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap();
    let sent = s.mailbox.folder_of_kind(id, FolderKind::Sent).unwrap();
    let message = s
        .mailbox
        .add_message(Message {
            account: id,
            folder: inbox,
            unfetched: true,
            server: Some(mailcraft_model::sync::ServerMessage { id: "301".into(), folder: "2".into(), ..Default::default() }),
            ..Default::default()
        })
        .unwrap();
    assert!(s.mailbox.copy_message(message, sent).is_err());
    assert_eq!(s.mailbox.count(sent), 0);
    s.mailbox.message_mut(message).unwrap().unfetched = false;
    assert!(s.mailbox.copy_message(message, sent).is_ok());
}
