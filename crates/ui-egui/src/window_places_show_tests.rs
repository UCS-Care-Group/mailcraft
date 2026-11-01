// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where windows of the system open (#286), as [`super::show`] puts them: a stand-in for the
//! backend opens each and says where it is, so that what is wired is tested, not only the rule.

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

fn monitors() -> Vec<Monitor> {
    let laptop = Monitor { rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0)), scale: 1.0 };
    let four_k = Monitor { rect: egui::Rect::from_min_size(egui::pos2(1920.0, 0.0), egui::vec2(3840.0, 2160.0)), scale: 2.0 };
    vec![laptop, four_k]
}

#[test]
fn a_window_of_the_system_waits_for_the_monitors_opens_at_its_place_and_is_remembered() {
    let (ctx, seen) = backend();
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    // The last message window was on the 4K monitor, at its pixel (2200, 100), 900 × 700 points.
    app.ui.window_places.set(Kind::Message, Place { position: Some([2200.0, 100.0]), width: 1800.0, height: 1400.0, scale: 2.0 });
    let messages: Vec<u64> = app.session.mailbox.messages.values().filter(|m| !m.draft).map(|m| m.id).take(2).collect();
    app.execute("ui.openWindow", json!({"message": messages[0]})).unwrap();
    // The first frame it waits for the host's list of monitors, which the host is asked for.
    frame(&ctx, &mut app, &seen);
    assert!(seen.borrow().builders.is_empty());
    assert!(app.wants_monitors());
    // Listed, it opens there: on the 4K monitor, at its scale, then put at the pixel exactly.
    app.set_monitors(monitors());
    frame(&ctx, &mut app, &seen);
    {
        let seen = seen.borrow();
        let b = seen.builders.first().expect("opened");
        assert_eq!((b.inner_size, b.position), (Some(egui::vec2(900.0, 700.0)), Some(egui::pos2(1100.0, 50.0))));
        assert!(
            seen.commands.iter().any(|c| matches!(c, egui::ViewportCommand::OuterPosition(p) if *p == egui::pos2(1100.0, 50.0))),
            "{:?}",
            seen.commands
        );
    }
    assert!(!app.wants_monitors());
    // Where the system says it is (the owner moved it), in its pixels, is remembered for the next
    // of its kind.
    let place = app.ui.window_places.get(Kind::Message);
    assert_eq!(place, Some(Place { position: Some([2400.0, 120.0]), width: 1800.0, height: 1400.0, scale: 2.0 }));
    // A second one cascades from where the first is now, 26 points (52 pixels) on.
    app.execute("ui.openWindow", json!({"message": messages[1]})).unwrap();
    seen.borrow_mut().builders.clear();
    frame(&ctx, &mut app, &seen);
    app.set_monitors(monitors());
    frame(&ctx, &mut app, &seen);
    let second = seen.borrow().builders.iter().find(|b| b.position != Some(egui::pos2(1100.0, 50.0))).cloned().expect("the second opened");
    assert_eq!(second.position, Some(egui::pos2(1226.0, 86.0)));
}
