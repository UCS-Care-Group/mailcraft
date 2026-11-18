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
    /// A phased stream was cancelled before it ended: ignore its remaining movement.
    discarded: bool,
    /// A terminal event was consumed in this frame; other rows must not replay its events.
    blocked_frame: Option<u64>,
    /// A terminal phase observed even if the originating row is not drawn in that frame.
    terminal_frame: Option<u64>,
    scope: Option<Scope>,
    frame: u64,
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

impl Gesture {
    fn discard(&self) -> Self {
        Self {
            discarded: self.has_phases && (!self.settling || self.discarded) && self.terminal_frame.is_none(),
            blocked_frame: self.terminal_frame,
            ..Default::default()
        }
    }
}

/// The list and mailbox in which a gesture started. IDs are only meaningful in that opening.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scope {
    pub opening: u64,
    pub folder: u64,
    pub list: egui::Id,
}

fn slot() -> egui::Id {
    egui::Id::unique("message_swipe_gesture")
}

/// Cancel a gesture if its list or mailbox changed, the list missed a frame, or its row left view.
pub(crate) fn reconcile(ui: &egui::Ui, scope: Scope, visible: impl FnOnce(u64, u16) -> bool) {
    let frame = ui.ctx().cumulative_frame_nr();
    ui.ctx().data_mut(|data| {
        if let Some(state) = data.get_temp::<Gesture>(slot())
            && state.active
            && (state.scope != Some(scope) || frame.saturating_sub(state.frame) > 1 || !visible(state.row, state.copy))
        {
            data.insert_temp(slot(), state.discard());
        }
    });
    observe_cancelled(ui.ctx());
}

/// A cancelled stream can end while the mail list is hidden or empty.
pub(crate) fn observe_cancelled(ctx: &egui::Context) {
    let ended = ctx.input(|input| {
        input.events.iter().any(|event| {
            matches!(event, Event::MouseWheel { unit, phase: TouchPhase::End | TouchPhase::Cancel, source, .. }
            if scale(*unit, *source, cfg!(target_os = "windows")).is_some())
        })
    });
    if ended {
        let frame = ctx.cumulative_frame_nr();
        ctx.data_mut(|data| {
            if let Some(mut state) = data.get_temp::<Gesture>(slot()) {
                if state.discarded && !state.active {
                    state = Gesture { blocked_frame: Some(frame), ..Default::default() };
                } else if state.active && state.has_phases {
                    state.terminal_frame = Some(frame);
                }
                data.insert_temp(slot(), state);
            }
        });
    }
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
pub(crate) fn input(ui: &egui::Ui, scope: Scope, key: (u64, u16), members: &[u64], hovered: bool, width: f32, actions: (Action, Action)) -> Frame {
    let (row, copy) = key;
    let width = if width.is_finite() { width.clamp(0.0, 100_000.0) } else { 0.0 };
    let id = slot();
    let (events, now) = ui.input(|i| (i.events.clone(), i.time));
    let frame_nr = ui.ctx().cumulative_frame_nr();
    let mut state = ui.ctx().data_mut(|data| data.get_temp::<Gesture>(id).unwrap_or_default());
    if state.active && state.scope != Some(scope) {
        state = state.discard();
    }
    if state.blocked_frame == Some(frame_nr) {
        return Frame::default();
    }
    let mut end = false;
    let mut cancel = false;
    for event in events {
        let Event::MouseWheel { unit, mut delta, phase, source, modifiers } = event else { continue };
        if state.discarded && !state.active {
            match phase {
                TouchPhase::Start if hovered && modifiers == egui::Modifiers::NONE && scale(unit, source, cfg!(target_os = "windows")).is_some() => {
                    state = Gesture::default();
                }
                TouchPhase::End | TouchPhase::Cancel if scale(unit, source, cfg!(target_os = "windows")).is_some() => {
                    state = Gesture { blocked_frame: Some(frame_nr), ..Default::default() };
                    break;
                }
                _ => continue,
            }
        }
        if state.active && state.settling && state.discarded && matches!(phase, TouchPhase::End | TouchPhase::Cancel) {
            state.discarded = false;
        }
        if modifiers != egui::Modifiers::NONE {
            if matches!(phase, TouchPhase::End | TouchPhase::Cancel) && state.active && !state.settling && (state.row, state.copy) == key {
                end = true;
                cancel = true;
            }
            continue;
        }
        let Some(scale) = scale(unit, source, cfg!(target_os = "windows")) else { continue };
        delta *= scale;
        // If its row vanished, a later gesture can start elsewhere without acting on it.
        if state.active && hovered && now - state.last > 1.0 && (state.row != row || state.copy != copy) {
            state = state.discard();
            if state.discarded {
                if phase == TouchPhase::Start {
                    state = Gesture::default();
                } else {
                    continue;
                }
            }
        }
        if !state.active && hovered && phase != TouchPhase::End && phase != TouchPhase::Cancel {
            state = Gesture {
                active: true,
                scope: Some(scope),
                row,
                copy,
                members: members.to_vec(),
                has_phases: phase == TouchPhase::Start,
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
        ui.ctx().data_mut(|data| data.insert_temp(id, state));
        return Frame::default();
    }
    state.frame = ui.ctx().cumulative_frame_nr();
    let horizontal = state.delta.x.abs() > state.delta.y.abs() * 2.0;
    let action = if state.delta.x < 0.0 { actions.0 } else { actions.1 };
    let offset = if horizontal && action != Action::None { state.delta.x.clamp(-width * 0.85, width * 0.85) } else { 0.0 };
    let timed_out = state.has_phases && now - state.last >= 5.0;
    if !state.settling && (end || timed_out || (!state.has_phases && now - state.last >= 0.18)) {
        state.settling = true;
        state.settled_at = now;
        state.from = offset;
        state.action = if !cancel && !timed_out && horizontal && state.delta.x.abs() >= 80.0 { action } else { Action::None };
        state.discarded = timed_out;
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
            state = Gesture { discarded: state.discarded, blocked_frame: Some(frame_nr), ..Default::default() };
        }
    }
    if state.active {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
    }
    ui.ctx().data_mut(|data| data.insert_temp(id, state));
    frame
}
