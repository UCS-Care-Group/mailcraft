// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a window of its own opens (#286): each kind of window (a message, a message being
//! written, an appointment) remembers the size and place it last had, in `ui.json`, and a new one
//! opens there, or a step down and to the right of the last one of its kind that is open, as
//! classic Outlook cascades them. A cascade that would run off its monitor starts again at the
//! monitor's top left.
//!
//! Every place goes through [`window_geometry::on_screen`] against the monitors connected now: a
//! window whose monitor is gone opens in the middle of the main one. Where no monitor is listed
//! (Wayland, which gives windows no place either), the system puts the window where it wants.
//!
//! Sizes and positions are in egui points, as [`crate::window_geometry`] has them. The browser's
//! windows, inside the page, open where they always did.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::window_geometry::{self, Placement};

/// How far down and to the right each window of a kind opens from the last one.
pub const CASCADE: f32 = 26.0;
/// Larger sizes, and positions farther out, are corrupt (as for the main window).
const MAX_SIZE: f32 = 16384.0;
const MAX_COORD: f32 = 32768.0;

/// A kind of window of its own, as its place is remembered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A message being read, or a message file opened.
    Message,
    /// A message being written.
    Compose,
    /// An appointment or a meeting.
    Appointment,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Message, Kind::Compose, Kind::Appointment];

    /// Its name in `ui.json`.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Message => "message",
            Kind::Compose => "compose",
            Kind::Appointment => "appointment",
        }
    }
}

/// A window's size and place: its top-left corner (of the frame, or of the content on macOS, as
/// [`window_geometry::WindowGeometry`] has it; none where the system doesn't say) and the size of
/// its content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Place {
    pub position: Option<[f32; 2]>,
    pub width: f32,
    pub height: f32,
}

impl Place {
    /// `None` for a size that isn't positive and finite; otherwise the size clamped to `min` up to
    /// 16384, and a position that can't be real dropped.
    pub fn sanitized(self, min: egui::Vec2) -> Option<Self> {
        let size = egui::vec2(self.width, self.height);
        if !size.is_finite() || size.x <= 0.0 || size.y <= 0.0 {
            return None;
        }
        let size = size.clamp(min.max(egui::Vec2::splat(1.0)), egui::Vec2::splat(MAX_SIZE));
        let position = self.position.filter(|p| p.iter().all(|c| c.is_finite() && c.abs() <= MAX_COORD));
        Some(Self { position, width: size.x, height: size.y })
    }

    pub fn size(&self) -> egui::Vec2 {
        egui::vec2(self.width, self.height)
    }

    /// Its top-left corner and size, when it has a position.
    pub fn rect(&self) -> Option<egui::Rect> {
        let [x, y] = self.position?;
        Some(egui::Rect::from_min_size(egui::pos2(x, y), self.size()))
    }

    /// The place of a window of the system as it is now; `None` while it is minimized, maximized
    /// or full screen (the place to come back to is the one before), or before the system said.
    pub fn of(info: &egui::ViewportInfo) -> Option<Self> {
        if info.minimized == Some(true) || info.maximized == Some(true) || info.fullscreen == Some(true) {
            return None;
        }
        let size = info.inner_rect?.size();
        let rect = if cfg!(target_os = "macos") { info.inner_rect } else { info.outer_rect };
        let position = rect.map(|r| [r.min.x, r.min.y]);
        Self { position, width: size.x, height: size.y }.sanitized(egui::Vec2::splat(1.0))
    }
}

/// The place each kind of window last had, by the kind's name ([`Kind::name`]). What can't be
/// read in a saved one (an unknown kind, a size that isn't a number) is left out, never an error.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Places(BTreeMap<String, Place>);

impl<'de> Deserialize<'de> for Places {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let mut out = BTreeMap::new();
        if let Some(map) = value.as_object() {
            for kind in Kind::ALL {
                let place = map.get(kind.name()).and_then(|v| serde_json::from_value::<Place>(v.clone()).ok());
                if let Some(place) = place.and_then(|p| p.sanitized(egui::Vec2::splat(1.0))) {
                    out.insert(kind.name().to_string(), place);
                }
            }
        }
        Ok(Places(out))
    }
}

impl Places {
    /// Where the last window of `kind` was.
    pub fn get(&self, kind: Kind) -> Option<Place> {
        self.0.get(kind.name()).copied()
    }

    /// A window of `kind` is at `place` now. `false` when that is what was remembered already.
    pub fn set(&mut self, kind: Kind, place: Place) -> bool {
        let Some(place) = place.sanitized(egui::Vec2::splat(1.0)) else { return false };
        self.0.insert(kind.name().to_string(), place) != Some(place)
    }
}

/// Where a new window of a kind opens: the size of its content, and its top-left corner when it
/// is to be put somewhere (`None`: where the system puts it).
///
/// `saved` is where the last one of its kind was, `default` (no smaller than `min`) the size it
/// has without one, `last` the place of the last window of its kind that is open now, and
/// `monitors` the monitors connected now. With a window of its kind open, it opens a
/// [`CASCADE`] down and to the right of it, starting again at the top left of that monitor when it
/// would go past the monitor's right or bottom edge; else where `saved` says. Either way
/// [`window_geometry::on_screen`] has the last word, and with no monitor listed the system has it.
pub fn place_new(
    saved: Option<Place>,
    default: egui::Vec2,
    min: egui::Vec2,
    last: Option<egui::Rect>,
    monitors: &[egui::Rect],
) -> (egui::Vec2, Option<egui::Pos2>) {
    let saved = saved.and_then(|p| p.sanitized(min));
    let fallback = Place { position: None, width: default.x, height: default.y }.sanitized(min).map_or(min, |p| p.size());
    let size = saved.map_or(fallback, |p| p.size());
    if !window_geometry::any_monitor(monitors) {
        return (size, None);
    }
    let last = last.filter(|r| r.min.is_finite() && r.min.x.abs() <= MAX_COORD && r.min.y.abs() <= MAX_COORD);
    let start = match last {
        Some(last) => Some(cascade(last.min, size, monitors)),
        None => saved.and_then(|p| p.rect()).map(|r| r.min),
    };
    let Some(start) = start else { return (size, None) };
    let at = match window_geometry::on_screen(egui::Rect::from_min_size(start, size), monitors) {
        Placement::Keep => start,
        Placement::Move(to) => to,
    };
    (size, Some(at))
}

/// A step down and to the right of `from`, or, when a window of `size` there would go past the
/// right or bottom edge of the monitor `from` is on, a step in from that monitor's top left.
fn cascade(from: egui::Pos2, size: egui::Vec2, monitors: &[egui::Rect]) -> egui::Pos2 {
    let step = egui::Vec2::splat(CASCADE);
    let next = from + step;
    match window_geometry::monitor_holding(monitors, from) {
        Some(m) if next.x + size.x > m.max.x || next.y + size.y > m.max.y => m.min + step,
        _ => next,
    }
}

#[cfg(test)]
#[path = "window_places_tests.rs"]
mod window_places_tests;
