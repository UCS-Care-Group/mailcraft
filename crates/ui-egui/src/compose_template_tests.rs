// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_engine::Session;
use serde_json::json;

use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default())
}

#[test]
fn save_template_dialog_creates_and_inserts_with_fields() {
    let mut a = app();
    a.run("mail.new", json!({"subject": "Follow-up", "body": "Hi {recipient.name},\nCompany: {recipient.company}\nDate: {date}"})).unwrap();

    // Opening template.create without a name opens the SaveTemplate dialog.
    let _ = a.run("template.create", json!({}));
    assert_eq!(a.dialog.as_ref().map(|d| d.name()), Some("saveTemplate"));

    // Submitting via execute saves the template and clears dialog.
    let created = a.execute("template.create", json!({"name": "Follow-up Note"})).unwrap();
    let tid = created["id"].as_u64().unwrap();
    a.execute("ui.closeDialog", json!({})).unwrap();
    assert_eq!(a.session.mailbox.templates().len(), 1);

    // Start a fresh message to a known contact and insert the template.
    a.execute("compose.discard", json!({})).unwrap();
    a.execute("mail.new", json!({"to": "lin.zhao@harborworks.example"})).unwrap();
    a.run("template.insert", json!({"id": tid})).unwrap();

    let c = a.session.compose.as_ref().unwrap();
    assert_eq!(c.subject, "Follow-up");
    assert!(c.body.plain().starts_with("Hi Lin Zhao,\nCompany: Harborworks\nDate: October 14, 2026"));
}
