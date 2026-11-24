// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Linux's Wayland discrete wheel producer already emits `Line` events with `Moved` phases.
//! Keep this hook as an explicit boundary for platform-specific input handling; no phase rewrite
//! is needed. Point-based touchpad events pass through unchanged as well.

#[cfg(test)]
#[path = "wheel_input_tests.rs"]
mod tests;

/// Wayland discrete line-wheel events already arrive with `TouchPhase::Move` from winit.
/// Do not rewrite `Start`: the producer does not emit it for discrete wheel events.
pub(crate) fn smooth_discrete_wheel_events(_events: &mut [egui::Event]) {}
