// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use egui::{Event, MouseWheelUnit, RawInput, TouchPhase, Vec2, vec2};

fn wheel(unit: MouseWheelUnit, phase: TouchPhase) -> Event {
    Event::MouseWheel {
        unit,
        delta: vec2(0.0, -1.0),
        phase,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    }
}

fn first_frame_scroll(event: Event) -> Vec2 {
    let ctx = egui::Context::default();
    let input = RawInput { time: Some(0.0), events: vec![event], ..Default::default() };
    let mut delta = Vec2::ZERO;
    ctx.run_ui(input, |ui| {
        delta = ui.input(|input| input.smooth_scroll_delta);
    })
    .drop_without_applying_deltas();
    delta
}

#[test]
fn discrete_wheel_move_is_smoothed_and_touchpad_phases_are_unchanged() {
    // winit 0.30's Wayland pointer handler reports discrete scroll as LineDelta with Moved phase.
    let mut events = [
        wheel(MouseWheelUnit::Line, TouchPhase::Move),
        wheel(MouseWheelUnit::Line, TouchPhase::Start),
        wheel(MouseWheelUnit::Point, TouchPhase::Start),
        wheel(MouseWheelUnit::Point, TouchPhase::Move),
    ];

    super::smooth_discrete_wheel_events(&mut events);

    assert!(matches!(
        &events[0],
        Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            phase: TouchPhase::Move,
            ..
        }
    ));
    // Even a synthetic/unexpected Start is no longer rewritten.
    assert!(matches!(
        &events[1],
        Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            phase: TouchPhase::Start,
            ..
        }
    ));
    assert!(matches!(
        &events[2],
        Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            phase: TouchPhase::Start,
            ..
        }
    ));
    assert!(matches!(
        &events[3],
        Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            phase: TouchPhase::Move,
            ..
        }
    ));

    let delta = first_frame_scroll(wheel(MouseWheelUnit::Line, TouchPhase::Move));
    assert!(delta.y < 0.0 && delta.y > -40.0, "the first frame applied {delta:?}");
}

#[test]
fn small_touchpad_deltas_remain_immediate() {
    let delta = first_frame_scroll(wheel(MouseWheelUnit::Point, TouchPhase::Move));
    assert_eq!(delta, vec2(0.0, -1.0));
}
