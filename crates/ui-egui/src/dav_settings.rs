// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars and Contacts (#209, D7): where an account's calendars and contacts are kept, the
//! name they sign in with, and whether they have a password of their own, in File › Info.
//!
//! The dialog collects what `account.davSet` takes and runs it; Stop using these servers runs
//! `account.davOff`. Saving sends nothing: the DAV job reads the servers by itself, at its next
//! run. Find is the only thing here that sends: the host searches from the typed addresses, or
//! from the account's domain (`dav_find.rs`), asks before the password may go to a host that
//! isn't the owner's, and fills in the homes it found; the owner then saves them. A password typed here is never a command's parameter. It is held in memory for the origin
//! the field names ([`MailApp::hold_dav_password`]), and for no other: calendars and contacts on
//! one server (Infomaniak's) share it, and contacts on another server ask for their own in File ›
//! Info (`dav_password_ask.rs`). It is remembered only once that server took it, as the owner
//! allows (`dav_secret_read.rs`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use egui::Ui;
use mailcraft_engine::cmd::dav_account::may_have;
use mailcraft_model::{Account, DavFindFailure, DavFindQuestion, DavFound, DavProblem, Id, origin_of};
use serde_json::json;

use crate::dav_find::{DavFindEvent, DavFindJob};

use crate::MailApp;
use crate::dialogs::{buttons, window};
use crate::theme::{Tokens, regular};

/// The dialog's state: what the owner is typing.
pub struct DavSettings {
    pub(crate) account: Id,
    pub(crate) calendars: String,
    pub(crate) contacts: String,
    pub(crate) username: String,
    /// The servers want a password of their own (not the account's mail password).
    pub(crate) separate: bool,
    /// The password for [`DavSettings::password_origin`], when the servers have their own.
    pub(crate) password: String,
    /// The owner clicked the eye: the password is shown until the next click or the dialog's end.
    shown: bool,
    pub(crate) error: String,
    /// The search under way, when the owner pressed Find.
    pub(crate) finding: Option<Finding>,
    /// How the last search ended.
    pub(crate) found: Option<Result<DavFound, DavFindFailure>>,
}

/// A search under way: what the host tells, where the owner's answers go, and the question open.
pub(crate) struct Finding {
    events: Receiver<DavFindEvent>,
    answers: Sender<bool>,
    cancel: Arc<AtomicBool>,
    pub(crate) question: Option<(String, DavFindQuestion)>,
}

/// A dialog that goes away stops its search.
impl Drop for DavSettings {
    fn drop(&mut self) {
        if let Some(finding) = &self.finding {
            finding.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// What became of the dialog this frame.
pub enum Outcome {
    Open,
    Close,
}

/// The host of an origin, as it is shown (`dav.example.org`, with its port when it has one).
fn host(origin: &str) -> &str {
    origin.strip_prefix("https://").unwrap_or(origin)
}

impl DavSettings {
    pub(crate) fn name(&self) -> &'static str {
        "davSettings"
    }

    /// For the account `account`, with its settings as they are. `None` while the feature is off,
    /// or for an account that can't have them (`account.davSet`'s own rule).
    pub(crate) fn open(app: &MailApp, account: Id) -> Option<Self> {
        if !app.session.dav_enabled {
            return None;
        }
        let a = app.session.mailbox.account(account).filter(|a| may_have(a))?;
        let server = a.dav.as_ref();
        let first = |homes: Option<&Vec<String>>| homes.and_then(|h| h.first()).cloned().unwrap_or_default();
        Some(Self {
            account,
            calendars: first(server.map(|s| &s.calendar_homes)),
            contacts: first(server.map(|s| &s.contact_homes)),
            username: server.map(|s| s.username.clone()).unwrap_or_default(),
            separate: server.is_some_and(|s| s.separate_password),
            password: String::new(),
            shown: false,
            error: String::new(),
            finding: None,
            found: None,
        })
    }

    /// The password Find signs in with: the one typed when the servers have their own, else the
    /// account's mail password as the window has it. `None` when there is none yet.
    pub(crate) fn find_password(&self, app: &MailApp) -> Option<String> {
        if self.separate {
            return (!self.password.is_empty()).then(|| self.password.clone());
        }
        app.passwords.get(&self.account).cloned().or_else(|| app.remembered("password", self.account)).filter(|p| !p.is_empty())
    }

    /// Start the search: from the typed addresses, else from the account's domain. Nothing is
    /// sent before this. False when the host can't search or there is no password.
    pub(crate) fn find(&mut self, app: &MailApp) -> bool {
        let (Some(search), Some(password)) = (app.services.dav_find.as_ref(), self.find_password(app)) else { return false };
        let Some(a) = app.session.mailbox.account(self.account) else { return false };
        let (answers, asked) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = DavFindJob {
            email: a.email.trim().to_string(),
            mail_host: a.incoming.as_ref().map(|s| s.host.trim().to_string()).unwrap_or_default(),
            username: self.username.trim().to_string(),
            password,
            calendars: self.calendars.trim().to_string(),
            contacts: self.contacts.trim().to_string(),
            cancel: cancel.clone(),
            answers: asked,
        };
        self.error.clear();
        self.found = None;
        self.finding = Some(Finding { events: search(job), answers, cancel, question: None });
        true
    }

    /// Take in what the host told since the last frame.
    pub(crate) fn poll(&mut self) {
        let Some(finding) = self.finding.as_mut() else { return };
        loop {
            match finding.events.try_recv() {
                Ok(DavFindEvent::Question { host, why }) => finding.question = Some((host, why)),
                Ok(DavFindEvent::Finished(end)) => {
                    if let Ok(found) = &end {
                        if let Some(home) = found.calendar_homes.first() {
                            self.calendars = home.clone();
                        }
                        if let Some(home) = found.contact_homes.first() {
                            self.contacts = home.clone();
                        }
                    }
                    self.found = Some(end);
                    self.finding = None;
                    return;
                }
                Err(TryRecvError::Empty) => return,
                // (The host stopped without an end: said as nothing found.)
                Err(TryRecvError::Disconnected) => {
                    self.found = Some(Err(DavFindFailure::Unreachable));
                    self.finding = None;
                    return;
                }
            }
        }
    }

    /// The owner's answer to the open question.
    pub(crate) fn answer(&mut self, yes: bool) {
        if let Some(finding) = self.finding.as_mut()
            && finding.question.take().is_some()
        {
            let _ = finding.answers.send(yes);
        }
    }

    /// The origin the password field is for: the calendars', else the contacts'.
    pub(crate) fn password_origin(&self) -> Option<String> {
        origin_of(self.calendars.trim()).or_else(|| origin_of(self.contacts.trim()))
    }

    /// Whether the contacts are on another server than the calendars (which asks for its own).
    fn contacts_elsewhere(&self) -> bool {
        let card = origin_of(self.contacts.trim());
        card.is_some() && origin_of(self.calendars.trim()).is_some_and(|cal| Some(cal) != card)
    }

    /// Keep the settings, and hold the password typed for its origin. False, with the reason in
    /// [`DavSettings::error`], when the command refused them.
    pub(crate) fn save(&mut self, app: &mut MailApp) -> bool {
        let homes = |s: &str| if s.trim().is_empty() { vec![] } else { vec![s.trim().to_string()] };
        let params = json!({
            "id": self.account,
            "calendarHomes": homes(&self.calendars),
            "contactHomes": homes(&self.contacts),
            "username": self.username.trim(),
            "separatePassword": self.separate,
        });
        if let Err(e) = app.execute("account.davSet", params) {
            self.error = e;
            return false;
        }
        if self.separate
            && !self.password.is_empty()
            && let Some(origin) = self.password_origin()
            && app.hold_dav_password(self.account, &origin, &self.password)
        {
            app.dav.trouble.remove(&self.account);
        }
        // (The settings are new: the job may look at once.)
        app.dav.next = 0.0;
        true
    }
}

/// A small dim line under a field.
fn hint(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.label(egui::RichText::new(text).color(t.text_dim).size(11.0));
}

/// What the account's last read of its calendar servers came to, in words; `None` before any did.
pub(crate) fn status(app: &MailApp, a: &Account) -> Option<String> {
    let server = a.dav.as_ref()?;
    let said = match app.dav_trouble(a.id) {
        Some(DavProblem::NeedsPassword | DavProblem::WrongPassword | DavProblem::UnsupportedSignIn) => tl!("The server wouldn't sign in."),
        Some(DavProblem::Certificate) => tl!("The server's certificate isn't trusted, so nothing was sent to it."),
        Some(DavProblem::Unreachable | DavProblem::Busy) => tl!("The server couldn't be reached. MailCraft tries again later."),
        Some(DavProblem::Redirected | DavProblem::NotCalendarServer | DavProblem::Refused) => {
            tl!("The address doesn't lead to calendars or contacts. Check it in Change….")
        }
        Some(DavProblem::ServerErrors) => tl!("The server answered some requests with errors. MailCraft asks again later."),
        Some(DavProblem::Full) => tl!("The mailbox holds as much from this server as it may."),
        Some(DavProblem::None | DavProblem::Limits) | None => {
            let n = server.collections.len();
            if n == 0 {
                return None;
            }
            return Some(tln!(n, "{n} calendar or address book read", "{n} calendars and address books read"));
        }
    };
    Some(said.to_string())
}

pub fn show(ctx: &egui::Context, d: &mut DavSettings, app: &mut MailApp, t: &Tokens, enter: bool) -> Outcome {
    let watch = app.secret_watch();
    let Some(account) = app.session.mailbox.account(d.account).cloned() else { return Outcome::Close };
    let who = account.email.trim().to_string();
    let set_up = account.dav.is_some();
    let remember = app.ui.remember_sign_in;
    d.poll();
    let can_find = app.services.dav_find.is_some() && d.finding.is_none() && d.find_password(app).is_some();
    let (mut submit, mut close, mut stop, mut find, mut answer) = (false, false, false, false, None);
    let open = window(ctx, tl!("Calendars and Contacts"), 480.0, |ui| {
        ui.label(crate::i18n::fmt(tl!("Where the calendars and contacts of {account} are kept"), &[("account", &who)]));
        ui.label(tl!("Calendars"));
        crate::text_menu::field(
            ui.add(egui::TextEdit::singleline(&mut d.calendars).hint_text("https://dav.example.org/calendars/ada/").desired_width(f32::INFINITY)),
        );
        ui.label(tl!("Contacts"));
        crate::text_menu::field(
            ui.add(egui::TextEdit::singleline(&mut d.contacts).hint_text("https://dav.example.org/addressbooks/ada/").desired_width(f32::INFINITY)),
        );
        hint(ui, t, tl!("The https:// addresses your provider gives for calendars (CalDAV) and contacts (CardDAV). Either may be left empty."));
        ui.label(tl!("User name"));
        crate::text_menu::field(ui.add(egui::TextEdit::singleline(&mut d.username).hint_text(who.as_str()).desired_width(f32::INFINITY)));
        hint(ui, t, tl!("Leave it empty to sign in with the account's address."));
        ui.checkbox(&mut d.separate, tl!("These servers have a password of their own"));
        if d.separate {
            if let Some(origin) = d.password_origin() {
                let room = egui::Margin::from(ui.spacing().button_padding);
                ui.label(crate::i18n::fmt(tl!("Password for {server}"), &[("server", &crate::forms::kit::shown(host(&origin), 80))]));
                crate::secret_field::field(ui, &mut d.password, &mut d.shown, watch, room, |edit| edit);
                if d.contacts_elsewhere() {
                    hint(ui, t, tl!("The contacts are on another server: File › Info asks for its password."));
                }
            }
            let said = if remember {
                tl!("Once the server has taken it, MailCraft keeps it in your system's credential store, as File › Options lets it.")
            } else {
                tl!("MailCraft keeps it in memory until you close the app. It is never written to disk.")
            };
            hint(ui, t, said);
        } else {
            hint(ui, t, tl!("The account's mail password is used, and only for these addresses."));
        }
        if app.services.dav_find.is_some() {
            ui.horizontal(|ui| {
                let tip = if d.find_password(app).is_none() { tl!("Give the password first.") } else { tl!("Look for the servers now.") };
                find = ui.add_enabled(can_find, egui::Button::new(tl!("Find"))).on_hover_text(tip).clicked();
                if d.finding.is_some() {
                    ui.spinner();
                    ui.label(tl!("Searching…"));
                }
            });
            hint(
                ui,
                t,
                tl!(
                    "Find asks the typed addresses, or else public name servers and the usual places for your address's domain. The password goes only to a server of yours that asks for it, or one you agree to."
                ),
            );
        }
        if let Some((host, why)) = d.finding.as_ref().and_then(|f| f.question.clone()) {
            ui.label(crate::i18n::fmt(crate::dav_find::question(why), &[("host", &crate::forms::kit::shown(&host, 80))]));
            ui.horizontal(|ui| {
                if ui.button(tl!("Send It There")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tl!("Don't Send")).clicked() {
                    answer = Some(false);
                }
            });
        }
        match &d.found {
            Some(Ok(found)) => {
                let line = crate::i18n::fmt(
                    tl!("Found {calendars} and {books}. Save keeps them."),
                    &[
                        ("calendars", &tln!(found.calendars, "{n} calendar", "{n} calendars")),
                        ("books", &tln!(found.address_books, "{n} address book", "{n} address books")),
                    ],
                );
                hint(ui, t, &line);
                if found.passed_over > 0 {
                    hint(
                        ui,
                        t,
                        &tln!(
                            found.passed_over,
                            "{n} other item isn't read (a task list, a plain file)",
                            "{n} other items aren't read (task lists, plain files)"
                        ),
                    );
                }
            }
            Some(Err(failure)) => {
                ui.label(egui::RichText::new(crate::dav_find::said(*failure)).color(t.red));
            }
            None => {}
        }
        if let Some(line) = status(app, &account) {
            hint(ui, t, &line);
        }
        if !d.error.is_empty() {
            ui.label(egui::RichText::new(d.error.as_str()).color(t.red));
        }
        if set_up {
            stop = ui
                .link(tl!("Stop using these servers"))
                .on_hover_text(tl!("Forget these addresses and what was read from them. The calendars and contacts on the servers stay as they are."))
                .clicked();
        }
        hint(ui, t, tl!("Saving sends nothing. MailCraft reads the servers at its next run."));
        let ready = !(d.calendars.trim().is_empty() && d.contacts.trim().is_empty());
        let (ok, cancel) = buttons(ui, tl!("Save"), ready);
        submit = ok || (enter && ready);
        close = cancel;
    });
    if !open || close {
        return Outcome::Close;
    }
    if let Some(yes) = answer {
        d.answer(yes);
    }
    if find {
        d.find(app);
    }
    if stop {
        return match app.execute("account.davOff", json!({"id": d.account})) {
            Ok(_) => Outcome::Close,
            Err(e) => {
                d.error = e;
                Outcome::Open
            }
        };
    }
    if submit && d.save(app) {
        return Outcome::Close;
    }
    Outcome::Open
}

/// The account's line in File › Info: where its calendars and contacts are and how the last read
/// went, with a way to set them up or change them. Only while the feature is on.
pub(crate) fn info_row(app: &mut MailApp, ui: &mut Ui, t: &Tokens, a: &Account) {
    if !app.session.dav_enabled || !may_have(a) {
        return;
    }
    let status = status(app, a);
    ui.horizontal(|ui| {
        ui.add_space(36.0);
        let (line, link) = match a.dav.as_ref() {
            None => (tl!("Calendars and contacts: not set up").to_string(), tl!("Set Up…")),
            Some(server) => {
                let host = server.home_origins().first().map(|o| host(o).to_string()).unwrap_or_default();
                (crate::i18n::fmt(tl!("Calendars and contacts: {server}"), &[("server", &crate::forms::kit::shown(&host, 80))]), tl!("Change…"))
            }
        };
        ui.label(egui::RichText::new(line).font(regular(11.5)).color(t.text_dim));
        if ui.link(link).on_hover_text(tl!("Where this account's calendars and contacts are kept")).clicked() {
            let _ = app.run("ui.dialog", json!({"name": "davSettings", "id": a.id}));
        }
    });
    if let Some(status) = status {
        ui.horizontal(|ui| {
            ui.add_space(36.0);
            ui.label(egui::RichText::new(status).font(regular(11.5)).color(t.text_dim));
        });
    }
    ui.add_space(4.0);
}
