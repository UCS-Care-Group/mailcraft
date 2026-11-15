// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use serde_json::json;

use crate::{Session, sample};

// 2026-10-14 16:00:00 UTC
const NOW: i64 = 1_791_993_600;

fn sample_session() -> Session {
    Session::at(sample::mailbox(NOW, 0), NOW, 0)
}

#[test]
fn create_from_compose_list_insert_with_fields_and_remove() {
    let mut s = sample_session();

    // Compose a message that serves as the template source.
    s.run(
        "mail.new",
        &json!({
            "subject": "Welcome to {recipient.company}",
            "body": "Hi {recipient.name},\n\nGreat connecting on {date}.\nKeep: {unknown.field}"
        }),
    )
    .unwrap();

    let created = s.run("template.create", &json!({"name": "Intro Note"})).unwrap();
    let tid = created["id"].as_u64().unwrap();
    assert_eq!(created["name"], "Intro Note");
    assert_eq!(created["subject"], "Welcome to {recipient.company}");

    let list = s.run("template.list", &json!({})).unwrap();
    let templates = list["templates"].as_array().unwrap();
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0]["id"], tid);

    // Discard draft and start a fresh message to a known sample contact (Lin Zhao at Harborworks).
    s.run("compose.discard", &json!({})).unwrap();
    s.run("mail.new", &json!({"to": "lin.zhao@harborworks.example"})).unwrap();

    let inserted = s.run("template.insert", &json!({"id": tid})).unwrap();
    assert_eq!(inserted["subject"], "Welcome to Harborworks");
    assert_eq!(inserted["body"], "Hi Lin Zhao,\n\nGreat connecting on October 14, 2026.\nKeep: {unknown.field}");
    let compose = s.compose.as_ref().unwrap();
    assert_eq!(compose.subject, "Welcome to Harborworks");
    assert!(compose.body.plain().starts_with("Hi Lin Zhao,\n\nGreat connecting on October 14, 2026.\nKeep: {unknown.field}"));

    // Inserting into a message that already has a subject keeps the existing subject unless replaceSubject is set.
    s.run("compose.set", &json!({"subject": "Re: Contract", "body": ""})).unwrap();
    s.run("template.insert", &json!({"name": "Intro Note"})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().subject, "Re: Contract");

    // Removing template works and missing id errors cleanly.
    s.run("template.remove", &json!({"id": tid})).unwrap();
    assert!(s.run("template.remove", &json!({"id": tid})).is_err());
    assert!(s.run("template.insert", &json!({"id": tid})).is_err());
}

#[test]
fn create_without_compose_requires_content_and_accepts_explicit_params() {
    let mut s = sample_session();
    assert!(s.run("template.create", &json!({"name": "Empty"})).is_err());

    let res = s.run("template.create", &json!({"name": "Status", "subject": "Status {date}", "body": "Update for {recipient.name}."})).unwrap();
    assert_eq!(res["name"], "Status");
    assert_eq!(s.mailbox.templates().len(), 1);
}

#[test]
fn template_insert_requires_compose_and_handles_missing_recipient_safely() {
    let mut s = sample_session();
    let created = s.run("template.create", &json!({"name": "Checkin", "body": "Hi {recipient.name} ({recipient.company}) on {date}."})).unwrap();
    let tid = created["id"].as_u64().unwrap();

    // Disabled when no message is being written.
    assert!(s.run("template.insert", &json!({"id": tid})).is_err());

    // Open compose with unknown recipient email (no display name, no contact).
    s.run("mail.new", &json!({"to": "stranger@example.org"})).unwrap();
    let inserted = s.run("template.insert", &json!({"id": tid})).unwrap();
    assert_eq!(inserted["body"], "Hi  () on October 14, 2026.");
}

#[test]
fn hostile_template_inputs_are_bounded_and_never_panic() {
    let mut s = sample_session();
    let huge_name = "🦀".repeat(mailcraft_model::MAX_TEMPLATE_NAME + 50);
    let huge_body = "x".repeat(mailcraft_model::MAX_TEMPLATE_BODY + 500);
    let created = s
        .run(
            "template.create",
            &json!({
                "name": format!("Bad\x00\x07\x1b{huge_name}"),
                "subject": "Sub\r\nBcc: evil@example.org",
                "body": format!("Hi\x00\x08 {huge_body}")
            }),
        )
        .unwrap();
    let name = created["name"].as_str().unwrap();
    assert!(!name.chars().any(char::is_control));
    assert!(name.chars().count() <= mailcraft_model::MAX_TEMPLATE_NAME);
    let subject = created["subject"].as_str().unwrap();
    assert!(!subject.contains(['\r', '\n']));
    let body = created["body"].as_str().unwrap();
    assert!(body.chars().count() <= mailcraft_model::MAX_TEMPLATE_BODY);
    assert!(!body.contains('\x00'));

    // Contact whose name contains template syntax is never re-evaluated.
    let mut contact = mailcraft_model::Contact {
        first: "Mallory".into(),
        last: "{date}".into(),
        company: "{recipient.name}".into(),
        emails: vec![mailcraft_model::Labeled::new("work", "mallory@example.org")],
        ..Default::default()
    };
    contact.display_name = "Mallory {date}".into();
    s.mailbox.add_contact(contact);

    let tpl = s.run("template.create", &json!({"name": "RecursiveCheck", "body": "{recipient.name} @ {recipient.company} on {date}"})).unwrap();
    s.run("mail.new", &json!({"to": "mallory@example.org"})).unwrap();
    let ins = s.run("template.insert", &json!({"id": tpl["id"]})).unwrap();
    assert_eq!(ins["body"], "Mallory {date} @ {recipient.name} on October 14, 2026");
}

#[test]
fn create_keeps_text_after_mid_message_signature_and_rejects_duplicate_names() {
    let mut s = sample_session();

    // Signature in the middle: text before and after are kept, while only the signature block is removed.
    s.run("signature.set", &json!({"name": "Work", "text": "-- \nAlice Example"})).unwrap();
    s.run("mail.new", &json!({"subject": "Mid-sig", "body": "Opening paragraph."})).unwrap();
    let mid = s.compose.as_ref().unwrap().body.text().len();
    s.run("compose.select", &json!({"from": mid})).unwrap();
    s.run("compose.signature", &json!({"name": "Work"})).unwrap();
    let end = s.compose.as_ref().unwrap().body.text().len();
    s.run("compose.select", &json!({"from": end})).unwrap();
    s.run("compose.type", &json!({"text": "\nP.S. Text written after the signature."})).unwrap();

    let created = s.run("template.create", &json!({"name": "MidSigTpl"})).unwrap();
    let body = created["body"].as_str().unwrap();
    assert!(body.contains("Opening paragraph."), "{body:?}");
    assert!(body.contains("P.S. Text written after the signature."), "{body:?}");
    assert!(!body.contains("Alice Example"), "{body:?}");

    // Creating a second template with the same name (ignoring case and whitespace) is refused
    // and names the existing template.
    let err = s.run("template.create", &json!({"name": " midsigtpl ", "body": "Other"})).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("MidSigTpl"), "{msg}");
}
