// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Import a .pst File (#815): a synthetic file made by the reader's test writer (invented people
//! at `example.org`) is picked, its password asked for and checked, its mail brought in through
//! the dialog and the report shown; a host that sends batches has them put in as they come, and
//! Cancel keeps what is in.

use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_engine::import_pst::Event;
use mailcraft_pst::test_writer::{self as w, FileSpec, FolderSpec, Layout, MessageSpec};
use mailcraft_pst::{Encryption, Format};
use serde_json::json;

use crate::dialogs::Dialog;
use crate::import_pst::PstJob;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("mailcraft-ui-import-pst-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn note(subject: &str) -> MessageSpec {
    MessageSpec {
        class: "IPM.Note".into(),
        subject: subject.into(),
        body: format!("{subject}\r\n\r\nWritten for the tests."),
        delivered: 134_067_498_000_000_000,
        recipients: vec![("Ada Fernsby".into(), "ada@example.org".into(), 1)],
        ..Default::default()
    }
}

/// A file with `n` messages in its Inbox, written into `dir`.
fn pst(dir: &Scratch, n: usize, password: Option<&str>) -> String {
    let bytes = w::build(&FileSpec {
        layout: Layout::new(Format::Unicode, Encryption::Permute),
        password: password.map(str::to_string),
        password_ansi: false,
        name: "Personal Folders".into(),
        folders: vec![FolderSpec {
            name: "Inbox".into(),
            class: Some("IPF.Note".into()),
            messages: (0..n).map(|i| note(&format!("Shed roof {i}"))).collect(),
            children: vec![],
        }],
        named: vec![],
        named_strings: vec![],
    });
    let path = dir.0.join("garden.pst");
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().to_string()
}

fn window(services: Services, dir: &Scratch) -> Harness<'static, MailApp> {
    let mut session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    // The report is written beside the mailbox: in the scratch folder.
    session.path = Some(dir.0.join("mail.mailcraft"));
    let app = MailApp::new(session, services);
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn open(h: &mut Harness<'static, MailApp>, path: &str) {
    h.state_mut().run("ui.dialog", json!({"name": "importPst", "path": path})).unwrap();
    h.run_steps(4);
}

fn says(h: &Harness<'static, MailApp>, text: &str) -> bool {
    h.query_all_by_label_contains(text).next().is_some()
}

fn click(h: &mut Harness<'static, MailApp>, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
    h.run_steps(4);
}

fn shown(h: &Harness<'static, MailApp>) -> bool {
    h.state().dialog.as_ref().is_some_and(|d| d.name() == "importPst")
}

fn messages(h: &Harness<'static, MailApp>) -> usize {
    h.state().session.mailbox.messages.len()
}

fn type_password(h: &mut Harness<'static, MailApp>, password: &str) {
    if let Some(Dialog::ImportPst(d)) = h.state_mut().dialog.as_mut() {
        d.password = password.to_string();
    }
    h.run_steps(2);
}

#[test]
fn a_pst_files_mail_comes_in_through_the_dialog_once() {
    let dir = Scratch::new("once");
    let path = pst(&dir, 2, None);
    let mut h = window(Services::default(), &dir);
    let before = messages(&h);
    open(&mut h, &path);
    assert!(shown(&h) && says(&h, "garden.pst") && says(&h, "In a new folder called garden"));
    click(&mut h, "Import");
    assert_eq!(messages(&h), before + 2);
    assert!(says(&h, "2 messages brought in, of 2 items found."), "the report is shown");
    assert!(says(&h, "The full report is in"));
    assert!(says(&h, "In garden"), "the folder is named");
    click(&mut h, "Close");
    assert!(!shown(&h));

    open(&mut h, &path);
    click(&mut h, "Import");
    assert_eq!(messages(&h), before + 2, "a second import adds nothing");
    assert!(says(&h, "2 were here already"));
}

#[test]
fn a_wrong_password_imports_nothing() {
    let dir = Scratch::new("password");
    let path = pst(&dir, 1, Some("compost"));
    let mut h = window(Services::default(), &dir);
    let before = messages(&h);
    open(&mut h, &path);
    assert!(says(&h, "This file has a password"));
    type_password(&mut h, "mulch");
    click(&mut h, "Import");
    assert!(says(&h, "That isn't the password of this file") && shown(&h));
    assert_eq!(messages(&h), before);
    type_password(&mut h, "compost");
    click(&mut h, "Import");
    assert_eq!(messages(&h), before + 1);
}

#[test]
fn batches_come_in_as_they_arrive_and_cancel_keeps_them() {
    let dir = Scratch::new("cancel");
    let path = pst(&dir, 5, None);
    // A host that hands the job back to the test, which sends its events by hand.
    let (jobs_tx, jobs_rx) = channel::<(PstJob, Sender<Event>)>();
    let services = Services {
        import_pst: Some(Box::new(move |job: PstJob| {
            let (tx, rx) = channel();
            jobs_tx.send((job, tx)).unwrap();
            rx
        })),
        ..Default::default()
    };
    let mut h = window(services, &dir);
    let before = messages(&h);
    open(&mut h, &path);
    click(&mut h, "Import");
    let (job, tx) = jobs_rx.try_recv().unwrap();
    // The file is read here; the outcome is held back until Cancel has been pressed.
    let mut events = Vec::new();
    mailcraft_engine::import_pst::read_path(&job.path, &job.job, &job.progress, &mut |e| {
        events.push(e);
        true
    });
    let done = events.pop().unwrap();
    assert!(!events.is_empty());
    for e in events {
        tx.send(e).unwrap();
    }
    h.run_steps(4);
    assert!(messages(&h) > before, "a batch is in before the end");
    assert!(says(&h, "of 5 items"));
    let steps = h.state().session.undo_label();
    assert!(h.state_mut().session.run("edit.undo", &json!({})).is_err(), "Undo waits while it runs");
    // Closing the window while it reads stops the reading; the window stays for the report.
    if let Some(Dialog::ImportPst(d)) = h.state_mut().dialog.as_mut() {
        assert!(d.stop());
    }
    click(&mut h, "Cancel");
    assert!(job.progress.is_cancelled());
    tx.send(done).unwrap();
    h.run_steps(4);
    assert_eq!(messages(&h), before + 5);
    assert!(says(&h, "5 messages brought in"));
    // The whole import is one step to undo.
    assert_ne!(h.state().session.undo_label(), steps);
    assert_eq!(h.state().session.undo_label(), Some("Import a .pst File"));
    h.state_mut().session.run("edit.undo", &json!({})).unwrap();
    assert_eq!(messages(&h), before);
}

/// The window of an import that is reading goes (another dialog in its place): the import stops,
/// what came in is one step to undo, and Undo is free again.
#[test]
fn an_import_whose_window_goes_is_one_step_to_undo() {
    let dir = Scratch::new("replaced");
    let path = pst(&dir, 5, None);
    let (jobs_tx, jobs_rx) = channel::<(PstJob, Sender<Event>)>();
    let services = Services {
        import_pst: Some(Box::new(move |job: PstJob| {
            let (tx, rx) = channel();
            jobs_tx.send((job, tx)).unwrap();
            rx
        })),
        ..Default::default()
    };
    let mut h = window(services, &dir);
    let before = messages(&h);
    open(&mut h, &path);
    click(&mut h, "Import");
    let (job, tx) = jobs_rx.try_recv().unwrap();
    let mut events = Vec::new();
    mailcraft_engine::import_pst::read_path(&job.path, &job.job, &job.progress, &mut |e| {
        events.push(e);
        true
    });
    tx.send(events.remove(0)).unwrap();
    h.run_steps(4);
    assert!(messages(&h) > before && h.state().session.importing_pst());
    h.state_mut().dialog = None;
    h.run_steps(2);
    assert!(job.progress.is_cancelled());
    assert!(!h.state().session.importing_pst());
    assert_eq!(h.state().session.undo_label(), Some("Import a .pst File"));
    h.state_mut().session.run("edit.undo", &json!({})).unwrap();
    assert_eq!(messages(&h), before);
}

/// A window that is hidden or minimised paints no frame, but the toolkit still runs its logic
/// (#815): the batches must go in from there, or the reader waits for a frame that never comes.
#[test]
fn batches_go_in_while_no_frame_is_painted() {
    let dir = Scratch::new("hidden");
    let path = pst(&dir, 5, None);
    let (jobs_tx, jobs_rx) = channel::<(PstJob, Sender<Event>)>();
    let services = Services {
        import_pst: Some(Box::new(move |job: PstJob| {
            let (tx, rx) = channel();
            jobs_tx.send((job, tx)).unwrap();
            rx
        })),
        ..Default::default()
    };
    let mut h = window(services, &dir);
    let before = messages(&h);
    open(&mut h, &path);
    click(&mut h, "Import");
    let (job, tx) = jobs_rx.try_recv().unwrap();
    let mut events = Vec::new();
    mailcraft_engine::import_pst::read_path(&job.path, &job.job, &job.progress, &mut |e| {
        events.push(e);
        true
    });
    for e in events {
        tx.send(e).unwrap();
    }
    // Only the logic runs, as for a hidden window.
    let ctx = egui::Context::default();
    for _ in 0..8 {
        h.state_mut().logic(&ctx);
    }
    assert_eq!(messages(&h), before + 5, "everything is in without a frame");
    assert!(matches!(h.state().dialog.as_ref(), Some(Dialog::ImportPst(d)) if !d.is_importing()), "and the report is ready");
    h.run_steps(2);
    assert!(says(&h, "5 messages brought in"));
}
