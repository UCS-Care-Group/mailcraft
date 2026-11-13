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
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            ctx.request_repaint();
        }
        "mailcraft.tray.quit" => crate::quit::request(ctx),
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
    backend: backend::Backend,
    signals: Arc<Signals>,
    hidden: bool,
    can_hide: bool,
}

impl Tray {
    /// Called on the GUI thread after eframe has created its window and event loop.
    pub fn new(ctx: &Context, can_hide: bool) -> Self {
        let signals = Arc::new(Signals { pending: AtomicU8::new(0), online: AtomicBool::new(false), ctx: ctx.clone() });
        let backend = backend::Backend::start(signals.clone());
        Self { backend, signals, hidden: false, can_hide }
    }

    pub fn available(&self) -> bool {
        self.can_hide && self.signals.online.load(Ordering::Relaxed) && self.backend.ready()
    }

    pub fn show(&mut self, ctx: &Context) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.request_repaint();
        self.hidden = false;
    }

    /// After the app's logic: tray actions and existing requests to focus the window (a
    /// notification, another launch, an unsaved-work question) restore a hidden window too.
    pub fn pump(&mut self, ctx: &Context, app: &mut MailApp) {
        self.backend.poll();
        let pending = self.signals.pending.swap(0, Ordering::Relaxed);
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

    pub fn after_frame(&mut self, ctx: &Context) {
        ctx.viewport(|v| {
            for command in &v.commands {
                if let ViewportCommand::Visible(visible) = command {
                    self.hidden = !visible;
                }
            }
        });
        if self.hidden {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
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
