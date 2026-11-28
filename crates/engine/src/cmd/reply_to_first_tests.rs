// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whom a reply goes to (#736): the message's `Reply-To` first, whoever its `From` is; "a reply
//! to a message I sent goes to the people I sent it to" only when `Reply-To` names nobody but the
//! owner; Reply All adds the `To` and `Cc` that aren't the addresses the reply is from, each once,
//! and keeps the owner's other accounts (#1874).

use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::json;

use crate::Session;

const OWNER: &str = "owner@example.org";
const SHARED: &str = "support@example.org";

/// A session with a Microsoft account and a shared mailbox opened under it; the account's id.
fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("account.addSharedMailbox", &json!({"address": SHARED})).unwrap();
    (s, owner)
}

fn a(email: &str) -> Address {
    Address::new("", email)
}

fn list(emails: &[&str]) -> Vec<Address> {
    emails.iter().map(|e| a(e)).collect()
}

/// A message in the owner's Inbox.
fn message(s: &mut Session, account: Id, from: &str, to: &[&str], cc: &[&str], reply_to: &[&str]) -> Id {
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let m = Message {
        account,
        folder,
        message_id: "736.1@example.org".into(),
        from: a(from),
        to: list(to),
        cc: list(cc),
        reply_to: list(reply_to),
        subject: "Contact form".into(),
        date: 1_791_000_000,
        body: mailcraft_model::Body { text: "Please call me back.".into(), html: None },
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

/// The To and Cc lines of the reply `command` opens to `id`.
fn reply(s: &mut Session, command: &str, id: Id) -> (String, String) {
    s.run(command, &json!({"id": id})).unwrap();
    let c = s.compose.take().unwrap();
    (c.to, c.cc)
}

#[test]
fn a_contact_form_sent_from_the_owners_address_replies_to_its_reply_to() {
    let (mut s, owner) = session();
    // The report: the website sends as the owner, with the customer in Reply-To.
    let id = message(&mut s, owner, OWNER, &[OWNER], &[], &["customer@example.net"]);
    let (to, cc) = reply(&mut s, "mail.reply", id);
    assert_eq!((to.as_str(), cc.as_str()), ("customer@example.net", ""));
    let (to, cc) = reply(&mut s, "mail.replyAll", id);
    assert_eq!((to.as_str(), cc.as_str()), ("customer@example.net", ""), "the owner never in it");
}

#[test]
fn reply_all_adds_the_to_and_cc_that_arent_the_owners_once_each() {
    let (mut s, owner) = session();
    let id = message(
        &mut s,
        owner,
        OWNER,
        &[OWNER, "colleague@example.org", "Customer@Example.net"],
        &["boss@example.org", SHARED, "colleague@example.org"],
        &["customer@example.net"],
    );
    let (to, cc) = reply(&mut s, "mail.reply", id);
    assert_eq!(to, "customer@example.net");
    assert_eq!(cc, "");
    let (to, cc) = reply(&mut s, "mail.replyAll", id);
    assert_eq!(to, "customer@example.net; colleague@example.org");
    assert_eq!(cc, "boss@example.org");
}

#[test]
fn several_reply_to_addresses_are_all_answered_without_the_owners_own() {
    let (mut s, owner) = session();
    let id = message(&mut s, owner, "form@example.net", &[OWNER], &[], &["one@example.net", OWNER, "two@example.net", "ONE@example.net"]);
    let (to, _) = reply(&mut s, "mail.reply", id);
    assert_eq!(to, "one@example.net; two@example.net");
}

#[test]
fn a_message_the_owner_sent_with_his_own_reply_to_replies_to_its_recipients() {
    let (mut s, owner) = session();
    // Direct Replies To another of his own addresses (the shared mailbox).
    let id = message(&mut s, owner, OWNER, &["alex@example.net"], &["sam@example.net"], &[SHARED]);
    let (to, cc) = reply(&mut s, "mail.reply", id);
    assert_eq!((to.as_str(), cc.as_str()), ("alex@example.net", ""));
    let (to, cc) = reply(&mut s, "mail.replyAll", id);
    assert_eq!((to.as_str(), cc.as_str()), ("alex@example.net", "sam@example.net"));
}

#[test]
fn a_message_the_owner_sent_without_reply_to_replies_to_its_recipients() {
    let (mut s, owner) = session();
    let id = message(&mut s, owner, OWNER, &["alex@example.net"], &[], &[]);
    let (to, _) = reply(&mut s, "mail.reply", id);
    assert_eq!(to, "alex@example.net");
}

#[test]
fn a_message_the_owner_sent_with_a_colleague_in_reply_to_replies_to_the_colleague() {
    let (mut s, owner) = session();
    let id = message(&mut s, owner, OWNER, &["alex@example.net"], &[], &["colleague@example.org"]);
    let (to, _) = reply(&mut s, "mail.reply", id);
    assert_eq!(to, "colleague@example.org");
}

#[test]
fn a_note_to_self_replies_to_the_owner() {
    let (mut s, owner) = session();
    let id = message(&mut s, owner, OWNER, &[OWNER], &[], &[]);
    let (to, _) = reply(&mut s, "mail.reply", id);
    assert_eq!(to, OWNER);
}

#[test]
fn mail_from_a_shared_mailbox_counts_as_the_owners_own() {
    let (mut s, owner) = session();
    let id = message(&mut s, owner, SHARED, &["alex@example.net"], &[], &[OWNER]);
    let (to, _) = reply(&mut s, "mail.reply", id);
    assert_eq!(to, "alex@example.net");
    let id = message(&mut s, owner, SHARED, &[SHARED], &[], &["customer@example.net"]);
    let (to, _) = reply(&mut s, "mail.reply", id);
    assert_eq!(to, "customer@example.net");
}

#[test]
fn mail_from_others_replies_to_reply_to_else_from() {
    let (mut s, owner) = session();
    // A mailing list that sets Reply-To to itself.
    let id = message(&mut s, owner, "poster@example.net", &["list@example.net"], &[], &["list@example.net"]);
    let (to, _) = reply(&mut s, "mail.reply", id);
    assert_eq!(to, "list@example.net");
    let (to, cc) = reply(&mut s, "mail.replyAll", id);
    assert_eq!((to.as_str(), cc.as_str()), ("list@example.net", ""));
    let id = message(&mut s, owner, "poster@example.net", &[OWNER], &[], &[]);
    let (to, _) = reply(&mut s, "mail.reply", id);
    assert_eq!(to, "poster@example.net");
}

#[test]
fn a_forward_is_addressed_to_nobody_whatever_reply_to_says() {
    let (mut s, owner) = session();
    let id = message(&mut s, owner, OWNER, &[OWNER], &["boss@example.org"], &["customer@example.net"]);
    let (to, cc) = reply(&mut s, "mail.forward", id);
    assert_eq!((to.as_str(), cc.as_str()), ("", ""));
}

#[test]
fn a_reply_to_line_too_long_to_be_true_is_answered_only_in_part() {
    let (mut s, owner) = session();
    let many: Vec<String> = (0..5000).map(|i| format!("p{i}@example.net")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let id = message(&mut s, owner, OWNER, &[OWNER], &[], &refs);
    let (to, _) = reply(&mut s, "mail.replyAll", id);
    assert_eq!(to.split("; ").count(), mailcraft_mime::MAX_RECIPIENTS);
}

#[test]
fn mail_from_others_whose_reply_to_names_only_the_owner_replies_to_the_sender() {
    let (mut s, owner) = session();
    let id = message(&mut s, owner, "sender@example.net", &[OWNER, "alex@example.net"], &["sam@example.net", SHARED], &[OWNER, SHARED]);
    let (to, cc) = reply(&mut s, "mail.reply", id);
    assert_eq!((to.as_str(), cc.as_str()), ("sender@example.net", ""));
    let (to, cc) = reply(&mut s, "mail.replyAll", id);
    assert_eq!((to.as_str(), cc.as_str()), ("sender@example.net; alex@example.net", "sam@example.net"));
}

#[test]
fn reply_all_keeps_the_owners_other_accounts() {
    // #1874: one message to both of the owner's accounts and to somebody else, answered from one.
    let mut s = Session::empty();
    let imap = |email: &str| json!({"email": email, "incoming": {"host": "imap.example.org"}, "outgoing": {"host": "smtp.example.org"}});
    let org = s.run("account.add", &imap("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    s.run("account.add", &imap("ada@example.net")).unwrap();
    let id = message(&mut s, org, "bob@example.net", &["ada@example.org", "ada@example.net", "alex@example.net"], &[], &[]);
    let (to, cc) = reply(&mut s, "mail.replyAll", id);
    assert_eq!((to.as_str(), cc.as_str()), ("bob@example.net; ada@example.net; alex@example.net", ""));
    // In Cc too; a plain Reply still goes to the sender alone.
    let id = message(&mut s, org, "bob@example.net", &["ada@example.org"], &["ADA@example.net", "sam@example.net"], &[]);
    let (to, cc) = reply(&mut s, "mail.replyAll", id);
    assert_eq!((to.as_str(), cc.as_str()), ("bob@example.net", "ADA@example.net; sam@example.net"));
    let (to, cc) = reply(&mut s, "mail.reply", id);
    assert_eq!((to.as_str(), cc.as_str()), ("bob@example.net", ""));
}

#[test]
fn reply_all_leaves_out_the_sender_addresses_of_the_account_it_is_from() {
    let mut s = Session::empty();
    let imap = json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}, "outgoing": {"host": "smtp.example.org"}});
    let ada = s.run("account.add", &imap).unwrap()["id"].as_u64().unwrap();
    s.run("account.setSenderAddresses", &json!({"id": ada, "addresses": [{"email": "sales@example.org"}]})).unwrap();
    let id = message(&mut s, ada, "bob@example.net", &["ada@example.org", "Sales@example.org", "alex@example.net"], &["sales@example.org"], &[]);
    let (to, cc) = reply(&mut s, "mail.replyAll", id);
    assert_eq!((to.as_str(), cc.as_str()), ("bob@example.net; alex@example.net", ""));
}

#[test]
fn reply_all_from_a_shared_mailbox_leaves_out_the_account_it_is_opened_through() {
    let (mut s, _) = session();
    let shared = s.mailbox.accounts.iter().find(|a| a.email == SHARED).unwrap().id;
    let id = message(&mut s, shared, "customer@example.net", &[SHARED, OWNER, "alex@example.net"], &[], &[]);
    let (to, cc) = reply(&mut s, "mail.replyAll", id);
    assert_eq!((to.as_str(), cc.as_str()), ("customer@example.net; alex@example.net", ""));
}

#[test]
fn reply_all_to_own_sent_mail_still_skips_every_owner_address() {
    let mut s = Session::empty();
    let imap = |email: &str| json!({"email": email, "incoming": {"host": "imap.example.org"}, "outgoing": {"host": "smtp.example.org"}});
    let org = s.run("account.add", &imap("ada@example.org")).unwrap()["id"].as_u64().unwrap();
    s.run("account.add", &imap("ada@example.net")).unwrap();
    let id = message(&mut s, org, "ada@example.org", &["alex@example.net"], &["ada@example.net", "sam@example.net"], &[]);
    let (to, cc) = reply(&mut s, "mail.replyAll", id);
    assert_eq!((to.as_str(), cc.as_str()), ("alex@example.net", "sam@example.net"));
}
