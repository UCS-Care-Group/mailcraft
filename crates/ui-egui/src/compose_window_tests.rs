// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message being written in a window of its own (#286), as the window draws it: New Email opens
//! one, Pop Out moves the reading pane's into one, each types and runs its ribbon on its own
//! message, closing asks about changes, quitting saves them all, an agent's opens behind, and at
//! the bound of windows a reply stays in the reading pane.

use egui::Key;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::control::{ControlRequest, Outcome};
use crate::windows::MAX_WINDOWS;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Harness = egui_kittest::Harness<'static, MailApp>;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    app
}

/// The whole window, its frames a sixtieth of a second apart. (Windows of their own are inside
/// the main one here, as in the browser.)
fn window(app: MailApp) -> Harness {
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
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

fn click(harness: &mut Harness, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
}

/// Run `command` as the owner does (the ribbon, a key).
fn owner(harness: &mut Harness, command: &str, params: Value) -> Value {
    let answer = harness.state_mut().run(command, params).unwrap_or_else(|e| panic!("{command}: {e}"));
    harness.run_steps(3);
    answer
}

/// Run `command` as a script does.
fn script(harness: &mut Harness, command: &str, params: Value) -> Value {
    let answer = harness.state_mut().execute(command, params).unwrap_or_else(|e| panic!("{command}: {e}"));
    harness.run_steps(3);
    answer
}

fn type_text(harness: &mut Harness, text: &str) {
    harness.event(egui::Event::Text(text.into()));
    harness.run_steps(3);
}

/// The windows `ui.inspect` lists.
fn windows(harness: &mut Harness) -> Vec<Value> {
    let ctx = harness.ctx.clone();
    crate::control::inspect(harness.state_mut(), &ctx)["windows"].as_array().cloned().unwrap_or_default()
}

/// The messages being written `ui.inspect` lists.
fn being_written(harness: &mut Harness) -> Vec<Value> {
    let ctx = harness.ctx.clone();
    crate::control::inspect(harness.state_mut(), &ctx)["beingWritten"].as_array().cloned().unwrap_or_default()
}

/// The window of the message being written `compose`, by its number.
fn window_of(harness: &Harness, compose: u64) -> u64 {
    harness.state().windows.of_compose(compose).unwrap_or_else(|| panic!("message {compose} has no window"))
}

fn pane(harness: &Harness) -> Option<u64> {
    crate::windows::compose_window::pane_compose(harness.state())
}

/// The plain text of the message being written `compose`.
fn text_of(harness: &Harness, compose: u64) -> String {
    harness.state().session.composing_by_id(compose).map(|c| c.body.plain()).unwrap_or_default()
}

/// The drafts in the mailbox with `subject`.
fn drafts(harness: &Harness, subject: &str) -> usize {
    harness.state().session.mailbox.messages.values().filter(|m| m.draft && m.subject == subject).count()
}

/// Click the ribbon control `key` of the window numbered `window` (0: the main window's).
fn ribbon(harness: &mut Harness, window: u64, kind: &str, key: &str) {
    let rect = harness.state().hit.rect_in(window, kind, crate::ribbon::hit_id(key)).unwrap_or_else(|| panic!("no {kind} {key} in window {window}"));
    click(harness, rect.center());
}

/// The keyboard to the body of the message being written `compose`.
fn into_body(harness: &mut Harness, compose: u64) {
    harness.ctx.memory_mut(|m| m.request_focus(crate::compose::editor::id_of(compose)));
    harness.run_steps(3);
}

/// Answer the question the window numbered `window` asks about its unsaved changes.
fn answer(harness: &mut Harness, window: u64, kind: &str) {
    let rect = harness.state().hit.rect(kind, window).unwrap_or_else(|| panic!("window {window} doesn't ask ({kind})"));
    click(harness, rect.center());
}

/// `command` sent by an agent the owner allowed `allow` in advance.
fn agent(a: &mut MailApp, command: &str, params: Value, allow: Vec<Class>) -> Option<Value> {
    let (req, _rx) = ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    match crate::control::handle(a, &egui::Context::default(), &req.from_agent(allow)) {
        Outcome::Done(v) => Some(v),
        _ => None,
    }
}

#[test]
fn new_email_opens_in_a_window_of_its_own() {
    let mut h = window(app());
    // As the ribbon's New Email runs it: a window, its message current, the reading pane free and
    // the main window's ribbon as it was.
    let compose = owner(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    let w = window_of(&h, compose);
    assert_eq!((pane(&h), h.state().session.compose_id(), h.state().ui.tab.as_str()), (None, Some(compose), "Home"));
    let shown = windows(&mut h);
    assert_eq!(shown.len(), 1);
    assert_eq!(
        (&shown[0]["kind"], &shown[0]["compose"], &shown[0]["tab"], &shown[0]["dirty"]),
        (&json!("compose"), &json!(compose), &json!("Message"), &json!(false))
    );
    assert_eq!(shown[0]["title"], "Untitled - Message");
    let listed = being_written(&mut h);
    assert_eq!((&listed[0]["window"], &listed[0]["pane"]), (&json!(w), &json!(false)));
    // It is on screen with its ribbon, in its own tabs, and the To line has the keyboard: what
    // is typed goes to it.
    assert!(h.state().hit.rect("window", w).is_some());
    for tab in crate::windows::compose_window::TABS {
        assert!(h.state().hit.rect_in(w, "ribbonTab", crate::ribbon::hit_id(tab)).is_some(), "{tab}");
    }
    assert!(h.state().hit.rect_in(w, "ribbon", crate::ribbon::hit_id("compose.send")).is_some());
    assert!(h.state().hit.rect_in(w, "ribbon", crate::ribbon::hit_id("compose.popOut")).is_none(), "nothing to pop out of");
    type_text(&mut h, "maya@lumen.example");
    assert_eq!(h.state().session.composing_by_id(compose).map(|c| c.to.as_str()), Some("maya@lumen.example"));
    // Its tabs are its own: the main window's stays where it was.
    ribbon(&mut h, w, "ribbonTab", "Format Text");
    assert_eq!((windows(&mut h)[0]["tab"].as_str(), h.state().ui.tab.as_str()), (Some("Format Text"), "Home"));
    assert!(h.state().hit.rect_in(w, "ribbon", crate::ribbon::hit_id("compose.formatPlain")).is_some());
    // Ctrl+N, as the owner presses it in the main window: another window.
    h.state_mut().ui.tab = "Home".into();
    let before = h.state().windows.len();
    h.state_mut().run("mail.new", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().windows.len(), before + 1);
    // A script's New Email starts in the reading pane, as it always has.
    let scripted = script(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    assert_eq!((pane(&h), h.state().windows.of_compose(scripted)), (Some(scripted), None));
}

#[test]
fn pop_out_moves_the_reading_panes_message_into_a_window() {
    let mut h = window(app());
    let compose = script(&mut h, "mail.new", json!({"subject": "Inline"}))["compose"].as_u64().unwrap();
    script(&mut h, "compose.set", json!({"body": "Written in the pane"}));
    assert_eq!(pane(&h), Some(compose));
    // The button at the top right of the message in the reading pane.
    let rect = h.state().hit.rect_in(0, "composePopOut", 0).expect("Pop Out in the reading pane");
    assert!(h.state().hit.rect_in(0, "ribbon", crate::ribbon::hit_id("compose.popOut")).is_some(), "and on the Message tab");
    click(&mut h, rect.center());
    let w = window_of(&h, compose);
    // The reading pane is free; nothing of the message changed; the main window is back on Home.
    assert_eq!(pane(&h), None);
    assert_eq!(text_of(&h, compose), "Written in the pane");
    assert!(h.state().session.composing_by_id(compose).is_some_and(|c| c.dirty));
    assert_eq!(h.state().ui.tab, "Home");
    assert!(h.state().hit.rect_in(0, "composePopOut", 0).is_none() && h.state().hit.rect("window", w).is_some());
    assert_eq!(windows(&mut h)[0]["title"], "Inline - Message");
    // By command too, for another: one that has a window already only comes forward.
    let other = script(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    let answer = script(&mut h, "compose.popOut", json!({}));
    assert_eq!((answer["compose"].as_u64(), answer["window"].as_u64()), (Some(other), h.state().windows.of_compose(other)));
    assert_eq!(script(&mut h, "ui.openWindow", json!({"compose": other}))["window"].as_u64(), h.state().windows.of_compose(other));
    assert_eq!(h.state().windows.len(), 2);
    // Nonsense pops nothing out.
    for bad in [json!({"compose": 999}), json!({"compose": "1"}), json!({"compose": -1})] {
        assert!(h.state_mut().execute("ui.openWindow", bad.clone()).is_err(), "{bad}");
        assert!(h.state_mut().execute("compose.popOut", bad.clone()).is_err(), "{bad}");
    }
    assert_eq!(h.state().windows.len(), 2);
}

#[test]
fn two_windows_are_written_in_apart() {
    let mut h = window(app());
    let a = owner(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    let b = owner(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    assert_ne!(window_of(&h, a), window_of(&h, b));
    into_body(&mut h, a);
    type_text(&mut h, "For the first");
    into_body(&mut h, b);
    type_text(&mut h, "For the second");
    assert!(text_of(&h, a).starts_with("For the first") && !text_of(&h, a).contains("second"), "{}", text_of(&h, a));
    assert!(text_of(&h, b).starts_with("For the second") && !text_of(&h, b).contains("first"), "{}", text_of(&h, b));
}

#[test]
fn the_ribbon_of_a_window_acts_on_its_own_message() {
    let mut h = window(app());
    // (The classic layout, where Importance is in view in the window's width.)
    script(&mut h, "ui.ribbonMode", json!({"value": "classic"}));
    let inline = script(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    let popped = owner(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    let w = window_of(&h, popped);
    let importance = |h: &Harness, c: u64| h.state().session.composing_by_id(c).map(|c| c.importance);
    ribbon(&mut h, w, "ribbon", "compose.highImportance");
    assert_eq!(importance(&h, popped), Some(mailcraft_model::Importance::High));
    assert_eq!(importance(&h, inline), Some(mailcraft_model::Importance::Normal));
    // The main window's, on the reading pane's (the window moved down, off the main ribbon).
    let layer = crate::windows::embedded_layer(w);
    let mut state = egui::AreaState::load(&h.ctx, layer.id).unwrap();
    state.set_left_top_pos(egui::pos2(400.0, 600.0));
    h.ctx.memory_mut(|m| m.areas_mut().set_state(layer, state));

    h.state_mut().ui.tab = "Message".into();
    h.run_steps(3);

    ribbon(&mut h, 0, "ribbon", "compose.lowImportance");
    assert_eq!(importance(&h, inline), Some(mailcraft_model::Importance::Low));
    assert_eq!(importance(&h, popped), Some(mailcraft_model::Importance::High));
    // Its keys too: Ctrl+S saves its message, not the reading pane's.
    script(&mut h, "compose.set", json!({"compose": popped, "subject": "Keyed"}));
    script(&mut h, "compose.set", json!({"compose": inline, "subject": "Untouched"}));
    let wl = crate::windows::embedded_layer(w);
    let pos = h.state().hit.rect("window", w).unwrap().min + egui::vec2(12.0, 12.0);
    click(&mut h, pos);
    assert_eq!(crate::keys::active(&h.ctx), wl);
    let ctrl = egui::Modifiers { ctrl: !cfg!(target_os = "macos"), mac_cmd: cfg!(target_os = "macos"), command: true, ..Default::default() };
    h.key_press_modifiers(ctrl, Key::S);
    h.run_steps(3);
    assert_eq!((drafts(&h, "Keyed"), drafts(&h, "Untouched")), (1, 0));
}

#[test]
fn closing_a_window_with_changes_asks_save_dont_save_or_cancel() {
    for (said, kept, open) in [("unsavedSave", 1, false), ("unsavedDiscard", 0, false), ("unsavedCancel", 0, true)] {
        let mut h = window(app());
        let compose = owner(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
        script(&mut h, "compose.set", json!({"compose": compose, "subject": "Asked about", "body": "Something"}));
        let w = window_of(&h, compose);
        h.get_by_label("Close window").click();
        h.run_steps(3);
        assert_eq!(windows(&mut h)[0]["asking"], true, "{said}");
        answer(&mut h, w, said);
        assert_eq!(drafts(&h, "Asked about"), kept, "{said}");
        assert_eq!((h.state().windows.of_compose(compose).is_some(), h.state().session.composing_by_id(compose).is_some()), (open, open), "{said}");
    }
    // Without changes, it closes at once, and its message with it.
    let mut h = window(app());
    let compose = owner(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    h.get_by_label("Close window").click();
    h.run_steps(3);
    assert!(h.state().windows.is_empty() && h.state().session.composing_by_id(compose).is_none());
    // Sent, its window goes.
    let compose = owner(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    script(&mut h, "compose.set", json!({"compose": compose, "to": "maya@lumen.example", "subject": "Off it goes"}));
    script(&mut h, "compose.send", json!({"compose": compose}));
    assert!(h.state().windows.is_empty());
}

#[test]
fn ui_close_window_says_save_or_discard_for_a_message_being_written() {
    let mut h = window(app());
    let compose = owner(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    script(&mut h, "compose.set", json!({"compose": compose, "subject": "By command", "body": "Words"}));
    let w = window_of(&h, compose);
    for bad in [
        json!({"window": w}),
        json!({"compose": compose}),
        json!({"all": true}),
        json!({}),
        json!({"window": w, "save": true, "discard": true}),
        json!({"window": w, "save": "yes"}),
    ] {
        assert!(h.state_mut().execute("ui.closeWindow", bad.clone()).is_err(), "{bad}");
        assert!(h.state().dialog.is_none() && h.state().windows.of_compose(compose).is_some(), "{bad}: nothing closed, nothing asked");
    }
    assert_eq!(script(&mut h, "ui.closeWindow", json!({"compose": compose, "save": true}))["closed"], 1);
    assert_eq!(drafts(&h, "By command"), 1);
    assert!(h.state().session.composing_by_id(compose).is_none());
    // Discard: closed as it was last saved (here, never).
    let compose = owner(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    script(&mut h, "compose.set", json!({"compose": compose, "subject": "Thrown away", "body": "Words"}));
    let w = window_of(&h, compose);
    script(&mut h, "ui.closeWindow", json!({"window": w, "discard": true}));
    assert_eq!((drafts(&h, "Thrown away"), h.state().windows.len()), (0, 0));
}

#[test]
fn closing_the_main_window_saves_every_message_being_written_unasked() {
    let mut h = window(app());
    let inline = script(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    script(&mut h, "compose.set", json!({"compose": inline, "subject": "In the pane", "body": "One"}));
    for subject in ["Window one", "Window two"] {
        let compose = owner(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
        script(&mut h, "compose.set", json!({"compose": compose, "subject": subject, "body": "Two"}));
    }
    assert!(h.state_mut().close_requested(), "nothing is asked");
    assert!(h.state().dialog.is_none());
    for subject in ["In the pane", "Window one", "Window two"] {
        assert_eq!(drafts(&h, subject), 1, "{subject}");
    }
}

#[test]
fn an_agents_message_opens_behind_without_the_keyboard() {
    let mut h = window(app());
    // The owner is writing in the reading pane, the keyboard in its body.
    let mine = script(&mut h, "mail.new", json!({}))["compose"].as_u64().unwrap();
    into_body(&mut h, mine);
    type_text(&mut h, "Mine");
    let said = agent(h.state_mut(), "mail.new", json!({"subject": "From an agent"}), Class::ALL.to_vec()).expect("done at once");
    assert_eq!(said["ok"], true, "{said}");
    let theirs = said["result"]["compose"].as_u64().unwrap();
    h.run_steps(3);
    let w = window_of(&h, theirs);
    let shown = windows(&mut h);
    assert_eq!((&shown[0]["behind"], &shown[0]["compose"]), (&json!(true), &json!(theirs)));
    // The owner's message stays where they are writing, with the keyboard; what they type goes on
    // going to it.
    assert_eq!(pane(&h), Some(mine));
    assert!(h.ctx.memory(|m| m.has_focus(crate::compose::editor::id_of(mine))));
    assert_ne!(crate::keys::active(&h.ctx), crate::windows::embedded_layer(w));
    type_text(&mut h, " and more");
    assert!(text_of(&h, mine).starts_with("Mine and more"), "{}", text_of(&h, mine));
    assert!(!text_of(&h, theirs).contains("more"), "{}", text_of(&h, theirs));

    // A reply of an agent's too.
    let message = h.state().session.mailbox.messages.values().find(|m| !m.draft).map(|m| m.id).unwrap();
    let said = agent(h.state_mut(), "mail.reply", json!({"id": message}), Class::ALL.to_vec()).unwrap();
    let reply = said["result"]["compose"].as_u64().unwrap();
    h.run_steps(3);
    assert!(h.state().windows.of_compose(reply).is_some() && pane(&h) == Some(mine));
}

#[test]
fn at_the_bound_a_reply_stays_in_the_reading_pane_and_new_email_is_refused() {
    let mut h = window(app());
    let messages: Vec<Id> = h.state().session.mailbox.messages.values().filter(|m| !m.draft).map(|m| m.id).take(3).collect();
    for _ in 0..MAX_WINDOWS {
        owner(&mut h, "mail.new", json!({}));
    }
    assert_eq!(h.state().windows.len(), MAX_WINDOWS);
    // Twelve windows and the reading pane are the most the engine holds.
    assert_eq!(MAX_WINDOWS + 1, mailcraft_engine::composing::MAX_COMPOSING);
    // New Email is refused, said in the status bar; nothing opened.
    let count = h.state().session.composing_count();
    let refused = h.state_mut().run("mail.new", json!({})).unwrap_err();
    assert!(refused.contains("Too many windows"), "{refused}");
    assert!(h.state().status_msg.as_ref().is_some_and(|(said, _)| said.contains("Too many windows")));
    assert_eq!(h.state().session.composing_count(), count);
    // A reply, even with the switch to open them in windows, goes to the reading pane.
    h.state_mut().ui.replies_in_window = true;
    let reply = owner(&mut h, "mail.reply", json!({"id": messages[0]}))["compose"].as_u64().unwrap();
    assert_eq!((pane(&h), h.state().windows.of_compose(reply)), (Some(reply), None));
    // And the engine agrees that is all there can be.
    let refused = h.state_mut().run("mail.reply", json!({"id": messages[1]})).unwrap_err();
    assert!(refused.contains("13"), "{refused}");
    // A message window is a window too: one closed, there is room for another of any kind.
    let first = h.state().session.all_composing().map(|c| c.id).find(|c| h.state().windows.of_compose(*c).is_some()).unwrap();
    script(&mut h, "ui.closeWindow", json!({"compose": first, "discard": true}));
    script(&mut h, "ui.openWindow", json!({"message": messages[2]}));
    assert!(h.state_mut().execute("ui.openWindow", json!({"message": messages[1]})).unwrap_err().contains("Too many windows"));
}

#[test]
fn reply_from_a_messages_window_opens_a_window_of_its_own() {
    let mut h = window(app());
    let message = h.state().session.mailbox.messages.values().find(|m| !m.draft).map(|m| m.id).unwrap();
    script(&mut h, "ui.openWindow", json!({"message": message}));
    for (kind, prefix) in [("windowReply", "RE:"), ("windowForward", "FW:")] {
        let rect = h.state().hit.rect(kind, message).unwrap_or_else(|| panic!("no {kind}"));
        click(&mut h, rect.center());
        let compose = h.state().session.all_composing().find(|c| c.source == Some(message)).map(|c| (c.id, c.subject.clone())).unwrap();
        assert!(compose.1.starts_with(prefix), "{kind}: {}", compose.1);
        assert!(h.state().windows.of_compose(compose.0).is_some(), "{kind}");
        assert_eq!((pane(&h), h.state().ui.tab.as_str()), (None, "Home"), "{kind}");
        script(&mut h, "ui.closeWindow", json!({"compose": compose.0, "discard": true}));
    }
    assert!(h.state().windows.of_message(message).is_some(), "the message's window stays");
}

#[test]
fn the_options_switch_opens_replies_and_forwards_in_a_window() {
    let mut h = window(app());
    let message = h.state().session.mailbox.messages.values().find(|m| !m.draft).map(|m| m.id).unwrap();
    // Off at first: a reply starts in the reading pane, on the Message tab.
    assert!(!crate::UiState::default().replies_in_window);
    let reply = owner(&mut h, "mail.reply", json!({"id": message}))["compose"].as_u64().unwrap();
    assert_eq!((pane(&h), h.state().ui.tab.as_str()), (Some(reply), "Message"));
    script(&mut h, "compose.close", json!({"discard": true}));
    // File › Options › Mail.
    script(&mut h, "ui.backstage", json!({"value": true, "page": "options"}));
    let rect = h.state().hit.rect("optionRepliesInWindow", 0).expect("the switch in File › Options");
    click(&mut h, rect.center());
    assert!(h.state().ui.replies_in_window);
    script(&mut h, "ui.backstage", json!({"value": false}));
    for command in ["mail.reply", "mail.replyAll", "mail.forward"] {
        let compose = owner(&mut h, command, json!({"id": message}))["compose"].as_u64().unwrap();
        assert!(h.state().windows.of_compose(compose).is_some() && pane(&h).is_none(), "{command}");
        script(&mut h, "ui.closeWindow", json!({"compose": compose, "discard": true}));
    }
    // It is kept in ui.json, and turned over by command.
    let saved = serde_json::to_value(&h.state().ui).unwrap();
    assert_eq!(saved["repliesInWindow"], true);
    let read: crate::UiState = serde_json::from_value(json!({"repliesInWindow": true})).unwrap();
    assert!(read.replies_in_window);
    assert_eq!(script(&mut h, "ui.repliesInWindow", json!({})), json!({"value": false}));
    assert!(h.state_mut().execute("ui.repliesInWindow", json!({"value": "on"})).is_err());
}

#[test]
fn a_double_clicked_draft_opens_in_a_window_ready_to_write_in() {
    let mut h = window(app());
    script(&mut h, "mail.new", json!({"to": "maya@lumen.example", "subject": "Unfinished business"}));
    script(&mut h, "compose.saveDraft", json!({}));
    script(&mut h, "compose.close", json!({}));
    let (draft, folder) =
        h.state().session.mailbox.messages.values().find(|m| m.draft && m.subject == "Unfinished business").map(|m| (m.id, m.folder)).unwrap();
    script(&mut h, "folder.select", json!({"id": folder}));
    h.run_steps(40);
    let at = h.state().hit.rect("message", draft).unwrap().center();
    h.event(egui::Event::PointerMoved(at));
    h.step();
    for _ in 0..2 {
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            h.step();
        }
    }
    h.run_steps(3);
    let compose = h.state().session.all_composing().find(|c| c.draft == Some(draft)).map(|c| c.id).expect("it is being written");
    assert!(h.state().windows.of_compose(compose).is_some() && pane(&h).is_none());
    // Again: the same window, not a second copy of the draft.
    let again = script(&mut h, "ui.openWindow", json!({"message": draft}));
    assert_eq!((again["compose"].as_u64(), again["window"].as_u64()), (Some(compose), h.state().windows.of_compose(compose)));
    assert_eq!((h.state().session.composing_count(), h.state().windows.len()), (1, 1));
    // Ready to write in: what is typed goes to it.
    into_body(&mut h, compose);
    type_text(&mut h, "More. ");
    assert!(text_of(&h, compose).starts_with("More. "), "{}", text_of(&h, compose));
    assert!(h.query_all_by_label_contains("Unfinished business - Message").next().is_some());
}
