// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The guard, in the model egui itself gives: `ctx.embed_viewports() == false`, two
//! viewports, and `ui.calendarCopy` asked from the main window's pass while the appointment's
//! window is the one the system says has the keyboard. No native window, no OS focus and no
//! private `Window.focused` is forged: the only input is `RawInput.viewports`, which is what
//! eframe hands every pass (eframe glow_integration.rs:699, wgpu_integration.rs:708) and what
//! `compose_window::follow_focus` already reads.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use egui::ViewportId;
use mailcraft_engine::Session;
use serde_json::{Value, json};

use super::super::{CLIP_ID, Clip};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC, as the sample's own tests have it.
const NOW: i64 = 1_791_993_600;

/// What a backend reports of one window. `focused` is the OS's word (egui-winit 0.37
/// lib.rs:1511: `Some(window.has_focus())`).
fn info(focused: bool) -> egui::ViewportInfo {
    egui::ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(focused), ..Default::default() }
}

fn focused_of(ctx: &egui::Context, v: ViewportId) -> Option<bool> {
    ctx.input(|i| i.raw.viewports.get(&v).and_then(|info| info.focused))
}

/// The Calendar's clipboard, as `take_for_paste` stored it.
fn clipboard(ctx: &egui::Context) -> Option<Clip> {
    ctx.data(|d| d.get_temp::<Clip>(egui::Id::unique(CLIP_ID)))
}

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.execute("view.calendar", json!({})).unwrap();
    let _ = app.execute("ui.dismissReminder", json!({ "all": true }));
    // The host's list of monitors, as the window layer waits for before it opens one.
    app.set_monitors(Vec::new());
    app
}

/// One pass of the main window: `logic` (where the control channel's commands run), the command,
/// then the frame that shows the windows of the system. The focus flags go in before anything
/// reads them, exactly as eframe fills `RawInput.viewports` at the start of a pass.
fn frame(ctx: &egui::Context, app: &mut MailApp, root: &Cell<bool>, child: &Cell<bool>) -> Result<Value, String> {
    let mut input = egui::RawInput::default();
    input.viewports.insert(ViewportId::ROOT, info(root.get()));
    for v in crate::windows::viewports(app) {
        input.viewports.insert(v, info(child.get()));
    }
    let mut answer = Ok(Value::Null);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        answer = app.execute("ui.calendarCopy", json!({}));
        app.ui(ui);
    });
    out.textures_delta.clear();
    answer
}

#[test]
fn copy_is_refused_only_while_another_window_of_the_system_has_the_keyboard() {
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let root = Rc::new(Cell::new(true));
    let child = Rc::new(Cell::new(false));
    let ran = Rc::new(Cell::new(0u32));
    let (r, c, n) = (root.clone(), child.clone(), ran.clone());
    // A stand-in backend, as window_places_show_tests.rs and window_minimized_tests.rs already
    // install: it runs the window's own pass and says what the OS says of it.
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        n.set(n.get() + 1);
        let mut input = egui::RawInput { viewport_id: viewport.ids.this, ..Default::default() };
        input.viewports.insert(viewport.ids.this, info(c.get()));
        input.viewports.insert(ViewportId::ROOT, info(r.get()));
        let mut out = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
        out.textures_delta.clear();
    });

    let mut app = app();
    let lunch = app.session.mailbox.events.values().find(|e| e.title.starts_with("Lunch with Dev")).cloned().expect("the sample's Lunch with Dev");
    let dentist = app.session.mailbox.events.values().find(|e| e.title.starts_with("Dentist")).cloned().expect("the sample's Dentist");
    // Fixture: two distinct whole items of this computer's own; the opened one is not a series,
    // so `ui.openWindow` opens its form directly and asks nothing.
    assert_ne!(lunch.id, dentist.id);
    assert!(lunch.recurrence.is_none());
    app.execute("calendar.select", json!({ "items": [{ "id": lunch.id }, { "id": dentist.id }] })).unwrap();
    for _ in 0..3 {
        let _ = frame(&ctx, &mut app, &root, &child);
    }
    // Precondition: with no window of the system open, Copy works (the set is taken).
    assert_eq!(frame(&ctx, &mut app, &root, &child).unwrap(), json!({ "items": 2, "cut": false }));

    // The owner opens one of them: a window of the system over the Calendar.
    let opened = app.execute("ui.openWindow", json!({ "event": lunch.id })).expect("an appointment opens in a window of its own");
    assert!(opened.get("window").and_then(Value::as_u64).is_some(), "{opened}");
    for _ in 0..4 {
        let _ = frame(&ctx, &mut app, &root, &child);
    }
    // Fixture: it is a real window of the system (the stand-in ran its pass), not embedded, and
    // none of the command's existing guards is the one that could refuse.
    assert!(ran.get() > 0, "the window's own pass ran");
    let open = crate::windows::inspect(&app);
    assert_eq!((&open[0]["kind"], &open[0]["embedded"]), (&json!("event"), &json!(false)), "{open}");
    assert_eq!(app.session.view.module, mailcraft_engine::Module::Calendar);
    assert!(app.dialog.is_none() && app.asking.is_empty() && !app.ui.backstage);
    assert!(!egui::Popup::is_any_open(&ctx));
    // Fixture: the model says exactly what this frame set — the guard's input arrived as input.
    assert_eq!(focused_of(&ctx, ViewportId::ROOT), Some(true));
    assert_eq!(crate::windows::viewports(&app).len(), 1);
    for v in crate::windows::viewports(&app) {
        assert_eq!(focused_of(&ctx, v), Some(false), "the appointment's window is not the one with the keyboard");
    }

    // (a) An unfocused appointment merely exists: the main window still copies.
    assert_eq!(frame(&ctx, &mut app, &root, &child).unwrap(), json!({ "items": 2, "cut": false }));

    // (b) No MailCraft window has the keyboard (a terminal does); the last MailCraft context was
    // the main window: local control still copies.
    root.set(false);
    child.set(false);
    for _ in 0..2 {
        let _ = frame(&ctx, &mut app, &root, &child);
    }
    let ok = frame(&ctx, &mut app, &root, &child);
    assert!(ok.is_ok(), "local control from a terminal still copies the Calendar's set: {ok:?}");
    let prior = clipboard(&ctx).expect("Copy left the Calendar's clipboard");

    // (c) THE GUARD: the appointment's window has the keyboard, and it is the one that refuses.
    root.set(false);
    child.set(true);
    for _ in 0..2 {
        let _ = frame(&ctx, &mut app, &root, &child);
    }
    let prior_mailbox = app.session.mailbox.clone();
    let prior_selection = app.session.sel.clone();
    let refused = frame(&ctx, &mut app, &root, &child);
    assert!(refused.is_err(), "ui.calendarCopy must refuse while an appointment window has the keyboard: {refused:?}");
    assert!(!app.enabled("ui.calendarCopy"), "Copy is disabled while the reported appointment viewport is focused");
    assert!(app.session.mailbox == prior_mailbox, "a refused Copy leaves the mailbox untouched");
    assert_eq!(app.session.sel, prior_selection, "a refused Copy leaves the selection untouched");
    // What must survive it: the same clipboard, the same selection, the same mailbox.
    let after = clipboard(&ctx).expect("the refused Copy left the prior clipboard in place");
    assert_eq!((after.items, after.cut, after.text.as_str(), after.opening), (prior.items, prior.cut, prior.text.as_str(), prior.opening));
    assert_eq!(after.sources.len(), prior.sources.len(), "the same number of captured source events");
    assert!(after.sources.iter().zip(&prior.sources).all(|(now, was)| Arc::ptr_eq(now, was)));
    assert_eq!(crate::windows::viewports(&app).len(), 1);

    // (d) The owner is back in the main window: Copy works again.
    root.set(true);
    child.set(false);
    for _ in 0..2 {
        let _ = frame(&ctx, &mut app, &root, &child);
    }
    assert_eq!(frame(&ctx, &mut app, &root, &child).unwrap(), json!({ "items": 2, "cut": false }));
}

/// One more window of the system than the appointment, message and compose windows the window
/// layer lists: the Reminders window, which the manual View › Reminders Window command opens as
/// its own deferred viewport (`reminders.rs:277-279`, `reminders.rs:319`). While the backend
/// reports that viewport as the one with the keyboard, the Calendar's Copy must be disabled and
/// must refuse. The proposed guard (the change this file's other test drives) asks only about
/// `crate::windows::viewports` (`windows.rs:895`), which reads `app.windows.open`; this owned
/// window is not in that list, so that guard alone cannot see it.
///
/// The viewport is the real one the window opened: its id is taken from the pass's own
/// `FullOutput.viewport_output`, never from a private helper, and its own pass is run from that
/// output's callback as a backend would. Only `RawInput.viewports` says which window the system
/// reports focused: no native surface, no OS focus and no private `focused` is forged. The
/// fixture is the sample's real Calendar with two selected appointments, and the window is opened
/// with the owner's command. The unfocused and the closed-but-still-reported cases keep Copy
/// working, so the refusal is about a live, focused window and not about a viewport merely
/// existing.
mod reminder_viewport_regression {
    use std::sync::Arc;

    use egui::{ViewportEvent, ViewportId};
    use serde_json::{Value, json};

    use super::{app, clipboard, focused_of, info};
    use crate::MailApp;

    /// What one pass of the main window answered, and the deferred viewports of the system it
    /// asked the backend to run.
    struct Pass {
        answer: Option<Result<Value, String>>,
        /// Each: its id, its title, and the callback of its own pass. Deferred viewports only:
        /// the window layer's appointment and message windows are immediate ones.
        deferred: Vec<(ViewportId, String, Arc<egui::DeferredViewportUiCallback>)>,
    }

    /// One pass of the main window as eframe runs it: the backend's report of every viewport
    /// (the root's and, when `reminder` is given, the Reminders window's), the logic, the
    /// command when `copy`, then the frame that shows the windows of the system.
    fn pass(ctx: &egui::Context, app: &mut MailApp, root_focused: bool, reminder: Option<(ViewportId, bool)>, copy: bool) -> Pass {
        let mut input = egui::RawInput::default();
        input.viewports.insert(ViewportId::ROOT, info(root_focused));
        if let Some((id, focused)) = reminder {
            input.viewports.insert(id, info(focused));
        }
        let mut answer = None;
        let out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            if copy {
                answer = Some(app.execute("ui.calendarCopy", json!({})));
            }
            app.ui(ui);
        });
        let mut deferred = Vec::new();
        for (id, viewport) in &out.viewport_output {
            if *id == ViewportId::ROOT || viewport.class != egui::ViewportClass::Deferred {
                continue;
            }
            let Some(paint) = viewport.viewport_ui_cb.clone() else { continue };
            deferred.push((*id, viewport.builder.title.clone().unwrap_or_default(), paint));
        }
        out.drop_without_applying_deltas();
        Pass { answer, deferred }
    }

    /// A pass of the Reminders window of its own, as the backend runs the callback the main
    /// window's output named; `events` are what the system reports to it (its Close).
    fn reminder_pass(
        ctx: &egui::Context,
        id: ViewportId,
        paint: &egui::DeferredViewportUiCallback,
        root_focused: bool,
        focused: bool,
        events: Vec<ViewportEvent>,
    ) {
        let mut viewport = info(focused);
        viewport.events = events;
        let mut input = egui::RawInput {
            viewport_id: id,
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(420.0, 300.0))),
            ..Default::default()
        };
        input.viewports.insert(id, viewport);
        input.viewports.insert(ViewportId::ROOT, info(root_focused));
        ctx.run_ui(input, |ui| paint(ui)).drop_without_applying_deltas();
    }

    /// The Reminders window that comes up by itself while reminders are due, which the owner's
    /// View › Reminders Window then asks for too (nothing keeps it from coming up by itself, so the
    /// window the owner asks for is that same one), is a live window of the system, and the backend
    /// reports it as the one with the keyboard: the Calendar's Copy must refuse and leave everything
    /// it would have taken as it was. An unfocused live window and a stale report of a manually
    /// closed one must not veto it, and the root window's own report is not required: a terminal
    /// can still drive the Calendar.
    #[test]
    fn copy_is_refused_while_the_manual_reminders_window_has_the_keyboard() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let mut app = app();
        // There is no switch for the automatic pop-ups: with the sample's reminders due, the
        // Reminders window comes up by itself, and the owner's View › Reminders Window below asks
        // for that same window. It has no say until the backend reports it as having the keyboard.
        let lunch =
            app.session.mailbox.events.values().find(|e| e.title.starts_with("Lunch with Dev")).cloned().expect("the sample's Lunch with Dev");
        let dentist = app.session.mailbox.events.values().find(|e| e.title.starts_with("Dentist")).cloned().expect("the sample's Dentist");
        assert_ne!(lunch.id, dentist.id, "fixture: two distinct appointments");
        assert!(lunch.recurrence.is_none(), "fixture: the opened one is not a series");
        app.execute("calendar.select", json!({ "items": [{ "id": lunch.id }, { "id": dentist.id }] })).unwrap();

        // (a) Fixture: the Reminders window that came up by itself within the first passes is not
        // reported to have the keyboard, so the Calendar's Copy works and leaves the clipboard the
        // refusal below must not touch.
        let mut came_up = 0;
        for _ in 0..3 {
            came_up = pass(&ctx, &mut app, true, None, false).deferred.len();
        }
        assert_eq!(came_up, 1, "the Reminders window came up by itself");
        assert!(!app.reminders.asked_for, "fixture: nobody asked for the window yet");
        let copied = pass(&ctx, &mut app, true, None, true).answer.expect("the pass asks for Copy");
        assert_eq!(copied, Ok(json!({ "items": 2, "cut": false })), "fixture: Copy takes the selected set");
        let prior = clipboard(&ctx).expect("Copy left the Calendar's clipboard");

        // (b) The owner asks for the Reminders window: the real command, and the real deferred
        // viewport its pass asks the backend for.
        let asked = app.execute("view.reminders", json!({})).expect("View › Reminders Window");
        assert!(
            asked.get("reminders").and_then(Value::as_array).is_some_and(|due| !due.is_empty()),
            "fixture: the sample has reminders due: {asked}"
        );
        assert!(app.reminders.asked_for, "the manual command asked for the window");
        let opened = pass(&ctx, &mut app, true, None, false);
        assert_eq!(opened.deferred.len(), 1, "the manual Reminders window is one deferred viewport of the system");
        let (reminder_id, title, paint) = {
            let (id, title, paint) = &opened.deferred[0];
            (*id, title.clone(), paint.clone())
        };
        assert_ne!(reminder_id, ViewportId::ROOT);
        assert!(!title.is_empty(), "the discovered deferred viewport is a titled window of the system");
        // `reminders::viewport()` hashes this documented name (`reminders.rs:277-279`), and that
        // module holds the only `show_viewport_deferred` in this crate (`reminders.rs:319`). The
        // test names the id only to say the viewport it found is that window; it never reaches
        // into the module, and the id itself was discovered from the pass's output above.
        assert_eq!(reminder_id, ViewportId::from_hash_of("mailcraft_reminders"), "the deferred viewport is the Reminders window");
        assert_eq!(app.session.view.module, mailcraft_engine::Module::Calendar, "fixture: the Calendar is the module");
        assert_eq!(crate::windows::inspect(&app), json!([]), "fixture: no appointment or message window is open");
        assert!(!app.reminders.showing.is_empty(), "the manual window lists the reminders still due");

        // Its own pass, and the main window's, with the backend saying it has the keyboard.
        reminder_pass(&ctx, reminder_id, paint.as_ref(), true, true, Vec::new());
        for _ in 0..2 {
            let focused = pass(&ctx, &mut app, false, Some((reminder_id, true)), false);
            assert_eq!(focused.deferred.len(), 1, "the Reminders window stays a viewport of the system");
            reminder_pass(&ctx, reminder_id, focused.deferred[0].2.as_ref(), false, true, Vec::new());
        }
        let _ = pass(&ctx, &mut app, false, Some((reminder_id, true)), false);
        // Fixture: the only child window is the Reminders one, the main window is the active
        // layer, and none of the command's existing guards is the one that could refuse.
        assert_eq!(focused_of(&ctx, ViewportId::ROOT), Some(false), "no MailCraft main window has the keyboard");
        assert_eq!(focused_of(&ctx, reminder_id), Some(true), "the backend reports the Reminders window focused");
        assert_eq!(crate::keys::active(&ctx), crate::keys::main_layer(), "fixture: the root Calendar is the active layer");
        assert!(app.dialog.is_none(), "fixture: no dialog");
        assert!(app.asking.is_empty(), "fixture: no agent question");
        assert!(!app.ui.backstage, "fixture: no File page");
        assert!(!egui::Popup::is_any_open(&ctx), "fixture: no menu or list");

        // What must survive the refusal, and what a wrongful copy would take.
        let prior_mailbox = app.session.mailbox.clone();
        let prior_selection = app.session.sel.clone();
        let prior_changes = app.session.mailbox.event_changes.clone();
        let prior_undo = (app.session.can_undo(), app.session.undo_label(), app.session.undo_steps_moved());

        // (c) THE GUARD: the Reminders window is a live window of the system and has the
        // keyboard; the Calendar's Copy, run directly, refuses and is not offered.
        let refused = pass(&ctx, &mut app, false, Some((reminder_id, true)), true).answer.expect("the pass asks for Copy");
        assert!(refused.is_err(), "ui.calendarCopy must refuse while the Reminders window has the keyboard: {refused:?}");
        assert!(!app.enabled("ui.calendarCopy"), "the Calendar's Copy is disabled while the Reminders window has the keyboard");
        assert!(app.session.mailbox == prior_mailbox, "a refused Copy leaves the mailbox untouched");
        assert_eq!(app.session.sel, prior_selection, "a refused Copy leaves the selection untouched");
        assert_eq!(app.session.mailbox.event_changes, prior_changes, "a refused Copy queues no change");
        assert_eq!(
            (app.session.can_undo(), app.session.undo_label(), app.session.undo_steps_moved()),
            prior_undo,
            "a refused Copy adds no undo step"
        );
        let after = clipboard(&ctx).expect("the refused Copy left the prior clipboard in place");
        assert_eq!((after.items, after.cut, after.text.as_str(), after.opening), (prior.items, prior.cut, prior.text.as_str(), prior.opening));
        assert_eq!(after.sources.len(), prior.sources.len(), "the same number of captured source events");
        assert!(
            after.sources.iter().zip(&prior.sources).all(|(now, was)| Arc::ptr_eq(now, was)),
            "the prior clipboard still points at the same source events"
        );

        // (d) The same live window, unfocused: no MailCraft window has the keyboard (a terminal
        // does), and local control still copies.
        let allowed = pass(&ctx, &mut app, false, Some((reminder_id, false)), true).answer.expect("the pass asks for Copy");
        assert_eq!(allowed, Ok(json!({ "items": 2, "cut": false })), "an unfocused Reminders window does not veto Copy");

        // (e) The owner closes it with its own Close: the request goes back and its reminders are
        // put off. A backend that still reports that now-closed viewport focused must not veto:
        // only a window open now has any say.
        let open = pass(&ctx, &mut app, true, Some((reminder_id, false)), false);
        assert_eq!(open.deferred.len(), 1, "the Reminders window is still open before its Close");
        let paint = open.deferred[0].2.clone();
        reminder_pass(&ctx, reminder_id, paint.as_ref(), true, false, vec![ViewportEvent::Close]);
        let closed = pass(&ctx, &mut app, true, Some((reminder_id, false)), false);
        assert!(closed.deferred.is_empty(), "the closed Reminders window is no longer a viewport of the system");
        assert!(!app.reminders.asked_for, "closing the manual window puts the request back");
        assert!(app.reminders.showing.is_empty(), "closing with reminders showing puts them off");
        assert!(!app.reminders.snoozed.is_empty(), "they are snoozed, as closing a reminder window does");
        let stale = pass(&ctx, &mut app, false, Some((reminder_id, true)), true);
        assert_eq!(focused_of(&ctx, reminder_id), Some(true), "fixture: the backend still reports the closed viewport focused");
        assert_eq!(stale.deferred.len(), 0, "fixture: MailCraft has no such window open now");
        assert_eq!(
            stale.answer.expect("the pass asks for Copy"),
            Ok(json!({ "items": 2, "cut": false })),
            "a stale report for a closed viewport does not veto Copy"
        );
    }
}
