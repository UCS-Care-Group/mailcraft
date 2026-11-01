// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sender warnings in a session (#508): what `mail.get` says of a message, and how what they are
//! measured against (contacts, Sent Items, the accounts) keeps up with the mailbox.
//!
//! The people, hosts and addresses are invented.

use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, Body, FolderKind, Id, Message};
use serde_json::{Value, json};

use crate::Session;

/// A session with Avery's IMAP account at Lumen, whose Inbox has `messages` from its server; the
/// account's id.
fn imap(messages: Vec<Message>) -> (Session, Id) {
    let mut s = Session::empty();
    let a = s
        .run("account.add", &json!({"email": "avery@lumen.example", "displayName": "Avery Quinn", "incoming": {"host": "imap.lumen.example"}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let new = messages.into_iter().enumerate().map(|(i, m)| Message { uid: i as u32 + 1, ..m }).collect();
    let result = SyncResult {
        account: a,
        folders: vec![FolderDelta {
            id: None,
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: 50,
            new,
            ..Default::default()
        }],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": result})).unwrap();
    (s, a)
}

fn mail(name: &str, email: &str, subject: &str) -> Message {
    Message {
        from: Address::new(name, email),
        subject: subject.into(),
        message_id: format!("{subject}@example.org"),
        date: 100,
        ..Default::default()
    }
}

fn id_of(s: &Session, subject: &str) -> Id {
    s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.id).unwrap()
}

fn kinds(s: &mut Session, subject: &str) -> Vec<String> {
    let got = s.run("mail.get", &json!({"id": id_of(s, subject)})).unwrap();
    got["warnings"].as_array().unwrap().iter().map(|w| w["kind"].as_str().unwrap().to_string()).collect()
}

#[test]
fn mail_get_gives_the_warnings_as_data() {
    let mut failed = mail("Lin Zhao", "lin.zhao@harborworks.example", "Invoice");
    failed.headers = vec![("Authentication-Results".into(), "mx.lumen.example; dmarc=fail header.from=harborworks.example".into())];
    failed.body = Body {
        text: "Pay".into(),
        html: Some("<p>Pay at <a href=\"https://pay.evil.example/x\"><b>https://</b>harborworks.example/pay</a></p>".into()),
    };
    let (mut s, _) = imap(vec![failed, mail("Maya Okafor", "maya@lumen.example", "Lunch")]);
    let got = s.run("mail.get", &json!({"id": id_of(&s, "Invoice")})).unwrap();
    let warnings = got["warnings"].as_array().unwrap();
    assert_eq!(warnings[0], json!({"kind": "failedCheck", "level": "warning", "method": "dmarc"}));
    assert_eq!(warnings[1]["kind"], "linkMismatch");
    assert_eq!(warnings[1]["textHost"], "harborworks.example");
    assert_eq!(warnings[1]["host"], "pay.evil.example");
    assert_eq!(warnings[2], json!({"kind": "external", "level": "tag", "domain": "harborworks.example"}));
    // A colleague's ordinary mail: nothing.
    assert_eq!(kinds(&mut s, "Lunch"), Vec::<String>::new());
}

#[test]
fn a_contact_or_a_new_recipient_makes_a_lookalike_known() {
    let (mut s, _) = imap(vec![
        mail("Lin Zhao", "lin.zhao@harborworks.example", "Old thread"),
        mail("Lin", "lin@harborwork.example", "Lookalike"),
        mail("Robin Hale", "robin@cloudlines.example", "Second"),
    ]);
    // Nobody at harborworks.example is known yet: nothing to look like.
    assert_eq!(kinds(&mut s, "Lookalike"), ["external"]);
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    assert_eq!(kinds(&mut s, "Lookalike"), ["lookalikeSender", "external"]);
    // Then the lookalike itself becomes a contact: it is known.
    s.run("people.new", &json!({"displayName": "Lin at the depot", "email": "lin@harborwork.example"})).unwrap();
    assert_eq!(kinds(&mut s, "Lookalike"), ["external"]);

    // Written to: cloudline.example, from the Outbox on, makes cloudlines.example a lookalike;
    // and writing to cloudlines.example makes it known again.
    assert_eq!(kinds(&mut s, "Second"), ["external"]);
    s.run("mail.new", &json!({})).unwrap();
    s.run("compose.set", &json!({"to": "Billing <billing@cloudline.example>", "subject": "Card"})).unwrap();
    s.run("compose.send", &json!({})).unwrap();
    assert_eq!(kinds(&mut s, "Second"), ["lookalikeSender", "external"]);
    s.run("mail.new", &json!({})).unwrap();
    s.run("compose.set", &json!({"to": "robin@cloudlines.example", "subject": "Hello"})).unwrap();
    s.run("compose.send", &json!({})).unwrap();
    assert_eq!(kinds(&mut s, "Second"), ["external"]);
}

#[test]
fn a_known_name_on_a_stranger_address() {
    let (mut s, _) = imap(vec![mail("Maya Okafor", "maya.okafor@freemail.example", "Urgent"), mail("Avery Quinn", "ceo@freemail.example", "Mine")]);
    assert_eq!(kinds(&mut s, "Urgent"), ["external"]);
    s.run("people.new", &json!({"first": "Maya", "last": "Okafor", "email": "maya@lumen.example"})).unwrap();
    assert_eq!(kinds(&mut s, "Urgent"), ["nameHidesAddress", "external"]);
    let got = s.run("mail.get", &json!({"id": id_of(&s, "Mine")})).unwrap();
    assert_eq!(got["warnings"][0], json!({"kind": "nameHidesAddress", "level": "warning", "name": "Avery Quinn", "reason": "ownName"}));
}

#[test]
fn a_name_written_in_sent_items_is_not_compared() {
    let (mut s, _) = imap(vec![mail("Robin Hale", "robin.hale@freemail.example", "Hello")]);
    // The owner wrote to Robin by that name: the address is known, the name is not.
    s.run("mail.new", &json!({})).unwrap();
    s.run("compose.set", &json!({"to": "Robin Hale <robin@cloudline.example>", "subject": "Hi"})).unwrap();
    s.run("compose.send", &json!({})).unwrap();
    assert_eq!(kinds(&mut s, "Hello"), ["external"]);
    // A contact's name is.
    s.run("people.new", &json!({"displayName": "Robin Hale", "email": "robin@cloudline.example"})).unwrap();
    assert_eq!(kinds(&mut s, "Hello"), ["nameHidesAddress", "external"]);
}

#[test]
fn warnings_are_kept_until_what_they_rest_on_changes() {
    let (mut s, _) = imap(vec![mail("Lin", "lin@harborwork.example", "Lookalike")]);
    let m = s.mailbox.messages.get(&id_of(&s, "Lookalike")).cloned().unwrap();
    let first = s.sender_warnings(&m, false);
    let again = s.sender_warnings(&m, false);
    assert!(std::sync::Arc::ptr_eq(&first, &again));
    // Marking it read changes the mailbox, not what the warnings rest on.
    s.run("mail.markRead", &json!({"ids": [m.id]})).unwrap();
    let m = s.mailbox.messages.get(&m.id).cloned().unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &s.sender_warnings(&m, false)));
    // A contact does.
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first, &s.sender_warnings(&m, false)));
    // So does the owner's organisation.
    let before = s.sender_warnings(&m, false);
    s.run("account.add", &json!({"email": "avery@harborwork.example", "incoming": {"host": "imap.harborwork.example"}})).unwrap();
    let after = s.sender_warnings(&m, false);
    assert!(!std::sync::Arc::ptr_eq(&before, &after));
    // Now from the owner's own domain, so not External; and that domain is known, so no lookalike.
    assert!(after.is_empty(), "{after:?}");
}

#[test]
fn a_new_body_of_the_same_length_or_another_server_is_worked_out_again() {
    let (mut s, _) = imap(vec![mail("Lin", "lin@harborworks.example", "Body")]);
    let mut m = s.mailbox.messages.get(&id_of(&s, "Body")).cloned().unwrap();
    std::sync::Arc::make_mut(&mut m).body.html = Some(r#"<a href="https://lumen.example/">lumen.example</a>"#.into());
    assert!(s.sender_warnings(&m, true).iter().all(|w| w.kind() != "linkMismatch"));
    // The same length, another place.
    std::sync::Arc::make_mut(&mut m).body.html = Some(r#"<a href="https://lumen.exampl/">lumen.example</a>"#.into());
    assert!(s.sender_warnings(&m, true).iter().any(|w| w.kind() == "linkMismatch"));
    // A row asks without the links, and the pane's answer stands for it.
    let pane = s.sender_warnings(&m, true);
    assert!(std::sync::Arc::ptr_eq(&pane, &s.sender_warnings(&m, false)));
    // The server of the account is part of what the verdict rests on.
    let account = m.account;
    if let Some(a) = s.mailbox.accounts.iter_mut().find(|a| a.id == account)
        && let Some(server) = a.incoming.as_mut()
    {
        server.host = "imap.elsewhere.example".into();
    }
    assert!(!std::sync::Arc::ptr_eq(&pane, &s.sender_warnings(&m, false)));
}

#[test]
fn the_knowledge_follows_another_mailbox() {
    let (mut s, _) = imap(vec![]);
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    assert!(s.knows("lin.zhao@harborworks.example"));
    s.replace_mailbox(mailcraft_model::Mailbox::with_local_account("", ""));
    assert!(!s.knows("lin.zhao@harborworks.example"));
}

#[test]
fn hostile_messages_never_panic_mail_get() {
    let long = "x".repeat(100_000);
    let mut m = mail(&format!("{long}@{long}"), &format!("a@{long}.example"), "Long");
    m.headers = vec![("Authentication-Results".into(), format!("mx.lumen.example; dmarc=fail header.from={long}; {}", ";".repeat(10_000)))];
    let links: String = (0..3_000).map(|i| format!("<a href=\"https://x{i}.example/\">site{i}.example</a> ")).collect();
    m.body = Body { text: String::new(), html: Some(format!("<table><tr><td>{links}</td></tr></table>")) };
    let (mut s, _) = imap(vec![m]);
    let got = s.run("mail.get", &json!({"id": id_of(&s, "Long")})).unwrap();
    let warnings = got["warnings"].as_array().unwrap();
    assert!(warnings.len() <= 25, "{}", warnings.len());
    assert!(warnings.iter().all(|w| w.to_string().len() < 2_000));
    let _: &Value = &got;
}

fn banner_dismissed(s: &mut Session, subject: &str) -> bool {
    let m = s.mailbox.messages[&id_of(s, subject)].clone();
    let warnings = s.sender_warnings(&m, true);
    mailcraft_model::sender_warnings::dismissed(&warnings, &m.warnings_dismissed)
}

#[test]
fn dismissing_the_banner_is_remembered_with_the_message_and_can_be_undone() {
    let (mut s, _) = imap(vec![mail("Lin", "lin@harborwork.example", "Odd"), mail("Maya Okafor", "maya@lumen.example", "Lunch")]);
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    let id = id_of(&s, "Odd");
    assert!(!banner_dismissed(&mut s, "Odd"));
    assert_eq!(s.run("mail.dismissWarnings", &json!({"ids": [id]})).unwrap()["changed"], 1);
    assert!(banner_dismissed(&mut s, "Odd"));
    // Saved and read back, it stays dismissed.
    let saved = serde_json::to_string(&s.mailbox).unwrap();
    let mut again = Session::new(serde_json::from_str(&saved).unwrap());
    assert!(banner_dismissed(&mut again, "Odd"));
    // Undo shows it again; showing it again by command too.
    assert!(s.undo());
    assert!(!banner_dismissed(&mut s, "Odd"));
    s.run("mail.dismissWarnings", &json!({"ids": [id]})).unwrap();
    s.run("mail.dismissWarnings", &json!({"ids": [id], "value": false})).unwrap();
    assert!(!banner_dismissed(&mut s, "Odd"));
    // A message with nothing to warn of has no banner to dismiss.
    assert!(!banner_dismissed(&mut s, "Lunch"));
}

#[test]
fn a_new_kind_of_warning_brings_the_banner_back() {
    let (mut s, _) = imap(vec![mail("Lin", "lin@harborwork.example", "Odd")]);
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    let id = id_of(&s, "Odd");
    s.run("mail.dismissWarnings", &json!({"ids": [id]})).unwrap();
    assert!(banner_dismissed(&mut s, "Odd"));
    // The server's checks now fail for it: a kind that was not there.
    s.mailbox.message_mut(id).unwrap().headers =
        vec![("Authentication-Results".into(), "mx.lumen.example; dmarc=fail header.from=harborwork.example".into())];
    assert!(kinds(&mut s, "Odd").contains(&"failedCheck".to_string()));
    assert!(!banner_dismissed(&mut s, "Odd"));
}

#[test]
fn an_agent_may_not_hide_a_warning() {
    use crate::agent::{Access, Class, Verdict, access, check};
    assert_eq!(access("mail.dismissWarnings"), Some(Access::Owner(Class::Settings)));
    let (s, _) = imap(vec![mail("Lin", "lin@harborwork.example", "Odd")]);
    let params = json!({"ids": [id_of(&s, "Odd")]});
    assert!(matches!(check(&s, "mail.dismissWarnings", &params, &[]), Verdict::Ask(Class::Settings, _)));
}
