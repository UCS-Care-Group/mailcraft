// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account's other sender addresses (#797): kept by `account.setSenderAddresses`, offered as
//! From, and a reply, Reply All or forward is from the address the original was sent to.

use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::json;

use crate::Session;

const OWN: &str = "jorgen@company.example";
const PRODUCT: &str = "jorgen@product.example";
const SUPPORT: &str = "support@product.example";

/// A session with an IMAP account (`OWN`) that sends from two other addresses; its id.
fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let id = s
        .run("account.add", &json!({"email": OWN, "displayName": "Jorgen", "incoming": {"host": "imap.company.example"}, "outgoing": {"host": "smtp.company.example"}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.run("account.setSenderAddresses", &json!({"id": id, "addresses": [{"email": PRODUCT}, {"email": SUPPORT, "displayName": "Product Support"}]}))
        .unwrap();
    (s, id)
}

fn list(emails: &[&str]) -> Vec<Address> {
    emails.iter().map(|e| Address::new("", *e)).collect()
}

/// A message in the account's Inbox from a customer.
fn message(s: &mut Session, account: Id, to: &[&str], cc: &[&str], headers: &[(&str, &str)]) -> Id {
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let m = Message {
        account,
        folder,
        message_id: "797.1@example.net".into(),
        from: Address::new("Customer", "customer@example.net"),
        to: list(to),
        cc: list(cc),
        headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
        subject: "Licence".into(),
        date: 1_791_000_000,
        body: mailcraft_model::Body { text: "Which plan do I need?".into(), html: None },
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

/// What `command` on `id` opens: its From, To and Cc lines.
fn respond(s: &mut Session, command: &str, id: Id) -> (String, String, String) {
    s.run(command, &json!({"id": id})).unwrap();
    let c = s.compose.take().unwrap();
    (c.from, c.to, c.cc)
}

#[test]
fn the_list_is_kept_checked_and_answered() {
    let (mut s, id) = session();
    let shown = s.run("account.senderAddresses", &json!({"id": id})).unwrap();
    assert_eq!(shown["takes"], true);
    assert_eq!(shown["addresses"], json!([{"email": PRODUCT}, {"email": SUPPORT, "displayName": "Product Support"}]));
    // An address that isn't one is refused, and nothing changes.
    for bad in
        [json!([{"email": "nobody"}]), json!([{"email": "a@b.example\r\nBcc: x@evil.example"}]), json!([{"email": 5}]), json!(["x"]), json!("x")]
    {
        assert!(s.run("account.setSenderAddresses", &json!({"id": id, "addresses": bad})).is_err(), "{bad}");
    }
    assert_eq!(s.mailbox.account(id).unwrap().sender_addresses.len(), 2);
    // A signature it names must be one; it is kept as it is spelled there.
    assert!(s.run("account.setSenderAddresses", &json!({"id": id, "addresses": [{"email": SUPPORT, "signature": "Nope"}]})).is_err());
    s.run("signature.set", &json!({"name": "Support", "text": "The Product team"})).unwrap();
    s.run("account.setSenderAddresses", &json!({"id": id, "addresses": [{"email": SUPPORT, "signature": "support"}]})).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().sender_addresses[0].signature, "Support");
    // Renamed or deleted, the signature follows.
    s.run("signature.rename", &json!({"name": "Support", "to": "Helpdesk"})).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().sender_addresses[0].signature, "Helpdesk");
    s.run("signature.remove", &json!({"name": "Helpdesk"})).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().sender_addresses[0].signature, "");
    // None is fine.
    s.run("account.setSenderAddresses", &json!({"id": id, "addresses": []})).unwrap();
    assert!(s.mailbox.account(id).unwrap().sender_addresses.is_empty());
}

#[test]
fn a_microsoft_account_takes_none() {
    let mut s = Session::empty();
    let id = s.run("account.add", &json!({"email": "ada@contoso.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let why = s.run("account.setSenderAddresses", &json!({"id": id, "addresses": [{"email": "sales@contoso.example"}]})).unwrap_err().to_string();
    assert!(why.contains("Microsoft"), "{why}");
    assert_eq!(s.run("account.senderAddresses", &json!({"id": id})).unwrap()["takes"], false);
    s.run("account.setSenderAddresses", &json!({"id": id, "addresses": []})).unwrap();
}

#[test]
fn a_reply_is_from_the_address_the_original_was_sent_to() {
    let (mut s, id) = session();
    // To, in any letter case: kept as the account spells it.
    let m = message(&mut s, id, &["SUPPORT@Product.example"], &[], &[]);
    assert_eq!(respond(&mut s, "mail.reply", m).0, SUPPORT);
    // Cc.
    let m = message(&mut s, id, &["someone@example.net"], &[PRODUCT], &[]);
    assert_eq!(respond(&mut s, "mail.replyAll", m).0, PRODUCT);
    // Plus-addressing only as it is kept; nothing of the account's: its own address.
    for to in ["support+urgent@product.example", "list@example.net"] {
        let m = message(&mut s, id, &[to], &[], &[]);
        assert_eq!(respond(&mut s, "mail.reply", m).0, "", "{to}");
    }
    // In Bcc: by the header its server wrote.
    let m = message(&mut s, id, &["list@example.net"], &[], &[("X-Original-To", PRODUCT), ("Delivered-To", OWN)]);
    assert_eq!(respond(&mut s, "mail.reply", m).0, PRODUCT);
    // A forward too.
    let m = message(&mut s, id, &[SUPPORT], &[], &[]);
    assert_eq!(respond(&mut s, "mail.forward", m).0, SUPPORT);
    s.run("mail.forwardAsAttachment", &json!({"id": m})).unwrap();
    assert_eq!(s.compose.take().unwrap().from, SUPPORT);
}

#[test]
fn reply_all_leaves_out_the_owners_other_addresses() {
    let (mut s, id) = session();
    let m = message(&mut s, id, &[SUPPORT, "colleague@example.net"], &[PRODUCT, "boss@example.net"], &[]);
    let (from, to, cc) = respond(&mut s, "mail.replyAll", m);
    assert_eq!(from, SUPPORT);
    assert_eq!((to.as_str(), cc.as_str()), ("Customer <customer@example.net>; colleague@example.net", "boss@example.net"));
}

#[test]
fn from_is_chosen_and_checked() {
    let (mut s, id) = session();
    s.run("mail.new", &json!({"account": id, "from": "Jorgen@Product.example"})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().from, PRODUCT);
    s.run("compose.set", &json!({"from": OWN})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().from, "");
    s.run("compose.set", &json!({"from": SUPPORT})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().from, SUPPORT);
    // Not one of the account's: refused, and From stays.
    for bad in ["boss@example.net", "support+x@product.example", "not an address"] {
        assert!(s.run("compose.set", &json!({"from": bad})).is_err(), "{bad}");
    }
    assert!(s.run("mail.new", &json!({"account": id, "from": "boss@example.net"})).is_err());
    assert_eq!(s.compose.as_ref().unwrap().from, SUPPORT);
    // Another account is from its own address.
    let local = s.mailbox.accounts.iter().find(|a| a.id != id).map(|a| a.id);
    if let Some(local) = local {
        s.run("compose.set", &json!({"account": local})).unwrap();
        assert_eq!(s.compose.as_ref().unwrap().from, "");
    }
}

#[test]
fn a_message_from_a_sender_address_is_sent_from_it_and_saved_with_it() {
    let (mut s, id) = session();
    s.run("signature.set", &json!({"name": "Support", "text": "The Product team"})).unwrap();
    s.run(
        "account.setSenderAddresses",
        &json!({"id": id, "addresses": [{"email": PRODUCT}, {"email": SUPPORT, "displayName": "Product Support", "signature": "Support"}]}),
    )
    .unwrap();
    // Its signature comes with it.
    s.run("mail.new", &json!({"account": id, "from": SUPPORT, "to": "customer@example.net", "subject": "Plans"})).unwrap();
    assert!(s.compose.as_ref().unwrap().body.text().to_plain().contains("The Product team"));
    // Saved as a draft, it opens again from the same address.
    let draft = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    assert_eq!(s.mailbox.message(draft).unwrap().from, Address::new("Product Support", SUPPORT));
    s.compose = None;
    s.run("mail.editDraft", &json!({"id": draft})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().from, SUPPORT);
    // Sent, it waits in the account's Outbox, from that address, and its Message-ID is of that domain.
    let sent = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let m = s.mailbox.message(sent).unwrap();
    assert_eq!(m.folder, s.mailbox.folder_of_kind(id, FolderKind::Outbox).unwrap());
    assert_eq!(m.from, Address::new("Product Support", SUPPORT));
    assert!(m.message_id.ends_with("@product.example"), "{}", m.message_id);
}

#[test]
fn a_signature_that_is_gone_falls_back_to_the_accounts() {
    let (mut s, id) = session();
    s.run("signature.set", &json!({"name": "Main", "text": "Jorgen at the company"})).unwrap();
    // A list read from a file may name a signature that isn't there.
    let account = s.mailbox.account_mut(id).unwrap();
    account.signature = "Main".into();
    account.sender_addresses[1].signature = "Gone".into();
    s.run("mail.new", &json!({"account": id, "from": SUPPORT, "to": "customer@example.net", "subject": "Plans"})).unwrap();
    assert!(s.compose.as_ref().unwrap().body.text().to_plain().contains("Jorgen at the company"));
}

#[test]
fn a_row_with_a_name_but_no_address_says_so() {
    let (mut s, id) = session();
    let why =
        s.run("account.setSenderAddresses", &json!({"id": id, "addresses": [{"email": "", "displayName": "Support"}]})).unwrap_err().to_string();
    assert!(why.contains("no e-mail address") && !why.contains("\"\""), "{why}");
}

#[test]
fn a_sender_address_taken_away_is_not_sent_from() {
    let (mut s, id) = session();
    s.run("mail.new", &json!({"account": id, "from": PRODUCT, "to": "customer@example.net", "subject": "Plans"})).unwrap();
    s.run("account.setSenderAddresses", &json!({"id": id, "addresses": [{"email": SUPPORT}]})).unwrap();
    let why = s.run("compose.send", &json!({})).unwrap_err().to_string();
    assert!(why.contains(PRODUCT) && why.contains("no longer"), "{why}");
    assert!(s.compose.is_some());
    // From its own address, it goes.
    s.run("compose.set", &json!({"from": ""})).unwrap();
    let sent = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.message(sent).unwrap().from.email, OWN);
}

#[test]
fn an_agent_is_asked_before_it_changes_them_and_told_who_mail_is_from() {
    let (mut s, id) = session();
    let params = json!({"id": id, "addresses": [{"email": "ceo@company.example"}]});
    let verdict = crate::agent::check(&s, "account.setSenderAddresses", &params, &[]);
    assert!(matches!(verdict, crate::agent::Verdict::Ask(..)), "{verdict:?}");
    assert_eq!(crate::agent::check(&s, "account.senderAddresses", &json!({"id": id}), &[]), crate::agent::Verdict::Run);
    let said = crate::agent::describe(&s, "account.setSenderAddresses", &params);
    assert!(said.contains("ceo@company.example") && said.contains(OWN), "{said}");
    // Every address whole, however long: the end of its domain is what says whose it is.
    let long = format!("{}@{}.example", "a".repeat(60), "sub.".repeat(20));
    let said = crate::agent::describe(&s, "account.setSenderAddresses", &json!({"id": id, "addresses": [{"email": long}]}));
    assert!(said.contains(&long), "{said}");
    s.run("mail.new", &json!({"account": id, "from": SUPPORT, "to": "customer@example.net", "subject": "Plans"})).unwrap();
    let said = crate::agent::describe(&s, "compose.send", &json!({}));
    assert!(said.contains(&format!("from {SUPPORT}")), "{said}");
}

#[test]
fn a_message_as_its_server_delivered_it_is_answered_from_the_alias_and_sent_through_the_account() {
    let (mut s, id) = session();
    // To a list, the alias in Bcc: only its server's headers say which address it came to.
    let raw = "X-Original-To: support@product.example\r\nDelivered-To: jorgen@company.example\r\nFrom: Customer <customer@example.net>\r\n\
               To: list@example.net\r\nSubject: Licence\r\nMessage-ID: <797.2@example.net>\r\nDate: Tue, 01 Jul 2003 10:52:37 +0200\r\n\r\nWhich plan?\r\n";
    let mut m = mailcraft_mime_model::parse(raw.as_bytes());
    (m.account, m.folder) = (id, s.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap());
    let original = s.mailbox.add_message(m).unwrap();
    s.run("mail.reply", &json!({"id": original})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().from, SUPPORT);
    s.run("compose.send", &json!({})).unwrap();
    // What a Send/Receive hands the host: the account with its sender addresses, and the message
    // from the alias, for SMTP to send from it over the account's sign-in.
    let request = crate::cmd::account::sync_request(&s, id).unwrap();
    assert!(request.account.sends_as(SUPPORT));
    assert_eq!(request.outbox.iter().map(|m| m.from.email.as_str()).collect::<Vec<_>>(), [SUPPORT]);
}
