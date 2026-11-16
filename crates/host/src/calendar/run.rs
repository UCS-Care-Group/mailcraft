// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar jobs' runs (native only).

use super::{CalendarCheckRequest, CalendarCheckResult, CalendarCheckStep, FreeBusyRequest, FreeBusyResult};
use crate::graph::local_zone;

/// File › Info › Check Calendar Connection: on the account's real server, and only because the
/// owner asked for it and said yes to what it does. One appointment is made a year from today
/// in the main calendar, read back, changed (once as an instant, once said in this computer's
/// time zone), read back each time and deleted; what the server answered at each step goes back
/// to the window, which shows and logs it. Nothing else of the calendar is read or changed.
pub fn check_calendar(request: CalendarCheckRequest) -> CalendarCheckResult {
    check_calendar_in(request, &local_zone(), &mailcraft_time::local_offset_minutes)
}

/// [`check_calendar`], with the owner's time zone as `zone` and how far east of UTC it is at an
/// instant as `offset_at` (minutes).
pub fn check_calendar_in(request: CalendarCheckRequest, zone: &str, offset_at: &dyn Fn(i64) -> i32) -> CalendarCheckResult {
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

/// When the people a meeting is for are free and busy, for its Find a Time page: one
/// question to Microsoft Graph, about when their time is taken and how, never by what. It
/// reads; nothing is written, and nobody is told that it was asked.
pub fn free_busy(request: FreeBusyRequest) -> FreeBusyResult {
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
