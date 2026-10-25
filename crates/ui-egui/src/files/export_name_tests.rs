// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The name a save dialog suggests is that of what is saved: Save Message As of a message named
//! by `id` (as one that waited for its download is) suggests that message's subject, whatever is
//! selected now, and `message.eml` for one with no subject, as the command names it.

use std::sync::mpsc;

use mailcraft_engine::Session;
use serde_json::json;

use super::FileDialogRequest;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

#[test]
fn save_message_as_suggests_the_name_of_the_message_it_saves() {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    let (tx, asked) = mpsc::channel();
    app.services.file_dialog = Some(Box::new(move |request| {
        let _ = tx.send(request);
        mpsc::channel::<Option<String>>().1
    }));
    let mut subjects = app.session.mailbox.messages.values().filter(|m| !m.subject.is_empty()).map(|m| (m.id, m.subject.clone()));
    let (saved, subject) = subjects.next().unwrap();
    let (selected, other) = subjects.find(|(_, s)| *s != subject).unwrap();
    app.run("mail.select", json!({"id": selected})).unwrap();

    app.run("file.exportMessage", json!({"id": saved})).unwrap();
    let want = mailcraft_engine::opening::export_name(&subject, "eml");
    assert_eq!(asked.try_recv().unwrap(), FileDialogRequest::Save { name: want });

    // Named by nothing, it is the selected message's.
    app.file_dialog = None;
    app.run("file.exportMessage", json!({})).unwrap();
    let want = mailcraft_engine::opening::export_name(&other, "eml");
    assert_eq!(asked.try_recv().unwrap(), FileDialogRequest::Save { name: want });

    // With no subject, or none a file name can keep, it is `message.eml`, as Forward as Attachment
    // and the command name it.
    for subject in ["", "  ", "???", "\u{200b}"] {
        app.session.mailbox.message_mut(saved).unwrap().subject = subject.into();
        app.file_dialog = None;
        app.run("file.exportMessage", json!({"id": saved})).unwrap();
        assert_eq!(asked.try_recv().unwrap(), FileDialogRequest::Save { name: "message.eml".into() }, "{subject:?}");
    }
}
