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
        mpsc::channel::<Option<Vec<String>>>().1
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

/// A name typed over the suggestion without an extension gets the suggestion's (a Linux save
/// dialog keeps it as typed), but never over a file the dialog didn't ask about.
#[test]
fn a_name_typed_without_an_extension_is_saved_with_the_suggested_one() {
    let dir = std::env::temp_dir().join(format!("mailcraft-export-ext-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let typed = dir.join("v6").to_string_lossy().into_owned();
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    let answer = typed.clone();
    app.services.file_dialog = Some(Box::new(move |_| {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(Some(vec![answer.clone()]));
        rx
    }));
    let id = *app.session.mailbox.messages.keys().next().unwrap();
    app.run("file.exportMessage", json!({"id": id})).unwrap();
    assert!(dir.join("v6.eml").is_file());
    assert!(!dir.join("v6").exists());

    // `v6.eml` is there now: a second `v6` is saved as typed.
    app.run("file.exportMessage", json!({"id": id})).unwrap();
    assert!(dir.join("v6").is_file());

    // A name with an extension, or a folder's path, is left alone.
    assert_eq!(super::with_extension(format!("{typed}.txt"), "eml"), format!("{typed}.txt"));
    assert_eq!(super::with_extension("/".into(), "eml"), "/");
    let _ = std::fs::remove_dir_all(&dir);
}
