// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message dropped on Calendar, Tasks or People (#873): what the new item starts with, made
//! safe, and nothing changed by asking.

use mailcraft_model::{Address, Body, FolderKind, Id, Message};
use serde_json::{Value, json};

use super::item_from_message::{MAX_TEXT, clean_text};
use crate::Session;
use crate::agent::{Access, classify};

/// Wednesday 2026-10-14 16:10 UTC.
const NOW: i64 = 1_791_993_600 + 600;

fn session() -> Session {
    Session::at(crate::sample::mailbox(NOW, 0), NOW, 0)
}

/// A message in the first account's Inbox.
fn message(s: &mut Session, subject: &str, body: Body) -> Id {
    let account = s.mailbox.accounts[0].id;
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let m = Message { account, folder, subject: subject.into(), from: Address::new("Robin Vale", "robin@example.org"), body, ..Default::default() };
    s.mailbox.add_message(m).unwrap()
}

fn run(s: &mut Session, params: Value) -> Value {
    s.run("mail.itemFromMessage", &params).unwrap_or_else(|e| panic!("{params}: {e}"))
}

#[test]
fn an_appointment_has_the_subject_the_text_and_the_next_half_hour() {
    let mut s = session();
    let html = "<p>Agenda:</p><ul><li>Budget</li><li>Plan</li></ul><script>alert(1)</script><div style=\"display:none\">secret</div>";
    let id = message(&mut s, "RE: Planning", Body { text: "the sender's other text".into(), html: Some(html.into()) });
    let got = run(&mut s, json!({"id": id, "as": "appointment"}));
    assert_eq!(got["title"], "Planning");
    let text = got["description"].as_str().unwrap();
    // What the owner saw: the HTML read as text, never the alternative beside it, nothing hidden or run.
    assert!(text.contains("Agenda:") && text.contains("Budget") && text.contains("Plan"), "{text}");
    assert!(!text.contains("other text") && !text.contains("alert") && !text.contains("secret"), "{text}");
    // 16:10 today: from 16:30 to 17:00.
    assert_eq!((got["start"].as_str(), got["end"].as_str()), (Some("2026-10-14T16:30"), Some("2026-10-14T17:00")));
}

#[test]
fn a_task_has_the_text_and_goes_to_the_messages_account_when_it_keeps_to_dos() {
    let mut s = session();
    let id = message(&mut s, "Send the report", Body { text: "By Friday,\r\nplease.".into(), html: None });
    let got = run(&mut s, json!({"id": id, "as": "task"}));
    assert_eq!((got["title"].as_str(), got["notes"].as_str()), (Some("Send the report"), Some("By Friday,\nplease.")));
    // The sample's account keeps no to-dos: where a new task goes.
    assert_eq!(got.get("account").and_then(Value::as_u64), super::tasks::new_task_place(&s));
    // An account that keeps them has the task of its own message.
    let work =
        s.run("account.add", &json!({"email": "work@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap()["id"]
            .as_u64()
            .unwrap();
    s.mailbox.message_mut(id).unwrap().account = work;
    assert_eq!(run(&mut s, json!({"id": id, "as": "task"}))["account"], json!(work));
}

#[test]
fn a_contact_is_the_sender() {
    let mut s = session();
    let id = message(&mut s, "Hello", Body::default());
    assert_eq!(run(&mut s, json!({"id": id, "as": "contact"})), json!({"first": "Robin", "last": "Vale", "email": "robin@example.org"}));
    s.mailbox.message_mut(id).unwrap().from = Address::new("", "not an address");
    assert!(s.run("mail.itemFromMessage", &json!({"id": id, "as": "contact"})).is_err());
    // An address cleaning would change is refused, never altered.
    s.mailbox.message_mut(id).unwrap().from = Address::new("Robin", "robin\u{202e}@example.org");
    assert!(s.run("mail.itemFromMessage", &json!({"id": id, "as": "contact"})).is_err());
}

#[test]
fn hostile_text_loses_controls_and_turns_and_is_cut_on_a_character() {
    let mut s = session();
    let evil = format!("a\u{0}b\u{1b}[31mc\u{202e}d\u{2066}e\u{7}\tf\u{200d}g\r{}", "ü".repeat(MAX_TEXT * 2));
    let id = message(&mut s, "Sub\u{202e}ject\u{0}\r\nline", Body { text: evil, html: None });
    let got = run(&mut s, json!({"id": id, "as": "task"}));
    let notes = got["notes"].as_str().unwrap();
    assert!(notes.starts_with("ab[31mcde\tf\u{200d}g\nüü"), "{:?}", notes.get(..30));
    assert_eq!(notes.chars().count(), MAX_TEXT);
    assert!(!notes.chars().any(|c| (c.is_control() && c != '\n' && c != '\t') || matches!(c, '\u{202e}' | '\u{2066}')));
    // The title: one line, nothing that turns or hides.
    assert_eq!(got["title"], "Subject line");
    assert_eq!(clean_text("  \u{0}x\r\n\ry  ", 10), "x\n\ny");
    assert_eq!(clean_text("äöü", 2), "äö");
}

#[test]
fn asking_changes_nothing() {
    let mut s = session();
    let id = message(&mut s, "Keep me", Body { text: "Body".into(), html: None });
    s.sel.messages = vec![id];
    let (mailbox, sel, module) = (s.mailbox.clone(), s.sel.clone(), s.view.module);
    for kind in ["appointment", "task", "contact"] {
        // The selected message, when none is named.
        assert_eq!(run(&mut s, json!({"as": kind}))["title"].as_str().is_some(), kind != "contact");
        assert!(s.mailbox == mailbox, "{kind}");
        assert_eq!((&s.sel, s.view.module), (&sel, module), "{kind}");
    }
    assert!(!s.can_undo());
    // Asked wrongly: refused.
    for wrong in [json!({"id": id}), json!({"id": id, "as": "meeting"}), json!({"id": 0, "as": "task"}), json!({"id": 9_999_999, "as": "task"})] {
        assert!(s.run("mail.itemFromMessage", &wrong).is_err(), "{wrong}");
    }
    // An agent may ask: it reads, and makes nothing.
    assert_eq!(classify(&s, "mail.itemFromMessage", &json!({"id": id, "as": "task"})), Access::Safe);
}
