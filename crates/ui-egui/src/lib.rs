// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's egui front end: an Outlook-style window (title bar with Quick Access Toolbar and
//! search, ribbon, module bar, folder pane, message list, reading pane, calendar, people, tasks,
//! status bar, Backstage, dialogs) over `mailcraft-engine`.
//!
//! The UI is thin: it reads `Session` state and acts through [`MailApp::run`] (command ids), so
//! the ribbon, shortcuts, command search, control channel and MCP all reach the same behaviour.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// An English UI string in the current interface language ([`i18n::t`]).
#[macro_export]
macro_rules! tl {
    ($s:expr) => {
        $crate::i18n::t($s)
    };
}

/// An English UI string in a context, in the current interface language: `tlc!("folder",
/// "Archive")` is the folder where `tl!("Archive")` is the command ([`i18n::tc`]). Both are
/// literals, so `cargo xtask i18n` finds them.
#[macro_export]
macro_rules! tlc {
    ($context:expr, $s:expr) => {
        $crate::i18n::tc($context, $s)
    };
}

/// A count in the current interface language: `tln!(n, "{n} message", "{n} messages")` picks the
/// form the language's plural rule gives `n` and fills in `{n}` ([`i18n::tn`]). Both English forms
/// are literals, so `cargo xtask i18n` finds them.
#[macro_export]
macro_rules! tln {
    ($n:expr, $one:expr, $other:expr) => {
        $crate::i18n::tn($crate::i18n::Count::count($n), $one, $other)
    };
}

mod account_setup;
mod advanced_search;
pub mod agent_gate;
#[cfg(test)]
mod agent_leaves_tests;
#[cfg(test)]
mod agent_names_tests;
pub mod alerts;
pub mod approval;
mod attachment_open;
pub mod backstage;
mod calendar_sync;
pub mod calendar_view;
pub mod chrome;
mod clicks;
#[cfg(test)]
mod clicks_tests;
pub mod compose;
#[cfg(test)]
mod confirm_ids_tests;
pub mod control;
pub mod credits;
pub mod data_removal;
pub mod date_picker;
pub mod dates;
#[cfg(test)]
mod dialog_scrim_tests;
pub mod dialogs;
#[cfg(test)]
mod draft_continue_tests;
mod drag_pointer;
#[cfg(test)]
mod drag_pointer_tests;
#[cfg(test)]
mod ews_info_tests;
pub mod ews_mail;
pub mod ews_setup;
pub mod favorites_sync;
#[cfg(test)]
mod field_drag_tests;
pub mod files;
mod find_replace;
mod folder_properties;
pub mod forms;
#[cfg(test)]
#[path = "forms_date_tests.rs"]
mod forms_date_tests;
pub mod graph_fetch;
mod graph_switch;
pub mod i18n;
pub mod icons;
pub mod import_tree;
mod inbox_pass;
pub mod jmap_mail;
mod keep_offline;
#[cfg(test)]
mod key_chain_tests;
pub mod keys;
#[cfg(test)]
#[path = "late_work_tests.rs"]
mod late_work_tests;
#[cfg(test)]
mod leave_message_tests;
#[cfg(test)]
mod list_keys_tests;
pub mod mail_view;
mod meeting_dialogs;
pub mod menubar;
mod merge_contacts;
pub mod nav;
#[cfg(test)]
mod on_top_tests;
mod open_shared;
#[cfg(test)]
mod open_shared_tests;
mod opened_message;
#[cfg(test)]
mod opened_message_tests;
pub mod opening;
pub mod people_view;
pub mod reading;
mod reminders;
mod report_problem;
pub mod ribbon;
pub mod scheduling;
mod search_pills;
#[cfg(test)]
mod search_pills_tests;
mod search_suggest;
mod secret_field;
#[cfg(test)]
mod secret_field_tests;
mod secret_read;
#[cfg(test)]
#[path = "secret_read_tests.rs"]
mod secret_read_tests;
#[cfg(test)]
#[path = "secrets_bound_tests.rs"]
mod secrets_bound_tests;
#[cfg(test)]
#[path = "secrets_replaced_tests.rs"]
mod secrets_replaced_tests;
mod sender_images;
pub mod server_search;
pub mod settings_notice;
mod shared_calendars;
#[cfg(test)]
mod shared_calendars_tests;
mod shared_mailboxes;
mod signatures;
mod sync_panel;
pub mod task_sync;
#[cfg(test)]
mod task_sync_tests;
pub mod tasks_view;
mod text_menu;
#[cfg(test)]
mod text_menu_gesture_tests;
#[cfg(test)]
mod text_menu_tests;
pub mod theme;
pub mod unconfirmed;
pub mod updates;
pub mod widgets;
pub mod window_geometry;
pub mod windows;
#[cfg(test)]
mod writing_tools_tests;
mod zones;

use std::sync::mpsc::Receiver;

use mailcraft_engine::Session;
use mailcraft_model::Id;
use mailcraft_model::sync::{SyncPhase, SyncRequest, SyncResult, Tokens};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use approval::{AdminConsentAnswer, AdminConsentRequest};
pub use control::ControlRequest;
pub use files::FileDialogRequest;
pub use scheduling::{FreeBusyRequest, FreeBusyResult};
pub use settings_notice::SettingsUnread;
pub use unconfirmed::ConfirmedMailbox;

/// Platform services injected by the host (file dialogs, downloads, servers, storage).
#[derive(Default)]
pub struct Services {
    /// Desktop: the icon and application id of the main window, which every other window of the
    /// system (reminders, messages) takes too.
    pub window_identity: Option<windows::WindowIdentity>,
    /// Desktop: show a native file dialog without blocking the UI thread. The host sends the
    /// picked path (`None` when cancelled) through the returned channel and wakes the UI; a
    /// dropped sender counts as cancelled. The app shows one dialog at a time ([`files`]).
    pub file_dialog: Option<Box<dyn Fn(FileDialogRequest) -> Receiver<Option<String>>>>,
    /// Web: open a file picker; the file arrives later through `inbox`. The argument says what
    /// for (`mailbox`, `import`, `attach`).
    pub open_async: Option<Box<dyn Fn(&str)>>,
    /// Files (name, bytes) delivered asynchronously (the web picker, drag and drop).
    pub inbox: Option<Inbox>,
    /// Desktop: open an attachment the owner asked to open (`mail.openAttachment`) with the
    /// system's own program for its kind: the host writes it, under the name given (already
    /// made safe, and made safe again there), into a folder of MailCraft's own that only this
    /// user can read and that is emptied when MailCraft quits, and asks the system to open it.
    /// Only ever called for the owner's own double-click or Open, after the window has refused
    /// programs and scripts and asked about macros. An error means nothing was opened. Absent in
    /// the browser (the file is downloaded instead) and the tests.
    pub open_file: Option<Box<dyn Fn(&str, &[u8]) -> Result<(), String>>>,
    /// Web: hand bytes to the browser as a download. An error means no download started.
    pub download: Option<Box<dyn Fn(&str, &[u8]) -> Result<(), String>>>,
    /// Web: told whether there are unsaved changes after each pass, for the leave-page guard.
    pub on_dirty: Option<Box<dyn Fn(bool)>>,
    /// Desktop: synchronise with a mail server on a background thread. What it brings arrives
    /// through the channel in pieces as it goes (`SyncResult::partial` on all but the last), and
    /// the host wakes the UI for each. Absent in the browser, which can't open sockets.
    pub sync: Option<Box<dyn Fn(SyncRequest) -> Receiver<SyncResult>>>,
    /// Set to call a Send/Receive off: the host's run stops at the next safe place and says so
    /// in its last piece. The host clears it when it starts a run.
    pub sync_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Desktop: sign an account's owner in on the provider's own page (OAuth 2.0) on a background
    /// thread: renew quietly with the refresh token when there is one, else open the browser and
    /// wait. The tokens, or why there are none, arrive through the channel and the host wakes the
    /// UI. Absent in the browser.
    pub sign_in: Option<Box<dyn Fn(SignInRequest) -> Receiver<Result<Tokens, String>>>>,
    /// Desktop: what Microsoft Graph does for an account that signs in with Microsoft, on a
    /// background thread: send the Outbox (no "authenticated SMTP" switch stands in its way) and
    /// bring the calendar and the contacts down. Absent in the browser.
    pub graph: Option<Box<dyn Fn(GraphRequest) -> Receiver<GraphResult>>>,
    /// Desktop: fetch messages of a Microsoft account whole on a background thread (R3.2).
    pub graph_fetch: Option<Box<dyn Fn(graph_fetch::GraphFetchJob) -> Receiver<mailcraft_model::sync::GraphFetchResult>>>,
    /// Desktop: search a Microsoft mailbox on its server, one page, on a background thread
    /// (R6.2: More results on the server, only on the owner's click).
    pub graph_search: Option<Box<dyn Fn(server_search::GraphSearchJob) -> Receiver<mailcraft_model::sync::GraphSearchResult>>>,
    /// Keep a Microsoft account's tasks in step with Microsoft To Do (#248; [`task_sync`]).
    pub graph_tasks: Option<Box<dyn Fn(task_sync::TasksJob) -> Receiver<task_sync::TasksResult>>>,
    /// A host that reaches JMAP servers (over `fetch` in the browser): read an account kept in step
    /// over JMAP (M6) as the job says, as a task of its own: connect to the account's server with
    /// the job's secret, read what the request asks for (`mailcraft_jmap::sync_mail`) and hand the
    /// run back through the channel in pieces as it goes (`JmapMailResult::partial` on all but the
    /// last), waking the UI for each. Absent where no host does (the desktop app has no runner for
    /// it yet): a JMAP account then has no Send/Receive ([`jmap_mail`]).
    pub jmap: Option<Box<dyn Fn(jmap_mail::JmapJob) -> Receiver<mailcraft_model::sync::JmapMailResult>>>,
    /// The same host: fetch messages of a JMAP account whole, as `graph_fetch` does for a Microsoft
    /// account (`mailcraft_jmap::fetch::fetch_messages`).
    pub jmap_fetch: Option<Box<dyn Fn(jmap_mail::JmapFetchJob) -> Receiver<mailcraft_model::sync::GraphFetchResult>>>,
    /// Desktop: read an Exchange server account (#252, E6) as the job says, on a background thread:
    /// sign in to the account's server with the job's password (Basic over TLS, the certificate
    /// trusted as the request says), read what the request asks for (`mailcraft_ews::sync_mail`) and
    /// hand the run back through the channel in pieces (`EwsMailResult::partial` on all but the
    /// last), waking the UI for each. Absent where no host does (the browser): an Exchange server
    /// account then has no Send/Receive ([`ews_mail`]).
    pub ews: Option<Box<dyn Fn(ews_mail::EwsJob) -> Receiver<mailcraft_model::sync::EwsMailResult>>>,
    /// The same host: fetch messages of an Exchange server account whole
    /// (`mailcraft_ews::fetch_messages`), as `graph_fetch` does for a Microsoft account.
    pub ews_fetch: Option<Box<dyn Fn(ews_mail::EwsFetchJob) -> Receiver<mailcraft_model::sync::GraphFetchResult>>>,
    /// Desktop: set an Exchange server account up on a background thread (#252, E7), once the owner
    /// has pressed Connect and not before: find the server unless its address is given, ask the
    /// owner (through the job's answers) before the sign-in goes to a host outside the domain, and
    /// test the account with one request for the Inbox ([`ews_setup`]). Absent where no host does
    /// (the browser): Add Account then says it can't connect.
    pub ews_setup: Option<Box<dyn Fn(ews_setup::EwsSetupJob) -> Receiver<ews_setup::EwsSetupEvent>>>,
    /// Whether the host holds the credential of this JMAP account itself (a page that signed the
    /// owner in to its server): for such an account the window never asks for a password, and its
    /// jobs go without one. Absent, or false for an account: the owner types it.
    pub jmap_host_credentials: Option<Box<dyn Fn(&mailcraft_model::Account) -> bool>>,
    /// Desktop: ask the DNS for a domain's mail exchangers on a background thread (best first),
    /// so account setup can tell whose servers an address at a company's own domain uses. Only
    /// the domain is given, never the mailbox name.
    pub lookup_mx: Option<Box<dyn Fn(String) -> Receiver<Result<Vec<String>, String>>>>,
    /// Desktop: try signing in to an account's servers with a secret (a password, or an access
    /// token for an account that signs in on its provider's page) on a background thread, before
    /// the account is added.
    pub test_account: Option<Box<dyn Fn(mailcraft_model::Account, String) -> Receiver<Result<(), String>>>>,
    /// Desktop: the operating system's credential store (Windows Credential Manager, the macOS
    /// keychain, the Secret Service on Linux and FreeBSD), where a password or a refresh token is
    /// remembered between runs when the owner wants that. Never the mailbox file, the settings or
    /// a log.
    pub secrets: Option<std::sync::Arc<dyn SecretStore>>,
    /// The application (client) id this build presents to Microsoft when neither the account nor
    /// the settings name one; empty when the build has none.
    pub microsoft_client_id: String,
    /// Keep the mailbox where the host stores it (the browser's storage); called a moment after
    /// each change. Desktop saves to the mailbox file instead (see `saver`).
    pub persist: Option<Box<dyn Fn(&[u8]) -> Result<(), String>>>,
    /// Desktop: write the mailbox file on a background thread, so the window never waits for the
    /// disk. Without one (tests, scripts) the mailbox is saved in place through `file.save`.
    pub saver: Option<Box<dyn mailcraft_engine::io::BackgroundSave>>,
    /// Desktop: write a copy of the mailbox to a file (as one file that holds everything) on a
    /// background thread: the copy kept before an account switches to Microsoft Graph, which can
    /// take a while for a large mailbox. Without one (tests, scripts) the copy is written in
    /// place by `account.graphSwitch`.
    pub write_copy: Option<Box<dyn Fn(std::path::PathBuf, mailcraft_model::Mailbox) -> Receiver<Result<(), String>>>>,
    /// Desktop: walk, read and parse a folder of mbox files on a background thread
    /// (`mailcraft_engine::import_tree::load_path`), so that neither the look at it nor the
    /// import holds the window up (#420). The job carries the folder and a shared progress the
    /// window shows and cancels through. Without one (tests, scripts) the window does it in place.
    pub import_tree: Option<Box<dyn Fn(import_tree::TreeJob) -> Receiver<Result<mailcraft_engine::import_tree::Parsed, String>>>>,
    /// Desktop: read a mailbox file on a background thread ahead of its taking the place of the
    /// one open (File › Open, a mailbox file dropped or handed over; [`opening`]), counting what
    /// it reads in the progress it is given and stopping when that is cancelled
    /// (`mailcraft_engine::store::open_ahead`). Whether it could be read arrives through the
    /// channel and the host wakes the UI; `file.open` then takes what was read. Absent in the
    /// browser and the tests: `file.open` reads the file there and then.
    pub read_ahead: Option<Box<dyn Fn(std::path::PathBuf, std::sync::Arc<mailcraft_engine::io::OpenProgress>) -> Receiver<Result<(), String>>>>,
    /// Desktop: download one picture a message refers to on the web, on a background thread,
    /// once the owner has asked for that message's pictures ([`reading::remote`]). The address
    /// is the message's own choice: the host checks where it leads before going there, takes
    /// only so much and waits only so long (`mailcraft_net::pictures::fetch`). The bytes, or why
    /// there are none, arrive through the channel and the host wakes the UI. Absent in the
    /// browser and the tests: pictures on the web then stay placeholders, and nothing is offered.
    pub pictures: Option<Box<dyn Fn(String) -> Receiver<Result<Vec<u8>, String>>>>,
    /// Desktop: the pictures the host keeps on disk between runs, so that a picture the owner
    /// asked for once is not asked of its server again at every start ([`PictureCache`]). The
    /// host looks there before it downloads, and what it finds comes through `pictures` like
    /// any download; this is only for saying how much is kept and for forgetting it. Absent
    /// where nothing is kept (the browser, the tests, the sample mailbox).
    pub picture_cache: Option<Box<dyn PictureCache>>,
    /// Desktop: the picture of a sender, by their address, on a background thread, once the
    /// owner turned sender images on ([`sender_images`]): what Gravatar has for the address,
    /// else the icon favicon.im (or else Google) has for its domain, downloaded and kept like
    /// the pictures in mail. The bytes, or why there are none, arrive through the channel.
    /// Once the token can't be upgraded (sender images turned off, the pictures cleared) the
    /// window no longer wants it, and the host asks nothing more for it.
    /// Absent in the browser and the tests: senders keep their initials.
    pub sender_images: Option<Box<dyn Fn(String, std::sync::Weak<()>) -> Receiver<Result<Vec<u8>, String>>>>,
    /// Desktop: remove everything MailCraft keeps on this computer, in the folders the host
    /// itself works in ([`data_removal::DataWipe`], File › Info › Remove MailCraft's data from
    /// this computer, #177). Absent in the browser and the tests.
    pub data_wipe: Option<Box<dyn data_removal::DataWipe>>,
    /// Desktop: the settings folder, and the host's way of showing a folder in the system's file
    /// manager ([`data_removal::DataFolder`], File › Info › Open MailCraft's data folder, #460):
    /// the window shows the open mailbox's own folder instead when it is kept elsewhere. Absent in
    /// the browser, the tests and a run that keeps nothing.
    pub data_folder: Option<data_removal::DataFolder>,
    /// Desktop: ask an account's server when people are free and busy, on a background thread,
    /// for a meeting's Find a Time page ([`scheduling`]). It reads and writes nothing. The
    /// answer, or why there is none, arrives through the channel and the host wakes the UI.
    /// Absent in the browser and for accounts whose server has no calendar: the assistant then
    /// shows the owner's own calendar only, and says so.
    pub free_busy: Option<Box<dyn Fn(FreeBusyRequest) -> Receiver<FreeBusyResult>>>,
    /// Desktop: check an account's calendar connection on a background thread, when the owner
    /// asks for it and at no other time (File › Info › Check Calendar Connection,
    /// [`calendar_sync`]): one appointment is made in the account's main calendar a year ahead,
    /// read back, changed, read back again and deleted, and every step comes back with what the
    /// server answered. Nothing else of the calendar is touched. Absent in the browser.
    pub check_calendar: Option<Box<dyn Fn(CalendarCheckRequest) -> Receiver<CalendarCheckResult>>>,
    /// Desktop: the address an organisation's administrator opens to approve this program, once
    /// for everybody, for everything it asks an owner for (Microsoft's admin-consent page): for
    /// the sign-in settings (with the application id filled in), the directory when it is known
    /// (else empty). It names the application, the directory and the permissions and nothing
    /// else; `None` when there is no such address. Nothing is fetched.
    pub admin_consent_link: Option<Box<dyn Fn(&mailcraft_model::OAuth, &str) -> Option<String>>>,
    /// Desktop: have an administrator approve this program for an organisation, on a background
    /// thread: open that same page of Microsoft's in the system's browser, with a way back to
    /// this machine, and wait for its yes or no ([`approval`]). Only ever on the owner's say-so
    /// ("I'm an administrator"). The answer arrives through the channel and the host wakes the
    /// UI. Absent in the browser.
    pub admin_consent: Option<Box<dyn Fn(AdminConsentRequest) -> Receiver<AdminConsentAnswer>>>,
    /// Desktop: the system's clipboard for what egui's can't carry, HTML: the body of a message
    /// being written copies its selection as HTML and text, and pastes HTML when there is some;
    /// and the text Paste in a text's menu takes (`text_menu`). Absent in the browser and the
    /// tests: copying and pasting are then text, and a text's menu has no Paste to offer.
    pub clipboard: Option<Box<dyn RichClipboard>>,
    /// Desktop: what Help › Copy Diagnostics puts on the clipboard (`ui.copyDiagnostics`): the
    /// version, the platform and the end of the logs with addresses, names, paths, hosts, links,
    /// ids and quoted text taken out. Never sent anywhere. Absent in the browser and the tests:
    /// the copy then has the version and the platform only.
    pub diagnostics: Option<Box<dyn Fn() -> String>>,
    /// Desktop: ask, on a background thread, which is the newest release of MailCraft
    /// ([`updates`]): GitHub alone is asked, about this build's repository, and is told nothing of
    /// the owner (`mailcraft_net::update::latest`). The argument says whether a pre-release will
    /// do (the running build is one). The release, or why there is none in words that name no
    /// address, arrives through the channel and the host wakes the UI. Nothing is downloaded or
    /// installed. Absent in the browser and the tests: nothing is asked and nothing is offered.
    pub update_check: Option<Box<dyn Fn(bool) -> Receiver<Result<mailcraft_model::update::Release, String>>>>,
    /// A development build (not one of the project's packages): it never asks about the daily
    /// check for new versions and never checks by itself; Check for Updates still works.
    pub development_build: bool,
    /// Desktop, for a copy that updates itself (the Windows `.msi`, the macOS app, a Linux
    /// AppImage): download, on a background thread, this copy's file of a newer release into
    /// the data folder, and check it by the release's own signature ([`updates`];
    /// `mailcraft_net::update_download`). Where the file is, or why there is none in words that
    /// name no address, arrives through the channel. Nothing is installed. Absent for every
    /// other copy, in the browser and in the tests: the owner is told of new versions, and that
    /// is all.
    pub update_download: Option<Box<dyn Fn(mailcraft_model::update::Release) -> Receiver<Result<std::path::PathBuf, String>>>>,
    /// The organisation has turned updating MailCraft by itself off for every user of the
    /// computer (a policy in the registry on Windows, a file on macOS and Linux): Install
    /// updates automatically is off and can't be turned on, and only the notice remains.
    pub updates_turned_off_by_organisation: bool,
    /// Desktop, for a copy that updates itself: what the host found at this start about the last
    /// update it installed ([`updates::UpdateStart`]): one that wasn't installed after all, or a
    /// newly installed version that didn't start twice running, with the one it replaced kept.
    pub update_start: updates::UpdateStart,
    /// Desktop: the system's notifications, for a reminder falling due and for new mail while the
    /// window isn't in front ([`alerts`]): the host shows what it is handed, says why it can't
    /// when it can't, and hands back which were clicked. Only the window asks for one, never a
    /// command. Absent in the browser and the tests: nothing is shown.
    pub alert: Option<Box<dyn alerts::AlertHost>>,
    /// The system's locale (`fr-FR`; the browser's in the browser build), which the interface
    /// language follows while it is Automatic. `None` (the tests, scripts): English.
    pub system_locale: Option<String>,
    /// Web: told the interface language's code (`en`) whenever it changes, for the page's
    /// `<html lang>`.
    pub on_language: Option<Box<dyn Fn(&str)>>,
}

impl Services {
    /// Whether this build reaches mail servers at all: with mail connections, over JMAP, or through Exchange Web Services.
    pub fn can_sync(&self) -> bool {
        self.sync.is_some() || self.jmap.is_some() || self.ews.is_some()
    }
}

/// The system's clipboard with HTML on it ([`Services::clipboard`]).
pub trait RichClipboard {
    /// Put `html` on the clipboard with `text` beside it for programs that take text.
    fn put(&self, html: &str, text: &str) -> Result<(), String>;
    /// The HTML on the clipboard, when what is on it has HTML.
    fn html(&self) -> Option<String>;
    /// The picture on the clipboard as PNG, when what is on it is a picture.
    fn image(&self) -> Option<Vec<u8>> {
        None
    }
    /// The text on the clipboard, all of it, for Paste in a text's menu (`text_menu`): a key press
    /// has the system read it, a menu item has nothing to. The menu then does with it what Ctrl+V
    /// does with what the system read.
    fn text(&self) -> Option<String> {
        None
    }
}

/// The largest side of a picture taken from the clipboard, in pixels.
pub const MAX_PICTURE_SIDE: usize = 4096;

/// A picture from the clipboard, RGBA pixels `width` by `height`, as PNG: what the clipboard's
/// picture is when it is pasted, by a text's menu (`RichClipboard::image`) or by Ctrl+V
/// (`egui::Event::PasteImage`). Nothing when it is no picture of a size that is taken.
pub fn clipboard_picture_png(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    if width == 0
        || height == 0
        || width > MAX_PICTURE_SIDE
        || height > MAX_PICTURE_SIDE
        || rgba.len() != width.checked_mul(height)?.checked_mul(4)?
    {
        return None;
    }
    let mut out = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut out);
    image::ImageEncoder::write_image(encoder, rgba, u32::try_from(width).ok()?, u32::try_from(height).ok()?, image::ExtendedColorType::Rgba8).ok()?;
    Some(out)
}

/// Files delivered asynchronously.
pub type Inbox = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// The operating system's credential store ([`Services::secrets`]). Keys are `password:<address>`
/// and `refresh:<address>`.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, key: &str);
    /// Why the store itself couldn't be used the last time something was asked of it (none is
    /// running, or it is locked), as opposed to there being nothing kept under a key. Taking it
    /// clears it. (`get` may wait for the owner to unlock the store: the window never calls it
    /// itself, only a background thread does, [`secret_read`].)
    fn take_trouble(&self) -> Option<StoreTrouble> {
        None
    }
}

/// Why the system's credential store couldn't keep or give back a sign-in
/// ([`SecretStore::take_trouble`]). Then nothing is remembered between runs: the owner is asked
/// at each start, as on a system without a store, and is told why once a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreTrouble {
    /// No store answered: on Linux, no Secret Service (GNOME Keyring, KWallet) is running.
    Unavailable,
    /// The store is locked, or unlocking it was refused.
    Locked,
}

impl StoreTrouble {
    /// What the owner is told: in the status bar once, and under the switch in File › Options.
    pub fn notice(self) -> String {
        let why = match self {
            StoreTrouble::Unavailable => tl!("No credential store answered (on Linux, GNOME Keyring or KWallet provides one)"),
            StoreTrouble::Locked => tl!("The system's credential store is locked, or unlocking it was refused"),
        };
        i18n::fmt(tl!("{why}, so sign-ins aren't remembered: MailCraft asks again each time it starts."), &[("why", why)])
    }
}

/// The pictures from the web that the host keeps on disk between runs
/// ([`Services::picture_cache`]). The window calls these itself, so they are quick: saying how
/// much is kept reads nothing, and forgetting takes as long as removing the files takes.
pub trait PictureCache {
    /// How many pictures are kept, and how many bytes they take.
    fn usage(&self) -> (usize, u64);
    /// Forget what is kept for one address: what came from there turned out to be no picture,
    /// or the owner wants it asked for afresh.
    fn forget(&self, url: &str);
    /// Forget them all.
    fn clear(&self) -> Result<(), String>;
}

/// What the host needs to sign an account's owner in ([`Services::sign_in`]). A sign-in on the
/// provider's page that ends without tokens in one of the ways the window has something of its
/// own to say about is answered with exactly [`approval::NEEDS_APPROVAL`],
/// [`approval::SIGN_IN_DECLINED`], [`approval::SIGN_IN_TIMED_OUT`] or [`approval::NO_BROWSER`]
/// as its error; any other error is shown as it is.
pub struct SignInRequest {
    /// The account, or 0 while one is being set up.
    pub account: Id,
    /// The account's sign-in settings; an empty client id means the host's own.
    pub oauth: mailcraft_model::OAuth,
    /// The address, offered to the provider's page so the owner needn't type it.
    pub email: String,
    /// A refresh token from earlier in this session to try first; empty when there is none.
    pub refresh: String,
    /// Set when the owner gives up waiting, for the host to stop too.
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Nobody asked for this (a Send/Receive that started by itself): renew with the refresh
    /// token or give up, but don't put a sign-in page in front of the owner.
    pub quiet: bool,
    /// Ask for what Microsoft Graph needs (sending, calendar, contacts) rather than for mail:
    /// the owner chose "Connect calendar and contacts".
    pub graph: bool,
    /// Ask for mail alone, without the calendar and contacts along: the owner's way out when the
    /// provider's page won't have the fuller request.
    pub mail_only: bool,
    /// With `graph`: ask also for what reading the Favorites of Outlook takes (the owner chose
    /// "Show Outlook's Favorites"). The tokens that come back are then good for that too. A no
    /// that is not the owner's to change is answered with exactly
    /// [`favorites_sync::NEEDS_APPROVAL`], [`favorites_sync::REFUSED_BY_ORGANISATION`] or
    /// [`favorites_sync::REFUSED_FOR_ACCOUNT`].
    pub favorites: bool,
    /// With `graph`: ask also for reading and changing mail (`Mail.ReadWrite`), for an account kept
    /// in step through Microsoft Graph (R3).
    pub mail_write: bool,
    /// With `graph`: ask for reading the shared mailboxes the owner was given Full Access to
    /// (`Mail.ReadWrite.Shared`, #70) as well, by itself: the owner opened one and it is to be
    /// read. Never with a sign-in of any other kind. A no that is not the owner's to change is
    /// answered as for the Favorites ([`favorites_sync::NEEDS_APPROVAL`]…).
    pub shared: bool,
    /// With `shared`: ask for sending from shared mailboxes (`Mail.Send.Shared`, #70) as well,
    /// by itself: the owner is sending from one for the first time, and said go ahead. Never
    /// with a sign-in of any other kind.
    pub shared_send: bool,
    /// Ask for reading other people's calendars (`Calendars.ReadWrite.Shared`, #70) as well, by
    /// itself, never with a sign-in of any other kind: with `shared`, the owner ticked a shared
    /// mailbox's calendar and said go ahead; with `graph` alone, the owner opened a calendar by
    /// address and it is to be read (Open Shared Calendar). One permission, one question.
    pub shared_calendars: bool,
    /// With `graph`: ask for reading and changing the owner's To Do lists and tasks
    /// (`Tasks.ReadWrite`, #248) by itself, never with a sign-in of any other kind: a job found
    /// the sign-in doesn't cover it, and the owner said go ahead.
    pub tasks: bool,
}

/// What the host needs to check an account's calendar connection ([`Services::check_calendar`]).
pub struct CalendarCheckRequest {
    pub account: mailcraft_model::Account,
    /// The account's sign-in settings, with the application id filled in.
    pub oauth: mailcraft_model::OAuth,
    /// The account's refresh token, to get a Graph token with when `access` is empty.
    pub refresh: String,
    /// A Graph access token from earlier in this session that is still good; else empty.
    pub access: String,
}

/// One step of a calendar connection check, as the host reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CalendarCheckStep {
    /// What was done ("Make an appointment", "Read it back"…).
    pub what: String,
    /// The server did it, and what it has is what was sent.
    pub ok: bool,
    /// The status of the server's answer; 0 when there was none.
    pub status: u16,
    /// What was sent and what the server has, or the server's own words for its refusal.
    pub said: String,
}

/// What a [`CalendarCheckRequest`] came to.
#[derive(Default)]
pub struct CalendarCheckResult {
    pub account: Id,
    /// A Graph token that was obtained for this (kept for the session).
    pub tokens: Option<Tokens>,
    /// The owner hasn't allowed this program to use the calendar.
    pub consent_needed: bool,
    /// The token that was handed in is no longer taken.
    pub token_refused: bool,
    /// The steps that were taken, in order.
    pub steps: Vec<CalendarCheckStep>,
    /// Every step went through, and every reading was what was sent.
    pub ok: bool,
    /// The appointment the check made is gone again (or none was made).
    pub cleaned_up: bool,
    /// Why the check couldn't start, when it couldn't.
    pub error: String,
}

/// What the host is asked to do with Microsoft Graph for one account ([`Services::graph`]).
pub struct GraphRequest {
    pub account: mailcraft_model::Account,
    /// The account's sign-in settings, with the application id filled in.
    pub oauth: mailcraft_model::OAuth,
    /// The account's refresh token, to get a Graph token with when `access` is empty.
    pub refresh: String,
    /// A Graph access token from earlier in this session that is still good; else empty.
    pub access: String,
    /// Mail waiting in the Outbox, to send through Graph.
    pub outbox: Vec<mailcraft_model::Message>,
    /// Also bring the calendar and the contacts down.
    pub directory: bool,
    /// A stretch of the calendar to bring down when `directory` isn't asked for (Unix seconds,
    /// from and up to): the days on screen, for a refresh that costs one question.
    pub calendar: Option<(i64, i64)>,
    /// What was made, changed or deleted in the calendar here, for the server to be told before
    /// the calendar is brought down.
    pub changes: Option<mailcraft_model::remote::EventsUpload>,
    /// The calendars of the account to bring down, each with the series the mailbox expects in
    /// the stretch (the account's main calendar is the one with no name; it comes anyway).
    pub asks: Vec<mailcraft_model::remote::CalendarAsk>,
    /// Also bring the working hours the mailbox has set: an extra that takes a permission of its
    /// own, without which the answer says [`GraphResult::extras_missing`].
    pub settings: bool,
    /// Also read the Favorites as Outlook has them ([`GraphResult::favorites`]).
    pub favorites: bool,
    /// A token that is obtained for this job is asked for with the permissions that reading the
    /// Favorites takes ([`GraphResult::favorites_covered`] says whether it has them). Without
    /// it, a token is asked for as it always was.
    pub favorites_scopes: bool,
    /// For an account kept in step through Microsoft Graph (R3): the folder changes to make and
    /// the folders to list ([`GraphResult::mail`]). A token obtained for this job is asked for
    /// with `Mail.ReadWrite`.
    pub mail: Option<mailcraft_model::sync::GraphMailRequest>,
    /// Where the pieces of that mail run go as they come (R3.2): the folders, then a page at a
    /// time, each taken in by the window as it arrives. `None`: the run comes back whole in
    /// [`GraphResult::mail`].
    pub mail_pieces: Option<std::sync::mpsc::SyncSender<mailcraft_model::sync::GraphMailResult>>,
    /// Set when the owner stops the run (Cancel): it stops after the page in hand.
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// What a [`GraphRequest`] came to. Never an error as a whole: mail goes on either way.
#[derive(Default)]
pub struct GraphResult {
    pub account: Id,
    /// A Graph token that was obtained for this (kept for the session; its refresh token is the
    /// account's newest).
    pub tokens: Option<Tokens>,
    /// The owner hasn't allowed this program to use Graph: "Connect calendar and contacts" asks.
    pub consent_needed: bool,
    /// A shared mailbox's run had mail to send, and the owner hasn't allowed sending from shared
    /// mailboxes yet (`Mail.Send.Shared`, #70): none of it was sent (it waits); the window asks.
    pub send_consent_needed: bool,
    /// The account has calendars opened by address (#70) and the owner hasn't allowed reading
    /// other people's calendars yet (`Calendars.ReadWrite.Shared`): they weren't read; the
    /// window asks.
    pub shared_calendars_consent_needed: bool,
    /// The Graph token that was handed in is no longer taken.
    pub token_refused: bool,
    /// The sign-in itself was refused when a token was asked for (it has run out, or was taken
    /// back): the owner has to sign in again.
    pub sign_in_refused: bool,
    /// Outbox messages that were sent (Microsoft files the Sent Items copy itself).
    pub sent: Vec<Id>,
    /// Outbox messages that must not be sent another way in this run: whether they went is not
    /// known, or the server said to try later.
    pub held: Vec<Id>,
    pub events: Option<mailcraft_model::remote::EventsDownload>,
    pub contacts: Option<mailcraft_model::remote::ContactsDownload>,
    /// What the server said to the calendar changes it was told ([`GraphRequest::changes`]).
    pub uploaded: Option<mailcraft_model::remote::EventsUploaded>,
    pub errors: Vec<String>,
    /// The same stretch of the account's other calendars.
    pub more_events: Vec<mailcraft_model::remote::EventsDownload>,
    /// The account's calendars and the groups they are listed in, when they were asked for.
    pub calendars: Option<mailcraft_model::remote::CalendarsDownload>,
    /// The mailbox's own categories with their colours, when they could be read.
    pub categories: Option<mailcraft_model::remote::CategoriesDownload>,
    /// The sign-in covers everything but an optional extra (reading the mailbox's category
    /// list): all else works, and "Connect calendar and contacts" would add it.
    pub extras_missing: bool,
    /// The working hours the mailbox has set, when they were asked for and could be read.
    pub mailbox_hours: Option<mailcraft_engine::cmd::calendar_settings::MailboxHours>,
    /// The Favorites as Outlook has them, when they were asked for: the list, or how the read
    /// went instead (the mailbox has none, the sign-in doesn't cover it, it failed).
    pub favorites: Option<mailcraft_model::remote::FavoritesDownload>,
    /// Why reading the Favorites failed, in a few plain words for the folder pane; no name
    /// and no id in it.
    pub favorites_error: String,
    /// When a token was obtained for this job: whether it is good for reading the Favorites
    /// (it was asked for with that, and the sign-in covers it).
    pub favorites_covered: Option<bool>,
    /// What the folders came to, when [`GraphRequest::mail`] asked.
    pub mail: Option<mailcraft_model::sync::GraphMailResult>,
}

/// A sign-in the host is busy with.
struct SigningIn {
    /// For Microsoft Graph (calendar, contacts, sending) rather than for mail.
    graph: bool,
    /// And for the Favorites of Outlook with it ([`favorites_sync`]).
    favorites: bool,
    /// Nobody asked for it (a Send/Receive that started by itself).
    quiet: bool,
    account: Id,
    /// For reading this shared mailbox, which is read once it is had (#70).
    shared: Option<Id>,
    /// And for sending from it: what waits in its Outbox goes once it is had, or is held.
    shared_send: bool,
    /// And for reading other people's calendars (one permission): a shared mailbox's (with
    /// `shared`) or those opened by address (without); read once it is had (#70).
    shared_calendars: bool,
    /// And for the To Do lists (#248).
    tasks: bool,
    /// The owner signs in on a page in the browser (no kept token to renew it with).
    browser: bool,
    /// The mailbox opening it was started under ([`Session::openings`]): an answer that comes
    /// after another mailbox took its place is thrown away.
    opening: u64,
    rx: Receiver<Result<Tokens, String>>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// UI state that persists between runs (and that agents can read and set).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiState {
    pub tab: String,
    pub backstage: bool,
    pub backstage_page: String,
    pub ribbon_collapsed: bool,
    /// The ribbon's layout: one row of commands (simplified, the default), or three rows in groups.
    pub ribbon_mode: ribbon::Mode,
    /// Interface theme: light, dark, or follow the OS appearance.
    pub theme: theme::Appearance,
    /// A message's body on a light page in the light theme's colours, as its sender made it, also
    /// when the window is dark: the reading pane and a message's own window, not its header.
    pub light_messages: bool,
    /// Interface size in percent: scales the whole interface on top of the system's display scale.
    pub interface_size: theme::InterfaceSize,
    /// Desktop: the main window's size and position, restored at the next launch.
    pub window: Option<window_geometry::WindowGeometry>,
    /// Interface language: `auto` or a language code.
    pub language: String,
    /// Engine preferences; the session owns them while running (see [`MailApp::prefs`]).
    pub options: mailcraft_engine::Prefs,
    /// How the window was arranged; the session owns it while running.
    pub view: mailcraft_engine::ViewState,
    pub folder_pane_width: f32,
    pub list_width: f32,
    /// Send/Receive Progress's width, and the height of its list of accounts (0: as tall as what
    /// it lists, up to a limit), as the owner dragged them (#145).
    pub sync_panel_width: f32,
    pub sync_panel_height: f32,
    /// Folders shown collapsed in the folder pane.
    pub collapsed_folders: Vec<Id>,
    /// Mailbox files opened lately, most recent first.
    pub recent: Vec<String>,
    /// The application (client) id Microsoft knows this installation by, for accounts that sign
    /// in on Microsoft's page and name none of their own. An identifier, not a secret.
    pub microsoft_client_id: String,
    /// Keep passwords and sign-ins in the operating system's credential store between runs.
    pub remember_sign_in: bool,
    /// Passwords and sign-ins kept by the system are filed under the servers they go to; the
    /// ones kept before that were moved (once, for the mailbox opened at start).
    pub sign_ins_bound: bool,
    /// The names (never the secrets) of every entry MailCraft has put in the credential store,
    /// so that removing the data, or turning remembering off, finds the ones of mailboxes that
    /// are no longer open.
    pub remembered_keys: Vec<String>,
    /// Mailbox files whose accounts' servers the owner confirmed, with a fingerprint of the
    /// addresses and servers confirmed ([`unconfirmed`]).
    pub confirmed_mailboxes: Vec<ConfirmedMailbox>,
    /// Reminders the owner dismissed (`event@occurrence`), so they don't come back at the next start.
    pub dismissed_reminders: Vec<String>,
    /// Download the pictures mail refers to on the web without being asked. Off unless the owner
    /// turns it on: a picture on the web tells its sender that the message was opened.
    pub download_pictures: bool,
    /// Senders whose pictures are downloaded without asking (addresses, in lower case).
    pub picture_senders: Vec<String>,
    /// Domains whose senders' pictures are downloaded without asking (`shop.example`: what
    /// follows the `@` of an address, in lower case, and that domain only).
    pub picture_domains: Vec<String>,
    /// Show the pictures of the senders the owner knows, or whose mail's pictures are downloaded,
    /// in the message list, from Gravatar, favicon.im and Google. Off unless the owner turns it on:
    /// asking tells those services whose pictures are looked at.
    pub sender_images: bool,
    /// The groups of the calendar list that are folded away under their headings.
    pub collapsed_calendar_groups: Vec<String>,
    /// Whether MailCraft looks for new versions once a day ([`updates`]): off until the owner says.
    pub update_check: updates::UpdateCheck,
    /// How many starts the owner was asked about it at, without an answer.
    pub update_asked: u8,
    /// When a check for a new version last got an answer (seconds since 1970; 0: never).
    pub update_checked: i64,
    /// The last newer version the owner was told of (`0.4.3`), so it is said once.
    pub update_told: String,
    /// Whether a copy that updates itself downloads a new version by itself, while the daily
    /// check is on ([`updates`]): on unless the owner turns it off.
    pub update_install: bool,
    /// A version that isn't installed by itself: the one the owner went back from because it
    /// didn't start ([`updates`]). Newer ones are.
    pub update_skip: String,
    /// The owner has been told, once, that MailCraft now installs its updates itself.
    pub update_install_told: bool,
    /// Show a desktop alert when new mail arrives in an Inbox of the owner's own accounts while
    /// the window isn't in front ([`alerts`]): on unless the owner turns it off.
    pub alert_new_mail: bool,
    /// Show a desktop alert for each reminder as it falls due: on unless the owner turns it off.
    pub alert_reminders: bool,
    /// The filters Advanced Search's Add filters puts in its panel (#329).
    pub advanced_filters: advanced_search::Filters,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            tab: "Home".into(),
            backstage: false,
            backstage_page: "info".into(),
            ribbon_collapsed: false,
            ribbon_mode: ribbon::Mode::default(),
            theme: theme::Appearance::default(),
            light_messages: false,
            interface_size: theme::InterfaceSize::default(),
            window: None,
            language: i18n::AUTO.into(),
            options: mailcraft_engine::Prefs::default(),
            view: mailcraft_engine::ViewState::default(),
            folder_pane_width: 230.0,
            list_width: 380.0,
            sync_panel_width: sync_panel::WIDTH,
            sync_panel_height: 0.0,
            collapsed_folders: Vec::new(),
            recent: Vec::new(),
            microsoft_client_id: String::new(),
            remember_sign_in: true,
            sign_ins_bound: false,
            remembered_keys: Vec::new(),
            confirmed_mailboxes: Vec::new(),
            dismissed_reminders: Vec::new(),
            advanced_filters: Default::default(),
            download_pictures: false,
            picture_senders: Vec::new(),
            picture_domains: Vec::new(),
            sender_images: false,
            collapsed_calendar_groups: Vec::new(),
            update_check: updates::UpdateCheck::default(),
            update_asked: 0,
            update_checked: 0,
            update_told: String::new(),
            update_install: true,
            update_skip: String::new(),
            update_install_told: false,
            alert_new_mail: true,
            alert_reminders: true,
        }
    }
}

/// How long a save runs before the status bar says how far it has got ([`MailApp::saving_text`]).
const SAVING_SHOWN_AFTER_MS: u64 = 700;

/// UI-level commands (`ui.*`): id, label, location, params. They change the window, not the
/// mailbox, and are listed with the engine's commands in command search and `engine.commands`.
pub const UI_COMMANDS: &[(&str, &str, &str, &str)] = &[
    ("ui.tab", "Show Ribbon Tab", "Ribbon", r#"{"tab": "Home"|"Send / Receive"|"Folder"|"View"|"Help"}"#),
    ("ui.backstage", "File", "Ribbon", r#"{"value"?: bool, "page"?: "info"|"open"|"options"|"about"}"#),
    ("ui.collapseRibbon", "Collapse the Ribbon", "Ribbon", r#"{"value"?: bool}"#),
    ("ui.ribbonMode", "Ribbon Layout", "View › Layout", r#"{"value": "simplified"|"classic"}"#),
    (
        "ui.dialog",
        "Open Dialog",
        "Window",
        r#"{"name": "commands"|"event"|"meeting"|"editEvent"|"contact"|"editContact"|"task"|"editTask"|"folder"|"goToDate"|"openCalendar"|"link"|"dateTime"|"wordCount"|"account"|"categories"|"rules"|"junkOptions"|"about"|"contributors"|"testers"|"reportProblem"|"proposeTime"|"forwardMeeting"|"proposals", …} (account: "email"?: string to start with, or "id": number for that account's settings, and with "page": "approval" the page that says what to do when its organisation has to approve MailCraft first (as `ui.approvalHelp` opens it); proposeTime: "id"?: number (an item of the calendar) | "message": number (an invitation in the mail); forwardMeeting: "id"?: number; proposals: "id"?: number | "message": number; junkOptions: "list"?: "safe"|"blocked" (the page it opens on) — the first two write to other people only from their own Propose and Send buttons)"#,
    ),
    ("ui.closeDialog", "Close Dialog", "Window", "{}"),
    ("ui.theme", "Interface Theme", "File › Options", r#"{"value": "light"|"dark"|"system"}"#),
    (
        "ui.lightMessages",
        "Messages on a Light Background",
        "File › Options",
        r#"{"value"?: bool (default: toggle)} — a message's body on a light page as its sender made it, also in the dark theme"#,
    ),
    ("ui.language", "Interface Language", "File › Options", r#"{"language": "auto"|"en"|"fr"|"pseudo"}"#),
    ("ui.interfaceSize", "Interface Size", "File › Options", r#"{"value": 80-200}"#),
    ("ui.focusSearch", "Search", "Search", "{}"),
    (
        "ui.advancedSearch",
        "Advanced Search",
        "Search",
        r#"{"open"?: bool (open it, or close it; left out: open it when closed, close it when open), "search"?: bool (true: search with what the open panel says, and close it)}"#,
    ),
    (
        "ui.removeAllData",
        "Remove MailCraft's Data from This Computer",
        "File › Info",
        "{} — removes everything MailCraft keeps on this computer, for an installed or a portable copy alike: every account's mail and the settings (the mailbox file and its content, the settings, the logs), the sign-ins remembered in the system's credential store, the downloaded pictures and the opened attachments; then quits. Mail on the servers is not touched. By hand it asks first; never for an agent",
    ),
    (
        "ui.openDataFolder",
        "Open Data Folder",
        "File › Info",
        "{} — shows, in the system's file manager, the folder of the mailbox open now when it is kept outside MailCraft's settings folder, else the settings folder (which holds the mailbox and the settings). Answers {\"opened\": \"mailbox\"|\"settings\"}. Not in the browser; never for an agent",
    ),
    (
        "ui.confirmServers",
        "Confirm Servers",
        "File › Info",
        "{} — the owner's yes to a mailbox opened from a file (File › Open, a double-click, a drop) once they have seen where each of its accounts connects to: until then nothing of it reaches a server (no Send/Receive, its Outbox stays, its changes wait, its rules don't run). Kept for that file while its accounts' addresses and servers stay the same. Asks first, listing the servers. Answers {\"confirmed\": bool, \"accounts\": [string]}",
    ),
    (
        "ui.sync",
        "Send/Receive All Folders",
        "Send / Receive",
        r#"{"account"?: number, "password"?: string, "quiet"?: bool (nobody asked: no page of a provider's is put in front of the owner, and no dialog)} — an account without a password asks for it (the "password" dialog); one that signs in with Microsoft and is told there that an administrator of its organisation has to approve MailCraft first gets the page that says what to do next (the account dialog's "approval" page, as `ui.approvalHelp` opens it), unless "quiet""#,
    ),
    (
        "ui.cancelSignIn",
        "Cancel Sign-In",
        "Send / Receive",
        "{} — gives up the sign-in, or the administrator's approval, that the browser is open for",
    ),
    ("ui.cancelSync", "Cancel Send/Receive", "Send / Receive", "{}"),
    (
        "ui.cancelOpen",
        "Cancel Opening the Mailbox",
        "Status bar",
        "{} — stops reading the mailbox File › Open is opening in the background; the one open stays",
    ),
    (
        "ui.moreResultsOnServer",
        "More Results on the Server",
        "Search",
        "{} — asks the server of each Microsoft mailbox the search in view involves for its next 50 results (the owner's click only: never by itself)",
    ),
    (
        "ui.skipSync",
        "Skip",
        "Send / Receive",
        r#"{"account": number} — leaves an account that is waiting its turn out of the Send/Receive under way (Send/Receive Progress); refused for the one running (Stop is for that) and for one that isn't waiting"#,
    ),
    (
        "ui.graphSwitch",
        "Switch to Microsoft Graph",
        "File › Info",
        r#"{"account"?: number} — the page that offers a Microsoft account kept in step by IMAP the switch to Microsoft Graph"#,
    ),
    (
        "ui.switchToGraph",
        "Switch to Microsoft Graph",
        "File › Info",
        r#"{"account"?: number, "answer": "switch"|"switchWithout"|"later"} — the page's answer: switch goes ahead once nothing waits for the IMAP server (a Send/Receive first, when something does), with a copy of the mailbox kept beside it; switchWithout goes ahead at once, leaving what waits for the IMAP server in that copy; later keeps the account on IMAP"#,
    ),
    (
        "ui.allowSharedCalendars",
        "Allow Reading Other People's Calendars",
        "File › Info and File › Open & Export",
        r#"{"account": number (a shared mailbox, or a Microsoft account that opens calendars by address), "confirmed": true} — asks the owner, on Microsoft's page in the browser, to let MailCraft read the calendars of the shared mailboxes he was given access to, and the calendars people share with him that he opens by address (Calendars.ReadWrite.Shared, one permission for both), by itself and never with a sign-in. The window asks first, when the owner ticks a shared mailbox's calendar for the first time, and File › Info and File › Open & Export has "Ask again". Allowed, the calendars of that shared mailbox are read with the next look at the calendar; refused, or when an administrator has to approve first, the window says what to do next. Answers {"pending": "signIn", "account": the account it is opened through}"#,
    ),
    (
        "ui.allowTasks",
        "Allow Keeping Tasks in Step with Microsoft To Do",
        "File › Info",
        r#"{"account": number (a Microsoft account), "confirmed": true} — asks the owner, on Microsoft's page in the browser, to let MailCraft read and change the To Do lists and tasks (Tasks.ReadWrite), by itself: an account that signed in before this was asked is not asked with a sign-in. The window asks first, when a tasks run finds the sign-in doesn't cover it, and File › Info has "Ask again". Allowed, the tasks are kept in step at the next look; refused, or when an administrator has to approve first, the window says what to do next. Answers {"pending": "signIn", "account": number}"#,
    ),
    (
        "ui.allowSharedSend",
        "Allow Sending from Shared Mailboxes",
        "Send / Receive",
        r#"{"account": number (a shared mailbox)} — asks the owner, on Microsoft's page in the browser, to let MailCraft send from the shared mailboxes he may send as or on behalf of (Mail.Send.Shared), by itself and never with a sign-in. The window asks first, at the first send from a shared mailbox. Allowed, what waits in its Outbox goes through its own address with the next Send/Receive; refused, it is held there with why, and nothing is sent from another address. Answers {"pending": "signIn", "account": the account it is opened through}"#,
    ),
    (
        "ui.keepOffline",
        "Mail to Keep Offline",
        "File › Info",
        r#"{"account": number (a Microsoft account, not a shared mailbox), "keepOffline": "3d"|"1w"|"2w"|"1m"|"3m"|"6m"|"12m"|"2y"|"5y"|"10y"|"all", "confirmed"?: true} — the slider's: how far back the account's mail (and its shared mailboxes') is kept in full on this computer. Shorter asks first, saying how much space it frees (`account.offlineSize`), unless "confirmed"; then the mail now outside goes back to its headers and preview at once (`account.thinOffline`, a few hundred messages a frame) and its files leave the disk with the next save. It stays on the server. Longer fetches the newly covered mail in the background, newest first. Answers {"account", "keepOffline", "changed": bool}"#,
    ),
    (
        "ui.connectMicrosoft",
        "Connect Calendar and Contacts",
        "File › Info",
        r#"{"account"?: number} — signs in for sending, calendar and contacts through Microsoft"#,
    ),
    (
        "ui.checkCalendar",
        "Check Calendar Connection",
        "File › Info",
        r#"{"account"?: number, "confirmed": true} — on the account's real server: makes one appointment called "MailCraft connection check" a year from today in its main calendar, reads it back, changes its time (once as an instant, once said in this computer's time zone), reads it back each time, and deletes it; nothing else is read or changed and nobody is written to. Every step is reported with the server's status and words, in a report dialog ("calendarCheck") and in the log. It asks first; a script says "confirmed": true. Answers {"pending": "calendarCheck", "account"}"#,
    ),
    (
        "ui.connectFavorites",
        "Show Outlook's Favorites",
        "File › Options",
        r#"{"account"?: number} — signs in on Microsoft's page for the two permissions that reading the Favorites of Outlook takes (the mailbox's configuration items, and its folders by name); nothing is written there, ever. Answers {"pending": "signIn", "account"}. When the organisation doesn't let its people give them, the folder pane says so in one line, and nothing else changes"#,
    ),
    (
        "ui.favorites",
        "Outlook's Favorites",
        "Folder › Favorites",
        r#"{"account"?: number, "refresh"?: bool, "answer"?: {"outcome": "read"|"missing"|"notPermitted"|"refused"|"approval"|"failed", "folders"?: [{"path": [string], "kind": string, "root"?: string}], "why"?: string}} — what the folder pane has of the Favorites of an account that signs in with Microsoft. "refresh": true reads them from its server again at the next chance ("Try again"); with "answer" nothing is asked of anybody, and it is taken in as what the host brought (for scripts, and for looking at the pane in each of its states: "approval" is a sign-in that an administrator has to approve first). They are read through Microsoft Graph. Answers {"account", "from": "server"|"mailbox", "own": bool, "directory": "organisation"|"personal"|null, "line": "offer"|"approval"|"refused"|"failed"|null (what the pane says under the Favorites heading), "text": string, "steps": ["copyMessage"|"help"] (the links under that line: `ui.copyAdminMessage`, `ui.approvalHelp`), "favorites": [number]}"#,
    ),
    (
        "ui.copyAdminConsentLink",
        "Copy a Link for Your Administrator",
        "File › Options",
        r#"{"account"?: number} — puts on the clipboard the address of Microsoft's page where an administrator of the account's organisation approves this application, once for everybody, for everything it asks an owner for (mail, calendar, contacts, and reading Outlook's Favorites through Microsoft Graph). The address names the application, the organisation's directory when it is known and those permissions, and nothing else; nothing is opened and nothing is sent. Answers {"account", "url"}. (The window's own links copy a whole message with it: `ui.copyAdminMessage`.)"#,
    ),
    (
        "ui.approvalHelp",
        "What to Do When Your Organisation Has to Approve MailCraft",
        "File › Info",
        r#"{"account"?: number} — opens the page that says what to do when an organisation lets only an administrator approve a new program: in Add Account, while it is at its sign-in with Microsoft, its own page for it (the sign-in that is waiting goes on waiting); for an account that exists (the one named, else the first that signs in with Microsoft), a dialog with the same page. Nothing is asked of anybody. Answers {"dialog": "account", "page": "approval", "account": number (0: the one being set up)}"#,
    ),
    (
        "ui.copyAdminMessage",
        "Copy a Message for My Administrator",
        "File › Info",
        r#"{"account"?: number} — puts on the clipboard a short, plain message for an administrator of the organisation: what MailCraft is, that it needs their approval once for the organisation, the address of Microsoft's page for that (the one `ui.copyAdminConsentLink` copies), and that their browser may show a page that can't be reached afterwards. Nothing of the owner is in it, nothing is opened and nothing is sent; the status bar says it was copied. For the account named; else the one Add Account is signing in; else the first that signs in with Microsoft. Answers {"account", "url", "text"}"#,
    ),
    (
        "ui.adminConsent",
        "I'm an Administrator: Approve MailCraft for My Organisation",
        "File › Info",
        r#"{"account"?: number} — opens Microsoft's admin-consent page in the system's browser, for this application and everything it asks an owner for, for the organisation's directory when it is known, and waits for the page to come back to this computer with a yes or a no. Approved: Add Account says so and goes on to the sign-in by itself; an account that exists signs in again (`ui.signInAgain`). Refused: what Microsoft answered is said. A sign-in that Add Account was waiting for gives way to it. Answers {"pending": "adminConsent", "account": number (0: the one being set up)}; `ui.cancelSignIn` gives it up"#,
    ),
    (
        "ui.signInAgain",
        "Try Signing In Again",
        "File › Info",
        r#"{"account"?: number} — "Try again", once an administrator has approved: Add Account starts its sign-in with Microsoft afresh (answers {"pending": "signIn", "account": 0}); an account that exists asks once more for what was refused for want of an approval: reading its Favorites in Outlook when the folder pane says they wait on an administrator (as `ui.connectFavorites`), and otherwise its sign-in, by Send/Receive (as `ui.sync`)"#,
    ),
    ("ui.dismissReminder", "Dismiss Reminder", "Reminders", r#"{"event": number, "occurrence": "YYYY-MM-DDTHH:MM"} | {"all": true}"#),
    (
        "ui.snoozeReminders",
        "Snooze Reminders",
        "Reminders",
        r#"{"minutes": 1-10080, "event"?: number, "occurrence"?: string (default: all that are showing)}"#,
    ),
    ("ui.microsoftClientId", "Microsoft Application ID", "File › Options", r#"{"value": string (empty: the build's own)}"#),
    ("ui.rememberSignIn", "Remember Passwords and Sign-Ins", "File › Options", r#"{"value"?: bool (default: toggle)}"#),
    (
        "ui.desktopAlerts",
        "Desktop Alerts",
        "File › Options",
        r#"{"newMail"?: bool, "reminders"?: bool} — whether the system shows a desktop alert when new mail arrives in an Inbox of your own accounts while the window isn't in front, and for each reminder as it falls due (both on unless turned off). Nothing else is changed, and no alert is shown by this. Answers the two switches"#,
    ),
    ("ui.open", "Open Mailbox", "File › Open & Export", "{}"),
    ("ui.import", "Import", "File › Open & Export", "{}"),
    ("ui.attach", "Attach File", "Message › Include", "{}"),
    ("ui.toggleFolder", "Expand or Collapse Folder", "Folder", r#"{"id": number}"#),
    (
        "ui.openWindow",
        "Open in a Window of Its Own",
        "Window",
        r#"{"message": number} | {"event": number, "proposed"?: {"start": "YYYY-MM-DDTHH:MM", "end": same} (another time, as somebody proposed it, put into the fields: nothing is changed or sent until the window sends it)} | {"new": "appointment"|"meeting", "start"?: "YYYY-MM-DDTHH:MM", "end"?: same, "allDay"?: bool, "calendar"?: number, "title"?: string, …the other fields of calendar.new} — a draft opens in the composer instead; a new appointment or meeting is made when its window is saved"#,
    ),
    (
        "ui.closeWindow",
        "Close Window",
        "Window",
        r#"{"window": number} | {"message": number} | {"event": number} | {"all": true} (default: the one in front) — closes without asking, also an appointment with unsaved changes"#,
    ),
    (
        "ui.moveWindowMessage",
        "Move to Folder",
        "Window",
        r#"{"folder": number, "window"?: number (default: the message-file window in front)} — files the message of a window that shows a message file (an .eml the system opened: it is in no folder until now) into `folder`; the mailbox gains that one message and the window is about it from then on. Answers {"window", "message"}. Never for an agent"#,
    ),
    (
        "ui.saveWindowMessage",
        "Save Message File As",
        "Window",
        r#"{"window"?: number (default: the message-file window in front), "path"?: string} — writes the file of a window that shows a message file to `path`; without one, answers {"name", "bytes", "data" (base64)}. Never for an agent"#,
    ),
    (
        "ui.eventWindow",
        "Appointment Window",
        "Window",
        r#"{"window"?: number (default: the appointment or meeting window in front), "page"?: "appointment"|"scheduling" (a meeting's Find a Time page), "recurrence"?: bool (open or close its Appointment Recurrence dialog), "do"?: "save"|"saveAndClose"|"send"|"close" — as its keys and its close button do: closing asks first when something isn't saved, and nothing is sent to anybody without the owner's yes; "freeBusy"?: {"people": [{"email", "busy": [{"start": seconds, "end": seconds, "showAs"}], "error"?: string}]} — a stand-in for the server's answer about the people invited, for Find a Time} — answers {"window", "event", "page", "meeting", "dirty", "recurrence", "asking", "importance": "low"|"normal"|"high", "online": bool (it is, or is to be, a Teams meeting), "joinUrl": string (the link to join by, when its server gave one: data, never followed by itself), "scheduling"?: {"day", "start", "end", "rows", "known", "unknown", "bars", "pending", "note"}}"#,
    ),
    (
        "ui.downloadPictures",
        "Download Pictures",
        "Message",
        r#"{"message"?: number (default: the selected message)} — its pictures from the web, until the app closes"#,
    ),
    (
        "ui.alwaysDownloadPictures",
        "Always Download Pictures from Sender",
        "File › Options",
        r#"{"sender": address} | {"domain": name (what follows the @; everybody at that domain)} | {"message"?: number} (its sender), "value"?: bool (default: true; false forgets the sender or the domain)"#,
    ),
    ("ui.downloadPicturesAutomatically", "Download Pictures in Mail Automatically", "File › Options", r#"{"value"?: bool (default: toggle)}"#),
    (
        "ui.senderImages",
        "Show Sender Images",
        "File › Options",
        r#"{"value"?: bool (default: toggle)} — the pictures of the senders you know (your contacts, and whoever mail in Sent Items went to), and of those whose mail's pictures are downloaded (for all mail, the sender, the domain or the message), in the message list, from Gravatar, else their domain's icon from favicon.im or Google; never for mail in Junk E-mail"#,
    ),
    (
        "ui.retryPictures",
        "Try Downloading Pictures Again",
        "Message",
        r#"{"message"?: number (default: the selected message)} — asks once more for those of its pictures that couldn't be downloaded"#,
    ),
    (
        "ui.clearPictureCache",
        "Clear Downloaded Pictures",
        "File › Options",
        "{} — forgets the pictures from the web that are kept, on this computer's disk and in memory; they are downloaded again when a message that may show them is read",
    ),
    (
        "ui.copySelection",
        "Copy Selected Text",
        "Message",
        r#"{"pane": true} | {"window": number} (default: where the keys go) — the text selected in a message being read, to the clipboard as plain text"#,
    ),
    (
        "ui.selectAllInMessage",
        "Select All Text in Message",
        "Message",
        r#"{"pane": true} | {"window": number} (default: where the keys go) — the whole body of the message being read"#,
    ),
    (
        "ui.copyDiagnostics",
        "Copy Diagnostics",
        "Help",
        r#"{} — the version, the platform and the end of the program's log, with addresses, names, paths, hosts, links, ids and quoted text taken out, to the clipboard for a problem report. Nothing is sent"#,
    ),
    (
        "ui.reportProblem",
        "Report a Problem",
        "Help",
        r#"{"what": string, "title"?: string, "expected"?: string, "via"?: "github"|"email" (default github), "diagnostics"?: bool, "path"?: string (github: where the diagnostics file is saved), "open"?: bool (default true)} — by hand without `what`: the Report a Problem window. github: opens the repository's new-issue page in the system's browser, filled in with the title, what was typed, the version and the platform, and nothing else (never the diagnostics, never more than 8,000 characters: text that doesn't fit is refused); with `diagnostics`, the same text as `ui.copyDiagnostics` is saved to `path` (without one it is answered as `name` and `data`), to attach on the page. email: starts a new message to MailCraft's maintainers from an account that sends mail, with the diagnostics attached when asked; nothing is sent until Send is pressed. `open: false` only answers what would be opened. MailCraft posts nothing itself. Answers {"via", "url"?, "length"?, "saved"?, "attached"?}"#,
    ),
    ("ui.copyLinkAddress", "Copy Link Address", "Message", r#"{"url": string} — a link's address (http, https, mailto or tel) to the clipboard"#),
    (
        "ui.checkForUpdates",
        "Check for Updates",
        "File › About",
        r#"{} — asks GitHub once which is the newest release of MailCraft (nothing about the owner or their mail is sent; nothing is downloaded or installed). A script or an agent may ask only while the owner has the daily check on (`ui.updateCheck`); otherwise refused: "The owner hasn't allowed update checks." Answers {"pending": "updateCheck", "version"}; a newer version is then said in the status bar and in File › About"#,
    ),
    (
        "ui.updateCheck",
        "Check for New Versions",
        "File › Options",
        r#"{"value"?: bool (default: toggle)} — whether MailCraft asks GitHub once a day which is the newest release (off until the owner turns it on). Answers {"value", "setting": "notAsked"|"on"|"off", "asked": number, "version", "newer": {"version", "security", "page"}|null, "checking": bool, "install": {"value": bool, "setting": "on"|"off"|"needsCheck"|"organisation"|"notThisCopy", "downloading": bool, "downloaded": version|null}}"#,
    ),
    (
        "ui.updateInstall",
        "Install Updates Automatically",
        "File › Options",
        r#"{"value"?: bool (default: toggle)} — whether a copy that updates itself (the Windows .msi, the macOS app, a Linux AppImage) downloads a newer release by itself while the daily check is on, and checks it by the release's own signature (on unless the owner turns it off). Refused when the organisation has turned it off ("Turned off by your organisation") and for a copy that doesn't update itself. Agents and scripts can't start a download. Answers what `ui.updateCheck` answers"#,
    ),
    (
        "ui.restartToUpdate",
        "Restart to Update",
        "Status bar · File › About",
        r#"{} — once a newer version has been downloaded and checked by its release signature: closes the window as quitting does (what is being written is asked about, Send/Receive stops, the mailbox is saved), installs the new version and starts it. The owner's click only: refused for agents and the control channel ("Restart to update is the owner's click."). Not pressed, the new version is installed when MailCraft is quit. MailCraft never restarts by itself"#,
    ),
    (
        "ui.goBackVersion",
        "Go Back to the Previous Version",
        "Above the status bar",
        r#"{} — offered when a newly installed version didn't start twice running and the one it replaced is kept: closes the window as quitting does, puts the kept version back (and the mailbox file as it was before the new version changed its format, when it did) and starts it; the version gone back from is never installed by itself again. The owner's click only: refused for agents and the control channel"#,
    ),
    (
        "ui.dismissSettingsNotice",
        "Dismiss Settings Notice",
        "Above the status bar",
        r#"{} — puts away the line that says the settings file couldn't be read at start, so MailCraft started with its own settings (for this run). Answers {"dismissed": bool}"#,
    ),
    (
        "ui.dismissUpdateNotice",
        "Dismiss Update Notice",
        "Above the status bar",
        r#"{"notice": "installsItself" | "didntStart"} — puts away the line that says MailCraft now installs its updates itself (said once), or the offer to go back to the previous version (kept for this run)"#,
    ),
    (
        "ui.toggleCalendarGroup",
        "Expand or Collapse Calendar Group",
        "Calendar",
        r#"{"group": string (its heading in the list of calendars), "value"?: bool (true: folded away; default: the other way than it is)}"#,
    ),
    (
        "ui.joinMeeting",
        "Join Online Meeting",
        "Meeting › Teams Meeting",
        r#"{"event"?: number (default: the selected item)} — opens the link to join an online meeting by in the system's browser. The link comes from the meeting's server: only an https address is opened, and only on this command"#,
    ),
    (
        "ui.findInvitation",
        "Look for This Meeting on the Calendar",
        "Meeting › Respond",
        r#"{"message"?: number (default: the selected message)} — asks the account's server for the days round the meeting a message invites to, when the calendar here doesn't have it yet"#,
    ),
];

/// The application.
pub struct MailApp {
    pub session: Session,
    pub ui: UiState,
    pub services: Services,
    pub dialog: Option<dialogs::Dialog>,
    /// The contacts picked together in the People list with Ctrl (⌘ on a Mac) and a click, the
    /// selected one among them; empty when one contact alone is selected. The window's own: a
    /// command is given the ids (Merge Contacts…, #506).
    pub people_picked: Vec<Id>,
    pub status_msg: Option<(String, f64)>,
    /// macOS: the window has no title bar; leave room for the traffic lights.
    pub integrated_titlebar: bool,
    /// The search box's text while it is being typed.
    pub(crate) search_text: String,
    pub(crate) search_focus: bool,
    /// Advanced Search, while its panel is open under the search box (#329).
    pub(crate) advanced_search: Option<advanced_search::Panel>,
    /// Whether the title bar had room for the search box when it was last drawn.
    pub(crate) search_box_shown: bool,
    /// The Bcc line of the message being written is shown.
    pub(crate) compose_bcc: bool,
    /// The body of the message being written takes the keyboard back at its next frame (after a
    /// ribbon button or a dialog changed its text).
    pub(crate) body_focus_wanted: bool,
    /// File › Signatures was on screen in the last frame: leaving it saves and closes the open
    /// signature.
    signatures_shown: bool,
    /// What File › Signatures last said of a picture (a refusal, or that it was made smaller).
    signature_note: Option<signatures::Note>,
    /// The last input the window was given, by kind only (never what was typed), for
    /// `ui.inspect`: what a script looks at when typing or the keyboard goes astray.
    pub(crate) input_trail: std::collections::VecDeque<String>,
    /// The message list should scroll to the selected row (after a keyboard move).
    pub(crate) reveal_selection: bool,
    /// The rows the message list was last drawn from, the selection then (its messages, and those
    /// picked as their conversation) and what the rows drew selected: worked out again only when
    /// one of them changes, not at every frame (#503).
    pub(crate) drawn: Option<(std::sync::Arc<Vec<mailcraft_engine::list::Row>>, Vec<Id>, Vec<Id>, std::sync::Arc<mailcraft_engine::Drawn>)>,
    /// The rows of the message list that fit in it as last drawn: what Page Up and Page Down move
    /// by (#426).
    pub(crate) list_page_rows: usize,
    /// Passwords typed this session, per account. Never saved.
    passwords: std::collections::HashMap<Id, String>,
    /// The folders of an Exchange server account the last run could not read (named as the owner
    /// knows them; no id), for File › Info. Not saved.
    pub(crate) ews_unreadable: std::collections::HashMap<Id, Vec<String>>,
    /// How the last run of each Exchange server account signed in (`basic` or `ntlm`), for File ›
    /// Info. Not saved.
    pub(crate) ews_signed_in: std::collections::HashMap<Id, String>,
    /// The fingerprint of the certificate an Exchange server account's server showed that needs
    /// the owner's say (its last run stopped at it), for File › Info. Not saved.
    pub(crate) ews_certificate: std::collections::HashMap<Id, String>,
    /// [`mailcraft_engine::Session::openings`] when the secrets in memory were last checked
    /// against the mailbox they belong to.
    secrets_opening: u64,
    /// What the credential store gave back for each key this session, read in the background
    /// ([`secret_read`]); only what was found. The window asks this, never the store.
    secret_cache: std::collections::HashMap<String, String>,
    /// The background read of the store under way, with the opening it was started for.
    secret_read: Option<(u64, std::sync::mpsc::Receiver<secret_read::SecretRead>)>,
    /// The opening, and whether remembering was on, when the store was last read by itself.
    secrets_asked: Option<(u64, bool)>,
    /// Send/Receive the owner asked for, held until the store has answered.
    sync_after_secrets: Vec<Value>,
    /// The open mailbox came from a file whose servers nobody has confirmed: nothing of it
    /// reaches a server ([`unconfirmed`]).
    pub(crate) unconfirmed: bool,
    /// The settings file couldn't be read at start, and the line saying so hasn't been put away
    /// ([`settings_notice`]).
    pub settings_unread: Option<settings_notice::SettingsUnread>,
    /// The jobs that reach servers, put aside while the mailbox is unconfirmed.
    pub(crate) held: unconfirmed::Held,
    /// The file the window started with (the owner's own mailbox, or the one `--mailbox` named),
    /// which is never unconfirmed.
    pub(crate) own_mailbox: Option<String>,
    /// Why the system's credential store couldn't be used this session, once that has been found
    /// out; the owner has been told.
    pub(crate) secrets_trouble: Option<StoreTrouble>,
    /// What signing in on a provider's page brought back this session, per account. Never saved.
    tokens: std::collections::HashMap<Id, Tokens>,
    /// Microsoft Graph tokens obtained this session, per account. Never saved.
    pub(crate) graph_tokens: std::collections::HashMap<Id, Tokens>,
    /// What Graph is doing for an account before its mail is synchronised: the account, whether
    /// nobody asked, whether the calendar and contacts were asked for, and the answer to come.
    graph_rx: Option<(Id, bool, bool, Receiver<GraphResult>)>,
    /// The pieces of a Graph mail run as they come (R3.2), and what they had to say.
    graph_pieces: Option<Receiver<mailcraft_model::sync::GraphMailResult>>,
    graph_problems: Vec<String>,
    /// Pieces of the Graph mail run under way taken in so far: when there were some, the mail
    /// came that way, and the job's own answer has none of it.
    pub(crate) graph_pieces_taken: usize,
    /// The account whose Graph mail run follows a sign-in for it that just went through: when
    /// that run is refused too, the owner is told rather than sent to Microsoft's page again (#124).
    graph_after_sign_in: Option<Id>,
    /// Messages of Microsoft accounts being fetched whole ([`graph_fetch`]).
    pub(crate) fetching: graph_fetch::Fetching,
    /// Mail outside its window taken back to its headers and preview (#143, [`keep_offline`]).
    pub(crate) thinning: keep_offline::Thinning,
    /// Pages of a search on the server being asked for ([`server_search`]).
    pub(crate) server_searching: server_search::ServerSearching,
    /// When each account's calendar and contacts were last brought down (milliseconds).
    graph_checked: std::collections::HashMap<Id, f64>,
    /// Accounts whose owner hasn't allowed Graph yet ("Connect calendar and contacts").
    graph_consent_needed: std::collections::HashSet<Id>,
    /// Keeping the calendar current between Send/Receive runs ([`calendar_sync`]).
    calendar: calendar_sync::CalendarSync,
    /// Keeping the Favorites of the folder pane the ones Outlook shows ([`favorites_sync`]).
    favorites: favorites_sync::FavoritesSync,
    /// The next sign-in for Microsoft Graph also asks for the Favorites' permissions.
    favorites_next: bool,
    /// The next sign-in for Microsoft Graph asks for reading shared mailboxes too, for this
    /// shared mailbox, which is read once it is had ([`shared_mailboxes`], #70).
    shared_next: Option<Id>,
    /// With `shared_next`: that sign-in asks for sending from shared mailboxes too (#70).
    shared_send_next: bool,
    /// With `shared_next`: that sign-in asks for reading the calendars of shared mailboxes too;
    /// without it, for reading the calendars opened by address (#70).
    shared_calendars_next: bool,
    tasks_next: bool,
    pub(crate) tasks: task_sync::TaskSync,
    /// Opening other people's calendars by address (#70).
    open_shared: open_shared::OpenShared,
    /// Where each shared mailbox's calendars stand with the owner's permission for them (#70):
    /// what a run found out, and what was asked.
    shared_calendar_perm: std::collections::HashMap<Id, shared_calendars::Permission>,
    /// The access tokens that read shared mailboxes' calendars (they carry
    /// `Calendars.ReadWrite.Shared`, which a shared mailbox's mail token needn't), in memory only.
    shared_calendar_tokens: std::collections::HashMap<Id, Tokens>,
    /// Shared mailboxes whose Outbox waits for the owner's permission to send from shared
    /// mailboxes (#70): a run that starts by itself neither takes that mail along nor starts for
    /// it, until the owner asks for a run, presses Send in it, or allows it.
    shared_send_waits: std::collections::HashSet<Id>,
    /// Shared mailboxes the owner pressed Send in since their last run: when that run finds
    /// sending from shared mailboxes isn't allowed yet, the window asks, whoever started it (#70).
    shared_send_pressed: std::collections::HashSet<Id>,
    /// Shared mailboxes whose server won't let the owner in, and why (no Full Access, no such
    /// mailbox…): shown as No access, and not read again by a Send/Receive that started by
    /// itself, only by one the owner asks for (#70).
    shared_refused: std::collections::HashMap<Id, mailcraft_model::sync::SharedRefusal>,
    /// Shared mailboxes just added in Add Shared Mailbox whose Inbox hasn't been read yet: kept
    /// only once it has been, and removed again when it can't be (#70).
    shared_checking: std::collections::HashSet<Id>,
    /// A shared mailbox being added whose first run waits for the Send/Receive under way, which
    /// was called off for it: it starts as soon as that one has stopped.
    shared_check_next: Option<Id>,
    /// The next sign-in asks for mail alone (account setup's "mail only").
    mail_only_next: bool,
    /// A sign-in the host is waiting on (the owner is in the browser).
    signing_in: Option<SigningIn>,
    /// What a sign-in for an account that is being set up came to, until its dialog takes it.
    setup_sign_in: Option<Result<Tokens, String>>,
    /// An administrator's approval the host is waiting on, and what the last came to ([`approval`]).
    approvals: approval::Approvals,
    /// A synchronisation in flight: an IMAP run, or a JMAP account's ([`MailRun`]).
    sync_rx: Option<MailRun>,
    /// What the run in flight has come to so far, piece by piece.
    sync_totals: mailcraft_engine::cmd::account::SyncTotals,
    /// The run in flight: whose it is, how much was still waiting to go out when it turned to
    /// bringing mail down (unknown until then), whether it was called off to let more out, and
    /// whether nobody asked for it (it comes back as it was).
    sync_run: Option<(Id, Option<usize>, bool, bool)>,
    /// A piece of the run in flight said the server refused the sign-in.
    sync_refused: bool,
    /// JMAP accounts whose server turned their credential down this session: no round that starts
    /// by itself goes back to one until the owner asks for its Send/Receive ([`jmap_mail`]).
    jmap_refused: std::collections::HashSet<Id>,
    control_rx: Option<Receiver<ControlRequest>>,
    shot_token: u64,
    queued_shots: Vec<(u64, f64, u32)>,
    /// Frames the integration handed back, by the token they were asked with.
    shot_images: std::sync::Arc<std::sync::Mutex<Vec<(u64, std::sync::Arc<egui::ColorImage>)>>>,
    pending_shots: Vec<(u64, Option<String>, bool, std::sync::mpsc::Sender<Value>, f64)>,
    pub(crate) synthetic: Vec<egui::Event>,
    /// What agents want that waits for the owner's yes ([`agent_gate`]).
    pub(crate) asking: agent_gate::Questions,
    styled: bool,
    /// The interface theme last installed in egui ([`theme::apply`]).
    applied_theme: Option<egui::ThemePreference>,
    /// The interface language last told the host ([`Services::on_language`]).
    applied_language: Option<i18n::Lang>,
    fonts_frames: u32,
    pub frame_ms: f64,
    /// The window title last sent; a viewport command schedules a repaint, so only send changes.
    sent_title: String,
    pub quit_requested: bool,
    /// MailCraft's data was removed from this computer ([`data_removal`]): nothing is written
    /// or remembered from here on, and the window goes when its report is read.
    pub data_removed: bool,
    /// Save the mailbox by itself a moment after each change (on by default).
    pub autosave: bool,
    /// Open account setup when the app starts on a mailbox nobody has set up yet (the host turns
    /// this on for an ordinary start; not for the sample or a mailbox named on the command line).
    pub offer_setup: bool,
    /// Offer each Microsoft account still on IMAP the switch to Microsoft Graph, on its page, by
    /// itself ([`graph_switch`]): the desktop app does, for a mailbox kept in a file. Off, the
    /// page opens only when asked for (`ui.graphSwitch`).
    pub offer_graph_switch: bool,
    /// Microsoft accounts on IMAP whose page offering the switch to Microsoft Graph was shown
    /// in this run (closed without an answer, it is shown again in the next one).
    graph_switch_shown: std::collections::HashSet<Id>,
    /// Accounts switching to Microsoft Graph for which the switch was tried since their last
    /// Send/Receive ([`graph_switch`]).
    graph_switch_tried: std::collections::HashSet<Id>,
    /// The copy of the mailbox being written before an account switches to Microsoft Graph
    /// ([`graph_switch`]): the window shows that it is working until it is done.
    pub(crate) graph_switch_copy: Option<graph_switch::Copying>,
    /// Accounts whose waiting switch was offered "Switch without Them" in this run.
    graph_switch_waiting_shown: std::collections::HashSet<Id>,
    /// Send/Receive by itself when the app starts and every few minutes (on by default; how often
    /// is the owner's `syncMinutes` option).
    pub auto_sync: bool,
    /// Scroll bars float over what they scroll, slim, and show only while it moves or the pointer
    /// is over it, as a Mac's do: the message list keeps no lane for its bar and has no arrows.
    /// On a Mac only; tests and screenshots turn it on anywhere.
    pub overlay_scroll_bars: bool,
    /// The window is drawn for headless screenshots (`examples/ui_shot`): a password shown with
    /// its eye is drawn as dots all the same, as while `ui.screenshot` waits for a frame (#438).
    pub headless_shots: bool,
    /// This frame's input came from the control channel (`synthetic`), or a request from it is
    /// being handled: a password's eye doesn't answer it (#438), and what it selects in a message
    /// isn't put in the primary selection (`reading::select::settle`, `compose::editor`).
    driven: bool,
    /// When the last round that started by itself began (milliseconds; 0 before the first).
    last_auto_sync: f64,
    /// Accounts waiting their turn in a Send/Receive round, and whether nobody asked for it.
    sync_queue: std::collections::VecDeque<(Id, bool)>,
    /// Accounts whose Inbox pass is still to come, at the start of a round, and whether nobody
    /// asked for it (#276).
    inbox_queue: std::collections::VecDeque<(Id, bool)>,
    /// The account whose Inbox pass is under way.
    inbox_run: Option<Id>,
    /// The Inbox checked by itself every minute, between rounds (#276, [`inbox_pass`]).
    inbox_check: inbox_pass::InboxCheck,
    /// When it was last looked whether something made here waits to go to a server (milliseconds).
    push_checked: f64,
    /// Accounts whose next run that nobody asked for only tells the server what changed here
    /// (the quick way out for a read mark or a move), bringing nothing down.
    push_only_next: std::collections::HashSet<Id>,
    /// Per account: how much was still waiting after its last Send/Receive, and when that was.
    /// What a server refuses keeps waiting, and must not start a run every few seconds.
    push_seen: std::collections::HashMap<Id, (usize, f64)>,
    last_autosave: f64,
    /// When the draft being written was last saved by itself (ms; 0: not since it was opened).
    draft_saved_at: f64,
    saved_rev: u64,
    /// How far the background saver has got with what it was handed, as of this frame (`None`:
    /// nothing waiting or being written).
    pub(crate) save_progress: Option<mailcraft_engine::io::SaveProgress>,
    /// The window was asked to close while a save was still to be written: the state it was
    /// asked to close with (the session's `rev`), which closes it once written
    /// ([`MailApp::close_requested`]). A download that goes on meanwhile changes the mailbox
    /// every page: those later states are `save_on_exit`'s.
    pub(crate) closing_after_save: Option<u64>,
    /// What closing waited for is written: the next close request goes through.
    close_ready: bool,
    /// What closing waited for couldn't be written (the status bar said why): the window stayed
    /// open, and the next close request goes through without waiting again, until a save works.
    close_anyway: bool,
    /// File › New or File › Open, asked for while the saver had the open mailbox to write: it
    /// runs once the saver is done with the file ([`MailApp::through_saver`]).
    pub(crate) after_save: Option<(String, Value)>,
    /// What became of the last command that waited for a save (`ui.inspect`'s `saving.last`): an
    /// agent that was answered `"pending": "save"` learns here whether it ran.
    pub(crate) after_save_outcome: Option<Value>,
    /// The command that waited for a save is running now: it isn't deferred again.
    running_after_save: bool,
    /// The egui context, once the first frame has run (background work wakes the UI with it).
    pub(crate) ctx: Option<egui::Context>,
    /// The file dialog the host is showing, and what its answer is for ([`files`]).
    pub(crate) file_dialog: Option<files::PendingDialog>,
    /// Web: what the file the picker will deliver is for.
    pub(crate) pending_open: Option<String>,
    /// Calendar and contact files the system asked MailCraft to open, each waiting to be asked
    /// about ([`files::ask_next`]).
    pub(crate) opened_waiting: std::collections::VecDeque<String>,
    /// The time zone the calendar was shown in when the window was last laid out: what the
    /// appointments that stand open were read by ([`MailApp::follow_zone`]).
    shown_zone: Option<mailcraft_time::Zone>,
    /// Where each calendar occurrence was drawn (for clicks injected by tests and agents).
    pub(crate) hit: control::HitMap,
    /// The reminders window: what is due, what was put off.
    pub(crate) reminders: reminders::State,
    /// What went wrong in each account's last Send/Receive, until a run of it goes through:
    /// the account, its name, when, and the lines.
    sync_problems: Vec<(Id, String, String, Vec<String>)>,
    /// Send/Receive Progress, when it is open ([`sync_panel`]).
    pub(crate) sync_panel: Option<sync_panel::Panel>,
    /// What runs now was asked for by the owner's own hand: a frame of the window that a press, a
    /// release or a key started, or a file let go over it (#152). Never so for the control
    /// channel, an agent or the background.
    pub(crate) by_hand: bool,
    /// The engine's last refusal as it was ([`execute`](Self::execute)), so that the window says
    /// it in the interface language; scripts and agents get its English.
    last_error: Option<mailcraft_engine::CmdError>,
    /// A refusal of what the owner has just done by hand, shown in a message box once the frame
    /// is laid out ([`MailApp::refusal`]).
    pub(crate) refused: Option<String>,
    /// Messages and appointments opened in windows of their own ([`windows`]).
    pub(crate) windows: windows::Windows,
    /// Pictures from the web: which messages may show them, and the ones on their way.
    pub(crate) remote_pictures: reading::RemotePictures,
    /// The pictures of the senders in the message list, as far as they have come.
    pub(crate) sender_images: sender_images::SenderImages,
    /// The text selected in the messages that are being read, in the pane and in windows.
    pub(crate) text_selection: reading::Selections,
    /// The last calendar command the owner ran to its end ([`MailApp::run`]), numbered, with how
    /// it went: an appointment window whose save had to ask first learns from it what came of
    /// the question once it is answered.
    pub(crate) ran: (u64, String, Result<Value, String>),
    /// When people are free and busy, as Find a Time found out ([`scheduling`]).
    pub(crate) free_busy: scheduling::Store,
    /// What this run knows of new versions ([`updates`]).
    pub(crate) updates: updates::Updates,
    /// The desktop alerts shown, and the mailbox as the last Send/Receive found it ([`alerts`]).
    pub(crate) alerts: alerts::State,
    /// The mailbox the window starts with is still being read ([`opening`]).
    pub(crate) opening: Option<opening::Opening>,
    /// What the window takes up once it shows its mailbox ([`opening::after`]).
    pub(crate) later: opening::Later,
    /// Another mailbox being read in the background, to take the place of this one ([`opening`]).
    pub(crate) reading_ahead: Option<opening::ReadAhead>,
    /// `file.open` runs for a mailbox read in the background: it isn't read again.
    pub(crate) swapping: bool,
}

/// A Send/Receive of one account in flight, its pieces to come. A JMAP account's run takes the place
/// of an IMAP one, so that whatever waits for a Send/Receive (another run, Cancel, Skip, a round by
/// itself, the calendar's job) waits for it too.
pub(crate) enum MailRun {
    Imap(Receiver<SyncResult>),
    /// `opening`: the mailbox opening it was started under ([`Session::openings`]).
    Jmap {
        account: Id,
        opening: u64,
        pieces: Receiver<mailcraft_model::sync::JmapMailResult>,
    },
    /// An Exchange server account's run ([`ews_mail`]).
    Ews {
        account: Id,
        opening: u64,
        pieces: Receiver<mailcraft_model::sync::EwsMailResult>,
    },
}

impl MailApp {
    pub fn new(session: Session, services: Services) -> Self {
        let saved_rev = session.rev();
        let own_mailbox = session.path.as_ref().map(|p| unconfirmed::kept_path(&p.to_string_lossy()));
        MailApp {
            session,
            ui: UiState::default(),
            services,
            dialog: None,
            people_picked: Vec::new(),
            status_msg: None,
            integrated_titlebar: false,
            search_text: String::new(),
            search_focus: false,
            advanced_search: None,
            search_box_shown: false,
            compose_bcc: false,
            body_focus_wanted: false,
            signatures_shown: false,
            signature_note: None,
            input_trail: std::collections::VecDeque::new(),
            reveal_selection: false,
            drawn: None,
            list_page_rows: 10,
            passwords: Default::default(),
            ews_unreadable: Default::default(),
            ews_signed_in: Default::default(),
            ews_certificate: Default::default(),
            secrets_opening: 0,
            secret_cache: Default::default(),
            secret_read: None,
            secrets_asked: None,
            sync_after_secrets: Vec::new(),
            unconfirmed: false,
            settings_unread: None,
            held: Default::default(),
            own_mailbox,
            secrets_trouble: None,
            tokens: Default::default(),
            graph_tokens: Default::default(),
            graph_rx: None,
            graph_pieces: None,
            graph_problems: Vec::new(),
            graph_pieces_taken: 0,
            graph_after_sign_in: None,
            fetching: Default::default(),
            thinning: Default::default(),
            server_searching: Default::default(),
            graph_checked: Default::default(),
            graph_consent_needed: Default::default(),
            calendar: Default::default(),
            favorites: Default::default(),
            favorites_next: false,
            shared_next: None,
            shared_send_next: false,
            shared_calendars_next: false,
            tasks_next: false,
            tasks: Default::default(),
            open_shared: Default::default(),
            shared_calendar_perm: Default::default(),
            shared_calendar_tokens: Default::default(),
            shared_send_waits: Default::default(),
            shared_send_pressed: Default::default(),
            shared_refused: Default::default(),
            shared_checking: Default::default(),
            shared_check_next: None,
            mail_only_next: false,
            signing_in: None,
            setup_sign_in: None,
            approvals: Default::default(),
            sync_rx: None,
            sync_totals: Default::default(),
            sync_run: None,
            sync_refused: false,
            jmap_refused: Default::default(),
            control_rx: None,
            shot_token: 0,
            queued_shots: Vec::new(),
            shot_images: Default::default(),
            pending_shots: Vec::new(),
            synthetic: Vec::new(),
            asking: Default::default(),
            styled: false,
            applied_theme: None,
            applied_language: None,
            fonts_frames: 0,
            frame_ms: 0.0,
            sent_title: String::new(),
            shown_zone: None,
            quit_requested: false,
            data_removed: false,
            autosave: true,
            offer_setup: false,
            offer_graph_switch: false,
            graph_switch_shown: Default::default(),
            graph_switch_tried: Default::default(),
            graph_switch_copy: None,
            graph_switch_waiting_shown: Default::default(),
            auto_sync: true,
            overlay_scroll_bars: cfg!(target_os = "macos"),
            headless_shots: false,
            driven: false,
            last_auto_sync: 0.0,
            sync_queue: Default::default(),
            inbox_queue: Default::default(),
            inbox_run: None,
            inbox_check: Default::default(),
            push_checked: 0.0,
            push_only_next: Default::default(),
            push_seen: Default::default(),
            last_autosave: 0.0,
            draft_saved_at: 0.0,
            saved_rev,
            save_progress: None,
            closing_after_save: None,
            close_ready: false,
            close_anyway: false,
            after_save: None,
            after_save_outcome: None,
            running_after_save: false,
            ctx: None,
            file_dialog: None,
            pending_open: None,
            opened_waiting: Default::default(),
            hit: Default::default(),
            reminders: Default::default(),
            sync_problems: Vec::new(),
            sync_panel: None,
            by_hand: false,
            last_error: None,
            refused: None,
            windows: Default::default(),
            remote_pictures: Default::default(),
            sender_images: Default::default(),
            text_selection: Default::default(),
            ran: (0, String::new(), Ok(Value::Null)),
            free_busy: Default::default(),
            updates: Default::default(),
            alerts: Default::default(),
            opening: None,
            later: Default::default(),
            reading_ahead: None,
            swapping: false,
        }
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Preferences to save between runs: the UI state plus the engine's preferences and view.
    pub fn prefs(&self) -> UiState {
        let mut ui = self.ui.clone();
        ui.options = self.session.prefs.clone();
        ui.view = self.session.view.clone();
        // What is searched for and looked at belongs to the run, not to the next one.
        ui.view.search.clear();
        ui.view.people_search.clear();
        ui
    }

    /// Restore preferences saved by [`MailApp::prefs`].
    pub fn apply_prefs(&mut self, ui: UiState) {
        self.ui = ui;
        self.ui.backstage = false;
        // As in Outlook, a run starts on Home, and File on Info: where the owner last was isn't
        // where they want to start.
        self.ui.tab = "Home".into();
        self.ui.backstage_page = "info".into();
        self.ui.folder_pane_width = if self.ui.folder_pane_width.is_finite() { self.ui.folder_pane_width.clamp(150.0, 500.0) } else { 230.0 };
        self.ui.list_width = if self.ui.list_width.is_finite() { self.ui.list_width.clamp(240.0, 900.0) } else { 380.0 };
        (self.ui.sync_panel_width, self.ui.sync_panel_height) = sync_panel::sane_size(self.ui.sync_panel_width, self.ui.sync_panel_height);
        self.ui.collapsed_folders.truncate(10_000);
        self.ui.recent.truncate(20);
        self.ui.confirmed_mailboxes.truncate(200);
        self.ui.collapsed_calendar_groups.truncate(200);
        updates::sanitize(&mut self.ui);
        self.session.prefs = self.ui.options.clone();
        // Layout choices carry over; the folder, the day and the search start fresh.
        let saved = self.ui.view.clone();
        let v = &mut self.session.view;
        (v.sort, v.sort_descending, v.conversations, v.reading_pane, v.folder_pane, v.calendar, v.task_filter) =
            (saved.sort, saved.sort_descending, saved.conversations, saved.reading_pane, saved.folder_pane, saved.calendar, saved.task_filter);
        // How People and Tasks are arranged, likewise.
        v.people_sort = saved.people_sort;
        v.task_sort = saved.task_sort;
        // Which calendars lie over one another is a layout choice too (calendars that this
        // mailbox doesn't have are simply not found).
        (v.overlay, v.overlay_front) = (saved.overlay.iter().copied().take(200).collect(), saved.overlay_front);
    }

    /// Whether a command exists and is enabled.
    pub fn enabled(&self, id: &str) -> bool {
        // Nothing runs while the mailbox the window starts with is read.
        if self.opening.is_some() {
            return false;
        }
        match self.session.registry.get(id) {
            Some(s) => (s.enabled)(&self.session).is_none(),
            None if id == "ui.sync" => {
                !self.data_removed
                    && self.session.enabled("sync.request")
                    && self.services.can_sync()
                    && self.sync_rx.is_none()
                    && (self.graph_rx.is_none() || self.inbox_check.under_way())
                    && self.signing_in.is_none()
            }
            None if id == "ui.cancelSignIn" => self.signing_in.is_some() || self.approving(),
            None if id == "ui.cancelSync" => self.sync_rx.is_some(),
            None if id == "ui.cancelOpen" => self.reading_ahead.is_some(),
            None if id == "ui.moreResultsOnServer" => {
                matches!(self.server_link(), server_search::Link::Ask | server_search::Link::NotConnected { retry: true })
            }
            None if id == "ui.copySelection" => self.text_selection.any(),
            None if id == "ui.openDataFolder" => self.services.data_folder.is_some() && !self.data_removed,
            None => UI_COMMANDS.iter().any(|c| c.0 == id),
        }
    }

    /// Run a command the user asked for (ribbon, shortcut, menu, file drop). A command that needs
    /// input the user hasn't given yet opens its dialog; one that writes a file asks where (or, in
    /// the browser, downloads it); a failure shows in the status bar.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        if self.opening.is_some() {
            return Err(opening::STILL_OPENING.into());
        }
        // (Whether one occurrence or the series was meant when a recurring item was opened.)
        let params = dialogs::as_opened(self, id, params);
        // A message being written isn't let go of without a word (#442).
        if dialogs::leaves_message(self, id, &params) {
            self.dialog = Some(dialogs::leaving(id, params));
            return Ok(json!({"pending": "discardDraft"}));
        }
        if let Some(name) = dialogs::input_dialog(id, &params)
            && self.enabled(id)
        {
            // An appointment or a meeting has a window of its own to be filled in.
            if let Some(opened) = windows::open_form(self, name, &params) {
                return opened;
            }
            self.dialog = dialogs::open_dialog(self, name, &params);
            // Folder Properties asks the engine first: when the engine refuses, there is no
            // dialog, and the command runs below so that the refusal is said as any command's is.
            if self.dialog.is_some() || !matches!(name, "folderProperties" | "wordCount") {
                return Ok(json!({"pending": name}));
            }
        }
        // A message not fetched yet is fetched first, and then the command goes on (R3.2); one that
        // can't be is said, as a refusal of the command's own would be.
        if let Some(r) = self.fetch_first(id, &params) {
            if let Err(e) = &r {
                self.say_refused(id, e);
            }
            return r;
        }
        if let Some(r) = files::intercept(self, id, &params) {
            return r;
        }
        // What can't be taken back is asked about first, once.
        // (An answer to an invitation in the mail has a question of its own.)
        if params.get("confirmed").and_then(Value::as_bool) != Some(true)
            && let Some(ask) = reading::invitation_question(self, id, &params).unwrap_or_else(|| dialogs::confirmation(self, id, &params))
        {
            let params = if params.is_object() { params } else { json!({}) };
            // What the question named is what goes ahead, though the selection changes while it
            // is open (#620).
            let params = dialogs::pin_targets(self, id, params);
            self.dialog = Some(dialogs::Dialog::Confirm { command: id.to_string(), params, ask });
            return Ok(json!({"pending": "confirm"}));
        }
        let opening_file = (id == "file.open").then(|| params.get("path").and_then(Value::as_str).map(str::to_string)).flatten();
        // Check for Updates pressed by the owner checks whether the daily check is on or not.
        // So are Restart to update and Go back, which are the owner's clicks alone.
        let r = match id {
            updates::CHECK => self.check_for_updates(true),
            updates::RESTART => self.restart_to_update(),
            updates::GO_BACK => self.go_back_a_version(),
            // Another mailbox file is read in the background, and opens once it is.
            "file.open" if opening::reads_ahead(self, &params) => opening::read_ahead(self, &params),
            _ => self.execute(id, params),
        };
        // Sent from a shared mailbox (#70): pressing Send is the owner asking for it to go.
        if id == "compose.send"
            && let Ok(v) = &r
            && let Some(sent) = v.get("id").and_then(Value::as_u64)
        {
            self.shared_send_pressed_in(sent);
        }
        if id.starts_with("calendar.") {
            self.ran = (self.ran.0.wrapping_add(1), id.to_string(), r.clone());
        }
        // A mailbox file the owner opened is a recent one once it is open, not while it is still
        // being read or waits for a save; and File closes then.
        if let (Some(path), Ok(v)) = (&opening_file, &r)
            && v.get("pending").is_none()
        {
            files::remember_recent(self, path);
        }
        match &r {
            Err(e) => self.say_refused(id, e),
            Ok(v) => {
                if let Some(msg) = feedback(id, v).or_else(|| self.stays_here_note(id, v)) {
                    self.status(msg);
                }
            }
        }
        // And closing a conversation, which may select its row (#503).
        if keys::MOVES_IN_LIST.contains(&id) || id == "mail.collapseConversation" {
            self.reveal_selection = true;
        }
        // What was done to the text being written leaves the keyboard in it.
        if r.is_ok()
            && (id.starts_with("format.")
                || matches!(id, "compose.link" | "compose.undo" | "compose.redo" | "edit.paste" | "compose.formatHtml" | "compose.formatPlain"))
        {
            self.body_focus_wanted = true;
        }
        r
    }

    /// Command `id` was refused with `e`: said in the status bar, in the interface language (the
    /// engine's refusal, its template filled; any other text looked up whole), and, when the
    /// owner has just done it by hand, in a message box too, as Outlook does (#152).
    fn say_refused(&mut self, id: &str, e: &str) {
        let said = match self.last_error.take() {
            Some(err) if err.to_string() == *e => err.in_lang(i18n::current()),
            _ => tl!(e).to_string(),
        };
        if self.by_hand && refusal_worth_a_box(id, e) {
            self.refused = Some(said.clone());
        }
        self.status(said);
    }

    /// Run a command for a script or an agent (control channel, MCP bridge): never asks first.
    /// UI-level commands (`ui.*`) are handled here, the rest by the engine.
    pub fn execute(&mut self, id: &str, params: Value) -> Result<Value, String> {
        if self.opening.is_some() {
            return Err(opening::STILL_OPENING.into());
        }
        // Whatever replaces the mailbox gives up one being read in the background, which would
        // otherwise take the place of this one later.
        if matches!(id, "file.open" | "file.new") {
            opening::cancel_ahead(self);
        }
        if let Some(r) = self.ui_command(id, &params) {
            return r;
        }
        let composing = self.session.compose.is_some();
        // An account that goes takes what was remembered for it along.
        let leaving = if id == "account.remove" {
            params.get("id").and_then(Value::as_u64).and_then(|a| self.session.mailbox.account(a)).cloned()
        } else {
            None
        };
        let edited =
            if id == "account.edit" { params.get("id").and_then(Value::as_u64).and_then(|a| self.session.mailbox.account(a)).cloned() } else { None };
        // Undo and Redo may put an account's servers back as they were before an edit.
        let keys = |a: &mailcraft_model::Account| ["password", "refresh"].map(|k| Self::secret_key(k, a));
        let undone =
            matches!(id, "edit.undo" | "edit.redo").then(|| self.session.mailbox.accounts.iter().map(|a| (a.id, keys(a))).collect::<Vec<_>>());
        if let Some(r) = self.through_saver(id, &params) {
            return r;
        }
        let r = self.session.run(id, &params);
        self.drop_secrets_if_replaced();
        self.last_error = r.as_ref().err().cloned();
        let r = r.map_err(|e| e.to_string());
        // Asked for the reminders: their window comes up, due or not.
        if r.is_ok() && id == "view.reminders" {
            self.reminders.asked_for = true;
        }
        // A calendar opened by address (#70), from the dialog or a script: read at once.
        if let (Ok(opened), "calendar.openShared") = (&r, id)
            && let Some(account) = opened.get("account").and_then(Value::as_u64)
        {
            self.read_opened_now(account);
        }
        // The copy of a shared mailbox's calendar shown again (#70): the account's list brings it
        // back at once.
        if let (Ok(shown), "calendar.hideListedCopy") = (&r, id)
            && shown.get("hidden").and_then(Value::as_bool) == Some(false)
            && let Some(account) = shown.get("account").and_then(Value::as_u64)
        {
            // (Kept both when asked: it is here already, and nothing need be asked.)
            let address = shown.get("address").and_then(Value::as_str).unwrap_or_default();
            let here = self
                .session
                .mailbox
                .calendars
                .iter()
                .any(|c| c.origin.account == account && c.owner.as_ref().is_some_and(|o| o.address.trim().eq_ignore_ascii_case(address)));
            if !here {
                self.read_opened_now(account);
            }
        }
        // Outlook's Favorites are wanted again: they are read at once.
        if r.is_ok() && id == "view.favorites" && params.get("own").and_then(Value::as_bool) == Some(false) {
            for account in self.session.mailbox.accounts.iter().map(|a| a.id).collect::<Vec<_>>() {
                self.favorites_due(account);
            }
        }
        if let (Ok(_), Some(gone)) = (&r, leaving) {
            let account = gone.id;
            // What the window keeps for it (the command took what came from its server).
            self.forget_favorites(account);
            self.graph_tokens.remove(&account);
            self.graph_checked.remove(&account);
            self.graph_consent_needed.remove(&account);
            self.passwords.remove(&account);
            self.tokens.remove(&account);
            self.jmap_refused.remove(&account);
            self.ews_unreadable.remove(&account);
            self.ews_signed_in.remove(&account);
            self.ews_certificate.remove(&account);
            self.forget_account_secrets(&gone);
        }
        // Servers changed: what was kept for the old ones is not given to the new, and the owner
        // is asked again.
        if let (Ok(_), Some(before)) = (&r, edited)
            && let Some(after) = self.session.mailbox.account(before.id).cloned()
            && ["password", "refresh"].into_iter().any(|k| Self::secret_key(k, &before) != Self::secret_key(k, &after))
        {
            self.forget_account_secrets(&before);
            self.passwords.remove(&before.id);
            self.tokens.remove(&before.id);
            self.graph_tokens.remove(&before.id);
            self.jmap_refused.remove(&before.id);
            self.ews_signed_in.remove(&before.id);
            self.ews_certificate.remove(&before.id);
        }
        // Likewise when Undo or Redo changed them: what was typed for the ones it had goes to none
        // of the others.
        if let (Ok(_), Some(before)) = (&r, undone) {
            for (account, was) in before {
                if self.session.mailbox.account(account).map(keys) != Some(was) {
                    self.passwords.remove(&account);
                    self.tokens.remove(&account);
                    self.graph_tokens.remove(&account);
                    self.jmap_refused.remove(&account);
                }
            }
        }
        // Starting to write brings up the Message tab; finishing goes back to Home.
        if r.is_ok() && composing != self.session.compose.is_some() {
            self.compose_bcc = false;
            self.ui.tab = if composing { "Home".into() } else { "Message".into() };
            self.ui.backstage = false;
        }
        r
    }

    /// Open a message from outside the message list (a task, a contact's recent mail): in Mail, in
    /// its folder's All tab with no search, the list scrolled to it (#485).
    pub(crate) fn open_in_mail(&mut self, id: Id) {
        self.ui.backstage = false;
        let _ = self.run("mail.select", json!({"id": id, "elsewhere": true}));
        let _ = self.run("view.mail", json!({}));
        self.reveal_selection = true;
    }

    fn ui_command(&mut self, id: &str, p: &Value) -> Option<Result<Value, String>> {
        let s = |k: &str| p.get(k).and_then(Value::as_str);
        let b = |k: &str| p.get(k).and_then(Value::as_bool);
        Some(match id {
            "ui.removeAllData" => self.remove_all_data(),
            "ui.openDataFolder" => self.open_data_folder(),
            "ui.confirmServers" => self.confirm_servers(),
            "ui.dismissSettingsNotice" => settings_notice::dismiss(self),
            "ui.tab" => {
                let Some(tab) = s("tab") else { return Some(Err("missing `tab`".into())) };
                if tab == "File" {
                    if !self.ui.backstage {
                        self.ui.backstage_page = "info".into();
                    }
                    self.ui.backstage = true;
                } else if ribbon::tabs(self).contains(&tab) {
                    self.ui.tab = tab.to_string();
                    self.ui.backstage = false;
                    self.ui.ribbon_collapsed = false;
                } else {
                    return Some(Err(format!("no ribbon tab \"{tab}\"")));
                }
                Ok(json!({"tab": self.ui.tab}))
            }
            "ui.backstage" => {
                let was_open = self.ui.backstage;
                self.ui.backstage = b("value").unwrap_or(!self.ui.backstage);
                // As in Outlook, File opens on Info; a page is kept only while it stays open.
                if self.ui.backstage && !was_open {
                    self.ui.backstage_page = "info".into();
                }
                if let Some(page) = s("page") {
                    if !backstage::PAGES.iter().any(|(code, _, _)| *code == page) {
                        return Some(Err(format!("no File page \"{page}\"")));
                    }
                    self.ui.backstage_page = page.to_string();
                }
                Ok(json!({"value": self.ui.backstage, "page": self.ui.backstage_page}))
            }
            "ui.lightMessages" => {
                self.ui.light_messages = b("value").unwrap_or(!self.ui.light_messages);
                Ok(json!({"value": self.ui.light_messages}))
            }
            "ui.collapseRibbon" => {
                self.ui.ribbon_collapsed = b("value").unwrap_or(!self.ui.ribbon_collapsed);
                Ok(json!({"value": self.ui.ribbon_collapsed}))
            }
            "ui.ribbonMode" => match s("value").and_then(ribbon::Mode::from_code) {
                Some(mode) => {
                    self.ui.ribbon_mode = mode;
                    Ok(json!({"value": mode.code()}))
                }
                None => Err("`value` is simplified or classic".into()),
            },
            "ui.dialog" => {
                let Some(name) = s("name") else { return Some(Err("missing `name`".into())) };
                // Send/Receive Progress is a panel in the window, not a dialog; the problems are
                // in it, under each account.
                if let Some(panel) = sync_panel::Panel::named(name) {
                    self.sync_panel = Some(panel);
                    return Some(Ok(json!({"panel": "syncProgress"})));
                }
                match dialogs::open_dialog(self, name, p) {
                    Some(d) => {
                        self.dialog = Some(d);
                        self.sync_panel = None;
                        Ok(json!({"dialog": name}))
                    }
                    None => Err(format!("can't open the \"{name}\" dialog now")),
                }
            }
            "ui.closeDialog" => {
                self.dialog = None;
                self.sync_panel = None;
                Ok(json!({}))
            }
            "ui.theme" => match s("value").and_then(theme::Appearance::from_code) {
                Some(a) => {
                    self.ui.theme = a;
                    Ok(json!({"value": a.code()}))
                }
                None => Err("`value` is light, dark or system".into()),
            },
            "ui.language" => {
                let code = s("language").unwrap_or_default();
                let lang = i18n::Lang::from_code(code);
                if code.trim().eq_ignore_ascii_case(i18n::AUTO) {
                    self.ui.language = i18n::AUTO.into();
                } else if let Some(lang) = lang.filter(|l| *l == i18n::Lang::Pseudo || i18n::Lang::available().contains(l)) {
                    self.ui.language = lang.code().into();
                } else {
                    let codes: Vec<&str> = i18n::Lang::available().iter().map(|l| l.code()).collect();
                    return Some(Err(format!("`language` is auto or one of: {}", codes.join(", "))));
                }
                self.apply_language();
                Ok(json!({"language": self.ui.language, "inForce": self.language().code()}))
            }
            "ui.interfaceSize" => match p.get("value").and_then(Value::as_f64).and_then(theme::InterfaceSize::from_percent) {
                Some(size) => {
                    self.ui.interface_size = size;
                    Ok(json!({"value": size.percent()}))
                }
                None => Err("`value` is a percentage from 80 to 200".into()),
            },
            "ui.advancedSearch" => advanced_search::command(self, p),
            "ui.focusSearch" => {
                self.search_focus = true;
                self.ui.backstage = false;
                // People searches its own contacts; Calendar's and Tasks' box searches the mail (#555).
                if self.session.view.module != mailcraft_engine::Module::People {
                    let _ = self.session.run("view.mail", &json!({}));
                }
                Ok(json!({}))
            }
            "ui.copyDiagnostics" => {
                let text = report_problem::diagnostics(self);
                if let Some(ctx) = &self.ctx {
                    ctx.copy_text(text.clone());
                }
                self.status(tl!("Diagnostics copied. Nothing was sent."));
                Ok(json!({"text": text}))
            }
            "ui.sync" => self.start_sync(p),
            "ui.checkCalendar" => self.check_calendar(p),
            "ui.microsoftClientId" => {
                let id = s("value").unwrap_or_default().trim();
                if id.len() > 200 || !id.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c)) {
                    return Some(Err(tl!("An application ID is letters, digits and dashes").to_string()));
                }
                self.ui.microsoft_client_id = id.to_string();
                Ok(json!({"value": id}))
            }
            "ui.rememberSignIn" => {
                let on = p.get("value").and_then(Value::as_bool).unwrap_or(!self.ui.remember_sign_in);
                self.ui.remember_sign_in = on;
                if !on {
                    // Switched off: what was kept goes too.
                    let left = self.forget_every_secret();
                    if !left.is_empty() {
                        self.status(tln!(
                            left.len(),
                            "{n} remembered sign-in could not be removed from the credential store",
                            "{n} remembered sign-ins could not be removed from the credential store"
                        ));
                    }
                }
                Ok(json!({"value": on}))
            }
            "ui.desktopAlerts" => {
                let (mail, reminders) = (p.get("newMail"), p.get("reminders"));
                if mail.is_none() && reminders.is_none() {
                    return Some(Err(tl!("Say which: `newMail`, `reminders` (true or false)").to_string()));
                }
                let (Some(mail), Some(reminders)) =
                    (mail.map_or(Some(self.ui.alert_new_mail), Value::as_bool), reminders.map_or(Some(self.ui.alert_reminders), Value::as_bool))
                else {
                    return Some(Err(tl!("`newMail` and `reminders` are true or false").to_string()));
                };
                (self.ui.alert_new_mail, self.ui.alert_reminders) = (mail, reminders);
                Ok(json!({"newMail": mail, "reminders": reminders}))
            }
            "ui.dismissReminder" => {
                let one = p.get("event").and_then(Value::as_u64).zip(s("occurrence"));
                if one.is_none() && p.get("all").and_then(Value::as_bool) != Some(true) {
                    return Some(Err(tl!("Say which reminder (`event` and `occurrence`), or `all`").to_string()));
                }
                Ok(json!({"dismissed": reminders::dismiss(self, one)}))
            }
            "ui.snoozeReminders" => {
                let Some(minutes) = p.get("minutes").and_then(Value::as_u64).filter(|m| (1..=10_080).contains(m)) else {
                    return Some(Err(tl!("`minutes` is 1 to 10080").to_string()));
                };
                let one = p.get("event").and_then(Value::as_u64).zip(s("occurrence"));
                Ok(json!({"snoozed": reminders::snooze(self, one, minutes), "minutes": minutes}))
            }
            "ui.keepOffline" => self.keep_offline(p),
            "ui.connectMicrosoft" => {
                let account = p
                    .get("account")
                    .and_then(Value::as_u64)
                    .or_else(|| self.session.mailbox.accounts.iter().find(|a| a.oauth.is_some()).map(|a| a.id))
                    .and_then(|id| self.session.mailbox.account(id).cloned());
                let Some(account) = account else { return Some(Err(tl!("No account signs in with Microsoft").to_string())) };
                let Some(oauth) = account.oauth.clone() else { return Some(Err(tl!("This account doesn't sign in with Microsoft").to_string())) };
                self.begin_sign_in(account.id, oauth, &account.email, String::new(), false, true)
                    .map(|()| json!({"pending": "signIn", "account": account.id}))
            }
            "ui.allowSharedCalendars" => {
                if p.get("confirmed").and_then(Value::as_bool) != Some(true) {
                    return Some(Err(tl!("Say that this is meant with \"confirmed\": true.").to_string()));
                }
                let Some(shared) = p.get("account").and_then(Value::as_u64) else { return Some(Err(tl!("`account` is required").to_string())) };
                if self.session.mailbox.account(shared).is_some_and(mailcraft_model::Account::is_shared) {
                    self.begin_shared_calendars_sign_in(shared)
                } else {
                    self.begin_opened_calendars_sign_in(shared)
                }
            }
            "ui.allowTasks" => {
                if p.get("confirmed").and_then(Value::as_bool) != Some(true) {
                    return Some(Err(tl!("Say that this is meant with \"confirmed\": true.").to_string()));
                }
                let Some(account) = p.get("account").and_then(Value::as_u64) else { return Some(Err(tl!("`account` is required").to_string())) };
                self.begin_tasks_sign_in(account)
            }
            "ui.allowSharedSend" => {
                let Some(shared) = p.get("account").and_then(Value::as_u64) else { return Some(Err(tl!("`account` is required").to_string())) };
                self.begin_shared_send_sign_in(shared)
            }
            "ui.moreResultsOnServer" => {
                if !self.enabled("ui.moreResultsOnServer") {
                    return Some(Err(tl!("More results on the server can't be asked for now").to_string()));
                }
                self.more_on_server();
                Ok(json!({"pending": "serverSearch"}))
            }
            "ui.cancelOpen" => {
                if !opening::cancel_ahead(self) {
                    return Some(Err(tl!("No mailbox is being opened").to_string()));
                }
                Ok(json!({"cancelled": true}))
            }
            "ui.cancelSync" => {
                if !self.syncing() {
                    return Some(Err(tl!("No Send/Receive is running").to_string()));
                }
                // The run stops at the next safe place; what it brought stays. The rest of a
                // round that was under way is called off with it.
                self.services.sync_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                self.pause_fetch();
                self.sync_queue.clear();
                self.inbox_queue.clear();
                // (Nor does a run called off for something else go on after.)
                if let Some((_, _, called_off, _)) = self.sync_run.as_mut() {
                    *called_off = false;
                }
                // (Send/Receive Progress: the accounts still to go are stopped with it.)
                let waiting: Vec<Id> = self
                    .session
                    .sync_progress
                    .accounts
                    .iter()
                    .filter(|r| r.state == mailcraft_engine::cmd::sync_progress::RunState::Waiting)
                    .map(|r| r.account)
                    .collect();
                for account in waiting {
                    self.session.sync_progress.end(account, true);
                }
                self.session.sync_status = tl!("Stopping…").to_string();
                Ok(json!({"cancelling": true}))
            }
            "ui.skipSync" => self.skip_sync(p),
            "ui.cancelSignIn" => match self.signing_in.take() {
                Some(s) => {
                    s.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    self.session.sync_status = tl!("Sign-in cancelled").to_string();
                    Ok(json!({"cancelled": s.account}))
                }
                // (An administrator's approval that is waited for is called off the same way.)
                None => match self.cancel_admin_consent() {
                    Some(account) => {
                        self.session.sync_status = tl!("Sign-in cancelled").to_string();
                        Ok(json!({"cancelled": account}))
                    }
                    None => Err(tl!("Nobody is signing in").to_string()),
                },
            },
            "ui.open" => {
                files::open(self, "mailbox");
                Ok(json!({"pending": "open"}))
            }
            "ui.import" => {
                files::open(self, "import");
                Ok(json!({"pending": "import"}))
            }
            "ui.attach" => {
                if self.session.compose.is_none() {
                    return Some(Err("no message is being written".into()));
                }
                files::open(self, "attach");
                Ok(json!({"pending": "attach"}))
            }
            "ui.toggleFolder" => {
                let Some(id) = p.get("id").and_then(Value::as_u64) else { return Some(Err("missing `id`".into())) };
                match self.ui.collapsed_folders.iter().position(|x| *x == id) {
                    Some(i) => {
                        self.ui.collapsed_folders.remove(i);
                    }
                    None => self.ui.collapsed_folders.push(id),
                }
                Ok(json!({"collapsed": self.ui.collapsed_folders.contains(&id)}))
            }
            _ => {
                return windows::ui_command(self, id, p)
                    .or_else(|| reading::remote::ui_command(self, id, p))
                    .or_else(|| calendar_view::ui_command(self, id, p))
                    .or_else(|| meeting_dialogs::ui_command(self, id, p))
                    .or_else(|| favorites_sync::ui_command(self, id, p))
                    .or_else(|| approval::ui_command(self, id, p))
                    .or_else(|| graph_switch::ui_command(self, id, p))
                    .or_else(|| updates::ui_command(self, id, p))
                    .or_else(|| attachment_open::ui_command(self, id, p))
                    .or_else(|| report_problem::ui_command(self, id, p))
                    .or_else(|| sender_images::ui_command(self, id, p));
            }
        })
    }

    /// The name a secret is kept under in the system's credential store: its kind (`password` or
    /// `refresh`), the account's address and the servers it goes to, so that an account that
    /// names other servers (a mailbox from a file, a changed setting) is not given the secret.
    /// A Microsoft account kept in step through Graph goes to Microsoft alone: its sign-in page
    /// and Graph are fixed (a shared mailbox is such an account too; one of another kind that a
    /// file calls shared is not, and gets the name of its servers, never the owner's token).
    /// The one place this name is made.
    pub(crate) fn secret_key(kind: &str, a: &mailcraft_model::Account) -> String {
        let address = a.email.trim().to_ascii_lowercase();
        let server = |s: &Option<mailcraft_model::Server>| match s {
            Some(s) => format!("{}:{}", s.host.trim().to_ascii_lowercase(), s.port),
            None => "none".to_string(),
        };
        if a.kind == mailcraft_model::AccountKind::Microsoft {
            format!("{kind}:{address}|graph")
        } else if a.kind == mailcraft_model::AccountKind::Jmap {
            // (Its whole session address: another one, on the same server or not, is another server;
            // and a bearer token is no password.)
            let (session, bearer) = a.jmap.as_ref().map_or(("none", false), |j| (j.session.trim(), j.bearer));
            format!("{kind}:{address}|jmap={session}|bearer={bearer}")
        } else if a.kind == mailcraft_model::AccountKind::Exchange {
            // (Its whole EWS address: another one, on the same server or not, is another server;
            // and the user name it signs in with: a password is for one name.)
            let url = a.ews.as_ref().map_or("none", |e| e.url.trim());
            let user = a.ews.as_ref().map_or_else(String::new, |e| e.username.trim().to_ascii_lowercase());
            format!("{kind}:{address}|ews={url}|user={user}")
        } else {
            format!("{kind}:{address}|imap={}|smtp={}", server(&a.incoming), server(&a.outgoing))
        }
    }

    /// The name secrets were kept under before they were bound to servers.
    pub(crate) fn legacy_secret_key(kind: &str, address: &str) -> String {
        format!("{kind}:{}", address.trim().to_ascii_lowercase())
    }

    /// The account in this mailbox with this address.
    fn account_at(&self, address: &str) -> Option<&mailcraft_model::Account> {
        let address = address.trim();
        self.session.mailbox.accounts.iter().find(|a| a.email.trim().eq_ignore_ascii_case(address))
    }

    /// What the owner had the operating system remember for `address` (`password` or `refresh`),
    /// for the account of that address with the servers it has now.
    fn remembered(&self, kind: &str, address: &str) -> Option<String> {
        if !self.ui.remember_sign_in {
            return None;
        }
        // (What the background read brought, never the store itself: it may be locked, and then
        // asking blocks until the owner answers.)
        self.secret_cache.get(&Self::secret_key(kind, self.account_at(address)?)).cloned()
    }

    /// Have the operating system remember a secret for `address`, when the owner wants that.
    fn remember(&mut self, kind: &str, address: &str, secret: &str) {
        if !self.ui.remember_sign_in || secret.is_empty() || self.data_removed {
            return;
        }
        let Some(account) = self.account_at(address) else { return };
        let key = Self::secret_key(kind, account);
        let Some(store) = &self.services.secrets else { return };
        match store.set(&key, secret) {
            Ok(()) => {
                self.secret_cache.insert(key.clone(), secret.to_string());
                self.index_secret_key(key);
            }
            Err(e) => log::warn!("couldn't keep the sign-in for next time: {e}"),
        }
    }

    /// Note the name of an entry put in the credential store (the name alone, in `ui.json`).
    fn index_secret_key(&mut self, key: String) {
        // (A bound: a mailbox's accounts are few, and a file can't make this list grow.)
        if self.ui.remembered_keys.len() < 1000 && !self.ui.remembered_keys.contains(&key) {
            self.ui.remembered_keys.push(key);
        }
    }

    /// Delete every entry MailCraft put in the credential store: those noted in `remembered_keys`
    /// (whichever mailbox they were for), those of this mailbox's accounts, and the old names.
    /// Gives the names of the ones that are still there afterwards.
    pub(crate) fn forget_every_secret(&mut self) -> Vec<String> {
        let mut keys: Vec<String> = std::mem::take(&mut self.ui.remembered_keys);
        for a in &self.session.mailbox.accounts {
            for kind in ["password", "refresh"] {
                keys.push(Self::secret_key(kind, a));
                keys.push(Self::legacy_secret_key(kind, &a.email));
            }
        }
        keys.sort();
        keys.dedup();
        let Some(store) = &self.services.secrets else { return Vec::new() };
        let mut left = Vec::new();
        for key in keys {
            store.delete(&key);
            if store.get(&key).is_some() {
                left.push(key);
            }
        }
        self.ui.remembered_keys = left.clone();
        self.secret_cache.clear();
        left
    }

    /// Tell the owner, once a session, when the system's credential store can't be used, so that
    /// being asked again at the next start is no surprise.
    pub(crate) fn check_secret_store(&mut self) {
        if let Some(trouble) = self.services.secrets.as_ref().and_then(|store| store.take_trouble()) {
            self.note_secret_trouble(trouble);
        }
    }

    fn note_secret_trouble(&mut self, trouble: StoreTrouble) {
        if self.secrets_trouble.is_none() {
            self.secrets_trouble = Some(trouble);
            self.status(trouble.notice());
        }
    }

    /// Drop everything the operating system remembers for this account as it is, and under the
    /// name from before secrets were bound to servers; and the names of the entries that went
    /// from `remembered_keys`, so that the settings don't name the account any more (#325). A
    /// name whose entry is still there stays, for the full removal to try again; so does every
    /// name when the store couldn't be used (locked, or not answering), which says nothing then.
    pub(crate) fn forget_account_secrets(&mut self, a: &mailcraft_model::Account) {
        let Some(store) = &self.services.secrets else { return };
        let mut gone = Vec::new();
        for kind in ["password", "refresh"] {
            for key in [Self::secret_key(kind, a), Self::legacy_secret_key(kind, &a.email)] {
                store.delete(&key);
                if store.get(&key).is_none() {
                    gone.push(key);
                }
            }
        }
        for key in &gone {
            self.secret_cache.remove(key);
        }
        // (Taken here, the owner hears of it as `check_secret_store` would have said it.)
        match store.take_trouble() {
            Some(trouble) => self.note_secret_trouble(trouble),
            None => self.ui.remembered_keys.retain(|key| !gone.contains(key)),
        }
    }

    /// The sign-in settings as the host should use them: an account that names no application id
    /// of its own takes this installation's.
    pub(crate) fn effective_oauth(&self, mut oauth: mailcraft_model::OAuth) -> mailcraft_model::OAuth {
        if oauth.client_id.is_empty() && oauth.provider == mailcraft_model::OAuthProvider::Microsoft {
            let own = &self.ui.microsoft_client_id;
            oauth.client_id = if own.is_empty() { self.services.microsoft_client_id.clone() } else { own.clone() };
        }
        oauth
    }

    /// Hand a sign-in to the host. `account` is 0 while an account is being set up.
    fn begin_sign_in(
        &mut self,
        account: Id,
        oauth: mailcraft_model::OAuth,
        email: &str,
        refresh: String,
        quiet: bool,
        graph: bool,
    ) -> Result<(), String> {
        let mail_only = std::mem::take(&mut self.mail_only_next);
        let favorites = std::mem::take(&mut self.favorites_next) && graph;
        let shared = self.shared_next.take().filter(|_| graph);
        let shared_send = std::mem::take(&mut self.shared_send_next) && shared.is_some();
        // (Alone with `graph`: for the calendars opened by address; with a shared mailbox: for its.)
        let shared_calendars = std::mem::take(&mut self.shared_calendars_next) && graph;
        let tasks = std::mem::take(&mut self.tasks_next) && graph && shared.is_none();
        if self.signing_in.is_some() || self.approving() {
            return Err(tl!("Finish signing in first (your browser is waiting)").to_string());
        }
        let Some(sign_in) = &self.services.sign_in else {
            return Err(tl!("This build can't sign in to this kind of account. Use the desktop app.").to_string());
        };
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let browser = refresh.is_empty();
        let waiting = if browser { tl!("Sign in on the page that opened in your browser…") } else { tl!("Renewing the sign-in…") };
        let request = SignInRequest {
            account,
            oauth: self.effective_oauth(oauth),
            email: email.to_string(),
            refresh,
            cancel: cancel.clone(),
            quiet,
            graph,
            mail_only,
            favorites,
            // An account kept in step through Graph needs its mail read and changed there too.
            mail_write: graph
                && (account == 0 || self.session.mailbox.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Microsoft)),
            shared: shared.is_some(),
            shared_send,
            shared_calendars,
            tasks,
        };
        let rx = sign_in(request);
        let opening = self.session.openings();
        self.signing_in =
            Some(SigningIn { graph, favorites, quiet, account, shared, shared_send, shared_calendars, tasks, browser, opening, rx, cancel });
        self.session.sync_status = waiting.to_string();
        Ok(())
    }

    /// Account setup: sign the owner in on the provider's page before the account exists. The
    /// answer is picked up with [`take_setup_sign_in`](Self::take_setup_sign_in).
    pub(crate) fn begin_setup_sign_in(&mut self, oauth: mailcraft_model::OAuth, email: &str, mail_only: bool) -> Result<(), String> {
        self.setup_sign_in = None;
        self.mail_only_next = mail_only;
        // A Microsoft account is kept in step through Graph (R3): its sign-in is for Graph, with
        // its mail, rather than for IMAP and SMTP. (Unless the owner asked for mail only: that
        // is IMAP's until IMAP is turned off for Microsoft accounts.)
        let graph = oauth.provider == mailcraft_model::OAuthProvider::Microsoft && !mail_only;
        self.begin_sign_in(0, oauth, email, String::new(), false, graph)
    }

    /// Account setup: what the sign-in started with `begin_setup_sign_in` came to, once it has.
    pub(crate) fn take_setup_sign_in(&mut self) -> Option<Result<Tokens, String>> {
        self.setup_sign_in.take()
    }

    /// Account setup: the account exists now; keep what signing in brought back for it.
    pub(crate) fn adopt_tokens(&mut self, account: Id, tokens: Tokens) {
        if self.signed_in_as_other(account, &tokens) {
            return;
        }
        if let Some(address) = self.session.mailbox.account(account).map(|a| a.email.clone()) {
            self.remember("refresh", &address, &tokens.refresh);
        }
        self.note_directory(account, &tokens.tenant);
        self.tokens.insert(account, tokens);
    }

    /// Whether tokens that came back for `account`, an account that exists, are somebody
    /// else's: the provider named who signed in, and that is neither the account's address nor
    /// its user name (#103). They are then not to be used or remembered, and the owner is told
    /// who to sign in as. Tokens that name nobody are the account's, as they always were.
    pub(crate) fn signed_in_as_other(&mut self, account: Id, tokens: &Tokens) -> bool {
        let Some(a) = self.session.mailbox.account(account) else { return false };
        if !tokens.someone_else(&a.email, &a.username) {
            return false;
        }
        let who = tokens.email.trim();
        // (Said on screen; never written to the log, which names nobody.)
        let said = i18n::fmt(
            tl!("You signed in as {signedIn}, but this account is {account}. Sign in again as {account}."),
            &[("signedIn", &forms::kit::shown(who, 120)), ("account", &forms::kit::shown(a.email.trim(), 120))],
        );
        self.session.sync_status.clear();
        self.status(said);
        true
    }

    /// Account setup: the password that was just shown to work, for this session's Send/Receive
    /// (it is remembered for later runs once the first one has gone through).
    pub(crate) fn adopt_password(&mut self, account: Id, password: String) {
        self.jmap_refused.remove(&account);
        self.passwords.insert(account, password);
    }

    /// Start a Send/Receive for an account (default: the first one with a server). Asks for the
    /// password the first time, or has the owner sign in on the provider's page; the answer is
    /// kept in memory until the app closes, and by the operating system when that is wanted.
    fn start_sync(&mut self, p: &Value) -> Result<Value, String> {
        self.drop_secrets_if_replaced();
        // A mailbox from a file: the owner confirms where it connects to first.
        if self.unconfirmed {
            if p.get("quiet").and_then(Value::as_bool) == Some(true) {
                return Err(tl!("This mailbox was opened from a file: confirm its servers first").to_string());
            }
            return self.ask_to_confirm_servers();
        }
        if self.data_removed {
            return Err(tl!("MailCraft's data has been removed: nothing is sent or received").to_string());
        }
        // The Inbox checked by itself gives way to a run (#276).
        self.give_up_inbox_check();
        if self.sync_rx.is_some() || self.graph_rx.is_some() {
            return Err(tl!("Send/Receive is already running").to_string());
        }
        let cant_connect = || {
            tl!("This build can't connect to mail servers (a browser can't open mail connections). Use the desktop app, or bring mail in with File › Import.")
                .to_string()
        };
        if !self.services.can_sync() {
            return Err(cant_connect());
        }
        let quiet = p.get("quiet").and_then(Value::as_bool).unwrap_or(false);
        // Send/Receive the owner asked for is the time to ask the store again (it may have been
        // locked, or refused): the run goes on when it has answered ([`secret_read`]).
        if !quiet && self.hold_for_secrets(p) {
            return Ok(json!({"pending": "secrets"}));
        }
        // Never two jobs for one account at once: the calendar's job (a short one, but the
        // wide stretch every quarter of an hour is not) finishes first.
        let target = p.get("account").and_then(Value::as_u64).or_else(|| self.server_accounts().first().copied());
        if target.is_some_and(|account| self.calendar_job_for(account)) {
            return Err(tl!("The calendar is being updated already.").to_string());
        }
        // A run starts, the owner's or one by itself: full messages paused by Stop go on.
        self.resume_fetch();
        // Send/Receive the owner asked for: the calendar and contacts follow the round.
        if !quiet {
            let accounts: Vec<Id> =
                target.into_iter().chain(p.get("account").is_none().then(|| self.server_accounts()).into_iter().flatten()).collect();
            for account in accounts {
                self.calendar_after_round(account);
            }
        }
        let account = match p.get("account").and_then(Value::as_u64) {
            Some(account) => account,
            // Nobody in particular: every account with a server, one after the other.
            None => {
                let round = self.server_accounts();
                let none = || tl!("No account is connected to a mail server (File › Add Account)").to_string();
                if round.is_empty() {
                    return Err(none());
                }
                // One run of them all, in this order: the rest wait their turn.
                self.sync_queue = round.iter().map(|a| (*a, quiet)).collect();
                self.session.sync_progress.begin(&round);
                // The Inbox of every account kept in step through Graph comes first (#276).
                self.queue_inbox_passes(&round, quiet);
                if let Some(started) = self.start_next_inbox_pass() {
                    return Ok(started);
                }
                self.sync_queue.pop_front().map(|(first, _)| first).ok_or_else(none)?
            }
        };
        // An Exchange server account is kept in step through Exchange Web Services (#252), where the
        // switch is on and the host runs it.
        if self.session.mailbox.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Exchange) {
            if !self.session.ews {
                return Err(mailcraft_engine::cmd::ews_mail::SWITCHED_OFF.to_string());
            }
            return self.start_ews_mail(account, quiet, p);
        }
        // Nor has one kept in step over JMAP, where the host runs JMAP (M6).
        if self.services.jmap.is_some() && self.session.mailbox.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Jmap) {
            return self.start_jmap_mail(account, quiet, p);
        }
        if self.services.sync.is_none() {
            return Err(cant_connect());
        }
        // An account kept in step through Microsoft Graph has no IMAP leg at all (R3).
        if self.session.mailbox.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Microsoft) {
            let after_sign_in = p.get("afterSignIn").and_then(Value::as_bool).unwrap_or(false);
            let started = self.start_graph_mail(account, quiet);
            self.graph_after_sign_in = (after_sign_in && started.as_ref().is_ok_and(|v| v.get("started").is_some())).then_some(account);
            return started;
        }
        let mut request = mailcraft_engine::cmd::account::sync_request(&self.session, account).map_err(|e| e.to_string())?;
        // A run that only tells the server what changed here, when that is all it was started for.
        request.push_only = self.push_only_next.remove(&account) && quiet && request.outbox.is_empty();
        let address = request.account.email.clone();
        if let Some(oauth) = request.account.oauth.clone() {
            // Signed in on the provider's page: a token that is still good goes along; without
            // one the host renews it or the owner signs in, and this runs again after.
            let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
            let refresh = kept.or_else(|| self.remembered("refresh", &address)).unwrap_or_default();
            // Good for the whole run: Microsoft ends an IMAP session the moment its token runs
            // out, folders still to go. ponytail: ten minutes covers a run; a longer first
            // download still meets the end, and the next run renews.
            match self.tokens.get(&account).filter(|t| t.fresh(mailcraft_time::now() + 600)).map(|t| t.access.clone()) {
                Some(access) => request.access_token = access,
                None => {
                    self.begin_sign_in(account, oauth, &address, refresh, quiet, false)?;
                    return Ok(json!({"pending": "signIn", "account": account}));
                }
            }
            // Microsoft Graph first, for what it does better than the mail protocols: sending
            // (the answer says what it sent, and this runs again for the mail itself). The
            // calendar and contacts are not part of it: they come in a job of their own after
            // the round ([`calendar_sync`]), so that they never hold up the mail (#276).
            let after_graph = p.get("afterGraph").and_then(Value::as_bool).unwrap_or(false);
            let microsoft = oauth.provider == mailcraft_model::OAuthProvider::Microsoft;
            if !after_graph
                && microsoft
                && !refresh.is_empty()
                && !self.graph_consent_needed.contains(&account)
                && self.services.graph.is_some()
                && !request.outbox.is_empty()
            {
                let access = self.graph_access_for(account, false);
                let ask = GraphRequest {
                    account: request.account.clone(),
                    oauth: self.effective_oauth(oauth),
                    refresh,
                    access,
                    outbox: request.outbox.clone(),
                    directory: false,
                    calendar: None,
                    changes: None,
                    asks: Vec::new(),
                    // (The calendar's own job asks for these: [`calendar_sync`].)
                    settings: false,
                    favorites: false,
                    favorites_scopes: false,
                    mail: None,
                    mail_pieces: None,
                    cancel: Default::default(),
                };
                self.session.sync_status = tl!("Sending…").to_string();
                self.session.sync_progress.phase(account, SyncPhase::Sending);
                if let Some(graph) = &self.services.graph {
                    self.graph_rx = Some((account, quiet, false, graph(ask)));
                }
                return Ok(json!({"pending": "graph", "account": account}));
            }
            // What Graph sent, or couldn't say whether it sent, is not sent again by SMTP.
            let skip: Vec<Id> = p.get("skip").and_then(Value::as_array).map(|ids| ids.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
            request.outbox.retain(|m| !skip.contains(&m.id));
        } else {
            if let Some(pw) = p.get("password").and_then(Value::as_str) {
                self.passwords.insert(account, pw.to_string());
            } else if !self.passwords.contains_key(&account)
                && let Some(pw) = self.remembered("password", &address)
            {
                self.passwords.insert(account, pw);
            }
            let Some(password) = self.passwords.get(&account).cloned() else {
                self.dialog = dialogs::Dialog::open("password", self, &json!({"account": account}));
                return Ok(json!({"pending": "password"}));
            };
            request.password = password;
        }
        let Some(start) = &self.services.sync else { return Err(tl!("This build can't connect to mail servers").to_string()) };
        self.sync_run = Some((account, None, false, quiet));
        self.sync_rx = Some(MailRun::Imap(start(request)));
        self.session.sync_status = tl!("Sending and receiving…").to_string();
        Ok(json!({"started": true, "account": account}))
    }

    /// A Send/Receive of an account kept in step through Microsoft Graph (R3): one Graph job
    /// sends the Outbox, makes the folder changes made here, lists the folders and, when they are
    /// due, brings the calendar and the contacts; no IMAP, no SMTP. Its token is always renewed
    /// for the job, with `Mail.ReadWrite`.
    fn start_graph_mail(&mut self, account: Id, quiet: bool) -> Result<Value, String> {
        if self.services.graph.is_none() {
            return Err(tl!("This build can't connect to Microsoft").to_string());
        }
        // A shared mailbox is read with its owner's sign-in, at its own address (#70).
        if self.session.mailbox.account(account).is_some_and(|a| a.is_shared()) {
            return self.start_shared_mail(account, quiet);
        }
        let owner = self.session.mailbox.account(account).cloned().ok_or_else(|| tl!("The account is gone").to_string())?;
        let oauth = owner.oauth.clone().ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        let address = owner.email.clone();
        let kept = self.tokens.get(&account).or(self.graph_tokens.get(&account)).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", &address)).unwrap_or_default();
        if refresh.is_empty() {
            // Nobody asked for this run: no page is put in front of the owner.
            if quiet {
                return Err(i18n::fmt(tl!("Use Send/Receive to sign in to {address}"), &[("address", &address)]));
            }
            self.begin_sign_in(account, oauth, &address, String::new(), false, true)?;
            return Ok(json!({"pending": "signIn", "account": account}));
        }
        let mail = mailcraft_engine::cmd::graph_mail::graph_mail_request(&self.session, account).map_err(|e| e.to_string())?;
        // The Outbox goes with the mail run itself (R3.4.2): sent as drafts or with `sendMail`,
        // and a message held there is not sent again by itself.
        let outbox: Vec<mailcraft_model::Message> = Vec::new();
        // The mail comes a piece at a time; a busy window holds the run back rather than letting
        // pieces pile up.
        let (pieces, taken) = std::sync::mpsc::sync_channel(8);
        let ask = GraphRequest {
            account: owner,
            oauth: self.effective_oauth(oauth),
            refresh,
            // (Renewed for the job: a token kept from another job may not cover the mail.)
            access: String::new(),
            outbox,
            // The calendar and the contacts are a job of their own after the round, so that they
            // never hold up the next mailbox's mail ([`calendar_sync`], #276).
            directory: false,
            calendar: None,
            changes: None,
            asks: Vec::new(),
            settings: false,
            // (The calendar's own job reads them, with a token that covers them.)
            favorites: false,
            favorites_scopes: false,
            mail: Some(mail),
            mail_pieces: Some(pieces),
            cancel: self.services.sync_cancel.clone(),
        };
        self.resume_fetch();
        let Some(graph) = &self.services.graph else { return Err(tl!("This build can't connect to Microsoft").to_string()) };
        self.services.sync_cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        self.graph_pieces = Some(taken);
        self.graph_problems.clear();
        self.graph_pieces_taken = 0;
        self.sync_run = Some((account, None, false, quiet));
        self.graph_rx = Some((account, quiet, false, graph(ask)));
        self.session.sync_status = tl!("Sending and receiving…").to_string();
        Ok(json!({"started": true, "account": account, "graph": true}))
    }

    /// Nobody has set this mailbox up: it holds only the placeholder account and no mail.
    pub fn needs_setup(&self) -> bool {
        let mb = &self.session.mailbox;
        mb.accounts.iter().all(|a| a.email.trim().is_empty()) && mb.messages.is_empty()
    }

    pub(crate) fn server_accounts(&self) -> Vec<Id> {
        // A JMAP account where the host runs JMAP (without, every run of it would end in a
        // refusal); the others where it opens mail connections, or, when it reaches no server
        // at all, to be told so.
        let runs = |a: &&mailcraft_model::Account| match a.kind {
            mailcraft_model::AccountKind::Jmap => self.services.jmap.is_some(),
            // (Where the switch is on and the host runs it, #252.)
            mailcraft_model::AccountKind::Exchange => self.session.ews && self.services.ews.is_some(),
            kind => kind.has_server() && (self.services.sync.is_some() || self.services.jmap.is_none()),
        };
        self.session.mailbox.accounts.iter().filter(runs).map(|a| a.id).collect()
    }

    /// How much of `account` waits to go to its server: mail in the Outbox, and changes made here
    /// that the server hasn't been told.
    pub(crate) fn waiting_to_go(&self, account: Id) -> usize {
        let mb = &self.session.mailbox;
        // (A held message waits for the owner, not for a run; and so does mail in a shared
        // mailbox's Outbox that may not be sent from it yet, #70.)
        let outbox = if self.shared_send_waits.contains(&account) {
            0
        } else {
            mb.folder_of_kind(account, mailcraft_model::FolderKind::Outbox).map_or(0, |f| mb.messages_in(f).filter(|m| m.held.is_empty()).count())
        };
        outbox + mailcraft_engine::cmd::account::unpushed(&self.session, account)
    }

    /// A run of `account` is over: what still waits to go to its server, and when that was seen.
    /// What a server refused, or what waits on the owner, then doesn't start a run every few
    /// seconds ([`MailApp::auto_sync`]), whichever way the account is kept in step.
    pub(crate) fn note_pushed(&mut self, account: Id) {
        self.push_seen.insert(account, (self.waiting_to_go(account), now_ms()));
    }

    /// The passwords and sign-ins in memory belong to the accounts of the mailbox they were given
    /// for: a mailbox that takes its place (File › Open, File › New, whatever the way) drops them
    /// all, since its accounts have ids of their own choosing and servers of their own naming.
    pub(crate) fn drop_secrets_if_replaced(&mut self) {
        let opening = self.session.openings();
        if opening != self.secrets_opening {
            self.secrets_opening = opening;
            // A question about the mailbox that was open goes with it: it names messages and
            // accounts by ids, which the other mailbox may have too (#503).
            if matches!(self.dialog, Some(dialogs::Dialog::Confirm { .. })) {
                self.dialog = None;
            }
            // An agent's too.
            agent_gate::forget(self);
            self.passwords.clear();
            self.secret_cache.clear();
            self.secret_read = None;
            self.secrets_asked = None;
            self.sync_after_secrets.clear();
            self.tokens.clear();
            self.graph_tokens.clear();
            self.jmap_refused.clear();
            self.ews_unreadable.clear();
            self.ews_signed_in.clear();
            self.ews_certificate.clear();
            // Work started for the old mailbox stays with it: a JMAP run and the messages being
            // fetched whole are called off (what they still bring is thrown away, `jmap_mail`,
            // `graph_fetch`), a sign-in on its way is called off, and a switch to Graph (its copy,
            // its backup) is forgotten.
            if matches!(self.sync_rx, Some(MailRun::Jmap { .. } | MailRun::Ews { .. })) {
                self.services.sync_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.fetching.forget();
            if let Some(s) = self.signing_in.take() {
                s.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                self.session.sync_status.clear();
            }
            self.forget_graph_switch();
            // And a mailbox from a file is held back until its servers are confirmed.
            self.judge_mailbox();
        }
    }

    /// Whether a Send/Receive for `account` can go ahead without asking anybody anything: its
    /// password or sign-in is at hand, in memory or with the operating system.
    fn can_sync_quietly(&self, account: Id) -> bool {
        // A shared mailbox the server wouldn't open waits for the owner to ask (#70); one that
        // can be read is read with its owner's sign-in.
        if self.shared_refused.contains_key(&account) {
            return false;
        }
        let account = self.session.mailbox.owner_of(account).map_or(account, |owner| owner.id);
        let Some(a) = self.session.mailbox.account(account) else { return false };
        // (A JMAP account's secret is in memory only, never with the operating system; and one its
        // server turned down waits for the owner.)
        if a.kind == mailcraft_model::AccountKind::Jmap {
            return !self.jmap_refused.contains(&account) && self.jmap_secret(account).is_some();
        }
        // (An Exchange server account's password is in memory only, for now; and one its server turned
        // down waits for the owner.)
        if a.kind == mailcraft_model::AccountKind::Exchange {
            return !self.jmap_refused.contains(&account) && self.ews_secret(account).is_some();
        }
        if a.oauth.is_some() {
            let kept = self.tokens.get(&account).is_some_and(|t| t.fresh(mailcraft_time::now()) || !t.refresh.is_empty());
            self.services.sign_in.is_some() && (kept || self.remembered("refresh", &a.email).is_some())
        } else {
            self.passwords.contains_key(&account) || self.remembered("password", &a.email).is_some()
        }
    }

    /// Send/Receive without being asked: the next account of a round that is under way, or a new
    /// round when the app starts and every few minutes after (the owner's `syncMinutes`). A round
    /// that started by itself skips accounts it would have to ask about.
    fn auto_sync(&mut self) {
        // Something the owner did (a read mark, a move, a message to send) waits while a long run
        // is still bringing mail down: the run is called off, which loses nothing, and the next
        // one tells the server first.
        // (Whichever way the account is kept in step: a Graph run too, #274.)
        if self.auto_sync
            && self.syncing()
            && let Some((whose, Some(baseline), false, quiet)) = self.sync_run
            && now_ms() - self.push_checked >= 3_000.0
        {
            self.push_checked = now_ms();
            if self.waiting_to_go(whose) > baseline {
                self.services.sync_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                self.sync_run = Some((whose, Some(baseline), true, quiet));
                self.session.sync_status = tl!("Sending your changes…").to_string();
            }
        }
        let busy = self.sync_rx.is_some() || self.graph_rx.is_some() || self.signing_in.is_some() || self.approving();
        if !self.services.can_sync() || busy || self.dialog.is_some() {
            return;
        }
        // The round's Inbox passes come before anything else of it (#276); neither starts while
        // the calendar's job is at work for the account: it is a short wait.
        if self.inbox_queue.front().is_some_and(|(account, _)| self.calendar_job_for(*account)) || self.start_next_inbox_pass().is_some() {
            return;
        }
        if self.sync_queue.front().is_some_and(|(account, _)| self.calendar_job_for(*account)) {
            return;
        }
        if let Some((account, quiet)) = self.sync_queue.pop_front() {
            // An account that would have to ask, or can't start, is left out of the run.
            let started = (!quiet || self.can_sync_quietly(account)) && self.start_sync(&json!({"account": account, "quiet": quiet})).is_ok();
            if !started {
                self.session.sync_progress.leave_out(account);
            }
            return;
        }
        self.settle_progress();
        let minutes = f64::from(self.session.prefs.sync_minutes);
        let now = now_ms();
        // Something made here waits to go out (a message to send, a read mark, a move): it
        // doesn't wait for the next round. Looked at every few seconds; what didn't go through
        // last time is left for a few minutes unless there is more of it.
        if self.auto_sync && now - self.push_checked >= 1_500.0 {
            self.push_checked = now;
            for account in self.server_accounts() {
                let waiting = self.waiting_to_go(account);
                let (seen, at) = self.push_seen.get(&account).copied().unwrap_or((0, 0.0));
                if waiting > 0 && (waiting != seen || now - at > 300_000.0) && self.can_sync_quietly(account) {
                    self.sync_queue.push_back((account, true));
                    self.push_only_next.insert(account);
                }
            }
            if !self.sync_queue.is_empty() {
                let round: Vec<Id> = self.sync_queue.iter().map(|(a, _)| *a).collect();
                self.session.sync_progress.begin(&round);
                return;
            }
        }
        if !self.auto_sync || minutes == 0.0 || (self.last_auto_sync != 0.0 && now - self.last_auto_sync < minutes * 60_000.0) {
            return;
        }
        self.last_auto_sync = now;
        self.sync_queue = self.server_accounts().into_iter().map(|a| (a, true)).collect();
        let round: Vec<Id> = self.sync_queue.iter().map(|(a, _)| *a).collect();
        if !round.is_empty() {
            self.session.sync_progress.begin(&round);
            self.queue_inbox_passes(&round, true);
        }
    }

    /// Nothing of a Send/Receive is under way or waiting to start, but Send/Receive Progress
    /// still has accounts running or waiting (a run that ended without its last piece, a sign-in
    /// or a password that came to nothing): the run is over, and what never started is left out.
    fn settle_progress(&mut self) {
        use mailcraft_engine::cmd::sync_progress::RunState;
        if !self.session.sync_progress.running() || !self.sync_queue.is_empty() || self.syncing() || self.graph_rx.is_some() {
            return;
        }
        let rows: Vec<(Id, RunState)> = self.session.sync_progress.accounts.iter().map(|r| (r.account, r.state)).collect();
        for (account, state) in rows {
            match state {
                RunState::Waiting => {
                    self.session.sync_progress.leave_out(account);
                }
                RunState::Running => {
                    let now = self.session.now();
                    self.session.sync_progress.count_full_if_due(&self.session.mailbox, account, now);
                    self.session.sync_progress.end(account, false);
                }
                RunState::Done | RunState::Stopped | RunState::Skipped => {}
            }
        }
    }

    /// Whether the owner still has to allow sending, calendar and contacts through Microsoft for
    /// `account` ("Connect calendar and contacts").
    pub fn graph_consent_needed(&self, account: Id) -> bool {
        self.graph_consent_needed.contains(&account)
    }

    /// Graph answered: keep its token, take in the calendar and contacts, file what it sent, and
    /// go on with the mail (for an account kept in step through Graph: take its folders in, and
    /// the run is over). Public for a host's tests, which have no window.
    pub fn poll_graph(&mut self) {
        let Some((account, quiet, directory, _)) = &self.graph_rx else { return };
        let (account, quiet, directory) = (*account, *quiet, *directory);
        // The mail's pieces first, each taken in as it comes; the job's answer is looked at once
        // the last of them is in.
        if let Some(pieces) = &self.graph_pieces {
            let mut taken = Vec::new();
            let mut over = false;
            // A few a frame: each one redraws the lists, and the window should stay quick.
            for _ in 0..4 {
                match pieces.try_recv() {
                    Ok(piece) => taken.push(piece),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        over = true;
                        break;
                    }
                }
            }
            for piece in taken {
                self.graph_pieces_taken = self.graph_pieces_taken.saturating_add(1);
                // New mail may be waiting to be fetched whole: the background looks again.
                if !piece.pages.is_empty() {
                    self.fetching.wake();
                }
                // A shared mailbox the server won't open for the owner (#70).
                if let Some(why) = piece.refused {
                    self.shared_refused.insert(account, why);
                }
                // The Inbox checked by itself leaves Send/Receive Progress and the status as
                // they were: it is quiet (#276).
                let kept = self.inbox_check.under_way().then(|| (self.session.sync_progress.clone(), self.session.sync_status.clone()));
                match mailcraft_engine::cmd::graph_mail::apply_graph_mail(&mut self.session, piece) {
                    Ok(said) => {
                        let errors =
                            said["errors"].as_array().map(|all| all.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>());
                        self.graph_problems.extend(errors.unwrap_or_default());
                    }
                    Err(e) => self.graph_problems.push(e.to_string()),
                }
                if let Some((progress, status)) = kept {
                    (self.session.sync_progress, self.session.sync_status) = (progress, status);
                }
            }
            // Its first piece made the changes made here and sent the Outbox; what waits beyond
            // what waits now was done here since, and calls the run off ([`MailApp::auto_sync`]).
            if self.graph_pieces_taken > 0
                && let Some((whose, None, false, q)) = self.sync_run
                && whose == account
            {
                self.sync_run = Some((whose, Some(self.waiting_to_go(whose)), false, q));
            }
            // A shared mailbox being added is kept once its Inbox is read, not after all its mail.
            if self.inbox_run.is_none() {
                self.shared_check_piece(account, over);
            }
            if !over {
                return;
            }
            self.graph_pieces = None;
            // The mail is in; the calendar and contacts come next (not after the Inbox pass).
            if self.inbox_run.is_none() && self.session.sync_progress.holding(account) {
                self.session.sync_progress.phase(account, SyncPhase::CalendarAndContacts);
            }
        }
        let Some((_, _, _, rx)) = &self.graph_rx else { return };
        let mut result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::warn!("graph, account {account}: the job ended without an answer (its thread ended early)");
                GraphResult { account, ..Default::default() }
            }
        };
        self.graph_rx = None;
        // The Inbox pass is over: the round goes on (#276).
        if self.inbox_run.take() == Some(account) {
            self.graph_pieces_taken = 0;
            return self.finish_inbox_pass(account, result);
        }
        // Called off so that what the owner did could go out: it goes on at once, as it was.
        if self.sync_run.is_some_and(|(whose, ..)| whose == account)
            && let Some((whose, _, true, quiet)) = self.sync_run.take()
            && !self.sync_queue.iter().any(|(a, _)| *a == whose)
        {
            self.sync_queue.push_front((whose, quiet));
        }
        let streamed = std::mem::take(&mut self.graph_pieces_taken) > 0;
        let after_sign_in = self.graph_after_sign_in.take() == Some(account);
        // A shared mailbox's run is its mail alone (#70).
        if self.session.mailbox.account(account).is_some_and(|a| a.is_shared()) {
            return self.finish_shared_mail(account, quiet, result);
        }
        // The Favorites as Outlook has them, when they came along ([`favorites_sync`]).
        self.take_in_favorites(account, &mut result);
        let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
        let mut errors = result.errors;
        if result.token_refused {
            self.graph_tokens.remove(&account);
        }
        if let Some(tokens) = result.tokens
            && !self.signed_in_as_other(account, &tokens)
        {
            self.note_directory(account, &tokens.tenant);
            // The newest refresh token is the account's, for mail and Graph alike.
            if !tokens.refresh.is_empty() {
                self.remember("refresh", &address, &tokens.refresh);
                if let Some(mail) = self.tokens.get_mut(&account) {
                    mail.refresh = tokens.refresh.clone();
                }
            }
            let now = mailcraft_time::now();
            self.graph_tokens.insert(account, Tokens { expires_at: tokens.expires_at.clamp(now, now.saturating_add(86_400)), ..tokens });
        }
        let kept_in_step = self.session.mailbox.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Microsoft);
        if result.consent_needed {
            // (An account kept in step through Graph is asked below, for its mail.)
            if self.graph_consent_needed.insert(account) && !quiet && !kept_in_step {
                self.status(tl!("Calendar and contacts need your permission: File › Info › Connect calendar and contacts"));
            }
        } else {
            self.graph_consent_needed.remove(&account);
        }
        // What the server said to the calendar changes it was told, then what it has now.
        self.take_in_calendar_list(account, result.calendars.as_ref(), result.categories.as_ref(), result.extras_missing, &mut errors);
        let calendar_ok = self.take_in_calendar_of_sync(account, result.uploaded.as_ref(), result.events.as_ref(), &mut errors);
        self.take_in_more_calendars(&result.more_events, &mut errors);
        let mut contacts_ok = false;
        if let Some(contacts) = &result.contacts {
            match mailcraft_engine::cmd::remote::apply_contacts(&mut self.session, contacts) {
                Ok(_) => contacts_ok = true,
                Err(e) => errors.push(i18n::fmt(tl!("The contacts couldn't be taken in: {error}"), &[("error", &e.to_string())])),
            }
        }
        if directory && calendar_ok && contacts_ok {
            self.graph_checked.insert(account, now_ms());
        }
        if !result.sent.is_empty() {
            // (A piece of the run, not the end of it: the mail server's leg comes next, and
            // Send/Receive Progress counts what was sent with what that brings.)
            let sent = mailcraft_model::sync::SyncResult {
                account,
                sent: result.sent.clone(),
                filed: result.sent.clone(),
                partial: !kept_in_step,
                ..Default::default()
            };
            if let Err(e) = mailcraft_engine::cmd::account::apply_sync(&mut self.session, sent) {
                errors.push(e.to_string());
            }
        }
        // An account kept in step through Graph: its folders, and the run is over (no IMAP leg).
        if kept_in_step {
            self.session.sync_progress.end(account, false);
            errors.append(&mut self.graph_problems);
            // What still waits after this run doesn't start another every few seconds.
            self.note_pushed(account);
            // No token for the mail: Microsoft wants the owner's say-so for it, or the sign-in has
            // run out. There is no IMAP leg to ask instead, so the owner is asked here (#124).
            if result.mail.is_none() && (result.consent_needed || result.sign_in_refused) {
                self.graph_mail_needs_sign_in(account, quiet, after_sign_in, result.consent_needed);
                return;
            }
            let status = self.session.sync_status.clone();
            match result.mail.take() {
                Some(mail) => match mailcraft_engine::cmd::graph_mail::apply_graph_mail(&mut self.session, mail) {
                    Ok(taken) => {
                        let said = taken["errors"].as_array().map(|all| all.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>());
                        errors.extend(said.unwrap_or_default());
                    }
                    Err(e) => errors.push(e.to_string()),
                },
                // The mail came a piece at a time, each taken in as it came: the job's own answer
                // has none of it, and the run is over.
                None if streamed => {
                    if status == tl!("Sending and receiving…") {
                        self.session.sync_status.clear();
                    }
                }
                // No answer for the folders (Graph wasn't reached, or the job ended early): what
                // the job said is why, and it is logged with the rest of the problems below.
                None => {
                    if errors.is_empty() {
                        log::warn!("graph mail, account {account}: the run ended without the mail, and nothing said why");
                    } else {
                        log::warn!("graph mail, account {account}: the run ended without the mail");
                    }
                    if status == tl!("Sending and receiving…") {
                        self.session.sync_status.clear();
                    }
                }
            }
            errors.dedup();
            if !errors.is_empty() {
                self.session.sync_status = errors.first().cloned().unwrap_or_default();
            } else if self.session.sync_status == tl!("Sending and receiving…") {
                // The run is over: it never says it is still going.
                self.session.sync_status.clear();
            }
            self.note_sync_problems(account, &errors, true);
            return;
        }
        self.note_sync_problems(account, &errors, false);
        let skip: Vec<Id> = result.sent.iter().chain(&result.held).copied().collect();
        if let Err(e) = self.start_sync(&json!({"account": account, "quiet": quiet, "afterGraph": true, "skip": skip})) {
            self.session.sync_progress.end(account, false);
            self.session.sync_status.clear();
            self.status(e);
        }
    }

    /// A Graph run of an account kept in step through Graph got no token for the mail: the owner
    /// signs in with Microsoft for it when they asked for the run (the run goes on once they
    /// have), and is told how when they didn't.
    fn graph_mail_needs_sign_in(&mut self, account: Id, quiet: bool, after_sign_in: bool, consent: bool) {
        self.session.sync_status.clear();
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return };
        let why = if consent { "the sign-in needs permission for the mail" } else { "the sign-in was refused" };
        // Just signed in, and still refused: another trip to Microsoft's page wouldn't change it.
        if after_sign_in {
            log::warn!("graph mail, account {account}: {why} straight after signing in; not asking again");
            self.status(i18n::fmt(
                tl!("Signed in, but Microsoft still won't let MailCraft read the mail of {account}. Try Send/Receive again later."),
                &[("account", &owner.email)],
            ));
            return;
        }
        let Some(oauth) = owner.oauth.clone() else {
            log::warn!("graph mail, account {account}: {why}, and the account has no sign-in with Microsoft");
            self.status(tl!("This account has no sign-in with Microsoft"));
            return;
        };
        if quiet {
            log::warn!("graph mail, account {account}: {why}; waiting for the owner to press Send/Receive");
            self.status(i18n::fmt(tl!("Sign in to Microsoft again to get the mail of {account}: press Send/Receive"), &[("account", &owner.email)]));
            return;
        }
        log::warn!("graph mail, account {account}: {why}; asking the owner to sign in");
        // What an earlier run said is over: the sign-in's own words take its place.
        self.status_msg = None;
        // A refused sign-in's refresh token is no good to anybody: a fresh sign-in replaces it.
        if !consent {
            self.graph_tokens.remove(&account);
            self.tokens.remove(&account);
        }
        if let Err(e) = self.begin_sign_in(account, oauth, &owner.email, String::new(), false, true) {
            self.status(e);
        }
    }

    /// Whether the owner is being signed in on a provider's page.
    pub fn signing_in(&self) -> bool {
        self.signing_in.is_some()
    }

    /// The sign-in came back: keep the tokens for the session and carry on with the Send/Receive
    /// that asked for them; or say why not. (Public for a host's tests, which have no window.)
    pub fn poll_sign_in(&mut self) {
        let Some(waiting) = &self.signing_in else { return };
        let answer = match waiting.rx.try_recv() {
            Ok(answer) => answer,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(tl!("Signing in stopped unexpectedly").to_string()),
        };
        let (account, for_graph, for_favorites) = (waiting.account, waiting.graph, waiting.favorites);
        let asked_for = !waiting.quiet;
        let (for_shared, for_shared_send, for_shared_calendars) = (waiting.shared, waiting.shared_send, waiting.shared_calendars);
        let for_tasks = waiting.tasks;
        let started_under = waiting.opening;
        self.signing_in = None;
        // Another mailbox took the place of the one it was for: its accounts are other accounts,
        // whatever their ids, and this answer is nobody's.
        if started_under != self.session.openings() {
            self.session.sync_status.clear();
            return;
        }
        // A day at most, whatever the provider said; a sign-in is asked for again after.
        let now = mailcraft_time::now();
        let answer = answer.and_then(|tokens| {
            if tokens.access.is_empty() {
                Err(tl!("Signing in brought nothing back").to_string())
            } else {
                Ok(Tokens { expires_at: tokens.expires_at.clamp(now, now.saturating_add(86_400)), ..tokens })
            }
        });
        if account == 0 {
            // For an account that is being set up: its dialog takes it from here.
            self.session.sync_status.clear();
            self.setup_sign_in = Some(answer);
            return;
        }
        // For reading a shared mailbox (#70): it is read now, or why not is said.
        if let Some(shared) = for_shared {
            // (And for reading its calendars: they are read now, or why not is said.)
            if for_shared_calendars {
                return self.shared_calendars_signed_in(shared, account, answer);
            }
            // (And for sending from it: what waits in its Outbox goes now, or is held.)
            if for_shared_send {
                return self.shared_send_signed_in(shared, account, answer);
            }
            return self.shared_signed_in(shared, account, answer);
        }
        // For keeping the tasks in step with Microsoft To Do (#248): they are, now, or why not is said.
        if for_tasks {
            return self.tasks_signed_in(account, answer);
        }
        // For reading the calendars opened by address (#70): they are read now, or why not is said.
        if for_shared_calendars {
            return self.opened_calendars_signed_in(account, answer);
        }
        let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
        // The owner picks who signs in on the page: it has to be this account. Somebody else's
        // tokens are neither used nor remembered, mail's or Graph's (#103).
        if let Ok(tokens) = &answer {
            if self.signed_in_as_other(account, tokens) {
                return;
            }
            // Which kind of Microsoft account it is, as the sign-in says ([`favorites_sync`]).
            self.note_directory(account, &tokens.tenant);
        }
        if for_graph {
            // "Connect calendar and contacts": the token is Graph's; the next run uses it.
            match answer {
                Ok(tokens) => {
                    if !tokens.refresh.is_empty() {
                        self.remember("refresh", &address, &tokens.refresh);
                        if let Some(mail) = self.tokens.get_mut(&account) {
                            mail.refresh = tokens.refresh.clone();
                        }
                    }
                    self.graph_tokens.insert(account, tokens);
                    self.graph_consent_needed.remove(&account);
                    self.graph_checked.remove(&account);
                    // Calendar changes that waited for this permission go now.
                    self.calendar_connected(account);
                    // Asked for with the Favorites' permissions, the token is good for them, and
                    // they are read at once. Asked for without, it may not be: an account that
                    // has Outlook's list is not to lose it to a token that can't read it.
                    if for_favorites {
                        self.favorites_connected(account);
                    } else if self.session.mailbox.favorites_from_server.iter().any(|list| list.account == account) {
                        self.favorites_token_is_plain(account);
                    }
                    // (A Send/Receive that is under way already, as the first one of an account
                    // that was just added is, brings what this one would: the Favorites are
                    // read by the calendar's own job either way.)
                    let busy = for_favorites && (self.sync_rx.is_some() || self.graph_rx.is_some());
                    if busy {
                        self.session.sync_status.clear();
                    } else if let Err(e) = self.start_sync(&json!({"account": account, "afterSignIn": true})) {
                        self.session.sync_status.clear();
                        self.status(e);
                    }
                }
                // The Favorites were not to be had: said under their heading, or in the status
                // bar; nothing else is any different for it.
                Err(e) if for_favorites => {
                    self.session.sync_status.clear();
                    self.favorites_refused(account, &e);
                }
                Err(e) => {
                    self.session.sync_status.clear();
                    self.sign_in_came_to_nothing(account, &e, asked_for);
                }
            }
            return;
        }
        match answer {
            Ok(tokens) => {
                self.remember("refresh", &address, &tokens.refresh);
                self.tokens.insert(account, tokens);
                if let Err(e) = self.start_sync(&json!({"account": account})) {
                    self.session.sync_status.clear();
                    self.status(e);
                }
            }
            Err(e) => {
                // What was kept stays: the host only gives up on it when the provider refuses it,
                // and a sign-in that works replaces it. The access token is spent either way.
                if let Some(tokens) = self.tokens.get_mut(&account) {
                    tokens.access.clear();
                }
                self.session.sync_status.clear();
                self.note_sync_problems(account, std::slice::from_ref(&e), false);
                self.sign_in_came_to_nothing(account, &e, asked_for);
            }
        }
    }

    /// A sign-in of an account that exists (to its mail, or for its calendar and contacts)
    /// brought no tokens: `error` says why, in the status bar. When it is that an administrator
    /// of the organisation has to approve MailCraft first, and the owner had asked for the
    /// sign-in, the page that says what to do next comes up as well ([`approval`]): the owner
    /// has just come back from a page of Microsoft's that said no, and is not left with a line
    /// at the foot of the window. Never for a sign-in nobody asked for, and never over another
    /// dialog.
    fn sign_in_came_to_nothing(&mut self, account: Id, error: &str, asked_for: bool) {
        log::warn!("sign-in, account {account}: ended without signing in ({:?})", approval::SignInEnd::of(error));
        if approval::SignInEnd::of(error) == approval::SignInEnd::Approval && asked_for && self.dialog.is_none() {
            self.dialog = dialogs::Dialog::open("account", self, &json!({"id": account, "page": "approval"}));
        }
        self.status(tl!(error));
    }

    /// Whether the last Send/Receive of any account had problems.
    pub fn has_sync_problems(&self) -> bool {
        !self.sync_problems.is_empty()
    }

    /// What the status bar says now and what the last Send/Receive found wrong, as one text, for a
    /// host's tests (which have no window to look at).
    #[doc(hidden)]
    pub fn said_text(&self) -> String {
        let status = self.status_msg.as_ref().map(|(said, _)| said.as_str()).unwrap_or_default();
        format!("{status}\n{}", self.sync_problem_lines().join("\n"))
    }

    /// What went wrong in the last Send/Receive, account by account, as lines to show and copy.
    pub(crate) fn sync_problem_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (_, name, at, lines) in &self.sync_problems {
            out.push(format!("{name} — {at}"));
            out.extend(lines.iter().map(|l| format!("  {l}")));
        }
        out
    }

    /// Keep what a Send/Receive said went wrong (the log gets only how many); a run without
    /// problems clears what was kept for its account.
    fn note_sync_problems(&mut self, account: Id, errors: &[String], finished: bool) {
        if errors.is_empty() {
            if finished {
                self.sync_problems.retain(|(a, ..)| *a != account);
            }
            return;
        }
        // The lines name subjects, addresses and folders: they are for the owner to read in the
        // problems report, and the log says only how many there were.
        log::warn!("send/receive, account {account}: {} problem(s) reported", errors.len());
        self.keep_sync_problems(account, errors);
    }

    /// Keep lines for the Send/Receive problems report without putting them in the log: what
    /// names an item (a line about an appointment, with its title) is for the owner to read,
    /// and whoever wrote it has logged what kind of thing happened.
    pub(crate) fn keep_sync_problems(&mut self, account: Id, errors: &[String]) {
        if errors.is_empty() {
            return;
        }
        let name = self.session.mailbox.account(account).map(|a| a.name.clone()).unwrap_or_default();
        let at = crate::dates::clock(self.session.local_now(), self.session.prefs.twelve_hour);
        let clip = |e: &String| e.chars().take(600).collect::<String>();
        match self.sync_problems.iter_mut().find(|(a, ..)| *a == account) {
            Some((_, _, when, lines)) => {
                *when = at;
                lines.extend(errors.iter().map(clip));
                let extra = lines.len().saturating_sub(60);
                lines.drain(..extra);
            }
            None => self.sync_problems.push((account, name, at, errors.iter().take(60).map(clip).collect())),
        }
    }

    /// Whether a job of Microsoft Graph is under way for an account.
    pub fn graph_running(&self) -> bool {
        self.graph_rx.is_some()
    }

    /// The account whose part of the Send/Receive is under way (or whose sign-in for it is).
    fn sync_running_account(&self) -> Option<Id> {
        let imap = self.sync_rx.as_ref().and(self.sync_run.as_ref()).map(|(account, ..)| *account);
        let graph = self.graph_rx.as_ref().map(|(account, ..)| *account);
        imap.or(graph).or_else(|| self.signing_in.as_ref().filter(|s| !s.graph && !s.favorites).map(|s| s.account))
    }

    /// Skip: leave an account that waits its turn out of the run under way.
    fn skip_sync(&mut self, p: &Value) -> Result<Value, String> {
        use mailcraft_engine::cmd::sync_progress::RunState;
        let account = p.get("account").and_then(Value::as_u64).ok_or_else(|| tl!("Say which account to skip").to_string())?;
        let name = self.session.mailbox.account(account).map(|a| a.name.clone()).ok_or_else(|| tl!("There is no such account").to_string())?;
        let row = self.session.sync_progress.accounts.iter().find(|r| r.account == account).map(|r| r.state);
        if self.sync_running_account() == Some(account) || row == Some(RunState::Running) {
            return Err(i18n::fmt(tl!("{name} is being sent and received now. Use Stop."), &[("name", &name)]));
        }
        if !self.sync_queue.iter().any(|(a, _)| *a == account) {
            return Err(i18n::fmt(tl!("{name} isn't waiting its turn in a Send/Receive"), &[("name", &name)]));
        }
        self.sync_queue.retain(|(a, _)| *a != account);
        self.push_only_next.remove(&account);
        self.session.sync_progress.skip(account);
        Ok(json!({"skipped": account}))
    }

    /// Whether a Send/Receive is running.
    pub fn syncing(&self) -> bool {
        // (The Inbox checked by itself is not one: it is quiet, #276.)
        self.sync_rx.is_some() || (self.graph_pieces.is_some() && !self.inbox_check.under_way())
    }

    /// Take in what a Send/Receive has brought so far. It arrives in pieces (what was sent, what
    /// the server was told, then mail folder by folder, the Inbox first), each applied as it
    /// comes, so mail shows up while the rest is still on its way; the last piece ends the run.
    pub fn poll_sync(&mut self) {
        // A few pieces a frame: each one redraws the lists, and the window should stay quick.
        for _ in 0..4 {
            // (A JMAP account's run has pieces of its own: [`jmap_mail`].)
            if matches!(self.sync_rx, Some(MailRun::Jmap { .. })) {
                return self.poll_jmap();
            }
            if matches!(self.sync_rx, Some(MailRun::Ews { .. })) {
                return self.poll_ews();
            }
            let Some(MailRun::Imap(rx)) = &self.sync_rx else { return };
            let piece = match rx.try_recv() {
                Ok(piece) => piece,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    log::warn!("send/receive: the run stopped without an answer (its thread ended early)");
                    self.sync_rx = None;
                    self.session.sync_status = tl!("Send/Receive stopped unexpectedly").to_string();
                    return;
                }
            };
            let (account, last) = (piece.account, !piece.partial);
            let downloading = !piece.folders.is_empty();
            let refused = piece.errors.iter().any(|e| {
                let e = e.to_lowercase();
                // Microsoft's word for a token that ran out during the run.
                e.contains("sign-in") || e.contains("sign in") || e.contains("accesstokenexpired")
            });
            self.sync_refused |= refused;
            self.note_sync_problems(account, &piece.errors, false);
            match mailcraft_engine::cmd::account::apply_sync_part(&mut self.session, &mut self.sync_totals, piece) {
                // New mail doesn't ask the taskbar for attention: KDE and Windows keep the button lit
                // until the window is clicked, and a first download lit it all day. System
                // notifications (R7.1) are how new mail is announced; reminders still ask.
                Ok(_) => {}
                Err(e) => self.status(e.to_string()),
            }
            // Once it brings mail down, the run has told the server what there was to tell: what
            // waits beyond what waits at this moment was done here since.
            if downloading && let Some((whose, None, false, quiet)) = self.sync_run {
                self.sync_run = Some((whose, Some(self.waiting_to_go(whose)), false, quiet));
            }
            if !last {
                continue;
            }
            // The run is over.
            self.sync_rx = None;
            // Called off so that what the owner did could go out: the next run starts at once,
            // tells the server first, and carries on bringing mail down.
            if let Some((whose, _, true, quiet)) = self.sync_run.take() {
                self.sync_queue.push_front((whose, quiet));
            }
            let totals = std::mem::take(&mut self.sync_totals);
            let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
            if std::mem::take(&mut self.sync_refused) {
                // Forget the password so the next try asks again, and the access token so the
                // next try renews it (the refresh token stays for that).
                self.passwords.remove(&account);
                if let (Some(store), Some(a)) = (&self.services.secrets, self.account_at(&address)) {
                    let key = Self::secret_key("password", a);
                    store.delete(&key);
                    self.secret_cache.remove(&key);
                }
                if let Some(tokens) = self.tokens.get_mut(&account) {
                    tokens.access.clear();
                }
            } else if totals.errors.is_empty() {
                if let Some(password) = self.passwords.get(&account).cloned() {
                    // It went through: this password is worth remembering.
                    self.remember("password", &address, &password);
                }
                // And nothing is left of the last run's problems.
                self.note_sync_problems(account, &[], true);
            }
            self.status(totals.summary_in(i18n::current()));
            self.note_pushed(account);
            self.graph_switch_after_sync(account, totals.errors.is_empty() && !totals.stopped);
            return;
        }
    }

    /// Open a file by path (the command line, a drop on the window, the system's Open With): a
    /// mailbox opens, a message, calendar or contact file is imported, anything else is attached
    /// to the message being written. A failure shows in the status bar.
    pub fn open_path(&mut self, path: &str) {
        // Handed over while the mailbox opens: opened once it is.
        if opening::queue(self, path) {
            return;
        }
        files::dropped_path(self, path);
    }

    /// Open what the system handed MailCraft (the command line, a later start, the Finder): a
    /// `mailto:` link starts a new message, a message file opens read-only in a window of its own
    /// (filed nowhere until the owner says so: Move to Folder, #515), a calendar or contact file is imported only
    /// once the owner says so, and a mailbox or a folder of mail as [`open_path`](Self::open_path).
    pub fn open_from_system(&mut self, what: &str) {
        // Handed over while the mailbox opens: opened once it is.
        if opening::queue(self, what) {
            return;
        }
        files::from_system(self, what);
    }

    /// `n` things the system handed over were no mail link and no file MailCraft opens, and were
    /// left alone: the status bar says so.
    pub fn note_left_alone(&mut self, n: usize) {
        if n > 0 {
            self.status(tln!(
                n,
                "MailCraft was started with {n} thing that isn't a mail link or a file it opens: it was left alone",
                "MailCraft was started with {n} things that aren't mail links or files it opens: they were left alone"
            ));
        }
    }

    pub fn status(&mut self, s: impl Into<String>) {
        self.status_msg = Some((s.into(), now_ms()));
    }

    /// Run a command the owner asked for that may well be refused, with another way to go when it
    /// is (a window of its own where there are none, say): a refusal is nobody's business, and
    /// the status bar and the message box stay as they were.
    pub(crate) fn run_or_else(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let (said, refused) = (self.status_msg.clone(), self.refused.clone());
        let r = self.run(id, params);
        if r.is_err() {
            (self.status_msg, self.refused) = (said, refused);
        }
        r
    }

    /// The message box for what the owner did by hand and was refused (#152), once the frame is
    /// laid out: only when nothing else is asking the owner anything (a dialog that ran the
    /// command and stays open says what went wrong itself).
    fn refusal(&mut self) {
        let Some(text) = self.refused.take() else { return };
        if self.dialog.is_none() {
            self.dialog = Some(dialogs::Dialog::Refused { text });
        }
    }

    /// The name of what is in view, for the window title.
    pub fn title_stem(&self) -> String {
        use mailcraft_engine::Module;
        let account =
            self.session.mailbox.folder(self.session.view.folder).and_then(|f| self.session.mailbox.account(f.account)).map(|a| a.name.clone());
        match self.session.view.module {
            Module::Mail => {
                let folder = self.session.mailbox.folder(self.session.view.folder).map_or(tl!("Mail"), crate::nav::folder_name);
                match account.filter(|a| !a.is_empty()) {
                    Some(a) => i18n::fmt(tl!("{folder} - {mailbox}"), &[("folder", folder), ("mailbox", &a)]),
                    None => folder.to_string(),
                }
            }
            m => tl!(m.label()).to_string(),
        }
    }

    /// The interface language: the owner's choice (`ui.json`'s `language`), with the system's
    /// locale for Automatic.
    pub fn language(&self) -> i18n::Lang {
        i18n::Lang::from_pref(&self.ui.language, self.services.system_locale.as_deref())
    }

    /// Put the interface language in force: the window's text, the engine's, and the host's
    /// (`<html lang>` in the browser) when it changed.
    fn apply_language(&mut self) {
        let lang = self.language();
        i18n::set_current(lang);
        self.session.interface_language = lang;
        self.session.prefs.follow_language(lang);
        if self.applied_language != Some(lang) {
            self.applied_language = Some(lang);
            if let Some(tell) = &self.services.on_language {
                tell(lang.code());
            }
        }
    }

    /// Per-frame logic before layout: control requests, screenshots, background results, file
    /// drops, saving.
    pub fn logic(&mut self, ctx: &egui::Context) {
        self.apply_language();
        if !self.styled {
            theme::install_fonts(ctx);
            // Mod with -, = and 0 would also scale the whole window; interface size is in Options.
            ctx.options_mut(|o| o.zoom_with_keyboard = false);
            // egui closes the window on Mod+Q by itself; here that is Mark as Read. A Mac quits
            // from its app menu (craft-quit).
            ctx.options_mut(|o| o.quit_shortcuts.clear());
            self.styled = true;
        }
        // Settled from the main window's input, so every window has the same theme.
        let preference = self.ui.theme.preference(theme::main_system_theme(ctx));
        if self.applied_theme != Some(preference) {
            theme::apply(ctx, preference);
            self.applied_theme = Some(preference);
        }
        let zoom = self.ui.interface_size.zoom_factor();
        if ctx.zoom_factor() != zoom {
            ctx.set_zoom_factor(zoom);
        }
        if self.ctx.is_none() {
            self.ctx = Some(ctx.clone());
        }
        windows::remember_identity(ctx, self.services.window_identity.as_ref());
        // Waiting for the mailbox: nothing else happens.
        if self.opening.is_some() {
            opening::logic(self, ctx);
            return;
        }
        opening::after(self, ctx);
        // Before anything can start or end a Send/Receive in this frame ([`alerts`]).
        alerts::tick(self, ctx);
        self.drain_control(ctx);
        control::trail(self, ctx);
        self.poll_sign_in();
        // A sign-in for an account that is being set up has nobody waiting for it once that dialog
        // is gone, however it went (a script closing it, say): the host is told to stop.
        if self.signing_in.as_ref().is_some_and(|s| s.account == 0) && !matches!(&self.dialog, Some(d) if d.name() == "account") {
            let _ = self.execute("ui.cancelSignIn", json!({}));
            self.session.sync_status.clear();
        }
        // An administrator's approval that was waited for ([`approval`]).
        self.poll_admin_consent();
        self.sweep_admin_consent();
        self.drop_secrets_if_replaced();
        // The mailbox the window started with: secrets kept the old way are filed under its servers.
        self.secrets_tick();
        self.poll_graph();
        self.drive_fetch();
        self.offline_tick();
        self.drive_server_search();
        if self.server_searching.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        if self.fetching.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        // The pieces of a Graph mail run come without waking the window: it looks again soon.
        if self.graph_pieces.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        self.poll_calendar();
        self.poll_tasks();
        self.poll_calendar_check();
        self.poll_free_busy();
        self.poll_sync();
        // (Before any other run can start: a shared mailbox being added goes first.)
        // Once MailCraft's data is removed nothing starts by itself any more: no Send/Receive,
        // no calendar run, no check for a new version (#177).
        if !self.data_removed {
            self.shared_check_tick();
        }
        self.check_secret_store();
        self.data_removal_tick();
        if !self.data_removed {
            // Whether a new version is out, when the owner wants that known ([`updates`]).
            self.updates_tick(ctx);
            self.auto_sync();
            // The Inbox by itself every minute, between rounds (#276).
            self.inbox_check_tick();
            // The calendar by itself, between Send/Receive runs ([`calendar_sync`]).
            self.calendar_tick(ctx);
            // The tasks by themselves, after the calendar ([`task_sync`]).
            self.tasks_tick(ctx);
        }
        reminders::tick(self, ctx);
        if std::mem::take(&mut self.offer_setup) && self.dialog.is_none() && self.needs_setup() {
            self.dialog = dialogs::Dialog::open("account", self, &json!({}));
        }
        self.poll_graph_switch_copy();
        self.tick_graph_switch();
        let _ = files::poll(self);
        files::drain_inbox(self);
        opening::poll_ahead(self, ctx);
        files::ask_next(self);
        self.autosave_tick(ctx);
        self.draft_autosave_tick(ctx);
        self.collect_screenshots();
        self.issue_screenshots(ctx);
        let waiting = self.sync_rx.is_some() || self.graph_rx.is_some() || self.signing_in.is_some() || self.approving();
        if !self.synthetic.is_empty() || waiting || !self.sync_queue.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else if self.auto_sync && self.services.can_sync() && self.session.prefs.sync_minutes > 0 && !self.server_accounts().is_empty() {
            // Nothing is happening: look again in a while, in case a Send/Receive round is due or
            // something waits to go out.
            ctx.request_repaint_after(std::time::Duration::from_millis(1_500));
        }
        let signatures = self.ui.backstage && self.ui.backstage_page == "signatures";
        if std::mem::replace(&mut self.signatures_shown, signatures) && !signatures {
            self.leave_signature();
        }
        #[cfg(not(target_arch = "wasm32"))]
        for f in ctx.input(|i| i.raw.dropped_files.clone()) {
            // A file let go over the window is the owner's doing.
            self.by_hand = true;
            let path = f.path().to_string_lossy().to_string();
            // A picture dropped on File › Signatures goes into the signature open there.
            if signatures && self.session.signature_edit.is_some() {
                signatures::add_picture(self, json!({"path": path}));
            } else {
                self.open_path(&path);
            }
        }
        self.by_hand = false;
        self.report_dirty();
    }

    /// Keep what was changed in the signature open in File › Signatures, and close it.
    fn leave_signature(&mut self) {
        self.signature_note = None;
        if self.session.signature_edit.as_ref().is_some_and(|e| e.dirty)
            && let Err(e) = self.execute("signature.save", json!({}))
        {
            self.status(e);
        }
        let _ = self.execute("signature.close", json!({"discard": true}));
    }

    /// Tell the host whether there are unsaved changes ([`Services::on_dirty`]).
    fn report_dirty(&self) {
        if let Some(f) = &self.services.on_dirty {
            f(self.session.dirty && self.session.rev() != self.saved_rev);
        }
    }

    /// Save a moment after the last change: to the mailbox file on the desktop (through the
    /// host's background saver when it has one, so the window never waits for the disk), to the
    /// host's storage in the browser. A failure shows in the status bar and is tried again later.
    /// A draft of a Microsoft account that is being written is saved every five minutes while
    /// it has changes, and so goes up to its server with the next Send/Receive (R3.4.1).
    fn draft_autosave_tick(&mut self, ctx: &egui::Context) {
        const EVERY_MS: f64 = 5.0 * 60_000.0;
        let Some(c) = &self.session.compose else {
            self.draft_saved_at = 0.0;
            return;
        };
        let graph = self.session.mailbox.account(c.account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Microsoft);
        if !graph || !c.dirty {
            return;
        }
        let now = now_ms();
        if self.draft_saved_at == 0.0 {
            self.draft_saved_at = now;
        }
        let waited = now - self.draft_saved_at;
        if waited >= EVERY_MS {
            self.draft_saved_at = now;
            if let Err(e) = self.run("compose.saveDraft", json!({})) {
                log::info!("the draft couldn't be saved by itself: {e}");
            }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis((EVERY_MS - waited).max(1_000.0) as u64));
        }
    }

    fn autosave_tick(&mut self, ctx: &egui::Context) {
        // How far the saver has got is looked at before what it reports: when it has nothing
        // left, whatever it wrote is among the reports read next.
        if let Some(saver) = &self.services.saver {
            self.save_progress = saver.progress();
            // The saver wakes the window when a state is written; how far it has got in between
            // is looked at a few times a second.
            if self.save_progress.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        let saved_before = self.saved_rev;
        let failed = self.collect_saves();
        if self.saved_rev > saved_before {
            // Saving works again: closing waits for it again.
            self.close_anyway = false;
        }
        let free = self.save_progress.is_none();
        if let Some(e) = &failed {
            // Closing waited for a save that failed: the window stays open (the status bar says
            // why) rather than closing by itself later, and is closed by the next close request.
            if self.closing_after_save.take().is_some() {
                self.close_anyway = true;
            }
            // So does File › New or File › Open, as the command itself would have.
            if let Some((id, params)) = self.after_save.take() {
                opening::forget_unopened(&id, &params);
                let error = format!("the mailbox couldn't be saved first, so it stays open ({e})");
                self.after_save_outcome = Some(json!({"command": id, "ok": false, "error": error}));
            }
        }
        // Closing waited for the state it was asked to close with: written (a download going on
        // meanwhile has changed the mailbox since, and `save_on_exit` writes that), or nothing is
        // left to write.
        if let Some(target) = self.closing_after_save
            && (self.saved_rev >= target || free)
        {
            self.closing_after_save = None;
            self.close_ready = true;
            self.quit_requested = true;
        }
        // File › New or File › Open runs once the saver is done with the file (it is handed
        // nothing newer meanwhile, below), writing what changed since itself. Run any earlier,
        // it would wait for the file and give up. Never on the way out: a window about to close
        // doesn't open another mailbox first (#134).
        if free
            && !self.close_ready
            && let Some((id, params)) = self.after_save.take()
        {
            self.running_after_save = true;
            let r = self.run(&id, params);
            self.running_after_save = false;
            self.after_save_outcome = Some(match r {
                Ok(result) => json!({"command": id, "ok": true, "result": result}),
                Err(error) => json!({"command": id, "ok": false, "error": error}),
            });
        }
        if !self.autosave || self.session.rev() == self.saved_rev || self.after_save.is_some() {
            return;
        }
        // From the first change on, whatever replaces the mailbox (File › Open, File › New)
        // writes what is unsaved first.
        if let (Some(saver), Some(path)) = (&self.services.saver, &self.session.path) {
            saver.follow(path);
        }
        let now = now_ms();
        if now - self.last_autosave < 3000.0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(1000));
            return;
        }
        self.last_autosave = now;
        let rev = self.session.rev();
        let result = if let Some(persist) = &self.services.persist {
            mailcraft_engine::io::write_mailbox(&self.session.mailbox).and_then(|bytes| persist(&bytes))
        } else if let (Some(saver), Some(path)) = (&self.services.saver, &self.session.path) {
            // A snapshot costs next to nothing (the items are shared), and is only taken of a
            // state not already on its way. Writing it is the saver's business, all of it on its
            // own thread however large the mailbox; `collect_saves` hears how it went. Asking
            // again until then changes nothing.
            saver.submit(path, &self.session.mailbox, rev);
            ctx.request_repaint_after(std::time::Duration::from_millis(1000));
            return;
        } else if self.session.path.is_some() {
            self.session.run("file.save", &json!({})).map(|_| ()).map_err(|e| e.to_string())
        } else {
            return;
        };
        match result {
            Ok(()) => self.saved_rev = rev,
            Err(e) => {
                log::warn!("saving the mailbox failed: {e}");
                self.status(i18n::fmt(tl!("Couldn't save the mailbox: {error}"), &[("error", &e)]));
            }
        }
    }

    /// What the background saver got done since the last look ([`Services::saver`]). Returns
    /// why the open mailbox's last state handed in couldn't be written, if it couldn't.
    fn collect_saves(&mut self) -> Option<String> {
        let reports = self.services.saver.as_ref()?.reports();
        let mut failed = None;
        for report in reports {
            // About a mailbox that isn't the one open any more: nothing to tell the owner.
            if self.session.path.as_deref() != Some(report.path.as_path()) {
                if let Err(e) = &report.result {
                    log::warn!("saving {} failed: {e}", report.path.display());
                }
                continue;
            }
            match report.result {
                Ok(()) => {
                    self.saved_rev = self.saved_rev.max(report.rev);
                    if report.rev == self.session.rev() {
                        self.session.dirty = false;
                    }
                }
                Err(e) => {
                    log::warn!("saving the mailbox failed: {e}");
                    self.status(i18n::fmt(tl!("Couldn't save the mailbox: {error}"), &[("error", &e)]));
                    // Tried again in ten seconds rather than three: a full disk needs a moment.
                    self.last_autosave = now_ms() + 7000.0;
                    failed = Some(e);
                }
            }
        }
        failed
    }

    /// Whether File › New or Open would wait for the background saver to write the mailbox first
    /// ([`Self::through_saver`]), and run later.
    pub(crate) fn waits_for_save(&self) -> bool {
        let (Some(saver), Some(_)) = (&self.services.saver, &self.session.path) else { return false };
        !self.running_after_save && (self.session.rev() != self.saved_rev || saver.progress().is_some())
    }

    /// With a background saver keeping the mailbox file, what writes that file doesn't do it on
    /// the window's thread, where it would wait for whatever the saver is in the middle of (the
    /// first save after a large download) and then do the writing itself: Save hands the mailbox
    /// to the saver as the timer does, and New or Open, which write what is unsaved first, hand
    /// it over and run once it is written, the window going on meanwhile. `None`: the command
    /// runs as it is.
    fn through_saver(&mut self, id: &str, params: &Value) -> Option<Result<Value, String>> {
        let (Some(saver), Some(path)) = (&self.services.saver, &self.session.path) else { return None };
        let rev = self.session.rev();
        match id {
            // Where it is kept. (Saved to another file, which keeps it from then on, it is written
            // there and then, as before.)
            "file.save" if params.get("path").and_then(Value::as_str).is_none_or(|p| std::path::Path::new(p) == path) => {
                saver.submit(path, &self.session.mailbox, rev);
                Some(Ok(json!({"path": path.to_string_lossy(), "background": true})))
            }
            // The window closes once the save it waits for is done: closing wins, and nothing
            // else opens on the way out (#134).
            "file.new" | "file.open" if !self.running_after_save && (self.closing_after_save.is_some() || self.close_ready) => {
                Some(Err(format!("{id} wasn't run: MailCraft closes once the mailbox is saved")))
            }
            "file.new" | "file.open" if !self.running_after_save => {
                if rev != self.saved_rev {
                    saver.submit(path, &self.session.mailbox, rev);
                } else if saver.progress().is_none() {
                    return None;
                }
                // Asked again meanwhile: the newer request takes the place of the older.
                if let Some((before, waited)) = self.after_save.replace((id.to_string(), params.clone())) {
                    opening::forget_unopened(&before, &waited);
                    self.after_save_outcome = Some(json!({"command": before, "ok": false, "error": format!("{id} was asked for instead")}));
                } else {
                    self.after_save_outcome = None;
                }
                Some(Ok(json!({"pending": "save"})))
            }
            _ => None,
        }
    }

    /// A File › New or File › Open waiting for the saver gives way to closing the window: the
    /// owner asked to close after it, and the window would otherwise open the other mailbox the
    /// moment the save is done and then close on it (#134). An agent that was answered
    /// `"pending": "save"` hears that it didn't happen, and why.
    fn drop_for_close(&mut self) {
        // Nor is a mailbox read in the background opened on the way out.
        opening::cancel_ahead(self);
        if let Some((id, params)) = self.after_save.take() {
            opening::forget_unopened(&id, &params);
            self.after_save_outcome = Some(json!({"command": id, "ok": false, "error": "the window was closed instead"}));
        }
    }

    /// How saving is going, for `ui.inspect`: how far the saver has got (`null`: nothing waiting or
    /// being written), whether closing waits for it, the command that waits for it (`file.new`,
    /// `file.open`), and what became of the last one that did.
    pub fn saving_inspect(&self) -> Value {
        use mailcraft_engine::io::SaveStage;
        let progress = self.save_progress.map(|p| {
            let stage = match p.stage {
                SaveStage::Waiting => "waiting",
                SaveStage::Messages => "messages",
                SaveStage::Content => "content",
                SaveStage::Sweeping => "sweeping",
            };
            json!({"stage": stage, "done": p.done, "total": p.total, "millis": p.millis})
        });
        json!({
            "progress": progress,
            "closing": self.closing_after_save.is_some(),
            "pending": self.after_save.as_ref().map(|(id, _)| id),
            "last": self.after_save_outcome,
        })
    }

    /// What the status bar says of a save that is taking a while (the first after a large
    /// download): how far it has got. Nothing for a save of a moment, unless something waits
    /// for it (closing the window, File › Open).
    pub fn saving_text(&self) -> Option<String> {
        use mailcraft_engine::io::SaveStage;
        let p = self.save_progress?;
        let waited_for = self.closing_after_save.is_some() || self.after_save.is_some();
        // Tidying the content folder is nothing to tell, unless something waits for it.
        if !waited_for && (p.millis < SAVING_SHOWN_AFTER_MS || p.stage == SaveStage::Sweeping) {
            return None;
        }
        let (done, total) = (p.done.to_string(), p.total.to_string());
        let text = match p.stage {
            SaveStage::Messages if p.total > 0 => {
                i18n::fmt(tl!("Saving the mailbox… {done} of {total} messages"), &[("done", &done), ("total", &total)])
            }
            SaveStage::Content if p.total > 0 => {
                i18n::fmt(tl!("Saving the mailbox… writing {done} of {total} bodies and attachments"), &[("done", &done), ("total", &total)])
            }
            SaveStage::Sweeping => tl!("Tidying the mailbox's content folder…").to_string(),
            _ => tl!("Saving the mailbox…").to_string(),
        };
        Some(if self.closing_after_save.is_some() {
            i18n::fmt(tl!("{saving} · MailCraft closes when it is done"), &[("saving", &text)])
        } else if self.after_save.is_some() {
            i18n::fmt(tl!("{saving} · the other mailbox opens when it is done"), &[("saving", &text)])
        } else {
            text
        })
    }

    /// What a password's eye has to know of the window (#438): whether it is being captured
    /// (a screenshot is waited for, or every frame is one), and whether this frame's input came
    /// from the control channel.
    pub(crate) fn secret_watch(&self) -> secret_field::Watch {
        let captured = self.headless_shots || !self.queued_shots.is_empty() || !self.pending_shots.is_empty();
        secret_field::Watch { captured, driven: self.driven }
    }

    /// Inject synthetic input events from the control channel (one pointer event per frame).
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        self.driven = !self.synthetic.is_empty();
        if self.synthetic.is_empty() {
            return;
        }
        let n = match self.synthetic.first() {
            Some(egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. }) => 1,
            _ => self.synthetic.iter().position(|e| matches!(e, egui::Event::Key { pressed: false, .. })).map_or(self.synthetic.len(), |i| i + 1),
        };
        if let Some(egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. }) = self.synthetic.first() {
            raw.events.push(egui::Event::PointerMoved(*p));
        }
        let n = n.min(self.synthetic.len());
        raw.events.extend(self.synthetic.drain(..n));
    }

    /// Have the mailbox, and the appointments that stand open, read by the clock of the zone the
    /// calendar is shown in now ([`Session::settle_zone`](mailcraft_engine::Session::settle_zone)):
    /// nothing to do but when that zone has changed. Called with every frame, and by whatever
    /// changes the zone.
    pub fn follow_zone(&mut self) {
        let was = self.session.mailbox.written_in();
        let shown = self.session.zone();
        if was == Some(shown) && self.shown_zone == Some(shown) {
            return;
        }
        // (The forms are told also when the mailbox had nothing in it for a zone to matter to.)
        let before = self.shown_zone.replace(shown).or(was);
        self.session.settle_zone();
        if let Some(before) = before.filter(|before| *before != shown && !shown.is_local()) {
            windows::rezone(self, before);
        }
    }

    /// Lay out the window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let t0 = now_ms();
        let ctx = ui.ctx().clone();
        self.apply_language();
        // Fonts installed by `logic` take effect on the next frame.
        if self.fonts_frames < 2 {
            self.fonts_frames += 1;
            ctx.request_repaint();
            return;
        }
        // Waiting for the mailbox: the opening screen and nothing else.
        if self.opening.is_some() {
            opening::show(self, ui);
            return;
        }
        let t = theme::Tokens::get(&ctx);
        self.hit.clear();
        // What was chosen in a text's menu, for a text of the main window, before anything reads
        // the input.
        let menus = text_menu::deliver(&ctx);
        // (Before the shortcuts take their keys.)
        self.by_hand = owner_input(&ctx);
        // The computer may have been carried to another time zone since the last frame: the
        // mailbox's times are put by the new clock before anything of it is drawn, and so are
        // the appointments that stand open.
        self.follow_zone();
        // Escape and Enter go to one place, and to nothing under it: worked out here, before
        // anything reads them (#406). The agent's question is on top of everything.
        keys::gate_main(self, &ctx);
        agent_gate::keys(self, &ctx);
        // Windows of their own (a message, an appointment), before the shortcuts: their keys are
        // not also the main window's where they are inside it.
        windows::show(self, &ctx);
        sync_panel::escape(self, &ctx);
        // Advanced Search closes when what it was opened for has gone, and on Escape, before the
        // window's Escape ends the search (#329).
        advanced_search::follow(self);
        advanced_search::escape(self, &ctx);
        // Shortcuts first, so the frame shows what the key did.
        keys::global_shortcuts(self, &ctx);
        if self.ui.backstage {
            backstage::show(self, ui);
        } else {
            // The module bar stands beside the tabs and the ribbon, from under the title bar down
            // to the status bar across the window's foot: both are laid out before the ribbon.
            chrome::title_bar(self, ui);
            chrome::sync_bar_rest(self, ui.ctx());
            chrome::status_bar(self, ui);
            // The question about checking for new versions, at the first starts: above the status bar.
            updates::question(self, ui);
            settings_notice::bar(self, ui);
            unconfirmed::bar(self, ui);
            nav::module_bar(self, ui);
            ribbon::show(self, ui);
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.canvas)).show(ui, |ui| match self.session.view.module {
                mailcraft_engine::Module::Mail => mail_view::show(self, ui),
                mailcraft_engine::Module::Calendar => calendar_view::show(self, ui),
                mailcraft_engine::Module::People => people_view::show(self, ui),
                mailcraft_engine::Module::Tasks => tasks_view::show(self, ui),
            });
        }
        sync_panel::show(self, &ctx);
        dialogs::show(self, &ctx);
        reminders::show(self, &ctx);
        // What was chosen in a text's menu, once everything that has one is drawn.
        text_menu::carry_out(self, &ctx);
        // Over everything: what an agent wants that needs the owner.
        agent_gate::show(self, &ctx);
        // While messages are dragged: the pointer for what is under it (#430).
        drag_pointer::apply(&ctx);
        self.by_hand = false;
        self.refusal();
        let title = format!("{} - MailCraft", self.title_stem());
        if title != self.sent_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.sent_title = title;
        }
        // Pictures are kept as textures only while something on screen shows them.
        reading::images::Pictures::of(&ctx).lock().unwrap_or_else(|e| e.into_inner()).release_unseen(&ctx);
        self.frame_ms = now_ms() - t0;
        self.report_dirty();
        text_menu::finish(&ctx, menus);
    }

    /// The window's close button. MailCraft saves by itself, so closing only waits for a message
    /// being written: `false` keeps the window open and asks about it.
    pub fn close_requested(&mut self) -> bool {
        // What closing waited for being written lets this one close request through, and no
        // later one: held below by a question, the close the owner asks for next waits again for
        // whatever the saver is at by then.
        let (ready, anyway) = (std::mem::take(&mut self.close_ready), std::mem::take(&mut self.close_anyway));
        // Still opening: nothing to wait for or write.
        if self.opening.is_some() {
            return true;
        }
        // (Its saved draft gone, what is written is saved nowhere: #494.)
        if self.session.compose.as_ref().is_some_and(|c| c.dirty || mailcraft_engine::cmd::compose::draft_gone(&self.session, c)) {
            self.dialog = Some(dialogs::leaving("quit", json!({})));
            return false;
        }
        // An appointment window with unsaved changes asks about them first.
        if windows::hold_quit(self) {
            return false;
        }
        // What closing waited for is written, or couldn't be (the owner was told): it closes.
        if ready || anyway {
            self.drop_for_close();
            return true;
        }
        // What is still to be written (the first save after a large download can take minutes)
        // is written with the window still there, saying how far it has got; it closes when that
        // is done. Asked again meanwhile, it goes on waiting, saying so: closing then would leave
        // the last changes unwritten, for `save_on_exit` waits a while at most.
        if let (Some(saver), Some(path)) = (&self.services.saver, &self.session.path) {
            if self.closing_after_save.is_some() {
                self.quit_requested = false;
                return false;
            }
            let rev = self.session.rev();
            let unsaved = self.autosave && rev != self.saved_rev;
            if unsaved {
                saver.submit(path, &self.session.mailbox, rev);
            }
            // (A sweep of the content folder writes nothing of the mailbox: it isn't waited for,
            // #586, and is left where it got to.)
            let writing = saver.progress().is_some_and(|p| p.stage != mailcraft_engine::io::SaveStage::Sweeping);
            if unsaved || writing {
                self.closing_after_save = Some(rev);
                self.quit_requested = false;
                // Closing waits for the save now: an Open that waited for it too gives way.
                self.drop_for_close();
                return false;
            }
        }
        self.drop_for_close();
        true
    }

    /// The window is going away: write the changes the timer hadn't got to yet, and wait for
    /// whatever is still being written, for a while at most (the window waited for the long save
    /// already, [`MailApp::close_requested`]: what is left is what changed since, a short one).
    /// `Err` says why the mailbox couldn't be saved.
    pub fn save_on_exit(&mut self) -> Result<(), String> {
        // Its data was removed from this computer: writing now would bring some of it back.
        // And a window that was still opening its mailbox has nothing to write.
        if self.data_removed || self.opening.is_some() {
            return Ok(());
        }
        let unsaved = self.autosave && self.session.path.is_some() && (self.session.dirty || self.session.rev() != self.saved_rev);
        if let (Some(saver), Some(path)) = (&self.services.saver, &self.session.path) {
            // Through the saver too, never in place: a save in place would wait out whatever the
            // saver is in the middle of, for as long as that takes. The newest state goes in, an
            // older one still waiting gives way to it, and the wait for both is bounded.
            let rev = self.session.rev();
            if unsaved {
                saver.submit(path, &self.session.mailbox, rev);
            }
            saver.flush();
            let failed = self.collect_saves();
            if unsaved && self.saved_rev < rev {
                return Err(failed
                    .unwrap_or_else(|| "it was still being written when MailCraft stopped waiting; the file is as the last save left it".into()));
            }
            return Ok(());
        }
        let mut result = Ok(());
        if unsaved {
            // In place: this is the newest state, and an older one still waiting gives way to it.
            result = self.session.run("file.save", &json!({})).map(|_| ()).map_err(|e| e.to_string());
            if result.is_ok() {
                self.saved_rev = self.session.rev();
            }
        }
        result
    }

    fn drain_control(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.control_rx.take() else { return };
        while let Ok(req) = rx.try_recv() {
            let reply = req.reply.clone();
            match control::handle(self, ctx, &req) {
                control::Outcome::Done(v) => {
                    let _ = reply.send(v);
                }
                control::Outcome::Screenshot { path, inline } => {
                    self.shot_token += 1;
                    let token = self.shot_token;
                    self.queued_shots.push((token, now_ms() + 120.0, 0));
                    self.pending_shots.push((token, path, inline, reply, now_ms() + 8000.0));
                }
                // The owner is asked first; the answer goes back from there.
                control::Outcome::Pending => {}
            }
        }
        self.control_rx = Some(rx);
    }

    fn issue_screenshots(&mut self, ctx: &egui::Context) {
        let now = now_ms();
        let sink = self.shot_images.clone();
        self.queued_shots.retain_mut(|(token, at, frames)| {
            *frames += 1;
            if now >= *at && *frames >= 3 {
                let token = *token;
                let sink = sink.clone();
                let repaint = ctx.clone();
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |image| {
                    sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((token, image));
                    repaint.request_repaint();
                })));
                false
            } else {
                true
            }
        });
        if !self.queued_shots.is_empty() || !self.pending_shots.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn collect_screenshots(&mut self) {
        if self.pending_shots.is_empty() {
            return;
        }
        let events: Vec<_> = std::mem::take(&mut *self.shot_images.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        for (token, image) in events {
            if let Some(i) = self.pending_shots.iter().position(|(t, ..)| *t == token) {
                let (_, path, inline, reply, _) = self.pending_shots.remove(i);
                let _ = reply.send(if inline { control::inline_screenshot(&image) } else { control::save_screenshot(&image, path.as_deref()) });
            }
        }
        let now = now_ms();
        self.pending_shots.retain(|(_, _, _, reply, deadline)| {
            if now < *deadline {
                return true;
            }
            let _ = reply.send(json!({"ok": false, "error": "no frame was presented (window hidden?)"}));
            false
        });
    }
}

/// What the owner is told when part of the mailbox file was damaged and left out as it opened.
pub fn damaged_mailbox_notice() -> String {
    tl!("Part of the mailbox file is damaged and was left out, so some recent changes may be missing. The file as it was is kept beside it the next time the mailbox is saved.").to_string()
}

/// What the status bar says after a command the user ran, when it is worth saying.
/// The window's input this frame came from the owner's hand: a press or a release of the pointer,
/// or a key going down (#152). Moving the pointer about isn't doing anything.
fn owner_input(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::PointerButton { .. } | egui::Event::Key { pressed: true, .. })))
}

/// Whether a command's refusal of what the owner just did is said in a message box (#152). The
/// window's own commands (`ui.*`) say what they need where they are; and a command that wasn't
/// available (`not available: …`), was given what it can't take, or had nothing to do is no
/// refusal: those stay in the status bar.
pub(crate) fn refusal_worth_a_box(id: &str, error: &str) -> bool {
    let quiet = ["not available:", "unknown command", "bad parameters:", "nothing "];
    let lower = error.trim_start().to_lowercase();
    !id.starts_with("ui.") && !error.trim().is_empty() && !quiet.iter().any(|q| lower.starts_with(q))
}

fn feedback(id: &str, v: &Value) -> Option<String> {
    let n = |k: &str| v.get(k).and_then(Value::as_u64);
    match id {
        "compose.send" if v.get("queued").and_then(Value::as_bool) == Some(true) => Some(tl!("In the Outbox: it is on its way").to_string()),
        // Mail put into a Microsoft account from elsewhere: Microsoft's server makes it without
        // most of the original's headers, which is said rather than hidden (#451).
        "mail.move" => n("intoMicrosoft").filter(|c| *c > 0).map(|c| {
            tln!(
                c,
                "{n} message moved into the account: Send/Receive puts it on Microsoft's server, without most of its headers (such as Received)",
                "{n} messages moved into the account: Send/Receive puts them on Microsoft's server, without most of their headers (such as Received)"
            )
        }),
        "mail.copy" => n("intoMicrosoft").filter(|c| *c > 0).map(|c| {
            tln!(
                c,
                "{n} message copied into the account: Send/Receive puts it on Microsoft's server, without most of its headers (such as Received)",
                "{n} messages copied into the account: Send/Receive puts them on Microsoft's server, without most of their headers (such as Received)"
            )
        }),
        // A mailbox whose content folder lost something still opens; say how much is missing. A
        // mailbox file damaged part of the way opens as far as it can be read: that is said first.
        "file.open" => {
            let damaged = ["lostSaves", "unfinished", "lostRecords"].into_iter().any(|k| n(k).is_some_and(|c| c > 0));
            let lost = if damaged {
                Some(damaged_mailbox_notice())
            } else {
                n("unavailable").filter(|c| *c > 0).map(|c| {
                    tln!(
                        c,
                        "{n} part of a message couldn't be read from the mailbox's content folder",
                        "{n} parts of messages couldn't be read from the mailbox's content folder"
                    )
                })
            };
            // A mailbox whose Favorites were read another way than Microsoft Graph: said once.
            let graph = n("favoritesNowGraph").filter(|c| *c > 0).map(|_| tl!(favorites_sync::FAVORITES_NOW_GRAPH).to_string());
            match (lost, graph) {
                (Some(lost), Some(graph)) => Some(format!("{}. {graph}", lost.trim_end_matches('.'))),
                (lost, graph) => lost.or(graph),
            }
        }
        "compose.send" => Some(tl!("Sent").to_string()),
        "compose.saveDraft" => Some(tl!("Draft saved").to_string()),
        // A draft its JMAP account's server has isn't deleted yet (M6): the editor closes, and why
        // the draft is still in Drafts is said, as the Move menu says it.
        "compose.discard" => {
            v.get("stays").and_then(Value::as_str).map(|why| i18n::fmt(tl!("The draft stays in Drafts. {why}"), &[("why", tl!(why))]))
        }
        "mail.markAllRead" | "rules.run" => n("changed").or(n("matched")).map(|c| tln!(c, "{n} message changed", "{n} messages changed")),
        "file.importFolderTree" => {
            let messages = n("messages")?;
            let what = tln!(messages, "{n} message", "{n} messages");
            let left = n("skippedCount").unwrap_or(0);
            let name = v.get("name").and_then(Value::as_str).unwrap_or_default();
            let said = i18n::fmt(tl!("Imported {what} into the new folder {name}"), &[("what", &what), ("name", name)]);
            let stopped = v.get("stopped").and_then(Value::as_bool) == Some(true);
            Some(match (left, stopped) {
                (0, false) => said,
                (left, _) => {
                    let left = tln!(left, "{n} file left out", "{n} files left out");
                    i18n::fmt(
                        tl!("{said}; {left}{more}"),
                        &[("said", &said), ("left", &left), ("more", if stopped { tl!(", stopped at a limit") } else { "" })],
                    )
                }
            })
        }
        "file.import" => {
            let mut parts = Vec::new();
            let counts: [(&str, fn(u64) -> String); 4] = [
                ("messages", |c| tln!(c, "{n} message", "{n} messages")),
                ("events", |c| tln!(c, "{n} event", "{n} events")),
                ("tasks", |c| tln!(c, "{n} task", "{n} tasks")),
                ("contacts", |c| tln!(c, "{n} contact", "{n} contacts")),
            ];
            for (key, said) in counts {
                if let Some(c) = n(key).filter(|c| *c > 0) {
                    parts.push(said(c));
                }
            }
            let what = parts.join(", ");
            // Mail meant for a Microsoft account went to a local folder (R3 spec S30); moved into
            // the account from there, it is put on the server (#451). Drafts stay in their own
            // mailbox, this computer's.
            let local = v.get("local").and_then(Value::as_bool) == Some(true);
            if local && v.get("drafts").and_then(Value::as_bool) == Some(true) {
                return Some(i18n::fmt(
                    tl!("Imported {what} into Drafts on this computer: a draft stays on this computer until it is sent"),
                    &[("what", &what)],
                ));
            }
            if local {
                return Some(i18n::fmt(
                    tl!("Imported {what} into a local folder: move it into the account to put it on Microsoft's server"),
                    &[("what", &what)],
                ));
            }
            if let Some(c) = n("stoppedAfter") {
                let count = tln!(c, "{n} message", "{n} messages");
                return Some(i18n::fmt(
                    tl!("Imported {what}; stopped after {count}: the file has more than one import takes"),
                    &[("what", &what), ("count", &count)],
                ));
            }
            (!parts.is_empty()).then(|| i18n::fmt(tl!("Imported {what}"), &[("what", &what)]))
        }
        "people.fromMessage" if v.get("existing").and_then(Value::as_bool) == Some(true) => Some(tl!("Already in your contacts").to_string()),
        "folder.empty" => n("removed").map(|c| tln!(c, "{n} message removed", "{n} messages removed")),
        // An appointment that goes into another calendar of its account is made there first, and
        // taken out of the one it was in once that is done: said, as it takes a moment.
        "calendar.edit" if v.get("moved").and_then(Value::as_bool) == Some(true) => {
            Some(tl!("Moving it to the other calendar: the account's server makes it there, then takes it out of this one").to_string())
        }
        // What went into the account's calendar, what was there already, and what stayed.
        "calendar.moveToAccount" => {
            let mut parts =
                vec![tln!(n("moved").unwrap_or(0), "{n} item moved to your account's calendar", "{n} items moved to your account's calendar")];
            if let Some(c) = n("already").filter(|c| *c > 0) {
                parts.push(tln!(c, "{n} was there already", "{n} were there already"));
            }
            let kept = v.get("kept").and_then(Value::as_array).map_or(0, Vec::len);
            if kept > 0 {
                parts.push(tln!(kept, "{n} stays on this computer", "{n} stay on this computer"));
            }
            Some(parts.join("; "))
        }
        _ => None,
    }
}

/// Wall-clock milliseconds since the Unix epoch: the system clock, or the browser's on the web.
pub fn now_ms() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
    }
    // The std clock panics on wasm32-unknown-unknown, so ask the browser's.
    #[cfg(target_arch = "wasm32")]
    {
        let ms = js_sys::Date::now();
        if ms.is_finite() && ms > 0.0 { ms } else { 0.0 }
    }
}

#[cfg(test)]
mod privacy_tests;
#[cfg(test)]
mod product_names_tests;
#[cfg(test)]
mod tests;
