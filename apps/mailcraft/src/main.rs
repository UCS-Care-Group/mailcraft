// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft desktop app.
//!
//! Usage: `mailcraft [--control <port> [--control-agents-only]] [--sample] [--mailbox <file>] [files or mailto: links…]`
//!
//! The mailbox lives in `<settings>/mailbox.mailcraft` (or the file `--mailbox` names) and is saved
//! as you go. `--sample` opens the sample mailbox instead and keeps nothing. On the command line
//! a `mailto:` link starts a new message, a `.eml` file opens in a window of its own, a `.ics` or
//! `.vcf` file is imported once the owner says so, and a `.mbox` file is imported
//! (`MailApp::open_from_system`). A mailbox is open in one window only: started again, MailCraft
//! hands its files and links to the window that is already open, which comes to the front, and
//! exits (`single_instance`, which also says which runs take part).
//!
//! `--control <port>` (or `MAILCRAFT_CONTROL_PORT`) starts a localhost JSON-lines control server:
//! `{"id":1,"key":"…","method":"engine.execute","params":{"command":"mail.markAllRead","params":{}}}`.
//! Every request needs the key the app writes to `<settings>/control-key.<instance>` (mode 0600,
//! removed on exit). See `docs/control-protocol.md`. `--control-agents-only` (or
//! `MAILCRAFT_CONTROL_AGENTS_ONLY=1`) takes every request for an agent's, allowed nothing in
//! advance, so that an agent that can read the key can't pass for the owner's script.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod alerts;
#[cfg(target_os = "macos")]
mod apple_events;
#[cfg(test)]
mod approval_flow;
mod attachment_render;
mod block_on;
mod browser;
#[cfg(test)]
mod calendar_flow;
mod clipboard;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod clipboard_mime;
#[cfg(test)]
mod control_challenge_flow;
mod control_server;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
mod desktop_icons;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
mod desktop_theme;
mod diagnostics;
#[cfg(test)]
mod diagnostics_problem_tests;
#[cfg(test)]
mod ews_mail_flow;
mod ews_runner;
mod ews_setup;
#[cfg(test)]
mod ews_setup_flow;
#[cfg(test)]
mod ews_up_flow;
#[cfg(test)]
mod favorites_flow;
#[cfg(test)]
mod favorites_organisation_flow;
mod file_dialogs;
mod file_launcher;
// OpenPGP keys kept in GnuPG, through gpg-agent (#30): signing and decrypting (parts 5 and 6) use it.
#[cfg_attr(not(test), expect(dead_code, reason = "#30 part 5 signs through it"))]
mod gpg_agent;
#[cfg(test)]
mod gpg_agent_tests;
#[cfg(test)]
mod graph_folders_flow;
#[cfg(test)]
mod graph_mail_flow;
#[cfg(any(target_os = "windows", test))]
mod graphics;
mod install;
#[cfg(test)]
mod jmap_mail_flow;
mod key_holders;
mod launch;
#[cfg(test)]
mod listed_twice_flow;
mod logging;
#[cfg(target_os = "macos")]
mod menubar;
mod opened_files;
#[cfg(test)]
mod opened_flow;
mod own_keys;
mod picture_cache;
mod quit;
#[cfg(target_os = "macos")]
mod quit_macos;
#[cfg(test)]
mod rules_job_tests;
mod secrets;
mod self_update;
mod sender_images;
#[cfg(test)]
mod settings_unread_tests;
#[cfg(test)]
mod shared_calendar_flow;
#[cfg(test)]
mod shared_send_flow;
mod single_instance;
mod system_locale;
mod system_motion;
#[cfg(test)]
mod token_renew_flow;
mod tray;
#[cfg(test)]
mod unconfirmed_flow;
#[cfg(test)]
mod update_flow;
mod updater;
mod wipe;

use std::path::PathBuf;

use mailcraft_engine::Session;
#[cfg(test)]
use mailcraft_host::calendar::check_calendar_in;
#[cfg(test)]
use mailcraft_host::graph::{graph_in, graph_in_with, graph_sending_with, graph_with};
use mailcraft_model::sync::Tokens;
use mailcraft_ui_egui::{
    AdminConsentAnswer, AdminConsentRequest, CalendarCheckRequest, FreeBusyRequest, GraphRequest, MailApp, Services, SettingsUnread, SignInRequest,
    UiState,
    window_geometry::{self, WindowGeometry, take_rescue},
};

/// The app, the restored window geometry until the first frame has checked it, the control
/// server's key file (removed on exit), the file dialogs the app asked for, the hold on the
/// mailbox that later starts hand their files and links to, a copy that updates itself's part in
/// that (installing at exit) and, on macOS, the files opened from Finder and the menu bar.
struct App(
    MailApp,
    Option<WindowGeometry>,
    Option<control_server::KeyFile>,
    file_dialogs::Launcher,
    Option<single_instance::Guard>,
    Option<HostUpdates>,
    tray::Tray,
    #[cfg(target_os = "macos")] fmv_macos_events::Inbox,
    #[cfg(target_os = "macos")] Option<menubar::MenuBar>,
);

/// A copy that updates itself (`self_update`): where it keeps what it needs, what it is, and what
/// the download checked in this run.
#[derive(Clone)]
struct HostUpdates {
    folders: self_update::Folders,
    install: install::Install,
    verified: self_update::Shared,
}

impl HostUpdates {
    /// For a copy that updates itself and is one of the project's packages, in a run that keeps
    /// its settings.
    fn of_this_copy() -> Option<HostUpdates> {
        if build_sha() == "dev" || !prefs_enabled() {
            return None;
        }
        let install = install::this_copy();
        if install == install::Install::NoticeOnly {
            return None;
        }
        let folders = self_update::Folders::new(mailcraft_control_key::settings_dir()?);
        Some(HostUpdates { folders, install, verified: Default::default() })
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        apple_events::poll(&self.7, &mut self.0, ctx);
        // MailCraft was started again (the shortcut, a `mailto:` link, a file opened from the
        // file manager): what it was started with opens here, and the window comes forward.
        if let Some(asked) = self.4.as_ref().and_then(single_instance::Guard::handed_over) {
            for what in &asked.open {
                self.0.open_from_system(what);
            }
            self.0.note_left_alone(asked.refused);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            // Where the system won't let a window step in front by itself, its button says so.
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
        // The menu bar's choices run before the frame's own work (macOS).
        #[cfg(target_os = "macos")]
        if let Some(bar) = &mut self.8 {
            bar.pump(&mut self.0, ctx);
        }
        self.0.logic(ctx);
        // Another mailbox file opened in the window (or File › New): the lock follows it (#517).
        if let Some(guard) = &self.4
            && !self.0.is_opening()
        {
            guard.follow(self.0.session.path.as_deref());
        }
        self.3.show(frame);
        let prev = self.0.ui.window;
        self.0.ui.window = ctx.input(|i| WindowGeometry::track(prev, i.viewport(), i.viewport_rect().size()));
        // A window restored onto a monitor that is gone is moved onto the main one (#719): the
        // monitors are asked for only until that check is done.
        if self.1.is_some() {
            let monitors = connected_monitors(ctx, frame);
            if let Some(pos) = ctx.input(|i| take_rescue(&mut self.1, i.viewport(), &monitors)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            }
        }
        // A window of its own about to open is put on a monitor that is connected (#286).
        if self.0.wants_monitors() {
            self.0.set_monitors(monitors_in_pixels(frame));
        }
        // Pages to open, with the browser brought in front (`browser`).
        open_pages(ctx, frame);
        // Leaving (the close button, Quit, the Dock): a message being written asks first, then
        // the platform's own way out (`quit`). Last in `logic`, which runs while minimized too.
        self.6.pump(ctx, &mut self.0);
        quit::on_frame(ctx, &mut self.0, self.6.available());
        self.6.after_frame(ctx);
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        // On Linux and the BSDs, the desktop's light or dark preference (`desktop_theme`), for
        // "Use system setting": winit has none there.
        #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
        if let Some(desktop) = DESKTOP_THEME.get() {
            desktop.apply(raw);
        }
        // Under Wayland the clipboard keys come as key events, to be turned into cut, copy and
        // paste here, where a picture on the clipboard can be seen (`mailcraft_ui_egui::paste_keys`, #1217).
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if self.0.services.clipboard_keys {
            mailcraft_ui_egui::paste_keys::translate(&mut raw.events, self.0.services.clipboard.as_deref());
        }
        self.0.raw_input_hook(raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.0.ui(ui);
        self.3.show(frame);
        open_pages(ui.ctx(), frame);
    }
    fn on_exit(&mut self) {
        // A window closed while its mailbox was still opening has nothing to save (#519).
        if !self.0.is_opening() {
            save_prefs(&self.0);
            // The last changes, if the timer hadn't got to them yet, and whatever the background
            // saver is still writing.
            if let Err(e) = self.0.save_on_exit() {
                log::error!("saving the mailbox on exit failed: {e}");
            }
        }
        // The attachments opened in this run (#126).
        if let Some(opened) = OPENED.get() {
            opened.clear();
        }
        // What is still to be noted of the pictures kept on disk.
        if let Some(pictures) = PICTURES.get().filter(|_| !self.0.data_removed) {
            pictures.flush();
        }
        if let Some(key_file) = &self.2 {
            key_file.remove();
        }
        if let Some(guard) = &self.4 {
            guard.close();
        }
        // A new version downloaded and checked is installed now (and started, when the owner
        // pressed Restart to update), or the version before is put back (`self_update`).
        // (Not once MailCraft's data was removed: that would write into the folder again, and
        // could install and start another version after the owner asked for everything to go.)
        if let Some(updates) = self.5.as_ref().filter(|_| wipe::may_install_at_exit(self.0.data_removed)) {
            let running = env!("CARGO_PKG_VERSION");
            self_update::settle(&updates.folders, running);
            if let Some(what) = self.0.update_on_exit() {
                let verified = updates.verified.lock().map(|v| v.clone()).unwrap_or_else(|e| e.into_inner().clone());
                match self_update::on_exit(&updates.folders, updates.install, what, verified, self.0.session.path.as_deref()) {
                    Ok(()) => log::info!("the new version of MailCraft is being installed"),
                    Err(e) => log::error!("the new version of MailCraft couldn't be installed: {e}"),
                }
            }
        }
        // Written and done with: the mailbox's memory goes with the process instead of being
        // handed back message by message with the window still showing (a fifth of a second for
        // 64,000 messages, #586).
        std::mem::forget(std::mem::take(&mut self.0.session.mailbox));
    }
}

/// `ui.json` in the settings folder, which the MCP bridge also uses to find the control key.
fn prefs_path() -> Option<PathBuf> {
    mailcraft_control_key::settings_dir().map(|b| b.join("ui.json"))
}

/// Where the mailbox is kept unless `--mailbox` says otherwise.
fn default_mailbox_path() -> Option<PathBuf> {
    mailcraft_control_key::settings_dir().map(|b| b.join(format!("mailbox.{}", mailcraft_engine::io::MAILBOX_EXT)))
}

/// The pictures from the web that are kept between runs (`picture_cache`): in the settings
/// folder, beside the mailbox's own content folder. None for the sample mailbox or a run
/// without preferences, which keep nothing; and none is made until a picture is kept.
static PICTURES: std::sync::OnceLock<std::sync::Arc<picture_cache::Cache>> = std::sync::OnceLock::new();

/// Start keeping downloaded pictures in the settings folder, and tidy up there a moment later,
/// off the window's thread.
fn keep_pictures() -> Option<std::sync::Arc<picture_cache::Cache>> {
    let dir = mailcraft_control_key::settings_dir()?.join(picture_cache::FOLDER);
    let cache = PICTURES.get_or_init(|| picture_cache::Cache::at(dir)).clone();
    let tidying = cache.clone();
    let tidy = move || {
        std::thread::sleep(std::time::Duration::from_secs(20));
        tidying.tidy();
    };
    if let Err(e) = std::thread::Builder::new().name("picture tidying".into()).spawn(tidy) {
        log::debug!("the downloaded pictures were not tidied: {e}");
    }
    Some(cache)
}

/// The attachments the owner opened in this run (`opened_files`), in this user's cache folder.
static OPENED: std::sync::OnceLock<opened_files::Opened> = std::sync::OnceLock::new();

/// Where opened attachments are written, and what earlier runs left there a day ago and more
/// tidied a moment later, off the window's thread. None when there is no cache folder.
fn opened_files() -> Option<&'static opened_files::Opened> {
    let base = opened_files::base()?;
    let opened = OPENED.get_or_init(|| opened_files::Opened::at(base));
    let tidy = || {
        std::thread::sleep(std::time::Duration::from_secs(30));
        if let Some(opened) = OPENED.get() {
            opened.tidy(opened_files::MAX_AGE);
        }
    };
    if let Err(e) = std::thread::Builder::new().name("opened attachments tidying".into()).spawn(tidy) {
        log::debug!("the opened attachments were not tidied: {e}");
    }
    Some(opened)
}

/// Where the log files live: `logs` in the preferences folder (see `logging`).
fn log_dir() -> Option<PathBuf> {
    Some(prefs_path()?.parent()?.join("logs"))
}

/// Runs without preferences (`MAILCRAFT_NO_PREFS`, agents' test runs) neither read nor write them,
/// and keep their mailbox in memory only.
fn prefs_enabled() -> bool {
    std::env::var_os("MAILCRAFT_NO_PREFS").is_none()
}

/// File › Info › Open MailCraft's data folder (#460): the settings folder (the one Remove
/// MailCraft's data clears first), and the system's own opener for whichever folder the window
/// shows: that one, or the open mailbox's own when it is kept elsewhere. None for a run that keeps
/// nothing.
fn data_folder(keeps_data: bool) -> Option<mailcraft_ui_egui::data_removal::DataFolder> {
    let path = mailcraft_control_key::settings_dir().filter(|_| keeps_data)?;
    Some(mailcraft_ui_egui::data_removal::DataFolder { path, open: Box::new(file_launcher::open_folder) })
}

/// The preferences, read once at start (before the window opens, for its size and place), and
/// what became of the file when it couldn't be read. Nothing for a run without preferences.
fn load_prefs() -> (Option<UiState>, Option<SettingsUnread>) {
    match prefs_path() {
        Some(path) if prefs_enabled() => read_prefs(&path),
        _ => (None, None),
    }
}

/// A settings file that couldn't be set aside stays where it was, and this run's settings
/// aren't saved over it.
static PREFS_LEFT_IN_PLACE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The preferences in `path`. A file that can't be read as them (damaged, or written by a version
/// that keeps them otherwise) is moved aside to `ui.json.damaged` (or `.damaged-2` and on, so an
/// earlier one is kept), so the next save doesn't overwrite what the owner had set and it can
/// still be looked at; the window says so. One that can't be read at all, or can't be moved
/// aside, is left where it is and not saved over (the window says which). The log names the kind of error and where in
/// the file it is, never what the file says.
fn read_prefs(path: &std::path::Path) -> (Option<UiState>, Option<SettingsUnread>) {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (None, None),
        Err(e) => {
            log::warn!("the settings file couldn't be read ({:?}); it is left as it is and not saved over", e.kind());
            PREFS_LEFT_IN_PLACE.store(true, std::sync::atomic::Ordering::Relaxed);
            return (None, Some(SettingsUnread::CouldntRead));
        }
    };
    let e = match serde_json::from_slice::<UiState>(&bytes) {
        Ok(ui) => return (Some(ui), None),
        Err(e) => e,
    };
    let at = format!("{:?} error at line {}, column {}", e.classify(), e.line(), e.column());
    match set_aside(path) {
        Ok(aside) => {
            let name = aside.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            log::warn!("the settings file couldn't be read ({at}); set aside as {name}, started with MailCraft's own settings");
            (None, Some(SettingsUnread::SetAside(name)))
        }
        Err(_) => {
            log::warn!("the settings file couldn't be read ({at}) nor set aside; it is left as it is and not saved over");
            PREFS_LEFT_IN_PLACE.store(true, std::sync::atomic::Ordering::Relaxed);
            (None, Some(SettingsUnread::LeftInPlace))
        }
    }
}

/// How many damaged copies of the settings are kept; past that the oldest is replaced.
const MAX_DAMAGED_COPIES: usize = 20;

/// Move the settings file at `path` to `<file>.damaged` (or `.damaged-2` and on) without
/// replacing anything already there: a name is taken when anything is on it, a link that leads
/// nowhere included (`symlink_metadata`, which doesn't follow it, and which only an absent name
/// answers with an error). When all the names are taken, the oldest plain file among them is
/// replaced, so settings go on being saved however often the file has been damaged.
fn set_aside(path: &std::path::Path) -> std::io::Result<PathBuf> {
    let name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    let mut oldest: Option<(std::time::SystemTime, PathBuf)> = None;
    for n in 1..=MAX_DAMAGED_COPIES {
        let mut aside = name.clone();
        aside.push(if n == 1 { ".damaged".to_string() } else { format!(".damaged-{n}") });
        let aside = path.with_file_name(aside);
        match std::fs::symlink_metadata(&aside) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::rename(path, &aside)?;
                return Ok(aside);
            }
            Err(e) => return Err(e),
            Ok(meta) if meta.is_file() => {
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                if oldest.as_ref().is_none_or(|(t, _)| modified < *t) {
                    oldest = Some((modified, aside));
                }
            }
            Ok(_) => {}
        }
    }
    let (_, aside) = oldest.ok_or_else(|| std::io::Error::other("no name is free for a damaged copy"))?;
    std::fs::rename(path, &aside)?;
    Ok(aside)
}

/// Write the preferences so that a crash leaves the old file or the whole new one, readable by
/// its owner only.
fn write_prefs(path: &std::path::Path, ui: &UiState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        mailcraft_engine::store::create_private_dir_all(dir)?;
    }
    mailcraft_engine::store::write_atomic(path, &serde_json::to_vec_pretty(ui).map_err(std::io::Error::other)?)
}

fn save_prefs(app: &MailApp) {
    if let Some(p) = prefs_path() {
        save_prefs_at(app, &p);
    }
}

/// [`save_prefs`] to `path`.
fn save_prefs_at(app: &MailApp, path: &std::path::Path) {
    // Its data was removed from this computer (File › Info): nothing is written back. Nor is
    // anything of a window closed while its mailbox was still opening (#519).
    if !prefs_enabled() || app.data_removed || app.is_opening() || PREFS_LEFT_IN_PLACE.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let _ = write_prefs(path, &app.prefs());
}

/// Run `work` on a background thread of its own; the answer comes back through the channel and
/// wakes the window. A dropped sender (the thread couldn't start) reads as "stopped unexpectedly".
fn background<T: Send + 'static>(ctx: &egui::Context, name: &str, work: impl FnOnce() -> T + Send + 'static) -> std::sync::mpsc::Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = ctx.clone();
    let spawned = std::thread::Builder::new().name(name.into()).spawn(move || {
        let _ = tx.send(work());
        wake.request_repaint();
    });
    if let Err(e) = spawned {
        log::error!("couldn't start {name}: {e}");
    }
    rx
}

/// A job on the host's threads, or, when the host didn't start, on a thread of its own as
/// `background` does: the jobs never stop for want of it.
fn host_job<T: Send + 'static>(
    host: Option<&std::sync::Arc<mailcraft_host::Host>>,
    ctx: &egui::Context,
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::sync::mpsc::Receiver<T> {
    match host {
        Some(host) => {
            let wake = ctx.clone();
            host.run_job(name, move |_platform| work(), move || wake.request_repaint())
        }
        None => background(ctx, name, work),
    }
}

/// The application (client) id this build presents to Microsoft's sign-in page: the
/// `MAILCRAFT_MS_CLIENT_ID` environment variable when the app starts, else the one the build was
/// given under the same name. An identifier, not a secret; empty when there is none.
fn microsoft_client_id() -> String {
    let built_in = option_env!("MAILCRAFT_MS_CLIENT_ID").unwrap_or_default().to_string();
    let id = std::env::var("MAILCRAFT_MS_CLIENT_ID").unwrap_or(built_in);
    let id = id.trim();
    if id.len() <= 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c)) { id.to_string() } else { String::new() }
}

/// The MailCraft-owned Google Desktop registration. Its built-in client secret is a
/// public installed-app identifier, NEVER an account's password or OAuth refresh token.
fn google_client_id() -> String {
    let built_in = option_env!("MAILCRAFT_GOOGLE_CLIENT_ID").unwrap_or_default();
    let id = std::env::var("MAILCRAFT_GOOGLE_CLIENT_ID").unwrap_or_else(|_| built_in.into());
    let id = id.trim();
    if id.len() <= 200 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')) { id.to_string() } else { String::new() }
}

fn google_client_secret() -> String {
    let built_in = option_env!("MAILCRAFT_GOOGLE_CLIENT_SECRET").unwrap_or_default();
    let secret = std::env::var("MAILCRAFT_GOOGLE_CLIENT_SECRET").unwrap_or_else(|_| built_in.into());
    if secret.len() <= 512 && secret.bytes().all(|b| b.is_ascii_graphic()) { secret } else { String::new() }
}

/// Open one of the provider's pages in the system's browser: whether a browser could be started.
/// (Made here, rather than through the window's own links, so that a browser that can't be
/// started is known of, instead of leaving the owner looking at a page that waits.) On Linux and
/// FreeBSD the window hands the browser its activation token, so that it comes in front
/// (`browser`). The address is not for the log: it names who is signing in.
fn open_in_browser(url: &str) -> bool {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    if let Some(browser) = BROWSER.get() {
        return browser.open_and_wait(url);
    }
    browser::system_open(url)
}

/// The desktop's light or dark preference, read from its portal and followed (`desktop_theme`),
/// set once as the window is made.
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
static DESKTOP_THEME: std::sync::OnceLock<desktop_theme::DesktopTheme> = std::sync::OnceLock::new();

/// The monitors connected now, in the points the window's place is in, from the window eframe made
/// (egui doesn't list them). None while there is no window; where the system lists none, none.
fn connected_monitors(ctx: &egui::Context, frame: &eframe::Frame) -> Vec<egui::Rect> {
    let Some(window) = frame.winit_window() else { return Vec::new() };
    // As egui turns the window's own rect into points (`egui_winit::pixels_per_point`).
    let pixels_per_point = ctx.zoom_factor() * window.scale_factor() as f32;
    let listed = window.available_monitors().map(|m| {
        let (at, size) = (m.position(), m.size());
        ([at.x, at.y], [size.width, size.height])
    });
    window_geometry::monitors_in_points(listed, pixels_per_point)
}

/// The monitors connected now, in the system's pixels with each one's scale, for the windows of
/// their own about to open (#286): one space for every monitor, whatever its scale.
fn monitors_in_pixels(frame: &eframe::Frame) -> Vec<mailcraft_ui_egui::window_places::Monitor> {
    let Some(window) = frame.winit_window() else { return Vec::new() };
    window
        .available_monitors()
        .take(64)
        .map(|m| {
            let (at, size) = (m.position(), m.size());
            // (In points on macOS, where winit gives each monitor's place by its own scale.)
            mailcraft_ui_egui::window_places::monitor(
                [at.x, at.y],
                [size.width, size.height],
                m.scale_factor(),
                mailcraft_ui_egui::window_places::POINTS,
            )
        })
        .collect()
}

/// The pages waiting for the window's activation token (`browser`), set once before the window opens.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
static BROWSER: std::sync::OnceLock<browser::Browser<winit::event_loop::AsyncRequestSerial>> = std::sync::OnceLock::new();

/// The links the window followed in this frame, and the pages asked for since the last: each
/// asks winit for an activation token, and opens when it comes (`WithTokens`) or is overdue.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn open_pages(ctx: &egui::Context, frame: &eframe::Frame) {
    use winit::platform::startup_notify::WindowExtStartupNotify as _;
    let Some(browser) = BROWSER.get() else { return };
    browser.follow(browser::take_links(ctx));
    let window = frame.winit_window();
    let mut request = || window.and_then(|w| w.request_activation_token().ok());
    if let Some(next) = browser.pump(&mut request, std::time::Instant::now()) {
        ctx.request_repaint_after(next);
    }
}

/// Everywhere else the system's own way brings the browser forward; the links are taken out of
/// the frame all the same, so that each is checked (`browser::normalised`) on the way out and
/// nothing reaches the opener unchecked (AU2-V1).
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn open_pages(ctx: &egui::Context, _frame: &eframe::Frame) {
    for url in browser::take_links(ctx) {
        // A thread of its own: the opener can take a moment. No thread, no page.
        let _ = std::thread::Builder::new().name("browser".into()).spawn(move || browser::system_open(&url));
    }
}

/// eframe's own handling of winit's events, and the activation tokens the window asked for
/// (`open_pages`), which eframe would drop, handed to the pages waiting for them.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
struct WithTokens<'a>(eframe::EframeWinitApplication<'a>);

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl winit::application::ApplicationHandler<eframe::UserEvent> for WithTokens<'_> {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.0.resumed(event_loop);
    }
    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        match event {
            winit::event::WindowEvent::ActivationTokenDone { serial, token } => {
                if let Some(browser) = BROWSER.get() {
                    browser.token_arrived(&serial, token.into_raw());
                }
            }
            event => self.0.window_event(event_loop, window_id, event),
        }
    }
    fn new_events(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, cause: winit::event::StartCause) {
        self.0.new_events(event_loop, cause);
    }
    fn user_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, event: eframe::UserEvent) {
        self.0.user_event(event_loop, event);
    }
    fn device_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, device_id: winit::event::DeviceId, event: winit::event::DeviceEvent) {
        self.0.device_event(event_loop, device_id, event);
    }
    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.0.about_to_wait(event_loop);
    }
    fn suspended(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.0.suspended(event_loop);
    }
    fn exiting(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.0.exiting(event_loop);
    }
    fn memory_warning(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.0.memory_warning(event_loop);
    }
}

/// Run the window: on Linux and FreeBSD on an event loop of our own, so that the activation
/// tokens reach `WithTokens`; elsewhere as eframe runs it.
fn run_window(options: eframe::NativeOptions, creator: eframe::AppCreator<'_>, close_to_tray: bool) -> eframe::Result {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        let _ = BROWSER.set(browser::Browser::new(std::sync::Arc::new(browser::Desktop)));
        let mut builder = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event();
        // winit 0.30's native Wayland set_visible is a no-op. X11 (including XWayland)
        // lets a close really hide the window, removing its taskbar button. Both backends
        // are already built; pure Wayland still runs, with close-to-tray unavailable.
        if tray::prefer_x11(close_to_tray, tray::x11_reachable) {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_x11();
        }
        let event_loop = builder.build()?;
        // eframe keeps a start-up failure to itself on this path: the app is only created once
        // the window is, so a loop that ends without having created it failed to start.
        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = started.clone();
        let creator: eframe::AppCreator<'_> = Box::new(move |cc| {
            seen.store(true, std::sync::atomic::Ordering::Relaxed);
            creator(cc)
        });
        let mut app = WithTokens(eframe::create_native("MailCraft", options, creator, &event_loop));
        event_loop.run_app(&mut app)?;
        if started.load(std::sync::atomic::Ordering::Relaxed) {
            Ok(())
        } else {
            Err(eframe::Error::AppCreation("the window couldn't be started (see the log)".into()))
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let _ = close_to_tray;
        eframe::run_native("MailCraft", options, creator)
    }
}

/// Sign an account's owner in: quietly with what was kept from last time when there is any, on
/// the provider's page in the system's browser otherwise.
fn sign_in(ctx: &egui::Context, request: SignInRequest) -> Result<Tokens, String> {
    sign_in_with(request, &|url| {
        let opened = open_in_browser(url);
        ctx.request_repaint();
        opened
    })
}

/// [`sign_in`], with `open_page` in the browser's place: it opens the address it is given, and
/// says whether it could.
///
/// A sign-in on the provider's page that ends without tokens is answered in the words the
/// window knows each way by (`mailcraft_ui_egui::approval`): an administrator has to approve
/// first, the page came back with a no, nothing came back, no browser could be started.
/// Anything else is said as it is.
fn sign_in_with(request: SignInRequest, open_page: &dyn Fn(&str) -> bool) -> Result<Tokens, String> {
    use std::sync::atomic::{AtomicBool, Ordering};

    use mailcraft_net::oauth::{Endpoints, Resource, SignInEnd};
    use mailcraft_ui_egui::approval::{
        GOOGLE_REAUTH_REQUIRED, GOOGLE_SIGN_IN_BLOCKED, GOOGLE_SIGN_IN_DECLINED, GOOGLE_SIGN_IN_TIMED_OUT, NEEDS_APPROVAL, NO_BROWSER,
        SIGN_IN_DECLINED, SIGN_IN_TIMED_OUT,
    };
    // No browser could be started: nothing can come back, so the wait is called off at once,
    // and that is what is said.
    let no_browser = AtomicBool::new(false);
    let open = |url: &str| {
        if !open_page(url) {
            no_browser.store(true, Ordering::Relaxed);
            request.cancel.store(true, Ordering::Relaxed);
        }
    };
    let unopened = |said: String| if no_browser.load(Ordering::Relaxed) { NO_BROWSER.to_string() } else { said };
    // "Show Outlook's Favorites": the owner asked, so the page comes up, for what Microsoft Graph
    // was asked for before and the two permissions that reading the Favorites takes. A no that
    // is not the owner's to change is said in the words the window knows it by; nothing is
    // asked a second time.
    if request.graph && request.favorites {
        let asked = mailcraft_net::oauth::sign_in_for_favorites(&request.oauth, &request.email, &open, &request.cancel);
        return asked.map_err(|refusal| unopened(favorites_refusal(refusal)));
    }
    // A shared mailbox is to be read (#70): the page comes up for what reading shared mailboxes
    // takes, by itself, in the same words for a no as the Favorites'.
    // A shared mailbox's calendar was ticked for the first time, and the owner said go ahead
    // (#70): the page comes up for what reading shared mailboxes' calendars takes, by itself.
    if request.graph && request.shared && request.shared_calendars {
        let asked = mailcraft_net::oauth::sign_in_for_shared_calendars(&request.oauth, &request.email, &open, &request.cancel);
        return asked.map_err(|refusal| unopened(favorites_refusal(refusal)));
    }
    // Mail is to be sent from a shared mailbox for the first time, and the owner said go ahead
    // (#70): the page comes up for what sending from shared mailboxes takes too, by itself.
    if request.graph && request.shared && request.shared_send {
        let asked = mailcraft_net::oauth::sign_in_for_shared_send(&request.oauth, &request.email, &open, &request.cancel);
        return asked.map_err(|refusal| unopened(favorites_refusal(refusal)));
    }
    // A tasks run found the sign-in doesn't cover the To Do lists (#248) and the owner said go
    // ahead: the page comes up for that permission alone, by itself.
    if request.graph && request.tasks {
        let asked = mailcraft_net::oauth::sign_in_for_tasks(&request.oauth, &request.email, &open, &request.cancel);
        return asked.map_err(|refusal| unopened(favorites_refusal(refusal)));
    }
    // A rule is to go to the server, the sign-in doesn't cover changing the rules (#566), and the
    // owner said go ahead: the page comes up for that permission alone, by itself.
    if request.graph && request.rules {
        let asked = mailcraft_net::oauth::sign_in_for_rules(&request.oauth, &request.email, &open, &request.cancel);
        return asked.map_err(|refusal| unopened(favorites_refusal(refusal)));
    }
    // A change of the automatic replies found the sign-in doesn't cover changing the mailbox's
    // settings, and the owner said go ahead: the page comes up for that permission alone.
    if request.graph && request.replies {
        let asked = mailcraft_net::oauth::sign_in_for_replies(&request.oauth, &request.email, &open, &request.cancel);
        return asked.map_err(|refusal| unopened(favorites_refusal(refusal)));
    }
    // Calendars are opened by address for the first time (#70): the page comes up for the same
    // one permission, without what reading shared mailboxes' mail takes, by itself.
    if request.graph && request.shared_calendars {
        let asked = mailcraft_net::oauth::sign_in_for_opened_calendars(&request.oauth, &request.email, &open, &request.cancel);
        return asked.map_err(|refusal| unopened(favorites_refusal(refusal)));
    }
    if request.graph && request.shared {
        let asked = mailcraft_net::oauth::sign_in_for_shared(&request.oauth, &request.email, &open, &request.cancel);
        return asked.map_err(|refusal| unopened(favorites_refusal(refusal)));
    }
    // A sign-in on the page, at `endpoints`: how it ended, when it brought no tokens. (How, and
    // never in whose words, is what the log is told.)
    let on_the_page = |endpoints: &Endpoints| {
        let asked = if request.oauth.provider == mailcraft_model::OAuthProvider::Google {
            mailcraft_net::oauth::sign_in_google_explained_at(
                endpoints,
                &request.oauth,
                &request.google_client_secret,
                &request.email,
                &open,
                &request.cancel,
            )
        } else {
            mailcraft_net::oauth::sign_in_explained(endpoints, &request.oauth, &request.email, &open, &request.cancel)
        };
        asked.map_err(|end| {
            let google = request.oauth.provider == mailcraft_model::OAuthProvider::Google;
            let (how, said) = match end {
                _ if no_browser.load(Ordering::Relaxed) => ("no browser could be started", NO_BROWSER.to_string()),
                SignInEnd::Approval if google => ("Google policy prevented authorization", GOOGLE_SIGN_IN_BLOCKED.to_string()),
                SignInEnd::Approval => ("an administrator has to approve the application first", NEEDS_APPROVAL.to_string()),
                SignInEnd::Declined if google => ("Google's page came back with a no", GOOGLE_SIGN_IN_DECLINED.to_string()),
                SignInEnd::Declined => ("the page came back with a no", SIGN_IN_DECLINED.to_string()),
                SignInEnd::TimedOut if google => ("Google did not complete sign-in", GOOGLE_SIGN_IN_TIMED_OUT.to_string()),
                SignInEnd::TimedOut => ("nothing came back in time", SIGN_IN_TIMED_OUT.to_string()),
                SignInEnd::Cancelled => ("it was called off", mailcraft_net::NetError::SignInCancelled.to_string()),
                // Never hand a potentially sensitive provider response to the window or log.
                SignInEnd::Other(_) if google => ("it failed", "Google could not complete sign-in. Please retry or use an app password.".into()),
                SignInEnd::Other(e) => ("it failed", e.to_string()),
            };
            // The failure category is enough; no account identifier belongs in this log.
            log::info!("sign-in on the provider's page: {how}");
            said
        })
    };
    // "Connect calendar and contacts": the owner asked, so the page comes up, for what Microsoft
    // Graph needs rather than for mail.
    if request.graph {
        // (For an account kept in step through Graph, its mail too: R3.)
        let resource = if request.mail_write { Resource::GraphMail } else { Resource::Graph };
        return on_the_page(&Endpoints::for_resource(&request.oauth, resource));
    }
    if !request.refresh.is_empty() {
        let refreshed = if request.oauth.provider == mailcraft_model::OAuthProvider::Google {
            mailcraft_net::oauth::refresh_google_at(&Endpoints::of(&request.oauth), &request.oauth, &request.google_client_secret, &request.refresh)
        } else {
            mailcraft_net::oauth::refresh(&request.oauth, &request.refresh)
        };
        match refreshed {
            Ok(tokens) => return Ok(tokens),
            // The machine-readable invalid_grant response confirms this Google refresh
            // credential was revoked. Only a quiet attempt returns the structured signal to
            // erase this account's old stored grant: interactive attempts proceed to the browser.
            Err(e) if e.is_revoked_google_refresh() && request.quiet => return Err(GOOGLE_REAUTH_REQUIRED.to_string()),
            // A refused refresh needs a fresh browser sign-in if the owner asked for it.
            Err(e) if e.is_refused_sign_in() => {}
            // A network or server failure must not erase a still-usable refresh token or echo
            // an untrusted provider error containing secret material.
            Err(_) if request.oauth.provider == mailcraft_model::OAuthProvider::Google => {
                return Err("Google sign-in could not reach the authorization server. Retry later.".into());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    // Nobody asked for this run: no page is put in front of the owner.
    if request.quiet {
        if request.oauth.provider == mailcraft_model::OAuthProvider::Google {
            return Err("Google needs a new sign-in. Use Send/Receive to reconnect.".into());
        }
        return Err(format!("The sign-in for {} has run out. Use Send/Receive to sign in again.", request.email));
    }
    if request.mail_only {
        // Without the calendar and contacts along: for when the provider's page won't have both.
        let mail = Endpoints { consent: String::new(), lesser: String::new(), ..Endpoints::of(&request.oauth) };
        return on_the_page(&mail);
    }
    on_the_page(&Endpoints::of(&request.oauth))
}

/// Have an administrator approve this program for an organisation: Microsoft's admin-consent
/// page in the system's browser, and the wait for its yes or no. Only ever because the owner
/// said "I'm an administrator". No tokens come of it, and nothing is believed of what comes
/// back but the yes or the no.
fn admin_consent(ctx: &egui::Context, request: AdminConsentRequest) -> AdminConsentAnswer {
    admin_consent_with(request, &|url| {
        let opened = open_in_browser(url);
        ctx.request_repaint();
        opened
    })
}

/// [`admin_consent`], with `open_page` in the browser's place.
fn admin_consent_with(request: AdminConsentRequest, open_page: &dyn Fn(&str) -> bool) -> AdminConsentAnswer {
    use std::sync::atomic::{AtomicBool, Ordering};

    use mailcraft_net::oauth::AdminRefusal;
    let no_browser = AtomicBool::new(false);
    let open = |url: &str| {
        if !open_page(url) {
            no_browser.store(true, Ordering::Relaxed);
            request.cancel.store(true, Ordering::Relaxed);
        }
    };
    match mailcraft_net::oauth::admin_consent(&request.oauth, &request.tenant, &open, &request.cancel) {
        Ok(()) => AdminConsentAnswer::Approved,
        Err(_) if no_browser.load(Ordering::Relaxed) => AdminConsentAnswer::NoBrowser,
        Err(AdminRefusal::NotAdministrator(words)) => AdminConsentAnswer::NotAdministrator(words),
        Err(AdminRefusal::Declined(words)) => AdminConsentAnswer::Declined(words),
        Err(AdminRefusal::TimedOut) => AdminConsentAnswer::TimedOut,
        Err(AdminRefusal::Cancelled) => AdminConsentAnswer::Cancelled,
        Err(AdminRefusal::Other(e)) => AdminConsentAnswer::Failed(e.to_string()),
    }
}

/// Why the owner's yes to reading Outlook's Favorites wasn't had, in the words the window knows
/// each kind of no by: an administrator has to approve it first, the organisation has shut the
/// door, this kind of account has no such thing. Anything else is said as it is.
fn favorites_refusal(refusal: mailcraft_net::oauth::FavoritesRefusal) -> String {
    use mailcraft_net::oauth::FavoritesRefusal;
    use mailcraft_ui_egui::favorites_sync::{NEEDS_APPROVAL, REFUSED_BY_ORGANISATION, REFUSED_FOR_ACCOUNT};
    match refusal {
        FavoritesRefusal::Approval => NEEDS_APPROVAL.to_string(),
        FavoritesRefusal::Organisation => REFUSED_BY_ORGANISATION.to_string(),
        FavoritesRefusal::Account => REFUSED_FOR_ACCOUNT.to_string(),
        FavoritesRefusal::Other(e) => e.to_string(),
    }
}

/// Keep an account's rules on its server (#566): when something waits to go, a token that may
/// change the rules (a sign-in that doesn't cover it gets none: the answer says the owner is to
/// be asked, and nothing is sent), what waits told to the server; then the server's rules read
/// whole, with the account's ordinary token when nothing went. Nothing of a rule is logged.
fn rules_job(job: mailcraft_ui_egui::rule_sync::RulesJob) -> mailcraft_ui_egui::rule_sync::RulesResult {
    rules_job_with(job, &mailcraft_net::oauth::refresh_graph_rules, &mailcraft_net::oauth::refresh_graph_mail)
}

/// How a rules job gets a token: for changing the rules, or for reading them.
type RenewGraph<'a> = &'a dyn Fn(&mailcraft_model::OAuth, &str) -> Result<mailcraft_net::oauth::GraphTokens, mailcraft_net::NetError>;

/// [`rules_job`], with how the tokens are had as `write` and `read` (the tests' own, which ask
/// nobody).
fn rules_job_with(
    job: mailcraft_ui_egui::rule_sync::RulesJob,
    write: RenewGraph<'_>,
    read: RenewGraph<'_>,
) -> mailcraft_ui_egui::rule_sync::RulesResult {
    use mailcraft_model::remote_rules::RuleResult;
    use mailcraft_net::{NetError, graph};
    let mut out = mailcraft_ui_egui::rule_sync::RulesResult { account: job.account.id, ..Default::default() };
    let failed = |out: &mut mailcraft_ui_egui::rule_sync::RulesResult, e: &NetError| {
        out.sign_in_refused = e.is_refused_sign_in();
        if !out.sign_in_refused {
            out.errors.push(format!("Couldn't sign in to Microsoft for the rules: {e}"));
        }
    };
    let mut refresh = job.refresh.clone();
    let mut access = String::new();
    if !job.upload.ops.is_empty() {
        match write(&job.oauth, &refresh) {
            Ok(got) => {
                access = got.tokens.access.clone();
                if !got.tokens.refresh.is_empty() {
                    refresh = got.tokens.refresh.clone();
                }
                out.tokens = Some(got.tokens);
            }
            // Nothing is sent: the owner is asked. The server's rules are still read.
            Err(e) if e.is_consent_needed() => out.consent_needed = true,
            Err(e) => {
                failed(&mut out, &e);
                return out;
            }
        }
    }
    if !access.is_empty() {
        let uploaded = graph::push_rules(&access, &job.upload).for_account(&job.account);
        out.denied = uploaded.outcomes.iter().any(|o| o.result == RuleResult::Denied);
        let refused = uploaded.token_refused;
        out.uploaded = Some(uploaded);
        if refused {
            out.sign_in_refused = true;
            return out;
        }
    }
    if access.is_empty() {
        match read(&job.oauth, &refresh) {
            Ok(got) => {
                access = got.tokens.access.clone();
                out.tokens = Some(got.tokens);
            }
            Err(e) if e.is_consent_needed() => {
                // (The account's mail isn't covered either: its own run says so.)
                out.unreadable = true;
                return out;
            }
            Err(e) => {
                failed(&mut out, &e);
                return out;
            }
        }
    }
    match graph::rules(&access, &job.folders) {
        Ok(download) => out.download = Some(download.for_account(&job.account)),
        // The sign-in doesn't cover reading the mailbox's settings.
        Err(NetError::Forbidden(_)) => out.unreadable = true,
        Err(e) if e.is_refused_sign_in() => out.sign_in_refused = true,
        Err(e) => out.errors.push(format!("Couldn't read the rules on the server: {e}")),
    }
    out
}

/// File › Info › Automatic Replies: a read takes the calendar's tokens, which read the
/// mailbox's settings (no more token requests than any Graph job makes); a change takes a token
/// for changing them, which a sign-in doesn't carry until the owner allowed it by itself (then
/// the answer says he is to be asked, and nothing is sent). The setting is read back after a
/// change. Nothing of a reply is logged.
fn auto_replies_job(job: mailcraft_ui_egui::auto_replies::AutoRepliesJob) -> mailcraft_ui_egui::auto_replies::AutoRepliesResult {
    use mailcraft_net::{NetError, graph, oauth};
    let mut out = mailcraft_ui_egui::auto_replies::AutoRepliesResult { account: job.account.id, ..Default::default() };
    let renewed = match &job.change {
        Some(_) => {
            oauth::refresh_graph_replies(&job.oauth, &job.refresh, mailcraft_host::tasks::mail_resource(&job.account)).map(|got| (got.tokens, true))
        }
        None => oauth::refresh_graph(&job.oauth, &job.refresh).map(|got| (got.tokens, !got.extras_missing)),
    };
    let access = match renewed {
        Ok((tokens, covered)) => {
            let access = tokens.access.clone();
            out.tokens = Some(tokens);
            if !covered {
                // The sign-in doesn't read the mailbox's settings: the owner is to sign in again.
                out.consent_needed = true;
                return out;
            }
            access
        }
        Err(e) if e.is_consent_needed() && job.change.is_some() => {
            out.write_consent_needed = true;
            return out;
        }
        Err(e) => {
            out.consent_needed = e.is_consent_needed();
            out.sign_in_refused = !out.consent_needed && e.is_refused_sign_in();
            if !out.consent_needed && !out.sign_in_refused {
                out.error = format!("Couldn't sign in to Microsoft for the automatic replies: {e}");
            }
            return out;
        }
    };
    if let Some(change) = &job.change {
        match graph::set_automatic_replies(&access, change) {
            Ok(()) => out.changed = true,
            // The token covers the permission and Graph still says no: the organisation keeps them.
            Err(NetError::Forbidden(_)) => out.write_denied = true,
            Err(e) if e.is_refused_sign_in() => {
                out.sign_in_refused = true;
                return out;
            }
            Err(e) => out.error = format!("Couldn't change the automatic replies: {e}"),
        }
    }
    match graph::automatic_replies(&access) {
        Ok(setting) => out.setting = Some(setting),
        Err(e) if e.is_refused_sign_in() => out.sign_in_refused = true,
        Err(e) if out.error.is_empty() => out.error = format!("Couldn't read the automatic replies: {e}"),
        Err(_) => {}
    }
    out
}

/// What the app needs from the system: native file dialogs, shown without blocking the window
/// (`file_dialogs`); everything that opens a socket (Send/Receive, signing in, finding a domain's
/// mail servers, checking an account, asking whether a new version is out), each on a background
/// thread; and the credential store.
/// `kept_pictures` is where downloaded pictures are kept between runs, when they are.
fn services(
    file_dialog: file_dialogs::Hook,
    ctx: egui::Context,
    kept_pictures: Option<std::sync::Arc<picture_cache::Cache>>,
    updates: Option<&HostUpdates>,
) -> Services {
    let pictures_ctx = ctx.clone();
    let pictures_kept = kept_pictures.clone();
    let (images_ctx, images_kept) = (ctx.clone(), kept_pictures.clone());
    let wipe_pictures = kept_pictures.clone();
    let busy_ctx = ctx.clone();
    let check_ctx = ctx.clone();
    let approval_ctx = ctx.clone();
    let fetch_ctx = ctx.clone();
    let (ews_ctx, ews_fetch_ctx, ews_setup_ctx) = (ctx.clone(), ctx.clone(), ctx.clone());
    let search_ctx = ctx.clone();
    let tasks_ctx = ctx.clone();
    let rules_ctx = ctx.clone();
    let replies_ctx = ctx.clone();
    let ahead_ctx = ctx.clone();
    let update_ctx = ctx.clone();
    let download_ctx = ctx.clone();
    let report_ctx = ctx.clone();
    let copy_ctx = ctx.clone();
    let tree_ctx = ctx.clone();
    let pst_ctx = ctx.clone();
    let alert_ctx = ctx.clone();
    let autodiscover_ctx = ctx.clone();
    let (keys_ctx, keys_store) = (ctx.clone(), secrets::store());
    let attachment_preview = attachment_render::service(&ctx);
    let (sync_ctx, sign_in_ctx, graph_ctx, mx_ctx, test_ctx) = (ctx.clone(), ctx.clone(), ctx.clone(), ctx.clone(), ctx);
    let sync_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel = sync_cancel.clone();
    let idle_ctx = tasks_ctx.clone();
    // The host: what the platform gives the program, held by the threads that run its jobs. The
    // desktop's platform asks the window, by message, for what only the window may do.
    let platform_calls = mailcraft_ui_egui::platform::PlatformCalls::default();
    let wake_ctx = tasks_ctx.clone();
    let platform = platform_calls.platform(secrets::store(), std::sync::Arc::new(move || wake_ctx.request_repaint()));
    let host = match mailcraft_host::init(std::sync::Arc::new(platform)) {
        Ok(host) => Some(std::sync::Arc::new(host)),
        // The jobs still run, each on a thread of its own, as before the host (the desktop's
        // platform gives no memory budget, so this is not expected); nothing stops silently.
        Err(e) => {
            log::error!("the host couldn't start, jobs run without it: {e}");
            None
        }
    };
    Services {
        platform_calls,
        file_dialog: Some(file_dialog),
        // Mail arrives in pieces as the run goes (what was sent, then folder by folder), through a
        // channel short enough that a busy window holds the run back instead of piling pieces up.
        sync: Some(Box::new(move |request| {
            use std::sync::atomic::Ordering;
            let (tx, rx) = std::sync::mpsc::sync_channel(8);
            let (wake, cancel) = (sync_ctx.clone(), cancel.clone());
            cancel.store(false, Ordering::Relaxed);
            let spawned = std::thread::Builder::new().name("send/receive".into()).spawn(move || {
                mailcraft_net::sync_with(&request, &cancel, &mut |piece| {
                    // Nobody is listening any more (the window closed): stop.
                    if tx.send(piece).is_err() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    wake.request_repaint();
                });
            });
            // A dropped sender reads as "stopped unexpectedly" in the app.
            if let Err(e) = spawned {
                log::error!("couldn't start Send/Receive: {e}");
            }
            rx
        })),
        sync_cancel,
        // Waiting for new mail in IDLE (#276): one thread an account, which ends when its flag is set.
        idle: Some(Box::new(move |request| {
            let (tx, notes) = std::sync::mpsc::sync_channel(16);
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (flag, wake) = (stop.clone(), idle_ctx.clone());
            let spawned = std::thread::Builder::new().name("idle".into()).spawn(move || {
                mailcraft_net::idle::watch(&request, &flag, &mut |note| {
                    use mailcraft_model::sync::IdleNote;
                    match note {
                        // What matters is that there is news, not how often it was said.
                        IdleNote::Arrived => {
                            let _ = tx.try_send(note);
                        }
                        IdleNote::Ended(_) => {
                            let _ = tx.send(note);
                        }
                    }
                    wake.request_repaint();
                });
            });
            // A dropped sender reads as "ended without a word" in the app.
            if let Err(e) = spawned {
                log::error!("couldn't start the wait for new mail: {e}");
            }
            mailcraft_ui_egui::IdleWatch { stop, notes }
        })),
        sign_in: Some(Box::new(move |request| {
            let ctx = sign_in_ctx.clone();
            background(&sign_in_ctx, "sign-in", move || sign_in(&ctx, request))
        })),
        graph: Some(Box::new({
            let host = host.clone();
            move |request: GraphRequest| host_job(host.as_ref(), &graph_ctx, "microsoft graph", move || mailcraft_host::graph::graph(request))
        })),
        // Messages of a Microsoft account fetched whole, a few at a time (R3.2).
        graph_fetch: Some(Box::new({
            let host = host.clone();
            move |job: mailcraft_ui_egui::graph_fetch::GraphFetchJob| {
                host_job(host.as_ref(), &fetch_ctx, "message download", move || mailcraft_host::fetch::run(job))
            }
        })),
        // An Exchange server account (#252): read through Exchange Web Services, and its messages fetched whole.
        ews: Some(Box::new(move |job| ews_runner::run_mail(job, &ews_ctx))),
        ews_fetch: Some(Box::new(move |job| ews_runner::run_fetch(job, &ews_fetch_ctx))),
        ews_setup: Some(Box::new(move |job| ews_setup::run_setup(job, &ews_setup_ctx))),
        // A Microsoft mailbox searched on its server, a page at a time, on the owner's click (R6.2).
        graph_search: Some(Box::new(move |job: mailcraft_ui_egui::server_search::GraphSearchJob| {
            background(&search_ctx, "server search", move || mailcraft_net::graph::search_messages(&job.access, &job.request))
        })),
        // The tasks of a Microsoft account, kept in step with Microsoft To Do (#248).
        graph_tasks: Some(Box::new({
            let host = host.clone();
            move |job: mailcraft_ui_egui::task_sync::TasksJob| {
                host_job(host.as_ref(), &tasks_ctx, "microsoft to do", move || mailcraft_host::tasks::run(job))
            }
        })),
        // The rules a Microsoft account's server keeps (#566).
        graph_rules: Some(Box::new(move |job: mailcraft_ui_egui::rule_sync::RulesJob| {
            background(&rules_ctx, "server rules", move || rules_job(job))
        })),
        // File › Info › Automatic Replies of a Microsoft account, read and changed on its server.
        graph_auto_replies: Some(Box::new(move |job: mailcraft_ui_egui::auto_replies::AutoRepliesJob| {
            background(&replies_ctx, "automatic replies", move || auto_replies_job(job))
        })),
        discover: Some(Box::new(move |domain| {
            background(&mx_ctx, "server discovery", move || mailcraft_net::discover(&domain).map_err(|e| e.to_string()))
        })),
        lookup_autodiscover: Some(Box::new(move |domain| {
            background(&autodiscover_ctx, "autodiscover records", move || mailcraft_net::autodiscover_hosts(&domain).map_err(|e| e.to_string()))
        })),
        test_account: Some(Box::new(move |account, secret| {
            // The window says why in its own language (#872).
            background(&test_ctx, "account check", move || mailcraft_net::test_account(&account, &secret).map_err(|e| e.said()))
        })),
        // The copy of the mailbox kept before an account switches to Microsoft Graph: a large
        // mailbox takes a while to write, and the window shows a spinner meanwhile (#118).
        write_copy: Some(Box::new(move |path, mailbox| {
            background(&copy_ctx, "mailbox copy", move || mailcraft_engine::store::save_single(&path, &mailbox).map(|_| ()))
        })),
        // A folder of mbox files, walked, read and parsed off the window's thread (#420); the
        // window shows its progress and cancels it.
        import_tree: Some(Box::new(move |job: mailcraft_ui_egui::import_tree::TreeJob| {
            background(&tree_ctx, "folder import", move || {
                mailcraft_engine::import_tree::load_path(&job.path, None, job.count_only, &job.progress).map_err(|e| e.to_string())
            })
        })),
        // A .pst file's mail, read off the window's thread a batch at a time (#815): a small
        // channel, so that the reader waits for the window rather than holding the whole file.
        import_pst: Some(Box::new(move |job: mailcraft_ui_egui::import_pst::PstJob| {
            let (tx, rx) = std::sync::mpsc::sync_channel(2);
            let wake = pst_ctx.clone();
            let spawned = std::thread::Builder::new().name("pst import".into()).spawn(move || {
                mailcraft_engine::import_pst::read_path(&job.path, &job.job, &job.progress, &mut |event| {
                    let sent = tx.send(event).is_ok();
                    wake.request_repaint();
                    sent
                });
            });
            if let Err(e) = spawned {
                log::error!("couldn't start the .pst import: {e}");
            }
            rx
        })),
        // Another mailbox file, read off the window's thread before it takes the place of the one
        // open (File › Open, #519): the window shows how far it has got and can call it off. A
        // panic on the way is a file that couldn't be read.
        read_ahead: Some(Box::new(move |path: PathBuf, progress: std::sync::Arc<mailcraft_engine::io::OpenProgress>| {
            background(&ahead_ctx, "opening a mailbox", move || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mailcraft_engine::store::open_ahead(&path, &progress)))
                    .unwrap_or_else(|_| Err("MailCraft stopped reading it before it was done.".to_string()))
            })
        })),
        // A picture a message keeps on the web, once the owner asked for that message's pictures.
        // The address is the message's choice, so `fetch` checks where it leads before it goes
        // there (never to this machine or a private network), and takes and waits only so much.
        // One that was downloaded before and is still kept comes from the disk instead: nobody
        // is asked, and so nobody is told again. Either way the window gets bytes to check.
        pictures: Some(Box::new(move |url| {
            let kept = pictures_kept.clone();
            background(&pictures_ctx, "picture", move || picture_cache::picture(&url, kept.as_ref()))
        })),
        // A sender's picture for the message list, once the owner turned that on: from
        // Gravatar, favicon.im or Google, downloaded and kept like a message's pictures.
        sender_images: Some(Box::new(move |address, wanted| {
            let kept = images_kept.clone();
            background(&images_ctx, "sender image", move || sender_images::look_up(&address, kept.as_ref(), &wanted, mailcraft_net::pictures::fetch))
        })),
        picture_cache: kept_pictures.map(|kept| Box::new(picture_cache::Kept(kept)) as Box<dyn mailcraft_ui_egui::PictureCache>),
        // When the people of a meeting are free and busy, while its Find a Time page is open.
        free_busy: Some(Box::new({
            let host = host.clone();
            move |request: FreeBusyRequest| {
                host_job(host.as_ref(), &busy_ctx, "free and busy times", move || mailcraft_host::calendar::free_busy(request))
            }
        })),
        // A check of the calendar connection on the account's real server, when the owner asks.
        check_calendar: Some(Box::new(move |request: CalendarCheckRequest| {
            host_job(host.as_ref(), &check_ctx, "calendar check", move || mailcraft_host::calendar::check_calendar(request))
        })),
        // The address an administrator approves this application at: made here, for the owner to
        // pass on in a message.
        admin_consent_link: Some(Box::new(|oauth, tenant| mailcraft_net::oauth::admin_consent_url(oauth, tenant).ok())),
        // And the same page opened from here, for an owner who is an administrator and said so.
        admin_consent: Some(Box::new(move |request| {
            let ctx = approval_ctx.clone();
            background(&approval_ctx, "admin consent", move || admin_consent(&ctx, request))
        })),
        // An attachment the owner opened (double-click, Open): written into MailCraft's own
        // folder and opened with the system's program for its kind (#126).
        open_file: opened_files()
            .map(|opened| Box::new(move |name: &str, bytes: &[u8]| opened.open(name, bytes)) as Box<dyn Fn(&str, &[u8]) -> Result<(), String>>),
        attachment_preview,
        secrets: secrets::store(),
        // File › Options › Security (#30 R13): the owner's own certificates and keys listed,
        // GnuPG's valid keys read, and a key file imported, each on a thread of its own.
        own_keys: Some(Box::new(move |job| {
            let store = keys_store.clone();
            background(&keys_ctx, "own keys", move || own_keys::run(job, store))
        })),
        // File › Info › Remove MailCraft's data from this computer (#177): the folders this
        // program works in, and no others. Not offered by a run that keeps nothing.
        data_wipe: prefs_enabled().then(|| {
            Box::new(wipe::Wipe { roots: wipe::Roots::of_this_program(), pictures: wipe_pictures })
                as Box<dyn mailcraft_ui_egui::data_removal::DataWipe>
        }),
        // File › Info › Open MailCraft's data folder (#460): the settings folder, and the opener
        // for whichever folder the window shows (that one, or the open mailbox's own when it is
        // kept elsewhere). Not for a run that keeps nothing.
        data_folder: data_folder(prefs_enabled()),
        // Desktop alerts, through the system's notifications (`alerts`).
        alert: alerts::host(alert_ctx),
        open_page: Some(Box::new(open_in_browser)),
        open_page_waiting: Some(std::sync::Arc::new(open_in_browser)),
        clipboard: clipboard::board(None),
        // Help › Copy Diagnostics: the version, the platform and the redacted end of the logs.
        diagnostics: Some(Box::new(|| diagnostics::report(env!("CARGO_PKG_VERSION"), build_sha(), log_dir().filter(|_| prefs_enabled()).as_deref()))),
        // Help › Report a Problem › Send report (#956), on the owner's click alone: the report the
        // window showed goes to the project's relay, once. A build that names no relay has none.
        report_send: mailcraft_net::report::relay().map(|_| {
            Box::new(move |report: mailcraft_model::report::ProblemReport| {
                background(&report_ctx, "problem report", move || mailcraft_net::report::send(&report))
            }) as Box<dyn Fn(_) -> _>
        }),
        microsoft_client_id: microsoft_client_id(),
        google_client_id: google_client_id(),
        google_client_secret: google_client_secret(),
        // ponytail: false once Google has verified the app (#232).
        google_testers_only: true,
        // Whether a new version is out: GitHub is asked about this build's repository and told
        // nothing else, once a day when the owner wants that, or when they press Check for Updates.
        update_check: Some(Box::new(move |pre_releases| {
            background(&update_ctx, "update check", move || mailcraft_net::update::latest(pre_releases).map_err(|e| e.to_string()))
        })),
        // A build that isn't one of the project's packages (they name their commit) never asks.
        development_build: build_sha() == "dev",
        // A copy that updates itself downloads a newer release, checked by its signature.
        update_download: updates.map(|updates| update_download(&download_ctx, updates.clone())),
        updates_turned_off_by_organisation: updater::turned_off_by_organisation(),
        // The system's language, for an interface language left on Automatic.
        system_locale: system_locale::system_locale(),
        // And its regional format, whose region English dates are written for (#978).
        system_time_locale: system_locale::system_time_locale(),
        // Whether the system plays animations, for Options › Animations (#1183).
        animations_enabled: system_motion::reader(),
        ..Default::default()
    }
}

/// The download of a newer release, for a copy that updates itself (the `.msi`, the macOS app,
/// an AppImage) and is one of the project's packages, into the data folder's `updates/`, on a
/// background thread; checked by the release's own signature there (`mailcraft_net`). What was
/// checked, and the SHA-256 its signed list gives, is noted for installing it at exit.
fn update_download(
    ctx: &egui::Context,
    updates: HostUpdates,
) -> Box<dyn Fn(mailcraft_model::update::Release) -> std::sync::mpsc::Receiver<Result<PathBuf, String>>> {
    let ctx = ctx.clone();
    Box::new(move |release: mailcraft_model::update::Release| {
        let updates = updates.clone();
        background(&ctx, "update download", move || {
            if wipe::begun() {
                return Err("MailCraft's data was removed from this computer".to_string());
            }
            let dir = updates.folders.updates();
            // The SHA-256 noted is the signed list's, not the file's read back: what is
            // installed is checked against what the release signed.
            let verified = updater::download(&release, updates.install, &dir, mailcraft_net::update_download::download)?;
            let got = verified.path.clone();
            *updates.verified.lock().unwrap_or_else(|e| e.into_inner()) = Some(verified);
            Ok(got)
        })
    })
}

/// The application id of every window (by which Wayland desktops find the icon).
const APP_ID: &str = "io.github.billsecond.mailcraft";

/// Window, Dock and taskbar icon.
fn app_icon() -> Option<egui::IconData> {
    #[cfg(target_os = "macos")]
    let png: &[u8] = include_bytes!("../../../assets/app-icon/mailcraft-macos-512.png");
    #[cfg(not(target_os = "macos"))]
    let png: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/io.github.billsecond.mailcraft.png");
    eframe::icon_data::from_png_bytes(png).map_err(|e| log::warn!("app icon: {e}")).ok()
}

/// The commit the build came from (short), or `dev`.
fn build_sha() -> &'static str {
    option_env!("MAILCRAFT_BUILD_SHA").map(|s| s.get(..8).unwrap_or(s)).unwrap_or("dev")
}

/// The thread that writes the mailbox file, so the window never waits for the disk; it wakes the
/// window when a state has been written, or couldn't be. It also clears out content no message
/// uses any more, a first time once the mailbox is open.
fn background_saver(ctx: &egui::Context, path: &std::path::Path) -> Option<Box<dyn mailcraft_engine::io::BackgroundSave>> {
    use mailcraft_engine::io::BackgroundSave as _;
    let wake = ctx.clone();
    match mailcraft_engine::store::Saver::spawn(move || wake.request_repaint()) {
        Ok(saver) => {
            saver.follow(path);
            if path.exists() {
                saver.sweep_later(path);
            }
            Some(Box::new(saver))
        }
        // Without it the mailbox is saved in place.
        Err(e) => {
            log::error!("couldn't start the mailbox saver: {e}");
            None
        }
    }
}

/// Once the window shows (#519): look for the content files of the mailbox file at `path` that
/// its opening left on disk and read the bodies it left for later (`store::content_later`), hand
/// over which are missing or damaged, then the bodies in batches, on a
/// thread of its own, waking the window each time. `None` when the thread couldn't be started:
/// then nothing more is said than the opening found, and the bodies load as they are shown.
#[allow(clippy::type_complexity)]
fn content_later(
    ctx: &egui::Context,
    path: &std::path::Path,
) -> Option<(std::sync::mpsc::Receiver<mailcraft_engine::Missing>, std::sync::mpsc::Receiver<Vec<mailcraft_engine::BodyRead>>)> {
    /// Bodies read and handed over at a time.
    const BATCH: usize = 256;
    let (missing_tx, missing_rx) = std::sync::mpsc::channel();
    let (bodies_tx, bodies_rx) = std::sync::mpsc::channel();
    let (wake, path) = (ctx.clone(), path.to_path_buf());
    let spawned = std::thread::Builder::new().name("looking for the mailbox's content".into()).spawn(move || {
        // (What is missing is known once the bodies are read: a damaged file shows only then.)
        let (missing, bodies) = mailcraft_engine::store::content_later(&path);
        let _ = missing_tx.send(missing);
        wake.request_repaint();
        let mut bodies = bodies.into_iter();
        loop {
            let batch: Vec<_> = bodies.by_ref().take(BATCH).collect();
            // Done, or the window has let go (another mailbox, or it closed).
            if batch.is_empty() || bodies_tx.send(batch).is_err() {
                return;
            }
            wake.request_repaint();
        }
    });
    match spawned {
        Ok(_) => Some((missing_rx, bodies_rx)),
        Err(e) => {
            log::warn!("couldn't start looking for the mailbox's content files: {e}");
            None
        }
    }
}

/// Said when the mailbox is open in a MailCraft that doesn't answer: opening it a second time
/// would let two windows save over each other, so this start ends here.
fn already_open() {
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("MailCraft")
        .set_description("MailCraft already has this mailbox open, but it isn't answering. Close that window (or end MailCraft in the system's task list) and start MailCraft again.")
        .show();
}

fn main() -> eframe::Result {
    // First, so the panic hook and every start-up warning are recorded (`logging`).
    let logger = logging::install();
    logging::install_panic_hook();
    // A copy of the program in the data folder that installs an `.msi` once MailCraft has gone
    // (Windows), and does nothing else (`self_update`).
    let all: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if all.first().is_some_and(|a| a == self_update::FINISH_UPDATE) {
        // (This copy runs from the data folder, not from where the installer put MailCraft.)
        let folders = mailcraft_control_key::settings_dir().filter(|_| prefs_enabled()).map(self_update::Folders::new);
        let code = match (self_update::Job::parse(&all), folders) {
            (Some(job), Some(folders)) => self_update::finish(&job, &folders),
            _ => 2,
        };
        std::process::exit(code);
    }
    let mut control_port: Option<u16> = std::env::var("MAILCRAFT_CONTROL_PORT").ok().and_then(|p| p.parse().ok());
    let mut control_agents_only = std::env::var("MAILCRAFT_CONTROL_AGENTS_ONLY").is_ok_and(|v| v == "1");
    let mut files = Vec::new();
    let mut sample = false;
    let mut mailbox_path: Option<PathBuf> = None;
    let mut after_update = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--control" => control_port = args.next().and_then(|p| p.parse().ok()),
            "--control-agents-only" => control_agents_only = true,
            "--sample" => sample = true,
            "--mailbox" => mailbox_path = args.next().map(PathBuf::from),
            // Started by the version this one replaced: it is let go first.
            self_update::AFTER_UPDATE => after_update = true,
            "--version" => {
                println!("mailcraft {} ({})", env!("CARGO_PKG_VERSION"), build_sha());
                return Ok(());
            }
            _ => files.push(a),
        }
    }
    // A copy that updates itself: a new version waits for the one it replaced to have gone, and
    // finds out what came of the last update (`self_update`); a start that lasts a minute worked.
    let updates = HostUpdates::of_this_copy();
    let update_start = match &updates {
        Some(updates) => {
            if after_update {
                self_update::wait_for_the_old_one(&updates.folders);
            }
            let start = self_update::at_start(&updates.folders, env!("CARGO_PKG_VERSION"));
            let folders = updates.folders.clone();
            let _ = std::thread::Builder::new().name("update settle".into()).spawn(move || {
                std::thread::sleep(self_update::SETTLED_AFTER);
                // Not into a folder MailCraft's data was removed from (#177).
                if !wipe::begun() {
                    self_update::settle(&folders, env!("CARGO_PKG_VERSION"));
                }
            });
            start
        }
        None => Default::default(),
    };
    // What an update that stopped partway left beside a copy this program replaces, also where it
    // no longer updates itself (a Mac app, #497), and a Mac copy's leftovers in the data folder.
    // Not while another start is installing (the update lock).
    if build_sha() != "dev"
        && prefs_enabled()
        && let found @ (install::Install::MacApp | install::Install::AppImage) = install::found()
        && let Some(dir) = mailcraft_control_key::settings_dir()
    {
        let folders = self_update::Folders::new(dir);
        if let Some(target) = self_update::target(found) {
            self_update::clear_leftovers(&folders, &target);
        }
        if found == install::Install::MacApp && updates.is_none() {
            self_update::clear_mac_data(&folders);
        }
    }
    // The icons of MailCraft in the owner's own data folder, brought up to this version's: an
    // update replaces the program, not the icons a copy put in place by hand came with (#732).
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
    if build_sha() != "dev" && prefs_enabled() {
        desktop_icons::refresh();
    }
    // The settings folder holds the mailbox, the control key and the logs: for the owner alone.
    // One an earlier version left open to others is closed; best effort, never a reason not to
    // start (a folder of somebody else's, named by `--mailbox`, is not touched).
    if let Some(dir) = mailcraft_control_key::settings_dir().filter(|_| prefs_enabled())
        && let Err(e) = mailcraft_engine::store::make_private(&dir)
    {
        log::debug!("the settings folder was not made private: {e}");
    }
    // One window per mailbox file: started again, MailCraft hands what it was started with to
    // the window that has the mailbox and exits. The sample keeps no file, so nothing is locked at the
    // start; a mailbox file it opens later is (#517).
    let kept = if sample { None } else { mailbox_path.clone().or_else(|| default_mailbox_path().filter(|_| prefs_enabled())) };
    let guard = match kept {
        Some(path) => match single_instance::start(&path, &files) {
            single_instance::Start::First(guard) => Some(guard),
            single_instance::Start::Handed => return Ok(()),
            single_instance::Start::Unreachable(why) => {
                log::error!("MailCraft already has {} open and can't be reached: {why}", path.display());
                already_open();
                return Ok(());
            }
        },
        // Nothing held yet: a mailbox file opened later in the window is locked all the same.
        None => Some(single_instance::none()),
    };
    // The log file lives in the settings directory, next to the preferences; opened after the
    // arguments, so `--version` leaves no file behind. Runs without preferences (agents' test
    // runs) log to standard error only, so they don't rotate away the user's own logs.
    if let Some(logger) = logger {
        match log_dir().filter(|_| prefs_enabled()) {
            Some(dir) => match logger.attach_dir(&dir) {
                Ok(path) => log::info!("MailCraft {} ({}), log file {}", env!("CARGO_PKG_VERSION"), build_sha(), path.display()),
                Err(e) => log::warn!("no log file: {e}"),
            },
            None => logger.no_file(),
        }
    }
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MailCraft")
            .with_app_id(APP_ID)
            .with_inner_size([1440.0, 920.0])
            .with_min_inner_size([760.0, 480.0])
            .with_drag_and_drop(true)
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false),
        ..Default::default()
    };
    if let Some(icon) = app_icon() {
        options.viewport = options.viewport.with_icon(icon);
    }
    // Under Wayland the window layer's clipboard can't see a picture or HTML (in the Flatpak it
    // has no clipboard of its own at all): the clipboard keys are handled by `mailcraft_ui_egui::paste_keys` (#1217).
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let clipboard_keys = mailcraft_ui_egui::paste_keys::wayland_session(
        std::env::var("WAYLAND_SOCKET").ok().as_deref(),
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
    );
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    if clipboard_keys {
        options.clipboard_shortcuts = false;
    }
    // Before eframe creates the wgpu instance: default Windows to DirectX 12 only, or OpenGL when
    // DirectX 12 has no GPU driver (see graphics.rs).
    #[cfg(target_os = "windows")]
    graphics::configure(&mut options, eframe::wgpu::Backends::from_env());
    let (prefs, settings_unread) = load_prefs();
    let close_to_tray = prefs.as_ref().is_some_and(|ui| ui.close_to_tray);
    let restored = prefs.as_ref().and_then(|ui| ui.window).and_then(WindowGeometry::sanitized);
    if let Some(window) = restored {
        options.viewport = window.apply(options.viewport);
    }
    // Registered before the event loop starts, so it catches the Finder event that launched us as
    // well as later ones. Lives until the event loop returns; the app creator only borrows it.
    #[cfg(target_os = "macos")]
    let apple_events = apple_events::AppleEvents::install();
    #[cfg(target_os = "macos")]
    let apple_events = &apple_events;
    // For the window, to take the lock of another mailbox file before it opens it (#517).
    let claimer = guard.as_ref().map(single_instance::Guard::claimer);
    run_window(
        options,
        Box::new(move |cc| {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            if let Some(browser) = BROWSER.get() {
                browser.set_window(&cc.egui_ctx);
            }
            // Before the first frame, so a dark desktop's window doesn't come up light.
            #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
            let _ = DESKTOP_THEME.set(desktop_theme::follow(&cc.egui_ctx));
            // The mailbox is read on a thread of its own, so that the window is there at once
            // (#519): it shows the opening screen meanwhile, and is made as it always was once
            // the mailbox is there (`build`).
            // Drawing by the processor (a software rasteriser) is said once and slows the window's
            // own repaints down (#software-render); the adapter is known now.
            let software = cc.wgpu_render_state.as_ref().is_some_and(|s| s.adapter.get_info().device_type == eframe::wgpu::DeviceType::Cpu);
            if software {
                log::warn!("graphics: drawing with a software rasteriser, expect high processor use");
            }
            mailcraft_ui_egui::software::set_software(software);
            let progress = std::sync::Arc::new(mailcraft_engine::io::OpenProgress::default());
            let reading = progress.clone();
            // The certification authorities this computer trusts, for checking Digital
            // signatures (#30): read from the system's store while the mailbox is read.
            let anchors = trust_anchors();
            let wake = cc.egui_ctx.clone();
            let arriving = launch::start(move || launch::first_mailbox(sample, mailbox_path, &reading), move || wake.request_repaint());
            let (file_dialog, dialogs) = file_dialogs::hook(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            let wayland_display = clipboard::wayland_display(cc);
            let (first_prefs, host_updates) = (prefs.clone(), updates.clone());
            let build: mailcraft_ui_egui::opening::Build = Box::new(move |arrival| {
                let (mailbox, path, unavailable, unchecked, damaged) =
                    (arrival.mailbox, arrival.path, arrival.unavailable, arrival.unchecked, arrival.damaged);
                let mut session = Session::new(mailbox);
                let kept = path.is_some();
                session.path = path;
                // Search answers from a word index kept beside the mailbox file, built in the
                // background (R6.2).
                session.index_words = true;
                // A run that keeps its mailbox and its settings keeps downloaded pictures too.
                let kept_pictures = if kept && !sample && prefs_enabled() { keep_pictures() } else { None };
                let mut services = services(file_dialog, ctx.clone(), kept_pictures, host_updates.as_ref());
                services.window_identity =
                    Some(mailcraft_ui_egui::windows::WindowIdentity { icon: app_icon().map(std::sync::Arc::new), app_id: Some(APP_ID.into()) });
                // Under Wayland the clipboard goes through the window's own connection (#1217).
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                {
                    services.clipboard = clipboard::board(wayland_display);
                    services.clipboard_keys = clipboard_keys;
                }
                services.update_start = update_start;
                services.claim_mailbox =
                    claimer.map(|c| Box::new(move |path: &std::path::Path| c.claim(path)) as Box<dyn Fn(&std::path::Path) -> bool>);
                let mut app = MailApp::new(session, services);
                app.trust_anchors_arriving = Some(anchors);
                // Nowhere to keep the mailbox (the sample, a run without preferences): don't try.
                app.autosave = kept;
                if let Some(path) = &app.session.path {
                    app.services.saver = background_saver(&ctx, path);
                }
                // The word index starts once the window has drawn its first frames (#519).
                app.index_words_later();
                // The content files the opening left on disk are looked for now that the window
                // shows, and the bodies it left for later are read (#519); what is missing is
                // said when that is done (the damaged-mailbox notice is said instead).
                let later = match &app.session.path {
                    Some(path) if unchecked => content_later(&ctx, path),
                    _ => None,
                };
                let later = later.map(|(missing, bodies)| {
                    app.check_parts_later(missing, unavailable, !damaged);
                    app.take_bodies_later(bodies);
                });
                if damaged {
                    app.status(mailcraft_ui_egui::damaged_mailbox_notice());
                } else if later.is_some() {
                    // (Said once the look is over.)
                } else if unavailable > 0 {
                    app.status(format!("{unavailable} bodies or attachments are missing from the mailbox's content folder, or damaged."));
                }
                // A mailbox whose Outlook Favorites were read another way than Microsoft Graph,
                // which is no more: said once, and the file is saved without it. Mail of
                // Microsoft accounts kept with characters that couldn't be read (fetched before
                // their character sets were) is fetched again, once (#185); IMAP accounts' mail
                // the same way, by UID, at the next Send/Receive (#650); and mail an earlier
                // version put back empty after a refused delete for good, before anything can
                // move it (#793). The same steps as `file.open`'s.
                let after = mailcraft_engine::cmd::file::after_opening(&mut app.session);
                if after.favorites_now_graph > 0 {
                    app.status(mailcraft_ui_egui::favorites_sync::FAVORITES_NOW_GRAPH);
                }
                for notice in mailcraft_ui_egui::fetched_again_notices(after.unreadable, after.hollow) {
                    app.status(notice);
                }
                // An ordinary start on a mailbox nobody has set up yet begins with adding an account.
                app.offer_setup = kept && !sample;
                // A Microsoft account still on IMAP is offered the switch to Graph (R3.7): only
                // for a mailbox kept in a file, which is where the copy from before the switch goes.
                app.offer_graph_switch = kept && !sample;
                match prefs {
                    Some(ui) => app.apply_prefs(ui),
                    None => app.fresh_prefs(),
                }
                app.settings_unread = settings_unread;
                app.software_drawing = software;
                app.ui.window = restored;
                app.integrated_titlebar = cfg!(target_os = "macos");
                for f in files {
                    app.open_from_system(&f);
                }
                app
            });
            let mut app =
                MailApp::opening(mailcraft_ui_egui::opening::Opening::new(arriving, progress, build), first_prefs, system_locale::system_locale());
            app.ui.window = restored;
            app.integrated_titlebar = cfg!(target_os = "macos");
            let mut key_file = None;
            if let Some(port) = control_port
                && let Some((rx, file)) =
                    control_server::start(port, cc.egui_ctx.clone(), mailcraft_control_key::settings_dir().as_deref(), control_agents_only)
            {
                app = app.with_control(rx);
                key_file = file;
            }
            if let Some(guard) = &guard {
                guard.wake(&cc.egui_ctx);
            }
            // macOS: the menu bar (File, Edit, View, Folder, Message, Window, Help; every item a
            // command), whose app menu's Quit (Cmd+Q) asks the window to close, so a message being
            // written is asked about first (winit's own quits without asking). Here, after winit
            // has built its menu. If it can't be made, the smaller app menu does the asking.
            #[cfg(target_os = "macos")]
            let menu_bar = match menubar::install(&cc.egui_ctx, &app) {
                Ok(bar) => Some(bar),
                Err(e) => {
                    log::warn!("the menu bar couldn't be set up: {e}");
                    if let Err(e) = quit_macos::install_app_menu(&cc.egui_ctx, "MailCraft") {
                        log::warn!("the app menu couldn't be set up, so Quit won't ask about unsaved work: {e}");
                    }
                    None
                }
            };
            Ok(Box::new(App(
                app,
                restored,
                key_file,
                dialogs,
                guard,
                updates,
                tray::Tray::new(&cc.egui_ctx, tray::can_hide(cc)),
                #[cfg(target_os = "macos")]
                apple_events.connect(&cc.egui_ctx),
                #[cfg(target_os = "macos")]
                menu_bar,
            )))
        }),
        close_to_tray,
    )
}

/// The system's certification authorities for mail, read on a thread of their own (#30): the
/// answer comes on the channel, and the window takes it when it comes, never waiting. A read
/// that finds none, or stops on a panic, is tried once more a little later; a panic is a store
/// that couldn't be read, said as that.
fn trust_anchors() -> std::sync::mpsc::Receiver<Vec<Vec<u8>>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let read = move || {
        let once = || std::panic::catch_unwind(mailcraft_net::system_trust_anchors).unwrap_or_default();
        let mut found = once();
        if found.is_empty() {
            std::thread::sleep(std::time::Duration::from_secs(5));
            found = once();
        }
        if found.is_empty() {
            log::warn!("no certification authorities for mail could be read: Digital signatures are checked without them");
        }
        let _ = tx.send(found);
    };
    if let Err(e) = std::thread::Builder::new().name("trust anchors".into()).spawn(read) {
        log::warn!("the system's certification authorities couldn't be read for checking Digital signatures: {e}");
    }
    rx
}
