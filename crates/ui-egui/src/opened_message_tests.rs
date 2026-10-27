// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message file read without being filed (#515): the window holds it in memory, the mailbox
//! gains nothing until Move to Folder, closing leaves no trace, and a hostile or oversized file
//! is refused or shown, never a panic.

use std::path::PathBuf;

use mailcraft_engine::Session;
use mailcraft_model::FolderKind;
use serde_json::{Value, json};

use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

const EML: &str = "From: Maya Lin <maya@lumen.example>\r\nTo: avery@lumen.example\r\nSubject: Handed over\r\nDate: Wed, 14 Oct 2026 10:00:00 +0000\r\nMessage-ID: <handed@lumen.example>\r\n\r\nHello.\r\n";

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.auto_sync = false;
    a
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-opened-message-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn file_windows(a: &MailApp) -> Vec<Value> {
    crate::windows::inspect(a).as_array().unwrap().iter().filter(|w| w["kind"] == "file").cloned().collect()
}

#[test]
fn opening_an_eml_adds_nothing_to_the_mailbox_and_closing_leaves_no_trace() {
    let dir = scratch("none");
    let path = dir.join("handed.eml");
    std::fs::write(&path, EML).unwrap();
    let mut a = app();
    let (before, selected, folder) = (a.session.mailbox.messages.len(), a.session.sel.messages.clone(), a.session.view.folder);
    a.open_from_system(&path.to_string_lossy());
    // The same file again is the same kind of window, and still nothing is filed.
    // As a file manager writes it: `file:///tmp/…`, or `file:///C:/…` on Windows.
    let url = path.to_string_lossy().replace('\\', "/");
    a.open_from_system(&format!("file:///{}", url.trim_start_matches('/')));
    let windows = file_windows(&a);
    assert_eq!(windows.len(), 2, "{windows:?}");
    assert_eq!(a.session.mailbox.messages.len(), before);
    assert!(a.session.mailbox.messages.values().all(|m| m.subject != "Handed over"));
    assert_eq!((a.session.sel.messages.clone(), a.session.view.folder), (selected, folder));
    assert!(a.dialog.is_none() && a.session.compose.is_none());
    a.run("ui.closeWindow", json!({"all": true})).unwrap();
    assert!(file_windows(&a).is_empty());
    assert_eq!(a.session.mailbox.messages.len(), before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn move_to_folder_files_exactly_one_message_and_the_window_follows_it() {
    let dir = scratch("move");
    let path = dir.join("handed.eml");
    std::fs::write(&path, EML).unwrap();
    let mut a = app();
    a.open_from_system(&path.to_string_lossy());
    let account = a.session.mailbox.default_account().unwrap().id;
    let archive = a.session.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    let before = a.session.mailbox.messages.len();
    let r = a.run("ui.moveWindowMessage", json!({"folder": archive})).unwrap();
    assert_eq!(a.session.mailbox.messages.len(), before + 1);
    let id = r["message"].as_u64().unwrap();
    let m = a.session.mailbox.message(id).unwrap();
    assert_eq!((m.subject.as_str(), m.folder, m.draft), ("Handed over", archive, false));
    // The window is about the filed message now.
    let windows = crate::windows::inspect(&a);
    assert!(windows.as_array().unwrap().iter().any(|w| w["kind"] == "message" && w["message"] == json!(id)), "{windows}");
    assert!(file_windows(&a).is_empty());
    // Nothing more to move.
    assert!(a.run("ui.moveWindowMessage", json!({"folder": archive})).is_err());
    assert_eq!(a.session.mailbox.messages.len(), before + 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn move_to_folder_is_refused_for_the_outbox_and_for_a_folder_that_is_not_there() {
    let dir = scratch("refused");
    let path = dir.join("handed.eml");
    std::fs::write(&path, EML).unwrap();
    let mut a = app();
    a.open_from_system(&path.to_string_lossy());
    let account = a.session.mailbox.default_account().unwrap().id;
    let outbox = a.session.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let before = a.session.mailbox.messages.len();
    assert!(a.run("ui.moveWindowMessage", json!({"folder": outbox})).is_err());
    assert!(a.run("ui.moveWindowMessage", json!({"folder": 987_654})).is_err());
    assert!(a.run("ui.moveWindowMessage", json!({})).is_err());
    assert_eq!(a.session.mailbox.messages.len(), before);
    assert_eq!(file_windows(&a).len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn save_as_writes_the_file_itself() {
    let dir = scratch("save");
    let path = dir.join("handed.eml");
    std::fs::write(&path, EML).unwrap();
    let mut a = app();
    a.open_from_system(&path.to_string_lossy());
    let copy = dir.join("copy.eml");
    a.run("ui.saveWindowMessage", json!({"path": copy})).unwrap();
    assert_eq!(std::fs::read(&copy).unwrap(), EML.as_bytes());
    // Without a path the bytes are answered.
    let r = a.run("ui.saveWindowMessage", json!({})).unwrap();
    assert_eq!(r["bytes"], json!(EML.len()));
    assert!(r["name"].as_str().unwrap().ends_with(".eml"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn hostile_and_oversized_message_files_never_panic() {
    let dir = scratch("hostile");
    let mut a = app();
    let before = a.session.mailbox.messages.len();
    // Bytes that are no message, deep and wide structure, a binary blob, an empty file.
    let deep = format!(
        "From: a@example.org\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n{}",
        "--b\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n".repeat(5_000)
    );
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty.eml", Vec::new()),
        ("binary.eml", (0..=255u8).cycle().take(100_000).collect()),
        ("deep.eml", deep.into_bytes()),
        ("nul.eml", b"Subject: a\0b\r\nFrom: \0\0\r\n\r\n\0".to_vec()),
        ("html.eml", b"Subject: x\r\nContent-Type: text/html\r\n\r\n<script>alert(1)</script><img src=\"http://example.org/p.png\"><a href=\"javascript:x\">y</a>".to_vec()),
    ];
    for (name, bytes) in cases {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        a.open_from_system(&path.to_string_lossy());
    }
    // A file that is no regular file, and one that is not there.
    a.open_from_system(&dir.to_string_lossy());
    a.open_from_system(&dir.join("gone.eml").to_string_lossy());
    assert_eq!(a.session.mailbox.messages.len(), before);
    // Past the bound, a file is refused without being read into a window.
    let big = dir.join("big.eml");
    let file = std::fs::File::create(&big).unwrap();
    file.set_len(crate::opened_message::MAX_BYTES + 1).unwrap();
    let n = file_windows(&a).len();
    a.open_from_system(&big.to_string_lossy());
    assert_eq!(file_windows(&a).len(), n);
    assert!(a.status_msg.as_ref().is_some_and(|s| s.0.contains("larger than")), "{:?}", a.status_msg);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_so_many_windows_open_and_a_file_window_is_never_a_draft_or_in_the_list() {
    let dir = scratch("many");
    let path = dir.join("handed.eml");
    std::fs::write(&path, "X-Unsent: 1\r\nFrom: a@example.org\r\nSubject: unsent\r\n\r\nx").unwrap();
    let mut a = app();
    for _ in 0..crate::windows::MAX_WINDOWS + 3 {
        a.open_from_system(&path.to_string_lossy());
    }
    assert_eq!(file_windows(&a).len(), crate::windows::MAX_WINDOWS);
    assert!(a.session.compose.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

/// The window draws: its bar, the message, and a note instead of pictures from the web; and its
/// attachments name no message of the mailbox.
#[test]
fn the_window_shows_the_message_with_its_bar() {
    let dir = scratch("draw");
    let path = dir.join("pics.eml");
    let html = "From: Maya Lin <maya@lumen.example>\r\nTo: avery@lumen.example\r\nSubject: Pictures\r\nContent-Type: text/html\r\n\r\n<p>Look at this</p><img src=\"https://example.org/x.png\" width=\"200\" height=\"100\">";
    std::fs::write(&path, html).unwrap();
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app(),
        );
    harness.run_steps(3);
    harness.state_mut().open_from_system(&path.to_string_lossy());
    harness.run_steps(6);
    assert_eq!(file_windows(harness.state()).len(), 1);
    assert_eq!(file_windows(harness.state())[0]["title"], "Pictures");
    assert!(harness.state().session.mailbox.messages.values().all(|m| m.subject != "Pictures"));
    let _ = std::fs::remove_dir_all(dir);
}
