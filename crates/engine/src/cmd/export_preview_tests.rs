// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Save As and Export never write a preview as if it were the whole message (#653): a message of
//! which only the headers and a preview are here is refused, and a folder with such messages is
//! refused with how many there are, so a folder exported as a backup is never a pile of stubs.

use mailcraft_model::{Body, Id};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

use crate::{Session, sample};

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

/// An inbox message turned into a preview that can't be fetched now (no UID after a move).
fn preview(s: &mut Session) -> Id {
    let inbox = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, mailcraft_model::FolderKind::Inbox).unwrap();
    let id = s.mailbox.messages_in(inbox).find(|m| !m.draft && !m.attachments.is_empty()).unwrap().id;
    let m = s.mailbox.message_mut(id).unwrap();
    (m.unfetched, m.uid, m.server) = (true, 0, None);
    m.attachments.clear();
    m.body = Body { text: "Only the first 255 characters".into(), html: None };
    id
}

#[test]
fn a_preview_is_not_saved_as_a_message() {
    let mut s = session();
    let id = preview(&mut s);
    let e = s.run("file.exportMessage", &json!({"id": id})).unwrap_err().to_string();
    assert!(e.contains("isn't downloaded yet"), "{e}");
}

#[test]
fn a_folder_with_previews_is_refused_with_how_many() {
    let mut s = session();
    let id = preview(&mut s);
    let folder = s.mailbox.message(id).unwrap().folder;
    let e = s.run("file.exportFolder", &json!({"id": folder})).unwrap_err().to_string();
    assert!(e.contains("1 of the messages") && e.contains("aren't downloaded yet"), "{e}");
    // Downloaded, it goes.
    s.mailbox.message_mut(id).unwrap().unfetched = false;
    assert!(s.run("file.exportFolder", &json!({"id": folder})).is_ok());
}

#[test]
fn whole_messages_are_exported_as_before() {
    let mut s = session();
    let id = s.mailbox.messages.values().find(|m| !m.draft && !m.attachments.is_empty()).unwrap().id;
    assert!(s.run("file.exportMessage", &json!({"id": id})).is_ok());
}
