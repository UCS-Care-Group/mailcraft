// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Resend Message (a sent message opened again as a new one) and Direct Replies To (the
//! `Reply-To` line of the message being written).

use mailcraft_model::{FolderKind, Id};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Session, sample};

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn session() -> Session {
    Session::at(sample::mailbox(now(), 0), now(), 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn in_folder(s: &Session, kind: FolderKind, subject: &str) -> Id {
    let folder = s.mailbox.folders.iter().find(|f| f.kind == kind).unwrap().id;
    s.mailbox.messages.values().find(|m| m.folder == folder && m.subject == subject).unwrap().id
}

#[test]
fn a_sent_message_opens_again_as_a_new_one_and_the_original_stays() {
    let mut s = session();
    let id = in_folder(&s, FolderKind::Sent, "Lisbon conference travel");
    let before = s.mailbox.messages.len();
    let r = run(&mut s, "mail.resend", json!({"id": id}));
    assert_eq!(r["from"], json!(id));
    let c = s.compose.as_ref().unwrap();
    assert_eq!(c.subject, "Lisbon conference travel");
    assert!(c.to.contains("travel@lumen.example"), "{}", c.to);
    assert!(c.body.plain().contains("Lisbon conference"));
    assert!(c.draft.is_none(), "a new message, not the original");
    assert_eq!(s.mailbox.messages.len(), before);
    assert!(s.mailbox.message(id).is_some());
}

#[test]
fn only_the_owners_own_sent_mail_is_resent() {
    let mut s = session();
    let draft = in_folder(&s, FolderKind::Drafts, "RE: Feedback on the onboarding flow");
    assert!(s.run("mail.resend", &json!({"id": draft})).is_err());
    let theirs = in_folder(&s, FolderKind::Archive, "Budget for next year: first pass");
    assert!(s.run("mail.resend", &json!({"id": theirs})).is_err());
    assert!(s.compose.is_none());
}

#[test]
fn an_agent_names_the_message_it_resends() {
    let mut s = session();
    let id = in_folder(&s, FolderKind::Sent, "Lisbon conference travel");
    s.sel.messages = vec![id];
    assert!(matches!(crate::agent::check(&s, "mail.resend", &json!({}), &[]), crate::agent::Verdict::Refuse(_)));
}

#[test]
fn direct_replies_to_checks_each_address_and_strips_line_breaks() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    assert!(s.run("compose.replyTo", &json!({"value": "not an address"})).is_err());
    assert!(s.run("compose.replyTo", &json!({"value": "a@example.org", "extra": 1})).is_err());
    let many: Vec<String> = (0..21).map(|n| format!("p{n}@example.org")).collect();
    assert!(s.run("compose.replyTo", &json!({"value": many.join(", ")})).is_err());
    assert_eq!(s.compose.as_ref().unwrap().reply_to, "");

    run(&mut s, "compose.replyTo", json!({"value": "Desk <desk@example.org>\r\nBcc: evil@example.org"}));
    let line = s.compose.as_ref().unwrap().reply_to.clone();
    assert!(!line.contains(['\r', '\n', '\0']), "{line:?}");
    assert!(line.contains("desk@example.org"), "{line}");

    // Clearing it is allowed.
    run(&mut s, "compose.replyTo", json!({"value": ""}));
    assert_eq!(s.compose.as_ref().unwrap().reply_to, "");
}

#[test]
fn the_saved_message_carries_reply_to_and_opens_again_with_it() {
    let mut s = session();
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"to": "ada@example.org", "subject": "Replies elsewhere", "body": "Hello"}));
    run(&mut s, "compose.replyTo", json!({"value": "desk@example.org"}));
    run(&mut s, "compose.saveDraft", json!({}));
    let draft = s.compose.as_ref().unwrap().draft.unwrap();
    let saved = s.loaded_message(draft).unwrap();
    assert_eq!(saved.reply_to.len(), 1);
    assert_eq!(saved.reply_to[0].email, "desk@example.org");
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    assert!(s.compose.as_ref().unwrap().reply_to.contains("desk@example.org"));
}
