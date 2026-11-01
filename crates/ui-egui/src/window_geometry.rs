// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The main window's size, position and maximized state, saved with the UI preferences so the
//! next launch opens the window where the user left it (#23).
//!
//! Sizes and positions are in egui points: what [`egui::ViewportInfo`] reports and what
//! [`egui::ViewportBuilder`] takes.
//!
//! A restored place is checked against the monitors connected now ([`on_screen`]), so a window
//! whose monitor is gone opens on the main one (#719). The rule is about any window, not only the
//! main one: a window of its own that remembers its place is checked the same way.

use serde::{Deserialize, Serialize};

/// The smallest window restored: the desktop app's minimum inner size.
pub const MIN_SIZE: egui::Vec2 = egui::vec2(760.0, 480.0);
/// Larger saved sizes are corrupt. eframe also shrinks the start size to the largest monitor.
const MAX_SIZE: egui::Vec2 = egui::vec2(16384.0, 16384.0);
/// Saved positions farther from the origin than this are corrupt.
const MAX_COORD: f32 = 32768.0;

/// The window as it was when the app last closed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowGeometry {
    /// Top-left corner: of the frame, or of the content on macOS (where winit places a new
    /// window by it). `None` where the platform doesn't report it (Wayland).
    pub position: Option<[f32; 2]>,
    /// Content size of the un-maximized window.
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
    /// Size of the monitor the window was on: where no list of monitors can be had, a different
    /// size now is how a lost monitor is noticed.
    pub monitor: Option<[f32; 2]>,
}

impl WindowGeometry {
    /// The geometry to save, given the last saved one and the window as it is now. While the
    /// window is minimized, full screen or maximized, the last un-maximized size and position are
    /// kept, so un-maximizing after the next launch returns to them.
    pub fn track(prev: Option<Self>, info: &egui::ViewportInfo, content_size: egui::Vec2) -> Option<Self> {
        if info.minimized == Some(true) || info.fullscreen == Some(true) {
            return prev;
        }
        // egui reads the maximized state on macOS only when the window opens (egui#3494), so a
        // window that opened zoomed would stay "maximized" for good. There the zoomed frame is
        // saved like any other size instead.
        let maximized = info.maximized == Some(true) && !cfg!(target_os = "macos");
        if maximized && let Some(prev) = prev {
            return Some(Self { maximized, ..prev });
        }
        let rect = if cfg!(target_os = "macos") { info.inner_rect } else { info.outer_rect };
        let size = info.inner_rect.map_or(content_size, |r| r.size());
        let position = rect.map(|r| [r.min.x, r.min.y]);
        let monitor = info.monitor_size.map(|s| [s.x, s.y]);
        Self { position, width: size.x, height: size.y, maximized, monitor }.sanitized().or(prev)
    }

    /// `None` for a size that isn't positive and finite. Otherwise the size is clamped to
    /// [`MIN_SIZE`]..=16384, and a position or monitor size that can't be real is dropped.
    pub fn sanitized(self) -> Option<Self> {
        let size = egui::vec2(self.width, self.height);
        if !size.is_finite() || size.x <= 0.0 || size.y <= 0.0 {
            return None;
        }
        let size = size.clamp(MIN_SIZE, MAX_SIZE);
        let position = self.position.filter(|p| p.iter().all(|c| c.is_finite() && c.abs() <= MAX_COORD));
        let monitor = self.monitor.filter(|m| m.iter().all(|c| c.is_finite() && *c > 0.0 && *c <= MAX_SIZE.x));
        Some(Self { position, width: size.x, height: size.y, maximized: self.maximized, monitor })
    }

    /// Open the window at this size, position and maximized state.
    pub fn apply(self, builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
        let builder = builder.with_inner_size([self.width, self.height]).with_maximized(self.maximized);
        match self.position {
            Some(p) => builder.with_position(p),
            None => builder,
        }
    }

    /// After a restore: where to move the window so that it doesn't open off screen, given the
    /// monitors connected now (in points, as [`monitors_in_points`] makes them). `None`: it stays.
    ///
    /// With a list of monitors, the rule is [`on_screen`]'s. Without one, the window is moved
    /// only when the monitor it now reports is a different size from the one it was saved on,
    /// centred on that monitor. A maximized window fills whichever monitor the system picks, and a
    /// window without a saved position (Wayland) is where the system put it: both stay.
    pub fn rescue(&self, monitors: &[egui::Rect], monitor_now: Option<egui::Vec2>) -> Option<egui::Pos2> {
        if self.maximized {
            return None;
        }
        let [x, y] = self.position?;
        let size = egui::vec2(self.width, self.height);
        if usable_monitors(monitors).next().is_some() {
            return match on_screen(egui::Rect::from_min_size(egui::pos2(x, y), size), monitors) {
                Placement::Keep => None,
                Placement::Move(to) => Some(to),
            };
        }
        let saved = egui::Vec2::from(self.monitor?);
        let now = monitor_now.filter(|s| usable_size(*s))?;
        if !saved.is_finite() || (now - saved).length() <= 1.0 {
            return None;
        }
        Some(centred(egui::Rect::from_min_size(egui::Pos2::ZERO, now), size))
    }
}

/// Where a window goes, by [`on_screen`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// Where it was: some monitor shows enough of it.
    Keep,
    /// Its top-left corner goes here, in the middle of the main monitor.
    Move(egui::Pos2),
}

/// The part of a window's top that a monitor has to show for the window to stay where it was:
/// enough of its title bar to take hold of it and drag it.
pub const TITLE_BAR_SHOWN: egui::Vec2 = egui::vec2(100.0, 40.0);

/// Whether a window at `window` (its top-left corner and size, in points) can be seen and taken
/// hold of on one of `monitors` (in points): some monitor shows at least [`TITLE_BAR_SHOWN`] of
/// the strip along its top. Otherwise it goes in the middle of the main monitor (the one at the
/// origin, or else the first listed), its top-left corner on that monitor if it is larger.
///
/// Monitors that can't be real (not finite, no size, farther out than any desk) are left out;
/// with none left, the window is kept where the system put it. A window whose place or size can't
/// be real is moved.
pub fn on_screen(window: egui::Rect, monitors: &[egui::Rect]) -> Placement {
    let Some(main) = main_monitor(monitors) else { return Placement::Keep };
    let size = window.size();
    if !window.min.is_finite() || !size.is_finite() {
        return Placement::Move(centred(main, size));
    }
    let need = TITLE_BAR_SHOWN.min(size.max(egui::Vec2::ZERO));
    let strip = egui::Rect::from_min_size(window.min, egui::vec2(size.x, need.y));
    let shown = usable_monitors(monitors).any(|m| {
        let both = m.intersect(strip);
        both.is_positive() && both.width() >= need.x && both.height() >= need.y
    });
    if shown { Placement::Keep } else { Placement::Move(centred(main, size)) }
}

/// The main monitor: the one whose top-left corner is the origin (or else that holds it), else
/// the first one listed.
fn main_monitor(monitors: &[egui::Rect]) -> Option<egui::Rect> {
    let mut usable = usable_monitors(monitors);
    let first = usable.next()?;
    let at_origin = usable_monitors(monitors).find(|m| m.min == egui::Pos2::ZERO);
    Some(at_origin.or_else(|| usable_monitors(monitors).find(|m| m.contains(egui::Pos2::ZERO))).unwrap_or(first))
}

fn usable_monitors(monitors: &[egui::Rect]) -> impl Iterator<Item = egui::Rect> + '_ {
    monitors.iter().copied().filter(usable_monitor)
}

fn usable_monitor(m: &egui::Rect) -> bool {
    m.min.is_finite() && m.min.x.abs() <= MAX_COORD && m.min.y.abs() <= MAX_COORD && usable_size(m.size())
}

fn usable_size(s: egui::Vec2) -> bool {
    s.is_finite() && s.x > 0.0 && s.y > 0.0 && s.x <= MAX_SIZE.x && s.y <= MAX_SIZE.y
}

/// The top-left corner that puts a window of `size` in the middle of `monitor`, never above or to
/// the left of the monitor's own corner. A size that can't be real is taken as [`MIN_SIZE`].
fn centred(monitor: egui::Rect, size: egui::Vec2) -> egui::Pos2 {
    let size = if size.is_finite() { size.max(egui::Vec2::ZERO) } else { MIN_SIZE };
    let free = monitor.size() - size;
    monitor.min + egui::vec2((free.x / 2.0).max(0.0), (free.y / 2.0).max(0.0))
}

/// The monitors as winit lists them (top-left corner and size in physical pixels), in the points
/// a window's place is in: divided by the window's points-per-pixel (its scale factor times
/// egui's zoom), as egui does for the window's own rect. Nothing that can't be real comes out.
pub fn monitors_in_points(monitors: impl IntoIterator<Item = ([i32; 2], [u32; 2])>, pixels_per_point: f32) -> Vec<egui::Rect> {
    if !pixels_per_point.is_finite() || pixels_per_point <= 0.0 {
        return Vec::new();
    }
    let rects = monitors
        .into_iter()
        .take(64)
        .map(|([x, y], [w, h])| egui::Rect::from_min_size(egui::pos2(x as f32, y as f32), egui::vec2(w as f32, h as f32)) / pixels_per_point);
    rects.filter(usable_monitor).collect()
}

/// Checks the restored geometry in `pending` once there is something to check it against: a list
/// of monitors, or else the size of the monitor the window is on. Until then `pending` is left in
/// place for a later frame. Then it is taken, and [`WindowGeometry::rescue`] says where to move
/// the window, if anywhere.
pub fn take_rescue(pending: &mut Option<WindowGeometry>, info: &egui::ViewportInfo, monitors: &[egui::Rect]) -> Option<egui::Pos2> {
    let monitor_now = info.monitor_size.filter(|s| usable_size(*s));
    if usable_monitors(monitors).next().is_none() && monitor_now.is_none() {
        return None;
    }
    pending.take()?.rescue(monitors, monitor_now)
}

#[cfg(test)]
#[path = "window_geometry_tests.rs"]
mod window_geometry_tests;
