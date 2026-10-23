// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! macOS: quitting through AppKit, and the small app menu used when the menu bar can't be made.
//! Written from the public documentation of `NSApplication`, Grand Central Dispatch and `muda`,
//! through the generated bindings' safe calls only.
//!
//! eframe closing the window can crash AppKit (its Touch Bar), so a close MailCraft allows goes
//! through `-[NSApplication terminate:]`, scheduled on the main queue: `terminate:` runs the
//! application's shutdown, which ends the event loop, and `eframe::App::on_exit` saves.

use std::sync::atomic::{AtomicBool, Ordering};

use dispatch2::DispatchQueue;
use egui::Context;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;

use crate::quit::{QUIT_MENU_ID, on_menu_event};

/// Set once a quit is scheduled, so that frames drawn before it runs schedule it only once.
static SCHEDULED: AtomicBool = AtomicBool::new(false);

/// Schedule `-[NSApplication terminate:]` on the main queue. Returns whether it is (or already
/// was) scheduled.
pub fn schedule_terminate() -> bool {
    if SCHEDULED.swap(true, Ordering::SeqCst) {
        return true;
    }
    DispatchQueue::main().exec_async(|| match MainThreadMarker::new() {
        Some(mtm) => NSApplication::sharedApplication(mtm).terminate(None),
        // The main queue runs on the main thread, so this is not expected; let the next close try.
        None => SCHEDULED.store(false, Ordering::SeqCst),
    });
    true
}

/// An app menu with a Quit (Cmd+Q) that asks the window to close, so that unsaved work is asked
/// about first. Used when the full menu bar can't be made. Call from the eframe creator, after
/// winit has built its own menu.
pub fn install_app_menu(ctx: &Context, name: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let menu = Menu::new();
    let app_menu = Submenu::new(name, true);
    let quit = MenuItem::with_id(QUIT_MENU_ID, format!("Quit {name}"), true, Some(Accelerator::new(Modifiers::META, Code::KeyQ)));
    app_menu.append_items(&[&PredefinedMenuItem::hide(None), &PredefinedMenuItem::separator(), &quit])?;
    menu.append(&app_menu)?;
    menu.init_for_nsapp();
    let ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        on_menu_event(&ctx, event.id.as_ref());
    }));
    // The native menu refers to its items for as long as the program runs: keep them.
    std::mem::forget((menu, app_menu, quit));
    Ok(())
}
