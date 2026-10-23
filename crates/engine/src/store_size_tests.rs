// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #300: the size on the server of an IMAP message too large to download is kept in the mailbox
//! file, and a file from before, without it, opens as it is.

use std::fs;

use mailcraft_model::{Address, Body, Mailbox, Message, NOT_DOWNLOADED};
use serde_json::Value;

use super::*;

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-store-size-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A mailbox with one message too large to download, as the synchronisation keeps it, and one
/// whole message.
fn mailbox() -> (Mailbox, Id, Id) {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    let from = Address::new("Ada Aldrin", "ada@example.org");
    let large = Message {
        folder: inbox,
        subject: "Holiday pictures".into(),
        from: from.clone(),
        uid: 7,
        body: Body { text: "This message is too large to download.".into(), html: None },
        headers: vec![(NOT_DOWNLOADED.into(), "62914560".into())],
        not_downloaded: 62_914_560,
        ..Default::default()
    };
    let large = mb.add_message(large).unwrap();
    let whole = Message { folder: inbox, subject: "Lunch".into(), from, body: Body { text: "At noon.".into(), html: None }, ..Default::default() };
    let whole = mb.add_message(whole).unwrap();
    (mb, large, whole)
}

fn reopened(path: &Path) -> Mailbox {
    let mut opened = open(path).unwrap();
    assert_eq!(opened.unavailable, 0);
    load_all(path, &mut opened.mailbox).unwrap();
    opened.mailbox
}

#[test]
fn the_size_on_the_server_is_saved_and_read_back() {
    let tmp = Temp::new("round-trip");
    let path = tmp.0.join("mailbox.mailcraft");
    let (mb, large, whole) = mailbox();
    save(&path, &mb).unwrap();
    let back = reopened(&path);
    assert!(back == mb);
    let m = back.message(large).unwrap();
    assert_eq!((m.not_downloaded, m.list_size()), (62_914_560, Some(62_914_560)));
    assert_eq!(back.message(whole).unwrap().not_downloaded, 0);
    // In one self-contained file too (exports, the browser, Save As).
    let one = crate::io::read_mailbox(&crate::io::write_mailbox(&mb).unwrap()).unwrap();
    assert_eq!(one.message(large).unwrap().list_size(), Some(62_914_560));
}

/// A mailbox file written before the size was kept: the message has only its header, which is
/// not believed, and the file opens and saves as it is, with no new format.
#[test]
fn a_file_from_before_opens_as_it_is() {
    let tmp = Temp::new("before");
    let path = tmp.0.join("mailbox.mailcraft");
    let (mb, large, _) = mailbox();
    let mut file = serde_json::to_value(&mb).unwrap();
    fn forget(v: &mut Value) {
        match v {
            Value::Object(o) => {
                o.remove("notDownloaded");
                o.values_mut().for_each(forget);
            }
            Value::Array(a) => a.iter_mut().for_each(forget),
            _ => {}
        }
    }
    forget(&mut file);
    let bytes = serde_json::to_vec(&file).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("notDownloaded"));
    fs::write(&path, &bytes).unwrap();
    let mut back = reopened(&path);
    let m = back.message(large).unwrap();
    assert_eq!((m.not_downloaded, m.list_size()), (0, None));
    assert!(m.headers.iter().any(|(name, _)| name == NOT_DOWNLOADED));
    // Changed and saved, it is a file like any other and still says nothing it doesn't know.
    back.message_mut(large).unwrap().read = true;
    save(&path, &back).unwrap();
    let again = reopened(&path);
    assert!(again == back);
    assert_eq!(again.message(large).unwrap().list_size(), None);
}
