// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The keyboard scrolls the reading pane once the pointer went down in it (#1739): the arrows,
//! Page Up and Down, Space and Home and End move the message, not the selection in the list.

use egui::{Key, Modifiers, Pos2};
use mailcraft_engine::Session;
use serde_json::json;

use super::tests::{select, window};
use crate::{MailApp, Services};

type Window = egui_kittest::Harness<'static, MailApp>;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The window on the sample mailbox, its first message made long and shown in the reading pane.
fn long_message() -> (Window, u64) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let id = app.session.rows().first().map(|r| r.id).unwrap();
    let m = std::sync::Arc::make_mut(app.session.mailbox.messages.get_mut(&id).unwrap());
    m.body.html = None;
    m.body.text = (1..=300).map(|n| format!("Line {n} of a long message.\n")).collect();
    let mut harness = window(app, (1280.0, 900.0));
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    select(&mut harness, id);
    (harness, id)
}

/// Where the top of the message's body is on the screen.
fn top(harness: &Window, id: u64) -> f32 {
    super::view::kept(&harness.ctx, id, |_, origin| origin.y).unwrap()
}

fn click(harness: &mut Window, at: Pos2) {
    harness.event(egui::Event::PointerMoved(at));
    harness.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    harness.step();
    harness.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    harness.run_steps(2);
}

fn press(harness: &mut Window, key: Key) {
    harness.key_press(key);
    harness.run_steps(3);
}

fn selected(harness: &Window) -> Vec<u64> {
    harness.state().session.sel.messages.to_vec()
}

#[test]
fn after_a_click_in_the_message_the_keys_scroll_it() {
    let (mut harness, id) = long_message();
    let start = top(&harness, id);
    let at = super::view::kept(&harness.ctx, id, |_, origin| origin + egui::vec2(60.0, 40.0)).unwrap();
    click(&mut harness, at);
    let picked = selected(&harness);
    press(&mut harness, Key::ArrowDown);
    let line = start - top(&harness, id);
    assert!(line > 10.0, "Down scrolls a line: {line}");
    press(&mut harness, Key::PageDown);
    let page = start - top(&harness, id) - line;
    assert!(page > 300.0, "Page Down scrolls about what is seen: {page}");
    press(&mut harness, Key::Space);
    assert!(start - top(&harness, id) > line + page + 300.0, "Space pages down too");
    press(&mut harness, Key::End);
    let end = start - top(&harness, id);
    assert!(end > line + 2.0 * page + 1000.0, "End goes to the end: {end}");
    press(&mut harness, Key::PageUp);
    assert!(start - top(&harness, id) < end - 300.0, "Page Up goes back");
    press(&mut harness, Key::Home);
    assert!((top(&harness, id) - start).abs() < 0.5, "Home is the top");
    assert_eq!(selected(&harness), picked, "the list's selection never moved");
}

#[test]
fn without_a_click_in_the_message_the_keys_are_the_lists() {
    let (mut harness, id) = long_message();
    press(&mut harness, Key::ArrowDown);
    assert_ne!(selected(&harness), vec![id], "Down moves to the next message");
}
