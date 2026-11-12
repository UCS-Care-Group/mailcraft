// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::Arc;
use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_model::{Folder, FolderKind, Mailbox, Message};
use serde_json::json;

use crate::control::{Outcome, handle};
use crate::{ControlRequest, MailApp, Services};

#[test]
fn window_and_agent_use_the_same_read_only_suggestions() {
    let mut mailbox = Mailbox::default();
    mailbox.folders.extend([
        Folder { id: 10, account: 1, kind: FolderKind::Inbox, ..Folder::default() },
        Folder { id: 11, account: 1, kind: FolderKind::Custom, parent: Some(10), ..Folder::default() },
    ]);
    for (id, folder) in [(100, 10), (101, 11)] {
        let mut message = Message { id, account: 1, folder, subject: "Project ABC123".into(), ..Message::default() };
        message.from.email = "sender@example.org".into();
        mailbox.messages.insert(id, Arc::new(message));
    }
    let mut app = MailApp::new(Session::new(mailbox), Services::default());
    let ctx = egui::Context::default();
    let (request, _) = ControlRequest::new("engine.execute", json!({"command": "mail.suggestFolders"}));
    let Outcome::Done(refusal) = handle(&mut app, &ctx, &request.from_agent(vec![])) else { panic!("the agent must name a message") };
    assert_eq!(refusal["ok"], false);

    let params = json!({"id": 100});
    let expected = app.session.run("mail.suggestFolders", &params).unwrap();
    assert_eq!(expected, json!({"suggestions": [{"folder": 11, "reason": "senderAndSubject"}]}));
    let (request, response) = ControlRequest::new("engine.execute", json!({"command": "mail.suggestFolders", "params": params}));
    assert!(matches!(handle(&mut app, &ctx, &request.from_agent(vec![])), Outcome::Pending));
    assert_eq!(response.recv_timeout(Duration::from_secs(5)).unwrap(), json!({"ok": true, "result": expected}));
    assert_eq!(app.session.mailbox.message(100).unwrap().folder, 10);
}
