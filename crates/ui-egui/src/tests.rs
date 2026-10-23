// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window without a window: what `run` does around commands, and frames laid out headlessly in
//! every module, page and dialog (nothing may panic, whatever is on screen).

use mailcraft_engine::Session;
use serde_json::{Value, json};

use crate::{MailApp, Services, UiState};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

fn first_message(app: &mut MailApp) -> u64 {
    app.session.rows().first().map(|r| r.id).unwrap()
}

/// Lay out `frames` frames of the whole window at `size`.
fn frames(app: MailApp, size: (f32, f32), frames: usize) -> MailApp {
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(size.0, size.1)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    for _ in 0..frames {
        harness.step();
    }
    // The harness owns the app; hand back a fresh one carrying the same session for more checks.
    let state = harness.state_mut();
    let session = std::mem::replace(&mut state.session, Session::empty());
    let mut out = MailApp::new(session, Services::default());
    out.ui = state.ui.clone();
    out.autosave = false;
    out
}

#[test]
fn commands_missing_input_open_their_dialog() {
    let mut a = app();
    assert_eq!(a.run("folder.new", json!({})).unwrap()["pending"], "folder");
    assert_eq!(a.dialog.as_ref().map(|d| d.name()), Some("folder"));
    a.dialog = None;
    // With its input, the same command just runs.
    assert!(a.run("folder.new", json!({"name": "Later", "parent": null})).unwrap()["id"].is_u64());
    assert!(a.dialog.is_none());
    assert_eq!(a.run("view.goToDate", json!({})).unwrap()["pending"], "goToDate");
    a.dialog = None;
    // A disabled command doesn't open a dialog for input it couldn't use.
    assert!(a.run("calendar.edit", json!({})).is_err());
    assert!(a.dialog.is_none());
    // Scripts never get dialogs: the command answers for itself.
    assert!(a.execute("folder.new", json!({})).is_err());
    assert!(a.dialog.is_none());
}

#[test]
fn writing_brings_up_the_message_tab_and_errors_reach_the_status_bar() {
    let mut a = app();
    let id = first_message(&mut a);
    a.run("mail.reply", json!({"id": id})).unwrap();
    assert_eq!(a.ui.tab, "Message");
    assert!(crate::ribbon::tabs(&a).contains(&"Message"));
    a.run("compose.set", json!({"to": "not an address"})).unwrap();
    let e = a.run("compose.send", json!({})).unwrap_err();
    assert_eq!(a.status_msg.as_ref().map(|(m, _)| m.as_str()), Some(e.as_str()));
    a.run("compose.close", json!({})).unwrap();
    assert_eq!(a.ui.tab, "Home");
    assert!(a.run("ui.tab", json!({"tab": "Message"})).is_err());
    assert!(a.run("ui.tab", json!({"tab": "View"})).is_ok());
}

#[test]
fn window_commands_validate() {
    let mut a = app();
    assert_eq!(a.execute("ui.theme", json!({"value": "dark"})).unwrap()["value"], "dark");
    assert!(a.execute("ui.theme", json!({"value": "sepia"})).is_err());
    // (Only English here: the window's language is shared by the tests alongside; the pseudo
    // language has a test of its own, `tests/pseudo.rs`.)
    assert_eq!(a.execute("ui.language", json!({"language": "en"})).unwrap()["language"], "en");
    assert_eq!(a.ui.language, "en");
    assert!(a.execute("ui.language", json!({"language": "xx"})).is_err());
    assert!(a.execute("ui.language", json!({})).is_err());
    // Dutch has no catalogue: it can't be chosen, and Automatic in the Netherlands is English.
    assert!(a.execute("ui.language", json!({"language": "nl"})).is_err());
    a.services.system_locale = Some("nl-NL".into());
    assert_eq!(a.execute("ui.language", json!({"language": "auto"})).unwrap()["inForce"], "en");
    assert_eq!(a.session.interface_language, crate::i18n::Lang::English);
    assert_eq!(a.execute("ui.interfaceSize", json!({"value": 9000})).unwrap()["value"], 200);
    assert!(a.execute("ui.interfaceSize", json!({"value": "big"})).is_err());
    assert!(a.execute("ui.backstage", json!({"page": "nowhere"})).is_err());
    assert_eq!(a.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap()["page"], "options");
    assert!(a.execute("ui.dialog", json!({"name": "no such dialog"})).is_err());
    assert!(a.execute("ui.attach", json!({})).is_err());
    // Without a host that has sockets, Send/Receive is off and says why.
    assert!(!a.enabled("ui.sync"));
    assert!(a.execute("ui.sync", json!({})).unwrap_err().contains("mail servers"));
    // Every window command is known to `enabled`, and none shadows an engine command.
    for (id, ..) in crate::UI_COMMANDS {
        assert!(id.starts_with("ui.") && a.session.registry.get(id).is_none(), "{id}");
    }
}

#[test]
fn exports_answer_their_bytes_when_nobody_can_be_asked() {
    let mut a = app();
    let id = first_message(&mut a);
    let r = a.run("file.exportMessage", json!({"id": id})).unwrap();
    assert!(r["name"].as_str().unwrap().ends_with(".eml") && r["data"].is_string());
    // In a browser, the bytes go to the download hook instead.
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, usize)>::new()));
    let sink = got.clone();
    a.services.download = Some(Box::new(move |name, bytes| {
        sink.lock().unwrap().push((name.to_string(), bytes.len()));
        Ok(())
    }));
    let r = a.run("file.exportFolder", json!({})).unwrap();
    assert_eq!(r["downloaded"], true);
    let got = got.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert!(got[0].0 == "Inbox.mbox" && got[0].1 > 1000, "{got:?}");
}

#[test]
fn files_arriving_from_the_browser_go_where_they_belong() {
    let mut a = app();
    let inbox: crate::Inbox = Default::default();
    a.services.inbox = Some(inbox.clone());
    let contacts = a.session.mailbox.contacts.len();
    let card = b"BEGIN:VCARD\r\nVERSION:3.0\r\nN:Haddad;Noor;;;\r\nFN:Noor Haddad\r\nEMAIL:noor@example.org\r\nEND:VCARD\r\n";
    inbox.lock().unwrap().push(("noor.vcf".into(), card.to_vec()));
    crate::files::drain_inbox(&mut a);
    assert_eq!(a.session.mailbox.contacts.len(), contacts + 1);
    // While writing, an unknown file is an attachment.
    a.run("mail.new", json!({})).unwrap();
    inbox.lock().unwrap().push(("notes.bin".into(), vec![1, 2, 3]));
    crate::files::drain_inbox(&mut a);
    assert_eq!(a.session.compose.as_ref().unwrap().attachments.len(), 1);
    // Junk is refused with a message, not a crash.
    a.run("compose.close", json!({})).unwrap();
    inbox.lock().unwrap().push(("junk.bin".into(), vec![0, 159, 146, 150]));
    crate::files::drain_inbox(&mut a);
    assert!(a.status_msg.is_some());
}

#[test]
fn saved_settings_come_back_and_bad_ones_are_tamed() {
    let mut a = app();
    a.execute("ui.theme", json!({"value": "dark"})).unwrap();
    a.execute("view.conversations", json!({"value": true})).unwrap();
    a.execute("view.search", json!({"query": "secret"})).unwrap();
    let saved = serde_json::to_string(&a.prefs()).unwrap();
    // What was searched for is not written down.
    assert!(!saved.contains("secret"));
    let mut b = app();
    b.apply_prefs(serde_json::from_str(&saved).unwrap());
    assert_eq!(b.ui.theme, crate::theme::Appearance::Dark);
    assert!(b.session.view.conversations);
    // A hand-edited file with nonsense in it still loads.
    let odd: UiState = serde_json::from_value(
        json!({"theme": 7_i32.to_string(), "interfaceSize": "huge", "folderPaneWidth": -5.0, "listWidth": 1e30, "tab": 3, "nonsense": true}),
    )
    .unwrap_or_default();
    b.apply_prefs(odd);
    assert!((150.0..=500.0).contains(&b.ui.folder_pane_width) && (240.0..=900.0).contains(&b.ui.list_width));
    let nan: UiState = UiState { folder_pane_width: f32::NAN, list_width: f32::INFINITY, ..Default::default() };
    b.apply_prefs(nan);
    assert_eq!((b.ui.folder_pane_width, b.ui.list_width), (230.0, 380.0));
}

#[test]
fn the_control_channel_answers() {
    let (tx, rx) = std::sync::mpsc::channel();
    let a = app().with_control(rx);
    let ask = |method: &str, params: Value| {
        let (req, reply) = crate::ControlRequest::new(method, params);
        tx.send(req).unwrap();
        reply
    };
    let commands = ask("engine.commands", json!({}));
    let done = ask("engine.execute", json!({"command": "mail.markAllRead"}));
    let bad = ask("engine.execute", json!({}));
    let shorthand = ask("view.calendar", json!({}));
    let inspect = ask("ui.inspect", json!({}));
    let _ = frames(a, (1200.0, 800.0), 6);
    let commands = commands.try_recv().unwrap();
    assert!(commands["result"].as_array().unwrap().iter().any(|c| c["id"] == "ui.sync"));
    assert_eq!(done.try_recv().unwrap()["result"]["changed"], 4);
    assert_eq!(bad.try_recv().unwrap()["ok"], false);
    assert_eq!(shorthand.try_recv().unwrap()["result"]["module"], "calendar");
    let inspect = inspect.try_recv().unwrap();
    assert_eq!(inspect["result"]["view"]["module"], "calendar");
}

/// Never-crash, for the interface: every module, File page and dialog lays out, in both themes,
/// in a comfortable window and in one far too small.
#[test]
fn everything_lays_out_without_panicking() {
    let setups: Vec<(&str, Vec<(&str, Value)>)> = vec![
        ("mail", vec![("view.mail", json!({})), ("mail.next", json!({})), ("mail.next", json!({}))]),
        (
            "mail, bottom pane, conversations",
            vec![("view.readingPane", json!({"value": "bottom"})), ("view.conversations", json!({"value": true})), ("mail.next", json!({}))],
        ),
        (
            "mail, no panes, searching",
            vec![
                ("view.readingPane", json!({"value": "off"})),
                ("view.folderPane", json!({"value": false})),
                ("view.search", json!({"query": "has:attachment", "scope": "everywhere"})),
            ],
        ),
        ("html mail", vec![("view.search", json!({"query": "subject:arrivals"})), ("mail.next", json!({}))]),
        ("writing", vec![("mail.next", json!({})), ("mail.replyAll", json!({})), ("compose.attach", json!({"name": "a.txt", "data": "aGk="}))]),
        ("calendar day", vec![("view.day", json!({}))]),
        ("calendar work week", vec![("view.workWeek", json!({}))]),
        ("calendar week", vec![("view.week", json!({})), ("view.next", json!({}))]),
        ("calendar month", vec![("view.month", json!({}))]),
        ("calendar at the end of time", vec![("view.month", json!({})), ("view.goToDate", json!({"date": "9999-12-31"}))]),
        ("calendar at the start of time", vec![("view.week", json!({})), ("view.goToDate", json!({"date": "0001-01-01"}))]),
        ("people", vec![("view.people", json!({}))]),
        ("people, searching", vec![("view.people", json!({})), ("people.search", json!({"query": "zzzz"}))]),
        ("tasks", vec![("view.tasks", json!({})), ("view.taskFilter", json!({"value": "all"}))]),
        ("an empty mailbox", vec![("file.new", json!({})), ("view.calendar", json!({}))]),
        ("empty people and tasks", vec![("file.new", json!({})), ("view.people", json!({}))]),
        ("file: info", vec![("ui.backstage", json!({"value": true, "page": "info"}))]),
        ("file: open", vec![("ui.backstage", json!({"value": true, "page": "open"}))]),
        ("file: options", vec![("ui.backstage", json!({"value": true, "page": "options"}))]),
        ("file: about", vec![("ui.backstage", json!({"value": true, "page": "about"}))]),
    ];
    let dialogs = [
        "commands",
        "folder",
        "goToDate",
        "about",
        "contributors",
        "testers",
        "discardDraft",
        "event",
        "meeting",
        "contact",
        "task",
        "account",
        "categories",
        "rules",
    ];
    for theme in ["light", "dark"] {
        for size in [(1280.0, 800.0), (320.0, 200.0)] {
            for (what, steps) in &setups {
                let mut a = app();
                a.execute("ui.theme", json!({"value": theme})).unwrap();
                for (id, params) in steps {
                    a.execute(id, params.clone()).unwrap_or_else(|e| panic!("{what}: {id}: {e}"));
                }
                let _ = frames(a, size, 4);
            }
            for name in dialogs {
                let mut a = app();
                a.execute("ui.theme", json!({"value": theme})).unwrap();
                a.execute("ui.dialog", json!({"name": name})).unwrap_or_else(|e| panic!("dialog {name}: {e}"));
                let a = frames(a, size, 4);
                drop(a);
            }
            // The edit forms, on something selected in each module.
            for (select, list, key, name) in [
                ("calendar.select", "calendar.list", "events", "editEvent"),
                ("people.select", "people.list", "contacts", "editContact"),
                ("tasks.select", "tasks.list", "tasks", "editTask"),
            ] {
                let mut a = app();
                let id = a.execute(list, json!({"from": "2026-10-01", "to": "2026-10-31", "filter": "all"})).unwrap()[key][0]["id"].clone();
                a.execute(select, json!({"id": id})).unwrap();
                a.execute("ui.dialog", json!({"name": name})).unwrap_or_else(|e| panic!("dialog {name}: {e}"));
                let _ = frames(a, size, 4);
            }
        }
    }
}

/// About credits the contributors, with a link to them: in the Help tab's window and on the File
/// page. Nobody is named above the rest.
#[test]
fn about_credits_the_contributors() {
    use egui_kittest::kittest::Queryable as _;
    for (id, params) in [("ui.dialog", json!({"name": "about"})), ("ui.backstage", json!({"value": true, "page": "about"}))] {
        let mut a = app();
        a.execute(id, params).unwrap();
        let mut harness = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                a,
            );
        harness.run_steps(4);
        assert!(harness.query_by_label_contains("Made by its contributors").is_some(), "{id}");
        assert!(harness.query_by_label_contains("github.com/UCS-Care-Group/mailcraft/graphs/contributors").is_some(), "{id}");
        assert!(harness.query_by_label_contains("Initial contributor").is_none(), "{id}");
    }
}

/// About › Testers thanks the testers, and while nobody is credited says where to sign up.
#[test]
fn about_thanks_the_testers() {
    use egui_kittest::kittest::Queryable as _;
    for params in [json!({"name": "testers"}), json!({"name": "about", "tab": "testers"})] {
        let mut a = app();
        a.execute("ui.dialog", params.clone()).unwrap();
        assert_eq!(a.dialog.as_ref().map(|d| d.name()), Some("testers"), "{params}");
        let mut harness = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                a,
            );
        harness.run_steps(4);
        assert!(harness.query_by_label_contains("tell us what goes wrong. Thank you.").is_some(), "{params}");
        if crate::credits::TESTERS.is_empty() {
            assert!(harness.query_by_label_contains("No testers credited yet.").is_some(), "{params}");
            assert!(harness.query_by_label_contains("Sign up in #11").is_some(), "{params}");
        } else {
            assert!(harness.query_by_label_contains("No testers credited yet.").is_none(), "{params}");
        }
    }
}

/// A mailbox with a great deal in it still lays out (only what is on screen is drawn).
#[test]
fn a_large_mailbox_lays_out() {
    let mut a = app();
    let inbox = a.session.view.folder;
    for i in 0..20_000u32 {
        let m = mailcraft_model::Message {
            folder: inbox,
            subject: format!("Message {i}"),
            from: mailcraft_model::Address::new(format!("Sender {}", i % 97), format!("s{}@example.org", i % 97)),
            date: NOW - i64::from(i) * 977,
            received: NOW - i64::from(i) * 977,
            ..Default::default()
        };
        a.session.mailbox.add_message(m).unwrap();
        if i % 40 == 0 {
            a.session.mailbox.add_contact(mailcraft_model::Contact { first: format!("Person {i}"), ..Default::default() });
            a.session.mailbox.add_task(mailcraft_model::Task { title: format!("Task {i}"), ..Default::default() });
        }
    }
    a.session.replace_mailbox(a.session.mailbox.clone());
    let t0 = std::time::Instant::now();
    let a = frames(a, (1280.0, 800.0), 5);
    for id in ["view.people", "view.tasks", "view.calendar"] {
        let mut b = MailApp::new(Session::at(a.session.mailbox.clone(), NOW, 0), Services::default());
        b.autosave = false;
        b.execute(id, json!({})).unwrap();
        let _ = frames(b, (1280.0, 800.0), 5);
    }
    // Generous: this is a debug build on whatever machine runs the tests.
    assert!(t0.elapsed().as_secs() < 60, "{:?}", t0.elapsed());
}

/// The whole window in a harness that is kept, for tests that click and look at the result.
fn window(app: MailApp) -> egui_kittest::Harness<'static, MailApp> {
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
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

fn click_at(harness: &mut egui_kittest::Harness<'static, MailApp>, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(2);
}

/// Ctrl+Q is Mark as Read: egui's own Mod+Q quit is off, so it doesn't also close the window.
#[test]
fn ctrl_q_does_not_quit() {
    let harness = window(app());
    assert!(harness.ctx.options(|o| o.quit_shortcuts.is_empty()));
}

/// The strip down the left edge of a row switches read and unread, and leaves the selection alone.
#[test]
fn the_left_edge_of_a_row_switches_read_and_unread() {
    let mut harness = window(app());
    let selected = harness.state().session.sel.messages.clone();
    let row = harness.state_mut().session.rows().iter().find(|r| r.unread > 0 && !selected.contains(&r.id)).cloned().unwrap();
    let is_read = |h: &egui_kittest::Harness<'static, MailApp>| h.state().session.mailbox.message(row.id).unwrap().read;
    assert!(!is_read(&harness));
    let strip = harness.state().hit.rect("messageRead", row.id).unwrap();
    assert!(strip.width() < 20.0, "the switch is a strip, not the row");
    click_at(&mut harness, strip.center());
    assert!(is_read(&harness), "a click on the strip of an unread message marks it read");
    assert_eq!(harness.state().session.sel.messages, selected, "and doesn't select it");
    click_at(&mut harness, strip.center());
    assert!(!is_read(&harness), "the next click marks it unread again");
    // The rest of the row still selects.
    let body = harness.state().hit.rect("message", row.id).unwrap();
    click_at(&mut harness, body.center());
    assert_eq!(harness.state().session.sel.focus(), Some(row.id));
}

/// A new installation follows the system's light or dark appearance, live; a choice pins it.
#[test]
fn the_theme_follows_the_system_until_one_is_chosen() {
    assert_eq!(UiState::default().theme, crate::theme::Appearance::System);
    let mut harness = window(app());
    let dark = |h: &egui_kittest::Harness<'static, MailApp>| crate::theme::Tokens::get(&h.ctx).dark;
    for (system, expected) in [(egui::Theme::Dark, true), (egui::Theme::Light, false), (egui::Theme::Dark, true)] {
        harness.input_mut().system_theme = Some(system);
        harness.run_steps(2);
        assert_eq!(dark(&harness), expected, "system {system:?}");
    }
    harness.state_mut().execute("ui.theme", json!({"value": "light"})).unwrap();
    harness.run_steps(2);
    assert!(!dark(&harness), "a chosen theme wins over the system's");
    harness.state_mut().execute("ui.theme", json!({"value": "system"})).unwrap();
    harness.run_steps(2);
    assert!(dark(&harness));
}

/// #266: a window of its own reports a theme of its own (light, on Linux), and egui keeps the
/// last pass's, whichever window it was. `logic` runs between passes, so the theme must come from
/// the main window's input, not from whichever window drew last.
#[test]
fn another_windows_theme_does_not_change_the_system_theme() {
    let mut app = app();
    let ctx = egui::Context::default();
    let pass = |viewport: egui::ViewportId, theme: Option<egui::Theme>| {
        let viewports = std::iter::once((viewport, egui::ViewportInfo::default())).collect();
        let raw = egui::RawInput { viewport_id: viewport, viewports, system_theme: theme, ..Default::default() };
        ctx.run_ui(raw, |_| {}).drop_without_applying_deltas();
    };
    pass(egui::ViewportId::ROOT, Some(egui::Theme::Dark));
    pass(egui::ViewportId::from_hash_of("appointment"), Some(egui::Theme::Light));
    app.logic(&ctx);
    assert!(crate::theme::Tokens::get(&ctx).dark, "the main window is dark, so the program is");
}

/// An account that signs in on its provider's page: Send/Receive has the host sign in first and
/// takes the token along (a password is never asked for), reuses it while it is good, renews it
/// after a refusal, and the wait can be given up.
#[test]
fn send_receive_signs_in_first_for_an_account_without_a_password() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};

    use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens};

    type Answer = Result<Tokens, String>;
    // What each Send/Receive was handed (token, password), and each sign-in (address, refresh token).
    let synced: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let asked: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let refuse = Arc::new(AtomicBool::new(false));
    // An error the next Send/Receive ends with, once.
    let error: Arc<Mutex<Option<String>>> = Default::default();
    // What the next sign-in answers; `None` leaves the owner in the browser.
    let answer: Arc<Mutex<Option<Answer>>> = Default::default();
    let waiting: Arc<Mutex<Vec<(mpsc::Sender<Answer>, Arc<AtomicBool>)>>> = Default::default();
    let good =
        |access: &str| Ok(Tokens { access: access.into(), refresh: "ref-1".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() });

    let mut a = app();
    let (synced2, refuse2, error2) = (synced.clone(), refuse.clone(), error.clone());
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        synced2.lock().unwrap().push((req.access_token.clone(), req.password.clone()));
        let mut errors = if refuse2.load(Ordering::Relaxed) { vec!["The server refused the sign-in".to_string()] } else { Vec::new() };
        errors.extend(error2.lock().unwrap().take());
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, errors, ..Default::default() }).unwrap();
        rx
    }));
    let (asked2, answer2, waiting2) = (asked.clone(), answer.clone(), waiting.clone());
    a.services.sign_in = Some(Box::new(move |req: crate::SignInRequest| {
        asked2.lock().unwrap().push((req.email.clone(), req.refresh.clone()));
        let (tx, rx) = mpsc::channel();
        match answer2.lock().unwrap().take() {
            Some(answer) => tx.send(answer).unwrap(),
            None => waiting2.lock().unwrap().push((tx, req.cancel.clone())),
        }
        rx
    }));
    let added = a.execute(
        "account.add",
        json!({"email": "avery@mail.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "imap.mail.example"}}),
    );
    let id = added.unwrap()["id"].as_u64().unwrap();

    // The first time: sign in, then the Send/Receive that asked for it goes ahead by itself.
    *answer.lock().unwrap() = Some(good("tok-1"));
    assert_eq!(a.execute("ui.sync", json!({"account": id})).unwrap()["pending"], "signIn");
    assert!(a.dialog.is_none(), "no password is asked for");
    assert!(a.signing_in() && !a.enabled("ui.sync") && a.enabled("ui.cancelSignIn"));
    a.poll_sign_in();
    assert!(!a.signing_in() && a.syncing());
    a.poll_sync();
    assert_eq!(*asked.lock().unwrap(), [("avery@mail.example".to_string(), String::new())]);
    assert_eq!(*synced.lock().unwrap(), [("tok-1".to_string(), String::new())]);

    // While the token is good nobody is asked again.
    assert_eq!(a.execute("ui.sync", json!({"account": id})).unwrap()["started"], true);
    a.poll_sync();
    assert_eq!((asked.lock().unwrap().len(), synced.lock().unwrap().len()), (1, 2));

    // The server refuses the token: the next run renews it with the refresh token.
    refuse.store(true, Ordering::Relaxed);
    a.execute("ui.sync", json!({"account": id})).unwrap();
    a.poll_sync();
    refuse.store(false, Ordering::Relaxed);
    *answer.lock().unwrap() = Some(good("tok-2"));
    assert_eq!(a.execute("ui.sync", json!({"account": id})).unwrap()["pending"], "signIn");
    a.poll_sign_in();
    a.poll_sync();
    assert_eq!(asked.lock().unwrap().last().unwrap().1, "ref-1");
    assert_eq!(synced.lock().unwrap().last().unwrap().0, "tok-2");

    // A token that runs out before a run could be over is renewed first, not taken along.
    a.tokens.get_mut(&id).unwrap().expires_at = mailcraft_time::now() + 300;
    *answer.lock().unwrap() = Some(good("tok-3"));
    assert_eq!(a.execute("ui.sync", json!({"account": id})).unwrap()["pending"], "signIn");
    a.poll_sign_in();
    a.poll_sync();
    assert_eq!(synced.lock().unwrap().last().unwrap().0, "tok-3");

    // Microsoft ends the session when the token runs out during the run: the next run renews it.
    *error.lock().unwrap() = Some("Couldn't open \"Inbox\": the server ended the session: Session invalidated - AccessTokenExpired".into());
    a.execute("ui.sync", json!({"account": id})).unwrap();
    a.poll_sync();
    *answer.lock().unwrap() = Some(good("tok-4"));
    assert_eq!(a.execute("ui.sync", json!({"account": id})).unwrap()["pending"], "signIn");
    a.poll_sign_in();
    a.poll_sync();
    assert_eq!(synced.lock().unwrap().last().unwrap().0, "tok-4");

    // A sign-in that fails says so. What was kept to renew with stays (the trouble may have been
    // the network); the host decides when the provider has refused it for good.
    refuse.store(true, Ordering::Relaxed);
    a.execute("ui.sync", json!({"account": id})).unwrap();
    a.poll_sync();
    refuse.store(false, Ordering::Relaxed);
    *answer.lock().unwrap() = Some(Err("Microsoft didn't sign you in: the request was declined".into()));
    a.execute("ui.sync", json!({"account": id})).unwrap();
    a.status_msg = None;
    a.poll_sign_in();
    assert!(!a.signing_in() && !a.syncing());
    assert!(a.status_msg.as_ref().unwrap().0.contains("declined"));

    // Still in the browser: the wait can be given up, and the host is told to stop.
    let runs = synced.lock().unwrap().len();
    assert_eq!(a.execute("ui.sync", json!({"account": id})).unwrap()["pending"], "signIn");
    assert_eq!(asked.lock().unwrap().last().unwrap().1, "ref-1", "still there to renew with");
    a.poll_sign_in();
    assert!(a.signing_in());
    assert!(a.execute("ui.sync", json!({"account": id})).is_err(), "one sign-in at a time");
    a.execute("ui.cancelSignIn", json!({})).unwrap();
    assert!(!a.signing_in() && waiting.lock().unwrap()[0].1.load(Ordering::Relaxed));
    assert!(a.execute("ui.cancelSignIn", json!({})).is_err());
    assert_eq!(synced.lock().unwrap().len(), runs);

    // Without a host that can sign in (the browser build), it says so instead of asking for a password.
    a.services.sign_in = None;
    assert!(a.execute("ui.sync", json!({"account": id})).unwrap_err().contains("sign in"));
    assert!(a.dialog.is_none());
}

/// What the system is asked to remember: a password once a Send/Receive with it went through, a
/// sign-in once it succeeded. Both are used at the next start, dropped when a server refuses
/// them, when remembering is switched off, and with the account they belong to.
#[test]
fn the_system_remembers_passwords_and_sign_ins_only_when_wanted() {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};

    use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens};

    #[derive(Clone, Default)]
    struct Store(Arc<Mutex<HashMap<String, String>>>);
    impl crate::SecretStore for Store {
        fn get(&self, key: &str) -> Option<String> {
            self.0.lock().unwrap().get(key).cloned()
        }
        fn set(&self, key: &str, secret: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(key.to_string(), secret.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) {
            self.0.lock().unwrap().remove(key);
        }
    }
    let store = Store::default();
    let kept = |key: &str| store.0.lock().unwrap().get(key).cloned();
    let refuse = Arc::new(AtomicBool::new(false));
    // What each Send/Receive carried (password, token) and each sign-in was handed to renew with.
    let synced: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let renewals: Arc<Mutex<Vec<String>>> = Default::default();
    // An app as it starts: the same mailbox and the same system store, nothing in memory.
    let start = |mailbox: mailcraft_model::Mailbox| {
        let mut a = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
        a.autosave = false;
        let (synced, refuse) = (synced.clone(), refuse.clone());
        a.services.sync = Some(Box::new(move |req: SyncRequest| {
            synced.lock().unwrap().push((req.password.clone(), req.access_token.clone()));
            let errors = if refuse.load(Ordering::Relaxed) { vec!["The server refused the sign-in".to_string()] } else { Vec::new() };
            let (tx, rx) = mpsc::channel();
            tx.send(SyncResult { account: req.account.id, errors, ..Default::default() }).unwrap();
            rx
        }));
        let renewals = renewals.clone();
        a.services.sign_in = Some(Box::new(move |req: crate::SignInRequest| {
            renewals.lock().unwrap().push(req.refresh.clone());
            let (tx, rx) = mpsc::channel();
            let tokens = Tokens { access: "tok".into(), refresh: "ref-kept".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() };
            tx.send(Ok(tokens)).unwrap();
            rx
        }));
        a.services.secrets = Some(Box::new(store.clone()));
        a
    };

    let mut a = start(mailcraft_engine::sample::mailbox(NOW, 0));
    let plain =
        a.execute("account.add", json!({"email": "Avery@Mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let signed = a.execute("account.add", json!({"email": "quinn@mail.example", "auth": "oauth2", "incoming": {"host": "imap.mail.example"}}));
    let signed = signed.unwrap()["id"].as_u64().unwrap();

    // A password is remembered once it has worked, not when it is typed.
    a.execute("ui.sync", json!({"account": plain, "password": "pw-one"})).unwrap();
    assert_eq!(kept("password:avery@mail.example|imap=imap.mail.example:0|smtp=none"), None);
    a.poll_sync();
    assert_eq!(kept("password:avery@mail.example|imap=imap.mail.example:0|smtp=none").as_deref(), Some("pw-one"));
    // A sign-in is remembered as its refresh token; the access token is not kept anywhere.
    a.execute("ui.sync", json!({"account": signed})).unwrap();
    a.poll_sign_in();
    a.poll_sync();
    assert_eq!(kept("refresh:quinn@mail.example|imap=imap.mail.example:0|smtp=none").as_deref(), Some("ref-kept"));
    assert!(!store.0.lock().unwrap().values().any(|v| v == "tok"));
    // Neither reaches the settings or the mailbox.
    let written = format!("{}{}", serde_json::to_string(&a.prefs()).unwrap(), serde_json::to_string(&a.session.mailbox).unwrap());
    assert!(!written.contains("pw-one") && !written.contains("ref-kept") && !written.contains("\"tok\""));

    // The next start: nobody is asked for the password, and the sign-in renews without a browser.
    let mut b = start(a.session.mailbox.clone());
    assert_eq!(b.execute("ui.sync", json!({"account": plain})).unwrap()["started"], true);
    assert!(b.dialog.is_none());
    b.poll_sync();
    assert_eq!(synced.lock().unwrap().last().unwrap().0, "pw-one");
    b.execute("ui.sync", json!({"account": signed})).unwrap();
    assert_eq!(renewals.lock().unwrap().last().unwrap(), "ref-kept");
    b.poll_sign_in();
    b.poll_sync();

    // A server that refuses the password: it is forgotten, and the next run asks.
    refuse.store(true, Ordering::Relaxed);
    b.execute("ui.sync", json!({"account": plain})).unwrap();
    b.poll_sync();
    refuse.store(false, Ordering::Relaxed);
    assert_eq!(kept("password:avery@mail.example|imap=imap.mail.example:0|smtp=none"), None);
    assert_eq!(b.execute("ui.sync", json!({"account": plain})).unwrap()["pending"], "password");
    b.dialog = None;

    // Switched off: what was kept goes, and nothing new is kept.
    b.execute("ui.sync", json!({"account": plain, "password": "pw-two"})).unwrap();
    b.poll_sync();
    assert_eq!(kept("password:avery@mail.example|imap=imap.mail.example:0|smtp=none").as_deref(), Some("pw-two"));
    assert_eq!(b.execute("ui.rememberSignIn", json!({"value": false})).unwrap()["value"], false);
    assert!(store.0.lock().unwrap().is_empty());
    b.execute("ui.sync", json!({"account": plain})).unwrap();
    b.poll_sync();
    assert!(store.0.lock().unwrap().is_empty());

    // An account that is removed takes what was remembered for it along.
    b.execute("ui.rememberSignIn", json!({"value": true})).unwrap();
    b.execute("ui.sync", json!({"account": plain})).unwrap();
    b.poll_sync();
    assert!(kept("password:avery@mail.example|imap=imap.mail.example:0|smtp=none").is_some());
    b.execute("account.remove", json!({"id": plain})).unwrap();
    assert_eq!(kept("password:avery@mail.example|imap=imap.mail.example:0|smtp=none"), None);
}

/// (#103) An account that exists is signed in again, and on the provider's page the owner picks
/// somebody else: those tokens are neither used nor remembered, mail's or Graph's, however they
/// come in, and the owner is told who to sign in as. The account's own address in any case, its
/// user name, and a sign-in that names nobody are the account's, as before.
#[test]
fn a_sign_in_as_somebody_else_is_never_taken_for_an_account_that_exists() {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, mpsc};

    use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens};

    #[derive(Clone, Default)]
    struct Store(Arc<Mutex<HashMap<String, String>>>);
    impl crate::SecretStore for Store {
        fn get(&self, key: &str) -> Option<String> {
            self.0.lock().unwrap().get(key).cloned()
        }
        fn set(&self, key: &str, secret: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(key.to_string(), secret.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) {
            self.0.lock().unwrap().remove(key);
        }
    }
    let store = Store::default();
    let who: Arc<Mutex<String>> = Arc::new(Mutex::new("admin@mail.example".into()));
    let synced: Arc<Mutex<Vec<String>>> = Default::default();
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    (a.autosave, a.auto_sync) = (false, false);
    let log = synced.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        log.lock().unwrap().push(req.access_token.clone());
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    let signs = who.clone();
    a.services.sign_in = Some(Box::new(move |_req: crate::SignInRequest| {
        let (tx, rx) = mpsc::channel();
        let email = signs.lock().unwrap().clone();
        let tokens =
            Tokens { access: "tok-other".into(), refresh: "ref-other".into(), expires_at: mailcraft_time::now() + 3600, email, ..Default::default() };
        tx.send(Ok(tokens)).unwrap();
        rx
    }));
    a.services.secrets = Some(Box::new(store.clone()));
    let id = a.execute(
        "account.add",
        json!({"email": "quinn@mail.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}}),
    );
    let id = id.unwrap()["id"].as_u64().unwrap();
    let said = "You signed in as admin@mail.example, but this account is quinn@mail.example. Sign in again as quinn@mail.example.";
    let nothing_kept = |a: &MailApp| !a.tokens.contains_key(&id) && !a.graph_tokens.contains_key(&id) && store.0.lock().unwrap().is_empty();

    // Send/Receive signs in for mail: refused, said, and no run with the other one's token.
    a.execute("ui.sync", json!({"account": id})).unwrap();
    a.poll_sign_in();
    assert!(nothing_kept(&a) && synced.lock().unwrap().is_empty() && !a.syncing());
    assert_eq!(a.status_msg.as_ref().map(|(m, _)| m.as_str()), Some(said));
    // "Connect calendar and contacts" signs in for Graph: the same.
    a.status_msg = None;
    a.execute("ui.connectMicrosoft", json!({"account": id})).unwrap();
    a.poll_sign_in();
    assert!(nothing_kept(&a));
    assert_eq!(a.status_msg.as_ref().map(|(m, _)| m.as_str()), Some(said));
    // Tokens a background job renewed for it are looked at the same way.
    a.keep_graph_tokens(
        id,
        Tokens { access: "tok-other".into(), refresh: "ref-other".into(), email: "admin@mail.example".into(), ..Default::default() },
    );
    assert!(nothing_kept(&a));

    // The account's own address, whatever its case: used and remembered as before.
    *who.lock().unwrap() = "Quinn@Mail.Example".into();
    a.execute("ui.sync", json!({"account": id})).unwrap();
    a.poll_sign_in();
    assert_eq!(a.tokens.get(&id).map(|t| t.access.as_str()), Some("tok-other"));
    assert_eq!(
        store.0.lock().unwrap().get("refresh:quinn@mail.example|imap=outlook.office365.com:0|smtp=none").map(String::as_str),
        Some("ref-other")
    );
    assert_eq!(synced.lock().unwrap().as_slice(), ["tok-other"]);
    a.poll_sync();
    // A sign-in that names nobody: the account's, as it always was.
    a.tokens.clear();
    store.0.lock().unwrap().clear();
    who.lock().unwrap().clear();
    a.execute("ui.sync", json!({"account": id})).unwrap();
    a.poll_sign_in();
    assert!(a.tokens.contains_key(&id));
    a.poll_sync();
    // An address that is another name for the one that signs in: its user name is who signs in.
    a.tokens.clear();
    a.execute("account.edit", json!({"id": id, "username": "q.admin@mail.example"})).unwrap();
    *who.lock().unwrap() = "Q.Admin@mail.example".into();
    a.execute("ui.sync", json!({"account": id})).unwrap();
    a.poll_sign_in();
    assert!(a.tokens.contains_key(&id));
}

/// A system whose credential store can't be used (on Linux, no Secret Service running, or one
/// that stays locked): the session goes on with what was typed, the next start asks again as on
/// a system without a store, and the owner is told why once a session, not at every try.
#[test]
fn a_store_that_cant_be_used_is_said_once_and_the_next_start_asks() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};

    use mailcraft_model::sync::{SyncRequest, SyncResult};

    use crate::StoreTrouble;

    /// Every question fails the way the store says; how often it was asked is counted.
    #[derive(Clone, Default)]
    struct Broken(Arc<Mutex<Option<StoreTrouble>>>, Arc<AtomicUsize>);
    impl crate::SecretStore for Broken {
        fn get(&self, _: &str) -> Option<String> {
            self.1.fetch_add(1, Ordering::Relaxed);
            *self.0.lock().unwrap() = Some(StoreTrouble::Unavailable);
            None
        }
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            self.1.fetch_add(1, Ordering::Relaxed);
            *self.0.lock().unwrap() = Some(StoreTrouble::Unavailable);
            Err("the system's credential store refused: no Secret Service".into())
        }
        fn delete(&self, _: &str) {}
        fn take_trouble(&self) -> Option<StoreTrouble> {
            self.0.lock().unwrap().take()
        }
    }
    let store = Broken::default();
    let synced: Arc<Mutex<Vec<String>>> = Default::default();
    let start = |mailbox: mailcraft_model::Mailbox| {
        let mut a = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
        a.autosave = false;
        let synced = synced.clone();
        a.services.sync = Some(Box::new(move |req: SyncRequest| {
            synced.lock().unwrap().push(req.password.clone());
            let (tx, rx) = mpsc::channel();
            tx.send(SyncResult { account: req.account.id, ..Default::default() }).unwrap();
            rx
        }));
        a.services.secrets = Some(Box::new(store.clone()));
        a
    };

    let mut a = start(mailcraft_engine::sample::mailbox(NOW, 0));
    let plain =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    // Nothing has been asked of the store yet: nothing to say.
    a.check_secret_store();
    assert_eq!(a.secrets_trouble, None);

    // The password works; keeping it fails, and the owner hears why.
    a.execute("ui.sync", json!({"account": plain, "password": "pw-one"})).unwrap();
    a.poll_sync();
    assert!(store.1.load(Ordering::Relaxed) > 0, "the store was asked");
    a.check_secret_store();
    assert_eq!(a.secrets_trouble, Some(StoreTrouble::Unavailable));
    let said = a.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert_eq!(said, StoreTrouble::Unavailable.notice());
    assert!(said.contains("asks again each time it starts"), "{said}");
    assert!(!said.contains("pw-one"));

    // The session carries on with the password typed, and the store failing again says nothing more.
    a.status_msg = None;
    a.execute("ui.sync", json!({"account": plain})).unwrap();
    assert!(a.dialog.is_none());
    a.poll_sync();
    a.check_secret_store();
    assert_eq!(synced.lock().unwrap().last().map(String::as_str), Some("pw-one"));
    assert_ne!(a.status_msg.as_ref().map(|(text, _)| text.clone()), Some(said), "told once a session");

    // The next start asks for the password, as on a system without a store, and tells again.
    let mut b = start(a.session.mailbox.clone());
    assert_eq!(b.execute("ui.sync", json!({"account": plain})).unwrap()["pending"], "password");
    b.check_secret_store();
    assert_eq!(b.secrets_trouble, Some(StoreTrouble::Unavailable));

    // A locked store is told as such.
    assert_ne!(StoreTrouble::Locked.notice(), StoreTrouble::Unavailable.notice());
    assert!(StoreTrouble::Locked.notice().contains("locked"));
}

/// Send/Receive starts by itself (when the app starts, then every few minutes) for the accounts
/// it need not ask anybody about, and never puts a password prompt or a sign-in page up unasked;
/// asked for, it goes round every account.
#[test]
fn send_receive_runs_by_itself_without_asking_anybody() {
    use std::sync::{Arc, Mutex, mpsc};

    use mailcraft_model::sync::{SyncRequest, SyncResult, Tokens};

    let synced: Arc<Mutex<Vec<u64>>> = Default::default();
    let sign_ins: Arc<Mutex<Vec<(u64, bool)>>> = Default::default();
    let mut a = app();
    let synced2 = synced.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        synced2.lock().unwrap().push(req.account.id);
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    let sign_ins2 = sign_ins.clone();
    a.services.sign_in = Some(Box::new(move |req: crate::SignInRequest| {
        sign_ins2.lock().unwrap().push((req.account, req.quiet));
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Tokens { access: "tok".into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() })).unwrap();
        rx
    }));
    let add = |a: &mut MailApp, email: &str, auth: &str| {
        a.execute("account.add", json!({"email": email, "auth": auth, "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap()
    };
    let (known, unknown, signed) =
        (add(&mut a, "one@mail.example", "password"), add(&mut a, "two@mail.example", "password"), add(&mut a, "three@mail.example", "oauth2"));
    // Everything a round does, until it has nothing left to do.
    let settle = |a: &mut MailApp| {
        for _ in 0..20 {
            a.poll_sign_in();
            a.poll_sync();
            a.auto_sync();
        }
    };

    // Nothing is at hand yet: the round that starts by itself asks nobody and syncs nothing.
    settle(&mut a);
    assert!(a.dialog.is_none() && synced.lock().unwrap().is_empty() && sign_ins.lock().unwrap().is_empty());

    // Asked for, every account gets its turn; the one without a password stops for it.
    a.passwords.insert(known, "pw".into());
    assert_eq!(a.execute("ui.sync", json!({})).unwrap()["account"], known);
    settle(&mut a);
    assert_eq!(*synced.lock().unwrap(), [known]);
    assert!(matches!(a.dialog, Some(crate::dialogs::Dialog::Password { account, .. }) if account == unknown));
    a.dialog = None;
    settle(&mut a);
    // The account that signs in on its provider's page was asked for too, so a page may open.
    assert_eq!(*sign_ins.lock().unwrap(), [(signed, false)]);
    assert_eq!(*synced.lock().unwrap(), [known, signed]);

    // The next round by itself: the two it can do quietly, and still no prompt for the third.
    synced.lock().unwrap().clear();
    a.last_auto_sync = 0.0;
    settle(&mut a);
    assert_eq!(*synced.lock().unwrap(), [known, signed]);
    assert!(a.dialog.is_none());
    // A sign-in that has run out is renewed quietly: no page is put in front of the owner.
    a.tokens.get_mut(&signed).unwrap().expires_at = 0;
    a.last_auto_sync = 0.0;
    settle(&mut a);
    assert_eq!(sign_ins.lock().unwrap().last(), Some(&(signed, true)));

    // Not before it is due, not when switched off, and not when the owner wants to ask.
    synced.lock().unwrap().clear();
    settle(&mut a);
    a.last_auto_sync = 0.0;
    a.execute("file.options", json!({"syncMinutes": 0})).unwrap();
    settle(&mut a);
    a.execute("file.options", json!({"syncMinutes": 5})).unwrap();
    a.auto_sync = false;
    settle(&mut a);
    assert!(synced.lock().unwrap().is_empty());
}

/// A change made here (a read mark, a message to send) goes to the server without waiting for
/// the next round; one the server doesn't take is left for a while instead of starting a run
/// every few seconds.
#[test]
fn a_change_made_here_goes_out_soon_and_a_refused_one_backs_off() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};

    use mailcraft_model::sync::{FlagsPushed, FolderDelta, RemoteOp, SyncRequest, SyncResult};
    use mailcraft_model::{Address, FolderKind, Message};

    let runs: Arc<Mutex<Vec<(usize, usize)>>> = Default::default();
    let accept = Arc::new(AtomicBool::new(false));
    let mut a = app();
    let (runs2, accept2) = (runs.clone(), accept.clone());
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        runs2.lock().unwrap().push((req.ops.len(), req.outbox.len()));
        let mut result = SyncResult { account: req.account.id, ..Default::default() };
        if accept2.load(Ordering::Relaxed) {
            for op in &req.ops {
                if let RemoteOp::Flags { folder, uid, read, flagged, answered } = op {
                    result.flagged.push(FlagsPushed { folder: folder.clone(), uid: *uid, read: *read, flagged: *flagged, answered: *answered });
                }
            }
            result.sent = req.outbox.iter().map(|m| m.id).collect();
        }
        let (tx, rx) = mpsc::channel();
        tx.send(result).unwrap();
        rx
    }));
    let id =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    a.passwords.insert(id, "pw".into());
    // Two messages that came from the server.
    let inbox = a.session.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap();
    let mail =
        |uid: u32| Message { uid, subject: format!("Message {uid}"), from: Address::new("", "x@example.org"), date: NOW, ..Default::default() };
    let came = SyncResult {
        account: id,
        folders: vec![FolderDelta {
            id: Some(inbox),
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: 2,
            new: vec![mail(1), mail(2)],
            ..Default::default()
        }],
        ..Default::default()
    };
    mailcraft_engine::cmd::account::apply_sync(&mut a.session, came).unwrap();
    let ids: Vec<u64> = a.session.mailbox.messages_in(inbox).map(|m| m.id).collect();
    // A look that is due now, with the round that starts by itself not due.
    let look = |a: &mut MailApp| {
        a.last_auto_sync = crate::now_ms();
        a.push_checked = 0.0;
        for _ in 0..6 {
            a.poll_sync();
            a.auto_sync();
        }
    };

    // Nothing waits: nothing runs.
    look(&mut a);
    assert!(runs.lock().unwrap().is_empty());

    // A read mark goes out at the next look. The server doesn't take it: no run every look.
    a.execute("mail.markRead", json!({"ids": [ids[0]]})).unwrap();
    look(&mut a);
    assert_eq!(*runs.lock().unwrap(), [(1, 0)]);
    look(&mut a);
    look(&mut a);
    assert_eq!(runs.lock().unwrap().len(), 1, "a change that didn't go through waits");
    // More to tell is a reason to try again, and so is time.
    a.execute("mail.markRead", json!({"ids": [ids[1]]})).unwrap();
    look(&mut a);
    assert_eq!(runs.lock().unwrap().last(), Some(&(2, 0)));
    a.push_seen.get_mut(&id).unwrap().1 = 0.0;
    look(&mut a);
    assert_eq!(runs.lock().unwrap().len(), 3);

    // The server takes them: nothing is left, so nothing more runs.
    accept.store(true, Ordering::Relaxed);
    a.push_seen.get_mut(&id).unwrap().1 = 0.0;
    look(&mut a);
    assert_eq!(mailcraft_engine::cmd::account::unpushed(&a.session, id), 0);
    let done = runs.lock().unwrap().len();
    look(&mut a);
    assert_eq!(runs.lock().unwrap().len(), done);

    // A message put in the Outbox is sent at the next look, not at the next round.
    a.execute("mail.new", json!({"to": "maya@lumen.example", "subject": "Soon", "account": id})).unwrap();
    assert_eq!(a.execute("compose.send", json!({})).unwrap()["queued"], true);
    look(&mut a);
    assert_eq!(runs.lock().unwrap().last(), Some(&(0, 1)));
    assert_eq!(a.session.mailbox.count(a.session.mailbox.folder_of_kind(id, FolderKind::Outbox).unwrap()), 0);

    // Not without a password at hand, and not when the owner wants to ask for every run.
    a.execute("mail.markUnread", json!({"ids": [ids[0]]})).unwrap();
    a.passwords.clear();
    let before = runs.lock().unwrap().len();
    look(&mut a);
    a.passwords.insert(id, "pw".into());
    a.auto_sync = false;
    look(&mut a);
    assert_eq!(runs.lock().unwrap().len(), before);
    assert!(a.dialog.is_none());
}

/// Messages can be dragged out of the list and let go over a folder, which files them there; a
/// drag that ends anywhere else moves nothing, and a click still just selects.
#[test]
fn a_message_dragged_onto_a_folder_is_filed_there() {
    use mailcraft_model::FolderKind;

    let mut harness = window(app());
    let account = harness.state().session.mailbox.accounts.first().unwrap().id;
    let archive = harness.state().session.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    let row = harness.state_mut().session.rows().first().cloned().unwrap();
    let from = harness.state().session.mailbox.message(row.id).unwrap().folder;
    let start = harness.state().hit.rect("message", row.id).unwrap().center();
    let target = harness.state().hit.rect("folder", archive).unwrap().center();
    let drag = |harness: &mut egui_kittest::Harness<'static, MailApp>, to: egui::Pos2| {
        harness.event(egui::Event::PointerMoved(start));
        harness.step();
        harness.event(egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        harness.step();
        // In steps, as a hand would: far enough to be a drag, then on to where it ends.
        for k in 1..=8 {
            harness.event(egui::Event::PointerMoved(start + (to - start) * (k as f32 / 8.0)));
            harness.step();
        }
        harness.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        harness.run_steps(3);
    };

    // Let go over nothing in particular (the reading pane): nothing moves.
    drag(&mut harness, egui::pos2(1100.0, 600.0));
    assert_eq!(harness.state().session.mailbox.message(row.id).unwrap().folder, from);

    // Let go over Archive: the message is there, as one undoable step.
    drag(&mut harness, target);
    assert_eq!(harness.state().session.mailbox.message(row.id).unwrap().folder, archive);
    assert!(harness.state_mut().session.undo());
    assert_eq!(harness.state().session.mailbox.message(row.id).unwrap().folder, from);

    // A selection goes together when one of its rows is dragged.
    harness.run_steps(2);
    let rows: Vec<u64> = harness.state_mut().session.rows().iter().take(2).map(|r| r.id).collect();
    harness.state_mut().execute("mail.select", json!({"id": rows[0]})).unwrap();
    harness.state_mut().execute("mail.select", json!({"id": rows[1], "extend": true})).unwrap();
    harness.run_steps(2);
    assert_eq!(rows[0], row.id);
    drag(&mut harness, target);
    for id in &rows {
        assert_eq!(harness.state().session.mailbox.message(*id).unwrap().folder, archive, "{id}");
    }
}

/// A mailbox with a Microsoft account and a shared mailbox under it, each with mail in its Inbox,
/// and the shared mailbox's Inbox in view; the owner's Inbox and the shared mailbox's.
fn shared_and_own() -> (MailApp, mailcraft_model::Id, mailcraft_model::Id) {
    use mailcraft_engine::cmd::graph_mail::apply_graph_mail;
    use mailcraft_model::FolderKind;
    use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, GraphMessage, MessagePage, ServerMessage};

    let mut s = Session::empty();
    let owner = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"address": "support@example.org"})).unwrap()["id"].as_u64().unwrap();
    for (account, email) in [(owner, "owner@example.org"), (shared, "support@example.org")] {
        let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
        let folders = vec![folder("in", "Inbox", FolderKind::Inbox), folder("de", "Deleted Items", FolderKind::Deleted)];
        let listing = FolderListing { folders, complete: true };
        apply_graph_mail(&mut s, GraphMailResult { account, email: email.into(), listing: Some(listing), ..Default::default() }).unwrap();
        let messages = ["AAkA1", "AAkA2"]
            .iter()
            .map(|id| GraphMessage {
                server: ServerMessage { id: (*id).into(), folder: "in".into(), ..Default::default() },
                subject: format!("{email}: {id}"),
                ..Default::default()
            })
            .collect();
        let page = MessagePage { folder: "in".into(), messages, caught_up: true, ..Default::default() };
        apply_graph_mail(&mut s, GraphMailResult { account, email: email.into(), pages: vec![page], whole: true, ..Default::default() }).unwrap();
    }
    let (mine, theirs) = (s.mailbox.folder_of_kind(owner, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap());
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    app.auto_sync = false;
    app.execute("folder.select", json!({"id": theirs})).unwrap();
    (app, mine, theirs)
}

/// #152: a message dragged from a shared mailbox to the owner's own is refused at once, and the
/// reason comes in a message box with OK (and stays in the status bar). The same move through
/// the control channel, as an agent or a script asks it, opens nothing: it answers the error.
#[test]
fn a_refused_drag_says_why_in_a_message_box_and_the_control_channel_never_does() {
    let (a, mine, theirs) = shared_and_own();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut harness = window(a.with_control(rx));
    let row = harness.state_mut().session.rows().first().cloned().unwrap();
    assert_eq!(harness.state().session.mailbox.message(row.id).map(|m| m.folder), Some(theirs));
    let start = harness.state().hit.rect("message", row.id).unwrap().center();
    let target = harness.state().hit.rect("folder", mine).expect("the owner's Inbox in the folder pane").center();
    harness.event(egui::Event::PointerMoved(start));
    harness.step();
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    harness.event(button(start, true));
    harness.step();
    for k in 1..=8 {
        harness.event(egui::Event::PointerMoved(start + (target - start) * (k as f32 / 8.0)));
        harness.step();
    }
    harness.event(button(target, false));
    harness.run_steps(3);
    assert_eq!(harness.state().session.mailbox.message(row.id).map(|m| m.folder), Some(theirs), "nothing moved");
    let Some(crate::dialogs::Dialog::Refused { text }) = &harness.state().dialog else {
        panic!("a message box: {:?}", harness.state().dialog.as_ref().map(|d| d.name()))
    };
    assert!(text.contains("isn't downloaded yet"), "{text}");
    assert!(harness.state().status_msg.as_ref().is_some_and(|(said, _)| said.contains("isn't downloaded yet")));
    // OK closes it.
    let ok = harness.state().hit.rect("refusedOk", 0).expect("OK");
    harness.event(egui::Event::PointerMoved(ok.center()));
    harness.step();
    for pressed in [true, false] {
        harness.event(button(ok.center(), pressed));
        harness.step();
    }
    harness.run_steps(2);
    assert!(harness.state().dialog.is_none());

    // Through the control channel: the error, and no box.
    harness.state_mut().status_msg = None;
    let (req, reply) = crate::ControlRequest::new("engine.execute", json!({"command": "mail.move", "params": {"ids": [row.id], "folder": mine}}));
    tx.send(req).unwrap();
    harness.run_steps(3);
    let reply = reply.try_recv().unwrap();
    assert_eq!(reply["ok"], false);
    assert!(reply["error"].as_str().is_some_and(|e| e.contains("isn't downloaded yet")), "{reply}");
    assert!(harness.state().dialog.is_none() && harness.state().refused.is_none());
    // Nor does a command the window runs by itself, outside of anything the owner did.
    let _ = harness.state_mut().run("mail.move", json!({"ids": [row.id], "folder": mine}));
    harness.run_steps(2);
    assert!(harness.state().dialog.is_none());
}

/// What isn't a refusal stays in the status bar: a command that wasn't available, had nothing to
/// do or was given what it can't take, and the window's own commands.
#[test]
fn only_a_real_refusal_is_worth_a_message_box() {
    use crate::refusal_worth_a_box as boxed;
    assert!(boxed("mail.move", mailcraft_model::BETWEEN_MAILBOXES));
    assert!(!boxed("mail.delete", "not available: nothing is selected"));
    assert!(!boxed("compose.undo", "nothing to undo in the message"));
    assert!(!boxed("mail.move", "bad parameters: `folder` is a folder id"));
    assert!(!boxed("ui.openWindow", "no windows of their own here"));
    assert!(!boxed("mail.move", "  "));
}

/// Reminders fire: an event inside its reminder time shows in the reminders window, stays until
/// it is dismissed (which is remembered) or put off (which is not), and the window comes up on
/// its own.
#[test]
fn reminders_come_up_and_can_be_dismissed_or_put_off() {
    use mailcraft_time::DateTime;

    let mut a = app();
    // The sample has its own events; this test wants only the ones it makes.
    let ids: Vec<u64> = a.session.mailbox.events.keys().copied().collect();
    for id in ids {
        a.execute("calendar.delete", json!({"id": id})).unwrap();
    }
    let now = a.session.local_now();
    let at = |minutes: i64| -> DateTime { now.add_minutes(minutes) };
    let add = |a: &mut MailApp, title: &str, start: DateTime, reminder: Value| {
        let made = a.execute("calendar.new", json!({"title": title, "start": start.iso(), "end": start.add_minutes(30).iso(), "reminder": reminder}));
        made.unwrap()["id"].as_u64().unwrap()
    };
    let soon = add(&mut a, "Soon", at(10), json!(15));
    let later = add(&mut a, "Later", at(120), json!(15));
    let started = add(&mut a, "Started", at(-20), json!(5));
    add(&mut a, "No reminder", at(5), Value::Null);
    add(&mut a, "Long over", at(-3000), json!(15));

    // The engine's answer, as a script would ask for it.
    let due = a.execute("view.reminders", json!({})).unwrap();
    let titles: Vec<&str> = due["reminders"].as_array().unwrap().iter().map(|r| r["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Started", "Soon"]);
    assert_eq!(due["reminders"][1]["minutes"], 10);

    // The window shows them by itself once the app has looked.
    let mut harness = window(a);
    harness.run_steps(3);
    let showing = |h: &egui_kittest::Harness<'static, MailApp>| h.state().reminders.showing.iter().map(|r| r.event).collect::<Vec<_>>();
    assert_eq!(showing(&harness), [started, soon]);
    assert!(harness.state().hit.rect("reminder", soon).is_some(), "the window is on screen");

    // Put off: gone for now, not forgotten.
    harness.state_mut().execute("ui.snoozeReminders", json!({"minutes": 5, "event": soon, "occurrence": at(10).iso()})).unwrap();
    assert_eq!(showing(&harness), [started]);
    assert!(harness.state().ui.dismissed_reminders.is_empty());
    // Dismissed: gone for good, also after a restart with the same settings.
    harness.state_mut().execute("ui.dismissReminder", json!({"event": started, "occurrence": at(-20).iso()})).unwrap();
    assert!(showing(&harness).is_empty());
    harness.run_steps(3);
    assert!(harness.state().hit.rect("reminder", soon).is_none(), "nothing due: no window");
    let saved = harness.state().prefs();
    let mut again = MailApp::new(Session::at(harness.state().session.mailbox.clone(), NOW, 0), Services::default());
    again.autosave = false;
    again.apply_prefs(saved);
    let mut again = window(again);
    again.run_steps(3);
    // What was only put off is back after a restart; what was dismissed is not.
    assert_eq!(showing(&again), [soon]);
    again.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    assert!(showing(&again).is_empty());

    // An event whose reminder time comes later shows up then, not before.
    assert!(!showing(&again).contains(&later));
    for bad in [json!({}), json!({"event": soon}), json!({"all": false})] {
        assert!(again.state_mut().execute("ui.dismissReminder", bad).is_err());
    }
    for bad in [json!({}), json!({"minutes": 0}), json!({"minutes": 1e12}), json!({"minutes": "x"})] {
        assert!(again.state_mut().execute("ui.snoozeReminders", bad).is_err());
    }
}

/// The row under the pointer offers flag and delete at its right edge; they act on that row
/// without selecting it, and are not there for a row the pointer isn't over.
#[test]
fn the_row_under_the_pointer_offers_flag_and_delete() {
    use mailcraft_model::{FlagStatus, FolderKind};

    let mut harness = window(app());
    let selected = harness.state().session.sel.messages.clone();
    let rows: Vec<u64> = harness.state_mut().session.rows().iter().map(|r| r.id).collect();
    let (first, second) = (rows[0], rows[1]);
    assert!(harness.state().hit.rect("messageFlag", first).is_none(), "nothing is offered until the pointer is there");
    let hover = |harness: &mut egui_kittest::Harness<'static, MailApp>, id: u64| {
        let at = harness.state().hit.rect("message", id).unwrap().center();
        harness.event(egui::Event::PointerMoved(at));
        harness.run_steps(2);
    };

    hover(&mut harness, first);
    assert!(harness.state().hit.rect("messageFlag", second).is_none());
    let flag = harness.state().hit.rect("messageFlag", first).unwrap().center();
    let was = harness.state().session.mailbox.message(first).unwrap().flag;
    click_at(&mut harness, flag);
    let now = harness.state().session.mailbox.message(first).unwrap().flag;
    assert_ne!(now, was);
    assert!(now == FlagStatus::Flagged || was == FlagStatus::Flagged);
    assert_eq!(harness.state().session.sel.messages, selected, "the selection is left alone");

    hover(&mut harness, second);
    let delete = harness.state().hit.rect("messageDelete", second).unwrap().center();
    click_at(&mut harness, delete);
    let account = harness.state().session.mailbox.message(second).unwrap().account;
    let bin = harness.state().session.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    assert_eq!(harness.state().session.mailbox.message(second).unwrap().folder, bin);
    assert_eq!(harness.state().session.sel.messages, selected);
    assert!(harness.state_mut().session.undo());
}

/// What can't be taken back is asked about first when a person does it (deleting for good,
/// emptying or deleting a folder, removing an account); an ordinary delete is not, and neither is
/// a script, which said what it meant.
#[test]
fn what_cant_be_taken_back_is_asked_about_first() {
    use mailcraft_model::FolderKind;

    let mut a = app();
    let account = a.session.mailbox.accounts.first().unwrap().id;
    let bin = a.session.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let first = first_message(&mut a);
    let total = a.session.mailbox.messages.len();
    let asked = |a: &MailApp| matches!(&a.dialog, Some(crate::dialogs::Dialog::Confirm { .. }));

    // An ordinary delete moves to Deleted Items and asks nothing.
    a.run("mail.delete", json!({"ids": [first]})).unwrap();
    assert!(!asked(&a));
    assert_eq!(a.session.mailbox.message(first).unwrap().folder, bin);
    // Deleting it from there is for good: asked, and nothing happens until the answer.
    assert_eq!(a.run("mail.delete", json!({"ids": [first]})).unwrap()["pending"], "confirm");
    assert!(asked(&a) && a.session.mailbox.message(first).is_some());
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &a.dialog else { panic!("no question") };
    assert!(ask.text.contains("for good") && !ask.text.contains("server"), "{}", ask.text);
    // Cancelled: still there. Confirmed: gone.
    a.dialog = None;
    assert_eq!(a.session.mailbox.messages.len(), total);
    a.run("mail.delete", json!({"ids": [first], "confirmed": true})).unwrap();
    assert!(a.session.mailbox.message(first).is_none() && a.dialog.is_none());

    // The same for the permanent delete, wherever the message is; a script is not asked.
    let second = first_message(&mut a);
    assert_eq!(a.run("mail.deletePermanently", json!({"ids": [second]})).unwrap()["pending"], "confirm");
    a.dialog = None;
    assert!(a.session.mailbox.message(second).is_some());
    a.execute("mail.deletePermanently", json!({"ids": [second]})).unwrap();
    assert!(a.session.mailbox.message(second).is_none());

    // A message on a mail server: the question says the server loses it too.
    let server =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let inbox = a.session.mailbox.folder_of_kind(server, FolderKind::Inbox).unwrap();
    let id = a.session.mailbox.add_message(mailcraft_model::Message { folder: inbox, subject: "On a server".into(), ..Default::default() }).unwrap();
    a.run("mail.deletePermanently", json!({"ids": [id]})).unwrap();
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &a.dialog else { panic!("no question") };
    assert!(ask.text.contains("mail server"), "{}", ask.text);
    a.dialog = None;

    // Emptying a folder that has something in it, deleting a folder, removing an account.
    let third = first_message(&mut a);
    a.run("mail.delete", json!({"ids": [third]})).unwrap();
    assert_eq!(a.run("folder.empty", json!({"id": bin})).unwrap()["pending"], "confirm");
    a.dialog = None;
    assert_eq!(a.run("account.remove", json!({"id": server})).unwrap()["pending"], "confirm");
    a.dialog = None;
    assert!(a.session.mailbox.account(server).is_some());
    a.run("account.remove", json!({"id": server, "confirmed": true})).unwrap();
    assert!(a.session.mailbox.account(server).is_none());

    // The question lays out, and its Cancel leaves everything as it was.
    a.run("folder.empty", json!({"id": bin})).unwrap();
    let kept = a.session.mailbox.count(bin);
    let a = frames(a, (1280.0, 800.0), 3);
    assert_eq!(a.session.mailbox.count(bin), kept);
}

/// #186: a contact has no Deleted Items to go to, so deleting one by hand is asked about first,
/// naming it; a script is not asked.
#[test]
fn deleting_a_contact_is_asked_about_first() {
    let mut a = app();
    let id = a.execute("people.new", json!({"first": "Ivana", "last": "Horvat", "email": "ivana@example.org"})).unwrap()["id"].as_u64().unwrap();
    let total = a.session.mailbox.contacts.len();
    assert_eq!(a.run("people.delete", json!({"id": id})).unwrap()["pending"], "confirm");
    let Some(crate::dialogs::Dialog::Confirm { ask, command, .. }) = &a.dialog else { panic!("no question") };
    assert_eq!((command.as_str(), ask.title.as_str(), ask.action.as_str()), ("people.delete", "Delete Contact", "Delete"));
    assert_eq!(ask.text, "\"Ivana Horvat\" will be deleted for good. Contacts don't go to Deleted Items.");
    // Cancelled: still there. Confirmed: gone, and Undo brings it back.
    a.dialog = None;
    assert_eq!(a.session.mailbox.contacts.len(), total);
    a.run("people.delete", json!({"id": id, "confirmed": true})).unwrap();
    assert!(a.session.mailbox.contact(id).is_none() && a.dialog.is_none());
    a.execute("edit.undo", json!({})).unwrap();
    assert!(a.session.mailbox.contact(id).is_some());
    // The selected one, from the ribbon or the Delete key, is asked about the same way.
    a.execute("people.select", json!({"id": id})).unwrap();
    assert_eq!(a.run("people.delete", json!({})).unwrap()["pending"], "confirm");
    a.dialog = None;
    // A script is not asked.
    a.execute("people.delete", json!({"id": id})).unwrap();
    assert!(a.session.mailbox.contact(id).is_none());
    // A contact known by its address alone is named by it; the question lays out, and leaves
    // the contact where it is.
    let only = a.execute("people.new", json!({"email": "kim@example.org"})).unwrap()["id"].as_u64().unwrap();
    a.run("people.delete", json!({"id": only})).unwrap();
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &a.dialog else { panic!("no question") };
    assert!(ask.text.starts_with("\"kim@example.org\" will be deleted for good."), "{}", ask.text);
    let a = frames(a, (1280.0, 800.0), 3);
    assert!(a.session.mailbox.contact(only).is_some());
}

/// Typing a recipient offers the people it could be; Enter takes the one that is marked, the
/// arrows mark another, and typing carries on after it.
#[test]
fn recipients_are_suggested_as_they_are_typed() {
    use crate::compose::{completed, typing};
    use mailcraft_engine::cmd::compose::{Suggestion, suggest};

    // The pieces.
    assert_eq!(typing("Maya <maya@lumen.example>; pri"), ("Maya <maya@lumen.example>;", "pri"));
    assert_eq!(typing("pri"), ("", "pri"));
    assert_eq!(typing("a@b.example,"), ("a@b.example,", ""));
    let who = |name: &str, email: &str| Suggestion { name: name.into(), email: email.into(), contact: true };
    assert_eq!(completed("pri", &who("Priya Nair", "priya@lumen.example")), "Priya Nair <priya@lumen.example>; ");
    assert_eq!(completed("a@b.example; x", &who("", "x@y.example")), "a@b.example; x@y.example; ");
    assert_eq!(completed("x", &who("Nair, Priya", "priya@lumen.example")), "\"Nair, Priya\" <priya@lumen.example>; ");
    assert_eq!(completed("x", &who("Evil\" <evil@x.example>\r\n", "priya@lumen.example")), "Evil evil@x.example <priya@lumen.example>; ");

    // The engine's answer: contacts by any word of the name or the address, then people mail was
    // exchanged with, never the owner, never twice.
    let a = app();
    let found = suggest(&a.session, "pri", 6);
    assert_eq!(found.first().map(|s| (s.name.as_str(), s.contact)), Some(("Priya Nair", true)));
    assert!(suggest(&a.session, "NAIR", 6).iter().any(|s| s.email == "priya@lumen.example"));
    assert!(suggest(&a.session, "avery", 6).iter().all(|s| s.email != "avery@lumen.example"), "not the owner");
    let all = suggest(&a.session, "l", 20);
    let mut emails: Vec<String> = all.iter().map(|s| s.email.to_lowercase()).collect();
    emails.sort();
    emails.dedup();
    assert_eq!(emails.len(), all.len(), "each address once");
    assert!(suggest(&a.session, "", 6).is_empty() && suggest(&a.session, "zzzzqq", 6).is_empty());
    assert!(suggest(&a.session, &"x".repeat(100_000), 6).is_empty());

    // In the window: type, see them, take one with Enter, carry on.
    let mut a = app();
    a.execute("mail.new", json!({})).unwrap();
    let mut harness = window(a);
    harness.run_steps(3);
    let to = |h: &egui_kittest::Harness<'static, MailApp>| h.state().session.compose.as_ref().unwrap().to.clone();
    harness.event(egui::Event::Text("pri".into()));
    harness.run_steps(3);
    assert_eq!(to(&harness), "pri");
    let key = |h: &mut egui_kittest::Harness<'static, MailApp>, key: egui::Key| {
        for pressed in [true, false] {
            h.event(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
            h.step();
        }
        h.run_steps(2);
    };
    key(&mut harness, egui::Key::Enter);
    assert_eq!(to(&harness), "Priya Nair <priya@lumen.example>; ");
    // The keyboard is still in the line: the next name goes after it.
    harness.event(egui::Event::Text("may".into()));
    harness.run_steps(3);
    key(&mut harness, egui::Key::Tab);
    assert_eq!(to(&harness), "Priya Nair <priya@lumen.example>; Maya Okafor <maya@lumen.example>; ");
    // Nothing fits: Enter is just Enter, and what was typed stays.
    harness.event(egui::Event::Text("zzzzqq".into()));
    harness.run_steps(3);
    key(&mut harness, egui::Key::Enter);
    assert!(to(&harness).ends_with("zzzzqq"));
}

/// What a Send/Receive said went wrong stays to be read and copied (the status bar only flashes
/// it by), account by account, until a run of that account goes through.
#[test]
fn send_receive_problems_can_be_read_afterwards() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    use mailcraft_model::sync::{SyncRequest, SyncResult};

    let failing = Arc::new(AtomicBool::new(true));
    let mut a = app();
    let failing2 = failing.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        let errors = if failing2.load(Ordering::Relaxed) {
            vec![
                "Couldn't send \"Hello\": 535 5.7.139 Authentication unsuccessful, SmtpClientAuthentication is disabled for the Tenant".to_string(),
                "x".repeat(5000),
            ]
        } else {
            Vec::new()
        };
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, errors, ..Default::default() }).unwrap();
        rx
    }));
    let id =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    assert!(!a.has_sync_problems());

    a.execute("ui.sync", json!({"account": id, "password": "pw"})).unwrap();
    a.poll_sync();
    assert!(a.has_sync_problems());
    let lines = a.sync_problem_lines();
    assert!(lines[0].starts_with("avery@mail.example"), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("SmtpClientAuthentication is disabled")));
    assert!(lines.iter().all(|l| l.chars().count() < 700), "a server's megabyte of text is cut");
    // The whole of it under the account's row in Send/Receive Progress, which lays out.
    assert_eq!(a.execute("ui.dialog", json!({"name": "syncProblems"})).unwrap(), json!({"panel": "syncProgress"}));
    assert!(a.dialog.is_none() && a.sync_panel.as_ref().is_some_and(|p| p.to_problem));
    let rows = crate::sync_panel::rows(&a);
    assert_eq!((rows.len(), rows[0].problems.len(), rows[0].line()), (1, 2, "2 problems".to_string()));
    let mut a = frames(a, (1280.0, 800.0), 3);

    // A run that goes through clears them.
    a.services.sync = Some(Box::new(|req: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    a.dialog = None;
    a.execute("ui.sync", json!({"account": id, "password": "pw"})).unwrap();
    a.poll_sync();
    assert!(!a.has_sync_problems());
}

/// An account that signs in with Microsoft: before its mail is synchronised, Graph sends what
/// waits in the Outbox (so the mail server isn't asked to send it again) and brings the calendar
/// and contacts down, every quarter of an hour; without the owner's permission mail goes on as it
/// was, and "Connect calendar and contacts" asks for it.
#[test]
fn microsoft_graph_sends_and_brings_the_calendar_before_the_mail() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};

    use mailcraft_engine::cmd::sync_progress::RunState;
    use mailcraft_model::FolderKind;
    use mailcraft_model::remote::{ContactsDownload, EventsDownload, RemoteContact, RemoteEvent};
    use mailcraft_model::sync::{SyncPhase, SyncRequest, SyncResult, Tokens};

    // What Graph was asked (outbox, directory, had a token) and what the mail server was asked to send.
    let asked: Arc<Mutex<Vec<(usize, bool, bool)>>> = Default::default();
    let smtp: Arc<Mutex<Vec<usize>>> = Default::default();
    let sign_ins: Arc<Mutex<Vec<bool>>> = Default::default();
    let consent = Arc::new(AtomicBool::new(true));
    let fresh =
        |access: &str| Tokens { access: access.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() };

    let mut a = app();
    let smtp2 = smtp.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        smtp2.lock().unwrap().push(req.outbox.len());
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    let (asked2, consent2) = (asked.clone(), consent.clone());
    a.services.graph = Some(Box::new(move |req: crate::GraphRequest| {
        asked2.lock().unwrap().push((req.outbox.len(), req.directory, !req.access.is_empty()));
        let mut out = crate::GraphResult { account: req.account.id, ..Default::default() };
        if !consent2.load(Ordering::Relaxed) {
            out.consent_needed = true;
        } else {
            if req.access.is_empty() {
                out.tokens = Some(Tokens {
                    access: "graph-tok".into(),
                    refresh: "ref-2".into(),
                    expires_at: mailcraft_time::now() + 3600,
                    ..Default::default()
                });
            }
            out.sent = req.outbox.iter().map(|m| m.id).collect();
            if req.directory {
                let event =
                    RemoteEvent { id: "ev-1".into(), title: "From the server".into(), start: NOW + 3600, end: NOW + 7200, ..Default::default() };
                out.events = Some(
                    EventsDownload { from: NOW - 86_400, to: NOW + 86_400, complete: true, events: vec![event], ..Default::default() }
                        .for_account(&req.account),
                );
                let contact = RemoteContact { id: "c-1".into(), display_name: "Remote Person".into(), ..Default::default() };
                out.contacts = Some(ContactsDownload { complete: true, contacts: vec![contact], ..Default::default() }.for_account(&req.account));
            }
        }
        let (tx, rx) = mpsc::channel();
        tx.send(out).unwrap();
        rx
    }));
    let sign_ins2 = sign_ins.clone();
    a.services.sign_in = Some(Box::new(move |req: crate::SignInRequest| {
        sign_ins2.lock().unwrap().push(req.graph);
        let (tx, rx) = mpsc::channel();
        let tokens = Tokens { access: "graph-tok-2".into(), refresh: "ref-3".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() };
        tx.send(Ok(tokens)).unwrap();
        rx
    }));
    let added = a.execute(
        "account.add",
        json!({"email": "avery@mail.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "imap.mail.example"}}),
    );
    let id = added.unwrap()["id"].as_u64().unwrap();
    a.tokens.insert(id, fresh("mail-tok"));
    let settle = |a: &mut MailApp| {
        for _ in 0..8 {
            a.poll_sign_in();
            a.poll_graph();
            a.poll_sync();
        }
    };

    // The calendar's own job, which follows a round (#276): looked at, then answered.
    let mut clock = crate::now_ms();
    let calendar = |a: &mut MailApp, clock: &mut f64| {
        *clock += 400_000.0;
        let started = a.calendar_tick_at(true, *clock);
        for _ in 0..4 {
            a.poll_calendar();
        }
        started
    };

    // A message to send, and the first run: Graph sends it; the calendar and contacts follow.
    a.execute("mail.new", json!({"to": "maya@lumen.example", "subject": "Through Graph", "account": id})).unwrap();
    assert_eq!(a.execute("compose.send", json!({})).unwrap()["queued"], true);
    assert_eq!(a.execute("ui.sync", json!({"account": id})).unwrap()["pending"], "graph");
    assert!(!a.enabled("ui.sync"), "one run at a time");
    // Send/Receive Progress: sending first, through Microsoft.
    let row = |a: &MailApp| a.session.sync_progress.accounts.iter().find(|r| r.account == id).cloned().unwrap();
    assert_eq!((row(&a).state, row(&a).phase), (RunState::Running, Some(SyncPhase::Sending)));
    settle(&mut a);
    assert_eq!(*asked.lock().unwrap(), [(1, false, false)], "what is sent, and nothing of the calendar");
    assert_eq!(*smtp.lock().unwrap(), [0], "what Graph sent is not sent again by the mail server");
    // What Graph sent is part of the run, not the end of it: the mail server's leg comes after,
    // in the same row, which counts the message as sent by this run (not one kept from before).
    let r = row(&a);
    assert_eq!((r.state, r.sent, r.kept, a.session.sync_progress.accounts.len()), (RunState::Done, 1, false, 1));
    let (outbox, sent) =
        (a.session.mailbox.folder_of_kind(id, FolderKind::Outbox).unwrap(), a.session.mailbox.folder_of_kind(id, FolderKind::Sent).unwrap());
    assert_eq!((a.session.mailbox.count(outbox), a.session.mailbox.count(sent)), (0, 1));
    assert!(a.session.mailbox.events.values().all(|e| e.title != "From the server"), "not in the run");
    // The round is over: the calendar and contacts come, with the wide stretch.
    assert!(calendar(&mut a, &mut clock));
    assert_eq!(*asked.lock().unwrap(), [(1, false, false), (0, true, false)]);
    assert!(a.session.mailbox.events.values().any(|e| e.title == "From the server"));
    assert!(a.session.mailbox.contacts.values().any(|c| c.name() == "Remote Person"));
    assert_eq!(a.tokens.get(&id).unwrap().refresh, "ref-2", "the newest refresh token is the account's");

    // Soon after, with nothing to send: a Send/Receive the owner asked for is the mail alone, and
    // the days on screen follow it (not the wide stretch, nor the contacts).
    assert_eq!(a.execute("ui.sync", json!({"account": id})).unwrap()["started"], true);
    settle(&mut a);
    assert_eq!(asked.lock().unwrap().len(), 2);
    assert!(calendar(&mut a, &mut clock));
    assert_eq!(asked.lock().unwrap().last().map(|x| (x.0, x.1)), Some((0, false)));
    // One that started by itself goes straight to the mail: the calendar keeps itself current.
    assert_eq!(a.execute("ui.sync", json!({"account": id, "quiet": true})).unwrap()["started"], true);
    settle(&mut a);
    assert_eq!(asked.lock().unwrap().len(), 3);
    // A quarter of an hour on: the wide stretch again, with the token it has.
    a.graph_checked.insert(id, 0.0);
    a.execute("ui.sync", json!({"account": id})).unwrap();
    settle(&mut a);
    assert!(calendar(&mut a, &mut clock));
    assert_eq!(asked.lock().unwrap().last().map(|x| (x.0, x.1)), Some((0, true)));
    assert_eq!(a.session.mailbox.events.values().filter(|e| e.title == "From the server").count(), 1, "the same event, once");

    // No permission for Graph: mail goes on by the mail server, and Graph isn't asked every run.
    consent.store(false, Ordering::Relaxed);
    a.graph_checked.insert(id, 0.0);
    a.graph_tokens.clear();
    a.execute("mail.new", json!({"to": "maya@lumen.example", "subject": "By SMTP", "account": id})).unwrap();
    a.execute("compose.send", json!({})).unwrap();
    a.execute("ui.sync", json!({"account": id})).unwrap();
    settle(&mut a);
    calendar(&mut a, &mut clock);
    assert!(a.graph_consent_needed(id));
    assert_eq!(smtp.lock().unwrap().last(), Some(&1), "the mail server sends it");
    let before = asked.lock().unwrap().len();
    a.execute("ui.sync", json!({"account": id})).unwrap();
    settle(&mut a);
    assert_eq!(asked.lock().unwrap().len(), before);

    // "Connect calendar and contacts": a sign-in for Graph, then Graph is back.
    consent.store(true, Ordering::Relaxed);
    assert_eq!(a.execute("ui.connectMicrosoft", json!({"account": id})).unwrap()["pending"], "signIn");
    settle(&mut a);
    assert_eq!(sign_ins.lock().unwrap().last(), Some(&true));
    calendar(&mut a, &mut clock);
    assert!(!a.graph_consent_needed(id));
    assert_eq!(asked.lock().unwrap().last().map(|x| x.1), Some(true));

    // The account goes: what came from its server goes with it.
    a.execute("account.remove", json!({"id": id})).unwrap();
    assert!(!a.session.mailbox.events.values().any(|e| e.title == "From the server"));
    assert!(!a.session.mailbox.contacts.values().any(|c| c.name() == "Remote Person"));
}

/// A Send/Receive arrives in pieces: mail shows as each piece comes, the status bar says how far
/// it is, the run ends with the last piece, and it can be called off (what arrived stays).
#[test]
fn mail_arrives_in_pieces_and_a_run_can_be_called_off() {
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex, mpsc};

    use mailcraft_model::sync::{FolderDelta, SyncRequest, SyncResult};
    use mailcraft_model::{Address, FolderKind, Message};

    // The test hands the pieces over itself, one at a time.
    let pipe: Arc<Mutex<Option<mpsc::Sender<SyncResult>>>> = Default::default();
    let mut a = app();
    let pipe2 = pipe.clone();
    a.services.sync = Some(Box::new(move |_req: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        *pipe2.lock().unwrap() = Some(tx);
        rx
    }));
    let id =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let inbox = a.session.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap();
    let mail =
        |uid: u32| Message { uid, subject: format!("Message {uid}"), from: Address::new("", "x@example.org"), date: NOW, ..Default::default() };
    let piece = |uids: &[u32], seen: u32, partial: bool, progress: &str| SyncResult {
        account: id,
        partial,
        progress: progress.into(),
        folders: vec![FolderDelta {
            id: Some(inbox),
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: seen,
            new: uids.iter().map(|u| mail(*u)).collect(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let send = |p: SyncResult| pipe.lock().unwrap().as_ref().unwrap().send(p).unwrap();

    a.execute("ui.sync", json!({"account": id, "password": "pw"})).unwrap();
    assert!(a.syncing() && a.enabled("ui.cancelSync"));
    // The first piece: its mail is there at once, the run goes on, the status says how far.
    send(piece(&[4, 5], 3, true, "Inbox: 2 of 5"));
    a.poll_sync();
    assert_eq!(a.session.mailbox.count(inbox), 2);
    assert!(a.syncing());
    assert_eq!(a.session.sync_status, "Inbox: 2 of 5");
    // More, then the last piece ends it and says what the whole run came to.
    send(piece(&[2, 3], 1, true, "Inbox: 4 of 5"));
    send(piece(&[1], 5, false, ""));
    a.poll_sync();
    assert!(!a.syncing());
    assert_eq!(a.session.mailbox.count(inbox), 5);
    assert!(a.session.sync_status.starts_with("5 new messages"), "{}", a.session.sync_status);
    assert!(!a.enabled("ui.cancelSync") && a.execute("ui.cancelSync", json!({})).is_err());

    // Called off: the host is told, what came stays, and the round's other accounts are let be.
    a.execute("ui.sync", json!({"account": id})).unwrap();
    send(piece(&[6], 5, true, "Inbox: 1 of 3"));
    a.poll_sync();
    a.sync_queue.push_back((id, true));
    a.execute("ui.cancelSync", json!({})).unwrap();
    assert!(a.services.sync_cancel.load(Ordering::Relaxed) && a.sync_queue.is_empty());
    assert!(a.syncing(), "until the host has said it stopped");
    send(SyncResult { account: id, stopped: true, ..Default::default() });
    a.poll_sync();
    assert!(!a.syncing());
    assert_eq!(a.session.mailbox.count(inbox), 6);

    // A host that goes away mid-run is said to have stopped; nothing hangs.
    a.execute("ui.sync", json!({"account": id})).unwrap();
    send(piece(&[7], 6, true, "Inbox: 1 of 2"));
    *pipe.lock().unwrap() = None;
    a.poll_sync();
    a.poll_sync();
    assert!(!a.syncing());
    assert_eq!(a.session.mailbox.count(inbox), 7);
}

/// Something the owner does while a long run is still bringing mail down doesn't wait for the
/// run to end: the run is called off (what it brought stays), and the next one starts at once,
/// with the change to tell the server first.
#[test]
fn a_change_made_during_a_long_download_goes_out_without_waiting_for_it() {
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex, mpsc};

    use mailcraft_model::sync::{FolderDelta, SyncRequest, SyncResult};
    use mailcraft_model::{Address, FolderKind, Message};

    // Every run: what it was asked to tell the server, and the pipe its pieces come through.
    let asked: Arc<Mutex<Vec<usize>>> = Default::default();
    let pipe: Arc<Mutex<Option<mpsc::Sender<SyncResult>>>> = Default::default();
    let mut a = app();
    let (asked2, pipe2) = (asked.clone(), pipe.clone());
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        asked2.lock().unwrap().push(req.ops.len());
        let (tx, rx) = mpsc::channel();
        *pipe2.lock().unwrap() = Some(tx);
        rx
    }));
    let id =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    a.passwords.insert(id, "pw".into());
    let inbox = a.session.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap();
    let mail =
        |uid: u32| Message { uid, subject: format!("Message {uid}"), from: Address::new("", "x@example.org"), date: NOW, ..Default::default() };
    let piece = |uids: &[u32], seen: u32, partial: bool| SyncResult {
        account: id,
        partial,
        folders: vec![FolderDelta {
            id: Some(inbox),
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: seen,
            new: uids.iter().map(|u| mail(*u)).collect(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let send = |p: SyncResult| pipe.lock().unwrap().as_ref().unwrap().send(p).unwrap();
    // A look that is due, with no round of its own due.
    let look = |a: &mut MailApp| {
        a.last_auto_sync = crate::now_ms();
        a.push_checked = 0.0;
        a.poll_sync();
        a.auto_sync();
    };

    // A long run is bringing mail down.
    a.execute("ui.sync", json!({"account": id})).unwrap();
    send(piece(&[2, 3], 1, true));
    look(&mut a);
    assert!(a.syncing() && !a.services.sync_cancel.load(Ordering::Relaxed), "nothing waits: the run goes on");

    // The owner marks a message read: at the next look the run is called off.
    let first = a.session.mailbox.messages_in(inbox).next().unwrap().id;
    a.execute("mail.markRead", json!({"ids": [first]})).unwrap();
    look(&mut a);
    assert!(a.services.sync_cancel.load(Ordering::Relaxed));
    assert!(a.syncing(), "until the host says it stopped");
    look(&mut a);
    assert_eq!(asked.lock().unwrap().len(), 1, "one run at a time");

    // The host stops; the next run starts by itself and has the change to tell.
    send(SyncResult { account: id, stopped: true, ..Default::default() });
    look(&mut a);
    look(&mut a);
    assert_eq!(*asked.lock().unwrap(), [0, 1]);
    assert_eq!(a.session.mailbox.count(inbox), 2, "what the first run brought is still here");

    // A run the owner calls off is not started again behind their back.
    send(piece(&[4], 3, true));
    look(&mut a);
    a.execute("ui.cancelSync", json!({})).unwrap();
    send(SyncResult { account: id, stopped: true, ..Default::default() });
    look(&mut a);
    look(&mut a);
    assert_eq!(asked.lock().unwrap().len(), 2);
    assert!(!a.syncing());
}

/// Started with a `mailto:` link (MailCraft is the system's mail program and a link was clicked
/// somewhere), the program opens a new message to whoever the link names.
#[test]
fn a_mailto_link_handed_to_the_program_starts_a_message() {
    let mut a = app();
    a.open_path(" mailto:maya@lumen.example?subject=Hello%20there&body=Hi%20Maya ");
    let c = a.session.compose.clone().unwrap();
    assert_eq!((c.to.as_str(), c.subject.as_str()), ("maya@lumen.example", "Hello there"));
    assert!(c.body.plain().starts_with("Hi Maya"));
    // Something that only looks like a file is still treated as one (and isn't found).
    let mut b = app();
    b.open_path("mailto.eml");
    assert!(b.session.compose.is_none());
}

/// D1: the title bar's pill copies the same report as Help › Copy Diagnostics.
#[test]
fn the_title_bar_pill_copies_diagnostics() {
    let mut a = app();
    a.services.diagnostics = Some(Box::new(|| "the host's report".into()));
    let mut harness = window(a);
    use egui_kittest::kittest::Queryable as _;
    harness.get_by_label("Copy Diagnostics").click();
    harness.run_steps(2);
    assert_eq!(harness.state().status_msg.as_ref().map(|(s, _)| s.as_str()), Some("Diagnostics copied. Nothing was sent."));
}

#[test]
fn copy_diagnostics_takes_the_hosts_report_or_says_what_it_can() {
    let mut a = app();
    let alone = a.execute("ui.copyDiagnostics", json!({})).unwrap();
    assert!(alone["text"].as_str().unwrap().contains(env!("CARGO_PKG_VERSION")));
    a.services.diagnostics = Some(Box::new(|| "the host's report".into()));
    // (The host's report, then the last Send/Receive's timings, #276.)
    let text = a.execute("ui.copyDiagnostics", json!({})).unwrap()["text"].as_str().unwrap().to_string();
    assert!(text.starts_with("the host's report") && text.contains("Send/Receive timings"), "{text}");
}

#[test]
fn opening_a_damaged_mailbox_file_says_what_was_left_out() {
    let said = |v: Value| crate::feedback("file.open", &v);
    // Part of the file itself left out is said first, whichever of the counts says it.
    let damaged = crate::damaged_mailbox_notice();
    assert_eq!(said(json!({"lostSaves": 1, "unfinished": 300, "unavailable": 2})), Some(damaged.clone()));
    assert_eq!(said(json!({"lostSaves": 0, "unfinished": 300})), Some(damaged.clone()));
    // A damaged item of the file's first save is that too.
    assert_eq!(said(json!({"lostSaves": 0, "unfinished": 0, "lostRecords": 1})), Some(damaged));
    // Otherwise what the content folder lost, and nothing when nothing was.
    assert!(said(json!({"lostSaves": 0, "unfinished": 0, "unavailable": 2})).unwrap().starts_with("2 parts of messages"));
    assert_eq!(said(json!({"lostSaves": 0, "unfinished": 0, "unavailable": 0})), None);
}

/// R3 spec S30: mail imported for a Microsoft account goes to a local folder, and moving it into
/// the account's folders is refused, with the reason in the status bar.
#[test]
fn imported_mail_for_a_microsoft_account_stays_in_a_local_folder_and_says_why() {
    let mut a = app();
    let account = a.run("account.add", json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = a.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    let id = first_message(&mut a);
    let eml = a.execute("file.exportMessage", json!({"id": id})).unwrap();
    let r = a.run("file.import", json!({"name": "in.eml", "data": eml["data"], "folder": inbox})).unwrap();
    let said = a.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert_eq!(said, "Imported 1 message into a local folder: Microsoft doesn't let a mail program put received mail into your mailbox");
    let imported = r["ids"][0].as_u64().unwrap();
    assert_ne!(a.session.mailbox.message(imported).unwrap().folder, inbox);
    assert!(a.run("mail.move", json!({"ids": [imported], "folder": inbox})).is_err());
    let said = a.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert_eq!(said, "Microsoft doesn't let a mail program put received mail into your mailbox; it stays in a local folder.");
    assert_ne!(a.session.mailbox.message(imported).unwrap().folder, inbox);
}

/// A search says so over the list: how many it found and where; ✕ or Esc ends it (#115).
#[test]
fn a_search_is_said_over_the_list_and_ended_by_its_cross_or_esc() {
    use egui_kittest::kittest::Queryable;
    let mut a = app();
    let found = a.execute("view.search", json!({"query": "Atlas"})).unwrap()["found"].as_u64().unwrap();
    assert!(found > 1);
    let inbox = a.session.mailbox.folder(a.session.view.folder).map(|f| f.name.clone()).unwrap();
    let mut h = window(a);
    h.run_steps(3);
    h.get_by_label(&format!("{found} results in {inbox}"));
    // Every mailbox.
    h.state_mut().execute("view.search", json!({"query": "Atlas", "scope": "everywhere"})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label_contains("results in All Mailboxes").is_some());
    // Nothing found.
    h.state_mut().execute("view.search", json!({"query": "zzzzqq", "scope": "folder"})).unwrap();
    h.run_steps(3);
    h.get_by_label(&format!("No results in {inbox}"));
    // The cross ends it.
    let close = h.state().hit.rect("searchBarClose", 0).unwrap();
    h.event(egui::Event::PointerMoved(close.center()));
    h.event(egui::Event::PointerButton {
        pos: close.center(),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    h.event(egui::Event::PointerButton {
        pos: close.center(),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
    assert!(h.state().session.view.search.is_empty());
    assert!(h.query_by_label_contains("results in").is_none() && h.query_by_label_contains("No results").is_none());
    // So does Esc, with the keyboard in the list.
    h.state_mut().execute("view.search", json!({"query": "Atlas"})).unwrap();
    h.run_steps(3);
    for pressed in [true, false] {
        h.event(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    assert!(h.state().session.view.search.is_empty());
}

/// #122: signatures have one place in File, their own page; File › Options no longer has a
/// Signatures section with a button that opens that page.
#[test]
fn signatures_are_in_file_once() {
    use egui_kittest::kittest::Queryable as _;
    let mut harness = window(app());
    harness.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    harness.run_steps(3);
    assert!(harness.query_by_label_contains("Show week numbers").is_some());
    // Neither the button nor the section's heading (File's list of pages is painted, not labelled).
    assert!(harness.query_by_label("Signatures…").is_none());
    assert!(harness.query_by_label("Signatures").is_none());
    assert!(crate::backstage::PAGES.iter().any(|(page, ..)| *page == "signatures"));
    harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    harness.run_steps(3);
    assert_eq!(harness.state().ui.backstage_page, "signatures");
}

/// File › Info for a Microsoft account: what comes after its card (the download choice, the link
/// to connect the calendar) is laid out below the card, never over it or over each other (#121).
#[test]
fn info_account_card_keeps_what_follows_below_it() {
    use egui_kittest::kittest::Queryable;
    let mut a = app();
    if let Some(acc) = a.session.mailbox.accounts.first_mut() {
        acc.kind = mailcraft_model::AccountKind::Microsoft;
    }
    a.execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    let mut h = window(a);
    h.run_steps(3);
    let rect = |n: egui_kittest::Node<'_>| n.rect();
    let settings = rect(h.get_by_label("Settings"));
    let download = rect(h.get_by_label_contains("Download messages in full"));
    // The card is 58 pt, and Settings sits in its middle: the download line starts below the
    // card's bottom edge, with the page's spacing between them.
    let card_bottom = settings.center().y + 29.0;
    assert!(download.min.y >= card_bottom + 8.0, "download {download:?} under the card ending at {card_bottom}");
}

/// #313: File › Info has Remove for the only account too, beside its Settings, and the question
/// says MailCraft will have no account afterwards. The empty mailbox left has nothing to remove.
#[test]
fn the_only_account_can_be_removed_in_file_info() {
    use egui_kittest::kittest::Queryable;
    let mut a = app();
    let _ = a.execute("ui.dismissReminder", json!({"all": true}));
    let only = match a.session.mailbox.accounts.as_slice() {
        [only] => only.id,
        more => panic!("the sample has one account: {more:?}"),
    };
    a.execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    let mut h = window(a);
    h.run_steps(3);
    let (settings, remove) = (h.get_by_label("Settings").rect(), h.get_by_label("Remove").rect());
    assert!(settings.max.x < remove.min.x, "Settings {settings:?} beside Remove {remove:?}");
    h.get_by_label("Remove").click();
    h.run_steps(3);
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &h.state().dialog else { panic!("no question") };
    assert!(ask.text.contains("MailCraft will have no account afterwards"), "{}", ask.text);
    assert!(h.state().session.mailbox.account(only).is_some(), "only asked so far");

    // Not said for an account that isn't the last.
    let a = h.state_mut();
    a.dialog = None;
    let other = a.execute("account.add", json!({"email": "ada@lumen.example"})).unwrap()["id"].as_u64().unwrap();
    a.run("account.remove", json!({"id": other})).unwrap();
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &a.dialog else { panic!("no question") };
    assert!(!ask.text.contains("no account afterwards"), "{}", ask.text);
    a.dialog = None;
    a.execute("account.remove", json!({"id": other})).unwrap();

    a.run("account.remove", json!({"id": only, "confirmed": true})).unwrap();
    assert!(matches!(a.session.mailbox.accounts.as_slice(), [fresh] if fresh.email.is_empty()), "{:?}", a.session.mailbox.accounts);
    h.run_steps(3);
    assert!(h.state().ui.backstage && h.state().ui.backstage_page == "info");
    assert!(h.query_by_label("Remove").is_none() && h.query_by_label("Settings").is_none());
    // Asked for anyway, nothing is asked: the command says there is nothing to remove.
    let a = h.state_mut();
    let placeholder = a.session.mailbox.placeholder().unwrap();
    assert!(a.run("account.remove", json!({"id": placeholder})).is_err());
    assert!(a.dialog.is_none());
}

/// #196: the search box draws keywords as pills, and the query stays what was typed: the pills'
/// capitals never reach it, Backspace edits it as text, and the keyboard alone does all of it.
#[test]
fn search_pills_are_only_how_the_box_draws_the_query() {
    let mut a = app();
    let _ = a.execute("ui.dismissReminder", json!({"all": true}));
    a.execute("ui.focusSearch", json!({})).unwrap();
    let mut harness = window(a);
    harness.run_steps(3);
    let key = |h: &mut egui_kittest::Harness<'static, MailApp>, key: egui::Key| {
        for pressed in [true, false] {
            h.event(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
            h.step();
        }
        h.run_steps(2);
    };
    harness.event(egui::Event::Text("from: priya subject: atlas ".into()));
    harness.run_steps(3);
    assert_eq!(harness.state().session.view.search, "from: priya subject: atlas ");
    assert_eq!(crate::search_pills::pills(&harness.state().search_text, None).len(), 2);
    key(&mut harness, egui::Key::Backspace);
    key(&mut harness, egui::Key::Backspace);
    assert_eq!(harness.state().session.view.search, "from: priya subject: atla");
    harness.event(egui::Event::Text("s".into()));
    harness.run_steps(3);
    assert_eq!(harness.state().session.view.search, "from: priya subject: atlas");
    assert_eq!(harness.state_mut().session.rows().len(), 1, "the Atlas welcome from Priya");
}

/// An account a newer MailCraft added: its card in File › Info says what it is, no Send/Receive
/// of the window takes it in, and the window lays it out as any other.
#[test]
fn an_account_a_newer_version_added_is_said_to_be_one_and_left_out_of_send_receive() {
    let mut a = app();
    let servers = json!({"email": "ada@imap.example", "incoming": {"host": "imap.example.org"}});
    let imap = a.execute("account.add", servers).unwrap()["id"].as_u64().unwrap();
    let newer = mailcraft_model::Account { email: "ada@jmap.example".into(), kind: mailcraft_model::AccountKind::Unknown, ..Default::default() };
    let newer = a.session.mailbox.add_account(newer);
    let card = crate::backstage::account_detail(&a, a.session.mailbox.account(newer).unwrap());
    assert_eq!(card, "Added by a newer version of MailCraft: send and receive with it there");
    assert_eq!(a.server_accounts(), vec![imap]);
    a.execute("ui.backstage", json!({"value": true, "page": "info"})).unwrap();
    let mut h = window(a);
    h.run_steps(3);
    assert!(h.state().ui.backstage && h.state().session.mailbox.account(newer).is_some());
}

/// A JMAP account (M6): no Send/Receive of the window takes it in until the engine has its
/// commands. Asked for by name, it is refused with its reason, nothing is handed to the host and
/// nothing is asked of the owner; the accounts beside it go on as before.
#[test]
fn a_jmap_account_is_left_out_of_send_receive_until_one_can_be_run() {
    let mut a = app();
    let handed: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let log = handed.clone();
    a.services.sync = Some(Box::new(move |req: mailcraft_model::sync::SyncRequest| {
        log.lock().unwrap().push(req.account.email.clone());
        std::sync::mpsc::channel::<mailcraft_model::sync::SyncResult>().1
    }));
    let servers = json!({"email": "ada@imap.example", "incoming": {"host": "imap.example.org"}});
    let imap = a.execute("account.add", servers).unwrap()["id"].as_u64().unwrap();
    let server = mailcraft_model::JmapServer { session: "https://jmap.example/.well-known/jmap".into(), bearer: true };
    let jmap = mailcraft_model::Account {
        email: "ada@jmap.example".into(),
        kind: mailcraft_model::AccountKind::Jmap,
        jmap: Some(server),
        ..Default::default()
    };
    let jmap = a.session.mailbox.add_account(jmap);
    assert_eq!(a.server_accounts(), vec![imap]);
    let why = a.execute("ui.sync", json!({"account": jmap})).unwrap_err();
    assert_eq!(why, "ada@jmap.example is a JMAP account, and this version doesn't send and receive with one yet");
    assert!(handed.lock().unwrap().is_empty() && a.dialog.is_none());
    // A Send/Receive of every account is the IMAP account's alone.
    a.execute("ui.sync", json!({"password": "not-a-real-password"})).unwrap();
    assert_eq!(*handed.lock().unwrap(), ["ada@imap.example"]);
}

/// A noisy PNG far over a signature picture's cap.
fn big_png(side: u32) -> Vec<u8> {
    use image::ImageEncoder as _;
    let mut x = 0x9E37_79B9_u32;
    let raw: Vec<u8> = (0..side * side * 3)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect();
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&raw, side, side, image::ExtendedColorType::Rgb8).unwrap();
    out
}

fn signature_app() -> MailApp {
    let mut a = app();
    a.execute("signature.set", json!({"name": "Mine", "text": "Avery"})).unwrap();
    a.execute("signature.open", json!({"name": "Mine"})).unwrap();
    a
}

/// #270: a picture over the cap is made smaller to fit, and the page says so.
#[test]
fn signature_picture_over_the_cap_is_made_smaller() {
    use mailcraft_model::signature::{MAX_PICTURE_BYTES, MAX_WIDTH};
    let png = big_png(1200);
    assert!(png.len() > 4 * MAX_PICTURE_BYTES);
    let mut a = signature_app();
    crate::signatures::add_picture(&mut a, json!({"data": crate::files::base64_lite::encode(&png), "file": "logo.png"}));
    let e = a.session.signature_edit.as_ref().unwrap();
    let p = e.signature.pictures().first().expect("picture added");
    assert!(p.data.len() <= MAX_PICTURE_BYTES);
    assert!(image::load_from_memory(&p.data).unwrap().width() <= u32::from(MAX_WIDTH));
    assert!(a.signature_note.as_ref().is_some_and(|n| !n.error));
}

/// #270: a refusal shows on the Signatures page (the status bar is hidden behind File).
#[test]
fn signature_picture_refusal_is_on_the_page() {
    let mut a = signature_app();
    crate::signatures::add_picture(&mut a, json!({"data": crate::files::base64_lite::encode(b"not a picture at all"), "file": "x.png"}));
    assert!(a.signature_note.as_ref().is_some_and(|n| n.error && !n.text.is_empty()));
    assert!(a.session.signature_edit.as_ref().unwrap().signature.pictures().is_empty());
    // A new try clears it.
    crate::signatures::add_picture(&mut a, json!({"data": crate::files::base64_lite::encode(&big_png(900)), "file": "y.png"}));
    assert!(a.signature_note.as_ref().is_some_and(|n| !n.error));
}

/// #270: a shrunk picture that would take the signature over its total is refused on the page,
/// and the signature keeps what it had.
#[test]
fn signature_picture_over_the_total_is_refused_on_the_page() {
    use mailcraft_model::signature::{MAX_PICTURES, MAX_PICTURES_BYTES};
    let png = crate::files::base64_lite::encode(&big_png(1200));
    let mut a = signature_app();
    let total = |a: &MailApp| a.session.signature_edit.as_ref().unwrap().signature.pictures().iter().map(|p| p.data.len()).sum::<usize>();
    let mut refused = false;
    for _ in 0..MAX_PICTURES {
        let before = total(&a);
        crate::signatures::add_picture(&mut a, json!({"data": png, "file": "logo.png"}));
        if a.signature_note.as_ref().is_some_and(|n| n.error) {
            assert_eq!(total(&a), before);
            refused = true;
            break;
        }
    }
    assert!(refused);
    assert!(total(&a) <= MAX_PICTURES_BYTES);
}
