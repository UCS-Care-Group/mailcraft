// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Drawing without the graphics card. When the window is drawn by a software rasteriser (the
//! host says so: the adapter eframe got is a processor), every frame costs processor time, so
//! this module (a) says so once in a line above the status bar and (b) holds the window's own
//! animation and progress repaints to a few a second. Input still repaints at once: only the
//! repaints MailCraft asks for itself go through [`repaint_after`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use egui::{Context, Ui};
use serde_json::{Value, json};

use crate::MailApp;

/// The shortest wait between two repaints MailCraft asks for itself while drawing in software
/// (about four a second).
pub const SOFTWARE_MIN_WAIT: Duration = Duration::from_millis(250);

static SOFTWARE: AtomicBool = AtomicBool::new(false);

/// Whether the window is being drawn by the processor.
pub fn is_software() -> bool {
    SOFTWARE.load(Ordering::Relaxed)
}

/// Called by the host at start, once it knows the adapter in use.
pub fn set_software(on: bool) {
    SOFTWARE.store(on, Ordering::Relaxed);
}

/// The wait to use for a repaint asked for after `wanted`: as asked, or no shorter than
/// [`SOFTWARE_MIN_WAIT`] while drawing in software.
pub fn capped(wanted: Duration, software: bool) -> Duration {
    if software { wanted.max(SOFTWARE_MIN_WAIT) } else { wanted }
}

/// `ctx.request_repaint_after(wanted)`, held to a few a second while drawing in software.
pub fn repaint_after(ctx: &Context, wanted: Duration) {
    ctx.request_repaint_after(capped(wanted, is_software()));
}

/// Whether the line should be on screen.
pub(crate) fn notice_due(software: bool, told: bool) -> bool {
    software && !told
}

/// The line, until the owner puts it away (kept in the settings, so it is said once).
pub(crate) fn bar(app: &mut MailApp, ui: &mut Ui) {
    if !notice_due(app.software_drawing, app.ui.software_told) {
        return;
    }
    let said = tl!(
        "MailCraft is drawing without the graphics card, which uses a lot of processor time. Installing or updating the graphics driver usually fixes this."
    );
    crate::updates::bar(app, ui, "software_drawing", said, &[("softwareNoticeOk", tl!("OK"), false)], |app, _| {
        let _ = app.run("ui.dismissSoftwareNotice", json!({}));
    });
}

/// `ui.dismissSoftwareNotice`: puts the line away for good.
pub(crate) fn dismiss(app: &mut MailApp) -> Result<Value, String> {
    let was = !std::mem::replace(&mut app.ui.software_told, true);
    Ok(json!({"dismissed": was}))
}

#[cfg(test)]
#[path = "software_tests.rs"]
mod tests;
