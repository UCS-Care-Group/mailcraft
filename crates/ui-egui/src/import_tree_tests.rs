// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Import a Folder of Mailboxes… (#420) off the window's thread: progress shown while the host
//! looks and reads, Cancel at either stage leaving the mailbox as it was, and the answer of
//! another mailbox's job dropped. The host is a stand-in whose threads the test answers itself.

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_engine::import_tree::{Parsed, Progress, load_entries};
use serde_json::{Value, json};

use crate::import_tree::TreeJob;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Jobs = Arc<Mutex<Vec<(bool, Arc<Progress>, Sender<Result<Parsed, String>>)>>>;

fn mbox(n: usize, tag: &str) -> String {
    let mut out = String::new();
    for i in 0..n {
        out.push_str(&format!(
            "From a@example.org Mon Oct 12 10:00:00 2026\nFrom: a@example.org\nSubject: {tag} {i}\nMessage-ID: <{tag}-{i}@example.org>\n\nBody\n\n"
        ));
    }
    out
}

/// Base64 of text, enough for the engine's `entries`.
fn b64(text: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in text.as_bytes().chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, sh) in [18u32, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= c.len() { T[((n >> sh) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

fn parsed(count_only: bool) -> Parsed {
    let list: Vec<Value> = vec![json!({"path": "M/Inbox", "data": b64(&mbox(3, "in"))}), json!({"path": "M/Sent", "data": b64(&mbox(2, "se"))})];
    load_entries(&list, Some("Old"), count_only, &Progress::default()).unwrap()
}

fn window() -> (Harness<'static, MailApp>, Jobs) {
    let jobs: Jobs = Arc::default();
    let seen = jobs.clone();
    let services = Services {
        import_tree: Some(Box::new(move |job: TreeJob| {
            let (tx, rx) = channel();
            seen.lock().unwrap().push((job.count_only, job.progress, tx));
            rx
        })),
        ..Default::default()
    };
    let app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), services);
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    (h, jobs)
}

fn open(h: &mut Harness<'static, MailApp>) {
    h.state_mut().run("ui.dialog", json!({"name": "importTree", "path": "/mail"})).unwrap();
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
    h.state().dialog.as_ref().is_some_and(|d| d.name() == "importTree")
}

fn fingerprint(h: &Harness<'static, MailApp>) -> (usize, usize) {
    (h.state().session.mailbox.messages.len(), h.state().session.mailbox.folders.len())
}

#[test]
fn the_look_shows_progress_and_cancel_changes_nothing() {
    let (mut h, jobs) = window();
    let before = fingerprint(&h);
    open(&mut h);
    assert!(shown(&h) && says(&h, "Looking…"));
    let progress = jobs.lock().unwrap()[0].1.clone();
    assert!(jobs.lock().unwrap()[0].0, "the look only counts");
    // The counts the thread has reached are shown, and move on.
    progress.folders.store(3, std::sync::atomic::Ordering::Relaxed);
    progress.messages.store(41, std::sync::atomic::Ordering::Relaxed);
    h.run_steps(4);
    assert!(says(&h, "3 folders") && says(&h, "41 messages"));
    progress.messages.store(900, std::sync::atomic::Ordering::Relaxed);
    h.run_steps(4);
    assert!(says(&h, "900 messages"));
    click(&mut h, "Cancel");
    assert!(!shown(&h));
    assert!(progress.is_cancelled());
    assert_eq!(fingerprint(&h), before);
    assert!(!h.state().session.can_undo());
}

#[test]
fn cancelling_the_import_leaves_the_mailbox_as_it_was() {
    let (mut h, jobs) = window();
    let before = fingerprint(&h);
    open(&mut h);
    jobs.lock().unwrap()[0].2.send(Ok(parsed(true))).unwrap();
    h.run_steps(4);
    assert!(says(&h, "Nothing is imported until you press Import."));
    click(&mut h, "Import");
    assert!(says(&h, "Importing…"));
    let (count_only, progress, _) = {
        let j = jobs.lock().unwrap();
        (j[1].0, j[1].1.clone(), ())
    };
    assert!(!count_only);
    progress.messages.store(2, std::sync::atomic::Ordering::Relaxed);
    h.run_steps(4);
    assert!(says(&h, "2 messages"));
    // Nothing is in the mailbox while it is read.
    assert_eq!(fingerprint(&h), before);
    click(&mut h, "Cancel");
    assert!(!shown(&h) && progress.is_cancelled());
    assert_eq!(fingerprint(&h), before);
    assert!(h.state().session.staged_tree.is_none() && !h.state().session.can_undo());
}

#[test]
fn a_finished_import_is_put_in_as_one_undo_step() {
    let (mut h, jobs) = window();
    let before = fingerprint(&h);
    open(&mut h);
    jobs.lock().unwrap()[0].2.send(Ok(parsed(true))).unwrap();
    h.run_steps(4);
    click(&mut h, "Import");
    jobs.lock().unwrap()[1].2.send(Ok(parsed(false))).unwrap();
    h.run_steps(4);
    assert!(!shown(&h));
    assert_eq!(h.state().session.mailbox.messages.len(), before.0 + 5);
    assert!(h.state_mut().session.undo());
    assert_eq!(fingerprint(&h), before);
    assert!(!h.state().session.can_undo());
}

#[test]
fn an_answer_after_the_mailbox_was_swapped_is_dropped() {
    let (mut h, jobs) = window();
    open(&mut h);
    jobs.lock().unwrap()[0].2.send(Ok(parsed(true))).unwrap();
    h.run_steps(4);
    click(&mut h, "Import");
    let other = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0).mailbox;
    h.state_mut().session.replace_mailbox(other);
    let after_swap = fingerprint(&h);
    jobs.lock().unwrap()[1].2.send(Ok(parsed(false))).unwrap();
    h.run_steps(4);
    assert!(!shown(&h));
    assert_eq!(fingerprint(&h), after_swap);
    assert!(jobs.lock().unwrap()[1].1.is_cancelled());
    assert!(h.state().session.staged_tree.is_none());
}

#[test]
fn a_look_that_fails_says_why_and_offers_no_import() {
    let (mut h, jobs) = window();
    open(&mut h);
    jobs.lock().unwrap()[0].2.send(Err("/mail: not a folder".to_string())).unwrap();
    h.run_steps(4);
    assert!(says(&h, "not a folder"));
    assert!(shown(&h));
}
