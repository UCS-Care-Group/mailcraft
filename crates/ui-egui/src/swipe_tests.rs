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
        let scope = Scope { opening: 1, folder: 1, list: egui::Id::unique("test-list") };
        result = super::input(ui, scope, (row, 0), members, true, 400.0, (Action::Delete, Action::Archive));
    });
    output.textures_delta.clear();
    result
}

#[test]
fn modified_end_cancels_a_phased_gesture() {
    let ctx = egui::Context::default();
    let source = MouseWheelSource::Trackpad;
    frame(&ctx, 1.0, 1, vec![wheel(-100.0, 0.0, TouchPhase::Start, source)]);
    let mut end = wheel(0.0, 0.0, TouchPhase::End, source);
    if let Event::MouseWheel { modifiers, .. } = &mut end {
        modifiers.shift = true;
    }
    frame(&ctx, 1.1, 1, vec![end]);
    assert_eq!(frame(&ctx, 1.4, 1, vec![]).completed, None);
    assert_eq!(frame(&ctx, 1.5, 1, vec![wheel(-20.0, 0.0, TouchPhase::Start, source)]).offset, -20.0);
}

#[test]
fn unknown_source_with_phases_waits_for_end_and_missing_end_cancels() {
    let ctx = egui::Context::default();
    frame(&ctx, 1.0, 1, vec![wheel(-100.0, 0.0, TouchPhase::Start, MouseWheelSource::Unknown)]);
    let held = frame(&ctx, 2.0, 1, vec![]);
    assert_eq!(held.offset, -100.0);
    assert_eq!(held.completed, None);
    frame(&ctx, 6.1, 1, vec![]);
    assert_eq!(frame(&ctx, 6.4, 1, vec![]).completed, None);
    assert_eq!(frame(&ctx, 6.5, 1, vec![wheel(-20.0, 0.0, TouchPhase::Start, MouseWheelSource::Unknown)]).offset, -20.0);
}

#[test]
fn a_replaced_mailbox_changed_list_or_missing_row_discards_the_gesture() {
    let ctx = egui::Context::default();
    for change in 0..4 {
        let start =
            egui::RawInput { time: Some(1.0), events: vec![wheel(-100.0, 0.0, TouchPhase::Start, MouseWheelSource::Trackpad)], ..Default::default() };
        let old = Scope { opening: 1, folder: 1, list: egui::Id::unique("list") };
        let mut output = ctx.run_ui(start, |ui| {
            let _ = super::input(ui, old, (1, 0), &[1], true, 400.0, (Action::Delete, Action::Archive));
        });
        output.textures_delta.clear();
        let mut result = Frame::default();
        let input = egui::RawInput { time: Some(2.0), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            let scope = Scope {
                opening: if change == 1 { 2 } else { 1 },
                folder: if change == 2 { 2 } else { 1 },
                list: if change == 3 { egui::Id::unique("other-list") } else { old.list },
            };
            super::reconcile(ui, scope, |_, _| change != 0);
            result = super::input(ui, scope, (1, 0), &[1], false, 400.0, (Action::Delete, Action::Archive));
        });
        output.textures_delta.clear();
        assert_eq!(result.offset, 0.0);
        assert_eq!(result.completed, None);
    }
}

#[test]
fn a_discarded_phased_stream_cannot_restart_until_a_terminal_event_or_fresh_start() {
    for terminal in [TouchPhase::End, TouchPhase::Cancel, TouchPhase::Start] {
        let ctx = egui::Context::default();
        let old = Scope { opening: 1, folder: 1, list: egui::Id::unique("list") };
        let new = Scope { folder: 2, ..old };
        let run = |time, scope, row, events| {
            let mut result = Frame::default();
            let raw = egui::RawInput { time: Some(time), events, ..Default::default() };
            let mut output = ctx.run_ui(raw, |ui| {
                super::reconcile(ui, scope, |_, _| true);
                result = super::input(ui, scope, (row, 0), &[row], true, 400.0, (Action::Delete, Action::Archive));
            });
            output.textures_delta.clear();
            result
        };
        run(1.0, old, 1, vec![wheel(-50.0, 0.0, TouchPhase::Start, MouseWheelSource::Trackpad)]);
        let continued = run(1.1, new, 2, vec![wheel(-90.0, 0.0, TouchPhase::Move, MouseWheelSource::Trackpad)]);
        assert_eq!(continued.offset, 0.0);
        assert_eq!(run(1.2, new, 2, vec![wheel(-90.0, 0.0, TouchPhase::Move, MouseWheelSource::Trackpad)]).completed, None);
        let terminal_delta = if terminal == TouchPhase::Start { -20.0 } else { 0.0 };
        let result = run(1.3, new, 2, vec![wheel(terminal_delta, 0.0, terminal, MouseWheelSource::Trackpad)]);
        assert_eq!(result.offset, terminal_delta);
        if terminal != TouchPhase::Start {
            assert_eq!(run(1.4, new, 2, vec![wheel(-20.0, 0.0, TouchPhase::Start, MouseWheelSource::Trackpad)]).offset, -20.0);
        }
    }
}

#[test]
fn unphased_input_can_start_in_a_new_scope_after_a_scope_change() {
    let ctx = egui::Context::default();
    let old = Scope { opening: 1, folder: 1, list: egui::Id::unique("list") };
    let new = Scope { folder: 2, ..old };
    for (time, scope, row) in [(1.0, old, 1), (1.1, new, 2)] {
        let mut result = Frame::default();
        let raw =
            egui::RawInput { time: Some(time), events: vec![wheel(-30.0, 0.0, TouchPhase::Move, MouseWheelSource::Unknown)], ..Default::default() };
        let mut output = ctx.run_ui(raw, |ui| {
            super::reconcile(ui, scope, |_, _| true);
            result = super::input(ui, scope, (row, 0), &[row], true, 400.0, (Action::Delete, Action::Archive));
        });
        output.textures_delta.clear();
        assert_eq!(result.offset, -30.0);
    }
}

#[test]
fn terminal_event_does_not_let_another_row_replay_the_same_frames_movement() {
    let ctx = egui::Context::default();
    let old = Scope { opening: 1, folder: 1, list: egui::Id::unique("list") };
    let new = Scope { folder: 2, ..old };
    let mut start = ctx.run_ui(
        egui::RawInput { time: Some(1.0), events: vec![wheel(-30.0, 0.0, TouchPhase::Start, MouseWheelSource::Trackpad)], ..Default::default() },
        |ui| {
            let _ = super::input(ui, old, (1, 0), &[1], true, 400.0, (Action::Delete, Action::Archive));
        },
    );
    start.textures_delta.clear();
    let raw = egui::RawInput {
        time: Some(1.1),
        events: vec![wheel(0.0, 0.0, TouchPhase::End, MouseWheelSource::Trackpad), wheel(-100.0, 0.0, TouchPhase::Move, MouseWheelSource::Trackpad)],
        ..Default::default()
    };
    let mut result = Frame::default();
    let mut output = ctx.run_ui(raw, |ui| {
        super::reconcile(ui, new, |_, _| true);
        let _ = super::input(ui, new, (2, 0), &[2], false, 400.0, (Action::Delete, Action::Archive));
        result = super::input(ui, new, (3, 0), &[3], true, 400.0, (Action::Delete, Action::Archive));
    });
    output.textures_delta.clear();
    assert_eq!(result.offset, 0.0);
    assert_eq!(result.completed, None);
}

#[test]
fn a_settled_gesture_cannot_replay_its_events_on_another_row() {
    for phase in [TouchPhase::End, TouchPhase::Cancel] {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        frame(&ctx, 1.0, 1, vec![wheel(-20.0, 0.0, TouchPhase::Start, MouseWheelSource::Trackpad)]);
        let raw = egui::RawInput {
            time: Some(1.1),
            events: vec![wheel(-90.0, 0.0, TouchPhase::Move, MouseWheelSource::Trackpad), wheel(0.0, 0.0, phase, MouseWheelSource::Trackpad)],
            ..Default::default()
        };
        let mut first = Frame::default();
        let mut second = Frame::default();
        let mut output = ctx.run_ui(raw, |ui| {
            let scope = Scope { opening: 1, folder: 1, list: egui::Id::unique("test-list") };
            first = super::input(ui, scope, (1, 0), &[1], false, 400.0, (Action::Delete, Action::Archive));
            second = super::input(ui, scope, (2, 0), &[2], true, 400.0, (Action::Delete, Action::Archive));
        });
        output.textures_delta.clear();
        assert_eq!(first.completed, if phase == TouchPhase::End { Some((Action::Delete, vec![1])) } else { None });
        assert_eq!(second.offset, 0.0);
        assert_eq!(second.completed, None);
    }
}

#[test]
fn a_cancelled_stream_can_end_while_the_list_is_hidden() {
    let ctx = egui::Context::default();
    let old = Scope { opening: 1, folder: 1, list: egui::Id::unique("list") };
    let new = Scope { folder: 2, ..old };
    let mut output = ctx.run_ui(
        egui::RawInput { time: Some(1.0), events: vec![wheel(-30.0, 0.0, TouchPhase::Start, MouseWheelSource::Trackpad)], ..Default::default() },
        |ui| {
            let _ = super::input(ui, old, (1, 0), &[1], true, 400.0, (Action::Delete, Action::Archive));
        },
    );
    output.textures_delta.clear();
    let mut output = ctx.run_ui(egui::RawInput { time: Some(1.1), ..Default::default() }, |ui| {
        super::reconcile(ui, new, |_, _| false);
    });
    output.textures_delta.clear();
    let mut output = ctx.run_ui(
        egui::RawInput { time: Some(1.2), events: vec![wheel(0.0, 0.0, TouchPhase::End, MouseWheelSource::Trackpad)], ..Default::default() },
        |ui| {
            super::observe_cancelled(ui.ctx());
        },
    );
    output.textures_delta.clear();
    let mut result = Frame::default();
    let mut output = ctx.run_ui(
        egui::RawInput { time: Some(1.3), events: vec![wheel(-30.0, 0.0, TouchPhase::Move, MouseWheelSource::Unknown)], ..Default::default() },
        |ui| {
            result = super::input(ui, new, (2, 0), &[2], true, 400.0, (Action::Delete, Action::Archive));
        },
    );
    output.textures_delta.clear();
    assert_eq!(result.offset, -30.0);
}

#[test]
fn terminal_event_while_the_original_list_is_hidden_does_not_block_later_unphased_input() {
    let ctx = egui::Context::default();
    let old = Scope { opening: 1, folder: 1, list: egui::Id::unique("list") };
    let new = Scope { folder: 2, ..old };
    let mut output = ctx.run_ui(
        egui::RawInput { time: Some(1.0), events: vec![wheel(-30.0, 0.0, TouchPhase::Start, MouseWheelSource::Trackpad)], ..Default::default() },
        |ui| {
            let _ = super::input(ui, old, (1, 0), &[1], true, 400.0, (Action::Delete, Action::Archive));
        },
    );
    output.textures_delta.clear();
    let mut output = ctx.run_ui(
        egui::RawInput { time: Some(1.1), events: vec![wheel(0.0, 0.0, TouchPhase::End, MouseWheelSource::Trackpad)], ..Default::default() },
        |ui| {
            super::observe_cancelled(ui.ctx());
        },
    );
    output.textures_delta.clear();
    let mut result = Frame::default();
    let mut output = ctx.run_ui(
        egui::RawInput { time: Some(1.2), events: vec![wheel(-30.0, 0.0, TouchPhase::Move, MouseWheelSource::Unknown)], ..Default::default() },
        |ui| {
            super::reconcile(ui, new, |_, _| true);
            result = super::input(ui, new, (2, 0), &[2], true, 400.0, (Action::Delete, Action::Archive));
        },
    );
    output.textures_delta.clear();
    assert_eq!(result.offset, -30.0);
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
fn swipe_selectors_have_distinct_accessible_names() {
    use crate::{MailApp, Services};
    use egui_kittest::kittest::Queryable as _;
    use mailcraft_engine::Session;
    use serde_json::json;

    let now = 1_791_993_600;
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(now, 0), now, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
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
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    w.run_steps(4);
    for name in ["Swipe to the left", "Swipe to the right"] {
        assert!(w.query_by_role_and_label(egui::accesskit::Role::ComboBox, name).is_some(), "{name}");
    }
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

#[test]
fn leaving_the_folder_cancels_a_gesture_before_the_row_can_reappear() {
    use crate::{MailApp, Services};
    use mailcraft_engine::Session;
    use serde_json::json;

    let now = 1_791_993_600;
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(now, 0), now, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.session.prefs.mark_read_on_open = false;
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
    let rect = w.state().hit.rect("message", row.id).unwrap();
    w.event(Event::PointerMoved(rect.center()));
    w.step();
    w.event(wheel(-100.0, 0.0, TouchPhase::Start, MouseWheelSource::Trackpad));
    w.step();
    w.state_mut().execute("folder.select", json!({"kind": "drafts"})).unwrap();
    w.run_steps(3);
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    w.run_steps(3);
    w.event(wheel(0.0, 0.0, TouchPhase::End, MouseWheelSource::Trackpad));
    w.run_steps(40);
    assert!(row.members.iter().any(|id| w.state().session.mailbox.messages.get(id).is_some_and(|message| !message.read)));
}

#[test]
fn a_swipe_continued_over_a_draft_after_changing_folders_does_not_delete_it() {
    use crate::{MailApp, Services};
    use mailcraft_engine::Session;
    use serde_json::json;

    let now = 1_791_993_600;
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(now, 0), now, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.ui.swipe_left = Action::Delete;
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
    let inbox_rows = w.state_mut().session.rows();
    let inbox = inbox_rows.iter().find_map(|row| w.state().hit.rect("message", row.id)).unwrap();
    w.event(Event::PointerMoved(inbox.center()));
    w.step();
    w.event(wheel(-30.0, 0.0, TouchPhase::Start, MouseWheelSource::Trackpad));
    w.step();
    w.state_mut().execute("folder.select", json!({"kind": "drafts"})).unwrap();
    w.run_steps(3);
    let draft_rows = w.state_mut().session.rows();
    let draft = draft_rows.iter().find(|row| w.state().hit.rect("message", row.id).is_some()).unwrap().clone();
    let original_folder = w.state().session.mailbox.messages.get(&draft.id).unwrap().folder;
    let draft_rect = w.state().hit.rect("message", draft.id).unwrap();
    w.event(Event::PointerMoved(draft_rect.center()));
    w.step();
    w.event(wheel(-90.0, 0.0, TouchPhase::Move, MouseWheelSource::Trackpad));
    w.step();
    w.event(wheel(0.0, 0.0, TouchPhase::End, MouseWheelSource::Trackpad));
    w.run_steps(40);
    assert_eq!(w.state().session.mailbox.messages.get(&draft.id).map(|message| message.folder), Some(original_folder));
}
