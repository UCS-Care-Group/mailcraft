// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message of which only the headers and a preview are here (#653) is never answered or
//! forwarded as if it were whole: Reply, Reply All and Forward refuse until it is downloaded,
//! also when it can't be fetched right now (a move within the account sets its UID to 0 until
//! the next Send/Receive, so the window has nothing to wait for).

use mailcraft_model::{Body, Id};
use serde_json::json;

use crate::{Session, sample};

fn session() -> Session {
    let now = mailcraft_time::DateTime::new(mailcraft_time::Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

/// A sample message turned into a preview that can't be fetched: no server id, no UID.
fn preview(s: &mut Session) -> Id {
    let id = s.mailbox.messages.values().find(|m| !m.draft && !m.attachments.is_empty()).unwrap().id;
    let m = s.mailbox.message_mut(id).unwrap();
    m.unfetched = true;
    m.uid = 0;
    m.server = None;
    m.attachments.clear();
    m.body = Body { text: "Only the first 255 characters".into(), html: None };
    id
}

#[test]
fn a_preview_is_neither_answered_nor_forwarded() {
    let mut s = session();
    let id = preview(&mut s);
    assert!(!s.mailbox.fetchable(s.mailbox.message(id).unwrap()), "nothing to wait for");
    for command in ["mail.reply", "mail.replyAll", "mail.forward", "mail.replyAndDelete"] {
        let e = s.run(command, &json!({"id": id})).unwrap_err().to_string();
        assert!(e.contains("isn't downloaded yet"), "{command}: {e}");
        assert!(s.compose.is_none(), "{command}: nothing is being written");
    }
}

#[test]
fn a_message_downloaded_is_answered_as_before() {
    let mut s = session();
    let id = preview(&mut s);
    s.mailbox.message_mut(id).unwrap().unfetched = false;
    s.run("mail.reply", &json!({"id": id})).unwrap();
    assert!(s.compose.is_some());
}
