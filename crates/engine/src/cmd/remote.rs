// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What an account's server keeps besides its mail: the calendar and the contacts.
//!
//! As with mail, the engine never talks to a server. A host with sockets (`mailcraft-net`, which
//! asks Microsoft Graph) brings an [`EventsDownload`] or a [`ContactsDownload`], and
//! `sync.applyEvents` and `sync.applyContacts` ([`apply_events`], [`apply_contacts`]) merge it
//! into the mailbox: what is new is added, what changed is updated, and what the server no longer
//! has is removed. The server is the source of truth for what came from it and for nothing else:
//! an event or a contact that was made here is never touched.
//!
//! What came from a server carries its [`Origin`] and is a copy ([`keeper`] says whose). Its
//! categories are the server's too, when a download says what they are.
//!
//! A series is one event here, which repeats by its rule ([`series_from_remote`]), when the rule
//! is one the mailbox can say; otherwise it comes as its occurrences, an event each. What the
//! server has on each day of a stretch is what shows: a day of the series that the server
//! doesn't have (it was deleted there) is skipped, and a day that was changed on its own, or
//! doesn't fall where the rule puts it here, is an event of its own that says which day it
//! stands for ([`Origin::occurrence`]). One occurrence that is changed or deleted here is told to
//! the server by its series and its time ([`note_occurrence_changed`], [`note_occurrence_gone`]).
//!
//! Mail goes to nobody the owner didn't say yes to. What makes a server write to other people
//! (a new meeting with people invited, a change to one the owner organizes, its deletion) is
//! refused by the commands unless they are told `"send": true`, is noted with that yes
//! ([`EventChange::send`]), and is not handed to a host without it ([`permitted`]), whatever a
//! mailbox file says. An answer to somebody else's invitation tells the organizer only when asked.
//! Two things are mail and nothing else, and are noted only with that yes: an answer that
//! proposes another time to the organizer ([`propose_with_answer`]), and a meeting that is sent
//! on to more people ([`note_forward`]).
//!
//! An appointment that is put into another calendar of its account is not moved by the server
//! (Microsoft Graph has no such thing): it is made anew in the other calendar, and the original
//! is deleted only once a download has shown the copy there ([`note_moved`],
//! [`waits_for_copy`]). Should the server refuse either step, or turn out not to have the copy
//! it said it made, the appointment is put back where it was, and that is said.
//!
//! The same holds into and out of another mailbox's calendar (#70: a shared mailbox's, or one
//! opened by address), where the owner may edit both: the copy is noted against the account that
//! keeps the target, the deletion against the one that kept the original ([`EventChange::copy_account`]
//! says where the copy is), and a download of the original's account leaves it out meanwhile. A
//! copy in another mailbox is never taken out again behind its owner's back: should the original
//! not go, the appointment is in both calendars and that is said. A meeting is not moved there.
//!
//! Events go back to the server, as mail does ([`Mailbox::pending`]): a calendar command makes
//! its change here at once and notes an [`EventChange`] in [`Mailbox::event_changes`]
//! ([`note_created`], [`note_changed`], [`note_deleted`]). `sync.eventChanges`
//! ([`event_upload`]) says what the server is to be told, a host carries that out
//! (`mailcraft-net`: Microsoft Graph), and `sync.applyEventChanges`
//! ([`apply_event_outcomes`]) takes in what the server said: the new event's id, the new change
//! key, or the refusal. What couldn't be sent waits and is tried again, later each time; what the
//! server refuses for good is put back as it was, and says so. Until a change has gone through, a
//! download leaves the event as it is here.
//!
//! A server's "yes" is not taken for the change having arrived. Every change it says it took is
//! noted once more ([`EventCheck`], [`Mailbox::event_checks`]) and looked for in the downloads
//! that follow ([`check_sent`]): until one shows the event as it was sent, a download leaves it
//! as it is here (a server may answer a question put right after a change with what it had
//! before); when [`MAX_CHECK_TRIES`] of them haven't, the event is put back to what the server
//! has and the owner is told. And no note is ever dropped without a word: one that can't be sent
//! any more ([`unfit`]) is given up with a line for the owner and one for the log
//! ([`Dropped`]), and what it changed is put back.
//!
//! Undo: a change that hasn't been sent yet is undone like any other, and the note goes with it
//! (the mailbox is put back whole, the notes included). Once the server has been told, the undo
//! history is forgotten, as it is when mail was moved on the server: the mailbox of before is
//! not what the server has any more. Should an answer arrive for a change that was undone while
//! it was on its way, the server is told the opposite ([`apply_event_outcomes`]).
//!
//! Contacts and calendars as such are not sent back yet: the commands that would change or
//! delete one that a server keeps refuse ([`changeable`]). The calendars of an account and the
//! mailbox's categories are taken in as the server lists them ([`apply_calendars`],
//! [`apply_categories`]).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use mailcraft_model::remote::EventZones;
use mailcraft_model::remote::{
    CalendarsDownload, CategoriesDownload, ContactsDownload, EventChange, EventChangeKind, EventCheck, EventFields, EventOp, EventOutcome,
    EventReminder, EventRepeat, EventResult, EventTime, EventWall, EventsDownload, EventsUpload, EventsUploaded, FavoritesDownload, FavoritesOutcome,
    MAX_CHECK_AGE, MAX_CHECK_TRIES, MAX_CONTACTS, MAX_EVENT_CHANGES, MAX_EVENT_CHECKS, MAX_EVENT_OPS, MAX_EVENTS, MAX_FAVORITE_DEPTH, MAX_KEY,
    RemoteContact, RemoteEvent, RemoteEventKind, RemoteFavorite, clean_key, id_tag,
};
use mailcraft_model::zoned;
use mailcraft_model::{
    Account, Attendee, Calendar, Contact, Event, FolderKind, Id, Importance, Mailbox, OnlineMeeting, Origin, Proposal, Response, Rights,
};
use mailcraft_time::{Date, DateTime, Timestamp, Weekday, Zone};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Most calendars a mailbox has, as for the ones made here.
const MAX_CALENDARS: usize = 200;
/// Longest text of a parameter error handed back.
const MAX_SHOWN: usize = 200;
/// Most outcomes taken from one answer, and most lines said about them.
const MAX_OUTCOMES: usize = 1_000;
const MAX_MESSAGES: usize = 20;
/// How long a change that couldn't be sent waits before it is tried again: half a minute after
/// the first failure, twice as long after each one more, a quarter of an hour at most (seconds).
const FIRST_WAIT: i64 = 30;
const LONGEST_WAIT: i64 = 900;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.eventChanges", "Calendar Changes to Send", "Send / Receive", event_changes_command)
            .pure()
            .params(r#"{"account": number} — what that account's server is to be told of events made, changed or deleted here: {"account", "email", "ops": [{"kind": "create"|"update"|"delete"|"respond"|"forward", "event": number, "remote"?: string, "token"?: string, "calendar"?: string, "fields": {…}, "send"?: bool, "comment"?: string, "proposed"?: {"start": seconds, "end": seconds} (another time, with an answer that is sent), "to"?: [{"name", "email"}] (who a meeting is forwarded to)}], "dropped": {"count": number, "reasons": [string], "messages": [string]}} — "dropped": the waiting changes that can't be sent any more and were given up just now (what they changed was put back), for a host to log and to tell the owner. Nothing that writes to other people is listed without the owner's yes to it"#),
        CommandSpec::new("sync.applyEventChanges", "Apply Sent Calendar Changes", "Send / Receive", apply_event_changes_command)
            .pure()
            .params(r#"{"result": EventsUploaded} — what a host heard from the server: {"account": number, "email"?: string, "outcomes": [{"op": EventOp, "result": "done"|"gone"|"refused"|"denied"|"failed", "remote"?: string, "changeKey"?: string, "modified"?: seconds, "error"?: string, "retryAfter"?: seconds}]} — answers {"done", "refused", "failed", "denied", "waiting", "unverified" (changes the server said it took that no download has shown yet), "messages": [string], "dropped": {"count", "reasons": [string], "messages": [string]}}"#),
        CommandSpec::new("sync.applyEvents", "Apply Downloaded Calendar", "Send / Receive", apply_events_command)
            .pure()
            .params(r#"{"download": EventsDownload} — what a host brought from the account's server: {"account": number, "email"?: string, "calendar"?: string, "from": seconds, "to": seconds, "complete": bool, "events": [RemoteEvent]} — answers {"added", "updated", "removed", "unchanged", "skipped", "complete", "calendar", "verified" (changes that were sent and are now seen there), "putBack" (changes the server said it took and doesn't show: the events are as the server has them again), "unverified", "messages": [string], "dropped": {"count", "reasons": [string], "messages": [string]}}"#),
        CommandSpec::new("sync.applyContacts", "Apply Downloaded Contacts", "Send / Receive", apply_contacts_command)
            .pure()
            .params(r#"{"download": ContactsDownload} — what a host brought from the account's server: {"account": number, "email"?: string, "complete": bool, "contacts": [RemoteContact]}"#),
        CommandSpec::new("sync.applyCalendars", "Apply Downloaded Calendar List", "Send / Receive", apply_calendars_command)
            .pure()
            .params(r#"{"download": CalendarsDownload} — the account's calendars as a host brought them from its server: {"account": number, "email"?: string, "complete": bool, "calendars": [{"id": string, "name": string, "color"?: [r, g, b], "group"?: string, "readOnly": bool, "main": bool}]}"#),
        CommandSpec::new("sync.applyCategories", "Apply Downloaded Categories", "Send / Receive", apply_categories_command)
            .pure()
            .params(r#"{"download": CategoriesDownload} — the mailbox's categories as a host brought them from the account's server: {"account": number, "email"?: string, "categories": [{"name": string, "color": [r, g, b]}]}"#),
        CommandSpec::new("sync.applyFavorites", "Apply Downloaded Favorites", "Send / Receive", apply_favorites_command)
            .pure()
            .params(r#"{"download": FavoritesDownload} — the account's Favorites as a host read them from its server (the folders Outlook shows under Favorites): {"account": number, "email"?: string, "outcome": "read"|"missing"|"notPermitted"|"failed", "folders": [{"path": [string] (names from the top of the mailbox down to the folder), "kind": "inbox"|"drafts"|"sent"|"deleted"|"junk"|"archive"|"outbox"|"custom", "root"?: the same (which special folder the first name of the path is)}], "skipped": number, "unresolved": number} — "read": the folders of that account that match (by kind first, then by path, whatever the case) are its Favorites from now on, in that order, and `folder.favorite` is refused for it; "missing" or "notPermitted": the server has no list to show, and the Favorites marked here are what there is; "failed": nothing changes. The marks made here are never touched. Answers {"outcome", "from": "server"|"mailbox", "favorites": [number], "absent": number (listed by the server, not a folder here), "changed": bool}"#),
        CommandSpec::new("sync.forgetRemote", "Forget Downloaded Calendar and Contacts", "Send / Receive", forget_command)
            .pure()
            .params(r#"{"account": number} — removes the events, contacts and calendars that came from that account's server"#),
    ]
}

/// What merging a download changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    pub added: usize,
    pub updated: usize,
    /// Taken out because the server no longer has them.
    pub removed: usize,
    /// Here already, and as the server has them.
    pub unchanged: usize,
    /// Not merged: no id, said twice, a series as a whole, a contact with nothing to show.
    pub skipped: usize,
    /// The calendar the events are in (its id in the mailbox); 0 for contacts.
    pub calendar: Id,
    /// That calendar was made by this merge.
    pub new_calendar: bool,
    /// A calendar of the mailbox's own was renamed or removed beside the server's
    /// ([`settle_calendars`]).
    pub settled: bool,
    /// Changes that were sent and that this download shows on the server ([`EventCheck`]).
    pub verified: usize,
    /// Changes the server said it took and doesn't show: the events are as the server has them
    /// again, and `messages` say so.
    pub put_back: usize,
    /// Changes that were sent and that the server doesn't show yet, after this.
    pub unverified: usize,
    /// What the owner should hear, a line each.
    pub messages: Vec<String>,
    /// Waiting changes that were given up without being sent ([`Dropped`]).
    pub dropped: Vec<Dropped>,
    /// What was noted of sent changes is different for the merge.
    pub checked: bool,
}

impl Merged {
    /// Whether the mailbox is any different for the merge.
    pub fn changed(&self) -> bool {
        self.added + self.updated + self.removed + self.put_back + self.verified > 0
            || self.new_calendar
            || self.settled
            || self.checked
            || !self.dropped.is_empty()
    }

    pub(super) fn say(&mut self, line: String) {
        if self.messages.len() < MAX_MESSAGES {
            self.messages.push(line);
        }
    }

    pub(super) fn json(&self, complete: bool) -> Value {
        json!({
            "added": self.added,
            "updated": self.updated,
            "removed": self.removed,
            "unchanged": self.unchanged,
            "skipped": self.skipped,
            "complete": complete,
            "calendar": if self.calendar == 0 { Value::Null } else { json!(self.calendar) },
            "verified": self.verified,
            "putBack": self.put_back,
            "unverified": self.unverified,
            "messages": self.messages,
            "dropped": dropped_json(&self.dropped),
        })
    }
}

/// A change that waited for a server and was given up without being sent: it could not go any
/// more (its event is gone, it would write to people nobody said to write to, it doesn't match
/// the mailbox). What it changed here is put back to what the server has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dropped {
    /// The account whose server the change was for.
    pub account: Id,
    pub kind: EventChangeKind,
    /// Why, in words that say nothing of the event itself: they go to the log.
    pub reason: &'static str,
    /// What the owner is told (it names the event); empty when there is nothing to tell.
    pub message: String,
}

/// What a command answers of the changes that were given up: how many, why (each reason once),
/// and what the owner is told. Nothing of the events but their titles, in the messages.
pub fn dropped_json(dropped: &[Dropped]) -> Value {
    let mut reasons: Vec<String> = Vec::new();
    for one in dropped.iter().take(MAX_OUTCOMES) {
        let reason = format!("{:?}: {}", one.kind, one.reason);
        if reasons.len() < MAX_MESSAGES && !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
    let messages: Vec<&str> = dropped.iter().map(|one| one.message.as_str()).filter(|line| !line.is_empty()).take(MAX_MESSAGES).collect();
    json!({"count": dropped.len(), "reasons": reasons, "messages": messages})
}

/// Why a waiting change is given up ([`unfit`]).
const NO_ACCOUNT: &str = "its account is no longer in the mailbox";
const NOT_SAID: &str = "it would make the server write to the people invited, and nobody said to send that";
const NO_EVENT: &str = "the event it is about is no longer in the calendar";
const NO_SERIES: &str = "the series it is about is no longer in the calendar";
const NOT_ITS_OWN: &str = "it doesn't match the event it names";
const NO_NAME: &str = "it doesn't say which event it is about";
const STILL_HERE: &str = "the event it deletes is still in the calendar";
const SAID_TWICE: &str = "another change about the same event waits before it";
const TOO_MANY: &str = "too many changes were waiting";
/// And why one that was sent is: the server no longer has the event.
const GONE_THERE: &str = "the server no longer has the event";

/// The account whose server keeps an item of this origin, for as long as the mailbox has that
/// account. Such an item is a copy of the server's. (When the account is gone nothing will ever
/// update the item again: it is then the mailbox's own, to change or delete like any other.)
pub fn keeper<'a>(mailbox: &'a Mailbox, origin: &Origin) -> Option<&'a Account> {
    if origin.is_local() { None } else { mailbox.account(origin.account) }
}

/// What an account is called when something is said about its server.
pub(crate) fn who(account: &Account) -> &str {
    [account.name.trim(), account.email.trim()].into_iter().find(|name| !name.is_empty()).unwrap_or("its account")
}

/// Refuse to change `what` ("contact", "calendar") when an account's server keeps it: the change
/// would not reach the server, and the next download would take it back. (Events are sent back:
/// [`note_changed`].)
pub(crate) fn changeable(mailbox: &Mailbox, origin: &Origin, what: &'static str) -> Result<(), CmdError> {
    let Some(account) = keeper(mailbox, origin) else { return Ok(()) };
    let who = who(account);
    Err(failed!(
        "This {what} is kept by the server of {who}. Changes made here are not sent back yet, so it can't be changed or deleted here.",
        what = mailcraft_i18n::Arg::words(what),
        who
    ))
}

/// The instant of a wall-clock time here, in a zone whose offset at an instant is `offset_at`.
/// Asked twice, so a time near a daylight-saving change still gets its own offset.
#[allow(dead_code)] // (An event's own times are read through `Event::instants`, which knows which of two it is.)
fn instant_of(wall: DateTime, offset_at: &dyn Fn(Timestamp) -> i32) -> Timestamp {
    // As everything that reads a wall-clock time reads it (`mailcraft_model::zoned`): the first
    // of two instants the clocks show a time at, in whichever zone.
    zoned::instant_at(wall, offset_at)
}

/// The zones an event of the mailbox keeps as its own for what a server says its zones are
/// (`start`, `end`: names as the server gives them), when it is at the instants `at`.
fn zones_from_remote(start: &str, end: &str, at: (Timestamp, Timestamp), repeats: bool, offset_at: &dyn Fn(Timestamp) -> i32) -> (String, String) {
    zoned::own_zones(Zone::named(start), Zone::named(end), at, repeats, offset_at)
}

/// The day of `series` that an occurrence at `instant` is, as the series names its days
/// ([`mailcraft_model::zoned`]): for a series that repeats in a zone of its own, by that zone.
fn series_day_at(series: &Event, instant: Timestamp, offset_at: &dyn Fn(Timestamp) -> i32) -> Date {
    if series.all_day { occurrence_day(true, instant, offset_at) } else { series.occurrence_day(instant, offset_at) }
}

/// The occurrence of a series on its day `day` as an event of its own, in a calendar whose
/// offset is `offset_at`: [`occurrence_of`], for a series that may repeat in a zone of its own
/// (the occurrence is then at that zone's clock time, whatever that is here on that day).
pub fn occurrence_in(series: &Event, day: Date, offset_at: &dyn Fn(Timestamp) -> i32) -> Event {
    let mut one = occurrence_of(series, day);
    if !series.all_day && series.zone().is_some() {
        let start = series.occurrence_instant(day, offset_at);
        let (begins, ends) = series.instants(offset_at);
        one.set_instants(start, start.saturating_add(ends.saturating_sub(begins)), offset_at);
    }
    one
}

/// The wall-clock times here of a server's instants ([`event_from_remote`] says how).
fn local_times(start: Timestamp, end: Timestamp, all_day: bool, offset_at: &dyn Fn(Timestamp) -> i32) -> (DateTime, DateTime) {
    let local = |instant: Timestamp| DateTime::from_timestamp(instant, offset_at(instant));
    if all_day {
        let day = |instant: Timestamp| {
            let named = DateTime::from_timestamp(instant, 0);
            if named.seconds_of_day() == 0 {
                return named.date;
            }
            // A server that names a midnight somewhere instead: the midnight nearest to it here.
            let here = local(instant);
            if here.hour >= 12 { here.date.add_days(1) } else { here.date }
        };
        let first = day(start);
        // At least one day; the end is the midnight after the last.
        (DateTime::at_midnight(first), DateTime::at_midnight(day(end).max(first.add_days(1))))
    } else {
        let start = local(start);
        (start, local(end).max(start))
    }
}

/// The day a whole-day event is on that a server has at `instant`. A server names the day by a
/// midnight: UTC's (the day is the one that reads), or the midnight of the zone the event was
/// made in (`zone`, by the name the server gives it), and the day is then the one it is there,
/// wherever this computer is. When neither says, it is the midnight nearest to the instant here.
fn whole_day(instant: Timestamp, zone: &str, offset_at: &dyn Fn(Timestamp) -> i32) -> Date {
    let named = DateTime::from_timestamp(instant, 0);
    if named.seconds_of_day() == 0 {
        return named.date;
    }
    if let Some(there) = Zone::named(zone).map(|zone| zone.wall(instant)).filter(|there| there.seconds_of_day() == 0) {
        return there.date;
    }
    local_times(instant, instant, true, offset_at).0.date
}

/// The wall-clock times here of an event as a server has it: [`local_times`], with the days of a
/// whole-day event read by the zone it was made in ([`whole_day`]).
fn remote_times(remote: &RemoteEvent, offset_at: &dyn Fn(Timestamp) -> i32) -> (DateTime, DateTime) {
    if !remote.all_day {
        return local_times(remote.start, remote.end, false, offset_at);
    }
    let ends_in = if remote.end_zone.is_empty() { &remote.start_zone } else { &remote.end_zone };
    let first = whole_day(remote.start, &remote.start_zone, offset_at);
    // At least one day; the end is the midnight after the last.
    let after = whole_day(remote.end, ends_in, offset_at).max(first.add_days(1));
    (DateTime::at_midnight(first), DateTime::at_midnight(after))
}

/// When an event of the mailbox is, as the server counts it: the instants of its wall-clock
/// times here, each with the offset of its own moment; for a whole-day event the midnights (UTC)
/// of its first day and of the day after its last.
pub fn server_time(event: &Event, offset_at: &dyn Fn(Timestamp) -> i32) -> EventTime {
    if event.all_day {
        let start = event.start.to_timestamp(0);
        return EventTime { start, end: event.end.to_timestamp(0).max(start.saturating_add(mailcraft_time::DAY)), all_day: true };
    }
    // (Which of two instants a time in an hour that comes twice is, the event says.)
    let (start, end) = event.instants(offset_at);
    EventTime { start, end, all_day: false }
}

/// Everything of an event that a server is told ([`EventFields`]), as a new event is sent.
pub fn fields_of(event: &Event, offset_at: &dyn Fn(Timestamp) -> i32) -> EventFields {
    // By the clock of its own zone, for an event that has one: that is the clock a series
    // repeats by and counts its days by, and the zone the server is to keep the event in.
    let own = event.zone().map(|zone| {
        let (start, end) = event.instants(offset_at);
        let ends_in = event.zone_of_end().unwrap_or(zone);
        (EventWall { start: zone.wall(start), end: ends_in.wall(end) }, EventZones { start: zone.id(), end: ends_in.id() })
    });
    let (wall, zones) = match own {
        Some((wall, zones)) => (wall, Some(zones)),
        None => (EventWall { start: event.start, end: event.end.max(event.start) }, None),
    };
    let mut fields = EventFields {
        title: Some(event.title.clone()),
        location: Some(event.location.clone()),
        description: Some(event.description.clone()),
        time: Some(server_time(event, offset_at)),
        show_as: Some(event.show_as),
        private: Some(event.private),
        reminder: Some(EventReminder { on: event.reminder_minutes.is_some(), minutes: event.reminder_minutes.unwrap_or(0) }),
        // A series repeats by the clock (here, or in its own zone), which the server is told
        // beside the instants.
        wall: event.recurrence.as_ref().filter(|_| !event.all_day).map(|_| wall),
        zones,
        repeat: event
            .recurrence
            .clone()
            .map(|rule| EventRepeat { rule: Some(rule), first: if event.all_day { event.start.date } else { wall.start.date } }),
        // Who is invited, not what they answered: that is the server's to say.
        attendees: (!event.attendees.is_empty()).then(|| invited(event)),
        categories: (!event.categories.is_empty()).then(|| event.categories.clone()),
        // What a server takes to be so unless it is told otherwise is left unsaid.
        importance: (event.importance != Importance::Normal).then_some(event.importance),
        request_responses: (!event.response_options.request_responses).then_some(false),
        allow_proposals: (!event.response_options.allow_proposals).then_some(false),
        // (One that has a link to join by is somebody's online meeting already: the server is
        // asked to make one only for a meeting that has none yet.)
        online: event.online.as_ref().filter(|online| online.join_url.is_empty()).map(|online| online.provider),
    };
    fields.tidy();
    fields
}

/// The people invited to an event, as a server is told who they are.
fn invited(event: &Event) -> Vec<Attendee> {
    event
        .attendees
        .iter()
        .map(|attendee| Attendee { address: attendee.address.clone(), optional: attendee.optional, response: Response::None })
        .collect()
}

/// What of `now` is not as `base` has it, of what a server is told: what a change sends.
pub fn changed_fields(base: &Event, now: &Event, offset_at: &dyn Fn(Timestamp) -> i32) -> EventFields {
    // Both as they would be sent: what differs only in what is cut away on the way (a carriage
    // return, a space at the end) is no difference, and would never stop being sent.
    let (was, all) = (fields_of(base, offset_at), fields_of(now, offset_at));
    // A rule goes with the times it counts from: a series is told when it is by the clock.
    let repeats = was.repeat != all.repeat;
    // (A series that is by another clock than it was is another series, also when its first
    // occurrence is at the same instant. Zones are not compared by name: a series made here
    // and the same series as its server names its zone are the same.)
    let another_clock = was.wall.is_some() && all.wall.is_some() && was.wall != all.wall;
    let retimed = was.time != all.time || another_clock || (repeats && all.repeat.is_some());
    // Who is invited is who it is by address: a name the server spells its own way is no change.
    let people = |list: &Option<Vec<Attendee>>| {
        let mut people: Vec<(String, bool)> = list.iter().flatten().map(|who| (who.address.email.to_lowercase(), who.optional)).collect();
        people.sort();
        people
    };
    let reinvited = people(&was.attendees) != people(&all.attendees);
    EventFields {
        title: all.title.filter(|title| was.title.as_ref() != Some(title)),
        location: all.location.filter(|place| was.location.as_ref() != Some(place)),
        description: all.description.filter(|words| was.description.as_ref() != Some(words)),
        time: all.time.filter(|_| retimed),
        show_as: all.show_as.filter(|show_as| was.show_as != Some(*show_as)),
        private: all.private.filter(|private| was.private != Some(*private)),
        reminder: all.reminder.filter(|reminder| was.reminder != Some(*reminder)),
        wall: all.wall.filter(|_| retimed),
        zones: all.zones.filter(|_| retimed),
        repeat: repeats.then(|| all.repeat.unwrap_or(EventRepeat { rule: None, first: now.start.date })),
        attendees: reinvited.then(|| all.attendees.unwrap_or_default()),
        categories: (was.categories != all.categories).then(|| all.categories.unwrap_or_default()),
        importance: (base.importance != now.importance).then_some(now.importance),
        request_responses: (base.response_options.request_responses != now.response_options.request_responses)
            .then_some(now.response_options.request_responses),
        allow_proposals: (base.response_options.allow_proposals != now.response_options.allow_proposals)
            .then_some(now.response_options.allow_proposals),
        // Held online from now on. (There is no saying "no longer": [`EventFields::online`].)
        online: now.online.as_ref().filter(|online| base.online.is_none() && online.join_url.is_empty()).map(|online| online.provider),
    }
}

/// Whether a change in these fields is one the other people of a meeting would see (and be
/// written to about by the server): anything but how it shows in the owner's own calendar.
pub fn seen_by_others(fields: &EventFields) -> bool {
    let quiet = EventFields { show_as: fields.show_as, reminder: fields.reminder, categories: fields.categories.clone(), ..Default::default() };
    *fields != quiet
}

/// Make `event` as a server was told it is: the fields that were sent, with times as they read
/// here.
fn put_fields(event: &mut Event, fields: &EventFields, offset_at: &dyn Fn(Timestamp) -> i32) {
    if let Some(title) = &fields.title {
        event.title = title.clone();
    }
    if let Some(place) = &fields.location {
        event.location = place.clone();
    }
    if let Some(words) = &fields.description {
        event.description = words.clone();
    }
    if let Some(time) = fields.time {
        (event.start, event.end) = local_times(time.start, time.end, time.all_day, offset_at);
        event.all_day = time.all_day;
        (event.start_repeated, event.end_repeated) = (false, false);
        if !time.all_day {
            event.set_instants(time.start, time.end, offset_at);
        }
        // Its zones went with its times: the ones that were named, or none.
        let zones = fields.zones.clone().filter(|_| !time.all_day).unwrap_or_default();
        (event.start_zone, event.end_zone) = (zones.start.clone(), if zones.end == zones.start { String::new() } else { zones.end });
    }
    if let Some(show_as) = fields.show_as {
        event.show_as = show_as;
    }
    if let Some(private) = fields.private {
        event.private = private;
    }
    if let Some(reminder) = fields.reminder {
        event.reminder_minutes = reminder.on.then_some(reminder.minutes);
    }
    if let Some(repeat) = &fields.repeat {
        event.recurrence = repeat.rule.clone();
        if repeat.rule.is_none() {
            event.exceptions.clear();
        }
    }
    if let Some(people) = &fields.attendees {
        // Those who were invited before keep what they answered.
        let answered =
            |who: &Attendee| event.attendees.iter().find(|old| old.address.same_mailbox(&who.address)).map_or(Response::None, |old| old.response);
        event.attendees = people.iter().map(|who| Attendee { response: answered(who), ..who.clone() }).collect();
    }
    if let Some(categories) = &fields.categories {
        event.categories = categories.clone();
    }
    if let Some(importance) = fields.importance {
        event.importance = importance;
    }
    if let Some(asked) = fields.request_responses {
        event.response_options.request_responses = asked;
    }
    if let Some(allowed) = fields.allow_proposals {
        event.response_options.allow_proposals = allowed;
    }
    if let Some(provider) = fields.online
        && event.online.is_none()
    {
        event.online = Some(OnlineMeeting { provider, join_url: String::new() });
    }
}

/// The account a download is for, when the mailbox has it and it is the one the download names.
pub(super) fn owner<'a>(mailbox: &'a Mailbox, account: Id, email: &str) -> Result<&'a Account, CmdError> {
    let owner = mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    // A download for another account (the mailbox was replaced while it was under way) would fill
    // this one's calendar with a stranger's.
    if !email.trim().is_empty() && !owner.email.trim().eq_ignore_ascii_case(email.trim()) {
        return Err(failed!("this download was for another account, not for {name}", name = owner.name));
    }
    Ok(owner)
}

/// The event of the mailbox that a server's event is, for an owner with the address `owner`, in
/// a zone whose offset from UTC at an instant is `offset_at` (minutes east). The id, the
/// calendar and the account are left for the merge to fill in.
///
/// The server's times are instants; the mailbox keeps wall-clock times. Each instant is turned
/// into the wall-clock time of its own moment, so that a meeting at nine is at nine on both
/// sides of a daylight-saving change. A whole-day event keeps the days the server names: they
/// are the same days everywhere.
pub fn event_from_remote(remote: &RemoteEvent, owner: &str, offset_at: &dyn Fn(Timestamp) -> i32) -> Event {
    let (start, end) = remote_times(remote, offset_at);
    // Somebody else's meeting: the server says so, or else the organizer's address does.
    let other =
        |organizer: &mailcraft_model::Address| !organizer.email.trim().is_empty() && !organizer.email.trim().eq_ignore_ascii_case(owner.trim());
    let guest = match remote.is_organizer {
        Some(organizes) => !organizes,
        None => !owner.trim().is_empty() && remote.organizer.as_ref().is_some_and(other),
    };
    let mut attendees = remote.attendees.clone();
    // What the owner answered is known for sure, whatever the list of people says.
    if remote.response != Response::None && !owner.trim().is_empty() {
        for attendee in attendees.iter_mut().filter(|attendee| attendee.address.email.eq_ignore_ascii_case(owner.trim())) {
            attendee.response = remote.response;
        }
    }
    // Where it was made, when the clock there shows another time than the one here.
    let (start_zone, end_zone) = match remote.all_day {
        true => (String::new(), String::new()),
        false => zones_from_remote(&remote.start_zone, &remote.end_zone, (remote.start, remote.end.max(remote.start)), false, offset_at),
    };
    // (And which of two instants it is, in an hour the clocks here show twice.)
    let again = |instant: Timestamp| !remote.all_day && zoned::wall_of(instant, offset_at).1;
    Event {
        uid: remote.uid.clone(),
        title: remote.title.clone(),
        location: remote.location.clone(),
        description: remote.description.clone(),
        start,
        end,
        all_day: remote.all_day,
        start_zone,
        end_zone,
        start_repeated: again(remote.start),
        end_repeated: again(remote.end.max(remote.start)),
        organizer: remote.organizer.clone(),
        attendees,
        reminder_minutes: remote.reminder_minutes,
        show_as: remote.show_as,
        private: remote.private,
        cancelled: remote.cancelled,
        categories: remote.categories.clone(),
        origin: Origin {
            account: 0,
            id: remote.id.clone(),
            change_key: remote.change_key.clone(),
            series: remote.series.clone(),
            guest,
            occurrence: None,
        },
        importance: remote.importance,
        response_options: remote.options,
        online: remote.online.clone(),
        proposals: proposals_from_remote(remote, offset_at),
        has_attachments: remote.attachments,
        ..Default::default()
    }
}

/// The other times people proposed for a server's meeting, by the clock here.
fn proposals_from_remote(remote: &RemoteEvent, offset_at: &dyn Fn(Timestamp) -> i32) -> Vec<Proposal> {
    let local = |proposal: &mailcraft_model::remote::RemoteProposal| {
        let (start, end) = local_times(proposal.start, proposal.end, false, offset_at);
        Proposal { from: proposal.from.clone(), start, end }
    };
    remote.proposals.iter().map(local).collect()
}

/// A download that doesn't say how much its events matter, whether they are held online and the
/// rest of it ([`EventsDownload::has_options`]): those stay as they are here.
fn keep_options(old: &Event, fresh: &mut Event) {
    fresh.importance = old.importance;
    fresh.response_options = old.response_options;
    fresh.online = old.online.clone();
    fresh.proposals = old.proposals.clone();
    fresh.has_attachments = old.has_attachments;
}

/// The event of the mailbox that a server's series is: [`event_from_remote`], repeating by the
/// series' rule when it has one that can be said here. The days it skips are for the merge to
/// work out, from the occurrences that came with it.
pub fn series_from_remote(remote: &RemoteEvent, owner: &str, offset_at: &dyn Fn(Timestamp) -> i32) -> Event {
    let mut event = event_from_remote(remote, owner, offset_at);
    let Some(mut rule) = remote.recurrence.clone() else { return event };
    // A series whose zone is known repeats by that zone's clock, and its rule counts that
    // zone's days as it stands (`mailcraft_model::zoned`): nothing of it is shifted.
    if !remote.all_day {
        (event.start_zone, event.end_zone) = zones_from_remote(&remote.start_zone, "", (remote.start, remote.end.max(remote.start)), true, offset_at);
    }
    if event.zone().is_some() {
        event.recurrence = Some(rule);
        return event;
    }
    // Of a series whose zone nobody says, the rule counts its days where the series was made.
    // Here, the first of them may be the day before or the day after, and so then is every other.
    let shift = remote.series_day.map_or(0, |day| day.days_until(event.start.date));
    if shift == 1 || shift == -1 {
        rule.by_day = rule.by_day.iter().map(|day| Weekday::from_index((i64::from(day.index()) + shift).rem_euclid(7))).collect();
        rule.by_day.sort();
        rule.until = rule.until.map(|until| until.add_days(shift));
    }
    event.recurrence = Some(rule);
    event
}

/// When a series has its occurrence of `day`, as the server counts it ([`server_time`]).
pub fn occurrence_instant(series: &Event, day: Date, offset_at: &dyn Fn(Timestamp) -> i32) -> Timestamp {
    if series.all_day { DateTime::at_midnight(day).to_timestamp(0) } else { series.occurrence_instant(day, offset_at) }
}

/// The day here that an occurrence falls on which its series has at `instant`.
fn occurrence_day(all_day: bool, instant: Timestamp, offset_at: &dyn Fn(Timestamp) -> i32) -> Date {
    local_times(instant, instant, all_day, offset_at).0.date
}

/// The occurrence of a series on `day` as an event of its own: the same in everything, but
/// once. (Its id, and where it comes from, are for whoever asks to fill in.)
pub fn occurrence_of(series: &Event, day: Date) -> Event {
    let start = DateTime { date: day, ..series.start };
    Event { id: 0, start, end: start.add_minutes(series.duration_minutes()), recurrence: None, exceptions: Vec::new(), ..series.clone() }
}

/// The day of its series that a server's occurrence is, when it is just what the series (as it
/// is here, `series`, which happens on `days`) has on that day: there is nothing of its own to
/// keep of it then.
fn covered(series: &Event, days: &HashSet<Date>, instance: &RemoteEvent, offset_at: &dyn Fn(Timestamp) -> i32) -> Option<Date> {
    if instance.kind != RemoteEventKind::Occurrence || instance.all_day != series.all_day || instance.cancelled != series.cancelled {
        return None;
    }
    let (start, end) = remote_times(instance, offset_at);
    if series.zone().is_some() {
        // By the clock of the series' own zone: where that puts the occurrence of its day.
        let day = series.occurrence_day(instance.start, offset_at);
        return (series.occurrence_times(day, offset_at) == (start, end) && days.contains(&day)).then_some(day);
    }
    let as_the_series = start == DateTime { date: start.date, ..series.start } && start.minutes_until(end) == series.duration_minutes();
    (as_the_series && days.contains(&start.date)).then_some(start.date)
}

/// Most days a series skips that are kept, as for a series made here.
const MAX_SKIPPED: usize = 5_000;
/// How many days before a stretch an occurrence that reaches into it may start.
const REACH_DAYS: i64 = 40;

/// The days a server's series skips here, after a download: the days its rule names in the
/// stretch on which the server has no occurrence that is the series' own (it was deleted there,
/// or it was changed and is an event of its own, `loose`); what was skipped before outside the
/// stretch; and the days of occurrences that were changed or deleted here and wait to be sent
/// (`waiting`). A download that isn't complete says nothing about what is missing from it.
#[allow(clippy::too_many_arguments)]
fn skipped_days(
    series: &Event,
    before: &[Date],
    download: &EventsDownload,
    days: &HashSet<Date>,
    own: Option<&HashSet<Date>>,
    loose: Option<&HashSet<Date>>,
    waiting: Option<&Vec<Timestamp>>,
    offset_at: &dyn Fn(Timestamp) -> i32,
) -> Vec<Date> {
    let complete = download.complete && download.events.len() <= MAX_EVENTS && download.from < download.to;
    let asked = |day: Date| complete && (download.from..download.to).contains(&occurrence_instant(series, day, offset_at));
    let is_own = |day: &Date| own.is_some_and(|own| own.contains(day));
    let mut skipped: BTreeSet<Date> = before.iter().copied().filter(|day| !asked(*day) && !is_own(day)).collect();
    skipped.extend(days.iter().copied().filter(|day| asked(*day) && !is_own(day)));
    skipped.extend(loose.into_iter().flatten().copied());
    skipped.extend(waiting.into_iter().flatten().map(|instant| series_day_at(series, *instant, offset_at)));
    skipped.into_iter().take(MAX_SKIPPED).collect()
}

/// The contact of the mailbox that a server's contact is. The id and the account are left for
/// the merge to fill in.
pub fn contact_from_remote(remote: &RemoteContact) -> Contact {
    Contact {
        display_name: remote.display_name.clone(),
        first: remote.first.clone(),
        middle: remote.middle.clone(),
        last: remote.last.clone(),
        nickname: remote.nickname.clone(),
        company: remote.company.clone(),
        title: remote.title.clone(),
        department: remote.department.clone(),
        emails: remote.emails.clone(),
        phones: remote.phones.clone(),
        addresses: remote.addresses.clone(),
        websites: remote.websites.clone(),
        birthday: remote.birthday,
        notes: remote.notes.clone(),
        origin: Origin { account: 0, id: remote.id.clone(), change_key: remote.change_key.clone(), ..Default::default() },
        ..Default::default()
    }
}

/// When an event of the mailbox starts, as the server counts it: the instant of its wall-clock
/// time here, or for a whole-day event the midnight (UTC) of its first day.
fn server_start(event: &Event, offset_at: &dyn Fn(Timestamp) -> i32) -> Timestamp {
    if event.all_day { event.start.to_timestamp(0) } else { event.instants(offset_at).0 }
}

/// The main calendar of an account's server in the mailbox, when a download has brought one and
/// the mailbox still has the account: where an event made here goes to be on the server too.
pub(crate) fn server_calendar(mailbox: &Mailbox) -> Option<&Calendar> {
    // The default account's first; never a shared mailbox's (#70), which is no place for what
    // its owner makes without saying where.
    mailbox
        .accounts
        .iter()
        .filter(|account| !account.is_shared())
        .find_map(|account| mailbox.calendars.iter().find(|calendar| calendar.origin.account == account.id && calendar.origin.id.is_empty()))
}

/// The main calendar of an account's server in the mailbox ([`server_calendar`]), by its id here.
pub fn server_calendar_id(mailbox: &Mailbox) -> Option<Id> {
    server_calendar(mailbox).map(|calendar| calendar.id)
}

/// The calendars of the mailbox's own that hold something while an account's server keeps a
/// calendar too, each with how many events are in it: what is in them is on this computer only,
/// and is in nobody's Outlook calendar, however much it looks the part beside the server's.
/// Nothing when no server keeps a calendar here: then everything is on this computer, and
/// nobody expects otherwise.
pub fn only_here(mailbox: &Mailbox) -> Vec<(Id, usize)> {
    if server_calendar(mailbox).is_none() {
        return Vec::new();
    }
    let own = mailbox.calendars.iter().filter(|calendar| keeper(mailbox, &calendar.origin).is_none());
    own.map(|calendar| (calendar.id, mailbox.events.values().filter(|event| event.calendar == calendar.id).count())).filter(|(_, n)| *n > 0).collect()
}

/// Whether the event `id` is kept on this computer only although an account's server keeps a
/// calendar here ([`only_here`]): a change to it goes nowhere, and whoever makes one is told.
pub fn stays_here(mailbox: &Mailbox, id: Id) -> bool {
    let own = |calendar: Id| mailbox.calendar(calendar).is_some_and(|calendar| keeper(mailbox, &calendar.origin).is_none());
    server_calendar(mailbox).is_some() && mailbox.event(id).is_some_and(|event| own(event.calendar))
}

/// Whether the events of `calendar` are the owner's own to be reminded of (#70): those of the
/// account's own calendars and of the ones kept on this computer. Not a shared mailbox's, not a
/// calendar somebody shares with the account (they are the other person's to be reminded of), and
/// not one that is only free and busy blocks.
pub fn is_own_calendar(mailbox: &Mailbox, calendar: &Calendar) -> bool {
    match keeper(mailbox, &calendar.origin) {
        None => true,
        Some(account) => !account.is_shared() && calendar.rights != Rights::FreeBusy && !super::calendar_settings::is_shared_with(account, calendar),
    }
}

/// Give every shared mailbox that has none a calendar to be ticked (#70): its main one, unticked
/// and read only, named after its address, until the server lists it. Ticking it is what makes a
/// run read the shared mailbox's calendars (and ask the owner for permission, once). Whether
/// anything was added.
pub fn ensure_shared_calendars(s: &mut Session) -> bool {
    let mailbox = &mut s.mailbox;
    let wanted: Vec<(Id, String)> = mailbox
        .accounts
        .iter()
        .filter(|account| account.is_shared() && mailbox.owner_of(account.id).is_some())
        .filter(|account| !mailbox.calendars.iter().any(|calendar| calendar.origin.account == account.id && calendar.origin.id.is_empty()))
        .map(|account| (account.id, account.email.trim().chars().take(mailcraft_model::MAX_NAME).collect()))
        .collect();
    let mut added = false;
    for (account, name) in wanted {
        if mailbox.calendars.len() >= MAX_CALENDARS {
            break;
        }
        let id = mailbox.alloc_id();
        let color = super::calendar::next_color(mailbox);
        let name = if name.is_empty() { SERVER_CALENDAR.to_string() } else { name };
        let origin = Origin { account, ..Default::default() };
        mailbox.calendars.push(Calendar { id, name, color, visible: false, origin, rights: Rights::Read, ..Default::default() });
        added = true;
    }
    if added {
        s.mailbox_changed();
    }
    added
}

/// Whether one more change to an event may be noted for a server: a mailbox that can't reach its
/// server for a long time doesn't pile them up without end.
pub(crate) fn room_for_change(mailbox: &Mailbox) -> Result<(), CmdError> {
    if mailbox.event_changes.len() >= MAX_EVENT_CHANGES {
        return Err(fail("Too many calendar changes are waiting to be sent to the server. Let Send/Receive get them through first."));
    }
    Ok(())
}

/// A mark for a new event ([`EventChange::token`]), in the shape of a GUID: no two the same,
/// whenever and wherever they were made.
fn new_token(event: Id, salt: u64) -> String {
    static MADE: AtomicU64 = AtomicU64::new(0);
    let mix = |mut x: u64| {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    };
    let nth = MADE.fetch_add(1, Ordering::Relaxed);
    // The clock as the machine has it, not as a test may have stopped it.
    let a = mix(mailcraft_time::now().unsigned_abs() ^ mix(event) ^ nth.rotate_left(40));
    let b = mix(a ^ salt.rotate_left(17) ^ nth);
    format!("{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}", a >> 32, (a >> 16) & 0xFFFF, a & 0xFFF, (b >> 48) & 0xFFF, b & 0xFFFF_FFFF_FFFF)
}

/// Refuse to change what is in a calendar that an account's server keeps and the account may
/// only read (somebody else's, a calendar of holidays): the server would refuse it anyway.
pub(crate) fn writable(mailbox: &Mailbox, calendar: Id) -> Result<(), CmdError> {
    match mailbox.calendar(calendar) {
        // A shared mailbox's calendar, with the rights this account has on it (#70): the owner
        // of the mailbox sets them, and they are read again at every listing.
        Some(c) if !c.can_edit() && keeper(mailbox, &c.origin).is_some_and(Account::is_shared) => {
            if c.rights == Rights::FreeBusy {
                Err(failed!(
                    "The shared mailbox's calendar \"{name}\" only shows when its owner is busy: this account can't change it.",
                    name = c.name
                ))
            } else {
                Err(failed!(
                    "This account may only read the shared mailbox's calendar \"{name}\": the mailbox's owner hasn't given it edit rights (or they haven't been read yet).",
                    name = c.name
                ))
            }
        }
        Some(c) if !c.can_edit() && keeper(mailbox, &c.origin).is_some() => {
            if c.rights == Rights::FreeBusy {
                Err(failed!("The calendar \"{name}\" only shows when its owner is busy: this account can't change it.", name = c.name))
            } else {
                Err(failed!("The calendar \"{name}\" can only be read: its owner hasn't let this account change it.", name = c.name))
            }
        }
        _ => Ok(()),
    }
}

/// An event was made in (or put into) a calendar that a server keeps: it is that server's from
/// now on, and the server is to be told. Nothing to do for a calendar of the mailbox's own;
/// refused for one the account may only read.
pub(crate) fn note_created(s: &mut Session, id: Id) -> Result<(), CmdError> {
    let mailbox = &s.mailbox;
    let Some(event) = mailbox.event(id) else { return Ok(()) };
    writable(mailbox, event.calendar)?;
    let account = mailbox.calendar(event.calendar).and_then(|calendar| keeper(mailbox, &calendar.origin)).map(|account| account.id);
    let Some(account) = account else { return Ok(()) };
    room_for_change(mailbox)?;
    let token = new_token(id, s.rev() ^ s.now().unsigned_abs().rotate_left(32));
    let mailbox = &mut s.mailbox;
    mailbox.event_mut(id)?.origin = Origin { account, ..Default::default() };
    mailbox.event_changes.retain(|change| !(change.kind == EventChangeKind::Create && change.event == id));
    mailbox.event_changes.push(EventChange { account, kind: EventChangeKind::Create, event: id, token, ..Default::default() });
    Ok(())
}

/// An event was changed here; `before` is what it was. When a server keeps it, the server is to
/// be told what differs from its own copy: `before`, or what it was before an earlier change
/// that is still waiting. A change that takes everything back to that leaves nothing to tell.
pub(crate) fn note_changed(mailbox: &mut Mailbox, before: &Event, offset_at: &dyn Fn(Timestamp) -> i32) -> Result<(), CmdError> {
    let Some(now) = mailbox.event(before.id).cloned() else { return Ok(()) };
    let Some(account) = keeper(mailbox, &now.origin).map(|account| account.id) else { return Ok(()) };
    // One the server doesn't have yet is sent as it is when it goes.
    if now.origin.id.is_empty() {
        return Ok(());
    }
    let waiting = |change: &EventChange| change.kind == EventChangeKind::Update && change.account == account && change.remote == now.origin.id;
    match mailbox.event_changes.iter().position(waiting) {
        Some(at) => {
            let back = mailbox
                .event_changes
                .get(at)
                .and_then(|change| change.base.as_deref())
                .is_some_and(|base| changed_fields(base, &now, offset_at).is_empty());
            if back {
                mailbox.event_changes.remove(at);
            } else if let Some(change) = mailbox.event_changes.get_mut(at) {
                // Something new to say: it doesn't wait out what the last try earned.
                change.event = now.id;
                change.retry_at = 0;
            }
        }
        None if changed_fields(before, &now, offset_at).is_empty() => {}
        None => {
            room_for_change(mailbox)?;
            mailbox.event_changes.push(EventChange {
                account,
                kind: EventChangeKind::Update,
                event: now.id,
                remote: now.origin.id.clone(),
                base: Some(Box::new(before.clone())),
                ..Default::default()
            });
        }
    }
    Ok(())
}

/// An event was deleted here; `before` is what it was. When a server keeps it, the server is to
/// delete it too; one the server never had needs no telling.
pub(crate) fn note_deleted(mailbox: &mut Mailbox, before: &Event) -> Result<(), CmdError> {
    let Some(account) = keeper(mailbox, &before.origin).map(|account| account.id) else { return Ok(()) };
    if before.origin.id.is_empty() {
        // One that was on its way into another calendar ([`note_moved`]): the original, which
        // the server still has, is what is to be deleted now.
        let making = |change: &&EventChange| change.kind == EventChangeKind::Create && change.account == account && change.event == before.id;
        let original = mailbox.event_changes.iter().find(making).filter(|change| !change.replaces.is_empty());
        let original = original.map(|change| (change.replaces.clone(), change.base.clone()));
        mailbox.event_changes.retain(|change| !(change.kind == EventChangeKind::Create && change.account == account && change.event == before.id));
        if let Some((remote, base)) = original {
            // (The original is on the server it was moved from, which may not be this one.)
            let account = base.as_deref().map_or(account, |base| base.origin.account);
            mailbox.event_changes.push(EventChange { account, kind: EventChangeKind::Delete, event: before.id, remote, base, ..Default::default() });
        }
        // One occurrence of a series that was changed here and not sent yet: what the server
        // is to hear now is that the occurrence is gone.
        for change in
            mailbox.event_changes.iter_mut().filter(|change| change.account == account && change.event == before.id && change.of_occurrence())
        {
            change.kind = EventChangeKind::Delete;
            (change.base, change.retry_at) = (None, 0);
        }
        return Ok(());
    }
    // A series goes with what was kept of its occurrences: the server deletes them with it.
    if before.recurrence.is_some() {
        let of_series = |origin: &Origin| origin.account == account && origin.series == before.origin.id;
        let loose: Vec<Id> = mailbox.events.values().filter(|event| of_series(&event.origin)).map(|event| event.id).collect();
        for id in loose {
            let _ = mailbox.remove_event(id);
        }
        mailbox.event_changes.retain(|change| !(change.account == account && change.series == before.origin.id && change.of_occurrence()));
    }
    let remote = before.origin.id.clone();
    // A change that was waiting is overtaken; what the server has is still what it was before it.
    let waiting = |change: &EventChange| change.kind == EventChangeKind::Update && change.account == account && change.remote == remote;
    let base = match mailbox.event_changes.iter().position(waiting) {
        Some(at) => mailbox.event_changes.remove(at).base,
        None => {
            room_for_change(mailbox)?;
            None
        }
    };
    let base = base.unwrap_or_else(|| Box::new(before.clone()));
    mailbox.event_changes.retain(|change| !(change.account == account && change.remote == remote));
    mailbox.event_changes.push(EventChange {
        account,
        kind: EventChangeKind::Delete,
        event: before.id,
        remote,
        base: Some(base),
        ..Default::default()
    });
    Ok(())
}

/// Why a noted change no longer hangs together with the mailbox, when it doesn't: only one that
/// does is sent. (A file from elsewhere could otherwise have the server told anything.) One that
/// doesn't is given up, with a word to the owner ([`tidy_changes`]).
fn unfit(mailbox: &Mailbox, change: &EventChange, here: &HashSet<(Id, &str)>) -> Option<&'static str> {
    if mailbox.account(change.account).is_none() {
        return Some(NO_ACCOUNT);
    }
    let of_account = |event: &&Event| event.origin.account == change.account;
    // Mail goes to nobody the owner didn't say yes to.
    if !permitted(mailbox, change) {
        return Some(NOT_SAID);
    }
    let named = |name: &str| !name.is_empty() && clean_key(name) == name;
    if change.of_occurrence() {
        // About one occurrence of a series the mailbox has: the event the occurrence is now, for
        // a change; nothing but the series, for a deletion.
        if change.occurrence == 0 || clean_key(&change.series) != change.series {
            return Some(NO_NAME);
        }
        let series = || mailbox.events.values().any(|event| event.origin.account == change.account && event.origin.id == change.series);
        return match change.kind {
            EventChangeKind::Update => match mailbox.event(change.event).filter(of_account) {
                None => Some(NO_EVENT),
                Some(event) if change.base.is_some() && event.origin.id.is_empty() && event.origin.series == change.series => None,
                Some(_) => Some(NOT_ITS_OWN),
            },
            // (One day of a meeting, sent on to people with addresses, under a mark of its own.)
            EventChangeKind::Forward if change.to.is_empty() || change.token.is_empty() => Some(NO_NAME),
            EventChangeKind::Delete | EventChangeKind::Respond | EventChangeKind::Forward if series() => None,
            EventChangeKind::Delete | EventChangeKind::Respond | EventChangeKind::Forward => Some(NO_SERIES),
            EventChangeKind::Create => Some(NO_NAME),
        };
    }
    match change.kind {
        EventChangeKind::Create if change.token.is_empty() => Some(NO_NAME),
        EventChangeKind::Create => match mailbox.event(change.event).filter(of_account) {
            None => Some(NO_EVENT),
            Some(event) if event.origin.id.is_empty() => None,
            Some(_) => Some(NOT_ITS_OWN),
        },
        EventChangeKind::Update if !named(&change.remote) || change.base.is_none() => Some(NO_NAME),
        EventChangeKind::Update => match mailbox.event(change.event).filter(of_account) {
            None => Some(NO_EVENT),
            Some(event) if event.origin.id == change.remote => None,
            Some(_) => Some(NOT_ITS_OWN),
        },
        EventChangeKind::Delete if !named(&change.remote) => Some(NO_NAME),
        EventChangeKind::Delete => {
            // The event it deletes is one that was here, and isn't any more.
            let was_here = change.base.as_deref().is_none_or(|base| base.origin.id == change.remote && base.origin.account == change.account);
            match (was_here, here.contains(&(change.account, change.remote.as_str()))) {
                (false, _) => Some(NOT_ITS_OWN),
                (true, true) => Some(STILL_HERE),
                (true, false) => None,
            }
        }
        EventChangeKind::Respond if !named(&change.remote) => Some(NO_NAME),
        EventChangeKind::Respond => {
            // An answer to somebody else's meeting: one that was declined is gone here, as one
            // that was deleted is; any other is here.
            match (change.response, here.contains(&(change.account, change.remote.as_str()))) {
                (Response::None, _) => Some(NO_NAME),
                (Response::Declined, true) => Some(STILL_HERE),
                (Response::Declined, false) => None,
                (_, true) => None,
                (_, false) => Some(NO_EVENT),
            }
        }
        // A meeting that is here and that the server has, sent on to people with addresses.
        EventChangeKind::Forward if !named(&change.remote) || change.to.is_empty() || change.token.is_empty() => Some(NO_NAME),
        EventChangeKind::Forward if here.contains(&(change.account, change.remote.as_str())) => None,
        EventChangeKind::Forward => Some(NO_EVENT),
    }
}

/// Whether a noted change still hangs together with the mailbox ([`unfit`]).
fn holds(mailbox: &Mailbox, change: &EventChange, here: &HashSet<(Id, &str)>) -> bool {
    unfit(mailbox, change, here).is_none()
}

/// The calendar here that a noted change is about: where its event is, or was.
fn change_calendar(mailbox: &Mailbox, change: &EventChange) -> Option<Id> {
    let now = mailbox.event(change.event).filter(|event| event.origin.account == change.account || change.kind == EventChangeKind::Create);
    now.map(|event| event.calendar).or_else(|| change.base.as_deref().map(|base| base.calendar))
}

/// Whether a noted change is held back for want of rights (#70): the calendar it is about is one
/// that a server keeps and this account may no longer change (its rights were read again and
/// shrank). It is not sent and not dropped: it waits here, and the owner is told
/// ([`held_for_rights`]).
fn lacks_rights(mailbox: &Mailbox, change: &EventChange) -> bool {
    change_calendar(mailbox, change)
        .and_then(|calendar| mailbox.calendar(calendar))
        .is_some_and(|calendar| !calendar.can_edit() && keeper(mailbox, &calendar.origin).is_some())
}

/// The calendars of `account` that changes are held back for, because the account may no longer
/// change them (#70), with how many each: for the window to tell the owner, by the calendar's name.
pub fn held_for_rights(mailbox: &Mailbox, account: Id) -> Vec<(Id, String, usize)> {
    let mut held: Vec<(Id, String, usize)> = Vec::new();
    for change in mailbox.event_changes.iter().filter(|change| change.account == account && lacks_rights(mailbox, change)) {
        let Some(calendar) = change_calendar(mailbox, change).and_then(|id| mailbox.calendar(id)) else { continue };
        match held.iter_mut().find(|(id, ..)| *id == calendar.id) {
            Some((.., n)) => *n += 1,
            None => held.push((calendar.id, calendar.name.clone(), 1)),
        }
    }
    held
}

/// Whether a noted change may go to its server as far as mail is concerned: one that makes the
/// server write to other people (invitations, updates, a cancellation) only with the owner's
/// yes ([`EventChange::send`]). Whatever a mailbox file says, nobody is written to without it.
fn permitted(mailbox: &Mailbox, change: &EventChange) -> bool {
    if change.send {
        return true;
    }
    let meeting = |event: &Event| !event.origin.guest && !event.attendees.is_empty();
    let now = mailbox.event(change.event).filter(|event| event.origin.account == change.account);
    match change.kind {
        EventChangeKind::Create => !now.is_some_and(meeting),
        // How it shows in the owner's own calendar is nobody else's business; the rest is.
        EventChangeKind::Update => match (change.base.as_deref(), now) {
            (Some(base), Some(now)) if meeting(base) || meeting(now) => !seen_by_others(&changed_fields(base, now, &|_| 0)),
            _ => true,
        },
        // A deletion without a word of who was invited (a file from elsewhere) says nothing
        // either way: it is the owner's own copy that goes. One of an occurrence is asked about
        // through its series.
        EventChangeKind::Delete if change.of_occurrence() => {
            !mailbox.events.values().any(|event| event.origin.account == change.account && event.origin.id == change.series && meeting(event))
        }
        EventChangeKind::Delete => !change.base.as_deref().is_some_and(meeting),
        // An answer tells the organizer only when it says so, which is `send` itself. One that
        // proposes another time is mail to the organizer whatever else it says.
        EventChangeKind::Respond => change.proposed.is_none(),
        // Forwarding is nothing but mail.
        EventChangeKind::Forward => false,
    }
}

/// What the mailbox has of each server's events, by account and by what the server calls them:
/// for [`holds`]. Nothing is listed when no deletion waits (nothing would ask).
fn events_here(mailbox: &Mailbox) -> HashSet<(Id, &str)> {
    let asks = |change: &EventChange| matches!(change.kind, EventChangeKind::Delete | EventChangeKind::Respond | EventChangeKind::Forward);
    if !mailbox.event_changes.iter().any(asks) {
        return HashSet::new();
    }
    mailbox.events.values().filter(|event| !event.origin.id.is_empty()).map(|event| (event.origin.account, event.origin.id.as_str())).collect()
}

/// Give up the noted changes that no longer hang together with the mailbox (their event is gone,
/// or is the server's under another name), and all but the first about any one event. None of
/// them goes without a word: what each changed here is put back to what the server has, and
/// what became of it is said ([`Dropped`]), for the owner and for the log. A note that says
/// nothing any more (the event is as the server has it again) just goes: there was nothing to send.
fn tidy_changes(mailbox: &mut Mailbox) -> Vec<Dropped> {
    if mailbox.event_changes.is_empty() {
        return Vec::new();
    }
    /// A new event is known by its id here, anything else by what the server calls it; one
    /// occurrence of a series by the series and its time.
    fn about(change: &EventChange) -> (Id, &str, Id, &str, Timestamp) {
        match change.kind {
            EventChangeKind::Create => (change.account, "", change.event, "", 0),
            // (Forwarding changes nothing: it is beside whatever else waits for its event, and
            // one forward beside another, each known by its own mark.)
            EventChangeKind::Forward => (change.account, change.token.as_str(), Id::MAX, "", 0),
            _ if change.of_occurrence() => (change.account, "", 0, change.series.as_str(), change.occurrence),
            _ => (change.account, change.remote.as_str(), 0, "", 0),
        }
    }
    let mut kept: Vec<EventChange> = Vec::new();
    let mut given_up: Vec<(EventChange, &'static str)> = Vec::new();
    let mut said_nothing = 0usize;
    {
        let here = events_here(mailbox);
        let mut seen: HashSet<(Id, &str, Id, &str, Timestamp)> = HashSet::new();
        for change in &mailbox.event_changes {
            let why = unfit(mailbox, change, &here).or_else(|| (!seen.insert(about(change))).then_some(SAID_TWICE));
            match why {
                Some(reason) => given_up.push((change.clone(), reason)),
                None if says_nothing(mailbox, change) => said_nothing += 1,
                None if kept.len() >= MAX_EVENT_CHANGES => given_up.push((change.clone(), TOO_MANY)),
                None => kept.push(change.clone()),
            }
        }
    }
    if kept.len() == mailbox.event_changes.len() {
        return Vec::new();
    }
    if said_nothing > 0 {
        log::debug!("calendar: {said_nothing} waiting changes had nothing left to send");
    }
    mailbox.event_changes = kept;
    given_up.iter().take(MAX_OUTCOMES).map(|(change, reason)| give_up(mailbox, change, reason)).collect()
}

/// Whether a noted change has nothing to tell the server any more: the event is, in everything
/// a server is told, as the server has it (it was changed back some other way than by a
/// command, which would have taken the note away).
fn says_nothing(mailbox: &Mailbox, change: &EventChange) -> bool {
    if change.kind != EventChangeKind::Update {
        return false;
    }
    match (change.base.as_deref(), mailbox.event(change.event)) {
        (Some(base), Some(event)) => changed_fields(base, event, &|_| 0).is_empty(),
        _ => false,
    }
}

/// Take an event that can't be on a server out of the server's calendar and keep it in one of
/// the mailbox's own (made when there is none): what the owner wrote is never thrown away.
/// Whether it is kept so.
fn keep_on_this_computer(mailbox: &mut Mailbox, id: Id) -> bool {
    if mailbox.event(id).is_none() {
        return false;
    }
    let own = mailbox.calendars.iter().find(|calendar| keeper(mailbox, &calendar.origin).is_none()).map(|calendar| calendar.id);
    let own = match own {
        Some(own) => own,
        None if mailbox.calendars.len() < MAX_CALENDARS => {
            let (calendar, color) = (mailbox.alloc_id(), super::calendar::next_color(mailbox));
            mailbox.calendars.push(Calendar { id: calendar, name: LOCAL_CALENDAR.into(), color, visible: true, ..Default::default() });
            calendar
        }
        None => return false,
    };
    match mailbox.event_mut(id) {
        Ok(event) => {
            event.calendar = own;
            event.origin = Origin::default();
            true
        }
        Err(_) => false,
    }
}

/// Give a waiting change up for `reason`: what it changed here is put back to what the server
/// has, where that can be told, and what became of it is said, for the owner (who is named the
/// event) and for the log (which is not).
fn give_up(mailbox: &mut Mailbox, change: &EventChange, reason: &'static str) -> Dropped {
    let here = mailbox.event(change.event).filter(|event| event.origin.account == change.account).cloned();
    let name = called(here.as_ref().or(change.base.as_deref()));
    let unasked = "it would have made the server write to the people invited, and nobody said to send that";
    let message = match (reason, change.kind) {
        // (The first of them still goes; an account that was removed took its calendar along.)
        (SAID_TWICE | NO_ACCOUNT, _) => String::new(),
        (NOT_SAID, EventChangeKind::Update) if change.of_occurrence() => {
            // The day is the series' own again.
            if let Some(one) = &here {
                let _ = mailbox.remove_event(one.id);
                let series = mailbox.events.values().find(|event| event.origin.account == change.account && event.origin.id == change.series);
                if let (Some(series), Some(day)) = (series.map(|series| series.id), one.origin.occurrence)
                    && let Ok(series) = mailbox.event_mut(series)
                {
                    series.exceptions.retain(|skipped| *skipped != day);
                }
            }
            format!("The change to one day of {name} was not sent: {unasked}. It was put back to what the server has.")
        }
        (NOT_SAID, EventChangeKind::Update) => {
            // As the server has it.
            if let (Some(base), Some(event)) = (change.base.as_deref(), here.as_ref())
                && base.origin.id == event.origin.id
                && let Ok(event) = mailbox.event_mut(event.id)
            {
                let (id, calendar) = (event.id, event.calendar);
                *event = Event { id, calendar, ..base.clone() };
            }
            format!("The change to {name} was not sent: {unasked}. It was put back to what the server has.")
        }
        (NOT_SAID, EventChangeKind::Create) => {
            if keep_on_this_computer(mailbox, change.event) {
                format!("{name} was not sent to the server: {unasked}. It is kept on this computer only.")
            } else {
                format!("{name} was not sent to the server: {unasked}.")
            }
        }
        (NOT_SAID, EventChangeKind::Delete) => {
            format!("Deleting {name} was not sent to the server: {unasked}. It is still there, and comes back into the calendar.")
        }
        // Mail and nothing else: nothing was changed here, so there is nothing to put back.
        (NOT_SAID, EventChangeKind::Forward) => {
            format!("{name} was not forwarded: that writes to other people, and nobody said to send it. Nobody was written to.")
        }
        (NOT_SAID, EventChangeKind::Respond) => format!(
            "The other time proposed for {name} was not sent: that writes to its organizer, and nobody said to send it. The next Send/Receive shows the meeting as the server has it."
        ),
        (NO_EVENT | NO_SERIES, EventChangeKind::Forward) => format!("{name} was not forwarded: it is no longer in the calendar."),
        (NO_EVENT | NO_SERIES, _) => format!("A change to {name} was not sent: it is no longer in the calendar."),
        _ => "A calendar change that was waiting to be sent no longer matched the calendar. It was not sent.".to_string(),
    };
    log::warn!("calendar: a waiting change was given up without being sent ({:?}): {reason}", change.kind);
    Dropped { account: change.account, kind: change.kind, reason, message }
}

/// Give up the waiting changes that can't be sent any more ([`tidy_changes`]), outside of an
/// answer or a download: before a host is told what to send. What was given up, for the host to
/// tell the owner and to log.
pub fn settle_changes(s: &mut Session) -> Vec<Dropped> {
    let before = s.mailbox.event_changes.len();
    let dropped = tidy_changes(&mut s.mailbox);
    if s.mailbox.event_changes.len() != before || !dropped.is_empty() {
        s.mailbox_changed();
    }
    dropped
}

/// How much of an account's calendar waits to go to its server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventsWaiting {
    /// Changes that can be sent now.
    pub due: usize,
    /// All changes that wait, those that are being held back after a failure included.
    pub total: usize,
    /// When the next of those held back is due (Unix seconds).
    pub next: Option<Timestamp>,
}

/// How much of `account`'s calendar waits to go to its server ([`EventsWaiting`]).
pub fn events_waiting(s: &Session, account: Id) -> EventsWaiting {
    let mut waiting = EventsWaiting::default();
    if !s.mailbox.event_changes.iter().any(|change| change.account == account) {
        return waiting;
    }
    let (now, here) = (s.now(), events_here(&s.mailbox));
    for change in s
        .mailbox
        .event_changes
        .iter()
        .filter(|change| change.account == account && holds(&s.mailbox, change, &here) && !lacks_rights(&s.mailbox, change))
    {
        waiting.total += 1;
        // (The end of a move waits for the copy to be seen on the server, for a while.)
        let held = waits_for_copy(&s.mailbox, change, now).filter(|until| *until > change.retry_at);
        if let Some(until) = held {
            waiting.next = Some(waiting.next.map_or(until, |next| next.min(until)));
        } else if change.retry_at <= now {
            waiting.due += 1;
        } else {
            waiting.next = Some(waiting.next.map_or(change.retry_at, |next| next.min(change.retry_at)));
        }
    }
    waiting
}

/// What the server of `account` is to be told now of the events made, changed and deleted here:
/// the changes that are due, oldest first, [`MAX_EVENT_OPS`] at most. Times go as instants, in
/// the session's zone ([`server_time`]).
pub fn event_upload(s: &Session, account: Id) -> Result<EventsUpload, CmdError> {
    let owner = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    let (offset_at, now) = (offset_of(s), s.now());
    let mailbox = &s.mailbox;
    let here = events_here(mailbox);
    let mut ops = Vec::new();
    for change in mailbox
        .event_changes
        .iter()
        .filter(|change| change.account == account && change.retry_at <= now && holds(mailbox, change, &here) && !lacks_rights(mailbox, change))
    {
        if ops.len() >= MAX_EVENT_OPS {
            break;
        }
        // The original of a moved appointment stays until its copy is seen on the server.
        if waits_for_copy(mailbox, change, now).is_some() {
            continue;
        }
        let event = mailbox.event(change.event);
        let fields = match (change.kind, event, change.base.as_deref()) {
            (EventChangeKind::Create, Some(event), _) => fields_of(event, &offset_at),
            (EventChangeKind::Update, Some(event), Some(base)) => changed_fields(base, event, &offset_at),
            (EventChangeKind::Delete | EventChangeKind::Respond | EventChangeKind::Forward, _, _) => EventFields::default(),
            _ => continue,
        };
        // Changed and changed back: nothing to say (the note goes when the next answer is taken in).
        if change.kind == EventChangeKind::Update && fields.is_empty() {
            continue;
        }
        // The server's calendar the event is in (a new one goes into the one that its calendar
        // here follows): an event of another calendar than the main one is asked for through
        // that calendar. One that is gone here says where it was.
        let home = |calendar: Id| mailbox.calendar(calendar).filter(|c| c.origin.account == account).map(|c| c.origin.id.clone()).unwrap_or_default();
        // (What is deleted is where it was: the end of a move to another calendar has the copy
        // here under the id the original had, in the other calendar.)
        let calendar = match (change.kind, event, change.base.as_deref()) {
            (EventChangeKind::Delete, _, Some(base)) => home(base.calendar),
            (_, Some(event), _) => home(event.calendar),
            (_, None, Some(base)) => home(base.calendar),
            (_, None, None) => String::new(),
        };
        // A meeting the owner called is called off, which tells the people invited; the owner
        // said yes to that, or the change would not be here ([`permitted`]).
        let invited = |event: &Event| !event.origin.guest && !event.attendees.is_empty();
        let series = || mailbox.events.values().find(|event| event.origin.account == account && event.origin.id == change.series);
        let cancel = change.kind == EventChangeKind::Delete
            && change.send
            && if change.of_occurrence() { series().is_some_and(|series| invited(series)) } else { change.base.as_deref().is_some_and(invited) };
        let forwards = change.kind == EventChangeKind::Forward;
        let said = cancel || change.kind == EventChangeKind::Respond || forwards;
        // Another time is proposed with an answer that is sent, and only with one that isn't a yes.
        let proposes = change.kind == EventChangeKind::Respond && change.send && matches!(change.response, Response::Tentative | Response::Declined);
        ops.push(EventOp {
            kind: change.kind,
            event: change.event,
            remote: change.remote.clone(),
            token: change.token.clone(),
            fields,
            series: if change.of_occurrence() { change.series.clone() } else { String::new() },
            occurrence: if change.of_occurrence() { change.occurrence } else { 0 },
            calendar,
            cancel,
            response: if change.kind == EventChangeKind::Respond { change.response } else { Response::None },
            send: (change.kind == EventChangeKind::Respond || forwards) && change.send,
            comment: if said { change.comment.clone() } else { String::new() },
            proposed: change.proposed.filter(|_| proposes),
            to: if forwards && change.send { change.to.clone() } else { Vec::new() },
        });
    }
    // (The zone the calendar is shown in: whole days are days there, and a series without a
    // zone of its own is told by its clock.)
    Ok(EventsUpload { ops, zone: s.zone().id(), ..Default::default() }.for_account(owner))
}

/// The sign-in of `account` changed (the owner gave a permission, say): what was held back
/// after a failure is tried again at once.
pub fn retry_event_changes(s: &mut Session, account: Id) {
    let held = |change: &&mut EventChange| change.account == account && change.retry_at != 0;
    let mut any = false;
    for change in s.mailbox.event_changes.iter_mut().filter(held) {
        change.retry_at = 0;
        any = true;
    }
    if any {
        s.mailbox_changed();
    }
}

/// How long a change waits after failing `failures` times in a row (seconds).
fn wait_after(failures: u32, asked: Option<u32>) -> i64 {
    let own = FIRST_WAIT.saturating_mul(1i64 << failures.saturating_sub(1).min(10)).min(LONGEST_WAIT);
    own.max(i64::from(asked.unwrap_or(0)).min(86_400))
}

/// A title for a line about an event: short, and never empty.
fn called(event: Option<&Event>) -> String {
    let title: String = event.map_or("", |event| event.title.trim()).chars().filter(|c| !c.is_control()).take(60).collect();
    if title.is_empty() { "an event".into() } else { format!("\"{title}\"") }
}

/// Why the server said no, for the end of a sentence.
fn because(error: &str) -> String {
    let error = error.trim();
    if error.is_empty() { String::new() } else { format!(" ({error})") }
}

/// What taking in the answers to an [`EventsUpload`] came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventsApplied {
    /// Changes the server took on (or that needed no more telling).
    pub done: usize,
    /// Changes the server refused for good: put back as they were, or kept here only.
    pub refused: usize,
    /// Changes that couldn't be sent and wait to be tried again.
    pub failed: usize,
    /// The account may not change its calendar on the server: its owner is to be asked.
    pub denied: bool,
    /// What still waits, after this.
    pub waiting: usize,
    /// Changes the server said it took that no download has shown yet ([`EventCheck`]).
    pub unverified: usize,
    /// What the owner should hear, a line each.
    pub messages: Vec<String>,
    /// Changes that were given up without the server taking them: one that could not be sent
    /// any more, or one about an event the server no longer has ([`Dropped`]).
    pub dropped: Vec<Dropped>,
}

impl EventsApplied {
    fn say(&mut self, line: String) {
        if self.messages.len() < MAX_MESSAGES && !line.is_empty() && !self.messages.contains(&line) {
            self.messages.push(line);
        }
    }

    fn json(&self) -> Value {
        json!({
            "done": self.done,
            "refused": self.refused,
            "failed": self.failed,
            "denied": self.denied,
            "waiting": self.waiting,
            "unverified": self.unverified,
            "messages": self.messages,
            "dropped": dropped_json(&self.dropped),
        })
    }
}

/// What of the fields that were sent a download can show again just as they were sent: when it
/// is, what it is called, how it shows and whether it is private. (The rest a server may word
/// its own way: a description comes back as the server laid it out, a place as it knows it.)
fn seen_again(fields: &EventFields) -> EventFields {
    EventFields { title: fields.title.clone(), time: fields.time, show_as: fields.show_as, private: fields.private, ..Default::default() }
}

/// The server said it took a change: note that the downloads that follow are to show it
/// ([`check_sent`]). What was sent of the same event before, and is yet to be seen, is still to
/// be seen beside this.
fn expect(mailbox: &mut Mailbox, mut check: EventCheck) {
    let same = |other: &EventCheck| {
        other.account == check.account
            && other.of_occurrence() == check.of_occurrence()
            && if check.of_occurrence() { other.series == check.series && other.occurrence == check.occurrence } else { other.remote == check.remote }
    };
    if let Some(at) = mailbox.event_checks.iter().position(same) {
        let earlier = mailbox.event_checks.remove(at);
        if earlier.kind != EventChangeKind::Delete && check.kind != EventChangeKind::Delete {
            let now = std::mem::take(&mut check.fields);
            check.fields = EventFields {
                title: now.title.or(earlier.fields.title),
                time: now.time.or(earlier.fields.time),
                show_as: now.show_as.or(earlier.fields.show_as),
                private: now.private.or(earlier.fields.private),
                ..Default::default()
            };
            // One that was made and not seen yet is still one that was made here.
            check.kind = earlier.kind;
        }
    }
    if mailbox.event_checks.len() >= MAX_EVENT_CHECKS {
        mailbox.event_checks.remove(0);
    }
    mailbox.event_checks.push(check);
}

/// How many changes of `account`'s calendar the server said it took that no download has shown
/// yet.
pub fn events_unverified(s: &Session, account: Id) -> usize {
    s.mailbox.event_checks.iter().filter(|check| check.account == account).count()
}

/// The calendars here that hold something of `account`'s which was sent and is yet to be seen on
/// the server ([`EventCheck`]): for a host to ask for them too while that is so, shown or not.
/// (An appointment that was put into a calendar which isn't ticked could otherwise not be seen
/// there until the next Send/Receive.)
pub fn unverified_calendars(s: &Session, account: Id) -> Vec<Id> {
    let mailbox = &s.mailbox;
    if !mailbox.event_checks.iter().any(|check| check.account == account) {
        return Vec::new();
    }
    let here: HashMap<&str, Id> = mailbox
        .events
        .values()
        .filter(|event| event.origin.account == account && !event.origin.id.is_empty())
        .map(|event| (event.origin.id.as_str(), event.calendar))
        .collect();
    let mut calendars: Vec<Id> = Vec::new();
    for check in mailbox.event_checks.iter().filter(|check| check.account == account).take(MAX_EVENT_CHECKS) {
        let name = if check.of_occurrence() { check.series.as_str() } else { check.remote.as_str() };
        // One that was deleted is looked for where it was.
        let calendar = match (check.kind, check.base.as_deref()) {
            (EventChangeKind::Delete, Some(base)) => Some(base.calendar),
            _ => here.get(name).copied(),
        };
        if let Some(calendar) = calendar.filter(|calendar| !calendars.contains(calendar)) {
            calendars.push(calendar);
        }
    }
    calendars
}

/// How long after a change was sent a host asks again soon for the sake of seeing it there
/// (seconds): after that the ordinary look at the calendar will do.
const RECHECK_FOR: i64 = 120;

/// Whether a change of `account`'s calendar was sent just now and is yet to be seen on the
/// server: for a host to ask for the calendar again soon, rather than when it is next due.
pub fn recheck_due(s: &Session, account: Id) -> bool {
    let now = s.now();
    s.mailbox.event_checks.iter().any(|check| check.account == account && now.saturating_sub(check.made) <= RECHECK_FOR)
}

/// Take in what a server said to an [`EventsUpload`] ([`EventsUploaded`]).
///
/// - Done: a new event gets the server's name for it and its change key, a changed one its new
///   change key, and the note is dropped. When the event was changed again while the change was
///   on its way, what still differs stays noted against what the server has now.
/// - Refused for good: a change is put back as the server has it, a deleted event comes back, a
///   new event is kept in a calendar of the mailbox's own; each says so.
/// - Denied: the account lacks the permission. Kept once, for the owner to give it; refused the
///   second time.
/// - Failed: kept, and held back for longer after every failure in a row.
/// - An answer about a change that isn't noted any more (it was undone while it was on its way):
///   the server did what the mailbox no longer says, so the opposite is noted for it.
///
/// Like mail that arrives, this is not an undo step; and once the server was told anything the
/// undo history is forgotten, as it is for mail.
pub fn apply_event_outcomes(s: &mut Session, uploaded: &EventsUploaded) -> Result<EventsApplied, CmdError> {
    let account = owner(&s.mailbox, uploaded.account, &uploaded.email)?.id;
    let (offset_at, now) = (offset_of(s), s.now());
    let before = s.mailbox.event_changes.clone();
    let checks_before = s.mailbox.event_checks.clone();
    let mut applied = EventsApplied::default();
    let mut told = false;
    for outcome in uploaded.outcomes.iter().take(MAX_OUTCOMES) {
        let mut outcome = outcome.clone();
        outcome.tidy();
        let mailbox = &mut s.mailbox;
        match outcome.op.kind {
            EventChangeKind::Create => created(mailbox, account, &outcome, &offset_at, now, &mut applied, &mut told),
            EventChangeKind::Forward => forwarded(mailbox, account, &outcome, now, &mut applied, &mut told),
            _ if outcome.op.of_occurrence() => one_occurrence(mailbox, account, &outcome, &offset_at, now, &mut applied, &mut told),
            EventChangeKind::Update => updated(mailbox, account, &outcome, &offset_at, now, &mut applied, &mut told),
            EventChangeKind::Delete => deleted(mailbox, account, &outcome, now, &mut applied, &mut told),
            EventChangeKind::Respond => responded(mailbox, account, &outcome, now, &mut applied, &mut told),
        }
    }
    for dropped in tidy_changes(&mut s.mailbox) {
        applied.say(dropped.message.clone());
        applied.dropped.push(dropped);
    }
    applied.waiting = s.mailbox.event_changes.iter().filter(|change| change.account == account).count();
    applied.unverified = events_unverified(s, account);
    if told {
        s.forget_history();
    }
    if told || applied.refused > 0 || !applied.dropped.is_empty() || s.mailbox.event_changes != before || s.mailbox.event_checks != checks_before {
        s.mailbox_changed();
    }
    Ok(applied)
}

/// Note that a change couldn't be sent, or may not be: it waits, longer after each failure.
/// `true` when it has been denied before and is now to be given up.
fn held_back(change: &mut EventChange, outcome: &EventOutcome, now: Timestamp) -> bool {
    if outcome.result == EventResult::Denied {
        if change.denied {
            return true;
        }
        change.denied = true;
    }
    change.failures = change.failures.saturating_add(1);
    change.retry_at = now.saturating_add(wait_after(change.failures, outcome.retry_after));
    change.error = outcome.error.clone();
    false
}

/// The answer to making an event.
fn created(
    mailbox: &mut Mailbox,
    account: Id,
    outcome: &EventOutcome,
    offset_at: &dyn Fn(Timestamp) -> i32,
    now: Timestamp,
    applied: &mut EventsApplied,
    told: &mut bool,
) {
    let op = &outcome.op;
    let noted = |change: &EventChange| {
        change.kind == EventChangeKind::Create && change.account == account && !op.token.is_empty() && change.token == op.token
    };
    let at = mailbox.event_changes.iter().position(noted);
    let event = at.and_then(|at| mailbox.event_changes.get(at)).map(|change| change.event).filter(|id| mailbox.event(*id).is_some());
    match outcome.result {
        EventResult::Done if outcome.remote.is_empty() => {
            // Made, but under what name wasn't said: the next download says (it carries the mark).
            if let Some(change) = at.and_then(|at| mailbox.event_changes.get_mut(at)) {
                change.failures = change.failures.saturating_add(1);
                change.retry_at = now.saturating_add(wait_after(change.failures, None));
            }
            applied.failed += 1;
        }
        EventResult::Done => {
            *told = true;
            applied.done += 1;
            let (Some(at), Some(id)) = (at, event) else {
                // Undone, or deleted again, while it was on its way: the server is to delete it.
                // Not a meeting, though: the invitations are out, and deleting it would send
                // everyone a cancellation nobody was asked about. The next download brings it
                // back, for the owner to call off (or keep).
                if op.fields.attendees.as_ref().is_some_and(|people| !people.is_empty()) {
                    let title: String = op.fields.title.as_deref().unwrap_or("").chars().filter(|c| !c.is_control()).take(60).collect();
                    applied.say(format!(
                        "The invitations to \"{title}\" were already sent when it was taken back here. It is still on the server and comes back into the calendar: delete it there to call it off."
                    ));
                    return;
                }
                let orphan = |change: &EventChange| change.account == account && change.remote == outcome.remote;
                if !mailbox.event_changes.iter().any(orphan)
                    && !mailbox.events.values().any(|e| e.origin.account == account && e.origin.id == outcome.remote)
                {
                    // (With what was made, as far as its answer says: the deletion then goes
                    // through the calendar it was made in, and a download can tell whether it
                    // is gone.)
                    let home = mailbox.calendars.iter().find(|calendar| calendar.origin.account == account && calendar.origin.id == op.calendar);
                    let base = home.map(|calendar| calendar.id).map(|calendar| {
                        let origin = Origin { account, id: outcome.remote.clone(), change_key: outcome.change_key.clone(), ..Default::default() };
                        let mut made = Event { calendar, origin, ..Default::default() };
                        put_fields(&mut made, &op.fields, offset_at);
                        Box::new(made)
                    });
                    mailbox.event_changes.push(EventChange {
                        account,
                        kind: EventChangeKind::Delete,
                        remote: outcome.remote.clone(),
                        base,
                        ..Default::default()
                    });
                }
                return;
            };
            let Ok(event) = mailbox.event_mut(id) else { return };
            event.origin.id = outcome.remote.clone();
            event.origin.change_key = outcome.change_key.clone();
            if event.uid.is_empty() {
                event.uid = outcome.uid.clone();
            }
            settle_online(event, outcome, applied);
            // What the server has is what was sent; what was changed here since is still to go.
            let mut sent = event.clone();
            put_fields(&mut sent, &op.fields, offset_at);
            let more = !changed_fields(&sent, event, offset_at).is_empty();
            let (remote, id) = (event.origin.id.clone(), event.id);
            let made = mailbox.event_changes.remove(at);
            let send = made.send;
            // The server said so: the downloads that follow are to show it.
            expect(
                mailbox,
                EventCheck {
                    account,
                    kind: EventChangeKind::Create,
                    event: id,
                    remote: remote.clone(),
                    fields: seen_again(&op.fields),
                    change_key: outcome.change_key.clone(),
                    modified: outcome.modified,
                    made: now,
                    ..Default::default()
                },
            );
            // An appointment that was moved here from another calendar: the server says it has
            // the copy now, and the original is to go (once the copy is seen there:
            // [`waits_for_copy`]).
            finish_move(mailbox, account, id, made, &remote);
            // A series made here with days left out: the server has it whole so far.
            skip_days_there(mailbox, account, id, offset_at);
            if more {
                // (With the owner's yes to the mail it makes, when that was given: a change to a
                // meeting that isn't on the server yet is asked about like any other.)
                mailbox.event_changes.push(EventChange {
                    account,
                    kind: EventChangeKind::Update,
                    event: id,
                    remote,
                    base: Some(Box::new(sent)),
                    send,
                    ..Default::default()
                });
            }
        }
        EventResult::Failed => {
            if let Some(change) = at.and_then(|at| mailbox.event_changes.get_mut(at)) {
                held_back(change, outcome, now);
                applied.failed += 1;
            }
        }
        EventResult::Denied | EventResult::Refused | EventResult::Gone => {
            let Some(at) = at else { return };
            let give_up = match mailbox.event_changes.get_mut(at) {
                Some(change) if outcome.result == EventResult::Denied => held_back(change, outcome, now),
                _ => true,
            };
            if !give_up {
                applied.denied = true;
                applied.failed += 1;
                return;
            }
            let noted = mailbox.event_changes.remove(at);
            applied.refused += 1;
            // An appointment that was being moved from another calendar is still in that one on
            // the server: it is put back there.
            if move_refused(mailbox, noted, outcome, applied) {
                return;
            }
            // What was written is not thrown away: it stays, in a calendar of the mailbox's own.
            let name = called(event.and_then(|id| mailbox.event(id)));
            if event.is_some_and(|id| keep_on_this_computer(mailbox, id)) {
                applied.say(format!("The server wouldn't take {name}{}. It is kept on this device only.", because(&outcome.error)));
            } else {
                applied.say(format!("The server wouldn't take {name}{}.", because(&outcome.error)));
            }
        }
    }
}

/// Most days of a new series whose skipping the server is told, one by one.
const MAX_SKIPS_TOLD: usize = 200;

/// A series of the owner's own is on the server now, and skips days here: the server is to
/// delete each of those occurrences. (Never for a meeting: every one of them would be a
/// cancellation in somebody's mail. The commands don't put such a series on a server.)
fn skip_days_there(mailbox: &mut Mailbox, account: Id, id: Id, offset_at: &dyn Fn(Timestamp) -> i32) {
    let Some(series) = mailbox.event(id).cloned() else { return };
    if series.recurrence.is_none() || series.origin.id.is_empty() || !series.attendees.is_empty() {
        return;
    }
    for day in series.exceptions.iter().take(MAX_SKIPS_TOLD) {
        if mailbox.event_changes.len() >= MAX_EVENT_CHANGES {
            break;
        }
        let occurrence = occurrence_instant(&series, *day, offset_at);
        let noted = |change: &EventChange| {
            change.account == account && change.of_occurrence() && change.series == series.origin.id && change.occurrence == occurrence
        };
        if !mailbox.event_changes.iter().any(noted) {
            mailbox.event_changes.push(EventChange {
                account,
                kind: EventChangeKind::Delete,
                event: id,
                series: series.origin.id.clone(),
                occurrence,
                ..Default::default()
            });
        }
    }
}

/// The owner said yes to the mail that what was just done to an event makes its server send
/// (`event` is the event here, `remote` what the server calls it), with a note to go along.
pub(crate) fn approve_mail(mailbox: &mut Mailbox, account: Id, event: Id, remote: &str, comment: &str) {
    let about = |change: &EventChange| match change.kind {
        EventChangeKind::Create => change.event == event,
        _ => !remote.is_empty() && change.remote == remote,
    };
    for change in mailbox.event_changes.iter_mut().filter(|change| change.account == account && about(change)) {
        change.send = true;
        change.comment = comment.to_string();
    }
}

/// One occurrence of a server's series (`series`, on `day`) was changed here on its own: it is
/// the event `id` now, and `was` is what the series had on that day. The server is to be told
/// what differs, of the occurrence it has at that time. `send`: the owner's yes to the mail
/// that makes, for a meeting.
pub(crate) fn note_occurrence_changed(s: &mut Session, series: &Event, day: Date, id: Id, was: Event, send: bool) -> Result<(), CmdError> {
    let Some(account) = keeper(&s.mailbox, &series.origin).map(|account| account.id) else { return Ok(()) };
    room_for_change(&s.mailbox)?;
    let occurrence = occurrence_instant(series, day, &offset_of(s));
    s.mailbox.event_changes.push(EventChange {
        account,
        kind: EventChangeKind::Update,
        event: id,
        base: Some(Box::new(was)),
        series: series.origin.id.clone(),
        occurrence,
        send,
        ..Default::default()
    });
    Ok(())
}

/// One occurrence of a server's series (`series`, on `day`) was deleted here, or declined
/// (`response`): the server is to delete or decline the occurrence it has at that time.
pub(crate) fn note_occurrence_gone(
    s: &mut Session,
    series: &Event,
    day: Date,
    response: Response,
    send: bool,
    comment: &str,
) -> Result<(), CmdError> {
    let Some(account) = keeper(&s.mailbox, &series.origin).map(|account| account.id) else { return Ok(()) };
    room_for_change(&s.mailbox)?;
    let occurrence = occurrence_instant(series, day, &offset_of(s));
    s.mailbox.event_changes.retain(|change| {
        !(change.account == account && change.of_occurrence() && change.series == series.origin.id && change.occurrence == occurrence)
    });
    s.mailbox.event_changes.push(EventChange {
        account,
        kind: if response == Response::None { EventChangeKind::Delete } else { EventChangeKind::Respond },
        event: series.id,
        series: series.origin.id.clone(),
        occurrence,
        send,
        comment: comment.to_string(),
        response,
        ..Default::default()
    });
    Ok(())
}

/// The owner answered somebody else's meeting that a server keeps (`before` is the event as it
/// was; it is changed, or gone, here already): the server is to be told, and tells the
/// organizer when `send`.
pub(crate) fn note_responded(mailbox: &mut Mailbox, before: &Event, response: Response, send: bool, comment: &str) -> Result<(), CmdError> {
    let Some(account) = keeper(mailbox, &before.origin).map(|account| account.id) else { return Ok(()) };
    if before.origin.id.is_empty() {
        return Ok(());
    }
    let remote = before.origin.id.clone();
    // What waited for this event is overtaken; what the server has is what it was before that.
    let waiting = |change: &EventChange| change.account == account && change.remote == remote && change.kind != EventChangeKind::Create;
    let base = match mailbox.event_changes.iter().position(waiting) {
        Some(at) => mailbox.event_changes.remove(at).base,
        None => {
            room_for_change(mailbox)?;
            None
        }
    };
    mailbox.event_changes.retain(|change| !waiting(change));
    mailbox.event_changes.push(EventChange {
        account,
        kind: EventChangeKind::Respond,
        event: before.id,
        remote,
        base: Some(base.unwrap_or_else(|| Box::new(before.clone()))),
        send,
        comment: comment.to_string(),
        response,
        ..Default::default()
    });
    Ok(())
}

/// The answer that was just noted for a meeting ([`note_responded`], [`note_occurrence_gone`])
/// proposes another time to its organizer. It is mail to the organizer, so it goes only with the
/// owner's yes: an answer that wasn't noted as one to send is left as it is.
pub(crate) fn propose_with_answer(mailbox: &mut Mailbox, proposed: mailcraft_model::remote::EventTime) {
    if let Some(change) = mailbox.event_changes.last_mut().filter(|change| change.kind == EventChangeKind::Respond && change.send) {
        change.proposed = Some(proposed);
    }
}

/// The owner sends a meeting that a server keeps on to more people (`to`), with a note. Nothing
/// changes here: the server writes to them, and only because the owner said so (`send` is what
/// this is). `occurrence`: one day of a series that is known by its series and its time only.
pub(crate) fn note_forward(
    s: &mut Session,
    event: &Event,
    occurrence: Option<Date>,
    to: Vec<mailcraft_model::Address>,
    comment: &str,
) -> Result<(), CmdError> {
    let Some(account) = keeper(&s.mailbox, &event.origin).map(|account| account.id) else { return Ok(()) };
    room_for_change(&s.mailbox)?;
    let token = new_token(event.id, s.rev() ^ s.now().unsigned_abs().rotate_left(24));
    let (remote, series, at) = match occurrence {
        Some(day) => (String::new(), event.origin.id.clone(), occurrence_instant(event, day, &offset_of(s))),
        None => (event.origin.id.clone(), String::new(), 0),
    };
    s.mailbox.event_changes.push(EventChange {
        account,
        kind: EventChangeKind::Forward,
        event: event.id,
        remote,
        token,
        series,
        occurrence: at,
        send: true,
        comment: comment.to_string(),
        to,
        ..Default::default()
    });
    Ok(())
}

/// An appointment that a server keeps (`before`, as it was) was put into another calendar of
/// the same account; it is the event `id` here still. A server has no way of moving an event
/// from one calendar to another, so it is made anew in the calendar it is in now, and the
/// original is deleted once that has gone through and the copy has been seen there
/// ([`finish_move`], [`waits_for_copy`]); should either step be refused, or the copy not be
/// there after all, it is put back as it was ([`move_refused`], [`take_back_copy`],
/// [`move_not_seen`]). Until then a download leaves the original out.
pub(crate) fn note_moved(s: &mut Session, id: Id, before: &Event) -> Result<(), CmdError> {
    room_for_change(&s.mailbox)?;
    let token = new_token(id, s.rev() ^ s.now().unsigned_abs().rotate_left(16));
    // The account that keeps the calendar it is in now: the original's, or (#70) another's, a
    // shared mailbox's say, when the appointment goes across.
    let target = s.mailbox.event(id).and_then(|event| s.mailbox.calendar(event.calendar)).map(|calendar| calendar.origin.account);
    let account = target.filter(|account| *account != 0).unwrap_or(before.origin.account);
    let event = s.mailbox.event_mut(id)?;
    // A new event as far as the server goes, with a name of its own to come.
    event.origin = Origin { account, ..Default::default() };
    event.uid.clear();
    s.mailbox.event_changes.push(EventChange {
        account,
        kind: EventChangeKind::Create,
        event: id,
        token,
        base: Some(Box::new(before.clone())),
        replaces: before.origin.id.clone(),
        ..Default::default()
    });
    Ok(())
}

/// The server has made the copy of an appointment that was moved to another calendar (`noted`
/// is the note of making it, `copy` what the server calls it): now the original goes.
fn finish_move(mailbox: &mut Mailbox, account: Id, id: Id, noted: EventChange, copy: &str) {
    if noted.replaces.is_empty() {
        return;
    }
    // The original is deleted where it is: on the server it was moved from, which (#70) may be
    // another account's than the copy's.
    let source = noted.base.as_deref().map_or(account, |base| base.origin.account);
    mailbox.event_changes.push(EventChange {
        account: source,
        kind: EventChangeKind::Delete,
        event: id,
        remote: noted.replaces,
        base: noted.base,
        moved: copy.to_string(),
        copy_account: if source == account { 0 } else { account },
        ..Default::default()
    });
}

/// The server wouldn't make the copy of an appointment that was being moved to another calendar
/// (`noted` is the note of making it): the appointment is back as it was, in the calendar the
/// server still has it in. Whether that is what was done.
fn move_refused(mailbox: &mut Mailbox, noted: EventChange, outcome: &EventOutcome, applied: &mut EventsApplied) -> bool {
    let Some(base) = noted.base.filter(|_| !noted.replaces.is_empty()) else { return false };
    let name = called(Some(&base));
    match mailbox.event_mut(base.id) {
        Ok(event) => *event = *base,
        Err(_) => {
            let _ = mailbox.add_event(*base);
        }
    }
    applied.say(format!(
        "The server wouldn't take {name} into the other calendar{}, so it wasn't moved: it is back where it was.",
        because(&outcome.error)
    ));
    true
}

/// The server made the copy of a moved appointment (`copy`) and then wouldn't delete the
/// original: the copy goes again, here and there. Not when people were invited to it meanwhile
/// (deleting it would send every one of them a cancellation nobody was asked about): then it
/// stays, and `false` says so.
fn take_back_copy(mailbox: &mut Mailbox, account: Id, copy: &str) -> bool {
    let found = mailbox.events.values().find(|event| event.origin.account == account && event.origin.id == copy).cloned();
    if found.as_ref().is_some_and(|event| !event.attendees.is_empty()) {
        return false;
    }
    let Some(found) = found else {
        // Deleted here meanwhile: that deletion waits already, or has gone.
        return true;
    };
    let _ = mailbox.remove_event(found.id);
    mailbox.event_changes.retain(|change| !(change.account == account && change.remote == copy));
    // (With the copy as it was: the server is asked for it in the calendar it is in.)
    mailbox.event_changes.push(EventChange {
        account,
        kind: EventChangeKind::Delete,
        event: found.id,
        remote: copy.to_string(),
        base: Some(Box::new(found.as_ref().clone())),
        ..Default::default()
    });
    true
}

/// How long the original of a moved appointment is kept on the server for the sake of seeing
/// its copy there first (seconds): for as long as a host asks again soon ([`recheck_due`]). A
/// copy that no download can show (it is outside what is asked for) is taken on the server's
/// word after that, as it is everywhere else.
const COPY_WAIT: i64 = RECHECK_FOR;

/// Whether the deletion that finishes a move to another calendar ([`finish_move`]) is to wait
/// still, and until when (Unix seconds): the server said it made the copy, and no download has
/// shown it there yet. A server's "yes" is not taken for the copy being there
/// ([`EventCheck`]), least of all for deleting the original on the strength of it. Should the
/// copy turn out not to be there, the move is taken back ([`check_sent`]).
fn waits_for_copy(mailbox: &Mailbox, change: &EventChange, now: Timestamp) -> Option<Timestamp> {
    if change.kind != EventChangeKind::Delete || change.moved.is_empty() {
        return None;
    }
    let copy_of = if change.copy_account == 0 { change.account } else { change.copy_account };
    let of_copy = |check: &&EventCheck| check.account == copy_of && check.kind != EventChangeKind::Delete && check.remote == change.moved;
    let until = mailbox.event_checks.iter().find(of_copy).map(|check| check.made.saturating_add(COPY_WAIT))?;
    (now <= until).then_some(until.saturating_add(1))
}

/// The server said it made the copy of a moved appointment (`copy`) and doesn't have it after
/// all, and the original has not been deleted yet: the move is taken back. The appointment is
/// as it was, in the calendar the server still has it in. Its name, when that is what was done.
fn move_not_seen(mailbox: &mut Mailbox, account: Id, copy: &str) -> Option<String> {
    let ends = |change: &EventChange| {
        let copy_of = if change.copy_account == 0 { change.account } else { change.copy_account };
        change.kind == EventChangeKind::Delete && copy_of == account && change.moved == copy
    };
    let at = mailbox.event_changes.iter().position(ends)?;
    // (Only an original that can be put back: its place here is the copy's.)
    let base = mailbox.event_changes.get(at).and_then(|change| change.base.clone())?;
    mailbox.event_changes.remove(at);
    mailbox.event_changes.retain(|change| !(change.account == account && change.remote == copy));
    let name = called(Some(&base));
    match mailbox.event_mut(base.id) {
        Ok(event) => *event = *base,
        Err(_) => {
            let _ = mailbox.add_event(*base);
        }
    }
    Some(name)
}

/// What the server made of an event that was to be held online: the link to join it by, when
/// its answer has one; and when it wouldn't have an online meeting (an account that can't make
/// one), the event is as the server has it, without, and that is said.
fn settle_online(event: &mut Event, outcome: &EventOutcome, applied: &mut EventsApplied) {
    if outcome.op.fields.online.is_none() {
        return;
    }
    if outcome.online == Some(false) {
        event.online = None;
        let name = called(Some(event));
        applied.say(format!(
            "{name} couldn't be made a Teams meeting: the account's server wouldn't have it{}. The meeting itself is saved, without a link to join.",
            because(&outcome.error)
        ));
    } else if let Some(online) = &mut event.online
        && !outcome.join_url.is_empty()
    {
        online.join_url = outcome.join_url.clone();
    }
}

/// The answer to forwarding a meeting.
fn forwarded(mailbox: &mut Mailbox, account: Id, outcome: &EventOutcome, now: Timestamp, applied: &mut EventsApplied, told: &mut bool) {
    let op = &outcome.op;
    let noted = |change: &EventChange| {
        change.kind == EventChangeKind::Forward && change.account == account && !op.token.is_empty() && change.token == op.token
    };
    let Some(at) = mailbox.event_changes.iter().position(noted) else {
        *told |= outcome.result == EventResult::Done;
        return;
    };
    let name = called(mailbox.event_changes.get(at).and_then(|change| mailbox.event(change.event)));
    match outcome.result {
        EventResult::Done => {
            *told = true;
            applied.done += 1;
            mailbox.event_changes.remove(at);
        }
        EventResult::Failed => {
            if let Some(change) = mailbox.event_changes.get_mut(at) {
                held_back(change, outcome, now);
                applied.failed += 1;
            }
        }
        EventResult::Gone => {
            mailbox.event_changes.remove(at);
            applied.refused += 1;
            applied.say(format!("{name} is no longer on the server, so it wasn't forwarded."));
        }
        EventResult::Denied | EventResult::Refused => {
            let Some(change) = mailbox.event_changes.get_mut(at) else { return };
            if outcome.result == EventResult::Denied && !held_back(change, outcome, now) {
                applied.denied = true;
                applied.failed += 1;
                return;
            }
            mailbox.event_changes.remove(at);
            applied.refused += 1;
            applied.say(format!("The server wouldn't forward {name}{}. Nobody was written to.", because(&outcome.error)));
        }
    }
}

/// The answer to changing, deleting or answering one occurrence of a series.
fn one_occurrence(
    mailbox: &mut Mailbox,
    account: Id,
    outcome: &EventOutcome,
    offset_at: &dyn Fn(Timestamp) -> i32,
    now: Timestamp,
    applied: &mut EventsApplied,
    told: &mut bool,
) {
    let op = &outcome.op;
    let noted = |change: &EventChange| {
        change.account == account
            && change.kind == op.kind
            && change.of_occurrence()
            && change.series == op.series
            && change.occurrence == op.occurrence
    };
    let Some(at) = mailbox.event_changes.iter().position(noted) else {
        // Undone while it was on its way: what the server has now comes with the next download.
        *told |= outcome.result == EventResult::Done;
        return;
    };
    let changing = op.kind == EventChangeKind::Update;
    let series = mailbox.events.values().find(|event| event.origin.account == account && event.origin.id == op.series).map(|event| event.id);
    // The event the occurrence is here, for one that was changed.
    let one = mailbox
        .event_changes
        .get(at)
        .map(|change| change.event)
        .filter(|id| changing && mailbox.event(*id).is_some_and(|event| event.origin.id.is_empty() && event.origin.series == op.series));
    match outcome.result {
        EventResult::Done if changing && (outcome.remote.is_empty() || one.is_none()) => {
            // Changed, but under what name wasn't said: asked again, which changes nothing more.
            if let Some(change) = mailbox.event_changes.get_mut(at) {
                change.failures = change.failures.saturating_add(1);
                change.retry_at = now.saturating_add(wait_after(change.failures, None));
            }
            applied.failed += 1;
        }
        EventResult::Done => {
            *told = true;
            applied.done += 1;
            let change = mailbox.event_changes.remove(at);
            // A day that was deleted there is to stay deleted in what comes down.
            if op.kind == EventChangeKind::Delete {
                expect(
                    mailbox,
                    EventCheck {
                        account,
                        kind: EventChangeKind::Delete,
                        event: series.unwrap_or(0),
                        series: op.series.clone(),
                        occurrence: op.occurrence,
                        made: now,
                        ..Default::default()
                    },
                );
            }
            let Some(Ok(event)) = one.map(|id| mailbox.event_mut(id)) else { return };
            // The occurrence is an event of its own on the server now, under a name of its own.
            event.origin.id = outcome.remote.clone();
            event.origin.change_key = outcome.change_key.clone();
            let here = event.clone();
            if !seen_again(&op.fields).is_empty() {
                expect(
                    mailbox,
                    EventCheck {
                        account,
                        kind: EventChangeKind::Update,
                        event: here.id,
                        remote: here.origin.id.clone(),
                        fields: seen_again(&op.fields),
                        change_key: outcome.change_key.clone(),
                        modified: outcome.modified,
                        made: now,
                        ..Default::default()
                    },
                );
            }
            let Some(mut sent) = change.base.map(|base| *base) else { return };
            put_fields(&mut sent, &op.fields, offset_at);
            (sent.id, sent.calendar, sent.origin) = (here.id, here.calendar, here.origin.clone());
            // What was changed here since is still to go.
            if !changed_fields(&sent, &here, offset_at).is_empty() && mailbox.event_changes.len() < MAX_EVENT_CHANGES {
                mailbox.event_changes.push(EventChange {
                    account,
                    kind: EventChangeKind::Update,
                    event: here.id,
                    remote: here.origin.id.clone(),
                    base: Some(Box::new(sent)),
                    send: change.send,
                    ..Default::default()
                });
            }
        }
        EventResult::Gone => {
            // The series has nothing at that time any more: nothing to delete, nothing to change.
            mailbox.event_changes.remove(at);
            applied.done += 1;
            // (So the server says. A day that was to be deleted is looked for in what comes
            // down all the same: "no such occurrence" is also what a wrong address is answered.)
            if op.kind == EventChangeKind::Delete {
                expect(
                    mailbox,
                    EventCheck {
                        account,
                        kind: EventChangeKind::Delete,
                        event: series.unwrap_or(0),
                        series: op.series.clone(),
                        occurrence: op.occurrence,
                        made: now,
                        not_found: true,
                        ..Default::default()
                    },
                );
            }
            if let Some(id) = one {
                let name = called(mailbox.event(id));
                let _ = mailbox.remove_event(id);
                applied.refused += 1;
                let message = format!("{name} is no longer on the server, so it couldn't be changed there. It was removed here too.");
                log::warn!("calendar: a change to one occurrence was not taken: {GONE_THERE}");
                applied.say(message.clone());
                applied.dropped.push(Dropped { account, kind: op.kind, reason: GONE_THERE, message });
            }
        }
        EventResult::Failed => {
            if let Some(change) = mailbox.event_changes.get_mut(at) {
                held_back(change, outcome, now);
                applied.failed += 1;
            }
        }
        EventResult::Denied | EventResult::Refused => {
            let Some(change) = mailbox.event_changes.get_mut(at) else { return };
            if outcome.result == EventResult::Denied && !held_back(change, outcome, now) {
                applied.denied = true;
                applied.failed += 1;
                return;
            }
            mailbox.event_changes.remove(at);
            applied.refused += 1;
            let name = called(one.or(series).and_then(|id| mailbox.event(id)));
            if let Some(id) = one {
                let _ = mailbox.remove_event(id);
            }
            // The day is the series' own again.
            if let Some(Ok(series)) = series.map(|id| mailbox.event_mut(id)) {
                let day = series_day_at(series, op.occurrence, offset_at);
                series.exceptions.retain(|skipped| *skipped != day);
            }
            let what = match op.kind {
                EventChangeKind::Delete => "deleting one occurrence of",
                EventChangeKind::Respond => "your answer to one occurrence of",
                _ => "the change to one occurrence of",
            };
            applied.say(format!("The server wouldn't take {what} {name}{}. It was put back as it was.", because(&outcome.error)));
        }
    }
}

/// The answer to answering somebody else's meeting.
fn responded(mailbox: &mut Mailbox, account: Id, outcome: &EventOutcome, now: Timestamp, applied: &mut EventsApplied, told: &mut bool) {
    let op = &outcome.op;
    if op.remote.is_empty() {
        return;
    }
    let noted = |change: &EventChange| change.kind == EventChangeKind::Respond && change.account == account && change.remote == op.remote;
    let Some(at) = mailbox.event_changes.iter().position(noted) else {
        *told |= outcome.result == EventResult::Done;
        return;
    };
    match outcome.result {
        // (Gone: the organizer called it off meanwhile; the next download takes it away here.)
        EventResult::Done | EventResult::Gone => {
            *told |= outcome.result == EventResult::Done;
            applied.done += 1;
            mailbox.event_changes.remove(at);
        }
        EventResult::Failed => {
            if let Some(change) = mailbox.event_changes.get_mut(at) {
                held_back(change, outcome, now);
                applied.failed += 1;
            }
        }
        EventResult::Denied | EventResult::Refused => {
            let Some(change) = mailbox.event_changes.get_mut(at) else { return };
            if outcome.result == EventResult::Denied && !held_back(change, outcome, now) {
                applied.denied = true;
                applied.failed += 1;
                return;
            }
            let change = mailbox.event_changes.remove(at);
            applied.refused += 1;
            let name = called(change.base.as_deref());
            // As it was before the answer: back in the calendar when it was declined away.
            if let Some(base) = change.base {
                match mailbox.event_mut(base.id) {
                    Ok(event) => *event = *base,
                    Err(_) => {
                        let _ = mailbox.add_event(*base);
                    }
                }
            }
            applied.say(format!("The server wouldn't take your answer to {name}{}. It was put back as it was.", because(&outcome.error)));
        }
    }
}

/// The answer to changing an event.
fn updated(
    mailbox: &mut Mailbox,
    account: Id,
    outcome: &EventOutcome,
    offset_at: &dyn Fn(Timestamp) -> i32,
    now: Timestamp,
    applied: &mut EventsApplied,
    told: &mut bool,
) {
    let op = &outcome.op;
    if op.remote.is_empty() {
        return;
    }
    let about = |change: &EventChange| change.account == account && change.remote == op.remote && change.kind != EventChangeKind::Create;
    let at = mailbox.event_changes.iter().position(about);
    let event = mailbox.events.values().find(|event| event.origin.account == account && event.origin.id == op.remote).map(|event| event.id);
    match outcome.result {
        EventResult::Done => {
            *told = true;
            applied.done += 1;
            if let Some(Ok(event)) = event.map(|id| mailbox.event_mut(id))
                && !outcome.change_key.is_empty()
            {
                event.origin.change_key = outcome.change_key.clone();
            }
            if let Some(Ok(event)) = event.map(|id| mailbox.event_mut(id)) {
                settle_online(event, outcome, applied);
            }
            // What the server took of the change: all of it, or all but whether another time may
            // be proposed, which it may not let a change say. Then that option alone is as it
            // was here too, and the owner is told; the rest of what was changed stays.
            let mut taken = op.fields.clone();
            if outcome.new_times == Some(false)
                && let Some(asked) = taken.allow_proposals.take()
                && let Some(Ok(event)) = event.map(|id| mailbox.event_mut(id))
                && event.response_options.allow_proposals == asked
            {
                event.response_options.allow_proposals = !asked;
                let name = called(Some(&*event));
                log::info!("calendar: #{}: the server took a change without what it said of proposing another time", id_tag(&op.remote));
                applied.say(format!(
                    "The account's server doesn't let it be changed, once a meeting is made, whether another time may be proposed for {name}. That option is as it was."
                ));
            }
            let here = event.and_then(|id| mailbox.event(id)).cloned();
            match at {
                // The server has what was sent now; what was changed here since is still to go
                // (and what is to be put back, should a deletion be refused, is this).
                Some(at) => {
                    // The server said so: the downloads that follow are to show it (what of it
                    // a download can show: [`seen_again`]).
                    if let Some(id) = event.filter(|_| !seen_again(&op.fields).is_empty()) {
                        expect(
                            mailbox,
                            EventCheck {
                                account,
                                kind: EventChangeKind::Update,
                                event: id,
                                remote: op.remote.clone(),
                                fields: seen_again(&op.fields),
                                change_key: outcome.change_key.clone(),
                                modified: outcome.modified,
                                made: now,
                                ..Default::default()
                            },
                        );
                    }
                    let mut settled = false;
                    if let Some(change) = mailbox.event_changes.get_mut(at) {
                        if let Some(base) = change.base.as_deref_mut() {
                            put_fields(base, &taken, offset_at);
                            base.origin.change_key = outcome.change_key.clone();
                        }
                        change.failures = 0;
                        change.retry_at = 0;
                        change.error.clear();
                        settled = change.kind == EventChangeKind::Update
                            && match (change.base.as_deref(), here.as_ref()) {
                                (Some(base), Some(event)) => changed_fields(base, event, offset_at).is_empty(),
                                _ => true,
                            };
                    }
                    if settled {
                        mailbox.event_changes.remove(at);
                    }
                }
                // Undone while it was on its way: the server has the change and the mailbox
                // doesn't, so what the mailbox says is to be sent after it.
                None => {
                    let Some(event) = here else { return };
                    let mut sent = event.clone();
                    put_fields(&mut sent, &taken, offset_at);
                    if !changed_fields(&sent, &event, offset_at).is_empty() {
                        let back = EventChange {
                            account,
                            kind: EventChangeKind::Update,
                            event: event.id,
                            remote: event.origin.id.clone(),
                            base: Some(Box::new(sent.clone())),
                            ..Default::default()
                        };
                        if permitted(mailbox, &back) {
                            mailbox.event_changes.push(back);
                        } else if let Ok(here) = mailbox.event_mut(event.id) {
                            // Taking it back there would be a second update in the mail of
                            // everyone invited, which nobody said to send: the event is as the
                            // server has it, and the owner hears why.
                            let name = called(Some(&event));
                            *here = sent;
                            applied.say(format!(
                                "The change to {name} had already reached the server, and the people invited, when it was taken back here. It is shown as the server has it."
                            ));
                        }
                    }
                }
            }
        }
        EventResult::Gone => {
            // The server no longer has it: there is nothing to change, here either.
            let Some(at) = at else { return };
            let deleting = mailbox.event_changes.get(at).is_some_and(|change| change.kind == EventChangeKind::Delete);
            mailbox.event_changes.remove(at);
            applied.done += 1;
            if !deleting && let Some(id) = event {
                let name = called(mailbox.event(id));
                let _ = mailbox.remove_event(id);
                applied.refused += 1;
                let message = format!("{name} is no longer on the server, so it couldn't be changed there. It was removed here too.");
                log::warn!("calendar: #{}: a change was not taken: {GONE_THERE}", id_tag(&op.remote));
                applied.say(message.clone());
                applied.dropped.push(Dropped { account, kind: EventChangeKind::Update, reason: GONE_THERE, message });
            }
        }
        EventResult::Failed => {
            if let Some(change) = at.and_then(|at| mailbox.event_changes.get_mut(at)).filter(|change| change.kind == EventChangeKind::Update) {
                held_back(change, outcome, now);
                applied.failed += 1;
            }
        }
        EventResult::Denied | EventResult::Refused => {
            let Some(at) = at else { return };
            let Some(change) = mailbox.event_changes.get_mut(at).filter(|change| change.kind == EventChangeKind::Update) else { return };
            if outcome.result == EventResult::Denied && !held_back(change, outcome, now) {
                applied.denied = true;
                applied.failed += 1;
                return;
            }
            let change = mailbox.event_changes.remove(at);
            applied.refused += 1;
            let name = called(event.and_then(|id| mailbox.event(id)));
            if let (Some(base), Some(Ok(event))) = (change.base, event.map(|id| mailbox.event_mut(id))) {
                // As the server has it.
                let (id, calendar) = (event.id, event.calendar);
                *event = Event { id, calendar, ..*base };
            }
            applied.say(format!("The server wouldn't take the change to {name}{}. It was put back as it was.", because(&outcome.error)));
        }
    }
}

/// The answer to deleting an event.
fn deleted(mailbox: &mut Mailbox, account: Id, outcome: &EventOutcome, now: Timestamp, applied: &mut EventsApplied, told: &mut bool) {
    let op = &outcome.op;
    if op.remote.is_empty() {
        return;
    }
    let noted = |change: &EventChange| change.kind == EventChangeKind::Delete && change.account == account && change.remote == op.remote;
    let at = mailbox.event_changes.iter().position(noted);
    match outcome.result {
        EventResult::Done | EventResult::Gone => {
            *told |= outcome.result == EventResult::Done;
            applied.done += 1;
            if let Some(at) = at {
                let change = mailbox.event_changes.remove(at);
                // It is to stay gone in what comes down. So is one of which the server said that
                // it has no such event: that is its answer for one that is gone already, and for
                // one that was asked for at the wrong address. Only a download tells which.
                expect(
                    mailbox,
                    EventCheck {
                        account,
                        kind: EventChangeKind::Delete,
                        event: change.event,
                        remote: op.remote.clone(),
                        made: now,
                        base: change.base,
                        not_found: outcome.result == EventResult::Gone,
                        moved: change.moved,
                        copy_account: change.copy_account,
                        ..Default::default()
                    },
                );
                return;
            }
            // Undone while it was on its way: the event is here again and the server no longer
            // has it. An appointment of the owner's own is made anew there; somebody else's
            // meeting can't be, and goes here too.
            mailbox.event_changes.retain(|change| !(change.account == account && change.remote == op.remote));
            let Some(id) =
                mailbox.events.values().find(|event| event.origin.account == account && event.origin.id == op.remote).map(|event| event.id)
            else {
                return;
            };
            let own = mailbox.event(id).is_some_and(|event| !event.origin.guest && event.attendees.is_empty() && event.origin.series.is_empty());
            if own && mailbox.event_changes.len() < MAX_EVENT_CHANGES {
                if let Ok(event) = mailbox.event_mut(id) {
                    event.origin = Origin { account, ..Default::default() };
                }
                // The mark is new: the one event the server had under the old one is gone.
                let token = new_token(id, now.unsigned_abs());
                mailbox.event_changes.push(EventChange { account, kind: EventChangeKind::Create, event: id, token, ..Default::default() });
            } else {
                let name = called(mailbox.event(id));
                let _ = mailbox.remove_event(id);
                applied.say(format!("{name} was already deleted on the server when the deletion was undone here; it couldn't be brought back."));
            }
        }
        EventResult::Failed => {
            if let Some(change) = at.and_then(|at| mailbox.event_changes.get_mut(at)) {
                held_back(change, outcome, now);
                applied.failed += 1;
            }
        }
        EventResult::Denied | EventResult::Refused => {
            let Some(at) = at else { return };
            let Some(change) = mailbox.event_changes.get_mut(at) else { return };
            if outcome.result == EventResult::Denied && !held_back(change, outcome, now) {
                applied.denied = true;
                applied.failed += 1;
                return;
            }
            let change = mailbox.event_changes.remove(at);
            applied.refused += 1;
            let name = called(change.base.as_deref());
            // The end of a move to another calendar: the copy that was made there goes again.
            let moving = !change.moved.is_empty();
            // (A copy in another mailbox's calendar stays: it is a good copy, and the owner is
            // told that the original is still where it was, rather than something being undone
            // in a calendar of somebody else's behind their back.)
            let taken_back = moving && change.copy_account == 0 && take_back_copy(mailbox, account, &change.moved);
            // Back in its calendar, under the id it had when that is still free.
            let back = change.base.is_some_and(|base| mailbox.add_event(*base).is_ok());
            let then = if back { " It is back in the calendar." } else { "" };
            if taken_back {
                applied.say(format!(
                    "The server wouldn't take {name} out of the calendar it was in{}, so it wasn't moved: it is back there, and the copy in the other calendar was removed.",
                    because(&outcome.error)
                ));
            } else if moving {
                applied.say(format!(
                    "The server wouldn't take {name} out of the calendar it was in{}. It is in both calendars now.",
                    because(&outcome.error)
                ));
            } else {
                applied.say(format!("The server wouldn't delete {name}{}.{then}", because(&outcome.error)));
            }
        }
    }
}

/// The calendar in the mailbox that follows the server's calendar `remote` of `account`
/// (`remote` empty: the account's main one), made when there is none yet. With the id: whether
/// it was made just now.
fn calendar_for(mailbox: &mut Mailbox, account: Id, remote: &str) -> Result<(Id, bool), CmdError> {
    if let Some(calendar) = mailbox.calendars.iter().find(|calendar| calendar.origin.account == account && calendar.origin.id == remote) {
        return Ok((calendar.id, false));
    }
    if mailbox.calendars.len() >= MAX_CALENDARS {
        return Err(fail("there are too many calendars"));
    }
    // Simply "Calendar", as the server has it: whose it is, the account it is listed under says.
    // Another calendar of the same account gets a number.
    let taken = |name: &str| mailbox.calendars.iter().any(|calendar| calendar.origin.account == account && calendar.name.eq_ignore_ascii_case(name));
    let free = (1..=MAX_CALENDARS + 1)
        .map(|n| if n == 1 { SERVER_CALENDAR.to_string() } else { format!("{SERVER_CALENDAR} {n}") })
        .find(|name| !taken(name));
    let name = free.unwrap_or_else(|| SERVER_CALENDAR.into());
    let color = super::calendar::next_color(mailbox);
    let id = mailbox.alloc_id();
    let origin = Origin { account, id: remote.to_string(), ..Default::default() };
    mailbox.calendars.push(Calendar { id, name, color, visible: true, origin, ..Default::default() });
    Ok((id, true))
}

/// What the calendar that follows an account's main one is called, and what a calendar of the
/// mailbox's own is called beside it.
const SERVER_CALENDAR: &str = "Calendar";
pub const LOCAL_CALENDAR: &str = "On this computer";

/// Beside a server's calendar there is no second "Calendar" to puzzle over. The calendar the
/// mailbox was made with goes when nothing is in it (new events go to the server's, and one
/// that can't is given a calendar of its own when it is made); when it holds events it stays,
/// under a name that says where it lives. A calendar the owner made, under a name of the
/// owner's, is left alone. And a server's calendar that was named after its account, as it once
/// was, is called what the server calls it.
fn settle_calendars(mailbox: &mut Mailbox) {
    let server_kept = |mailbox: &Mailbox, calendar: &Calendar| keeper(mailbox, &calendar.origin).is_some();
    if !mailbox.calendars.iter().any(|calendar| server_kept(mailbox, calendar)) {
        return;
    }
    let renamed: Vec<Id> = mailbox
        .calendars
        .iter()
        .filter(|calendar| server_kept(mailbox, calendar) && calendar.origin.id.is_empty() && calendar.name.starts_with("Calendar ("))
        .map(|calendar| calendar.id)
        .collect();
    let own: Vec<Id> = mailbox
        .calendars
        .iter()
        .filter(|calendar| !server_kept(mailbox, calendar) && calendar.name.trim().eq_ignore_ascii_case(SERVER_CALENDAR))
        .map(|calendar| calendar.id)
        .collect();
    let in_use: HashSet<Id> = if own.is_empty() { HashSet::new() } else { mailbox.events.values().map(|event| event.calendar).collect() };
    // The name is free unless the owner already has a calendar called that.
    let taken = mailbox.calendars.iter().any(|calendar| calendar.name.trim().eq_ignore_ascii_case(LOCAL_CALENDAR));
    for calendar in &mut mailbox.calendars {
        if renamed.contains(&calendar.id) {
            calendar.name = SERVER_CALENDAR.into();
        } else if own.contains(&calendar.id) && in_use.contains(&calendar.id) && !taken && own.first() == Some(&calendar.id) {
            calendar.name = LOCAL_CALENDAR.into();
        }
    }
    mailbox.calendars.retain(|calendar| !(own.contains(&calendar.id) && !in_use.contains(&calendar.id)));
}

/// What a download makes of the changes that were sent and are yet to be seen ([`check_sent`]).
#[derive(Default)]
struct Checked {
    /// Events the download doesn't show as they were sent yet, by what the server calls them:
    /// for now they stay as they are here (`Update`), or stay deleted (`Delete`).
    held: HashMap<String, EventChangeKind>,
    /// Days of series that were deleted on the server and that the download still has, by series.
    held_days: HashMap<String, Vec<Timestamp>>,
}

/// What of an event as a server has it (`theirs`) is not as it was `sent`: empty when the server
/// shows what it was told.
fn not_shown(sent: &EventFields, theirs: &Event, offset_at: &dyn Fn(Timestamp) -> i32) -> Vec<&'static str> {
    let have = fields_of(theirs, offset_at);
    let mut differs = Vec::new();
    if sent.time.is_some_and(|time| have.time != Some(time)) {
        differs.push("its time");
    }
    if sent.title.as_deref().is_some_and(|title| have.title.as_deref().map(str::trim) != Some(title.trim())) {
        differs.push("its title");
    }
    if sent.show_as.is_some_and(|show_as| have.show_as != Some(show_as)) {
        differs.push("how it shows");
    }
    if sent.private.is_some_and(|private| have.private != Some(private)) {
        differs.push("whether it is private");
    }
    differs
}

/// "its time", "its time and its title", "its time, its title and how it shows".
fn listed(parts: &[&str]) -> String {
    match parts {
        [] => "it".to_string(),
        [one] => (*one).to_string(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

/// Look in a download of one calendar for the changes that were sent to `account`'s server and
/// are yet to be seen there ([`EventCheck`]).
///
/// - The event is there as it was sent (or gone, when it was deleted): the change has arrived,
///   and the note goes.
/// - It isn't, and the server's copy is not the version it said it made of the change: the
///   answer may be older than the change. The event stays as it is here for now (`held`), up to
///   [`MAX_CHECK_TRIES`] downloads.
/// - It isn't, and the server's copy is that version, or enough downloads have gone by: the
///   server doesn't have what the owner did. The event is as the server has it from here on
///   (the merge that follows puts it so), the owner is told which event and what of it, and a
///   line goes to the log. An event that was made here and that the server doesn't have after
///   all is kept on this computer rather than lost.
///
/// A change that still waits for the same event is looked at when it has gone. A download that
/// can't tell (it isn't complete, the event is outside its stretch or in another calendar) says
/// nothing.
#[allow(clippy::too_many_arguments)]
fn check_sent(
    mailbox: &mut Mailbox,
    account: Id,
    calendar: Id,
    download: &EventsDownload,
    series: &[RemoteEvent],
    rest: &[RemoteEvent],
    waiting: &HashMap<String, EventChangeKind>,
    email: &str,
    offset_at: &dyn Fn(Timestamp) -> i32,
    merged: &mut Merged,
) -> Checked {
    let mut checked = Checked::default();
    if !mailbox.event_checks.iter().any(|check| check.account == account) {
        return checked;
    }
    let complete = download.complete && download.events.len() <= MAX_EVENTS && download.from < download.to;
    let asked = |instant: Timestamp| complete && (download.from..download.to).contains(&instant);
    let covers = |event: &Event| event.calendar == calendar && event.recurrence.is_none() && asked(server_start(event, offset_at));
    let theirs: HashMap<&str, &RemoteEvent> = series.iter().chain(rest).map(|remote| (remote.id.as_str(), remote)).collect();
    let mut still: Vec<EventCheck> = Vec::new();
    // What a log line calls the event: a tag made of its id, the same in every line about it.
    let tag = |check: &EventCheck| id_tag(if check.of_occurrence() { &check.series } else { &check.remote });
    let seen = |merged: &mut Merged, check: &EventCheck| {
        log::info!("calendar: #{} is on the server as it was sent ({:?})", tag(check), check.kind);
        merged.verified += 1;
    };
    let not_yet = |check: &EventCheck| {
        log::info!("calendar: #{} is not on the server as it was sent yet (look {} of {MAX_CHECK_TRIES})", tag(check), check.tries);
    };
    let failed = |merged: &mut Merged, check: &EventCheck, why: &str, line: String| {
        log::warn!("calendar: #{}: a change the server said it took is not on the server ({:?}): {why}", tag(check), check.kind);
        merged.put_back += 1;
        merged.say(line);
    };
    for mut check in std::mem::take(&mut mailbox.event_checks) {
        if check.account != account {
            still.push(check);
            continue;
        }
        if check.of_occurrence() {
            // One day of a series that was deleted there: the series is not to have it.
            let of = mailbox.events.values().find(|event| event.origin.account == account && event.origin.id == check.series);
            let Some(of) = of.map(|of| of.as_ref().clone()) else { continue };
            if of.calendar != calendar || !asked(check.occurrence) {
                still.push(check);
                continue;
            }
            let near = if of.all_day { 12 * 3_600 - 1 } else { 90 };
            let is_it = |remote: &RemoteEvent| {
                remote.series == check.series
                    && remote.kind != RemoteEventKind::Single
                    && remote.original.unwrap_or(remote.start).saturating_sub(check.occurrence).saturating_abs() <= near
            };
            if !rest.iter().any(is_it) {
                seen(merged, &check);
                continue;
            }
            check.tries = check.tries.saturating_add(1);
            if check.tries < MAX_CHECK_TRIES {
                not_yet(&check);
                checked.held_days.entry(check.series.clone()).or_default().push(check.occurrence);
                still.push(check);
            } else {
                let name = called(Some(&of));
                let said = if check.not_found { "the server answered that it has no such day" } else { "the server said it deleted it too" };
                let line = format!("One day of {name} was deleted here and {said}, but it still has it. That day is back in the calendar.");
                failed(merged, &check, "the server still has the occurrence", line);
            }
            continue;
        }
        // A change still waits for it: what it is to be on the server is not settled yet.
        if waiting.contains_key(&check.remote) {
            still.push(check);
            continue;
        }
        let here = mailbox.events.values().find(|event| event.origin.account == account && event.origin.id == check.remote);
        let here = here.map(|event| event.as_ref().clone());
        match (check.kind, theirs.get(check.remote.as_str())) {
            (EventChangeKind::Delete, found) => {
                // Still there, itself or (a series) in one of its days?
                if found.is_some() || rest.iter().any(|remote| remote.series == check.remote) {
                    check.tries = check.tries.saturating_add(1);
                    if check.tries < MAX_CHECK_TRIES {
                        not_yet(&check);
                        checked.held.insert(check.remote.clone(), EventChangeKind::Delete);
                        still.push(check);
                    } else {
                        let name = called(check.base.as_deref());
                        let (said, why) = match check.not_found {
                            true => ("the server answered that it has no such item", "the server still has an event it answered \"not found\" for"),
                            false => ("the server said it deleted it too", "the server still has the event"),
                        };
                        // The end of a move to another calendar: the original is there still, so
                        // the copy goes again, and the appointment is where the server has it.
                        let line = if check.moved.is_empty() {
                            format!("{name} was deleted here and {said}, but it still has it. It is back in the calendar.")
                        } else if check.copy_account == 0 && take_back_copy(mailbox, account, &check.moved) {
                            format!(
                                "{name} was moved to another calendar here, and when it was to be taken out of the one it was in {said}, but it still has it there. It wasn't moved: it is back there, and the copy in the other calendar was removed."
                            )
                        } else {
                            format!(
                                "{name} was moved to another calendar here, and when it was to be taken out of the one it was in {said}, but it still has it there. It is in both calendars now."
                            )
                        };
                        failed(merged, &check, why, line);
                    }
                    continue;
                }
                // Gone, as far as this download can tell.
                let told = match check.base.as_deref() {
                    Some(base) if base.recurrence.is_some() => complete && base.calendar == calendar,
                    Some(base) => covers(base),
                    None => complete,
                };
                if told {
                    seen(merged, &check);
                } else {
                    still.push(check);
                }
            }
            (_, Some(remote)) => {
                // (No longer here: there is nothing to compare, and nothing to put back.)
                let Some(here) = here else { continue };
                let differs = not_shown(&check.fields, &series_from_remote(remote, email, offset_at), offset_at);
                // Shown as it was sent: the change has arrived.
                if differs.is_empty() {
                    seen(merged, &check);
                    continue;
                }
                // Not the version the server said it made of the change: an answer from before
                // it, perhaps. Looked for again in the next download, a few times.
                let their_version = !check.change_key.is_empty() && remote.change_key == check.change_key;
                if !their_version {
                    check.tries = check.tries.saturating_add(1);
                    if check.tries < MAX_CHECK_TRIES {
                        not_yet(&check);
                        checked.held.insert(check.remote.clone(), EventChangeKind::Update);
                        still.push(check);
                        continue;
                    }
                }
                let (name, what) = (called(Some(&here)), listed(&differs));
                if their_version {
                    let line = format!(
                        "The server took the change to {name} but has {what} otherwise than it was sent. It is shown here as the server has it."
                    );
                    failed(merged, &check, "the server shows other values than it was sent", line);
                } else {
                    let line = format!(
                        "{name} was changed here and the server said it took the change, but it still has {what} as before. It was put back to what the server has."
                    );
                    failed(merged, &check, "the server still shows the values of before", line);
                }
            }
            (_, None) => {
                let Some(here) = here else { continue };
                if !covers(&here) {
                    still.push(check);
                    continue;
                }
                check.tries = check.tries.saturating_add(1);
                if check.tries < MAX_CHECK_TRIES {
                    not_yet(&check);
                    checked.held.insert(check.remote.clone(), EventChangeKind::Update);
                    still.push(check);
                    continue;
                }
                let name = called(Some(&here));
                // The copy of an appointment that was being moved here from another calendar,
                // whose original the server still has: the move is taken back.
                if let Some(name) = move_not_seen(mailbox, account, &check.remote) {
                    let line = format!(
                        "{name} was moved to another calendar here and the server said it took it there, but that calendar doesn't have it. It wasn't moved: it is back where it was."
                    );
                    failed(merged, &check, "the server doesn't have the copy of a moved event", line);
                    continue;
                }
                if check.kind == EventChangeKind::Create && keep_on_this_computer(mailbox, here.id) {
                    let line = format!(
                        "{name} was made here and the server said it took it, but it is not in the server's calendar. It is kept on this computer only: it is not in your Outlook calendar."
                    );
                    failed(merged, &check, "the server doesn't have the event", line);
                } else {
                    let line = format!(
                        "{name} was changed here and the server said it took the change, but the server's calendar doesn't have it where it is here. It was taken out here; the next Send/Receive shows it where the server has it."
                    );
                    failed(merged, &check, "the server doesn't have the event where it was sent to", line);
                }
            }
        }
    }
    mailbox.event_checks = still;
    checked
}

/// Merge the events a server has in a stretch of one calendar into the mailbox.
///
/// - An event the mailbox doesn't have (by what the server calls it) is added, in the calendar
///   that follows the server's; one it has is updated when anything about it differs, keeping its
///   id here and its categories.
/// - An event that came from this calendar of the server earlier, starts within the stretch and
///   is not in the download is removed: the server no longer has it. Only when the download is
///   [complete](EventsDownload::complete); and never an event outside the stretch.
/// - Events made here, and those of other accounts, are not looked at. Doing it twice changes
///   nothing the second time.
///
/// `offset_at` is the local zone's offset (minutes east of UTC) at an instant
/// ([`event_from_remote`]). Nothing is changed when this returns an error.
pub fn merge_events(mailbox: &mut Mailbox, download: &EventsDownload, offset_at: &dyn Fn(Timestamp) -> i32) -> Result<Merged, CmdError> {
    let (account, email) = owner(mailbox, download.account, &download.email).map(|owner| (owner.id, owner.email.clone()))?;
    let remote_calendar = download.calendar.trim();
    if remote_calendar.len() > MAX_KEY || remote_calendar.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(bad("`calendar` isn't what a server calls a calendar"));
    }
    let (calendar, new_calendar) = calendar_for(mailbox, account, remote_calendar)?;
    let mut merged = Merged { calendar, new_calendar, ..Default::default() };
    // A calendar whose events can't be listed, only its free and busy times (#70), is shown as
    // that; one whose events can be listed again is at least read.
    if let Some(shown) = mailbox.calendars.iter_mut().find(|shown| shown.id == calendar) {
        let rights = if download.free_busy {
            Rights::FreeBusy
        } else if shown.rights == Rights::FreeBusy {
            Rights::Read
        } else {
            shown.rights
        };
        if shown.rights != rights {
            shown.rights = rights;
            merged.settled = true;
        }
    }
    // What the mailbox has from this account's server, by what the server calls it: a name is
    // one event's, whichever calendar it sits in here (one that strayed is put back in its own).
    // Two of the same (only a file from elsewhere could hold that) leave one.
    let mut known: HashMap<String, Id> = HashMap::new();
    let mut extra: Vec<Id> = Vec::new();
    for event in mailbox.events.values().filter(|event| event.origin.account == account && !event.origin.id.is_empty()) {
        extra.extend(known.insert(event.origin.id.clone(), event.id));
    }
    // What was changed or deleted here and not sent yet, by what the server calls the event; what
    // was made here and not sent yet (or sent, with the answer still to come), by its mark; and
    // the occurrences of series that were changed or deleted here and wait, by their series.
    let mut waiting: HashMap<String, EventChangeKind> = HashMap::new();
    let mut made: HashMap<String, Id> = HashMap::new();
    let mut waiting_days: HashMap<String, Vec<Timestamp>> = HashMap::new();
    // (An appointment on its way here from another account's calendar leaves the original on
    // that server until the copy is seen: the download of that one leaves it out too.)
    for change in mailbox.event_changes.iter().filter(|change| change.account != account && !change.replaces.is_empty()) {
        if change.base.as_deref().is_some_and(|base| base.origin.account == account) {
            waiting.insert(change.replaces.clone(), EventChangeKind::Delete);
        }
    }
    for change in mailbox.event_changes.iter().filter(|change| change.account == account) {
        // An appointment on its way into another calendar: its original, which the server still
        // has, is as good as deleted here. And a meeting that is being forwarded is as it is on
        // the server: a download goes on telling what it is.
        if !change.replaces.is_empty() {
            waiting.insert(change.replaces.clone(), EventChangeKind::Delete);
        }
        if change.kind == EventChangeKind::Forward {
            continue;
        }
        match change.kind {
            EventChangeKind::Create if !change.token.is_empty() => {
                made.insert(change.token.clone(), change.event);
            }
            EventChangeKind::Create => {}
            _ if change.of_occurrence() => waiting_days.entry(change.series.clone()).or_default().push(change.occurrence),
            // A meeting that was declined is gone here, as one that was deleted is.
            EventChangeKind::Respond if change.response == Response::Declined => {
                waiting.insert(change.remote.clone(), EventChangeKind::Delete);
            }
            kind => {
                waiting.insert(change.remote.clone(), kind);
            }
        }
    }
    // Everything from outside is cut to size first, and the series are told from the rest.
    let mut seen: HashSet<String> = HashSet::new();
    let (mut series, mut rest): (Vec<RemoteEvent>, Vec<RemoteEvent>) = (Vec::new(), Vec::new());
    for remote in download.events.iter().take(MAX_EVENTS) {
        let mut remote = remote.clone();
        remote.tidy();
        if remote.id.is_empty() || !seen.insert(remote.id.clone()) {
            merged.skipped += 1;
        } else if remote.kind == RemoteEventKind::SeriesMaster {
            series.push(remote);
        } else {
            rest.push(remote);
        }
    }
    // What was sent to the server and is yet to be seen there: is it in this download as it was
    // sent? What isn't yet stays as it is here for now, as what waits to be sent does.
    let before_checks = mailbox.event_checks.clone();
    let checked = check_sent(mailbox, account, calendar, download, &series, &rest, &waiting, &email, offset_at, &mut merged);
    waiting.extend(checked.held);
    for (name, days) in checked.held_days {
        waiting_days.entry(name).or_default().extend(days);
    }
    // Each series as it is here from now on (`shapes`): one event that repeats, which its
    // occurrences are told from. A series whose rule can't be said here is passed over, and its
    // occurrences are events of their own. `fixed` are the series that are left as they are
    // here: changed here with the server yet to hear, or not in the download themselves (they
    // couldn't be asked for just now) though occurrences of theirs are.
    let kept = |mailbox: &Mailbox, known: &HashMap<String, Id>, name: &str| {
        known.get(name).and_then(|id| mailbox.event(*id)).filter(|event| event.recurrence.is_some()).cloned()
    };
    let mut shapes: HashMap<String, Event> = HashMap::new();
    let mut fixed: HashSet<String> = HashSet::new();
    let mut ruleless: HashSet<String> = HashSet::new();
    for master in &series {
        if master.recurrence.is_none() || shapes.len() >= MAX_SERIES {
            ruleless.insert(master.id.clone());
            merged.skipped += 1;
            continue;
        }
        match waiting.get(&master.id) {
            // Deleted here, and the server is yet to hear: it stays deleted.
            Some(EventChangeKind::Delete) => merged.skipped += 1,
            // Changed here, and the server is yet to hear: what the download has is the older.
            Some(_) if known.contains_key(&master.id) => {
                if let Some(here) = kept(mailbox, &known, &master.id) {
                    shapes.insert(master.id.clone(), here);
                    fixed.insert(master.id.clone());
                }
                merged.unchanged += 1;
            }
            _ => {
                let mut fresh = series_from_remote(master, &email, offset_at);
                fresh.calendar = calendar;
                fresh.origin.account = account;
                shapes.insert(master.id.clone(), fresh);
            }
        }
    }
    for remote in rest.iter().filter(|remote| !remote.series.is_empty() && remote.kind != RemoteEventKind::Single) {
        let name = &remote.series;
        let unknown = !shapes.contains_key(name) && !ruleless.contains(name) && waiting.get(name) != Some(&EventChangeKind::Delete);
        if unknown
            && shapes.len() < MAX_SERIES
            && let Some(here) = kept(mailbox, &known, name)
        {
            shapes.insert(name.clone(), here);
            fixed.insert(name.clone());
        }
    }
    // The days each of them happens on, around the stretch (no further than a few years of it).
    let day_of = |instant: Timestamp| DateTime::from_timestamp(instant, 0).date;
    let first_day = day_of(download.from).add_days(-REACH_DAYS);
    let last_day = day_of(download.to).add_days(2).min(first_day.add_days(MAX_STRETCH_DAYS));
    let days: HashMap<String, HashSet<Date>> = shapes
        .iter()
        .map(|(name, shape)| {
            let on = shape.series_days(first_day, last_day, offset_at);
            (name.clone(), on.into_iter().collect())
        })
        .collect();
    // For each series: the days on which the server has the series' own occurrence, and the
    // days whose occurrence is an event of its own.
    let mut own: HashMap<String, HashSet<Date>> = HashMap::new();
    let mut loose: HashMap<String, HashSet<Date>> = HashMap::new();
    for remote in &rest {
        // One occurrence of a series that is one event here: either it is the series' own, and
        // nothing is kept of it, or it is an event of its own that takes the place of a day.
        let mut stands_for = None;
        if !remote.series.is_empty() && remote.kind != RemoteEventKind::Single {
            if waiting.get(&remote.series) == Some(&EventChangeKind::Delete) {
                merged.skipped += 1;
                continue;
            }
            if let (Some(shape), Some(on)) = (shapes.get(&remote.series), days.get(&remote.series)) {
                let own_day = covered(shape, on, remote, offset_at);
                // (One that was changed here as an event of its own, with the server yet to
                // hear, stays that event.)
                let changed_here = waiting.contains_key(&remote.id) && known.contains_key(&remote.id);
                if let Some(day) = own_day.filter(|_| !changed_here) {
                    own.entry(remote.series.clone()).or_default().insert(day);
                    continue;
                }
                let was_at = remote.original.unwrap_or(remote.start);
                let day = own_day.unwrap_or_else(|| match shape.all_day {
                    true => whole_day(was_at, &remote.start_zone, offset_at),
                    false => series_day_at(shape, was_at, offset_at),
                });
                loose.entry(remote.series.clone()).or_default().insert(day);
                stands_for = Some(day);
            }
        }
        match waiting.get(&remote.id) {
            // Deleted here, and the server is yet to hear: it stays deleted.
            Some(EventChangeKind::Delete) => {
                merged.skipped += 1;
                continue;
            }
            // Changed here, and the server is yet to hear: what the download has is the older.
            Some(_) if known.contains_key(&remote.id) => {
                if let Some(Ok(event)) = known.get(&remote.id).map(|id| mailbox.event_mut(*id)).filter(|_| stands_for.is_some()) {
                    event.origin.occurrence = stands_for;
                }
                merged.unchanged += 1;
                continue;
            }
            _ => {}
        }
        let mut fresh = event_from_remote(remote, &email, offset_at);
        fresh.calendar = calendar;
        fresh.origin.account = account;
        fresh.origin.occurrence = stands_for;
        match known.get(&remote.id).and_then(|id| mailbox.event(*id)).cloned() {
            Some(old) => {
                fresh.id = old.id;
                if !download.has_options {
                    keep_options(&old, &mut fresh);
                }
                replace(mailbox, account, &old, fresh, download.has_categories, &mut merged);
            }
            None => {
                // One that was made here: the server has it, though its answer never came (or is
                // still on its way). It is that event, not a second one.
                let mine = made.get(&remote.token).copied().filter(|_| !remote.token.is_empty() && remote.series.is_empty());
                let mine = mine.filter(|id| mailbox.event(*id).is_some_and(|event| event.origin.account == account && event.origin.id.is_empty()));
                if let Some(id) = mine {
                    adopt(mailbox, account, id, fresh, &remote.token, offset_at);
                    known.insert(remote.id.clone(), id);
                    merged.updated += 1;
                } else if mailbox.add_event(fresh).is_ok() {
                    merged.added += 1;
                }
            }
        }
    }
    // The series themselves, with the days they skip: in the order they came, then the others.
    let mut order: Vec<String> = series.iter().map(|master| master.id.clone()).filter(|name| shapes.contains_key(name)).collect();
    let mut others: Vec<String> = shapes.keys().filter(|name| !order.contains(name)).cloned().collect();
    others.sort();
    order.extend(others);
    let no_days = HashSet::new();
    for name in &order {
        let Some(mut fresh) = shapes.get(name).cloned() else { continue };
        let old = known.get(name).and_then(|id| mailbox.event(*id)).cloned();
        if fixed.contains(name) {
            // As it is here; only the days of occurrences that are events of their own go.
            let Some(old) = old else { continue };
            let mut skipped: BTreeSet<Date> = old.exceptions.iter().copied().collect();
            skipped.extend(loose.get(name).into_iter().flatten().copied());
            let skipped: Vec<Date> = skipped.into_iter().take(MAX_SKIPPED).collect();
            if skipped != old.exceptions
                && let Ok(event) = mailbox.event_mut(old.id)
            {
                event.exceptions = skipped;
                merged.updated += 1;
            }
            continue;
        }
        let before = old.as_ref().map(|old| old.exceptions.clone()).unwrap_or_default();
        let on = days.get(name).unwrap_or(&no_days);
        fresh.exceptions = skipped_days(&fresh, &before, download, on, own.get(name), loose.get(name), waiting_days.get(name), offset_at);
        match old {
            Some(old) => {
                fresh.id = old.id;
                if !download.has_options {
                    keep_options(&old, &mut fresh);
                }
                replace(mailbox, account, &old, fresh, download.has_categories, &mut merged);
            }
            None => {
                // A series that was made here, and is on the server before its answer came.
                let token = series.iter().find(|master| master.id == *name).map(|master| master.token.clone()).unwrap_or_default();
                let mine = made.get(&token).copied().filter(|_| !token.is_empty());
                let mine = mine.filter(|id| mailbox.event(*id).is_some_and(|event| event.origin.account == account && event.origin.id.is_empty()));
                if let Some(id) = mine {
                    adopt(mailbox, account, id, fresh, &token, offset_at);
                    known.insert(name.clone(), id);
                    merged.updated += 1;
                } else if let Ok(id) = mailbox.add_event(fresh) {
                    known.insert(name.clone(), id);
                    merged.added += 1;
                }
            }
        }
    }
    // What was kept of a series' occurrences when a series came as nothing but them: the series
    // has them now. (An occurrence that is an event of its own says which day it stands for.)
    let absorbed = |event: &Event| {
        event.origin.account == account
            && event.origin.occurrence.is_none()
            && event.recurrence.is_none()
            && !event.origin.series.is_empty()
            && shapes.contains_key(&event.origin.series)
            && !waiting.contains_key(&event.origin.id)
    };
    let old_way: Vec<Id> = mailbox.events.values().filter(|event| absorbed(event)).map(|event| event.id).collect();
    // A series the server says it no longer has goes, with what was kept of its occurrences;
    // one whose rule can't be said here any more goes as a series, and its occurrences are
    // events of their own again.
    let mut going: Vec<Id> = old_way;
    for name in download.gone.iter().take(MAX_OUTCOMES).map(|name| clean_key(name)).filter(|name| !name.is_empty()) {
        if !waiting.contains_key(&name) && !shapes.contains_key(&name) && !ruleless.contains(&name) {
            let of_it = |event: &&std::sync::Arc<Event>| event.origin.account == account && (event.origin.id == name || event.origin.series == name);
            going.extend(mailbox.events.values().filter(of_it).map(|event| event.id));
        }
    }
    for name in ruleless.iter().filter(|name| !waiting.contains_key(*name)) {
        going.extend(kept(mailbox, &known, name).map(|series| series.id));
    }
    for id in going {
        if mailbox.remove_event(id).is_ok() {
            merged.removed += 1;
        }
    }
    // What the server no longer has: of this calendar, in the stretch that was asked about. A
    // download that isn't all there is says nothing about that. (A series is not in a stretch as
    // its occurrences are: it goes when the server says it is gone.)
    if download.complete && download.events.len() <= MAX_EVENTS && download.from < download.to {
        let within = |event: &Event| event.calendar == calendar && (download.from..download.to).contains(&server_start(event, offset_at));
        // (Not one that a change waits for, or that was just sent and isn't seen there yet: it
        // may be elsewhere on the server than it is here, outside what was asked for. The
        // server says so itself when it is told the change, should it no longer have it.)
        let gone: Vec<Id> = known
            .iter()
            .filter(|(remote, _)| !seen.contains(*remote) && !waiting.contains_key(*remote))
            .filter_map(|(_, id)| mailbox.event(*id))
            .filter(|event| within(event) && event.recurrence.is_none())
            .map(|event| event.id)
            .chain(extra)
            .collect();
        for id in gone {
            if mailbox.remove_event(id).is_ok() {
                merged.removed += 1;
            }
        }
    }
    // A change that can't be sent any more is given up, and says so.
    for dropped in tidy_changes(mailbox) {
        if !dropped.message.is_empty() && !merged.messages.contains(&dropped.message) {
            merged.say(dropped.message.clone());
        }
        merged.dropped.push(dropped);
    }
    merged.unverified = mailbox.event_checks.iter().filter(|check| check.account == account).count();
    merged.checked = mailbox.event_checks != before_checks;
    // And beside the server's calendar, the mailbox's own says where it lives, or goes.
    let listed: Vec<(Id, String)> = mailbox.calendars.iter().map(|calendar| (calendar.id, calendar.name.clone())).collect();
    settle_calendars(mailbox);
    merged.settled |= mailbox.calendars.len() != listed.len() || mailbox.calendars.iter().zip(&listed).any(|(now, was)| now.name != was.1);
    Ok(merged)
}

/// Most series one download is taken to hold as series (more come as their occurrences), and
/// how many days from its start a stretch is looked at for the days a series skips.
const MAX_SERIES: usize = 500;
const MAX_STRETCH_DAYS: i64 = 1_200;

/// Put the server's copy `fresh` of an event in the place of `old`, when they differ.
///
/// Its categories are the server's, when the download says them. One thing is kept: categories
/// given here while they were kept here only, on an event the server hasn't changed since (its
/// mark of the version is the same, and it has none): the server never heard of them. They stay,
/// and are noted to go up.
fn replace(mailbox: &mut Mailbox, account: Id, old: &Event, mut fresh: Event, has_categories: bool, merged: &mut Merged) {
    if !has_categories {
        fresh.categories = old.categories.clone();
    } else if fresh.categories.is_empty() && !old.categories.is_empty() && old.origin.change_key == fresh.origin.change_key {
        let base = fresh.clone();
        fresh.categories = old.categories.clone();
        // (Not by itself for a meeting the owner called: nothing is written to the server about
        // one of those that the owner didn't just do. The category stays here.)
        let own_meeting = !fresh.origin.guest && !fresh.attendees.is_empty();
        if !own_meeting && mailbox.event_changes.len() < MAX_EVENT_CHANGES {
            mailbox.event_changes.push(EventChange {
                account,
                kind: EventChangeKind::Update,
                event: fresh.id,
                remote: fresh.origin.id.clone(),
                base: Some(Box::new(base)),
                ..Default::default()
            });
            merged.settled = true;
        }
    }
    if *old == fresh {
        merged.unchanged += 1;
    } else if let Ok(event) = mailbox.event_mut(fresh.id) {
        *event = fresh;
        merged.updated += 1;
    }
}

/// An event made here turns out to be on the server already (`fresh` is the server's copy, with
/// the mark the event was sent with): the event here is that one from now on. It stays as it is
/// here; what differs from the server's copy is a change still to be sent.
fn adopt(mailbox: &mut Mailbox, account: Id, id: Id, mut fresh: Event, token: &str, offset_at: &dyn Fn(Timestamp) -> i32) {
    let Some(mut here) = mailbox.event(id).cloned() else { return };
    fresh.id = id;
    fresh.calendar = here.calendar;
    fresh.categories = here.categories.clone();
    here.origin = fresh.origin.clone();
    here.uid = fresh.uid.clone();
    here.organizer = fresh.organizer.clone();
    // (An appointment that was moved here from another calendar: the original is still to go.)
    let making = |change: &EventChange| change.kind == EventChangeKind::Create && change.account == account && change.token == token;
    let moved = mailbox.event_changes.iter().position(|change| making(change) && !change.replaces.is_empty());
    if let Some(noted) = moved.map(|at| mailbox.event_changes.remove(at)) {
        finish_move(mailbox, account, id, noted, &here.origin.id);
    }
    mailbox.event_changes.retain(|change| !(change.kind == EventChangeKind::Create && change.account == account && change.token == token));
    if !changed_fields(&fresh, &here, offset_at).is_empty() {
        let remote = here.origin.id.clone();
        mailbox.event_changes.push(EventChange {
            account,
            kind: EventChangeKind::Update,
            event: id,
            remote,
            base: Some(Box::new(fresh)),
            ..Default::default()
        });
    }
    if let Ok(event) = mailbox.event_mut(id) {
        *event = here;
    }
    skip_days_there(mailbox, account, id, offset_at);
}

/// Merge the contacts a server has into the mailbox.
///
/// - A contact the mailbox doesn't have (by what the server calls it) is added; one it has is
///   updated when anything about it differs, keeping its id here, its categories and whether it
///   is a favourite.
/// - A contact that came from this account's server earlier and is not in the download is
///   removed, when the download is [complete](ContactsDownload::complete).
/// - Contacts made here, and those of other accounts, are not looked at. Doing it twice changes
///   nothing the second time.
///
/// Nothing is changed when this returns an error.
pub fn merge_contacts(mailbox: &mut Mailbox, download: &ContactsDownload) -> Result<Merged, CmdError> {
    let account = owner(mailbox, download.account, &download.email)?.id;
    let mut merged = Merged::default();
    let mut known: HashMap<String, Id> = HashMap::new();
    let mut extra: Vec<Id> = Vec::new();
    for contact in mailbox.contacts.values().filter(|contact| contact.origin.account == account && !contact.origin.id.is_empty()) {
        extra.extend(known.insert(contact.origin.id.clone(), contact.id));
    }
    let mut seen: HashSet<String> = HashSet::new();
    for remote in download.contacts.iter().take(MAX_CONTACTS) {
        let mut remote = remote.clone();
        remote.tidy();
        let mut fresh = contact_from_remote(&remote);
        // Somebody with no name, company or address would be an empty line in the list.
        if remote.id.is_empty() || fresh.name().is_empty() || !seen.insert(remote.id.clone()) {
            merged.skipped += 1;
            continue;
        }
        fresh.origin.account = account;
        match known.get(&remote.id).and_then(|id| mailbox.contact(*id)) {
            Some(old) => {
                fresh.id = old.id;
                // What is only kept here stays as it is here.
                fresh.categories = old.categories.clone();
                fresh.favorite = old.favorite;
                fresh.uid = old.uid.clone();
                if *old == fresh {
                    merged.unchanged += 1;
                } else if let Ok(contact) = mailbox.contact_mut(fresh.id) {
                    *contact = fresh;
                    merged.updated += 1;
                }
            }
            None => {
                mailbox.add_contact(fresh);
                merged.added += 1;
            }
        }
    }
    if download.complete && download.contacts.len() <= MAX_CONTACTS {
        let gone: Vec<Id> = known.iter().filter(|(remote, _)| !seen.contains(*remote)).map(|(_, id)| *id).chain(extra).collect();
        for id in gone {
            if mailbox.remove_contact(id).is_ok() {
                merged.removed += 1;
            }
        }
    }
    Ok(merged)
}

/// The local zone's offset at an instant, as the session's clock has it.
pub(crate) fn offset_of(s: &Session) -> impl Fn(Timestamp) -> i32 + use<> {
    // The zone the calendar is shown in, which is the one the mailbox's times are by.
    let (fixed, zone) = (s.clock.offset, s.zone());
    move |instant| fixed.unwrap_or_else(|| zone.offset_at(instant))
}

/// Take in the events a host downloaded ([`merge_events`]), with times in the session's zone.
/// Like mail that arrives, this is not an undo step: the server is the source of truth for
/// what came from it. A download that changes nothing leaves the mailbox as it is, unsaved
/// changes or none.
pub fn apply_events(s: &mut Session, download: &EventsDownload) -> CmdResult {
    Ok(take_in_events(s, download)?.json(download.complete))
}

/// [`apply_events`], with what the merge came to as it is ([`Merged`]): for a host that tells
/// the owner what was put back, and logs it.
pub fn take_in_events(s: &mut Session, download: &EventsDownload) -> Result<Merged, CmdError> {
    let (offset_at, now) = (offset_of(s), s.now());
    // A change that no download has been able to say anything about for a day is not waited
    // for any longer (its event is outside everything that is asked for).
    // (And no more of them than are ever noted: a file from elsewhere may hold any number.)
    s.mailbox.event_checks.truncate(MAX_EVENT_CHECKS);
    let noted = s.mailbox.event_checks.len();
    s.mailbox.event_checks.retain(|check| check.made != 0 && now.saturating_sub(check.made) <= MAX_CHECK_AGE);
    let expired = noted - s.mailbox.event_checks.len();
    if expired > 0 {
        log::debug!("calendar: {expired} sent changes were not seen in any download for a day");
    }
    let merged = merge_events(&mut s.mailbox, download, &offset_at)?;
    if merged.changed() || expired > 0 {
        // Not through `run` when the host calls this, so say it here: what is shown is out of date.
        s.mailbox_changed();
    }
    Ok(merged)
}

/// Take in the contacts a host downloaded ([`merge_contacts`]). Not an undo step, as [`apply_events`].
pub fn apply_contacts(s: &mut Session, download: &ContactsDownload) -> CmdResult {
    let merged = merge_contacts(&mut s.mailbox, download)?;
    if merged.changed() {
        s.mailbox_changed();
    }
    Ok(merged.json(download.complete))
}

/// Remove everything that came from the server of `account`: its events, its contacts and the
/// calendars that follow it. For an account that is no longer to be followed, or is being
/// removed. What was made here stays, an event made in one of its calendars that the server
/// wasn't given yet too, and the mailbox keeps a calendar to put events in.
pub fn forget(s: &mut Session, account: Id) -> CmdResult {
    if account == 0 {
        return Err(bad("`account` (id) is required"));
    }
    let mailbox = &mut s.mailbox;
    // An event made here that the server has no id for yet ([`note_created`]) is the owner's,
    // not the server's: it stays, as the mailbox's own.
    let (made_here, events): (Vec<&Event>, Vec<&Event>) = mailbox
        .events
        .values()
        .map(|event| event.as_ref())
        .filter(|event| event.origin.account == account)
        .partition(|event| event.origin.id.is_empty());
    let (made_here, events): (Vec<Id>, Vec<Id>) = (made_here.iter().map(|event| event.id).collect(), events.iter().map(|event| event.id).collect());
    let contacts: Vec<Id> = mailbox.contacts.values().filter(|contact| contact.origin.account == account).map(|contact| contact.id).collect();
    for id in &events {
        let _ = mailbox.remove_event(*id);
    }
    for id in &made_here {
        if let Ok(event) = mailbox.event_mut(*id) {
            event.origin = Origin::default();
        }
    }
    for id in &contacts {
        let _ = mailbox.remove_contact(*id);
    }
    // And its to-dos (the server isn't told: it is no longer followed), with the deletions that
    // waited. A to-do the server has no id for yet was made here, as an event above: it stays.
    let (tasks_made_here, tasks): (Vec<(Id, bool)>, Vec<(Id, bool)>) = mailbox
        .tasks
        .values()
        .filter(|task| task.origin.account == account)
        .map(|task| (task.id, task.origin.id.is_empty()))
        .partition(|(_, made_here)| *made_here);
    let (tasks_made_here, tasks): (Vec<Id>, Vec<Id>) =
        (tasks_made_here.into_iter().map(|(id, _)| id).collect(), tasks.into_iter().map(|(id, _)| id).collect());
    for id in &tasks {
        let _ = mailbox.remove_task_quietly(*id);
    }
    for id in &tasks_made_here {
        if let Some(task) = mailbox.tasks.get_mut(id) {
            std::sync::Arc::make_mut(task).origin = Origin::default();
        }
    }
    let waiting = mailbox.task_gone.len();
    mailbox.task_gone.retain(|gone| gone.account != account);
    let tasks_gone = tasks.len() + (waiting - mailbox.task_gone.len());
    // What was waiting to be sent to that server has nowhere to go.
    let unsent = mailbox.event_changes.len();
    mailbox.event_changes.retain(|change| change.account != account);
    let unsent = unsent - mailbox.event_changes.len();
    mailbox.event_checks.retain(|check| check.account != account);
    let followed = |calendar: &Calendar| calendar.origin.account == account;
    let calendars = mailbox.calendars.iter().filter(|calendar| followed(calendar)).count();
    // What is left in its calendars now was made here, and needs one of the mailbox's own to go
    // to: not another account's, whose server would never be sent it.
    let left_in = |event: &std::sync::Arc<Event>| mailbox.calendars.iter().any(|calendar| calendar.id == event.calendar && followed(calendar));
    let homeless = mailbox.events.values().any(left_in);
    let own_stays = mailbox.calendars.iter().any(|calendar| !followed(calendar) && calendar.origin.is_local());
    if calendars > 0 && (calendars == mailbox.calendars.len() || (homeless && !own_stays)) {
        // No calendar of the mailbox's own would be left (for what was made here, or at all):
        // one becomes that, the account's main one when it is here (not a Birthdays it lists
        // unticked), shown, in no group, and the owner's to change. Its colour, and the one
        // chosen for it here, stay.
        let main = mailbox.calendars.iter().position(|calendar| followed(calendar) && calendar.origin.id.is_empty());
        if let Some(kept) = main.or_else(|| mailbox.calendars.iter().position(followed)).and_then(|main| mailbox.calendars.get_mut(main)) {
            *kept =
                Calendar { id: kept.id, name: "Calendar".into(), color: kept.color, visible: true, own_color: kept.own_color, ..Default::default() };
        }
    }
    let gone: Vec<Id> = mailbox.calendars.iter().filter(|calendar| followed(calendar)).map(|calendar| calendar.id).collect();
    // Events made here that sat in a calendar that goes move to one of the mailbox's own.
    let home = mailbox.calendars.iter().find(|calendar| !followed(calendar) && calendar.origin.is_local());
    if let Some(home) = home.map(|calendar| calendar.id) {
        for event in mailbox.events.values_mut().filter(|event| gone.contains(&event.calendar)) {
            std::sync::Arc::make_mut(event).calendar = home;
        }
    }
    mailbox.calendars.retain(|calendar| !followed(calendar));
    if !events.is_empty()
        || !made_here.is_empty()
        || !contacts.is_empty()
        || calendars > 0
        || unsent > 0
        || tasks_gone > 0
        || !tasks_made_here.is_empty()
    {
        s.mailbox_changed();
    }
    Ok(json!({"events": events.len(), "contacts": contacts.len(), "calendars": gone.len()}))
}

/// Take in the list of an account's calendars ([`CalendarsDownload`]): each is a calendar here,
/// under the name, in the group and with the colour the server has for it, and says whether
/// the account may change it. The account's main calendar is the one events came into before
/// there was a list. A calendar the server no longer lists goes, with the copies of its events,
/// when the list is complete. Whether each is shown is kept here, and stays: a calendar that
/// arrives is shown when it is the account's main one and listed unticked otherwise (birthdays,
/// holidays and the rest wait for the owner to tick them), and one that is here already keeps
/// whatever the owner set, whatever a later list says. Not an undo step.
pub fn apply_calendars(s: &mut Session, download: &CalendarsDownload) -> CmdResult {
    let account = owner(&s.mailbox, download.account, &download.email)?.id;
    let mut download = download.clone();
    download.tidy();
    // Calendars opened by address (#70) come from the host, never from a server's list, and
    // only for an address the owner opened under this account.
    let opened: Vec<mailcraft_model::OpenedCalendar> = s.mailbox.account(account).map(|a| a.opened_calendars.clone()).unwrap_or_default();
    let is_opened = |id: &str| mailcraft_model::remote::opened_address(id).is_some_and(|a| opened.iter().any(|o| o.address.eq_ignore_ascii_case(&a)));
    download.calendars.retain(|calendar| !calendar.id.starts_with(mailcraft_model::remote::OPENED_PREFIX) || is_opened(&calendar.id));
    // The copy of a shared mailbox's calendar that the owner hid (#70: the server lists it too,
    // under Shared Calendars) is neither kept nor shown; the shared mailbox has its own.
    if let Some(owner) = s.mailbox.account(account).filter(|a| !a.is_shared()) {
        download.calendars.retain(|calendar| calendar.main || is_opened(&calendar.id) || !owner.hides_listed(calendar.owner.as_ref()));
    }
    // A list that doesn't say which calendar is the main one can't be told from the calendar
    // that is here already (its events would come a second time, under another calendar's
    // name): nothing is made of it.
    if !download.calendars.iter().any(|calendar| calendar.main) {
        return Ok(json!({"added": 0, "updated": 0, "removed": 0, "complete": false}));
    }
    let mailbox = &mut s.mailbox;
    let (mut added, mut updated) = (0usize, 0usize);
    let mut listed: HashSet<String> = HashSet::new();
    for remote in &download.calendars {
        // The main one has no name of its own here: it is "the account's calendar".
        let named = if remote.main { String::new() } else { remote.id.clone() };
        let title = if remote.name.is_empty() { SERVER_CALENDAR.to_string() } else { remote.name.clone() };
        listed.insert(named.clone());
        let at = mailbox.calendars.iter().position(|calendar| calendar.origin.account == account && calendar.origin.id == named);
        if let Some(calendar) = at.and_then(|at| mailbox.calendars.get_mut(at)) {
            let color = remote.color.unwrap_or(calendar.color);
            let same = calendar.name == title
                && calendar.color == color
                && calendar.group == remote.group
                && calendar.rights == remote.rights
                && calendar.owner == remote.owner;
            if !same {
                (calendar.name, calendar.color, calendar.group) = (title, color, remote.group.clone());
                (calendar.rights, calendar.owner) = (remote.rights, remote.owner.clone());
                updated += 1;
            }
            // What its meetings may be held online with, when the list says (else: as it was).
            if remote.online_providers.is_some() && calendar.online_providers != remote.online_providers {
                calendar.online_providers = remote.online_providers.clone();
                updated += usize::from(same);
            }
        } else if mailbox.calendars.len() < MAX_CALENDARS {
            // (One opened by address has the colour and the tick the owner's list gave it.)
            let own = opened.iter().find(|o| mailcraft_model::remote::opened_address(&remote.id).is_some_and(|a| o.address.eq_ignore_ascii_case(&a)));
            let color = own.map(|o| o.color).or(remote.color).unwrap_or_else(|| super::calendar::next_color(mailbox));
            let id = mailbox.alloc_id();
            let origin = Origin { account, id: named, ..Default::default() };
            mailbox.calendars.push(Calendar {
                id,
                name: title,
                color,
                // Only the account's own calendar is ticked to begin with.
                visible: remote.main || own.is_some_and(|o| o.ticked),
                origin,
                rights: remote.rights,
                owner: remote.owner.clone(),
                group: remote.group.clone(),
                online_providers: remote.online_providers.clone(),
                ..Default::default()
            });
            added += 1;
        }
    }
    // What the server no longer lists: never the main one, and only when the list is all of it.
    let gone: Vec<Id> = if download.complete {
        // (One opened by address goes only when the owner removes it: a list that hasn't it, for
        // a calendar that couldn't be read just now, takes nothing away.)
        let followed = |calendar: &&Calendar| {
            calendar.origin.account == account
                && !calendar.origin.id.is_empty()
                && !listed.contains(&calendar.origin.id)
                && !is_opened(&calendar.origin.id)
        };
        mailbox.calendars.iter().filter(followed).map(|calendar| calendar.id).collect()
    } else {
        Vec::new()
    };
    let mut dropped: Vec<Dropped> = Vec::new();
    if !gone.is_empty() {
        let events: Vec<Id> = mailbox.events.values().filter(|event| gone.contains(&event.calendar)).map(|event| event.id).collect();
        for id in events {
            let _ = mailbox.remove_event(id);
        }
        mailbox.calendars.retain(|calendar| !gone.contains(&calendar.id));
        dropped = tidy_changes(mailbox);
    }
    settle_calendars(mailbox);
    let before = s.mailbox.account(account).map(|a| a.opened_calendars.clone());
    let about_opened = super::calendar_opened::after_list(s, account, &download.refused);
    let changed_list = s.mailbox.account(account).map(|a| a.opened_calendars.clone()) != before;
    if added + updated + gone.len() > 0 || changed_list {
        s.mailbox_changed();
    }
    Ok(json!({
        "added": added, "updated": updated, "removed": gone.len(), "complete": download.complete, "dropped": dropped_json(&dropped),
        "refused": about_opened["refused"], "duplicates": about_opened["duplicates"], "listedTwice": about_opened["listedTwice"],
    }))
}

/// Remove the calendars `ids` and their events from the mailbox, and the changes of them that
/// still waited ([`tidy_changes`]). How many events went. (Calendars opened by address that the
/// owner removes, #70.)
pub(crate) fn forget_calendars(mailbox: &mut Mailbox, ids: &[Id]) -> usize {
    let events: Vec<Id> = mailbox.events.values().filter(|event| ids.contains(&event.calendar)).map(|event| event.id).collect();
    for id in &events {
        let _ = mailbox.remove_event(*id);
    }
    mailbox.calendars.retain(|calendar| !ids.contains(&calendar.id));
    tidy_changes(mailbox);
    events.len()
}

/// Take in the mailbox's own categories as an account's server has them
/// ([`CategoriesDownload`]): each is a category here, in the server's colour. Categories that
/// are only here stay. Not an undo step.
pub fn apply_categories(s: &mut Session, download: &CategoriesDownload) -> CmdResult {
    owner(&s.mailbox, download.account, &download.email)?;
    let mut download = download.clone();
    download.tidy();
    let mut changed = 0usize;
    for category in &download.categories {
        let have = s.mailbox.category(&category.name).map(|have| (have.name == category.name, have.color));
        if have != Some((true, category.color)) && s.mailbox.categories.len() < 2 * mailcraft_model::remote::MAX_REMOTE_CATEGORIES {
            // Under the server's spelling of the name, wherever it is used.
            if have.is_some_and(|(same, _)| !same) {
                let old = s.mailbox.category(&category.name).map(|have| have.name.clone()).unwrap_or_default();
                let _ = s.mailbox.rename_category(&old, &category.name);
            }
            if s.mailbox.set_category(&category.name, category.color).is_ok() {
                changed += 1;
            }
        }
    }
    // Outlook's list, by name, for its colours to win here (R3 spec S18).
    if s.mailbox.account(download.account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Microsoft) {
        let names: Vec<String> = download.categories.iter().map(|c| c.name.clone()).collect();
        if s.mailbox.outlook_categories != names {
            s.mailbox.outlook_categories = names;
            changed += 1;
        }
    }
    if changed > 0 {
        s.mailbox_changed();
    }
    Ok(json!({"changed": changed, "categories": s.mailbox.categories.len()}))
}

/// The folder of `account` that a favourite of its server's list is: the special folder it says
/// it is; else the one at its path, name by name from the top whatever their case, starting at
/// the special folder its first name is when it lies under one. `None` when the mailbox has no
/// such folder (yet).
fn folder_of_favorite(mailbox: &Mailbox, account: Id, favorite: &RemoteFavorite) -> Option<Id> {
    if favorite.kind != FolderKind::Custom {
        return mailbox.folder_of_kind(account, favorite.kind);
    }
    let same = |have: &str, want: &str| have.trim().to_lowercase() == want.trim().to_lowercase();
    let child = |parent: Option<Id>, name: &str| {
        mailbox.folders.iter().find(|f| f.account == account && f.parent == parent && same(&f.name, name)).map(|f| f.id)
    };
    let mut names = favorite.path.iter().take(MAX_FAVORITE_DEPTH);
    let first = names.next()?;
    let mut at = match favorite.root {
        FolderKind::Custom => child(None, first)?,
        root => mailbox.folder_of_kind(account, root)?,
    };
    for name in names {
        at = child(Some(at), name)?;
    }
    Some(at)
}

/// Take in an account's Favorites as its server has them ([`FavoritesDownload`]: the folders
/// Outlook shows under Favorites). A list that was read is the account's from now on, in the
/// server's order: each of its folders is matched to the mailbox's own by what it is, then by
/// where it is, and one the mailbox doesn't have is left out (and counted). When the server has
/// no list, or it may not be read, what was kept of one goes, and the favourites marked here
/// are what the pane shows; when it couldn't be read this time, nothing changes. The marks made
/// here ([`mailcraft_model::Folder::favorite`]) are never touched. Not an undo step.
pub fn apply_favorites(s: &mut Session, download: &FavoritesDownload) -> CmdResult {
    let account = owner(&s.mailbox, download.account, &download.email)?.id;
    let mut download = download.clone();
    download.tidy();
    let mut absent = 0usize;
    let changed = match download.outcome {
        FavoritesOutcome::Read => {
            let mut folders: Vec<Id> = Vec::new();
            for favorite in &download.folders {
                match folder_of_favorite(&s.mailbox, account, favorite) {
                    Some(id) if !folders.contains(&id) => folders.push(id),
                    Some(_) => {}
                    None => absent += 1,
                }
            }
            s.mailbox.set_server_favorites(account, Some(folders))
        }
        FavoritesOutcome::Missing | FavoritesOutcome::NotPermitted => s.mailbox.set_server_favorites(account, None),
        FavoritesOutcome::Failed => false,
    };
    if changed {
        s.mailbox_changed();
    }
    let from_server = super::folder::favorites_from_server(s, account);
    Ok(json!({
        "outcome": download.outcome,
        "from": if from_server { "server" } else { "mailbox" },
        "favorites": s.mailbox.server_favorites(account).unwrap_or_default(),
        "absent": absent,
        "changed": changed,
    }))
}

fn apply_favorites_command(s: &mut Session, v: &Value) -> CmdResult {
    apply_favorites(s, &download_of::<FavoritesDownload>(v)?)
}

/// The series of an account that the mailbox expects to happen between `from` and `to` (Unix
/// seconds) in the server's calendar `calendar` (empty: the main one), by what the server calls
/// them: for a host to ask about by name when a download of the stretch brings nothing of them.
pub fn expected_series(s: &Session, account: Id, calendar: &str, from: Timestamp, to: Timestamp) -> Vec<String> {
    let offset_at = offset_of(s);
    let day = |instant: Timestamp| DateTime::from_timestamp(instant, offset_at(instant)).date;
    let (first, last) = (day(from), day(to));
    let here: Vec<Id> = s.mailbox.calendars.iter().filter(|c| c.origin.account == account && c.origin.id == calendar).map(|c| c.id).collect();
    let expected = |event: &&std::sync::Arc<Event>| {
        event.origin.account == account
            && !event.origin.id.is_empty()
            && event.recurrence.is_some()
            && here.contains(&event.calendar)
            && !event.occurrences_in(first, last, &offset_at).is_empty()
    };
    s.mailbox.events.values().filter(expected).map(|event| event.origin.id.clone()).take(MAX_SERIES).collect()
}

/// The download in a command's parameters.
fn download_of<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, CmdError> {
    let download = v.get("download").cloned().ok_or_else(|| bad("`download` is required"))?;
    // What can't be read is said in a few words: the error may repeat what it choked on.
    serde_json::from_value(download)
        .map_err(|e| bad(format!("`download`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(MAX_SHOWN).collect::<String>())))
}

fn apply_events_command(s: &mut Session, v: &Value) -> CmdResult {
    apply_events(s, &download_of::<EventsDownload>(v)?)
}

fn apply_contacts_command(s: &mut Session, v: &Value) -> CmdResult {
    apply_contacts(s, &download_of::<ContactsDownload>(v)?)
}

fn apply_calendars_command(s: &mut Session, v: &Value) -> CmdResult {
    apply_calendars(s, &download_of::<CalendarsDownload>(v)?)
}

fn apply_categories_command(s: &mut Session, v: &Value) -> CmdResult {
    apply_categories(s, &download_of::<CategoriesDownload>(v)?)
}

fn forget_command(s: &mut Session, v: &Value) -> CmdResult {
    forget(s, p::req_id(v, "account")?)
}

fn event_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::req_id(v, "account")?;
    // What can't be sent any more is given up first, and said: none of it goes without a word.
    let dropped = settle_changes(s);
    let upload = event_upload(s, account)?;
    let mut answer = serde_json::to_value(upload).map_err(|e| fail(e.to_string()))?;
    if let Some(answer) = answer.as_object_mut() {
        answer.insert("dropped".into(), dropped_json(&dropped));
    }
    Ok(answer)
}

fn apply_event_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let result = v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?;
    let uploaded: EventsUploaded = serde_json::from_value(result)
        .map_err(|e| bad(format!("`result`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(MAX_SHOWN).collect::<String>())))?;
    Ok(apply_event_outcomes(s, &uploaded)?.json())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod shared_calendar_tests;

#[cfg(test)]
mod cross_move_tests;
