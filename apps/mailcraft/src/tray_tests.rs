// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use egui::{RawInput, ViewportEvent, ViewportId};
use mailcraft_engine::Session;
use mailcraft_model::Mailbox;
use mailcraft_ui_egui::Services;

fn frame(state: &mut WindowState, app: &mut MailApp, close: bool, minimized: bool, action: Option<Action>) -> Vec<V> {
    let ctx = Context::default();
    let (tx, rx) = channel();
    if let Some(action) = action {
        tx.send(action).unwrap();
    }
    let mut input = RawInput::default();
    let root = input.viewports.entry(ViewportId::ROOT).or_default();
    root.minimized = Some(minimized);
    if close {
        root.events.push(ViewportEvent::Close);
    }
    let mut out = ctx.run_logic(&input, |ctx| {
        if !state.on_frame(ctx, app, &rx) {
            crate::quit::on_frame(ctx, app);
        }
        state.after_frame(ctx);
    });
    out.viewport_commands.remove(&ViewportId::ROOT).unwrap_or_default()
}

#[test]
fn close_and_minimize_hide_without_discarding_a_draft_and_show_restores() {
    let mut app = MailApp::new(Session::new(Mailbox::default()), Services::default());
    app.session.run("mail.new", &serde_json::json!({})).unwrap();
    app.session.compose.as_mut().unwrap().dirty = true;
    let mut state = WindowState { hidden: false, start_hidden: false, quitting: false };
    let commands = frame(&mut state, &mut app, true, false, None);
    assert!(commands.contains(&V::CancelClose) && commands.contains(&V::Visible(false)));
    assert!(app.session.compose.as_ref().unwrap().dirty);
    let commands = frame(&mut state, &mut app, false, true, Some(Action::Show));
    assert!(commands.contains(&V::Visible(true)) && commands.contains(&V::Focus));
    assert!(!commands.contains(&V::Visible(false)), "restore wins over a stale minimized event");
    assert!(!state.hidden);
    let commands = frame(&mut state, &mut app, false, true, None);
    assert!(commands.contains(&V::Visible(false)) && !commands.contains(&V::Close));
}

#[test]
fn startup_hides_once_and_explicit_quit_keeps_the_unsaved_work_question() {
    let mut app = MailApp::new(Session::new(Mailbox::default()), Services::default());
    let mut state = WindowState { hidden: false, start_hidden: true, quitting: false };
    assert!(frame(&mut state, &mut app, false, false, None).contains(&V::Visible(false)));
    assert!(!state.start_hidden);
    app.session.run("mail.new", &serde_json::json!({})).unwrap();
    app.session.compose.as_mut().unwrap().dirty = true;
    assert!(frame(&mut state, &mut app, false, false, Some(Action::Quit)).contains(&V::Close));
    assert!(frame(&mut state, &mut app, true, false, None).contains(&V::CancelClose));
    assert!(!state.quitting, "cancelled quit restores close-to-tray behavior");
    assert!(app.session.compose.is_some());
}

#[test]
fn a_control_channel_quit_is_not_swallowed_by_close_to_tray() {
    let mut app = MailApp::new(Session::new(Mailbox::default()), Services::default());
    let mut state = WindowState { hidden: true, start_hidden: false, quitting: false };
    app.quit_requested = true;
    assert!(frame(&mut state, &mut app, false, false, None).contains(&V::Close));
    let commands = frame(&mut state, &mut app, true, false, None);
    assert!(!commands.contains(&V::Visible(false)) && !commands.contains(&V::CancelClose));
}
