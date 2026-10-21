// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account a newer MailCraft added (`AccountKind::Unknown`): its file's account goes back into
//! the file as it came, and nothing here sends, receives or changes its settings. Its mail can be
//! read, and what is changed in it is kept in the mailbox; no server is told.

use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Account, AccountKind, Address, FolderKind, Id, Mailbox, Message, Security, Server};
use serde_json::{Value, json};

use crate::Session;

/// What a newer MailCraft would write: a local account, an account of a kind this version doesn't
/// know (`fromANewerVersion`, which no version has) with settings of its own, one of them in a shape this version would refuse for an
/// account it knows, and an IMAP account; the newer account's Inbox has a message. The file, and
/// the ids of the local, the newer and the IMAP account.
fn written_by_a_newer_version() -> (Vec<u8>, Id, Id, Id) {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let local = mb.accounts[0].id;
    let newer = mb.add_account(Account { email: "ada@jmap.example".into(), ..Default::default() });
    let servers = Some(Server { host: "imap.example.org".into(), port: 993, security: Security::Tls });
    let imap = mb.add_account(Account { email: "ada@imap.example".into(), kind: AccountKind::Imap, incoming: servers, ..Default::default() });
    let inbox = mb.folder_of_kind(newer, FolderKind::Inbox).unwrap();
    let from = Address::new("Bo Marsh", "bo@example.org");
    mb.add_message(Message { folder: inbox, subject: "Written in the newer version".into(), from, date: 10, ..Default::default() }).unwrap();
    let mut file = serde_json::to_value(&mb).unwrap();
    let account = file["accounts"].as_array_mut().unwrap().iter_mut().find(|a| a["id"] == newer).unwrap();
    *account = json!({
        "id": newer,
        "name": "Ada at home",
        "email": "ada@jmap.example",
        "displayName": "Ada",
        "kind": "fromANewerVersion",
        "settings": {"session": "https://jmap.example/.well-known/jmap", "auth": "bearer"},
        "incoming": "a newer version's own way of naming a server",
        "oauth": {"provider": "microsoft", "tenant": "organizations"},
        "username": "ada",
        "signature": "Home",
        "comesLater": [1, 2.5, {"deep": null, "flag": true}],
    });
    (serde_json::to_vec(&file).unwrap(), local, newer, imap)
}

/// That file opened as a mailbox file is (`io::read_mailbox`, which repairs what it reads).
fn opened() -> (Session, Id, Id, Id) {
    let (file, local, newer, imap) = written_by_a_newer_version();
    (Session::new(crate::io::read_mailbox(&file).unwrap()), local, newer, imap)
}

/// The account `id` as the mailbox file would have it now.
fn as_saved(s: &Session, id: Id) -> Value {
    let file: Value = serde_json::from_slice(&crate::io::write_mailbox(&s.mailbox).unwrap()).unwrap();
    file["accounts"].as_array().unwrap().iter().find(|a| a["id"] == id).cloned().unwrap_or(Value::Null)
}

fn refusal(s: &mut Session, id: &str, params: Value) -> String {
    match s.run(id, &params) {
        Err(e) => e.to_string(),
        Ok(answer) => panic!("{id} {params} wasn't refused: {answer}"),
    }
}

#[test]
fn it_is_read_and_goes_back_into_the_file_as_it_came() {
    let (file, _, newer, _) = written_by_a_newer_version();
    let (s, ..) = opened();
    let a = s.mailbox.account(newer).unwrap();
    assert_eq!((a.kind, a.email.as_str(), a.incoming.is_none(), a.oauth.is_none()), (AccountKind::Unknown, "ada@jmap.example", true, true));
    let written: Value = serde_json::from_slice(&file).unwrap();
    let came = written["accounts"].as_array().unwrap().iter().find(|a| a["id"] == newer).cloned().unwrap();
    assert_eq!(as_saved(&s, newer), came);
    // Its mail is there to read, as anybody's.
    let inbox = s.mailbox.folder_of_kind(newer, FolderKind::Inbox).unwrap();
    assert_eq!(s.mailbox.messages_in(inbox).map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Written in the newer version"]);
}

#[test]
fn it_takes_no_part_in_a_send_receive() {
    let (mut s, _, newer, imap) = opened();
    // Send/Receive with no account named is the IMAP account's, never the newer one's.
    assert_eq!(s.run("sync.request", &json!({})).unwrap()["account"]["id"].as_u64(), Some(imap));
    let why = refusal(&mut s, "sync.request", json!({"account": newer}));
    assert!(why.contains("was added by a newer version of MailCraft, which sends and receives its mail"), "{why}");
    assert!(crate::cmd::account::sync_request(&s, newer).is_err());
    // An answer for it is refused: no run here asks its server anything.
    let before = s.mailbox.clone();
    let inbox = s.mailbox.folder_of_kind(newer, FolderKind::Inbox).unwrap();
    let new = vec![Message { uid: 1, subject: "From nowhere".into(), ..Default::default() }];
    let delta =
        FolderDelta { id: Some(inbox), remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 1, uid_seen: 1, new, ..Default::default() };
    refusal(&mut s, "sync.apply", json!({"result": SyncResult { account: newer, folders: vec![delta], ..Default::default() }}));
    assert!(s.mailbox == before);
    // As agents and scripts see it: of a kind this version doesn't know, with no settings to use.
    let listed = s.run("account.list", &json!({})).unwrap();
    let it = listed["accounts"].as_array().unwrap().iter().find(|a| a["id"] == newer).cloned().unwrap();
    assert_eq!(
        (it["kind"].as_str(), &it["incoming"], &it["oauth"], it["username"].as_str()),
        (Some("unknown"), &Value::Null, &Value::Null, Some(""))
    );
    // A mailbox with no other account on a server has no Send/Receive to offer.
    let mut alone = Session::new(s.mailbox.clone());
    alone.run("account.remove", &json!({"id": imap})).unwrap();
    assert!(alone.run("sync.request", &json!({})).is_err());
}

#[test]
fn a_message_written_from_it_is_neither_sent_nor_filed_and_stays_written() {
    let (mut s, local, newer, _) = opened();
    let waiting =
        |s: &Session, account: Id| [FolderKind::Outbox, FolderKind::Sent].map(|k| s.mailbox.count(s.mailbox.folder_of_kind(account, k).unwrap()));
    // (To the local account's address: a local account's mail would be delivered there.)
    s.run("mail.new", &json!({"account": newer, "to": "ada@example.org", "subject": "Lunch"})).unwrap();
    let why = refusal(&mut s, "compose.send", json!({}));
    assert!(why.contains("send this message from another account, or save it as a draft"), "{why}");
    // Still being written as it was: nothing queued, nothing filed as sent, nothing delivered.
    assert_eq!(s.compose.as_ref().map(|c| (c.account, c.to.as_str(), c.subject.as_str())), Some((newer, "ada@example.org", "Lunch")));
    let local_inbox = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    assert_eq!((waiting(&s, newer), s.mailbox.count(local_inbox)), ([0, 0], 0));
    // Saved as a draft, it is kept in its Drafts, in this mailbox.
    let draft = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    let drafts = s.mailbox.folder_of_kind(newer, FolderKind::Drafts).unwrap();
    assert_eq!(s.mailbox.message(draft).map(|m| (m.account, m.folder)), Some((newer, drafts)));
    // From another account it goes as any message does.
    s.run("compose.set", &json!({"account": local})).unwrap();
    let sent = s.run("compose.send", &json!({})).unwrap();
    assert_eq!((sent["queued"].as_bool(), sent["delivered"].as_u64()), (Some(false), Some(1)));
    assert_eq!(waiting(&s, newer), [0, 0]);
}

#[test]
fn its_settings_cant_be_changed_here_and_it_can_be_removed() {
    let (mut s, local, newer, imap) = opened();
    let kept = as_saved(&s, newer);
    for (id, params) in [
        ("account.edit", json!({"id": newer, "displayName": "Someone else"})),
        ("account.edit", json!({"id": newer, "kind": "local"})),
        ("signature.defaults", json!({"account": newer, "new": ""})),
        ("signature.set", json!({"name": "Home", "text": "Ada", "default": true, "account": newer})),
    ] {
        let why = refusal(&mut s, id, params);
        assert!(why.contains("was added by a newer version of MailCraft"), "{id}: {why}");
    }
    assert_eq!(as_saved(&s, newer), kept);
    // Nor does what is done to the signatures reach it, though its file names one by that name.
    s.run("signature.set", &json!({"name": "Home", "text": "Ada at home", "default": true, "account": local})).unwrap();
    s.run("signature.rename", &json!({"name": "Home", "to": "Away"})).unwrap();
    s.run("signature.remove", &json!({"name": "Away"})).unwrap();
    assert_eq!(as_saved(&s, newer), kept);
    assert_eq!(s.mailbox.account(newer).map(|a| (a.signature.as_str(), a.reply_signature.clone())), Some(("", None)));
    // Removed, it goes with its folders and mail, and nothing else changes.
    let others = [local, imap].map(|id| as_saved(&s, id));
    s.run("account.remove", &json!({"id": newer})).unwrap();
    assert!(s.mailbox.account(newer).is_none() && !s.mailbox.folders.iter().any(|f| f.account == newer));
    assert!(!s.mailbox.messages.values().any(|m| m.account == newer));
    assert_eq!([local, imap].map(|id| as_saved(&s, id)), others);
    assert!(!String::from_utf8(crate::io::write_mailbox(&s.mailbox).unwrap()).unwrap().contains("fromANewerVersion"));
}

#[test]
fn what_is_changed_in_its_mail_is_kept_and_an_undo_brings_it_back_whole() {
    let (mut s, _, newer, _) = opened();
    let kept = as_saved(&s, newer);
    let inbox = s.mailbox.folder_of_kind(newer, FolderKind::Inbox).unwrap();
    let id = s.mailbox.messages_in(inbox).next().unwrap().id;
    s.run("mail.flag", &json!({"id": id})).unwrap();
    let file: Value = serde_json::from_slice(&crate::io::write_mailbox(&s.mailbox).unwrap()).unwrap();
    assert_eq!(file["messages"][id.to_string()]["flag"], "flagged");
    assert_eq!(as_saved(&s, newer), kept);
    // Changed and brought back by Undo, it is as its file had it. (Removing it keeps no Undo
    // step: #177.)
    s.run("mail.flag", &json!({"id": id})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(as_saved(&s, newer), kept);
}
