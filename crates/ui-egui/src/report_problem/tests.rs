// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Report a Problem: what goes into GitHub's address (and what never does), the file of
//! diagnostics, and the window.

use egui_kittest::kittest::{NodeT as _, Queryable as _};
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
    assert!(saves_file(&json!({"diagnostics": true})));
}

#[test]
fn email_is_not_a_way_any_more() {
    let mut a = app();
    assert!(report(&mut a, json!({"what": "x", "via": "email"})).is_err());
    assert!(a.session.compose.is_none());
}

/// The whole window, kept for clicking.
fn window(a: MailApp) -> egui_kittest::Harness<'static, MailApp> {
    window_of(a, 800.0)
}

/// [`window`], `height` high.
fn window_of(a: MailApp, height: f32) -> egui_kittest::Harness<'static, MailApp> {
    let mut h =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, height)).build_ui_state(
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
    let (answers_tx, answers) = std::sync::mpsc::channel::<std::sync::mpsc::Sender<Option<Vec<String>>>>();
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
    answers.try_recv().unwrap().send(Some(vec![path.to_string_lossy().to_string()])).unwrap();
    h.step();
    let urls = opened(&h);
    assert!(h.state().dialog.is_none());
    assert!(urls.first().is_some_and(|u| u.contains("what=It%20stopped")), "{urls:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), diagnostics(h.state()));
    let _ = std::fs::remove_file(&path);
}

/// What a stand-in host was asked to send.
type Asked = std::sync::Arc<std::sync::Mutex<Vec<ProblemReport>>>;

/// `a` with a host that sends reports: each is kept, and answered with the next of `answers`
/// (the last one again once they run out).
fn with_relay(a: &mut MailApp, answers: Vec<Result<u64, NotSent>>) -> Asked {
    let asked: Asked = Default::default();
    let kept = asked.clone();
    a.services.report_send = Some(Box::new(move |report| {
        let mut all = kept.lock().unwrap();
        all.push(report);
        let answer = answers.get(all.len() - 1).or(answers.last()).copied().unwrap_or(Err(NotSent::Garbled));
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(answer).unwrap();
        rx
    }));
    asked
}

fn draft<'h>(h: &'h mut egui_kittest::Harness<'static, MailApp>) -> &'h mut Draft {
    let Some(crate::dialogs::Dialog::ReportProblem(d)) = h.state_mut().dialog.as_mut() else { panic!("not the window") };
    d
}

fn open_window(a: MailApp) -> egui_kittest::Harness<'static, MailApp> {
    // (Tall enough for the window's buttons below a long text.)
    let mut h = window_of(a, 1400.0);
    h.get_by_label("Report a Problem").click();
    h.run_steps(3);
    let d = draft(&mut h);
    d.typed.title = "Search stops".into();
    d.typed.what = "It stopped".into();
    h.run_steps(2);
    h
}

#[test]
fn send_report_sends_what_the_window_shows_once_and_says_the_reference() {
    let mut a = app();
    let asked = with_relay(&mut a, vec![Ok(42)]);
    let mut h = open_window(a);
    draft(&mut h).username = "@octocat".into();
    h.run_steps(1);
    // The privacy line is above Send.
    assert!(h.query_by_label_contains("Sent to the MailCraft team privately").is_some());
    h.get_by_label("Send report").click();
    h.run_steps(3);
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    let r = &asked[0];
    assert_eq!(
        (r.title.as_str(), r.text.as_str(), r.version.as_str(), r.platform.as_str()),
        ("Search stops", "It stopped", "9.8.7", "testos testarch")
    );
    assert_eq!(r.username.as_deref(), Some("octocat"));
    // The diagnostics are those the window showed, and nothing of the account is in the report.
    assert_eq!(r.diagnostics.as_deref(), Some(diagnostics(h.state()).as_str()));
    let all = format!("{r:?}");
    for secret in [EMAIL, PASSWORD, TOKEN, REFRESH] {
        assert!(!all.contains(secret), "{secret}");
    }
    // Sent: the reference, not a link, and the draft cleared.
    assert!(h.query_by_label_contains("Sent, reference R-42").is_some());
    let d = draft(&mut h);
    assert_eq!(d.sent, Some(42));
    assert!(d.typed.what.is_empty() && d.typed.title.is_empty() && d.username.is_empty());
    assert!(opened(&h).is_empty(), "nothing opened in the browser");
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_none());
}

#[test]
fn not_sent_keeps_the_draft_says_why_and_never_tries_again_by_itself() {
    let kinds = [
        (NotSent::Refused(Field::Title), "title"),
        (NotSent::Refused(Field::Username), "GitHub username"),
        (NotSent::Refused(Field::Diagnostics), "Untick Include diagnostics"),
        (NotSent::Refused(Field::Request), "didn't take the report"),
        (NotSent::RateLimited, "as many reports"),
        (NotSent::Disabled, "isn't taking reports"),
        (NotSent::Unavailable, "isn't working"),
        (NotSent::Failed, "couldn't file"),
        (NotSent::Unreachable, "couldn't be reached"),
        (NotSent::TooSlow, "didn't answer in time"),
        (NotSent::Garbled, "may have arrived"),
    ];
    for (why, said) in kinds {
        assert!(why_not_sent(why).contains(said), "{why:?}");
        let mut a = app();
        let asked = with_relay(&mut a, vec![Err(why), Ok(7)]);
        let mut h = open_window(a);
        h.get_by_label("Send report").click();
        h.run_steps(6);
        assert_eq!(asked.lock().unwrap().len(), 1, "{why:?}: sent once and no more");
        assert!(h.query_by_label_contains(why_not_sent(why)).is_some(), "{why:?}");
        let d = draft(&mut h);
        assert_eq!(d.not_sent, Some(why));
        assert_eq!(d.typed.what, "It stopped", "{why:?}: the draft is kept");
        // Try again, and Open on GitHub, are offered; Try again sends once more.
        assert!(h.query_by_label("Open on GitHub").is_some());
        h.get_by_label("Try again").click();
        h.run_steps(3);
        assert_eq!(asked.lock().unwrap().len(), 2, "{why:?}");
        assert_eq!(draft(&mut h).sent, Some(7));
    }
}

#[test]
fn a_report_the_relay_would_refuse_sends_nothing() {
    let mut a = app();
    let asked = with_relay(&mut a, vec![Ok(1)]);
    let mut h = open_window(a);
    draft(&mut h).username = "not a name".into();
    h.run_steps(1);
    h.get_by_label("Send report").click();
    h.run_steps(3);
    assert!(asked.lock().unwrap().is_empty());
    // Said as found here (nothing went), and the button is still Send report, not Try again.
    let d = draft(&mut h);
    assert_eq!((d.not_sent, d.error.as_str()), (None, why_not_sendable(Field::Username)));
    // No title: said, and nothing sent.
    let d = draft(&mut h);
    d.username.clear();
    d.typed.title.clear();
    h.run_steps(1);
    h.get_by_label("Send report").click();
    h.run_steps(3);
    assert!(asked.lock().unwrap().is_empty());
    assert_eq!(draft(&mut h).error, why_not_sendable(Field::Title));
    // Too long altogether: what happened and what was expected count together, and that is shown.
    let d = draft(&mut h);
    d.typed.title = "t".into();
    d.typed.what = "a".repeat(TEXT_CHARS);
    d.typed.expected = "b".repeat(10);
    h.run_steps(2);
    assert!(h.query_by_label_contains("characters together").is_some());
    // (Sent as the button does, the boxes being too tall for the button to be on screen.)
    let Some(crate::dialogs::Dialog::ReportProblem(mut d)) = h.state_mut().dialog.take() else { panic!("not the window") };
    d.send(h.state());
    assert!(asked.lock().unwrap().is_empty());
    assert_eq!(d.error, why_not_sendable(Field::Text));
}

/// A host that sends reports and never answers: the report stays on its way.
fn with_silent_relay(a: &mut MailApp) -> Asked {
    let asked: Asked = Default::default();
    let kept = asked.clone();
    let held: std::sync::Arc<std::sync::Mutex<Vec<std::sync::mpsc::Sender<Result<u64, NotSent>>>>> = Default::default();
    a.services.report_send = Some(Box::new(move |report| {
        kept.lock().unwrap().push(report);
        let (tx, rx) = std::sync::mpsc::channel();
        held.lock().unwrap().push(tx);
        rx
    }));
    asked
}

#[test]
fn a_report_on_its_way_keeps_the_window_and_nothing_takes_its_place() {
    let mut a = app();
    let asked = with_silent_relay(&mut a);
    let mut h = open_window(a);
    h.get_by_label("Send report").click();
    h.run_steps(3);
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(draft(&mut h).sending.is_some());
    // Escape and Cancel don't close it: its answer is waited for, and said.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.get_by_label("Cancel").accesskit_node().is_disabled());
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(draft(&mut h).sending.is_some(), "the window stayed");
    // A script can't put another report in its place, by either way.
    assert!(h.state_mut().execute("ui.reportProblem", json!({"what": "other", "via": "send"})).is_err());
    assert!(h.state_mut().execute("ui.dialog", json!({"name": "reportProblem", "what": "other"})).is_err());
    assert_eq!(draft(&mut h).typed.what, "It stopped");
}

#[test]
fn a_click_the_control_channel_makes_on_send_report_sends_nothing() {
    let mut a = app();
    let asked = with_relay(&mut a, vec![Ok(1)]);
    let mut h = open_window(a);
    let at = h.get_by_label("Send report").rect().center();
    // As `ui.click` queues it, and as the window takes it in, frame by frame (`raw_input_hook`).
    let synthetic = &mut h.state_mut().synthetic;
    synthetic.push(egui::Event::PointerMoved(at));
    for pressed in [true, false] {
        synthetic.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
    }
    for _ in 0..6 {
        let mut raw = egui::RawInput::default();
        h.state_mut().raw_input_hook(&mut raw);
        h.input_mut().events.extend(raw.events);
        h.step();
    }
    assert!(h.state().synthetic.is_empty(), "the click was taken in");
    assert!(asked.lock().unwrap().is_empty(), "only the owner's own click sends");
    // The owner's own click, at the same place, does.
    h.get_by_label("Send report").click();
    h.run_steps(3);
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[test]
fn with_diagnostics_too_open_on_github_keeps_what_didnt_fit() {
    let mut a = app();
    let _asked = with_relay(&mut a, vec![Ok(1)]);
    let (answers_tx, answers) = std::sync::mpsc::channel::<std::sync::mpsc::Sender<Option<Vec<String>>>>();
    a.services.file_dialog = Some(Box::new(move |_| {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = answers_tx.send(tx);
        rx
    }));
    let mut h = open_window(a);
    let long = "€".repeat(1_000);
    draft(&mut h).typed.what = long.clone();
    // (The window settles at its new height first.)
    h.run_steps(6);
    h.get_by_label("Open on GitHub").click();
    h.step();
    let path = std::env::temp_dir().join(format!("mailcraft-report-kept-{}.txt", std::process::id()));
    answers.try_recv().unwrap().send(Some(vec![path.to_string_lossy().to_string()])).unwrap();
    h.step();
    assert!(opened(&h).first().is_some_and(|u| u.len() <= URL_MAX));
    h.run_steps(3);
    let d = draft(&mut h);
    assert!(d.cut && !d.waiting && d.error.is_empty(), "{}", d.error);
    assert_eq!(d.typed.what, long);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_script_or_an_agent_fills_the_window_and_never_sends() {
    let mut a = app();
    let asked = with_relay(&mut a, vec![Ok(1)]);
    // `via: "send"` from a script: the window, filled in, and nothing sent.
    let r = report(&mut a, json!({"title": "From a script", "what": "It stopped", "username": "octocat", "via": "send"})).unwrap();
    assert_eq!(r["draft"], true);
    assert_eq!(r["sent"], false);
    let Some(crate::dialogs::Dialog::ReportProblem(d)) = a.dialog.as_ref() else { panic!("no window") };
    assert_eq!((d.typed.title.as_str(), d.typed.what.as_str(), d.username.as_str()), ("From a script", "It stopped", "octocat"));
    // Never over a window that is open: what the owner may be writing stays.
    assert!(report(&mut a, json!({"what": "Something else", "via": "send"})).is_err());
    let Some(crate::dialogs::Dialog::ReportProblem(d)) = a.dialog.as_ref() else { panic!("no window") };
    assert_eq!(d.typed.what, "It stopped");
    // What a script fills in is held to what the boxes hold.
    a.dialog = None;
    a.execute("ui.dialog", json!({"name": "reportProblem", "what": "x", "expected": "y".repeat(TEXT_CHARS * 2)})).unwrap();
    let Some(crate::dialogs::Dialog::ReportProblem(d)) = a.dialog.as_ref() else { panic!("no window") };
    assert_eq!(d.typed.expected.chars().count(), TEXT_CHARS);
    // From an agent the owner let send: the same, and still nothing sent.
    a.dialog = None;
    let (req, _rx) = crate::ControlRequest::new("engine.execute", json!({"command": "ui.reportProblem", "params": {"what": "x", "via": "send"}}));
    let _ = crate::control::handle(&mut a, &egui::Context::default(), &req.from_agent(vec![mailcraft_engine::agent::Class::Send]));
    assert!(matches!(a.dialog, Some(crate::dialogs::Dialog::ReportProblem(_))));
    assert!(asked.lock().unwrap().is_empty());
    // A click the control channel made on Send report sends nothing either.
    a.dialog = None;
    let mut h = open_window(a);
    h.state_mut().driven = true;
    h.get_by_label("Send report").click();
    h.run_steps(3);
    assert!(asked.lock().unwrap().is_empty(), "only the owner's own click sends");
    assert!(draft(&mut h).sending.is_none());
    // The owner's own click does.
    h.state_mut().driven = false;
    h.get_by_label("Send report").click();
    h.run_steps(3);
    assert_eq!(asked.lock().unwrap().len(), 1);
}

/// `MAILCRAFT_SHOTS_DIR=<dir> cargo test -p mailcraft-ui-egui shots_of_report_a_problem -- --ignored`:
/// the window as written, sending, sent, and not sent for each reason, in light and dark.
#[test]
#[ignore = "writes screenshots, for looking at them"]
fn shots_of_report_a_problem() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else {
        eprintln!("set MAILCRAFT_SHOTS_DIR to the directory to write to");
        return;
    };
    let mut states: Vec<(String, Option<Result<u64, NotSent>>, bool)> =
        vec![("draft".into(), None, true), ("github-only".into(), None, false), ("sending".into(), None, true), ("sent".into(), Some(Ok(42)), true)];
    for why in [
        NotSent::Refused(Field::Username),
        NotSent::Refused(Field::Request),
        NotSent::RateLimited,
        NotSent::Disabled,
        NotSent::Unavailable,
        NotSent::Failed,
        NotSent::Unreachable,
        NotSent::TooSlow,
        NotSent::Garbled,
    ] {
        states.push((format!("{why:?}").to_lowercase().replace(['(', ')'], "-"), Some(Err(why)), true));
    }
    for theme in ["light", "dark"] {
        for (name, answer, relay) in &states {
            let mut a = app();
            a.execute("ui.theme", json!({"value": theme})).unwrap();
            if *relay {
                // (Sending: an answer that never comes.)
                let held: std::sync::Arc<std::sync::Mutex<Vec<std::sync::mpsc::Sender<Result<u64, NotSent>>>>> = Default::default();
                let answer = *answer;
                a.services.report_send = Some(Box::new(move |_| {
                    let (tx, rx) = std::sync::mpsc::channel();
                    match answer {
                        Some(answer) => tx.send(answer).unwrap(),
                        None => held.lock().unwrap().push(tx),
                    }
                    rx
                }));
            }
            let mut d = Draft::new(
                &a,
                &json!({"title": "Search stops after the first page", "what": "I searched the Inbox for invoices and the results stopped after twenty.", "expected": "All of them"}),
            );
            if *relay && name != "draft" {
                d.username = "octocat".into();
            }
            a.dialog = Some(crate::dialogs::Dialog::ReportProblem(Box::new(d)));
            let mut harness = egui_kittest::Harness::builder()
                .with_accessibility_check(false)
                .allow_missing_glyphs()
                .with_size(egui::vec2(1280.0, 900.0))
                .with_step_dt(1.0 / 60.0)
                .wgpu()
                .build_ui_state(
                    |ui, app: &mut MailApp| {
                        let ctx = ui.ctx().clone();
                        app.logic(&ctx);
                        app.ui(ui);
                    },
                    a,
                );
            harness.input_mut().max_texture_side = Some(8192);
            harness.run_steps(2);
            let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
            harness.run_steps(4);
            if name != "draft" && name != "github-only" {
                harness.get_by_label("Send report").click();
            }
            harness.run_steps(12);
            let path = std::path::Path::new(&dir).join(format!("report-{name}-{theme}.png"));
            harness.render().unwrap().save(&path).unwrap();
            println!("{}", path.display());
        }
    }
}

#[test]
fn without_a_relay_the_window_offers_open_on_github_alone() {
    let h = open_window(app());
    assert!(h.query_by_label("Send report").is_none());
    assert!(h.query_by_label("Open on GitHub").is_some());
    assert!(h.query_by_label_contains("Sent to the MailCraft team privately").is_none());
    assert!(h.query_by_label_contains("GitHub issues are public").is_some());
}

#[test]
fn with_a_relay_open_on_github_leaves_the_boxes_whole() {
    let mut a = app();
    let _asked = with_relay(&mut a, vec![Ok(1)]);
    let mut h = open_window(a);
    // (Nine bytes each in the address: a thousand are more than it carries, and few lines.)
    let long = "€".repeat(1_000);
    draft(&mut h).typed.what = long.clone();
    draft(&mut h).include = false;
    h.run_steps(2);
    // The boxes aren't cut for GitHub's sake while Send report can take it all.
    assert_eq!(draft(&mut h).typed.what, long);
    h.get_by_label("Open on GitHub").click();
    h.step();
    let urls = opened(&h);
    assert!(urls.first().is_some_and(|u| u.len() <= URL_MAX), "{urls:?}");
    // What didn't fit stays in the window, and that is said.
    let d = draft(&mut h);
    assert!(d.cut);
    assert_eq!(d.typed.what, long);
}
