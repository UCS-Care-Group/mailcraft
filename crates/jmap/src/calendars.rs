// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A JMAP account's calendars and the events of a stretch of them (JMAP for Calendars,
//! draft-ietf-jmap-calendars; the events are JSCalendar, RFC 8984), read for the engine. Nothing here writes
//! to the server.
//!
//! [`sync_calendar`] takes the request the engine worked out ([`JmapCalendarRequest`]) and hands back the
//! calendars and the events it could read ([`JmapCalendarResult`]). Nothing is kept between runs.
//!
//! **Whose calendars.** The account the session names for calendars (`primaryAccounts`), when it offers them;
//! else the account for mail, when that one does. A session that does not advertise
//! `urn:ietf:params:jmap:calendars`, or no such account: [`JmapCalendarResult::not_offered`], which is no
//! failure, and nothing is asked.
//!
//! **What is asked of the server**, each in a request of one call:
//!
//! 1. `Calendar/get` of all the calendars. Those the account does not subscribe to, and those whose events it
//!    may not read, are passed over; [`MAX_REMOTE_CALENDARS`] are kept at the most. Exactly one of them is
//!    the main one: the one the server says is the default, else the first by its place in the server's order
//!    (`sortOrder`) and then by its id. Each is one the account only reads ([`Rights::Read`]), whatever the
//!    server allows: no change goes up from here yet.
//! 2. For each calendar, in the list's order, `CalendarEvent/query` for the ids of the events of the stretch
//!    (`inCalendar`, `after`, `before`), what repeats expanded by the server (`expandRecurrences`): a page of
//!    500 at a time by `position`, until the server lists no more (or as many as it said there are), 400
//!    pages and [`MAX_EVENTS`] ids at the most.
//! 3. `CalendarEvent/get` of those ids, 200 to a request (or as many as the server takes in a `get`), for the
//!    properties that are read and no others ([`PROPERTIES`]; the instants `utcStart` and `utcEnd` come only
//!    when asked for). When the server answers an error to such a call, or its answer is too large, the call
//!    is halved until the event that breaks it is found; that one is not read, the rest are.
//!
//! The paging and the halving are what every reader of one kind of data does (`reader`), the contacts' too:
//! here is what is asked for a calendar's events ([`Run::events`]), and what an event becomes (`event`).
//!
//! **What an event becomes.** Each object is one plain event under the id the server gave it: an occurrence
//! of a series comes as an event of its own, and the series as such does not come at all (`event`).
//!
//! **Complete.** [`CalendarsDownload::complete`] says that every calendar the account reads is in the list.
//! [`EventsDownload::complete`] says that every event the server has in the stretch of that calendar is in
//! the answer, and the engine then removes what the answer lacks: so it is true only when every page of the
//! list was read, every id listed was answered for (with an event, or as gone since), none was left out for
//! the cap, and the run was not stopped. Anything less adds and updates and removes nothing.
//!
//! **When something goes wrong.** `cancel` is looked at before every request: set, the run ends with
//! `stopped`. The credentials or the access refused (401, 403): `auth_refused`. A busy server (429, 503):
//! `busy_for`, as many seconds as it asked, or 30. Anything else is a line in `errors`. A calendar whose
//! events can not be listed has no download, and the others are still read; after [`MAX_DOWN`] calendars one
//! after the other for which the server could not be had, the rest are left for another run. The lines name
//! no calendar and no event, and what a server says in one is scrubbed of the session's address, the
//! account's, the credentials and any word that holds an `@`.
//!
//! **What comes back is hostile.** An answer is held to the size its request can need; ids are held to the
//! JMAP alphabet before they go into a request; only events that were asked for are read, each once; every
//! map, list and string of a calendar and of an event is bounded (`event`); nothing is indexed, unwrapped or
//! recursed into.

mod event;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;

use mailcraft_model::remote::{CalendarsDownload, EventsDownload, MAX_EVENTS, MAX_REMOTE_CALENDARS, RemoteCalendar, RemoteEvent};
use mailcraft_model::sync::{JmapCalendarRequest, JmapCalendarResult};
use mailcraft_model::{Rgb, Rights};
use mailcraft_time::{Timestamp, format_iso_utc};
use serde_json::{Map, Value, json};

use self::event::{Mapped, event};
use crate::reader::{Calls, Kind, Made, Outcome, Stop, account_for, cut, sign_in_failed};
use crate::{Connection, Error, HttpExchange, Id};

/// The capability of JMAP for Calendars (draft-ietf-jmap-calendars §1.4).
pub const CAPABILITY_CALENDARS: &str = "urn:ietf:params:jmap:calendars";

/// Calendars, one after the other, for which the server could not be had before the run gives up on the rest.
const MAX_DOWN: usize = 3;
/// Most calendars of an answer that are looked at.
const MAX_LISTED: usize = 1_000;
/// Longest name of a calendar kept, in bytes: the model cuts again, to less.
const MAX_NAME: usize = 1_024;
/// The most an answer may be, in bytes: the calendars, and 200 events.
const CALENDARS_BYTES: usize = 4 << 20;
const GET_BYTES: usize = 8 << 20;
const _: () = assert!(GET_BYTES <= crate::http::API_RESPONSE_LIMIT);
/// The longest stretch that is read, in seconds (five years), and the last instant one may reach to (the end
/// of the year 9999, the last a time of the protocol can say).
const MAX_STRETCH: i64 = 5 * 366 * 86_400;
const LAST: Timestamp = 253_402_300_799;
/// What is asked of an event: what is read of it (`event`), and no more.
const PROPERTIES: [&str; 20] = [
    "id",
    "@type",
    "uid",
    "title",
    "description",
    "descriptionContentType",
    "locations",
    "start",
    "timeZone",
    "duration",
    "showWithoutTime",
    "utcStart",
    "utcEnd",
    "updated",
    "keywords",
    "privacy",
    "status",
    "freeBusyStatus",
    "alerts",
    "participants",
];

/// Read the calendars of a JMAP account and the events of the stretch `request` names (see the module
/// documentation). `connection` is signed in already ([`Connection::connect`]); a caller whose sign-in failed
/// ends with [`failed_calendar`]. `cancel` is read before every request: set, the run ends there, `stopped`.
///
/// Never an error as a whole: what went wrong is in the result, and what was read until then with it.
/// Nothing here waits: a busy server ends the run with the seconds it asked for.
pub async fn sync_calendar<H: HttpExchange>(connection: &Connection<H>, request: &JmapCalendarRequest, cancel: &AtomicBool) -> JmapCalendarResult {
    let mut result = answer_for(request);
    let Some(account) = account_for(connection, CAPABILITY_CALENDARS) else {
        log::debug!("jmap calendars: the server offers none for the account");
        result.not_offered = true;
        return result;
    };
    let length = request.to.saturating_sub(request.from);
    if request.from < 0 || request.to > LAST || length <= 0 || length > MAX_STRETCH {
        result.errors.push("Couldn't read the calendars from the server: the time asked for is not one that can be read.".to_string());
        return result;
    }
    let run = Run {
        calls: Calls::new(connection, cancel, &request.email, &request.session, account, CAPABILITY_CALENDARS),
        after: format_iso_utc(request.from),
        before: format_iso_utc(request.to),
        owner: request.email.trim(),
    };
    let listed = match run.calendars().await {
        Ok(listed) => listed,
        Err(stop) => {
            end(&mut result, stop);
            return result;
        }
    };
    let mut told = Told::default();
    let mut stop: Option<Stop> = None;
    let mut down = 0usize;
    for (at, calendar) in listed.calendars.iter().enumerate() {
        let mut out: Outcome<RemoteEvent> = Outcome::default();
        let ended = run.events(&calendar.id, &mut out).await.err();
        let whole = ended.is_none() && out.clean();
        told.capped |= out.capped;
        let why = out.why.take();
        if out.listed {
            result.events.push(EventsDownload {
                account: request.account,
                email: request.email.trim().to_string(),
                calendar: if calendar.main { String::new() } else { calendar.id.clone() },
                from: request.from,
                to: request.to,
                complete: whole,
                events: out.items,
                gone: Vec::new(),
                has_categories: true,
                has_options: false,
                free_busy: false,
            });
        }
        match ended {
            None => {
                down = 0;
                if !whole {
                    told.partly(why);
                }
            }
            Some(Stop::Failed(line)) => {
                down = 0;
                told.trouble(out.listed, line);
            }
            Some(Stop::Down(line)) => {
                down = down.saturating_add(1);
                told.trouble(out.listed, line);
                if down >= MAX_DOWN {
                    // The rest would fare no better now.
                    told.failed = told.failed.saturating_add(listed.calendars.len().saturating_sub(at).saturating_sub(1));
                    break;
                }
            }
            Some(other) => {
                stop = Some(other);
                break;
            }
        }
    }
    log::debug!(
        "jmap calendars: {} listed (whole: {}), {} read, {} not at all, {} in part",
        listed.calendars.len(),
        listed.complete,
        result.events.len(),
        told.failed,
        told.partial
    );
    let ended = stop.is_some();
    if let Some(stop) = stop {
        end(&mut result, stop);
    }
    told.lines(&mut result.errors);
    if !listed.complete && result.errors.is_empty() && !ended {
        result.errors.push("Couldn't read all the calendars from the server.".to_string());
    }
    result.calendars = Some(CalendarsDownload {
        account: request.account,
        email: request.email.trim().to_string(),
        complete: listed.complete,
        calendars: listed.calendars,
        refused: Vec::new(),
    });
    result
}

/// The end of a run that could not begin: signing in to the server ([`Connection::connect`]) failed with
/// `error`. What [`sync_calendar`] would have said of the same trouble: the sign-in refused, the server
/// busy, or a line.
pub fn failed_calendar(request: &JmapCalendarRequest, error: &Error) -> JmapCalendarResult {
    let failed = sign_in_failed(&request.email, &request.session, error);
    JmapCalendarResult { auth_refused: failed.refused, busy_for: failed.busy_for, errors: Vec::from_iter(failed.line), ..answer_for(request) }
}

/// An answer to `request`, with nothing in it yet.
fn answer_for(request: &JmapCalendarRequest) -> JmapCalendarResult {
    JmapCalendarResult { account: request.account, email: request.email.clone(), session: request.session.clone(), ..Default::default() }
}

/// What ended the run, into its answer.
fn end(result: &mut JmapCalendarResult, stop: Stop) {
    let line = match stop {
        Stop::Cancelled => {
            result.stopped = true;
            return;
        }
        Stop::Auth => {
            result.auth_refused = true;
            return;
        }
        Stop::Busy(seconds, line) => {
            result.busy_for = Some(seconds);
            line
        }
        Stop::Down(line) | Stop::Failed(line) => line,
    };
    result.errors.push(format!("Couldn't read the calendars from the server: {line}"));
}

/// So many calendars, in words.
fn calendars(count: usize) -> String {
    if count == 1 { "1 calendar".to_string() } else { format!("{count} calendars") }
}

/// What there is to tell of the calendars whose events were not all read.
#[derive(Default)]
struct Told {
    /// Calendars none of whose events could be read, and the first reason.
    failed: usize,
    failed_why: Option<String>,
    /// Calendars whose events were read in part, and the first reason that was given.
    partial: usize,
    partial_why: Option<String>,
    /// A calendar has more events in the stretch than a run reads.
    capped: bool,
}

impl Told {
    /// A calendar's events were not all read: `why`, when something said so.
    fn partly(&mut self, why: Option<String>) {
        self.partial = self.partial.saturating_add(1);
        if self.partial_why.is_none() {
            self.partial_why = why;
        }
    }

    /// Reading a calendar's events ended with `line`: after some were listed (`listed`), or before.
    fn trouble(&mut self, listed: bool, line: String) {
        if listed {
            self.partly(Some(line));
        } else {
            self.failed = self.failed.saturating_add(1);
            self.failed_why.get_or_insert(line);
        }
    }

    fn lines(self, errors: &mut Vec<String>) {
        if self.failed > 0 {
            let why = self.failed_why.unwrap_or_default();
            errors.push(format!("Couldn't read the events of {}: {why}", calendars(self.failed)));
        }
        if self.partial > 0 {
            match self.partial_why {
                Some(why) => errors.push(format!("Couldn't read all the events of {}: {why}", calendars(self.partial))),
                None => errors.push(format!("Couldn't read all the events of {} from the server.", calendars(self.partial))),
            }
        }
        if self.capped {
            errors.push(format!("A calendar has more events in that time than are read: {MAX_EVENTS} at the most."));
        }
    }
}

/// The calendars that are read, in the order the server listed them.
struct Listed {
    calendars: Vec<RemoteCalendar>,
    /// Every calendar the account reads on the server is among them.
    complete: bool,
}

/// A colour as a calendar's is written (a CSS colour): `#rgb` or `#rrggbb`. A colour by name, or written any
/// other way, is none here.
fn color(text: &str) -> Option<Rgb> {
    let hex = text.trim().strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        6 => Rgb::parse(hex),
        3 => Rgb::parse(&hex.chars().flat_map(|digit| [digit, digit]).collect::<String>()),
        _ => None,
    }
}

/// One run: its calls, and what is worked out once.
struct Run<'a, H> {
    calls: Calls<'a, H>,
    /// The stretch, as the server is told it: events after this instant and before that one.
    after: String,
    before: String,
    /// The address of the account's owner.
    owner: &'a str,
}

impl<H: HttpExchange> Run<'_, H> {
    /// `Calendar/get` of all the calendars: those the account subscribes to and may read the events of, each
    /// once, one of them the main one.
    async fn calendars(&self) -> Result<Listed, Stop> {
        let args = json!({"accountId": self.calls.account.as_str(), "ids": null});
        let answer = self.calls.one_call("Calendar/get", args, CALENDARS_BYTES).await?;
        let list = answer.get("list").and_then(Value::as_array).ok_or_else(|| self.calls.shape("the answer has no list of calendars"))?;
        let mut complete = list.len() <= MAX_LISTED;
        let mut seen: HashSet<&str> = HashSet::new();
        // Each with its place in the server's order and whether the server says it is the default.
        let mut kept: Vec<(bool, u64, RemoteCalendar)> = Vec::new();
        for entry in list.iter().take(MAX_LISTED) {
            let Some(id) = entry.get("id").and_then(Value::as_str).filter(|id| Id::new(id).is_ok()) else {
                // What has no id can not be asked for its events.
                complete = false;
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            let unread = entry.get("myRights").and_then(|rights| rights.get("mayReadItems")).and_then(Value::as_bool) == Some(false);
            if unread || entry.get("isSubscribed").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            if kept.len() >= MAX_REMOTE_CALENDARS {
                complete = false;
                break;
            }
            let calendar = RemoteCalendar {
                id: id.to_string(),
                name: entry.get("name").and_then(Value::as_str).map(|name| cut(name, MAX_NAME).to_string()).unwrap_or_default(),
                color: entry.get("color").and_then(Value::as_str).filter(|color| color.len() <= 16).and_then(color),
                // Whatever the server allows: no change goes up from here.
                rights: Rights::Read,
                ..RemoteCalendar::default()
            };
            let default = entry.get("isDefault").and_then(Value::as_bool) == Some(true);
            kept.push((default, entry.get("sortOrder").and_then(Value::as_u64).unwrap_or(0), calendar));
        }
        // The main one: the default (the first of them, should the server name several), else the first.
        let main =
            kept.iter().min_by(|a, b| (!a.0, a.1, a.2.id.as_str()).cmp(&(!b.0, b.1, b.2.id.as_str()))).map(|(_, _, calendar)| calendar.id.clone());
        let calendars = kept
            .into_iter()
            .map(|(_, _, mut calendar)| {
                calendar.main = main.as_deref() == Some(calendar.id.as_str());
                calendar
            })
            .collect();
        Ok(Listed { calendars, complete })
    }

    /// The events of the stretch of the calendar `calendar`: `CalendarEvent/query` for their ids, what repeats
    /// expanded, [`MAX_EVENTS`] at the most, then `CalendarEvent/get` for the properties that are read. A
    /// failure before the first page of the list is the calendar's; an event whose time can not be told counts
    /// as not read.
    async fn events(&self, calendar: &str, out: &mut Outcome<RemoteEvent>) -> Result<(), Stop> {
        let filter = json!({"inCalendar": calendar, "after": self.after, "before": self.before});
        let kind = Kind {
            query: "CalendarEvent/query",
            get: "CalendarEvent/get",
            listing: Map::from_iter([("filter".to_string(), filter), ("expandRecurrences".to_string(), json!(true))]),
            reading: Map::from_iter([("properties".to_string(), json!(PROPERTIES))]),
            most: MAX_EVENTS,
            get_bytes: GET_BYTES,
            no_list: "the answer has no list of events",
            repeats: "the list of events repeats itself",
            too_long: "the list of events has too many pages",
        };
        let made = |id: &str, entry: &Value| match event(id, entry, self.owner) {
            Mapped::Event(event) => Made::Item(*event),
            // (What is no event is answered for and is none.)
            Mapped::Other => Made::Other,
            Mapped::Unreadable => Made::Unreadable,
        };
        self.calls.read_all(&kind, out, made).await
    }
}
