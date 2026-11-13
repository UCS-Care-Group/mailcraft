// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of `quit.rs`.

use std::cell::Cell;

use egui::{RawInput, ViewportEvent, ViewportId};
use mailcraft_engine::Session;
use mailcraft_model::Mailbox;
use mailcraft_ui_egui::Services;
use serde_json::json;

use super::*;

fn app() -> MailApp {
    MailApp::new(Session::new(Mailbox::default()), Services::default())
}

/// A message being written that something was typed into.
fn writing(app: &mut MailApp) {
    app.session.run("mail.new", &json!({})).unwrap();
    app.session.compose.as_mut().unwrap().dirty = true;
}

/// One logic frame of the main window (as eframe runs it, also while minimized), with its
/// close asked for or not. Returns whether the platform's quit ran (a stand-in that quits
/// whenever it is reached) and the window's commands.
fn frame(app: &mut MailApp, close: bool, minimized: bool) -> (bool, Vec<ViewportCommand>) {
    let ctx = Context::default();
    let mut input = RawInput::default();
    let root = input.viewports.entry(ViewportId::ROOT).or_default();
    root.minimized = Some(minimized);
    if close {
        root.events.push(ViewportEvent::Close);
    }
    let quit = Cell::new(false);
    let mut out = ctx.run_logic(&input, |ctx| {
        on_frame_with(ctx, app, |ctx| {
            take_over_close(ctx, || {
                quit.set(true);
                true
            })
        });
    });
    (quit.get(), out.viewport_commands.remove(&ViewportId::ROOT).unwrap_or_default())
}

#[test]
fn nothing_unsaved_quits_at_once() {
    let mut app = app();
    let (quit, commands) = frame(&mut app, true, false);
    assert!(quit);
    assert!(commands.contains(&ViewportCommand::CancelClose), "eframe must not close the window as well");
    assert!(!commands.contains(&ViewportCommand::Close));
}

#[test]
fn no_close_asked_for_does_nothing() {
    let mut app = app();
    let (quit, commands) = frame(&mut app, false, false);
    assert!(!quit && commands.is_empty());
}

#[test]
fn a_message_being_written_asks_first_and_holds_the_close() {
    let mut app = app();
    writing(&mut app);
    let (quit, commands) = frame(&mut app, true, false);
    assert!(!quit, "the question comes before the quit");
    assert!(commands.contains(&ViewportCommand::CancelClose));
    assert!(app.dialog.is_some(), "the discard question is open");
    assert!(!app.quit_requested);
}

#[test]
fn a_held_close_brings_a_minimized_window_back() {
    let mut app = app();
    writing(&mut app);
    let (_, commands) = frame(&mut app, true, true);
    assert!(commands.contains(&ViewportCommand::Minimized(false)));
    assert!(commands.contains(&ViewportCommand::Focus));
}

#[test]
fn the_question_answered_closes_and_then_quits() {
    let mut app = app();
    writing(&mut app);
    frame(&mut app, true, false);
    // Don't Save: the draft goes, and the app asks to quit.
    app.session.compose = None;
    app.dialog = None;
    app.quit_requested = true;
    let (quit, commands) = frame(&mut app, false, false);
    assert!(!quit);
    assert!(commands.contains(&ViewportCommand::Close), "the close is asked for again");
    assert!(!app.quit_requested, "asked once");
    // The next frame decides that close like any other: nothing is left to ask about.
    let (quit, commands) = frame(&mut app, true, false);
    assert!(quit);
    assert!(commands.contains(&ViewportCommand::CancelClose), "eframe must not close the window as well");
}

#[test]
fn a_platform_without_a_quit_leaves_the_close_to_eframe() {
    let ctx = Context::default();
    let mut input = RawInput::default();
    input.viewports.entry(ViewportId::ROOT).or_default().events.push(ViewportEvent::Close);
    let mut taken = true;
    let out = ctx.run_logic(&input, |ctx| taken = take_over_close(ctx, || false));
    assert!(!taken);
    let commands = out.viewport_commands.get(&ViewportId::ROOT).cloned().unwrap_or_default();
    assert!(!commands.contains(&ViewportCommand::CancelClose));
}

#[test]
fn a_held_close_is_never_taken_over() {
    let ctx = Context::default();
    let mut input = RawInput::default();
    input.viewports.entry(ViewportId::ROOT).or_default().events.push(ViewportEvent::Close);
    let called = Cell::new(false);
    let _ = ctx.run_logic(&input, |ctx| {
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        assert!(!take_over_close(ctx, || {
            called.set(true);
            true
        }));
    });
    assert!(!called.get());
}

#[test]
fn no_close_asked_for_is_not_taken_over() {
    let ctx = Context::default();
    let called = Cell::new(false);
    let _ = ctx.run_logic(&RawInput::default(), |ctx| {
        assert!(!take_over_close(ctx, || {
            called.set(true);
            true
        }));
    });
    assert!(!called.get());
}

#[test]
fn the_menus_quit_asks_the_window_to_close() {
    let ctx = Context::default();
    let mut handled = (false, true);
    let out = ctx.run_logic(&RawInput::default(), |ctx| handled = (on_menu_event(ctx, QUIT_MENU_ID), on_menu_event(ctx, "other")));
    assert_eq!(handled, (true, false));
    let commands = out.viewport_commands.get(&ViewportId::ROOT).cloned().unwrap_or_default();
    assert_eq!(commands.iter().filter(|c| **c == ViewportCommand::Close).count(), 1);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn elsewhere_eframe_closes_the_window() {
    assert!(!platform_terminate());
    let mut app = app();
    let (quit, _) = frame(&mut app, true, false);
    assert!(quit, "the seam's stand-in quits; the real platform quit is absent here");
}

fn tray_frame(ctx: &Context, app: &mut MailApp, close: bool, available: bool) -> (bool, Vec<ViewportCommand>) {
    app.ui.close_to_tray_notice_seen = true;
    let mut input = RawInput::default();
    if close {
        input.viewports.entry(ViewportId::ROOT).or_default().events.push(ViewportEvent::Close);
    }
    let mut quit = false;
    let mut out = ctx.run_logic(&input, |ctx| {
        quit = on_frame_with_tray(ctx, app, available, |ctx| take_over_close(ctx, || true));
    });
    (quit, out.viewport_commands.remove(&ViewportId::ROOT).unwrap_or_default())
}

#[test]
fn close_hides_only_when_both_the_option_and_a_working_tray_are_present() {
    for (option, available) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut app = app();
        app.ui.close_to_tray = option;
        let (quit, commands) = tray_frame(&Context::default(), &mut app, true, available);
        assert_eq!(quit, !(option && available));
        assert_eq!(commands.contains(&ViewportCommand::Visible(false)), option && available);
        if option && available {
            assert!(commands.contains(&ViewportCommand::CancelClose));
        }
    }
}

#[test]
fn the_first_close_stays_visible_until_the_owner_has_seen_the_tray_notice() {
    let mut app = app();
    app.ui.close_to_tray = true;
    let ctx = Context::default();
    let mut input = RawInput::default();
    input.viewports.entry(ViewportId::ROOT).or_default().events.push(ViewportEvent::Close);
    let out = ctx.run_logic(&input, |ctx| {
        on_frame_with_tray(ctx, &mut app, true, |_| panic!("a tray close must not terminate"));
    });
    let commands = out.viewport_commands.get(&ViewportId::ROOT).unwrap();
    assert!(commands.contains(&ViewportCommand::CancelClose));
    assert!(!commands.contains(&ViewportCommand::Visible(false)));
    assert!(!app.ui.close_to_tray_notice_seen);
    assert!(!app.quit_requested);
}

#[test]
fn hiding_keeps_the_message_being_written_without_opening_a_quit_question() {
    let mut app = app();
    app.ui.close_to_tray = true;
    writing(&mut app);
    let (quit, commands) = tray_frame(&Context::default(), &mut app, true, true);
    assert!(!quit);
    assert!(commands.contains(&ViewportCommand::Visible(false)));
    assert!(app.session.compose.as_ref().unwrap().dirty);
    assert!(app.dialog.is_none());
    assert!(!app.quit_requested);
}

#[test]
fn explicit_quit_bypasses_the_tray_but_still_protects_unsaved_work() {
    for unsaved in [false, true] {
        let mut app = app();
        app.ui.close_to_tray = true;
        if unsaved {
            writing(&mut app);
        }
        let ctx = Context::default();
        app.quit_requested = true;
        let (quit, commands) = tray_frame(&ctx, &mut app, false, true);
        assert!(!quit);
        assert!(commands.contains(&ViewportCommand::Close));
        let (quit, commands) = tray_frame(&ctx, &mut app, true, true);
        assert_eq!(quit, !unsaved);
        assert!(!commands.contains(&ViewportCommand::Visible(false)));
        if unsaved {
            assert!(app.dialog.is_some());
            assert!(commands.contains(&ViewportCommand::Visible(true)));
            // Cancel the quit question; the next ordinary close hides again.
            app.dialog = None;
            let (quit, commands) = tray_frame(&ctx, &mut app, true, true);
            assert!(!quit);
            assert!(commands.contains(&ViewportCommand::Visible(false)));
            assert!(app.session.compose.is_some());
        }
    }
}

#[test]
fn minimizing_without_a_close_request_is_left_to_the_desktop() {
    let mut app = app();
    app.ui.close_to_tray = true;
    let (_, commands) = tray_frame(&Context::default(), &mut app, false, true);
    assert!(commands.is_empty());
}
