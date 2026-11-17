// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Configurable horizontal gestures on message rows.

use egui::{Event, MouseWheelSource, MouseWheelUnit, TouchPhase, Vec2};
use serde::{Deserialize, Serialize};

#[cfg(test)]
#[path = "swipe_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Delete,
    Archive,
    Read,
    Flag,
    #[default]
    #[serde(other)]
    None,
}

impl Action {
    pub(crate) const ALL: [Self; 5] = [Self::None, Self::Delete, Self::Archive, Self::Read, Self::Flag];
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Delete => "delete",
            Self::Archive => "archive",
            Self::Read => "read",
            Self::Flag => "flag",
        }
    }
    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.code() == code)
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => tl!("None"),
            Self::Delete => tl!("Delete"),
            Self::Archive => tl!("Archive"),
            Self::Read => tl!("Mark as read"),
            Self::Flag => tl!("Flag"),
        }
    }
    pub(crate) fn command(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Delete => Some("mail.delete"),
            Self::Archive => Some("mail.archive"),
            Self::Read => Some("mail.markRead"),
            Self::Flag => Some("mail.toggleFlag"),
        }
    }
}

/// The gesture stays on the row where it began, including while it settles.
#[derive(Clone, Default)]
struct Gesture {
    active: bool,
    row: u64,
    copy: u16,
    members: Vec<u64>,
    has_phases: bool,
    delta: Vec2,
    last: f64,
    settling: bool,
    settled_at: f64,
    from: f32,
    action: Action,
}

#[derive(Default)]
pub(crate) struct Frame {
    pub offset: f32,
    pub action: Action,
    pub completed: Option<(Action, Vec<u64>)>,
}

fn scale(unit: MouseWheelUnit, source: MouseWheelSource, windows: bool) -> Option<f32> {
    if !matches!(source, MouseWheelSource::Trackpad | MouseWheelSource::Unknown) {
        return None;
    }
    match unit {
        MouseWheelUnit::Point => Some(1.0),
        // Windows does not distinguish precision touchpads from horizontal mouse wheels.
        MouseWheelUnit::Line if windows && source == MouseWheelSource::Unknown => Some(40.0),
        _ => None,
    }
}

/// Follow the input while fingers are down, then settle back or slide away before acting.
/// A short idle ends input on platforms that report no end phase (notably Windows).
pub(crate) fn input(ui: &egui::Ui, row: u64, copy: u16, members: &[u64], hovered: bool, width: f32, actions: (Action, Action)) -> Frame {
    let width = if width.is_finite() { width.clamp(0.0, 100_000.0) } else { 0.0 };
    let id = egui::Id::unique("message_swipe_gesture");
    let (events, now) = ui.input(|i| (i.events.clone(), i.time));
    let mut state = ui.ctx().data_mut(|data| data.get_temp::<Gesture>(id).unwrap_or_default());
    let mut end = false;
    let mut cancel = false;
    for event in events {
        let Event::MouseWheel { unit, mut delta, phase, source, modifiers } = event else { continue };
        if modifiers != egui::Modifiers::NONE {
            continue;
        }
        let Some(scale) = scale(unit, source, cfg!(target_os = "windows")) else { continue };
        delta *= scale;
        // If its row vanished, a later gesture can start elsewhere without acting on it.
        if state.active && hovered && now - state.last > 1.0 && (state.row != row || state.copy != copy) {
            state = Gesture::default();
        }
        if !state.active && hovered && phase != TouchPhase::End && phase != TouchPhase::Cancel {
            state = Gesture {
                active: true,
                row,
                copy,
                members: members.to_vec(),
                has_phases: source == MouseWheelSource::Trackpad && phase == TouchPhase::Start,
                last: now,
                ..Default::default()
            };
        }
        if !state.active || state.row != row || state.copy != copy || state.settling {
            continue;
        }
        match phase {
            TouchPhase::Cancel => {
                cancel = true;
                end = true;
            }
            TouchPhase::End => end = true,
            _ => {
                state.last = now;
                if delta.x.is_finite() && delta.y.is_finite() {
                    state.delta.x = (state.delta.x + delta.x).clamp(-width, width);
                    state.delta.y = (state.delta.y + delta.y).clamp(-width, width);
                }
            }
        }
    }
    if !state.active || state.row != row || state.copy != copy {
        return Frame::default();
    }
    let horizontal = state.delta.x.abs() > state.delta.y.abs() * 2.0;
    let action = if state.delta.x < 0.0 { actions.0 } else { actions.1 };
    let offset = if horizontal && action != Action::None { state.delta.x.clamp(-width * 0.85, width * 0.85) } else { 0.0 };
    if !state.settling && (end || (!state.has_phases && now - state.last >= 0.18)) {
        state.settling = true;
        state.settled_at = now;
        state.from = offset;
        state.action = if !cancel && horizontal && state.delta.x.abs() >= 80.0 { action } else { Action::None };
    }
    let mut frame = Frame { offset, action, completed: None };
    if state.settling {
        let duration = f64::from(crate::motion::fade(ui, 0.18));
        let progress = if duration > 0.0 { ((now - state.settled_at) / duration).clamp(0.0, 1.0) as f32 } else { 1.0 };
        let eased = 1.0 - (1.0 - progress).powi(3);
        let target = if matches!(state.action, Action::Delete | Action::Archive) { state.from.signum() * width } else { 0.0 };
        frame.offset = state.from + (target - state.from) * eased;
        if progress >= 1.0 {
            frame.completed = state.action.command().map(|_| (state.action, state.members.clone()));
            state = Gesture::default();
        }
    }
    if state.active {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
    }
    ui.ctx().data_mut(|data| data.insert_temp(id, state));
    frame
}
