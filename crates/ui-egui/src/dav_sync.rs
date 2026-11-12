// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keeping an account's calendars and address books current over CalDAV and CardDAV.
//!
//! The account may keep mail on IMAP and SMTP, which the ordinary Send/Receive brings, and may
//! have its calendars and address books alone on CalDAV and CardDAV. This is the small job that
//! reads those: every calendar
//! the account may read (its own and the ones shared with it), the events in the stretch on
//! screen, and every card of every address book it may read (its personal book, books shared
//! with it and the organization directory). Each card says which book it came from, so a shared
//! book is not mistaken for the owner's own.
//!
//! The job runs after a Send/Receive, when the account is added, when the days on screen move
//! into a stretch that was not read, and otherwise now and then. It is one at a time, and it
//! never holds up any account's mail.

use std::collections::{HashMap, HashSet};

use mailcraft_engine::cmd::remote;
use mailcraft_model::remote::{CalendarsDownload, ContactsDownload, EventsDownload, EventsUpload, EventsUploaded};
use mailcraft_model::{Account, DavServer, Id};
use mailcraft_time::Timestamp;
use std::sync::mpsc::Receiver;

use crate::MailApp;
use serde_json::json;

/// How long between two reads of an account's calendars and address books (milliseconds).
const EVERY_MS: f64 = 300_000.0;

/// Whether both DAV services are on the account's own mail server. Only then is the account's
/// mail password used for them too; calendars at another provider have a sign-in of their own.
fn shares_mail_host(account: &Account) -> bool {
    let Some(mail) = account.incoming.as_ref() else { return false };
    let mail = mail.host.trim().trim_end_matches('.').to_ascii_lowercase();
    let Some(dav) = account.dav.as_ref() else { return false };
    let on_mail = |address: &str| DavServer::host(address).is_some_and(|host| host.trim_end_matches('.').eq_ignore_ascii_case(&mail));
    !mail.is_empty() && on_mail(&dav.caldav) && on_mail(&dav.carddav)
}

/// What the host is asked to read, for one DAV account.
pub struct DavJob {
    pub account: Account,
    /// The name the DAV service knows the owner by: the one given for the calendars and address
    /// books, else the account's login name.
    pub username: String,
    /// The password for the DAV service (its own, or the account's when they share a sign-in),
    /// in memory for this session only. Never logged or saved here.
    pub password: String,
    /// The stretch of the calendar to bring (Unix seconds, UTC).
    pub from: Timestamp,
    pub to: Timestamp,
    /// What was made, changed or deleted here, to tell the server before reading it again.
    pub changes: Option<EventsUpload>,
    /// The calendar an event that names none belongs in (the account's main one), once it has
    /// been listed.
    pub main_calendar: String,
}

/// What the host brought back. Never an error as a whole: one part that couldn't be read leaves
/// the rest as it was.
#[derive(Default)]
pub struct DavResult {
    pub account: Id,
    pub calendars: Option<CalendarsDownload>,
    pub events: Vec<EventsDownload>,
    pub contacts: Option<ContactsDownload>,
    /// What the server made of the changes that were sent.
    pub uploaded: Option<EventsUploaded>,
    /// What one part couldn't do, in words for the owner.
    pub errors: Vec<String>,
}

/// The job in flight, when each account was last read, and whether one is wanted now.
#[derive(Default)]
pub(crate) struct DavSync {
    /// The account, the mailbox opening the job started under, and the answer to come.
    rx: Option<(Id, u64, Receiver<DavResult>)>,
    checked: HashMap<Id, f64>,
    /// The server's address of each account's main calendar, once it has been listed: an event
    /// made here that says no calendar of its own goes there.
    main: HashMap<Id, String>,
    /// The accounts whose next read was asked for (a Send/Receive, or a change made here).
    requested: HashSet<Id>,
    /// The accounts already told that their calendars and address books wait for a sign-in.
    told_no_password: HashSet<Id>,
    /// The stretch of the calendar each account was last asked for, as [`MailApp::calendar_window`]
    /// gave it: the days on screen, and a week either side. Days that come into view are asked
    /// for at once rather than at the next tick, as the calendar view does for Microsoft's.
    shown: HashMap<Id, (Timestamp, Timestamp)>,
}

impl MailApp {
    /// Ask for `account`'s calendars and address books now (after it was added, or after a
    /// Send/Receive the owner asked for).
    pub(crate) fn dav_sync_soon(&mut self, account: Id) {
        if self.session.mailbox.account(account).is_some_and(Account::is_dav) {
            self.dav.checked.remove(&account);
            self.dav.requested.insert(account);
        }
    }

    /// Forget a DAV sign-in the server has just refused: the password in memory and the store's,
    /// and the cached answer, so the next run asks for it again.
    fn forget_dav_password(&mut self, account: Id) {
        self.dav_passwords.remove(&account);
        self.dav.checked.remove(&account);
        self.dav.told_no_password.remove(&account);
        if let Some(a) = self.session.mailbox.account(account) {
            let key = Self::secret_key("davpassword", a);
            self.secret_cache.remove(&key);
            if let Some(store) = &self.services.secrets {
                store.delete(&key);
            }
        }
    }

    /// Look whether an account's calendars and address books are due, and start a job when they
    /// are. Called every frame.
    pub(crate) fn dav_tick(&mut self, ctx: &egui::Context) {
        if self.dav.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        if self.dav_tick_at(crate::now_ms()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if self.session.mailbox.accounts.iter().any(Account::is_dav) {
            // Nothing is happening: look again in a while (sooner if a job is wanted).
            ctx.request_repaint_after(std::time::Duration::from_secs(if self.dav.requested.is_empty() { 5 } else { 1 }));
        }
    }

    /// [`dav_tick`](Self::dav_tick) at `now` (milliseconds). Whether a job was started.
    pub(crate) fn dav_tick_at(&mut self, now: f64) -> bool {
        // A mailbox from a file names its own servers: nothing is fetched until the owner has
        // seen where each account connects.
        if !self.auto_sync || self.unconfirmed || self.services.dav.is_none() || self.dav.rx.is_some() || self.offline() {
            return false;
        }
        self.dav.requested.retain(|id| self.session.mailbox.account(*id).is_some_and(Account::is_dav));
        let window = self.calendar_window();
        for account in self.session.mailbox.accounts.iter().filter(|a| a.is_dav()).map(|a| a.id).collect::<Vec<_>>() {
            let Some(found) = self.session.mailbox.account(account).cloned() else { continue };
            let due = self.dav.requested.remove(&account);
            // Other days came into view: what was read is not what is on screen any more.
            let elsewhere = self.dav.shown.get(&account).is_some_and(|shown| *shown != window);
            if !due && !elsewhere && self.dav.checked.get(&account).is_some_and(|at| now - at < EVERY_MS) {
                continue;
            }
            let username = found.dav_login().to_string();
            let mail_password = || self.passwords.get(&account).cloned().or_else(|| self.remembered("password", account));
            let password = self
                .dav_passwords
                .get(&account)
                .cloned()
                .or_else(|| self.remembered("davpassword", account))
                // Calendars and mail at one provider usually share a sign-in: the account's
                // password is the calendars' too when they know the owner by the same name and
                // are on the mail server itself. Another host is given nothing of the mail's
                // sign-in (a mailbox from a file may name any host it likes).
                .or_else(|| if username == found.login() && shares_mail_host(&found) { mail_password() } else { None });
            let Some(password) = password else {
                // The password is asked for by a Send/Receive (or, for an account with no mail,
                // by its settings); until then there is nothing to read the calendars with. Say
                // so once, so a reopened mailbox doesn't look as if DAV had stopped working.
                if self.dav.told_no_password.insert(account) {
                    self.keep_sync_problems(
                        account,
                        &[tl!("The calendars and address books need their sign-in again: press Send/Receive.").to_string()],
                    );
                }
                continue;
            };
            self.dav.told_no_password.remove(&account);
            let (from, to) = window;
            let main_calendar = self.dav.main.get(&account).cloned().unwrap_or_default();
            let changes = remote::event_upload(&self.session, account)
                .ok()
                .map(|mut upload| {
                    // An event with no calendar of its own goes into the main one, whose URL this
                    // run has not been told yet: the read below finds it, and the next run sends it.
                    if main_calendar.trim().is_empty() {
                        upload.ops.retain(|op| !op.calendar.trim().is_empty());
                    }
                    upload
                })
                .filter(|upload| !upload.ops.is_empty());
            let job = DavJob { username, password, from, to, account: found, changes, main_calendar };
            let Some(dav) = &self.services.dav else { return false };
            self.dav.checked.insert(account, now);
            self.dav.shown.insert(account, window);
            let opening = self.session.openings();
            self.dav.rx = Some((account, opening, dav(job)));
            return true;
        }
        // The wish to ask at once doesn't outlive the look when there was nobody to ask.
        false
    }

    /// Take in what the host brought: the calendars, their events and the contacts of every
    /// address book (including the ones shared with the account).
    pub(crate) fn poll_dav(&mut self) {
        let Some((account, opening, rx)) = &self.dav.rx else { return };
        let (account, opening) = (*account, *opening);
        let mut result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::warn!("calendar and address books, account {account}: the job ended without an answer");
                DavResult { account, ..Default::default() }
            }
        };
        self.dav.rx = None;
        // A mailbox that took the place of the one the job was for: its accounts are other
        // accounts, whatever their ids and addresses, and what the job brings is nobody's.
        if opening != self.session.openings() {
            log::info!("calendar and address books, account {account}: the answer came for another mailbox; it is left out");
            return;
        }
        let mut errors = std::mem::take(&mut result.errors);
        if let Some(uploaded) = &result.uploaded {
            match remote::apply_event_outcomes(&mut self.session, uploaded) {
                Ok(applied) => errors.extend(applied.messages),
                Err(e) => errors.push(crate::i18n::fmt(tl!("The calendar changes couldn't be taken in: {error}"), &[("error", &e.to_string())])),
            }
        }
        if let Some(calendars) = &result.calendars {
            let mut calendars = calendars.clone();
            calendars.account = account;
            calendars.email = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
            match calendars.calendars.iter().find(|calendar| calendar.main && calendar.rights == mailcraft_model::Rights::Edit) {
                Some(main) => {
                    self.dav.main.insert(account, main.id.clone());
                }
                None => {
                    self.dav.main.remove(&account);
                }
            }
            if let Err(e) = remote::apply_calendars(&mut self.session, &calendars) {
                errors.push(crate::i18n::fmt(tl!("The calendars couldn't be taken in: {error}"), &[("error", &e.to_string())]));
            }
            // An event made before the main calendar's URL was known was held back rather than
            // sent into nothing; now that it is known, the next run sends it.
            if self.dav.main.contains_key(&account) && remote::event_upload(&self.session, account).is_ok_and(|upload| !upload.ops.is_empty()) {
                self.dav.requested.insert(account);
            }
        }
        for events in &result.events {
            let mut events = events.clone();
            events.account = account;
            events.email = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
            if let Err(e) = remote::apply_events(&mut self.session, &events) {
                errors.push(crate::i18n::fmt(tl!("The calendar events couldn't be taken in: {error}"), &[("error", &e.to_string())]));
            }
        }
        if let Some(contacts) = &result.contacts {
            let mut contacts = contacts.clone();
            contacts.account = account;
            contacts.email = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
            if let Err(e) = remote::apply_contacts(&mut self.session, &contacts) {
                errors.push(crate::i18n::fmt(tl!("The contacts couldn't be taken in: {error}"), &[("error", &e.to_string())]));
            }
        }
        if errors.iter().any(|line| line.contains("answered 401") || line.contains("didn't take that user name")) {
            self.forget_dav_password(account);
            errors.push(tl!("The server refused the sign-in for the calendars and address books: type it again.").to_string());
            if self.dialog.is_none() {
                self.dialog = crate::dialogs::Dialog::open("password", self, &json!({"account": account}));
            }
        }
        if self.session.mailbox.account(account).is_some() {
            // A whole answer that says nothing is wrong clears what an earlier run or a missing
            // sign-in said; a stale line would keep saying DAV is broken while it works.
            self.note_sync_problems(account, &errors, true);
        }
    }
}

#[cfg(test)]
mod tests;
