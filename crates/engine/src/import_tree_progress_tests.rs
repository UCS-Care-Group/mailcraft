// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The off-thread half of `file.importFolderTree` (#420): reading and parsing without a
//! `Session`, progress that advances, cancelling that changes nothing, and a staged tree that is
//! put in as one undo step.

use mailcraft_time::{Date, DateTime, Timestamp};
use proptest::prelude::*;
use serde_json::{Value, json};

use crate::Session;
use crate::import_tree::{Progress, finish, load_entries};

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
            "From a@example.org Mon Oct 12 10:00:00 2026\nFrom: a@example.org\nSubject: {tag} {i}\nMessage-ID: <{tag}-{i}@example.org>\n\nBody\n\n"
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

fn entries() -> Vec<Value> {
    vec![
        json!({"path": "M/Inbox", "data": b64(&mbox(3, "in"))}),
        json!({"path": "M/Inbox.sbd/Sub", "data": b64(&mbox(2, "su"))}),
        json!({"path": "M/notes.txt", "data": b64(b"hi")}),
    ]
}

fn fingerprint(s: &Session) -> (usize, usize) {
    (s.mailbox.messages.len(), s.mailbox.folders.len())
}

#[test]
fn progress_counts_advance_as_the_tree_is_read() {
    let p = Progress::default();
    assert_eq!(p.counts(), (0, 0, 0));
    let parsed = load_entries(&entries(), Some("T"), false, &p).unwrap();
    assert_eq!(p.counts(), (0, 2, 5));
    let mut s = session();
    let r = finish(&mut s, parsed, true).unwrap();
    assert_eq!(r["messages"], 5);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_walk_counts_folders_files_and_messages() {
    let root = std::env::temp_dir().join(format!("mc-420-progress-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Inbox.sbd")).unwrap();
    std::fs::write(root.join("Inbox"), mbox(3, "a")).unwrap();
    std::fs::write(root.join("Inbox.sbd/Sub"), mbox(2, "b")).unwrap();
    let p = Progress::default();
    let parsed = crate::import_tree::load_path(root.to_str().unwrap(), None, true, &p).unwrap();
    assert_eq!(p.counts(), (2, 2, 5));
    // Only counting: nothing can be imported from it.
    let mut s = session();
    let before = fingerprint(&s);
    assert!(finish(&mut s, parsed, false).is_err());
    assert_eq!(fingerprint(&s), before);
    // Cancelled before the walk starts: an error, nothing read.
    let c = Progress::default();
    c.cancel();
    assert!(crate::import_tree::load_path(root.to_str().unwrap(), None, false, &c).is_err());
    assert_eq!(c.counts(), (0, 0, 0));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancelling_a_scan_or_an_import_changes_nothing() {
    let mut s = session();
    let before = fingerprint(&s);
    assert!(!s.can_undo());
    for count_only in [true, false] {
        let p = Progress::default();
        p.cancel();
        assert!(load_entries(&entries(), Some("T"), count_only, &p).is_err());
        assert_eq!(p.counts().2, 0);
    }
    // A cancel that arrives while messages are being read ends it with nothing to apply.
    let big = vec![json!({"path": "Big", "data": b64(&mbox(5_000, "big"))})];
    let p = std::sync::Arc::new(Progress::default());
    let watcher = {
        let p = p.clone();
        std::thread::spawn(move || {
            while p.counts().2 == 0 {
                std::thread::yield_now();
            }
            p.cancel();
        })
    };
    let r = load_entries(&big, None, false, &p);
    let _ = watcher.join();
    if r.is_err() {
        assert!(p.counts().2 < 5_000);
    }
    assert_eq!(fingerprint(&s), before);
    assert!(!s.can_undo());
    assert!(s.staged_tree.is_none());
    // And a staged import with nothing staged is an error, not a crash.
    assert!(s.run("file.importFolderTree", &json!({"staged": true})).is_err());
    assert_eq!(fingerprint(&s), before);
}

#[test]
fn a_staged_tree_is_one_undo_step() {
    let mut s = session();
    let before = fingerprint(&s);
    assert!(!s.can_undo());
    s.staged_tree = Some(load_entries(&entries(), Some("Staged"), false, &Progress::default()).unwrap());
    let r = s.run("file.importFolderTree", &json!({"staged": true})).unwrap();
    assert_eq!(r["messages"], 5);
    assert!(s.staged_tree.is_none());
    assert!(s.can_undo());
    assert!(s.undo());
    assert!(!s.can_undo());
    assert_eq!(fingerprint(&s), before);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn arbitrary_names_and_bytes_never_panic(
        files in proptest::collection::vec((".{0,40}", proptest::collection::vec(any::<u8>(), 0..600), any::<bool>()), 0..8),
        count_only in any::<bool>(),
    ) {
        let list: Vec<Value> = files
            .iter()
            .map(|(name, bytes, mboxy)| {
                let mut data = if *mboxy { b"From a@example.org Mon Oct 12 10:00:00 2026\nSubject: x\n\n".to_vec() } else { Vec::new() };
                data.extend_from_slice(bytes);
                json!({"path": name, "data": b64(&data)})
            })
            .collect();
        let p = Progress::default();
        if let Ok(parsed) = load_entries(&list, None, count_only, &p) {
            let mut s = session();
            let r = finish(&mut s, parsed, count_only);
            prop_assert!(r.is_ok() || !count_only);
        }
    }
}
