// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's side of keeping the mailbox on disk: a change is handed to the host's background
//! saver instead of being written on the window's own thread, quitting writes what is left and
//! waits for it, a failure is told and tried again, and replacing the mailbox loses nothing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_engine::io::{BackgroundSave, SaveProgress, SaveReport, SaveStage};
use mailcraft_engine::{Session, store};
use mailcraft_model::{Attachment, Mailbox, Message};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::json;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A folder of its own under the system's temporary one, removed when the test is over.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-autosave-{}-{name}", std::process::id()));
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

/// The sample mailbox and one message with a sizeable file, kept in `path` and saved by itself
/// through `saver`.
fn app(path: &Path, saver: Box<dyn BackgroundSave>) -> MailApp {
    let mut mailbox = mailcraft_engine::sample::mailbox(NOW, 0);
    let scan = Attachment::new("scan.pdf", "application/pdf", (0..50_000u32).map(|i| (i * 31 % 251) as u8).collect());
    mailbox.add_message(Message { folder: mailbox.inbox().unwrap(), subject: "Scan".into(), attachments: vec![scan], ..Default::default() }).unwrap();
    let mut session = Session::at(mailbox, NOW, 0);
    session.path = Some(path.to_path_buf());
    let mut app = MailApp::new(session, Services { saver: Some(saver), ..Services::default() });
    app.auto_sync = false;
    app
}

/// Run the window's per-frame logic until `done`, as frames would.
fn frames_until(app: &mut MailApp, ctx: &egui::Context, done: impl Fn(&MailApp) -> bool) {
    let t = Instant::now();
    loop {
        app.logic(ctx);
        if done(app) {
            return;
        }
        assert!(t.elapsed() < Duration::from_secs(60), "the mailbox was never saved");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The mailbox as the file has it, with the content the store keeps on disk brought back in
/// (a reopened mailbox loads attachments only when they are wanted).
fn on_disk(path: &Path) -> Mailbox {
    let mut mailbox = store::open(path).unwrap().mailbox;
    store::load_all(path, &mut mailbox).unwrap();
    mailbox
}

fn unread(app: &MailApp) -> u64 {
    app.session.mailbox.messages.values().find(|m| !m.read).map(|m| m.id).unwrap()
}

#[test]
fn changes_are_written_in_the_background_and_quitting_writes_the_rest() {
    let tmp = Temp::new("background");
    let path = tmp.0.join("mailbox.mailcraft");
    let ctx = egui::Context::default();
    let mut app = app(&path, Box::new(store::Saver::spawn(|| {}).unwrap()));

    // Nothing changed, nothing written.
    app.logic(&ctx);
    assert!(!path.exists());

    // A change: the next frame hands the mailbox over and goes on; the file follows.
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    assert!(app.session.dirty);
    frames_until(&mut app, &ctx, |app| !app.session.dirty);
    assert!(on_disk(&path) == app.session.mailbox);
    // In the form that costs what changed: the content is beside the mailbox file.
    assert!(store::blob_dir(&path).is_dir());

    // Another change straight after waits its three seconds; closing the window doesn't.
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    app.logic(&ctx);
    assert!(app.session.dirty);
    app.save_on_exit().unwrap();
    assert!(!app.session.dirty && on_disk(&path) == app.session.mailbox);
    // Nothing left to do: quitting again writes nothing.
    let written = std::fs::metadata(&path).unwrap().modified().unwrap();
    app.save_on_exit().unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), written);

    // Replacing the mailbox (File › New) writes what the timer hadn't got to first.
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let before = app.session.mailbox.clone();
    // Through the saver: the window goes on, and the new mailbox takes its place once written.
    assert_eq!(app.execute("file.new", json!({})).unwrap(), json!({"pending": "save"}));
    frames_until(&mut app, &ctx, |app| app.session.path.is_none());
    assert!(on_disk(&path) == before);
}

/// A saver that keeps what it is handed and answers what the test tells it to.
#[derive(Default)]
struct Scripted {
    handed: std::sync::Mutex<Vec<(PathBuf, u64)>>,
    answers: std::sync::Mutex<Vec<SaveReport>>,
    progress: std::sync::Mutex<Option<SaveProgress>>,
    flushed: AtomicBool,
}

struct Handle(Arc<Scripted>);

impl BackgroundSave for Handle {
    fn follow(&self, _: &Path) {}
    fn submit(&self, path: &Path, _: &Mailbox, rev: u64) {
        self.0.handed.lock().unwrap().push((path.to_path_buf(), rev));
    }
    fn reports(&self) -> Vec<SaveReport> {
        std::mem::take(&mut self.0.answers.lock().unwrap())
    }
    fn progress(&self) -> Option<SaveProgress> {
        *self.0.progress.lock().unwrap()
    }
    fn flush(&self) {
        self.0.flushed.store(true, Ordering::Relaxed);
    }
}

#[test]
fn the_window_never_writes_itself_and_a_failure_is_told_and_tried_again() {
    let tmp = Temp::new("scripted");
    let path = tmp.0.join("mailbox.mailcraft");
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    let mut app = app(&path, Box::new(Handle(script.clone())));
    let answer = |rev: u64, result: Result<(), String>| script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev, result });

    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();
    for _ in 0..5 {
        app.logic(&ctx);
    }
    // Handed over once, with the state as it is now, and nothing written from here.
    assert_eq!(*script.handed.lock().unwrap(), [(path.clone(), rev)]);
    assert!(!path.exists() && app.session.dirty);

    // It couldn't be written: the status bar says why, and the mailbox still counts as unsaved.
    answer(rev, Err("the disk is full".into()));
    app.logic(&ctx);
    let (status, _) = app.status_msg.clone().unwrap();
    assert!(status.contains("the disk is full"), "{status}");
    assert!(app.session.dirty);
    // An answer about some other mailbox file, or an older state, doesn't make this one saved.
    script.answers.lock().unwrap().push(SaveReport { path: tmp.0.join("other.mailcraft"), rev, result: Ok(()) });
    answer(rev - 1, Ok(()));
    app.logic(&ctx);
    assert!(app.session.dirty);
    // Written at last.
    answer(rev, Ok(()));
    app.logic(&ctx);
    assert!(!app.session.dirty);
    // Quitting with nothing unsaved writes nothing, and still waits for the saver.
    app.save_on_exit().unwrap();
    assert!(!path.exists() && script.flushed.load(Ordering::Relaxed));
}

#[test]
fn a_long_save_shows_how_far_it_has_got_and_closing_waits_for_it_with_the_window() {
    let tmp = Temp::new("long");
    let path = tmp.0.join("mailbox.mailcraft");
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    let mut app = app(&path, Box::new(Handle(script.clone())));
    let progress = |p: Option<SaveProgress>| *script.progress.lock().unwrap() = p;
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();
    app.logic(&ctx);
    assert_eq!(*script.handed.lock().unwrap(), [(path.clone(), rev)]);

    // A save of a moment isn't worth a word; one that takes a while says how far it has got.
    progress(Some(SaveProgress { stage: SaveStage::Messages, done: 12_000, total: 200_000, millis: 100 }));
    app.logic(&ctx);
    assert_eq!(app.saving_text(), None);
    progress(Some(SaveProgress { stage: SaveStage::Messages, done: 12_000, total: 200_000, millis: 4_000 }));
    app.logic(&ctx);
    assert_eq!(app.saving_text().as_deref(), Some("Saving the mailbox… 12000 of 200000 messages"));
    progress(Some(SaveProgress { stage: SaveStage::Content, done: 3, total: 90_000, millis: 9_000 }));
    app.logic(&ctx);
    assert_eq!(app.saving_text().as_deref(), Some("Saving the mailbox… writing 3 of 90000 bodies and attachments"));

    // Closing the window meanwhile keeps it open until the save is done, and says so; the window
    // writes nothing itself.
    assert!(!app.close_requested());
    assert!(!app.quit_requested);
    app.logic(&ctx);
    assert!(!app.quit_requested && app.saving_text().unwrap().ends_with("MailCraft closes when it is done"));
    assert!(!path.exists());
    // Written: the window closes.
    progress(None);
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev, result: Ok(()) });
    app.logic(&ctx);
    assert!(app.quit_requested && !app.session.dirty);
    assert!(app.close_requested());
    app.save_on_exit().unwrap();

    // Quitting another way meanwhile (the window isn't asked) waits a while at most for what is
    // left (here the saver never answers: the error says so, and the window wrote nothing).
    let mut app = self::app(&path, Box::new(Handle(script.clone())));
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let error = app.save_on_exit().unwrap_err();
    assert!(error.contains("still being written"), "{error}");
    assert!(!path.exists() && app.session.dirty);
}

#[test]
fn closing_again_while_it_waits_keeps_waiting_and_never_leaves_the_last_changes_behind() {
    // Review of R6.3: a second Close used to close at once, and quitting gave up on the save
    // after two minutes, without the last changes. It goes on waiting now, saying so.
    let tmp = Temp::new("twice");
    let path = tmp.0.join("mailbox.mailcraft");
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    let mut app = app(&path, Box::new(Handle(script.clone())));
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();
    assert!(!app.close_requested());
    *script.progress.lock().unwrap() = Some(SaveProgress { stage: SaveStage::Messages, done: 10, total: 200_000, millis: 30_000 });
    for _ in 0..3 {
        assert!(!app.close_requested());
        app.logic(&ctx);
        assert!(!app.quit_requested);
    }
    assert!(app.saving_text().unwrap().ends_with("MailCraft closes when it is done"));
    assert_eq!(app.saving_inspect()["closing"], json!(true));
    // Written: it closes, and the close request that follows goes through.
    *script.progress.lock().unwrap() = None;
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev, result: Ok(()) });
    app.logic(&ctx);
    assert!(app.quit_requested && app.close_requested());
    app.save_on_exit().unwrap();
}

#[test]
fn closing_during_a_download_closes_once_what_it_was_closed_with_is_written() {
    // Review of R6.3: closing waited for the mailbox as it is *now*, and a download changes it
    // every page: the window never closed. It waits for the state it was closed with.
    let tmp = Temp::new("download-close");
    let path = tmp.0.join("mailbox.mailcraft");
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    let mut app = app(&path, Box::new(Handle(script.clone())));
    let busy = SaveProgress { stage: SaveStage::Content, done: 3, total: 90_000, millis: 9_000 };
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    *script.progress.lock().unwrap() = Some(busy);
    assert!(!app.close_requested());
    let closed_with = app.session.rev();
    // The download goes on: another page, and the saver is handed it and is busy again.
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    assert!(app.session.rev() > closed_with);
    app.logic(&ctx);
    assert!(!app.quit_requested);
    // What it was closed with is written; the saver is already at the next page's state.
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev: closed_with, result: Ok(()) });
    app.logic(&ctx);
    assert!(app.quit_requested, "the window waited for the download's newest state");
    assert!(app.close_requested());
}

#[test]
fn a_save_that_fails_while_closing_keeps_the_window_open_until_closed_again() {
    // Review of R6.3: closing went on waiting after a save that failed, and the window closed
    // by itself later, after some other save went through.
    let tmp = Temp::new("close-fails");
    let path = tmp.0.join("mailbox.mailcraft");
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    let mut app = app(&path, Box::new(Handle(script.clone())));
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();
    assert!(!app.close_requested());
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev, result: Err("the disk is full".into()) });
    app.logic(&ctx);
    assert!(!app.quit_requested && app.status_msg.clone().unwrap().0.contains("the disk is full"));
    assert_eq!(app.saving_inspect()["closing"], json!(false));
    // A later save that goes through doesn't close it.
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev: app.session.rev(), result: Ok(()) });
    for _ in 0..3 {
        app.logic(&ctx);
    }
    assert!(!app.quit_requested);
    // After a failure, closing again closes (quitting tries once more, a while at most).
    let mut app = self::app(&path, Box::new(Handle(script.clone())));
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();
    assert!(!app.close_requested());
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev, result: Err("the disk is full".into()) });
    app.logic(&ctx);
    assert!(!app.quit_requested);
    assert!(app.close_requested());
}

#[test]
fn a_close_held_by_a_question_waits_for_the_saver_again_next_time() {
    // Review of R6.3: what closing waited for being written let every later close through, so a
    // close held by "discard the draft?" and asked for again later skipped the wait for a save
    // the saver was in the middle of by then.
    let tmp = Temp::new("close-held");
    let path = tmp.0.join("mailbox.mailcraft");
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    let mut app = app(&path, Box::new(Handle(script.clone())));
    let busy = SaveProgress { stage: SaveStage::Messages, done: 10, total: 200_000, millis: 30_000 };
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();
    *script.progress.lock().unwrap() = Some(busy);
    assert!(!app.close_requested());
    // Meanwhile a message is being written, and something is typed into it.
    app.session.run("mail.new", &json!({})).unwrap();
    app.session.compose.as_mut().unwrap().dirty = true;
    // Written: the window would close, but the draft is asked about and the owner keeps it.
    *script.progress.lock().unwrap() = None;
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev, result: Ok(()) });
    app.logic(&ctx);
    assert!(app.quit_requested);
    assert!(!app.close_requested(), "the draft is asked about");
    app.quit_requested = false;
    app.dialog = None;
    app.session.compose = None;
    // Later, the saver is in the middle of a long save again: closing waits for it again.
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    *script.progress.lock().unwrap() = Some(busy);
    assert!(!app.close_requested(), "a later close skipped the wait for the saver");
    assert_eq!(app.saving_inspect()["closing"], json!(true));
    // A failed save lets the next close through, and only the next.
    let rev = app.session.rev();
    *script.progress.lock().unwrap() = None;
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev, result: Err("the disk is full".into()) });
    app.logic(&ctx);
    app.session.run("mail.new", &json!({})).unwrap();
    app.session.compose.as_mut().unwrap().dirty = true;
    assert!(!app.close_requested(), "the draft is asked about");
    app.session.compose = None;
    *script.progress.lock().unwrap() = Some(busy);
    assert!(!app.close_requested(), "a failure long ago let a later close skip the wait");
}

#[test]
fn save_new_and_open_never_write_on_the_window_while_the_saver_is_busy() {
    // The saver is in the middle of a save that takes minutes (it never finishes here): nothing
    // the window does writes the file itself or waits for the disk.
    let tmp = Temp::new("busy");
    let (path, other) = (tmp.0.join("mailbox.mailcraft"), tmp.0.join("other.mailcraft"));
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    *script.progress.lock().unwrap() = Some(SaveProgress { stage: SaveStage::Messages, done: 5, total: 200_000, millis: 60_000 });
    let mut app = app(&path, Box::new(Handle(script.clone())));
    // Another mailbox to open, written before the window's saver was busy.
    store::save(&other, &mailcraft_engine::sample::mailbox(NOW, 0)).unwrap();
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();

    let t = Instant::now();
    // Save (Ctrl+Shift+S, the ribbon, an agent): handed to the saver, which answers later.
    let saved = app.execute("file.save", json!({})).unwrap();
    assert_eq!(saved["background"], json!(true));
    assert_eq!(*script.handed.lock().unwrap(), [(path.clone(), rev)]);
    // Open waits for the save with the window going on, and says so.
    let opened = app.execute("file.open", json!({"path": other.to_string_lossy()})).unwrap();
    assert_eq!(opened, json!({"pending": "save"}));
    for _ in 0..10 {
        app.logic(&ctx);
    }
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert_eq!(app.session.path.as_deref(), Some(path.as_path()));
    assert!(app.saving_text().unwrap().ends_with("the other mailbox opens when it is done"));
    assert!(!path.exists());

    // Written: the other mailbox opens.
    *script.progress.lock().unwrap() = None;
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev, result: Ok(()) });
    app.logic(&ctx);
    assert_eq!(app.session.path.as_deref(), Some(other.as_path()));
    assert!(app.saving_text().is_none());

    // A save that fails keeps the mailbox open, and says why.
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();
    assert_eq!(app.execute("file.new", json!({})).unwrap(), json!({"pending": "save"}));
    script.answers.lock().unwrap().push(SaveReport { path: other.clone(), rev, result: Err("the disk is full".into()) });
    app.logic(&ctx);
    app.logic(&ctx);
    assert_eq!(app.session.path.as_deref(), Some(other.as_path()));
    assert!(app.status_msg.clone().unwrap().0.contains("the disk is full"));
    // An agent that was answered `"pending": "save"` hears that it didn't happen, and why.
    let saving = app.saving_inspect();
    assert_eq!((&saving["pending"], &saving["last"]["command"], &saving["last"]["ok"]), (&json!(null), &json!("file.new"), &json!(false)));
    assert!(saving["last"]["error"].as_str().unwrap().contains("the disk is full"), "{saving}");
}

#[test]
fn open_during_a_download_runs_once_the_saver_is_done_and_says_so() {
    // Review of R6.3: File › Open waited for the mailbox as it is *now*, which a download
    // changes every page, and never ran. It runs once the saver is done with the file (it is
    // handed nothing newer meanwhile), writing what changed since itself.
    let tmp = Temp::new("download-open");
    let (path, other) = (tmp.0.join("mailbox.mailcraft"), tmp.0.join("other.mailcraft"));
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    let mut app = app(&path, Box::new(Handle(script.clone())));
    store::save(&other, &mailcraft_engine::sample::mailbox(NOW, 0)).unwrap();
    let busy = SaveProgress { stage: SaveStage::Messages, done: 5, total: 200_000, millis: 60_000 };
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let asked_with = app.session.rev();
    *script.progress.lock().unwrap() = Some(busy);
    assert_eq!(app.execute("file.open", json!({"path": other.to_string_lossy()})).unwrap(), json!({"pending": "save"}));
    assert_eq!(app.saving_inspect()["pending"], json!("file.open"));
    // The download goes on; the window hands the saver nothing newer while the Open waits.
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let after = app.session.mailbox.clone();
    for _ in 0..3 {
        app.logic(&ctx);
    }
    assert_eq!(*script.handed.lock().unwrap(), [(path.clone(), asked_with)]);
    // The state it was asked with is written (here by hand: the scripted saver writes nothing,
    // nor tells the store that a host keeps the file, which a real one does) and the saver has
    // nothing left: the other mailbox opens, the changes since written first.
    store::save(&path, &app.session.mailbox).unwrap();
    store::Saver::spawn(|| {}).unwrap().follow(&path);
    *script.progress.lock().unwrap() = None;
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev: asked_with, result: Ok(()) });
    app.logic(&ctx);
    assert_eq!(app.session.path.as_deref(), Some(other.as_path()));
    assert!(on_disk(&path) == after);
    let saving = app.saving_inspect();
    assert_eq!((&saving["pending"], &saving["last"]["command"], &saving["last"]["ok"]), (&json!(null), &json!("file.open"), &json!(true)));
}

#[test]
fn closing_while_the_content_folder_is_swept_closes_at_once_and_a_change_still_waits_to_be_written() {
    // #586: a sweep writes nothing of the mailbox; the window closed only once it was over.
    let tmp = Temp::new("sweep-close");
    let path = tmp.0.join("mailbox.mailcraft");
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    let mut app = app(&path, Box::new(Handle(script.clone())));
    *script.progress.lock().unwrap() = Some(SaveProgress { stage: SaveStage::Sweeping, done: 0, total: 0, millis: 5_000 });
    app.logic(&ctx);
    assert!(app.close_requested(), "nothing to write: it closes");
    assert!(script.handed.lock().unwrap().is_empty());
    // A change made just before: handed to the saver, and the window waits for it.
    let mut app = self::app(&path, Box::new(Handle(script.clone())));
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();
    assert!(!app.close_requested());
    assert_eq!(*script.handed.lock().unwrap(), [(path.clone(), rev)]);
}

#[test]
fn open_waits_for_a_sweep_of_the_content_folder_too() {
    // Review of R6.3: the saver said it was free while it swept the content folder, which holds
    // the file; an Open run then waited for it, gave up, and was lost.
    let tmp = Temp::new("sweep-open");
    let (path, other) = (tmp.0.join("mailbox.mailcraft"), tmp.0.join("other.mailcraft"));
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    let mut app = app(&path, Box::new(Handle(script.clone())));
    store::save(&other, &mailcraft_engine::sample::mailbox(NOW, 0)).unwrap();
    let sweeping = SaveProgress { stage: SaveStage::Sweeping, done: 0, total: 0, millis: 5_000 };
    *script.progress.lock().unwrap() = Some(sweeping);
    // A sweep on its own is nothing to tell.
    app.logic(&ctx);
    assert_eq!(app.saving_text(), None);
    assert_eq!(app.saving_inspect()["progress"]["stage"], json!("sweeping"));
    // Nothing unsaved, but the file is busy: the Open waits for it, saying so.
    assert_eq!(app.execute("file.open", json!({"path": other.to_string_lossy()})).unwrap(), json!({"pending": "save"}));
    for _ in 0..3 {
        app.logic(&ctx);
    }
    assert_eq!(app.session.path.as_deref(), Some(path.as_path()));
    assert_eq!(app.saving_text().as_deref(), Some("Tidying the mailbox's content folder… · the other mailbox opens when it is done"));
    *script.progress.lock().unwrap() = None;
    app.logic(&ctx);
    assert_eq!(app.session.path.as_deref(), Some(other.as_path()));
    assert_eq!(app.saving_inspect()["last"]["ok"], json!(true));
}

#[test]
fn closing_while_an_open_waits_for_the_saver_closes_and_opens_nothing_on_the_way_out() {
    // #134: an Open and a Close both waiting for the background save, the Open ran first when
    // the save was done, and the window then closed on the other mailbox. Closing wins now.
    let tmp = Temp::new("open-then-close");
    let (path, other) = (tmp.0.join("mailbox.mailcraft"), tmp.0.join("other.mailcraft"));
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    let busy = SaveProgress { stage: SaveStage::Messages, done: 5, total: 200_000, millis: 60_000 };
    let mut app = app(&path, Box::new(Handle(script.clone())));
    store::save(&other, &mailcraft_engine::sample::mailbox(NOW, 0)).unwrap();
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();
    *script.progress.lock().unwrap() = Some(busy);

    // File › Open waits for the save; then the owner closes the window, which waits too.
    assert_eq!(app.execute("file.open", json!({"path": other.to_string_lossy()})).unwrap(), json!({"pending": "save"}));
    app.logic(&ctx);
    assert!(!app.close_requested());
    app.logic(&ctx);
    assert!(!app.quit_requested);
    let saving = app.saving_inspect();
    assert_eq!((&saving["closing"], &saving["pending"]), (&json!(true), &json!(null)));
    // An agent answered `"pending": "save"` hears that the Open gave way.
    assert_eq!((&saving["last"]["command"], &saving["last"]["ok"]), (&json!("file.open"), &json!(false)));
    assert!(app.saving_text().unwrap().ends_with("MailCraft closes when it is done"));

    // Written: the window closes on the mailbox it had, and the other one never opened.
    *script.progress.lock().unwrap() = None;
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev, result: Ok(()) });
    app.logic(&ctx);
    assert_eq!(app.session.path.as_deref(), Some(path.as_path()));
    assert!(app.quit_requested && !app.session.dirty);
    app.logic(&ctx);
    assert_eq!(app.session.path.as_deref(), Some(path.as_path()));
    assert!(app.close_requested());
    app.save_on_exit().unwrap();
}

#[test]
fn an_open_asked_for_while_closing_waits_is_refused_and_an_open_alone_never_closes() {
    // #134, the other way round: closing waits for the save, then File › Open is asked for.
    let tmp = Temp::new("close-then-open");
    let (path, other) = (tmp.0.join("mailbox.mailcraft"), tmp.0.join("other.mailcraft"));
    let ctx = egui::Context::default();
    let script = Arc::new(Scripted::default());
    let busy = SaveProgress { stage: SaveStage::Messages, done: 5, total: 200_000, millis: 60_000 };
    let mut app = app(&path, Box::new(Handle(script.clone())));
    store::save(&other, &mailcraft_engine::sample::mailbox(NOW, 0)).unwrap();
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();
    *script.progress.lock().unwrap() = Some(busy);
    assert!(!app.close_requested());
    let error = app.execute("file.open", json!({"path": other.to_string_lossy()})).unwrap_err();
    assert!(error.contains("closes once the mailbox is saved"), "{error}");
    assert_eq!(app.saving_inspect()["pending"], json!(null));
    *script.progress.lock().unwrap() = None;
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev, result: Ok(()) });
    app.logic(&ctx);
    assert_eq!(app.session.path.as_deref(), Some(path.as_path()));
    assert!(app.quit_requested && app.close_requested());

    // An Open with nobody closing opens the other mailbox once the save is done, and the window
    // stays.
    let mut app = self::app(&path, Box::new(Handle(script.clone())));
    let id = unread(&app);
    app.execute("mail.markRead", json!({"id": id})).unwrap();
    let rev = app.session.rev();
    *script.progress.lock().unwrap() = Some(busy);
    assert_eq!(app.execute("file.open", json!({"path": other.to_string_lossy()})).unwrap(), json!({"pending": "save"}));
    app.logic(&ctx);
    *script.progress.lock().unwrap() = None;
    script.answers.lock().unwrap().push(SaveReport { path: path.clone(), rev, result: Ok(()) });
    for _ in 0..3 {
        app.logic(&ctx);
    }
    assert_eq!(app.session.path.as_deref(), Some(other.as_path()));
    assert!(!app.quit_requested);
    assert_eq!(app.saving_inspect()["last"]["ok"], json!(true));
}
