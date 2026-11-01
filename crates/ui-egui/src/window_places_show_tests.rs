// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where windows of the system open (#286), as [`super::show`] puts them: a stand-in for the
//! backend opens each and says where it is, so that what is wired is tested, not only the rule.
//! Both models are tried on every system: pixels (Windows, X11) and points (macOS).

use std::cell::RefCell;
use std::rc::Rc;

use mailcraft_engine::Session;
use serde_json::json;

use crate::window_places::{Kind, Monitor, Place};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// What the stand-in backend was asked: each window's builder and the commands it sent.
#[derive(Default)]
struct Seen {
    builders: Vec<egui::ViewportBuilder>,
    commands: Vec<egui::ViewportCommand>,
}

/// A window of the system, as the backend says it is once the owner moved it: at (1200, 60)
/// points on a monitor of scale 2 (its pixel (2400, 120)), 900 × 700 points.
fn child_info() -> egui::ViewportInfo {
    let rect = egui::Rect::from_min_size(egui::pos2(1200.0, 60.0), egui::vec2(900.0, 700.0));
    egui::ViewportInfo {
        native_pixels_per_point: Some(2.0),
        inner_rect: Some(rect),
        outer_rect: Some(rect),
        focused: Some(false),
        ..Default::default()
    }
}

fn backend() -> (egui::Context, Rc<RefCell<Seen>>) {
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let seen = Rc::new(RefCell::new(Seen::default()));
    let record = seen.clone();
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        record.borrow_mut().builders.push(viewport.builder.clone());
        let mut input = egui::RawInput { viewport_id: viewport.ids.this, ..Default::default() };
        input.viewports.insert(viewport.ids.this, child_info());
        let mut out = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
        out.textures_delta.clear();
    });
    (ctx, seen)
}

/// One frame of the main window, which shows the windows of their own.
fn frame(ctx: &egui::Context, app: &mut MailApp, seen: &Rc<RefCell<Seen>>) {
    let mut input = egui::RawInput::default();
    let info = egui::ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() };
    input.viewports.insert(egui::ViewportId::ROOT, info);
    let mut out = ctx.run_ui(input, |ui| super::show(app, ui.ctx()));
    out.textures_delta.clear();
    // (What a window sent to itself comes out with the main window's pass.)
    for (id, o) in &out.viewport_output {
        if *id != egui::ViewportId::ROOT {
            seen.borrow_mut().commands.extend(o.commands.iter().cloned());
        }
    }
}

/// What the host lists: a laptop at ×1 and a 4K monitor at ×2 to its right, as winit gives them
/// (on macOS a monitor's place is its point times its own scale), kept in pixels or in points
/// (`points`) as the host keeps them.
fn monitors(points: bool) -> Vec<Monitor> {
    let laptop = crate::window_places::monitor([0, 0], [1920, 1080], 1.0, points);
    let at = if points { 3840 } else { 1920 };
    let four_k = crate::window_places::monitor([at, 0], [3840, 2160], 2.0, points);
    vec![laptop, four_k]
}

/// What is looked for in a model: the place saved, where the window opens (the builder's and
/// the put's), the place noted from [`child_info`], and where the second one opens.
struct Expected {
    saved: Place,
    opens: egui::Pos2,
    noted: Place,
    second: egui::Pos2,
}

fn opens_at_its_place_and_is_remembered(points: bool, x: Expected) {
    let (ctx, seen) = backend();
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    app.windows.points = Some(points);
    // The last message window was on the 4K monitor, 900 × 700 points.
    app.ui.window_places.set(Kind::Message, x.saved);
    let messages: Vec<u64> = app.session.mailbox.messages.values().filter(|m| !m.draft).map(|m| m.id).take(2).collect();
    app.execute("ui.openWindow", json!({"message": messages[0]})).unwrap();
    // The first frame it waits for the host's list of monitors, which the host is asked for.
    frame(&ctx, &mut app, &seen);
    assert!(seen.borrow().builders.is_empty());
    assert!(app.wants_monitors());
    // Listed, it opens there: on the 4K monitor, at its size in points, then put there exactly.
    app.set_monitors(monitors(points));
    frame(&ctx, &mut app, &seen);
    {
        let seen = seen.borrow();
        let b = seen.builders.first().expect("opened");
        assert_eq!((b.inner_size, b.position), (Some(egui::vec2(900.0, 700.0)), Some(x.opens)));
        assert!(seen.commands.iter().any(|c| matches!(c, egui::ViewportCommand::OuterPosition(p) if *p == x.opens)), "{:?}", seen.commands);
    }
    assert!(!app.wants_monitors());
    // Where the system says it is (the owner moved it) is remembered for the next of its kind.
    assert_eq!(app.ui.window_places.get(Kind::Message), Some(x.noted));
    // A second one cascades from where the first is now.
    app.execute("ui.openWindow", json!({"message": messages[1]})).unwrap();
    seen.borrow_mut().builders.clear();
    frame(&ctx, &mut app, &seen);
    app.set_monitors(monitors(points));
    frame(&ctx, &mut app, &seen);
    let second = seen.borrow().builders.iter().find(|b| b.position != Some(x.opens)).cloned().expect("the second opened");
    assert_eq!(second.position, Some(x.second));
}

#[test]
fn a_window_of_the_system_waits_for_the_monitors_opens_at_its_place_and_is_remembered() {
    // Pixels (Windows, X11): saved at the 4K's pixel (2200, 100), it opens at that over the 4K's
    // scale and is put there; it is noted at (1200, 60) points times 2; the second cascades 26
    // points (52 pixels) on.
    opens_at_its_place_and_is_remembered(
        false,
        Expected {
            saved: Place { position: Some([2200.0, 100.0]), width: 1800.0, height: 1400.0, scale: 2.0 },
            opens: egui::pos2(1100.0, 50.0),
            noted: Place { position: Some([2400.0, 120.0]), width: 1800.0, height: 1400.0, scale: 2.0 },
            second: egui::pos2(1226.0, 86.0),
        },
    );
}

#[test]
fn in_points_as_on_macos_a_window_opens_at_its_place_and_is_remembered() {
    // Points (macOS): saved at (2200, 100), on the 4K (from point 1920 on), it opens and is put
    // there; it is noted at (1200, 60), on the laptop, from where the second would run off the
    // laptop's right edge and starts again at its top left.
    opens_at_its_place_and_is_remembered(
        true,
        Expected {
            saved: Place { position: Some([2200.0, 100.0]), width: 900.0, height: 700.0, scale: 1.0 },
            opens: egui::pos2(2200.0, 100.0),
            noted: Place { position: Some([1200.0, 60.0]), width: 900.0, height: 700.0, scale: 1.0 },
            second: egui::pos2(26.0, 26.0),
        },
    );
}
