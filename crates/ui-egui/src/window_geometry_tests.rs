// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's saved geometry (#23), and a lost monitor found (#719).

use super::*;

fn info(rect: egui::Rect, monitor: egui::Vec2) -> egui::ViewportInfo {
    egui::ViewportInfo { inner_rect: Some(rect), outer_rect: Some(rect), monitor_size: Some(monitor), ..Default::default() }
}

fn saved() -> WindowGeometry {
    WindowGeometry { position: Some([100.0, 50.0]), width: 1200.0, height: 800.0, maximized: false, monitor: Some([1920.0, 1080.0]) }
}

/// A window saved at `[x, y]`, 1200 × 800.
fn at(x: f32, y: f32) -> WindowGeometry {
    WindowGeometry { position: Some([x, y]), ..saved() }
}

fn monitor(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
}

/// A laptop's own screen at the origin.
fn laptop() -> egui::Rect {
    monitor(0.0, 0.0, 1920.0, 1080.0)
}

/// Where a 1200 × 800 window goes in the middle of [`laptop`].
const LAPTOP_MIDDLE: egui::Pos2 = egui::pos2(360.0, 140.0);

#[test]
fn records_the_window_and_restores_it() {
    let rect = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(1200.0, 800.0));
    let g = WindowGeometry::track(None, &info(rect, egui::vec2(1920.0, 1080.0)), egui::Vec2::ZERO);
    assert_eq!(g, Some(saved()));
    let b = saved().apply(egui::ViewportBuilder::default());
    assert_eq!(b.inner_size, Some(egui::vec2(1200.0, 800.0)));
    assert_eq!(b.position, Some(egui::pos2(100.0, 50.0)));
    assert_eq!(b.maximized, Some(false));
}

#[test]
fn maximized_keeps_the_normal_size_and_position() {
    let full = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1040.0));
    let mut i = info(full, egui::vec2(1920.0, 1080.0));
    i.maximized = Some(true);
    let g = WindowGeometry::track(Some(saved()), &i, egui::Vec2::ZERO);
    if cfg!(target_os = "macos") {
        // The zoomed frame is saved as the size; the window never reopens "maximized".
        assert_eq!(g.map(|g| (g.position, g.width, g.height, g.maximized)), Some((Some([0.0, 0.0]), 1920.0, 1040.0, false)));
    } else {
        assert_eq!(g, Some(WindowGeometry { maximized: true, ..saved() }));
    }
    // Minimized or full screen: nothing changes.
    i.maximized = Some(false);
    i.minimized = Some(true);
    assert_eq!(WindowGeometry::track(Some(saved()), &i, egui::Vec2::ZERO), Some(saved()));
    i.minimized = None;
    i.fullscreen = Some(true);
    assert_eq!(WindowGeometry::track(Some(saved()), &i, egui::Vec2::ZERO), Some(saved()));
}

#[test]
fn without_a_position_the_content_size_is_kept() {
    // Wayland reports neither rect.
    let i = egui::ViewportInfo::default();
    let g = WindowGeometry::track(None, &i, egui::vec2(1000.0, 700.0));
    assert_eq!(g, Some(WindowGeometry { position: None, width: 1000.0, height: 700.0, maximized: false, monitor: None }));
    assert_eq!(g.map(|g| g.apply(egui::ViewportBuilder::default()).position), Some(None));
}

#[test]
fn insane_values_are_clamped_or_dropped() {
    let zero = WindowGeometry { width: 0.0, height: 800.0, ..saved() };
    assert_eq!(zero.sanitized(), None);
    assert_eq!(WindowGeometry::default().sanitized(), None);
    let nan = WindowGeometry { width: f32::NAN, ..saved() };
    assert_eq!(nan.sanitized(), None);
    let tiny = WindowGeometry { width: 10.0, height: 1.0e9, ..saved() }.sanitized();
    assert_eq!(tiny.map(|g| (g.width, g.height)), Some((MIN_SIZE.x, 16384.0)));
    let far = WindowGeometry { position: Some([1.0e7, 0.0]), monitor: Some([f32::INFINITY, 1080.0]), ..saved() }.sanitized();
    assert_eq!(far.map(|g| (g.position, g.monitor)), Some((None, None)));
    assert_eq!(saved().sanitized(), Some(saved()));
}

#[test]
fn a_window_on_a_connected_monitor_stays() {
    let external = monitor(1920.0, 0.0, 1920.0, 1080.0);
    assert_eq!(saved().rescue(&[laptop()], None), None);
    assert_eq!(at(2100.0, 100.0).rescue(&[laptop(), external], None), None);
}

#[test]
fn a_same_size_monitor_unplugged() {
    // Saved on an external monitor of the laptop's own size, to its right; only the laptop now.
    // The monitor's size alone can't tell that it is gone.
    let g = at(2100.0, 100.0);
    assert_eq!(g.rescue(&[laptop()], Some(egui::vec2(1920.0, 1080.0))), Some(LAPTOP_MIDDLE));
}

#[test]
fn a_monitor_to_the_left_unplugged() {
    // The external monitor sat to the left of the main one: negative coordinates.
    let left = monitor(-2560.0, -200.0, 2560.0, 1440.0);
    let g = at(-2000.0, 0.0);
    assert_eq!(g.rescue(&[left, laptop()], None), None);
    assert_eq!(g.rescue(&[laptop()], None), Some(LAPTOP_MIDDLE));
    // The main monitor is the one at the origin even when it isn't listed first.
    assert_eq!(on_screen(monitor(-5000.0, 0.0, 1200.0, 800.0), &[left, laptop()]), Placement::Move(LAPTOP_MIDDLE));
}

#[test]
fn a_scale_change_is_measured_in_points() {
    // A 3840 × 2160 monitor at 100 %: the window was saved at x = 2500 points.
    let g = at(2500.0, 100.0);
    let physical = [([0, 0], [3840, 2160])];
    assert_eq!(g.rescue(&monitors_in_points(physical, 1.0), None), None);
    // The same monitor at 200 % is 1920 × 1080 points: x = 2500 is past its right edge.
    let at_200 = monitors_in_points(physical, 2.0);
    assert_eq!(at_200, vec![laptop()]);
    assert_eq!(g.rescue(&at_200, None), Some(LAPTOP_MIDDLE));
    // A points-per-pixel that can't be real gives no monitors, and so no move.
    assert!(monitors_in_points(physical, 0.0).is_empty());
    assert!(monitors_in_points(physical, f32::NAN).is_empty());
    assert_eq!(g.rescue(&monitors_in_points(physical, f32::INFINITY), None), None);
}

#[test]
fn wholly_off_screen_is_centred() {
    for (x, y) in [(5000.0, 100.0), (100.0, 3000.0), (-5000.0, -5000.0), (100.0, -900.0)] {
        assert_eq!(at(x, y).rescue(&[laptop()], None), Some(LAPTOP_MIDDLE), "at {x}, {y}");
    }
    // A window larger than the main monitor goes to its top-left corner.
    let big = WindowGeometry { width: 2500.0, height: 1500.0, ..at(5000.0, 0.0) };
    assert_eq!(big.rescue(&[laptop()], None), Some(egui::Pos2::ZERO));
    // Centred on the main monitor wherever that is: the first listed when none is at the origin.
    let elsewhere = monitor(100.0, 100.0, 1920.0, 1080.0);
    assert_eq!(on_screen(monitor(9000.0, 0.0, 1200.0, 800.0), &[elsewhere]), Placement::Move(egui::pos2(460.0, 240.0)));
}

#[test]
fn partly_off_screen_with_the_title_bar_shown_stays() {
    // Most of the window hangs off the right and bottom edges; 100 × 40 of its top is shown.
    assert_eq!(at(1820.0, 1040.0).rescue(&[laptop()], None), None);
    // Hanging off the left: its right-hand 100 points are on screen.
    assert_eq!(at(-1100.0, 200.0).rescue(&[laptop()], None), None);
    // One point less either way, and it can't be taken hold of.
    assert_eq!(at(1821.0, 1040.0).rescue(&[laptop()], None), Some(LAPTOP_MIDDLE));
    assert_eq!(at(1820.0, 1041.0).rescue(&[laptop()], None), Some(LAPTOP_MIDDLE));
    // Its title bar above the top of the screen, the rest of it below: it is moved.
    assert_eq!(at(100.0, -30.0).rescue(&[laptop()], None), Some(LAPTOP_MIDDLE));
}

#[test]
fn without_a_list_of_monitors_the_monitor_size_decides() {
    // Wayland and the browser give no list (and Wayland no position): the system's placement,
    // or else the check of the monitor's size the window had before.
    let off = at(2100.0, 100.0);
    assert_eq!(off.rescue(&[], Some(egui::vec2(1920.0, 1080.0))), None);
    assert_eq!(off.rescue(&[], Some(egui::vec2(1600.0, 900.0))), Some(egui::pos2(200.0, 50.0)));
    assert_eq!(off.rescue(&[], Some(egui::vec2(1024.0, 768.0))), Some(egui::Pos2::ZERO));
    assert_eq!(off.rescue(&[], None), None);
    assert_eq!(WindowGeometry { monitor: None, ..off }.rescue(&[], Some(egui::vec2(1600.0, 900.0))), None);
    // No saved position, or maximized: it stays, monitors or not.
    assert_eq!(WindowGeometry { position: None, ..off }.rescue(&[laptop()], None), None);
    assert_eq!(WindowGeometry { maximized: true, ..off }.rescue(&[laptop()], None), None);
}

#[test]
fn the_rescue_waits_for_something_to_check_against() {
    let off = at(2100.0, 100.0);
    // The first frame knows neither the monitors nor the monitor's size: the geometry is kept.
    let mut pending = Some(off);
    assert_eq!(take_rescue(&mut pending, &egui::ViewportInfo::default(), &[]), None);
    assert_eq!(pending, Some(off));
    // Then the monitors: centred on the main one, and the check is done.
    assert_eq!(take_rescue(&mut pending, &egui::ViewportInfo::default(), &[laptop()]), Some(LAPTOP_MIDDLE));
    assert_eq!(pending, None);
    assert_eq!(take_rescue(&mut pending, &egui::ViewportInfo::default(), &[laptop()]), None);
    // Only the monitor's size, the same as before: no move, and the check is done as well.
    let rect = egui::Rect::from_min_size(egui::pos2(2100.0, 100.0), egui::vec2(1200.0, 800.0));
    let mut pending = Some(off);
    assert_eq!(take_rescue(&mut pending, &info(rect, egui::vec2(1920.0, 1080.0)), &[]), None);
    assert_eq!(pending, None);
}

#[test]
fn hostile_values_never_panic_or_move_off_screen() {
    let odd = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, f32::MIN, -0.0, 0.0, 1.0e30, -1.0e30];
    let good = [laptop()];
    for &a in &odd {
        for &b in &odd {
            // A window that can't be real is put on the main monitor, never somewhere unreal.
            for window in [monitor(a, b, 1200.0, 800.0), monitor(100.0, 100.0, a, b), monitor(a, a, b, b)] {
                match on_screen(window, &good) {
                    Placement::Keep => {}
                    Placement::Move(p) => assert!(p.is_finite() && laptop().contains(p), "{window:?} → {p:?}"),
                }
            }
            // Monitors that can't be real are left out: with none left, nothing moves.
            let bad = [monitor(a, b, 1920.0, 1080.0), monitor(0.0, 0.0, a, b)];
            if usable_monitors(&bad).next().is_none() {
                assert_eq!(on_screen(monitor(5000.0, 0.0, 1200.0, 800.0), &bad), Placement::Keep);
            }
            let g = WindowGeometry { position: Some([a, b]), width: a, height: b, maximized: false, monitor: Some([a, b]) };
            if let Some(p) = g.rescue(&bad, Some(egui::vec2(a, b))) {
                assert!(p.is_finite(), "{g:?} → {p:?}");
            }
        }
    }
    // Huge pixel values from the system.
    let huge = monitors_in_points([([i32::MIN, i32::MAX], [u32::MAX, u32::MAX]), ([0, 0], [0, 0])], 1.0);
    assert!(huge.is_empty());
    // A list far longer than any desk has is cut short.
    assert_eq!(monitors_in_points(std::iter::repeat_n(([0, 0], [1920, 1080]), 10_000), 1.0).len(), 64);
}

#[test]
fn survives_the_prefs_round_trip() {
    let ui = crate::UiState { window: Some(saved()), ..Default::default() };
    let json = serde_json::to_string(&ui).unwrap();
    let back: crate::UiState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.window, Some(saved()));
    // Prefs written before #23 have no window.
    let old: crate::UiState = serde_json::from_str(r#"{"tab":"Insert"}"#).unwrap();
    assert_eq!(old.window, None);
    assert_eq!(old.tab, "Insert");
}
