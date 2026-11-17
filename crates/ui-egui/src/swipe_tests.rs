// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

fn wheel(x: f32, y: f32, phase: TouchPhase, source: MouseWheelSource) -> Event {
    Event::MouseWheel { unit: MouseWheelUnit::Point, delta: egui::vec2(x, y), phase, source, modifiers: egui::Modifiers::NONE }
}

fn frame(ctx: &egui::Context, time: f64, row: u64, events: Vec<Event>) -> Frame {
    frame_with_members(ctx, time, row, &[row], events)
}

fn frame_with_members(ctx: &egui::Context, time: f64, row: u64, members: &[u64], events: Vec<Event>) -> Frame {
    let mut result = Frame::default();
    let input = egui::RawInput { time: Some(time), events, ..Default::default() };
    let mut output = ctx.run_ui(input, |ui| {
        result = super::input(ui, row, 0, members, true, 400.0, (Action::Delete, Action::Archive));
    });
    output.textures_delta.clear();
    result
}

#[test]
fn a_conversation_changed_during_animation_keeps_the_original_targets() {
    let ctx = egui::Context::default();
    let source = MouseWheelSource::Trackpad;
    frame_with_members(&ctx, 1.0, 1, &[1, 2], vec![wheel(-100.0, 0.0, TouchPhase::Start, source)]);
    frame_with_members(&ctx, 1.1, 1, &[1, 2, 3], vec![wheel(0.0, 0.0, TouchPhase::End, source)]);
    assert_eq!(frame_with_members(&ctx, 1.4, 1, &[1, 2, 3], vec![]).completed, Some((Action::Delete, vec![1, 2])));
}

#[test]
fn gesture_follows_input_and_fires_once_after_release() {
    let ctx = egui::Context::default();
    let trackpad = MouseWheelSource::Trackpad;
    let first = frame(&ctx, 1.0, 1, vec![wheel(-40.0, 1.0, TouchPhase::Start, trackpad)]);
    assert_eq!(first.offset, -40.0);
    assert_eq!(first.completed, None);
    let next = frame(&ctx, 1.1, 1, vec![wheel(-45.0, 1.0, TouchPhase::Move, trackpad)]);
    assert_eq!(next.offset, -85.0);
    assert_eq!(next.completed, None);
    assert_eq!(frame(&ctx, 1.15, 2, vec![wheel(-100.0, 0.0, TouchPhase::Move, trackpad)]).completed, None);
    let release = frame(&ctx, 1.2, 1, vec![wheel(0.0, 0.0, TouchPhase::End, trackpad)]);
    assert_eq!(release.completed, None);
    let in_flight = frame(&ctx, 1.25, 1, vec![]);
    assert!(in_flight.offset < -85.0);
    assert_eq!(in_flight.completed, None);
    assert_eq!(frame(&ctx, 1.5, 1, vec![]).completed, Some((Action::Delete, vec![1])));
    assert_eq!(frame(&ctx, 1.6, 1, vec![]).completed, None);
}

#[test]
fn short_and_cancelled_gestures_return_without_acting() {
    let ctx = egui::Context::default();
    let source = MouseWheelSource::Trackpad;
    assert_eq!(frame(&ctx, 1.0, 1, vec![wheel(40.0, 0.0, TouchPhase::Start, source)]).offset, 40.0);
    frame(&ctx, 1.1, 1, vec![wheel(0.0, 0.0, TouchPhase::End, source)]);
    let returning = frame(&ctx, 1.15, 1, vec![]);
    assert!(returning.offset > 0.0 && returning.offset < 40.0);
    assert_eq!(frame(&ctx, 1.4, 1, vec![]).completed, None);
    frame(&ctx, 2.0, 1, vec![wheel(-100.0, 0.0, TouchPhase::Start, source)]);
    frame(&ctx, 2.1, 1, vec![wheel(0.0, 0.0, TouchPhase::Cancel, source)]);
    assert_eq!(frame(&ctx, 2.4, 1, vec![]).completed, None);
}

#[test]
fn phased_gesture_waits_for_release_and_unphased_input_finishes_after_idle() {
    let ctx = egui::Context::default();
    frame(&ctx, 1.0, 1, vec![wheel(-100.0, 0.0, TouchPhase::Start, MouseWheelSource::Trackpad)]);
    let held = frame(&ctx, 2.0, 1, vec![]);
    assert_eq!(held.offset, -100.0);
    assert_eq!(held.completed, None);
    frame(&ctx, 2.1, 1, vec![wheel(0.0, 0.0, TouchPhase::Cancel, MouseWheelSource::Trackpad)]);
    frame(&ctx, 2.4, 1, vec![]);
    frame(&ctx, 3.0, 1, vec![wheel(100.0, 0.0, TouchPhase::Move, MouseWheelSource::Unknown)]);
    frame(&ctx, 3.2, 1, vec![]);
    assert_eq!(frame(&ctx, 3.5, 1, vec![]).completed, Some((Action::Archive, vec![1])));
}

#[test]
fn vertical_scroll_wheel_momentum_do_not_move_a_row() {
    for (x, y, source) in [(90.0, 100.0, MouseWheelSource::Trackpad), (120.0, 0.0, MouseWheelSource::Wheel), (120.0, 0.0, MouseWheelSource::Momentum)]
    {
        let result = frame(&egui::Context::default(), 1.0, 1, vec![wheel(x, y, TouchPhase::Start, source)]);
        assert_eq!(result.offset, 0.0);
        assert_eq!(result.completed, None);
    }
}

#[test]
fn windows_scroll_lines_are_accepted_only_when_the_source_is_unknown() {
    assert_eq!(scale(MouseWheelUnit::Line, MouseWheelSource::Unknown, true), Some(40.0));
    assert_eq!(scale(MouseWheelUnit::Line, MouseWheelSource::Unknown, false), None);
    assert_eq!(scale(MouseWheelUnit::Line, MouseWheelSource::Wheel, true), None);
    assert_eq!(scale(MouseWheelUnit::Point, MouseWheelSource::Momentum, true), None);
}

#[test]
fn choices_round_trip_and_unknown_codes_are_refused() {
    for action in Action::ALL {
        assert_eq!(Action::from_code(action.code()), Some(action));
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), action);
    }
    assert_eq!(Action::from_code("other"), None);
    let ui: crate::UiState = serde_json::from_str("{}").unwrap();
    assert_eq!((ui.swipe_left, ui.swipe_right), (Action::None, Action::None));
    let ui: crate::UiState = serde_json::from_str(r#"{"swipeLeft":"futureAction"}"#).unwrap();
    assert_eq!(ui.swipe_left, Action::None);
}

#[test]
fn swipe_marks_the_hovered_message_not_the_selected_one() {
    use crate::{MailApp, Services};
    use mailcraft_engine::Session;
    use serde_json::json;

    let now = 1_791_993_600;
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(now, 0), now, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.ui.swipe_left = Action::Read;
    let mut w =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    w.run_steps(4);
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.run_steps(3);
    let rows = w.state_mut().session.rows();
    let row = rows.iter().find(|r| r.unread > 0 && w.state().hit.rect("message", r.id).is_some()).unwrap().clone();
    let other = rows.iter().find(|r| r.id != row.id).unwrap().id;
    w.state_mut().execute("mail.select", json!({"id": other})).unwrap();
    w.run_steps(2);
    let rect = w.state().hit.rect("message", row.id).unwrap();
    w.event(Event::PointerMoved(rect.center()));
    w.step();
    w.event(wheel(-100.0, 0.0, TouchPhase::Start, MouseWheelSource::Trackpad));
    w.step();
    w.event(wheel(0.0, 0.0, TouchPhase::End, MouseWheelSource::Trackpad));
    w.run_steps(40);
    assert!(row.members.iter().all(|id| w.state().session.mailbox.messages.get(id).is_some_and(|message| message.read)));
    assert!(w.state_mut().session.acting_on().contains(&other));
}
