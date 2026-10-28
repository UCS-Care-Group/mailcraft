// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening a mailbox (#519).
//!
//! **At launch** the window comes up at once and the host reads the mailbox on a thread of its
//! own ([`Opening`]). Until it has arrived the window is an [`MailApp::opening`]: it shows the
//! opening screen (our logo, how many bytes are read of how many, then "Getting ready…") once
//! opening has taken longer than a moment, and nothing else. It can be moved, resized and closed,
//! and that is all:
//! - every command is refused, and every request of the control channel (agents, the MCP bridge)
//!   is answered [`STILL_OPENING`], but `app.status`, which says how far it has got;
//! - files and links handed to the window (a second start, Finder, the command line) wait, and
//!   are opened once the mailbox is;
//! - nothing starts: no Send/Receive, no Inbox pass, no word index, no pictures;
//! - closing it writes nothing (no mailbox, no settings): nothing has changed.
//!
//! A mailbox that couldn't be read, or was damaged or repaired, says so on the same screen with a
//! Continue button ([`Trouble`]); after it the window carries on as it always did with such a
//! mailbox. The host then builds the window as it would have with the mailbox in hand ([`Build`]).
//!
//! **Later** (File › Open, a mailbox file dropped or handed over), the host reads the other file
//! in the background ([`crate::Services::read_ahead`]) while the mailbox that is open stays in
//! use; the status bar says how far it has got, with Cancel (`ui.cancelOpen`). Once it is read,
//! `file.open` runs as it always did (what is unsaved is written first) and takes what was read.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use egui::{Rect, Ui, pos2, vec2};
use mailcraft_engine::io::OpenProgress;
use mailcraft_engine::{BodyRead, Missing};
use mailcraft_model::Mailbox;
use serde_json::{Value, json};

use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, i18n};

/// What every command and control request is answered while the mailbox opens.
pub const STILL_OPENING: &str = "the mailbox is still opening";

/// How long opening may take before the opening screen shows (a quick open never shows it).
pub const SHOW_AFTER_MS: f64 = 300.0;

/// How many files and links handed over while opening are kept to open after it.
const MAX_QUEUED: usize = 64;

/// Why the owner is told something about the mailbox before the window carries on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// The file couldn't be read (the reason, in the engine's words): the window starts with an
    /// empty mailbox that keeps nothing, so the file stays as it is.
    Unreadable(String),
    /// Reading it stopped before it was done (the reading thread ended unexpectedly): as
    /// unreadable.
    Stopped,
    /// Part of the file was damaged and left out.
    Damaged,
    /// So many things in it didn't fit together and were put right.
    Repaired(usize),
}

/// The mailbox the window starts with, as the host read it.
pub struct Arrival {
    pub mailbox: Mailbox,
    /// The file it is kept in (none: nowhere, or it couldn't be read, so nothing is written).
    pub path: Option<PathBuf>,
    /// Bodies and attachments that couldn't be read back from the content folder.
    pub unavailable: usize,
    /// The content folder's files left on disk are still to be looked for: those missing aren't
    /// in `unavailable` yet (`mailcraft_engine::store::check_parts`, which the host runs once the
    /// window shows, [`MailApp::check_parts_later`]).
    pub unchecked: bool,
    /// Part of the mailbox file itself was damaged and left out.
    pub damaged: bool,
    /// What the owner is told first, if anything.
    pub trouble: Option<Trouble>,
}

impl Arrival {
    /// A mailbox read without trouble (or one that isn't kept anywhere, `path` none).
    pub fn opened(mailbox: Mailbox, path: Option<PathBuf>) -> Arrival {
        Arrival { mailbox, path, unavailable: 0, unchecked: false, damaged: false, trouble: None }
    }

    /// The file couldn't be read (`trouble` says why): an empty mailbox, kept nowhere.
    pub fn unreadable(trouble: Trouble) -> Arrival {
        Arrival { mailbox: Mailbox::with_local_account("", ""), path: None, unavailable: 0, unchecked: false, damaged: false, trouble: Some(trouble) }
    }
}

/// How the host makes the window once its mailbox is there: what it would have done at launch
/// with the mailbox in hand.
pub type Build = Box<dyn FnOnce(Arrival) -> MailApp>;

/// A window waiting for the mailbox it starts with ([`MailApp::opening`]).
pub struct Opening {
    rx: Receiver<Arrival>,
    progress: Arc<OpenProgress>,
    build: Option<Build>,
    /// When the window first waited ([`crate::now_ms`]).
    since: Option<f64>,
    /// The mailbox, while the owner is told what was wrong with it.
    held: Option<Arrival>,
    /// Files and links handed to the window meanwhile.
    queued: Vec<String>,
}

impl Opening {
    /// Wait for the mailbox the host reads and sends through `rx`, counting what it reads in
    /// `progress`; `build` makes the window once it has arrived.
    pub fn new(rx: Receiver<Arrival>, progress: Arc<OpenProgress>, build: Build) -> Opening {
        Opening { rx, progress, build: Some(build), since: None, held: None, queued: Vec::new() }
    }

    /// What `app.status` says while the window waits.
    fn status(&self) -> Value {
        let stage = if self.held.is_some() {
            "problem"
        } else if self.progress.reading_done() {
            "gettingReady"
        } else {
            "reading"
        };
        json!({"mailbox": "opening", "stage": stage, "read": self.progress.read(), "total": self.progress.total()})
    }
}

impl MailApp {
    /// A window whose mailbox is still being read ([`Opening`]), in the owner's interface
    /// settings (`prefs`: the theme, the language and the size of the interface are there from
    /// the first frame) and the system's language (`system_locale`).
    pub fn opening(opening: Opening, prefs: Option<crate::UiState>, system_locale: Option<String>) -> MailApp {
        let mut app = MailApp::new(mailcraft_engine::Session::new(Mailbox::with_local_account("", "")), crate::Services::default());
        app.services.system_locale = system_locale;
        if let Some(prefs) = prefs {
            app.apply_prefs(prefs);
        }
        // Nothing of the window waiting is ever written.
        app.autosave = false;
        app.auto_sync = false;
        app.opening = Some(opening);
        app
    }

    /// The window is still waiting for its mailbox.
    pub fn is_opening(&self) -> bool {
        self.opening.is_some()
    }
}

/// Once a frame while the window waits: answer the control channel, and take the mailbox when it
/// has arrived.
pub(crate) fn logic(app: &mut MailApp, ctx: &egui::Context) {
    if app.ctx.is_none() {
        app.ctx = Some(ctx.clone());
    }
    answer_control(app);
    let Some(o) = app.opening.as_mut() else { return };
    o.since.get_or_insert_with(crate::now_ms);
    if o.held.is_none() {
        match o.rx.try_recv() {
            Ok(arrival) if arrival.trouble.is_some() => o.held = Some(arrival),
            Ok(arrival) => return arrive(app, arrival),
            Err(TryRecvError::Empty) => {}
            // The host's thread ended without a word.
            Err(TryRecvError::Disconnected) => o.held = Some(Arrival::unreadable(Trouble::Stopped)),
        }
    }
    if o.held.is_none() {
        // The bytes read move on, and the screen comes after a moment.
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// What the window takes up once it shows its mailbox, so that the first frames come first
/// (#519): the content files found missing ([`MailApp::check_parts_later`]), the bodies left on
/// disk ([`MailApp::take_bodies_later`]), and
/// the word index ([`MailApp::index_words_later`]).
#[derive(Default)]
pub(crate) struct Later {
    /// The content files to come that are missing, the bodies and attachments already known to
    /// be, and which opening of the session it is about ([`mailcraft_engine::Session::openings`]).
    parts: Option<(Receiver<Missing>, Said, u64)>,
    /// The bodies the opening left for later, read in batches, and which opening they are of.
    bodies: Option<(Receiver<Vec<BodyRead>>, u64)>,
    /// Frames still to be drawn before the word index starts.
    words_in: Option<u32>,
}

/// How many bodies and attachments the opening found missing, and whether the count is to be said
/// (not when the mailbox file itself was damaged: that is said instead).
type Said = (usize, bool);

/// Frames the window draws with its mailbox before the word index starts.
pub const WORDS_AFTER_FRAMES: u32 = 5;

/// Batches of bodies taken in a frame, at most: the window goes on drawing meanwhile.
pub const BODY_BATCHES_A_FRAME: usize = 4;

impl MailApp {
    /// Say how many bodies and attachments are missing from the mailbox's content folder once
    /// the host's look for them (`rx`) is over, `known` of them having been found while opening
    /// (only when `say`); the bodies among them read as unavailable from then on
    /// ([`mailcraft_engine::Session::mark_unavailable`]). About the mailbox open now: one that
    /// takes its place before then hears nothing of it.
    pub fn check_parts_later(&mut self, rx: Receiver<Missing>, known: usize, say: bool) {
        self.later.parts = Some((rx, (known, say), self.session.openings()));
    }

    /// Take in the bodies the opening left on disk as the host reads them (`rx`, in batches),
    /// a few batches a frame ([`mailcraft_engine::Session::take_bodies`]). About the mailbox open
    /// now, as [`MailApp::check_parts_later`].
    pub fn take_bodies_later(&mut self, rx: Receiver<Vec<BodyRead>>) {
        self.later.bodies = Some((rx, self.session.openings()));
    }

    /// Start the word index of the mailbox's file ([`mailcraft_engine::Session::index_words`])
    /// once the window has drawn its first frames with the mailbox.
    pub fn index_words_later(&mut self) {
        self.session.index_words = false;
        self.later.words_in = Some(WORDS_AFTER_FRAMES);
    }
}

/// Once a frame with the mailbox shown: take up what was left for after the first frames.
pub(crate) fn after(app: &mut MailApp, ctx: &egui::Context) {
    if let Some(frames) = app.later.words_in.as_mut() {
        if *frames == 0 {
            app.later.words_in = None;
            app.session.index_words = true;
        } else {
            *frames -= 1;
        }
        // (A window with nothing to do draws no more frames by itself.)
        ctx.request_repaint();
    }
    let opening = app.session.openings();
    // Another mailbox took the place of the one these are about: they are dropped unread.
    if app.later.parts.as_ref().is_some_and(|(_, _, of)| *of != opening) {
        app.later.parts = None;
    }
    if app.later.bodies.as_ref().is_some_and(|(_, of)| *of != opening) {
        app.later.bodies = None;
    }
    // What is missing first: a body found missing is never taken in after it.
    if let Some((rx, (known, say), _)) = &app.later.parts {
        let say = *say;
        let found = match rx.try_recv() {
            Ok(missing) => Some(missing),
            Err(TryRecvError::Empty) => None,
            // The look stopped without a word: what was known is said.
            Err(TryRecvError::Disconnected) => Some(Missing::default()),
        };
        if let Some(found) = found {
            let missing = known.saturating_add(found.count);
            app.later.parts = None;
            app.session.mark_unavailable(&found.refs);
            if say && missing > 0 {
                app.status(crate::tln!(
                    missing,
                    "{n} body or attachment is missing from the mailbox's content folder, or damaged.",
                    "{n} bodies or attachments are missing from the mailbox's content folder, or damaged."
                ));
            }
        }
    }
    // Bodies only once what is missing is known.
    if app.later.parts.is_none()
        && let Some((rx, _)) = &app.later.bodies
    {
        let mut batches = Vec::new();
        let mut done = false;
        while batches.len() < BODY_BATCHES_A_FRAME {
            match rx.try_recv() {
                Ok(batch) => batches.push(batch),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
            }
        }
        if done {
            app.later.bodies = None;
        } else if batches.len() == BODY_BATCHES_A_FRAME {
            // More may be waiting.
            ctx.request_repaint();
        }
        for batch in batches {
            app.session.take_bodies(batch);
        }
    }
}

/// Every request of the control channel while the window waits: `app.status` says how far it
/// has got, anything else that the mailbox is still opening.
fn answer_control(app: &mut MailApp) {
    let Some(rx) = app.control_rx.take() else { return };
    while let Ok(req) = rx.try_recv() {
        let answer = match (req.method.as_str(), &app.opening) {
            ("app.status", Some(o)) => json!({"ok": true, "result": o.status()}),
            _ => json!({"ok": false, "error": STILL_OPENING}),
        };
        let _ = req.reply.send(answer);
    }
    app.control_rx = Some(rx);
}

/// A file or link handed to the window while it waits: kept to open after.
pub(crate) fn queue(app: &mut MailApp, path: &str) -> bool {
    let Some(o) = app.opening.as_mut() else { return false };
    if o.queued.len() < MAX_QUEUED {
        o.queued.push(path.to_string());
    } else {
        log::warn!("more than {MAX_QUEUED} files were handed over while the mailbox opened; the rest are left");
    }
    true
}

/// The owner pressed Continue after reading what was wrong with the mailbox.
pub(crate) fn go_on(app: &mut MailApp) {
    let Some(arrival) = app.opening.as_mut().and_then(|o| o.held.take()) else { return };
    arrive(app, arrival);
}

/// The mailbox is here: the host makes the window, which takes over what the waiting one had
/// (the control channel, where the window is, the look already in force), and the files handed
/// over meanwhile are opened.
fn arrive(app: &mut MailApp, arrival: Arrival) {
    let Some(mut o) = app.opening.take() else { return };
    let Some(build) = o.build.take() else { return };
    let mut next = build(arrival);
    if next.control_rx.is_none() {
        next.control_rx = app.control_rx.take();
    }
    next.ui.window = app.ui.window.or(next.ui.window);
    next.ctx = app.ctx.take();
    next.styled = app.styled;
    next.fonts_frames = app.fonts_frames;
    next.applied_theme = app.applied_theme;
    *app = next;
    for what in o.queued {
        app.open_from_system(&what);
    }
}

/// The window while it waits: the canvas, then, after a moment, the opening screen.
pub(crate) fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let mut continued = false;
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.canvas)).show(ui, |ui| {
        let Some(o) = &app.opening else { return };
        let waited = crate::now_ms() - o.since.unwrap_or_else(crate::now_ms);
        if o.held.is_none() && waited < SHOW_AFTER_MS {
            return;
        }
        let area = ui.max_rect();
        let width = 440.0f32.min(area.width() - 32.0).max(160.0);
        let top = area.center().y - 150.0;
        let column =
            Rect::from_min_max(pos2(area.center().x - width / 2.0, top.max(area.min.y + 16.0)), pos2(area.center().x + width / 2.0, area.max.y));
        ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
            ui.vertical_centered(|ui| {
                let (mark_rect, _) = ui.allocate_exact_size(vec2(72.0, 72.0), egui::Sense::hover());
                if let Some(logo) = logo(ui.ctx()) {
                    egui::Image::new(&logo).fit_to_exact_size(mark_rect.size()).paint_at(ui, mark_rect);
                }
                ui.add_space(22.0);
                match &o.held {
                    Some(arrival) => continued = problem(ui, &t, arrival.trouble.as_ref()),
                    None => progress(ui, &t, &o.progress),
                }
            });
        });
    });
    if continued {
        go_on(app);
    }
}

/// How far opening has got: the bytes read of the file's, then "Getting ready…".
fn progress(ui: &mut Ui, t: &Tokens, progress: &OpenProgress) {
    let total = progress.total();
    if total == 0 {
        // Where reading can't say how far it has got (the browser's storage).
        ui.label(egui::RichText::new(tl!("Opening…")).font(semibold(18.0)).color(t.text));
        return;
    }
    ui.label(egui::RichText::new(tl!("Opening your mailbox…")).font(semibold(18.0)).color(t.text));
    ui.add_space(16.0);
    let read = progress.read().min(total);
    let fraction = (read as f64 / total as f64) as f32;
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width().min(320.0), 6.0), egui::Sense::hover());
    ui.painter().rect_filled(bar, 3.0, if t.dark { t.border } else { t.border_strong.linear_multiply(0.6) });
    if fraction > 0.0 {
        let filled = Rect::from_min_max(bar.min, pos2(bar.min.x + (bar.width() * fraction).max(6.0), bar.max.y));
        ui.painter().rect_filled(filled, 3.0, t.accent);
    }
    ui.add_space(10.0);
    let said = if progress.reading_done() {
        tl!("Getting ready…").to_string()
    } else {
        i18n::fmt(tl!("{read} of {total} MB"), &[("read", &megabytes(read, total)), ("total", &megabytes(total, total))])
    };
    ui.label(egui::RichText::new(said).font(regular(13.0)).color(t.text_dim));
}

/// `bytes` in megabytes, as precisely as `total` needs: a small file in tenths.
pub(crate) fn megabytes(bytes: u64, total: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if total < 10_000_000 { format!("{mb:.1}") } else { format!("{}", mb.round() as u64) }
}

/// What was wrong with the mailbox, and Continue: whether it was pressed.
fn problem(ui: &mut Ui, t: &Tokens, trouble: Option<&Trouble>) -> bool {
    let (heading, why, then) = match trouble {
        Some(Trouble::Unreadable(why)) => (
            tl!("Your mailbox couldn't be opened"),
            tl!(why).to_string(),
            tl!("MailCraft starts with an empty mailbox and leaves the file as it is."),
        ),
        Some(Trouble::Stopped) | None => (
            tl!("Your mailbox couldn't be opened"),
            tl!("MailCraft stopped reading it before it was done.").to_string(),
            tl!("MailCraft starts with an empty mailbox and leaves the file as it is."),
        ),
        Some(Trouble::Damaged) => {
            (tl!("Part of your mailbox is damaged"), crate::damaged_mailbox_notice(), tl!("Everything else in it opens as usual."))
        }
        Some(Trouble::Repaired(n)) => (
            tl!("Your mailbox was repaired"),
            tln!(*n, "{n} thing in it didn't fit with the rest and was put right.", "{n} things in it didn't fit with the rest and were put right."),
            tl!("Everything else in it opens as usual."),
        ),
    };
    ui.label(egui::RichText::new(heading).font(semibold(18.0)).color(t.text));
    ui.add_space(12.0);
    ui.label(egui::RichText::new(why).font(regular(13.0)).color(t.text));
    ui.add_space(8.0);
    ui.label(egui::RichText::new(then).font(regular(13.0)).color(t.text_dim));
    ui.add_space(20.0);
    crate::widgets::primary_button(ui, tl!("Continue"), true).clicked()
}

/// MailCraft's logo, the app icon itself, as a texture made once and kept in the context.
/// `None` when the picture can't be read (the screen then shows no mark).
pub(crate) fn logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    /// The 256 px icon every build ships (`assets/app-icon`, attributed in `ATTRIBUTION.md`).
    const PNG: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/io.github.billsecond.mailcraft.png");
    let id = egui::Id::unique("mailcraft-opening-logo");
    if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(texture);
    }
    let rgba = image::load_from_memory_with_format(PNG, image::ImageFormat::Png).ok()?.into_rgba8();
    let size = [usize::try_from(rgba.width()).ok()?, usize::try_from(rgba.height()).ok()?];
    let texture = ctx.load_texture("mailcraft-logo", egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    Some(texture)
}

// ---- a mailbox opened later, in the background ----

/// Another mailbox file being read while the one open stays in use (File › Open).
pub(crate) struct ReadAhead {
    path: PathBuf,
    /// Its file's name, for the status bar.
    name: String,
    progress: Arc<OpenProgress>,
    rx: Receiver<Result<(), String>>,
}

/// Whether `file.open` with `params`, asked for by the owner, reads the file in the background
/// first: a file on disk, other than the one open, where the host can read ahead.
pub(crate) fn reads_ahead(app: &MailApp, params: &Value) -> bool {
    if app.services.read_ahead.is_none() || app.swapping || app.running_after_save || params.get("data").is_some() {
        return false;
    }
    let Some(path) = params.get("path").and_then(Value::as_str) else { return false };
    // Never the mailbox open now, however its path is spelt.
    let whole = |p: &std::path::Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    app.session.path.as_deref().map(whole) != Some(whole(std::path::Path::new(path)))
}

/// Start reading the file `params` names in the background, in place of any other being read.
pub(crate) fn read_ahead(app: &mut MailApp, params: &Value) -> Result<Value, String> {
    let path = PathBuf::from(params.get("path").and_then(Value::as_str).unwrap_or_default());
    cancel_ahead(app);
    // An earlier one, read and waiting for the save before it opens, gives way to this one.
    if app.after_save.as_ref().is_some_and(|(id, _)| id == "file.open")
        && let Some((id, waited)) = app.after_save.take()
    {
        forget_unopened(&id, &waited);
        app.after_save_outcome = Some(json!({"command": id, "ok": false, "error": "file.open was asked for instead"}));
    }
    let Some(start) = &app.services.read_ahead else { return Err("this window can't open mailboxes in the background".into()) };
    let progress = Arc::new(OpenProgress::default());
    let rx = start(path.clone(), progress.clone());
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string_lossy().to_string());
    app.reading_ahead = Some(ReadAhead { path: path.clone(), name, progress, rx });
    Ok(json!({"pending": "opening", "path": path.to_string_lossy()}))
}

/// Give up the mailbox being read in the background, and what was read of it (`ui.cancelOpen`).
pub(crate) fn cancel_ahead(app: &mut MailApp) -> bool {
    let Some(ahead) = app.reading_ahead.take() else { return false };
    ahead.progress.cancel();
    #[cfg(not(target_arch = "wasm32"))]
    mailcraft_engine::store::forget_ahead(&ahead.path);
    true
}

/// A `file.open` that read its file ahead and then didn't run (the save it waited for failed, or
/// something else was asked for instead): what was read of it is let go of.
pub(crate) fn forget_unopened(id: &str, params: &Value) {
    #[cfg(not(target_arch = "wasm32"))]
    if id == "file.open"
        && let Some(path) = params.get("path").and_then(Value::as_str)
    {
        mailcraft_engine::store::forget_ahead(std::path::Path::new(path));
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (id, params);
}

/// Once a frame: the mailbox read in the background, once it is, takes the place of the one open
/// through `file.open`, which writes what is unsaved first, as it always did.
pub(crate) fn poll_ahead(app: &mut MailApp, ctx: &egui::Context) {
    let Some(ahead) = &app.reading_ahead else { return };
    let read = match ahead.rx.try_recv() {
        Err(TryRecvError::Empty) => {
            ctx.request_repaint_after(Duration::from_millis(200));
            return;
        }
        Ok(read) => read,
        Err(TryRecvError::Disconnected) => Err(tl!("MailCraft stopped reading it before it was done.").to_string()),
    };
    let Some(ahead) = app.reading_ahead.take() else { return };
    match read {
        Ok(()) => {
            app.swapping = true;
            let r = app.run("file.open", json!({"path": ahead.path.to_string_lossy()}));
            app.swapping = false;
            // Not opened: what was read of it isn't kept for later.
            if r.is_err() {
                #[cfg(not(target_arch = "wasm32"))]
                mailcraft_engine::store::forget_ahead(&ahead.path);
            }
        }
        Err(e) => {
            let said = i18n::fmt(tl!("Couldn't open {name}: {error}"), &[("name", &ahead.name), ("error", tl!(&e))]);
            app.status(said);
        }
    }
}

/// The status bar's word on the mailbox being read in the background: "Opening Archive.mailcraft…
/// 45%" and its bar, then without the share once it is all read. `None`: there is none.
pub(crate) fn ahead_label(app: &MailApp) -> Option<(String, f32)> {
    let ahead = app.reading_ahead.as_ref()?;
    let (read, total) = (ahead.progress.read(), ahead.progress.total());
    if total == 0 || ahead.progress.reading_done() {
        return Some((i18n::fmt(tl!("Opening {name}…"), &[("name", &ahead.name)]), 1.0));
    }
    let fraction = (read.min(total) as f64 / total as f64) as f32;
    let percent = ((fraction * 100.0).floor() as u32).min(100);
    Some((i18n::fmt(tl!("Opening {name}… {percent}%"), &[("name", &ahead.name), ("percent", &percent.to_string())]), fraction))
}

/// What `app.status` and `ui.inspect` say of the mailbox being read in the background.
pub(crate) fn ahead_inspect(app: &MailApp) -> Value {
    match &app.reading_ahead {
        // The file's name alone: where it is kept is nobody else's business.
        Some(a) => json!({"name": a.name, "read": a.progress.read(), "total": a.progress.total()}),
        None => Value::Null,
    }
}

#[cfg(test)]
#[path = "opening_tests.rs"]
mod opening_tests;
