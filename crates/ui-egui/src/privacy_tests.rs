// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the window says and logs about the owner's mail and choices (security batch 6).

use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;

use crate::{MailApp, Services};

const NOW: i64 = 1_791_990_000;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    app
}

/// (A5-3) A Send/Receive problem names a subject, an address, a folder: the owner reads it in the
/// problems report, and the log has only how many there were.
#[test]
fn send_receive_problems_are_kept_for_the_owner_and_not_logged() {
    let logged = crate::graph_switch::tests::logged();
    let mut app = app();
    let account = app.session.mailbox.accounts[0].id;
    let lines = vec![
        "Held \"Quarterly figures for Ada\" (ada@example.org): the server said no".to_string(),
        "Couldn't open \"Private/Taxes\": no".to_string(),
    ];
    app.note_sync_problems(account, &lines, true);
    assert_eq!(app.sync_problem_lines().iter().filter(|l| l.contains("Quarterly figures") || l.contains("Private/Taxes")).count(), 2);
    let log = logged.lock().unwrap().join("\n");
    assert!(log.contains("2 problem(s) reported"), "{log}");
    for secret in ["Quarterly figures", "ada@example.org", "Private/Taxes"] {
        assert!(!log.contains(secret), "{secret} reached the log");
    }
}

/// (A3-5) The question says what a Yes does: check, download and install.
#[test]
fn the_update_question_names_installing() {
    let mut app = app();
    app.updates.asking = true;
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    assert!(harness.query_all_by_label_contains("download and install them when you quit").next().is_some());
}
