//! MailCraft desktop app.
//!
//! Usage: `mailcraft [--control <port>] [--sample] [--mailbox <file>] [files or mailto: links…]`
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
//! removed on exit). See `docs/control-protocol.md`.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "macos")]
mod apple_events;
#[cfg(test)]
mod approval_flow;
#[cfg(test)]
mod calendar_flow;
mod control_server;
#[cfg(test)]
mod favorites_choice_flow;
#[cfg(test)]
mod favorites_flow;
mod file_dialogs;
#[cfg(any(target_os = "windows", test))]
mod graphics;
mod logging;
mod notifications;
mod picture_cache;
mod printing;
mod secrets;
mod single_instance;

use std::path::PathBuf;

use mailcraft_engine::Session;
use mailcraft_model::Mailbox;
use mailcraft_model::sync::Tokens;
use mailcraft_ui_egui::{
    AdminConsentAnswer, AdminConsentRequest, CalendarCheckRequest, CalendarCheckResult, CalendarCheckStep, EwsRequest, EwsResult, FreeBusyRequest,
    FreeBusyResult, GraphRequest, GraphResult, MailApp, Services, SignInRequest, UiState,
    window_geometry::{WindowGeometry, take_rescue},
};

/// The app, the restored window geometry until the first frame has checked it, the control
/// server's key file (removed on exit), the file dialogs the app asked for, the hold on the
/// mailbox that later starts hand their files and links to and, on macOS, the files opened from
/// Finder.
struct App(
    MailApp,
    Option<WindowGeometry>,
    Option<control_server::KeyFile>,
    file_dialogs::Launcher,
    Option<single_instance::Guard>,
    #[cfg(target_os = "macos")] fmv_macos_events::Inbox,
);

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        apple_events::poll(&self.5, &mut self.0, ctx);
        // MailCraft was started again (the shortcut, a `mailto:` link, a file opened from the
        // file manager): what it was started with opens here, and the window comes forward.
        if let Some(asked) = self.4.as_ref().and_then(single_instance::Guard::handed_over) {
            for what in &asked {
                self.0.open_path(what);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.0.logic(ctx);
        self.3.show(frame);
        let prev = self.0.ui.window;
        self.0.ui.window = ctx.input(|i| WindowGeometry::track(prev, i.viewport(), i.viewport_rect().size()));
        if let Some(pos) = ctx.input(|i| take_rescue(&mut self.1, i.viewport())) {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
        // The window's close button: a message being written asks first (`MailApp::close_requested`).
        if ctx.input(|i| i.viewport().close_requested()) && !self.0.close_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        if self.0.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.0.raw_input_hook(raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.0.ui(ui);
        self.3.show(frame);
    }
    fn on_exit(&mut self) {
        save_prefs(&self.0);
        // The last changes, if the timer hadn't got to them yet, and whatever the background
        // saver is still writing.
        if let Err(e) = self.0.save_on_exit() {
            log::error!("saving the mailbox on exit failed: {e}");
        }
        // What is still to be noted of the pictures kept on disk.
        if let Some(pictures) = PICTURES.get() {
            pictures.flush();
        }
        if let Some(key_file) = &self.2 {
            key_file.remove();
        }
        if let Some(guard) = &self.4 {
            guard.close();
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
    if let Some(p) = prefs_path()
        && let Ok(bytes) = std::fs::read(&p)
        && let Ok(ui) = serde_json::from_slice::<UiState>(&bytes)
    {
        app.apply_prefs(ui);
    }
}

/// The saved window geometry, read before the window opens (`load_prefs` runs after).
fn saved_window() -> Option<WindowGeometry> {
    if !prefs_enabled() {
        return None;
    }
    let bytes = std::fs::read(prefs_path()?).ok()?;
    serde_json::from_slice::<UiState>(&bytes).ok()?.window?.sanitized()
}

fn save_prefs(app: &MailApp) {
    if !prefs_enabled() {
        return;
    }
    if let Some(p) = prefs_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&app.prefs()) {
            let _ = std::fs::write(&p, bytes);
        }
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
/// (The same call the window's own links end in; made here so that a browser that can't be
/// started is known of, instead of leaving the owner looking at a page that waits.) The address
/// is not for the log: it names who is signing in.
fn open_in_browser(url: &str) -> bool {
    match webbrowser::open(url) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("couldn't start a browser for Microsoft's page ({:?})", e.kind());
            false
        }
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
    // The owner chose Exchange Web Services for Outlook's Favorites, and the sign-in doesn't
    // cover it: the page comes up for that permission alone. Nothing is asked a second time.
    if request.ews {
        let asked = mailcraft_net::oauth::sign_in_for_ews(&request.oauth, &request.email, &open, &request.cancel);
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
        return on_the_page(&Endpoints::for_resource(&request.oauth, Resource::Graph));
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
    match mailcraft_net::oauth::admin_consent(&request.oauth, &request.tenant, request.source, &open, &request.cancel) {
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

/// Read an account's Favorites of Outlook through Exchange Web Services, for an owner who
/// chose that way: a token for it from the refresh token when the window handed none (nobody
/// is asked anything for it), and then the list. It only reads, and nothing else waits on it:
/// every trouble is a word in the answer.
fn ews(request: EwsRequest) -> EwsResult {
    ews_with(request, &mailcraft_net::oauth::refresh_ews)
}

/// How a read through Exchange Web Services gets its token when the window handed it none.
type RenewEws<'a> = &'a dyn Fn(&mailcraft_model::OAuth, &str) -> Result<Tokens, mailcraft_net::NetError>;

/// [`ews`], with the token coming from `renew` when the request brings none.
fn ews_with(request: EwsRequest, renew: RenewEws<'_>) -> EwsResult {
    use mailcraft_model::remote::{FavoritesDownload, FavoritesOutcome};
    use mailcraft_net::NetError;
    let mut out = EwsResult { account: request.account.id, ..Default::default() };
    let outcome = |outcome: FavoritesOutcome| Some(FavoritesDownload { outcome, ..Default::default() }.for_account(&request.account));
    let mut access = request.access.clone();
    if access.is_empty() {
        match renew(&request.oauth, &request.refresh) {
            Ok(tokens) => {
                access = tokens.access.clone();
                out.tokens = Some(tokens);
            }
            // The sign-in is good and doesn't cover this: the owner is asked, if they chose to be.
            Err(e) if e.is_consent_needed() => {
                log::info!("favorites (EWS): the sign-in doesn't cover Exchange Web Services");
                out.consent_needed = true;
                return out;
            }
            // A sign-in that has run out is the mail's to notice and ask about, once: nothing
            // is said of the Favorites for it, and nothing is asked again before its time.
            Err(e) if e.is_refused_sign_in() => return out,
            Err(e) => {
                log::warn!("favorites (EWS): no token for Exchange Web Services: {e}");
                out.favorites = outcome(FavoritesOutcome::Failed);
                out.favorites_error = favorites_words(&e).to_string();
                return out;
            }
        }
    }
    match mailcraft_net::ews::favorites(&access) {
        Ok(download) => out.favorites = Some(download.for_account(&request.account)),
        // The organisation doesn't allow the service: its own no, said by the server's header.
        Err(NetError::Blocked(why)) => {
            log::info!("favorites (EWS): {why}");
            out.blocked = true;
        }
        Err(NetError::Forbidden(_)) => {
            log::info!("favorites (EWS): the server says this account may not read them (status 403)");
            out.favorites = outcome(FavoritesOutcome::NotPermitted);
        }
        // The token the window had kept is no longer taken: the next look gets another. (One
        // that was only just obtained and isn't taken either is a read that failed.)
        Err(e) if e.is_refused_sign_in() && out.tokens.is_none() => out.token_refused = true,
        // (The error says what was being done and the status: no name, no id.)
        Err(e) => {
            log::warn!("favorites (EWS): they couldn't be read: {e}");
            out.favorites = outcome(FavoritesOutcome::Failed);
            out.favorites_error = favorites_words(&e).to_string();
        }
    }
    out
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
    use mailcraft_net::oauth;
    // A token for everything Graph is asked for, and for reading the Favorites of Outlook when
    // the window wants that too (it stops wanting it for the day once it was told no).
    graph_with(request, zone, &|settings, refresh, favorites| {
        if favorites { oauth::refresh_graph_with_favorites(settings, refresh) } else { oauth::refresh_graph(settings, refresh) }
    })
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

/// [`graph_in`], with the token coming from `renew` when the request brings none.
fn graph_with(request: GraphRequest, zone: &str, renew: Renew<'_>) -> GraphResult {
    use mailcraft_model::remote::{FavoritesDownload, FavoritesOutcome};
    use mailcraft_net::{NetError, graph};
    let mut out = GraphResult { account: request.account.id, ..Default::default() };
    let mut access = request.access.clone();
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
                out.consent_needed = true;
                return out;
            }
            // A sign-in that has run out is the mail's to notice and ask about, once.
            Err(e) if e.is_refused_sign_in() => return out,
            Err(e) => {
                out.errors.push(format!("Couldn't reach Microsoft for sending, the calendar and the contacts: {e}"));
                return out;
            }
        }
    }
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
    // What was made, moved, changed or deleted in the calendar here goes up before the calendar
    // comes down: what comes down then is what the server has after it.
    if let Some(changes) = request.changes.as_ref().filter(|changes| !changes.ops.is_empty() && !refused) {
        log::info!("calendar: {} changes go to the server", changes.ops.len());
        let uploaded = graph::push_events(&access, changes, &graph::EventOptions { zone: zone.to_string() });
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
    // The series the mailbox expects in each calendar's stretch: asked about by name when the
    // stretch brings nothing of them.
    let expected = |calendar: &str| request.asks.iter().find(|ask| ask.calendar == calendar).map(|ask| ask.series.clone()).unwrap_or_default();
    // Which calendars the account has, and in which groups: with the wide stretch.
    let mut others: Vec<String> = request.asks.iter().map(|ask| ask.calendar.clone()).filter(|calendar| !calendar.is_empty()).collect();
    if request.directory && !refused {
        match graph::calendars(&access) {
            Ok(list) => {
                // Every calendar there is comes with the wide stretch, known here already or not.
                // (Not from a list that doesn't say which one is the main calendar: that one
                // comes anyway, and would then come twice.)
                let told_apart = list.calendars.iter().any(|calendar| calendar.main);
                for calendar in list.calendars.iter().filter(|calendar| told_apart && !calendar.main) {
                    if !others.contains(&calendar.id) {
                        others.push(calendar.id.clone());
                    }
                }
                out.calendars = Some(list.for_account(&request.account));
            }
            Err(e) if e.is_refused_sign_in() => refused = true,
            Err(e) => out.errors.push(format!("Couldn't bring the list of calendars down: {e}")),
        }
    }
    if let Some((from, to)) = stretch.filter(|_| !refused) {
        let main = graph::EventsQuery { calendar: String::new(), from, to, series: expected("") };
        let days = |instant: i64| mailcraft_time::DateTime::from_timestamp(instant, 0).date.iso();
        match graph::events_in(&access, &main) {
            Ok(events) => {
                log::info!(
                    "calendar: the main calendar came down: {} events from {} to {} (all of them: {})",
                    events.events.len(),
                    days(from),
                    days(to),
                    events.complete
                );
                out.events = Some(events.for_account(&request.account));
            }
            Err(e) if e.is_refused_sign_in() => refused = true,
            Err(e) => out.errors.push(format!("Couldn't bring the calendar down: {e}")),
        }
        for calendar in others.iter().take(MAX_OTHER_CALENDARS) {
            if refused {
                break;
            }
            let query = graph::EventsQuery { calendar: calendar.clone(), from, to, series: expected(calendar) };
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
    if request.directory && !refused {
        match graph::contacts(&access) {
            Ok(contacts) => out.contacts = Some(contacts.for_account(&request.account)),
            Err(e) if e.is_refused_sign_in() => refused = true,
            Err(e) => out.errors.push(format!("Couldn't bring the contacts down: {e}")),
        }
    }
    // The mailbox's own categories and their colours: an extra that an older sign-in doesn't
    // cover, and nothing to trouble the owner with when it can't be had.
    if request.directory && !refused {
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

/// Most calendars of an account, beside its main one, that are brought down in one go.
const MAX_OTHER_CALENDARS: usize = 20;

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

/// Most message bytes a Send/Receive keeps back while the window is behind; past that, the run
/// waits for it.
const HELD_BYTES: usize = 256 * 1024 * 1024;

/// How the pieces of a Send/Receive get to the window: through a short channel, and while that is
/// full, put together here when they only bring news ([`SyncResult::can_absorb`]), up to
/// [`HELD_BYTES`]. The order things are taken in stays as the run did them.
struct Forward {
    tx: std::sync::mpsc::SyncSender<mailcraft_model::sync::SyncResult>,
    held: Option<mailcraft_model::sync::SyncResult>,
}

impl Forward {
    fn new(tx: std::sync::mpsc::SyncSender<mailcraft_model::sync::SyncResult>) -> Forward {
        Forward { tx, held: None }
    }

    /// Pass a piece on. Returns `false` when nobody is listening any more.
    fn send(&mut self, piece: mailcraft_model::sync::SyncResult) -> bool {
        use std::sync::mpsc::TrySendError;
        let piece = match self.held.take() {
            Some(mut held) if held.can_absorb(&piece) => {
                held.absorb(piece);
                held
            }
            // What was held goes first, waiting for room if it must.
            Some(held) => {
                if self.tx.send(held).is_err() {
                    return false;
                }
                piece
            }
            None => piece,
        };
        // The last piece always gets there.
        if !piece.partial {
            return self.tx.send(piece).is_ok();
        }
        match self.tx.try_send(piece) {
            Ok(()) => true,
            Err(TrySendError::Disconnected(_)) => false,
            Err(TrySendError::Full(piece)) if held_bytes(&piece) > HELD_BYTES => self.tx.send(piece).is_ok(),
            Err(TrySendError::Full(piece)) => {
                self.held = Some(piece);
                true
            }
        }
    }

    /// Pass on what is still held (the run ended without a last piece to carry it).
    fn flush(&mut self) {
        if let Some(held) = self.held.take() {
            let _ = self.tx.send(held);
        }
    }
}

/// About how many bytes the messages of a piece take.
fn held_bytes(piece: &mailcraft_model::sync::SyncResult) -> usize {
    piece
        .folders
        .iter()
        .flat_map(|f| &f.new)
        .map(|m| {
            let attached: usize = m.attachments.iter().map(|a| a.data.len()).fold(0, usize::saturating_add);
            m.body.text.len().saturating_add(m.body.html.as_ref().map_or(0, String::len)).saturating_add(attached)
        })
        .fold(0, usize::saturating_add)
}

/// What the app needs from the system: native file dialogs, shown without blocking the window
/// (`file_dialogs`); everything that opens a socket (Send/Receive, signing in, finding a domain's
/// mail servers, checking an account), each on a background thread; and the credential store.
/// `kept_pictures` is where downloaded pictures are kept between runs, when they are.
fn services(file_dialog: file_dialogs::Hook, ctx: egui::Context, kept_pictures: Option<std::sync::Arc<picture_cache::Cache>>) -> Services {
    let pictures_ctx = ctx.clone();
    let pictures_kept = kept_pictures.clone();
    let busy_ctx = ctx.clone();
    let check_ctx = ctx.clone();
    let ews_ctx = ctx.clone();
    let approval_ctx = ctx.clone();
    let idle_ctx = ctx.clone();
    let (sync_ctx, sign_in_ctx, graph_ctx, mx_ctx, test_ctx) = (ctx.clone(), ctx.clone(), ctx.clone(), ctx.clone(), ctx);
    let sync_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel = sync_cancel.clone();
    let sync_updates: std::sync::Arc<std::sync::Mutex<Option<mailcraft_model::sync::SyncRequest>>> = Default::default();
    let updates = sync_updates.clone();
    Services {
        file_dialog: Some(file_dialog),
        // Mail arrives in pieces as the run goes (what was sent, then folder by folder), through a
        // short channel. While the window is behind (busy, or not drawn because it is out of
        // sight), the pieces that only bring news are put together here instead of holding the
        // download up ([`Forward`]); only so much is kept that way. Changes the owner makes while
        // it runs come through `sync_updates` and go up on the run's own connection.
        sync: Some(Box::new(move |request| {
            use std::sync::atomic::Ordering;
            let (tx, rx) = std::sync::mpsc::sync_channel(8);
            let (wake, cancel, updates) = (sync_ctx.clone(), cancel.clone(), updates.clone());
            cancel.store(false, Ordering::Relaxed);
            let spawned = std::thread::Builder::new().name("send/receive".into()).spawn(move || {
                let mut forward = Forward::new(tx);
                let mut more = || updates.lock().ok().and_then(|mut waiting| waiting.take());
                mailcraft_net::sync_with_updates(
                    &request,
                    &cancel,
                    &mut |piece| {
                        // Nobody is listening any more (the window closed): stop.
                        if !forward.send(piece) {
                            cancel.store(true, Ordering::Relaxed);
                        }
                        wake.request_repaint();
                    },
                    &mut more,
                );
                forward.flush();
                wake.request_repaint();
            });
            // A dropped sender reads as "stopped unexpectedly" in the app.
            if let Err(e) = spawned {
                log::error!("couldn't start Send/Receive: {e}");
            }
            rx
        })),
        sync_cancel,
        sync_updates: Some(sync_updates),
        // New mail heard of as it comes: one more connection waits on the Inbox of an account
        // whose server can say so (IMAP IDLE), on a thread of its own, until the watch is dropped
        // (the account goes, the app quits). What it hears wakes the window, which starts a run.
        idle: Some(Box::new(move |request| inbox_watch(&idle_ctx, request))),
        sign_in: Some(Box::new(move |request| {
            let ctx = sign_in_ctx.clone();
            background(&sign_in_ctx, "sign-in", move || sign_in(&ctx, request))
        })),
        graph: Some(Box::new(move |request| background(&graph_ctx, "microsoft graph", move || graph(request)))),
        lookup_mx: Some(Box::new(move |domain| {
            background(&mx_ctx, "mail exchangers", move || mailcraft_net::lookup_mx(&domain).map_err(|e| e.to_string()))
        })),
        test_account: Some(Box::new(move |account, secret| {
            background(&test_ctx, "account check", move || mailcraft_net::test_account(&account, &secret).map_err(|e| e.to_string()))
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
        picture_cache: kept_pictures.map(|kept| Box::new(picture_cache::Kept(kept)) as Box<dyn mailcraft_ui_egui::PictureCache>),
        // When the people of a meeting are free and busy, while its Scheduling Assistant is open.
        free_busy: Some(Box::new(move |request| background(&busy_ctx, "free and busy times", move || free_busy(request)))),
        // A check of the calendar connection on the account's real server, when the owner asks.
        check_calendar: Some(Box::new(move |request| background(&check_ctx, "calendar check", move || check_calendar(request)))),
        // Outlook's Favorites through Exchange Web Services, for an account whose owner chose that.
        ews: Some(Box::new(move |request| background(&ews_ctx, "exchange web services", move || ews(request)))),
        // The address an administrator approves this application at: made here, for the owner to
        // pass on in a message.
        admin_consent_link: Some(Box::new(|oauth, tenant, source| mailcraft_net::oauth::admin_consent_url(oauth, tenant, source).ok())),
        // And the same page opened from here, for an owner who is an administrator and said so.
        admin_consent: Some(Box::new(move |request| {
            let ctx = approval_ctx.clone();
            background(&approval_ctx, "admin consent", move || admin_consent(&ctx, request))
        })),
        secrets: secrets::store(),
        // New mail and reminders, said by the system while the window is elsewhere.
        notify: Some(notifications::hook()),
        // File › Print: the PDF goes to the system's printing.
        print: Some(printing::hook()),
        microsoft_client_id: microsoft_client_id(),
        ..Default::default()
    }
}

/// Wait on the Inbox of `request`'s account for news (`mailcraft_net::idle`), on a thread of its
/// own, until the watch that comes back is dropped. Each piece of news goes through the watch's
/// channel and wakes the window; so does the end of the watch, with why it ended.
fn inbox_watch(ctx: &egui::Context, request: mailcraft_model::sync::SyncRequest) -> mailcraft_ui_egui::inbox_watch::InboxWatch {
    use mailcraft_ui_egui::inbox_watch::{InboxWatch, WatchNews};
    let (tx, rx) = std::sync::mpsc::channel();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (stopping, wake) = (stop.clone(), ctx.clone());
    let spawned = std::thread::Builder::new().name("new mail".into()).spawn(move || {
        let end = mailcraft_net::idle::watch_inbox(&request, &stopping, &mut || {
            let _ = tx.send(WatchNews::Changed);
            wake.request_repaint();
        });
        let news = match end {
            mailcraft_net::idle::IdleEnd::Stopped => return,
            mailcraft_net::idle::IdleEnd::NotOffered => WatchNews::NotOffered,
            mailcraft_net::idle::IdleEnd::SignIn(_) => WatchNews::SignIn,
            mailcraft_net::idle::IdleEnd::Unusable(why) => WatchNews::Ended(why),
        };
        let _ = tx.send(news);
        wake.request_repaint();
    });
    // A watch that never began reads as one that ended: the timed runs bring the mail.
    if let Err(e) = spawned {
        log::warn!("couldn't wait for new mail: {e}");
    }
    InboxWatch { news: rx, stop }
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

/// The mailbox to start with, the file it is kept in (none for the sample, or without prefs) and
/// how many bodies and attachments couldn't be read back from the content folder beside it.
fn first_mailbox(sample: bool, chosen: Option<PathBuf>) -> (Mailbox, Option<PathBuf>, usize) {
    let now = mailcraft_time::now();
    if sample {
        return (mailcraft_engine::sample::mailbox(now, mailcraft_time::local_offset_minutes(now)), None, 0);
    }
    let ours = chosen.is_none();
    let path = chosen.or_else(|| default_mailbox_path().filter(|_| prefs_enabled()));
    let Some(path) = path else { return (Mailbox::with_local_account("", ""), None, 0) };
    if !path.exists() {
        // A first start: the settings folder has to be there for the mailbox to be saved in it.
        if ours && let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        return (Mailbox::with_local_account("", ""), Some(path), 0);
    }
    match mailcraft_engine::store::open(&path) {
        Ok(opened) => (opened.mailbox, Some(path), opened.unavailable),
        Err(e) => {
            // Never write over a mailbox that couldn't be read: start empty, keep nothing, say why.
            log::error!("{e}");
            (Mailbox::with_local_account("", ""), None, 0)
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
    let mut control_port: Option<u16> = std::env::var("MAILCRAFT_CONTROL_PORT").ok().and_then(|p| p.parse().ok());
    let mut files = Vec::new();
    let mut sample = false;
    let mut mailbox_path: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--control" => control_port = args.next().and_then(|p| p.parse().ok()),
            "--sample" => sample = true,
            "--mailbox" => mailbox_path = args.next().map(PathBuf::from),
            "--version" => {
                println!("mailcraft {} ({})", env!("CARGO_PKG_VERSION"), build_sha());
                return Ok(());
            }
            _ => files.push(a),
        }
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
    eframe::run_native(
        "MailCraft",
        options,
        Box::new(move |cc| {
            let (mailbox, path, unavailable) = first_mailbox(sample, mailbox_path);
            let (file_dialog, dialogs) = file_dialogs::hook(&cc.egui_ctx);
            let mut session = Session::new(mailbox);
            let kept = path.is_some();
            session.path = path;
            // A run that keeps its mailbox and its settings keeps downloaded pictures too.
            let kept_pictures = if kept && !sample && prefs_enabled() { keep_pictures() } else { None };
            let mut app = MailApp::new(session, services(file_dialog, cc.egui_ctx.clone(), kept_pictures));
            // Nowhere to keep the mailbox (the sample, a run without preferences): don't try.
            app.autosave = kept;
            if let Some(path) = &app.session.path {
                app.services.saver = background_saver(&cc.egui_ctx, path);
            }
            if unavailable > 0 {
                app.status(format!("{unavailable} bodies or attachments are missing from the mailbox's content folder, or damaged."));
            }
            // An ordinary start on a mailbox nobody has set up yet begins with adding an account.
            app.offer_setup = kept && !sample;
            load_prefs(&mut app);
            app.ui.window = restored;
            app.integrated_titlebar = cfg!(target_os = "macos");
            let mut key_file = None;
            if let Some(port) = control_port
                && let Some((rx, file)) = control_server::start(port, cc.egui_ctx.clone(), mailcraft_control_key::settings_dir().as_deref())
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
            Ok(Box::new(App(
                app,
                restored,
                key_file,
                dialogs,
                guard,
                #[cfg(target_os = "macos")]
                apple_events.connect(&cc.egui_ctx),
            )))
        }),
    )
}
