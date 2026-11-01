// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a window of its own opens (#286): the remembered place, the cascade, a lost monitor and
//! `ui.json`.

use super::*;

fn monitor(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
}

fn laptop() -> egui::Rect {
    monitor(0.0, 0.0, 1920.0, 1080.0)
}

const MIN: egui::Vec2 = egui::vec2(420.0, 300.0);
const DEFAULT: egui::Vec2 = egui::vec2(820.0, 640.0);

fn at(x: f32, y: f32, w: f32, h: f32) -> Place {
    Place { position: Some([x, y]), width: w, height: h }
}

#[test]
fn a_new_window_opens_where_the_last_of_its_kind_was() {
    let (size, pos) = place_new(Some(at(300.0, 200.0, 900.0, 700.0)), DEFAULT, MIN, None, &[laptop()]);
    assert_eq!((size, pos), (egui::vec2(900.0, 700.0), Some(egui::pos2(300.0, 200.0))));
    // Nothing remembered: the size it has by default, where the system puts it.
    assert_eq!(place_new(None, DEFAULT, MIN, None, &[laptop()]), (DEFAULT, None));
    // A size too small is the least there may be.
    assert_eq!(place_new(Some(at(10.0, 10.0, 50.0, 50.0)), DEFAULT, MIN, None, &[laptop()]).0, MIN);
}

#[test]
fn the_next_cascades_from_the_last_one_open() {
    let open = egui::Rect::from_min_size(egui::pos2(400.0, 150.0), egui::vec2(900.0, 700.0));
    let (_, pos) = place_new(Some(at(100.0, 100.0, 900.0, 700.0)), DEFAULT, MIN, Some(open), &[laptop()]);
    assert_eq!(pos, Some(egui::pos2(400.0 + CASCADE, 150.0 + CASCADE)));
}

#[test]
fn a_cascade_that_would_run_off_the_monitor_starts_again_at_its_top_left() {
    // Down at the bottom right: the next would go past the bottom edge.
    let open = egui::Rect::from_min_size(egui::pos2(900.0, 370.0), egui::vec2(900.0, 700.0));
    let (_, pos) = place_new(Some(at(900.0, 370.0, 900.0, 700.0)), DEFAULT, MIN, Some(open), &[laptop()]);
    assert_eq!(pos, Some(egui::pos2(CASCADE, CASCADE)));
    // On a monitor to the left (negative x), it starts again at that monitor's top left.
    let left = monitor(-1600.0, 0.0, 1600.0, 900.0);
    let open = egui::Rect::from_min_size(egui::pos2(-900.0, 180.0), egui::vec2(900.0, 700.0));
    let (_, pos) = place_new(None, egui::vec2(900.0, 700.0), MIN, Some(open), &[laptop(), left]);
    assert_eq!(pos, Some(egui::pos2(-1600.0 + CASCADE, CASCADE)));
    // Room left: it steps on, on that monitor.
    let open = egui::Rect::from_min_size(egui::pos2(-1500.0, 50.0), egui::vec2(900.0, 700.0));
    let (_, pos) = place_new(None, egui::vec2(900.0, 700.0), MIN, Some(open), &[laptop(), left]);
    assert_eq!(pos, Some(egui::pos2(-1500.0 + CASCADE, 50.0 + CASCADE)));
}

#[test]
fn a_window_whose_monitor_is_gone_opens_in_the_middle_of_the_main_one() {
    // Last on a monitor to the right that isn't connected now.
    let (size, pos) = place_new(Some(at(2200.0, 100.0, 800.0, 600.0)), DEFAULT, MIN, None, &[laptop()]);
    assert_eq!((size, pos), (egui::vec2(800.0, 600.0), Some(egui::pos2(560.0, 240.0))));
    // A monitor to the left, still there, keeps it at its negative place.
    let left = monitor(-1600.0, 0.0, 1600.0, 900.0);
    assert_eq!(place_new(Some(at(-1500.0, 100.0, 800.0, 600.0)), DEFAULT, MIN, None, &[laptop(), left]).1, Some(egui::pos2(-1500.0, 100.0)));
    // Gone, it comes to the main one.
    assert_eq!(place_new(Some(at(-1500.0, 100.0, 800.0, 600.0)), DEFAULT, MIN, None, &[laptop()]).1, Some(egui::pos2(560.0, 240.0)));
    // A window still open on a monitor that has gone: the next of its kind isn't put beside it.
    let open = egui::Rect::from_min_size(egui::pos2(2200.0, 100.0), egui::vec2(800.0, 600.0));
    assert_eq!(place_new(None, egui::vec2(800.0, 600.0), MIN, Some(open), &[laptop()]).1, Some(egui::pos2(560.0, 240.0)));
}

#[test]
fn without_a_list_of_monitors_the_system_places_it() {
    // Wayland: no monitors listed, no position kept. The size is still the remembered one.
    assert_eq!(place_new(Some(at(300.0, 200.0, 900.0, 700.0)), DEFAULT, MIN, None, &[]), (egui::vec2(900.0, 700.0), None));
    let junk = [monitor(f32::NAN, 0.0, 10.0, 10.0), monitor(0.0, 0.0, 0.0, 0.0)];
    assert_eq!(place_new(Some(at(300.0, 200.0, 900.0, 700.0)), DEFAULT, MIN, None, &junk).1, None);
}

#[test]
fn a_place_that_cant_be_real_isnt_used() {
    for bad in [at(f32::NAN, 0.0, 900.0, 700.0), at(1e9, 0.0, 900.0, 700.0)] {
        // Its size stays; its position goes, and the system places it.
        assert_eq!(place_new(Some(bad), DEFAULT, MIN, None, &[laptop()]), (egui::vec2(900.0, 700.0), None));
    }
    for bad in [at(10.0, 10.0, f32::INFINITY, 700.0), at(10.0, 10.0, -5.0, 700.0), at(10.0, 10.0, 0.0, 0.0)] {
        assert_eq!(place_new(Some(bad), DEFAULT, MIN, None, &[laptop()]), (DEFAULT, None));
    }
    let open = egui::Rect::from_min_size(egui::pos2(f32::NAN, 0.0), egui::vec2(900.0, 700.0));
    assert_eq!(place_new(None, DEFAULT, MIN, Some(open), &[laptop()]), (DEFAULT, None));
}

#[test]
fn the_place_of_a_window_as_the_system_says() {
    let rect = egui::Rect::from_min_size(egui::pos2(-700.0, 40.0), egui::vec2(800.0, 600.0));
    let info = egui::ViewportInfo { inner_rect: Some(rect), outer_rect: Some(rect), ..Default::default() };
    assert_eq!(Place::of(&info), Some(at(-700.0, 40.0, 800.0, 600.0)));
    // Maximized, minimized or full screen: the place to come back to is the one before.
    for info in [
        egui::ViewportInfo { maximized: Some(true), ..info.clone() },
        egui::ViewportInfo { minimized: Some(true), ..info.clone() },
        egui::ViewportInfo { fullscreen: Some(true), ..info.clone() },
    ] {
        assert_eq!(Place::of(&info), None);
    }
    // Wayland: a size and no position.
    let info = egui::ViewportInfo { inner_rect: Some(rect), outer_rect: None, ..Default::default() };
    let place = Place::of(&info).unwrap();
    if !cfg!(target_os = "macos") {
        assert_eq!(place, Place { position: None, width: 800.0, height: 600.0 });
    }
}

#[test]
fn places_round_trip_through_ui_json_and_an_old_one_loads() {
    let mut ui = crate::UiState::default();
    assert!(ui.window_places.set(Kind::Compose, at(-1500.0, 120.0, 900.0, 720.0)));
    assert!(!ui.window_places.set(Kind::Compose, at(-1500.0, 120.0, 900.0, 720.0)), "the same again");
    assert!(ui.window_places.set(Kind::Message, Place { position: None, width: 800.0, height: 600.0 }));
    let text = serde_json::to_string(&ui).unwrap();
    let back: crate::UiState = serde_json::from_str(&text).unwrap();
    assert_eq!(back.window_places, ui.window_places);
    assert_eq!(back.window_places.get(Kind::Compose), Some(at(-1500.0, 120.0, 900.0, 720.0)));
    assert_eq!(back.window_places.get(Kind::Appointment), None);
    // A ui.json from before (#286) has none, and loads.
    let mut old = serde_json::to_value(crate::UiState::default()).unwrap();
    old.as_object_mut().unwrap().remove("windowPlaces");
    let old: crate::UiState = serde_json::from_value(old).unwrap();
    assert_eq!(old.window_places, Places::default());
}

#[test]
fn what_cant_be_read_in_a_saved_one_is_left_out() {
    let text = r#"{"windowPlaces": {"compose": {"position": [10, 20], "width": 900, "height": 700},
        "message": {"width": "wide"}, "appointment": {"width": -1, "height": 5}, "spaceship": {"width": 9, "height": 9}}}"#;
    let ui: crate::UiState = serde_json::from_str(text).unwrap();
    assert_eq!(ui.window_places.get(Kind::Compose), Some(at(10.0, 20.0, 900.0, 700.0)));
    assert_eq!((ui.window_places.get(Kind::Message), ui.window_places.get(Kind::Appointment)), (None, None));
    assert_eq!(serde_json::to_value(&ui.window_places).unwrap().as_object().unwrap().len(), 1);
    // Not even a map: none, and the rest of the settings load.
    let ui: crate::UiState = serde_json::from_str(r#"{"windowPlaces": 7, "repliesInWindow": true}"#).unwrap();
    assert_eq!((ui.window_places, ui.replies_in_window), (Places::default(), true));
}
