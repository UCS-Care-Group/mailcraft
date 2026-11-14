// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The desktop tray: bounded, coalesced events wake the main window even while it is hidden.
//! The window is hidden only while a tray is available; losing the tray brings it back.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use egui::{Context, ViewportCommand};
use mailcraft_ui_egui::MailApp;

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
#[path = "tray_unix.rs"]
mod backend;
#[cfg(any(target_os = "windows", target_os = "macos"))]
#[path = "tray_native.rs"]
mod backend;

const SHOW: u8 = 1;
const QUIT: u8 = 2;
const SYNC: u8 = 4;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn prefer_x11(close_to_tray: bool, reachable: impl FnOnce() -> bool) -> bool {
    close_to_tray && reachable()
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn x11_reachable() -> bool {
    if !std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty()) {
        return false;
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    if std::thread::Builder::new()
        .name("probe-x11".into())
        .spawn(move || {
            let _ = tx.send(x11rb::connect(None).is_ok());
        })
        .is_err()
    {
        return false;
    }
    // Test reachability before the one event loop is built: winit 0.30 marks an event loop
    // created even on failure, so a failed forced build cannot be retried in this process.
    rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap_or(false)
}

/// Native Wayland cannot hide a winit 0.30 window. Never swallow a close there.
pub fn can_hide(cc: &eframe::CreationContext<'_>) -> bool {
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
    {
        use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
        cc.display_handle().is_ok_and(|d| matches!(d.as_raw(), RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_)))
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly")))]
    {
        let _ = cc;
        cfg!(any(target_os = "windows", target_os = "macos"))
    }
}

/// muda's one menu handler is shared with the macOS application menu.
#[cfg(target_os = "macos")]
pub fn on_menu_event(ctx: &Context, id: &str) -> bool {
    match id {
        "mailcraft.tray.show" => {
            unhide_app();
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            ctx.request_repaint();
        }
        "mailcraft.tray.quit" => crate::quit::request(ctx),
        "mailcraft.tray.sync" => {
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique("mailcraft.tray.sync"), true));
            ctx.request_repaint();
        }
        _ => return false,
    }
    true
}

struct Signals {
    pending: AtomicU8,
    online: AtomicBool,
    ctx: Context,
}

impl Signals {
    fn send(&self, action: u8) {
        self.pending.fetch_or(action, Ordering::Relaxed);
        self.ctx.request_repaint();
    }

    fn online(&self, online: bool) {
        self.online.store(online, Ordering::Relaxed);
        // A hidden window must be reachable even if the desktop's tray disappears.
        self.ctx.request_repaint();
    }
}

pub struct Tray {
    backend: Option<backend::Backend>,
    signals: Arc<Signals>,
    hidden: bool,
    can_hide: bool,
}

impl Tray {
    /// Called on the GUI thread after eframe has created its window and event loop.
    pub fn new(ctx: &Context, can_hide: bool) -> Self {
        let signals = Arc::new(Signals { pending: AtomicU8::new(0), online: AtomicBool::new(false), ctx: ctx.clone() });
        Self { backend: None, signals, hidden: false, can_hide }
    }

    pub fn available(&self) -> bool {
        self.can_hide && self.signals.online.load(Ordering::Relaxed) && self.backend.as_ref().is_some_and(backend::Backend::ready)
    }

    pub fn show(&mut self, ctx: &Context) {
        #[cfg(target_os = "macos")]
        unhide_app();
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
        self.hidden = false;
    }

    /// After the app's logic: tray actions and existing requests to focus the window (a
    /// notification, another launch, an unsaved-work question) restore a hidden window too.
    pub fn pump(&mut self, ctx: &Context, app: &mut MailApp) {
        self.configure(app.ui.close_to_tray, backend::Backend::start);
        if let Some(backend) = &mut self.backend {
            backend.poll();
        }
        let pending = self.signals.pending.swap(0, Ordering::Relaxed);
        let sync = ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::unique("mailcraft.tray.sync"))).unwrap_or(false);
        if pending & SYNC != 0 || sync {
            let _ = app.run("ui.sync", serde_json::json!({}));
            // A sign-in or other question must be visible if Send/Receive needs one.
            self.show(ctx);
        }
        let focus = ctx.viewport(|v| {
            v.commands.iter().any(|c| matches!(c, ViewportCommand::Focus | ViewportCommand::Minimized(false) | ViewportCommand::Visible(true)))
        });
        if pending & QUIT != 0 {
            app.quit_requested = true;
        }
        if pending & (SHOW | QUIT) != 0 || app.quit_requested || self.hidden && (focus || !self.available() || !app.ui.close_to_tray) {
            self.show(ctx);
        }
    }

    fn configure(&mut self, enabled: bool, start: impl FnOnce(Arc<Signals>) -> backend::Backend) {
        if enabled && self.backend.is_none() {
            // Each lifetime owns its signals: a callback from a stopped service cannot mark
            // its replacement online or deliver a late menu action to it.
            self.signals = Arc::new(Signals { pending: AtomicU8::new(0), online: AtomicBool::new(false), ctx: self.signals.ctx.clone() });
            self.backend = Some(start(self.signals.clone()));
        } else if !enabled && self.backend.is_some() {
            self.backend = None;
            self.signals.online(false);
            self.signals.pending.store(0, Ordering::Relaxed);
        }
    }

    pub fn after_frame(&mut self, ctx: &Context) {
        #[cfg(target_os = "macos")]
        let native_hidden = Some(app_hidden());
        #[cfg(not(target_os = "macos"))]
        let native_hidden = None;
        self.after_frame_with_visibility(ctx, native_hidden);
    }

    fn after_frame_with_visibility(&mut self, ctx: &Context, native_hidden: Option<bool>) {
        if let Some(native_hidden) = native_hidden {
            let requested = ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::unique("mailcraft.tray.hidden"))).unwrap_or(false);
            // Track our own hide only: ordinary Cmd+H must keep working with the option off.
            // Dock unhide ends it. AppKit's state wins over earlier viewport commands.
            self.hidden = (self.hidden || requested) && native_hidden;
        } else {
            ctx.viewport(|v| {
                for command in &v.commands {
                    if let ViewportCommand::Visible(visible) = command {
                        self.hidden = !visible;
                    }
                }
            });
        }
        if self.hidden {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
}

/// AppKit's application hide preserves Dock reopening; ordering the last window out does not.
pub fn hide(ctx: &Context) {
    #[cfg(all(target_os = "macos", not(test)))]
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("mailcraft.tray.hidden"), true));
        objc2_app_kit::NSApplication::sharedApplication(mtm).hide(None);
        return;
    }
    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
}

#[cfg(target_os = "macos")]
pub fn unhide_app() {
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        objc2_app_kit::NSApplication::sharedApplication(mtm).unhide(None);
    }
}

#[cfg(target_os = "macos")]
fn app_hidden() -> bool {
    objc2::MainThreadMarker::new().is_some_and(|mtm| objc2_app_kit::NSApplication::sharedApplication(mtm).isHidden())
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "windows",
    target_os = "macos"
)))]
mod backend {
    pub struct Backend;
    impl Backend {
        pub fn start(_: std::sync::Arc<super::Signals>) -> Self {
            Self
        }
        pub fn poll(&mut self) {}
        pub fn ready(&self) -> bool {
            false
        }
    }
}

#[cfg(test)]
#[path = "tray_tests.rs"]
mod tests;
