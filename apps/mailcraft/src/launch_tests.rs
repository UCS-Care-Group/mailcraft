// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mailcraft_model::{Address, Message};

use super::*;

/// A folder of its own under the system's temporary one, removed when the test is over.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-launch-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mailbox(n: usize) -> Mailbox {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    for i in 0..n {
        let m = Message {
            folder: inbox,
            subject: format!("Note {i}"),
            from: Address::new("Blake Rivera", "blake@example.org"),
            date: 1_700_000_000 + i as i64,
            ..Default::default()
        };
        mb.add_message(m).unwrap();
    }
    mb
}

/// Every file under `dir`, with its bytes.
fn everything(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(e.path());
            } else {
                out.push((e.path(), std::fs::read(e.path()).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn a_panic_while_opening_is_a_mailbox_that_couldnt_be_read() {
    let woken = Arc::new(AtomicBool::new(false));
    let wake = woken.clone();
    let rx = start(|| panic!("a bug in reading"), move || wake.store(true, Ordering::SeqCst));
    let arrival = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(arrival.trouble, Some(Trouble::Stopped));
    assert!(arrival.path.is_none(), "nothing is written over the file");
    assert!(arrival.mailbox.messages.is_empty());
    // (The window is woken just after the mailbox is sent.)
    for _ in 0..500 {
        if woken.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(woken.load(Ordering::SeqCst));
}

#[test]
fn a_mailbox_file_opens_on_its_thread_and_counts_what_it_reads_and_writes_nothing() {
    let tmp = Temp::new("good");
    let file = tmp.0.join("mailbox.mailcraft");
    mailcraft_engine::store::save(&file, &mailbox(6)).unwrap();
    let before = everything(&tmp.0);
    let progress = Arc::new(OpenProgress::default());
    let (path, counted) = (file.clone(), progress.clone());
    let rx = start(move || first_mailbox(false, Some(path), &counted), || {});
    let arrival = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(arrival.trouble, None);
    assert_eq!(arrival.path.as_deref(), Some(file.as_path()));
    assert_eq!(arrival.mailbox.messages.len(), 6);
    let len = std::fs::metadata(&file).unwrap().len();
    assert_eq!((progress.read(), progress.total()), (len, len));
    // Opening only reads: the window closed meanwhile would leave the file as it was.
    drop(arrival);
    assert_eq!(everything(&tmp.0), before);
}

#[test]
fn a_file_that_isnt_a_mailbox_says_why_and_is_kept_nowhere() {
    let tmp = Temp::new("bad");
    let file = tmp.0.join("mailbox.mailcraft");
    std::fs::write(&file, b"{ this is not a mailbox").unwrap();
    let arrival = first_mailbox(false, Some(file.clone()), &OpenProgress::default());
    match &arrival.trouble {
        Some(Trouble::Unreadable(why)) => assert!(why.contains("isn't a MailCraft mailbox"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(arrival.path.is_none());
    assert_eq!(std::fs::read(&file).unwrap(), b"{ this is not a mailbox");
}

#[test]
fn a_mailbox_not_there_yet_starts_empty_where_it_will_be_kept() {
    let tmp = Temp::new("new");
    let file = tmp.0.join("mailbox.mailcraft");
    let arrival = first_mailbox(false, Some(file.clone()), &OpenProgress::default());
    assert_eq!(arrival.trouble, None);
    assert_eq!(arrival.path.as_deref(), Some(file.as_path()));
    assert!(!file.exists());
}

#[test]
fn the_sample_is_kept_nowhere() {
    let arrival = first_mailbox(true, None, &OpenProgress::default());
    assert!(arrival.path.is_none());
    assert!(!arrival.mailbox.messages.is_empty());
}

#[test]
fn a_window_closed_while_opening_writes_no_settings() {
    let tmp = Temp::new("prefs");
    let file = tmp.0.join("ui.json");
    let (_tx, rx) = channel();
    let build: mailcraft_ui_egui::opening::Build =
        Box::new(|a: Arrival| mailcraft_ui_egui::MailApp::new(mailcraft_engine::Session::new(a.mailbox), Default::default()));
    let opening = mailcraft_ui_egui::opening::Opening::new(rx, Arc::new(OpenProgress::default()), build);
    let app = mailcraft_ui_egui::MailApp::opening(opening, None, None);
    crate::save_prefs_at(&app, &file);
    assert!(!file.exists(), "nothing of the waiting window is written");
    let open = mailcraft_ui_egui::MailApp::new(mailcraft_engine::Session::new(mailbox(1)), Default::default());
    crate::save_prefs_at(&open, &file);
    assert!(file.exists(), "a window with its mailbox writes them");
}
