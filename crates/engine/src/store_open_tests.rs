// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening a mailbox file: the line it logs (#519).

use std::time::Duration;

use mailcraft_model::{Address, Mailbox, Message};

use super::*;

/// A folder of its own under the system's temporary one, removed when the test is over.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-open-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("mailbox.mailcraft")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A small mailbox whose mail has words and addresses that must never reach a log.
fn mailbox(messages: usize) -> Mailbox {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    for i in 0..messages {
        let m = Message {
            folder: inbox,
            subject: format!("Confidential merger plans {i}"),
            from: Address::new("Blake Rivera", "blake@example.org"),
            to: vec![Address::new("Avery Quinn", "avery@example.org")],
            date: 1_700_000_000 + i as i64,
            ..Default::default()
        };
        mb.add_message(m).unwrap();
    }
    mb
}

#[test]
fn the_open_line_says_how_long_how_big_and_how_many_and_nothing_of_the_mail() {
    let mb = mailbox(3);
    let line = open_line(Duration::from_millis(1_234), 5_000_000, counts(&mb), 0);
    assert!(line.contains("1.23 s"), "{line}");
    assert!(line.contains("5000000 bytes"), "{line}");
    assert!(line.contains("3 messages"), "{line}");
    assert!(line.contains(&format!("{} folders", mb.folders.len())), "{line}");
    assert!(line.contains("not repaired"), "{line}");
    for secret in ["Avery", "avery@", "Blake", "blake@", "example.org", "Confidential", "merger", "Inbox", "mailcraft"] {
        assert!(!line.contains(secret), "{secret} in {line}");
    }
    let repaired = open_line(Duration::from_millis(80), 10, counts(&mb), 2);
    assert!(repaired.contains("repaired (2 fixes)"), "{repaired}");
    assert!(!repaired.contains("not repaired"), "{repaired}");
}

#[test]
fn an_open_says_how_big_the_file_was_and_that_nothing_was_repaired() {
    let tmp = Temp::new("size");
    let path = tmp.file();
    save(&path, &mailbox(5)).unwrap();
    let opened = open(&path).unwrap();
    assert_eq!(opened.bytes, fs::metadata(&path).unwrap().len());
    assert_eq!(opened.repaired, 0);
    assert_eq!(opened.mailbox.messages.len(), 5);
}

#[test]
fn opening_counts_the_bytes_it_reads_of_the_file() {
    let tmp = Temp::new("count");
    let path = tmp.file();
    save(&path, &mailbox(40)).unwrap();
    let len = fs::metadata(&path).unwrap().len();
    let progress = OpenProgress::default();
    let opened = open_with(&path, &progress).unwrap();
    assert_eq!(opened.mailbox.messages.len(), 40);
    assert_eq!((progress.read(), progress.total()), (len, len));
    assert!(progress.reading_done());
}

#[test]
fn an_old_single_file_counts_its_bytes_too() {
    let tmp = Temp::new("single");
    let path = tmp.file();
    save_single(&path, &mailbox(3)).unwrap();
    let len = fs::metadata(&path).unwrap().len();
    let progress = OpenProgress::default();
    assert_eq!(open_with(&path, &progress).unwrap().mailbox.messages.len(), 3);
    assert_eq!((progress.read(), progress.total()), (len, len));
}

#[test]
fn a_cancelled_open_stops_with_an_error() {
    let tmp = Temp::new("cancel");
    let path = tmp.file();
    save(&path, &mailbox(3)).unwrap();
    let progress = OpenProgress::default();
    progress.cancel();
    let e = open_with(&path, &progress).unwrap_err();
    assert!(e.contains("cancelled"), "{e}");
}

#[test]
fn what_was_read_ahead_is_taken_by_the_next_open_of_the_file_once() {
    let tmp = Temp::new("ahead");
    let path = tmp.file();
    save(&path, &mailbox(4)).unwrap();
    open_ahead(&path, &OpenProgress::default()).unwrap();
    // Nothing is read again, so not even a cancelled reader is asked.
    let cancelled = OpenProgress::default();
    cancelled.cancel();
    assert_eq!(open_with(&path, &cancelled).unwrap().mailbox.messages.len(), 4);
    // Taken once: the next open reads the file.
    assert!(open_with(&path, &cancelled).is_err());
    assert_eq!(open(&path).unwrap().mailbox.messages.len(), 4);
}

#[test]
fn a_file_changed_since_it_was_read_ahead_is_read_again() {
    let tmp = Temp::new("ahead-changed");
    let path = tmp.file();
    save(&path, &mailbox(4)).unwrap();
    open_ahead(&path, &OpenProgress::default()).unwrap();
    save_single(&path, &mailbox(9)).unwrap();
    assert_eq!(open(&path).unwrap().mailbox.messages.len(), 9);
}

#[test]
fn a_read_ahead_cancelled_or_forgotten_keeps_nothing() {
    let tmp = Temp::new("ahead-gone");
    let path = tmp.file();
    save(&path, &mailbox(2)).unwrap();
    let cancelled = OpenProgress::default();
    cancelled.cancel();
    open_ahead(&path, &OpenProgress::default()).unwrap();
    forget_ahead(&path);
    assert!(open_with(&path, &cancelled).is_err(), "forgotten: it is read again");
    assert!(open_ahead(&path, &cancelled).is_err());
    assert!(open_with(&path, &cancelled).is_err(), "cancelled: nothing was kept");
}

#[test]
fn a_file_this_program_saved_since_it_was_read_ahead_is_read_again() {
    let tmp = Temp::new("ahead-saved");
    let path = tmp.file();
    save(&path, &mailbox(4)).unwrap();
    open_ahead(&path, &OpenProgress::default()).unwrap();
    save(&path, &mailbox(9)).unwrap();
    let opened = open(&path).unwrap();
    assert_eq!(opened.mailbox.messages.len(), 9, "not the stale read-ahead");
    // What is saved next is written against the file as it is, losing nothing.
    save(&path, &opened.mailbox).unwrap();
    assert_eq!(open(&path).unwrap().mailbox.messages.len(), 9);
}

#[test]
fn a_save_while_a_read_ahead_is_being_kept_is_never_lost() {
    let tmp = Temp::new("ahead-race");
    let path = tmp.file();
    save(&path, &mailbox(4)).unwrap();
    // Just as what was read is about to be kept, another thread saves a newer state; it is given
    // time enough to finish, unless the read-ahead holds the file until it is kept.
    let saver = Arc::new(Mutex::new(None));
    let handle = saver.clone();
    let newer = path.clone();
    *lock(&READ_AHEAD_HOOK) = Some((
        path.clone(),
        Box::new(move || {
            *lock(&handle) = Some(std::thread::spawn(move || save(&newer, &mailbox(9)).unwrap()));
            std::thread::sleep(Duration::from_millis(300));
        }),
    ));
    open_ahead(&path, &OpenProgress::default()).unwrap();
    let thread = lock(&saver).take();
    thread.unwrap().join().unwrap();
    assert_eq!(open(&path).unwrap().mailbox.messages.len(), 9, "not the read-ahead from before the save");
}

#[test]
fn a_message_file_read_ahead_is_sent_to_import() {
    let tmp = Temp::new("ahead-eml");
    let path = tmp.0.join("message.eml");
    fs::write(&path, b"Subject: hi\r\n\r\nbody\r\n").unwrap();
    assert!(open_ahead(&path, &OpenProgress::default()).unwrap_err().contains("File › Import"));
}
