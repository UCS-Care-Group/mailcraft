// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keeping a Linchange account's calendars and address books current over CalDAV and CardDAV.
//!
//! Linchange keeps mail on IMAP and SMTP, which the ordinary Send/Receive brings, and everything
//! else on CalDAV and CardDAV. This is the small job that reads those: every calendar the account
//! may read (its own and the ones shared with it), the events in the stretch on screen, and every
//! card of every address book it may read (its personal book, books shared with it and the
//! organization directory). Each card says which book it came from, so a shared book is not
//! mistaken for the owner's own.
//!
//! The job runs after a Send/Receive, when the account is added, and otherwise now and then. It
//! is one at a time, and it never holds up any account's mail.

use std::collections::{HashMap, HashSet};

use mailcraft_engine::cmd::remote;
use mailcraft_model::remote::{CalendarsDownload, ContactsDownload, EventsDownload, EventsUpload, EventsUploaded};
use mailcraft_model::{Account, Id};
use mailcraft_time::Timestamp;
use std::sync::mpsc::Receiver;

use crate::MailApp;

/// How long between two reads of an account's calendars and address books (milliseconds).
const EVERY_MS: f64 = 300_000.0;

/// What the host is asked to read, for one Linchange account.
pub struct DavJob {
    pub account: Account,
    /// The name the DAV service knows the owner by (the account's login name).
    pub username: String,
    /// The account's password, in memory for this session only. Never logged or saved here.
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
}

impl MailApp {
    /// Ask for `account`'s calendars and address books now (after it was added, or after a
    /// Send/Receive the owner asked for).
    pub(crate) fn dav_sync_soon(&mut self, account: Id) {
        if self.session.mailbox.account(account).is_some_and(Account::is_linchange) {
            self.dav.checked.remove(&account);
            self.dav.requested.insert(account);
        }
    }

    /// Look whether a Linchange account's calendars and address books are due, and start a job
    /// when they are. Called every frame.
    pub(crate) fn dav_tick(&mut self, ctx: &egui::Context) {
        if self.dav.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        if self.dav_tick_at(crate::now_ms()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if self.session.mailbox.accounts.iter().any(Account::is_linchange) {
            // Nothing is happening: look again in a while (sooner if a job is wanted).
            ctx.request_repaint_after(std::time::Duration::from_secs(if self.dav.requested.is_empty() { 5 } else { 1 }));
        }
    }

    /// [`dav_tick`](Self::dav_tick) at `now` (milliseconds). Whether a job was started.
    pub(crate) fn dav_tick_at(&mut self, now: f64) -> bool {
        if !self.auto_sync || self.services.dav.is_none() || self.dav.rx.is_some() || self.offline() {
            return false;
        }
        self.dav.requested.retain(|id| self.session.mailbox.account(*id).is_some_and(Account::is_linchange));
        for account in self.session.mailbox.accounts.iter().filter(|a| a.is_linchange()).map(|a| a.id).collect::<Vec<_>>() {
            let Some(found) = self.session.mailbox.account(account).cloned() else { continue };
            let due = self.dav.requested.remove(&account);
            if !due && self.dav.checked.get(&account).is_some_and(|at| now - at < EVERY_MS) {
                continue;
            }
            let Some(password) = self.passwords.get(&account).cloned().or_else(|| self.remembered("password", &found.email)) else {
                // The password is asked for by a Send/Receive; until then there is nothing to
                // read the calendars with.
                continue;
            };
            let (from, to) = self.calendar_window();
            let changes = remote::event_upload(&self.session, account).ok().filter(|upload| !upload.ops.is_empty());
            let main_calendar = self.dav.main.get(&account).cloned().unwrap_or_default();
            let job = DavJob { username: found.login().to_string(), password, from, to, account: found, changes, main_calendar };
            let Some(dav) = &self.services.dav else { return false };
            self.dav.checked.insert(account, now);
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
        if let Some(uploaded) = &result.uploaded
            && let Err(e) = remote::apply_event_outcomes(&mut self.session, uploaded)
        {
            errors.push(crate::i18n::fmt(tl!("The calendar changes couldn't be taken in: {error}"), &[("error", &e.to_string())]));
        }
        if let Some(calendars) = &result.calendars {
            let mut calendars = calendars.clone();
            calendars.account = account;
            calendars.email = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
            if let Some(main) = calendars.calendars.iter().find(|calendar| calendar.main).or_else(|| calendars.calendars.first()) {
                self.dav.main.insert(account, main.id.clone());
            }
            if let Err(e) = remote::apply_calendars(&mut self.session, &calendars) {
                errors.push(crate::i18n::fmt(tl!("The calendars couldn't be taken in: {error}"), &[("error", &e.to_string())]));
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
        if self.session.mailbox.account(account).is_some() {
            self.note_sync_problems(account, &errors, false);
        }
    }
}

#[cfg(test)]
mod tests;
