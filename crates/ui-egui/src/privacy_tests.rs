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

/// (A5-3) A problem told as a free string (Graph, calendar, tasks, rules, sign-in) names its
/// subjects in quotes nothing can be sure of: the log has the count and the job, never the text.
#[test]
fn free_string_problems_are_counted_in_the_log_and_never_quoted() {
    let logged = crate::graph_switch::tests::logged();
    let mut app = app();
    let account = app.session.mailbox.accounts[0].id;
    let lines = vec![
        "Couldn't send \"Re: \"Offer\" for Bob Smith through Microsoft: refused".to_string(),
        "Couldn't move \"Taxes 2026 for ada@example.org".to_string(),
    ];
    app.note_sync_problems(account, &lines, true);
    app.note_problems_from("calendar", account, &lines, None, false);
    assert!(app.sync_problem_lines().iter().any(|l| l.contains("Offer")));
    let log = logged.lock().unwrap().join("\n");
    assert!(log.contains("(mail): 2 problem(s) reported") && log.contains("(calendar): 2 problem(s) reported"), "{log}");
    for secret in ["Offer", "Bob Smith", "Taxes 2026", "ada@example.org"] {
        assert!(!log.contains(secret), "{secret} reached the log: {log}");
    }
}

/// A job that ends without problems forgets what the log said of it, whether or not it is the
/// kind that clears the window's report.
#[test]
fn a_job_with_no_problems_makes_a_returning_one_new() {
    let logged = crate::graph_switch::tests::logged();
    let mut app = app();
    let account = app.session.mailbox.accounts[0].id;
    let said = vec!["calendar-forget-marker …".to_string()];
    let count = || logged.lock().unwrap().iter().filter(|l| l.contains("calendar-forget-marker")).count();
    app.note_problems_from("calendar", account, &said, Some(&said), false);
    app.note_problems_from("calendar", account, &said, Some(&said), false);
    assert_eq!(count(), 1);
    app.note_problems_from("calendar", account, &[], None, false);
    app.note_problems_from("calendar", account, &said, Some(&said), false);
    assert_eq!(count(), 2);
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

/// Uses up the allowance of `account`'s mail, as a run of many problems does.
fn use_up_allowance(app: &mut MailApp, account: mailcraft_model::Id) {
    let lines: Vec<String> = (0..crate::problem_log::MAX_LOGGED).map(|i| format!("problem {}", "x".repeat(i + 1))).collect();
    assert_eq!(app.logged_problems.fresh(account, "mail", &lines).0.len(), lines.len());
}

/// What a run that stops without an answer leaves behind is not held against the next run.
#[test]
fn a_run_whose_thread_ended_early_gives_its_allowance_back() {
    let mut app = app();
    let account = app.session.mailbox.accounts[0].id;
    use_up_allowance(&mut app, account);
    let (tx, rx) = std::sync::mpsc::channel();
    drop(tx);
    app.sync_rx = Some(crate::MailRun::Imap { account, opening: app.session.openings(), rx });
    app.poll_sync();
    assert!(app.sync_rx.is_none());
    assert_eq!(app.logged_problems.fresh(account, "mail", &["a new problem".to_string()]).0.len(), 1);
}

/// A mailbox that takes the place of the one a run was for starts with a clean memory.
#[test]
fn a_run_let_go_of_with_its_mailbox_leaves_no_memory() {
    let mut app = app();
    let account = app.session.mailbox.accounts[0].id;
    use_up_allowance(&mut app, account);
    let (_tx, rx) = std::sync::mpsc::channel();
    app.sync_rx = Some(crate::MailRun::Imap { account, opening: app.session.openings().wrapping_add(1), rx });
    app.poll_sync();
    assert!(app.sync_rx.is_none());
    assert_eq!(app.logged_problems.fresh(account, "mail", &["problem x".to_string()]).0.len(), 1);
}
