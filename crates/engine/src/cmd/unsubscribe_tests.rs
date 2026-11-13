// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `mail.unsubscribe` (#1207): which way a message offers, when its headers are believed, and the
//! message that asks a list by mail.
//!
//! The people, lists and hosts are invented.

use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::{Value, json};

use crate::Session;

/// What Avery's server writes for mail from the Lumen News list that passed its checks.
const PASSED: &str = "mx.lumen.example; spf=pass smtp.mailfrom=news.example; dkim=pass header.d=news.example; dmarc=pass header.from=news.example";
const FAILED: &str = "mx.lumen.example; spf=fail smtp.mailfrom=elsewhere.example; dmarc=fail header.from=news.example";

/// A session with Avery's IMAP account, whose Inbox has one message from the list with `headers`.
fn with(headers: &[(&str, &str)]) -> (Session, Id) {
    let mut s = Session::empty();
    let a = s
        .run(
            "account.add",
            &json!({"email": "avery@lumen.example", "displayName": "Avery Quinn", "incoming": {"host": "imap.lumen.example"}, "outgoing": {"host": "smtp.lumen.example"}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let m = Message {
        uid: 1,
        from: Address::new("Lumen News", "letter@news.example"),
        subject: "This week".into(),
        message_id: "week@news.example".into(),
        date: 100,
        headers: headers.iter().map(|(h, v)| ((*h).into(), (*v).into())).collect(),
        ..Default::default()
    };
    let result = SyncResult {
        account: a,
        folders: vec![FolderDelta {
            id: None,
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: 5,
            new: vec![m],
            ..Default::default()
        }],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": result})).unwrap();
    let id = s.mailbox.messages.values().find(|m| m.subject == "This week").map(|m| m.id).unwrap();
    (s, id)
}

fn run(s: &mut Session, id: Id, more: Value) -> Result<Value, String> {
    let mut params = json!({"id": id});
    if let (Some(p), Some(more)) = (params.as_object_mut(), more.as_object()) {
        p.extend(more.clone());
    }
    s.run("mail.unsubscribe", &params).map_err(|e| e.to_string())
}

#[test]
fn a_vouched_message_says_its_way() {
    let (mut s, id) = with(&[
        ("Authentication-Results", PASSED),
        ("List-Unsubscribe", "<https://news.example/u/7f3a>, <mailto:leave@news.example>"),
        ("List-Unsubscribe-Post", "List-Unsubscribe=One-Click"),
    ]);
    let said = run(&mut s, id, json!({})).unwrap();
    assert_eq!(said["way"], "oneClick");
    assert_eq!(said["url"], "https://news.example/u/7f3a");
    assert_eq!(said["sent"], false);
    // One click is the window's: "send" doesn't post it from here.
    assert!(run(&mut s, id, json!({"send": true})).is_err());
}

#[test]
fn by_mail_the_request_goes_to_the_outbox_of_the_account_that_received_it() {
    let (mut s, id) = with(&[("Authentication-Results", PASSED), ("List-Unsubscribe", "<mailto:leave@news.example?subject=stop%20please>")]);
    let said = run(&mut s, id, json!({})).unwrap();
    assert_eq!(
        (said["way"].as_str(), said["to"].as_str(), said["subject"].as_str()),
        (Some("mail"), Some("leave@news.example"), Some("stop please"))
    );
    let outbox = s.mailbox.folders.iter().find(|f| f.kind == FolderKind::Outbox).map(|f| f.id).unwrap();
    assert_eq!(s.mailbox.messages_in(outbox).count(), 0, "nothing is sent without \"send\"");
    let said = run(&mut s, id, json!({"send": true})).unwrap();
    assert_eq!(said["sent"], true);
    let sent: Vec<&std::sync::Arc<Message>> = s.mailbox.messages_in(outbox).collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].from.email, "avery@lumen.example");
    assert_eq!(sent[0].to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["leave@news.example"]);
    assert_eq!(sent[0].subject, "stop please");
}

#[test]
fn headers_that_cant_be_believed_offer_nothing() {
    let list = ("List-Unsubscribe", "<https://news.example/u/7f3a>");
    // No verdict from the account's own server, or a failed one: anybody can write the header.
    for headers in [
        vec![list],
        vec![("Authentication-Results", FAILED), list],
        vec![("Authentication-Results", "mx.news.example; dmarc=pass header.from=news.example"), list],
    ] {
        let (mut s, id) = with(&headers);
        assert!(run(&mut s, id, json!({})).is_err(), "{headers:?}");
    }
    // An `id` named wrongly is refused, never read as the selection.
    let (mut s, id) = with(&[("Authentication-Results", PASSED), list]);
    s.run("mail.select", &json!({"id": id})).unwrap();
    assert!(s.run("mail.unsubscribe", &json!({"id": "invalid", "send": true})).is_err());
    // A message that doesn't say how.
    let (mut s, id) = with(&[("Authentication-Results", PASSED)]);
    assert!(run(&mut s, id, json!({})).is_err());
}

#[test]
fn a_message_with_a_sender_warning_offers_nothing() {
    let (mut s, id) = with(&[("Authentication-Results", PASSED), ("List-Unsubscribe", "<https://news.example/u/7f3a>")]);
    // The same list, under a name that shows another address (#508).
    if let Some(m) = s.mailbox.messages.get_mut(&id) {
        std::sync::Arc::make_mut(m).from.name = "billing@bank.example".into();
    }
    assert!(run(&mut s, id, json!({})).is_err_and(|e| e.contains("warning")));
}
