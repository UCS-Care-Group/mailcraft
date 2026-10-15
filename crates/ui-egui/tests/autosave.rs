//! The window's side of keeping the mailbox on disk: a change is handed to the host's background
//! saver instead of being written on the window's own thread, quitting writes what is left and
//! waits for it, a failure is told and tried again, and replacing the mailbox loses nothing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_engine::io::{BackgroundSave, SaveReport};
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
    app.execute("file.new", json!({})).unwrap();
    assert!(app.session.path.is_none() && on_disk(&path) == before);
}

/// A saver that keeps what it is handed and answers what the test tells it to.
#[derive(Default)]
struct Scripted {
    handed: std::sync::Mutex<Vec<(PathBuf, u64)>>,
    answers: std::sync::Mutex<Vec<SaveReport>>,
    flushed: AtomicBool,
}

struct Handle(Arc<Scripted>);

impl BackgroundSave for Handle {
    fn follow(&self, _: &Path) {}
    fn submit(&self, path: &Path, _: Mailbox, rev: u64) {
        self.0.handed.lock().unwrap().push((path.to_path_buf(), rev));
    }
    fn reports(&self) -> Vec<SaveReport> {
        std::mem::take(&mut self.0.answers.lock().unwrap())
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
