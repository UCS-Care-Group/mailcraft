// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Report a Problem: what goes into GitHub's address (and what never does), the file of
//! diagnostics, the message by email, and the window.

use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;

use super::*;
use crate::Services;

const NOW: i64 = 1_791_993_600;
const EMAIL: &str = "owner@contoso.example";
const PASSWORD: &str = "hunter2-correct-horse";
const TOKEN: &str = "eyJ0eXAiOi.access-token";
const REFRESH: &str = "M.R3_BAY.refresh-token";
/// Something only the host's diagnostics say (the end of the log).
const LOG_LINE: &str = "sync: 42 messages in 3 folders";

fn host_report() -> String {
    format!("### MailCraft diagnostics\n\n- Version: 9.8.7\n- Platform: testos testarch\n\n```\n{LOG_LINE}\n```\n")
}

/// The sample mailbox with an account that sends, whose password and tokens are in memory.
fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.services.diagnostics = Some(Box::new(host_report));
    let added = a.execute(
        "account.add",
        json!({"email": EMAIL, "incoming": {"host": "imap.contoso.example"}, "outgoing": {"host": "smtp.contoso.example", "port": 587, "security": "starttls"}}),
    );
    let id = added.unwrap()["id"].as_u64().unwrap();
    a.passwords.insert(id, PASSWORD.into());
    a.tokens.insert(id, mailcraft_model::sync::Tokens { access: TOKEN.into(), refresh: REFRESH.into(), email: EMAIL.into(), ..Default::default() });
    a
}

fn report(a: &mut MailApp, p: Value) -> Result<Value, String> {
    a.execute("ui.reportProblem", p)
}

fn assert_private(url: &str) {
    for secret in [EMAIL, PASSWORD, TOKEN, REFRESH, LOG_LINE, "contoso", "sync%3A"] {
        assert!(!url.contains(secret), "{secret} in {url}");
    }
}

#[test]
fn the_address_carries_what_was_typed_and_the_version_and_nothing_else() {
    let mut a = app();
    let p = json!({"title": "Search stops", "what": "I searched & it stopped", "expected": "Results", "diagnostics": true, "open": false});
    let r = report(&mut a, p).unwrap();
    let url = r["url"].as_str().unwrap();
    assert!(url.starts_with("https://github.com/UCS-Care-Group/mailcraft/issues/new?template=problem.yml&title=Search%20stops&"), "{url}");
    assert!(url.contains("&version=9.8.7&platform=testos%20testarch&what=I%20searched%20%26%20it%20stopped"), "{url}");
    assert!(url.contains("What%20I%20expected") && url.contains("&diagnostics=Drag%20the%20diagnostics%20file"), "{url}");
    assert_eq!(r["length"].as_u64().unwrap() as usize, url.len());
    assert_private(url);
    // Even with the owner's address typed in, nothing of the account or the log comes along.
    assert_private(&issue_url(&Typed { what: "x".into(), ..Default::default() }, false, "1", "p"));
    // Without diagnostics the Diagnostics box is left alone.
    let r = report(&mut a, json!({"what": "x", "open": false})).unwrap();
    assert!(!r["url"].as_str().unwrap().contains("diagnostics="));
}

#[test]
fn whatever_is_typed_the_window_cuts_it_to_fit_and_says_so() {
    for (c, n) in [('a', 20_000), ('é', 9_000), ('😀', 7_000), ('&', 9_000), ('\n', 9_000)] {
        let long: String = std::iter::repeat_n(c, n).collect();
        let mut typed = Typed { title: long.chars().take(TITLE_CHARS).collect(), what: long.clone(), expected: long };
        assert!(clamp(&mut typed, true, "9.8.7", "testos"));
        let url = issue_url(&typed, true, "9.8.7", "testos");
        assert!(url.len() <= URL_MAX, "{c:?}: {}", url.len());
        // What fits is kept: the cutting stops close to the limit, and what happened goes last.
        assert!(url.len() > URL_MAX - 16, "{c:?}: {}", url.len());
        assert!(typed.expected.is_empty() && !typed.what.is_empty(), "{c:?}");
    }
    // What fits is left alone.
    let mut short = Typed { title: "t".into(), what: "w".into(), expected: "e".into() };
    assert!(!clamp(&mut short, true, "1", "p"));
    assert_eq!(short.expected, "e");
}

#[test]
fn a_script_that_says_too_much_or_nothing_is_refused_never_cut() {
    let mut a = app();
    assert!(report(&mut a, json!({"title": "No text", "open": false})).is_err());
    assert!(report(&mut a, json!({"what": "   ", "open": false})).is_err());
    let long = "x".repeat(TEXT_CHARS + 1);
    assert!(report(&mut a, json!({"what": long, "open": false})).is_err());
    // Within each box's limit but too much for an address altogether.
    let wide = "€".repeat(TEXT_CHARS);
    assert!(report(&mut a, json!({"what": wide, "open": false})).unwrap_err().contains("too long"));
    assert!(report(&mut a, json!({"what": "x", "via": "fax", "open": false})).is_err());
    // A script asking for the page with no window to open it in is told so.
    assert!(report(&mut a, json!({"what": "x"})).unwrap_err().contains("window"));
}

#[test]
fn the_file_holds_exactly_what_copy_diagnostics_copies() {
    let mut a = app();
    let copied = a.execute("ui.copyDiagnostics", json!({})).unwrap()["text"].as_str().unwrap().to_string();
    assert!(copied.contains(LOG_LINE));
    // No file named: the bytes are the answer, under the day's name.
    let r = report(&mut a, json!({"what": "x", "diagnostics": true, "open": false})).unwrap();
    assert_eq!(r["name"], "mailcraft-diagnostics-2026-10-14.txt");
    let bytes = base64_lite::decode(r["data"].as_str().unwrap()).unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), copied);
    // With a file named and the window up, it is written there and the page opened.
    let dir = std::env::temp_dir().join(format!("mailcraft-report-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mailcraft-diagnostics.txt");
    a.ctx = Some(egui::Context::default());
    let r = report(&mut a, json!({"what": "x", "diagnostics": true, "path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["saved"].as_str().unwrap(), path.to_string_lossy());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), copied);
    assert!(a.status_msg.as_ref().unwrap().0.contains("MailCraft sent nothing"));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(saves_file(&json!({"diagnostics": true})) && !saves_file(&json!({"diagnostics": true, "via": "email"})));
}

#[test]
fn by_email_a_new_message_to_the_maintainers_waits_for_send() {
    let mut a = app();
    let p = json!({"title": "Search stops", "what": "It stopped", "expected": "Results", "diagnostics": true, "via": "email"});
    let r = report(&mut a, p).unwrap();
    assert_eq!((r["to"].as_str().unwrap(), r["subject"].as_str().unwrap()), (MAINTAINERS, "Search stops"));
    let c = a.session.compose.as_ref().unwrap();
    assert_eq!((c.to.as_str(), c.subject.as_str()), (MAINTAINERS, "Search stops"));
    let body = c.body.plain();
    assert!(body.starts_with("It stopped") && body.contains("What I expected:\nResults") && body.contains("MailCraft version: 9.8.7"), "{body}");
    assert!(!body.contains(LOG_LINE), "{body}");
    let file = c.attachments.iter().find(|f| f.name == "mailcraft-diagnostics-2026-10-14.txt").unwrap();
    assert_eq!(*file.data, diagnostics(&a).into_bytes());
    // It is sent from the account that sends mail, and not sent at all.
    let from = a.session.mailbox.account(c.account).unwrap();
    assert_eq!(from.email, EMAIL);
    assert!(
        a.session.mailbox.folders.iter().filter(|f| f.kind == mailcraft_model::FolderKind::Outbox).all(|f| a
            .session
            .mailbox
            .messages
            .values()
            .all(|m| m.folder != f.id))
    );
    // While that message is open another isn't started.
    assert!(report(&mut a, json!({"what": "x", "via": "email"})).unwrap_err().contains("Finish"));
}

#[test]
fn by_email_is_refused_without_an_account_that_sends() {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    assert!(a.session.mailbox.accounts.iter().all(|x| !x.kind.sends_mail()), "the sample sends nothing");
    assert!(email_refused(&a).unwrap().contains("Add an account"));
    assert!(report(&mut a, json!({"what": "x", "via": "email"})).is_err());
    assert!(a.session.compose.is_none());
}

#[test]
fn the_maintainers_address_is_funding_jsons() {
    let funding: Value = serde_json::from_str(include_str!("../../../../funding.json")).unwrap();
    assert_eq!(MAINTAINERS, funding["entity"]["email"].as_str().unwrap());
}

/// The whole window, kept for clicking.
fn window(a: MailApp) -> egui_kittest::Harness<'static, MailApp> {
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        a,
    );
    h.run_steps(3);
    h
}

#[test]
fn by_hand_the_window_opens_and_a_script_never_opens_it() {
    let mut a = app();
    assert!(report(&mut a, json!({})).is_err());
    assert!(a.dialog.is_none());
    let mut h = window(a);
    h.get_by_label("Report a Problem").click();
    h.run_steps(3);
    assert!(matches!(h.state().dialog, Some(crate::dialogs::Dialog::ReportProblem(_))));
    // It shows the diagnostics, all of them, before anything goes.
    assert!(h.query_by_label_contains(LOG_LINE).is_some() || h.query_by_value(&diagnostics(h.state())).is_some());
    let Some(crate::dialogs::Dialog::ReportProblem(d)) = h.state_mut().dialog.as_mut() else { panic!("not the window") };
    d.typed.what = "It stopped".into();
    d.include = false;
    h.run_steps(2);
    h.get_by_label("Open on GitHub").click();
    // (The frame that takes the click is the one that opens the page.)
    h.step();
    let opened: Vec<String> = h
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::OpenUrl(u) => Some(u.url.clone()),
            _ => None,
        })
        .collect();
    assert!(h.state().dialog.is_none());
    let url = opened.first().unwrap_or_else(|| panic!("no page: {:?} {:?}", h.state().status_msg, h.output().platform_output.commands));
    assert!(url.contains("what=It%20stopped"), "{url}");
    assert_private(url);
}

fn opened(h: &egui_kittest::Harness<'static, MailApp>) -> Vec<String> {
    h.output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::OpenUrl(u) => Some(u.url.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_cancelled_save_keeps_the_window_and_what_was_typed() {
    let mut a = app();
    // A save dialog the test answers when it likes, as the owner would.
    let (answers_tx, answers) = std::sync::mpsc::channel::<std::sync::mpsc::Sender<Option<String>>>();
    a.services.file_dialog = Some(Box::new(move |_| {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = answers_tx.send(tx);
        rx
    }));
    let mut h = window(a);
    h.get_by_label("Report a Problem").click();
    h.run_steps(3);
    let Some(crate::dialogs::Dialog::ReportProblem(d)) = h.state_mut().dialog.as_mut() else { panic!("not the window") };
    d.typed.what = "It stopped".into();
    h.run_steps(2);
    h.get_by_label("Open on GitHub").click();
    h.step();
    // The dialog is asked for, the window waits, and no page is opened yet.
    let answer = answers.try_recv().unwrap();
    assert!(opened(&h).is_empty());
    assert!(matches!(h.state().dialog, Some(crate::dialogs::Dialog::ReportProblem(_))));
    answer.send(None).unwrap();
    h.run_steps(3);
    let Some(crate::dialogs::Dialog::ReportProblem(d)) = h.state().dialog.as_ref() else { panic!("the window closed on a cancel") };
    assert_eq!(d.typed.what, "It stopped");
    assert!(!d.waiting && !d.error.is_empty());
    assert!(opened(&h).is_empty());

    // Once more, and this time the file is saved: then the page opens and the window closes.
    let path = std::env::temp_dir().join(format!("mailcraft-report-test-{}.txt", std::process::id()));
    h.get_by_label("Open on GitHub").click();
    h.step();
    answers.try_recv().unwrap().send(Some(path.to_string_lossy().to_string())).unwrap();
    h.step();
    let urls = opened(&h);
    assert!(h.state().dialog.is_none());
    assert!(urls.first().is_some_and(|u| u.contains("what=It%20stopped")), "{urls:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), diagnostics(h.state()));
    let _ = std::fs::remove_file(&path);
}
