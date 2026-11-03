// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars of other people that the owner opened by address (Open Shared Calendar, #70).
//!
//! Graph has no call that adds somebody's calendar to the owner's list, so MailCraft reads it at
//! `/users/{address}/calendar` with the owner's token (`Calendars.ReadWrite.Shared`) and keeps
//! the address in its own list. Such a calendar is known here by an id that names the address
//! ([`mailcraft_model::remote::opened_id`]); every request for it goes to
//! `/users/{address}/calendar/…`, and none reaches `/me`: the paths below are the only ones that
//! differ from an ordinary calendar's, and an id that says "opened by address" with an address
//! that isn't one is refused before anything is sent.
//!
//! What the server lets the owner do with it is read from the calendar itself: `canEdit` says
//! read or edit. When the calendar can't be read at all (the person shares only when they are
//! free and busy, which is every colleague's default) the schedule is asked about instead, and
//! the calendar is shown as blocks marked Busy, Tentative, Out of Office or Working Elsewhere,
//! never with a subject, and never changed here.
//!
//! The blocks of such a calendar are drawn by the same code as a shared mailbox's (the `Graph`
//! is simply at the person's address): nothing of the calendar itself goes through `/me`.

use mailcraft_model::remote::{FreeBusyAsk, OPENED_PREFIX, OpenedWhy, RemoteCalendar, opened_address};
use mailcraft_model::{CalendarOwner, Rights};
use serde_json::Value;

use super::{ASK_TIMEOUT, CALENDAR_FIELDS, Graph, MAX_PAGE, calendar_of, schedule_at, shared_mailbox_path};
use crate::http;

/// The address a calendar's id names, when it is one that was opened by address.
pub(super) fn address_of(calendar: &str) -> Option<String> {
    opened_address(calendar)
}

/// An id that says it was opened by address but doesn't name an address that could be one: not
/// to be asked of the server at all (it would otherwise be taken for an ordinary calendar's).
pub(super) fn unsafe_id(calendar: &str) -> bool {
    calendar.starts_with(OPENED_PREFIX) && opened_address(calendar).is_none()
}

/// How opening a calendar by address went.
#[derive(Clone, Debug, PartialEq)]
pub enum Opened {
    /// It can be read: its name, rights and owner, under the id it has here.
    Read(Box<RemoteCalendar>),
    /// The server wouldn't give it, and why.
    Refused(OpenedWhy),
    /// The sign-in itself was refused (it ran out or was taken back).
    SignInRefused,
}

/// Ask the server for the calendar of the person at `address` (`GET /users/{address}/calendar`),
/// as the owner of `token`, and say what the owner may do with it. Nothing is changed.
pub fn open_calendar(token: &str, address: &str) -> Opened {
    open_calendar_at(&super::graph_base(), token, address)
}

/// [`open_calendar`] from Graph at `base`.
pub(super) fn open_calendar_at(base: &str, token: &str, address: &str) -> Opened {
    let Some(address) = mailcraft_model::mailbox_address(address) else { return Opened::Refused(OpenedWhy::NoSuchPerson) };
    let id = mailcraft_model::remote::opened_id(&address);
    let Ok(root) = shared_mailbox_path(&address).map(|path| format!("/v1.0{path}/calendar")) else { return Opened::Refused(OpenedWhy::NoSuchPerson) };
    let Ok(graph) = Graph::at(base, token) else { return Opened::SignInRefused };
    let url = graph.url(&format!("{root}?{}", http::form(&[("$select", CALENDAR_FIELDS)])));
    let reply = match graph.ask(&url, "GET", &[], None, ASK_TIMEOUT, MAX_PAGE) {
        Ok(reply) => reply,
        Err(e) if e.is_refused_sign_in() => return Opened::SignInRefused,
        Err(_) => return Opened::Refused(OpenedWhy::Unreachable),
    };
    match reply.status {
        200 => {
            let entry = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            match listed_as(&entry, &address, &id) {
                Some(calendar) => Opened::Read(Box::new(calendar)),
                None => Opened::Refused(OpenedWhy::Unreachable),
            }
        }
        401 => Opened::SignInRefused,
        // The calendar can't be read: perhaps its time is shared as free and busy, and that can.
        403 => match free_busy_only(base, token, &address) {
            Some(calendar) => Opened::Read(Box::new(calendar)),
            None => Opened::Refused(OpenedWhy::NoRights),
        },
        404 => Opened::Refused(OpenedWhy::NoSuchPerson),
        _ => Opened::Refused(OpenedWhy::Unreachable),
    }
}

/// The calendar an answer of the server describes, as it is kept here: under `id`, named by its
/// owner, owned by the person at `address` whatever the answer says of that, and with no more
/// rights than the answer gives (an answer that doesn't say is read).
fn listed_as(entry: &Value, address: &str, id: &str) -> Option<RemoteCalendar> {
    if !entry.is_object() {
        return None;
    }
    // (`calendar_of` wants an id of the server's: the id here is put in its place.)
    let mut seen = entry.clone();
    seen["id"] = Value::String("opened".into());
    let mut calendar = calendar_of(&seen, "")?;
    let owner = calendar.owner.take().unwrap_or_default();
    let name = if owner.name.trim().is_empty() { address.to_string() } else { owner.name.clone() };
    calendar.id = id.to_string();
    calendar.name = name.clone();
    calendar.main = false;
    calendar.group = String::new();
    calendar.online_providers = None;
    calendar.rights = match entry.get("canEdit").and_then(Value::as_bool) {
        Some(true) => Rights::Edit,
        _ => Rights::Read,
    };
    // Whose it is: the person the owner asked for (an answer that names somebody else, a
    // delegate, say, is not believed with the address).
    calendar.owner = CalendarOwner { address: address.to_string(), name }.cleaned();
    Some(calendar)
}

/// The calendar of a person who shares only when they are free and busy, when that is so: the
/// schedule can be asked about them.
fn free_busy_only(base: &str, token: &str, address: &str) -> Option<RemoteCalendar> {
    let now = mailcraft_time::now();
    let ask = FreeBusyAsk { people: vec![address.to_string()], from: now, to: now.saturating_add(mailcraft_time::DAY) };
    let schedule = schedule_at(base, token, &ask).ok()?;
    let person = schedule.people.iter().find(|person| person.email.eq_ignore_ascii_case(address))?;
    if !person.error.is_empty() {
        return None;
    }
    Some(RemoteCalendar {
        id: mailcraft_model::remote::opened_id(address),
        name: address.to_string(),
        rights: Rights::FreeBusy,
        owner: CalendarOwner { address: address.to_string(), name: address.to_string() }.cleaned(),
        ..Default::default()
    })
}
