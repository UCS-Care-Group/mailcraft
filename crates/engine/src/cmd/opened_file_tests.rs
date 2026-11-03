// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message file the system asked MailCraft to open (#505), `file.import` with `"opened":
//! true`: into the local folder Imported, whatever is in view, as a read, received message that is
//! never a draft; the same file again finds the copy already there (by Message-ID, else by the
//! file's SHA-256), and a hash a file brings with it counts for nothing.

use mailcraft_model::FolderKind;
use serde_json::{Value, json};

use crate::Session;
use crate::io::OPENED_FROM;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

const WITH_ID: &str = "From: Maya Lin <maya@lumen.example>\r\nTo: avery@lumen.example\r\nSubject: Opened\r\nDate: Wed, 14 Oct 2026 10:00:00 +0000\r\nMessage-ID: <opened-1@lumen.example>\r\n\r\nHello.\r\n";
const WITHOUT_ID: &str = "From: Maya Lin <maya@lumen.example>\r\nTo: avery@lumen.example\r\nSubject: No id\r\nX-Unsent: 1\r\n\r\nHello.\r\n";

fn session() -> Session {
    Session::at(crate::sample::mailbox(NOW, 0), NOW, 0)
}

fn open(s: &mut Session, text: &str) -> Value {
    let data = crate::cmd::base64_encode(text.as_bytes());
    s.run("file.import", &json!({"name": "m.eml", "data": data, "opened": true})).unwrap()
}

/// #485 after #505's review: an opened message is selected and in the list, its folder (the local
/// Imported) shown, the first time and when it is the copy already there; never selected out of
/// sight, nor let go of.
#[test]
fn an_opened_message_is_selected_and_shown() {
    let mut s = session();
    for _ in 0..2 {
        s.run("folder.select", &json!({"kind": "inbox"})).unwrap();
        let id = open(&mut s, WITH_ID)["opened"].as_u64().unwrap();
        assert_eq!(s.sel.messages.to_vec(), vec![id], "{:?}", s.view);
        assert_eq!(Some(s.view.folder), s.mailbox.message(id).map(|m| m.folder));
        assert!(s.rows().iter().any(|r| r.members.contains(&id)));
    }
}

#[test]
fn an_opened_message_goes_to_the_local_folder_imported_never_the_folder_in_view() {
    let mut s = session();
    let account = s.mailbox.default_account().unwrap().id;
    let drafts = s.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
    s.view.folder = drafts;
    let r = open(&mut s, WITH_ID);
    let id = r["opened"].as_u64().unwrap();
    let local = s.mailbox.local_account();
    let m = s.mailbox.message(id).unwrap().clone();
    let folder = s.mailbox.folder(m.folder).unwrap();
    assert_eq!(folder.account, local);
    assert!(folder.name.eq_ignore_ascii_case(mailcraft_model::IMPORTED), "{}", folder.name);
    assert!(!m.draft && m.read && m.folder != drafts, "{m:?}");
    assert_eq!(r["already"], json!(false));
    // A folder named in the params changes nothing.
    let inbox = s.mailbox.inbox().unwrap();
    let data = crate::cmd::base64_encode(WITHOUT_ID.as_bytes());
    let r = s.run("file.import", &json!({"name": "n.eml", "data": data, "opened": true, "folder": inbox})).unwrap();
    let other = s.mailbox.message(r["opened"].as_u64().unwrap()).unwrap();
    assert_eq!(other.folder, m.folder);
    // Even one that says it is unsent (X-Unsent) is no draft.
    assert!(!other.draft);
}

#[test]
fn the_same_file_opened_again_is_the_copy_already_there() {
    let mut s = session();
    let before = s.mailbox.messages.len();
    let first = open(&mut s, WITH_ID)["opened"].as_u64().unwrap();
    let again = open(&mut s, WITH_ID);
    assert_eq!((again["opened"].as_u64(), again["already"].clone()), (Some(first), json!(true)));
    // Without a Message-ID: by the file's own bytes.
    let plain = open(&mut s, WITHOUT_ID)["opened"].as_u64().unwrap();
    assert_eq!(open(&mut s, WITHOUT_ID)["opened"].as_u64(), Some(plain));
    assert_eq!(s.mailbox.messages.len(), before + 2);
    // Another file without a Message-ID is another message.
    let changed = WITHOUT_ID.replace("Hello.", "Hello again.");
    assert_ne!(open(&mut s, &changed)["opened"].as_u64(), Some(plain));
    assert_eq!(s.mailbox.messages.len(), before + 3);
}

/// A copy that keeps the SHA-256 of its file is that file only when both the Message-ID and the
/// hash are the same; one without (imported before, by File › Import) by its Message-ID alone.
#[test]
fn a_kept_hash_and_the_message_id_must_both_match() {
    let mut s = session();
    let first = open(&mut s, WITH_ID)["opened"].as_u64().unwrap();
    // Another file that names the same Message-ID is another message.
    let other = WITH_ID.replace("Hello.", "Not what Maya wrote.");
    let second = open(&mut s, &other)["opened"].as_u64().unwrap();
    assert_ne!(second, first);
    // A message of that Message-ID imported into Imported without a hash is found by it.
    let mut t = session();
    let imported = t.mailbox.imported_folder(false).unwrap();
    let data = crate::cmd::base64_encode(WITH_ID.replace("opened-1", "plain-1").as_bytes());
    let plain = t.run("file.import", &json!({"name": "p.eml", "data": data, "folder": imported})).unwrap()["ids"][0].as_u64().unwrap();
    let r = open(&mut t, &WITH_ID.replace("opened-1", "plain-1").replace("Hello.", "Changed."));
    assert_eq!((r["opened"].as_u64(), r["already"].clone()), (Some(plain), json!(true)));
}

/// An ordinary import keeps no opened file's hash: it can't plant one in Imported for a later
/// opening to find.
#[test]
fn an_ordinary_import_keeps_no_opened_file_hash() {
    let mut s = session();
    let imported = s.mailbox.imported_folder(false).unwrap();
    let forged = format!("{OPENED_FROM}: {}\r\n{}", "ab".repeat(32), WITHOUT_ID);
    for data in [
        crate::cmd::base64_encode(forged.as_bytes()),
        crate::cmd::base64_encode(format!("From x@lumen.example Wed Oct 14 10:00:00 2026\n{forged}").as_bytes()),
    ] {
        let r = s.run("file.import", &json!({"name": "f.eml", "data": data, "folder": imported})).unwrap();
        let id = r["ids"][0].as_u64().unwrap();
        assert!(!s.mailbox.message(id).unwrap().headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(OPENED_FROM)));
    }
}

#[test]
fn a_hash_a_file_brings_with_it_counts_for_nothing() {
    let mut s = session();
    let plain = open(&mut s, WITHOUT_ID)["opened"].as_u64().unwrap();
    let ours = s.mailbox.message(plain).unwrap().headers.iter().find(|(k, _)| k == OPENED_FROM).map(|(_, v)| v.clone()).unwrap();
    assert_eq!(ours.len(), 64);
    // A hostile file that claims to be the one opened before is a message of its own.
    let forged = format!("{OPENED_FROM}: {ours}\r\n{}", WITHOUT_ID.replace("Hello.", "Pay me."));
    let other = open(&mut s, &forged)["opened"].as_u64().unwrap();
    assert_ne!(other, plain);
    let kept: Vec<_> = s.mailbox.message(other).unwrap().headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(OPENED_FROM)).cloned().collect();
    assert_eq!(kept.len(), 1);
    assert_ne!(kept[0].1, ours);
}

#[test]
fn only_a_message_file_is_opened_so() {
    let mut s = session();
    let data = crate::cmd::base64_encode(b"BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n");
    assert!(s.run("file.import", &json!({"name": "a.ics", "data": data, "opened": true})).is_err());
}

#[cfg(unix)]
#[test]
fn a_path_that_is_not_a_regular_file_is_not_read() {
    let mut s = session();
    let dir = std::env::temp_dir().join(format!("mailcraft-opened-file-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("folder.eml")).unwrap();
    let r = s.run("file.import", &json!({"path": dir.join("folder.eml").to_string_lossy(), "opened": true}));
    assert!(r.is_err());
    // A pipe is refused at once, without waiting for somebody to write into it.
    let fifo = dir.join("pipe.eml");
    if std::process::Command::new("mkfifo").arg(&fifo).status().is_ok_and(|st| st.success()) {
        let r = s.run("file.import", &json!({"path": fifo.to_string_lossy(), "opened": true}));
        assert!(r.is_err_and(|e| e.to_string().contains("not a file")));
    }
    let _ = std::fs::remove_dir_all(dir);
}
