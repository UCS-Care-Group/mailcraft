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
    let line = open_line(Duration::from_millis(1_234), 5_000_000, &mb, 0);
    assert!(line.contains("1.23 s"), "{line}");
    assert!(line.contains("5000000 bytes"), "{line}");
    assert!(line.contains("3 messages"), "{line}");
    assert!(line.contains(&format!("{} folders", mb.folders.len())), "{line}");
    assert!(line.contains("1 accounts"), "{line}");
    assert!(line.contains("not repaired"), "{line}");
    for secret in ["Avery", "avery@", "Blake", "blake@", "example.org", "Confidential", "merger", "Inbox", "mailcraft"] {
        assert!(!line.contains(secret), "{secret} in {line}");
    }
    let repaired = open_line(Duration::from_millis(80), 10, &mb, 2);
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
