// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A .pst import's memory, part two (#815): the reader's thread writes the attachments ahead, content
//! written ahead is protected by its name until a save names it (changed messages, Undo, a batch
//! taken out, a run that never saved), what an import adds to memory is counted honestly, Save As
//! links the content instead of reading it, and a reader that panics is an error.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mailcraft_model::{Mailbox, Message};
use mailcraft_pst::test_writer::MessageSpec;
use serde_json::json;

use crate::Session;
use crate::import_pst::{Staged, run_staged};
use crate::import_pst_memory_tests::{Temp, distinct, file_of, import, in_memory, kept_session, mail, noise, now};
use crate::store::{self, GRACE};

/// Every content file beside `dir`'s mailbox, made old enough for a sweep.
fn age_blobs(dir: &Temp) {
    let old = std::time::SystemTime::now() - GRACE * 2;
    let root = store::blob_dir(&dir.mailbox());
    for sub in std::fs::read_dir(root).into_iter().flatten().flatten() {
        for f in std::fs::read_dir(sub.path()).into_iter().flatten().flatten() {
            std::fs::OpenOptions::new().write(true).open(f.path()).unwrap().set_modified(old).unwrap();
        }
    }
}

fn sweep_all(dir: &Temp) -> usize {
    store::sweep(&dir.mailbox(), GRACE).unwrap().removed
}

/// The batches of reading `pst` for the session `s` (its job writes ahead when `s` has a file).
fn batches_for(s: &Session, pst: &str) -> (Vec<crate::import_pst::Batch>, crate::import_pst::Outcome) {
    let job = crate::import_pst::job(s, pst, None, false, &json!({})).unwrap().0;
    let mut batches = Vec::new();
    let outcome = crate::import_pst::read(
        std::io::Cursor::new(std::fs::read(pst).unwrap()),
        &job,
        mailcraft_pst::Limits::default(),
        &crate::import_pst::Progress::default(),
        &mut |b| {
            batches.push(b);
            Ok(())
        },
    )
    .unwrap();
    (batches, outcome)
}

fn reopened(dir: &Temp, path: &std::path::Path) -> Mailbox {
    let _ = dir;
    let mut mb = store::open(path).unwrap().mailbox;
    store::load_all(path, &mut mb).unwrap();
    mb
}

#[test]
fn a_reader_that_panics_is_an_error_not_a_crash() {
    let dir = Temp::new("panic");
    let path = dir.0.join("panic-in-reader.pst");
    std::fs::write(&path, b"not read").unwrap();
    let mut s = Session::at(Mailbox::with_local_account("", ""), now(), 0);
    let err = run_staged(&mut s, &path.display().to_string(), None, false, &json!({})).unwrap_err().to_string();
    assert!(err.contains("stopped reading"), "{err}");
    assert!(!s.importing_pst());
    assert!(s.mailbox.messages.is_empty());
}

#[test]
fn the_reader_writes_ahead_and_the_window_only_takes_batches_in() {
    let dir = Temp::new("reader");
    let mut s = kept_session(&dir);
    let pst = dir.pst(&distinct(6, 4000));
    let (batches, outcome) = batches_for(&s, &pst);
    // Before anything is applied the content is on disk and the batches name it.
    assert_eq!(dir.blobs(), 6);
    assert!(batches.iter().flat_map(|b| &b.messages).all(|(_, m)| m.attachments.iter().all(|a| a.kept.is_some() && a.data.is_empty())));
    let params = json!({"staged": true, "name": "old-mail"});
    for b in batches {
        s.staged_pst = Some(Staged::Batch(b));
        s.run("file.importPst", &params).unwrap();
    }
    s.staged_pst = Some(Staged::Done(Box::new(outcome)));
    s.run("file.importPst", &params).unwrap();
    assert_eq!(dir.blobs(), 6, "applying wrote nothing");
    assert_eq!(in_memory(&s), 0);
}

#[test]
fn content_that_went_missing_between_batches_is_written_again_and_renewed() {
    let dir = Temp::new("missing");
    let s = kept_session(&dir);
    let pst = dir.pst(&distinct(1, 3000));
    let (mut first, _) = batches_for(&s, &pst);
    let r = first[0].messages[0].1.attachments[0].kept.unwrap();
    let hex: String = r.sha256.iter().map(|b| format!("{b:02x}")).collect();
    let file = store::blob_dir(&dir.mailbox()).join(&hex[..2]).join(&hex);
    assert!(file.is_file());
    // The carriers went and a sweep took the file: the next batch with the same content writes it.
    std::fs::remove_file(&file).unwrap();
    let mut again = crate::import_pst::Spiller::new(Some(dir.mailbox()));
    let mut batch = first.remove(0);
    let refill = |batch: &mut crate::import_pst::Batch| {
        for (_, m) in &mut batch.messages {
            for a in &mut m.attachments {
                a.data = Arc::new(noise(1, 3000));
                a.kept = None;
            }
        }
    };
    refill(&mut batch);
    again.spill(&mut batch);
    assert!(file.is_file(), "written again");
    // And a file that is there has its time renewed.
    let old = std::time::SystemTime::now() - Duration::from_secs(3 * 3600);
    std::fs::OpenOptions::new().write(true).open(&file).unwrap().set_modified(old).unwrap();
    refill(&mut batch);
    again.spill(&mut batch);
    let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
    assert!(std::time::SystemTime::now().duration_since(modified).unwrap() < Duration::from_secs(60));
}

#[test]
fn content_named_only_by_unsaved_messages_survives_their_being_changed() {
    let dir = Temp::new("changed");
    let mut s = kept_session(&dir);
    let pst = dir.pst(&distinct(5, 3000));
    import(&mut s, &pst).unwrap();
    // The owner marks them read, and one is brought into memory and let go of: new messages
    // replace the imported ones, which nothing holds any more.
    let ids: Vec<u64> = s.mailbox.messages.keys().copied().collect();
    for id in &ids {
        let mut copy = Message::clone(s.mailbox.messages.get(id).unwrap());
        copy.read = true;
        s.mailbox.messages.insert(*id, Arc::new(copy));
    }
    let loaded = s.loaded_message(ids[0]).unwrap();
    drop(loaded);
    age_blobs(&dir);
    assert_eq!(sweep_all(&dir), 0, "named by messages the file doesn't have yet");
    assert_eq!(dir.blobs(), 5);
    s.run("file.save", &json!({})).unwrap();
    assert_eq!(sweep_all(&dir), 0);
    assert_eq!(dir.blobs(), 5);
    let mb = reopened(&dir, &dir.mailbox());
    assert!(mb.messages.values().all(|m| m.read && m.attachments[0].data.len() == 3000));
}

#[test]
fn undoing_an_import_then_saving_and_sweeping_keeps_the_file_whole() {
    let dir = Temp::new("undo");
    let mut s = kept_session(&dir);
    let pst = dir.pst(&distinct(5, 3000));
    import(&mut s, &pst).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.messages.is_empty());
    s.run("file.save", &json!({})).unwrap();
    age_blobs(&dir);
    // (Redo could bring the messages back: their content is not swept in this session.)
    assert_eq!(sweep_all(&dir), 0);
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.messages.len(), 5);
    s.run("file.save", &json!({})).unwrap();
    assert_eq!(reopened(&dir, &dir.mailbox()).messages.len(), 5);
    // Next session, with the import undone and saved, the orphans go.
    s.run("edit.undo", &json!({})).unwrap();
    s.run("file.save", &json!({})).unwrap();
    // (The session ends: nothing holds the undone messages any more.)
    drop(s);
    store::end_session_for_test(&dir.mailbox());
    age_blobs(&dir);
    assert_eq!(sweep_all(&dir), 5);
    assert_eq!(dir.blobs(), 0);
}

#[test]
fn cancelling_midway_keeps_what_is_in_with_its_content() {
    let dir = Temp::new("cancel");
    let mut s = kept_session(&dir);
    let pst = dir.pst(&distinct(10, 3000));
    let place = crate::import_pst::Place::New("old-mail".into());
    let progress = crate::import_pst::Progress::default();
    let job = crate::import_pst::job(&s, &pst, None, false, &json!({})).unwrap().0;
    let mut sink = |batch: crate::import_pst::Batch| -> Result<(), crate::CmdError> {
        crate::import_pst::apply(&mut s, &place, batch)?;
        progress.cancel();
        Ok(())
    };
    let outcome =
        crate::import_pst::read(std::io::Cursor::new(std::fs::read(&pst).unwrap()), &job, mailcraft_pst::Limits::default(), &progress, &mut sink)
            .unwrap();
    assert!(outcome.report.cancelled);
    crate::import_pst::finish(&mut s, &place, outcome, false).unwrap();
    let first = s.mailbox.messages.len();
    assert!(first > 0 && first < 10, "{first}");
    s.run("file.save", &json!({})).unwrap();
    let mb = reopened(&dir, &dir.mailbox());
    assert_eq!(mb.messages.len(), first);
    assert!(mb.messages.values().all(|m| m.attachments[0].data.len() == 3000));
}

#[test]
fn a_batch_that_fails_after_writing_leaves_orphans_for_the_next_session_only() {
    let dir = Temp::new("orphans");
    let mut s = kept_session(&dir);
    let pst = dir.pst(&distinct(9, 3000));
    let (batches, _) = batches_for(&s, &pst);
    assert_eq!(dir.blobs(), 9, "the reader wrote all of it ahead");
    let params = json!({"staged": true, "name": "old-mail"});
    let mut batches = batches.into_iter();
    s.staged_pst = Some(Staged::Batch(batches.next().unwrap()));
    s.run("file.importPst", &params).unwrap();
    let kept = s.mailbox.messages.len();
    let top = s.mailbox.folders.iter().find(|f| f.name == "old-mail").unwrap().id;
    s.mailbox.remove_folders(&[top]);
    s.staged_pst = Some(Staged::Batch(batches.next().unwrap()));
    assert!(s.run("file.importPst", &params).is_err());
    s.staged_pst = None;
    crate::import_pst::end_staged(&mut s);
    // (Saved with the folder gone: the file names the first batch's mail and none of the rest.)
    s.run("file.save", &json!({})).unwrap();
    drop(s);
    store::end_session_for_test(&dir.mailbox());
    age_blobs(&dir);
    assert_eq!(sweep_all(&dir), 9 - kept, "the next session sweeps what nothing names");
    let mb = reopened(&dir, &dir.mailbox());
    assert!(mb.messages.values().all(|m| m.attachments.iter().all(|a| a.data.len() == 3000)));
}

#[test]
fn after_an_unsaved_run_the_mailbox_opens_and_the_next_session_sweeps_the_litter() {
    let dir = Temp::new("crash");
    let mut s = kept_session(&dir);
    let pst = dir.pst(&distinct(6, 3000));
    import(&mut s, &pst).unwrap();
    assert_eq!(dir.blobs(), 6);
    // The program ends before any save: the file is as it was.
    drop(s);
    store::end_session_for_test(&dir.mailbox());
    let opened = store::open(&dir.mailbox()).unwrap();
    assert!(opened.mailbox.messages.is_empty());
    assert!(opened.notice().is_none());
    age_blobs(&dir);
    assert_eq!(sweep_all(&dir), 6);
    assert!(store::open(&dir.mailbox()).unwrap().notice().is_none());
}

#[test]
fn bodies_count_against_the_budget_without_a_file_and_the_error_says_what_to_do() {
    let dir = Temp::new("bodies");
    let all: Vec<MessageSpec> = (0..5)
        .map(|i| {
            let mut m = mail(&format!("Letter {i}"), vec![]);
            m.body = format!("{}\r\n", "words ".repeat(50_000));
            m
        })
        .collect();
    let pst = dir.pst(&file_of(all));
    let mut bare = Session::at(Mailbox::with_local_account("", ""), now(), 0);
    let err = import(&mut bare, &pst).unwrap_err().to_string();
    assert!(err.contains("too large to import without a mailbox file"), "{err}");
    // With a file the same bodies come in.
    let mut s = kept_session(&dir);
    assert_eq!(import(&mut s, &pst).unwrap()["added"], 5);
}

#[test]
fn a_batch_taken_out_gives_its_memory_back() {
    let dir = Temp::new("resident");
    let same = noise(3, 4000);
    let all = (0..6)
        .map(|i| mail(&format!("Letter {i}"), vec![(format!("f{i}.bin"), noise(i as u64 + 10, 4000)), ("same.bin".into(), same.clone())]))
        .collect();
    let pst = dir.pst(&file_of(all));
    let mut s = Session::at(Mailbox::with_local_account("", ""), now(), 0);
    let (batches, _) = batches_for(&s, &pst);
    let params = json!({"staged": true, "name": "old-mail"});
    let mut batches = batches.into_iter();
    s.staged_pst = Some(Staged::Batch(batches.next().unwrap()));
    s.run("file.importPst", &params).unwrap();
    let before = crate::import_pst::resident_of(&s).unwrap();
    assert!(before > 0);
    let top = s.mailbox.folders.iter().find(|f| f.name == "old-mail").unwrap().id;
    s.mailbox.remove_folders(&[top]);
    s.staged_pst = Some(Staged::Batch(batches.next().unwrap()));
    assert!(s.run("file.importPst", &params).is_err());
    assert_eq!(crate::import_pst::resident_of(&s), Some(before));
}

#[test]
fn saving_to_another_path_links_the_content_instead_of_reading_it() {
    let dir = Temp::new("saveas");
    let mut s = kept_session(&dir);
    let pst = dir.pst(&distinct(6, 5000));
    import(&mut s, &pst).unwrap();
    let other: PathBuf = dir.0.join("copy").join("other.mailcraft");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    s.run("file.save", &json!({"path": other.display().to_string()})).unwrap();
    // Still kept outside memory here, and the copy has the same content beside it.
    assert_eq!(in_memory(&s), 0);
    let copied =
        std::fs::read_dir(store::blob_dir(&other)).unwrap().flatten().map(|sub| std::fs::read_dir(sub.path()).unwrap().count()).sum::<usize>();
    assert_eq!(copied, 6);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let r = s.mailbox.messages.values().next().unwrap().attachments[0].kept.unwrap();
        let hex: String = r.sha256.iter().map(|b| format!("{b:02x}")).collect();
        let at = |root: &PathBuf| std::fs::metadata(root.join(&hex[..2]).join(&hex)).unwrap().ino();
        assert_eq!(at(&store::blob_dir(&dir.mailbox())), at(&store::blob_dir(&other)), "linked, not read and written");
    }
    let mb = reopened(&dir, &other);
    assert_eq!(mb.messages.len(), 6);
    for m in mb.messages.values() {
        let n: u64 = m.subject.trim_start_matches("Letter ").parse().unwrap();
        assert_eq!(m.attachments[0].data.as_slice(), noise(n + 1, 5000).as_slice());
    }
}

#[test]
fn the_content_of_a_run_that_never_saved_a_new_mailbox_is_removed() {
    let dir = Temp::new("unwritten");
    let mut s = Session::at(Mailbox::with_local_account("", ""), now(), 0);
    s.path = Some(dir.mailbox());
    let pst = dir.pst(&distinct(4, 3000));
    import(&mut s, &pst).unwrap();
    assert_eq!(dir.blobs(), 4);
    // A mailbox file that is there: nothing is touched.
    std::fs::write(dir.mailbox(), b"x").unwrap();
    assert_eq!(store::remove_unwritten(&dir.mailbox()), 0);
    assert_eq!(dir.blobs(), 4);
    std::fs::remove_file(dir.mailbox()).unwrap();
    assert_eq!(store::remove_unwritten(&dir.mailbox()), 4);
    assert!(!store::blob_dir(&dir.mailbox()).exists());
}

#[test]
fn a_disk_that_refuses_leaves_the_attachments_in_memory_and_under_the_budget() {
    let dir = Temp::new("refused");
    // A mailbox "file" inside a plain file: no content folder can be made beside it.
    let blocker = dir.0.join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let mut s = Session::at(Mailbox::with_local_account("", ""), now(), 0);
    s.path = Some(blocker.join("mailbox.mailcraft"));
    let pst = dir.pst(&distinct(3, 3000));
    assert_eq!(import(&mut s, &pst).unwrap()["added"], 3);
    assert!(s.mailbox.messages.values().all(|m| m.attachments[0].kept.is_none() && m.attachments[0].data.len() == 3000));
    // Six of 400 KB do not fit the budget in memory, and say so.
    let big = dir.pst(&distinct(6, 400 << 10));
    let mut t = Session::at(Mailbox::with_local_account("", ""), now(), 0);
    t.path = Some(blocker.join("mailbox.mailcraft"));
    assert!(import(&mut t, &big).unwrap_err().to_string().contains("too large to import"));
}

#[test]
fn a_self_contained_copy_past_the_cap_is_refused_before_anything_is_read_or_written() {
    let dir = Temp::new("saveas-cap");
    let mut s = kept_session(&dir);
    // Six files of 300 KB kept in the content folder: 1.8 MB against a cap of 1 MB.
    let pst = dir.pst(&distinct(6, 300 << 10));
    import(&mut s, &pst).unwrap();
    assert_eq!(in_memory(&s), 0);
    let before: Vec<_> = s.mailbox.messages.iter().map(|(id, m)| (*id, m.attachments[0].kept)).collect();
    let copy = dir.0.join("copy.mailcraft");
    let err = s.run("file.saveAs", &json!({"path": copy.display().to_string()})).unwrap_err().to_string();
    assert!(err.contains("too large") && err.contains("another folder"), "{err}");
    assert!(!copy.exists(), "nothing was written");
    assert_eq!(s.path.as_deref(), Some(dir.mailbox().as_path()));
    let after: Vec<_> = s.mailbox.messages.iter().map(|(id, m)| (*id, m.attachments[0].kept)).collect();
    assert_eq!(before, after, "the mailbox is as it was");
    assert_eq!(in_memory(&s), 0);
    // Saving to another folder (content linked) is the way, and works.
    let other = dir.0.join("elsewhere.mailcraft");
    s.run("file.save", &json!({"path": other.display().to_string()})).unwrap();
    assert!(other.exists());
}

#[test]
fn a_mailbox_is_not_moved_to_another_file_while_an_import_is_putting_mail_in() {
    let dir = Temp::new("moving");
    let mut s = kept_session(&dir);
    let pst = dir.pst(&distinct(6, 3000));
    let (batches, outcome) = batches_for(&s, &pst);
    let params = json!({"staged": true, "name": "old-mail"});
    let mut batches = batches.into_iter();
    s.staged_pst = Some(Staged::Batch(batches.next().unwrap()));
    s.run("file.importPst", &params).unwrap();
    assert!(s.importing_pst());
    let other = dir.0.join("other.mailcraft").display().to_string();
    let err = s.run("file.save", &json!({"path": other})).unwrap_err().to_string();
    assert!(err.contains("being imported"), "{err}");
    let err = s.run("file.saveAs", &json!({"path": other})).unwrap_err().to_string();
    assert!(err.contains("being imported"), "{err}");
    assert_eq!(s.path.as_deref(), Some(dir.mailbox().as_path()));
    assert!(!std::path::Path::new(&other).exists());
    // Saving where it is kept is fine meanwhile.
    s.run("file.save", &json!({})).unwrap();
    // The import goes on to its end, and then the mailbox may be saved elsewhere.
    for b in batches {
        s.staged_pst = Some(Staged::Batch(b));
        s.run("file.importPst", &params).unwrap();
    }
    s.staged_pst = Some(Staged::Done(Box::new(outcome)));
    s.run("file.importPst", &params).unwrap();
    s.run("file.save", &json!({"path": other})).unwrap();
    assert_eq!(reopened(&dir, std::path::Path::new(&other)).messages.len(), 6);
}
