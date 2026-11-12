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
/// problems report; the log has how many there were and each text with every value put aside
/// (`problem_log::masked`), so that no subject, folder, address, password or token reaches it,
/// whether or not the quotes round it pair up.
#[test]
fn send_receive_problems_are_kept_for_the_owner_and_logged_without_their_values() {
    use mailcraft_i18n::{Arg, Text};
    let logged = crate::graph_switch::tests::logged();
    let mut app = app();
    let account = app.session.mailbox.accounts[0].id;
    let said = |subject: &str, why: &str| Text {
        template: "Couldn't send \"{subject}\": {why}",
        args: vec![("subject", Arg::Plain(subject.into())), ("why", Arg::Plain(why.into()))],
    };
    let problems = vec![
        said("Quarterly figures for Ada", "535 5.7.8 password hunter2-secret-pw refused for ada@example.org"),
        said("Re: \"Offer\" for Bob Smith", "token tok_9f8e7d6c5b4a refused"),
        said("Bob's \"Secret plan for Carol", "no"),
        Text { template: "Couldn't open \"{name}\"", args: vec![("name", Arg::Plain("Private/Taxes".into()))] },
    ];
    app.note_said_problems(account, problems, true);
    // The owner's report keeps them whole.
    assert!(app.sync_problem_lines().iter().any(|l| l.contains("Quarterly figures")));
    let log = logged.lock().unwrap().join("\n");
    assert!(log.contains("problem: Couldn't send"), "{log}");
    for secret in
        ["Quarterly", "figures", "Offer", "Bob Smith", "Secret plan", "Carol", "Private", "Taxes", "ada@example.org", "hunter2", "tok_9f8e", "Bob's"]
    {
        assert!(!log.contains(secret), "{secret} reached the log: {log}");
    }
}

/// (A3-5) The question says what a Yes does: check, download and install.
#[test]
fn the_update_question_names_installing() {
    let mut app = app();
    app.updates.asking = true;
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1400.0, 800.0)).build_ui_state(
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
