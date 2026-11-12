// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Files copied in a file manager, pasted into the message being written (#807): attached, by
//! Ctrl+V whether or not the backend could hand over anything for it.

use std::path::PathBuf;

use egui::{Event, Key, Modifiers};
use serde_json::json;

use super::Keys;
use crate::compose::tests::{Window, app, key_with, plain, type_in, window};

fn release(modifiers: Modifiers) -> Event {
    Event::Key { key: Key::V, physical_key: None, pressed: false, repeat: false, modifiers }
}

fn press(modifiers: Modifiers) -> Event {
    Event::Key { key: Key::V, physical_key: None, pressed: true, repeat: false, modifiers }
}

#[test]
fn a_ctrl_v_the_backend_kept_is_told_from_a_v_typed() {
    // The press was kept, the release came with Ctrl still down: a lost Ctrl+V.
    let mut k = Keys::default();
    assert!(k.lost_paste(1.0, true, &[release(Modifiers::COMMAND)]));
    // Ctrl let go of first, a moment before: still the Ctrl+V.
    let mut k = Keys::default();
    assert!(!k.lost_paste(1.0, true, &[]));
    assert!(k.lost_paste(1.3, false, &[release(Modifiers::NONE)]));
    // ...but not long after.
    let mut k = Keys::default();
    assert!(!k.lost_paste(1.0, true, &[]));
    assert!(!k.lost_paste(3.0, false, &[release(Modifiers::NONE)]));
    // A v typed: its press was seen.
    let mut k = Keys::default();
    assert!(!k.lost_paste(1.0, false, &[press(Modifiers::NONE), release(Modifiers::NONE)]));
    let mut k = Keys::default();
    assert!(!k.lost_paste(1.0, false, &[press(Modifiers::NONE)]));
    assert!(!k.lost_paste(1.1, true, &[release(Modifiers::COMMAND)]), "the press was seen a pass before");
    // A release with no Ctrl anywhere near (the focus came here between the press and it).
    let mut k = Keys::default();
    assert!(!k.lost_paste(1.0, false, &[release(Modifiers::NONE)]));
    // A paste came of it: the backend handed over text or a picture, and that was taken.
    let mut k = Keys::default();
    assert!(!k.lost_paste(1.0, true, &[Event::Paste("x".into()), release(Modifiers::COMMAND)]));
    let mut k = Keys::default();
    assert!(!k.lost_paste(1.0, true, &[Event::Paste("x".into())]));
    assert!(!k.lost_paste(1.2, true, &[release(Modifiers::COMMAND)]));
    // A paste long before is no paste of this one.
    assert!(k.lost_paste(9.0, true, &[release(Modifiers::COMMAND)]));
    // A clock that went back is no moment ago.
    let mut k = Keys::default();
    assert!(!k.lost_paste(5.0, true, &[]));
    assert!(!k.lost_paste(1.0, false, &[release(Modifiers::NONE)]));
}

/// A clipboard holding files, as a file manager leaves it.
#[derive(Clone, Default)]
struct Files(std::rc::Rc<std::cell::RefCell<Vec<PathBuf>>>);

impl crate::RichClipboard for Files {
    fn put(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn html(&self) -> Option<String> {
        None
    }
    fn files(&self) -> Vec<PathBuf> {
        self.0.borrow().clone()
    }
}

/// Files in a folder of their own, for a test.
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Folder {
        let dir = std::env::temp_dir().join(format!("mailcraft-file-paste-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Folder(dir)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn writing(board: &Files) -> Window {
    let mut a = app();
    a.services.clipboard = Some(Box::new(board.clone()));
    a.execute("mail.new", json!({"to": "maya@lumen.example"})).unwrap();
    a.execute("compose.set", json!({"body": ""})).unwrap();
    a.execute("compose.select", json!({"from": 0})).unwrap();
    a.want_body_focus();
    let mut h = window(a);
    h.run_steps(2);
    h
}

fn attached(h: &Window) -> Vec<(String, Vec<u8>)> {
    h.state().session.compose.as_ref().unwrap().attachments.iter().map(|a| (a.name.clone(), a.data.to_vec())).collect()
}

#[test]
fn a_paste_of_files_attaches_them_and_leaves_the_text() {
    let folder = Folder::new("paste");
    let board = Files::default();
    let mut h = writing(&board);
    type_in(&mut h, "Here:");
    *board.0.borrow_mut() = vec![folder.file("report.pdf", b"one"), folder.file("notes.txt", b"two")];
    // (A file manager on Linux or macOS puts the names beside the files, as text.)
    h.event(Event::Paste(format!("{}\n{}", folder.0.join("report.pdf").display(), folder.0.join("notes.txt").display())));
    h.run_steps(2);
    assert_eq!(attached(&h), vec![("report.pdf".to_string(), b"one".to_vec()), ("notes.txt".to_string(), b"two".to_vec())]);
    assert_eq!(plain(&h), "Here:", "the names aren't pasted as text");
    // With no files on the clipboard, a paste is text again.
    board.0.borrow_mut().clear();
    h.event(Event::Paste(" text".into()));
    h.run_steps(2);
    assert_eq!(plain(&h), "Here: text");
    assert_eq!(attached(&h).len(), 2);
}

#[test]
fn ctrl_v_of_files_alone_attaches_them() {
    // Explorer's files: the backend has no text or picture to hand over, and keeps the press.
    let folder = Folder::new("lost");
    let board = Files::default();
    let mut h = writing(&board);
    *board.0.borrow_mut() = vec![folder.file("photo.jpg", b"jpg")];
    h.event(release(Modifiers::COMMAND));
    h.run_steps(2);
    assert_eq!(attached(&h), vec![("photo.jpg".to_string(), b"jpg".to_vec())]);
    // A v typed with files on the clipboard attaches nothing.
    key_with(&mut h, Modifiers::NONE, Key::V);
    type_in(&mut h, "v");
    assert_eq!(attached(&h).len(), 1);
    assert_eq!(plain(&h), "v");
}

#[test]
fn what_cant_be_attached_is_said_and_the_rest_goes_in() {
    let folder = Folder::new("partly");
    let board = Files::default();
    let mut h = writing(&board);
    let gone = folder.0.join("gone.pdf");
    *board.0.borrow_mut() = vec![gone, folder.0.clone(), folder.file("ok.txt", b"ok")];
    h.event(Event::Paste(String::new()));
    h.run_steps(2);
    assert_eq!(attached(&h), vec![("ok.txt".to_string(), b"ok".to_vec())]);
    let said = h.state().status_msg.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(said.contains("Attached 1 file") && said.contains("gone.pdf"), "{said}");
}

#[test]
fn a_signature_takes_no_files() {
    let folder = Folder::new("signature");
    let board = Files::default();
    let mut h = writing(&board);
    *board.0.borrow_mut() = vec![folder.file("logo.png", b"png")];
    // Only the message being written attaches: the signature's body is no message.
    assert!(super::files(h.state(), super::super::Which::Signature, "").is_none());
    assert!(super::files(h.state(), super::super::Which::Message, "").is_some());
    h.state_mut().services.clipboard = None;
    assert!(super::files(h.state(), super::super::Which::Message, "").is_none());
    let _ = &mut h;
}

#[test]
fn input_the_control_channel_made_attaches_nothing() {
    let folder = Folder::new("driven");
    let board = Files::default();
    let mut h = writing(&board);
    *board.0.borrow_mut() = vec![folder.file("secret.txt", b"s")];
    // A pass the control channel drove: a V let go of is no lost Ctrl+V of the owner's.
    h.state_mut().driven = true;
    h.event(release(Modifiers::COMMAND));
    h.run_steps(2);
    // Its keys come as a press with its release: never a lost Ctrl+V either.
    h.state_mut().driven = false;
    key_with(&mut h, Modifiers::COMMAND, Key::V);
    assert!(attached(&h).is_empty());
}

#[test]
fn text_that_names_other_files_is_pasted_as_text_and_attaches_nothing() {
    // A page put an address of a file of the owner's on the clipboard beside the text copied from
    // it: the text is what Ctrl+V pastes, and the file is not attached.
    let folder = Folder::new("hostile");
    let board = Files::default();
    let mut h = writing(&board);
    *board.0.borrow_mut() = vec![folder.file("secret.txt", b"s")];
    h.event(Event::Paste("Some words from a page".into()));
    h.run_steps(2);
    assert!(attached(&h).is_empty());
    assert_eq!(plain(&h), "Some words from a page");
    // More lines than files, or a line that is another file's: text too.
    h.event(Event::Paste(format!("{}\nother.txt", folder.0.join("secret.txt").display())));
    h.run_steps(2);
    assert!(attached(&h).is_empty());
}

#[test]
fn what_a_file_manager_leaves_beside_the_files_is_told_by_what_it_names() {
    let files = vec!["/home/ada/My Files/report.pdf".to_string(), "C:\\Users\\ada\\notes.txt".to_string()];
    // Nothing (Explorer), paths, addresses (spaces as %20, a local host, a drive), names (Finder).
    for text in [
        "",
        "  \n",
        "/home/ada/My Files/report.pdf",
        "/home/ada/My Files/report.pdf\r\nC:\\Users\\ada\\notes.txt\r\n",
        "file:///home/ada/My%20Files/report.pdf\nfile://localhost/home/ada/My%20Files/report.pdf",
        "file:///C:/Users/ada/notes.txt",
        "report.pdf\nnotes.txt",
    ] {
        assert!(super::names_them(text, &files), "{text:?}");
    }
    // Anything else, however much of it names a file.
    for text in [
        "hello",
        "report.pdf and more",
        "report.pdf\nnotes.txt\nreport.pdf",
        "file:///etc/passwd",
        "/home/ada/My Files/other.pdf",
        "file:///home/ada/My%2",
    ] {
        assert!(!super::names_them(text, &files), "{text:?}");
    }
}

#[test]
fn the_status_line_names_the_files_a_paste_attached() {
    let folder = Folder::new("named");
    let board = Files::default();
    let mut h = writing(&board);
    *board.0.borrow_mut() = vec![folder.file("report.pdf", b"one")];
    h.event(Event::Paste(String::new()));
    h.run_steps(2);
    let said = h.state().status_msg.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(said.contains("Attached 1 file") && said.contains("(report.pdf)"), "{said}");
}
