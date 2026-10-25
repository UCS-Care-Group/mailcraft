// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `file.importFolderTree` (#420): a tree of mbox files comes in as subfolders, found by their
//! bytes, within caps, and nothing in a name or a file can climb out or crash it.

use mailcraft_model::{AccountKind, FolderKind, Id};
use mailcraft_time::{Date, DateTime, Timestamp};
use serde_json::{Value, json};

use crate::Session;
use crate::import_tree::{looks_like_mbox, tidy_name};

fn now() -> Timestamp {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn session() -> Session {
    Session::at(crate::sample::mailbox(now(), 0), now(), 0)
}

fn mbox(n: usize, tag: &str) -> Vec<u8> {
    let mut out = String::new();
    for i in 0..n {
        out.push_str(&format!(
            "From a@example.org Mon Oct 12 10:00:00 2026\nFrom: a@example.org\nTo: b@example.org\nSubject: {tag} {i}\nMessage-ID: <{tag}-{i}@example.org>\nDate: Mon, 12 Oct 2026 10:00:00 +0000\n\nBody {i}\n\n"
        ));
    }
    out.into_bytes()
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, sh) in [18u32, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= c.len() { T[((n >> sh) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

fn entry(path: &str, data: &[u8]) -> Value {
    json!({"path": path, "data": b64(data)})
}

fn tree() -> Vec<Value> {
    vec![
        entry("Mail/Inbox", &mbox(3, "in")),
        entry("Mail/Inbox.msf", b"// <!-- <mdb:mork:z v=\"1.4\"/> -->"),
        entry("Mail/Inbox.sbd/Receipts", &mbox(2, "rc")),
        entry("Mail/Inbox.sbd/Receipts.msf", b"junk"),
        entry("Mail/Sent.mbox", &mbox(1, "se")),
        entry("Mail/.lock", b""),
        entry("Mail/notes.txt", b"hello"),
    ]
}

fn folder_named(s: &Session, parent: Option<Id>, name: &str) -> Option<Id> {
    s.mailbox.folders.iter().find(|f| f.parent == parent && f.name == name && f.kind == FolderKind::Custom).map(|f| f.id)
}

#[test]
fn a_tree_comes_in_as_subfolders_under_one_new_folder() {
    let mut s = session();
    let before = s.mailbox.messages.len();
    let r = s.run("file.importFolderTree", &json!({"entries": tree(), "name": "Old mail"})).unwrap();
    assert_eq!(r["messages"], 6, "{r}");
    assert_eq!(r["skippedCount"], 4, "{r}");
    assert_eq!(s.mailbox.messages.len(), before + 6);
    let top = folder_named(&s, None, "Old mail").unwrap();
    let local = s.mailbox.folder(top).unwrap().account;
    assert_eq!(s.mailbox.account(local).unwrap().kind, AccountKind::Local);
    let mail = folder_named(&s, Some(top), "Mail").unwrap();
    let inbox = folder_named(&s, Some(mail), "Inbox").unwrap();
    assert_eq!(s.mailbox.count(inbox), 3);
    assert_eq!(s.mailbox.count(folder_named(&s, Some(inbox), "Receipts").unwrap()), 2);
    assert_eq!(s.mailbox.count(folder_named(&s, Some(mail), "Sent").unwrap()), 1);
    assert!(!s.mailbox.is_send_queue(top));
    // One undo step takes it all back.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.messages.len(), before);
    assert!(folder_named(&s, None, "Old mail").is_none());
}

#[test]
fn a_dry_run_changes_nothing_and_says_what_would_come() {
    let mut s = session();
    let before = s.mailbox.clone();
    let steps = s.history.len();
    let r = s.run("file.importFolderTree", &json!({"entries": tree(), "dryRun": true, "name": "X"})).unwrap();
    assert_eq!(r["dryRun"], true);
    assert_eq!(r["messages"], 6);
    assert_eq!(r["folders"].as_array().unwrap().len(), 3);
    assert!(s.mailbox == before);
    assert_eq!(s.history.len(), steps);
}

#[test]
fn a_second_import_adds_nothing_already_here() {
    let mut s = session();
    s.run("file.importFolderTree", &json!({"entries": tree(), "name": "Old mail"})).unwrap();
    let n = s.mailbox.messages.len();
    let r = s.run("file.importFolderTree", &json!({"entries": tree(), "name": "Old mail"})).unwrap();
    assert_eq!(r["messages"], 0);
    assert_eq!(r["duplicates"], 6);
    assert_eq!(s.mailbox.messages.len(), n);
    assert_eq!(s.mailbox.folders.iter().filter(|f| f.name == "Old mail").count(), 1);
}

#[test]
fn a_file_is_an_mbox_by_its_bytes_not_its_name() {
    assert!(looks_like_mbox(&mbox(1, "a")));
    assert!(!looks_like_mbox(b"From the desk of nobody\nno header here"));
    assert!(!looks_like_mbox(b"Subject: x\n\nbody"));
    assert!(!looks_like_mbox(b""));
    let mut s = session();
    let files = vec![entry("plain.txt", &mbox(2, "t")), entry("Real.mbox", b"not mail at all")];
    let r = s.run("file.importFolderTree", &json!({"entries": files, "name": "N"})).unwrap();
    assert_eq!(r["messages"], 2);
    assert_eq!(r["skippedCount"], 1);
}

#[test]
fn hostile_names_are_made_harmless() {
    assert_eq!(tidy_name(".."), "Folder");
    assert_eq!(tidy_name("."), "Folder");
    assert_eq!(tidy_name("a\0b\r\nc/d\\e"), "a b  c d e");
    assert_eq!(tidy_name(&"é".repeat(10_000)).chars().count(), 120);
    assert_eq!(tidy_name(""), "Folder");
    let mut s = session();
    let files = vec![
        entry("../../etc/passwd", &mbox(1, "p")),
        entry("a/./../b\0c/Outbox", &mbox(1, "o")),
        entry(&format!("{}/x", "d/".repeat(200)), &mbox(1, "deep")),
        entry(&"n".repeat(100_000), &mbox(1, "long")),
        entry("", &mbox(1, "e")),
        json!({"path": 5, "data": 5}),
        json!(null),
        json!({"path": "k", "data": "!!!"}),
    ];
    let r = s.run("file.importFolderTree", &json!({"entries": files, "name": "../..\0"})).unwrap();
    assert_eq!(r["messages"], 3, "{r}");
    // Whatever the names, nothing is an Outbox or lands outside the one new folder.
    let top = folder_named(&s, None, ".. ..").unwrap();
    for f in &s.mailbox.folders {
        assert!(f.kind != FolderKind::Outbox || f.parent.is_none() && f.id != top);
    }
    for m in s.mailbox.messages.values().filter(|m| m.message_id.contains("@example.org") && m.subject.starts_with(['p', 'o', 'l'])) {
        assert!(s.mailbox.subtree(top).contains(&m.folder), "{}", m.subject);
        assert!(!s.mailbox.is_send_queue(m.folder));
    }
}

#[test]
fn a_folder_named_outbox_in_the_tree_is_not_the_outbox() {
    let mut s = session();
    let outbox = s.mailbox.folders.iter().find(|f| f.kind == FolderKind::Outbox).map(|f| f.id);
    let files = vec![entry("Outbox", &mbox(2, "ob"))];
    let r = s.run("file.importFolderTree", &json!({"entries": files, "name": "Outbox"})).unwrap();
    assert_eq!(r["messages"], 2);
    if let Some(outbox) = outbox {
        assert_eq!(s.mailbox.count(outbox), s.mailbox.messages_in(outbox).count());
        assert!(s.mailbox.messages_in(outbox).all(|m| !m.subject.starts_with("ob ")));
    }
    assert!(s.mailbox.messages.values().filter(|m| m.subject.starts_with("ob ")).all(|m| !s.mailbox.is_send_queue(m.folder)));
}

#[test]
fn caps_give_a_partial_import_with_a_note() {
    let mut s = session();
    let deep = format!("{}f", "d/".repeat(crate::import_tree::MAX_TREE_DEPTH + 3));
    let r = s.run("file.importFolderTree", &json!({"entries": [entry(&deep, &mbox(1, "deep")), entry("ok", &mbox(1, "ok"))], "name": "C"})).unwrap();
    assert_eq!(r["messages"], 1);
    assert_eq!(r["skipped"][0]["why"], "nested too deeply");
}

#[test]
fn nothing_to_import_is_an_error_not_a_crash() {
    let mut s = session();
    assert!(s.run("file.importFolderTree", &json!({})).is_err());
    assert!(s.run("file.importFolderTree", &json!({"path": "/nonexistent-dir-420"})).is_err());
    assert!(s.run("file.importFolderTree", &json!({"path": "\0"})).is_err());
    assert!(s.run("file.importFolderTree", &json!({"entries": 5})).is_err());
}

#[cfg(not(target_arch = "wasm32"))]
mod on_disk {
    use super::*;

    fn write(root: &std::path::Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mc-420-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_directory_is_walked_and_named_after_itself() {
        let root = scratch("walk").join("Profile Mail");
        write(&root, "Inbox", &mbox(2, "i"));
        write(&root, "Inbox.sbd/Sub", &mbox(1, "s"));
        write(&root, "Inbox.msf", b"index");
        write(&root, "empty/readme", b"x");
        let mut s = session();
        let r = s.run("file.importFolderTree", &json!({"path": root.to_string_lossy()})).unwrap();
        assert_eq!(r["name"], "Profile Mail");
        assert_eq!(r["messages"], 3, "{r}");
        let top = folder_named(&s, None, "Profile Mail").unwrap();
        let inbox = folder_named(&s, Some(top), "Inbox").unwrap();
        assert!(folder_named(&s, Some(inbox), "Sub").is_some());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_followed() {
        let base = scratch("link");
        let root = base.join("tree");
        let outside = base.join("outside");
        write(&outside, "Secret", &mbox(2, "x"));
        write(&root, "Real", &mbox(1, "r"));
        std::os::unix::fs::symlink(&outside, root.join("dir-link")).unwrap();
        std::os::unix::fs::symlink(outside.join("Secret"), root.join("file-link")).unwrap();
        let mut s = session();
        let r = s.run("file.importFolderTree", &json!({"path": root.to_string_lossy()})).unwrap();
        assert_eq!(r["messages"], 1, "{r}");
        assert_eq!(r["skippedCount"], 2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_walk_over_arbitrary_bytes_never_panics() {
        let root = scratch("fuzz");
        let mut x = 0x2545_f491u32;
        for i in 0..60 {
            let mut bytes = if i % 3 == 0 { b"From x\nA: b\n\n".to_vec() } else { Vec::new() };
            for _ in 0..(i * 7) {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                bytes.push((x & 0xff) as u8);
            }
            write(&root, &format!("d{}/f{i}", i % 4), &bytes);
        }
        let mut s = session();
        assert!(s.run("file.importFolderTree", &json!({"path": root.to_string_lossy()})).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }
}
