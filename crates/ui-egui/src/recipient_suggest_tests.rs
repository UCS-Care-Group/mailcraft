// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #308: a recipient line (a new message's To, Cc and Bcc, a meeting's attendees) asks for the
//! people it offers when what is typed or the mailbox changes, not on every frame it is drawn.

use egui_kittest::Harness;
use mailcraft_engine::Session;
use mailcraft_engine::cmd::compose::suggest;
use mailcraft_model::{Contact, Labeled};
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::forms::Form;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The sample mailbox in the window, after `command`.
fn window(command: &str, params: Value) -> Harness<'static, MailApp> {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.execute(command, params).unwrap();
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(6);
    harness
}

fn typed(h: &mut Harness<'static, MailApp>, text: &str) {
    h.event(egui::Event::Text(text.into()));
    h.run_steps(3);
}

fn press(h: &mut Harness<'static, MailApp>, key: egui::Key) {
    h.key_press(key);
    h.run_steps(4);
}

/// Types "zed", which is nobody, then puts two Zeds among the contacts behind the session's
/// back, and tells it only when `told`; a few frames go by.
fn zeds_come(h: &mut Harness<'static, MailApp>, told: bool) {
    typed(h, "zed");
    let s = &mut h.state_mut().session;
    assert!(suggest(s, "zed", 6).is_empty(), "nobody before");
    for (name, email) in [("Zed One", "zed1@example.org"), ("Zed Two", "zed2@example.org")] {
        s.mailbox.add_contact(Contact { display_name: name.into(), emails: vec![Labeled::new("work", email)], ..Default::default() });
    }
    if told {
        s.mailbox_changed();
    }
    h.run_steps(6);
}

/// A new message's To line asks once for what is typed: the frames after it ask nothing, so a
/// contact the session wasn't told of isn't seen, and Enter takes nobody. Once the mailbox
/// changes, it asks again, and Enter takes the first Zed without another key.
#[test]
fn the_to_line_asks_once_for_what_is_typed() {
    for told in [false, true] {
        let mut h = window("mail.new", json!({}));
        zeds_come(&mut h, told);
        press(&mut h, egui::Key::Enter);
        let to = h.state().session.compose.as_ref().map(|c| c.to.clone());
        assert_eq!(to.as_deref(), Some(if told { "Zed One <zed1@example.org>; " } else { "zed" }), "told: {told}");
    }
}

/// Left (the keyboard taken elsewhere, as by a click) and come back to, the To line offers again
/// what it offered: Enter takes it.
#[test]
fn the_to_line_offers_again_when_it_is_come_back_to() {
    let mut h = window("mail.new", json!({}));
    typed(&mut h, "pri");
    let line = h.ctx.memory(|m| m.focused()).unwrap();
    h.ctx.memory_mut(|m| m.surrender_focus(line));
    h.run_steps(4);
    h.ctx.memory_mut(|m| m.request_focus(line));
    h.run_steps(4);
    press(&mut h, egui::Key::Enter);
    let to = h.state().session.compose.as_ref().map(|c| c.to.clone());
    assert_eq!(to.as_deref(), Some("Priya Nair <priya@lumen.example>; "));
}

/// A meeting's Required line does the same: Tab takes nobody (and "zed", which names two
/// contacts, stays as typed) until the mailbox changes, and then the first Zed.
#[test]
fn an_attendee_line_asks_once_for_what_is_typed() {
    for told in [false, true] {
        let mut h = window("ui.dialog", json!({"name": "meeting"}));
        // From the title to the Required line.
        press(&mut h, egui::Key::Tab);
        zeds_come(&mut h, told);
        press(&mut h, egui::Key::Tab);
        let Some(Dialog::Form(Form::Event(form))) = &h.state().dialog else { panic!("the meeting's form is open") };
        let attendees = form.command().1["attendees"].clone();
        assert_eq!(attendees, json!(if told { "Zed One <zed1@example.org>; " } else { "zed" }), "told: {told}");
    }
}

/// Shift+Tab, with the list showing, goes back from the line as it does without the list: it
/// takes nobody, in a new message's To line and in a meeting's Required line alike.
#[test]
fn shift_tab_takes_nobody() {
    let back = |h: &mut Harness<'static, MailApp>| {
        let line = h.ctx.memory(|m| m.focused());
        h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
        h.run_steps(4);
        assert!(line.is_some() && h.ctx.memory(|m| m.focused()) != line, "the keyboard left the line");
    };
    let mut h = window("mail.new", json!({}));
    zeds_come(&mut h, true);
    back(&mut h);
    assert_eq!(h.state().session.compose.as_ref().map(|c| c.to.as_str()), Some("zed"));

    let mut h = window("ui.dialog", json!({"name": "meeting"}));
    press(&mut h, egui::Key::Tab);
    zeds_come(&mut h, true);
    back(&mut h);
    let Some(Dialog::Form(Form::Event(form))) = &h.state().dialog else { panic!("the meeting's form is open") };
    assert_eq!(form.command().1["attendees"], json!("zed"));
}
