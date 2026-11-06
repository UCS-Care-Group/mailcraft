// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A .pst import's memory (#815): the attachments of each batch go to the mailbox's content folder
//! as the batch comes in, once per distinct file; a mailbox with no file shares duplicate bytes
//! and stops at a budget; content written ahead is safe from a sweep until a save names it, comes
//! back byte for byte after the save and a reopen, and a batch taken out again leaves the file
//! consistent. Synthetic files by the reader's test writer, invented people at `example.org`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mailcraft_model::{Mailbox, Message};
use mailcraft_pst::test_writer::{self as w, AttachSpec, FileSpec, FolderSpec, Layout, MessageSpec};
use mailcraft_pst::{Encryption, Format, PS_PUBLIC_STRINGS};
use mailcraft_time::{Date, DateTime, Timestamp};
use serde_json::{Value as Json, json};

use crate::import_pst::{MEMORY_BUDGET, Staged, run_staged};
use crate::store::{self, GRACE};
use crate::{Part, Session};

/// A folder of its own under the system's temporary one, removed when the test is over.
pub(crate) struct Temp(pub(crate) PathBuf);

impl Temp {
    pub(crate) fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-pstmem-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    pub(crate) fn mailbox(&self) -> PathBuf {
        self.0.join("mailbox.mailcraft")
    }

    pub(crate) fn pst(&self, bytes: &[u8]) -> String {
        let path = self.0.join("old-mail.pst");
        std::fs::write(&path, bytes).unwrap();
        path.display().to_string()
    }

    /// How many content files there are beside the mailbox file.
    pub(crate) fn blobs(&self) -> usize {
        let dir = store::blob_dir(&self.mailbox());
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|sub| std::fs::read_dir(sub.path()).map(|d| d.flatten().filter(|e| e.file_name().len() == 64).count()).unwrap_or(0))
            .sum()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn now() -> Timestamp {
    DateTime::new(Date::new(2026, 11, 5).unwrap(), 10, 0, 0).unwrap().to_timestamp(0)
}

/// Bytes that are the same for the same seed and look like nothing in particular.
pub(crate) fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        out.extend_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    out.truncate(len);
    out
}

pub(crate) fn mail(subject: &str, files: Vec<(String, Vec<u8>)>) -> MessageSpec {
    MessageSpec {
        class: "IPM.Note".into(),
        subject: subject.into(),
        body: format!("{subject}\r\n\r\nWritten for the tests."),
        delivered: 134_067_498_000_000_000,
        attachments: files.into_iter().map(|(name, data)| AttachSpec::File { name, mime: "application/octet-stream".into(), data }).collect(),
        ..Default::default()
    }
}

pub(crate) fn file_of(messages: Vec<MessageSpec>) -> Vec<u8> {
    w::build(&FileSpec {
        layout: Layout::new(Format::Unicode, Encryption::Permute),
        password: None,
        password_ansi: false,
        name: "Personal Folders".into(),
        folders: vec![FolderSpec { name: "Inbox".into(), class: Some("IPF.Note".into()), messages, children: vec![] }],
        named: vec![],
        named_strings: vec![(PS_PUBLIC_STRINGS, "Keywords".into())],
    })
}

/// Distinct attachments of `each` bytes on `n` messages: `n` distinct files, then `(seed, i)`.
pub(crate) fn distinct(n: usize, each: usize) -> Vec<u8> {
    file_of((0..n).map(|i| mail(&format!("Letter {i}"), vec![(format!("file{i}.bin"), noise(i as u64 + 1, each))])).collect())
}

pub(crate) fn kept_session(dir: &Temp) -> Session {
    let mut s = Session::at(Mailbox::with_local_account("", ""), now(), 0);
    // A mailbox that is a file already (as an opened one is): a sweep can read what it names.
    store::save(&dir.mailbox(), &s.mailbox).unwrap();
    s.path = Some(dir.mailbox());
    s
}

pub(crate) fn import(s: &mut Session, pst: &str) -> Result<Json, crate::CmdError> {
    run_staged(s, pst, None, false, &json!({}))
}

pub(crate) fn messages(s: &Session) -> Vec<&Arc<Message>> {
    s.mailbox.messages.values().collect()
}

pub(crate) fn in_memory(s: &Session) -> usize {
    s.mailbox.messages.values().flat_map(|m| m.attachments.iter()).map(|a| a.data.len()).sum()
}

#[test]
fn attachments_go_to_the_content_folder_and_come_back_byte_for_byte() {
    let dir = Temp::new("ahead");
    let mut s = kept_session(&dir);
    let pst = dir.pst(&distinct(7, 5000));
    let answer = import(&mut s, &pst).unwrap();
    assert_eq!(answer["added"], 7);
    assert_eq!(in_memory(&s), 0, "nothing stays in memory");
    assert_eq!(dir.blobs(), 7);
    let mut seen = 0;
    for (id, m) in &s.mailbox.messages {
        let a = &m.attachments[0];
        let r = a.kept.expect("named by its content");
        assert_eq!(r.len, 5000);
        let n: u64 = m.subject.trim_start_matches("Letter ").parse().unwrap();
        assert_eq!(s.content(*id, Part::Attachment(0)).unwrap().as_slice(), noise(n + 1, 5000).as_slice());
        seen += 1;
    }
    assert_eq!(seen, 7);
    // Saved and opened again, the same bytes.
    s.run("file.save", &json!({})).unwrap();
    drop(s);
    let mut mb = store::open(&dir.mailbox()).unwrap().mailbox;
    store::load_all(&dir.mailbox(), &mut mb).unwrap();
    assert_eq!(mb.messages.len(), 7);
    for m in mb.messages.values() {
        let n: u64 = m.subject.trim_start_matches("Letter ").parse().unwrap();
        assert_eq!(m.attachments[0].data.as_slice(), noise(n + 1, 5000).as_slice());
    }
}

#[test]
fn content_written_ahead_is_not_swept_before_a_save_names_it() {
    let dir = Temp::new("sweep");
    let mut s = kept_session(&dir);
    let pst = dir.pst(&distinct(5, 3000));
    import(&mut s, &pst).unwrap();
    assert_eq!(dir.blobs(), 5);
    // Even a sweep that takes anything older than a moment finds the mailbox file naming none of
    // it, and leaves it, because the messages that name it are held.
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(store::sweep(&dir.mailbox(), Duration::from_millis(1)).unwrap().removed, 0);
    assert_eq!(dir.blobs(), 5);
    s.run("file.save", &json!({})).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(store::sweep(&dir.mailbox(), Duration::from_millis(1)).unwrap().removed, 0);
    assert_eq!(dir.blobs(), 5);
    // (A real sweep waits `GRACE` as well.)
    assert!(GRACE > Duration::from_secs(1));
}

#[test]
fn a_file_carried_by_many_messages_is_written_once() {
    let dir = Temp::new("dedup");
    let same = noise(99, 4000);
    let all = (0..9).map(|i| mail(&format!("Letter {i}"), vec![("same.bin".into(), same.clone())])).collect();
    let pst = dir.pst(&file_of(all));
    let mut s = kept_session(&dir);
    import(&mut s, &pst).unwrap();
    assert_eq!(s.mailbox.messages.len(), 9);
    assert_eq!(dir.blobs(), 1);
    for (id, _) in s.mailbox.messages.clone() {
        assert_eq!(s.content(id, Part::Attachment(0)).unwrap().as_slice(), same.as_slice());
    }
}

#[test]
fn without_a_file_duplicates_share_one_copy() {
    let dir = Temp::new("share");
    let same = noise(7, 4000);
    let all = (0..9).map(|i| mail(&format!("Letter {i}"), vec![("same.bin".into(), same.clone())])).collect();
    let pst = dir.pst(&file_of(all));
    let mut s = Session::at(Mailbox::with_local_account("", ""), now(), 0);
    import(&mut s, &pst).unwrap();
    let all = messages(&s);
    assert_eq!(all.len(), 9);
    for m in &all {
        assert!(Arc::ptr_eq(&m.attachments[0].data, &all[0].attachments[0].data), "one copy of the bytes");
        assert_eq!(m.attachments[0].data.as_slice(), same.as_slice());
        assert!(m.attachments[0].kept.is_none());
    }
    assert_eq!(dir.blobs(), 0, "no content folder is made");
}

#[test]
fn attachments_past_the_budget_stay_within_it() {
    let dir = Temp::new("budget");
    // Six files of 400 KB: 2.4 MB of distinct attachments against a budget of 1 MB.
    let each = 400 << 10;
    assert!(6 * each as u64 > MEMORY_BUDGET);
    let pst = dir.pst(&distinct(6, each));
    // With a file beside the mailbox: all come in, and none is held.
    let mut s = kept_session(&dir);
    let answer = import(&mut s, &pst).unwrap();
    assert_eq!(answer["added"], 6);
    assert_eq!(in_memory(&s), 0);
    // Without one: the import stops at the budget with its words, and what is in is within it.
    let mut bare = Session::at(Mailbox::with_local_account("", ""), now(), 0);
    let err = import(&mut bare, &pst).unwrap_err().to_string();
    assert!(err.contains("too large to import"), "{err}");
    assert!(in_memory(&bare) as u64 <= MEMORY_BUDGET, "{}", in_memory(&bare));
}

#[test]
fn a_batch_taken_out_again_leaves_the_file_consistent() {
    let dir = Temp::new("rollback");
    let mut s = kept_session(&dir);
    let pst = dir.pst(&distinct(9, 3000));
    // The batches by hand: two go in, then the folder they go into is taken away and the third fails.
    let (batches, outcome) = {
        let mut batches = Vec::new();
        let outcome = crate::import_pst::read(
            std::io::Cursor::new(std::fs::read(&pst).unwrap()),
            &crate::import_pst::job(&s, &pst, None, false, &json!({})).unwrap().0,
            mailcraft_pst::Limits::default(),
            &crate::import_pst::Progress::default(),
            &mut |b| {
                batches.push(b);
                Ok(())
            },
        )
        .unwrap();
        (batches, outcome)
    };
    assert!(batches.len() >= 3);
    let params = json!({"staged": true, "name": "old-mail"});
    let mut batches = batches.into_iter();
    for _ in 0..2 {
        s.staged_pst = Some(Staged::Batch(batches.next().unwrap()));
        s.run("file.importPst", &params).unwrap();
    }
    let before = s.mailbox.messages.len();
    assert!(before > 0);
    let top = s.mailbox.folders.iter().find(|f| f.name == "old-mail").unwrap().id;
    s.mailbox.remove_folders(&[top]);
    s.staged_pst = Some(Staged::Batch(batches.next().unwrap()));
    assert!(s.run("file.importPst", &params).is_err());
    s.staged_pst = None;
    drop(outcome);
    crate::import_pst::end_staged(&mut s);
    // What is in is whole, the content it names is on disk, and a save and a reopen agree.
    s.run("file.save", &json!({})).unwrap();
    let mut mb = store::open(&dir.mailbox()).unwrap().mailbox;
    store::load_all(&dir.mailbox(), &mut mb).unwrap();
    assert_eq!(mb.messages.len(), s.mailbox.messages.len());
    assert!(mb.messages.values().all(|m| m.attachments[0].data.len() == 3000));
}

#[test]
fn a_run_that_fails_leaves_nothing_and_the_next_is_whole() {
    let dir = Temp::new("failed");
    let mut s = kept_session(&dir);
    let all: Vec<MessageSpec> = (0..8).map(|i| mail(&format!("Letter {i}"), vec![(format!("f{i}.bin"), noise(i as u64 + 1, 2000))])).collect();
    let pst = dir.pst(&file_of(all));
    // An import under a folder that isn't there says no, cleanly.
    let missing = json!({"places": {"mail": 424_242}});
    assert!(run_staged(&mut s, &pst, None, false, &missing).is_err());
    assert!(s.mailbox.messages.is_empty());
    assert!(!s.importing_pst());
    // And an ordinary run afterwards is whole.
    import(&mut s, &pst).unwrap();
    assert_eq!(s.mailbox.messages.len(), 8);
}

#[test]
fn hostile_paths_and_params_say_so() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), now(), 0);
    assert!(run_staged(&mut s, "/nonexistent/none.pst", None, false, &json!({})).is_err());
    assert!(run_staged(&mut s, "", None, true, &json!({"places": 5})).is_err());
    assert!(!s.importing_pst());
}
