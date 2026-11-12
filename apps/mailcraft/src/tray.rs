// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Windows notification-area lifetime. A failed tray never makes the window inaccessible.
use egui::{Context, ViewportCommand as V};
use mailcraft_ui_egui::{
    MailApp,
    i18n::{self, Lang},
};
use std::sync::mpsc::{Receiver, channel};
use tray_icon::{
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

enum Action {
    Show,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    actions: Receiver<Action>,
    state: WindowState,
    show: MenuItem,
    quit: MenuItem,
    language: Lang,
}

struct WindowState {
    hidden: bool,
    start_hidden: bool,
    quitting: bool,
}

impl Tray {
    pub fn new(ctx: &Context, start_hidden: bool) -> Option<Self> {
        let made = || -> Result<Self, Box<dyn std::error::Error>> {
            let image = crate::app_icon().ok_or("the window icon could not be loaded")?;
            let icon = tray_icon::Icon::from_rgba(image.rgba, image.width, image.height)?;
            let menu = Menu::new();
            let show = MenuItem::with_id("mailcraft.tray.show", i18n::t("Open"), true, None);
            let quit = MenuItem::with_id("mailcraft.tray.quit", i18n::t("Quit MailCraft"), true, None);
            menu.append_items(&[&show, &quit])?;
            let icon =
                TrayIconBuilder::new().with_tooltip("MailCraft").with_icon(icon).with_menu(Box::new(menu)).with_menu_on_left_click(false).build()?;
            let (send, actions) = channel();
            let wake = ctx.clone();
            let click = send.clone();
            TrayIconEvent::set_event_handler(Some(move |event| {
                if matches!(event, TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }) {
                    let _ = click.send(Action::Show);
                    wake.request_repaint();
                }
            }));
            let wake = ctx.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let action = match event.id.as_ref() {
                    "mailcraft.tray.show" => Action::Show,
                    "mailcraft.tray.quit" => Action::Quit,
                    _ => return,
                };
                let _ = send.send(action);
                wake.request_repaint();
            }));
            Ok(Self {
                _icon: icon,
                actions,
                show,
                quit,
                language: i18n::current(),
                state: WindowState { hidden: false, start_hidden, quitting: false },
            })
        };
        made().map_err(|e| log::warn!("notification area unavailable: {e}")).ok()
    }

    pub fn on_frame(&mut self, ctx: &Context, app: &mut MailApp) -> bool {
        if self.language != i18n::current() {
            self.language = i18n::current();
            self.show.set_text(i18n::t("Open"));
            self.quit.set_text(i18n::t("Quit MailCraft"));
        }
        self.state.on_frame(ctx, app, &self.actions)
    }

    pub fn after_frame(&mut self, ctx: &Context) {
        self.state.after_frame(ctx);
    }
}

impl WindowState {
    /// `true` holds the close button; explicit Quit continues through the existing unsaved-work checks.
    fn on_frame(&mut self, ctx: &Context, app: &mut MailApp, actions: &Receiver<Action>) -> bool {
        for _ in 0..32 {
            let Ok(action) = actions.try_recv() else { break };
            self.hidden = false;
            ctx.send_viewport_cmd(V::Visible(true));
            ctx.send_viewport_cmd(V::Minimized(false));
            ctx.send_viewport_cmd(V::Focus);
            if matches!(action, Action::Quit) {
                // Ask through MailApp so unsaved drafts retain their existing confirmation.
                ctx.send_viewport_cmd(V::Close);
                self.quitting = true;
            }
        }
        if app.quit_requested {
            self.quitting = true;
        }
        let restoring = ctx.viewport(|v| v.commands.contains(&V::Visible(true)));
        if restoring {
            self.hidden = false;
            self.start_hidden = false;
        }
        let close = ctx.input(|i| i.viewport().close_requested());
        let minimized = ctx.input(|i| i.viewport().minimized == Some(true));
        if !restoring && !self.quitting && !app.quit_requested && (close || minimized || self.start_hidden) {
            if close {
                ctx.send_viewport_cmd(V::CancelClose);
            }
            ctx.send_viewport_cmd(V::Minimized(false));
            ctx.send_viewport_cmd(V::Visible(false));
            self.hidden = true;
            self.start_hidden = false;
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return true;
        }
        // Keep receiving mail and service the tray while no window is visible.
        if self.hidden {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        false
    }

    pub fn after_frame(&mut self, ctx: &Context) {
        if ctx.viewport(|v| v.commands.contains(&V::CancelClose)) {
            self.quitting = false;
        }
    }
}

#[cfg(test)]
#[path = "tray_tests.rs"]
mod tests;
