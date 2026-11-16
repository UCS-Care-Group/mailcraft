// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Letting go of memory while the main window is out of sight ([`window_out_of_sight`]):
//! minimised, wholly covered as the system says, or hidden by the program
//! ([`MailApp::window_hidden`]), never because it only lacks the keyboard. Once it has been out of
//! sight for [`AFTER`] seconds, what is kept only to show things quickly again goes: the bodies
//! laid out, the pictures' textures and bytes, the pictures from the web, senders' pictures, the
//! faces read for messages, the content read from disk, and the bodies and attachments brought
//! into messages, from their Undo steps too ([`mailcraft_engine::Session::let_go_of_content`]),
//! except those of messages open in a window of their own or being written. Everything is read
//! again when it is next shown.
//!
//! Nothing goes while a Send/Receive, a download of messages, a save, an import or a sending is
//! under way: it is tried again a while later. Pictures and faces on their way are not waited
//! for, and what they bring is not kept. While the window is out
//! of sight the toolkit runs its logic and not its frames, so this runs there
//! ([`MailApp::logic`]) and asks for the logic to run again when the time is up. A texture let go
//! of leaves the graphics card at the next frame painted.
//!
//! `MAILCRAFT_FLUSH_AFTER_SECS` sets another wait, in seconds (for measuring,
//! `docs/control-protocol.md`).

use std::collections::HashSet;

use mailcraft_model::Id;

use crate::MailApp;

/// Seconds the main window is out of sight before memory is let go of.
pub const AFTER: f64 = 180.0;

/// Messages let go of in one run of the logic: the rest in the next runs.
const SLICE: usize = 256;

/// How long a wait is before trying again while something is at work, in seconds.
const BUSY_AGAIN: f64 = 30.0;

/// Where letting go of memory is.
#[derive(Default)]
pub struct OutOfSight {
    /// When the window was first seen out of sight, in milliseconds ([`crate::now_ms`]: the
    /// toolkit's clock stands still while only the logic runs).
    since: Option<f64>,
    /// The caches were let go of, and the bodies are let go of from the message after this one.
    bodies_from: Option<Id>,
    caches: bool,
    /// Everything was let go of since the window went out of sight.
    done: bool,
}

impl OutOfSight {
    /// Whether everything was let go of since the window went out of sight.
    pub fn done(&self) -> bool {
        self.done
    }
}

/// The wait: [`AFTER`], or `MAILCRAFT_FLUSH_AFTER_SECS` where that is a number of seconds.
fn after() -> f64 {
    static AFTER_SET: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *AFTER_SET.get_or_init(|| wait_from(std::env::var("MAILCRAFT_FLUSH_AFTER_SECS").ok().as_deref()))
}

/// The wait `set` asks for: a number of seconds, at most a day; [`AFTER`] otherwise.
pub(crate) fn wait_from(set: Option<&str>) -> f64 {
    set.and_then(|s| s.trim().parse::<f64>().ok()).filter(|s| s.is_finite() && *s >= 0.0).map_or(AFTER, |s| s.min(86_400.0))
}

/// Whether the main window is out of sight: minimised, covered whole as the system says, or
/// hidden by the program (`hidden`). Not when it only lacks the keyboard.
pub fn window_out_of_sight(ctx: &egui::Context, hidden: bool) -> bool {
    hidden || ctx.input(|i| i.raw.viewports.get(&egui::ViewportId::ROOT).is_some_and(|v| v.minimized == Some(true) || v.occluded == Some(true)))
}

/// What is at work with what would be let go of, if anything: then nothing goes yet.
fn busy(app: &MailApp) -> Option<&'static str> {
    let at_work = [
        (app.job_busy() != mailcraft_engine::sync_driver::Busy::default(), "a job"),
        (app.syncing(), "Send/Receive"),
        (app.fetching.busy(), "a download of messages"),
        (app.save_progress.is_some(), "a save"),
        (app.session.importing_pst(), "an import"),
        (app.send_gate.is_some(), "a sending"),
    ];
    at_work.into_iter().find(|(busy, _)| *busy).map(|(_, what)| what)
}

/// The messages whose content stays in memory: those open in a window of their own, and those
/// the messages being written come from or are kept as.
fn kept(app: &MailApp) -> HashSet<Id> {
    let mut keep: HashSet<Id> = app.windows.messages().into_iter().collect();
    for c in app.session.compose.iter().chain(&app.session.composing) {
        keep.extend(c.source);
        keep.extend(c.draft);
        keep.extend(c.also.iter().copied());
    }
    keep
}

/// At each run of the logic: once the main window has been out of sight for [`after`] seconds,
/// let go of what is kept only to show things quickly again, a slice at a time. Coming back into
/// sight starts the wait again.
pub fn tick(app: &mut MailApp, ctx: &egui::Context) {
    if !window_out_of_sight(ctx, app.window_hidden) {
        app.out_of_sight = OutOfSight::default();
        return;
    }
    let now = crate::now_ms();
    tick_at(app, ctx, now, after());
}

/// [`tick`] at the time `now` (in milliseconds) with a wait of `wait` seconds, the window out of
/// sight.
pub(crate) fn tick_at(app: &mut MailApp, ctx: &egui::Context, now: f64, wait: f64) {
    if app.out_of_sight.done {
        return;
    }
    let since = *app.out_of_sight.since.get_or_insert(now);
    let left = wait - (now - since) / 1000.0;
    if left > 0.0 {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(left.min(86_400.0) + 0.5));
        return;
    }
    if let Some(what) = busy(app) {
        log::debug!("the window is out of sight; waiting for {what} before letting go of memory");
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(BUSY_AGAIN));
        return;
    }
    if !app.out_of_sight.caches {
        let_go_of_caches(app, ctx);
        app.out_of_sight.caches = true;
    }
    let keep = kept(app);
    match app.session.let_go_of_content(app.out_of_sight.bodies_from, &keep, SLICE) {
        Some(at) => {
            app.out_of_sight.bodies_from = Some(at);
            ctx.request_repaint();
        }
        None => {
            app.out_of_sight.done = true;
            #[cfg(not(target_arch = "wasm32"))]
            mailcraft_engine::store::clear_cache();
            log::info!("the window is out of sight: what was kept to show things again quickly was let go of");
        }
    }
}

/// Let go of the caches kept to show things again quickly.
fn let_go_of_caches(app: &mut MailApp, ctx: &egui::Context) {
    crate::reading::let_go(app, ctx);
    app.sender_images.let_go_of_pictures();
}
