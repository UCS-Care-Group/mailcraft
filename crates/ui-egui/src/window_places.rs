// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a window of its own opens (#286): each kind of window (a message, a message being
//! written, an appointment) remembers the size and place it last had, in `ui.json`, and a new one
//! opens there, or a step down and to the right of the last one of its kind that is open, as
//! classic Outlook cascades them. A cascade that would run off its monitor starts again at the
//! monitor's top left, and a window is never bigger than the monitor it opens on.
//!
//! Every place goes through [`window_geometry::on_screen`] against the monitors connected now: a
//! window whose monitor is gone opens in the middle of the main one. Where no monitor is listed
//! (Wayland, which gives windows no place either), the system puts the window where it wants.
//!
//! Places are in the system's pixels, one space for every monitor whatever its scale: a window's
//! own points are its pixels divided by its own scale, which differs from monitor to monitor, so
//! a place in points read on one monitor is somewhere else on another. A window opens at its size
//! in points for the scale of the monitor it opens on, and is put at its place in pixels once it
//! is there ([`Opening`]). The browser's windows, inside the page, open where they always did.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::window_geometry::{self, Placement};

/// How far down and to the right each window of a kind opens from the last one, in points.
pub const CASCADE: f32 = 26.0;
/// Larger sizes, and positions farther out, are corrupt (as for the main window).
const MAX_SIZE: f32 = 32768.0;
const MAX_COORD: f32 = 65536.0;
/// Scales outside these are corrupt.
const MIN_SCALE: f32 = 0.25;
const MAX_SCALE: f32 = 8.0;
/// Most kinds this version doesn't know that are kept for the version that does.
const MAX_OTHER: usize = 32;

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

/// A monitor connected now: where it is and how big, in the system's pixels, and its scale (the
/// system's pixels to a point, before egui's zoom).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    pub rect: egui::Rect,
    pub scale: f32,
}

impl Monitor {
    fn usable(&self) -> bool {
        usable_scale(self.scale) && window_geometry::any_monitor(&[self.rect])
    }
}

/// A window's size and place, in the system's pixels: the top-left corner of its frame (none
/// where the system doesn't say), the size of its content, and the pixels to a point it had then.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Place {
    pub position: Option<[f32; 2]>,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

impl Default for Place {
    fn default() -> Self {
        Place { position: None, width: 0.0, height: 0.0, scale: 1.0 }
    }
}

impl Place {
    /// `None` for a size that isn't positive and finite; otherwise the size kept to 32768, a
    /// position that can't be real dropped, and a scale that can't be real taken as 1.
    pub fn sanitized(self) -> Option<Self> {
        let size = egui::vec2(self.width, self.height);
        if !size.is_finite() || size.x < 1.0 || size.y < 1.0 {
            return None;
        }
        let size = size.min(egui::Vec2::splat(MAX_SIZE));
        let position = self.position.filter(|p| p.iter().all(|c| c.is_finite() && c.abs() <= MAX_COORD));
        let scale = if usable_scale(self.scale) { self.scale } else { 1.0 };
        Some(Self { position, width: size.x, height: size.y, scale })
    }

    pub fn size(&self) -> egui::Vec2 {
        egui::vec2(self.width, self.height)
    }

    /// Its top-left corner and size, when it has a position.
    pub fn rect(&self) -> Option<egui::Rect> {
        let [x, y] = self.position?;
        Some(egui::Rect::from_min_size(egui::pos2(x, y), self.size()))
    }

    /// The place of a window of the system as it is now, from what egui says of it (in the
    /// window's own points) and its points per pixel (`pixels_per_point`: its scale times egui's
    /// zoom), with egui's `zoom`. `None` while it is minimized, maximized or full screen (the
    /// place to come back to is the one before), or before the system said.
    pub fn of(info: &egui::ViewportInfo, pixels_per_point: f32, zoom: f32) -> Option<Self> {
        if info.minimized == Some(true) || info.maximized == Some(true) || info.fullscreen == Some(true) {
            return None;
        }
        if !usable_scale(pixels_per_point) || !zoom.is_finite() || zoom <= 0.0 {
            return None;
        }
        let size = info.inner_rect?.size() * pixels_per_point;
        let position = info.outer_rect.map(|r| [r.min.x * pixels_per_point, r.min.y * pixels_per_point]);
        Self { position, width: size.x, height: size.y, scale: pixels_per_point / zoom }.sanitized()
    }
}

/// The place each kind of window last had, by the kind's name ([`Kind::name`]). What can't be
/// read in a saved one (a size that isn't a number) is left out, never an error; a kind this
/// version doesn't know is kept as it was and written back, for the version that does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Places {
    known: BTreeMap<&'static str, Place>,
    other: BTreeMap<String, serde_json::Value>,
}

impl Serialize for Places {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(self.known.len() + self.other.len()))?;
        for (name, place) in &self.known {
            map.serialize_entry(name, place)?;
        }
        for (name, value) in &self.other {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Places {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let mut out = Places::default();
        let Some(map) = value.as_object() else { return Ok(out) };
        for (name, v) in map {
            match Kind::ALL.iter().find(|k| k.name() == name) {
                Some(kind) => {
                    if let Some(place) = serde_json::from_value::<Place>(v.clone()).ok().and_then(Place::sanitized) {
                        out.known.insert(kind.name(), place);
                    }
                }
                None if out.other.len() < MAX_OTHER && name.chars().count() <= 64 => {
                    out.other.insert(name.clone(), v.clone());
                }
                None => {}
            }
        }
        Ok(out)
    }
}

impl Places {
    /// Where the last window of `kind` was.
    pub fn get(&self, kind: Kind) -> Option<Place> {
        self.known.get(kind.name()).copied()
    }

    /// A window of `kind` is at `place` now. `false` when that is what was remembered already.
    pub fn set(&mut self, kind: Kind, place: Place) -> bool {
        let Some(place) = place.sanitized() else { return false };
        self.known.insert(kind.name(), place) != Some(place)
    }
}

/// How a new window opens: its size in its own points, for the monitor it opens on, and where its
/// frame's top-left corner goes, in the system's pixels (`None`: where the system puts it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opening {
    pub size: egui::Vec2,
    pub at: Option<egui::Pos2>,
    /// The pixels to a point, egui's zoom included, of the monitor it opens on: what `at` is
    /// divided by for the place to open it at before it is put there.
    pub pixels_per_point: f32,
    /// Its size in the system's pixels.
    pub pixels: egui::Vec2,
}

impl Opening {
    /// Where it is, in the system's pixels, when it has a place.
    pub fn rect(&self) -> Option<egui::Rect> {
        Some(egui::Rect::from_min_size(self.at?, self.pixels))
    }
}

/// Where a new window of a kind opens.
///
/// `saved` is where the last one of its kind was, `default` and `min` the size in points it has
/// without one and the least it may have, `last` where the last window of its kind that is open
/// now is (in pixels), `monitors` the monitors connected now and `zoom` egui's zoom. With a window
/// of its kind open, it opens a [`CASCADE`] down and to the right of it, starting again at the
/// top left of that monitor when it would go past the monitor's right or bottom edge; else where
/// `saved` says. Either way [`window_geometry::on_screen`] has the last word, the window is no
/// bigger than the monitor it opens on (nor smaller than `min`), and with no monitor listed the
/// system places it.
pub fn place_new(saved: Option<Place>, default: egui::Vec2, min: egui::Vec2, last: Option<egui::Rect>, monitors: &[Monitor], zoom: f32) -> Opening {
    let zoom = if zoom.is_finite() && zoom > 0.0 { zoom.clamp(0.2, 5.0) } else { 1.0 };
    let saved = saved.and_then(Place::sanitized);
    let monitors: Vec<Monitor> = monitors.iter().copied().filter(Monitor::usable).take(64).collect();
    let rects: Vec<egui::Rect> = monitors.iter().map(|m| m.rect).collect();
    let points = |size: egui::Vec2| if size.is_finite() { size.max(min) } else { min };
    let Some(main) = main_monitor(&monitors) else {
        // No monitors: the system places it, at its size in points as it last was.
        let size = saved.map_or(points(default), |p| points(p.size() / (p.scale * zoom)));
        let ppp = saved.map_or(zoom, |p| p.scale * zoom);
        return Opening { size, at: None, pixels_per_point: ppp, pixels: size * ppp };
    };
    let holding = |p: egui::Pos2| monitors.iter().copied().find(|m| m.rect.contains(p));
    let last = last.filter(|r| r.min.is_finite() && r.min.x.abs() <= MAX_COORD && r.min.y.abs() <= MAX_COORD);
    // Its size in pixels on a monitor: the size in points the last of its kind had (the same
    // pixels on a monitor of the same scale), at that monitor's scale.
    let pixels_on = |m: Monitor| saved.map_or(points(default), |p| points(p.size() / (p.scale * zoom))) * m.scale * zoom;
    let start = match last {
        Some(last) => Some(cascade(last.min, holding(last.min), pixels_on, zoom)),
        None => saved.and_then(|p| p.rect()).map(|r| r.min),
    };
    let first = start.and_then(holding).unwrap_or(main);
    let pixels = pixels_on(first);
    let at = start.map(|start| match window_geometry::on_screen(egui::Rect::from_min_size(start, pixels), &rects) {
        Placement::Keep => start,
        Placement::Move(to) => to,
    });
    // The monitor it opens on: no bigger than it, and at its scale.
    let on = at.and_then(holding).unwrap_or(main);
    let ppp = on.scale * zoom;
    let fitted = pixels.min(on.rect.size()).max(min * ppp);
    // Made smaller to fit, it is moved onto that monitor whole.
    let at = match at {
        Some(at) if fitted != pixels => {
            let far = (on.rect.max - fitted).max(on.rect.min);
            Some(at.min(far).max(on.rect.min))
        }
        at => at,
    };
    let pixels = fitted;
    Opening { size: pixels / ppp, at, pixels_per_point: ppp, pixels }
}

/// A step down and to the right of `from` (on monitor `on`), or, when the window there would go
/// past that monitor's right or bottom edge, a step in from its top left.
fn cascade(from: egui::Pos2, on: Option<Monitor>, pixels_on: impl Fn(Monitor) -> egui::Vec2, zoom: f32) -> egui::Pos2 {
    let Some(m) = on else { return from + egui::Vec2::splat(CASCADE * zoom) };
    let step = egui::Vec2::splat(CASCADE * m.scale * zoom);
    let next = from + step;
    let size = pixels_on(m).min(m.rect.size());
    if next.x + size.x > m.rect.max.x || next.y + size.y > m.rect.max.y { m.rect.min + step } else { next }
}

/// The main monitor, as [`window_geometry::on_screen`] takes it: the one at the origin, else
/// the one holding it, else the first.
fn main_monitor(monitors: &[Monitor]) -> Option<Monitor> {
    let at_origin = monitors.iter().find(|m| m.rect.min == egui::Pos2::ZERO);
    let holding = monitors.iter().find(|m| m.rect.contains(egui::Pos2::ZERO));
    at_origin.or(holding).or(monitors.first()).copied()
}

fn usable_scale(scale: f32) -> bool {
    scale.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(&scale)
}

#[cfg(test)]
#[path = "window_places_tests.rs"]
mod window_places_tests;
