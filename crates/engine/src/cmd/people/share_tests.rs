// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People › Meeting and Forward Contact: a meeting with the contact invited, as New Meeting makes
//! it, and a new message with the contact's card attached, which nothing sends. Invented people
//! at `.example`.

use mailcraft_model::{Contact, Id, Labeled};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::agent::{Access, Class, classify};
use crate::{Session, sample};

/// The sample mailbox on Wednesday 2026-10-14, 16:00 UTC.
fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn gil(s: &mut Session) -> Id {
    s.mailbox.add_contact(Contact {
        first: "Gil".into(),
        last: "Hart".into(),
        company: "Partner Works".into(),
        emails: vec![Labeled::new("work", "gil@partner.example")],
        ..Default::default()
    })
}

#[test]
fn forward_contact_attaches_the_card_and_sends_nothing() {
    let mut s = session();
    let id = gil(&mut s);
    let messages = s.mailbox.messages.len();
    run(&mut s, "people.forward", json!({"id": id}));
    let c = s.compose.as_ref().unwrap();
    assert_eq!((c.subject.as_str(), c.to.as_str()), ("Gil Hart", ""));
    let card = c.attachments.iter().find(|a| a.name == "Gil Hart.vcf").expect("the card goes along");
    assert_eq!(card.mime, "text/vcard");
    let text = String::from_utf8(card.data.to_vec()).unwrap();
    assert!(text.starts_with("BEGIN:VCARD") && text.contains("gil@partner.example") && text.contains("Partner Works"), "{text}");
    let back = mailcraft_vcard::parse(&text);
    assert_eq!(back.first().map(Contact::name).as_deref(), Some("Gil Hart"));
    assert_eq!(s.mailbox.messages.len(), messages);
}

#[test]
fn forward_contact_names_its_contact_strictly() {
    let mut s = session();
    let id = gil(&mut s);
    s.sel.contact = Some(id);
    for params in [json!({"id": "x"}), json!({"id": 999_999}), json!({"id": id, "to": "someone@partner.example"})] {
        assert!(s.run("people.forward", &params).is_err(), "{params}");
        assert!(s.compose.is_none(), "{params}");
    }
}

#[test]
fn forward_contact_saves_the_message_being_written_and_opens_beside_it() {
    let mut s = session();
    let id = gil(&mut s);
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"subject": "Unsaved", "body": "Half a thought"}));
    let steps = s.history.len();
    let r = run(&mut s, "people.forward", json!({"id": id}));
    // The owner's is saved to Drafts first (#286); the card goes on the new one.
    assert!(s.mailbox.messages.values().any(|m| m.draft && m.subject == "Unsaved"));
    assert!(s.composing.iter().any(|c| !c.dirty && c.subject == "Unsaved" && c.attachments.is_empty()));
    assert_eq!(s.compose_id(), r["compose"].as_u64());
    assert_eq!(s.compose.as_ref().unwrap().attachments.len(), 1);
    assert_eq!(s.history.len(), steps + 1);
}

#[test]
fn meeting_invites_the_contact_as_new_meeting_would() {
    let mut s = session();
    let id = gil(&mut s);
    let made = run(&mut s, "people.meeting", json!({"id": id, "title": "Catch up", "start": "2026-10-20T10:00", "attendees": "hal@partner.example"}));
    let e = s.mailbox.event(made["id"].as_u64().unwrap()).unwrap();
    let invited: Vec<&str> = e.attendees.iter().map(|a| a.address.email.as_str()).collect();
    assert_eq!(invited, ["gil@partner.example", "hal@partner.example"]);
    // Named already, the contact is not invited twice.
    let made = run(&mut s, "people.meeting", json!({"id": id, "title": "Again", "attendees": "Gil <gil@partner.example>"}));
    assert_eq!(s.mailbox.event(made["id"].as_u64().unwrap()).unwrap().attendees.len(), 1);
    // Without a title it is refused, as New Meeting refuses it.
    assert!(s.run("people.meeting", &json!({"id": id})).is_err());
    assert!(s.run("people.meeting", &json!({"id": id, "title": "x", "attendees": 3})).is_err());
}

#[test]
fn a_contact_without_an_address_has_no_meeting() {
    let mut s = session();
    let id = s.mailbox.add_contact(Contact { first: "Ivo".into(), ..Default::default() });
    assert!(s.run("people.meeting", &json!({"id": id, "title": "x"})).is_err());
}

#[test]
fn an_agent_is_asked_before_a_meeting_invites_by_mail() {
    let mut s = session();
    let id = gil(&mut s);
    assert_eq!(classify(&s, "people.meeting", &json!({"id": id, "title": "x"})), Access::Safe);
    assert_eq!(classify(&s, "people.meeting", &json!({"id": id, "title": "x", "send": true})), Access::Owner(Class::Send));
    assert_eq!(classify(&s, "people.forward", &json!({"id": id})), Access::Safe);
}
