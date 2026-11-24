// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Correct the phase winit reports for discrete wheel input on Linux before egui smooths it.

#[cfg(test)]
#[path = "wheel_input_tests.rs"]
mod tests;

/// Wayland can report `TouchPhase::Start` for a mouse wheel without a later `End`. egui treats
/// that as an active touchpad gesture and skips smoothing; line-unit events are discrete wheels,
/// so let egui handle them as ordinary scroll steps. Point-unit touchpad deltas stay untouched.
pub(crate) fn smooth_discrete_wheel_events(events: &mut [egui::Event]) {
    for event in events {
        if let egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            phase,
            ..
        } = event
            && *phase == egui::TouchPhase::Start
        {
            *phase = egui::TouchPhase::Move;
        }
    }
}
