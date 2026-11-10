// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reply and Delete, View Source and Save All Attachments.

use mailcraft_model::{Address, Attachment, Body, FolderKind, Id, Message};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

use crate::agent::{self, Verdict};
use crate::cmd::mail::{mail_source, save_all};
use crate::{ComposeMode, Session, sample};

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

/// A message in the sample account's Inbox, with these files and this HTML.
fn message(s: &mut Session, files: Vec<Attachment>, html: Option<&str>) -> Id {
    let account = s.mailbox.accounts[0].id;
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let m = Message {
        account,
        folder,
        message_id: "source.1@example.org".into(),
        from: Address::new("Avery Quinn", "avery@example.org"),
        to: vec![Address::new("Kit", "kit@example.org")],
        subject: "Plans".into(),
        date: 1_791_000_000,
        body: Body { text: "The plans.".into(), html: html.map(str::to_string) },
        attachments: files,
        headers: vec![("X-Mailer".into(), "Example Mail 1.0".into())],
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

fn file(name: &str, bytes: &[u8]) -> Attachment {
    Attachment::new(name.to_string(), "application/octet-stream", bytes.to_vec())
}

#[test]
fn reply_and_delete_answers_and_moves_the_original_to_deleted_items() {
    let mut s = session();
    let id = message(&mut s, Vec::new(), None);
    s.run("mail.select", &json!({"id": id})).unwrap();
    let r = s.run("mail.replyAndDelete", &json!({})).unwrap();
    assert_eq!(r["deleted"], true);
    let c = s.compose.as_ref().unwrap();
    assert_eq!((c.mode, c.source), (ComposeMode::Reply, Some(id)));
    assert!(c.to.contains("avery@example.org"));
    let m = s.mailbox.message(id).unwrap();
    let bin = s.mailbox.folder_of_kind(m.account, FolderKind::Deleted).unwrap();
    assert_eq!(m.folder, bin);
    // Undo puts it back.
    s.run("edit.undo", &json!({})).unwrap();
    assert_ne!(s.mailbox.message(id).unwrap().folder, bin);
}

#[test]
fn reply_and_delete_never_deletes_for_good() {
    let mut s = session();
    let id = message(&mut s, Vec::new(), None);
    let account = s.mailbox.message(id).unwrap().account;
    let bin = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.move_message(id, bin).unwrap();
    let r = s.run("mail.replyAndDelete", &json!({"id": id})).unwrap();
    assert_eq!(r["deleted"], false);
    assert_eq!(s.mailbox.message(id).unwrap().folder, bin);
    assert!(s.compose.is_some());
}

#[test]
fn agents_name_the_message_to_reply_and_delete() {
    let mut s = session();
    let id = message(&mut s, Vec::new(), None);
    s.run("mail.select", &json!({"id": id})).unwrap();
    assert!(matches!(agent::check(&s, "mail.replyAndDelete", &json!({}), &[]), Verdict::Refuse(_)));
    assert!(matches!(agent::check(&s, "mail.replyAndDelete", &json!({"id": null}), &[]), Verdict::Refuse(_)));
    assert!(matches!(agent::check(&s, "mail.replyAndDelete", &json!({"ids": [id, 1]}), &[]), Verdict::Refuse(_)));
    assert_eq!(agent::check(&s, "mail.replyAndDelete", &json!({"id": id}), &[]), Verdict::Run);
    assert!(s.run("mail.replyAndDelete", &json!({"id": "7"})).is_err());
}

#[test]
fn view_source_is_text_with_what_hides_made_visible() {
    let mut s = session();
    let html = "<p onclick=\"x()\">Hi <a href=\"https://example.org/t\">there</a>\u{202e}exe.txt\u{200b}\u{1b}[31m\u{0}</p>\r\n";
    let id = message(&mut s, Vec::new(), Some(html));
    let r = s.run("mail.viewSource", &json!({"id": id})).unwrap();
    let source = r["source"].as_str().unwrap();
    assert_eq!(r["truncated"], false);
    assert!(source.contains("From: Avery Quinn <avery@example.org>\n"), "{source}");
    assert!(source.contains("Subject: Plans\n"));
    assert!(source.contains("Message-ID: <source.1@example.org>\n"));
    assert!(source.contains("X-Mailer: Example Mail 1.0\n"));
    // The markup itself, as text.
    assert!(source.contains("<a href=\"https://example.org/t\">there</a>"));
    // Nothing that hides or turns text is left as it was.
    assert!(source.contains("<U+202E>exe.txt<U+200B>\u{241b}[31m\u{2400}</p>\n"), "{source}");
    assert!(!source.chars().any(|c| (c.is_control() && c != '\n' && c != '\t') || mailcraft_names::is_format(c)));
    // A message with no HTML shows its text.
    let plain = message(&mut s, Vec::new(), None);
    assert!(s.run("mail.viewSource", &json!({"id": plain})).unwrap()["source"].as_str().unwrap().ends_with("\n\nThe plans."));
    // Named wrongly, it is refused.
    assert!(s.run("mail.viewSource", &json!({"id": null})).is_err());
    assert!(s.run("mail.viewSource", &json!({"id": id, "path": "/x"})).is_err());
}

#[test]
fn view_source_is_capped() {
    let (text, cut) = mail_source::visible(&"é".repeat(10), 5);
    assert_eq!((text.as_str(), cut), ("éé", true));
    let (text, cut) = mail_source::visible("a\r\nb\rc", 100);
    assert_eq!((text.as_str(), cut), ("a\nb\u{240d}c", false));
    let mut s = session();
    let big = format!("<p>{}</p>", "x".repeat(mail_source::MAX_SOURCE + 10));
    let id = message(&mut s, Vec::new(), Some(&big));
    let r = s.run("mail.viewSource", &json!({"id": id})).unwrap();
    assert_eq!(r["truncated"], true);
    assert!(r["source"].as_str().unwrap().len() <= mail_source::MAX_SOURCE);
}

#[test]
fn save_all_names_are_made_safe_and_numbered() {
    let mut s = session();
    let files = vec![
        file("../../etc/passwd", b"a"),
        file("report.pdf", b"b"),
        file("report.pdf", b"c"),
        file("CON.txt", b"d"),
        file("\u{202e}fdp.exe", b"e"),
    ];
    let id = message(&mut s, files, None);
    let r = s.run("mail.saveAllAttachments", &json!({"id": id})).unwrap();
    let names: Vec<&str> = r["files"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(names[1..4], ["report.pdf", "report (2).pdf", "_CON.txt"]);
    for n in &names {
        assert!(!n.contains(['/', '\\']) && !n.starts_with('.') && !n.contains('\u{202e}'), "{n}");
    }
    assert_eq!(r["saved"], 5);
}

#[test]
fn save_all_never_writes_over_a_file() {
    let dir = std::env::temp_dir().join(format!("mailcraft-save-all-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("report.pdf"), b"mine").unwrap();
    let mut s = session();
    let id = message(&mut s, vec![file("report.pdf", b"new"), file("report.pdf", b"newer"), file("../up.txt", b"up")], None);
    let r = s.run("mail.saveAllAttachments", &json!({"id": id, "path": dir.to_string_lossy()})).unwrap();
    assert_eq!(r["saved"], 3);
    assert_eq!(std::fs::read(dir.join("report.pdf")).unwrap(), b"mine");
    assert_eq!(std::fs::read(dir.join("report (2).pdf")).unwrap(), b"new");
    assert_eq!(std::fs::read(dir.join("report (3).pdf")).unwrap(), b"newer");
    assert_eq!(std::fs::read(dir.join("up.txt")).unwrap(), b"up");
    assert!(!dir.parent().unwrap().join("up.txt").exists() || std::fs::read(dir.parent().unwrap().join("up.txt")).unwrap() != b"up");
    // Not a folder: nothing is written.
    assert!(s.run("mail.saveAllAttachments", &json!({"id": id, "path": dir.join("report.pdf").to_string_lossy()})).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn save_all_needs_files_and_agents_get_data() {
    let mut s = session();
    let none = message(&mut s, Vec::new(), None);
    assert!(s.run("mail.saveAllAttachments", &json!({"id": none})).is_err());
    let id = message(&mut s, vec![file("a.txt", b"a")], None);
    assert!(matches!(agent::check(&s, "mail.saveAllAttachments", &json!({"id": id, "path": "/tmp"}), &[]), Verdict::Refuse(_)));
    assert_eq!(agent::check(&s, "mail.saveAllAttachments", &json!({"id": id}), &[]), Verdict::Run);
    assert!(s.run("mail.saveAllAttachments", &json!({"id": id, "path": 5})).is_err());
}

#[test]
fn numbered_names() {
    use save_all::numbered;
    assert_eq!(numbered("a.pdf", 1), "a.pdf");
    assert_eq!(numbered("a.pdf", 2), "a (2).pdf");
    assert_eq!(numbered("archive", 3), "archive (3)");
}
