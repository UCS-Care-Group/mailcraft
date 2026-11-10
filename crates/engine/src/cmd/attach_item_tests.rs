// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Attach Item (a message as an `.eml`) and Contact Card (a contact as a `.vcf`).

use mailcraft_model::{Address, Message};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Session, sample};

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn writing() -> Session {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "mail.new", json!({}));
    s
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

#[test]
fn a_message_is_attached_whole_without_its_bcc() {
    let mut s = writing();
    let inbox = s.mailbox.inbox().unwrap();
    let account = s.mailbox.accounts[0].id;
    let id = s
        .mailbox
        .add_message(Message {
            account,
            folder: inbox,
            from: Address::new("Ada", "ada@example.org"),
            to: vec![Address::new("Avery", "avery@lumen.example")],
            bcc: vec![Address::new("Hidden", "hidden@example.org")],
            subject: "Plans".into(),
            date: 1_791_000_000,
            body: mailcraft_model::Body { text: "The plan, attached.".into(), html: None },
            flag: mailcraft_model::FlagStatus::Flagged,
            ..Default::default()
        })
        .unwrap();
    let r = run(&mut s, "compose.attachItem", json!({"id": id}));
    assert_eq!(r["name"], "Plans.eml");
    // Twice: a second file of the same name is numbered.
    let r = run(&mut s, "compose.attachItem", json!({"id": id}));
    assert_eq!(r["name"], "Plans (2).eml");
    let c = s.compose.as_ref().unwrap();
    assert!(c.dirty);
    let a = &c.attachments[0];
    assert_eq!(a.mime, "message/rfc822");
    let text = String::from_utf8_lossy(&a.data);
    assert!(text.contains("The plan, attached."), "{text}");
    assert!(!text.contains("hidden@example.org"), "{text}");
}

#[test]
fn a_contact_is_attached_as_a_vcard() {
    let mut s = writing();
    let id = *s.mailbox.contacts.keys().next().unwrap();
    let name = s.mailbox.contact(id).unwrap().name();
    let r = run(&mut s, "insert.businessCard", json!({"id": id}));
    assert!(r["name"].as_str().unwrap().ends_with(".vcf"), "{r}");
    let a = &s.compose.as_ref().unwrap().attachments[0];
    assert_eq!(a.mime, "text/vcard");
    let text = String::from_utf8_lossy(&a.data);
    assert!(text.starts_with("BEGIN:VCARD") && text.contains(&name), "{text}");
}

#[test]
fn the_item_is_named_never_taken_from_the_selection() {
    let mut s = writing();
    let some = *s.mailbox.messages.keys().next().unwrap();
    s.sel.messages = vec![some];
    for id in ["compose.attachItem", "insert.businessCard"] {
        for params in [json!({}), json!({"id": "x"}), json!({"id": -1}), json!({"id": 999_999}), json!({"id": some, "path": "/etc/passwd"})] {
            assert!(s.run(id, &params).is_err(), "{id} {params}");
        }
    }
    assert!(s.compose.as_ref().unwrap().attachments.is_empty());
    // Nothing being written: nothing to attach to.
    s.compose = None;
    assert!(s.run("compose.attachItem", &json!({"id": some})).is_err());
}
