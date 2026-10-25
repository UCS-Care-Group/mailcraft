// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft desktop app.
//!
//! Usage: `mailcraft [--control <port> [--control-agents-only]] [--sample] [--mailbox <file>] [files or mailto: links…]`
//!
//! The mailbox lives in `<settings>/mailbox.mailcraft` (or the file `--mailbox` names) and is saved
//! as you go. `--sample` opens the sample mailbox instead and keeps nothing. Files on the command
//! line (`.eml`, `.mbox`, `.ics`, `.vcf`) are imported, and a `mailto:` link starts a new message.
//! A mailbox is open in one window only: started again, MailCraft hands its files and links to
//! the window that is already open and exits (`single_instance`).
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
mod block_on;
mod browser;
#[cfg(test)]
mod calendar_flow;
mod clipboard;
mod control_server;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
mod desktop_theme;
mod diagnostics;
#[cfg(test)]
mod favorites_flow;
#[cfg(test)]
mod favorites_organisation_flow;
mod file_dialogs;
mod file_launcher;
#[cfg(test)]
mod graph_folders_flow;
#[cfg(test)]
mod graph_mail_flow;
#[cfg(any(target_os = "windows", test))]
mod graphics;
mod install;
#[cfg(test)]
mod listed_twice_flow;
mod logging;
#[cfg(target_os = "macos")]
mod menubar;
mod opened_files;
#[cfg(test)]
mod opened_flow;
mod picture_cache;
mod quit;
#[cfg(target_os = "macos")]
mod quit_macos;
mod secrets;
mod self_update;
mod sender_images;
#[cfg(test)]
mod shared_calendar_flow;
#[cfg(test)]
mod shared_send_flow;
mod single_instance;
mod system_locale;
#[cfg(test)]
mod token_renew_flow;
#[cfg(test)]
mod unconfirmed_flow;
#[cfg(test)]
mod update_flow;
mod updater;
mod wipe;

use std::path::PathBuf;

use mailcraft_engine::Session;
use mailcraft_model::Mailbox;
use mailcraft_model::sync::Tokens;
use mailcraft_ui_egui::{
    AdminConsentAnswer, AdminConsentRequest, CalendarCheckRequest, CalendarCheckResult, CalendarCheckStep, FreeBusyRequest, FreeBusyResult,
    GraphRequest, GraphResult, MailApp, Services, SignInRequest, UiState,
    window_geometry::{WindowGeometry, take_rescue},
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
        apple_events::poll(&self.6, &mut self.0, ctx);
        // MailCraft was started again (the shortcut, a `mailto:` link, a file opened from the
        // file manager): what it was started with opens here, and the window comes forward.
        if let Some(asked) = self.4.as_ref().and_then(single_instance::Guard::handed_over) {
            for what in &asked {
                self.0.open_path(what);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        // The menu bar's choices run before the frame's own work (macOS).
        #[cfg(target_os = "macos")]
        if let Some(bar) = &mut self.7 {
            bar.pump(&mut self.0, ctx);
        }
        self.0.logic(ctx);
        self.3.show(frame);
        let prev = self.0.ui.window;
        self.0.ui.window = ctx.input(|i| WindowGeometry::track(prev, i.viewport(), i.viewport_rect().size()));
        if let Some(pos) = ctx.input(|i| take_rescue(&mut self.1, i.viewport())) {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
        // Pages to open, with the browser brought in front (`browser`).
        open_pages(ctx, frame);
        // Leaving (the close button, Quit, the Dock): a message being written asks first, then
        // the platform's own way out (`quit`). Last in `logic`, which runs while minimized too.
        quit::on_frame(ctx, &mut self.0);
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        // On Linux and the BSDs, the desktop's light or dark preference (`desktop_theme`), for
        // "Use system setting": winit has none there.
        #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
        if let Some(desktop) = DESKTOP_THEME.get() {
            desktop.apply(raw);
        }
        self.0.raw_input_hook(raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.0.ui(ui);
        self.3.show(frame);
        open_pages(ui.ctx(), frame);
    }
    fn on_exit(&mut self) {
        save_prefs(&self.0);
        // The last changes, if the timer hadn't got to them yet, and whatever the background
        // saver is still writing.
        if let Err(e) = self.0.save_on_exit() {
            log::error!("saving the mailbox on exit failed: {e}");
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

fn load_prefs(app: &mut MailApp) {
    if !prefs_enabled() {
        return;
    }
    if let Some(ui) = prefs_path().and_then(|p| read_prefs(&p)) {
        app.apply_prefs(ui);
    }
}

/// The preferences in `path`. A file that can't be read as them (damaged, or written by a version
/// that keeps them otherwise) is moved aside to `ui.json.damaged`, so the next save doesn't
/// overwrite what the owner had set and it can still be looked at.
fn read_prefs(path: &std::path::Path) -> Option<UiState> {
    let bytes = std::fs::read(path).ok()?;
    let ui = serde_json::from_slice::<UiState>(&bytes);
    if ui.is_err() {
        let mut aside = path.as_os_str().to_os_string();
        aside.push(".damaged");
        let _ = std::fs::rename(path, aside);
    }
    ui.ok()
}

/// Write the preferences so that a crash leaves the old file or the whole new one, readable by
/// its owner only.
fn write_prefs(path: &std::path::Path, ui: &UiState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        mailcraft_engine::store::create_private_dir_all(dir)?;
    }
    mailcraft_engine::store::write_atomic(path, &serde_json::to_vec_pretty(ui).map_err(std::io::Error::other)?)
}

/// The saved window geometry, read before the window opens (`load_prefs` runs after).
fn saved_window() -> Option<WindowGeometry> {
    if !prefs_enabled() {
        return None;
    }
    read_prefs(&prefs_path()?)?.window?.sanitized()
}

fn save_prefs(app: &MailApp) {
    // Its data was removed from this computer (File › Info): nothing is written back.
    if !prefs_enabled() || app.data_removed {
        return;
    }
    if let Some(p) = prefs_path() {
        let _ = write_prefs(&p, &app.prefs());
    }
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

/// The application (client) id this build presents to Microsoft's sign-in page: the
/// `MAILCRAFT_MS_CLIENT_ID` environment variable when the app starts, else the one the build was
/// given under the same name. An identifier, not a secret; empty when there is none.
fn microsoft_client_id() -> String {
    let built_in = option_env!("MAILCRAFT_MS_CLIENT_ID").unwrap_or_default().to_string();
    let id = std::env::var("MAILCRAFT_MS_CLIENT_ID").unwrap_or(built_in);
    let id = id.trim();
    if id.len() <= 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c)) { id.to_string() } else { String::new() }
}

/// Open one of Microsoft's pages in the system's browser: whether a browser could be started.
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

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn open_pages(_ctx: &egui::Context, _frame: &eframe::Frame) {}

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
fn run_window(options: eframe::NativeOptions, creator: eframe::AppCreator<'_>) -> eframe::Result {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        let _ = BROWSER.set(browser::Browser::new(std::sync::Arc::new(browser::Desktop)));
        let event_loop = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event().build()?;
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
    eframe::run_native("MailCraft", options, creator)
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
    use mailcraft_ui_egui::approval::{NEEDS_APPROVAL, NO_BROWSER, SIGN_IN_DECLINED, SIGN_IN_TIMED_OUT};
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
        let asked = mailcraft_net::oauth::sign_in_explained(endpoints, &request.oauth, &request.email, &open, &request.cancel);
        asked.map_err(|end| {
            let (how, said) = match end {
                SignInEnd::Approval => ("an administrator has to approve the application first", NEEDS_APPROVAL.to_string()),
                _ if no_browser.load(Ordering::Relaxed) => ("no browser could be started", NO_BROWSER.to_string()),
                SignInEnd::Declined => ("the page came back with a no", SIGN_IN_DECLINED.to_string()),
                SignInEnd::TimedOut => ("nothing came back in time", SIGN_IN_TIMED_OUT.to_string()),
                SignInEnd::Cancelled => ("it was called off", mailcraft_net::NetError::SignInCancelled.to_string()),
                SignInEnd::Other(e) => ("it failed", e.to_string()),
            };
            log::info!("sign-in on Microsoft's page, account {}: {how}", request.account);
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
        match mailcraft_net::oauth::refresh(&request.oauth, &request.refresh) {
            Ok(tokens) => return Ok(tokens),
            // The provider no longer takes what was kept: the owner signs in again (below).
            Err(e) if e.is_refused_sign_in() => {}
            // Trouble reaching the provider: what was kept is still good, so try again later.
            Err(e) => return Err(e.to_string()),
        }
    }
    // Nobody asked for this run: no page is put in front of the owner.
    if request.quiet {
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

/// How far back and ahead of today the calendar is brought down (seconds).
const CALENDAR_BEHIND: i64 = 31 * 86_400;
const CALENDAR_AHEAD: i64 = 366 * 86_400;

/// This machine's time zone by a name Microsoft Graph knows, for an upload that doesn't name
/// the zone the calendar is shown in (the engine names it whenever it has a name: this is what
/// is left for a zone it can't name); empty when the system doesn't say either. The name is the
/// one Windows has for the zone (`Pacific Standard Time`), which is the one Graph prefers, else
/// the one of the IANA database (`Europe/Berlin`).
fn local_zone() -> String {
    let zone = mailcraft_time::Zone::system();
    zone.windows_name().map_or_else(|| zone.id(), str::to_string)
}

/// What the server made of one calendar change, for the log: the kind of change, a tag made of
/// the event's id (the same in every line about that event, and nothing of the id can be read
/// from it), which of the event's properties were sent (their names, never what they say), the
/// zone its times were said in, the calendar, and the server's answer. Nothing of the event.
fn log_outcome(outcome: &mailcraft_model::remote::EventOutcome) {
    use mailcraft_model::remote::{EventResult, id_tag};
    let op = &outcome.op;
    // What the server calls it: for a new event, what it is called from now on.
    let id = [op.remote.as_str(), outcome.remote.as_str()].into_iter().find(|id| !id.is_empty());
    let tag = id.map_or("an event without a name yet".to_string(), |id| format!("#{}", id_tag(id)));
    let one = if op.of_occurrence() { format!(" (one occurrence of the series #{})", id_tag(&op.series)) } else { String::new() };
    let names = op.fields.names();
    let sent = if names.is_empty() { String::new() } else { format!(", sent [{}]", names.join(", ")) };
    let zone = if outcome.zone.is_empty() { String::new() } else { format!(" with times said in {}", outcome.zone) };
    let calendar = if op.calendar.is_empty() { "the main calendar".to_string() } else { format!("calendar #{}", id_tag(&op.calendar)) };
    let status = if outcome.status == 0 { "no answer".to_string() } else { format!("status {}", outcome.status) };
    match outcome.result {
        EventResult::Done => log::info!("calendar: {:?} {tag}{one}{sent}{zone}, in {calendar}: taken by the server ({status})", op.kind),
        result => {
            log::warn!(
                "calendar: {:?} {tag}{one}{sent}{zone}, in {calendar}: not taken by the server ({result:?}, {status}): {}",
                op.kind,
                outcome.error
            )
        }
    }
}

/// What Microsoft Graph does for an account before its mail is synchronised: send what waits in
/// the Outbox, and bring the calendar and the contacts down. Nothing here stops the mail: every
/// trouble is a line in the answer.
fn graph(request: GraphRequest) -> GraphResult {
    graph_in(request, &local_zone())
}

/// [`graph`], with the owner's time zone as `zone` (a name Microsoft Graph knows).
fn graph_in(request: GraphRequest, zone: &str) -> GraphResult {
    graph_in_with(request, zone, &mailcraft_net::oauth::refresh_graph_opened_calendars)
}

/// [`graph_in`], with how a token that covers other people's calendars is had as `renew_calendars`
/// (the tests' own, which asks nobody).
fn graph_in_with(
    request: GraphRequest,
    zone: &str,
    renew_calendars: &dyn Fn(&mailcraft_model::OAuth, &str) -> Result<mailcraft_net::oauth::GraphTokens, mailcraft_net::NetError>,
) -> GraphResult {
    use mailcraft_net::oauth;
    // A token for everything Graph is asked for, and for reading the Favorites of Outlook when
    // the window wants that too (it stops wanting it for the day once it was told no).
    // (An account kept in step through Graph has its token asked for with its mail, R3.)
    let mail = request.mail.is_some();
    // (A shared mailbox's, with what reading shared mailboxes takes: #70.)
    let shared = request.mail.as_ref().is_some_and(|mail| mail.shared);
    // (A shared mailbox's calendars, with what reading those takes: #70.)
    let shared_calendars = !mail && request.account.is_shared();
    let renew: Renew<'_> = &|settings, refresh, favorites| {
        if shared_calendars {
            oauth::refresh_graph_shared_calendars(settings, refresh)
        } else if shared {
            oauth::refresh_graph_shared(settings, refresh)
        } else if mail {
            oauth::refresh_graph_mail(settings, refresh)
        } else if favorites {
            oauth::refresh_graph_with_favorites(settings, refresh)
        } else {
            oauth::refresh_graph(settings, refresh)
        }
    };
    // (And for sending from a shared mailbox, when it has mail to send: #70.)
    let (request, opened) = opened_token(request, shared, renew_calendars);
    // (And a shared mailbox's calendar change that makes the server send mail from it.)
    let mut out = if shared_calendars {
        graph_sending_with(request, zone, renew, &oauth::refresh_graph_shared_calendars_send)
    } else {
        graph_sending_with(request, zone, renew, &oauth::refresh_graph_shared_send)
    };
    out.shared_calendars_consent_needed = opened.consent_needed;
    if out.tokens.is_none() {
        out.tokens = opened.tokens;
    }
    out
}

/// What [`opened_token`] found out.
#[derive(Default)]
struct OpenedToken {
    /// The token that was obtained, when one was: it covers other people's calendars.
    tokens: Option<Tokens>,
    /// The sign-in doesn't cover reading other people's calendars: the window asks the owner.
    consent_needed: bool,
}

/// A calendar job of an account with calendars opened by address (#70) needs a token that covers
/// reading other people's calendars (`Calendars.ReadWrite.Shared`); one that was handed in may
/// not, and a refusal of it would read as "no rights". So such a job gets its own, from the
/// refresh token, and when the sign-in doesn't cover it the job goes on without those calendars
/// (the account's own are read as ever) and says that the owner is to be asked. A job that is not
/// the calendar's (mail's, a shared mailbox's) never reads them, and is left as it is.
fn opened_token(
    mut request: GraphRequest,
    shared: bool,
    renew: &dyn Fn(&mailcraft_model::OAuth, &str) -> Result<mailcraft_net::oauth::GraphTokens, mailcraft_net::NetError>,
) -> (GraphRequest, OpenedToken) {
    let mut found = OpenedToken::default();
    if request.account.opened_calendars.is_empty() || request.mail.is_some() || shared {
        request.account.opened_calendars.clear();
        return (request, found);
    }
    if request.refresh.is_empty() {
        // (Nothing to renew with: what the token covers isn't known, so they are left for later.)
        request.account.opened_calendars.clear();
        return (request, found);
    }
    match renew(&request.oauth, &request.refresh) {
        Ok(got) => {
            request.access = got.tokens.access.clone();
            found.tokens = Some(got.tokens);
        }
        Err(e) => {
            found.consent_needed = e.is_consent_needed();
            log::info!("calendar: other people's calendars aren't read in this run (consent needed: {})", found.consent_needed);
            request.account.opened_calendars.clear();
        }
    }
    (request, found)
}

/// How a job gets a token for Microsoft Graph when the window handed it none: from the sign-in
/// settings and the refresh token, and whether one that is good for the Favorites is wanted.
type Renew<'a> = &'a dyn Fn(&mailcraft_model::OAuth, &str, bool) -> Result<mailcraft_net::oauth::GraphTokens, mailcraft_net::NetError>;

/// Why reading the Favorites of Outlook failed, in a few plain words for the folder pane.
fn favorites_words(error: &mailcraft_net::NetError) -> &'static str {
    match error {
        mailcraft_net::NetError::Busy { .. } => "Outlook is busy",
        error if error.is_unreachable() => "Couldn't reach Outlook",
        _ => "Couldn't read the list",
    }
}

/// [`graph_in`], with the token coming from `renew` when the request brings none (and, for a
/// shared mailbox's mail to send, from `renew` too).
#[cfg(test)]
fn graph_with(request: GraphRequest, zone: &str, renew: Renew<'_>) -> GraphResult {
    graph_sending_with(request, zone, renew, &|settings, refresh| renew(settings, refresh, false))
}

/// How a shared mailbox's run that has mail to send gets a token that covers sending from it
/// (`Mail.Send.Shared`, #70) when the window handed it none.
type RenewSend<'a> = &'a dyn Fn(&mailcraft_model::OAuth, &str) -> Result<mailcraft_net::oauth::GraphTokens, mailcraft_net::NetError>;

/// [`graph_in`], with the token coming from `renew` when the request brings none, and from
/// `renew_send` for a shared mailbox's run that has mail to send.
fn graph_sending_with(mut request: GraphRequest, zone: &str, renew: Renew<'_>, renew_send: RenewSend<'_>) -> GraphResult {
    use mailcraft_model::remote::{FavoritesDownload, FavoritesOutcome};
    use mailcraft_net::{NetError, graph};
    let mut out = GraphResult { account: request.account.id, ..Default::default() };
    let mut access = request.access.clone();
    // A shared mailbox with mail to send (#70): the token covers sending from it, or nothing of
    // it is sent this run (it waits, and the window asks the owner when he asked for the run).
    // Its changes and drafts go all the same, with what reading it takes.
    let sending_shared = request.mail.as_ref().is_some_and(|mail| mail.shared && !mail.sends.is_empty());
    if access.is_empty() && sending_shared {
        match renew_send(&request.oauth, &request.refresh) {
            Ok(got) => {
                access = got.tokens.access.clone();
                out.tokens = Some(got.tokens);
            }
            Err(e) => {
                if e.is_consent_needed() {
                    log::info!("graph sending: sending from a shared mailbox isn't allowed yet; its mail waits for the owner's permission");
                    out.send_consent_needed = true;
                } else {
                    log::info!("graph sending: no token for sending from a shared mailbox ({e}); its mail waits");
                }
                if let Some(mail) = request.mail.as_mut() {
                    mail.sends.clear();
                }
            }
        }
    }
    // A shared mailbox's calendar changes that make the server send mail to other people (an
    // answer, a meeting called off or forwarded, a meeting with people invited): they go with a
    // token that covers sending from it (#70), or they wait for the owner's permission, and
    // everything else goes all the same. (Same as its mail: the window asks the owner.)
    let sending_calendar = request.account.is_shared()
        && request.mail.is_none()
        && request.changes.as_ref().is_some_and(|changes| changes.ops.iter().any(mailcraft_model::remote::EventOp::mails_others));
    if sending_calendar {
        let kept = if request.refresh.is_empty() {
            Err(NetError::Config("no sign-in to renew".into()))
        } else {
            renew_send(&request.oauth, &request.refresh)
        };
        match kept {
            Ok(got) => {
                access = got.tokens.access.clone();
                out.tokens = Some(got.tokens);
            }
            Err(e) => {
                if e.is_consent_needed() {
                    log::info!(
                        "calendar: sending from a shared mailbox isn't allowed yet; what writes to other people waits for the owner's permission"
                    );
                    out.send_consent_needed = true;
                } else {
                    log::info!("calendar: no token for sending from a shared mailbox ({e}); what writes to other people waits");
                }
                if let Some(changes) = request.changes.as_mut() {
                    changes.ops.retain(|op| !op.mails_others());
                }
            }
        }
    }
    if access.is_empty() {
        // For everything Graph is asked for; a sign-in from before the newest of it was asked
        // for gets its tokens for what it covers, and says what is missing.
        match renew(&request.oauth, &request.refresh, request.favorites_scopes) {
            Ok(got) => {
                access = got.tokens.access.clone();
                out.extras_missing = got.extras_missing;
                // (Asked for without the Favorites' permissions, it says nothing of them.)
                out.favorites_covered = request.favorites_scopes.then_some(got.covers_favorites);
                out.tokens = Some(got.tokens);
            }
            Err(e) if e.is_consent_needed() => {
                log::info!("graph: the sign-in doesn't cover what was asked for; the owner's permission is needed");
                out.consent_needed = true;
                return out;
            }
            // A sign-in that has run out: the window asks the owner (the mail's leg does for an
            // account on IMAP; for one kept in step through Graph, the Graph run does).
            Err(e) if e.is_refused_sign_in() => {
                log::info!("graph: the sign-in was refused when a token was asked for");
                out.sign_in_refused = true;
                return out;
            }
            Err(e) => {
                log::warn!("graph: the sign-in couldn't be renewed: {e}");
                out.errors.push(format!("Couldn't reach Microsoft for sending, the calendar and the contacts: {e}"));
                return out;
            }
        }
    }
    let shared_address = request.account.shared_mailbox();
    let mut refused = false;
    for message in &request.outbox {
        if refused {
            // The token stopped working part-way: the rest waits for the next run and a new one.
            out.held.push(message.id);
            continue;
        }
        match graph::send(&access, message, &graph::SendOptions::default()) {
            Ok(()) => out.sent.push(message.id),
            // Not allowed to send this way: the mail server gets its turn in this run.
            Err(NetError::Forbidden(_)) => {}
            Err(e) if e.is_refused_sign_in() => {
                refused = true;
                out.held.push(message.id);
            }
            // Whether it went is not known, or Microsoft said later: not sent a second way now.
            Err(e) => {
                out.held.push(message.id);
                out.errors.push(format!("Couldn't send \"{}\" through Microsoft: {e}", message.subject.trim()));
            }
        }
    }
    // An account kept in step through Graph (R3): the folder changes made here go up, and every
    // folder is listed. (Nothing of a folder's name or id goes into the log.)
    if refused && request.mail.is_some() {
        log::warn!("graph mail: the token stopped working while sending; the mail waits for the next run");
    }
    if let Some(mail) = request.mail.as_ref().filter(|_| !refused) {
        // The run comes a piece at a time (the folders, then a page of mail at a time): to the
        // window as each comes, or put together for a caller that takes it in at once.
        let mut result: Option<mailcraft_model::sync::GraphMailResult> = None;
        let (cancel, pieces) = (&request.cancel, &request.mail_pieces);
        // When Graph stops taking the token part-way (it ran out during a long run, or while the
        // computer slept), one new one from the refresh token, the newest there is (#306).
        let renewed: std::cell::RefCell<Option<mailcraft_model::sync::Tokens>> = std::cell::RefCell::new(None);
        let refresh = out.tokens.as_ref().map(|t| t.refresh.clone()).filter(|r| !r.is_empty()).unwrap_or_else(|| request.refresh.clone());
        let again = || {
            let got = if mail.shared && !mail.sends.is_empty() {
                renew_send(&request.oauth, &refresh)?
            } else {
                renew(&request.oauth, &refresh, request.favorites_scopes)?
            };
            let access = got.tokens.access.clone();
            *renewed.borrow_mut() = Some(got.tokens);
            Ok(access)
        };
        graph::sync_mail_renewing(&access, &again, mail, cancel, &mut |piece| match pieces {
            Some(to) => {
                let summary = (piece.listing.as_ref().map(|l| l.folders.len()), piece.errors.len());
                // Nobody is listening any more (the window closed): stop.
                if to.send(piece).is_err() {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if let (Some(folders), problems) = summary {
                    log::info!("graph folders: {folders} folders listed, {problems} problems");
                }
            }
            None => match &mut result {
                Some(all) => all.absorb(piece),
                None => result = Some(piece),
            },
        });
        if let Some(result) = result {
            log::info!(
                "graph folders: {} of {} changes answered, {} folders listed, complete: {}, {} problems",
                result.changes.len(),
                mail.changes.len(),
                result.listing.as_ref().map_or(0, |listing| listing.folders.len()),
                result.listing.as_ref().is_some_and(|listing| listing.complete),
                result.errors.len()
            );
            out.mail = Some(result);
        }
        // A token renewed part-way is the one kept, and the one the rest of the run shows.
        if let Some(tokens) = renewed.into_inner() {
            access = tokens.access.clone();
            out.tokens = Some(tokens);
        }
    }
    // What was made, moved, changed or deleted in the calendar here goes up before the calendar
    // comes down: what comes down then is what the server has after it.
    if let Some(changes) = request.changes.as_ref().filter(|changes| !changes.ops.is_empty() && !refused) {
        log::info!("calendar: {} changes go to the server", changes.ops.len());
        let uploaded =
            graph::push_events(&access, changes, &graph::EventOptions { zone: zone.to_string(), shared: shared_address.clone().unwrap_or_default() });
        // What the server made of each change, for the log: the kind of change, what was sent
        // of it by name, and the answer, never what the event says.
        for outcome in &uploaded.outcomes {
            log_outcome(outcome);
        }
        if uploaded.outcomes.len() < changes.ops.len() {
            log::warn!("calendar: {} of {} changes were answered; the rest wait", uploaded.outcomes.len(), changes.ops.len());
        }
        refused = uploaded.token_refused;
        out.uploaded = Some(uploaded);
    }
    // The wide stretch with the contacts, every quarter of an hour; otherwise the days on screen.
    let now = mailcraft_time::now();
    let stretch = if request.directory { Some((now - CALENDAR_BEHIND, now + CALENDAR_AHEAD)) } else { request.calendar };
    // A shared mailbox's calendars (#70): asked for and written at its own address, and only
    // those that are ticked are asked for (the window says which, in the asks); no contacts,
    // categories or settings of it.
    // The series the mailbox expects in each calendar's stretch: asked about by name when the
    // stretch brings nothing of them.
    let expected = |calendar: &str| request.asks.iter().find(|ask| ask.calendar == calendar).map(|ask| ask.series.clone()).unwrap_or_default();
    // Which calendars the account has, and in which groups: with the wide stretch.
    let mut others: Vec<String> = request.asks.iter().map(|ask| ask.calendar.clone()).filter(|calendar| !calendar.is_empty()).collect();
    if request.directory && !refused {
        let listed = match &shared_address {
            Some(address) => graph::shared_calendars(&access, address),
            None => graph::calendars(&access),
        };
        match listed {
            Ok(mut list) => {
                // The calendars the owner opened by address (#70), each read at its own address:
                // those that can be read are in the list as calendars of the account, and those
                // that can't are said, with why. (Not for a shared mailbox's job.)
                if shared_address.is_none() {
                    // The copy of a shared mailbox's calendar that the owner hid (the server lists
                    // it among the account's own too, #70) isn't read: the shared mailbox's job
                    // reads its calendar at its own address.
                    list.calendars.retain(|calendar| calendar.main || !request.account.hides_listed(calendar.owner.as_ref()));
                    let (opened, no_good) = open_by_address(&access, &request.account.opened_calendars);
                    list.calendars.extend(opened);
                    list.refused = no_good.0;
                    refused |= no_good.1;
                }
                // Every calendar there is comes with the wide stretch, known here already or not.
                // (Not from a list that doesn't say which one is the main calendar: that one
                // comes anyway, and would then come twice.) A shared mailbox's only when ticked.
                let told_apart = list.calendars.iter().any(|calendar| calendar.main);
                for calendar in list.calendars.iter().filter(|calendar| told_apart && !calendar.main && shared_address.is_none()) {
                    if !others.contains(&calendar.id) {
                        others.push(calendar.id.clone());
                    }
                }
                out.calendars = Some(list.for_account(&request.account));
            }
            Err(e) if e.is_refused_sign_in() => refused = true,
            // A shared mailbox that lists no calendars to this account (its calendar is shared
            // for free and busy times only, or not at all): its main one is asked for all the
            // same, and says which.
            Err(NetError::Forbidden(_)) if shared_address.is_some() => log::info!("calendar: a shared mailbox lists no calendars to this account"),
            Err(e) => out.errors.push(format!("Couldn't bring the list of calendars down: {e}")),
        }
    }
    if let Some((from, to)) = stretch.filter(|_| !refused) {
        let shared = shared_address.clone().unwrap_or_default();
        // (A shared mailbox's main calendar when it is ticked.)
        let wants_main = shared_address.is_none() || request.asks.iter().any(|ask| ask.calendar.is_empty());
        let main = graph::EventsQuery { calendar: String::new(), from, to, series: expected(""), shared: shared.clone() };
        let days = |instant: i64| mailcraft_time::DateTime::from_timestamp(instant, 0).date.iso();
        match wants_main.then(|| graph::events_in(&access, &main)) {
            None => {}
            Some(Ok(events)) => {
                log::info!(
                    "calendar: the main calendar came down: {} events from {} to {} (all of them: {})",
                    events.events.len(),
                    days(from),
                    days(to),
                    events.complete
                );
                out.events = Some(events.for_account(&request.account));
            }
            Some(Err(e)) if e.is_refused_sign_in() => refused = true,
            Some(Err(e)) if shared_address.is_some() => out.errors.push(format!("Couldn't bring the shared mailbox's calendar down: {e}")),
            Some(Err(e)) => out.errors.push(format!("Couldn't bring the calendar down: {e}")),
        }
        // Those opened by address come first (the owner asked for them by name): they count
        // within the same cap as the account's other calendars.
        others.sort_by_key(|calendar| !calendar.starts_with(mailcraft_model::remote::OPENED_PREFIX));
        for calendar in others.iter().take(MAX_OTHER_CALENDARS) {
            if refused {
                break;
            }
            let query = graph::EventsQuery { calendar: calendar.clone(), from, to, series: expected(calendar), shared: shared.clone() };
            match graph::events_in(&access, &query) {
                Ok(events) => {
                    log::info!(
                        "calendar: calendar #{} came down: {} events (all of them: {})",
                        mailcraft_model::remote::id_tag(calendar),
                        events.events.len(),
                        events.complete
                    );
                    out.more_events.push(events.for_account(&request.account));
                }
                Err(e) if e.is_refused_sign_in() => refused = true,
                // A calendar that is gone, or may not be read: the list says so the next time.
                Err(e) => log::debug!("one of the account's other calendars didn't come down: {e}"),
            }
        }
    }
    if request.directory && !refused && shared_address.is_none() {
        match graph::contacts(&access) {
            Ok(contacts) => out.contacts = Some(contacts.for_account(&request.account)),
            Err(e) if e.is_refused_sign_in() => refused = true,
            Err(e) => out.errors.push(format!("Couldn't bring the contacts down: {e}")),
        }
    }
    // The mailbox's own categories and their colours: an extra that an older sign-in doesn't
    // cover, and nothing to trouble the owner with when it can't be had.
    if request.directory && !refused && shared_address.is_none() {
        match graph::categories(&access) {
            Ok(categories) => out.categories = Some(categories.for_account(&request.account)),
            Err(NetError::Forbidden(_)) => out.extras_missing = true,
            Err(e) if e.is_refused_sign_in() => refused = true,
            Err(e) => log::debug!("the list of categories didn't come down: {e}"),
        }
    }
    // The working hours the mailbox has set: an extra of the same kind, and only ever read.
    if request.settings && !refused {
        match graph::mailbox_settings(&access) {
            Ok(settings) => out.mailbox_hours = settings.working_hours.map(|hours| mailbox_hours(&request.account.email, hours)),
            Err(NetError::Forbidden(_)) => out.extras_missing = true,
            Err(e) if e.is_refused_sign_in() => refused = true,
            Err(e) => log::debug!("the mailbox's working hours didn't come down: {e}"),
        }
    }
    // The Favorites as Outlook has them: read, never written, and nothing to trouble the owner
    // with when they can't be had (the folder pane says what there is to say, in one line).
    if request.favorites && !refused {
        let outcome = |outcome: FavoritesOutcome| Some(FavoritesDownload { outcome, ..Default::default() }.for_account(&request.account));
        if out.favorites_covered == Some(false) {
            // The sign-in doesn't cover it: the server isn't asked what it would only refuse.
            log::info!("favorites: the sign-in doesn't cover reading them");
            out.favorites = outcome(FavoritesOutcome::NotPermitted);
        } else {
            match graph::favorites(&access) {
                Ok(download) => out.favorites = Some(download.for_account(&request.account)),
                Err(NetError::Forbidden(_)) => {
                    log::info!("favorites: the server says the sign-in doesn't cover reading them (status 403)");
                    out.favorites = outcome(FavoritesOutcome::NotPermitted);
                }
                Err(e) if e.is_refused_sign_in() => refused = true,
                // (The error says what was being done and the status: no name, no id.)
                Err(e) => {
                    log::warn!("favorites: they couldn't be read: {e}");
                    out.favorites = outcome(FavoritesOutcome::Failed);
                    out.favorites_error = favorites_words(&e).to_string();
                }
            }
        }
    }
    out.token_refused = refused;
    out
}

/// File › Info › Check Calendar Connection: on the account's real server, and only because the
/// owner asked for it and said yes to what it does. One appointment is made a year from today
/// in the main calendar, read back, changed (once as an instant, once said in this computer's
/// time zone), read back each time and deleted; what the server answered at each step goes back
/// to the window, which shows and logs it. Nothing else of the calendar is read or changed.
fn check_calendar(request: CalendarCheckRequest) -> CalendarCheckResult {
    check_calendar_in(request, &local_zone(), &mailcraft_time::local_offset_minutes)
}

/// [`check_calendar`], with the owner's time zone as `zone` and how far east of UTC it is at an
/// instant as `offset_at` (minutes).
fn check_calendar_in(request: CalendarCheckRequest, zone: &str, offset_at: &dyn Fn(i64) -> i32) -> CalendarCheckResult {
    use mailcraft_net::{graph, oauth};
    let mut out = CalendarCheckResult { account: request.account.id, cleaned_up: true, ..Default::default() };
    let mut access = request.access.clone();
    if access.is_empty() {
        match oauth::refresh_graph(&request.oauth, &request.refresh) {
            Ok(got) => {
                access = got.tokens.access.clone();
                out.tokens = Some(got.tokens);
            }
            Err(e) => {
                out.consent_needed = e.is_consent_needed();
                out.token_refused = e.is_refused_sign_in();
                out.error = format!("Couldn't sign in to Microsoft for the calendar: {e}");
                return out;
            }
        }
    }
    let check = graph::check_calendar(&access, mailcraft_time::now(), zone, offset_at);
    (out.ok, out.cleaned_up, out.token_refused) = (check.ok(), check.cleaned_up, check.token_refused);
    out.steps = check
        .steps
        .into_iter()
        .map(|step| CalendarCheckStep { what: step.what.to_string(), ok: step.ok, status: step.status, said: step.said })
        .collect();
    out
}

/// A mailbox's working hours as the engine keeps them, for the account with the address `email`.
/// The zone they are in goes along by the name the mailbox has for it (`Pacific Standard Time`):
/// the engine knows the zones, and shifts the hours to the clock the calendar is shown by
/// (`calendar_settings::work_hours`). No offset is given beside it: a zone the engine doesn't
/// know (`Customized Time Zone`) is one this program can't look up either, and its hours are
/// then taken as they are written.
fn mailbox_hours(email: &str, hours: mailcraft_net::graph::WorkingHours) -> mailcraft_engine::cmd::calendar_settings::MailboxHours {
    mailcraft_engine::cmd::calendar_settings::MailboxHours {
        email: email.trim().to_string(),
        days: hours.days,
        start: hours.start,
        end: hours.end,
        zone: hours.zone,
        offset: None,
    }
}

/// Most calendars of an account, beside its main one, that are brought down in one go: those
/// the server lists and those the owner opened by address, together.
const MAX_OTHER_CALENDARS: usize = 20;

/// Read the calendars opened by address with `token`: the calendars that can be read (named by
/// their owners, with their rights), and those that can't, with why; and whether the sign-in
/// itself was refused. At most [`mailcraft_model::MAX_OPENED_CALENDARS`], one request each.
fn open_by_address(
    token: &str,
    opened: &[mailcraft_model::OpenedCalendar],
) -> (Vec<mailcraft_model::remote::RemoteCalendar>, (Vec<mailcraft_model::remote::OpenedRefusal>, bool)) {
    use mailcraft_model::remote::OpenedRefusal;
    use mailcraft_net::graph::{self, Opened};
    let (mut read, mut refusals, mut signed_out) = (Vec::new(), Vec::new(), false);
    for one in opened.iter().take(mailcraft_model::MAX_OPENED_CALENDARS) {
        if signed_out {
            break;
        }
        match graph::open_calendar(token, &one.address) {
            Opened::Read(calendar) => read.push(*calendar),
            Opened::Refused(why) => {
                // (Said by what happened, never by whom: the address is not in the log.)
                log::info!("calendar: a calendar opened by address couldn't be read ({why:?})");
                refusals.push(OpenedRefusal { address: one.address.clone(), why });
            }
            Opened::SignInRefused => signed_out = true,
        }
    }
    (read, (refusals, signed_out))
}

/// When the people a meeting is for are free and busy, for its Scheduling Assistant: one
/// question to Microsoft Graph, about when their time is taken and how, never by what. It
/// reads; nothing is written, and nobody is told that it was asked.
fn free_busy(request: FreeBusyRequest) -> FreeBusyResult {
    use mailcraft_net::{graph, oauth};
    let mut out = FreeBusyResult { account: request.account.id, ..Default::default() };
    let mut access = request.access.clone();
    if access.is_empty() {
        match oauth::refresh_graph(&request.oauth, &request.refresh) {
            Ok(got) => {
                access = got.tokens.access.clone();
                out.tokens = Some(got.tokens);
            }
            Err(e) => {
                out.consent_needed = e.is_consent_needed();
                out.error = format!("Couldn't reach Microsoft to see when people are free: {e}");
                return out;
            }
        }
    }
    match graph::schedule(&access, &request.ask) {
        Ok(schedule) => out.schedule = Some(schedule),
        Err(e) => {
            out.token_refused = e.is_refused_sign_in();
            out.error = format!("Couldn't see when people are free: {e}");
        }
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
    let search_ctx = ctx.clone();
    let update_ctx = ctx.clone();
    let download_ctx = ctx.clone();
    let copy_ctx = ctx.clone();
    let alert_ctx = ctx.clone();
    let (sync_ctx, sign_in_ctx, graph_ctx, mx_ctx, test_ctx) = (ctx.clone(), ctx.clone(), ctx.clone(), ctx.clone(), ctx);
    let sync_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel = sync_cancel.clone();
    Services {
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
        sign_in: Some(Box::new(move |request| {
            let ctx = sign_in_ctx.clone();
            background(&sign_in_ctx, "sign-in", move || sign_in(&ctx, request))
        })),
        graph: Some(Box::new(move |request| background(&graph_ctx, "microsoft graph", move || graph(request)))),
        // Messages of a Microsoft account fetched whole, a few at a time (R3.2).
        graph_fetch: Some(Box::new(move |job: mailcraft_ui_egui::graph_fetch::GraphFetchJob| {
            background(&fetch_ctx, "message download", move || mailcraft_net::graph::fetch_messages(&job.access, &job.request, &job.cancel))
        })),
        // A Microsoft mailbox searched on its server, a page at a time, on the owner's click (R6.2).
        graph_search: Some(Box::new(move |job: mailcraft_ui_egui::server_search::GraphSearchJob| {
            background(&search_ctx, "server search", move || mailcraft_net::graph::search_messages(&job.access, &job.request))
        })),
        lookup_mx: Some(Box::new(move |domain| {
            background(&mx_ctx, "mail exchangers", move || mailcraft_net::lookup_mx(&domain).map_err(|e| e.to_string()))
        })),
        test_account: Some(Box::new(move |account, secret| {
            background(&test_ctx, "account check", move || mailcraft_net::test_account(&account, &secret).map_err(|e| e.to_string()))
        })),
        // The copy of the mailbox kept before an account switches to Microsoft Graph: a large
        // mailbox takes a while to write, and the window shows a spinner meanwhile (#118).
        write_copy: Some(Box::new(move |path, mailbox| {
            background(&copy_ctx, "mailbox copy", move || mailcraft_engine::store::save_single(&path, &mailbox).map(|_| ()))
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
        // When the people of a meeting are free and busy, while its Scheduling Assistant is open.
        free_busy: Some(Box::new(move |request| background(&busy_ctx, "free and busy times", move || free_busy(request)))),
        // A check of the calendar connection on the account's real server, when the owner asks.
        check_calendar: Some(Box::new(move |request| background(&check_ctx, "calendar check", move || check_calendar(request)))),
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
        secrets: secrets::store(),
        // File › Info › Remove MailCraft's data from this computer (#177): the folders this
        // program works in, and no others. Not offered by a run that keeps nothing.
        data_wipe: prefs_enabled().then(|| {
            Box::new(wipe::Wipe { roots: wipe::Roots::of_this_program(), pictures: wipe_pictures })
                as Box<dyn mailcraft_ui_egui::data_removal::DataWipe>
        }),
        // Desktop alerts, through the system's notifications (`alerts`).
        alert: alerts::host(alert_ctx),
        clipboard: clipboard::board(),
        // Help › Copy Diagnostics: the version, the platform and the redacted end of the logs.
        diagnostics: Some(Box::new(|| diagnostics::report(env!("CARGO_PKG_VERSION"), build_sha(), log_dir().filter(|_| prefs_enabled()).as_deref()))),
        microsoft_client_id: microsoft_client_id(),
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

/// The mailbox to start with, the file it is kept in (none for the sample, or without prefs),
/// how many bodies and attachments couldn't be read back from the content folder beside it, and
/// whether part of the mailbox file itself was damaged and left out.
fn first_mailbox(sample: bool, chosen: Option<PathBuf>) -> (Mailbox, Option<PathBuf>, usize, bool) {
    let now = mailcraft_time::now();
    if sample {
        return (mailcraft_engine::sample::mailbox(now, mailcraft_time::local_offset_minutes(now)), None, 0, false);
    }
    let ours = chosen.is_none();
    let path = chosen.or_else(|| default_mailbox_path().filter(|_| prefs_enabled()));
    let Some(path) = path else { return (Mailbox::with_local_account("", ""), None, 0, false) };
    if !path.exists() {
        // A first start: the settings folder has to be there for the mailbox to be saved in it.
        if ours && let Some(dir) = path.parent() {
            let _ = mailcraft_engine::store::create_private_dir_all(dir);
        }
        return (Mailbox::with_local_account("", ""), Some(path), 0, false);
    }
    match mailcraft_engine::store::open(&path) {
        Ok(opened) => {
            if let Some(notice) = opened.notice() {
                log::warn!("{}: {notice}", path.display());
            }
            let damaged = opened.lost_saves > 0 || opened.unfinished > 0 || opened.lost_records > 0;
            (opened.mailbox, Some(path), opened.unavailable, damaged)
        }
        Err(e) => {
            // Never write over a mailbox that couldn't be read: start empty, keep nothing, say why.
            log::error!("{e}");
            (Mailbox::with_local_account("", ""), None, 0, false)
        }
    }
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
    // The settings folder holds the mailbox, the control key and the logs: for the owner alone.
    // One an earlier version left open to others is closed; best effort, never a reason not to
    // start (a folder of somebody else's, named by `--mailbox`, is not touched).
    if let Some(dir) = mailcraft_control_key::settings_dir().filter(|_| prefs_enabled())
        && let Err(e) = mailcraft_engine::store::make_private(&dir)
    {
        log::debug!("the settings folder was not made private: {e}");
    }
    // One window per mailbox file: started again, MailCraft hands what it was started with to
    // the window that has the mailbox and exits. The sample keeps no file, so it isn't guarded.
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
        None => None,
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
            .with_app_id("io.github.billsecond.mailcraft")
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
    // Before eframe creates the wgpu instance: default Windows to DirectX 12 only, or OpenGL when
    // DirectX 12 has no GPU driver (see graphics.rs).
    #[cfg(target_os = "windows")]
    graphics::configure(&mut options, eframe::wgpu::Backends::from_env());
    let restored = saved_window();
    if let Some(window) = restored {
        options.viewport = window.apply(options.viewport);
    }
    // Registered before the event loop starts, so it catches the Finder event that launched us as
    // well as later ones. Lives until the event loop returns; the app creator only borrows it.
    #[cfg(target_os = "macos")]
    let apple_events = apple_events::AppleEvents::install();
    #[cfg(target_os = "macos")]
    let apple_events = &apple_events;
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
            let (mailbox, path, unavailable, damaged) = first_mailbox(sample, mailbox_path);
            let (file_dialog, dialogs) = file_dialogs::hook(&cc.egui_ctx);
            let mut session = Session::new(mailbox);
            let kept = path.is_some();
            session.path = path;
            // Search answers from a word index kept beside the mailbox file, built in the
            // background (R6.2).
            session.index_words = true;
            // A run that keeps its mailbox and its settings keeps downloaded pictures too.
            let kept_pictures = if kept && !sample && prefs_enabled() { keep_pictures() } else { None };
            let mut services = services(file_dialog, cc.egui_ctx.clone(), kept_pictures, updates.as_ref());
            services.update_start = update_start;
            let mut app = MailApp::new(session, services);
            // Nowhere to keep the mailbox (the sample, a run without preferences): don't try.
            app.autosave = kept;
            if let Some(path) = &app.session.path {
                app.services.saver = background_saver(&cc.egui_ctx, path);
            }
            if damaged {
                app.status(mailcraft_ui_egui::damaged_mailbox_notice());
            } else if unavailable > 0 {
                app.status(format!("{unavailable} bodies or attachments are missing from the mailbox's content folder, or damaged."));
            }
            // A mailbox whose Outlook Favorites were read another way than Microsoft Graph, which
            // is no more: said once, and the file is saved without it.
            if app.session.mailbox.take_retired_favorites_choices() > 0 {
                app.session.mailbox_changed();
                app.status(mailcraft_ui_egui::favorites_sync::FAVORITES_NOW_GRAPH);
            }
            // Mail of Microsoft accounts kept with characters that couldn't be read (fetched
            // before their character sets were) is fetched again, once (#185).
            let again = mailcraft_engine::cmd::graph_down::refetch_unreadable(&mut app.session.mailbox);
            if again > 0 {
                app.session.mailbox_changed();
                app.status(format!("{again} messages with characters that couldn't be read are being downloaded again."));
            }
            // An ordinary start on a mailbox nobody has set up yet begins with adding an account.
            app.offer_setup = kept && !sample;
            // A Microsoft account still on IMAP is offered the switch to Graph (R3.7): only for a
            // mailbox kept in a file, which is where the copy from before the switch goes.
            app.offer_graph_switch = kept && !sample;
            load_prefs(&mut app);
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
            for f in files {
                app.open_path(&f);
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
                #[cfg(target_os = "macos")]
                apple_events.connect(&cc.egui_ctx),
                #[cfg(target_os = "macos")]
                menu_bar,
            )))
        }),
    )
}

#[cfg(test)]
mod prefs_tests {
    use super::*;

    #[test]
    fn prefs_written_whole_and_a_damaged_file_kept_aside() {
        let dir = std::env::temp_dir().join(format!("mailcraft-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("ui.json");
        let ui = UiState::default();
        write_prefs(&path, &ui).unwrap();
        let back = read_prefs(&path).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&ui).unwrap());
        // Nothing left beside it but the file itself.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        std::fs::write(&path, b"{\"theme\": ").unwrap();
        assert!(read_prefs(&path).is_none());
        assert!(!path.exists());
        assert_eq!(std::fs::read(dir.join("ui.json.damaged")).unwrap(), b"{\"theme\": ");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
