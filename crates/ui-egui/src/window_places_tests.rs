// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a window of its own opens (#286): the remembered place, the cascade, a lost monitor,
//! monitors of different scales and `ui.json`.

use super::*;

fn monitor(x: f32, y: f32, w: f32, h: f32, scale: f32) -> Monitor {
    Monitor { rect: egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h)), scale }
}

fn laptop() -> Monitor {
    monitor(0.0, 0.0, 1920.0, 1080.0, 1.0)
}

const MIN: egui::Vec2 = egui::vec2(420.0, 300.0);
const DEFAULT: egui::Vec2 = egui::vec2(820.0, 640.0);

fn at(x: f32, y: f32, w: f32, h: f32) -> Place {
    Place { position: Some([x, y]), width: w, height: h, scale: 1.0 }
}

fn open(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
}

/// Where it goes, and its size in points.
fn go(saved: Option<Place>, last: Option<egui::Rect>, monitors: &[Monitor]) -> (egui::Vec2, Option<egui::Pos2>) {
    let o = place_new(saved, DEFAULT, MIN, last, monitors, 1.0);
    (o.size, o.at)
}

#[test]
fn a_new_window_opens_where_the_last_of_its_kind_was() {
    assert_eq!(go(Some(at(300.0, 200.0, 900.0, 700.0)), None, &[laptop()]), (egui::vec2(900.0, 700.0), Some(egui::pos2(300.0, 200.0))));
    // Nothing remembered: the size it has by default, where the system puts it.
    assert_eq!(go(None, None, &[laptop()]), (DEFAULT, None));
    // A size too small is the least there may be.
    assert_eq!(go(Some(at(10.0, 10.0, 50.0, 50.0)), None, &[laptop()]).0, MIN);
}

#[test]
fn the_next_cascades_from_the_last_one_open() {
    let (_, pos) = go(Some(at(100.0, 100.0, 900.0, 700.0)), Some(open(400.0, 150.0, 900.0, 700.0)), &[laptop()]);
    assert_eq!(pos, Some(egui::pos2(400.0 + CASCADE, 150.0 + CASCADE)));
}

#[test]
fn a_cascade_that_would_run_off_the_monitor_starts_again_at_its_top_left() {
    // Down at the bottom right: the next would go past the bottom edge.
    let (_, pos) = go(Some(at(900.0, 370.0, 900.0, 700.0)), Some(open(900.0, 370.0, 900.0, 700.0)), &[laptop()]);
    assert_eq!(pos, Some(egui::pos2(CASCADE, CASCADE)));
    // On a monitor to the left (negative x), it starts again at that monitor's top left.
    let left = monitor(-1600.0, 0.0, 1600.0, 900.0, 1.0);
    let saved = Some(at(0.0, 0.0, 900.0, 700.0));
    assert_eq!(go(saved, Some(open(-900.0, 180.0, 900.0, 700.0)), &[laptop(), left]).1, Some(egui::pos2(-1600.0 + CASCADE, CASCADE)));
    // Room left: it steps on, on that monitor.
    assert_eq!(go(saved, Some(open(-1500.0, 50.0, 900.0, 700.0)), &[laptop(), left]).1, Some(egui::pos2(-1500.0 + CASCADE, 50.0 + CASCADE)));
}

#[test]
fn a_window_whose_monitor_is_gone_opens_in_the_middle_of_the_main_one() {
    // Last on a monitor to the right that isn't connected now.
    assert_eq!(go(Some(at(2200.0, 100.0, 800.0, 600.0)), None, &[laptop()]), (egui::vec2(800.0, 600.0), Some(egui::pos2(560.0, 240.0))));
    // A monitor to the left, still there, keeps it at its negative place.
    let left = monitor(-1600.0, 0.0, 1600.0, 900.0, 1.0);
    assert_eq!(go(Some(at(-1500.0, 100.0, 800.0, 600.0)), None, &[laptop(), left]).1, Some(egui::pos2(-1500.0, 100.0)));
    // Gone, it comes to the main one.
    assert_eq!(go(Some(at(-1500.0, 100.0, 800.0, 600.0)), None, &[laptop()]).1, Some(egui::pos2(560.0, 240.0)));
    // A window still open on a monitor that has gone: the next of its kind isn't put beside it.
    assert_eq!(go(Some(at(0.0, 0.0, 800.0, 600.0)), Some(open(2200.0, 100.0, 800.0, 600.0)), &[laptop()]).1, Some(egui::pos2(560.0, 240.0)));
}

#[test]
fn monitors_of_different_scales_are_one_space_of_pixels() {
    // The laptop at scale 1, a 4K monitor to its right at scale 2.
    let four_k = monitor(1920.0, 0.0, 3840.0, 2160.0, 2.0);
    // A window left on the 4K monitor at its pixel (2200, 100), 900 × 700 points there.
    let rect = egui::Rect::from_min_size(egui::pos2(1100.0, 50.0), egui::vec2(900.0, 700.0));
    let info = egui::ViewportInfo { inner_rect: Some(rect), outer_rect: Some(rect), ..Default::default() };
    let saved = Place::of(&info, 2.0, 1.0).unwrap();
    assert_eq!(saved, Place { position: Some([2200.0, 100.0]), width: 1800.0, height: 1400.0, scale: 2.0 });
    // It opens there again, not on the laptop: at that pixel, 900 × 700 points at scale 2.
    let o = place_new(Some(saved), DEFAULT, MIN, None, &[laptop(), four_k], 1.0);
    assert_eq!((o.at, o.size, o.pixels_per_point), (Some(egui::pos2(2200.0, 100.0)), egui::vec2(900.0, 700.0), 2.0));
    // The next of its kind steps on by 26 points there: 52 pixels.
    let next = place_new(Some(saved), DEFAULT, MIN, o.rect(), &[laptop(), four_k], 1.0);
    assert_eq!(next.at, Some(egui::pos2(2252.0, 152.0)));
    // With the 4K monitor gone, it opens on the laptop at the same size in points.
    let o = place_new(Some(saved), DEFAULT, MIN, None, &[laptop()], 1.0);
    assert_eq!((o.at, o.size, o.pixels_per_point), (Some(egui::pos2(510.0, 190.0)), egui::vec2(900.0, 700.0), 1.0));
    // egui's zoom is in the points, not in the pixels.
    let o = place_new(Some(saved), DEFAULT, MIN, None, &[laptop(), four_k], 1.5);
    assert_eq!((o.at, o.pixels_per_point, o.pixels), (Some(egui::pos2(2200.0, 100.0)), 3.0, egui::vec2(1800.0, 1400.0)));
}

#[test]
fn a_window_is_never_bigger_than_the_monitor_it_opens_on() {
    // Saved large on a big monitor that has gone: it fits the laptop, whole.
    let big = at(3000.0, 100.0, 2400.0, 1400.0);
    let o = place_new(Some(big), DEFAULT, MIN, None, &[laptop()], 1.0);
    assert_eq!((o.size, o.at), (egui::vec2(1920.0, 1080.0), Some(egui::pos2(0.0, 0.0))));
    // On the laptop itself, too big at its place: smaller, and moved to fit.
    let o = place_new(Some(at(600.0, 300.0, 2000.0, 1000.0)), DEFAULT, MIN, None, &[laptop()], 1.0);
    assert_eq!((o.size, o.at), (egui::vec2(1920.0, 1000.0), Some(egui::pos2(0.0, 80.0))));
    // One that fits stays where the owner left it, partly off the monitor or not.
    let o = place_new(Some(at(600.0, 300.0, 1700.0, 1000.0)), DEFAULT, MIN, None, &[laptop()], 1.0);
    assert_eq!((o.size, o.at), (egui::vec2(1700.0, 1000.0), Some(egui::pos2(600.0, 300.0))));
}

#[test]
fn without_a_list_of_monitors_the_system_places_it() {
    // Wayland: no monitors listed, no position kept. The size in points is still the remembered one.
    assert_eq!(go(Some(at(300.0, 200.0, 900.0, 700.0)), None, &[]), (egui::vec2(900.0, 700.0), None));
    let sharp = Place { scale: 2.0, ..at(300.0, 200.0, 1800.0, 1400.0) };
    assert_eq!(go(Some(sharp), None, &[]), (egui::vec2(900.0, 700.0), None));
    let junk = [monitor(f32::NAN, 0.0, 10.0, 10.0, 1.0), monitor(0.0, 0.0, 0.0, 0.0, 1.0), monitor(0.0, 0.0, 1920.0, 1080.0, f32::NAN)];
    assert_eq!(go(Some(at(300.0, 200.0, 900.0, 700.0)), None, &junk).1, None);
}

#[test]
fn a_place_that_cant_be_real_isnt_used() {
    for bad in [at(f32::NAN, 0.0, 900.0, 700.0), at(1e9, 0.0, 900.0, 700.0)] {
        // Its size stays; its position goes, and the system places it.
        assert_eq!(go(Some(bad), None, &[laptop()]), (egui::vec2(900.0, 700.0), None));
    }
    for bad in [at(10.0, 10.0, f32::INFINITY, 700.0), at(10.0, 10.0, -5.0, 700.0), at(10.0, 10.0, 0.0, 0.0)] {
        assert_eq!(go(Some(bad), None, &[laptop()]), (DEFAULT, None));
    }
    assert_eq!(go(None, Some(open(f32::NAN, 0.0, 900.0, 700.0)), &[laptop()]), (DEFAULT, None));
    let odd = Place { scale: f32::NAN, ..at(10.0, 10.0, 900.0, 700.0) };
    assert_eq!(odd.sanitized().map(|p| p.scale), Some(1.0));
    let o = place_new(Some(at(10.0, 10.0, 900.0, 700.0)), DEFAULT, MIN, None, &[laptop()], f32::NAN);
    assert_eq!(o.pixels_per_point, 1.0);
}

#[test]
fn the_place_of_a_window_as_the_system_says() {
    let rect = egui::Rect::from_min_size(egui::pos2(-700.0, 40.0), egui::vec2(800.0, 600.0));
    let info = egui::ViewportInfo { inner_rect: Some(rect), outer_rect: Some(rect), ..Default::default() };
    assert_eq!(Place::of(&info, 1.0, 1.0), Some(at(-700.0, 40.0, 800.0, 600.0)));
    // Maximized, minimized or full screen: the place to come back to is the one before.
    for info in [
        egui::ViewportInfo { maximized: Some(true), ..info.clone() },
        egui::ViewportInfo { minimized: Some(true), ..info.clone() },
        egui::ViewportInfo { fullscreen: Some(true), ..info.clone() },
    ] {
        assert_eq!(Place::of(&info, 1.0, 1.0), None);
    }
    // Wayland: a size and no position.
    let info = egui::ViewportInfo { inner_rect: Some(rect), outer_rect: None, ..Default::default() };
    assert_eq!(Place::of(&info, 1.0, 1.0), Some(Place { position: None, width: 800.0, height: 600.0, scale: 1.0 }));
    // A scale that can't be real says nothing.
    assert_eq!(Place::of(&info, f32::NAN, 1.0), None);
}

#[test]
fn places_round_trip_through_ui_json_and_an_old_one_loads() {
    let mut ui = crate::UiState::default();
    let place = Place { scale: 2.0, ..at(-1500.0, 120.0, 900.0, 720.0) };
    assert!(ui.window_places.set(Kind::Compose, place));
    assert!(!ui.window_places.set(Kind::Compose, place), "the same again");
    assert!(ui.window_places.set(Kind::Message, Place { position: None, width: 800.0, height: 600.0, scale: 1.0 }));
    let text = serde_json::to_string(&ui).unwrap();
    let back: crate::UiState = serde_json::from_str(&text).unwrap();
    assert_eq!(back.window_places, ui.window_places);
    assert_eq!(back.window_places.get(Kind::Compose), Some(place));
    assert_eq!(back.window_places.get(Kind::Appointment), None);
    // A ui.json from before (#286) has none, and loads.
    let mut old = serde_json::to_value(crate::UiState::default()).unwrap();
    old.as_object_mut().unwrap().remove("windowPlaces");
    let old: crate::UiState = serde_json::from_value(old).unwrap();
    assert_eq!(old.window_places, Places::default());
}

#[test]
fn what_cant_be_read_is_left_out_and_a_kind_this_version_doesnt_know_is_kept() {
    let text = r#"{"windowPlaces": {"compose": {"position": [10, 20], "width": 900, "height": 700},
        "message": {"width": "wide"}, "appointment": {"width": -1, "height": 5}, "contact": {"width": 600, "height": 500, "scale": 1.25}}}"#;
    let mut ui: crate::UiState = serde_json::from_str(text).unwrap();
    assert_eq!(ui.window_places.get(Kind::Compose), Some(at(10.0, 20.0, 900.0, 700.0)));
    assert_eq!((ui.window_places.get(Kind::Message), ui.window_places.get(Kind::Appointment)), (None, None));
    // The newer version's contact window is written back as it was, beside what this one sets.
    ui.window_places.set(Kind::Message, at(1.0, 2.0, 800.0, 600.0));
    let written = serde_json::to_value(&ui.window_places).unwrap();
    assert_eq!(written["contact"], serde_json::json!({"width": 600, "height": 500, "scale": 1.25}));
    assert_eq!(written.as_object().unwrap().len(), 3);
    // Not even a map: none, and the rest of the settings load.
    let ui: crate::UiState = serde_json::from_str(r#"{"windowPlaces": 7, "repliesInWindow": true}"#).unwrap();
    assert_eq!((ui.window_places, ui.replies_in_window), (Places::default(), true));
    // Kinds without end are kept only so far.
    let many: serde_json::Map<String, serde_json::Value> = (0..100).map(|i| (format!("k{i}"), serde_json::json!({}))).collect();
    let places: Places = serde_json::from_value(serde_json::Value::Object(many)).unwrap();
    assert!(serde_json::to_value(&places).unwrap().as_object().unwrap().len() <= 32);
}

#[test]
fn on_macos_places_are_points_and_a_window_on_the_external_monitor_reopens_there() {
    // winit on macOS: a Retina built-in at (0,0), 3024×1964 at ×2, and an external ×1 at logical
    // x=1512, whose place is its logical one times its own scale.
    let built_in = super::monitor([0, 0], [3024, 1964], 2.0, true);
    let external = super::monitor([1512, 0], [2560, 1440], 1.0, true);
    assert_eq!(built_in, monitor(0.0, 0.0, 1512.0, 982.0, 1.0));
    assert_eq!(external, monitor(1512.0, 0.0, 2560.0, 1440.0, 1.0));
    // A window at logical x=2000 on the external, as it is noted there (points, by the zoom).
    let rect = open(2000.0, 100.0, 900.0, 700.0);
    let info = egui::ViewportInfo { inner_rect: Some(rect), outer_rect: Some(rect), ..Default::default() };
    let place = Place::of(&info, 1.0, 1.0);
    assert_eq!(place, Some(at(2000.0, 100.0, 900.0, 700.0)));
    // It reopens on the external, where it was, not on the built-in.
    let o = place_new(place, DEFAULT, MIN, None, &[built_in, external], 1.0);
    assert_eq!((o.size, o.at, o.pixels_per_point), (egui::vec2(900.0, 700.0), Some(egui::pos2(2000.0, 100.0)), 1.0));
    // Read as pixels, as on Windows and X11, the same list would put x=2000 on the built-in.
    let as_pixels = super::monitor([0, 0], [3024, 1964], 2.0, false);
    assert!(as_pixels.rect.contains(egui::pos2(2000.0, 100.0)));
}

#[test]
fn without_a_place_as_under_wayland_the_size_is_not_held_to_a_monitor() {
    // Wayland: 1920×1080 at 150%, which winit gives as ×2; no window has a place.
    let screen = monitor(0.0, 0.0, 1920.0, 1080.0, 2.0);
    let o = place_new(None, DEFAULT, MIN, None, &[screen], 1.0);
    assert_eq!((o.size, o.at), (DEFAULT, None));
    let saved = Place { position: None, width: 1230.0, height: 960.0, scale: 1.5 };
    let o = place_new(Some(saved), DEFAULT, MIN, None, &[screen], 1.0);
    assert_eq!((o.size, o.at), (DEFAULT, None));
}
