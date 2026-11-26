// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A reply kept as a draft shows in the conversation it answers, in the list and in the reading pane.

use mailcraft_time::{Date, DateTime};
use serde_json::json;

use crate::{Session, sample};

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    s.run("view.conversations", &json!({"value": true})).unwrap();
    s
}

/// Reply to the sample's "Release checklist", save it, and take the headers off the draft as a
/// server's copy of it may come back without them (#1922): its place in the conversation is
/// what the draft knows of the message it answers.
fn headerless_draft(s: &mut Session) -> (u64, u64) {
    let root = s.mailbox.messages.values().find(|m| m.subject == "Release checklist").map(|m| m.id).unwrap();
    s.run("mail.reply", &json!({"id": root})).unwrap();
    let draft = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    let m = s.mailbox.message_mut(draft).unwrap();
    assert!(m.answers.is_some());
    (m.in_reply_to, m.references) = (String::new(), Vec::new());
    (root, draft)
}

#[test]
fn a_reply_saved_as_a_draft_is_in_its_conversation_without_headers() {
    let mut s = session();
    let (root, draft) = headerless_draft(&mut s);
    assert!(s.conversation(root).contains(&draft), "reading pane");
    let rows = s.rows();
    let row = rows.iter().find(|r| r.members.contains(&root)).unwrap();
    assert!(row.others.contains(&draft), "list: {:?}", row.others);
    assert!(!row.members.contains(&draft), "a draft of Drafts is no member of an Inbox conversation");
}

#[test]
fn deleting_the_inbox_conversation_leaves_the_draft() {
    let mut s = session();
    let (root, draft) = headerless_draft(&mut s);
    let row = s.rows().iter().find(|r| r.members.contains(&root)).cloned().unwrap();
    s.run("mail.delete", &json!({"ids": row.members})).unwrap();
    let m = s.mailbox.message(draft).unwrap();
    assert!(m.draft && s.mailbox.folder(m.folder).is_some_and(|f| f.kind == mailcraft_model::FolderKind::Drafts));
}
