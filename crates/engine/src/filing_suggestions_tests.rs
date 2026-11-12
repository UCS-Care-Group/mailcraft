// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::Arc;

use mailcraft_model::{Folder, FolderKind, Id, Mailbox, Message};

use super::{Reason, Scan, Suggestion};

fn suggest(mailbox: &Mailbox, id: Id) -> Vec<Suggestion> {
    Scan::new(mailbox, id).map_or_else(Vec::new, Scan::finish)
}

fn folder(id: Id, account: Id, kind: FolderKind, parent: Option<Id>) -> Folder {
    Folder { id, account, kind, parent, ..Folder::default() }
}

fn message(mailbox: &mut Mailbox, id: Id, account: Id, folder: Id, email: &str, subject: &str, message_id: &str) {
    let mut m = Message { id, account, folder, subject: subject.into(), message_id: message_id.into(), ..Message::default() };
    m.from.email = email.into();
    mailbox.messages.insert(id, Arc::new(m));
}

fn mailbox() -> Mailbox {
    let mut mailbox = Mailbox::default();
    mailbox.folders.extend([
        folder(10, 1, FolderKind::Inbox, None),
        folder(11, 1, FolderKind::Custom, Some(10)),
        folder(12, 1, FolderKind::Custom, Some(10)),
        folder(13, 1, FolderKind::Junk, None),
        folder(14, 1, FolderKind::Custom, Some(13)),
        folder(20, 2, FolderKind::Custom, None),
    ]);
    mailbox
}

#[test]
fn project_and_sender_evidence_outranks_sender_frequency() {
    let mut m = mailbox();
    message(&mut m, 100, 1, 10, "sales@example.com", "R: Preventivo ABC123", "new");
    message(&mut m, 101, 1, 11, "SALES@example.com", "I: Scheda tecnica ABC123", "project");
    message(&mut m, 102, 1, 12, "sales@example.com", "Old work", "old-1");
    message(&mut m, 103, 1, 12, "sales@example.com", "Another job", "old-2");
    assert_eq!(suggest(&m, 100), [Suggestion { folder: 11, reason: Reason::SenderAndSubject }, Suggestion { folder: 12, reason: Reason::Sender }]);
    assert_eq!(m.message(100).map(|msg| msg.folder), Some(10));
}

#[test]
fn repeated_subject_can_match_different_senders() {
    let mut m = mailbox();
    message(&mut m, 100, 1, 10, "a@example.com", "R: Verbale sopralluogo", "new");
    message(&mut m, 101, 1, 11, "b@example.com", "Verbale sopralluogo", "one");
    message(&mut m, 102, 1, 11, "c@example.com", "I: Verbale sopralluogo", "two");
    assert_eq!(suggest(&m, 100), [Suggestion { folder: 11, reason: Reason::Subject }]);
}

#[test]
fn duplicate_history_weak_topics_and_other_accounts_are_not_suggested() {
    let mut m = mailbox();
    message(&mut m, 100, 1, 10, "a@example.com", "R: Grazie", "new");
    message(&mut m, 101, 1, 11, "a@example.com", "Grazie", "duplicate");
    message(&mut m, 102, 1, 11, "a@example.com", "Grazie", "duplicate");
    message(&mut m, 103, 1, 11, "b@example.com", "Grazie", "different");
    message(&mut m, 104, 1, 14, "a@example.com", "Grazie", "junk-1");
    message(&mut m, 105, 1, 14, "a@example.com", "Grazie", "junk-2");
    message(&mut m, 106, 2, 20, "a@example.com", "Grazie", "other-1");
    message(&mut m, 107, 2, 20, "a@example.com", "Grazie", "other-2");
    assert!(suggest(&m, 100).is_empty());
}

#[test]
fn an_even_split_and_the_messages_current_folder_are_not_suggested() {
    let mut m = mailbox();
    message(&mut m, 100, 1, 11, "a@example.com", "New", "new");
    for (id, target) in [(101, 11), (102, 11), (103, 12), (104, 12)] {
        message(&mut m, id, 1, target, "a@example.com", "Old", "");
    }
    assert!(suggest(&m, 100).is_empty());
}

#[test]
fn junk_sources_and_junk_or_deleted_destinations_are_ignored() {
    let mut m = mailbox();
    m.folders.extend([folder(15, 1, FolderKind::Deleted, None), folder(16, 1, FolderKind::Custom, Some(15))]);
    for (id, target) in [(100, 10), (101, 13), (102, 14), (103, 11), (104, 14), (105, 16)] {
        message(&mut m, id, 1, target, "a@example.com", "Project ABC123", &id.to_string());
    }
    assert_eq!(suggest(&m, 100), [Suggestion { folder: 11, reason: Reason::SenderAndSubject }]);
    assert!(suggest(&m, 101).is_empty());
    assert!(suggest(&m, 102).is_empty());
}

#[test]
fn slices_and_the_command_return_the_same_results_and_respect_options() {
    let mut m = mailbox();
    message(&mut m, 100, 1, 10, "a@example.com", "Project ABC123", "new");
    message(&mut m, 101, 1, 11, "a@example.com", "Project ABC123", "filed");
    message(&mut m, 102, 1, 12, "c@example.com", "Another message", "other");
    let mut scan = super::Scan::new(&m, 100).unwrap();
    while !scan.advance(1) {}
    assert_eq!(scan.suggestions(), suggest(&m, 100));

    let mut session = crate::Session::new(m);
    let params = serde_json::json!({"id": 100});
    let expected = serde_json::json!({"suggestions": [{"folder": 11, "reason": "senderAndSubject"}]});
    assert_eq!(session.run("mail.suggestFolders", &params).unwrap(), expected);
    assert_eq!(crate::agent::check(&session, "mail.suggestFolders", &params, &[]), crate::agent::Verdict::Run);
    assert!(session.run("mail.suggestFolders", &serde_json::json!({"id": null})).is_err());
    session.run("file.options", &serde_json::json!({"suggestFolders": false})).unwrap();
    assert_eq!(session.run("mail.suggestFolders", &params).unwrap(), serde_json::json!({"suggestions": []}));
    assert_eq!(session.mailbox.message(100).unwrap().folder, 10);
}
