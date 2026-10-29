// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The opening screen and opening a mailbox in the background (#519).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_engine::io::OpenProgress;
use mailcraft_model::{Address, Mailbox, Message};
use serde_json::{Value, json};

use super::*;
use crate::{ControlRequest, MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A folder of its own under the system's temporary one, removed when the test is over.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-opening-{}-{name}", std::process::id()));
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

/// A mailbox of `n` made-up messages in its Inbox.
fn mailbox(n: usize) -> Mailbox {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    for i in 0..n {
        let m = Message {
            folder: inbox,
            subject: format!("Note {i}"),
            from: Address::new("Blake Rivera", "blake@example.org"),
            date: NOW - i as i64 * 60,
            ..Default::default()
        };
        mb.add_message(m).unwrap();
    }
    mb
}

/// What the host's [`Build`] was handed, when it was.
type Built = Rc<RefCell<Option<(usize, Option<PathBuf>, bool)>>>;

/// A window waiting for its mailbox, the sender the mailbox arrives through, what the window was
/// built from once it did, and the control channel's sender.
fn waiting() -> (MailApp, Sender<Arrival>, Arc<OpenProgress>, Built, Sender<ControlRequest>) {
    let (tx, rx) = channel();
    let progress = Arc::new(OpenProgress::default());
    let built: Built = Default::default();
    let seen = built.clone();
    let build: Build = Box::new(move |arrival: Arrival| {
        *seen.borrow_mut() = Some((arrival.mailbox.messages.len(), arrival.path.clone(), arrival.damaged));
        let mut session = Session::at(arrival.mailbox, NOW, 0);
        session.path = arrival.path;
        let mut app = MailApp::new(session, Services::default());
        app.autosave = false;
        if arrival.damaged {
            app.status(crate::damaged_mailbox_notice());
        }
        app
    });
    let (control_tx, control_rx) = channel();
    let app = MailApp::opening(Opening::new(rx, progress.clone(), build), None, None).with_control(control_rx);
    (app, tx, progress, built, control_tx)
}

fn harness(app: MailApp) -> egui_kittest::Harness<'static, MailApp> {
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1000.0, 700.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(3);
    harness
}

/// Pretend the window has waited a second already.
fn waited_a_while(app: &mut MailApp) {
    if let Some(o) = app.opening.as_mut() {
        o.since = Some(crate::now_ms() - 1_000.0);
    }
}

fn ask(control: &Sender<ControlRequest>, method: &str, params: Value) -> std::sync::mpsc::Receiver<Value> {
    let (req, reply) = ControlRequest::new(method, params);
    control.send(req).unwrap();
    reply
}

#[test]
fn while_opening_every_command_and_control_request_is_refused_but_the_status() {
    let (app, _tx, progress, _built, control) = waiting();
    progress.set_total(2_000_000);
    progress.add_read(500_000);
    let mut h = harness(app);
    let app = h.state_mut();
    assert!(app.is_opening());
    for id in ["mail.new", "file.save", "ui.sync", "sync.request", "app.quit"] {
        assert_eq!(app.run(id, json!({})).unwrap_err(), STILL_OPENING, "{id}");
        assert_eq!(app.execute(id, json!({})).unwrap_err(), STILL_OPENING, "{id}");
        assert!(!app.enabled(id), "{id}");
    }
    let executed = ask(&control, "engine.execute", json!({"command": "mail.new", "params": {}}));
    let inspected = ask(&control, "ui.inspect", json!({}));
    let quit = ask(&control, "app.quit", json!({}));
    let status = ask(&control, "app.status", json!({}));
    h.run_steps(2);
    for reply in [executed, inspected, quit] {
        let v = reply.try_recv().unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], STILL_OPENING);
    }
    let v = status.try_recv().unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"], json!({"mailbox": "opening", "stage": "reading", "read": 500_000, "total": 2_000_000}));
    assert!(!h.state().quit_requested);
    assert!(h.state().session.compose.is_none());
}

#[test]
fn nothing_starts_while_the_mailbox_opens() {
    let (app, _tx, _progress, _built, _control) = waiting();
    let mut h = harness(app);
    h.run_steps(10);
    let app = h.state();
    assert!(!app.syncing());
    assert!(app.sync_rx.is_none() && app.graph_rx.is_none() && app.signing_in.is_none());
    assert!(app.dialog.is_none(), "no Add Account, no question");
    assert!(app.services.saver.is_none());
}

#[test]
fn files_and_links_handed_over_while_opening_wait_then_open() {
    let (app, tx, _progress, built, _control) = waiting();
    let mut h = harness(app);
    h.state_mut().open_path("mailto:blake@example.org?subject=Lunch");
    h.run_steps(2);
    assert!(h.state().session.compose.is_none(), "nothing opens before the mailbox is there");
    tx.send(Arrival::opened(mailbox(3), None)).unwrap();
    h.run_steps(3);
    let app = h.state();
    assert!(!app.is_opening());
    assert_eq!(built.borrow().as_ref().map(|b| b.0), Some(3));
    let compose = app.session.compose.as_ref().expect("the link opened a message once the mailbox was there");
    assert_eq!(compose.subject, "Lunch");
}

#[test]
fn the_window_built_takes_over_the_control_channel() {
    let (app, tx, _progress, _built, control) = waiting();
    let mut h = harness(app);
    tx.send(Arrival::opened(mailbox(2), None)).unwrap();
    h.run_steps(3);
    let status = ask(&control, "app.status", json!({}));
    let listed = ask(&control, "engine.execute", json!({"command": "mail.list", "params": {}}));
    h.run_steps(2);
    assert_eq!(status.try_recv().unwrap()["result"]["mailbox"], "open");
    assert_eq!(listed.try_recv().unwrap()["ok"], true);
}

#[test]
fn closing_while_opening_writes_nothing() {
    let tmp = Temp::new("close");
    let file = tmp.0.join("mailbox.mailcraft");
    mailcraft_engine::store::save(&file, &mailbox(4)).unwrap();
    let before: Vec<(PathBuf, Vec<u8>)> = everything(&tmp.0);
    let (mut app, _tx, _progress, built, _control) = waiting();
    // The stand-in tied to the real file, with a saver and changes of its own: only the window
    // knowing it is still opening keeps them from being written over the mailbox.
    app.session.path = Some(file.clone());
    app.services.saver = Some(Box::new(mailcraft_engine::store::Saver::spawn(|| {}).unwrap()));
    app.autosave = true;
    app.session.dirty = true;
    app.saved_rev = u64::MAX;
    let mut h = harness(app);
    let app = h.state_mut();
    assert!(app.close_requested(), "the window closes at once");
    assert!(app.save_on_exit().is_ok());
    assert!(built.borrow().is_none(), "no window was built");
    assert_eq!(everything(&tmp.0), before, "the mailbox and its folder are as they were, byte for byte");
}

/// Every file under `dir`, with its bytes.
fn everything(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(e.path());
            } else {
                out.push((e.path(), std::fs::read(e.path()).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn a_quick_open_never_shows_the_screen() {
    let (app, tx, progress, _built, _control) = waiting();
    progress.set_total(30_000_000);
    tx.send(Arrival::opened(mailbox(1), None)).unwrap();
    let h = harness(app);
    assert!(!h.state().is_opening());
    assert!(h.query_by_label_contains("Opening").is_none());
}

#[test]
fn the_screen_shows_after_a_moment_with_the_bytes_read_then_getting_ready() {
    let (app, _tx, progress, _built, _control) = waiting();
    progress.set_total(750_000_000);
    progress.add_read(340_000_000);
    let mut h = harness(app);
    assert!(h.query_by_label("Opening your mailbox…").is_none(), "not in the first moment");
    waited_a_while(h.state_mut());
    h.run_steps(2);
    assert!(h.query_by_label("Opening your mailbox…").is_some());
    assert!(h.query_by_label("340 of 750 MB").is_some());
    progress.add_read(410_000_000);
    h.run_steps(2);
    assert!(h.query_by_label("Getting ready…").is_some());
    assert!(h.query_by_label_contains("of 750 MB").is_none());
}

#[test]
fn without_a_size_the_screen_says_opening_alone() {
    let (app, _tx, _progress, _built, _control) = waiting();
    let mut h = harness(app);
    waited_a_while(h.state_mut());
    h.run_steps(2);
    assert!(h.query_by_label("Opening…").is_some());
    assert!(h.query_by_label("Opening your mailbox…").is_none());
}

#[test]
fn a_mailbox_that_cant_be_read_says_why_then_starts_empty_and_kept_nowhere() {
    let (app, tx, _progress, built, _control) = waiting();
    let mut h = harness(app);
    tx.send(Arrival::unreadable(Trouble::Unreadable("this mailbox file is damaged (its header)".into()))).unwrap();
    h.run_steps(3);
    assert!(h.state().is_opening(), "the window waits for the owner");
    assert!(h.query_by_label("Your mailbox couldn't be opened").is_some());
    assert!(h.query_by_label("this mailbox file is damaged (its header)").is_some());
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert!(!h.state().is_opening());
    assert_eq!(*built.borrow(), Some((0, None, false)));
    assert!(h.state().session.path.is_none());
}

#[test]
fn a_damaged_mailbox_says_so_then_carries_on_as_it_always_did() {
    let (app, tx, _progress, built, _control) = waiting();
    let mut h = harness(app);
    let arrival =
        Arrival { damaged: true, trouble: Some(Trouble::Damaged), ..Arrival::opened(mailbox(5), Some(PathBuf::from("/nowhere/mailbox.mailcraft"))) };
    tx.send(arrival).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("Part of your mailbox is damaged").is_some());
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert_eq!(*built.borrow(), Some((5, Some(PathBuf::from("/nowhere/mailbox.mailcraft")), true)));
    // Today's notice, in the status bar.
    assert_eq!(h.state().status_msg.as_ref().map(|(m, _)| m.clone()), Some(crate::damaged_mailbox_notice()));
}

#[test]
fn a_repaired_mailbox_says_how_many_things_were_put_right() {
    let (app, tx, _progress, _built, _control) = waiting();
    let mut h = harness(app);
    tx.send(Arrival { trouble: Some(Trouble::Repaired(3)), ..Arrival::opened(mailbox(1), None) }).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("Your mailbox was repaired").is_some());
    assert!(h.query_by_label("3 things in it didn't fit with the rest and were put right.").is_some());
}

#[test]
fn a_reading_thread_that_ends_without_a_word_is_a_mailbox_that_couldnt_be_read() {
    let (app, tx, _progress, built, _control) = waiting();
    drop(tx);
    let mut h = harness(app);
    assert!(h.query_by_label("MailCraft stopped reading it before it was done.").is_some());
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert_eq!(*built.borrow(), Some((0, None, false)));
}

#[test]
fn the_waiting_window_has_the_owners_theme_from_the_start() {
    let (_tx, rx) = channel();
    let prefs = crate::UiState { theme: crate::theme::Appearance::Dark, ..Default::default() };
    let app = MailApp::opening(Opening::new(rx, Default::default(), Box::new(|_| sample_app(Services::default()))), Some(prefs), None);
    let h = harness(app);
    assert!(Tokens::get(&h.ctx).dark);
}

// ---- File › Open in the background ----

/// A host that reads mailbox files ahead as the desktop one does, on a thread of its own.
fn reading_host() -> Services {
    Services {
        read_ahead: Some(Box::new(|path: PathBuf, progress: Arc<OpenProgress>| {
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let _ = tx.send(mailcraft_engine::store::open_ahead(&path, &progress));
            });
            rx
        })),
        ..Default::default()
    }
}

/// A host whose reading goes no further than the test lets it: the reads it was asked for, with
/// where to answer.
type Held = Arc<Mutex<Vec<(PathBuf, Arc<OpenProgress>, Sender<Result<(), String>>)>>>;

fn held_host() -> (Services, Held) {
    let held: Held = Default::default();
    let asked = held.clone();
    let services = Services {
        read_ahead: Some(Box::new(move |path: PathBuf, progress: Arc<OpenProgress>| {
            let (tx, rx) = channel();
            asked.lock().unwrap().push((path, progress, tx));
            rx
        })),
        ..Default::default()
    };
    (services, held)
}

fn sample_app(services: Services) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), services);
    app.autosave = false;
    app
}

#[test]
fn file_open_reads_the_other_mailbox_while_the_open_one_stays_in_use() {
    let (services, held) = held_host();
    let mut h = harness(sample_app(services));
    let app = h.state_mut();
    let messages = app.session.mailbox.messages.len();
    let r = app.run("file.open", json!({"path": "/somewhere/Archive.mailcraft"})).unwrap();
    assert_eq!(r["pending"], "opening");
    assert_eq!(held.lock().unwrap().len(), 1);
    {
        let asked = held.lock().unwrap();
        asked[0].1.set_total(100_000_000);
        asked[0].1.add_read(45_000_000);
    }
    // The mailbox open is still the one in use.
    assert!(app.run("mail.new", json!({})).is_ok());
    assert!(app.execute("compose.close", json!({})).is_ok());
    assert_eq!(app.session.mailbox.messages.len(), messages);
    h.run_steps(2);
    assert!(h.query_by_label_contains("Opening Archive.mailcraft… 45%").is_some());
    let status = control_status(&mut h);
    assert_eq!(status["opening"]["read"], 45_000_000);
    assert_eq!(status["opening"]["name"], "Archive.mailcraft", "the name alone, not where it is kept");
    assert!(status["opening"].get("path").is_none());
}

#[test]
fn an_agent_hears_the_status_before_and_after_the_mailbox_opens() {
    let (app, tx, _progress, _built, control) = waiting();
    let mut h = harness(app);
    let status = |h: &mut egui_kittest::Harness<'static, MailApp>| {
        let (req, reply) = ControlRequest::new("app.status", json!({}));
        control.send(req.from_agent(vec![])).unwrap();
        h.run_steps(2);
        reply.try_recv().unwrap()
    };
    assert_eq!(status(&mut h)["result"]["mailbox"], "opening");
    tx.send(Arrival::opened(mailbox(3), None)).unwrap();
    h.run_steps(3);
    assert!(!h.state().is_opening());
    let v = status(&mut h);
    assert_eq!(v["result"]["mailbox"], "open", "{v}");
}

#[test]
fn whatever_replaces_the_mailbox_gives_up_the_one_being_read() {
    let tmp = Temp::new("replaced");
    let other = tmp.0.join("Other.mailcraft");
    mailcraft_engine::store::save(&other, &mailbox(2)).unwrap();
    for (id, params) in [("file.new", json!({})), ("file.open", json!({"path": other.to_string_lossy()}))] {
        let (services, held) = held_host();
        let mut h = harness(sample_app(services));
        h.state_mut().run("file.open", json!({"path": "/somewhere/Archive.mailcraft"})).unwrap();
        // An agent's, run as it is asked.
        h.state_mut().execute(id, params).unwrap();
        assert!(h.state().reading_ahead.is_none(), "{id}");
        let (_, progress, done) = held.lock().unwrap().pop().unwrap();
        assert!(progress.cancelled(), "{id}: the reading stops");
        let path = h.state().session.path.clone();
        let messages = h.state().session.mailbox.messages.len();
        // Done all the same, too late: it doesn't take the place of what replaced the mailbox.
        let _ = done.send(Ok(()));
        h.run_steps(3);
        assert_eq!(h.state().session.path, path, "{id}");
        assert_eq!(h.state().session.mailbox.messages.len(), messages, "{id}");
    }
}

#[test]
fn a_mailbox_is_a_recent_one_once_it_is_open_and_file_closes_then() {
    let tmp = Temp::new("recent");
    let file = tmp.0.join("Archive.mailcraft");
    mailcraft_engine::store::save(&file, &mailbox(7)).unwrap();
    let (services, held) = held_host();
    let mut h = harness(sample_app(services));
    h.state_mut().ui.backstage = true;
    let path = file.to_string_lossy().to_string();
    assert_eq!(h.state_mut().run("file.open", json!({"path": path})).unwrap()["pending"], "opening");
    assert!(h.state().ui.recent.is_empty(), "not while it is being read");
    assert!(h.state().ui.backstage);
    let (read, progress, done) = held.lock().unwrap().pop().unwrap();
    let _ = done.send(mailcraft_engine::store::open_ahead(&read, &progress));
    h.run_steps(3);
    assert_eq!(h.state().session.path.as_deref(), Some(file.as_path()));
    assert_eq!(h.state().ui.recent, vec![path]);
    assert!(!h.state().ui.backstage);
}

#[test]
fn the_mailbox_open_now_is_never_read_ahead_however_its_path_is_spelt() {
    let tmp = Temp::new("spelt");
    let file = tmp.0.join("mailbox.mailcraft");
    mailcraft_engine::store::save(&file, &mailbox(4)).unwrap();
    let (services, held) = held_host();
    let mut app = sample_app(services);
    app.session.path = Some(file.clone());
    let spelt = tmp.0.join(".").join("mailbox.mailcraft");
    let _ = app.run("file.open", json!({"path": spelt.to_string_lossy()}));
    assert!(held.lock().unwrap().is_empty());
}

fn control_status(h: &mut egui_kittest::Harness<'static, MailApp>) -> Value {
    let (tx, rx) = channel();
    h.state_mut().control_rx = Some(rx);
    let reply = ask(&tx, "app.status", json!({}));
    h.run_steps(1);
    reply.try_recv().unwrap()["result"].clone()
}

#[test]
fn cancel_drops_the_mailbox_being_read() {
    let (services, held) = held_host();
    let mut h = harness(sample_app(services));
    let path = h.state().session.path.clone();
    h.state_mut().run("file.open", json!({"path": "/somewhere/Archive.mailcraft"})).unwrap();
    assert!(h.state().enabled("ui.cancelOpen"));
    h.state_mut().run("ui.cancelOpen", json!({})).unwrap();
    assert!(!h.state().enabled("ui.cancelOpen"));
    let (_, progress, tx) = held.lock().unwrap().pop().unwrap();
    assert!(progress.cancelled(), "the reading stops");
    // Done all the same, too late: nothing changes.
    let _ = tx.send(Ok(()));
    h.run_steps(3);
    assert_eq!(h.state().session.path, path);
    assert!(h.query_by_label_contains("Opening Archive").is_none());
}

#[test]
fn a_finished_read_takes_the_place_of_the_open_mailbox() {
    let tmp = Temp::new("swap");
    let file = tmp.0.join("Archive.mailcraft");
    mailcraft_engine::store::save(&file, &mailbox(7)).unwrap();
    let mut h = harness(sample_app(reading_host()));
    h.state_mut().run("file.open", json!({"path": file.to_string_lossy()})).unwrap();
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().session.path.as_deref() == Some(file.as_path()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let app = h.state();
    assert_eq!(app.session.path.as_deref(), Some(file.as_path()));
    assert_eq!(app.session.mailbox.messages.len(), 7);
    assert!(app.reading_ahead.is_none());
}

#[test]
fn a_file_that_cant_be_read_in_the_background_leaves_the_open_mailbox_and_says_why() {
    let tmp = Temp::new("bad");
    let file = tmp.0.join("Broken.mailcraft");
    std::fs::write(&file, b"not a mailbox at all").unwrap();
    let mut h = harness(sample_app(reading_host()));
    let messages = h.state().session.mailbox.messages.len();
    h.state_mut().run("file.open", json!({"path": file.to_string_lossy()})).unwrap();
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().reading_ahead.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let app = h.state();
    assert_eq!(app.session.mailbox.messages.len(), messages);
    let said = app.status_msg.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(said.starts_with("Couldn't open Broken.mailcraft:"), "{said}");
}

#[test]
fn an_agent_opening_a_mailbox_is_not_put_off() {
    let (services, held) = held_host();
    let tmp = Temp::new("agent");
    let file = tmp.0.join("Other.mailcraft");
    mailcraft_engine::store::save(&file, &mailbox(2)).unwrap();
    let mut app = sample_app(services);
    app.execute("file.open", json!({"path": file.to_string_lossy()})).unwrap();
    assert!(held.lock().unwrap().is_empty());
    assert_eq!(app.session.mailbox.messages.len(), 2);
}

#[test]
fn sizes_are_said_in_megabytes() {
    assert_eq!(megabytes(340_000_000, 750_000_000), "340");
    assert_eq!(megabytes(1_260_000, 4_000_000), "1.3");
    assert_eq!(megabytes(0, 0), "0.0");
}

#[test]
fn the_logo_is_the_app_icon_and_is_made_once() {
    let ctx = egui::Context::default();
    let first = logo(&ctx).expect("the icon is built in");
    assert_eq!(first.size(), [256, 256]);
    let second = logo(&ctx).unwrap();
    assert_eq!(first.id(), second.id());
}

#[test]
fn what_is_missing_from_the_content_folder_is_said_once_it_has_been_looked_for() {
    let mut app = MailApp::new(Session::at(mailbox(3), NOW, 0), Services::default());
    app.autosave = false;
    let (tx, rx) = channel();
    app.check_parts_later(rx, 1, true);
    let mut h = harness(app);
    h.run_steps(3);
    // Nothing said while the look goes on.
    assert!(h.state().status_msg.is_none());
    tx.send(mailcraft_engine::Missing { count: 2, refs: Vec::new() }).unwrap();
    h.run_steps(2);
    let said = h.state().status_msg.as_ref().map(|(m, _)| m.clone());
    assert_eq!(said.as_deref(), Some("3 bodies or attachments are missing from the mailbox's content folder, or damaged."));
    // Nothing missing: nothing said.
    let mut app = MailApp::new(Session::at(mailbox(3), NOW, 0), Services::default());
    app.autosave = false;
    let (tx, rx) = channel();
    app.check_parts_later(rx, 0, true);
    tx.send(mailcraft_engine::Missing::default()).unwrap();
    let mut h = harness(app);
    h.run_steps(2);
    assert!(h.state().status_msg.is_none() && h.state().later.parts.is_none());
}

#[test]
fn the_word_index_starts_after_the_first_frames() {
    let mut app = MailApp::new(Session::at(mailbox(3), NOW, 0), Services::default());
    app.autosave = false;
    app.session.index_words = true;
    app.index_words_later();
    assert!(!app.session.index_words);
    // Not in the first frames, each of which asks for the next.
    let ctx = egui::Context::default();
    for _ in 0..WORDS_AFTER_FRAMES {
        after(&mut app, &ctx);
        assert!(!app.session.index_words);
        assert!(ctx.has_requested_repaint());
    }
    after(&mut app, &ctx);
    assert!(app.session.index_words);
    // And it isn't asked for again.
    let mut h = harness(app);
    h.run_steps(2);
    assert!(h.state().session.index_words && h.state().later.words_in.is_none());
}

/// A mailbox whose first message keeps its text on disk, and that text with its name.
fn kept_text() -> (Mailbox, mailcraft_model::Id, mailcraft_model::ContentRef, Vec<u8>) {
    let mut mb = mailbox(3);
    let id = *mb.messages.keys().next().unwrap();
    let text = b"The figures for the quarter are in the folder.".to_vec();
    let r = mailcraft_model::ContentRef { sha256: [7; 32], len: text.len() as u64 };
    let m = mb.message_mut(id).unwrap();
    m.body.text.clear();
    m.kept = Some(Box::new(mailcraft_model::KeptBody { text: Some(r), ..Default::default() }));
    (mb, id, r, text)
}

#[test]
fn bodies_read_after_the_window_shows_come_into_their_messages_and_their_rows() {
    let (mb, id, r, text) = kept_text();
    let mut app = MailApp::new(Session::at(mb, NOW, 0), Services::default());
    app.autosave = false;
    let (missing_tx, missing_rx) = channel();
    let (tx, rx) = channel();
    app.check_parts_later(missing_rx, 0, true);
    app.take_bodies_later(rx);
    tx.send(vec![mailcraft_engine::BodyRead { id, text: Some((r, text.clone())), html: None, hidden: None }]).unwrap();
    let mut h = harness(app);
    h.run_steps(2);
    // Not before what is missing is known: a body found missing is never taken in after it.
    assert!(h.state().session.mailbox.message(id).unwrap().body_is_kept());
    missing_tx.send(mailcraft_engine::Missing::default()).unwrap();
    drop(tx);
    h.run_steps(2);
    let m = h.state().session.mailbox.message(id).unwrap();
    assert_eq!(m.body.text.as_bytes(), text.as_slice());
    assert!(!m.body_is_kept());
    assert!(mailcraft_engine::list::preview(m, 80).starts_with("The figures for the quarter"));
    assert!(h.state().later.bodies.is_none());
}

#[test]
fn what_comes_late_for_a_mailbox_is_dropped_once_another_has_taken_its_place() {
    let (mb, id, r, text) = kept_text();
    let mut app = MailApp::new(Session::at(mb.clone(), NOW, 0), Services::default());
    app.autosave = false;
    let (missing_tx, missing_rx) = channel();
    let (tx, rx) = channel();
    app.check_parts_later(missing_rx, 0, true);
    app.take_bodies_later(rx);
    // File › Open of another mailbox (here, one that names the same content) before the look is over.
    app.session.replace_mailbox(mb);
    missing_tx.send(mailcraft_engine::Missing { count: 2, refs: vec![r] }).unwrap();
    tx.send(vec![mailcraft_engine::BodyRead { id, text: Some((r, text)), html: None, hidden: None }]).unwrap();
    let mut h = harness(app);
    h.run_steps(3);
    assert!(h.state().status_msg.is_none(), "{:?}", h.state().status_msg);
    let m = h.state().session.mailbox.message(id).unwrap();
    assert!(m.body_is_kept() && !m.kept.as_ref().unwrap().failed);
    assert!(h.state().later.parts.is_none() && h.state().later.bodies.is_none());
}

#[test]
fn a_damaged_mailbox_marks_what_is_missing_without_saying_the_count() {
    let (mb, id, r, _) = kept_text();
    let mut app = MailApp::new(Session::at(mb, NOW, 0), Services::default());
    app.autosave = false;
    let (tx, rx) = channel();
    app.check_parts_later(rx, 1, false);
    tx.send(mailcraft_engine::Missing { count: 1, refs: vec![r] }).unwrap();
    let mut h = harness(app);
    h.run_steps(2);
    assert!(h.state().status_msg.is_none());
    let m = h.state().session.mailbox.message(id).unwrap();
    assert!(m.kept.as_ref().unwrap().failed && m.body.text == mailcraft_engine::store_placeholder());
}
