// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A folder exported to an .mbox and handed back to the window (#1401): dropped on it, or
//! double-clicked so that the system hands it over, it is asked about first, with how many
//! messages and where they go; and what is already in the mailbox doesn't come in again.

use std::path::PathBuf;

use mailcraft_engine::Session;
use mailcraft_model::{Address, Mailbox, Message};
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

fn app() -> MailApp {
    let mut mb = Mailbox::with_local_account("", "");
    let inbox = mb.inbox().unwrap();
    for n in 1..=3u32 {
        mb.add_message(Message {
            folder: inbox,
            message_id: format!("m{n}@example.org"),
            subject: format!("Plans {n}"),
            from: Address::new("Bo", "bo@example.org"),
            to: vec![Address::new("Cy", "cy@example.org")],
            date: 1_700_000_000 + i64::from(n) * 60,
            ..Default::default()
        })
        .unwrap();
    }
    let mut a = MailApp::new(Session::new(mb), Services::default());
    a.autosave = false;
    a.session.view.folder = inbox;
    a
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-mail-drop-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The question on screen about importing `path`, answered Import.
fn import_on_the_owners_word(a: &mut MailApp, path: &str) -> String {
    let Some(Dialog::Confirm { command, params, ask }) = &a.dialog else { panic!("asked first") };
    assert_eq!(command, "file.import");
    assert_eq!(params["path"], json!(path));
    let text = ask.text.clone();
    let (command, params) = (command.clone(), crate::dialogs::answered(params, &Value::Null, None));
    a.dialog = None;
    crate::dialogs::go_ahead(a, &command, params);
    text
}

#[test]
fn an_exported_folder_handed_back_twice_is_asked_about_and_not_doubled() {
    let dir = scratch("twice");
    let mbox = dir.join("Inbox.mbox");
    let mut a = app();
    let inbox = a.session.view.folder;
    a.run("file.exportFolder", json!({"id": inbox, "path": mbox.to_string_lossy()})).unwrap();
    let before = a.session.mailbox.messages.len();
    let path = mbox.to_string_lossy().to_string();

    // Dropped on the window: asked, nothing in yet.
    a.open_path(&path);
    assert_eq!(a.session.mailbox.messages.len(), before, "nothing is imported before the owner says so");
    let text = import_on_the_owners_word(&mut a, &path);
    assert!(text.contains("Import 3 messages from Inbox.mbox into Inbox?"), "{text}");
    assert!(!text.contains(&dir.to_string_lossy().to_string()), "the file's name, never its path: {text}");
    assert_eq!(a.session.mailbox.messages.len(), before, "what is already here doesn't come in again");

    // Double-clicked: the system hands it over, asked again, and still nothing doubles.
    a.open_from_system(&path);
    import_on_the_owners_word(&mut a, &path);
    assert_eq!(a.session.mailbox.messages.len(), before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_question_names_where_the_mail_really_goes() {
    let dir = scratch("where");
    let eml = dir.join("one.eml");
    std::fs::write(&eml, "From: Bo <bo@example.org>\r\nSubject: One\r\n\r\nHi\r\n").unwrap();
    let path = eml.to_string_lossy().to_string();
    let mut a = app();
    let asked = |a: &MailApp| super::import_question(a, &path).text;
    assert!(asked(&a).contains("into Inbox?"), "{}", asked(&a));
    // An Outbox in view: its account's Drafts.
    let local = a.session.mailbox.local_account();
    if let Some(outbox) = a.session.mailbox.folder_of_kind(local, mailcraft_model::FolderKind::Outbox) {
        a.session.view.folder = outbox;
        let drafts = a.session.mailbox.folder_of_kind(local, mailcraft_model::FolderKind::Drafts).unwrap();
        let name = crate::nav::folder_name(a.session.mailbox.folder(drafts).unwrap()).to_string();
        assert!(asked(&a).contains(&format!("into {name}?")), "{}", asked(&a));
    }
    // A server's folder: this computer's Imported.
    let imap = a.run("account.add", json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    a.session.view.folder = a.session.mailbox.folder_of_kind(imap, mailcraft_model::FolderKind::Inbox).unwrap();
    assert!(asked(&a).contains("into Imported?"), "{}", asked(&a));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_command_imports_without_a_question() {
    let mut a = app();
    let eml = "From: Bo <bo@example.org>\r\nTo: cy@example.org\r\nSubject: New\r\nMessage-ID: <new@example.org>\r\nDate: Tue, 14 Nov 2023 22:13:20 +0000\r\n\r\nHello\r\n";
    let before = a.session.mailbox.messages.len();
    a.run("file.import", json!({"name": "new.eml", "data": mailcraft_engine::cmd::base64_encode(eml.as_bytes())})).unwrap();
    assert!(a.dialog.is_none());
    assert_eq!(a.session.mailbox.messages.len(), before + 1);
}

#[test]
fn an_export_says_how_much_it_saved_and_the_files_name_alone() {
    let said = super::saved_said("file.exportFolder", &json!({"messages": 2071}), "Inbox.mbox");
    assert_eq!(said, "Saved 2071 messages to Inbox.mbox");
    assert_eq!(super::saved_said("file.exportFolder", &json!({"messages": 1}), "Inbox.mbox"), "Saved 1 message to Inbox.mbox");
    assert_eq!(super::saved_said("file.exportMessage", &json!({}), "Plans.eml"), "Saved the message to Plans.eml");
    assert_eq!(super::file_name("/home/someone/mail/Inbox.mbox"), "Inbox.mbox");
}
