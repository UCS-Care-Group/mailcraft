// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Leaving MailCraft: the window's close button, Quit in the macOS app menu and ⌘Q, Quit from the
//! Dock (an Apple event), and the app's own quit once a question about unsaved work is answered.
//!
//! *Whether* MailCraft may go is decided here the same way on every platform ([`decide`]): a
//! message being written, or an appointment window with unsaved changes, asks first
//! (`MailApp::close_requested`), and the close waits for the answer. *How* it goes is the
//! platform's ([`on_frame`]): on macOS through `-[NSApplication terminate:]`, scheduled on the main
//! queue (`quit_macos.rs`), because eframe closing the window there can crash AppKit (its Touch
//! Bar); elsewhere eframe closes the window as it always has. Either way `eframe::App::on_exit`
//! runs and saves. All of it is MailCraft's own, written from the public documentation of AppKit,
//! Grand Central Dispatch and `muda`; the only platform-specific calls are in `quit_macos.rs`.

use egui::{Context, ViewportCommand};
use mailcraft_ui_egui::MailApp;

/// Once per frame, at the end of `eframe::App::logic`: the app's decision first, then the
/// platform's quit for a close nothing held. Returns whether the platform took the close over.
pub fn on_frame(ctx: &Context, app: &mut MailApp) -> bool {
    on_frame_with(ctx, app, |ctx| take_over_close(ctx, platform_terminate))
}

/// The id of the app menu's Quit item (macOS), as `muda` reports it.
#[cfg(any(target_os = "macos", test))]
pub const QUIT_MENU_ID: &str = "mailcraft.quit";

/// Hand the close over to the platform's quit, if the platform has one: `terminate` is called (it
/// only schedules the quit) and the window's own close is cancelled so that eframe does not close
/// the window as well. A close nothing asked for, or one the app already cancelled this frame
/// (something unsaved is being asked about), is left alone. Returns whether the close was taken.
fn take_over_close(ctx: &Context, terminate: impl FnOnce() -> bool) -> bool {
    let asked = ctx.input(|i| i.viewport().close_requested());
    let held = ctx.viewport(|v| v.commands.contains(&ViewportCommand::CancelClose));
    if !asked || held {
        return false;
    }
    if !terminate() {
        // The platform has no quit of its own: eframe closes the window.
        return false;
    }
    ctx.send_viewport_cmd(ViewportCommand::CancelClose);
    true
}

/// The app menu's Quit: ask the window to close, so that a message being written is asked about
/// first like at the close button. Returns whether `id` was the Quit item.
#[cfg(any(target_os = "macos", test))]
pub fn on_menu_event(ctx: &Context, id: &str) -> bool {
    if id != QUIT_MENU_ID {
        return false;
    }
    ctx.send_viewport_cmd(ViewportCommand::Close);
    ctx.request_repaint();
    true
}

#[cfg(target_os = "macos")]
fn platform_terminate() -> bool {
    crate::quit_macos::schedule_terminate()
}

#[cfg(not(target_os = "macos"))]
fn platform_terminate() -> bool {
    false
}

/// [`on_frame`] with the platform's take-over passed in (tests).
fn on_frame_with(ctx: &Context, app: &mut MailApp, take_over: impl FnOnce(&Context) -> bool) -> bool {
    decide(ctx, app);
    // Last, so it sees a `CancelClose` sent above and leaves that close alone.
    take_over(ctx)
}

/// The app's say on leaving, the same on every platform. A close the window was asked for (its
/// close button, the app menu's Quit, the Dock) is held with `CancelClose` while something
/// unsaved is being asked about, and the window is brought back so the question can be answered.
/// The app's own quit (`quit_requested`: the question answered, `app.quit`) becomes a close
/// request, decided on the next frame like any other.
fn decide(ctx: &Context, app: &mut MailApp) {
    if ctx.input(|i| i.viewport().close_requested()) && !app.close_requested() {
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        if ctx.input(|i| i.viewport().minimized == Some(true)) {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        }
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }
    if app.quit_requested {
        // Asked once: a quit that is held again asks again, and sets this again when answered.
        app.quit_requested = false;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}

#[cfg(test)]
#[path = "quit_tests.rs"]
mod quit_tests;
