// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Desktop Alerts (R7.1): a notification MailCraft asks the operating system to show while it
//! runs, for a reminder falling due and for new mail in an Inbox of the owner's own accounts.
//!
//! - A reminder's alert comes as it falls due, whether the window is in front or not, beside the
//!   reminders window ([`crate::reminders`]); a reminder put off comes again when it is due again.
//! - New mail is told of only while the window is not in front, once a Send/Receive is over: what
//!   arrived is the engine's answer ([`mailcraft_engine::arrivals`]: never an account's first
//!   download, mail a rule moved away, Junk E-mail or other folders, a shared mailbox, mail
//!   already read).
//! - Up to [`AT_ONCE`] at a time get an alert each; more get one that counts them.
//! - What an alert says is the message's own text, and so hostile: the sender's name and the
//!   subject only (never the body), as plain text, without control or format characters
//!   (bidirectional overrides, zero-width marks), cut to [`NAME_CHARS`] and [`SUBJECT_CHARS`].
//! - The window asks the host to show them ([`AlertHost`], [`crate::Services::alert`]); there is
//!   none in the browser, the command line or the tests, and then nothing is shown. MailCraft
//!   plays no sound of its own: sound and Do Not Disturb are the system's.
//! - A click on one, where the system says so, brings the window in front and opens the message
//!   (a reminder's: the reminders window). The host hands back only the window's own number for
//!   the alert, which the window looks up in what it showed.
//! - Nothing can ask for an alert of its own making: no command and no control method shows one.
//!   The window keeps a list of what it showed, for scripts and tests to read (`ui.alerts`).
//! - Alerts are not taken back when the message is read or the reminder dismissed.

use std::collections::VecDeque;

use mailcraft_engine::arrivals::{self, Arrival, InboxMark};
use mailcraft_engine::cmd::view::Reminder;
use mailcraft_model::Id;
use serde::Serialize;
use serde_json::{Value, json};

use crate::MailApp;

/// How many get an alert each; beyond that, one alert counts them.
pub const AT_ONCE: usize = 3;
/// How much of a sender's name is shown, in characters.
pub const NAME_CHARS: usize = 64;
/// How much of a subject, a title or a place is shown, in characters.
pub const SUBJECT_CHARS: usize = 120;
/// How many alerts shown are remembered for `ui.alerts`.
const KEPT: usize = 200;

/// What an alert is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AlertKind {
    Mail,
    Reminder,
}

/// What a click on an alert opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "opens")]
pub enum Opens {
    /// The message, in its folder.
    Message { message: Id, folder: Id },
    /// A folder (more new mail than gets an alert each: the Inbox it came to).
    Folder { folder: Id },
    /// The reminders window.
    Reminders,
}

/// An alert the window asks the host to show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAlert {
    /// The window's own number for it: a click comes back with this, and with nothing else.
    pub key: u64,
    pub kind: AlertKind,
    /// The first line (the sender, the appointment), plain text, cleaned ([`clean`]).
    pub title: String,
    /// The second line (the subject, the time and place), plain text, cleaned.
    pub body: String,
    #[serde(flatten)]
    pub opens: Opens,
}

/// Why the system won't show MailCraft's alerts, as far as the host can tell
/// ([`AlertHost::trouble`]). Said beside the switches in File › Options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertTrouble {
    /// Windows: this copy wasn't installed by the `.msi`, so Windows doesn't know it as a program
    /// that may show notifications.
    NotInstalled,
    /// macOS: this copy doesn't run from the MailCraft app.
    OutsideApp,
    /// The owner (or an organisation's policy) turned MailCraft's notifications off in the
    /// system's settings.
    TurnedOff,
    /// Linux and the BSDs: no notification service answers on the desktop.
    NoService,
}

impl AlertTrouble {
    /// What the owner is told, under the switches.
    pub fn notice(self) -> &'static str {
        match self {
            AlertTrouble::NotInstalled => {
                tl!("Desktop alerts can't be shown: this copy of MailCraft wasn't installed with its installer (.msi), so Windows doesn't know it.")
            }
            AlertTrouble::OutsideApp => tl!("Desktop alerts can't be shown: this copy doesn't run from the MailCraft app."),
            AlertTrouble::TurnedOff => tl!("Desktop alerts are turned off for MailCraft in System Settings."),
            AlertTrouble::NoService => tl!("Desktop alerts can't be shown: no notification service answers on this desktop."),
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            AlertTrouble::NotInstalled => "notInstalled",
            AlertTrouble::OutsideApp => "outsideApp",
            AlertTrouble::TurnedOff => "turnedOff",
            AlertTrouble::NoService => "noService",
        }
    }
}

/// The system's notifications ([`crate::Services::alert`]).
pub trait AlertHost {
    /// Ask the system to show `alert`. An error says why it couldn't (for the log).
    fn show(&self, alert: &DesktopAlert) -> Result<(), String>;
    /// Why the system won't show them, when that is known; `None` when it will (or may).
    fn trouble(&self) -> Option<AlertTrouble> {
        None
    }
    /// The alerts clicked since the last call, by their [`DesktopAlert::key`]. Returns at once.
    fn clicked(&self) -> Vec<u64> {
        Vec::new()
    }
}

/// The window's part.
#[derive(Default)]
pub struct State {
    /// What was shown, oldest first, at most [`KEPT`].
    shown: VecDeque<DesktopAlert>,
    next_key: u64,
    /// The mailbox as the last frame without a Send/Receive found it, and the file it was.
    idle: Option<(InboxMark, Option<std::path::PathBuf>)>,
    idle_rev: u64,
    /// The mark a Send/Receive under way started from.
    run: Option<(InboxMark, Option<std::path::PathBuf>)>,
}

/// Text from a message (or an appointment) made fit for an alert: one line of plain text, without
/// control characters (a line break or a tab becomes a space) or Unicode format characters
/// (bidirectional overrides and isolates, zero-width marks), spaces run together, at most `most`
/// characters with an ellipsis where it was cut.
pub fn clean(text: &str, most: usize) -> String {
    // One more than fits says that it didn't; nothing beyond that is looked at.
    let mut chars: Vec<char> = Vec::new();
    let mut space = false;
    for c in text.chars() {
        if mailcraft_engine::html::is_format(c) {
            continue;
        }
        if c.is_control() || c.is_whitespace() {
            space = !chars.is_empty();
            continue;
        }
        if std::mem::take(&mut space) {
            chars.push(' ');
        }
        chars.push(c);
        if chars.len() > most {
            break;
        }
    }
    if chars.len() <= most {
        return chars.into_iter().collect();
    }
    let mut kept: String = chars.into_iter().take(most.saturating_sub(1)).collect();
    kept.truncate(kept.trim_end().len());
    if most > 0 {
        kept.push('…');
    }
    kept
}

/// An alert to be shown, before it has its number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub kind: AlertKind,
    pub title: String,
    pub body: String,
    pub opens: Opens,
}

/// The alerts for new mail that arrived in a run: none while the window is in front or the
/// switch is off; one each for up to [`AT_ONCE`] messages, else one that counts them.
pub fn plan_mail(found: &[Arrival], on: bool, focused: bool) -> Vec<Planned> {
    if !on || focused || found.is_empty() {
        return Vec::new();
    }
    if found.len() > AT_ONCE {
        let first = found.first().map_or(0, |a| a.folder);
        return vec![Planned {
            kind: AlertKind::Mail,
            title: tl!("New mail").to_string(),
            body: tln!(found.len(), "{n} new message", "{n} new messages"),
            opens: Opens::Folder { folder: first },
        }];
    }
    found
        .iter()
        .map(|a| {
            let sender = clean(&a.sender, NAME_CHARS);
            let subject = clean(&a.subject, SUBJECT_CHARS);
            Planned {
                kind: AlertKind::Mail,
                title: if sender.is_empty() { tl!("New message").to_string() } else { sender },
                body: if subject.is_empty() { tl!("(no subject)").to_string() } else { subject },
                opens: Opens::Message { message: a.message, folder: a.folder },
            }
        })
        .collect()
}

/// The alerts for reminders that have just fallen due: one each for up to [`AT_ONCE`], else one
/// that counts them; none when the switch is off.
pub fn plan_reminders(due: &[Reminder], on: bool, twelve_hour: bool) -> Vec<Planned> {
    if !on || due.is_empty() {
        return Vec::new();
    }
    if due.len() > AT_ONCE {
        return vec![Planned {
            kind: AlertKind::Reminder,
            title: tl!("Reminders").to_string(),
            body: tln!(due.len(), "{n} reminder is due", "{n} reminders are due"),
            opens: Opens::Reminders,
        }];
    }
    due.iter()
        .map(|r| {
            let title = clean(&r.title, SUBJECT_CHARS);
            let time = if r.all_day { crate::dates::medium(r.start.date) } else { crate::dates::medium_time(r.start, twelve_hour) };
            let place = clean(&r.location, SUBJECT_CHARS);
            Planned {
                kind: AlertKind::Reminder,
                title: if title.is_empty() { tl!("(no title)").to_string() } else { title },
                body: if place.is_empty() { time } else { format!("{time} · {place}") },
                opens: Opens::Reminders,
            }
        })
        .collect()
}

/// Show what was planned, through the host when there is one; what was shown is remembered.
fn show(app: &mut MailApp, planned: Vec<Planned>) {
    let Some(host) = &app.services.alert else { return };
    for p in planned {
        app.alerts.next_key = app.alerts.next_key.wrapping_add(1);
        let alert = DesktopAlert { key: app.alerts.next_key, kind: p.kind, title: p.title, body: p.body, opens: p.opens };
        match host.show(&alert) {
            Ok(()) => {
                if app.alerts.shown.len() >= KEPT {
                    app.alerts.shown.pop_front();
                }
                app.alerts.shown.push_back(alert);
            }
            // (Nothing of the message in the log: only that it failed, and why.)
            Err(e) => log::debug!("a desktop alert wasn't shown: {e}"),
        }
    }
}

/// Reminders that have just fallen due ([`crate::reminders::tick`]).
pub fn reminders_due(app: &mut MailApp, due: &[Reminder]) {
    let planned = plan_reminders(due, app.ui.alert_reminders, app.session.prefs.twelve_hour);
    show(app, planned);
}

/// Whether a Send/Receive is under way, or waiting its turn.
fn busy(app: &MailApp) -> bool {
    app.syncing() || app.graph_rx.is_some() || !app.sync_queue.is_empty()
}

/// Every frame, before anything else: mark the mailbox while no Send/Receive is under way; once one
/// is over, tell of the new mail it brought; take in the clicks on alerts.
pub fn tick(app: &mut MailApp, ctx: &egui::Context) {
    clicks(app, ctx);
    let path = app.session.path.clone();
    if busy(app) {
        if app.alerts.run.is_none() {
            app.alerts.run = app.alerts.idle.take();
        }
        return;
    }
    if let Some((before, was)) = app.alerts.run.take()
        && was == path
    {
        let found = arrivals::arrivals(&app.session.mailbox, &before);
        let focused = ctx.input(|i| i.focused);
        let planned = plan_mail(&found, app.ui.alert_new_mail, focused);
        show(app, planned);
    }
    let rev = app.session.rev();
    if app.alerts.idle.is_none() || app.alerts.idle_rev != rev || app.alerts.idle.as_ref().is_some_and(|(_, p)| *p != path) {
        app.alerts.idle = Some((arrivals::mark(&app.session.mailbox), path));
        app.alerts.idle_rev = rev;
    }
}

/// Clicks on alerts: the window comes in front, and what the alert was about is opened.
fn clicks(app: &mut MailApp, ctx: &egui::Context) {
    let Some(host) = &app.services.alert else { return };
    let keys = host.clicked();
    for key in keys.into_iter().take(20) {
        let Some(opens) = app.alerts.shown.iter().find(|a| a.key == key).map(|a| a.opens) else { continue };
        open(app, ctx, opens);
    }
}

/// Bring the window in front and open what an alert was about.
pub fn open(app: &mut MailApp, ctx: &egui::Context, opens: Opens) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    match opens {
        Opens::Message { message, folder } => {
            app.ui.backstage = false;
            if app.session.mailbox.message(message).is_some_and(|m| m.folder == folder) {
                let _ = app.execute("folder.select", json!({"id": folder}));
                let _ = app.execute("mail.select", json!({"id": message}));
                app.reveal_selection = true;
            } else if app.session.mailbox.folder(folder).is_some() {
                // Moved or gone since: its folder, at least.
                let _ = app.execute("folder.select", json!({"id": folder}));
            }
        }
        Opens::Folder { folder } => {
            app.ui.backstage = false;
            if app.session.mailbox.folder(folder).is_some() {
                let _ = app.execute("folder.select", json!({"id": folder}));
            }
        }
        Opens::Reminders => app.reminders.asked_for = true,
    }
}

/// `ui.alerts`: the switches, whether alerts can be shown here, and the alerts shown, newest last.
pub fn inspect(app: &MailApp, p: &Value) -> Value {
    let most = p.get("limit").and_then(Value::as_u64).map_or(KEPT, |n| usize::try_from(n).unwrap_or(KEPT).min(KEPT));
    let skip = app.alerts.shown.len().saturating_sub(most);
    let shown: Vec<&DesktopAlert> = app.alerts.shown.iter().skip(skip).collect();
    let trouble = app.services.alert.as_ref().and_then(|h| h.trouble());
    json!({
        "newMail": app.ui.alert_new_mail,
        "reminders": app.ui.alert_reminders,
        "available": app.services.alert.is_some() && trouble.is_none(),
        "trouble": trouble.map(AlertTrouble::code),
        "shown": shown,
    })
}

/// File › Options: the two switches, and why alerts can't be shown when they can't.
pub(crate) fn options(app: &mut MailApp, ui: &mut egui::Ui, t: &crate::theme::Tokens) {
    let mut mail = app.ui.alert_new_mail;
    if ui
        .checkbox(&mut mail, tl!("Show a desktop alert when new mail arrives"))
        .on_hover_text(tl!("Only while MailCraft isn't the window in front, and only for the Inboxes of your own accounts."))
        .changed()
    {
        let _ = app.run("ui.desktopAlerts", json!({"newMail": mail}));
    }
    let mut reminders = app.ui.alert_reminders;
    if ui.checkbox(&mut reminders, tl!("Show a desktop alert for reminders")).changed() {
        let _ = app.run("ui.desktopAlerts", json!({"reminders": reminders}));
    }
    if let Some(trouble) = app.services.alert.as_ref().and_then(|h| h.trouble()) {
        ui.label(egui::RichText::new(trouble.notice()).color(t.text_dim));
    }
}

#[cfg(test)]
#[path = "alerts_tests.rs"]
mod tests;
