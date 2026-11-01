// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Attach File takes several files at once: the dialog lets the owner pick them, and each is
//! attached in the order picked. Every other dialog still picks one file.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use serde_json::json;

use super::FileDialogRequest;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A message being written, with a file dialog that answers `picked` at once.
fn writing(picked: Option<Vec<String>>) -> (MailApp, Arc<Mutex<Vec<FileDialogRequest>>>) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    app.services.file_dialog = Some(Box::new(move |request| {
        seen.lock().unwrap().push(request);
        let (tx, rx) = mpsc::channel();
        tx.send(picked.clone()).unwrap();
        rx
    }));
    app.run("mail.new", json!({"to": "kit@example.org", "subject": "Figures"})).unwrap();
    (app, asked)
}

/// Files of a folder of their own for one test, removed when it ends.
struct Files(std::path::PathBuf);

impl Files {
    fn new(test: &str, names: &[&str]) -> (Self, Vec<String>) {
        let dir = std::env::temp_dir().join(format!("mailcraft-attach-many-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = names
            .iter()
            .map(|name| {
                let path = dir.join(name);
                std::fs::write(&path, format!("the text of {name}")).unwrap();
                path.to_string_lossy().to_string()
            })
            .collect();
        (Files(dir), paths)
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn attached(app: &MailApp) -> Vec<String> {
    app.session.compose.as_ref().unwrap().attachments.iter().map(|a| a.name.clone()).collect()
}

fn said(app: &MailApp) -> String {
    app.status_msg.clone().unwrap_or_default().0
}

#[test]
fn only_files_to_attach_are_picked_several_at_once() {
    assert!(FileDialogRequest::Open { purpose: "attach".into() }.picks_many());
    for purpose in ["mailbox", "import", "picture", "bodyPicture"] {
        assert!(!FileDialogRequest::Open { purpose: purpose.into() }.picks_many(), "{purpose}");
    }
    assert!(!FileDialogRequest::Folder.picks_many());
    assert!(!FileDialogRequest::Save { name: "a.eml".into() }.picks_many());
}

#[test]
fn every_file_picked_is_attached_in_the_order_picked() {
    let (_files, paths) = Files::new("all", &["minutes.txt", "budget.csv", "notes.eml"]);
    let (mut app, asked) = writing(Some(paths));
    app.run("ui.attach", json!({})).unwrap();
    assert_eq!(*asked.lock().unwrap(), [FileDialogRequest::Open { purpose: "attach".into() }]);
    // A message file among them is attached as the others are, not imported.
    assert_eq!(attached(&app), ["minutes.txt", "budget.csv", "notes.eml"]);
}

#[test]
fn a_file_that_cant_be_attached_doesnt_keep_the_others_out() {
    let (files, mut paths) = Files::new("one-gone", &["minutes.txt", "budget.csv"]);
    paths.insert(1, files.0.join("not-there.txt").to_string_lossy().to_string());
    let (mut app, _) = writing(Some(paths));
    app.run("ui.attach", json!({})).unwrap();
    assert_eq!(attached(&app), ["minutes.txt", "budget.csv"]);
    // The status bar says how many went in and why one didn't, in one line.
    let said = said(&app);
    assert!(said.starts_with("Attached 2 files · ") && said.contains("not-there.txt"), "{said}");
}

/// None could be attached: the reason, and an error.
#[test]
fn nothing_attached_is_an_error_with_the_reason() {
    let (files, _) = Files::new("none", &[]);
    let paths = ["a", "b"].map(|n| files.0.join(format!("{n}.txt")).to_string_lossy().to_string()).to_vec();
    let (mut app, _) = writing(Some(paths));
    // (`ui.attach` answers that the pick is pending; what came of it is the status bar's.)
    app.run("ui.attach", json!({})).unwrap();
    assert!(attached(&app).is_empty());
    assert!(!said(&app).starts_with("Attached") && said(&app).contains("a.txt"), "{}", said(&app));
}

/// A stuck key picks a great many: the first hundred go in, and the status bar says the rest
/// were left out.
#[test]
fn more_than_the_most_at_a_time_are_left_out_and_said() {
    let names: Vec<String> = (0..103).map(|i| format!("file-{i:03}.txt")).collect();
    let (_files, paths) = Files::new("many", &names.iter().map(String::as_str).collect::<Vec<_>>());
    let (mut app, _) = writing(Some(paths));
    app.run("ui.attach", json!({})).unwrap();
    assert_eq!(attached(&app).len(), 100);
    assert_eq!(attached(&app)[99], "file-099.txt");
    assert_eq!(said(&app), "Attached 100 files · 3 more were left out: at most 100 files at a time");
}

#[test]
fn one_file_picked_is_attached_as_before() {
    let (_files, paths) = Files::new("one", &["minutes.txt"]);
    let (mut app, _) = writing(Some(paths));
    app.run("ui.attach", json!({})).unwrap();
    assert_eq!(attached(&app), ["minutes.txt"]);
}

#[test]
fn a_cancelled_pick_attaches_nothing() {
    let (mut app, _) = writing(None);
    app.run("ui.attach", json!({})).unwrap();
    assert!(attached(&app).is_empty());
}

/// The browser: the picker's files arrive together, and each is attached, a message file too.
#[test]
fn every_file_from_the_browser_picker_is_attached() {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    let inbox: crate::Inbox = Arc::new(Mutex::new(Vec::new()));
    app.services.inbox = Some(inbox.clone());
    app.services.open_async = Some(Box::new(|_| {}));
    app.run("mail.new", json!({"to": "kit@example.org", "subject": "Figures"})).unwrap();
    app.run("ui.attach", json!({})).unwrap();
    inbox.lock().unwrap().extend([("minutes.txt".to_string(), b"one".to_vec()), ("notes.eml".to_string(), b"two".to_vec())]);
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app), ["minutes.txt", "notes.eml"]);
    assert_eq!(said(&app), "Attached 2 files");
    // One of them refused (the message carries as many as it can by then): the rest go in,
    // and the line says how many did and why one didn't.
    for i in 2..199 {
        app.run("compose.attach", json!({"name": format!("f{i}.txt"), "data": "eA=="})).unwrap();
    }
    app.run("ui.attach", json!({})).unwrap();
    inbox.lock().unwrap().extend([("budget.csv".to_string(), b"three".to_vec()), ("one-more.txt".to_string(), b"four".to_vec())]);
    super::drain_inbox(&mut app);
    assert_eq!(attached(&app).len(), 200);
    assert_eq!(attached(&app)[199], "budget.csv");
    assert!(said(&app).starts_with("Attached 1 file · ") && said(&app).contains("as many attachments"), "{}", said(&app));
}
