// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Explain the first close before the desktop host hides the window.

use egui::{Context, Id};

use crate::MailApp;

fn pending() -> Id {
    Id::unique("mailcraft.closeToTrayNotice")
}

/// A close-button request only: an explicit Quit never asks about hiding.
pub fn confirm_hide(ctx: &Context, app: &MailApp) -> bool {
    if app.ui.close_to_tray_notice_seen {
        return true;
    }
    ctx.data_mut(|d| d.insert_temp(pending(), true));
    ctx.request_repaint();
    false
}

/// The host rechecks tray availability after the first-close notice has been accepted.
pub fn take_hide_request(ctx: &Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(Id::unique("mailcraft.hideAfterTrayNotice"))).unwrap_or(false)
}

/// An explicit Quit dismisses the explanation before any unsaved-work question is shown.
pub fn cancel_notice(ctx: &Context) {
    ctx.data_mut(|d| d.remove::<bool>(pending()));
}

pub(crate) fn show(ctx: &Context, app: &mut MailApp) {
    if !ctx.data(|d| d.get_temp::<bool>(pending())).unwrap_or(false) {
        return;
    }
    if !app.ui.close_to_tray {
        ctx.data_mut(|d| d.remove::<bool>(pending()));
        return;
    }
    let mut hide = false;
    let mut cancel = false;
    let modal = egui::Modal::new(pending()).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(tl!("MailCraft will keep running"));
        ui.label(tl!(
            "Mail and reminders will keep running. To bring the window back, click MailCraft's system tray icon and choose Show MailCraft. On Windows, the icon may be under the arrow for hidden icons beside the clock. On macOS, use the menu-bar icon or click MailCraft in the Dock. Choose Quit MailCraft from the icon's menu to exit."
        ));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            hide = ui.button(tl!("Continue in tray")).clicked();
            cancel = ui.button(tl!("Cancel")).clicked();
        });
    });
    if hide {
        app.ui.close_to_tray_notice_seen = true;
        ctx.data_mut(|d| d.remove::<bool>(pending()));
        // This is a request to hide, not to quit if the tray disappears in the meantime.
        ctx.data_mut(|d| d.insert_temp(Id::unique("mailcraft.hideAfterTrayNotice"), true));
        ctx.request_repaint();
    } else if cancel || modal.should_close() {
        ctx.data_mut(|d| d.remove::<bool>(pending()));
    }
}
