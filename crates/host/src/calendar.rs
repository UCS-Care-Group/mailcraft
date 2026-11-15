// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar jobs the host runs besides the Graph run ([`crate::graph`]): the check of the
//! calendar connection (File › Info) and the free and busy times for Find a Time. What each is
//! told and what it brings back is plain data that builds for the browser; the runs
//! (`check_calendar`, `free_busy`) are native only. Nothing of an event is logged, and what the
//! check makes on the server it deletes again.

use mailcraft_model::remote::{FreeBusy, FreeBusyAsk};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Account, Id, OAuth};

/// What the host needs to check an account's calendar connection (the front end's `Services::check_calendar`).
pub struct CalendarCheckRequest {
    pub account: Account,
    /// The account's sign-in settings, with the application id filled in.
    pub oauth: OAuth,
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

/// What the host is asked to find out for Find a Time
/// (the front end's `Services::free_busy`): when the people of `ask` are busy, as
/// far as the account may know. It reads; nothing is written and nobody is told.
pub struct FreeBusyRequest {
    pub account: Account,
    /// The account's sign-in settings, with the application id filled in.
    pub oauth: OAuth,
    /// The account's refresh token, to get a token with when `access` is empty.
    pub refresh: String,
    /// An access token from earlier in this session that is still good; else empty.
    pub access: String,
    pub ask: FreeBusyAsk,
}

/// What a [`FreeBusyRequest`] came to.
#[derive(Default)]
pub struct FreeBusyResult {
    pub account: Id,
    /// A token that was obtained for this (kept for the session, as a calendar job's is).
    pub tokens: Option<Tokens>,
    /// The owner hasn't allowed this program to read the calendar.
    pub consent_needed: bool,
    /// The token that was handed in is no longer taken.
    pub token_refused: bool,
    /// What the server said; `None` when it couldn't be asked.
    pub schedule: Option<FreeBusy>,
    /// Why there is no answer, for the owner to read.
    pub error: String,
}

#[cfg(not(target_arch = "wasm32"))]
mod run;
/// The seam the flow tests run through with an offset of their own (`test-support`).
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "test-support")))]
pub use run::check_calendar_in;
#[cfg(not(target_arch = "wasm32"))]
pub use run::{check_calendar, free_busy};
